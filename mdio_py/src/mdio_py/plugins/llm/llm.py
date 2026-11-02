"""
LLM Plugin Module
"""

import ast
import asyncio
import json
import re
import time
import traceback
import contextvars
from pathlib import Path
from typing import Any, AsyncGenerator
from dataclasses import asdict

# 库相关的一切都在适配层；本文件不出现任何库名（可 grep 守）
from .backend import (
    DEFAULT_BACKEND,
    infer_provider,
    precheck_capabilities,
    KIND_AUTH,
    KIND_CONFIG,
    KIND_CONTENT_FILTER,
    KIND_CONTEXT_LENGTH,
    KIND_FORBIDDEN,
    KIND_INSUFFICIENT_FUNDS,
    KIND_INVALID_REQUEST,
    KIND_NOT_FOUND,
    KIND_RATE_LIMIT,
    LLMErrorInfo,
    LLMRequest,
    make_backend,
)

from ..plugin_base import PluginBase, Colors
from . import arrow_handoff
from . import providers as provider_ops
from mdio_py.plugins import info as plugin_info
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.text_utils import unquote_eval
from mdio_py.core.ast_utils import inline_text, walk
from mdio_py.core.error import MdioError, ErrCode, is_error_text
from mdio_py.core.selector_builder import SelectorBuilder, by_tag, drill
from mdio_py.core.mdio_ir import SelectorIR

DEBUG = False
V2_DEBUG = False  # True → 打印 _step 状态机的所有 _print_info 日志

# 系统默认模型与 XBT 默认 driver 的共享常量（CLI 也使用同一字符串）。
DEFAULT_ALIAS = "DEFAULT"
XBT_DRIVER = "XBT_DIRECTOR"
# 相对仓库根的 sysp 指针；_read_config 用 Path(__file__) 的仓库根解析。
DEFAULT_XBT_SYSP_FILE = "mdio_py/src/mdio_py/plugins/xbt/default_llm_driver_sysp.md"

# ========== 打印控制 ==========
SHORT_PRINT = False               # True 时截断过长内容
_PRINTED_MODELS = set()           # 记录已打印过模型信息的 agent
_PRINTED_MODEL_NAME = set()       # 记录已打印过的模型名称

_current_step_agent: contextvars.ContextVar[str | None] = contextvars.ContextVar("_current_step_agent", default=None)
_current_step_wl: contextvars.ContextVar[str | None] = contextvars.ContextVar("_current_step_wl", default=None)
_current_tc_id: contextvars.ContextVar[str | None] = contextvars.ContextVar("_current_tc_id", default=None)

def _fmt(text: str, max_len: int = 200) -> str:
    """截取字符串，用于 SHORT_PRINT=True 时"""
    if not SHORT_PRINT:
        return text
    if len(text) > max_len:
        return text[:max_len] + "..."
    return text

def _print_user(content: str):
    print(f"{Colors.GREEN}[user]{Colors.RESET} {_fmt(content)}")

def _print_assistant(content: str):
    print(f"{Colors.BLUE}[assistant]{Colors.RESET} {_fmt(content)}")

def _print_thinking(content: str):
    print(f"{Colors.CYAN}[thinking]{Colors.RESET} {_fmt(content)}")

def _print_tool_call(name: str, args_raw: str, tool_id: str):
    """打印工具调用，使用背景色高亮"""
    # 第一行：标签和工具名称（背景色）
    print(f"{Colors.BG_MAGENTA}{Colors.WHITE}[tool_call]{Colors.RESET} {Colors.BG_MAGENTA}{Colors.WHITE}{name} id={tool_id}{Colors.RESET}")
    # 尝试提取 pipeline 字段进行美化
    try:
        args_obj = json.loads(args_raw)
        pipeline = args_obj.get("pipeline", "")
        if pipeline:
            # pipeline 行也使用背景色
            print(f"  {Colors.BG_MAGENTA}{Colors.WHITE}pipeline:{Colors.RESET} {_fmt(pipeline)}")
        else:
            if DEBUG:
                print(f"  {Colors.BG_MAGENTA}{Colors.WHITE}raw_args:{Colors.RESET} {_fmt(repr(args_raw))}")
    except:
        if DEBUG:
            print(f"  {Colors.BG_MAGENTA}{Colors.WHITE}raw_args:{Colors.RESET} {_fmt(repr(args_raw))}")

def _print_tool_result(content: str):
    """打印工具执行结果，错误时使用红色"""
    if is_error_text(content):
        _print_error(content)
    else:
        # 对过长的结果进行截断
        display = _fmt(content, max_len=500) if SHORT_PRINT else content
        print(f"{Colors.YELLOW}[tool_res]{Colors.RESET} {display}")

def _print_info(msg: str):
    print(f"{Colors.WHITE}[info]{Colors.RESET} {msg}")

def _v2_info(msg: str):
    """_step 状态机日志 — 仅 V2_DEBUG=True 时输出。"""
    if V2_DEBUG:
        _print_info(msg)

def _print_error(msg: str):
    print(f"{Colors.RED}[error]{Colors.RESET} {msg}")

def _print_warn(msg: str):
    print(f"{Colors.YELLOW}[warn]{Colors.RESET} {msg}")

def _agent_names_in_agents_section(workspace, wt_uid: str) -> list[str]:
    """读 `llm.md` 的 AGENTS 段 → agent 名列表（`## <name>` 标题）。

    读法：引擎投影（`to_json_text`）→ 取 `lv == 2` 的标题；标题 `t` 可能是行内节点数组
    （名字含 `_` 等标记字符时），故用 `inline_text` 归一。任何异常退回空列表（调用方语义不变）。
    """
    try:
        chunks = workspace.read_section_chunks(wt_uid, "/.mds/llm.md", "AGENTS", children=True)   # Q11
        text = "\n".join(str(c) for c in (chunks or []) if c)
        if not text.strip():
            return []
        names = []
        for n in walk(json.loads(workspace.to_json_text(text))):
            if n.get("nt") == "h" and n.get("lv") == 2:
                name = inline_text(n.get("t")).strip()
                if name:
                    names.append(name)
        return names
    except Exception as e:
        _print_warn(f"AGENTS 名提取失败（退回无 agent）：{e}")
        return []

_PRECHECK_DONE = set()             # (provider, 键集) → 已提示过（避免每轮重复）

def _precheck_config(secrets: dict) -> list[str]:
    """能力预检（advisory，不阻断调用）：把 model/base_url/api_key 之外的键当可预检的能力键。

    - 按 (provider, 键名集合) 记忆化：首次会付一次 any-llm 导入（数百 ms），之后可忽略；
    - 同一份配置只返回一次消息：本函数在每轮 _handle_busy 都会被经过，重复提示只是噪声。
    """
    try:
        from .backend import precheck_capabilities
        sig = (str(secrets.get("provider") or ""), tuple(sorted(str(k) for k in secrets)))
        if sig in _PRECHECK_DONE:
            return []
        _PRECHECK_DONE.add(sig)
        return precheck_capabilities(secrets)
    except Exception:
        return []

def _print_separator():
    print(f"{Colors.WHITE}{'─' * 60}{Colors.RESET}")



# ========== 后端与错误分类（一律经适配层 .backend） ==========

_BACKEND_SINGLETONS: dict[str, Any] = {}


def _backend_for(name: str | None):
    """按名字取后端单例；name 为空时取 DEFAULT_BACKEND（见 .backend）。

    后端本身无状态，实例只为省对象创建；客户端/会话缓存都在适配层内部。
    """
    key = (name or DEFAULT_BACKEND).strip().lower() or DEFAULT_BACKEND
    if key not in _BACKEND_SINGLETONS:
        _BACKEND_SINGLETONS[key] = make_backend(key)
    return _BACKEND_SINGLETONS[key]


def _is_retryable(e: Exception, backend=None) -> bool:
    """可重试判定：交给适配层归一化后的 retryable（不再依赖任何库的类名）。"""
    return (backend or _backend_for(None)).normalize(e).retryable


def _format_llm_error(e: Exception, model_name: str, backend=None) -> str:
    """把异常格式化成标准 mdio_error 字符串（先经适配层归一化）。"""
    return _format_error_info((backend or _backend_for(None)).normalize(e), model_name)


def _format_error_info(info: LLMErrorInfo, model_name: str) -> str:
    """统一错误文案（对外契约，改文案要同步测试）。"""
    if info.kind == KIND_AUTH:
        return f"- mdio_error: [LLM] auth: {model_name} — check API key"
    if info.kind == KIND_FORBIDDEN:
        return f"- mdio_error: [LLM] forbidden: {model_name}"
    if info.kind == KIND_NOT_FOUND:
        return f"- mdio_error: [LLM] not found: model '{model_name}'"
    if info.kind == KIND_CONTEXT_LENGTH:
        return "- mdio_error: [LLM] context window exceeded: shorten input or history"
    if info.kind == KIND_INVALID_REQUEST:
        detail = info.message.split(":", 1)[-1].strip() if ":" in info.message else info.message
        suffix = f" (param={info.param})" if info.param else ""
        return f"- mdio_error: [LLM] bad request: {detail}{suffix}"
    if info.kind == KIND_CONTENT_FILTER:
        return f"- mdio_error: [LLM] content policy: {info.message}"
    if info.kind == KIND_INSUFFICIENT_FUNDS:
        return f"- mdio_error: [LLM] insufficient funds: {model_name}"
    if info.kind == KIND_CONFIG:
        return f"- mdio_error: [LLM] config: {info.message}"
    return f"- mdio_error: [LLM] {info.message}"


# 子 agent 调用（异步）：@LLM.<agent>.run / .stream
_ASYNC_LLM_CALL_RE = re.compile(r"@LLM\.[A-Za-z0-9_\-]+\.(run|stream)\b")

# agent 级可覆盖的请求项（别名文件 llm_config.md 管模型与密钥，agent 配置管"走哪个后端/端点"）
_REQUEST_OVERRIDE_KEYS = ("backend", "provider", "timeout", "proxy")


def _merge_request_overrides(secrets: dict, config: dict) -> dict:
    """把 agent 配置里的 backend/provider/timeout/proxy 并进 secrets（非空才覆盖）。

    没有这一步，`.init -provider …` / 配置更新写的键就是空转：`_infer_and_stream`
    只看 secrets，而 secrets 只来自模型别名文件。
    """
    merged = dict(secrets or {})
    for k in _REQUEST_OVERRIDE_KEYS:
        v = (config or {}).get(k)
        if v is not None and str(v).strip() != "":
            merged[k] = v
    return merged


def _is_async_llm_call(pipeline: str) -> bool:
    """管道里是否调用了子 agent 的 `.run`/`.stream`（异步 ⇒ 结果由子 agent 自己回写）。

    旧实现是 `".run" not in pipeline` 的子串判定：只要管道里任何位置出现 ".run"
    （例如 `$ /logs/..run_x`、字符串字面量）就被当成异步调用，于是这里**不写 tool_result**，
    父 agent 永远停在 WAIT_TOOL。结构化判据要求 ".run"/".stream" 挂在 `@LLM.<agent>` 引用上。
    """
    return bool(_ASYNC_LLM_CALL_RE.search(pipeline or ""))


# ========== 共享 agent 初始化 / sysp 路径 ==========


def _project_root() -> Path:
    """仓库根；开发态用它解析相对 sysp 路径。"""
    p = Path(__file__).resolve()
    return p.parents[5] if len(p.parents) > 5 else Path.cwd()


def _resolve_sysp_path(raw: str) -> Path:
    p = Path(str(raw or "")).expanduser()
    if not p.is_absolute():
        p = _project_root() / p
    return p


def _has_default_alias(workspace) -> bool:
    try:
        return bool(workspace.read_section_chunks(
            "genesis", "/.mds/llm_config.md", "LLM_CONFIG." + DEFAULT_ALIAS))
    except Exception:
        return False


def _write_agent_fields(workspace, agent_name: str, fields: dict) -> None:
    """按 agent 写字段；init 与懒注册共用。"""
    workspace.write_locator(
        "genesis", "/.mds/llm.md", drill("AGENTS"), "+>", "", auto_scaffold=True)
    for key, value in fields.items():
        workspace.write_locator(
            "genesis", "/.mds/llm.md",
            by_tag(f"AGENTS.{agent_name}", key),
            "/>", str(value), auto_scaffold=True,
        )


def _init_agent(workspace, ctx, agent_name: str, fields: dict) -> None:
    """init 的创建逻辑：写字段 + 建 main 实例 proc/hook。"""
    _write_agent_fields(workspace, agent_name, fields)
    _ensure_wl_hook(workspace, agent_name, "main", ctx)


# ========== 原插件类 ==========


def _input_text(input_data) -> str:
    """管道输入 → 消息文本：None / 空 → 空串。

    不能直接 `str(input_data)`：无输入时 input_data 是 None，`str(None)` 会得到
    "None" 字面量，regen（空输入 .stream）会把 `- user: > None` 写进 HIS。
    """
    if input_data is None:
        return ""
    return str(input_data).strip()


