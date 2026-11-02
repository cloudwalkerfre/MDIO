"""Named in-memory mem for `$$ -n/--name`.

Scope is one top-level `MdioRuntime.exec` call.  The ContextVar is set by
`MdioRuntime.exec`; nested `Interpreter.run` calls (map -f) see the same
frame, while hook callbacks start a new exec and therefore a new/empty frame.

Mem access reuses the existing mem FFI:
  read  -> PyProjectStore.query_memory
  write -> PyProjectStore.mem_iter_begin / next / commit / end
No VFS session is involved.
"""

from __future__ import annotations

import logging
import re
import uuid
from contextvars import ContextVar
from dataclasses import dataclass, field
from typing import Any, Optional

from .error import MdioError, ErrCode
from .mdio_ir import SelectorIR

_NAME_RE = re.compile(r"^[A-Za-z0-9_-]+$")
TEMP_DOC_PREFIX = "/.tmp/"

_log = logging.getLogger("mdio.mem")

_EXEC_MEMS: ContextVar[Optional[dict[str, "MemSlot"]]] = ContextVar(
    "mdio_exec_mems", default=None
)


@dataclass
class TempDocFrame:
    exec_id: str
    root: str
    store: Any
    aliases: dict[str, str] = field(default_factory=dict)
    cleanups: list = field(default_factory=list)
    duckdb_runtime: Any = None

    def on_destroy(self, fn) -> None:
        self.cleanups.append(fn)

    def create_doc(self, prefix: str, text: str) -> str:
        return _create_doc(self, prefix, text)

    def read_doc(self, path: str) -> str:
        return self.store.get_text("genesis", str(path)) or ""

    def write_full_doc(self, path: str, text: str) -> str:
        return _write_full_doc(self, path, text)


@dataclass
class ExecFrameToken:
    mem_token: Any
    temp_token: Any = None
    frame: Optional[TempDocFrame] = None
    closed: bool = False


_TEMP_FRAME: ContextVar[Optional[TempDocFrame]] = ContextVar(
    "mdio_temp_doc_frame", default=None
)


def begin_exec_frame(workspace=None):
    """Create and install one exec frame.

    ``workspace`` is optional: without it only the MemSlot frame is created;
    the runtime passes it so a TempDocFrame is created alongside the MemSlot
    frame.
    """
    mem_token = _EXEC_MEMS.set({})
    frame = None
    temp_token = None
    store = None
    root = None
    if workspace is not None:
        try:
            store = workspace.temp_store
            exec_id = uuid.uuid4().hex
            root = f"{TEMP_DOC_PREFIX}{exec_id}/"
            store.mk("genesis", root, True, False)
            frame = TempDocFrame(exec_id=exec_id, root=root, store=store)
            temp_token = _TEMP_FRAME.set(frame)
        except Exception:
            if store is not None and root is not None:
                try:
                    store.rm("genesis", root, recursive=True,
                             force=True, verbose=False)
                except Exception:
                    pass
            _EXEC_MEMS.reset(mem_token)
            raise
    return ExecFrameToken(mem_token=mem_token, temp_token=temp_token, frame=frame)


def end_exec_frame(token) -> None:
    """Tear down one exec frame; temp docs go by folder object."""
    if not isinstance(token, ExecFrameToken):
        _EXEC_MEMS.reset(token)
        return
    if token.closed:
        return
    token.closed = True

    frame = token.frame
    # DuckDBRuntime must shut down while the temp frame/snapshots are still
    # alive; doc sessions are closed before the temp root is removed.
    if frame is not None:
        runtime = getattr(frame, "duckdb_runtime", None)
        if runtime is not None:
            try:
                runtime.close()
            except Exception as e:
                _log.warning("duckdb runtime cleanup failed for %s: %s", frame.root, e)
            finally:
                frame.duckdb_runtime = None
    try:
        if frame is not None and frame.store is not None:
            frame.store.rm("genesis", frame.root, recursive=True,
                           force=True, verbose=False)
    except Exception as e:
        _log.warning("temp doc cleanup failed for %s: %s", frame.root, e)
    finally:
        if frame is not None:
            for fn in frame.cleanups:
                try:
                    fn()
                except Exception as e:
                    _log.warning("temp doc cleanup callback failed for %s: %s", frame.root, e)
            frame.cleanups.clear()
            frame.aliases.clear()
        if token.temp_token is not None:
            try:
                _TEMP_FRAME.reset(token.temp_token)
            except ValueError:
                pass
        try:
            _EXEC_MEMS.reset(token.mem_token)
        except ValueError:
            pass


def current_temp_frame() -> Optional[TempDocFrame]:
    return _TEMP_FRAME.get()


def resolve_temp_alias(name: str) -> Optional[str]:
    frame = _TEMP_FRAME.get()
    if frame is None or name is None:
        return None
    return frame.aliases.get(str(name))


def register_temp_doc(name: str, text: str) -> str:
    """Write the current exec input as a real temporary document."""
    clean = str(name or "")
    if not _NAME_RE.fullmatch(clean):
        raise MdioError(
            "interpreter",
            ErrCode.PARSE_TOKEN,
            f"非法 mem 名称 {name!r}：只允许字母、数字、_、-",
        )
    frame = _TEMP_FRAME.get()
    if frame is None:
        raise MdioError(
            "interpreter",
            ErrCode.INTERP_CRASH,
            "$$ -n 只能在 MdioRuntime.exec 的 temp doc frame 内使用",
        )

    path = f"{frame.root}{clean}.md"
    frame.store.mk("genesis", frame.root, True, False)
    frame.store.mk("genesis", path, True, False)
    frame.store.write_locator(
        "genesis", path, SelectorIR(file_path=None, steps=[]),
        "/>", str(text or ""), True, True, False, True, proj=None,
    )
    frame.aliases[clean] = path
    return path


