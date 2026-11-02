import json
import os
import asyncio
from datetime import datetime, timezone
from pathlib import Path
from contextlib import asynccontextmanager
from litestar import Litestar
from litestar.datastructures import State

from mdio_py import MdioRuntime
from mdio_py.core.ast_utils import bullets_to_kv
from mdio_py.core.bootstrap import bootstrap_runtime
from mdio_py.core.selector_builder import SelectorBuilder

# ── iroh 模块延迟导入（非必需依赖）──────────────────
try:
    import iroh as _iroh
    IROH_AVAILABLE = True
except ImportError:
    _iroh = None
    IROH_AVAILABLE = False

# ── gateway attach/detach 追踪（仅 MDIO_PYTEST_TRACE=1）────────────
def _trace_gateway(label: str) -> None:
    if os.environ.get("MDIO_PYTEST_TRACE") != "1":
        return
    try:
        with open("/tmp/mdio_gateway_trace.log", "a", encoding="utf-8") as f:
            f.write(f"{os.getpid()} {label}\n")
            f.flush()
    except Exception:
        pass


# ── runtime.md：runtime 的 VFS 投影，只属于 genesis ──────────────
RUNTIME_MD = "/.mds/runtime.md"


def process_mode() -> str:
    """control：只起 runtime；gateway：启动时挂 gateway。"""
    mode = os.getenv("MDIO_PROCESS_MODE", "gateway").strip().lower()
    return mode if mode in ("control", "gateway") else "gateway"


def runtime_md_read(workspace) -> dict[str, str]:
    """读 genesis 的 runtime.md 字段（tojson 信封 + bullets_to_kv）。

    文件不存在 / 非 genesis 都返回空表。
    入参 workspace 提供引擎文本出口 workspace.to_json_text。
    平铺 `- k: v` 与行解析逐键一致；不等的样例（`- k : v` 冒号前带空格、
    懒续行、值里的行内标记、CRLF 尾巴）由 tests/test_gateway_kv_migration.py 逐条钉住。
    """
    store = workspace.store     # 传错对象（裸 store）立刻 AttributeError，不静默成空表
    try:
        text = store.get_text("genesis", RUNTIME_MD)
    except Exception:
        return {}
    if not (text or "").strip():
        return {}
    try:
        env = json.loads(workspace.to_json_text(text))
    except Exception:
        return {}               # 引擎/JSON 异常与「文件不存在」同判空表（不抛）
    return bullets_to_kv(env)


def runtime_md_write(workspace, fields: dict[str, str]) -> None:
    """整文件重写 runtime.md。直接走 store，丢弃 CRDT packet，不触发广播。"""
    text = "# RUNTIME\n" + "".join(f"- {k}: {v}\n" for k, v in fields.items())
    workspace.store.write_locator(
        "genesis", RUNTIME_MD,
        SelectorBuilder().build(), "/>", text,
        True, True, False, True,   # auto_scaffold, god, auto_level, suppress_hooks
    )


def runtime_md_init(workspace, fields: dict[str, str]) -> None:
    """启动时写 runtime.md。已有投影则整文件替换，否则创建。"""
    if workspace.store.path_exists("genesis", RUNTIME_MD):
        runtime_md_write(workspace, fields)
        return
    text = "# RUNTIME\n" + "".join(f"- {k}: {v}\n" for k, v in fields.items())
    workspace.store.write_locator(
        "genesis", RUNTIME_MD,
        SelectorBuilder().build(), "+>", text,
        True, True, False, True,
    )


def runtime_md_set(workspace, key: str, value) -> None:
    fields = runtime_md_read(workspace)
    fields[key] = str(value)
    runtime_md_write(workspace, fields)


# ── Gateway 挂摘 ──────────────────────────────────────────────

