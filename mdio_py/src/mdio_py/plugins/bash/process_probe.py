from __future__ import annotations

import os
import time
from dataclasses import dataclass

try:
    import psutil
except ModuleNotFoundError as exc:  # pragma: no cover - dependency is declared
    raise RuntimeError("psutil is required by the BASH process owner") from exc

from .models import BashDisposeOutcome


@dataclass(frozen=True)
class ProcessIdentity:
    pid: int
    create_time: float
    pgid: int | None = None
    sid: int | None = None


@dataclass(frozen=True)
class ProcessFact:
    liveness: str
    zombie: bool = False
    children_alive: int | None = None
    message: str = ""


def _safe_getpgid(pid: int) -> int | None:
    try:
        return os.getpgid(pid)
    except Exception:
        return None


def _safe_getsid(pid: int) -> int | None:
    try:
        return os.getsid(pid)
    except Exception:
        return None


class BashProcessProbe:
    """Host-side process facts and tree termination.

    It never owns state: callers pass an identity recorded in proc.md and
    decide whether an observation is a terminal state or only liveness.
    """

    def capture(self, pid: int | str | None) -> ProcessIdentity | None:
        if pid in (None, ""):
            return None
        try:
            proc = psutil.Process(int(pid))
            return ProcessIdentity(
                pid=proc.pid,
                create_time=float(proc.create_time()),
                pgid=_safe_getpgid(proc.pid),
                sid=_safe_getsid(proc.pid),
            )
        except (psutil.NoSuchProcess, ValueError, TypeError):
            return None
        except psutil.AccessDenied:
            return None

    def probe(self, identity: ProcessIdentity | None) -> ProcessFact:
        if identity is None:
            return ProcessFact("unknown", message="no identity")
        try:
            proc = psutil.Process(identity.pid)
        except psutil.NoSuchProcess:
            return ProcessFact("gone", message="process gone")
        except psutil.AccessDenied:
            return ProcessFact("unknown", message="access denied")
        try:
            if abs(float(proc.create_time()) - identity.create_time) > 0.001:
                return ProcessFact("mismatch", message="create_time changed (pid reuse)")
        except psutil.NoSuchProcess:
            return ProcessFact("gone", message="process gone")
        try:
            status = proc.status()
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            return ProcessFact("unknown", message="status unavailable")
        zombie = status == psutil.STATUS_ZOMBIE
        try:
            children = len(proc.children(recursive=False))
        except (psutil.NoSuchProcess, psutil.AccessDenied):
            children = None
        return ProcessFact("alive", zombie=zombie, children_alive=children)

    def tree_members(self, identity: ProcessIdentity) -> tuple[list[ProcessIdentity], bool]:
        found: dict[int, ProcessIdentity] = {}
        incomplete = False
        try:
            root = psutil.Process(identity.pid)
            for proc in [root, *root.children(recursive=True)]:
                try:
                    found[proc.pid] = ProcessIdentity(
                        pid=proc.pid,
                        create_time=float(proc.create_time()),
                        pgid=_safe_getpgid(proc.pid),
                        sid=_safe_getsid(proc.pid),
                    )
                except (psutil.NoSuchProcess, psutil.AccessDenied):
                    incomplete = True
        except psutil.NoSuchProcess:
            pass
        except psutil.AccessDenied:
            incomplete = True
        if identity.sid is not None:
            try:
                for proc in psutil.process_iter(["pid", "create_time"]):
                    try:
                        if os.getsid(proc.pid) != identity.sid:
                            continue
                        found[proc.pid] = ProcessIdentity(
                            pid=proc.pid,
                            create_time=float(proc.create_time()),
                            pgid=_safe_getpgid(proc.pid),
                            sid=identity.sid,
                        )
                    except Exception:
                        # os.getsid raises PermissionError for protected unrelated
                        # processes; the session probe is best-effort.
                        continue
            except Exception:
                incomplete = True
        return list(found.values()), incomplete

    def _signal_group(self, identity: ProcessIdentity, sig: int) -> None:
        pgid = identity.pgid
        if pgid is not None and pgid != os.getpgid(0):
            try:
                os.killpg(pgid, sig)
            except ProcessLookupError:
                pass
            except Exception:
                pass
        for member in self._current_members(identity):
            if member.pid == os.getpid():
                continue
            try:
                os.kill(member.pid, sig)
            except ProcessLookupError:
                pass
            except Exception:
                pass

    def _current_members(self, identity: ProcessIdentity) -> list[ProcessIdentity]:
        members, _ = self.tree_members(identity)
        return members

    def _wait_empty(self, identity: ProcessIdentity, deadline: float) -> tuple[list[ProcessIdentity], bool]:
        while True:
            members, incomplete = self.tree_members(identity)
            alive: list[ProcessIdentity] = []
            for member in members:
                if member.pid == os.getpid():
                    continue
                try:
                    proc = psutil.Process(member.pid)
                    if proc.status() == psutil.STATUS_ZOMBIE:
                        continue
                except (psutil.NoSuchProcess, psutil.AccessDenied):
                    continue
                alive.append(member)
            if not alive:
                return [], incomplete
            if time.monotonic() >= deadline:
                return alive, incomplete
            time.sleep(0.02)

    def terminate_tree(
        self,
        handle_id: str,
        identity: ProcessIdentity | None,
        grace_ms: int = 1000,
    ) -> BashDisposeOutcome:
        if identity is None:
            return BashDisposeOutcome(handle_id=handle_id, confirmed=True, method="gone")
        fact = self.probe(identity)
        if fact.liveness == "gone":
            return BashDisposeOutcome(handle_id=handle_id, confirmed=True, method="gone")
        if fact.liveness == "mismatch":
            return BashDisposeOutcome(
                handle_id=handle_id,
                confirmed=False,
                method="mismatch",
                error="process identity changed (pid reuse)",
            )
        if fact.liveness != "alive":
            return BashDisposeOutcome(
                handle_id=handle_id,
                confirmed=False,
                method="unknown",
                error=fact.message or "process identity unavailable",
            )
        try:
            self._signal_group(identity, 15)  # SIGTERM
        except Exception:
            pass
        survivors, incomplete = self._wait_empty(identity, time.monotonic() + max(0.05, grace_ms / 1000))
        if survivors:
            try:
                self._signal_group(identity, 9)  # SIGKILL
            except Exception:
                pass
            survivors, incomplete2 = self._wait_empty(identity, time.monotonic() + 2.0)
            incomplete = incomplete or incomplete2
        if survivors:
            return BashDisposeOutcome(
                handle_id=handle_id,
                confirmed=False,
                survivors=len(survivors),
                method="sigkill",
                error="process tree survived SIGKILL",
            )
        if incomplete:
            return BashDisposeOutcome(
                handle_id=handle_id,
                confirmed=False,
                method="scan-incomplete",
                error="process scan incomplete; cannot prove quiescence",
            )
        return BashDisposeOutcome(handle_id=handle_id, confirmed=True, method="terminated")

    def is_quiescent(self, identity: ProcessIdentity | None) -> bool:
        if identity is None:
            return True
        members, incomplete = self.tree_members(identity)
        return not incomplete and not any(member.pid != os.getpid() for member in members)
