"""
CRON Plugin — time-based pipeline scheduling.

Modes:
  -block "5s"         blocking delay + data passthrough
  -at "2026-..."      one-shot registration (requires -cb)
  -every "30s"        recurring registration (requires -cb)
  -cron "0 9 * * *"   cron-expression registration (requires -cb)

Management: -list, -del
"""

from __future__ import annotations

import asyncio
import json
import re
import time
import uuid
from dataclasses import asdict
from typing import Any, Optional

import dateparser
from croniter import croniter
from datetime import datetime, timezone

from mdio_py.core.selector_builder import SelectorBuilder, by_tag, drill

from mdio_py.plugins.plugin_base import PluginBase
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.error import MdioError, ErrCode
from mdio_py.core.text_utils import unquote_plain, parse_kv_lines
from mdio_py.core.ast_utils import bullets_to_kv



# Time parsing


_DURATION_RE = re.compile(r"^(\d+)\s*(s|m|h|d)$")
_COMPOUND_RE = re.compile(r"(\d+\.?\d*)\s*([smhd])")
_UNIT_TO_SECONDS = {"s": 1, "m": 60, "h": 3600, "d": 86400}

# dateparser: always interpret input as UTC, return tz-aware datetime
_DATEPARSER_SETTINGS = {
    "TIMEZONE": "UTC",
    "RETURN_AS_TIMEZONE_AWARE": True,
    "PREFER_DATES_FROM": "future",
    "DATE_ORDER": "YMD",  # ISO / Chinese: year-month-day
}


def _parse_relative_duration(raw: str) -> int | None:
    """Parse '30s', '5m', '2h', '7d' → seconds.  Returns None on failure."""
    m = _DURATION_RE.match(raw.strip())
    if not m:
        return None
    return int(m.group(1)) * _UNIT_TO_SECONDS[m.group(2)]


def _parse_absolute_time(raw: str) -> int | None:
    """Parse a wide range of human-readable time strings → epoch seconds.

    Uses dateparser which handles:
      - ISO 8601:    2026-05-01T09:00:00Z, 2026-05-01
      - Common:      2026/05/01, May 1 2026
      - Relative:    tomorrow, in 2 hours, 5 min
      - Combined:    1h30m, 5m30s
      - Natural:     5 seconds, 2 hours, 3 days

    All results are treated as UTC.
    Returns None when the string cannot be parsed.
    """
    raw = raw.strip()
    dt = dateparser.parse(raw, settings=_DATEPARSER_SETTINGS)
    if dt is None:
        return None
    # dateparser with RETURN_AS_TIMEZONE_AWARE=True returns tz-aware datetime
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return int(dt.timestamp())


def _parse_time_spec(raw: str) -> tuple[str, int | None]:
    """Return (kind, epoch_or_duration_seconds).

    kind is one of: 'duration', 'absolute'.
    Returns (kind, None) when the string cannot be parsed.

    Priority: simple regex durations first (for -block),
    then dateparser for everything else.
    """
    raw = raw.strip()
    seconds = _parse_relative_duration(raw)
    if seconds is not None:
        return ("duration", seconds)
    epoch = _parse_absolute_time(raw)
    if epoch is not None:
        return ("absolute", epoch)
    return ("invalid", None)


def _parse_compound_duration(raw: str) -> float | None:
    """Parse '1h30m', '2m30s', '5s', '0.1s' → total seconds.  Returns None on failure."""
    raw = raw.strip()
    total = 0.0
    for m in _COMPOUND_RE.finditer(raw):
        total += float(m.group(1)) * _UNIT_TO_SECONDS[m.group(2)]
    consumed = sum(m.end() - m.start() for m in _COMPOUND_RE.finditer(raw))
    return total if consumed == len(raw) and total > 0 else None


def _parse_interval(raw: str) -> float | None:
    """Parse a duration string to seconds. Compound first, then dateparser for natural language."""
    dur = _parse_compound_duration(raw)
    if dur is not None:
        return dur
    epoch = _parse_absolute_time(raw)
    if epoch is not None:
        return max(0.0, epoch - time.time())
    return None


