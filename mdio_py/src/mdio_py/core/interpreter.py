import asyncio
import time
import json
import inspect
import posixpath
import ast
import textwrap
from dataclasses import replace, dataclass, asdict
from typing import List, Any, Optional

from mdio_py.core import compile_selector_text, split_selector_addr
from .text_utils import str_or_empty, str_or, unquote_plain, unquote_no_strip, ensure_md_ext
from .ast_utils import sections
from .error import MdioError, ErrCode, is_error_text
import mdio_py.std as std_module
from .mdio_ir import SelectorIR, Pipeline, Op, ReadOp, MemReadOp, WriteOp, HookOp, Literal, ScopeBlock, ExecBlock,\
        FuncCall, PluginCall, MacroToken, PIPE_DATA
from .selector_builder import SelectorBuilder, by_tag
from .mem_slot import register_temp_doc, resolve_mem, resolve_temp_alias
from .stream import classify, new_run_id, stream_write, write_stream_record

# from collections import defaultdict
# INTERP_PROFILER = defaultdict(lambda: {"count": 0, "time": 0.0})


DEBUG = False


# 纯虚拟求值坐标系 (完全抛弃物理路径和实体 Jail)

@dataclass(frozen=True)
class RuntimeContext:
    wt_uid: str = "genesis"   # uid 字符串
    vfs_path: str = "/"
    active_doc: Optional[str] = None
    caller_pid: Optional[str] = None
    worktree_name: Optional[str] = None

    def sys_dir(self) -> str:
        """根据自身维次感知系统盘位置"""
        return "/.mds"

    @property
    def proc_path(self) -> str:
        """系统进程表文档路径（Q10：全仓唯一出处）。"""
        return f"{self.sys_dir()}/proc.md"

    @property
    def hook_path(self) -> str:
        """系统钩子文档路径（Q10：全仓唯一出处）。"""
        return f"{self.sys_dir()}/hook.md"

    def resolve_path(self, target: Optional[str]) -> str:
        """纯字符串计算器：把语法层面的意图翻译成对应的绝对坐标"""
        if not target:
            if not self.active_doc:
                raise MdioError("context", ErrCode.VFS_NO_FOC, "[空指针] 目录作用域下使用了缺省寻址")
            return self.active_doc

        target_str = str(target)
        
        # 1. 语义：~/ 代表回到该 worktree 的 /home/（第一条 binding）
        if target_str == "~" or target_str.startswith("~/"):
            target_str = "/home/" + target_str.lstrip("~/")
            
        # 2. 相对路径计算
        elif not target_str.startswith("/"):
            target_str = posixpath.join(self.vfs_path, target_str)

        # 3. 洗消与规范化 (自带防止 /../../ 的降维打击，永远无法逃逸根节点)
        vfs_path = posixpath.normpath(target_str)
        
        return vfs_path


# 核心解释器体

VALID_PROJECTIONS = {"raw", "content", "tag", "mark", "container"}
# FILTER_FIELD 缩写 → -pj 全名（见 mdio_grammar.lark:FILTER_FIELD）
# 在输入边界展开，Rust 侧始终收到全名。
_PROJECTION_SHORTHAND = {
    "r": "raw", "ct": "content", "t": "tag",
    "m": "mark", "ctn": "container",
}

def get_projection(kwargs: dict) -> str | None:
    """统一投影参数提取：同时接受 -pj 和 --projection；-c 已废弃。"""
    if "-c" in kwargs:
        raise MdioError("interpreter", ErrCode.PARSE_TOKEN,
                        "-c 已废弃，请使用 -pj/--projection (raw/content/tag/mark/container)")
    proj = (str(kwargs.get("-pj", "")).lower() or
            str(kwargs.get("--projection", "")).lower() or None)
    if proj and proj not in VALID_PROJECTIONS:
        expanded = _PROJECTION_SHORTHAND.get(proj)
        if expanded is None:
            raise MdioError("interpreter", ErrCode.PARSE_TOKEN,
                            f"无效的 projection '{proj}'，可选: raw/content/tag/mark/container")
        proj = expanded
    return proj


def normalize_read_chunks(raw_list, *, dedupe: bool = False) -> str:
    """读结果归一化（Q5 唯一实现）：可选有序去重 + 剥 \n\r + 换行 join。"""
    if dedupe and isinstance(raw_list, list):
        seen = set()
        raw_list = [x for x in raw_list if not (x in seen or seen.add(x))]
    clean = [s.strip("\n\r") for s in (raw_list or []) if s.strip("\n\r")]
    return "\n".join(clean) if clean else ""


