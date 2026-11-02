"""VFS document hash: single file content SHA-256 or folder tree hash.

Command name is `hash`; the Python symbol is `hash_state` to avoid shadowing
the Python builtin `hash`.
"""

import hashlib
import json


def _is_system_path(path: str) -> bool:
    return path == "/.mds" or path.startswith("/.mds/")


def _is_glob(target: str) -> bool:
    return any(ch in target for ch in "*?[")


def _tree_hash(rows) -> str:
    """SHA-256 over sorted ``path:file_hash`` entries joined by NUL."""
    body = "\0".join(f"{path}:{file_hash}" for path, file_hash in sorted(rows))
    return hashlib.sha256(body.encode("utf-8")).hexdigest()


def _block(path: str, kind: str, digest: str) -> str:
    return f"## {path}\n- kind: {kind}\n- hash: {digest}"


def _info(message: str) -> str:
    return f"- mdio_info: [hash] {message}"


def _path_text(value) -> str:
    """Normalize both plain strings and parser path-token lists to a path."""
    if isinstance(value, str):
        return value
    if isinstance(value, list):
        parts = []
        for token in value:
            if isinstance(token, tuple) and len(token) >= 2:
                parts.append(str(token[1]))
            elif isinstance(token, str):
                parts.append(token)
            else:
                parts.append(str(token))
        return "".join(parts)
    return str(value)


async def hash_state(workspace, pipe_data, args, kwargs, ctx):
    """Hash one markdown file or a folder's visible markdown tree.

    Syntax: hash [path] [-l] [-a]
      - path omitted: current worktree root
      - -l: one h2 block per file instead of one tree-hash block
      - -a: include hidden documents (/.mds is always excluded)
    """
    flags = set()
    for key in kwargs:
        if key.startswith("-") and not key.startswith("--"):
            flags.update(key[1:])

    list_mode = "l" in flags
    show_all = "a" in flags

    raw = kwargs.get("-p")
    if raw is None:
        for arg in args:
            text = _path_text(arg).strip()
            if text and not text.startswith("-"):
                raw = text
                break
    if raw is None:
        for key, value in kwargs.items():
            if key == "-p" or not key.startswith("-"):
                continue
            if isinstance(value, str):
                text = value.strip()
                if text and not text.startswith("-"):
                    raw = text
                    break
    if raw is None:
        if pipe_data is not None:
            return "- mdio_error: [hash] missing path"
        raw = "/"

    target = ctx.resolve_path(str(raw).strip())
    target = target.rstrip("/") or "/"

    if _is_system_path(target):
        return _info("system path excluded: /.mds")

    try:
        tree_bytes = workspace.store.get_vfs_tree(ctx.wt_uid, show_all)
        entries = json.loads(tree_bytes.decode("utf-8")) if tree_bytes else []
    except Exception as exc:
        return f"- mdio_error: [hash] target worktree access failed: {exc}"

    hash_of = {
        entry["path"]: entry["hash"]
        for entry in entries
        if isinstance(entry, dict) and entry.get("path") and entry.get("hash")
    }

    # Exact file target: visible md file.
    if target in hash_of:
        if not target.endswith(".md"):
            return _info(f"not a markdown doc: {target}")
        digest = hash_of[target]
        return _block(target, "file", digest)

    # Directory / glob / missing target.
    if _is_glob(target):
        pattern = target
    elif target == "/":
        pattern = "/**/*.md"
    else:
        pattern = f"{target}/**/*.md"

    try:
        paths = workspace.store.expand_glob(ctx.wt_uid, pattern, show_all)
    except Exception as exc:
        return f"- mdio_error: [hash] glob failed: {exc}"

    rows = sorted(
        (path, hash_of[path])
        for path in set(paths)
        if path.endswith(".md") and path in hash_of and not _is_system_path(path)
    )

    if list_mode:
        if not rows:
            return _info(f"no matches: {target}")
        return "\n\n".join(_block(path, "file", digest) for path, digest in rows)

    if _is_glob(target):
        if not rows:
            return _info(f"no matches: {target}")
        return _block(target, "tree", _tree_hash(rows))

    # Non-glob target: rows imply a directory-like target (including "/").
    if rows:
        return _block(target, "dir", _tree_hash(rows))

    # A missing non-glob target that is not a visible file: distinguish an
    # existing empty directory (empty tree hash) from a miss. A hidden md file
    # is visible to path_exists but invisible without -a, so it must not be
    # mistaken for a directory.
    try:
        target_exists = bool(workspace.store.path_exists(ctx.wt_uid, target))
    except Exception:
        target_exists = False

    if target_exists:
        if target.endswith(".md"):
            return _info(f"no matches: {target}")
        try:
            exact = workspace.store.expand_glob(ctx.wt_uid, target, show_all)
        except Exception:
            exact = []
        if target in exact:
            return _info(f"not a markdown doc: {target}")
        return _block(target, "dir", _tree_hash(rows))

    return _info(f"no matches: {target}")