async def attach_gateway(runtime: MdioRuntime, app_state) -> None:
    """把 Gateway 投影挂到当前 runtime。声明文件 gateway.md / worktrees.md 不动。"""
    _trace_gateway("attach_enter")
    if getattr(app_state, "gateway_store", None) is not None:
        return

    from mdio_py.runtime.rule_manager import RuleManager
    from mdio_py.gateway.store import GatewayStore
    from mdio_py.gateway.crdt_scope import CrdtScope
    from mdio_py.gateway.ast_bridge import AstBridge
    from mdio_py.gateway import get_channels_plugin

    workspace = runtime._workspace
    rule_mgr = RuleManager(runtime.store, None, workspace)
    gateway_store = GatewayStore(
        store=runtime.store,
        workspace=workspace,
        iroh_endpoint=None,
    )
    rule_mgr._gateway_mgr = gateway_store

    # 单一 scope 注册表：WS 与 iroh 同表。
    app_state.rule_mgr = rule_mgr
    app_state.gateway_store = gateway_store
    app_state.wt_runtimes = gateway_store._scopes
    workspace._scopes = gateway_store._scopes
    gateway_store._scopes["genesis"] = CrdtScope("genesis")
    _trace_gateway("attach_reconcile_start")
    gateway_store.reconcile_all()
    _trace_gateway("attach_reconcile_done")
    # 长驻任务走**监护壳**：裸 task 崩了在 asyncio 里只会留一句没人看的
    # "Task exception was never retrieved"，而进程/exec/WS 一切正常 —— 见 task_supervisor.py。
    from mdio_py.gateway.task_supervisor import supervise, fault_gate

    def _tick_factory():
        fault_gate("tick")          # env 闸门的故障注入（默认关；见 task_supervisor.fault_gate）
        return gateway_store._tick_loop()

    _tick_sup, gateway_store._tick_task = supervise(
        "gateway_store.tick_loop", _tick_factory
    )
    workspace._gateway_store = gateway_store

    # iroh endpoint：失败不阻断 gateway 本身。
    iroh_ep = None
    if IROH_AVAILABLE:
        try:
            _iroh.iroh_ffi.uniffi_set_event_loop(asyncio.get_running_loop())
            from mdio_py.gateway.iroh_transport import make_iroh_creator
            creator = make_iroh_creator(gateway_store)
            iroh_ep = await _iroh.Endpoint.bind(_iroh.EndpointOptions(
                preset=_iroh.preset_n0(),
                alpns=[b"mdio-net/1"],
                protocols={b"mdio-net/1": creator},
            ))
            await iroh_ep.online()
            print(f"🌐 [iroh] Endpoint online: {iroh_ep.id()}")
        except Exception as e:
            print(f"⚠️ [iroh] Init skipped: {e}")

    app_state.iroh_endpoint = iroh_ep
    gateway_store._ep = iroh_ep
    workspace._channels = get_channels_plugin()
    # 投递走通道 ⇒ 出口需要插件句柄（publish 一次、零 await）
    gateway_store._channels = workspace._channels

    # AstBridge：P-patch 增量广播。
    ast_bridge = AstBridge(runtime.store)
    # **先绑循环再起消费者**：构造时就注册了 Rust 回调，中间那段窗口里来的包会丢
    #（实测：故障注入重启时 dropped_early=1，正是这个窗口丢的一包）。这里已经在 running loop 里。
    ast_bridge.bind_loop()

    async def _broadcast_ast_p(doc_id: int, packet: bytes):
        uid = runtime.store.doc_wt_uid(doc_id) or "genesis"
        # 兜底：scope 以 gateway_store 权威表为准（连接侧已保证登记；这里再兜一层，
        # 避免任何"先写入、后订阅"的时序把补丁丢成静默空洞）。
        # 走唯一出口（kind=Ast ⇒ 不抑制来源：发起端的 render 视图也要这一帧）
        from mdio_py.gateway.crdt_scope import deliver, KIND_AST
        _out = await deliver(gateway_store, doc_id, packet, KIND_AST, store=runtime.store)
        return _out

    def _ast_factory():
        fault_gate("ast_bridge")    # 同上：注入一次 ⇒ 看监护壳把它接回来（活体判据）
        return ast_bridge.consumer(_broadcast_ast_p)

    _trace_gateway("attach_supervise_start")
    _ast_sup, gateway_store._ast_task = supervise(
        "ast_bridge.consumer", _ast_factory
    )
    _trace_gateway("attach_supervise_done")
    app_state.ast_bridge_task = gateway_store._ast_task
    # 读数口（`/control/status` 查）：桥的流量 + 两条长驻任务的监护状态
    app_state.ast_bridge = ast_bridge
    app_state.task_supervisors = {"ast_bridge": _ast_sup, "tick": _tick_sup}
    _trace_gateway("attach_done")