def _split_selector_path(raw: str):
    """将运行时含 :: 的路径字符串拆为 (file_path, selector_steps).

    '/f.md::H'           → ('/f.md', [['Drill', 'H']])
    '/f.md::H._b:k'     → ('/f.md', [['Drill', 'H'], ['Wildcard'], ['TYPECHECK', 'bullet'],
                                      ['CMP', '.tag', '==', 'k'], ['Slice', 0]])
    '/f.md'              → ('/f.md', [])
    """
    addr = split_selector_addr(raw)   # Q2：统一切分入口（语法权威：file_path 段 / md_path 段）
    if addr.file_path is None:
        return raw, []
    if not addr.md_path:
        return addr.file_path, []
    # Q1：唯一编译入口；drill 回落 = 整串单 Drill
    return addr.file_path, compile_selector_text(addr.md_path, on_error="drill").steps


class Interpreter:
    def __init__(self, workspace):
        self.workspace = workspace
        # 开局默认sudo
        self.root_context = RuntimeContext(
            wt_uid="genesis",
            vfs_path="/",
            active_doc=None
        )
        # 函数注册保持不变
        self.std_functions = {
            "find": std_module.find,
            "tree": std_module.tree,
            "split": std_module.split,
            "join": std_module.join,
            "fmt": std_module.fmt,
            "len": std_module.length,
            "trim": std_module.trim,
            "loads": std_module.loads,
            "dumps": std_module.dumps,
            "tojson": std_module.tojson,
            "map": std_module.map,
            "foreach": std_module.foreach,
            "filter": std_module.filter,
            "get": std_module.get,
            "upper": std_module.upper,
            "lower": std_module.lower,
            "rm": std_module.rm,
            "mk": std_module.mk,
            "ls": std_module.ls,
            "reload": std_module.reload,
            "hash": std_module.hash_state,
            "test": std_module.test,
            "calc": std_module.calc,
            "snap": std_module.snap,
            "worktree": std_module.worktree,
            "sel": std_module.sel,
            # arrow 数据面：df（节点关系表 + narwhals 表达式）
            "arrow": std_module.arrow,
        }
    
    async def _eval_dynamic_path(self, dynamic_path: Any, ctx: RuntimeContext) -> str:
        if not dynamic_path: return ""
        if isinstance(dynamic_path, str):
            return unquote_no_strip(dynamic_path)   # Q7 档位③：不 strip（路径空格原样）

        if isinstance(dynamic_path, (ExecBlock, Op)):
            # Q13：ExecBlock / Op 两支逐字重复，合并；判据与去引号走唯一实现
            if isinstance(dynamic_path, ExecBlock):
                res = await self.run([dynamic_path.pipeline], ctx)
            else:
                res = await self.run([Pipeline(atoms=[dynamic_path])], ctx)
            if inspect.isasyncgen(res):
                res = await self._exhaust_stream(res)  # 流适配：动态路径是值消费者
            res_str = str_or_empty(res)
            # 不允许降级后的异常字符串作为合法的物理路径继续蔓延
            if is_error_text(res_str):
                raise MdioError("interpreter", ErrCode.INTERP_CRASH, f"动态路径推导陷入异常黑洞: {res_str}")
            return unquote_plain(res_str)   # Q7 档位①

        return str(dynamic_path)

    async def run(self, ir_list: List[Op], ctx: RuntimeContext = None) -> Any:
        if ctx is None: ctx = self.root_context
        last_result = None

        for op in ir_list:
            if isinstance(op, ScopeBlock):
                scope_ctx = ctx

                # Step 1: 时空前缀路由（ctx_prefix，数字或 %macro）
                if op.context_id is not None:
                    scope_ctx = await self._switch_prefix_context(op.context_id, scope_ctx)

                # Step 2: worktree 名路由（file_path 可能是注册的 worktree 名）
                target_str = op.file_path
                scope_ctx, routed_file, _ = self._apply_worktree_name(target_str, scope_ctx, None)

                # worktree 名消费了 file_path → 用 routed_file 替换
                if target_str and routed_file != target_str:
                    target_str = routed_file

                if target_str:
                    is_dir_scope = target_str.endswith('/')
                    if not is_dir_scope and '.' not in target_str.split('/')[-1]:
                        target_str += ".md"
                    vfs_path = scope_ctx.resolve_path(target_str)
                elif not target_str and scope_ctx.active_doc:
                    # 路由到 worktree 且无显式文件 → 沿用 active_doc
                    vfs_path = scope_ctx.active_doc
                    is_dir_scope = False
                else:
                    # fallback: cwd（保持兼容）
                    vfs_path = scope_ctx.resolve_path("/")
                    is_dir_scope = True

                if is_dir_scope:
                    new_ctx = replace(scope_ctx, vfs_path=vfs_path, active_doc=None)
                else:
                    new_ctx = replace(scope_ctx, vfs_path=posixpath.dirname(vfs_path), active_doc=vfs_path)

                last_result = await self.run(op.pipeline, new_ctx)
            
            elif isinstance(op, Pipeline):
                last_result = await self.visit_pipeline(op, ctx)
                
            elif isinstance(op, ExecBlock):
                if isinstance(op.pipeline, Pipeline):
                    last_result = await self.visit_pipeline(op.pipeline, ctx)
                elif isinstance(op.pipeline, list):
                    last_result = await self.run(op.pipeline, ctx)
                else:
                    last_result = await self.run([op.pipeline], ctx)
                # if hasattr(op.pipeline, 'atoms'):
                #     last_result = await self.run(op.pipeline.atoms, ctx)
                # else:
                #     last_result = await self.run(op.pipeline, ctx)
                
            if isinstance(last_result, str) and last_result.startswith("- mdio_error:"):
                return last_result
                
        return last_result

    async def _exhaust_stream(self, data) -> str:
        if hasattr(type(data), '__aiter__'):
            chunks = []
            async for chunk in data:
                if chunk: chunks.append(str(chunk))
            return "".join(chunks)
        return str(data) if data is not None else ""

    # async def _exhaust_stream(self, data) -> str:
    #     if inspect.isasyncgen(data):
    #         chunks = []
    #         async for chunk in data:
    #             if chunk: chunks.append(str(chunk))
    #         return "".join(chunks)
    #     return str(data) if data is not None else ""

    async def visit_pipeline(self, pipeline: Pipeline, ctx: RuntimeContext):
        data = None
        last_write_info = None

        for atom in pipeline.atoms:
            # if DEBUG: print(f"[DEBUG visit_pipeline] atom type: {type(atom).__name__}, atom: {atom}")
            # atom_type = type(atom).__name__
            # t_atom_start = time.perf_counter()
            try:   
                atom_ctx = ctx 
                
                # raw_kwargs = getattr(atom, 'kwargs', {})
                # raw_args = getattr(atom, 'args', [])
                
                # MacroToken
                if hasattr(atom, 'name') and type(atom).__name__ == "MacroToken":
                    data = await self._resolve_macro(atom.name, atom_ctx)
                    continue

                raw_kwargs = getattr(atom, 'kwargs', {})
                raw_args = getattr(atom, 'args', [])
                if getattr(atom, 'name', '') in {"map", "foreach", "filter"}:
                    # map/foreach/filter 是 thunk 命令：-f 的 ExecBlock 不能被 eager 执行。
                    resolved_kwargs = {}
                    for _k, _v in raw_kwargs.items():
                        if _k == '-f' and isinstance(_v, ExecBlock):
                            resolved_kwargs[_k] = _v
                        else:
                            resolved_kwargs[_k] = await self._evaluate_args(_v, data, atom_ctx)
                    resolved_args = await self._evaluate_args(raw_args, data, atom_ctx)
                else:
                    resolved_kwargs = await self._evaluate_args(raw_kwargs, data, atom_ctx)
                    resolved_args = await self._evaluate_args(raw_args, data, atom_ctx)
                
                # 下划线首 atom：表示当前管道数据
                if atom is PIPE_DATA:
                    if data is None:
                        return "- mdio_error: [PIPE_DATA] 当前没有管道数据"
                    continue

                # ScopeBlock or ExecBlock
                if isinstance(atom, ScopeBlock) or isinstance(atom, ExecBlock):
                    data = await self.run([atom], atom_ctx)
                    continue
                
                # Literal
                elif isinstance(atom, Literal):
                    data = atom.value
                
                # ReadOp
                elif isinstance(atom, ReadOp):
                    if isinstance(atom.selector, ExecBlock):
                        # $($inner) — 返回 inner 的执行结果（不是文件路径读）
                        data = await self.run([atom.selector.pipeline], atom_ctx)
                        continue
                    else:
                        atom_ctx = await self._switch_prefix_context(atom.selector.context_id, atom_ctx)
                        try:
                            raw_file = await self._eval_dynamic_path(atom.selector.file_path, atom_ctx)
                        except Exception as e:
                            err = MdioError("interpreter", ErrCode.INTERP_CRASH, f"路径推导失败: {e}", cause=e)
                            return err.to_markdown()

                    proj = get_projection(resolved_kwargs)
                    if atom.selector.is_tag_target():
                        proj = "content"  # :KEY 读也是 value-target（无条件覆盖，含显式 -pj）

                    temp_path = resolve_temp_alias(raw_file)
                    if temp_path is not None:
                        raw_file = temp_path
                    else:
                        mem = resolve_mem(raw_file)
                        if mem is not None:
                            data = normalize_read_chunks(mem.read(atom.selector, proj), dedupe=True)
                            continue

                    atom_ctx, raw_file, vfs_path = self._apply_worktree_name(raw_file, atom_ctx, atom.selector)

                    vfs_path = ensure_md_ext(vfs_path)   # Q4

                    if "*" in (raw_file or "") or "?" in (raw_file or ""):
                        v_paths = self.workspace.expand_path_pattern(atom_ctx.wt_uid, vfs_path)
                        merged_data = []
                        for vp in v_paths:
                            res = self.workspace.read_locator(atom_ctx.wt_uid, vp, atom.selector, proj=proj)
                            if res: merged_data.extend(res)
                        data = normalize_read_chunks(merged_data)   # Q5
                    else:
                        raw_list = self.workspace.read_locator(atom_ctx.wt_uid, vfs_path, atom.selector, proj=proj)
                        data = normalize_read_chunks(raw_list, dedupe=True)   # Q5

                # MemReadOp
                elif isinstance(atom, MemReadOp):
                    # 动态 selector 只写本地变量：-f 的 ExecBlock 会被 map/foreach/filter
                    # 逐 item 复用，若把编译结果回写到 atom.selector，第二个 item 会读到
                    # 上一个 item 的静态 selector（实测：foreach 两迭代取到同一目标）。
                    sel = atom.selector
                    if isinstance(sel, ExecBlock):
                        # 动态路径：$$($inner) → 执行 inner，结果作为 selector path
                        res = await self.run([sel.pipeline], atom_ctx)
                        if inspect.isasyncgen(res):
                            res = await self._exhaust_stream(res)  # 流适配：路径是值消费者
                        res_str = unquote_plain(str_or(res))   # Q7①/Q12
                        sel = compile_selector_text(res_str)   # Q1：失败上抛（外层 try 兜 I500，行为不变）
                    # 1. 抓取被送进来的死文本
                    if inspect.isasyncgen(data):
                        raw_text = await self._exhaust_stream(data)
                    else:
                        raw_text = str(data) if data is not None else ""
                    # MemReadOp: 当无 pipeline 数据时，使用 selector 的 file_path（即内联文本）
                    if not raw_text or raw_text == "None":
                        raw_text = sel.file_path or ""

                    # 命名 mem：无论 selector 是什么，命名的是输入 mem 全文。
                    # 空输入也允许注册为空 mem，因此放在 early-continue 之前。
                    mem_name = resolved_kwargs.get("-n") or resolved_kwargs.get("--name")
                    dedented_text = textwrap.dedent(raw_text) if raw_text else ""
                    if mem_name:
                        register_temp_doc(str(mem_name), dedented_text)

                    if not raw_text.strip():
                        data = ""
                        continue

                    # 2. 核心脱缩进装甲 (CommonMark Rule)

                    proj = get_projection(resolved_kwargs)
                    if sel.is_tag_target():
                        proj = "content"  # :KEY 内存读也是 value-target（无条件覆盖）

                    # 3. 进入一次性瞬态引擎！
                    try:
                        raw_list = self.workspace.query_memory(dedented_text, sel, proj=proj)
                        # if DEBUG:
                        #     t1_mem = time.perf_counter_ns()
                        #     print(f"[ TRACE MemReadOp] FFI Turnaround: {(t1_mem - t0_mem) / 1000:.1f} μs")
                        
                        data = normalize_read_chunks(raw_list, dedupe=True)   # Q5（含 recursive wildcard 有序去重）
                    except Exception as e:
                        return MdioError("rust", ErrCode.RUST_IO, f"内存沙盘穿透失败: {e}").to_markdown()

                # HookOp
                elif isinstance(atom, HookOp):
