import json
import uuid
import asyncio
from typing import Any
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.selector_builder import drill, by_tag

class PluginBase:
    # 输入流模式："value"（默认——asyncgen 输入先累积成串）|
    # "raw_stream"（流原样交付 input_data——interpreter 已实现）。
    # "chunk"（逐块应用）未实现：声明它不会生效（会静默走 value）。
    input_mode: str = "value"

    @property
    def protocol_name(self) -> str:
        raise NotImplementedError

    async def __call__(
        self,
        workspace, 
        input_data: Any, 
        chain: list,      
        ctx: RuntimeContext,
        kwargs: dict
    ) -> Any:
        pass

    async def dispose_wt(self, workspace, wt_uid: str) -> None:
        """Release plugin resources owned by one worktree (default: no-op)."""
        return None

#     async def _register_proc(self, workspace, ctx, identifier: str) -> str:
#         pid = f"wt_{ctx.wt_uid}_{identifier}_{uuid.uuid4().hex[:8]}"
#         proc_path = ctx.proc_path   # Q10
#         parent = ctx.caller_pid or "null"

#         cmd = f'''
# """### {pid}
# - status: IDLE
# - id: {identifier}
# - parent_pid: {parent}
# """ | +> "{proc_path}::PROCESS_TABLE.{self.protocol_name}" -f true
# '''
#         await workspace.runtime.exec(cmd, ctx=ctx)
#         workspace.active_tasks[pid] = asyncio.current_task()
#         return pid

    async def _register_proc(
        self, workspace, ctx, identifier: str, ppid: str = "root", 
        payload: dict = None, exact_pid: str = None, register_task: bool = True,
        suppress_hooks: bool = False,
    ) -> str:
        """
        通用进程注册接口
        """
        # 不额外加 Protocol 中缀：注册接口只认 plugin 协议名
        if exact_pid:
            pid = exact_pid
        else:
            pid = f"{self.protocol_name}_{identifier}_{uuid.uuid4().hex[:8]}"
        proc_path = ctx.proc_path
        payload = payload or {}
        status = payload.get("status", "IDLE")

        # Ensure heading + write ppid/status as bullet fields
        container = f"PROCESS_TABLE.{self.protocol_name}"
        workspace.write_locator(
            ctx.wt_uid, proc_path, drill(f"{container}.{pid}"),
            "+>", "", auto_scaffold=True, suppress_hooks=suppress_hooks,
        )
        workspace.write_locator(
            ctx.wt_uid, proc_path, by_tag(f"{container}.{pid}", "ppid"),
            "/>", ppid, auto_scaffold=True, suppress_hooks=suppress_hooks,
        )
        workspace.write_locator(
            ctx.wt_uid, proc_path, by_tag(f"{container}.{pid}", "status"),
            "/>", status, auto_scaffold=True, suppress_hooks=suppress_hooks,
        )

        # 同时写 id 字段，供按 id 查找的调用方使用
        if "id" not in payload and not exact_pid:
            workspace.write_locator(
                ctx.wt_uid, proc_path, by_tag(f"{container}.{pid}", "id"),
                "/>", identifier, auto_scaffold=True, suppress_hooks=suppress_hooks,
            )

        for k, v in payload.items():
            if k == "status":
                continue
            workspace.write_locator(
                ctx.wt_uid, proc_path, by_tag(f"{container}.{pid}", k),
                "/>", str(v), auto_scaffold=True, suppress_hooks=suppress_hooks,
            )
        
        # 把常驻任务安全地放进该时空专属的字典坑位
        if register_task:
            wt_tasks = workspace.active_tasks.setdefault(ctx.wt_uid, {})
            wt_tasks[pid] = asyncio.current_task()
            
        return pid

    async def _unregister_proc(self, workspace, ctx, pid: str, is_daemon: bool = True):
        proc_path = ctx.proc_path
        workspace.write_locator(
            ctx.wt_uid, proc_path,
            drill(f"PROCESS_TABLE.{self.protocol_name}.{pid}"),
            "->", "", auto_scaffold=False,
        )

        # 清理本宇宙的内存句柄
        wt_tasks = workspace.active_tasks.get(ctx.wt_uid, {})
        wt_tasks.pop(pid, None)
        # print(f"[UNREGISTER] active_tasks keys after pop: {list(workspace.active_tasks.keys())}")

    async def _get_god_secrets(self, workspace, model_alias: str) -> dict:
        """读 llm_config.md 的整个别名节 → 字典（整节一次读出，键表不再是白名单）。

        路径：节原文（raw 投影，含内容行）→ tojson（引擎投影）→ 消费侧提取 `- k: v`。
        白名单外的键一律保留，交由能力预检（backend.precheck_capabilities）判定。
        """
        config_path = "/.mds/llm_config.md"
        try:
            chunks = workspace.read_section_chunks(
                "genesis", config_path, f"LLM_CONFIG.{model_alias}")
        except Exception:
            return {}
        text = "\n".join(str(c) for c in (chunks or []) if c)
        if not text.strip():
            return {}
        try:
            from mdio_py.core import ast_utils
            env = json.loads(workspace.to_json_text(text))
            config = ast_utils.bullets_to_kv(env)
        except Exception:
            return {}
        # 值清洗：extra_body 是内嵌 JSON；其余保持纯文本（与逐键读取一致）
        if "extra_body" in config:
            try:
                config["extra_body"] = json.loads(config["extra_body"])
            except Exception:
                config["extra_body"] = {}
        return config


# 调试打点工具函数

class Colors:
    # 前景色
    BLACK = '\033[30m'
    RED = '\033[31m'
    GREEN = '\033[32m'
    YELLOW = '\033[33m'
    BLUE = '\033[34m'
    MAGENTA = '\033[35m'
    CYAN = '\033[36m'
    WHITE = '\033[37m'

    # 背景色
    BG_BLACK = '\033[40m'
    BG_RED = '\033[41m'
    BG_GREEN = '\033[42m'
    BG_YELLOW = '\033[43m'
    BG_BLUE = '\033[44m'
    BG_MAGENTA = '\033[45m'
    BG_CYAN = '\033[46m'
    BG_WHITE = '\033[47m'

    # 样式
    BOLD = '\033[1m'
    UNDERLINE = '\033[4m'
    BLINK = '\033[5m'
    REVERSE = '\033[7m'

    # 重置
    RESET = '\033[0m'
