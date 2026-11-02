"""DSH-compatible host text editor exposed through @BASH.tools.

The tool owns no BASH execution: it edits host files directly.  It mirrors the
DSH edit family closely enough for model use:

- view/create/str_replace/insert from str_replace_editor;
- edit/write from tool-fs;
- read-before-write and version guards per wt/session/path;
- exact literal matching, unique by default, optional replace_all;
- UTF-8 text only, LF matching with original line-ending restoration;
- atomic same-directory rewrite.

The md tool manifest supplies provider=python and module=this module.
"""

from __future__ import annotations

import hashlib
import os
import stat
import tempfile
from pathlib import Path
from typing import Any

_MAX_VIEW = 16_000
_OBSERVED: dict[tuple[str, str, str], str] = {}


class ToolError(Exception):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def _text(raw: bytes, display: str) -> tuple[str, str]:
    if b"\0" in raw:
        raise ToolError("FS_NOT_TEXT", f"cannot edit {display}: binary file")
    try:
        decoded = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise ToolError("FS_NOT_TEXT", f"cannot edit {display}: not valid UTF-8") from exc
    crlf = decoded[:4096].count("\r\n")
    lf = decoded[:4096].count("\n") - crlf
    return decoded.replace("\r\n", "\n"), ("CRLF" if crlf > lf else "LF")


def _restore(content: str, line_endings: str) -> str:
    if line_endings == "CRLF":
        return content.replace("\n", "\r\n")
    return content


def _token(raw: bytes, st: os.stat_result) -> str:
    return f"{st.st_mtime_ns}:{st.st_size}:{hashlib.sha256(raw).hexdigest()[:16]}"


def _current_token(path: Path) -> str:
    st = path.stat()
    return _token(path.read_bytes(), st)


def _key(wt_uid: str, session: str, path: Path) -> tuple[str, str, str]:
    return (wt_uid, session, str(path))


def _remember(wt_uid: str, session: str, path: Path, raw: bytes, st: os.stat_result) -> None:
    _OBSERVED[_key(wt_uid, session, path)] = _token(raw, st)


def _forget(wt_uid: str, session: str, path: Path) -> None:
    _OBSERVED.pop(_key(wt_uid, session, path), None)


def _require_observed(wt_uid: str, session: str, path: Path) -> None:
    expected = _OBSERVED.get(_key(wt_uid, session, path))
    if expected is None:
        raise ToolError(
            "FS_NOT_OBSERVED",
            f"cannot edit {path}: read the file with view first",
        )
    try:
        current = _current_token(path)
    except FileNotFoundError as exc:
        raise ToolError("FS_STALE_VERSION", f"cannot edit {path}: file changed since it was read") from exc
    if current != expected:
        raise ToolError("FS_STALE_VERSION", f"cannot edit {path}: file changed since it was read")


def _resolve(cwd: str, raw: Any) -> Path:
    value = "" if raw is None else str(raw)
    if not value.strip():
        raise ToolError("FS_INVALID_ARGUMENT", "path must be a non-empty string")
    path = Path(value).expanduser()
    if not path.is_absolute():
        path = Path(cwd) / path
    return path.resolve(strict=False)


def _read(path: Path, wt_uid: str, session: str) -> tuple[str, str, bytes, os.stat_result]:
    if not path.exists():
        _forget(wt_uid, session, path)
        raise ToolError("FS_NOT_FOUND", f"The path {path} does not exist. Please provide a valid path.")
    if not path.is_file():
        raise ToolError("FS_NOT_REGULAR_FILE", f"cannot edit {path}: not a regular file")
    st = path.stat()
    raw = path.read_bytes()
    text, endings = _text(raw, str(path))
    _remember(wt_uid, session, path, raw, st)
    return text, endings, raw, st


def _atomic_write(path: Path, text: str, mode: int | None = None) -> None:
    parent = path.parent
    if not parent.is_dir():
        raise ToolError("FS_NOT_FOUND", f"parent directory does not exist: {parent}")
    data = text.encode("utf-8")
    if mode is None:
        current_umask = os.umask(0)
        os.umask(current_umask)
        mode = 0o666 & ~current_umask
    fd, tmp_name = tempfile.mkstemp(prefix=f".{path.name}.", suffix=".tmp", dir=str(parent))
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(data)
            fh.flush()
            os.fsync(fh.fileno())
        os.chmod(tmp, stat.S_IMODE(mode))
        os.replace(tmp, path)
    except BaseException:
        try:
            tmp.unlink()
        except FileNotFoundError:
            pass
        raise