class LLMPlugin(PluginBase):
    """@LLM 插件：有状态智能体，HIS 以「记录 + 字段 + ic 字面区载荷」落盘。

    一条记录 = 列 0 的 bullet（tag = 角色，值恒空）；字段 = 缩进 2 的子 bullet；
    载荷 = 引用容器里的**缩进代码块**（每行 `  > ` + 4 空格 + 原文，空行裸 `  >`）
    ——字面区让载荷里的 md 标记不被解析，读侧 `ic.cd` 即原文。
    写走 _append_record（记录 + 字段 + 载荷一笔落盘），读走 _parse_his_records（一次 tojson）；
    工具调用的配对键是 tool_call / tool_result 的 id 字段，参数名在 `- tool:` 字段。
    规范形与实测记录见 /his_bq_scheme.md §10。
    """

    @property
    def protocol_name(self) -> str: return "LLM"

    # === v2 状态机类属性 ===
    _step_locks: dict[tuple, "asyncio.Lock"] = {}
    _done_events: dict[tuple, "asyncio.Event"] = {}
    _step_tasks: dict[tuple, "asyncio.Task"] = {}
    _stream_queues: dict[tuple, "asyncio.Queue"] = {}
    _instances: dict[tuple, dict] = {}
    _branch_locks: dict[tuple, "asyncio.Lock"] = {}

    def _safe_unquote(self, val: Any) -> str:
        return unquote_eval(val)   # Q7 档位④（llm 旧档：literal_eval；非 str⇒str，None⇒"None"）

    # _get_god_secrets 统一由 PluginBase 提供（整节一次读出：节原文 → tojson → 消费侧提取）。
    # 键表不再是白名单 —— 白名单外的键一律保留，交给 backend.precheck_capabilities 预检。

    async def _get_god_secrets(self, workspace, model_alias: str) -> dict:
        """读取别名配置并并入 PROVIDERS 默认值（plan D9）。

        基类负责整节读取；这里只加一层 provider 默认值合并与 api_key 解析。
        任何异常都退回基类结果，不让设置/预检变成新的失败点。
        """
        try:
            cfg = await self._raw_alias_config(workspace, model_alias)
            return provider_ops.merge_alias_config(workspace, cfg)
        except Exception:
            return {}

    async def _raw_alias_config(self, workspace, model_alias: str) -> dict:
        """读别名节；缺失且形如 "provider/model" 时按无别名直连模式构造配置。

        无别名直连：m = "provider/model"（如 deepseek/deepseek-v4-flash）时直接用
        PROVIDERS.<provider> 作为连接默认值，不再要求 LLM_CONFIG 里存在别名节。
        """
        cfg = await super()._get_god_secrets(workspace, model_alias)
        if not cfg and "/" in str(model_alias or ""):
            prov, model = str(model_alias).split("/", 1)
            cfg = dict(provider_ops.read_provider_section(workspace, prov.strip()))
            cfg["provider"] = prov.strip()
            cfg["model"] = model.strip()
        return cfg

    async def _ensure_llm_md(self, workspace, wt_uid: str):
        """懒创建 /.mds/llm.md（Rust 不再预创建）"""
        try:
            if not workspace.path_exists(wt_uid, "/.mds/llm.md"):
                workspace.mk(wt_uid, "/.mds/llm.md", parents=False, verbose=False)
        except Exception:
            pass

    async def _agent_exists(self, workspace, agent_name: str, ctx: RuntimeContext) -> bool:
        try:
            return len(workspace.read_section_chunks("genesis", "/.mds/llm.md", f"AGENTS.{agent_name}")) > 0   # Q11(管理面恒 genesis)
        except Exception:
            return False

    async def _ensure_xbt_director(self, workspace, ctx: RuntimeContext) -> str:
        """懒注册 XBT_DIRECTOR；复用 init 的字段写入 + main proc/hook。"""
        await self._ensure_llm_md(workspace, "genesis")
        if not _has_default_alias(workspace):
            return ("- mdio_error: XBT_DIRECTOR is missing provider/secret configuration.\n"
                    "  Run: mdio llm config <provider> <secret>\n"
                    "  Or edit /.mds/llm_config.md (# PROVIDERS / # LLM_CONFIG::DEFAULT).")
        fields = {
            "m": DEFAULT_ALIAS,
            "sysp_file": DEFAULT_XBT_SYSP_FILE,
            "tool": "true",
            "his": "HIS",
            "retry": "1",
            "backend": DEFAULT_BACKEND,
        }
        _init_agent(workspace, ctx, XBT_DRIVER, fields)
        return ""

    async def _infer_and_stream(self, messages, secrets, is_stream: bool,
                                tools=None, num_retries: int = 1) -> AsyncGenerator:
        """调用模型并产出事件；**所有库相关细节都在 .backend 适配层里**。

        事件契约（状态机与测试共同依赖，别改）：
            ("content", str) / ("thinking", str) / ("tool", {id,pipeline,name,raw_args}) / ("error", str)
        """
        backend = _backend_for(secrets.get("backend"))
        model_name = secrets.get("model", "unknown")
        if model_name not in _PRINTED_MODEL_NAME:
            print(f"{Colors.BLUE}Model:{Colors.RESET} {model_name}")
            _PRINTED_MODEL_NAME.add(model_name)

        extra_body_raw = secrets.get("extra_body", {})
        if isinstance(extra_body_raw, str):
            try:
                extra_body_raw = json.loads(extra_body_raw)
            except Exception:
                extra_body_raw = {}

        req = LLMRequest(
            model=model_name,
            messages=messages,
            api_key=secrets.get("api_key", "") or "",
            api_base=secrets.get("base_url") or None,
            provider=secrets.get("provider") or None,
            extra_body=dict(extra_body_raw or {}),
            reasoning_effort=secrets.get("reasoning_effort") or None,
            tools=tools or None,
            stream=is_stream,
            timeout=float(secrets.get("timeout") or 180.0),
            proxy=secrets.get("proxy") or None,
        )

        def _tool_event(tc) -> tuple:
            """适配层的 ToolCallData → 插件事件形态（含参数名 / 参数值解析）。"""
            args_raw = tc.arguments or ""
            name = tc.name or "execute_mdio"
            _print_tool_call(name, args_raw, tc.id)
            try:
                obj = json.loads(args_raw)
                pipeline = obj.get("pipeline", "") if isinstance(obj, dict) else args_raw
                # 单参数（现在只有 pipeline）⇒ 参数名入 `- tool:` 字段；多参数 ⇒ 留空，
                # 载荷退回整串 JSON（读侧见 `_tc_arguments`）
                tool_name = ("pipeline" if isinstance(obj, dict)
                             and len(obj) == 1 and "pipeline" in obj else "")
            except Exception:
                pipeline = args_raw
                tool_name = ""
            return ("tool", {"id": tc.id, "pipeline": pipeline, "name": name,
                             "raw_args": args_raw, "tool": tool_name})

        last_error: LLMErrorInfo | None = None
        stream_failed = False

        for attempt in range(num_retries + 1):
            use_stream = is_stream and not stream_failed

            if use_stream:
                assistant_prefix_printed = False
                assistant_buffer = ""
                thinking_buffer = ""
                errored: LLMErrorInfo | None = None
                tool_data = None
                async for ev in backend.stream(req):
                    if ev.kind == "thinking":
                        thinking_buffer += ev.text
                        if ev.text:
                            yield ("thinking", ev.text)
                    elif ev.kind == "content":
                        if not assistant_prefix_printed and ev.text.strip():
                            print(f"{Colors.BLUE}[assistant]{Colors.RESET} ", end="", flush=True)
                            assistant_prefix_printed = True
                        print(ev.text, end="", flush=True)
                        assistant_buffer += ev.text
                        yield ("content", ev.text)
                    elif ev.kind == "tool":
                        tool_data = ev.tool
                    elif ev.kind == "error":
                        errored = ev.error

                if errored is not None and not (assistant_buffer or thinking_buffer):
                    # 还没收到任何内容就断了：沿用旧行为——退化成非流式再试
                    last_error = errored
                    if attempt < num_retries:
                        stream_failed = True
                        continue
                    yield ("error", _format_error_info(errored, model_name))
                    return
                if assistant_prefix_printed:
                    print()
                if thinking_buffer:
                    _print_thinking(thinking_buffer)
                if tool_data is not None:
                    yield _tool_event(tool_data)
                return

            # 非流式
            try:
                resp = await backend.complete(req)
            except asyncio.CancelledError:
                raise
            except Exception as e:                       # noqa: BLE001 - 统一走归一化
                info = backend.normalize(e)
                last_error = info
                if info.retryable and attempt < num_retries:
                    await asyncio.sleep(min(2 ** attempt, 30))
                    continue
                if not info.retryable:
                    yield ("error", _format_error_info(info, model_name))
                    return
                yield ("error", f"- mdio_error: [LLM] retry exhausted "
                                f"({num_retries + 1}x): {info.message}")
                return

            if resp.reasoning:
                _print_thinking(resp.reasoning)
            yield ("thinking", resp.reasoning)
            if resp.content:
                _print_assistant(resp.content)
                yield ("content", resp.content)
            if resp.tool_calls:
                yield _tool_event(resp.tool_calls[0])
            return

        if last_error is not None:                       # 循环用尽且未产出事件
            yield ("error", _format_error_info(last_error, model_name))

    def _build_tools(self, tool_flag) -> list:
        if str(tool_flag).lower() != "true": return []
        try: doc_text = (Path(__file__).parent / "mdio_tool_docs.md").read_text(encoding="utf-8")
        except: doc_text = ""
        return [{
            "type": "function", "function": {
                "name": "execute_mdio", "description": f"执行MDIO脚本控制系统沙箱。\n\n[MDIO Reference]\n{doc_text}",
                "parameters": {"type": "object", "properties": {"pipeline": {"type": "string"}}, "required": ["pipeline"]}
            }
        }]

    async def __call__(self, workspace, input_data: Any, chain: list, ctx: RuntimeContext, kwargs: dict) -> Any:
        # 取消语义：run 进行中把当前 task 注册进 workspace.active_tasks（.close 靠它取消），
        # 被取消时把 proc 状态写成 ERROR/CANCELLED —— 否则页面刷新/客户端中止会把 agent 永久留在 BUSY。
        run_agent = ""
        run_wl = "main"
        run_active = False
        run_task_key = ""
        try:
            if DEBUG:
                print(f"\n[DEBUG __call__] chain={chain}, input_data={input_data!r}, kwargs={kwargs}")
            # v2: Hook callback routing (必须在 chain 检查之前)
            if "-step" in kwargs:
                agent = self._safe_unquote(kwargs["-step"])
                wl_raw = kwargs.get("-wl")
                wl = self._safe_unquote(wl_raw) if wl_raw is not None else "main"
                await self._step(workspace, agent, _wl_key(wl))
                return ""

            # ========== 0a. 只读 provider 命令（无 agent 名 + 保留 flag）==========
            # 形状（走读实测）：@LLM -provider [name] → chain=[]、kwargs={"-provider": True|name}
            if not chain and any(k in kwargs for k in provider_ops.RESERVED_FLAGS):
                return await self._provider_ops(workspace, kwargs)

            # ========== 0. 无 chain = 列出所有 agent ==========
            if not chain:
                if DEBUG:
                    print("[DEBUG] -> no chain, list agents")
                _print_info("LLM Plugin Operational.")
                return await self._list_agents(workspace, ctx)

            # 提取 agent_name 和 operation
            agent_name = chain[0]
            operation = chain[1] if len(chain) > 1 else "run"
            cb_code = str(kwargs.get("-cb") or "")   # 对话结束回调：整轮正常收尾后执行一次
            if DEBUG:
                print(f"[DEBUG] agent_name={agent_name}, operation={operation}")

            # 确保 llm.md 存在（Rust 不再预创建，由插件懒初始化）
            await self._ensure_llm_md(workspace, "genesis")   # 管理面恒 genesis

            # ========== 0a. 单 branch 创建（显式；先于配置更新） ==========
            if "-branch" in kwargs:
                return await self._handle_branch_create(workspace, ctx, chain, kwargs)

            # ========== 1. 配置更新（最高优先级） ==========
            # 配置参数列表（注意键名与 kwargs 一致，均为带短横线的形式）
            # provider/backend/timeout/proxy 是 any-llm 后端需要的：允许按 agent 覆盖，
            # 未配置时后端按 base_url 推断 provider（见 .backend.infer_provider）。
            config_keys = {"-m", "-sysp", "-tool", "-his", "-retry", "-wl",
                           "-provider", "-backend", "-timeout", "-proxy"}
            has_config_update = any(k in kwargs for k in config_keys)
            if DEBUG:
                print(f"[DEBUG] has_config_update={has_config_update}")
                print(f"[DEBUG] input_data stripped empty? {not str(input_data).strip()}")
                print(f"[DEBUG] operation not in exclude? {operation not in ('init', 'stream', 'daemon', 'close')}")

            # 条件：有配置参数、无管道数据、且不是 init/stream/daemon/close 操作
            # 注意：允许 operation 为 "run" 时更新配置
            # if has_config_update and not str(input_data).strip() and operation not in ("init", "stream", "daemon", "close"):
            is_input_empty = input_data is None or str(input_data).strip() == ""
            if has_config_update and is_input_empty and operation not in ('init', 'stream', 'daemon', 'close'):
                # 确保 agent 已存在
                if DEBUG:
                    print("[DEBUG] -> entering CONFIG UPDATE branch")
                exists = await self._agent_exists(workspace, agent_name, ctx)
                if not exists:
                    _print_error(f"Agent '{agent_name}' does not exist. Use `.init` first.")
                    return f"- mdio_error: Agent '{agent_name}' does not exist. Use `.init` to create."

                # 逐个更新配置项
                for k, v in kwargs.items():
                    if k in config_keys:
                        key = k.lstrip('-')
                        value = self._safe_unquote(v)
                        if DEBUG:
                            print(f"[DEBUG] updating {key} = {value}")
                        # 写入 AGENTS.{agent_name} 下的对应 key
                        if key == "wl":
                            _write_agent_wl(workspace, agent_name, value)
                        else:
                            workspace.write_locator(
                                "genesis", "/.mds/llm.md",
                                by_tag(f"AGENTS.{agent_name}", key),
                                "/>", value, auto_scaffold=True
                            )
                _print_info(f"Agent '{agent_name}' configuration updated.")
                return f"- mdio_info: Agent '{agent_name}' configuration updated."

            # ========== 2. 统计查询（仅当 chain 长度为 1、不是操作关键字、且无管道数据） ==========
            if len(chain) == 1 and chain[0] not in ('init', 'run', 'stream', 'daemon', 'close') and is_input_empty:
                if DEBUG:
                    print("[DEBUG] -> entering STATS branch")
                return await self._list_agents(workspace, ctx, specific_agent=agent_name)

            # ========== 3. 以下为原有的 init / 对话等逻辑 ==========
            exists = await self._agent_exists(workspace, agent_name, ctx)

            # XBT 默认 driver 懒注册：复用 init helper，不手写第二套。
            if (agent_name == XBT_DRIVER and not exists
                    and operation in ("run", "stream")):
                lazy_err = await self._ensure_xbt_director(workspace, ctx)
                if lazy_err:
                    return lazy_err
                exists = await self._agent_exists(workspace, agent_name, ctx)

            # ========== 3a. close 操作 ==========
            if operation == "close":
                if not exists:
                    _print_error(f"Agent '{agent_name}' does not exist.")
                    return f"- mdio_error: Agent '{agent_name}' does not exist."
                # 禁止 agent 通过 tool_call 关闭自身
                caller_pid = getattr(ctx, "caller_pid", None) or ""
                if caller_pid.startswith(f"LLM_{agent_name}_"):
                    _print_error(f"Agent '{agent_name}' cannot close itself.")
                    return f"- mdio_error: Agent '{agent_name}' cannot close itself."
                # v2: 遍历该 agent 的所有实例（#### main / #### <wl>），逐实例取消与清理
                wl_names = await _list_instance_wls(workspace, agent_name, ctx)
                for (a, w) in list(self._instances.keys()):
                    if a == agent_name and w not in wl_names:
                        wl_names.append(w)
                n = 0
                wt_tasks = workspace.active_tasks.get(ctx.wt_uid, {})
                for wl in wl_names:
                    task = self._step_tasks.pop((agent_name, wl), None)
                    if task is not None and not task.done():
                        task.cancel()
                    prefix = f"LLM_{agent_name}_{wl}_"
                    for pid, t in list(wt_tasks.items()):
                        if pid.startswith(prefix) and not t.done():
                            t.cancel()
                            n += 1
                    state = _read_state(workspace, agent_name, wl, ctx)
                    hook_id_str = str(state.get("sys_hook_id", "0"))
                    if hook_id_str.isdigit() and int(hook_id_str) > 0:
                        try:
                            workspace.store.remove_hook(int(hook_id_str))
                        except Exception:
                            pass
                    # stop 语义收尾：状态复位 IDLE + sys_hook_id 清零（下轮 run/stream 重注册 hook），
                    # 并走 _notify_done 的公共出口给 .stream 消费者推终止哨兵 —— 否则 asyncgen 永久等待。
                    try:
                        _write_proc(workspace, agent_name, wl, _make_ctx(state),
                                    status="IDLE", sys_hook_id="0")
                        _clear_transient(workspace, agent_name, wl, _make_ctx(state),
                                         _TERMINAL_CLEAR_FIELDS)
                    except Exception:
                        pass
                    self._notify_done(agent_name, wl)
                    self._instances.pop((agent_name, wl), None)
                _print_info(f"Agent '{agent_name}' closed ({n} task(s) cancelled).")
                return f"- mdio_info: Agent '{agent_name}' closed ({n} task(s) cancelled)."

            # ========== 3b. 门卫：防止并发（按实例）==========
            if operation in ("run", "stream"):
                req_wl = self._requested_wl(input_data, kwargs)
                guard_wl = req_wl
                if guard_wl is None:
                    guard_cfg = _read_config(workspace, agent_name, ctx)
                    guard_wl = _wl_key(guard_cfg.get("wl", ""))
                prefix = f"LLM_{agent_name}_{guard_wl}_"
                live = self._instances.get((agent_name, guard_wl))
                if live is not None:
                    live_task = live.get("task")
                    if live_task is None or not live_task.done():
                        _print_error(f"Agent '{agent_name}' is busy.")
                        return f"- mdio_error: Agent '{agent_name}' is busy. Use `.close` to stop it first."
                wt_tasks = workspace.active_tasks.get(ctx.wt_uid, {})
                for pid, task in wt_tasks.items():
                    if pid.startswith(prefix) and not task.done():
                        _print_error(f"Agent '{agent_name}' is busy.")
                        return f"- mdio_error: Agent '{agent_name}' is busy. Use `.close` to stop it first."

            # 首次打印智能体基础信息（仅一次）
            global _PRINTED_MODELS
            if agent_name not in _PRINTED_MODELS:
                config = _read_config(workspace, agent_name, ctx)
                model_alias = config.get("m", "")
                if model_alias:
                    secrets = await self._get_god_secrets(workspace, model_alias)
                    if secrets:
                        model_name = secrets.get("model", model_alias)
                        _print_info(f"Agent '{agent_name}' using model: {model_name}")
                        sysp = config.get("sysp", "")
                        if sysp:
                            _print_info(f"System prompt: {sysp[:100]}{'...' if len(sysp)>100 else ''}")
                _PRINTED_MODELS.add(agent_name)

            # 初始化智能体
            if operation == "init":
                if exists:
                    _print_error(f"Agent '{agent_name}' already exists.")
                    return f"- mdio_error: Agent '{agent_name}' already exists. Drop the `.init` to update configurations."
                model = kwargs.get("-m")
                if not model:
                    if not _has_default_alias(workspace):
                        _print_error("no default model configured")
                        return ("- mdio_error: no system default model configured.\n"
                                "  Run: mdio llm config <provider> <secret>\n"
                                "  Or edit /.mds/llm_config.md (# LLM_CONFIG::DEFAULT).")
                    model = DEFAULT_ALIAS

                # tool 值标准化（默认开：LLM driver 默认带 execute_mdio）
                tool_val = kwargs.get("-tool", True)
                if tool_val is None or (isinstance(tool_val, str) and tool_val.lower() == 'true'):
                    tool_val = True
                elif isinstance(tool_val, str) and tool_val.lower() == 'false':
                    tool_val = False

                # 写入 agent 配置 + 创建 main 实例 proc/hook；与懒注册共用同一 helper。
                fields = {
                    "m": model,
                    "sysp": kwargs.get('-sysp', ''),
                    "tool": tool_val,
                    "his": kwargs.get('-his', 'HIS'),
                    "retry": kwargs.get('-retry', '1'),
                    # any-llm 后端：provider 可显式指定（缺省由 base_url 推断）
                    "provider": kwargs.get('-provider', ''),
                    "backend": kwargs.get('-backend', DEFAULT_BACKEND),
                }
                _init_agent(workspace, ctx, agent_name, fields)
                _print_info(f"Agent '{agent_name}' initialized.")
                return f"- mdio_info: Agent '{agent_name}' initialized."

            # 智能体不存在
            if not exists:
                _print_error(f"Agent '{agent_name}' does not exist.")
                return f"- mdio_error: Agent '{agent_name}' does not exist. Use `.init` to create."

            # 处理仅更新配置的情况（不传管道数据且操作不是 run/stream/daemon/close）
            # 注意：这个分支已在最前面处理，这里不再重复

            # ========== v2: stream 延期 ==========
            if operation == "stream":
                req_wl = self._requested_wl(input_data, kwargs)
                config = _read_config(workspace, agent_name, ctx)
                his_name = config.get("his", "HIS")
                wl = req_wl if req_wl is not None else _wl_key(config.get("wl", ""))
                if req_wl is not None and not _wl_gate_exists(
                        workspace, agent_name, his_name, config, ctx, wl):
                    return f"- mdio_error: [LLM] wl not found: {req_wl}"
                config["wl"] = wl
                msg_input = _input_text(input_data)
                if msg_input:
                    _print_user(msg_input)
                    _append_record(workspace, agent_name, his_name, "user", msg_input, config, ctx)
                state = _read_state(workspace, agent_name, wl, ctx)
                st = state.get("status", "IDLE")
                # 与 run 一致：上一轮收尾后状态是 DONE/ERROR，也算空闲（只在真正运行中拒绝）。
                if st not in ("IDLE", "DONE", "ERROR"):
                    return "- mdio_error: agent busy ({})".format(st)
                _ensure_wl_hook(workspace, agent_name, wl, ctx)
                turn = int(state.get("turn", "0")) + 1
                queue = asyncio.Queue()
                self._stream_queues[(agent_name, wl)] = queue
                _write_proc(workspace, agent_name, wl, ctx, turn=str(turn), status="BUSY", step="0",
                            cb=cb_code, wt=ctx.wt_uid)
                async def _stream_gen():
                    while True:
                        chunk = await queue.get()
                        if chunk is None:
                            break
                        yield chunk
                return _stream_gen()

            # ========== v2: run (event-driven) ==========
            req_wl = self._requested_wl(input_data, kwargs)
            config = _read_config(workspace, agent_name, ctx)
            his_name = config.get("his", "HIS")
            wl = req_wl if req_wl is not None else _wl_key(config.get("wl", ""))
            if req_wl is not None and not _wl_gate_exists(
                    workspace, agent_name, his_name, config, ctx, wl):
                return f"- mdio_error: [LLM] wl not found: {req_wl}"
            config["wl"] = wl

            msg_input = _input_text(input_data)
            if msg_input:
                _print_user(msg_input)
                _append_record(workspace, agent_name, his_name, "user", msg_input, config, ctx)

            state = _read_state(workspace, agent_name, wl, ctx)
            st = state.get("status", "IDLE")
            # Allow re-run when agent is in a terminal state (DONE/ERROR).
            # Terminal states mean the agent completed its previous turn
            # and is ready for a new one.  Do NOT allow if still running
            # (BUSY, WAIT_TOOL, RETRY, WAIT_HUMAN).
            if st not in ("IDLE", "DONE", "ERROR"):
                return "- mdio_error: agent busy ({})".format(st)
            _ensure_wl_hook(workspace, agent_name, wl, ctx)

            turn = int(state.get("turn", "0")) + 1

            event = asyncio.Event()
            self._done_events[(agent_name, wl)] = event
            _write_proc(workspace, agent_name, wl, ctx, turn=str(turn), status="BUSY", step="0",
                        cb=cb_code, wt=ctx.wt_uid)

            # 注册可取消任务：.close 的取消分支按实例前缀找 task。
            run_agent = agent_name
            run_wl = wl
            run_active = True
            _cur_task = asyncio.current_task()
            if _cur_task is not None:
                run_task_key = "LLM_%s_%s_run_%d" % (agent_name, wl, id(_cur_task))
                try:
                    workspace.active_tasks.setdefault(ctx.wt_uid, {})[run_task_key] = _cur_task
                    self._instances[(agent_name, wl)] = {"task": _cur_task, "hook_id": 0}
                except Exception:
                    run_task_key = ""

            parent_agent = _current_step_agent.get()
            if parent_agent:
                parent_wl = _wl_key(_current_step_wl.get() or "main")
                _write_proc(workspace, agent_name, wl, ctx,
                            caller=parent_agent, caller_wl=parent_wl)
            parent_tc = _current_tc_id.get()
            if parent_tc:
                _write_proc(workspace, agent_name, wl, ctx, tc_id=parent_tc)

            try:
                await asyncio.wait_for(event.wait(), timeout=600)
            except asyncio.TimeoutError:
                _write_proc(workspace, agent_name, wl, ctx, status="ERROR",
                    error_code="TIMEOUT", error_detail="Agent execution timed out (600s)")
                return "- mdio_error: Agent execution timed out"
            finally:
                self._done_events.pop((agent_name, wl), None)
                self._instances.pop((agent_name, wl), None)
                if run_task_key:
                    try:
                        workspace.active_tasks.get(ctx.wt_uid, {}).pop(run_task_key, None)
                    except Exception:
                        pass
                    run_task_key = ""

            final_state = _read_state(workspace, agent_name, wl, ctx)
            if final_state.get("status") == "ERROR":
                return "- mdio_error: {}".format(final_state.get("error_detail", "unknown error"))

            result = _read_last_assistant(workspace, agent_name, his_name, config, ctx)
            return result if result else "- mdio_info: Agent completed with no response."

        except asyncio.CancelledError:
            # 被 .close 或客户端中止取消：收尾状态，别把 agent 留在 BUSY。
            if run_active and run_agent:
                try:
                    _write_proc(workspace, run_agent, run_wl, ctx, status="ERROR",
                        error_code="CANCELLED",
                        error_detail="run cancelled (client aborted or .close)")
                except Exception:
                    pass
            if run_task_key:
                try:
                    workspace.active_tasks.get(ctx.wt_uid, {}).pop(run_task_key, None)
                except Exception:
                    pass
            raise
        except Exception as e:
            err_msg = MdioError("plugin", ErrCode.PLG_CRASH, f"LLM 插件致命崩溃: {str(e)}", cause=e).to_markdown()
            _print_error(str(e))
            return err_msg


    # v2 Bootstrap: ensure all agents have proc.md entries + permanent Hooks

    async def _bootstrap_agents(self, workspace, wt_uid: str):
        """遍历 llm.md 中所有 agent，确保默认实例 #### main 的 proc 条目 + Hook 已注册。"""
        ctx = RuntimeContext(wt_uid="genesis")   # 管理面恒 genesis(proc/hook 都在 genesis)
        agent_names = _agent_names_in_agents_section(workspace, wt_uid)
        for name in agent_names:
            try:
                _ensure_wl_hook(workspace, name, "main", ctx)
            except Exception:
                pass

    async def _handle_branch_create(self, workspace, ctx, chain, kwargs):
        """@LLM.<agent> -wl <wl> -branch [idx] [-branch_target <path>]"""
        agent = self._safe_unquote(chain[0]) if chain else ""
        if not agent:
            return "- mdio_error: [LLM] -branch requires agent name"
        if not await self._agent_exists(workspace, agent, ctx):
            return f"- mdio_error: Agent '{agent}' does not exist."
        await self._ensure_llm_md(workspace, "genesis")
        wl_raw = kwargs.get("-wl")
        if wl_raw in (None, True, ""):
            return "- mdio_error: [LLM] -branch requires -wl <wl>"
        wl = _wl_key(self._safe_unquote(wl_raw))
        branch_arg = kwargs.get("-branch")
        target_arg = kwargs.get("-branch_target")
        config = _read_config(workspace, agent, ctx)
        lock = self._branch_locks.setdefault((agent, wl), asyncio.Lock())
        async with lock:
            res = _branch_create(workspace, agent, wl, branch_arg, target_arg, config, ctx)
        if isinstance(res, str):
            return res
        return json.dumps(res, ensure_ascii=False)

    def _requested_wl(self, input_data, kwargs):
        """带输入时 -wl 是本次 run 的落点；空输入仍走默认指针设置。"""
        if input_data is None or str(input_data).strip() == "":
            return None
        if "-wl" not in kwargs:
            return None
        return _wl_key(self._safe_unquote(kwargs["-wl"]))

    async def _list_agents(self, workspace, ctx, specific_agent=None):
        if specific_agent:
            agent_names = [specific_agent]
        else:
            agent_names = _agent_names_in_agents_section(workspace, "genesis")

        if not agent_names:
            return "- mdio_info: No agents initialized yet."

        output_lines = []
        for name in agent_names:
            config = _read_config(workspace, name, ctx)
            if not config:
                continue
            stats = await self._get_history_stats(workspace, name, config, ctx)
            status = self._get_agent_status(workspace, name, ctx)
            output_lines.extend(self._format_agent_stats(name, config, stats, status))
            output_lines.append('')

        if specific_agent:
            return "\n".join(output_lines).strip()
        else:
            header = "# CURRENT_AGENTS"
            return "\n".join([header] + output_lines).strip()


    # ── 只读 provider 命令（不写配置、不打印密钥）──────────────
    @staticmethod
    def _flag_str(value) -> str:
        """flag 值归一：True/None（无值）→ 空串；其余去引号转字符串。"""
        if value is True or value is None:
            return ""
        text = str(value).strip()
        if len(text) >= 2 and text[0] == text[-1] and text[0] in ('"', "'"):
            text = text[1:-1]
        return text

    @staticmethod
    def _md_bool(value) -> str:
        if value == "unknown":
            return "unknown"
        return "true" if bool(value) else "false"

    @staticmethod
    def _md_doc(title: str, blocks) -> str:
        lines = [title]
        for block in blocks:
            lines.append("")
            lines.extend(block)
        return "\n".join(lines).strip() + "\n"

    def _provider_block(self, entry: dict) -> list:
        lines = [f"## {entry.get('name', '')}"]
        for key in ("name", "class", "tier", "doc_url", "env_key", "env_api_base", "api_base", "available"):
            if key in entry:
                value = entry.get(key)
                lines.append(f"- {key}: {self._md_bool(value) if key == 'available' else value}")
        if entry.get("error"):
            lines.append(f"- error: {entry.get('error')}")
        caps = entry.get("capabilities") or {}
        if caps:
            lines.append("")
            lines.append("### capabilities")
            for name in provider_ops.CAPABILITY_FIELDS:
                lines.append(f"- {name}: {self._md_bool(caps.get(name, 'unknown'))}")
        return lines

    async def _provider_ops(self, workspace, kwargs: dict) -> str:
        """@LLM -provider | -provider <p> | -provider <p> -balance | -check <alias> | -models。"""
        provider = self._flag_str(kwargs.get("-provider", None))
        alias = self._flag_str(kwargs.get("-alias", None))
        check_alias = self._flag_str(kwargs.get("-check", None))
        want_balance = "-balance" in kwargs
        want_models = "-models" in kwargs

        # -check：合并后的有效配置 + 能力预检（不走 _precheck_config 的 memo）
        if "-check" in kwargs:
            alias = check_alias or alias
            if not alias:
                return "- mdio_error: -check requires an alias (e.g. @LLM -check DEEPSEEK_V4F)"
            raw_cfg = await self._raw_alias_config(workspace, alias)
            cfg = provider_ops.merge_alias_config(workspace, raw_cfg)
            prov = str(cfg.get("provider") or infer_provider(
                cfg.get("base_url") if isinstance(cfg.get("base_url"), str) else None) or "")
            source = provider_ops.api_key_source(workspace, raw_cfg)
            try:
                messages = precheck_capabilities(cfg, prov or None)
            except Exception:
                messages = []
            head = [f"## {alias}"]
            for key in ("model", "provider", "base_url", "reasoning_effort"):
                head.append(f"- {key}: {cfg.get(key, '')}")
            head.append(f"- api_key_source: {source}")
            extra = cfg.get("extra_body")
            if extra:
                head.append(f"- extra_body: {json.dumps(extra, ensure_ascii=False)}")
            blocks = [head, ["## messages"] + ([f"- {m}" for m in messages] or ["- (无预警)"])]
            return self._md_doc("# CHECK", blocks)

        # -balance：先解析 provider/凭据，再走 provider_ops.fetch_balance
        if want_balance:
            cfg: dict = {}
            if alias:
                cfg = await self._get_god_secrets(workspace, alias)
                prov = str(cfg.get("provider") or infer_provider(
                    cfg.get("base_url") if isinstance(cfg.get("base_url"), str) else None) or "")
            else:
                prov = provider
                cfg = provider_ops.read_provider_section(workspace, prov) if prov else {}
            prov = str(prov or provider or "").strip().lower()
            if not prov:
                return "- mdio_error: -balance requires -provider <name> or -alias <alias>"
            cfg, _source = provider_ops.resolve_api_key(cfg)
            payload = await provider_ops.fetch_balance(
                provider=prov, api_key=str(cfg.get("api_key") or ""),
                api_base=str(cfg.get("base_url") or ""),
                proxy=str(cfg.get("proxy") or "") or None,
                timeout=float(cfg.get("timeout") or 20.0))
            head = [f"## {prov}"]
            for key in ("supported", "fetched_at", "is_available", "error_kind", "http_status", "doc_url"):
                if key in payload:
                    value = payload.get(key)
                    head.append(f"- {key}: {self._md_bool(value) if key in ('supported', 'is_available') else value}")
            blocks = [head]
            balances = payload.get("balances") or []
            if balances:
                lines = ["### balances"]
                for item in balances:
                    lines.append("")
                    lines.append(f"#### {item.get('currency', '') or 'UNKNOWN'}")
                    for key in ("total_balance", "granted_balance", "topped_up_balance"):
                        if key in item:
                            lines.append(f'- {key}: "{item.get(key, "")}"')
                blocks.append(lines)
            return self._md_doc("# BALANCE", blocks)

        # -models：provider 的模型 id 列表（P2）
        if want_models:
            cfg = await self._get_god_secrets(workspace, alias) if alias else (
                provider_ops.read_provider_section(workspace, provider) if provider else {})
            prov = provider or str(cfg.get("provider") or infer_provider(
                cfg.get("base_url") if isinstance(cfg.get("base_url"), str) else None) or "")
            if not prov:
                return "- mdio_error: -models requires -provider <name> or -alias <alias>"
            cfg, _source = provider_ops.resolve_api_key(cfg)
            payload = await provider_ops.list_models(
                provider=prov, api_key=str(cfg.get("api_key") or ""),
                api_base=str(cfg.get("base_url") or ""),
                proxy=str(cfg.get("proxy") or "") or None,
                timeout=float(cfg.get("timeout") or 20.0))
            head = [f"## {prov}", f"- supported: {self._md_bool(payload.get('supported'))}"]
            if payload.get("error_kind"):
                head.append(f"- error_kind: {payload.get('error_kind')}")
            blocks = [head]
            if payload.get("models"):
                blocks.append(["### models"] + [f"- id: {m}" for m in payload.get("models") or []])
            return self._md_doc("# MODELS", blocks)

        # 默认：provider 元数据（单个或全部）
        entries = [provider_ops.provider_payload(provider)] if provider else provider_ops.providers_payload()
        blocks = [self._provider_block(entry) for entry in entries if entry]
        return self._md_doc("# PROVIDERS", blocks)

    def _get_agent_status(self, workspace, agent_name: str, ctx):
        state = _read_state(workspace, agent_name, "main", ctx)
        return state.get("status", "IDLE")

    def _format_agent_stats(self, name, config, stats, status, skip_title=False):
        lines = []
        if not skip_title:
            lines.append(f"## {name}")
        lines.append(f"- model: {config.get('m', 'N/A')}")
        lines.append(f"- backend: {config.get('backend') or DEFAULT_BACKEND}")
        provider = config.get('provider') or "(auto by base_url)"
        lines.append(f"- provider: {provider}")
        lines.append(f"- retry: {config.get('retry', '1')}")
        lines.append(f"- tool: {config.get('tool', True)}")
        lines.append(f"- his: {config.get('his', 'HIS')}")
        sysp = config.get('sysp', '')
        if len(sysp) > 20:
            sysp = sysp[:20] + "..."
        lines.append(f"- sysp: {sysp}")
        lines.append(f"- msg_rounds: {stats['rounds']}")
        lines.append(f"- msg_tool_calls: {stats['tool_calls']}")
        lines.append(f"- msg_thinking: {stats['thinking']}")
        lines.append(f"- msg_total: {stats['total_msgs']}")
        lines.append(f"- chars: {stats['chars']}")
        lines.append(f"- status: {status}")
        return lines
    
    async def _get_history_stats(self, workspace, agent_name: str, config: dict, ctx):
        his_locator = config.get("his", "HIS")
        file_path, node = _split_his(his_locator)   # Q2：统一切分入口
        # 最新 HIS 结构:统计 **main 主链**(记录在 main 下,不再直接挂在 HIS)。
        if file_path:
            vfs_path = ctx.resolve_path(file_path)
            selector = node + ".main"
            uid = "genesis"
        else:
            vfs_path = f"{ctx.sys_dir()}/llm.md"
            selector = f"AGENTS.{agent_name}.{his_locator}.main"
            uid = "genesis"

        try:
            tree_json = workspace.run_tree(
                uid, vfs_path,
                drill(selector),
                depth=1, detail=1
            )
            data = json.loads(tree_json)
            nodes = data.get("nodes", [])
            if not nodes:
                entries = []
            else:
                # selector 定位到 ### HIS 这个 Head 节点，其子节点在 children 中
                root_node = nodes[0]
                entries = []
                for child in root_node.get("children", []):
                    if child.get("node_type") != "key":
                        continue
                    name = child.get("name", "")
                    if not name.startswith(":"):
                        continue
                    role = name[1:]  # 去掉前导冒号
                    chars = child.get("chars", 0)
                    entries.append((role, chars))
        except Exception as e:
            print(f"[ERROR] _get_history_stats failed: {e}")
            entries = []

        # 计数：第一遍数所有条目，第二遍只数轮次
        # （把两件事混在一个循环里，user 分支 `i = j + 1` 会跳过 user 与下一条
        #   assistant 之间的条目——thinking 正好写在那段区间，于是计数恒为 0）
        total_msgs = len(entries)
        total_chars = sum(chars for _, chars in entries)
        tool_calls = sum(1 for role, _ in entries if role == "tool_call")
        thinking = sum(1 for role, _ in entries if role == "thinking")

        rounds = 0
        i = 0
        while i < len(entries):
            if entries[i][0] == "user":
                j = i + 1
                while j < len(entries) and entries[j][0] != "assistant":
                    j += 1
                if j < len(entries):
                    rounds += 1
                    i = j + 1
                    continue
            i += 1

        return {
            "rounds": rounds,
            "tool_calls": tool_calls,
            "thinking": thinking,
            "total_msgs": total_msgs,
            "chars": total_chars,
        }


    # v2 状态机 — _step 骨架 + BUSY / WAIT_TOOL / terminal handlers

    async def _step(self, workspace, agent_name: str, wl: str):
        wl = _wl_key(wl)
        _v2_info(f"[v2:_step:ENTER] {agent_name}/{wl}")
        lock = self._step_locks.setdefault((agent_name, wl), asyncio.Lock())
        async with lock:
            task = asyncio.current_task()
            self._step_tasks[(agent_name, wl)] = task
            try:
                ctx = RuntimeContext(wt_uid="genesis")
                state = _read_state(workspace, agent_name, wl, ctx)
                status = state.get("status", "IDLE")
                config = _read_config(workspace, agent_name, ctx)
                config["wl"] = wl

                if state.get("timeout") == "1":
                    _write_proc(workspace, agent_name, wl, _make_ctx(state),
                        status="ERROR", error_code="TIMEOUT",
                        error_detail="CRON timeout guard triggered")
                    _clear_transient(workspace, agent_name, wl, _make_ctx(state), _TERMINAL_CLEAR_FIELDS)
                    self._notify_done(agent_name, wl)
                    return

                if status == "IDLE":
                    return
                elif status == "BUSY":
                    await self._handle_busy(workspace, agent_name, wl, state, config)
                elif status == "WAIT_TOOL":
                    await self._handle_wait_tool(workspace, agent_name, wl, state, config)
                elif status == "RETRY":
                    await self._handle_retry(workspace, agent_name, wl, state, config)
                elif status in ("DONE", "ERROR"):
                    self._handle_terminal_sync(workspace, agent_name, wl, state, config)
                elif status in ("WAIT_HUMAN",):
                    return
            except asyncio.CancelledError:
                _write_proc(workspace, agent_name, wl, _make_ctx(state), status="IDLE")
                _clear_transient(workspace, agent_name, wl, _make_ctx(state))
            finally:
                self._step_tasks.pop((agent_name, wl), None)

    async def _handle_busy(self, workspace, agent_name: str, wl: str, state: dict, config: dict):
        his_name = config.get("his", "HIS")
        his_text = _read_his(workspace, agent_name, his_name, config,
            _make_ctx(state))
        _v2_info(f"[v2:BUSY] RAW HIS ({len(his_text)} chars): {repr(his_text[:300])}")
        # 世界线装配:唯一实现 = arrow_handoff(单次 arrow -json + Python plan/fold)。
        # 失败只做原样 HIS 的行解析兜底(不是旧六步装配);不再有 engine 开关。
        raw_his = his_text
        wl = _wl_key(config.get("wl", ""))
        try:
            _wl_ctx = RuntimeContext(wt_uid=_his_addr(agent_name, his_name, _make_ctx(state)).wt_uid, vfs_path="/", active_doc=None)
        except Exception as _e:
            _wl_ctx = None
            _v2_info(f"[v2:handoff] ctx 构建失败: {str(_e)[:120]}")
        _t0 = time.perf_counter()
        messages = await _messages_for_turn(raw_his, wl, workspace, _wl_ctx)
        _v2_info(f"[v2:handoff] wl={wl!r} → msgs={len(messages)} "
                 f"{1000 * (time.perf_counter() - _t0):.1f}ms")
        _v2_info(f"[v2:BUSY] {agent_name} his={his_name} raw_len={len(raw_his)} msgs={len(messages)}")
        if messages:
            _v2_info(f"[v2:BUSY] msg0 role={messages[0].get('role')} content={str(messages[0].get('content',''))[:80]}")

        model_alias = config.get("m", "")
        secrets = await self._get_god_secrets(workspace, model_alias)
        if not secrets:
            _write_proc(workspace, agent_name, wl, _make_ctx(state),
                status="ERROR", error_code="NO_SECRETS",
                error_detail=f"Missing god secrets for model {model_alias}")
            self._stream_error(agent_name, wl, f"NO_SECRETS: 取不到模型别名 {model_alias} 的配置"
                                           f"（检查 /.mds/llm_config.md 的 LLM_CONFIG.{model_alias} 节点）")
            self._notify_done(agent_name, wl)
            return
        # agent 级覆盖：provider/backend/timeout/proxy。
        # 必须在"别名解析成功"之后合并——否则空 secrets 会被填成非空，绕过上面的 NO_SECRETS 守卫，
        # 拿着 model="unknown" 去调 API（只会得到一个含糊的错误）。
        secrets = _merge_request_overrides(secrets, config)
        if not secrets.get("model"):
            _write_proc(workspace, agent_name, wl, _make_ctx(state),
                status="ERROR", error_code="NO_MODEL",
                error_detail=f"别名 {model_alias} 解析不出 model（配置里缺 model 键）")
            self._stream_error(agent_name, wl, f"NO_MODEL: 别名 {model_alias} 解析不出 model（配置里缺 model 键）")
            self._notify_done(agent_name, wl)
            return

        # 能力预检（advisory）：白名单外的键当作能力键，逐个查提供者元数据；只提示、不阻断。
        for _w in _precheck_config(secrets):
            _print_warn(_w)

        if config.get("sysp") and not any(
            m["role"] == "system" for m in messages
        ):
            messages.insert(0, {"role": "system", "content": config["sysp"]})
        _v2_info(f"[v2:BUSY] final msgs={len(messages)} m0_role={messages[0]['role'] if messages else 'none'}")

        tools = self._build_tools(config.get("tool", "true"))
        plugin_info.emit("llm", agent=agent_name, wl=wl,
                         model=str(secrets.get("model", "")),
                         sysp=str(config.get("sysp") or ""),
                         tools=[str((t.get("function") or {}).get("name", ""))
                                for t in (tools or [])],
                         messages=[{"role": m.get("role", ""),
                                    "content": str(m.get("content") or "")}
                                   for m in messages])
        _v2_info(f"[v2:BUSY] LLM REQUEST: model={secrets.get('model','?')} tools={len(tools)}")
        for i, m in enumerate(messages):
            role = m.get('role','?')
            content = str(m.get('content',''))
            _v2_info(f"[v2:BUSY] LY msg[{i}] {role} [{len(content)}chars] {repr(content[:300])}")
            tc = m.get('tool_calls')
            if tc:
                for j, t in enumerate(tc):
                    fn_name = t.get('function',{}).get('name','?')
                    fn_args = t.get('function',{}).get('arguments','')
                    _v2_info(f"[v2:BUSY]   tc[{j}] {fn_name} args={fn_args[:200]}")
        retry_str = config.get("retry", "1")
        try:
            max_retries = int(retry_str)
        except (ValueError, TypeError):
            max_retries = 1
        # v2 stream: push chunks to queue if stream mode
        stream_q = self._stream_queues.get((agent_name, wl))
        if stream_q is not None:
            tool_data = None

            def _make_line_writer(role: str):
                """行缓冲写手：把 chunk 流还原成 bq 里的「一行一条」结构。

                追加契约（/his_bq_scheme.md §4.1 实测）：
                  · 非空行 -> "\n" + 行文本（前置换行，绝不把行粘在上一条后面）
                  · 空行   -> 单独 "\n"（真正的空行；它是结束嵌套 bq 的边界）
                  · 开头/末尾空行丢弃（不产生空 bq 行）
                不能把整块 chunk 直接 +>：chunk 内的 \n\n 会在追加时退化成换行，
                嵌套引用后的 ## 标题就被吞进引用（实测）。
                """
                st = {"started": False, "blank": 0, "buf": ""}

                def _emit(line: str):
                    if not line.strip():
                        st["blank"] += 1
                        return
                    if not st["started"]:
                        st["blank"] = 0
                        _append_record(workspace, agent_name, his_name, role,
                                       line, config, _make_ctx(state))
                        st["started"] = True
                        return
                    while st["blank"] > 0:
                        _append_record(workspace, agent_name, his_name, role,
                                       "\n", config, _make_ctx(state), part=True)
                        st["blank"] -= 1
                    _append_record(workspace, agent_name, his_name, role,
                                   "\n" + line, config, _make_ctx(state), part=True)

                def feed(chunk: str):
                    st["buf"] += chunk
                    while True:
                        i = st["buf"].find("\n")
                        if i < 0:
                            break
                        line = st["buf"][:i]
                        st["buf"] = st["buf"][i + 1:]
                        _emit(line)

                def eof():
                    if st["buf"]:
                        _emit(st["buf"])
                        st["buf"] = ""
                    st["blank"] = 0
                    return st["started"]

                return feed, eof

            thinking_feed, thinking_eof = _make_line_writer("thinking")
            assistant_feed, assistant_eof = _make_line_writer("assistant")
            assistant_started = False

            async for ctype, content in self._infer_and_stream(
                messages, secrets, True, tools=tools, num_retries=max_retries):
                if ctype == "thinking":
                    if content:
                        thinking_feed(content)
                elif ctype == "content":
                    if content:
                        if not assistant_started:
                            # 行缓冲只在 "\n" 触发写盘：单行（无换行）thinking 会被压到流末尾才 flush，
                            # assistant 若已多行写盘 ⇒ HIS 变成 assistant → thinking（chat 里 Reason 沉底）。
                            # 所以在 assistant 首次落盘前先收尾 thinking，保证记录顺序 thinking → assistant。
                            thinking_eof()
                            assistant_started = True
                        assistant_feed(content)
                    await stream_q.put(content)
                elif ctype == "tool":
                    tool_data = content
                    break
                elif ctype == "error":
                    # 已写的行保留；残行 flush 后再按错误收尾。
                    thinking_eof()
                    assistant_eof()
                    # 元组方言 error：裸文本会被下游 classify 当 chunk 脏写进文档
                    await stream_q.put(("error", str(content)))
                    await stream_q.put(None)
                    self._stream_queues.pop((agent_name, wl), None)
                    _write_proc(workspace, agent_name, wl, _make_ctx(state),
                        status="ERROR", error_code="LLM_CRASH", error_detail=str(content))
                    self._notify_done(agent_name, wl)
                    return
            # 末尾残行 flush；尾部空行丢弃（不制造空 bq）。
            thinking_eof()
            assistant_eof()
            if tool_data:
                _append_record(workspace, agent_name, his_name, "tool_call",
                               _tool_record_payload(tool_data), config, _make_ctx(state),
                               fields=_tool_record_fields(tool_data))
                step = int(state.get("step", "0")) + 1
                _write_proc(workspace, agent_name, wl, _make_ctx(state),
                    status="WAIT_TOOL", step=str(step))
                return
            # text response (DONE) — terminate stream
            await stream_q.put(None)
            self._stream_queues.pop((agent_name, wl), None)
            _write_proc(workspace, agent_name, wl, _make_ctx(state),
                status="DONE", completed_at=str(int(time.time())))
            _clear_transient(workspace, agent_name, wl, _make_ctx(state), _TERMINAL_CLEAR_FIELDS)
            await self._run_callback(workspace, agent_name, state.get("cb", ""), _make_ctx(state))
            self._notify_done(agent_name, wl)
            return

        try:
            # use _infer_and_stream directly (same as old working code)
            final_ans = ""
            tool_data = None
            thinking_text = ""
            async for ctype, content in self._infer_and_stream(
                messages, secrets, False, tools=tools, num_retries=max_retries
            ):
                if ctype == "thinking":
                    thinking_text = content
                elif ctype == "content":
                    final_ans += content
                elif ctype == "tool":
                    tool_data = content
                elif ctype == "error":
                    _v2_info(f"[v2:BUSY] sync LLM ERROR: {str(content)[:100]}")
                    _write_proc(workspace, agent_name, wl, _make_ctx(state),
                        status="ERROR", error_code="LLM_CRASH", error_detail=str(content))
                    _clear_transient(workspace, agent_name, wl, _make_ctx(state), _TERMINAL_CLEAR_FIELDS)
                    self._stream_error(agent_name, wl, f"LLM_CRASH: {content}")
                    self._notify_done(agent_name, wl)
                    return

            _v2_info(f"[v2:BUSY] sync-LLM-done: tool={tool_data is not None} text_len={len(final_ans)}")
            plugin_info.emit("llm_reply", agent=agent_name, wl=wl,
                             thinking=str(thinking_text or ""),
                             tool=(str(tool_data.get("name", "")) if tool_data else ""),
                             tool_args=(str(tool_data.get("pipeline", "")) if tool_data else ""),
                             reply=(final_ans if final_ans else
                                    (str(tool_data.get("pipeline", "")) if tool_data else "")))

            if tool_data:
                _v2_info(f"[v2:BUSY] TOOL path entered, writing to HIS...")
                try:
                    # write thinking/assistant/tool_call to HIS
                    if thinking_text:
                        _append_record(workspace, agent_name, his_name, "thinking",
                                       thinking_text, config, _make_ctx(state))
                    _append_record(workspace, agent_name, his_name, "assistant",
                                   final_ans, config, _make_ctx(state))
                    _append_record(workspace, agent_name, his_name, "tool_call",
                                   _tool_record_payload(tool_data), config, _make_ctx(state),
                                   fields=_tool_record_fields(tool_data))
                    step = int(state.get("step", "0")) + 1
                    gate = state.get("gate", "")
                    if gate:
                        _write_proc(workspace, agent_name, wl, _make_ctx(state),
                            status="WAIT_HUMAN", wait_reason="GATED", step=str(step))
                    else:
                        _write_proc(workspace, agent_name, wl, _make_ctx(state),
                            status="WAIT_TOOL", step=str(step))
                        _v2_info(f"[v2:BUSY] {agent_name} → WAIT_TOOL")
                except Exception as _e:
                    _print_error(f"TOOL path CRASH: {_e}")
            else:
                _v2_info(f"[v2:BUSY] TEXT branch entered, final_ans_len={len(final_ans)}")
                # text response → DONE
                _v2_info(f"[v2:BUSY] TEXT path: final_ans_len={len(final_ans)}")
                if thinking_text:
                    _append_record(workspace, agent_name, his_name, "thinking",
                                   thinking_text, config, _make_ctx(state))
                _append_record(workspace, agent_name, his_name, "assistant",
                               final_ans, config, _make_ctx(state))
                _v2_info(f"[v2:BUSY] wrote to his={his_name}, re-reading...")
                recheck = _read_his(workspace, agent_name, his_name, config, _make_ctx(state))
                _v2_info(f"[v2:BUSY] after write his_len={len(recheck)}")
                step = int(state.get("step", "0"))
                _write_proc(workspace, agent_name, wl, _make_ctx(state),
                    status="DONE", completed_at=str(int(time.time())),
                    total_steps=str(step))
                _clear_transient(workspace, agent_name, wl, _make_ctx(state),
                    ["retries", "retry_at", "retry_reason", "step", "cb"])
                # caller/tc_id 不在这里清：子 agent 的完成结果要靠它们回写给父 agent
                # （_step 的 DONE 分支 → _handle_terminal_sync），先清掉会让父 agent 永远停在 WAIT_TOOL。
                await self._run_callback(workspace, agent_name, state.get("cb", ""), _make_ctx(state))
                self._notify_done(agent_name, wl)
        except asyncio.CancelledError:
            raise
        except Exception as e:
            # 统一经适配层归一化：AUTH / PERM 走等待，其余瞬态错误走自管 RETRY
            info = _backend_for(config.get("backend")).normalize(e)
            if info.kind == KIND_AUTH:
                _write_proc(workspace, agent_name, wl, _make_ctx(state),
                    status="WAIT_HUMAN", wait_reason="AUTH",
                    error_code="AUTH_ERROR", error_detail=info.message)
                return
            if info.kind == KIND_FORBIDDEN:
                _write_proc(workspace, agent_name, wl, _make_ctx(state),
                    status="WAIT_HUMAN", wait_reason="PERM",
                    error_code="PERM_ERROR", error_detail=info.message)
                return
            if info.retryable:
                retries = int(state.get("retries", "0"))
                # 重试上限取 agent 配置的 retry 键（`.init -retry N` / 配置更新写的都是它）；
                # 旧代码读的 "max_retries" 从来没人写过，等于限死 3 次。
                try:
                    max_retries = int(config.get("retry", "3") or 3)
                except (TypeError, ValueError):
                    max_retries = 3
                if retries < max_retries:
                    delay = min(2 ** retries, 300)
                    retry_at = int(time.time() + delay)
                    _write_proc(workspace, agent_name, wl, _make_ctx(state),
                        status="RETRY", retries=str(retries + 1),
                        retry_at=str(retry_at),
                        retry_reason="rate_limit" if info.kind == KIND_RATE_LIMIT else "transient")
                    asyncio.create_task(
                        self._retry_wakeup(workspace, agent_name, wl, retry_at))
                    return
                _write_proc(workspace, agent_name, wl, _make_ctx(state),
                    status="ERROR", error_code="RETRY_EXHAUSTED",
                    error_detail=info.message)
                self._notify_done(agent_name, wl)
                return
            _write_proc(workspace, agent_name, wl, _make_ctx(state),
                status="ERROR", error_code="LLM_CRASH", error_detail=info.message)
            self._notify_done(agent_name, wl)
            return

    async def _handle_wait_tool(self, workspace, agent_name: str, wl: str, state: dict, config: dict):
        _v2_info(f"[v2:HWT] {agent_name} enter")
        his_name = config.get("his", "HIS")
        his_text = _read_his(workspace, agent_name, his_name, config,
            _make_ctx(state))
        unpaired = _find_unpaired_tool_calls(his_text, workspace=workspace)

        if not unpaired:
            step = int(state.get("step", "0")) + 1
            _write_proc(workspace, agent_name, wl, _make_ctx(state),
                status="BUSY", step=str(step))
            return

        tc = unpaired[0]
        _v2_info(f"[v2:HWT] {agent_name} executing: {tc['name']} {tc['tc_id']}")
        prev_agent = _current_step_agent.get()
        prev_wl = _current_step_wl.get()
        prev_tc = _current_tc_id.get()
        _current_step_agent.set(agent_name)
        _current_step_wl.set(wl)
        _current_tc_id.set(tc["tc_id"])
        try:
            pipeline = ""
            try:
                import json
                args = json.loads(tc.get("args", "{}"))
                pipeline = args.get("pipeline", "")
            except Exception:
                pipeline = tc.get("args", "")
            _v2_info(f"[v2:HWT] pipeline={pipeline[:80]}")

            if "@LLM" in pipeline:
                from mdio_py.core.interpreter import RuntimeContext as _RC
                exec_ctx = _RC(wt_uid=state.get("wt") or "genesis", vfs_path="/", active_doc=None)
                _v2_info(f"[v2:HWT] exec async for {pipeline[:60]}")
                result = await workspace.runtime.exec(
                    pipeline,
                    ctx=exec_ctx)
                # Write tool_result to HIS for ALL @LLM ops (sync + async)
                # Sync ops (.init, .close, stats): we write the result directly
                # Async ops (.run, .stream): sub-agent's _handle_terminal_sync
                #   also writes tool_result to caller's HIS. Two entries with
                #   the same tc_id are harmless — rebuild deduplicates.
                result_str = str(result) if result is not None else ""
                _print_tool_result(result_str)
                step = int(state.get("step", "0"))
                # For async ops (.run/.stream): sub-agent's _handle_terminal_sync
                # already wrote tool_result to caller's HIS.  Don't duplicate.
                # For sync ops (.init/.close/etc.): write tool_result + BUSY here.
                if not _is_async_llm_call(pipeline):
                    _append_record(workspace, agent_name, his_name, "tool_result",
                                   result_str, config, _make_ctx(state),
                                   fields={"name": tc["name"], "id": tc["tc_id"]})
                    _write_proc(workspace, agent_name, wl, _make_ctx(state),
                        status="BUSY", step=str(step))
                    _v2_info(f"[v2:HWT] sync @LLM done → BUSY for {agent_name}")
                else:
                    _v2_info(f"[v2:HWT] async @LLM launched, waiting for sub-agent for {agent_name}")
            else:
                exec_ctx = RuntimeContext(wt_uid=state.get("wt") or "genesis", vfs_path="/", active_doc=None)
                result = await workspace.runtime.exec(
                    pipeline, ctx=exec_ctx)
                result_str = str(result) if result is not None else ""
                _print_tool_result(result_str)
                _append_record(workspace, agent_name, his_name, "tool_result",
                               result_str, config, _make_ctx(state),
                               fields={"name": tc["name"], "id": tc["tc_id"]})
                step = int(state.get("step", "0"))
                _write_proc(workspace, agent_name, wl, _make_ctx(state),
                    status="BUSY", step=str(step))
                _v2_info(f"[v2:HWT] {agent_name} sync done → BUSY")
        finally:
            _current_step_agent.set(prev_agent)
            _current_step_wl.set(prev_wl)
            _current_tc_id.set(prev_tc)

    async def _handle_retry(self, workspace, agent_name: str, wl: str, state: dict, config: dict):
        retry_at = float(state.get("retry_at", "0"))
        if time.time() < retry_at:
            return
        _write_proc(workspace, agent_name, wl, _make_ctx(state), status="BUSY")
        _clear_transient(workspace, agent_name, wl, _make_ctx(state),
            ["retry_at", "retry_reason"])

    async def _retry_wakeup(self, workspace, agent_name: str, wl: str, retry_at: float):
        delay = retry_at - time.time()
        if delay > 0:
            await asyncio.sleep(delay)
        _write_proc(workspace, agent_name, wl, _make_ctx({}), status="BUSY")

    def _handle_terminal_sync(self, workspace, agent_name: str, wl: str, state: dict, config: dict):
        caller = state.get("caller", "")
        caller_wl = _wl_key(state.get("caller_wl") or "main")
        tc_id = state.get("tc_id", "")
        status = state.get("status", "DONE")

        if caller and tc_id:
            caller_config = _read_config(workspace, caller,
                _make_ctx(state))
            caller_config["wl"] = caller_wl
            caller_state = _read_state(workspace, caller, caller_wl, _make_ctx(state))
            caller_status = caller_state.get("status", "IDLE")
            if caller_status not in ("DONE", "ERROR"):
                his_name = config.get("his", "HIS")
                response = _read_last_assistant(workspace, agent_name, his_name, config,
                    _make_ctx(state)) or ""
                if status == "ERROR":
                    response = f"ERROR: {state.get("error_detail", "unknown")}"
                caller_his = caller_config.get("his", "HIS")
                _append_record(workspace, caller, caller_his, "tool_result",
                               response, caller_config, _make_ctx(state),
                               fields={"name": "execute_mdio", "id": tc_id})
                _write_proc(workspace, caller, caller_wl, _make_ctx(state), status="BUSY")

        _clear_transient(workspace, agent_name, wl, _make_ctx(state),
            ["caller", "caller_wl", "tc_id", "step", "retries", "retry_at", "retry_reason"])
        self._notify_done(agent_name, wl)

    async def _run_callback(self, workspace, agent_name: str, cb_code: str, ctx):
        """对话正常结束后执行一次 `-cb` 指定的 MDIO 代码。

        只在正常收尾（写 status="DONE"）时走这里：ERROR 路径与超 tool 上限不执行；
        子 agent 各自执行自己的 cb。失败只报错，不影响本轮结果。
        """
        code = str(cb_code or "").strip()
        if not code:
            return
        _v2_info(f"[v2:cb] {agent_name} callback: {code[:80]}")
        try:
            result = await workspace.runtime.exec(code, ctx=ctx)
            _v2_info(f"[v2:cb] {agent_name} callback result: {str(result)[:120]}")
        except Exception as e:
            _print_error(f"callback failed for {agent_name}: {e}")

    def _stream_error(self, agent_name: str, wl: str, msg: str):
        """给正在等待的 `@LLM.x.stream` 消费者推一条错误块（队列由 `_notify_done` 收尾）。

        早错路径（NO_SECRETS / NO_MODEL / 同步分支 LLM_CRASH）以前只写 proc + notify，
        不碰 stream 队列 ⇒ 消费端 `async for` 永久挂住（实测：无密钥时 40s 无输出）。
        """
        q = self._stream_queues.get((agent_name, _wl_key(wl)))
        if q is not None:
            q.put_nowait(("error", msg))

    def _notify_done(self, agent_name: str, wl: str):
        key = (agent_name, _wl_key(wl))
        tb = traceback.extract_stack(limit=8)
        _v2_info(f"[v2:notify] {agent_name}/{key[1]} via: {' -> '.join(f'{f.name}:{f.lineno}' for f in tb[-5:])}")
        event = self._done_events.pop(key, None)
        if event is not None:
            event.set()
        # 回合终止（DONE/ERROR）时把 stream 队列收尾：这是所有终点的公共出口，
        # 漏在这里就是消费端永久等待。流式分支自己已 pop 过 ⇒ 这里是幂等的 no-op。
        q = self._stream_queues.pop(key, None)
        if q is not None:
            q.put_nowait(None)