def _calc_next_run(mode: str, schedule: str, base_time: float | None = None) -> int:
    """Compute the *next* trigger time (epoch seconds)."""
    base = base_time or time.time()
    if mode == "at":
        kind, val = _parse_time_spec(schedule)
        if kind == "absolute":
            return val
        return int(base) + (_parse_relative_duration(schedule) or 0)

    if mode == "every":
        dur = _parse_interval(schedule)
        if dur is None:
            raise MdioError("CRON", ErrCode.INTERP_CRASH,
                            f"Cannot parse -every interval: {schedule}")
        return int(base + dur)

    if mode == "cron":
        try:
            dt = datetime.fromtimestamp(base, tz=timezone.utc)
            c = croniter(schedule, dt)
            return int(c.get_next(datetime).timestamp())
        except Exception as e:
            raise MdioError("CRON", ErrCode.INTERP_CRASH,
                            f"Invalid cron expression '{schedule}': {e}")

    raise MdioError("CRON", ErrCode.INTERP_CRASH, f"Unknown mode: {mode}")



# CronPlugin


class CRONPlugin(PluginBase):
    """@CRON — time-based pipeline scheduler."""

    _timers: dict[str, asyncio.Task]

    PROC_FIELDS = ("status", "tick", "next_run", "last_run")

    def __init__(self) -> None:
        self._timers = {}

    @property
    def protocol_name(self) -> str:
        return "CRON"


    # __call__ dispatcher


    async def __call__(
        self,
        workspace,
        input_data: Any,
        chain: list,
        ctx: RuntimeContext,
        kwargs: dict,
    ) -> Any:
        # -- Management commands --
        if "-list" in kwargs:
            return await self._list(workspace, ctx, kwargs)
        if "-del" in kwargs:
            return await self._delete(workspace, ctx, self._unquote(kwargs["-del"]))

        # -- -block: delay + passthrough --
        if "-block" in kwargs:
            time_spec = self._unquote(kwargs["-block"])
            dur = _parse_interval(time_spec)
            if dur is None:
                return f"- mdio_error: [CRON] Cannot parse -block time: {time_spec}"
            await asyncio.sleep(dur)
            return input_data

        # -- -at: one-shot registration --
        if "-at" in kwargs:
            if "-cb" not in kwargs:
                return "- mdio_error: [CRON] -at requires -cb"
            return await self._register(workspace, ctx, "at",
                                        self._unquote(kwargs["-at"]), kwargs)

        # -- -every: recurring registration --
        if "-every" in kwargs:
            if "-cb" not in kwargs:
                return "- mdio_error: [CRON] -every requires -cb"
            return await self._register(workspace, ctx, "every",
                                        self._unquote(kwargs["-every"]), kwargs)

        # -- -cron: cron-expression registration --
        if "-cron" in kwargs:
            if "-cb" not in kwargs:
                return "- mdio_error: [CRON] -cron requires -cb"
            return await self._register(workspace, ctx, "cron",
                                        self._unquote(kwargs["-cron"]), kwargs)

        # Default: list all
        return await self._list(workspace, ctx, kwargs)


    # Registration


    async def _register(
        self, workspace, ctx: RuntimeContext, mode: str, schedule: str, kwargs: dict
    ) -> str:
        user_pipeline = self._unquote(kwargs.get("-cb", ""))
        if not user_pipeline:
            return "- mdio_error: [CRON] -cb pipeline is empty"

        job_id = f"cron_{uuid.uuid4().hex[:8]}"
        created_at = int(time.time())

        try:
            if mode == "every":
                dur = _parse_interval(schedule)
                if dur is None:
                    return f"- mdio_error: [CRON] Cannot parse -every interval: {schedule}"
                if dur < 0.1:
                    return "- mdio_error: [CRON] -every minimum interval is 0.1s"
            next_run = _calc_next_run(mode, schedule, base_time=created_at)
        except MdioError as e:
            return e.to_markdown()

        await self._ensure_cron_md(workspace, ctx)

        # 1. Write cron.md job definition — per-key via write_locator
        container = "CRON_JOBS"
        cp = self._cron_md_path(ctx)
        fields = {
            "mode": mode, "schedule": schedule, "pipeline": user_pipeline,
            "enabled": "true", "wt_uid": ctx.wt_uid,
            "cwd": ctx.vfs_path, "created_at": created_at,
        }
        for key, value in fields.items():
            workspace.write_locator(
                ctx.wt_uid, cp,
                by_tag(f"{container}.{job_id}", key),
                "/>", str(value), auto_scaffold=True,
            )

        # 2. Write proc.md runtime entry (aligned with PluginBase._register_proc)
        payload = {
            "tick": "0",
            "next_run": str(next_run),
            "last_run": "-",
        }
        await self._register_proc(
            workspace, ctx, identifier=mode, ppid="root",
            payload=payload, exact_pid=job_id, register_task=False,
        )

        # 3. Register Hook (watches :tick).
        # Always is_always=True — the Hook is removed explicitly on -del / cleanup.
        wrapped_cb = self._make_wrapped_cb(job_id, user_pipeline)
        hook_ir = by_tag(f"PROCESS_TABLE.CRON.{job_id}", "tick")
        hook_id = workspace.store.register_hook(
            ctx.wt_uid,
            ctx.proc_path,
            hook_ir,
            wrapped_cb,
            asdict(ctx),
            is_always=True,
        )

        # 4. Record hook_id in proc.md
        workspace.write_locator(
            ctx.wt_uid, ctx.proc_path,
            by_tag(f"PROCESS_TABLE.CRON.{job_id}", "hook_id"),
            "/>", str(hook_id), auto_scaffold=True,
        )

        # 5. Start per-job timer loop & register in active_tasks
        task = asyncio.create_task(
            self._timer_loop(workspace, ctx, job_id, mode, schedule)
        )
        self._timers[job_id] = task
        wt_tasks = workspace.active_tasks.setdefault(ctx.wt_uid, {})
        wt_tasks[job_id] = task

        return f"- cron: {job_id} registered [{mode}: {schedule}]"


    # Per-job timer loop


    async def _timer_loop(
        self,
        workspace,
        ctx: RuntimeContext,
        job_id: str,
        mode: str,
        schedule: str,
    ) -> None:
        """Single-job timer.  Sleeps until next_run, writes tick, repeats.

        Like Unix cron, there is no overlap protection — a new pipeline
        starts on every tick regardless of whether the previous one finished.
        """
        proc_prefix = f"{ctx.proc_path}::PROCESS_TABLE.CRON.{job_id}"

        try:
            while True:
                state = await self._read_proc_state(workspace, ctx, job_id)
                if state is None:
                    break

                now = time.time()
                next_run = float(state.get("next_run", 0))

                if now >= next_run:
                    tick = int(state.get("tick", 0)) + 1
                    proc_path = ctx.proc_path
                    prefix = f"PROCESS_TABLE.CRON.{job_id}"

                    # Status bookkeeping with suppress_hooks to prevent cascade.
                    workspace.write_locator(
                        ctx.wt_uid, proc_path,
                        by_tag(f"{prefix}", "status"),
                        "/>", "RUNNING", auto_scaffold=True, suppress_hooks=True,
                    )

                    # Write tick — triggers hook (runs user_pipeline, no status wrapping).
                    workspace.write_locator(
                        ctx.wt_uid, proc_path,
                        by_tag(f"{prefix}", "tick"),
                        "/>", str(tick), auto_scaffold=True,
                    )

                    if mode == "at":
                        await asyncio.sleep(0.1)
                        await self._cleanup_job(workspace, ctx, job_id)
                        return

                    new_next = _calc_next_run(mode, schedule, now)
                    workspace.write_locator(
                        ctx.wt_uid, proc_path,
                        by_tag(f"{prefix}", "next_run"),
                        "/>", str(new_next), auto_scaffold=True, suppress_hooks=True,
                    )
                    workspace.write_locator(
                        ctx.wt_uid, proc_path,
                        by_tag(f"{prefix}", "status"),
                        "/>", "IDLE", auto_scaffold=True, suppress_hooks=True,
                    )
                    workspace.write_locator(
                        ctx.wt_uid, proc_path,
                        by_tag(f"{prefix}", "last_run"),
                        "/>", str(int(time.time())), auto_scaffold=True, suppress_hooks=True,
                    )

                    sleep_time = new_next - time.time()
                else:
                    sleep_time = next_run - now

                if sleep_time > 0:
                    await asyncio.sleep(sleep_time)

        except asyncio.CancelledError:
            pass
        finally:
            wt_tasks = workspace.active_tasks.get(ctx.wt_uid, {})
            wt_tasks.pop(job_id, None)


    # Management commands


    async def _list(self, workspace, ctx: RuntimeContext, kwargs: dict) -> str:
        raw = kwargs.get("-list", "")
        # When -list has no explicit value, the parser yields True/False.
        if isinstance(raw, bool) or str(raw).strip().lower() in ("true", ""):
            target = ""
        else:
            target = self._unquote(raw)
        if target:
            # Single job detail
            return await self._single_job_info(workspace, ctx, target)

        # List all
        lines = ["# CRON_JOBS"]

        # Read cron.md for definitions
        cron_defs = await self._read_cron_defs(workspace, ctx)
        if not cron_defs:
            return "- cron: No jobs registered."

        for job_id, cdef in cron_defs.items():
            enabled = cdef.get("enabled", "true")
            mode = cdef.get("mode", "?")
            schedule = cdef.get("schedule", "?")

            # Read runtime state from proc.md
            state = await self._read_proc_state(workspace, ctx, job_id)
            status = state.get("status", "?") if state else "?"
            last_run = state.get("last_run", "-") if state else "-"
            tick = state.get("tick", "0") if state else "0"

            lines.append(
                f"## {job_id}"
            )
            lines.append(f"- mode: {mode}")
            lines.append(f"- schedule: {schedule}")
            lines.append(f"- status: {status}")
            lines.append(f"- tick: {tick}")
            lines.append(f"- last_run: {last_run}")
            lines.append(f"- pipeline: {cdef.get('pipeline', '?')}")

        return "\n".join(lines)

    async def _single_job_info(self, workspace, ctx: RuntimeContext, job_id: str) -> str:
        cdef = await self._read_cron_def(workspace, ctx, job_id)
        if not cdef:
            return f"- mdio_error: [CRON] Job {job_id} not found"
        state = await self._read_proc_state(workspace, ctx, job_id) or {}
        lines = [f"# {job_id}"]
        for k, v in cdef.items():
            lines.append(f"- {k}: {v}")
        for k in self.PROC_FIELDS:
            if k in state:
                lines.append(f"- {k}: {state[k]}")
        return "\n".join(lines)




    async def _delete(self, workspace, ctx: RuntimeContext, job_id: str) -> str:
        if not job_id:
            return "- mdio_error: [CRON] -del requires job_id"
        if not await self._job_exists(workspace, ctx, job_id):
            return f"- mdio_error: [CRON] Job {job_id} not found"

        await self._cleanup_job(workspace, ctx, job_id)
        return f"- cron: {job_id} deleted"


    # Cleanup


    async def _cleanup_job(self, workspace, ctx: RuntimeContext, job_id: str) -> None:
        """Remove everything: timer, Hook, proc.md entry, cron.md entry."""
        # Cancel timer task
        task = self._timers.pop(job_id, None)
        if task is not None:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass

        proc_prefix = f"{ctx.proc_path}::PROCESS_TABLE.CRON.{job_id}"

        # Remove the Hook first — stop future callbacks
        try:
            raw = await workspace.runtime.exec(f"${proc_prefix}._b:hook_id", ctx=ctx)
            if raw and "- mdio_error:" not in raw:
                h_id_str = raw.strip()
                if h_id_str.isdigit():
                    workspace.store.remove_hook(int(h_id_str))
        except Exception:
            pass

        # Yield to let any already-scheduled callback complete
        await asyncio.sleep(0)

        # Unregister from proc.md + active_tasks
        try:
            await self._unregister_proc(workspace, ctx, job_id, is_daemon=False)
        except Exception:
            pass

        # Remove cron.md entry
        try:
            workspace.write_locator(
                ctx.wt_uid, self._cron_md_path(ctx),
                drill(f"CRON_JOBS.{job_id}"),
                "->", "", auto_scaffold=False,
            )
        except Exception:
            pass


    # Bootstrap (called after proc.md wipe)


    async def bootstrap(self, workspace) -> None:
        """Rebuild all CRON jobs from cron.md files after a proc.md wipe.

        Iterates all WTs via list_worktrees(). Each WT's cron.md is processed
        with the WT's uid for correct context resolution.
        """
        wt_list = workspace.store.list_worktrees()
        for wt_uid, _name, _storage_root, _ext_path in wt_list:
            try:
                if workspace.store.path_exists(wt_uid, "/.mds/cron.md"):
                    self._migrate_cdef_key(workspace, wt_uid)  # 补齐 mount_id→wt_uid 旧键改名（幂等）
                    await self._bootstrap_one_file(workspace, wt_uid)
            except Exception:
                pass

    def _migrate_cdef_key(self, workspace, wt_uid: str) -> None:
        """cron.md 旧键 `- mount_id:` → `- wt_uid:` 改名。

        文本级替换（幂等：无旧键则 no-op，含 wt_uid 键的条目不受影响）。"""
        try:
            text = workspace.store.get_text(wt_uid, "/.mds/cron.md")
        except Exception:
            return
        if not text or "- mount_id:" not in text:
            return
        new_text = text.replace("- mount_id:", "- wt_uid:")
        workspace.write_locator(
            wt_uid, "/.mds/cron.md",
            SelectorBuilder().build(), "/>", new_text,
            auto_scaffold=True, god=True, suppress_hooks=True,
        )

    async def _bootstrap_one_file(self, workspace, wt_uid: str) -> None:
        """Rebuild jobs from a single WT's cron.md, using wt_uid for context."""

        try:
            ir = SelectorBuilder().drill("CRON_JOBS").any_child().build()
            children = workspace.read_locator(wt_uid, "/.mds/cron.md", ir)
        except Exception:
            return
        if not children:
            return

        # Parse job definitions from selector results
        cron_defs = self._parse_cron_chunks(children, workspace=workspace)
        if not cron_defs:
            return

        for job_id, cdef in cron_defs.items():
            if not self._str_bool(cdef.get("enabled", "true")):
                continue

            # cdef 条目按 wt_uid 归属 worktree
            job_uid = cdef.get("wt_uid", "")
            if job_uid != wt_uid:
                continue  # skip jobs from other WTs

            job_wt_uid = wt_uid
            cwd = cdef.get("cwd", "/")
            mode = cdef.get("mode", "")
            schedule = cdef.get("schedule", "")
            pipeline = cdef.get("pipeline", "")
            now = int(time.time())
            created_at = int(cdef.get("created_at", "0"))
            if created_at == 0:
                created_at = now  # 缺字段时按当前时间补 created_at

            if not mode or not schedule or not pipeline:
                continue

            job_ctx = RuntimeContext(wt_uid=job_wt_uid, vfs_path=cwd, active_doc=None)

            # Compute next_run from the original registration time (created_at).
            if mode == "at":
                # One-shot: absolute trigger time, computed from creation context.
                # If the trigger time has passed, skip — it's intentionally missed.
                trigger_at = _calc_next_run(mode, schedule, base_time=created_at)
                if now >= trigger_at:
                    continue  # missed it
                next_run = trigger_at
            elif mode == "every":
                # Recurring interval: align to the original cadence.
                dur = _parse_interval(schedule)
                if dur is not None and dur > 0:
                    elapsed = now - created_at
                    cycles = elapsed // dur
                    next_run = created_at + (cycles + 1) * dur
                else:
                    next_run = _calc_next_run(mode, schedule, base_time=created_at)
            else:  # cron
                next_run = _calc_next_run(mode, schedule)
            if next_run <= now:
                next_run = _calc_next_run(mode, schedule)  # fallback

            # Rebuild proc.md entry
            proc_path = f"{job_ctx.sys_dir()}/proc.md"
            proc_fields = {"status": "IDLE", "tick": "0", "next_run": str(next_run), "last_run": "-"}
            container = "PROCESS_TABLE.CRON"
            workspace.write_locator(
                job_wt_uid, proc_path, drill(f"{container}.{job_id}"),
                "+>", "", auto_scaffold=True,
            )
            for key, value in proc_fields.items():
                workspace.write_locator(
                    job_wt_uid, proc_path,
                    by_tag(f"{container}.{job_id}", key),
                    "/>", str(value), auto_scaffold=True,
                )

            # Re-register Hook
            wrapped_cb = self._make_wrapped_cb(job_id, pipeline)
            hook_ir = by_tag(f"PROCESS_TABLE.CRON.{job_id}", "tick")
            hook_id = workspace.store.register_hook(
                job_wt_uid,
                f"{job_ctx.sys_dir()}/proc.md",
                hook_ir,
                wrapped_cb,
                asdict(job_ctx),
                is_always=True,
            )

            # Record hook_id
            workspace.write_locator(
                job_wt_uid, proc_path,
                by_tag(f"PROCESS_TABLE.CRON.{job_id}", "hook_id"),
                "/>", str(hook_id), auto_scaffold=True,
            )

            # Start timer loop (if not already running)
            if job_id not in self._timers:
                task = asyncio.create_task(
                    self._timer_loop(workspace, job_ctx, job_id,
                                     mode, schedule)
                )
                self._timers[job_id] = task
                wt_tasks = workspace.active_tasks.setdefault(job_ctx.wt_uid, {})
                wt_tasks[job_id] = task


    # Helpers


    def _make_wrapped_cb(self, job_id: str, user_pipeline: str) -> str:
        """Return user pipeline directly. Status/last_run are written by _timer_loop
        with suppress_hooks=True to prevent cascade."""
        return user_pipeline

    @staticmethod
    def _cron_md_path(ctx: RuntimeContext) -> str:
        return f"{ctx.sys_dir()}/cron.md"

    async def _ensure_cron_md(self, workspace, ctx: RuntimeContext) -> None:
        """Ensure cron.md exists in the current worktree."""
        cp = self._cron_md_path(ctx)
        try:
            if not workspace.store.path_exists(ctx.wt_uid, cp):
                workspace.write_locator(
                    ctx.wt_uid, cp,
                    drill("CRON_JOBS"),
                    "/>", "# CRON_JOBS\n", auto_scaffold=True, god=True,
                )
        except Exception:
            pass

    async def _job_exists(self, workspace, ctx: RuntimeContext, job_id: str) -> bool:
        """Check if a job definition exists in cron.md."""
        cdef = await self._read_cron_def(workspace, ctx, job_id)
        return cdef is not None

    async def _read_cron_defs(self, workspace, ctx: RuntimeContext) -> dict[str, dict]:
        """Read all job definitions from cron.md via selector traversal."""
        cp = self._cron_md_path(ctx)
        try:
            ir = SelectorBuilder().drill("CRON_JOBS").any_child().build()
            children = workspace.read_locator(ctx.wt_uid, cp, ir)
        except Exception:
            return {}
        return self._parse_cron_chunks(children, workspace=workspace)

    async def _read_cron_def(self, workspace, ctx: RuntimeContext, job_id: str) -> dict | None:
        """Read a single job definition from cron.md."""
        defs = await self._read_cron_defs(workspace, ctx)
        return defs.get(job_id)

    @staticmethod
    def _engine_kv(workspace, text: str) -> dict[str, str] | None:
        """新路径：`workspace.to_json_text` → 信封 → `bullets_to_kv`（句柄缺失/异常 → None）。"""
        if workspace is None:
            return None
        try:
            return bullets_to_kv(json.loads(workspace.to_json_text(text)))
        except Exception:
            return None

    @staticmethod
    def _parse_cron_chunks(chunks: list[str], workspace=None) -> dict[str, dict]:
        """Parse cron.md child chunks into {job_id: {key: value, ...}}.

        Each chunk is the raw text of a ## job_id section produced by
        ``drill('CRON_JOBS').any_child()`` with default (raw) projection.

        有 workspace 句柄走引擎；标题行剥掉（`lines[1:]`）后再喂引擎，输入形态不变。
        无句柄/引擎异常时走行解析。
        cron 不做 unquote 后处理（`_unquote` 只用于 CLI kwargs）。
        """
        result: dict[str, dict] = {}
        for chunk in chunks:
            lines = chunk.strip().split("\n")
            if not lines:
                continue
            name = lines[0].strip().lstrip("#").strip()
            if name:
                body = "\n".join(lines[1:])
                kv = CRONPlugin._engine_kv(workspace, body)
                if kv is None:
                    kv = parse_kv_lines(body)
                result[name] = kv
        return result

    async def _read_proc_state(self, workspace, ctx: RuntimeContext, job_id: str) -> dict | None:
        """Read runtime state from proc.md via selector navigation."""
        heading = f"PROCESS_TABLE.CRON.{job_id}"
        try:
            chunks = workspace.read_locator(ctx.wt_uid, ctx.proc_path, drill(heading))
        except Exception:
            return None
        if not chunks:
            return None
        kv = self._engine_kv(workspace, chunks[0])
        if kv is None:
            return parse_kv_lines(chunks[0])
        return kv

    @staticmethod
    def _unquote(s: Any) -> str:
        return unquote_plain(s)   # 值清洗：成对剥引号，不解转义

    @staticmethod
    def _str_bool(v: str | None) -> bool:
        if v is None:
            return False
        return v.strip().lower() in ("true", "1", "yes", "on")
