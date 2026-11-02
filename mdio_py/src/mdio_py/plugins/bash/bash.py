import json
from typing import Any

from ..plugin_base import PluginBase
from mdio_py.core.error import MdioError, ErrCode
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.text_utils import unquote_triple

from .service import BashService


_HELP = """# BASH
- @BASH：命令经管道或 -cmd；默认 stream，无输入列概览。
- @BASH.stream：显式同义。
- @BASH -sess <name>：在命名会话上下文执行。
- @BASH.spawn：异步执行，返回 handle id（P2）。
- @BASH.sess：会话列表；-create/-inspect/-restart/-close/-kill。
- @BASH.inspect -id <id>：查看 execution handle（-last 为 P1）。
- @BASH.kill -id <id>：终止 handle。
- @BASH.tools：argv / python 工具。
- @BASH.tools.edit：DSH 兼容编辑（view/create/edit/str_replace/insert/write）。
"""


class BASHPlugin(PluginBase):
    @property
    def protocol_name(self) -> str:
        return "BASH"

    def __init__(self):
        self._service = BashService(self)

    def _unquote(self, val: Any) -> str:
        return unquote_triple(val)

    def _normalize_chain(self, chain: list) -> list[str]:
        parts: list[str] = []
        for item in chain:
            if isinstance(item, str):
                parts.append(self._unquote(item))
                continue
            if isinstance(item, dict):
                method = item.get("method", "run")
                parts.append(self._unquote(method))
                continue
            parts.append(self._unquote(item))
        return [p for p in parts if p]

    def _has_input(self, input_data: Any) -> bool:
        if input_data is None or hasattr(input_data, "__dataclass_fields__"):
            return False
        return str(input_data) != ""

    def _resolve_command(self, input_data: Any) -> str:
        if input_data is None or hasattr(input_data, "__dataclass_fields__"):
            return ""
        return self._unquote(input_data)

    def _stringify_pipe_input(self, input_data: Any) -> str:
        if input_data is None or hasattr(input_data, "__dataclass_fields__"):
            return ""
        if isinstance(input_data, (dict, list)):
            return json.dumps(input_data, ensure_ascii=False)
        return str(input_data)

    def _resolve_oneshot_request(self, input_data: Any, kwargs: dict) -> tuple[str, str | None]:
        cmd_override = self._unquote(kwargs.get("-cmd"))
        has_stdin = "-stdin" in kwargs
        stdin_binding = kwargs.get("-stdin")
        has_input = self._has_input(input_data)
        command = self._resolve_command(input_data)

        if cmd_override:
            if has_stdin:
                if stdin_binding is True:
                    raise ValueError('invalid -stdin: use "-stdin _" or provide an explicit value')
                return cmd_override, self._stringify_pipe_input(stdin_binding)
            if has_input:
                raise ValueError('run with -cmd ignores pipe input; add "-stdin _" to pass current data to stdin')
            return cmd_override, None

        if has_stdin:
            raise ValueError('-stdin requires -cmd "..."')

        return command, None

    def _resolve_session_request(self, input_data: Any, kwargs: dict) -> str:
        if "-stdin" in kwargs:
            raise ValueError("-stdin is not supported with -sess")
        cmd_override = self._unquote(kwargs.get("-cmd"))
        has_input = self._has_input(input_data)
        if cmd_override:
            if has_input:
                raise ValueError("-cmd ignores pipe input; pipe the command or use -cmd, not both")
            return cmd_override
        return self._resolve_command(input_data)

    def _legacy_error(self, parts: list[str]) -> str:
        if parts and parts[0] == "run":
            return "- mdio_error: [@BASH] @BASH.run 已删除；用 @BASH（默认 stream）。"
        return "- mdio_error: [@BASH] 旧语法已删除；用 @BASH -sess <name> 或 @BASH.sess。-pid 改用 -id。"

    def _execution_target(self, kwargs: dict) -> tuple[str, str]:
        sess_raw = kwargs.get("-sess")
        if sess_raw not in (None, ""):
            return "session", self._unquote(sess_raw)
        return "host", ""

    async def _run_execution(
        self, workspace, ctx: RuntimeContext, input_data: Any, kwargs: dict,
        *, mode: str, allow_list: bool = False,
    ):
        target, name = self._execution_target(kwargs)

        if mode == "foreground":
            if "-cb" in kwargs:
                raise ValueError("-cb is supported only on @BASH.spawn")
            if target == "session":
                if not name or name == "True":
                    raise ValueError("-sess requires a session name")
                command = self._resolve_session_request(input_data, kwargs)
                if not command:
                    return "- mdio_error: [@BASH] -sess requires command input."
                clean = {k: v for k, v in kwargs.items() if k not in ("-cmd", "-stdin", "-sess")}
                return self._service.context_stream(workspace, ctx, name, command, clean)
            command, stdin_text = self._resolve_oneshot_request(input_data, kwargs)
            if not command:
                if allow_list and not self._has_input(input_data) and not kwargs.get("-cmd"):
                    return await self._service.list_state(workspace, ctx)
                return "- mdio_error: [@BASH] command required."
            return self._service.stream_oneshot(
                workspace, ctx, command, kwargs, stdin_text=stdin_text,
            )

        if target == "session":
            if not name:
                return "- mdio_error: [@BASH.spawn] -sess requires a session name."
            cb_raw = kwargs.get("-cb")
            cb_code = ""
            if cb_raw is not None:
                cb_code = self._unquote(cb_raw)
                if cb_code == "_":
                    cmd_override = self._unquote(kwargs.get("-cmd"))
                    if not cmd_override:
                        return "- mdio_error: [@BASH.spawn] -cb _ requires -cmd."
                    command = cmd_override
                    cb_code = self._stringify_pipe_input(input_data)
                else:
                    command = self._resolve_session_request(input_data, kwargs)
            else:
                command = self._resolve_session_request(input_data, kwargs)
            if not command:
                return "- mdio_error: [@BASH.spawn] command required."
            clean = {k: v for k, v in kwargs.items() if k not in ("-cmd", "-stdin", "-sess", "-cb")}
            return await self._service.spawn_session(
                workspace, ctx, name, command, clean, cb_code=cb_code,
            )

        if "-stdin" in kwargs and "-cb" in kwargs and self._unquote(kwargs.get("-cb")) == "_":
            return "- mdio_error: [@BASH.spawn] -stdin cannot combine with -cb _."
        cb_raw = kwargs.get("-cb")
        cb_code = ""
        if cb_raw is not None:
            cb_code = self._unquote(cb_raw)
            if cb_code == "_":
                if "-stdin" in kwargs:
                    return "- mdio_error: [@BASH.spawn] -stdin cannot combine with -cb _."
                cmd_override = self._unquote(kwargs.get("-cmd"))
                if not cmd_override:
                    return "- mdio_error: [@BASH.spawn] -cb _ requires -cmd (the pipe carries cb code)."
                command = cmd_override
                cb_code = self._stringify_pipe_input(input_data)
                stdin_text = None
            else:
                command, stdin_text = self._resolve_oneshot_request(input_data, kwargs)
        else:
            command, stdin_text = self._resolve_oneshot_request(input_data, kwargs)
        if not command:
            return "- mdio_error: [@BASH.spawn] command required."
        return await self._service.spawn_oneshot(
            workspace, ctx, command, kwargs, stdin_text=stdin_text, cb_code=cb_code,
        )

    async def _run_sess(self, workspace, ctx: RuntimeContext, input_data: Any, kwargs: dict) -> Any:
        if self._has_input(input_data):
            return "- mdio_error: [@BASH.sess] does not accept pipe input."
        actions = (
            ("-create", "create"),
            ("-inspect", "inspect"),
            ("-restart", "restart"),
            ("-close", "close"),
            ("-kill", "kill"),
        )
        chosen = [(flag, action) for flag, action in actions if flag in kwargs]
        if not chosen:
            return await self._service.list_state(workspace, ctx)
        if len(chosen) > 1:
            return "- mdio_error: [@BASH.sess] use exactly one of -create/-inspect/-restart/-close/-kill."
        flag, action = chosen[0]
        name = self._unquote(kwargs.get(flag))
        if not name or name == "True":
            return f"- mdio_error: [@BASH.sess] {flag} requires a session name."
        clean = {k: v for k, v in kwargs.items() if k != flag}
        if action == "create":
            return await self._service.init_context(workspace, ctx, name, clean)
        if action == "inspect":
            return await self._service.inspect_session(workspace, ctx, name)
        if action == "restart":
            return await self._service.restart_context(workspace, ctx, name, clean)
        if action == "close":
            return await self._service.close_context(workspace, ctx, name)
        return await self._service.kill_context(workspace, ctx, name)

    async def _run_tools(self, workspace, ctx: RuntimeContext, parts: list[str], input_data: Any, kwargs: dict):
        if len(parts) == 1:
            if self._has_input(input_data):
                return "- mdio_error: [@BASH.tools] does not accept pipe input."
            return await self._service.list_tools(workspace, ctx)
        tool_id = parts[1]
        if "-op" not in kwargs and "-args" not in kwargs:
            return await self._service.tool_help(workspace, ctx, tool_id)
        stdin_text = self._stringify_pipe_input(input_data) if self._has_input(input_data) else None
        return await self._service.run_tool(workspace, ctx, tool_id, kwargs, stdin_text=stdin_text)

    async def _run_inspect(self, workspace, ctx: RuntimeContext, input_data: Any, kwargs: dict) -> str:
        if self._has_input(input_data):
            return "- mdio_error: [@BASH.inspect] does not accept pipe input."
        if "-pid" in kwargs:
            return "- mdio_error: [@BASH.inspect] -pid 已删除；用 -id。"
        if "-last" in kwargs:
            run_id = await self._service.last_run_id(workspace, ctx)
            if not run_id:
                return "- mdio_error: [@BASH.inspect] no recent execution handle."
            return await self._service.inspect_handle(workspace, ctx, run_id)
        run_id = self._unquote(kwargs.get("-id"))
        if not run_id:
            return "- mdio_error: [@BASH.inspect] inspect requires -id <id>."
        return await self._service.inspect_handle(workspace, ctx, run_id)

    async def _run_kill(self, workspace, ctx: RuntimeContext, input_data: Any, kwargs: dict) -> str:
        if self._has_input(input_data):
            return "- mdio_error: [@BASH.kill] does not accept pipe input."
        if "-pid" in kwargs:
            return "- mdio_error: [@BASH.kill] -pid 已删除；用 -id。"
        run_id = self._unquote(kwargs.get("-id"))
        if not run_id:
            return "- mdio_error: [@BASH.kill] kill requires -id <id>."
        return await self._service.kill_run(workspace, ctx, run_id)

    async def dispose_wt(self, workspace, wt_uid: str) -> None:
        await self._service.dispose_wt(workspace, wt_uid)

    async def __call__(
        self,
        workspace,
        input_data: Any,
        chain: list,
        ctx: RuntimeContext,
        kwargs: dict,
    ) -> Any:
        try:
            await self._service.ensure_docs(workspace, ctx.wt_uid)

            parts = self._normalize_chain(chain)
            if not parts:
                return await self._run_execution(
                    workspace, ctx, input_data, kwargs, mode="foreground", allow_list=True,
                )

            head = parts[0]
            if len(parts) > 1:
                if head == "tools":
                    return await self._run_tools(workspace, ctx, parts, input_data, kwargs)
                return self._legacy_error(parts)

            if head == "stream":
                return await self._run_execution(
                    workspace, ctx, input_data, kwargs, mode="foreground", allow_list=False,
                )
            if head == "spawn":
                return await self._run_execution(
                    workspace, ctx, input_data, kwargs, mode="spawn",
                )
            if head == "sess":
                return await self._run_sess(workspace, ctx, input_data, kwargs)
            if head == "inspect":
                return await self._run_inspect(workspace, ctx, input_data, kwargs)
            if head == "kill":
                return await self._run_kill(workspace, ctx, input_data, kwargs)
            if head == "tools":
                return await self._run_tools(workspace, ctx, parts, input_data, kwargs)
            if head == "help":
                return _HELP
            return self._legacy_error(parts)
        except ValueError as e:
            return f"- mdio_error: [@BASH] {e}"
        except Exception as e:
            return MdioError("plugin", ErrCode.PLG_CRASH, f"@BASH 崩溃: {e}", cause=e).to_markdown()