_PROC_FIELDS = [
    "status", "turn", "step", "retries", "sys_hook_id",
    "caller", "caller_wl", "tc_id",
    "error_code", "error_detail", "completed_at", "total_steps",
    "timeout", "timeout_job", "cb", "wt",
    "retry_at", "retry_reason", "wait_reason",
]
"""proc.md 实例字段清单：_read_state 只按此清单逐字段读，不递归、不扫子节。"""


async def _list_instance_wls(workspace, agent_name: str, ctx) -> list[str]:
    """列出 agent 下所有 #### 实例子标题（wl 名）。"""
    try:
        raw = await workspace.runtime.exec(
            f"$ /.mds/proc.md::PROCESS_TABLE.LLM.{agent_name}._h -pj tag", ctx=ctx)
    except Exception:
        return []
    text = str(raw or "").strip()
    if not text or text.startswith("- mdio_error"):
        return []
    return [ln.strip() for ln in text.splitlines() if ln.strip()]


def _wl_key(wl) -> str:
    """空 wl 归一成 main；proc 标题与运行实例键都用它。"""
    return str(wl or "").strip() or "main"


def _proc_heading(agent_name: str, wl: str) -> str:
    return "PROCESS_TABLE.LLM.%s.%s" % (agent_name, _wl_key(wl))