async def detach_gateway(runtime: MdioRuntime, app_state) -> None:
    """把 Gateway 投影从 runtime 摘下；不写 gateway.md / worktrees.md。"""
    _trace_gateway("detach_enter")
    workspace = runtime._workspace
    gateway_store = getattr(app_state, "gateway_store", None)
    if gateway_store is None:
        gateway_store = getattr(workspace, "_gateway_store", None)
    if gateway_store is None:
        return

    handlers = [
        active.handler
        for active in list(gateway_store._actives.values())
        if getattr(active, "handler", None) is not None
    ]

    _trace_gateway("detach_deactivate_start")
    gateway_store.deactivate_all()
    _trace_gateway("detach_deactivate_done")
    for handler in handlers:
        _trace_gateway("detach_handler_stop")
        handler.stop()
        await handler.wait_stopped()
    _trace_gateway("detach_handlers_done")

    for task_name in ("_tick_task", "_ast_task"):
        task = getattr(gateway_store, task_name, None)
        if task is not None and not task.done():
            task.cancel()
            try:
                await task
            except (asyncio.CancelledError, Exception):
                pass

    # 摘掉 AstBridge 的 Rust 回调，避免 gateway off 后旧 queue/closure 被保活。
    try:
        runtime.store.register_ast_callback(lambda *a, **k: None)
    except Exception:
        pass

    ep = getattr(app_state, "iroh_endpoint", None)
    if ep is not None:
        try:
            await ep.close()
        except Exception:
            pass

    workspace._gateway_store = None
    workspace._scopes = {}
    workspace._channels = None
    app_state.gateway_store = None
    app_state.wt_runtimes = {}
    app_state.iroh_endpoint = None
    app_state.ast_bridge_task = None
    rule_mgr = getattr(app_state, "rule_mgr", None)
    if rule_mgr is not None:
        rule_mgr._gateway_mgr = None
    _trace_gateway("detach_done")


# ── 顶层停机：single-flight ────────────────────────────────────

async def _shutdown_once(runtime: MdioRuntime, app_state) -> None:
    workspace = runtime._workspace
    fields = runtime_md_read(workspace)
    if fields.get("gateway_enabled") == "true" or getattr(app_state, "gateway_store", None) is not None:
        await detach_gateway(runtime, app_state)

    for uid, *_ in runtime.store.list_worktrees():
        try:
            await workspace.dispose_plugins_wt(uid, strict=False)
        except Exception as e:
            print(f"⚠️ [shutdown] dispose {uid} failed: {e}")

    # 停机顺序：hook consumer 与 Rust 回调引用。
    try:
        runtime.close()
    except Exception as e:
        print(f"⚠️ [shutdown] runtime.close failed: {e}")

    # flush 全部 WT 的 session 到 VFS 内存。
    for uid, *_ in runtime.store.list_worktrees():
        try:
            runtime.store.flush_worktree_sessions(uid)
        except Exception as e:
            print(f"⚠️ [shutdown] flush {uid} failed: {e}")

    try:
        runtime_md_set(workspace, "status", "stopped")
    except Exception:
        pass


def start_shutdown(runtime: MdioRuntime, app_state) -> asyncio.Task:
    """创建 single-flight shutdown task；/control/stop 与 lifespan 共用。"""
    task = getattr(app_state, "shutdown_task", None)
    if task is None or task.done():
        task = asyncio.get_running_loop().create_task(
            _shutdown_once(runtime, app_state)
        )
        app_state.shutdown_task = task
    return task


async def await_shutdown(app_state, runtime=None) -> None:
    task = getattr(app_state, "shutdown_task", None)
    if task is None and runtime is not None:
        task = start_shutdown(runtime, app_state)
    if task is None:
        return
    try:
        await task
    except (asyncio.CancelledError, Exception):
        pass


def _report_module_docs(runtime: MdioRuntime) -> None:
    """启动自检（只报不动）：模块依赖仓 node_modules 落在文档根内时，pump 会把它的 *.md 也当文档。

    实测：genesis 隐藏树 283 篇里 116 篇来自 ui/node_modules（本根 58 + 子 wt 挂载镜像 58），
    exp_app 67 篇里 58 篇。修法在**盘面**：把 pnpm 虚拟仓移出文档根
    （`<文档根>/ui/pnpm-workspace.yaml` 的 `virtualStoreDir` 指到项目根下）——node_modules 里就只剩
    符号链接/元数据，而 pump 逐条 list 时**不跟随目录符号链接**，依赖仓的 .md 不再进 VFS。
    """
    hits = []
    for uid, name, _alias, _root in runtime.store.list_worktrees():
        try:
            tree = json.loads(runtime.store.get_vfs_tree(uid, True).decode())
        except Exception:
            continue
        n = sum(1 for d in tree if "node_modules" in (d.get("path") or ""))
        if n:
            hits.append(f"{name or uid}={n}")
    if hits:
        print(
            "⚠️ [vfs] 文档树里有 node_modules 依赖仓文档：" + ", ".join(hits) + "（挂载镜像会重复计）"
            " —— 修法：把 pnpm 虚拟仓移出文档根（<文档根>/ui/pnpm-workspace.yaml: virtualStoreDir）；"
            "目录符号链接 pump 不跟随，移出后这些 .md 不再进 VFS。"
        )


