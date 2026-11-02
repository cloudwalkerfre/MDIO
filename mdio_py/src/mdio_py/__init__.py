import os
import time
import inspect
import posixpath

# 根部 .env → 进程环境(兜底注入,幂等)——必须在任何 ProjectStore
# 构造之前执行(Rust 侧 persist_enabled() 只读标准环境变量)。所有入口(mdio_py.cli/
# mdio_py.gateway)都经本模块导入,故此处生效;Rust 测试/探针不经此层,自行 set env。
from ._env import load_project_env, runtime_root
load_project_env()

from . import mdio_core as mrc

from .core import parse_code
from .core.optimizer import optimize_ir
from .core.interpreter import Interpreter, RuntimeContext
from .core.mem_slot import begin_exec_frame, end_exec_frame
from .core.selector_builder import SelectorBuilder
from .runtime.workspace import Workspace
from .core.error import MdioError, ErrCode

DEBUG = False

class MdioRuntime:
    _instance = None

    def __init__(self, storage_root: str, telemetry: str = "off"):
        if not mrc:
            raise RuntimeError("CRITICAL: Rust Core not loaded.")

        # print(f"🌟 [MdioRuntime] Initializing VFS universe: {storage_root}")
        # 布局名由坐标文件给、boot 显式传给 core（core 不认识 wt/.wal 这些名字）；没有兜底。
        from mdio_py.runtime.worktree_paths import doc_subdir_for, wal_rel_for
        _genesis_project = str(storage_root).replace(";", ",").split(",")[0].split(":")[0].strip() or "/"
        self._workspace = Workspace(storage_root, mrc_module=mrc,
                                    doc_subdir=doc_subdir_for(_genesis_project),
                                    wal_rel=wal_rel_for(_genesis_project))
        self._interpreter = Interpreter(self._workspace)
        self._workspace.runtime = self

        # Telemetry
        self._telemetry_mode = telemetry
        if telemetry in ("auto", "full"):
            try:
                self.store.telemetry_session_create(
                    f"auto_{int(time.time())}", "trace"
                )
            except Exception:
                pass

    @property
    def store(self):
        """暴露给 Server 使用 (Sync/FS API)"""
        return self._workspace.store

    def telemetry(self, name: str, max_events: int | None = None,
                  max_meta_pool: int | None = None,
                  bench_samples_per_phase: int | None = None,
                  max_layer_samples: int | None = None):
        """Return a scope that auto-reports when exited.

        Decorator (recommended)::

            @runtime.telemetry("my_bench")
            async def test(): ...

        Context manager::

            with runtime.telemetry("my_bench"):
                ...
        """
        from .telemetry import _TelemetryScope
        return _TelemetryScope(
            self.store, name,
            max_events=max_events,
            max_meta_pool=max_meta_pool,
            bench_samples_per_phase=bench_samples_per_phase,
            max_layer_samples=max_layer_samples,
        )

    def telemetry_session(self, name: str, mode: str = "trace",
                          max_events: int | None = None,
                          max_meta_pool: int | None = None,
                          bench_samples_per_phase: int | None = None,
                          max_layer_samples: int | None = None) -> "TelemetrySession":
        """Create a named telemetry session for manual span instrumentation."""
        from .telemetry import TelemetrySession
        return TelemetrySession(
            self.store, name, mode,
            max_events=max_events,
            max_meta_pool=max_meta_pool,
            bench_samples_per_phase=bench_samples_per_phase,
            max_layer_samples=max_layer_samples,
        )

    def telemetry_report(self, name: str = "cli_report"):
        """Print a full Python performance report for the current telemetry session.

        统一出口 report_session: 终端报告 + 四产物 + Pipeline Latency + 状态行,
        报告后消费性 clear (对齐 Rust report_all 语义)。
        """
        try:
            from .telemetry import report_session
            print(report_session(self.store, name))
        except Exception as e:
            print(f"Telemetry report failed: {e}")

    @staticmethod
    def telemetry_help(topic: str = ""):
        """Print telemetry usage guide."""
        from .telemetry import telemetry_help
        telemetry_help(topic)

    async def exec(self, code: str, active_doc: str = "", wt_uid: str = "genesis", ctx: RuntimeContext = None, stream: bool = False ) -> str:
        """
        全链路执行入口
        :param wt_uid: 当前这句命令是谁发出的 (用来定夺沙箱权限)
        """
        frame_token = None
        frame_handed_off = False
        try:
            from mdio_py.telemetry import _TelemetrySpan as _tel_span
            if DEBUG: t0 = time.perf_counter_ns()
            with _tel_span(self.store, "rt_parse"):
                raw_ir = parse_code(code)
                # NOTE: optimize_ir 原地修改 raw_ir 的 steps（fuse_steps），
                # 这些对象同时也在 _PARSE_RESULT_CACHE 中 —— 第二次调用同 code 时
                # 缓存命中拿到的已经是融合后版本。这是安全的：fuse_steps 幂等，
                # Rust 端兼容融合前后两种形式。
                optimized_ir = optimize_ir(raw_ir)
            if DEBUG: t1 = time.perf_counter_ns()
            if DEBUG: print(f"[DEBUG] parse_code returned: {raw_ir}")
            # Context 里面再也没有物理依赖，单纯计算初始 CWD
            # 无论你是哪个 token，你的活动基准点通常也是虚拟根 /
            vfs_path = "/"
            doc_path_vfs = None

            if active_doc:
                # 规范化传入的参考焦点文档
                clean_doc = active_doc.lstrip('/')
                doc_path_vfs = posixpath.join(vfs_path, clean_doc)
                vfs_path = posixpath.dirname(doc_path_vfs)

            if ctx is not None and getattr(ctx, "wt_uid", None):
                # 传入的 ctx 优先：hook 回调（core/events.py）、cron job、XBT 步进都靠它
                # 表达「就在这份上下文里执行」。RuntimeContext 是 frozen dataclass，
                # 直接沿用无原地改写风险；ctx 未携带有效 wt（空/占位值）时退回下面的
                # 参数路径，避免一个畸形 ctx 把执行静默改道。
                initial_ctx = ctx
            else:
                initial_ctx = RuntimeContext(
                    wt_uid=wt_uid or self._workspace.active_worktree_uid,
                    vfs_path=vfs_path,
                    active_doc=doc_path_vfs,
                    worktree_name=self._workspace.active_worktree_name,
                )

            # 记录执行耗时
            if DEBUG: t2 = time.perf_counter_ns()
            frame_token = begin_exec_frame(self._workspace)
            with _tel_span(self.store, "rt_interpreter"):
                result = await self._interpreter.run(optimized_ir, ctx=initial_ctx)
            if DEBUG: t3 = time.perf_counter_ns()

            # 打印 Py 宏观周期（E1 解析 / E2 解释器求值；仅 DEBUG 打开）
            if DEBUG: 
                print(f"\n[⏱️ TRACE] E1 (Py-Parse): {(t1 - t0) / 1_000_000:.3f} ms")
                print(f"[⏱️ TRACE] Total Intepreter Eval: {(t3 - t2) / 1_000_000:.3f} ms")

            if inspect.isasyncgen(result):
                # 如果开启了 stream，直接源源不断吐出去。
                # 命名 mem 的 frame 必须覆盖 stream 的消费周期，故用 wrapper 延迟 reset。
                if stream:
                    frame_handed_off = True
                    async def _stream_with_frame():
                        try:
                            async for chunk in result:
                                yield chunk
                        finally:
                            try:
                                aclose = getattr(result, "aclose", None)
                                if aclose is not None:
                                    await aclose()
                            except BaseException:
                                pass
                            end_exec_frame(frame_token)
                    return _stream_with_frame()
                # 否则全部吃干抹净
                parts = []
                async for chunk in result:
                    parts.append(str(chunk))
                return ''.join(parts)
                
            # 为了确保如果发生错误或者返回的是纯文本，stream调用者不会崩溃
            # 伪造一个简单的 async 迭代器包住字符串
            if stream:
                async def _yield_str():
                    yield str(result) if result is not None else "OK"
                return _yield_str()
            return str(result) if result is not None else "OK"
        except MdioError as e:
            try:
                # 哪怕打印错误，也是借用 "root" 身份强行写回 VFS 日志区
                err_file_vfs = "/system_error.md"
                self._workspace.write_locator(
                    "genesis", err_file_vfs, SelectorBuilder().build(), "/>", "\n" + e.to_markdown()
                )
            except Exception:
                pass 
            return e.to_markdown()
            
        except Exception as e:
            err = MdioError("system", ErrCode.INTERP_CRASH, f"解释器系统级崩溃: {str(e)}")
            return err.to_markdown()
        finally:
            if frame_token is not None and not frame_handed_off:
                end_exec_frame(frame_token)

    def close(self):
        """显式生命周期终结（幂等）——业务分发语义零改动。

        _hook_consumer 常驻 Task + Rust 侧 dispatcher 的 Py
        引用构成 store 保活链（Task→coro帧→HookManager→Workspace→store），
        使 store 释放后 6 个 worker 线程永不退出（线程泄漏 → 资源耗尽）。
        close() 切断保活链：
          ① HookManager.close()：cancel 消费协程 + 占位回调替换 + 断链
          ② 断 workspace.runtime 反向引用（打破 rt↔ws 互指环）
        之后对象随引用计数回收，worker 线程随 channel 关闭退出。
        不调用时行为与现状完全一致；close 后 rt.store 仍可用（写操作
        正常），hook 分发停止（销毁语义）。
        """
        ws = getattr(self, "_workspace", None)
        if ws is None:
            return
        try:
            hm = getattr(ws, "hook_manager", None)
            if hm is not None:
                hm.close()
        except Exception:
            pass
        # 断 rt↔ws 互指（打破循环——引用计数路径释放）
        try:
            ws.runtime = None
        except Exception:
            pass

    @classmethod
    def get_instance(cls, storage_root: str = None) -> 'MdioRuntime':
        if cls._instance is None:
            # 默认 storage_root = **挂载配置串**，其 path 段 = 根 wt 的**项目根**
            #   （安装态 <home>/genesis、源码态 <home>/workspace）。
            #   注意：这里给的是项目根，**不是**文档根 —— 文档子目录由 core 按 doc_subdir 自己套。
            if storage_root is None:
                current_dir = os.path.abspath(str(runtime_root()))
                storage_root = os.getenv("MDIO_VFS_MOUNTS", f"{current_dir}")
            cls._instance = cls(storage_root)
        return cls._instance