def _ensure_wl_hook(workspace, agent_name: str, wl: str, ctx) -> None:
    """确保 (agent, wl) 实例的 proc 状态与永久 hook 存在。"""
    wl = _wl_key(wl)
    state = _read_state(workspace, agent_name, wl, ctx)
    hid = str(state.get("sys_hook_id", "0"))
    if hid.isdigit() and int(hid) > 0:
        return
    _write_proc(workspace, agent_name, wl, ctx, status="IDLE", turn="0", step="0")
    hook_ir = by_tag(_proc_heading(agent_name, wl), "status")
    hk_id = workspace.store.register_hook(
        "genesis", "/.mds/proc.md",
        hook_ir, '@LLM -step "%s" -wl "%s"' % (agent_name, wl),
        asdict(ctx), is_always=True)
    _write_proc(workspace, agent_name, wl, ctx, sys_hook_id=str(hk_id))



# v2 状态机 — Phase 1 纯函数


STATE_DEFAULTS = {
    "status": "IDLE",
    "turn": "0",
    "step": "0",
    "retries": "0",
    "sys_hook_id": "0",
}
"""proc.md 字段集中默认值。_read_state 统一返回，避免各处散布硬编码。"""


def _make_ctx(state: dict):
    """从 proc.md state dict 重建 RuntimeContext。"""
    return RuntimeContext(
        wt_uid=state.get("wt") or "genesis",   # 工地树:session 文档 state.wt 决定 exec/写树(默认 genesis)
        vfs_path=state.get("cwd", "/"),
        active_doc=state.get("active_doc"),
        worktree_name=state.get("worktree_name"),
    )