@asynccontextmanager
async def lifespan(app: Litestar):
    mode = process_mode()
    from mdio_py._env import runtime_root
    workspace_root = runtime_root()
    vfs_mounts = os.getenv("MDIO_VFS_MOUNTS", str(workspace_root))
    host = os.getenv("MDIO_GATEWAY_HOST", "127.0.0.1")
    port = os.getenv("MDIO_GATEWAY_PORT", "8011")
    print(f"🚀 Server Starting... mode={mode} VFS Multiverse Config: {vfs_mounts}")
    # 落盘模式必须一眼可见：core 侧开关缺省关（`persist_enabled()`），关着时写入只活内存、重启即回退。
    _persist = os.getenv("MDIO_PERSIST", "").strip().lower() in ("1", "true")
    print(
        f"💾 [persist] MDIO_PERSIST={os.getenv('MDIO_PERSIST') or '(unset)'} → "
        + ("文档写回盘 + WAL" if _persist else "⚠️ 只活内存（重启即回退）：请用 mdio serve 启动，或显式 MDIO_PERSIST=1")
    )

    runtime = None
    try:
        _mdio_debug = os.getenv("MDIO_DEBUG", "False").lower() in ("true", "1", "yes")
        runtime = MdioRuntime(vfs_mounts, telemetry="auto" if _mdio_debug else "off")
        app_state = app.state
        app_state.runtime = runtime
        app_state.shutdown_task = None

        runtime_md_init(runtime._workspace, {
            "pid": os.getpid(),
            "mode": mode,
            "host": host,
            "port": port,
            "root": vfs_mounts,
            "started_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "status": "starting",
            "gateway_enabled": "true" if mode == "gateway" else "false",
        })
        print("💿 [vfs] Initiating MDIO Bootstrap Sequence...")

        # 启动即挂载项目（现有行为）。
        from mdio_py.runtime.worktree_paths import is_anchor_dir
        try:
            _cwd = os.path.realpath(os.getcwd())
            if not is_anchor_dir(_cwd):
                from mdio_py.gateway.worktree_api import dispatch_worktree
                _load_rule = os.environ.get("MDIO_LOAD_RULE", "")
                loaded = await dispatch_worktree(runtime, "worktree.load", {
                    "path": _cwd,
                    "rule": _load_rule,
                    "parent": "genesis",
                }, self_uid="genesis")
                uid, is_new = loaded["uid"], loaded["is_new"]
                print(f"🚀 [lifespan] project worktree loaded: {_cwd} (uid {uid}, new={is_new}, rule={_load_rule!r})")
        except Exception as e:
            print(f"⚠️ [lifespan] project worktree load skipped: {e}")

        await bootstrap_runtime(runtime)
        _report_module_docs(runtime)

        if not runtime.store.path_exists("genesis", "/.mds/env.md"):
            runtime._workspace.write_locator(
                "genesis", "/.mds/env.md",
                SelectorBuilder().build(), "+>",
                "# SYSTEM_ENV\n- MDIO_VERSION: 0.8.0\n",
            )
        if not runtime.store.path_exists("genesis", "/.mds/ui.md"):
            runtime._workspace.write_locator(
                "genesis", "/.mds/ui.md",
                SelectorBuilder().build(), "+>",
                "## mods\n- std: .ui::std/appearance.js\n\n## match\n- std: .*::.*\n",
            )

        if mode == "gateway":
            await attach_gateway(runtime, app_state)
            runtime_md_set(runtime._workspace, "gateway_enabled", "true")
        else:
            app_state.gateway_store = None
            app_state.wt_runtimes = {}
            app_state.iroh_endpoint = None
            app_state.ast_bridge_task = None
            runtime._workspace._scopes = {}
            runtime._workspace._gateway_store = None

        runtime_md_set(runtime._workspace, "status", "running")
        print("✅ Rust VFS NEO Engine attached to App State.")
    except Exception as e:
        print(f"❌ Failed to init Rust VFS Runtime: {e}")
        if runtime is not None:
            try:
                if getattr(app.state, "gateway_store", None) is not None:
                    await detach_gateway(runtime, app.state)
            except Exception:
                pass
            try:
                runtime.close()
            except Exception:
                pass
        raise

    yield

    runtime = getattr(app.state, "runtime", None)
    if runtime is not None:
        try:
            runtime_md_set(runtime._workspace, "status", "stopping")
        except Exception:
            pass
        await await_shutdown(app.state, runtime)
    print("🛑 Sync Engine shutting down...")


def provide_runtime(state: State) -> MdioRuntime:
    return state.runtime