def _clip_view(text: str) -> str:
    if len(text) <= _MAX_VIEW:
        return text
    return text[:_MAX_VIEW] + "\n<response clipped>"


def _format_view(path: Path, text: str, view_range: Any) -> str:
    lines = text.split("\n")
    total = len(lines)
    initial = 1
    final = None
    if view_range is not None:
        if not isinstance(view_range, list) or len(view_range) != 2 or not all(isinstance(x, int) for x in view_range):
            raise ToolError("FS_INVALID_ARGUMENT", "Invalid view_range. It should be a list of two integers.")
        initial, final = view_range
        if initial < 1 or initial > total:
            raise ToolError("FS_INVALID_ARGUMENT", f"Invalid view_range: [{initial}, {final}]")
        if final != -1 and final > total:
            raise ToolError("FS_INVALID_ARGUMENT", f"Invalid view_range: [{initial}, {final}]")
        if final != -1 and final < initial:
            raise ToolError("FS_INVALID_ARGUMENT", f"Invalid view_range: [{initial}, {final}]")
        lines = lines[initial - 1:] if final == -1 else lines[initial - 1:final]
    numbered = "\n".join(f"{str(initial + i).rjust(6)}  {line}" for i, line in enumerate(lines))
    suffix = f" with view_range=[{initial}, {final}]" if view_range is not None else ""
    return _clip_view(
        f"Here's the content of {path} with line numbers (which has a total of {total} lines){suffix}:\n{numbered}\n"
    )


def _list_dir(path: Path) -> str:
    rows = [f"d\t{path}"]

    def visit(directory: Path, depth: int) -> None:
        try:
            children = sorted(directory.iterdir(), key=lambda p: p.name)
        except OSError:
            return
        for child in children:
            if child.name.startswith(".") or child.name in ("node_modules", "__pycache__"):
                continue
            if child.is_dir():
                rows.append(f"d\t{child}")
                if depth < 2:
                    visit(child, depth + 1)
            elif child.is_file():
                rows.append(f"f\t{child}")

    visit(path, 1)
    rows[1:] = sorted(rows[1:], key=lambda row: row.split("\t", 1)[1])
    return _clip_view(
        "Here're the files and directories up to 2 levels deep in "
        f"{path}, excluding hidden items, node_modules, and Python cache directories:\n"
        + "\n".join(rows)
        + "\n"
    )


def _content_arg(args: dict[str, Any], stdin_text: str | None) -> str:
    if "content" in args:
        return str(args["content"])
    if "file_text" in args:
        return str(args["file_text"])
    if stdin_text is not None:
        return stdin_text
    raise ToolError("FS_INVALID_ARGUMENT", "content is required")


def _old_arg(args: dict[str, Any]) -> str:
    for key in ("old_string", "old_str"):
        if key in args:
            return str(args[key])
    raise ToolError("FS_INVALID_ARGUMENT", "old_string is required")


def _new_arg(args: dict[str, Any], *, required: bool) -> str:
    for key in ("new_string", "new_str"):
        if key in args:
            return str(args[key])
    if required:
        raise ToolError("FS_INVALID_ARGUMENT", "new_string is required")
    return ""


def _bool_arg(args: dict[str, Any], key: str) -> bool:
    value = args.get(key, False)
    if isinstance(value, bool):
        return value
    return str(value).strip().lower() in ("true", "1", "yes")


def _insert_line(args: dict[str, Any]) -> int:
    value = args.get("insert_line")
    if value is None:
        raise ToolError("FS_INVALID_ARGUMENT", "insert_line is required")
    if isinstance(value, bool) or not isinstance(value, int):
        raise ToolError("FS_INVALID_ARGUMENT", "insert_line must be an integer")
    return value


def _op_view(cwd: str, args: dict[str, Any], wt_uid: str, session: str) -> str:
    path = _resolve(cwd, args.get("path") or args.get("file_path"))
    if not path.exists():
        raise ToolError("FS_NOT_FOUND", f"The path {path} does not exist. Please provide a valid path.")
    if path.is_dir():
        if args.get("view_range") is not None:
            raise ToolError("FS_INVALID_ARGUMENT", "view_range is not allowed for directories")
        return _list_dir(path)
    text, _endings, _raw, _st = _read(path, wt_uid, session)
    return _format_view(path, text, args.get("view_range"))