def _read_state(workspace, agent_name: str, wl: str, ctx) -> dict:
    wl = _wl_key(wl)
    heading = _proc_heading(agent_name, wl)
    state = dict(STATE_DEFAULTS)
    for key in _PROC_FIELDS:
        try:
            chunks = workspace.read_locator("genesis", "/.mds/proc.md", by_tag(heading, key))
        except Exception:
            continue
        if chunks:
            state[key] = str(chunks[0]).strip()
    _v2_info(f"[v2:proc] READ {agent_name}/{wl} status={state.get('status')} turn={state.get('turn')} step={state.get('step')} caller={state.get('caller','')} caller_wl={state.get('caller_wl','')} tc_id={state.get('tc_id','')}")
    return state


def _instance_heading_ir(agent_name: str, wl: str):
    """直接定位 #### <wl> 这个实例子标题（不是 bullet）。"""
    return (SelectorBuilder()
            .drill(f"PROCESS_TABLE.LLM.{agent_name}")
            .type_is("head")
            .tag_eq(_wl_key(wl))
            .slice(0)
            .build())


def _proc_field_ir(agent_name: str, wl: str, key: str):
    """直接子节点里 tag==key 的 bullet（用于整条删除，含 bq 子块）。"""
    return SelectorIR(file_path=None, steps=[
        ["Drill", "PROCESS_TABLE"], ["Drill", "LLM"],
        ["Drill", agent_name], ["Drill", _wl_key(wl)],
        ["Wildcard"],
        ["AND", [["TYPECHECK", "bullet"], ["CMP", ".tag", "==", key]]],
    ])