# 1. 拦截清理指令 (-del)
                    if "-del" in resolved_kwargs:
                        hook_id = int(resolved_kwargs["-del"])
                        self.workspace.store.remove_hook(hook_id)
                        hook_del_ir = SelectorBuilder().drill("HOOKS").drill(f"hook_{hook_id}").build()
                        try:
                            self.workspace.write_locator(atom_ctx.wt_uid, atom_ctx.hook_path, hook_del_ir, "->", "")
                        except:
                            pass
                        data = f"- hook: deleted id [{hook_id}]"
                        continue

                    # 2. 拦截批量清理指令 (-deldoc)
                    if "-deldoc" in resolved_kwargs:
                        vfs_path = atom_ctx.resolve_path(resolved_kwargs["-deldoc"])
                        if (str(vfs_path).startswith("/.tmp/")
                                or resolve_temp_alias(str(resolved_kwargs["-deldoc"])) is not None):
                            return "- mdio_error: [hook] temp 文档不支持 hook"
                        self.workspace.store.clear_doc_hooks(atom_ctx.wt_uid, vfs_path)
                        # 持久层同步：hook.md 里 target 指向该文档的条目也要删。否则重启时
                        # bootstrap 重放会把「已撤声明」的钩子复活（实测：撤声明只清 Rust 侧）。
                        # 整文件写：节级 `/>` 不幂等，反复写会累积副本。
                        removed = 0
                        hook_md = atom_ctx.hook_path   # Q10
                        raw = self.workspace.read_full_text(atom_ctx.wt_uid, hook_md)   # Q16
                        # 逐条按 id 删（复用 `-del` 的删节通路 `-> ::HOOKS.hook_<id>`），
                        # 不做「读全文→写全文」：更精准，且没有读改写竞态。
                        # 读法：交给引擎投影（节表 → 每节字段 kv）。
                        # 注意标题正文是标题的**兄弟**而非子节点（实测），故用 sections() 的
                        # 窗口切分；标题 `t` 可能是行内节点数组（`hook_x` 的下划线被当标记），
                        # sections() 内部已用 inline_text 归一。
                        try:
                            _hook_env = json.loads(self.workspace.to_json_text(raw))
                        except Exception:
                            _hook_env = {}
                        for sec_id, _kv in sections(_hook_env).items():
                            if not sec_id.startswith("hook_"):
                                continue
                            _tv = str(_kv.get("target") or "").strip()
                            if not _tv:
                                continue
                            _ta = split_selector_addr(_tv)   # Q2：统一切分入口
                            tgt = _ta.file_path if _ta.file_path is not None else _tv
                            if tgt != vfs_path:
                                continue
                            hook_del_ir = SelectorBuilder().drill("HOOKS").drill(sec_id).build()
                            try:
                                self.workspace.write_locator(atom_ctx.wt_uid, hook_md, hook_del_ir, "->", "")
                                removed += 1
                            except Exception:
                                pass
                        data = f"- hook: cleared all for {vfs_path}（持久条目 -{removed}）"
                        continue

                    # 3. 核心常规注册逻辑
                    cb_code = resolved_kwargs.get("-cb")
                    target_sel = resolved_kwargs.get("-target")

                    if not cb_code or not target_sel:
                        raise MdioError("interpreter", "HOOK_ERR", "必须提供 -cb 和 -target")

                    raw_str = str(target_sel).strip()
                    raw_file, steps = _split_selector_path(raw_str)
                    if not raw_file:
                        raw_file = atom_ctx.active_doc or ""
                    if steps:
                        hook_sel = SelectorIR(file_path=raw_file, steps=steps)
                    else:
                        hook_sel = SelectorBuilder().build()

                    vfs_path = ensure_md_ext(atom_ctx.resolve_path(raw_file))   # Q4
                    if (str(vfs_path).startswith("/.tmp/")
                            or resolve_temp_alias(raw_file) is not None):
                        return "- mdio_error: [hook] temp 文档不支持 hook"

                    is_always = "-is_always" in resolved_kwargs

                    try:
                        print(f"📍 [Py->Rust] 准备注册钩子 -> 锚点: {vfs_path} | steps: {hook_sel.steps}")

                        # if DEBUG:
                        #     print(f"[DEBUG Interpreter] Registering hook:")
                        #     print(f"  wt_uid: {atom_ctx.wt_uid}")
                        #     print(f"  vfs_path: {vfs_path}")
                        #     print(f"  selector_dict: {clean_selector_dict}")
                        #     print(f"  cb_code: {cb_code}")
                        #     print(f"  ctx: {atom_ctx}")
                        #     print(f"  is_always: {is_always}")

                        uid = atom_ctx.wt_uid
                        hook_id = self.workspace.store.register_hook(
                            atom_ctx.wt_uid,
                            vfs_path,
                            hook_sel,
                            str(cb_code),
                            asdict(atom_ctx),
                            is_always
                        )

                        # 核心：预写日志 (WAL) — wt_uid 是 uid
                        if is_always:
                            hook_body = (
                                f"\n## hook_{hook_id}\n"
                                f"- target: {raw_str}\n"
                                f"- cb: {cb_code!r}\n"
                                f"- is_always: true\n"
                                f"- wt_uid: {uid}\n"
                                f"- ctx_active_doc: {atom_ctx.active_doc or 'null'}\n"
                                f"- ctx_cwd: {atom_ctx.vfs_path}\n"
                            )
                            await self.workspace.runtime.exec(
                                f' """{hook_body}""" | +> {atom_ctx.hook_path}::HOOKS -f ',
                                ctx=atom_ctx
                            )
                        continue      
                    except Exception as e:
                        raise MdioError("interpreter", "HOOK_REG_ERR", f"底层钩子注册失败: {e}", cause=e)

                # FuncCall
                elif isinstance(atom, FuncCall):
                    if inspect.isasyncgen(data):
                        # 值语义适配：std 函数是值消费者（未声明流式）——流累积成串
                        data = await self._exhaust_stream(data)
                    if atom.name in self.std_functions:
                        data = await self.std_functions[atom.name](
                            self.workspace, data, resolved_args, resolved_kwargs, atom_ctx
                        )
                    else:
                        raise MdioError("interpreter", "I404", f"Unknown std function '{atom.name}'")
                
                # PluginCall
                elif isinstance(atom, PluginCall):
                    plugin_instance = self.workspace.get_plugin(atom.protocol)
                    if plugin_instance:
                        if inspect.isasyncgen(data) and getattr(plugin_instance, "input_mode", "value") != "raw_stream":
                            # 值语义适配：插件未声明 raw_stream——流累积成串再交付
                            data = await self._exhaust_stream(data)
                        data = await plugin_instance(
                            workspace=self.workspace, 
                            input_data=data, 
                            chain=atom.chain, 
                            ctx=atom_ctx,
                            kwargs=resolved_kwargs
                        )
                    else:
                        data = f"- error: Plugin @{atom.protocol} is not registered."
                
                # WriteOp
                elif isinstance(atom, WriteOp):
                    # 同 MemReadOp：动态写目标只落本地变量，避免 -f 里跨 item 复用 ExecBlock
                    # 时把上一个 item 的静态 selector 带进下一轮。
                    wsel = atom.selector
                    if isinstance(wsel, ExecBlock):
                        res = await self.run([wsel.pipeline], atom_ctx)
                        if inspect.isasyncgen(res):
                            res = await self._exhaust_stream(res)  # 流适配：路径是值消费者
                        res_str = unquote_plain(str_or(res))   # Q7①/Q12
                        file_path, steps = _split_selector_path(res_str)
                        wsel = SelectorIR(file_path=file_path, steps=steps)
                    atom_ctx = await self._switch_prefix_context(wsel.context_id, atom_ctx)
                    auto_scaffold = str(resolved_kwargs.get("-f", "true")).lower() == "true"
                    passthrough = str(resolved_kwargs.get("-pt", "false")).lower() == "true"
                    god_mode = str(resolved_kwargs.get("-god", "false")).lower() == "true"

                    try:
                        raw_file = await self._eval_dynamic_path(wsel.file_path, atom_ctx)
                    except Exception as e:
                        return MdioError("interpreter", ErrCode.INTERP_CRASH, f"写入算址失败: {e}", cause=e).to_markdown()

                    op = atom.operator
                    proj = get_projection(resolved_kwargs)
                    if proj is None and op not in ("->", "~>"):
                        proj = "content"  # /> +> default to content; -> ~> keep None (raw)
                    auto_level = str(resolved_kwargs.get("-l", "false")).lower() == "true"

                    temp_path = resolve_temp_alias(raw_file)
                    if temp_path is not None:
                        raw_file = temp_path
                        mem_slot = None
                    else:
                        mem_slot = resolve_mem(raw_file)
                    if mem_slot is not None:
                        # 命名 mem 目标：payload 稍后统一计算，这里只跳过 VFS 检查。
                        vfs_path = str(raw_file)
                    else:
                        atom_ctx, raw_file, vfs_path = self._apply_worktree_name(raw_file, atom_ctx, wsel)

                        vfs_path = ensure_md_ext(vfs_path)   # Q4

                        # 检查文件是否存在
                        file_exists = self.workspace.path_exists(atom_ctx.wt_uid, vfs_path)
                        if not file_exists:
                            if god_mode:
                                pass  # -god：交给 Rust 全自动创建目录和文件
                            elif auto_scaffold:
                                err = MdioError("interpreter", ErrCode.INTERP_CRASH,
                                                f"File does not exist: {vfs_path}. Use `mk -f` to create it first.")
                                return err.to_markdown()
                            else:
                                err = MdioError("interpreter", ErrCode.INTERP_CRASH,
                                                f"File does not exist: {vfs_path}")
                                return err.to_markdown()

                    # --- 3. 编排 Payload与数据流消费 ---
                    def _format_payload_sync(p: str, oper: str) -> str:
                        p = p.replace('\\n', '\n')
                        if oper == "~>": return self._parse_aider_patch(p)
                        return p

                    if mem_slot is not None and inspect.isasyncgen(data):
                        # 命名 mem 不接 VFS 流写；先收敛成完整 payload。
                        data = await self._exhaust_stream(data)

                    # === v4 流式分支：data 是 asyncgen = 流（自动，零 kwarg）===
                    # 归属：粒度 = 源（yield 边界）；驱动 = stream_write（延迟一拍：
                    # 前 N-1 笔真实内容 suppress，末笔真实内容非 suppress → hook
                    # 恰好一次——真实内容必触发，不依赖空写/剪枝路径；文档零污染）。
                    if mem_slot is None and inspect.isasyncgen(data) and op in ("+>", "/>"):
                        stream_run_id = new_run_id()
                        stream_status = "completed"
                        stream_reason = None
                        stream_batches = 0
                        stream_bytes = 0

                        def _wl(_op, _text, _suppress):
                            self.workspace.write_locator(
                                atom_ctx.wt_uid, vfs_path, wsel, _op,
                                _text, auto_scaffold, god_mode, auto_level,
                                _suppress, proj=proj,
                            )

                        try:
                            stream_status, stream_reason, stream_batches, stream_bytes = \
                                await stream_write(data, op, _wl)
                        except asyncio.CancelledError:
                            stream_status = "cancelled"
                            stream_reason = "cancelled"
                            raise
                        finally:
                            # 流表终态记录（记录自身 suppress——系统区防涟漪）
                            write_stream_record(
                                self.workspace, atom_ctx.wt_uid, stream_run_id,
                                stream_status, stream_reason,
                                meta={"target": vfs_path, "op": op,
                                      "actor": str(resolved_kwargs.get("-actor", "exec_api"))},
                                batches=stream_batches, total_bytes=stream_bytes,
                            )
                        data = None
                        if stream_status == "completed":
                            last_write_info = (f"- mdio_success: {op} {vfs_path} "
                                               f"(stream completed: {stream_batches} batches, {stream_bytes} chars)")
                        else:
                            _why = f": {stream_reason}" if stream_reason else ""
                            last_write_info = (f"- mdio_error: {op} {vfs_path} "
                                               f"(stream {stream_status} after {stream_batches} batches{_why})")
                        continue
                    elif inspect.isasyncgen(data):
                        payload_base = await self._exhaust_stream(data)
                    else:
                        if data is None: payload_base = ""
                        else:
                            if isinstance(data, (dict, list)): payload_base = json.dumps(data, ensure_ascii=False)
                            else:
                                payload_base = str(data)
                                # 注意：不可用 unquote 替代 —— 它的 strip() 会吃掉 payload 尾部换行，
                                # 破坏 "# APPEND\n" 与 ~> SEARCH 块边界（全测 test_append_content/P4 抓到）。
                                if payload_base.startswith('"') and payload_base.endswith('"'):
                                    try: payload_base = ast.literal_eval(payload_base)
                                    except: payload_base = payload_base[1:-1]

                    payload = _format_payload_sync(payload_base, op)

                    if mem_slot is not None:
                        mem_slot.write(wsel, op, payload, proj)
                        last_write_info = f"- mdio_success: {op} {raw_file}"
                    else:
                        self.workspace.write_locator(atom_ctx.wt_uid, vfs_path, wsel, op, payload, auto_scaffold, god_mode, auto_level, proj=proj)
                        last_write_info = f"- mdio_success: {op} {vfs_path}"

                    if passthrough:
                        data = payload_base
                    else:
                        data = None

            except MdioError as e:
                return e.to_markdown()
            except Exception as e:
                err = MdioError("interpreter", ErrCode.INTERP_CRASH, f"管道解析级宕机: {str(e)}", cause=e)
                return err.to_markdown()
            # finally:
            #     elapsed = time.perf_counter() - t_atom_start
            #     stat = INTERP_PROFILER[f"[Interp_Atom] {atom_type}"]
            #     stat["count"] += 1
            #     stat["time"] += elapsed

        if data is None and last_write_info:
            return last_write_info
        return data

    async def _evaluate_args(self, args_or_kwargs: Any, current_data: Any, ctx: RuntimeContext) -> Any:
        # 1. 拦截原生流 (Fast Path)
        if args_or_kwargs is PIPE_DATA:
            if hasattr(type(current_data), '__aiter__'):
                current_data = await self._exhaust_stream(current_data)
            return current_data

        # 2. 抛弃耗时的 hasattr/.__name__ 匹配，直接拥抱 Python 极速的实例检索
        if isinstance(args_or_kwargs, MacroToken):
            return await self._resolve_macro(args_or_kwargs.name, ctx)

        # $($inner) — ReadOp with ExecBlock as selector → execute inner and return result
        if isinstance(args_or_kwargs, ReadOp) and isinstance(args_or_kwargs.selector, ExecBlock):
            return await self.run([args_or_kwargs.selector.pipeline], ctx)

        if isinstance(args_or_kwargs, ExecBlock):
            return await self.run([args_or_kwargs.pipeline], ctx)

        # FuncCall in args: pass through (not execute). Allows `worktree bfork ...`
        # where `bfork` is a nested FuncCall that the outer function handles.
        if isinstance(args_or_kwargs, FuncCall):
            return args_or_kwargs

        if isinstance(args_or_kwargs, Op):
            return await self.run([Pipeline(atoms=[args_or_kwargs])], ctx)

        if isinstance(args_or_kwargs, list):
            return [await self._evaluate_args(x, current_data, ctx) for x in args_or_kwargs]
            
        if isinstance(args_or_kwargs, dict):
            return {k: await self._evaluate_args(v, current_data, ctx) for k, v in args_or_kwargs.items()}
            
        if hasattr(args_or_kwargs, 'value'):
            return args_or_kwargs.value
            
        return args_or_kwargs

    # async def _evaluate_args(self, args_or_kwargs: Any, current_data: Any, ctx: RuntimeContext) -> Any:
    #     if args_or_kwargs is PIPE_DATA:
    #         if inspect.isasyncgen(current_data):
    #             current_data = await self._exhaust_stream(current_data)
    #         return current_data

    #     # 必须显式拦截并求值，防止未展开的对象漏入下游插件
    #     if hasattr(args_or_kwargs, 'name') and type(args_or_kwargs).__name__ == "MacroToken":
    #         return await self._resolve_macro(args_or_kwargs.name, ctx)

    #     if isinstance(args_or_kwargs, ExecBlock):
    #         return await self.run([args_or_kwargs.pipeline], ctx)

    #     if isinstance(args_or_kwargs, ReadOp) and isinstance(args_or_kwargs.selector.md_path, ExecBlock):
    #         return await self.run([args_or_kwargs.selector.md_path.pipeline], ctx)

    #     if isinstance(args_or_kwargs, Op):
    #         return await self.run([Pipeline(atoms=[args_or_kwargs])], ctx)

    #     if isinstance(args_or_kwargs, list):
    #         return [await self._evaluate_args(x, current_data, ctx) for x in args_or_kwargs]
            
    #     if isinstance(args_or_kwargs, dict):
    #         return {k: await self._evaluate_args(v, current_data, ctx) for k, v in args_or_kwargs.items()}
            
    #     if hasattr(args_or_kwargs, 'value'):
    #         return args_or_kwargs.value
            
    #     return args_or_kwargs

    def _parse_aider_patch(self, payload: str) -> str:
        if not payload or "<<<<" not in payload: return payload
        blocks = []
        for part in payload.split("<<<<")[1:]:
            if "====" in part and ">>>>" in part:
                old_part, rest = part.split("====", 1)
                new_part, _ = rest.split(">>>>", 1)
                blocks.append({"old": old_part.strip("\r\n"), "new": new_part.strip("\r\n")})
        return json.dumps(blocks, ensure_ascii=False) if blocks else payload

    def _is_worktree_name_candidate(self, raw_file: Optional[str]) -> bool:
        file_raw = str_or(raw_file)   # Q12
        return bool(
            file_raw
            and not file_raw.startswith("/")
            and not file_raw.startswith(".")
            and "/" not in file_raw
            and not file_raw.endswith(".md")
        )

    def _apply_worktree_name(self, raw_file: str, ctx: RuntimeContext, selector) -> tuple[RuntimeContext, str, str]:
        file_raw = str_or(raw_file)   # Q12
        if not self._is_worktree_name_candidate(file_raw):
            return ctx, raw_file, ctx.resolve_path(raw_file)

        try:
            wt_entry = self.workspace.resolve_worktree_entry(file_raw)   # Q8：唯一实现
        except Exception:
            wt_entry = None
        if wt_entry is None:
            return ctx, raw_file, ctx.resolve_path(raw_file)

        next_ctx = replace(ctx, wt_uid=wt_entry.get("uid", ""), vfs_path="/",
                          worktree_name=file_raw)
        # SelectorIR 是 dataclass，不能原地修改；调用方不需要修改 selector
        try:
            vfs_path = next_ctx.resolve_path("")
        except MdioError:
            vfs_path = next_ctx.resolve_path("/")
        return next_ctx, "", vfs_path

    async def _resolve_macro(self, raw_name: str, ctx: RuntimeContext) -> str:
        # print(f'[_resolve_macro] raw_name: {raw_name},  ctx: {str(ctx)}')
        name = raw_name.strip()
        if name in ("TIME", "WT_UID", "wt_uid", "PWD", "pwd"):
            mapping = {
                "TIME": str(int(time.time())),
                "WT_UID": str(ctx.wt_uid),
                "wt_uid": str(ctx.wt_uid),
                "PWD": str(ctx.vfs_path),
                "pwd": str(ctx.vfs_path)
            }
            return mapping[name]
        
        is_global = name.isupper()
        target_wt = "genesis" if is_global else ctx.wt_uid
        target_vfs = "/.mds/env.md"
        head_name = "SYSTEM_ENV" if is_global else "LOCAL_ENV"
        sel = by_tag(head_name, name)

        try:
            res = self.workspace.read_locator(target_wt, target_vfs, sel)
            if res and isinstance(res, list) and len(res) > 0:
                return unquote_plain(str(res[0]).strip('\r\n'))   # Q7①
        except Exception:
            pass
            
        raise MdioError("interpreter", ErrCode.INTERP_CRASH, f"引擎遭遇法则阻断: 未查找到宏 %{name}")

    async def _switch_prefix_context(self, selector_ctx: Any, current_ctx: RuntimeContext) -> RuntimeContext:
        # if DEBUG: print(f"[DEBUG _switch_prefix_context] selector_ctx = {selector_ctx}, type = {type(selector_ctx)}")
        if selector_ctx is None:
            return current_ctx

        target_wt = "genesis"
        wt_name = current_ctx.worktree_name
        if isinstance(selector_ctx, int):
            target_wt = str(selector_ctx)
        elif hasattr(selector_ctx, 'name'):  # MacroToken
            val_str = await self._resolve_macro(selector_ctx.name, current_ctx)
            target_wt = val_str
        elif isinstance(selector_ctx, str):
            try:
                wt_entry = self.workspace.resolve_worktree_entry(selector_ctx)   # Q8：唯一实现，单次查
            except Exception:
                wt_entry = None
            if wt_entry is not None:
                target_wt = wt_entry.get("uid", "")
                wt_name = selector_ctx
            else:
                target_wt = selector_ctx
        else:
            target_wt = current_ctx.wt_uid

        new_ctx = replace(current_ctx, wt_uid=target_wt, vfs_path="/", active_doc=None,
                          worktree_name=wt_name)
        # if DEBUG: print(f"[DEBUG _switch_prefix_context] new_ctx.wt_uid = {new_ctx.wt_uid}")
        return new_ctx