def _op_create(cwd: str, args: dict[str, Any], wt_uid: str, session: str, stdin_text: str | None) -> str:
    path = _resolve(cwd, args.get("path") or args.get("file_path"))
    if path.exists():
        raise ToolError("FS_STALE_VERSION", f"File already exists at: {path}. Cannot overwrite files using create.")
    content = _content_arg(args, stdin_text)
    _atomic_write(path, content)
    raw = path.read_bytes()
    _remember(wt_uid, session, path, raw, path.stat())
    return f"New file created successfully at: {path}"


def _op_write(cwd: str, args: dict[str, Any], wt_uid: str, session: str, stdin_text: str | None) -> str:
    path = _resolve(cwd, args.get("path") or args.get("file_path"))
    content = _content_arg(args, stdin_text)
    exists = path.exists()
    if exists:
        if not path.is_file():
            raise ToolError("FS_NOT_REGULAR_FILE", f"cannot write {path}: not a regular file")
        _require_observed(wt_uid, session, path)
        mode = path.stat().st_mode
    else:
        mode = None
    _atomic_write(path, content, mode)
    raw = path.read_bytes()
    _remember(wt_uid, session, path, raw, path.stat())
    return "Updated file" if exists else "Created file"


def _op_replace(cwd: str, args: dict[str, Any], wt_uid: str, session: str, *, editor: bool) -> str:
    path = _resolve(cwd, args.get("path") or args.get("file_path"))
    old = _old_arg(args)
    new = _new_arg(args, required=False)
    if old == "":
        raise ToolError("FS_EDIT_NOT_FOUND", "old_string must be a non-empty string")
    if editor and old == new:
        raise ToolError("FS_INVALID_ARGUMENT", "old_string and new_string must differ")
    _require_observed(wt_uid, session, path)
    text, endings, raw, st = _read(path, wt_uid, session)
    count = text.count(old)
    if count == 0:
        raise ToolError("FS_EDIT_NOT_FOUND", f"old_string was not found in \"{path}\"")
    replace_all = _bool_arg(args, "replace_all")
    if not replace_all and count > 1:
        raise ToolError(
            "FS_AMBIGUOUS_EDIT",
            f"old_string matched {count} times in \"{path}\"; provide a more specific old_string or set replace_all to true",
        )
    updated = text.replace(old, new) if replace_all else text.replace(old, new, 1)
    _atomic_write(path, _restore(updated, endings), st.st_mode)
    new_raw = path.read_bytes()
    _remember(wt_uid, session, path, new_raw, path.stat())
    if editor:
        if replace_all:
            return f"The file {path} has been updated. All occurrences were successfully replaced."
        return f"The file {path} has been updated successfully."
    return f"The file {path} has been edited successfully."


def _op_insert(cwd: str, args: dict[str, Any], wt_uid: str, session: str) -> str:
    path = _resolve(cwd, args.get("path") or args.get("file_path"))
    line = _insert_line(args)
    value = _new_arg(args, required=True)
    _require_observed(wt_uid, session, path)
    text, endings, _raw, st = _read(path, wt_uid, session)
    lines = text.split("\n")
    if line < 0 or line > len(lines):
        raise ToolError("FS_INVALID_ARGUMENT", f"Invalid insert_line: {line}; expected 0..{len(lines)}")
    after = "\n".join([*lines[:line], *value.split("\n"), *lines[line:]])
    _atomic_write(path, _restore(after, endings), st.st_mode)
    new_raw = path.read_bytes()
    _remember(wt_uid, session, path, new_raw, path.stat())
    return f"The file {path} has been edited successfully."


def run(
    *, op: str, args: dict[str, Any], cwd: str, wt_uid: str, session: str = "",
    stdin_text: str | None = None,
) -> str:
    handlers = {
        "view": lambda: _op_view(cwd, args, wt_uid, session),
        "create": lambda: _op_create(cwd, args, wt_uid, session, stdin_text),
        "write": lambda: _op_write(cwd, args, wt_uid, session, stdin_text),
        "edit": lambda: _op_replace(cwd, args, wt_uid, session, editor=True),
        "str_replace": lambda: _op_replace(cwd, args, wt_uid, session, editor=False),
        "insert": lambda: _op_insert(cwd, args, wt_uid, session),
    }
    handler = handlers.get(str(op))
    if handler is None:
        raise ToolError("FS_INVALID_ARGUMENT", f"unknown edit op: {op}")
    return handler()