def _proc_field_block(key: str, value) -> str:
    """字段写成 md block；多行值按老办法写 - key: + 下挂 bq 行。"""
    text = str(value)
    if "\n" in text:
        body = "\n".join("  > " + ln if ln else "  >" for ln in text.splitlines())
        return "- %s:\n%s\n" % (key, body)
    return "- %s: %s\n" % (key, text)


def _write_proc(workspace, agent_name: str, wl: str, ctx, **fields):
    """删字段节点 + 追加干净 bullet；多行 bq 字段不会残留旧子行或重复 bullet。"""
    wl = _wl_key(wl)
    heading = _proc_heading(agent_name, wl)
    heading_ir = _instance_heading_ir(agent_name, wl)
    _v2_info(f"[v2:proc] WRITE {agent_name}/{wl} -> { {k:v for k,v in fields.items() if k != 'sys_hook_id'} }")

    if not workspace.head_exists("genesis", "/.mds/proc.md", heading_ir):
        # drill 路径 + bullet 写会自动 scaffold 出 #### wl
        workspace.write_locator(
            "genesis", "/.mds/proc.md", by_tag(heading, "status"), "/>", "IDLE",
            auto_scaffold=True,
        )

    for key, value in fields.items():
        try:
            workspace.write_locator(
                "genesis", "/.mds/proc.md", _proc_field_ir(agent_name, wl, key),
                "->", "", auto_scaffold=False,
            )
        except Exception:
            pass
        if value is None or str(value).strip() == "":
            continue
        workspace.write_locator(
            "genesis", "/.mds/proc.md", heading_ir, "+>", _proc_field_block(key, value),
            auto_scaffold=False,
        )


_TRANSIENT_FIELDS = [
    "step", "caller", "tc_id",
    "retries", "retry_at", "retry_reason",
    "wait_reason", "error_code", "error_detail",
    "completed_at", "total_steps",
    "timeout", "timeout_job",
    "cb",
]

# 收尾（DONE/ERROR）时**不能**清 caller/tc_id：子 agent 的结果要靠这两个字段回写给父 agent
# （_step 的 DONE/ERROR 分支 → _handle_terminal_sync）；先清掉父 agent 会永远停在 WAIT_TOOL。
_TERMINAL_CLEAR_FIELDS = [f for f in _TRANSIENT_FIELDS if f not in ("caller", "tc_id")]


def _clear_transient(workspace, agent_name: str, wl: str, ctx, fields: list[str] | None = None):
    targets = fields if fields is not None else _TRANSIENT_FIELDS
    _write_proc(workspace, agent_name, wl, ctx, **{f: "" for f in targets})


def _read_config(workspace, agent_name: str, ctx) -> dict:
    wt_uid = "genesis"
    llm_path = "/.mds/llm.md"
    config = {}
    for key in ("m", "sysp", "sysp_file", "tool", "his", "retry", "tool_timeout", "wl",
                "backend", "provider", "timeout", "proxy"):
        try:
            chunks = workspace.read_locator(
                wt_uid, llm_path,
                by_tag(f"AGENTS.{agent_name}", key),
            )
            if chunks:
                val = str(chunks[0]).strip()
                if (val.startswith('"') and val.endswith('"')) or \
                   (val.startswith("'") and val.endswith("'")):
                    try:
                        val = ast.literal_eval(val)
                    except Exception:
                        val = val[1:-1]
                config[key] = val
        except Exception:
            pass

    # XBT_DIRECTOR 预置：sysp_file 指向仓库里的 md；读时现加载，避免多行 sysp
    # 被 markdown bullet 存储改形。
    sysp_file = str(config.get("sysp_file") or "").strip()
    if sysp_file and not config.get("sysp"):
        try:
            config["sysp"] = _resolve_sysp_path(sysp_file).read_text(encoding="utf-8")
        except Exception:
            config["sysp"] = ""
    return config



# HIS 路径解析辅助（共享 same path resolution logic）


def _split_his(his_name: str):
    """HIS 定位符统一切分（Q2）：「file::节点」；无 :: ⇒ (None, 原文)。"""
    if "::" in his_name:
        from mdio_py.core import split_selector_addr
        addr = split_selector_addr(his_name)
        return addr.file_path, addr.md_path
    return None, str(his_name)


class _HisAddr:
    """HIS 地址的唯一解析器：his_name -> wt/doc/node/selector/exec_sel。"""

    def __init__(self, agent_name: str, his_name: str, ctx):
        self.agent_name = agent_name
        self.his_name = his_name
        wt_uid = "genesis"
        addr = his_name or ""
        if his_name and ":/" in his_name:
            wt_uid, _, addr = his_name.partition(":")
        elif ctx is not None and getattr(ctx, "wt_uid", None):
            wt_uid = ctx.wt_uid
        self.wt_uid = wt_uid
        self.addr = addr
        file_path, node = _split_his(addr)
        if file_path:
            self.doc_path = ctx.resolve_path(file_path) if ctx is not None else file_path
            self.node_path = node or ""
        else:
            self.doc_path = "/.mds/llm.md"
            self.node_path = f"AGENTS.{agent_name}.{addr}"

    def selector(self, with_main: bool = False, section: str = "main"):
        node = self.node_path
        if with_main:
            node = node + "." + (section or "main")
        return drill(node)

    def section_selector(self, section: str):
        """typed 定位某子节 heading（避免 tag 与 gate 撞名）。"""
        base = self.selector()
        return SelectorIR(steps=list(base.steps) + [
            ["Wildcard"], ["TYPECHECK", "h"], ["CMP", ".tag", "==", section],
        ])

    def exec_sel(self) -> str:
        return self.doc_path + "::" + self.node_path


def _his_addr(agent_name: str, his_name: str, ctx) -> _HisAddr:
    return _HisAddr(agent_name, his_name, ctx)




def _his_has_main(workspace, agent_name: str, his_name: str, config: dict, ctx) -> bool:
    """HIS 是否已是新结构(存在 ### main 节)。用于 _append_record 决定写入 main 还是 HIS 末尾。"""
    try:
        txt = _read_his(workspace, agent_name, his_name, config, ctx)
    except Exception:
        return False
    return bool(re.search(r"(?m)^#{3,6}\s+main\s*$", txt))


def _ensure_his_section(workspace, agent_name: str, his_name: str, section: str,
                        config: dict, ctx):
    """确保 HIS 下存在指定子节；返回该节 heading 的 typed 选择器。"""
    _ensure_his_main(workspace, agent_name, his_name, config, ctx)
    a = _his_addr(agent_name, his_name, ctx)
    sec_ir = a.section_selector(section)
    if workspace.head_exists(a.wt_uid, a.doc_path, sec_ir):
        return sec_ir
    workspace.write_locator(
        a.wt_uid, a.doc_path, a.selector(), "+>", "# " + section,
        auto_scaffold=True, auto_level=True,
    )
    return sec_ir


def _wl_gate_exists(workspace, agent_name: str, his_name: str, config: dict,
                    ctx, wl: str) -> bool:
    """wl 是否是已存在的 branch/join 门（main/空 alway true）。"""
    if _wl_key(wl) == "main":
        return True
    try:
        his_text = _read_his(workspace, agent_name, his_name, config, ctx)
    except Exception:
        return False
    for g in _parse_his_structure(his_text)["gates"]:
        if g["tag"] == wl and g["type"] in ("branch", "join"):
            return True
    return False