def _create_doc(frame: TempDocFrame, prefix: str, text: str) -> str:
    """Create an internal temp doc with a random name; no user alias."""
    name = f"__{prefix}_{uuid.uuid4().hex[:8]}.md"
    path = f"{frame.root}{name}"
    frame.store.mk("genesis", frame.root, True, False)
    frame.store.mk("genesis", path, True, False)
    frame.store.write_locator(
        "genesis", path, SelectorIR(file_path=None, steps=[]),
        "/>", str(text or ""), True, True, False, True, proj=None,
    )
    return path


def create_temp_doc(prefix: str, text: str) -> str:
    """Create an internal temp doc in the current exec frame."""
    frame = _TEMP_FRAME.get()
    if frame is None:
        raise MdioError(
            "interpreter", ErrCode.INTERP_CRASH,
            "临时内部文档只能在 MdioRuntime.exec 的 temp doc frame 内创建",
        )
    return _create_doc(frame, str(prefix), text)


def read_temp_doc(path: str) -> str:
    frame = _TEMP_FRAME.get()
    if frame is None:
        raise MdioError("interpreter", ErrCode.INTERP_CRASH, "没有当前 temp doc frame")
    return frame.store.get_text("genesis", str(path)) or ""


def _write_full_doc(frame: TempDocFrame, path: str, text: str) -> str:
    frame.store.mk("genesis", frame.root, True, False)
    frame.store.mk("genesis", str(path), True, False)
    frame.store.write_locator(
        "genesis", str(path), SelectorIR(file_path=None, steps=[]),
        "/>", str(text or ""), True, True, False, True, proj=None,
    )
    return str(path)


def write_full_temp_doc(path: str, text: str) -> str:
    frame = _TEMP_FRAME.get()
    if frame is None:
        raise MdioError("interpreter", ErrCode.INTERP_CRASH, "没有当前 temp doc frame")
    return _write_full_doc(frame, path, text)


def current_exec_frame() -> Optional[dict[str, "MemSlot"]]:
    return _EXEC_MEMS.get()


def resolve_mem(name: str) -> Optional["MemSlot"]:
    frame = _EXEC_MEMS.get()
    if frame is None:
        return None
    return frame.get(str(name))


def next_auto_mem_name(stage: str) -> str:
    """Allocate __<stage>_<n> in the current exec frame."""
    frame = _EXEC_MEMS.get()
    if frame is None:
        raise MdioError(
            "interpreter",
            ErrCode.INTERP_CRASH,
            "自动命名 mem 只能在 MdioRuntime.exec 的 exec frame 内使用",
        )
    temp_frame = _TEMP_FRAME.get()
    temp_names = temp_frame.aliases if temp_frame is not None else {}
    n = 1
    while f"__{stage}_{n}" in frame or f"__{stage}_{n}" in temp_names:
        n += 1
    return f"__{stage}_{n}"


def register_mem(name: str, slot: "MemSlot") -> None:
    """Register a MemSlot in the current exec frame.

    Name validation is intentionally narrow: a bare mem name, no path
    separators.  The `__` prefix remains a convention, not a requirement.
    """
    clean = str(name or "")
    if not _NAME_RE.fullmatch(clean):
        raise MdioError(
            "interpreter",
            ErrCode.PARSE_TOKEN,
            f"非法 mem 名称 {name!r}：只允许字母、数字、_、-",
        )
    frame = _EXEC_MEMS.get()
    if frame is None:
        raise MdioError(
            "interpreter",
            ErrCode.INTERP_CRASH,
            "$$ -n 只能在 MdioRuntime.exec 的 exec frame 内使用",
        )
    frame[clean] = slot


class MemSlot:
    """A writable in-memory Markdown document, addressable as a named mem."""

    def __init__(self, store, text: str = "") -> None:
        self.store = store
        self.text = str(text or "")

    def read(self, selector: SelectorIR, proj: Optional[str] = None) -> list[str]:
        return list(self.store.query_memory(self.text, selector, proj=proj))

    def write(self, selector: SelectorIR, operator: str, payload: str,
              proj: Optional[str] = None) -> int:
        """Apply one standard write op to every selector hit.

        Returns the hit count.  Raises when nothing matched, matching the
        existing write-op expectation that a missing target is an error.
        """
        write_proj = proj
        if write_proj is None and operator not in ("->", "~>"):
            write_proj = "content"

        h = self.store.mem_iter_begin(self.text, selector, proj=None)
        hits = 0
        try:
            while h.next() is not None:
                h.commit(payload, operator, write_proj)
                hits += 1
            new_text = h.end()
        except Exception as e:
            raise MdioError(
                "interpreter", ErrCode.INTERP_CRASH,
                f"mem 写入失败: {e}", cause=e,
            )

        if hits == 0:
            raise MdioError(
                "interpreter", ErrCode.INTERP_CRASH,
                "mem 写入目标不存在",
            )

        self.text = new_text
        return hits
