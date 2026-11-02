from __future__ import annotations

import asyncio
import os
import queue
import re
import signal
import threading
import time
import uuid
from dataclasses import dataclass, field
from typing import Any

from ..models import (
    BashContextHandle,
    BashDisposeOutcome,
    BashDisposeReport,
    BashExecutionSpec,
    BashResult,
    BashStarted,
    BashStreamHandle,
)
from ..process_probe import BashProcessProbe, ProcessIdentity
from .base import BashBackend


@dataclass
class _LocalContext:
    handle: BashContextHandle
    child: Any
    lock: asyncio.Lock = field(default_factory=asyncio.Lock)
    identity: ProcessIdentity | None = None


class ProcessOwner:
    """Owns process/session state; the backend only orchestrates execution."""

    def __init__(self):
        self.processes: dict[str, dict[str, asyncio.subprocess.Process]] = {}
        self.contexts: dict[str, dict[str, _LocalContext]] = {}
        self.killed_runs: dict[str, set[str]] = {}
        self.run_identities: dict[tuple[str, str], ProcessIdentity] = {}
        self.probe = BashProcessProbe()

    def remember_process(self, wt_uid: str, run_id: str, process: asyncio.subprocess.Process) -> None:
        self.processes.setdefault(wt_uid, {})[run_id] = process

    def forget_process(self, wt_uid: str, run_id: str) -> None:
        self.processes.get(wt_uid, {}).pop(run_id, None)
        killed = self.killed_runs.get(wt_uid)
        if killed:
            killed.discard(run_id)

    def mark_run_killed(self, wt_uid: str, run_id: str) -> None:
        self.killed_runs.setdefault(wt_uid, set()).add(run_id)

    def was_run_killed(self, wt_uid: str, run_id: str) -> bool:
        return run_id in self.killed_runs.get(wt_uid, set())

    def started_for(self, pid: int) -> tuple[BashStarted, ProcessIdentity | None]:
        identity = self.probe.capture(pid)
        if identity is None:
            return BashStarted(os_pid=str(pid), liveness="unknown"), None
        return (
            BashStarted(
                os_pid=str(pid),
                create_time=repr(identity.create_time),
                pgid=str(identity.pgid or ""),
                sid=str(identity.sid or ""),
                liveness="alive",
            ),
            identity,
        )