def _branch_create(workspace, agent_name: str, wl: str, branch_arg, target_arg,
                   config: dict, ctx) -> dict | str:
    """建一条 branch：+ <wl> 门 + control.path；同文档，不复制记录。"""
    wl = _wl_key(wl)
    his_name = config.get("his", "HIS")
    a = _his_addr(agent_name, his_name, ctx)
    wt_uid, llm_path, addr = a.wt_uid, a.doc_path, a.node_path
    _ensure_his_main(workspace, agent_name, his_name, config, ctx)
    his_text = _read_his(workspace, agent_name, his_name, config, ctx)

    # 同 tag 已存在：同 target 幂等；不同 target 报错
    structure = _parse_his_structure(his_text)
    existing = next((g for g in structure["gates"] if g["tag"] == wl), None)
    if existing is not None and existing.get("type") != "branch":
        return "- mdio_error: [LLM] wl tag already exists as non-branch: %s" % wl

    # target path / section
    target_raw = None
    if target_arg not in (None, True, ""):
        target_raw = str(target_arg).strip().strip('"').strip("'")
    if target_raw:
        fpath, node = _split_his(target_raw)
        if fpath:
            resolved = ctx.resolve_path(fpath) if ctx is not None else fpath
            if resolved != llm_path:
                return "- mdio_error: [LLM] -branch_target must stay in same md: %s" % target_raw
            node_s = node or ""
        else:
            node_s = target_raw
        tgt_section = arrow_handoff.target_section(node_s)
        if not tgt_section:
            return "- mdio_error: [LLM] -branch_target has no section: %s" % target_raw
        target_path = target_raw
    else:
        tgt_section = "b_" + wl
        target_path = a.exec_sel() + "." + tgt_section

    if existing is not None:
        if existing.get("target") == tgt_section:
            return {"wl": wl, "section": tgt_section, "gate": existing.get("section", "")}
        return ("- mdio_error: [LLM] wl tag already points to %s (not %s)"
                % (existing.get("target"), tgt_section))

    # 先建/确认 target 段，再挂 gate
    _ensure_his_section(workspace, agent_name, his_name, tgt_section, config, ctx)

    # parent 从 child wl 的最长已存在前缀推导；不再依赖 config 默认指针
    parents = [g["tag"] for g in structure["gates"]
               if g["tag"] != wl and wl.startswith(g["tag"] + "_")
               and g.get("type") in ("branch", "join")]
    parent_wl = max(parents, key=len) if parents else ""
    parent_section = _resolve_write_section(
        workspace, agent_name, his_name, {"wl": parent_wl}, ctx)
    parent_heading_ir = a.selector(with_main=True, section=parent_section)
    if not workspace.head_exists(wt_uid, llm_path, parent_heading_ir):
        return "- mdio_error: [LLM] parent section not found: %s" % parent_section

    # anchor: bare = 段尾新建空 thinking marker（fold 不出消息）；数字 = 第 idx 条记录
    text_arg = "" if branch_arg in (None, True) else str(branch_arg).strip()
    recs = structure["sections"].get(parent_section) or []
    if not text_arg:
        # 空 marker：不复制、不占用对话记录；父线上下文完整保留
        workspace.write_locator(
            wt_uid, llm_path, parent_heading_ir, "+>", "- thinking:\n",
            auto_scaffold=True)
        idx = len(recs)
    else:
        if not text_arg.isdigit():
            return "- mdio_error: [LLM] -branch expects a record idx (or bare flag)"
        idx = int(text_arg)
        if idx < 0 or idx >= len(recs):
            return ("- mdio_error: [LLM] -branch idx %d out of range for %s (%d records)"
                    % (idx, parent_section, len(recs)))

    # host 记录追加 branch 控制子 bullet（append，不改写原 content）：
    # 改写 content 会把既有 control/payload 的缩进整体重排（带门 host 会被破结构）；
    # 追加到记录尾部时引擎把 + 控制解析成记录的直属子 bullet，payload / 旧门原样保留。
    sec_ir = a.section_selector(parent_section)
    rec_ir = SelectorIR(file_path=None, steps=list(sec_ir.steps) + [
        ["Wildcard"], ["TYPECHECK", "bullet"], ["Slice", idx],
    ])
    ctl = "\n    + %s:\n        - type: branch\n        - path: %s" % (wl, target_path)
    workspace.write_locator(
        wt_uid, llm_path, rec_ir, "+>", ctl,
        auto_scaffold=False,
    )

    return {"wl": wl, "section": tgt_section, "parent": parent_section}


def _parse_his_structure(his_text: str) -> dict:
    """一次解析 HIS：{sections: {sec: [{role, controls: [{tag,mk,type,path,next}]}]}, gates: [...]}。

    只服务控制面（菜单/校验/写目标/branch 创建）；读端 walk 仍走 arrow SQL。
    """
    sections = {}
    sec = None
    rec = None
    ctl = None
    for line in str(his_text or "").splitlines():
        mh = re.match(r"^#{3,6}\s+(.+?)\s*$", line)
        if mh:
            sec = mh.group(1).strip()
            sections.setdefault(sec, [])
            rec = None
            ctl = None
            continue
        if sec is None:
            continue
        mr = re.match(r"^-\s+([A-Za-z_]+):\s*$", line)
        if mr:
            rec = {"role": mr.group(1), "controls": []}
            sections[sec].append(rec)
            ctl = None
            continue
        if rec is None:
            continue
        mc = re.match(r"^\s{2,6}([+*])\s+([^:]+):\s*$", line)
        if mc:
            ctl = {"tag": mc.group(2).strip(), "mk": mc.group(1),
                   "type": "", "path": "", "next": ""}
            rec["controls"].append(ctl)
            continue
        if ctl is None:
            continue
        mt = re.match(r"^\s*-\s*type:\s*(\S+)", line)
        if mt:
            ctl["type"] = mt.group(1).strip()
            continue
        mp = re.match(r"^\s*-\s*path:\s*(.+?)\s*$", line)
        if mp:
            ctl["path"] = mp.group(1).strip()
            continue
        mn = re.match(r"^\s*-\s*next:\s*(\S+)", line)
        if mn:
            ctl["next"] = mn.group(1).strip()

    gates = []
    for sec_name, recs in sections.items():
        for rec in recs:
            for ctl in rec["controls"]:
                if ctl.get("mk") != "+":
                    continue
                path = ctl.get("path", "")
                gates.append({
                    "tag": ctl["tag"],
                    "type": ctl.get("type", ""),
                    "path": path,
                    "target": arrow_handoff.target_section(path),
                    "section": sec_name,
                })
    return {"sections": sections, "gates": gates}


def _write_agent_wl(workspace, agent_name: str, value: str) -> None:
    """把 AGENTS.<agent>.wl 写成 agent 节下唯一一条（直接按节写，不整篇重写）。"""
    workspace.write_locator(
        "genesis", "/.mds/llm.md",
        by_tag(f"AGENTS.{agent_name}", "wl"),
        "/>", str(value), auto_scaffold=True,
    )


def _walk_write_section(sections: dict, wl: str) -> str:
    """按 wl 开集 walk branch/join，返回写目标节（最后活动段）。"""
    if "main" not in sections:
        return "main"
    frames = [{"sec": "main", "idx": 0, "alive": True}]
    last = "main"
    guard = 0
    while frames:
        guard += 1
        if guard > 100000:
            return "main"
        fr = frames[-1]
        recs = sections.get(fr["sec"]) or []
        if not fr["alive"] or fr["idx"] >= len(recs):
            frames.pop()
            continue
        rec = recs[fr["idx"]]
        moved = False
        for ctl in rec["controls"]:
            if not arrow_handoff.wl_open(ctl["tag"], wl):
                continue
            typ = ctl.get("type", "").strip()
            if typ not in ("branch", "join"):
                continue
            target = arrow_handoff.target_section(ctl.get("path", ""))
            if not target or target not in sections:
                continue
            if typ == "branch":
                fr["alive"] = False
            else:
                fr["idx"] += 1
            frames.append({"sec": target, "idx": 0, "alive": True})
            last = target
            moved = True
            break
        if moved:
            continue
        last = fr["sec"]
        fr["idx"] += 1
    return last if last in sections else "main"


def _resolve_write_section(workspace, agent_name: str, his_name: str,
                           config: dict, ctx) -> str:
    """写目标 = 与读端同一套 wl 开集 + branch/join walk 的最后活动段。

    compact/prune 只改视图，不改变写目标；无 wl/无门 ⇒ main。
    """
    wl = str((config or {}).get("wl", "") or "").strip()
    if not wl:
        return "main"
    try:
        text = _read_his(workspace, agent_name, his_name, config, ctx)
    except Exception:
        return "main"
    if not str(text or "").strip():
        return "main"
    return _walk_write_section(_parse_his_structure(text)["sections"], wl)


# arrow_handoff 是唯一装配实现。
# 下面只保留崩溃兜底:直接对原样 HIS 行解析(无世界线语义, 只为不丢整段历史)。


def _raw_messages(his_text: str, workspace) -> list:
    if not str(his_text or "").strip():
        return []
    try:
        return _rebuild_messages(his_text, workspace=workspace)
    except Exception as _e:
        _v2_info(f"[v2:messages] 原样 HIS 兜底也失败: {str(_e)[:120]}")
        return []


async def _messages_for_turn(his_text: str, wl: str, workspace, ctx) -> list:
    """一轮 handoff:arrow-only;失败仅退回原样 HIS 行解析(兜底, 无世界线语义)。"""
    try:
        return await arrow_handoff.handoff(his_text, wl, workspace, ctx)
    except Exception as _e:
        _v2_info(f"[v2:handoff] arrow 失败,退回原样 HIS 行解析: {str(_e)[:160]}")
        _print_warn(f"worldline arrow failed, fallback raw HIS: {str(_e)[:120]}")
        return _raw_messages(his_text, workspace)



# HIS 解析 / 重建


def _ensure_his_main(workspace, agent_name: str, his_name: str, config: dict, ctx) -> None:
    """确保 HIS 有 **### main** 节(最新结构唯一写入点);缺则补建骨架。

    HIS 节已存在 ⇒ 追加 "### main" 标题;整个 HIS 不存在 ⇒ 写 "## HIS\n### main" 接骨。
    (旧式"无 main"的 HIS 不再支持差分写入——首写即补建 main。)
    """
    if _his_has_main(workspace, agent_name, his_name, config, ctx):
        return
    a = _his_addr(agent_name, his_name, ctx)
    # 写 "# main"(h1)+ **auto_level**:引擎按目标父级自动调级(HIS h2 → main h3;
    # llm.md 的 HIS h3 → main h4)——层级恒为 HIS 的直接子级,不手写死级数。
    workspace.write_locator(a.wt_uid, a.doc_path, a.selector(), "+>", "# main",
                            auto_scaffold=True, auto_level=True)


def _read_his_main(workspace, agent_name: str, his_name: str, config: dict, ctx) -> str:
    """读 HIS 的 **main 子节**(最新 HIS 结构唯一形态);main 不存在 ⇒ 空串(不回退整节)。"""
    a = _his_addr(agent_name, his_name, ctx)
    try:
        chunks = workspace.read_locator(a.wt_uid, a.doc_path,
                                        a.selector(with_main=True))
        return "\n".join(str(c) for c in chunks) if chunks else ""
    except Exception:
        return ""


def _read_his(workspace, agent_name: str, his_name: str, config: dict, ctx) -> str:
    a = _his_addr(agent_name, his_name, ctx)
    try:
        chunks = workspace.read_locator(a.wt_uid, a.doc_path, a.selector())
        if chunks:
            text_out = "\n".join(str(c) for c in chunks)
            _v2_info("[v2:read] wt=%s path=%s n=%d head=%r" % (a.wt_uid, a.doc_path, len(text_out), text_out[:60]))
            return text_out
    except Exception as _e:
        _v2_info("[v2:read] EXC %s" % str(_e)[:120])
    return ""



# HIS v1.4：记录 + 字段 + 引用容器（bq）载荷 —— **只 bq，无 ic**

# 形状（规范形 /his_bq_scheme.md §2）：
#   - tool_call:            记录：列 0 的 bullet，tag = 角色，值恒空
#     - name: execute_mdio  字段：缩进 2 的子 bullet
#     - id: call_8f3a
#     - tool: pipeline      参数名（单参数工具记录才有）
#     >     $ /logs/today.md::ERRORS    载荷：`  > ` + 原文（bq 前缀码，**不再加 4 空格 ic**）
#
# 三条约定：C1 记录 "- role:"（marker 后恰一个空格 ⇒ 内容列 = 2）；
#          C2 载荷行 "  > "、空行 "  >"（无尾空格）——**一层前缀码**，读侧剥掉即原文；
#          C3 载荷首尾空行属结构层（节点 mark/边界，只读）⇒ 写侧 strip，不入 content。
#
# 为什么载荷必须留在 bq 里，而不是直接挂在 bullet 下（实测）：
#   直接子树形态下，载荷里的 md 标题行会被引擎当成**真标题**：把整节 HIS 回读成一条
#   tool_result 时，载荷里的 "  ### HIS" 从记录内容里逃逸，在根级再造一个 HIS 节
#   （实测 drill("AGENTS.t9.HIS") 命中 2 个节点、`+>` 追加到两处 ⇒ 记账重复、结构崩）。
#   bq 是 md 里唯一能把块级内容（标题 / 列表 / 围栏）关在容器内的结构：`  > ## x` 被
#   解析成 bq 的子节点（实测 b > bq > [h, p, bl, ...]），选择器不会被内容欺骗。
#   前端因此拿到的是 **bq 下已解析的 md 子树**（p / h / bl / c），直接交 std renderNode。


HIS_CONTENT_COL = 2                                # "- role:" 的内容列 = marker 宽 1 + 一个空格
_HIS_IND = " " * HIS_CONTENT_COL
HIS_IC_IND = "    "                                # 历史常量（ic 已废弃）；保留避免外部引用断
_HIS_PREFIX = _HIS_IND + "> "                       # 载荷行前缀 "  > "
_HIS_BLANK = _HIS_IND + ">"                        # 空行（无尾空格）


def _encode_bq_lines(content: str) -> str:
    """payload → **记录下引用容器（bq）里的一行一前缀**（HIS v1.4：只 bq，无 ic）。

    每行写成 "  > " + 原文；空行写成 "  >"（无尾空格）。bq 把块级内容关在容器内：
    载荷里的 `## 标题` / `- 列表` / 围栏都成为 bq 的子节点（b > bq > h|p|bl|c），
    不再从记录里逃逸成文档结构（直接子树形态实测会造出重复 HIS 节，见上方常量注释）。

    前端拿到的就是 bq 下**已解析的 md 子树**（直接交 std renderNode）；插件读侧用行解析
    剥掉一层 "  > " 前缀还原 md 源（见 `_parse_his_records`）。

    首尾空行按 mdio 约定属**结构层**（节点的 mark/边界，只读）⇒ 这里 strip 掉，不入 content。
    空载荷返回 ""（不建空 bq）——调用方据此决定是否追加内容行。
    """
    body = (content or "").strip("\n")
    if not body:
        return ""
    return "\n".join((_HIS_PREFIX + l) if l else _HIS_BLANK for l in body.split("\n"))


