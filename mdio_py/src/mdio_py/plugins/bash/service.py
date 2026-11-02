from __future__ import annotations

import asyncio
import importlib
import inspect
import json
import os
import shlex
import time
import uuid
from typing import Any

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.selector_builder import SelectorBuilder, by_tag

from .backends import LocalBashBackend
from .models import (
    BashConfig,
    BashDisposeError,
    BashDisposeOutcome,
    BashDisposeReport,
    BashExecutionSpec,
    BashHandle,
    BashResult,
    BashStarted,
    BashStreamHandle,
)
from .process_probe import BashProcessProbe, ProcessIdentity
from .state import BashState, _bq_lines, _unquote

try:
    from nah.bash import classify_command
    _NAH_AVAILABLE = True
except ImportError:
    classify_command = None  # type: ignore[assignment]
    _NAH_AVAILABLE = False


def _host_cwd(raw: str) -> str:
    """-cwd → 宿主绝对路径（realpath 链唯一实现）。"""
    return os.path.realpath(os.path.abspath(os.path.expanduser(_unquote(raw))))


class BashService:
    def __init__(self, plugin):
        self._plugin = plugin
        self._backend = LocalBashBackend()
        self._state = BashState(plugin)
        self._live: dict[tuple[str, str], BashHandle] = {}
        self._generations: dict[str, int] = {}
        self._probe = BashProcessProbe()

    def _check_sh_guard(self, command: str, enabled: bool = True) -> str | None:
        """nah — context-aware safety guard. Only blocks, never asks."""
        if not enabled or not _NAH_AVAILABLE:
            return None
        try:
            result = classify_command(command)
        except Exception:
            return None
        if result.final_decision in ("block",):
            return f"- mdio_error: [@BASH] blocked by nah — {result.final_decision}: {result.reason}"
        return None

    @staticmethod
    def _flag_enabled(kwargs: dict, flag: str) -> bool:
        raw = kwargs.get(flag, "true")
        return str(raw).strip().lower() not in ("false", "0", "no")

    @staticmethod
    def _normalized_state(result: BashResult) -> str:
        if result.status == "CANCELLED":
            return "TIMEOUT" if str(result.exit_code) == "TIMEOUT" else "KILLED"
        if result.status == "DONE":
            return "DONE"
        return "FAILED"

    @staticmethod
    def _doc(fields: dict[str, Any], stdout: str, stderr: str) -> str:
        order = (
            "state", "id", "kind", "mode", "cmd", "session", "run_id", "tool", "op", "cwd", "shell",
            "os_pid", "create_time", "pgid", "sid", "liveness", "last_seen", "confirmed_at",
            "started", "finished", "duration_ms", "exit", "signal", "timeout",
            "bytes", "truncated", "staging", "ctx", "cb", "cb_status",
        )
        lines = ["# BASH"]
        for key in order:
            if key in fields and fields[key] is not None:
                lines.append(f"- {key}: {str(fields[key]).replace(chr(10), ' ')}")
        lines.extend(["## RES", "- out:"])
        enc = _bq_lines(stdout)
        if enc:
            lines.append(enc)
        lines.extend(["## ERR", "- err:"])
        enc = _bq_lines(stderr)
        if enc:
            lines.append(enc)
        return "\n".join(lines)

    async def ensure_docs(self, workspace, wt_uid: str) -> None:
        await self._state.ensure_docs(workspace, wt_uid)

    async def list_state(self, workspace, ctx: RuntimeContext) -> str:
        config = await self._state.load_global_config(workspace, ctx)
        lines = [
            "# BASH",
            f"- backend: {config.backend}",
            f"- shell: {config.shell}",
            f"- timeout: {config.timeout}",
            f"- session_engine: {config.session_engine}",
        ]

        active_runs = []
        try:
            active_runs = workspace.read_section_chunks(ctx.wt_uid, ctx.proc_path, "PROCESS_TABLE.BASH", children=True)
        except Exception:
            active_runs = []

        lines.append("## ACTIVE")
        if not active_runs:
            lines.append("- none")
        else:
            for chunk in active_runs:
                text = str(chunk).strip()
                head = text.split("\n", 1)[0].lstrip("#").strip()
                kind = self._state.extract_strip_value(text, "kind", workspace)
                status = self._state.extract_strip_value(text, "status", workspace)
                liveness = self._state.extract_strip_value(text, "liveness", workspace)
                suffix = f", liveness={liveness}" if liveness else ""
                if kind:
                    lines.append(f"- {head}: kind={kind}, status={status or 'UNKNOWN'}{suffix}")
                else:
                    lines.append(f"- {head}: status={status or 'UNKNOWN'}{suffix}")

        names = sorted(set(self._state.list_context_names(workspace, ctx.wt_uid)) | {h.name for h in self._backend.list_contexts(ctx.wt_uid)})
        lines.append("## CONTEXTS")
        if not names:
            lines.append("- none")
        else:
            for name in names:
                handle = await self._backend.get_context(ctx.wt_uid, name)
                status = handle.status if handle is not None else "CONFIG_ONLY"
                proc = self._state.read_proc_fields(workspace, ctx, name)
                liveness = proc.get("liveness", "")
                suffix = f", liveness={liveness}" if liveness else ""
                lines.append(f"- {name}: status={status}{suffix}")

        last_handle = self._state.last_context_handle(workspace, ctx.wt_uid)
        if last_handle:
            parts = self._state.find_context_cmd(workspace, ctx, last_handle)
            if parts is not None:
                fields, _stdout, _stderr = parts
                lines.append("## LAST_SESSION_HANDLE")
                lines.append(
                    f"- {last_handle}: state={fields.get('state', 'UNKNOWN')}, "
                    f"session={fields.get('session', '')}, cmd={fields.get('cmd', '')}"
                )

        recent_chunks = []
        try:
            recent_chunks = workspace.read_section_chunks(ctx.wt_uid, "/var/bash.md", "EXEC", children=True)
        except Exception:
            recent_chunks = []

        lines.append("## RECENT")
        if not recent_chunks:
            lines.append("- none")
        else:
            for chunk in recent_chunks[-5:]:
                text = str(chunk).strip()
                run_id = text.split("\n", 1)[0].lstrip("#").strip()
                exit_code = self._state.extract_strip_value(text, "exit", workspace)
                status = self._state.extract_strip_value(text, "status", workspace)
                suffix = f"status={status}" if status else ""
                if exit_code:
                    suffix = f"{suffix}, exit={exit_code}".strip(", ")
                lines.append(f"- {run_id}: {suffix}" if suffix else f"- {run_id}")

        return "\n".join(lines)

    async def show_context(self, workspace, ctx: RuntimeContext, name: str) -> str:
        config = await self._state.load_context_config(workspace, ctx, name)
        handle = await self._backend.get_context(ctx.wt_uid, name)
        recent = self._state.compact_display_block(await self._state.read_context_var_block(workspace, ctx, name))

        if config is None and handle is None and not recent:
            return f"- mdio_error: [@BASH] context '{name}' not found."

        lines = ["# BASH_CONTEXT", f"- name: {name}"]
        lines.append("## CONFIG")
        if config is None:
            lines.append("- inherited: true")
        else:
            lines.extend(self._state.config_lines(config))

        lines.append("## PROC")
        if handle is None:
            lines.append("- status: CONFIG_ONLY")
        else:
            lines.append(f"- status: {handle.status}")
            lines.append(f"- shell: {handle.shell}")
            lines.append(f"- cwd: {handle.cwd}")
            lines.append(f"- engine: {handle.engine}")
            if handle.current_cmd:
                lines.append(f"- current_cmd: {handle.current_cmd}")

        if recent:
            lines.append("## HISTORY")
            lines.append(recent)

        return "\n".join(lines)

    async def init_context(self, workspace, ctx: RuntimeContext, name: str, kwargs: dict) -> str:
        updates = await self._extract_context_updates(kwargs)
        await self._state.ensure_context_config_head(workspace, ctx.wt_uid, name)
        for key, value in updates.items():
            workspace.write_locator(
                ctx.wt_uid,
                "/.mds/bash.md",
                by_tag(f"CONTEXTS.{name}", key),
                "/>",
                value,
                auto_scaffold=True,
            )
        config = await self._state.load_effective_context_config(workspace, ctx, name)
        await self._state.write_context_root(
            workspace,
            ctx.wt_uid,
            name,
            {
                "shell": config.shell,
                "cwd": config.cwd,
                "timeout": config.timeout,
                "engine": config.session_engine,
                "status": "CONFIGURED",
            },
        )
        return f"- mdio_info: [@BASH] context '{name}' configured."

    @staticmethod
    def _started_fields(started: BashStarted) -> dict[str, Any]:
        return {
            "os_pid": started.os_pid,
            "create_time": started.create_time,
            "pgid": started.pgid,
            "sid": started.sid,
            "liveness": started.liveness,
            "last_seen": repr(time.time()),
        }

    async def _record_started(self, workspace, ctx: RuntimeContext, key: tuple[str, str], started: BashStarted) -> None:
        live = self._live.get(key)
        if live is not None:
            live.update(self._started_fields(started))
        await self._state.update_run_proc(
            workspace, ctx, key[1], self._started_fields(started), suppress_hooks=True,
        )

    def _on_start(self, workspace, ctx: RuntimeContext, key: tuple[str, str]):
        async def callback(started: BashStarted) -> None:
            await self._record_started(workspace, ctx, key, started)
        return callback

    async def _start_run(
        self, workspace, ctx: RuntimeContext, command: str, config: BashConfig,
        *, kind: str = "run", run_id: str | None = None,
    ) -> tuple[str, dict[str, Any]]:
        run_id = run_id or uuid.uuid4().hex[:8]
        live = BashHandle(
            id=run_id, kind=kind, target="host", state="RUNNING",
            started=time.time(), command=command, config=config,
            cwd=config.cwd, shell=config.shell, timeout=config.timeout,
            gen=self._generations.get(ctx.wt_uid, 0),
        )
        self._live[(ctx.wt_uid, run_id)] = live
        await self._plugin._register_proc(
            workspace, ctx, identifier=run_id, ppid=ctx.caller_pid or "root",
            payload={"kind": kind}, exact_pid=run_id,
        )
        await self._state.update_run_proc(
            workspace, ctx, run_id, {"status": "RUNNING", "kind": kind},
        )
        return run_id, live

    async def _drive_stream(self, live, stream, fallback: BashResult | None = None):
        try:
            async for chunk in stream.stream:
                text = str(chunk)
                if live is not None:
                    live["stdout"] = (live.get("stdout") or "") + text
                yield text
        finally:
            try:
                await stream.stream.aclose()
            except BaseException:
                pass
            result = None
            try:
                result = await asyncio.wait_for(asyncio.shield(stream.done), timeout=5)
            except BaseException:
                pass
            if result is None:
                result = fallback or BashResult(
                    status="CANCELLED", exit_code="KILLED", liveness="gone",
                )
            if live is not None:
                live["result"] = result

    async def _start_handle(
        self, workspace, ctx: RuntimeContext, *, target: str, kind: str,
        command: str, config: BashConfig | None = None, kwargs: dict | None = None,
        session: str | None = None, handle_id: str | None = None,
    ) -> BashHandle:
        if target == "host":
            if config is None:
                config = await self._build_execution_spec(
                    workspace, ctx, None, kwargs or {}, allow_runtime_session_updates=True,
                )
            _run_id, handle = await self._start_run(
                workspace, ctx, command, config, kind=kind, run_id=handle_id,
            )
            return handle
        name = session or ""
        context_config, cmd_id = await self._start_context_command(
            workspace, ctx, name, command, kwargs or {}, handle_id=handle_id,
        )
        handle = BashHandle(
            id=cmd_id, kind=kind, target="session", session=name,
            state="RUNNING", started=time.time(), command=command,
            config=context_config, cwd=context_config.cwd,
            shell=context_config.shell, timeout=context_config.timeout,
            gen=self._generations.get(ctx.wt_uid, 0),
        )
        self._live[(ctx.wt_uid, cmd_id)] = handle
        return handle

    async def _finish_handle(
        self, workspace, ctx: RuntimeContext, handle: BashHandle, result: BashResult,
        *, meta: dict[str, Any] | None = None, cb_code: str = "", force: bool = False,
        stdin_text: str | None = None,
    ) -> None:
        if handle.state in ("DONE", "FAILED", "TIMEOUT", "KILLED") and not force:
            handle.result = result
            return
        if handle.target == "session":
            config = handle.config or BashConfig()
            await self._finish_context_command(
                workspace, ctx, handle.session, handle.id, handle.command,
                config, result, meta=meta, run_id=handle.id,
            )
        else:
            await self._finalize_run(
                workspace, ctx, handle.id, handle.command, result,
                stdin_text=stdin_text, kind=handle.kind, cb_code=cb_code, meta=meta,
                force=force, live_override=handle,
            )
        handle.result = result
        handle.state = self._normalized_state(result)
        self._live.pop((ctx.wt_uid, handle.id), None)

    def stream_oneshot(self, workspace, ctx: RuntimeContext, command: str, kwargs: dict, stdin_text: str | None = None, meta: dict[str, Any] | None = None):
        async def _err(msg: str):
            yield msg

        if not command:
            return _err('- mdio_error: [@BASH] stream requires command input.')
        if guard := self._check_sh_guard(command, enabled=self._flag_enabled(kwargs, "-nah")):
            return _err(guard)

        async def _runner():
            config = await self._build_execution_spec(workspace, ctx, None, kwargs, allow_runtime_session_updates=True)
            handle = await self._start_handle(
                workspace, ctx, target="host", kind="run", command=command, config=config,
            )
            run_id = handle.id
            live = handle
            fallback = BashResult(
                status="CANCELLED", exit_code="KILLED", shell=config.shell,
                cwd=config.cwd, timeout=config.timeout, liveness="gone",
            )
            try:
                stream_handle = self._backend.stream(
                    ctx.wt_uid, run_id, self._spec_from_config(config, command, stdin_text=stdin_text),
                    on_start=self._on_start(workspace, ctx, (ctx.wt_uid, run_id)),
                )
                async for chunk in self._drive_stream(live, stream_handle, fallback):
                    yield chunk
            finally:
                result = live.get("result") or fallback
                await self._finish_handle(
                    workspace, ctx, handle, result, meta=meta, stdin_text=stdin_text,
                )
            if result.status != "DONE":
                yield "\n" + self._render_error(run_id, result)

        return _runner()

    async def spawn_oneshot(
        self, workspace, ctx: RuntimeContext, command: str, kwargs: dict,
        stdin_text: str | None = None, cb_code: str = "",
    ) -> str:
        if not command:
            return "- mdio_error: [@BASH] spawn requires command input."
        if guard := self._check_sh_guard(command, enabled=self._flag_enabled(kwargs, "-nah")):
            return guard
        config = await self._build_execution_spec(workspace, ctx, None, kwargs, allow_runtime_session_updates=True)
        live = await self._start_handle(
            workspace, ctx, target="host", kind="spawn", command=command, config=config,
        )
        task = asyncio.create_task(self._spawn_runner(
            workspace, ctx, live, command, config, kwargs,
            stdin_text=stdin_text, cb_code=cb_code,
        ))
        live.task = task
        return f"- mdio_info: [@BASH] started {live.id}"

    async def _spawn_runner(self, workspace, ctx, live, command, config, kwargs, *, stdin_text=None, cb_code=""):
        run_id = live.id
        key = (ctx.wt_uid, run_id)
        fallback = BashResult(
            status="CANCELLED", exit_code="KILLED", shell=config.shell,
            cwd=config.cwd, timeout=config.timeout, liveness="gone",
        )
        finalized = False
        try:
            stream_handle = self._backend.stream(
                ctx.wt_uid, run_id,
                self._spec_from_config(config, command, stdin_text=stdin_text),
                on_start=self._on_start(workspace, ctx, (ctx.wt_uid, run_id)),
            )
            async for _chunk in self._drive_stream(live, stream_handle, fallback):
                pass
            result = live.get("result") or fallback
            await self._finish_handle(
                workspace, ctx, live, result, cb_code=cb_code, stdin_text=stdin_text,
            )
            finalized = True
            if cb_code:
                await self._run_cb(workspace, ctx, run_id, cb_code)
        except BaseException:
            if not finalized:
                live = self._live.get(key, live)
                result = live.get("result") if live is not None else None
                result = result or fallback
                try:
                    await self._finish_handle(workspace, ctx, live, result)
                except Exception:
                    pass
            raise
        finally:
            self._live.pop(key, None)

    async def _run_cb(self, workspace, ctx: RuntimeContext, run_id: str, cb_code: str) -> None:
        if not cb_code:
            return
        status = "DONE"
        try:
            code = f'@BASH.inspect -id "{run_id}" | {cb_code}'
            result = await workspace.runtime.exec(code, ctx=ctx)
            if str(result).startswith("- mdio_error"):
                status = "FAILED"
        except Exception:
            status = "FAILED"
        try:
            await self._state.update_exec_field(workspace, ctx.wt_uid, run_id, "cb_status", status)
        except Exception:
            pass

    async def _finish_confirmed_handle(
        self, workspace, ctx: RuntimeContext, handle_id: str, kind: str,
        live: BashHandle | dict[str, Any], confirmed_at: str,
    ) -> None:
        if isinstance(live, BashHandle):
            if live.target == "session":
                return
            result = BashResult(
                stdout=live.stdout, stderr="", exit_code="KILLED", status="CANCELLED",
                shell=live.shell, cwd=live.cwd, timeout=live.timeout,
                os_pid=live.os_pid, pgid=live.pgid,
                create_time=live.create_time, sid=live.sid, liveness="gone",
            )
            await self._finish_handle(workspace, ctx, live, result, force=True)
            return
        if live.get("target") == "session" or (kind == "spawn" and live.get("session")):
            return
        proc = self._state.read_proc_fields(workspace, ctx, handle_id)
        identity = {
            "os_pid": proc.get("os_pid") or live.get("os_pid", ""),
            "create_time": proc.get("create_time") or live.get("create_time", ""),
            "pgid": proc.get("pgid") or live.get("pgid", ""),
            "sid": proc.get("sid") or live.get("sid", ""),
        }
        result = BashResult(
            stdout=str(live.get("stdout") or ""), stderr="",
            exit_code="KILLED", status="CANCELLED",
            shell=live.get("shell", ""), cwd=live.get("cwd", ""),
            timeout=int(live.get("timeout") or 0),
            os_pid=identity["os_pid"], pgid=identity["pgid"],
            create_time=identity["create_time"], sid=identity["sid"],
            liveness="gone",
        )
        if kind == "context":
            await self._finish_disposed_shell(
                workspace, ctx, handle_id, proc, result, identity, confirmed_at,
            )
            return
        handle = BashHandle(
            id=handle_id, kind=kind, target="host", state="RUNNING",
            started=float(live.get("started") or time.time()),
            command=live.get("command", ""), cwd=live.get("cwd", ""),
            shell=live.get("shell", ""), timeout=int(live.get("timeout") or 0),
            stdout=str(live.get("stdout") or ""), os_pid=identity["os_pid"],
            create_time=identity["create_time"], pgid=identity["pgid"],
            sid=identity["sid"], liveness="gone", result=result,
        )
        await self._finish_handle(workspace, ctx, handle, result, force=True)

    async def _finish_disposed_shell(
        self, workspace, ctx: RuntimeContext, name: str, proc: dict[str, str],
        result: BashResult, identity: dict[str, str], confirmed_at: str,
    ) -> None:
        cmd_id = proc.get("current_cmd", "")
        if cmd_id:
            config = await self._state.load_effective_context_config(workspace, ctx, name)
            if config is None:
                config = BashConfig()
            await self._finish_context_command(
                workspace, ctx, name, cmd_id, proc.get("cmd", ""),
                config, result, terminal_status="KILLED",
            )
            return
        await self._state.update_context_proc(
            workspace, ctx, name,
            {"status": "KILLED", "liveness": "gone", "current_cmd": "",
             "confirmed_at": confirmed_at, **identity},
        )
        await self._state.write_context_root(workspace, ctx.wt_uid, name, {"status": "KILLED"})
        await self._plugin._unregister_proc(workspace, ctx, name)

    def _identity_for_handle(
        self, workspace, ctx: RuntimeContext, handle_id: str, live: dict[str, Any],
    ) -> ProcessIdentity | None:
        proc = self._state.read_proc_fields(workspace, ctx, handle_id)
        os_pid = proc.get("os_pid") or live.get("os_pid", "")
        create_time = proc.get("create_time") or live.get("create_time", "")
        if not os_pid or not create_time:
            return None
        try:
            return ProcessIdentity(
                pid=int(os_pid),
                create_time=float(create_time),
                pgid=int(proc["pgid"]) if proc.get("pgid") else None,
                sid=int(proc["sid"]) if proc.get("sid") else None,
            )
        except (TypeError, ValueError):
            return None

    def _handle_confirmed_gone(
        self, workspace, ctx: RuntimeContext, handle_id: str, live: dict[str, Any],
    ) -> bool:
        identity = self._identity_for_handle(workspace, ctx, handle_id, live)
        if identity is None:
            return False
        return self._probe.probe(identity).liveness == "gone"

    async def _mark_unconfirmed_handle(
        self, workspace, ctx: RuntimeContext, handle_id: str, kind: str,
    ) -> None:
        fields = {"status": "UNCONFIRMED", "liveness": "unknown"}
        if kind == "context":
            await self._state.update_context_proc(workspace, ctx, handle_id, fields)
        else:
            await self._state.update_run_proc(workspace, ctx, handle_id, fields)

    def _bash_proc_ids(self, workspace, wt_uid: str) -> set[str]:
        ctx = RuntimeContext(wt_uid=wt_uid, vfs_path="/", caller_pid="root")
        try:
            chunks = workspace.read_section_chunks(
                wt_uid, ctx.proc_path, "PROCESS_TABLE.BASH", children=True,
            )
        except Exception:
            return set()
        ids: set[str] = set()
        for chunk in chunks:
            text = str(chunk).strip()
            if not text:
                continue
            head = text.split("\n", 1)[0].lstrip("#").strip()
            if head:
                ids.add(head)
        return ids

    async def dispose_wt(self, workspace, wt_uid: str) -> None:
        self._generations[wt_uid] = self._generations.get(wt_uid, 0) + 1
        ctx = RuntimeContext(wt_uid=wt_uid, vfs_path="/", caller_pid="root")
        live_snapshot = {
            key: value for key, value in self._live.items() if key[0] == wt_uid
        }
        live_keys = list(live_snapshot)
        bash_ids = self._bash_proc_ids(workspace, wt_uid) | {key[1] for key in live_keys}
        tasks: dict[int, asyncio.Task] = {}
        for value in live_snapshot.values():
            task = value.get("task")
            if task is not None and not task.done():
                tasks[id(task)] = task
        wt_tasks = workspace.active_tasks.get(wt_uid, {})
        current = asyncio.current_task()
        for pid, task in list(wt_tasks.items()):
            if pid not in bash_ids or task is None or task is current or task.done():
                continue
            tasks[id(task)] = task
        for task in tasks.values():
            task.cancel()
        if tasks:
            try:
                await asyncio.wait(tasks.values(), timeout=5)
            except Exception:
                pass
        for pid in list(wt_tasks):
            if pid in bash_ids:
                wt_tasks.pop(pid, None)
        errors: list[BaseException] = []
        try:
            report = await self._backend.dispose_wt(wt_uid)
        except Exception as e:
            report = BashDisposeReport()
            for key in live_keys:
                report.outcomes.append(
                    BashDisposeOutcome(handle_id=key[1], confirmed=False, error=str(e))
                )
        by_id = {outcome.handle_id: outcome for outcome in report.outcomes}
        confirmed_at = time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime())
        handled: set[str] = set()
        for outcome in report.outcomes:
            handle_id = outcome.handle_id
            handled.add(handle_id)
            live = live_snapshot.get((wt_uid, handle_id), {})
            proc = self._state.read_proc_fields(workspace, ctx, handle_id)
            kind = proc.get("kind") or live.get("kind") or "run"
            if outcome.confirmed:
                try:
                    await self._finish_confirmed_handle(
                        workspace, ctx, handle_id, kind, live, confirmed_at,
                    )
                except Exception as e:
                    errors.append(e)
            else:
                try:
                    await self._mark_unconfirmed_handle(workspace, ctx, handle_id, kind)
                except Exception as e:
                    errors.append(e)
                errors.append(RuntimeError(
                    f"dispose unconfirmed for {handle_id}: {outcome.error or 'survivors'}"
                ))
        for key in live_keys:
            handle_id = key[1]
            live = live_snapshot.get(key, {})
            if handle_id in handled:
                self._live.pop(key, None)
                continue
            session = live.get("session")
            session_outcome = by_id.get(session) if session else None
            proc = self._state.read_proc_fields(workspace, ctx, handle_id)
            kind = proc.get("kind") or live.get("kind") or "run"
            if session_outcome is not None and session_outcome.confirmed:
                try:
                    await self._finish_confirmed_handle(
                        workspace, ctx, handle_id, "spawn", live, confirmed_at,
                    )
                except Exception as e:
                    errors.append(e)
            elif live.get("target") == "session":
                # Session shell state is owned by the context outcome; do not
                # create a synthetic process row for the command handle.
                pass
            elif self._handle_confirmed_gone(workspace, ctx, handle_id, live):
                try:
                    await self._finish_confirmed_handle(
                        workspace, ctx, handle_id, kind, live, confirmed_at,
                    )
                except Exception as e:
                    errors.append(e)
            else:
                try:
                    await self._mark_unconfirmed_handle(workspace, ctx, handle_id, kind)
                except Exception as e:
                    errors.append(e)
                errors.append(RuntimeError(
                    f"dispose unconfirmed for {handle_id}: no backend outcome"
                ))
            self._live.pop(key, None)
        handled |= {
            key[1] for key in live_keys
        }
        for handle_id in self._bash_proc_ids(workspace, wt_uid) - handled:
            proc = self._state.read_proc_fields(workspace, ctx, handle_id)
            kind = proc.get("kind") or "run"
            identity = self._identity_for_handle(workspace, ctx, handle_id, {})
            if identity is None or self._probe.probe(identity).liveness == "gone":
                try:
                    await self._finish_confirmed_handle(
                        workspace, ctx, handle_id, kind,
                        {"kind": kind, "os_pid": proc.get("os_pid", ""),
                         "create_time": proc.get("create_time", ""),
                         "pgid": proc.get("pgid", ""), "sid": proc.get("sid", "")},
                        confirmed_at,
                    )
                except Exception as e:
                    errors.append(e)
                continue
            outcome = await asyncio.to_thread(
                self._probe.terminate_tree, handle_id, identity, 1000,
            )
            if outcome.confirmed:
                try:
                    await self._finish_confirmed_handle(
                        workspace, ctx, handle_id, kind,
                        {"kind": kind, "os_pid": proc.get("os_pid", ""),
                         "create_time": proc.get("create_time", ""),
                         "pgid": proc.get("pgid", ""), "sid": proc.get("sid", "")},
                        confirmed_at,
                    )
                except Exception as e:
                    errors.append(e)
            else:
                try:
                    await self._mark_unconfirmed_handle(workspace, ctx, handle_id, kind)
                except Exception as e:
                    errors.append(e)
                errors.append(RuntimeError(
                    f"dispose unconfirmed for {handle_id}: {outcome.error or 'survivors'}"
                ))
        if errors:
            raise BashDisposeError(f"BASH dispose failed for wt {wt_uid}", report)


    async def spawn_session(
        self, workspace, ctx: RuntimeContext, name: str, command: str,
        kwargs: dict, cb_code: str = "",
    ) -> str:
        if not command:
            return "- mdio_error: [@BASH] spawn -sess requires command input."
        run_id = uuid.uuid4().hex[:8]
        handle = await self._start_handle(
            workspace, ctx, target="session", kind="spawn", command=command,
            kwargs=kwargs, session=name, handle_id=run_id,
        )
        task = asyncio.create_task(self._spawn_session_runner(
            workspace, ctx, handle, command, kwargs, cb_code,
        ))
        handle.task = task
        return f"- mdio_info: [@BASH] started {handle.id}"

    async def _spawn_session_runner(self, workspace, ctx, handle, command, kwargs, cb_code):
        key = (ctx.wt_uid, handle.id)
        fallback = BashResult(
            status="CANCELLED", exit_code="KILLED", shell=handle.shell,
            cwd=handle.cwd, timeout=handle.timeout, liveness="gone",
        )
        try:
            stream = self._backend.context_stream(
                ctx.wt_uid, handle.session, handle.id, command, timeout=handle.timeout,
            )
            async for _chunk in self._drive_stream(handle, stream, fallback):
                pass
            result = handle.get("result") or fallback
            await self._finish_handle(workspace, ctx, handle, result)
            if cb_code:
                await self._run_cb(workspace, ctx, handle.id, cb_code)
        except BaseException:
            result = handle.get("result") or fallback
            try:
                await self._finish_handle(workspace, ctx, handle, result)
            except Exception:
                pass
            raise
        finally:
            self._live.pop(key, None)

    async def list_tools(self, workspace, ctx: RuntimeContext) -> str:
        tools = self._state.list_tools(workspace, ctx.wt_uid)
        if not tools:
            return "# BASH_TOOLS\n- none"
        lines = ["# BASH_TOOLS"]
        for tid, spec in sorted(tools.items()):
            ops = spec.get("ops", "run")
            lines.append(f"- {tid}: ops={ops}; {spec.get('help', '')}".rstrip())
        return "\n".join(lines)

    async def tool_help(self, workspace, ctx: RuntimeContext, tool_id: str) -> str:
        spec = self._state.load_tool(workspace, ctx.wt_uid, tool_id)
        if spec is None:
            return f"- mdio_error: [@BASH.tools] tool '{tool_id}' not found."
        lines = [f"# BASH_TOOL {tool_id}"]
        for key in ("provider", "ops", "argv", "args", "help"):
            if key in spec:
                lines.append(f"- {key}: {spec[key]}")
        return "\n".join(lines)

    async def run_tool(
        self, workspace, ctx: RuntimeContext, tool_id: str,
        kwargs: dict, stdin_text: str | None = None,
    ):
        spec = self._state.load_tool(workspace, ctx.wt_uid, tool_id)
        if spec is None:
            return f"- mdio_error: [@BASH.tools] tool '{tool_id}' not found."
        op = _unquote(kwargs.get("-op") or "run")
        provider = str(spec.get("provider", "cli")).strip().lower()
        raw_args = _unquote(kwargs.get("-args") or "")
        try:
            args = json.loads(raw_args) if raw_args else {}
        except Exception as e:
            return f"- mdio_error: [@BASH.tools] invalid -args JSON: {e}"
        if not isinstance(args, dict):
            return "- mdio_error: [@BASH.tools] -args must be a JSON object."
        if str(spec.get("stdin", "true")).lower() in ("false", "0", "no") and stdin_text:
            return f"- mdio_error: [@BASH.tools] tool '{tool_id}' does not accept pipeline stdin."
        if provider == "python":
            return await self._run_python_tool(
                workspace, ctx, tool_id, op, spec, args, kwargs, stdin_text,
            )

        try:
            argv_spec = json.loads(spec.get("argv", "{}"))
        except Exception as e:
            return f"- mdio_error: [@BASH.tools] invalid argv for '{tool_id}': {e}"
        if isinstance(argv_spec, dict):
            if op not in argv_spec:
                return f"- mdio_error: [@BASH.tools] tool '{tool_id}' has no op '{op}'."
            template = argv_spec[op]
        else:
            template = argv_spec
        if not isinstance(template, list) or not template:
            return f"- mdio_error: [@BASH.tools] tool '{tool_id}' argv must be a non-empty list."
        argv: list[str] = []
        for item in template:
            text = str(item)
            for key, value in args.items():
                text = text.replace("{" + str(key) + "}", str(value))
            argv.append(text)
        allow_raw = spec.get("allow")
        if allow_raw:
            try:
                allow = json.loads(allow_raw)
            except Exception:
                allow = [part.strip() for part in str(allow_raw).split(",") if part.strip()]
            if isinstance(allow, str):
                allow = [allow]
            if allow and os.path.basename(argv[0]) not in allow and argv[0] not in allow:
                return f"- mdio_error: [@BASH.tools] tool '{tool_id}' command is not allowed: {argv[0]}"
        command = shlex.join(argv)
        sess_raw = kwargs.get("-sess")
        clean = {k: v for k, v in kwargs.items() if k not in ("-op", "-args", "-sess")}
        if spec.get("timeout") and "-timeout" not in clean:
            clean["-timeout"] = spec["timeout"]
        if sess_raw not in (None, ""):
            name = _unquote(sess_raw)
            return self._service_context_tool(
                workspace, ctx, name, command, clean,
                meta={"tool": tool_id, "op": op},
            )
        return self.stream_oneshot(workspace, ctx, command, clean, stdin_text=stdin_text, meta={"tool": tool_id, "op": op})

    def _service_context_tool(self, workspace, ctx, name, command, kwargs, meta=None):
        return self.context_stream(workspace, ctx, name, command, kwargs, meta=meta)

    async def _tool_cwd(
        self, workspace, ctx: RuntimeContext, kwargs: dict,
    ) -> str:
        sess_raw = kwargs.get("-sess")
        if sess_raw not in (None, ""):
            name = _unquote(sess_raw)
            handle = await self._backend.get_context(ctx.wt_uid, name)
            if handle is not None:
                return handle.cwd
            config = await self._state.load_effective_context_config(workspace, ctx, name)
            cwd = config.cwd
        else:
            config = await self._build_execution_spec(
                workspace, ctx, None, kwargs, allow_runtime_session_updates=True,
            )
            cwd = config.cwd
        if not cwd:
            from mdio_py.runtime.worktree_paths import project_root_for_wt
            cwd = project_root_for_wt(workspace.store, ctx.wt_uid)
        if not cwd:
            raise ValueError(f"cannot resolve host cwd for wt '{ctx.wt_uid}'")
        cwd = os.path.realpath(os.path.abspath(os.path.expanduser(cwd)))
        if not os.path.isdir(cwd):
            raise ValueError(f"host cwd does not exist: {cwd}")
        return cwd

    async def _record_native_tool(
        self, workspace, ctx: RuntimeContext, tool_id: str, op: str,
        cwd: str, session: str, text: str, state: str, exit_code: str,
        duration_ms: int,
    ) -> None:
        call_id = uuid.uuid4().hex[:8]
        now = time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime())
        fields: dict[str, Any] = {
            "state": state, "id": call_id, "run_id": call_id, "kind": "tool",
            "mode": "command", "cmd": f"{tool_id}.{op}", "tool": tool_id, "op": op,
            "session": session, "cwd": cwd, "shell": "", "started": now,
            "finished": now, "duration_ms": duration_ms, "exit": exit_code,
            "bytes": len(text.encode("utf-8")), "truncated": "false",
            "staging": "memory",
            "ctx": f"wt={ctx.wt_uid}, vfs={ctx.vfs_path}, caller={ctx.caller_pid or 'root'}",
        }
        stdout = text if state == "DONE" else ""
        stderr = "" if state == "DONE" else text
        try:
            if session:
                await self._state.append_context_cmd_record(
                    workspace, ctx.wt_uid, session, call_id, fields,
                    stdout=stdout, stderr=stderr,
                )
            else:
                await self._state.append_exec_record(
                    workspace, ctx.wt_uid, call_id, fields,
                    stdout=stdout, stderr=stderr,
                )
        except Exception:
            pass

    async def _run_python_tool(
        self, workspace, ctx: RuntimeContext, tool_id: str, op: str,
        spec: dict[str, Any], args: dict[str, Any], kwargs: dict,
        stdin_text: str | None,
    ) -> str:
        module_name = str(spec.get("module", "") or "").strip()
        entry = str(spec.get("entry", "run") or "run").strip()
        if not module_name:
            return f"- mdio_error: [@BASH.tools.{tool_id}] python provider requires module."
        ops_raw = str(spec.get("ops", "") or "")
        ops = [part.strip() for part in ops_raw.replace(",", " ").split() if part.strip()]
        if ops and op not in ops:
            return f"- mdio_error: [@BASH.tools.{tool_id}] no op '{op}'."
        try:
            module = importlib.import_module(module_name)
            fn = getattr(module, entry)
        except Exception as e:
            return f"- mdio_error: [@BASH.tools.{tool_id}] cannot load {module_name}.{entry}: {e}"
        session = _unquote(kwargs.get("-sess"))
        try:
            cwd = await self._tool_cwd(workspace, ctx, kwargs)
        except Exception as e:
            return f"- mdio_error: [@BASH.tools.{tool_id}] {e}"
        started = time.time()
        try:
            result = fn(
                op=op, args=args, cwd=cwd, wt_uid=ctx.wt_uid,
                session=session, stdin_text=stdin_text,
            )
            if inspect.isawaitable(result):
                result = await result
            text = str(result)
            await self._record_native_tool(
                workspace, ctx, tool_id, op, cwd, session, text, "DONE", "0",
                int((time.time() - started) * 1000),
            )
            return text
        except Exception as e:
            code = getattr(e, "code", "TOOL_ERROR")
            text = f"- mdio_error: [@BASH.tools.{tool_id}] {code}: {e}"
            await self._record_native_tool(
                workspace, ctx, tool_id, op, cwd, session, text, "FAILED", "1",
                int((time.time() - started) * 1000),
            )
            return text

    async def last_run_id(self, workspace, ctx: RuntimeContext) -> str:
        live = [(key[1], value) for key, value in self._live.items() if key[0] == ctx.wt_uid]
        if live:
            return max(live, key=lambda item: float(item[1].get("started") or 0))[0]
        return self._state.last_exec_id(workspace, ctx.wt_uid) or self._state.last_context_handle(workspace, ctx.wt_uid)

    def _normalize_handle_view(self, fields: dict[str, Any], handle_id: str) -> dict[str, Any]:
        out = dict(fields)
        out.setdefault("id", handle_id)
        out.setdefault("run_id", handle_id)
        out.setdefault("kind", "run")
        out.setdefault("mode", "command")
        return out

    async def inspect_handle(self, workspace, ctx: RuntimeContext, run_id: str) -> str:
        live = self._live.get((ctx.wt_uid, run_id))
        if live is not None:
            proc = self._state.read_proc_fields(workspace, ctx, run_id)
            identity = None
            try:
                if proc.get("os_pid"):
                    if proc.get("create_time"):
                        identity = ProcessIdentity(
                            pid=int(proc["os_pid"]),
                            create_time=float(proc["create_time"]),
                            pgid=int(proc["pgid"]) if proc.get("pgid") else None,
                            sid=int(proc["sid"]) if proc.get("sid") else None,
                        )
                    else:
                        identity = self._probe.capture(proc.get("os_pid"))
            except (ValueError, TypeError):
                identity = None
            fact = self._probe.probe(identity)
            if fact.liveness != "unknown" and proc:
                await self._state.update_run_proc(
                    workspace, ctx, run_id,
                    {"liveness": fact.liveness, "last_seen": repr(time.time())},
                    suppress_hooks=True,
                )
            started = float(live.get("started") or time.time())
            fields = {
                "state": live.get("state", "RUNNING"), "id": run_id,
                "kind": live.get("kind", "run"), "mode": "command",
                "cmd": live.get("command", ""), "session": live.get("session", ""),
                "run_id": run_id, "started": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(started)),
                "cwd": live.get("cwd"), "shell": live.get("shell"),
                "timeout": live.get("timeout", 0),
                "os_pid": proc.get("os_pid") or live.get("os_pid", ""),
                "create_time": proc.get("create_time") or live.get("create_time", ""),
                "pgid": proc.get("pgid") or live.get("pgid", ""),
                "sid": proc.get("sid") or live.get("sid", ""),
                "liveness": fact.liveness,
                "staging": "memory",
                "ctx": f"wt={ctx.wt_uid}, vfs={ctx.vfs_path}, caller={ctx.caller_pid or 'root'}",
            }
            return self._doc(self._normalize_handle_view(fields, run_id), str(live.get("stdout") or ""), "")
        parts = self._state.read_exec_parts(workspace, ctx, run_id)
        if parts is None:
            parts = self._state.find_context_cmd(workspace, ctx, run_id)
        if parts is None:
            return f"- mdio_error: [@BASH] id '{run_id}' not found."
        fields, stdout, stderr = parts
        return self._doc(self._normalize_handle_view(fields, run_id), stdout, stderr)

    async def kill_run(self, workspace, ctx: RuntimeContext, run_id: str) -> str:
        live = self._live.get((ctx.wt_uid, run_id))
        if live is not None and live.get("target") == "session":
            name = live.get("session", "")
            if not name:
                return f"- mdio_error: [@BASH] id '{run_id}' has no session target."
            killed = await self._backend.kill_context(ctx.wt_uid, name)
            if not killed:
                return f"- mdio_error: [@BASH] id '{run_id}' is not running."
            return f"- mdio_info: [@BASH] kill sent to '{run_id}'."
        killed = await self._backend.kill_run(ctx.wt_uid, run_id)
        if not killed:
            return f"- mdio_error: [@BASH] id '{run_id}' is not running."
        return f"- mdio_info: [@BASH] kill sent to '{run_id}'."

    async def _start_context_command(
        self, workspace, ctx: RuntimeContext, name: str, command: str, kwargs: dict,
        handle_id: str | None = None,
    ) -> tuple[BashConfig, str]:
        config = await self._build_execution_spec(
            workspace, ctx, name, kwargs, allow_runtime_session_updates=False,
        )
        await self._backend.open_context(ctx.wt_uid, name, self._spec_from_config(config, ""))
        await self._ensure_context_proc(workspace, ctx, name)
        cmd_id = handle_id or f"cmd_{uuid.uuid4().hex[:8]}"
        await self._state.update_context_proc(
            workspace, ctx, name,
            {"status": "RUNNING", "kind": "context", "current_cmd": cmd_id, "cmd": command},
        )
        await self._state.write_context_root(
            workspace, ctx.wt_uid, name,
            {
                "shell": config.shell, "cwd": config.cwd, "timeout": config.timeout,
                "engine": config.session_engine, "status": "RUNNING",
            },
        )
        return config, cmd_id

    async def _finish_context_command(
        self, workspace, ctx: RuntimeContext, name: str, cmd_id: str, command: str,
        config: BashConfig, result: BashResult,
        *, run_id: str | None = None, meta: dict[str, Any] | None = None,
        terminal_status: str | None = None,
    ) -> str:
        state = self._normalized_state(result)
        fields: dict[str, Any] = {
            "state": state, "mode": "command", "cmd": command,
            "exit": result.exit_code, "cwd": result.cwd or config.cwd,
            "shell": result.shell or config.shell,
            "session": name,
            "bytes": len((result.stdout or "").encode("utf-8")),
            "truncated": "true" if "...[truncated]..." in (result.stdout or "") else "false",
        }
        if meta:
            fields["tool"] = meta.get("tool")
            fields["op"] = meta.get("op")
        if run_id:
            fields["run_id"] = run_id
        await self._state.append_context_cmd_record(
            workspace, ctx.wt_uid, name, cmd_id, fields,
            stdout=result.stdout, stderr=result.stderr,
        )
        if result.status != "DONE":
            await self._backend.close_context(ctx.wt_uid, name)
            await self._plugin._unregister_proc(workspace, ctx, name)
            await self._state.write_context_root(
                workspace, ctx.wt_uid, name,
                {
                    "shell": result.shell or config.shell, "cwd": result.cwd or config.cwd,
                    "timeout": result.timeout or config.timeout,
                    "engine": config.session_engine,
                    "status": terminal_status or "CLOSED",
                },
            )
            return state
        await self._state.write_context_root(
            workspace, ctx.wt_uid, name,
            {
                "shell": result.shell or config.shell, "cwd": result.cwd or config.cwd,
                "timeout": result.timeout or config.timeout,
                "engine": config.session_engine, "status": state,
            },
        )
        handle = await self._backend.get_context(ctx.wt_uid, name)
        proc_payload: dict[str, Any] = {"status": "IDLE", "kind": "context", "current_cmd": ""}
        if handle is not None:
            proc_payload.update({
                "os_pid": handle.os_pid, "pgid": handle.pgid,
                "create_time": handle.create_time, "sid": handle.sid,
                "liveness": "alive",
            })
        await self._state.update_context_proc(workspace, ctx, name, proc_payload)
        return state

    async def context_run(
        self, workspace, ctx: RuntimeContext, name: str, command: str, kwargs: dict,
        link_run_id: str | None = None,
    ) -> str:
        if not command:
            return f"- mdio_error: [@BASH.{name}] run requires command input."
        if any(key in kwargs for key in ("-shell", "-cwd", "-env")):
            return f"- mdio_error: [@BASH.{name}] runtime shell/cwd/env overrides are not allowed. Use .init or .restart."
        if any(key in kwargs for key in ("-cmd", "-stdin")):
            return f'- mdio_error: [@BASH.{name}] -cmd/-stdin are currently supported only on @BASH.run and @BASH.stream.'
        if guard := self._check_sh_guard(command, enabled=self._flag_enabled(kwargs, "-nah")):
            return guard

        handle = await self._start_handle(
            workspace, ctx, target="session", kind="run", command=command,
            kwargs=kwargs, session=name,
        )
        result = await self._backend.context_run(
            ctx.wt_uid, name, handle.id, command, timeout=handle.timeout,
        )
        await self._finish_handle(workspace, ctx, handle, result)
        if result.status == "DONE":
            return result.stdout
        return self._render_error(name, result)

    def context_stream(
        self, workspace, ctx: RuntimeContext, name: str, command: str, kwargs: dict,
        meta: dict[str, Any] | None = None, link_run_id: str | None = None,
    ):
        async def _err(msg: str):
            yield msg

        if not command:
            return _err(f"- mdio_error: [@BASH.{name}] stream requires command input.")
        if any(key in kwargs for key in ("-shell", "-cwd", "-env")):
            return _err(f"- mdio_error: [@BASH.{name}] runtime shell/cwd/env overrides are not allowed. Use .init or .restart.")
        if any(key in kwargs for key in ("-cmd", "-stdin")):
            return _err(f'- mdio_error: [@BASH.{name}] -cmd/-stdin are currently supported only on @BASH.run and @BASH.stream.')
        if guard := self._check_sh_guard(command, enabled=self._flag_enabled(kwargs, "-nah")):
            return _err(guard)

        async def _runner():
            handle = await self._start_handle(
                workspace, ctx, target="session", kind="run", command=command,
                kwargs=kwargs, session=name,
            )
            fallback = BashResult(
                status="CANCELLED", exit_code="KILLED", shell=handle.shell,
                cwd=handle.cwd, timeout=handle.timeout, liveness="gone",
            )
            try:
                stream = self._backend.context_stream(
                    ctx.wt_uid, name, handle.id, command, timeout=handle.timeout,
                )
                async for chunk in self._drive_stream(handle, stream, fallback):
                    yield chunk
            finally:
                result = handle.get("result") or fallback
                await self._finish_handle(workspace, ctx, handle, result, meta=meta)
            if result.status != "DONE":
                yield "\n" + self._render_error(name, result)

        return _runner()

    async def inspect_session(self, workspace, ctx: RuntimeContext, name: str) -> str:
        proc_block = self._state.read_proc_block(workspace, ctx, name)
        var_block = self._state.compact_display_block(await self._state.read_context_var_block(workspace, ctx, name))
        config = await self._state.load_context_config(workspace, ctx, name)
        handle = await self._backend.get_context(ctx.wt_uid, name)
        if not proc_block and not var_block and config is None and handle is None:
            return f"- mdio_error: [@BASH] context '{name}' not found."
        proc = self._state.read_proc_fields(workspace, ctx, name)
        root_status = self._state.extract_strip_value(var_block, "status", workspace) if var_block else ""
        state = (
            handle.status if handle is not None
            else root_status or proc.get("status") or ("CONFIGURED" if config is not None else "UNKNOWN")
        )
        active = proc.get("current_cmd") or (handle.current_cmd if handle is not None else "")
        lines = [
            "# BASH_CONTEXT_INSPECT", f"- name: {name}", f"- state: {state}",
        ]
        if active:
            lines.append(f"- active_handle: {active}")
        if config is not None:
            lines.append("## CONFIG")
            lines.extend(self._state.config_lines(config))
        if proc_block:
            lines.append("## PROC")
            lines.append(proc_block)
        if handle is not None and not proc_block:
            lines.append("## PROC")
            lines.append(f"- status: {handle.status}")
        if var_block:
            lines.append("## VAR")
            lines.append(var_block)
        return "\n".join(lines)

    async def restart_context(self, workspace, ctx: RuntimeContext, name: str, kwargs: dict) -> str:
        config = await self._build_execution_spec(workspace, ctx, name, kwargs, allow_runtime_session_updates=False)
        handle = await self._backend.restart_context(ctx.wt_uid, name, self._spec_from_config(config, ""))
        await self._ensure_context_proc(workspace, ctx, name)
        await self._state.update_context_proc(
            workspace,
            ctx,
            name,
            {
                "status": handle.status,
                "kind": "context",
                "os_pid": handle.os_pid,
                "pgid": handle.pgid,
                "create_time": handle.create_time,
                "sid": handle.sid,
                "liveness": "alive",
            },
        )
        await self._state.write_context_root(
            workspace,
            ctx.wt_uid,
            name,
            {
                "shell": handle.shell,
                "cwd": handle.cwd,
                "timeout": handle.timeout,
                "engine": handle.engine,
                "status": handle.status,
            },
        )
        await self._state.append_context_event(workspace, ctx.wt_uid, name, "event_restart", {"reason": "manual restart"})
        return f"- mdio_info: [@BASH] context '{name}' restarted."

    async def kill_context(self, workspace, ctx: RuntimeContext, name: str) -> str:
        killed = await self._backend.kill_context(ctx.wt_uid, name)
        if not killed:
            return f"- mdio_error: [@BASH] context '{name}' is not running."
        await self._plugin._unregister_proc(workspace, ctx, name)
        await self._state.write_context_root(workspace, ctx.wt_uid, name, {"status": "KILLED"})
        await self._state.append_context_event(workspace, ctx.wt_uid, name, "event_kill", {"reason": "manual kill"})
        return f"- mdio_info: [@BASH] context '{name}' killed."

    async def close_context(self, workspace, ctx: RuntimeContext, name: str) -> str:
        closed = await self._backend.close_context(ctx.wt_uid, name)
        if not closed:
            return f"- mdio_info: [@BASH] context '{name}' already closed."
        await self._plugin._unregister_proc(workspace, ctx, name)
        await self._state.write_context_root(workspace, ctx.wt_uid, name, {"status": "CLOSED"})
        await self._state.append_context_event(workspace, ctx.wt_uid, name, "event_close", {"reason": "manual close"})
        return f"- mdio_info: [@BASH] context '{name}' closed."

    async def _build_execution_spec(
        self,
        workspace,
        ctx: RuntimeContext,
        name: str | None,
        kwargs: dict,
        allow_runtime_session_updates: bool,
    ) -> BashConfig:
        base = await self._state.load_effective_context_config(workspace, ctx, name) if name else await self._state.load_global_config(workspace, ctx)
        shell_raw = _unquote(kwargs.get("-shell"))
        cwd_raw = _unquote(kwargs.get("-cwd"))
        timeout_raw = _unquote(kwargs.get("-timeout"))
        env_raw = kwargs.get("-env")

        if shell_raw:
            base.shell = shell_raw
        if timeout_raw:
            base.timeout = self._state.parse_int(timeout_raw, base.timeout, label="timeout")
        if cwd_raw:
            base.cwd = cwd_raw
        if env_raw not in (None, ""):
            extra = self._parse_runtime_env(env_raw)
            if allow_runtime_session_updates or name is None:
                merged = dict(base.env)
                merged.update(extra)
                base.env = merged

        cwd_value = base.cwd
        if not cwd_value:
            from mdio_py.runtime.worktree_paths import project_root_for_wt
            cwd_value = project_root_for_wt(workspace.store, ctx.wt_uid)
        if not cwd_value:
            raise ValueError(f"cannot resolve project root for wt '{ctx.wt_uid}'")
        cwd_host = os.path.realpath(os.path.abspath(os.path.expanduser(cwd_value)))
        if not os.path.isdir(cwd_host):
            raise ValueError(f"host cwd does not exist: {cwd_host}")
        base.cwd = cwd_host
        return base

    def _spec_from_config(self, config: BashConfig, command: str, stdin_text: str | None = None) -> BashExecutionSpec:
        return BashExecutionSpec(
            command=command,
            shell=config.shell,
            cwd=config.cwd or os.getcwd(),
            timeout=config.timeout,
            stdin_text=stdin_text,
            env=dict(config.env),
            stream_chunk=config.stream_chunk,
            output_limit=config.output_limit,
        )

    async def _extract_context_updates(self, kwargs: dict) -> dict[str, str]:
        updates: dict[str, str] = {}
        if "-shell" in kwargs:
            updates["shell"] = _unquote(kwargs.get("-shell"))
        if "-cwd" in kwargs:
            cwd_host = _host_cwd(kwargs.get("-cwd"))
            if not os.path.isdir(cwd_host):
                raise ValueError(f"host cwd does not exist: {cwd_host}")
            updates["cwd"] = cwd_host
        if "-timeout" in kwargs:
            updates["timeout"] = str(self._state.parse_int(_unquote(kwargs.get("-timeout")), 30, label="timeout"))
        if "-env" in kwargs:
            updates["env"] = json.dumps(self._parse_runtime_env(kwargs.get("-env")), ensure_ascii=False)
        return updates

    def _parse_runtime_env(self, raw_env: Any) -> dict[str, str]:
        raw = _unquote(raw_env)
        if not raw:
            return {}
        try:
            parsed = json.loads(raw)
            if isinstance(parsed, dict):
                return {str(k): str(v) for k, v in parsed.items()}
        except Exception:
            pass
        if "=" not in raw:
            raise ValueError("invalid -env: expected JSON object or KEY=VAL")
        key, value = raw.split("=", 1)
        return {key.strip(): value}

    async def _ensure_context_proc(self, workspace, ctx: RuntimeContext, name: str) -> None:
        if self._state.read_proc_block(workspace, ctx, name):
            return
        handle = await self._backend.get_context(ctx.wt_uid, name)
        await self._plugin._register_proc(
            workspace,
            ctx,
            identifier=name,
            ppid=ctx.caller_pid or "root",
            payload={
                "status": handle.status if handle is not None else "IDLE",
                "kind": "context",
                "current_cmd": handle.current_cmd if handle is not None else "",
                "os_pid": handle.os_pid if handle is not None else "",
                "pgid": handle.pgid if handle is not None else "",
                "create_time": handle.create_time if handle is not None else "",
                "sid": handle.sid if handle is not None else "",
                "liveness": "alive" if handle is not None else "unknown",
            },
            exact_pid=name,
            register_task=False,
        )

    async def _finalize_run(
        self,
        workspace,
        ctx: RuntimeContext,
        run_id: str,
        command: str,
        result: BashResult,
        stdin_text: str | None = None,
        cb_code: str = "",
        kind: str = "run",
        unregister: bool = True,
        meta: dict[str, Any] | None = None,
        force: bool = False,
        live_override: dict[str, Any] | None = None,
    ) -> None:
        live = live_override if live_override is not None else self._live.pop((ctx.wt_uid, run_id), {})
        live = live or {}
        if not force and live.get("gen") is not None and live.get("gen") != self._generations.get(ctx.wt_uid, 0):
            return
        state = self._normalized_state(result)
        now = time.time()
        started = float(live.get("started") or now)
        fields: dict[str, Any] = {
            "state": state,
            "id": run_id,
            "kind": kind,
            "mode": "stdin" if stdin_text is not None else "command",
            "cmd": command,
            "cwd": result.cwd,
            "shell": result.shell,
            "started": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(started)),
            "finished": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(now)),
            "duration_ms": int((now - started) * 1000),
            "exit": result.exit_code,
            "signal": result.signal,
            "timeout": result.timeout,
            "os_pid": result.os_pid or live.get("os_pid", ""),
            "create_time": result.create_time or live.get("create_time", ""),
            "pgid": result.pgid or live.get("pgid", ""),
            "sid": result.sid or live.get("sid", ""),
            "liveness": "gone",
            "confirmed_at": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(now)),
            "bytes": len((result.stdout or "").encode("utf-8")),
            "truncated": "true" if "...[truncated]..." in (result.stdout or "") else "false",
            "staging": "memory",
            "ctx": f"wt={ctx.wt_uid}, vfs={ctx.vfs_path}, caller={ctx.caller_pid or 'root'}",
        }
        if meta:
            fields.update(meta)
        if stdin_text is not None:
            fields["stdin"] = stdin_text
        if cb_code:
            fields["cb"] = cb_code
            fields["cb_status"] = "PENDING"
        await self._state.append_exec_record(
            workspace, ctx.wt_uid, run_id, fields,
            stdout=result.stdout, stderr=result.stderr,
        )
        await self._state.update_run_proc(
            workspace, ctx, run_id,
            {
                "status": state, "kind": kind, "os_pid": result.os_pid or live.get("os_pid", ""),
                "create_time": result.create_time or live.get("create_time", ""),
                "pgid": result.pgid or live.get("pgid", ""),
                "sid": result.sid or live.get("sid", ""),
                "liveness": "gone",
                "confirmed_at": time.strftime("%Y-%m-%dT%H:%M:%S%z", time.localtime(now)),
            },
        )
        if unregister:
            await self._plugin._unregister_proc(workspace, ctx, run_id)

    def _render_error(self, ident: str, result: BashResult) -> str:
        detail = result.stderr.strip() or result.stdout.strip() or "command failed"
        return f"- mdio_error: [@BASH {ident}] {result.status.lower()} — {detail}"