class LocalBashBackend(BashBackend):
    _PROMPT = "__MDIO_PROMPT__> "
    _PROMPT_CONT = "__MDIO_PROMPT_CONT__> "

    def __init__(self):
        self._owner = ProcessOwner()

    @property
    def _processes(self):
        return self._owner.processes

    @property
    def _contexts(self):
        return self._owner.contexts

    @property
    def _killed_runs(self):
        return self._owner.killed_runs

    @property
    def _run_identities(self):
        return self._owner.run_identities

    @property
    def _probe(self):
        return self._owner.probe

    def _remember_process(self, wt_uid: str, run_id: str, process: asyncio.subprocess.Process) -> None:
        self._owner.remember_process(wt_uid, run_id, process)

    def _forget_process(self, wt_uid: str, run_id: str) -> None:
        self._owner.forget_process(wt_uid, run_id)

    def _mark_run_killed(self, wt_uid: str, run_id: str) -> None:
        self._owner.mark_run_killed(wt_uid, run_id)

    def _was_run_killed(self, wt_uid: str, run_id: str) -> bool:
        return self._owner.was_run_killed(wt_uid, run_id)

    def _started_for(self, pid: int) -> tuple[BashStarted, ProcessIdentity | None]:
        return self._owner.started_for(pid)

    async def _terminate_run(
        self,
        wt_uid: str,
        run_id: str,
        identity: ProcessIdentity | None,
        *,
        grace_ms: int = 1000,
    ) -> BashDisposeOutcome:
        return await asyncio.to_thread(
            self._probe.terminate_tree,
            run_id,
            identity,
            grace_ms,
        )

    async def _cleanup_stream_process(
        self,
        wt_uid: str,
        run_id: str,
        process: asyncio.subprocess.Process | None,
        identity: ProcessIdentity | None,
    ) -> None:
        if process is not None and process.returncode is None:
            await self._terminate_run(wt_uid, run_id, identity, grace_ms=1000)
            try:
                await asyncio.wait_for(asyncio.shield(process.wait()), timeout=5)
            except Exception:
                pass
        if process is not None and process.returncode is None:
            self._kill_process(process)

    def _clip(self, text: str, limit: int) -> str:
        if limit <= 0 or len(text) <= limit:
            return text
        return text[:limit] + "\n...[truncated]..."

    def _pgid_for_pid(self, pid: int | None) -> str:
        if not pid:
            return ""
        try:
            return str(os.getpgid(pid))
        except Exception:
            return ""

    @staticmethod
    def _signal_name(returncode: int | None) -> str:
        if returncode is not None and returncode < 0:
            try:
                return signal.Signals(-returncode).name
            except ValueError:
                return f"SIG{-returncode}"
        return ""

    def _kill_process(self, process: asyncio.subprocess.Process | None) -> None:
        if process is None or process.returncode is not None:
            return
        try:
            os.killpg(os.getpgid(process.pid), signal.SIGKILL)
            return
        except Exception:
            pass
        try:
            process.kill()
        except ProcessLookupError:
            pass

    async def run(
        self,
        wt_uid: str,
        run_id: str,
        spec: BashExecutionSpec,
        on_start=None,
    ) -> BashResult:
        process = None
        stdout_text = ""
        stderr_text = ""
        exit_code = ""
        status = "DONE"
        os_pid = ""
        pgid = ""
        create_time = ""
        sid = ""
        killed = False

        try:
            proc_env = os.environ.copy()
            proc_env.update(spec.env)
            process = await asyncio.create_subprocess_exec(
                spec.shell,
                "-lc",
                spec.command,
                cwd=spec.cwd,
                env=proc_env,
                stdin=asyncio.subprocess.PIPE if spec.stdin_text is not None else asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                start_new_session=True,
            )
            self._remember_process(wt_uid, run_id, process)
            started, identity = self._started_for(process.pid)
            self._run_identities[(wt_uid, run_id)] = identity
            os_pid = started.os_pid
            pgid = started.pgid
            create_time = started.create_time
            sid = started.sid
            if on_start is not None:
                await on_start(started)

            try:
                input_bytes = None
                if spec.stdin_text is not None:
                    input_bytes = spec.stdin_text.encode("utf-8")
                stdout_b, stderr_b = await asyncio.wait_for(process.communicate(input=input_bytes), timeout=spec.timeout)
            except asyncio.TimeoutError:
                status = "CANCELLED"
                exit_code = "TIMEOUT"
                await self._terminate_run(wt_uid, run_id, identity)
                try:
                    stdout_b, stderr_b = await asyncio.wait_for(asyncio.shield(process.communicate()), timeout=5)
                except Exception:
                    stdout_b, stderr_b = b"", b""

            stdout_text = stdout_b.decode("utf-8", errors="replace")
            stderr_text = stderr_b.decode("utf-8", errors="replace")
            if not exit_code:
                exit_code = str(process.returncode)
            if status != "CANCELLED":
                status = "DONE" if process.returncode == 0 else "FAILED"
        except asyncio.CancelledError:
            status = "CANCELLED"
            identity = self._run_identities.get((wt_uid, run_id))
            await asyncio.shield(self._cleanup_stream_process(wt_uid, run_id, process, identity))
            raise
        except Exception as e:
            status = "FAILED"
            stderr_text = str(e)
            exit_code = "SPAWN_ERROR"
            identity = self._run_identities.get((wt_uid, run_id))
            await asyncio.shield(self._cleanup_stream_process(wt_uid, run_id, process, identity))
        finally:
            killed = self._was_run_killed(wt_uid, run_id)
            self._forget_process(wt_uid, run_id)
            self._run_identities.pop((wt_uid, run_id), None)
        if killed:
            status = "CANCELLED"
            exit_code = "KILLED"

        return BashResult(
            stdout=self._clip(stdout_text, spec.output_limit),
            stderr=self._clip(stderr_text, spec.output_limit),
            exit_code=exit_code,
            status=status,
            shell=spec.shell,
            cwd=spec.cwd,
            timeout=spec.timeout,
            os_pid=os_pid,
            pgid=pgid,
            create_time=create_time,
            sid=sid,
            liveness="gone",
            signal=self._signal_name(process.returncode if process is not None else None),
        )

    def stream(self, wt_uid: str, run_id: str, spec: BashExecutionSpec, on_start=None) -> BashStreamHandle:
        loop = asyncio.get_running_loop()
        done: asyncio.Future[BashResult] = loop.create_future()

        async def _runner():
            process = None
            stdout_parts: list[str] = []
            stderr_text = ""
            exit_code = ""
            status = "DONE"
            os_pid = ""
            pgid = ""
            create_time = ""
            sid = ""
            identity: ProcessIdentity | None = None
            completed = False

            try:
                proc_env = os.environ.copy()
                proc_env.update(spec.env)
                process = await asyncio.create_subprocess_exec(
                    spec.shell,
                    "-lc",
                    spec.command,
                    cwd=spec.cwd,
                    env=proc_env,
                    stdin=asyncio.subprocess.PIPE if spec.stdin_text is not None else asyncio.subprocess.DEVNULL,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.PIPE,
                    start_new_session=True,
                )
                self._remember_process(wt_uid, run_id, process)
                started, identity = self._started_for(process.pid)
                self._run_identities[(wt_uid, run_id)] = identity
                os_pid = started.os_pid
                pgid = started.pgid
                create_time = started.create_time
                sid = started.sid
                if on_start is not None:
                    await on_start(started)
                if spec.stdin_text is not None and process.stdin is not None:
                    try:
                        process.stdin.write(spec.stdin_text.encode("utf-8"))
                        await process.stdin.drain()
                    except (BrokenPipeError, ConnectionResetError):
                        pass
                    finally:
                        try:
                            process.stdin.close()
                        except Exception:
                            pass
                stderr_task = asyncio.create_task(process.stderr.read())
                deadline = loop.time() + spec.timeout

                while True:
                    remain = deadline - loop.time()
                    if remain <= 0:
                        status = "CANCELLED"
                        exit_code = "TIMEOUT"
                        await self._terminate_run(wt_uid, run_id, identity)
                        break
                    try:
                        chunk = await asyncio.wait_for(process.stdout.read(spec.stream_chunk), timeout=remain)
                    except asyncio.TimeoutError:
                        status = "CANCELLED"
                        exit_code = "TIMEOUT"
                        await self._terminate_run(wt_uid, run_id, identity)
                        break
                    if not chunk:
                        break
                    text = chunk.decode("utf-8", errors="replace")
                    stdout_parts.append(text)
                    yield text

                await process.wait()
                completed = True
                tail = await process.stdout.read()
                if tail:
                    tail_text = tail.decode("utf-8", errors="replace")
                    stdout_parts.append(tail_text)
                    yield tail_text
                stderr_text = (await stderr_task).decode("utf-8", errors="replace")
                if not exit_code:
                    exit_code = str(process.returncode)
                if self._was_run_killed(wt_uid, run_id):
                    status = "CANCELLED"
                elif status != "CANCELLED":
                    status = "DONE" if process.returncode == 0 else "FAILED"
            except Exception as e:
                status = "FAILED"
                stderr_text = str(e)
                exit_code = "SPAWN_ERROR"
            finally:
                if process is not None and process.returncode is None:
                    if not completed:
                        status = "CANCELLED"
                        exit_code = "KILLED"
                    try:
                        await asyncio.shield(self._cleanup_stream_process(wt_uid, run_id, process, identity))
                    except BaseException:
                        pass
                self._forget_process(wt_uid, run_id)
                self._run_identities.pop((wt_uid, run_id), None)
                if not done.done():
                    done.set_result(
                        BashResult(
                            stdout=self._clip("".join(stdout_parts), spec.output_limit),
                            stderr=self._clip(stderr_text, spec.output_limit),
                            exit_code=exit_code,
                            status=status,
                            shell=spec.shell,
                            cwd=spec.cwd,
                            timeout=spec.timeout,
                            os_pid=os_pid,
                            pgid=pgid,
                            create_time=create_time,
                            sid=sid,
                            liveness="gone",
                            signal=self._signal_name(process.returncode if process is not None else None),
                        )
                    )

        return BashStreamHandle(stream=_runner(), done=done)

    async def kill_run(self, wt_uid: str, run_id: str) -> bool:
        process = self._processes.get(wt_uid, {}).get(run_id)
        if process is None:
            return False
        self._mark_run_killed(wt_uid, run_id)
        identity = self._run_identities.get((wt_uid, run_id))
        if identity is None:
            identity = self._probe.capture(process.pid)
        await self._terminate_run(wt_uid, run_id, identity)
        return True

    async def open_context(self, wt_uid: str, name: str, spec: BashExecutionSpec) -> BashContextHandle:
        existing = self._contexts.get(wt_uid, {}).get(name)
        if existing is not None:
            return existing.handle

        local = await asyncio.to_thread(self._spawn_context_sync, name, spec)
        self._contexts.setdefault(wt_uid, {})[name] = local
        return local.handle

    async def get_context(self, wt_uid: str, name: str) -> BashContextHandle | None:
        local = self._contexts.get(wt_uid, {}).get(name)
        return local.handle if local is not None else None

    def list_contexts(self, wt_uid: str) -> list[BashContextHandle]:
        return [local.handle for local in self._contexts.get(wt_uid, {}).values()]

    async def context_run(
        self,
        wt_uid: str,
        name: str,
        command_id: str,
        command: str,
        timeout: int | None = None,
    ) -> BashResult:
        local = self._contexts.get(wt_uid, {}).get(name)
        if local is None:
            return BashResult(status="FAILED", stderr=f"context '{name}' is not active", exit_code="NO_CONTEXT")

        async with local.lock:
            local.handle.status = "RUNNING"
            local.handle.current_cmd = command_id
            try:
                result = await asyncio.to_thread(
                    self._run_context_sync,
                    local,
                    command,
                    timeout or local.handle.timeout,
                )
            finally:
                local.handle.current_cmd = ""
                local.handle.status = "IDLE"
            return result

    def context_stream(
        self,
        wt_uid: str,
        name: str,
        command_id: str,
        command: str,
        timeout: int | None = None,
    ) -> BashStreamHandle:
        loop = asyncio.get_running_loop()
        done: asyncio.Future[BashResult] = loop.create_future()

        async def _runner():
            local = self._contexts.get(wt_uid, {}).get(name)
            if local is None:
                result = BashResult(status="FAILED", stderr=f"context '{name}' is not active", exit_code="NO_CONTEXT")
                if not done.done():
                    done.set_result(result)
                yield f"- mdio_error: [@BASH] context '{name}' is not active"
                return
            q: queue.Queue = queue.Queue()
            stop_event = threading.Event()
            thread = threading.Thread(
                target=self._context_stream_worker,
                args=(local, command, timeout or local.handle.timeout, q, stop_event),
                daemon=True,
            )
            result = None
            async with local.lock:
                local.handle.status = "RUNNING"
                local.handle.current_cmd = command_id
                thread.start()
                try:
                    while True:
                        kind, payload = await asyncio.to_thread(q.get)
                        if kind == "chunk":
                            if payload:
                                yield payload
                            continue
                        result = payload
                        break
                finally:
                    stop_event.set()
                    local.handle.current_cmd = ""
                    local.handle.status = "IDLE"
                    try:
                        await asyncio.to_thread(thread.join, 5)
                    except Exception:
                        pass
                    if result is None:
                        result = BashResult(
                            stdout="", stderr="context stream cancelled", exit_code="KILLED",
                            status="CANCELLED", shell=local.handle.shell, cwd=local.handle.cwd,
                            timeout=timeout or local.handle.timeout,
                            os_pid=local.handle.os_pid, pgid=local.handle.pgid,
                            create_time=local.handle.create_time, sid=local.handle.sid,
                            liveness="gone",
                        )
                        try:
                            await asyncio.to_thread(
                                self._probe.terminate_tree, name, local.identity, 1000,
                            )
                        except Exception:
                            pass
                        try:
                            await asyncio.to_thread(self._close_child_sync, local.child)
                        except Exception:
                            pass
                        self._contexts.get(wt_uid, {}).pop(name, None)
                        local.handle.status = "CLOSED"
                        if not done.done():
                            done.set_result(result)
            if result is None:
                result = BashResult(
                    stdout="", stderr="context stream cancelled", exit_code="KILLED",
                    status="CANCELLED", shell=local.handle.shell, cwd=local.handle.cwd,
                    timeout=timeout or local.handle.timeout,
                    os_pid=local.handle.os_pid, pgid=local.handle.pgid,
                    create_time=local.handle.create_time, sid=local.handle.sid,
                    liveness="gone",
                )
            if result.status != "DONE":
                try:
                    await asyncio.to_thread(
                        self._probe.terminate_tree, name, local.identity, 1000,
                    )
                except Exception:
                    pass
                try:
                    await asyncio.to_thread(self._close_child_sync, local.child)
                except Exception:
                    pass
                self._contexts.get(wt_uid, {}).pop(name, None)
                local.handle.status = "CLOSED"
            if not done.done():
                done.set_result(result)

        return BashStreamHandle(stream=_runner(), done=done)

    def _context_stream_worker(
        self,
        local: _LocalContext,
        command: str,
        timeout: int,
        q: queue.Queue,
        stop_event: threading.Event,
    ) -> None:
        import pexpect
        child = local.child
        marker = f"__MDIO_DONE_{uuid.uuid4().hex}__"
        wrapped = (
            f"{command}\n"
            "__mdio_exit=$?\n"
            f"printf '\\n{marker}:%s\\n' \"$__mdio_exit\""
        )
        parts: list[str] = []
        deadline = time.monotonic() + max(1, int(timeout))
        try:
            child.sendline(wrapped)
            while not stop_event.is_set():
                remain = deadline - time.monotonic()
                if remain <= 0:
                    result = BashResult(
                        stdout="".join(parts).rstrip("\n"), stderr="command timed out",
                        exit_code="TIMEOUT", status="CANCELLED",
                        shell=local.handle.shell, cwd=local.handle.cwd,
                        timeout=timeout, os_pid=local.handle.os_pid, pgid=local.handle.pgid,
                        create_time=local.handle.create_time, sid=local.handle.sid,
                        liveness="gone",
                    )
                    q.put(("done", result))
                    return
                try:
                    child.expect(re.compile(rf"{re.escape(marker)}:(\d+)"), timeout=min(0.1, remain))
                    chunk = self._normalize_text(child.before)
                    if chunk:
                        parts.append(chunk)
                        q.put(("chunk", chunk))
                    exit_code = child.match.group(1)
                    child.expect_exact(self._PROMPT, timeout=min(5, remain))
                    try:
                        cwd, _ = self._exec_capture_sync(child, "pwd", min(timeout, 5))
                        local.handle.cwd = cwd.strip() or local.handle.cwd
                    except Exception:
                        pass
                    result = BashResult(
                        stdout="".join(parts).rstrip("\n"), stderr="",
                        exit_code=exit_code,
                        status="DONE" if exit_code == "0" else "FAILED",
                        shell=local.handle.shell, cwd=local.handle.cwd,
                        timeout=timeout, os_pid=local.handle.os_pid, pgid=local.handle.pgid,
                        create_time=local.handle.create_time, sid=local.handle.sid,
                        liveness="gone",
                    )
                    q.put(("done", result))
                    return
                except pexpect.TIMEOUT:
                    chunk = self._normalize_text(child.before)
                    if chunk:
                        parts.append(chunk)
                        q.put(("chunk", chunk))
        except Exception as e:
            result = BashResult(
                stdout="".join(parts).rstrip("\n"), stderr=str(e),
                exit_code="PEXPECT_ERROR", status="FAILED",
                shell=local.handle.shell, cwd=local.handle.cwd,
                timeout=timeout, os_pid=local.handle.os_pid, pgid=local.handle.pgid,
                create_time=local.handle.create_time, sid=local.handle.sid,
                liveness="gone",
            )
            q.put(("done", result))
        finally:
            if stop_event.is_set():
                try:
                    self._probe.terminate_tree(local.handle.name, local.identity, grace_ms=1000)
                except Exception:
                    pass
                try:
                    self._close_child_sync(local.child)
                except Exception:
                    pass

    async def restart_context(self, wt_uid: str, name: str, spec: BashExecutionSpec) -> BashContextHandle:
        await self.close_context(wt_uid, name)
        return await self.open_context(wt_uid, name, spec)

    async def kill_context(self, wt_uid: str, name: str) -> bool:
        return await self.close_context(wt_uid, name)

    async def dispose_wt(self, wt_uid: str) -> BashDisposeReport:
        report = BashDisposeReport()
        for run_id, process in list(self._processes.get(wt_uid, {}).items()):
            identity = self._run_identities.get((wt_uid, run_id))
            if identity is None:
                identity = self._probe.capture(process.pid)
            outcome = await self._terminate_run(wt_uid, run_id, identity)
            report.outcomes.append(outcome)
            if outcome.confirmed:
                try:
                    await asyncio.wait_for(asyncio.shield(process.wait()), timeout=5)
                except Exception:
                    pass
                self._processes.get(wt_uid, {}).pop(run_id, None)
                self._run_identities.pop((wt_uid, run_id), None)
        for name, local in list(self._contexts.get(wt_uid, {}).items()):
            outcome = await asyncio.to_thread(
                self._probe.terminate_tree, name, local.identity, 1000,
            )
            report.outcomes.append(outcome)
            if outcome.confirmed:
                try:
                    await asyncio.to_thread(self._close_child_sync, local.child)
                except Exception:
                    pass
                self._contexts.get(wt_uid, {}).pop(name, None)
                local.handle.status = "CLOSED"
                local.handle.current_cmd = ""
        return report

    async def close_context(self, wt_uid: str, name: str) -> bool:
        local = self._contexts.get(wt_uid, {}).get(name)
        if local is None:
            return False
        await asyncio.to_thread(
            self._probe.terminate_tree, name, local.identity, 1000,
        )
        # Manual close is a reset path: close the PTY even when the process
        # probe could not prove quiescence, then drop the context so the next
        # command spawns a clean shell. Strict disposal uses dispose_wt instead.
        await asyncio.to_thread(self._close_child_sync, local.child)
        self._contexts.get(wt_uid, {}).pop(name, None)
        local.handle.status = "CLOSED"
        local.handle.current_cmd = ""
        return True

    def _spawn_context_sync(self, name: str, spec: BashExecutionSpec) -> _LocalContext:
        try:
            import pexpect
        except ModuleNotFoundError as e:
            raise RuntimeError("pexpect is not installed") from e

        proc_env = os.environ.copy()
        proc_env.update(spec.env)
        child = pexpect.spawn(
            spec.shell,
            self._shell_args(spec.shell),
            cwd=spec.cwd,
            env=proc_env,
            encoding="utf-8",
            codec_errors="replace",
            echo=False,
            timeout=spec.timeout,
        )
        child.delaybeforesend = 0
        ready = f"__MDIO_READY_{uuid.uuid4().hex}__"
        init_cmd = (
            f"bind 'set enable-bracketed-paste off' 2>/dev/null || true; "
            f"unset PROMPT_COMMAND 2>/dev/null; "
            f"export PS1='{self._PROMPT}'; "
            f"export PS2='{self._PROMPT_CONT}'; "
            f"printf '{ready}\\n'"
        )
        child.sendline(init_cmd)
        child.expect_exact(ready)
        child.expect_exact(self._PROMPT)
        identity = self._probe.capture(child.pid)

        handle = BashContextHandle(
            name=name,
            shell=spec.shell,
            cwd=spec.cwd,
            timeout=spec.timeout,
            env=dict(spec.env),
            engine="pexpect",
            status="IDLE",
            os_pid=str(child.pid),
            pgid=str(identity.pgid) if identity and identity.pgid is not None else self._pgid_for_pid(child.pid),
            create_time=repr(identity.create_time) if identity is not None else "",
            sid=str(identity.sid) if identity and identity.sid is not None else "",
        )
        return _LocalContext(handle=handle, child=child, identity=identity)

    def _shell_args(self, shell_name: str) -> list[str]:
        base = os.path.basename(shell_name)
        if base == "bash":
            return ["--noprofile", "--norc", "-i"]
        if base == "zsh":
            return ["-f", "-i"]
        return ["-i"]

    def _run_context_sync(self, local: _LocalContext, command: str, timeout: int) -> BashResult:
        child = local.child
        try:
            output, exit_code = self._exec_capture_sync(child, command, timeout)
            cwd, _ = self._exec_capture_sync(child, "pwd", min(timeout, 5))
            local.handle.cwd = cwd.strip() or local.handle.cwd
            return BashResult(
                stdout=output.rstrip("\n"),
                stderr="",
                exit_code=exit_code,
                status="DONE" if exit_code == "0" else "FAILED",
                shell=local.handle.shell,
                cwd=local.handle.cwd,
                timeout=timeout,
                os_pid=local.handle.os_pid,
                pgid=local.handle.pgid,
                create_time=local.handle.create_time,
                sid=local.handle.sid,
                liveness="gone",
            )
        except Exception as e:
            try:
                import pexpect
                timed_out = isinstance(e, pexpect.TIMEOUT)
            except Exception:
                timed_out = False
            self._probe.terminate_tree(local.handle.name, local.identity, grace_ms=1000)
            self._close_child_sync(child)
            return BashResult(
                stdout="",
                stderr=str(e),
                exit_code="TIMEOUT" if timed_out else "PEXPECT_ERROR",
                status="CANCELLED" if timed_out else "FAILED",
                shell=local.handle.shell,
                cwd=local.handle.cwd,
                timeout=timeout,
                os_pid=local.handle.os_pid,
                pgid=local.handle.pgid,
                create_time=local.handle.create_time,
                sid=local.handle.sid,
                liveness="gone",
            )

    def _exec_capture_sync(self, child: Any, command: str, timeout: int) -> tuple[str, str]:
        marker = f"__MDIO_DONE_{uuid.uuid4().hex}__"
        wrapped = (
            f"{command}\n"
            "__mdio_exit=$?\n"
            f"printf '\\n{marker}:%s\\n' \"$__mdio_exit\""
        )
        child.sendline(wrapped)
        child.expect(re.compile(rf"{re.escape(marker)}:(\d+)"), timeout=timeout)
        output = self._normalize_text(child.before)
        exit_code = child.match.group(1)
        child.expect_exact(self._PROMPT, timeout=timeout)
        return output, exit_code

    def _normalize_text(self, text: str) -> str:
        normalized = text.replace("\r\n", "\n").replace("\r", "\n")
        normalized = normalized.replace(self._PROMPT, "")
        normalized = normalized.replace(self._PROMPT_CONT, "")
        return normalized

    def _close_child_sync(self, child: Any) -> None:
        if child is None:
            return
        try:
            child.terminate(force=True)
        except Exception:
            try:
                child.close(force=True)
            except Exception:
                pass