def _tool_record_payload(tool_data: dict) -> str:
    """tool_call 的载荷：参数名已知（单参数）⇒ **参数值**；否则 ⇒ 整串 args JSON。

    参数值进 bq 前缀码行 ⇒ 值里的反引号 / 星号 / 链接都不会与容器混淆，
    读侧剥一层前缀 + `json.dumps({tool: 值})` 能逐字节重建回模型的 raw_args。
    """
    return ((tool_data.get("pipeline") or "") if tool_data.get("tool")
            else tool_data.get("raw_args", "{}"))


def _tool_record_fields(tool_data: dict) -> dict:
    """tool_call 的字段：name（工具名）+ id（配对键）+ tool（参数名，空值不落盘）。"""
    return {"name": tool_data.get("name", "execute_mdio"),
            "id": tool_data.get("id", ""),
            "tool": tool_data.get("tool", "")}


def _part_direct_bq_ir(agent_name: str, addr: str, role: str, section: str = "main"):
    """当前 role 记录（指定 HIS 节下最后一条）的**直属** bq 选择器。

    为什么不能取「递归最后一个 bq」：助手内容可以含嵌套引用（> >），
    递归 Slice(-1) 会命中内层引用，后续流式 chunk 就被追加进引用里，
    标题/列表降级成引用正文（实测复现见 tests/test_llm_stream_records.py）。
    """
    file_path, node = _split_his(addr)
    if file_path is None:
        node = f"AGENTS.{agent_name}.{addr}"
    main = node + "." + (section or "main")
    b = SelectorBuilder()
    for part in main.split("."):
        b.drill(part)
    b.type_is("bullet").tag_eq(role).slice(-1).type_is("bq").slice(-1)
    return b.build()


def _append_record(workspace, agent_name: str, his_name: str, role: str,
                   payload: str, config: dict, ctx, fields: dict | None = None,
                   part: bool = False):
    """HIS v2 写入：记录 bullet + 可选字段 + 引用载荷，一笔落盘。

    一笔而不是三笔：payload 以 "- role:" 开头，后续行按缩进归类，引擎一次成树
    （实测：记录行 + 缩进字段 + 缩进 "  >" 行写在同一个 payload 里能被正确分层）。
    """
    a = _his_addr(agent_name, his_name, ctx)
    section = _resolve_write_section(workspace, agent_name, his_name, config, ctx)

    if part:
        # 流式续写 = 字面意义的「追加到本记录的 bq 内容」：每个 chunk 一次 +>。
        # 关键：必须选 **当前 role 记录的直属 bq**，不能选「递归最后一个 bq」——
        # 助手内容里允许出现嵌套引用（> >），递归最后会命中内层 bq，把后续
        # 标题/列表全吸进引用里（实测见 tests/test_llm_stream_records.py 回归）。
        # section 与普通记录一致：wl 打开 branch/join 门后，流式 chunk 必须写进该线，
        # 不能永远写 main，否则新世界线只拿到 user 记录、assistant 回复跑回旧线。
        workspace.write_locator(a.wt_uid, a.doc_path,
                                _part_direct_bq_ir(agent_name, a.addr, role, section),
                                "+>", payload, auto_scaffold=False, proj="content")
        return

    # **新 HIS 结构(## HIS > ### main + 分支节)**:记录写入 wl 解析出的活动节。
    # 无 wl / 无匹配门 ⇒ main（基线）；有门打开的 wl ⇒ 落到 walk 的最后活动 branch 段。
    if section == "main":
        _ensure_his_main(workspace, agent_name, his_name, config, ctx)
    heading_ir = a.selector(with_main=True, section=section)

    lines = [f"- {role}:"]
    for k, v in (fields or {}).items():
        if v is None or str(v) == "":
            continue
        lines.append(f"{_HIS_IND}- {k}: {v}")
    enc = _encode_bq_lines(payload)
    if enc:                               # 空载荷不建空 bq（写 "- role:" 一笔即可）
        lines.append(enc)
    text = "\n".join(lines)

    # **不接骨**(auto_scaffold=False):目标节点不存在时写死拒绝,而不是凭空建 # HIS——接骨会把
    # 整篇 HIS 结构毁成"根级 # HIS + 新记录"(实测事故)。目标缺失时回退到 HIS 末尾(旧格式路径),
    # 仍失败则抛错(调用方可见),**永不静默污染账本**。
    def _write(ir, scaffold: bool = False) -> None:
        workspace.write_locator(a.wt_uid, a.doc_path, ir, "+>", text, auto_scaffold=scaffold)

    _write(heading_ir)                  # main 已由 _ensure_his_main 确保;**不接骨**
    after = 0
    try:
        after = len(_read_his(workspace, agent_name, his_name, config, ctx))
    except Exception:
        pass
    _v2_info(f"[v2:append] wrote {role} → {a.doc_path} "
             f"fields={list((fields or {}).keys())} payload_len={len(payload or '')} verify_len={after}")



def _top_bullets(env: dict) -> list[dict]:
    """JSON 信封里的 bullet（记录候选，按文档序）。

    **下钻标题节**：世界线文本($$ main 的输出)根是 "### main" 标题,记录在其 ct 里;
    HIS 文本根是 "## HIS"→"### main" 两层。只取根级 bl 会漏掉它们 ⇒ 内容整片丢失
    (实测:LY msg 全文 0 chars)。这里递归标题节点收集 bullet。
    """
    out: list[dict] = []

    def walk(ct: list, depth: int = 0) -> None:
        if depth > 8:
            return
        for blk in (ct or []):
            if not isinstance(blk, dict):
                continue
            nt = blk.get("nt")
            if nt == "bl":
                out.extend([b for b in (blk.get("ct") or [])
                            if isinstance(b, dict) and b.get("nt") == "b"])
            elif nt == "h":
                walk(blk.get("ct") or [], depth + 1)

    walk(env.get("ct") or [])
    return out


def _bq_plain_text(bq: dict | None) -> str:
    """普通 bq(未 ic 包装的新形载荷)→ 文本:段落 bq 内容(ic 优先已在上层处理)。"""
    if not isinstance(bq, dict):
        return ""
    parts: list[str] = []
    for c in (bq.get("ct") or []):
        if not isinstance(c, dict):
            continue
        nt = c.get("nt")
        if nt == "p":
            parts.append(inline_text(c))          # 段文本在 p.ct,p.t 恒空
        elif nt == "ic":
            parts.append(str(c.get("cd") or ""))
        elif nt == "bl":
            for b in (c.get("ct") or []):
                if isinstance(b, dict) and b.get("nt") == "b":
                    parts.append(inline_text(b.get("ct")))
    return "\n".join(p for p in parts if p)


def _is_record_bullet(b: dict) -> bool:
    """记录判定：**值恒空**的 bullet（子节点只有字段容器 `bl` 与载荷容器 `bq`）。

    `- user: 你好` 这种带内联值的 bullet 会带一个 `p` 子节点 ⇒ **不是记录**
    （规范形 §9.10「带内联值的 bullet 不构成记录」）。
    """
    return all(isinstance(c, dict) and c.get("nt") in ("bl", "bq")
               for c in (b.get("ct") or []))


_HIS_REC_RE = re.compile(r"^- ([^ :]+):[ \t]*$")               # 记录行:0 缩进 "- role:"(值恒空)
_HIS_FIELD_RE = re.compile(r"^  - (name|id|tool|status):[ \t]?(.*)$")  # 字段:2 空格缩进(白名单,避免误吃内容列表)


def _parse_his_records(his_text: str, workspace=None) -> list[dict]:
    """HIS 文本 → 记录列表（**HIS v1.4 形：记录 bullet + 引用容器 bq，行解析保真**）。

    记录 = 0 缩进 `- role:`；字段 = `  - name|id|tool|status:`（白名单，2 空格缩进）；
    载荷 = `  > ` + 原文 的行（bq 前缀码，**只剥一层前缀**）、空行 = `  >`（无尾空格）。
    标题行（`#`）与 0/1 缩进游离行结束当前记录；记录内空行保留（段落分隔）。

    为什么行解析而不用引擎 JSON：content 是 **md 源**，AST 对它有损（列表标记丢、
    软换行折成空格、loose 空行无标记）。前端则直接渲染**树**（不解析文本）。
    bq 里 md 被正常解析（标题/列表/围栏都在 bq 子树里），所以引擎侧读写**都按行**保真。
    """
    recs: list[dict] = []
    cur = None
    for ln in (his_text or "").split("\n"):
        if re.match(r"^#{1,6}\s", ln):                     # 标题行:结束当前记录
            cur = None
            continue
        m = _HIS_REC_RE.match(ln)
        if m:
            cur = {"role": m.group(1), "fields": {}, "lines": []}
            recs.append(cur)
            continue
        if not ln.strip():
            if cur is not None:
                cur["lines"].append("")                    # 记录内空行:段落分隔
            continue
        if cur is None:
            continue
        m = _HIS_FIELD_RE.match(ln)
        if m:
            cur["fields"][m.group(1)] = m.group(2).strip()
            continue
        if ln == _HIS_BLANK:                               # 空载荷行 "  >" → 段落分隔
            cur["lines"].append("")
            continue
        if ln.startswith(_HIS_PREFIX):                     # 载荷行 "  > 原文" → 剥一层前缀
            cur["lines"].append(ln[len(_HIS_PREFIX):])
            continue
        if ln.startswith("  "):
            cur["lines"].append(ln[2:])                     # 兜底:2 空格内容(手写/旧样本)
            continue
        cur = None                                        # 0/1 缩进游离行:结束当前记录
    return [{"role": r["role"], "fields": r["fields"],
             "content": "\n".join(r["lines"]).strip("\n")}
            for r in recs]





def _repair_messages(messages: list) -> list:
    """孤儿修复：丢掉没有结果的 tool_calls / 没有调用的 tool 消息，补 reasoning_content。"""
    tc_result_ids = {m["tool_call_id"] for m in messages if m["role"] == "tool"}
    repaired = []
    for msg in messages:
        if msg["role"] == "assistant":
            msg.setdefault("reasoning_content", "")
            if "tool_calls" in msg:
                valid = [tc for tc in msg["tool_calls"] if tc["id"] in tc_result_ids]
                if valid:
                    msg["tool_calls"] = valid
                    repaired.append(msg)
                elif msg.get("content"):
                    repaired.append({k: v for k, v in msg.items() if k != "tool_calls"})
            else:
                repaired.append(msg)
        elif msg["role"] == "tool":
            if msg.get("tool_call_id") in {
                tc["id"] for m in messages
                if m["role"] == "assistant" and "tool_calls" in m
                for tc in m["tool_calls"]
            }:
                repaired.append(msg)
        else:
            repaired.append(msg)
    return repaired


def _tc_arguments(fields: dict, content: str) -> str:
    """tool_call 记录 → OpenAI 的 `arguments` 字符串。

    形状把**参数名**放 `- tool:` 字段、**参数值**放 bq 载荷 ⇒ 这里 `json.dumps` 重建
    （参数值原本就是去转义后的文本，重建后与模型给的 raw_args 逐字节一致，实测 5/5）。
    没有 `tool` 字段（旧记录，或将来多参数的回退形状）⇒ 载荷本身就是整串 JSON，原样用。
    """
    tool = (fields or {}).get("tool")
    if tool:
        return json.dumps({tool: content}, ensure_ascii=False)
    return content.strip() or "{}"


def _rebuild_messages(his_text: str, workspace=None) -> list:
    """HIS 记录 → OpenAI messages。

    thinking 归到它**后面**那条 assistant（它本来就写在该 assistant 之前）；
    tool_result 按 id 去重：异步路径上同一个 tc_id 会被写两次（子 agent 与调用方各写一次），
    这里只保留先到的一条。
    """
    messages: list[dict] = []
    pending_reasoning = ""
    seen_tool_ids: set = set()
    for rec in _parse_his_records(his_text, workspace=workspace):
        role, content, fields = rec["role"], rec["content"], rec["fields"]
        if role == "user":
            messages.append({"role": "user", "content": content})
        elif role == "thinking":
            pending_reasoning = content
        elif role == "assistant":
            messages.append({"role": "assistant", "content": content or " ",
                             "reasoning_content": pending_reasoning})
            pending_reasoning = ""
        elif role == "tool_call":
            tc = {
                "id": fields.get("id") or f"fallback_{len(messages)}",
                "type": "function",
                "function": {"name": fields.get("name") or "execute_mdio",
                             "arguments": _tc_arguments(fields, content)},
            }
            if messages and messages[-1]["role"] == "assistant":
                messages[-1].setdefault("tool_calls", []).append(tc)
            else:
                messages.append({"role": "assistant", "content": None, "tool_calls": [tc]})
        elif role == "tool_result":
            tc_id = fields.get("id") or ""
            if tc_id and tc_id in seen_tool_ids:
                continue
            seen_tool_ids.add(tc_id)
            messages.append({"role": "tool",
                             "tool_call_id": tc_id,
                             "name": fields.get("name", "execute_mdio"),
                             "content": content})
    return _repair_messages(messages)


def _find_unpaired_tool_calls(his_text: str, workspace=None) -> list[dict]:
    """有 call 无 result 的 tool_call：两边都有 id 字段，配对就是集合相减。"""
    calls: list[dict] = []
    done: set = set()
    for rec in _parse_his_records(his_text, workspace=workspace):
        role, content, fields = rec["role"], rec["content"], rec["fields"]
        if role == "tool_result" and fields.get("id"):
            done.add(fields["id"])
        elif role == "tool_call" and fields.get("id"):
            calls.append({"tc_id": fields["id"],
                          "name": fields.get("name") or "execute_mdio",
                          "args": _tc_arguments(fields, content)})
    unpaired = [tc for tc in calls if tc["tc_id"] not in done]
    # 三连同名同参防毒丸
    if len(unpaired) >= 3:
        last3 = unpaired[-3:]
        if all(t["name"] == last3[0]["name"] and t["args"] == last3[0]["args"] for t in last3):
            return unpaired[:-3]
    return unpaired


def _read_last_assistant(workspace, agent_name: str, his_name: str, config: dict, ctx) -> str:
    """读最后一条 assistant 记录的载荷。

    整节读出后按记录取最后一条：批量选择器 `..*_b[.t=="assistant"][-1]._bq` 会把各条载荷
    连成一片、没有记录边界（见 /his_bq_scheme.md §4.3）。
    """
    try:
        # 新结构(有 ### main):只读当前 wl 的落点节，默认 main；否则会读到别的分支的 assistant。
        section = _resolve_write_section(workspace, agent_name, his_name, config, ctx)
        if section == "main":
            his_text = _read_his_main(workspace, agent_name, his_name, config, ctx)
        else:
            a = _his_addr(agent_name, his_name, ctx)
            sec_ir = a.section_selector(section)
            chunks = workspace.read_locator(a.wt_uid, a.doc_path, sec_ir)
            his_text = "\n".join(str(c) for c in chunks) if chunks else ""
        recs = [r for r in _parse_his_records(his_text, workspace=workspace)
                if r["role"] == "assistant"]
        if recs:
            return recs[-1]["content"].strip()
    except Exception:
        pass
    return ""