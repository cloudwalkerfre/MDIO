"""
WORKTREE & SNAP — worktree + snap orchestration + version engine.

Single source of truth for all version-management logic:
  - snap engine: create, log, diff, status, show, restore, del, verify, gc, checkout
  - worktree orchestration: bfork, cfork, use, current, list, commit, undo, rm
  - 命名统一：**wt 级 = load/offload**（本地项目）· **folder 级 = mount/unmount**（iroh 远程共享）
  - backward compat: snap() → thin wrapper, prints deprecation
"""

import os
import re
import json
import hashlib
from datetime import datetime, timezone
from typing import List, Dict, Any, Optional, Tuple, NamedTuple

from mdio_py import mdio_core as _mrc
from mdio_py.core.ast_utils import bullets_to_kv
from mdio_py.core.error import MdioError, ErrCode
from mdio_py.core.selector_builder import SelectorBuilder, by_tag, drill

# Constants
SNAP_MD = "/.mds/snap.md"
CKPT_DIR = "/.mds/ckpt"
GENESIS_WT = "genesis"
DEFAULT_BRANCH = "main"
DEFAULT_AUTHOR = "mdio"
WORKTREES_MD = "/.mds/worktrees.md"

# Hash helpers — single source of truth: Rust snap.rs

def state_hash(content: str) -> str:
    return _mrc.snap_state_hash(content)

def snap_hash(parent: str, author: str, message: str, at_str: str,
              file_states: str) -> str:
    """SHA256 via Rust snap::snap_hash. | in author/message are escaped."""
    return _mrc.snap_hash_ffi(parent, author, message, at_str, file_states)

def build_file_states(notes: Dict[str, str]) -> str:
    """Sorted `path:hash\\0path:hash` (SHA256 per path)."""
    pairs = [f"{p}:{notes[p]}" for p in sorted(notes.keys())]
    return "\0".join(pairs)

def _short_hash(h: str, n: int = 8) -> str:
    return h[:n]

def _utc_now_iso() -> str:
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

def _author() -> str:
    return os.environ.get("MDIO_AUTHOR", os.environ.get("USER", DEFAULT_AUTHOR))

# Sub-mount boundary

class BindingInfo(NamedTuple):
    """FFI bindings_info 返回值的具名包装。防止位置索引错位（B5）。"""
    my_path: str       # "/home", "/data"
    target_path: str   # "/docs", "/a1"
    mode: str          # "RW" or "RO"


def _bindings_info_typed(workspace, wt_uid: str) -> list[BindingInfo]:
    """FFI bindings_info → list[BindingInfo]。"""
    raw = workspace.store.bindings_info(wt_uid)
    return [BindingInfo(my_path=a, target_path=b, mode=c) for a, b, c in raw]


def _filter_owned_files(workspace, wt_uid: str, files: List[str]) -> List[str]:
    """Exclude RO binding files from snap operations."""
    info = _bindings_info_typed(workspace, wt_uid)
    ro_prefixes = {b.my_path + "/" for b in info if b.mode == "RO"}
    if not ro_prefixes:
        return files
    return [f for f in files if not any(f.startswith(p) for p in ro_prefixes)]

# VFS I/O

def _read_full_text(workspace, wt_uid: str, vfs_path: str) -> str:
    """全文读：唯一实现 = workspace.read_full_text（Q16：本地薄壳）。"""
    return workspace.read_full_text(wt_uid, vfs_path)

def _write_full_text(workspace, wt_uid: str, vfs_path: str, content: str,
                     *, suppress_hooks: bool = True) -> None:
    ir = SelectorBuilder().build()
    workspace.write_locator(
        wt_uid, vfs_path, ir, "/>", content,
        auto_scaffold=True, god=True, suppress_hooks=suppress_hooks,
    )

def _append_to_file(workspace, wt_uid: str, vfs_path: str, content: str,
                    *, suppress_hooks: bool = True) -> None:
    ir = SelectorBuilder().build()
    workspace.write_locator(
        wt_uid, vfs_path, ir, "+>", content,
        auto_scaffold=True, god=True, suppress_hooks=suppress_hooks,
    )

# snap.md parser — delegated to Rust snap::parse_all_snaps

def _parse_snap_entries(text: str) -> List[Dict[str, Any]]:
    entries_json, _ = _mrc.snap_parse_all(text)
    return json.loads(entries_json)

def _parse_branch_heads(text: str) -> Dict[str, str]:
    _, heads_json = _mrc.snap_parse_all(text)
    return json.loads(heads_json)

# snap ID resolution

def _find_entry(entries: List[Dict[str, Any]], snap_id: str) -> Optional[Dict[str, Any]]:
    exact = [e for e in entries if e["hash"] == snap_id]
    if exact:
        return exact[0]
    prefix_matches = [e for e in entries if e["hash"].startswith(snap_id)]
    if len(prefix_matches) == 1:
        return prefix_matches[0]
    if len(prefix_matches) > 1:
        raise MdioError(
            "snap", ErrCode.SNAP_NO_MATCH,
            f"Ambiguous prefix '{snap_id}' — matches {len(prefix_matches)} snaps: "
            + ", ".join(e["hash"][:12] for e in prefix_matches[:10]),
        )
    return None

def _get_branch_head(workspace, wt_uid: str, branch: str = DEFAULT_BRANCH) -> Optional[str]:
    text = _read_full_text(workspace, wt_uid, SNAP_MD)
    if not text or text.strip() == "# TIME":
        return None
    return _parse_branch_heads(text).get(branch)

# DAG traversal（跨分支 diff）

def _find_common_ancestor(entries: List[Dict[str, Any]],
                          id_a: str, id_b: str) -> Optional[str]:
    """Walk parent chains of two snap hashes, find first intersection.
    Each snap has a single parent — this is a tree, O(depth)."""
    index = {e["hash"]: e for e in entries}
    ancestors: Dict[str, int] = {}
    cur = id_a
    while cur and cur in index:
        ancestors[cur] = len(ancestors)
        cur = index[cur].get("parent", "")
    cur = id_b
    while cur and cur in index:
        if cur in ancestors:
            return cur
        cur = index[cur].get("parent", "")
    return None

# Ckpt I/O（- path: / - at: / - content: | metadata）
MAX_DELTA_DEPTH = 20

def _format_ckpt_entry(state_hash_str: str, vfs_path: str, at: str,
                       content: str, delta_from: str = "",
                       patch: str = "") -> str:
    """Format a ckpt section.
    If delta_from/patch are provided, writes delta format instead of full content."""
    lines = [f"## {state_hash_str}"]
    lines.append(f"- path: {vfs_path}")
    lines.append(f"- at: {at}")
    if delta_from and patch:
        lines.append(f"- delta_from: {delta_from}")
        lines.append("- patch: |")
        for segment in patch.split('\n'):
            lines.append(f"    {segment}")
    else:
        lines.append("- content: |")
        for segment in content.split('\n'):
            lines.append(f"    {segment}")
    return "\n".join(lines) + "\n"

def _parse_ckpt_content(ckpt_text: str, state_hash_str: str) -> Optional[str]:
    """Parse ckpt content via Rust snap::parse_ckpt_entry.
    For delta entries, returns placeholder — caller must use _reconstruct_ckpt."""
    full = _mrc.snap_parse_ckpt_full(ckpt_text, state_hash_str)
    if full is None:
        return None
    parts = json.loads(full)
    kind = parts[0]
    if kind == "full":
        return parts[3]  # content
    elif kind == "delta":
        return None  # signal "need reconstruction"
    return None

def _parse_ckpt_entry_info(ckpt_text: str, state_hash_str: str) -> Optional[dict]:
    """Parse a ckpt entry and return full info including delta metadata."""
    full = _mrc.snap_parse_ckpt_full(ckpt_text, state_hash_str)
    if full is None:
        return None
    parts = json.loads(full)
    return {"kind": parts[0], "path": parts[1], "at": parts[2],
            "data": parts[3], "patch": parts[4] if len(parts) > 4 else ""}

def _reconstruct_ckpt(workspace, wt_uid: str, state_hash_str: str,
                      at_timestamp: str = "", depth: int = MAX_DELTA_DEPTH,
                      visited: Optional[set] = None) -> Optional[str]:
    """Reconstruct file content from a ckpt entry, following delta chains.
    Returns None if entry not found or chain broken.
    Verifies state_hash at each reconstruction step."""
    if visited is None:
        visited = set()
    if state_hash_str in visited:
        return None  # cycle detected
    if depth <= 0:
        return None  # chain too deep
    visited.add(state_hash_str)

    ckpt_text = _find_ckpt_text(workspace, wt_uid, state_hash_str, at_timestamp)
    if not ckpt_text:
        return None

    info = _parse_ckpt_entry_info(ckpt_text, state_hash_str)
    if not info:
        return None

    if info["kind"] == "full":
        return info["data"]  # content

    if info["kind"] == "delta":
        parent_hash = info["data"]  # delta_from
        patch = info["patch"]
        parent_content = _reconstruct_ckpt(
            workspace, wt_uid, parent_hash, at_timestamp, depth - 1, visited
        )
        if parent_content is None:
            return None
        reconstructed = _mrc.snap_apply_ffi(parent_content, patch)
        if reconstructed is None:
            return None
        actual_hash = state_hash(reconstructed)
        if actual_hash != state_hash_str:
            return None
        return reconstructed

    return None

# Simple cache: (wt_uid, state_hash) → (month, ckpt_text) to avoid repeated file scans.
# 键必须带 wt 维度：同一 state_hash 在不同 wt 的 ckpt 文本不同，且 snap --gc
# 只清理自己的 wt。缓存自身不感知磁盘变化 ⇒ gc 删除条目后必须显式失效（见 gc）。
_ckpt_text_cache: Dict[tuple, tuple] = {}

def _find_ckpt_text(workspace, wt_uid: str, state_hash_str: str,
                    at_timestamp: str = "") -> Optional[str]:
    """Find the ckpt file and text for a given state_hash.
    Results are cached per (wt_uid, state_hash) for the lifetime of the process."""
    cache_key = (wt_uid, state_hash_str)
    if cache_key in _ckpt_text_cache:
        _month, text = _ckpt_text_cache[cache_key]
        return text

    months: List[str] = []
    if at_timestamp:
        try:
            ts = at_timestamp.replace("Z", "+00:00")
            dt = datetime.fromisoformat(ts)
            mk = dt.strftime("%Y-%m")
            if mk not in months:
                months.append(mk)
        except (ValueError, TypeError):
            pass
    now = datetime.now(timezone.utc)
    for offset in range(13):
        month = now.month - offset
        year = now.year
        while month < 1:
            month += 12
            year -= 1
        mk = f"{year}-{month:02d}"
        if mk not in months:
            months.append(mk)
    for mk in months:
        path = f"{CKPT_DIR}/{mk}.md"
        ckpt_text = _read_full_text(workspace, wt_uid, path)
        if ckpt_text and f"## {state_hash_str}\n" in ckpt_text:
            _ckpt_text_cache[cache_key] = (mk, ckpt_text)
            return ckpt_text
    return None

def _read_ckpt_for_hash(workspace, wt_uid: str, state_hash_str: str,
                        at_timestamp: str = "") -> Optional[str]:
    """Read ckpt content, following delta chains automatically.
    Tries month from at_timestamp first, then scans up to 12 months backward."""
    return _reconstruct_ckpt(workspace, wt_uid, state_hash_str, at_timestamp)

def _ckpt_already_stored(workspace, wt_uid: str, state_hash_str: str) -> bool:
    return _find_ckpt_text(workspace, wt_uid, state_hash_str) is not None

def _delta_chain_depth(workspace, wt_uid: str, state_hash_str: str,
                        max_depth: int = MAX_DELTA_DEPTH) -> int:
    """Compute delta chain depth for a given state_hash.
    Returns 0 for base entries, depth for deltas."""
    depth = 0
    cur = state_hash_str
    visited: set = set()
    while depth < max_depth:
        ckpt_text = _find_ckpt_text(workspace, wt_uid, cur)
        if not ckpt_text:
            return depth
        info = _parse_ckpt_entry_info(ckpt_text, cur)
        if not info:
            return depth
        if info["kind"] == "full":
            return depth
        if info["kind"] == "delta":
            parent = info["data"]  # delta_from
            if parent in visited:
                return depth
            visited.add(cur)
            cur = parent
            depth += 1
        else:
            return depth
    return depth

def _append_ckpt_entry(workspace, wt_uid: str, state_hash_str: str,
                       vfs_path: str, content: str, at_timestamp: str,
                       parent_hash: str = "", is_tagged: bool = False) -> str:
    """Append a ckpt section.  Automatically decides base vs delta storage.
    Returns the storage kind used ("base" or "delta")."""
    if _ckpt_already_stored(workspace, wt_uid, state_hash_str):
        return "skip"

    month = at_timestamp[:7] if len(at_timestamp) >= 7 \
        else datetime.now(timezone.utc).strftime("%Y-%m")
    ckpt_file = f"{CKPT_DIR}/{month}.md"
    existing = _read_full_text(workspace, wt_uid, ckpt_file)

    delta_from = ""
    patch = ""
    stored_kind = "base"

    if parent_hash:
        parent_content = _read_ckpt_for_hash(workspace, wt_uid, parent_hash)
        if parent_content is not None:
            diff_text = _mrc.snap_diff_ffi(parent_content, content)
            chain_depth = _delta_chain_depth(workspace, wt_uid, parent_hash)

            # delta 存储属阶段二功能（尚未启用）：这里只保留决策所需的计算；
            # 父内容缺失时不再引用未绑定的 chain_depth/diff_text（原崩溃点），
            # 一律落 base。见 BUGS 复盘 A2。
            if is_tagged:
                pass  # stay base
            elif len(content) > 0 and len(diff_text) > len(content) * 3 / 4:
                pass  # delta not worth it, stay base

    entry = _format_ckpt_entry(state_hash_str, vfs_path, at_timestamp,
                                content, delta_from, patch)

    if existing:
        _append_to_file(workspace, wt_uid, ckpt_file, f"\n{entry}")
    else:
        _write_full_text(workspace, wt_uid, ckpt_file, f"# CHECKPOINT\n\n{entry}")

    if stored_kind == "delta":
        reconstructed = _read_ckpt_for_hash(workspace, wt_uid, state_hash_str, at_timestamp)
        if reconstructed is None or state_hash(reconstructed) != state_hash_str:
            _write_full_text(workspace, wt_uid, ckpt_file,
                            f"# CHECKPOINT\n\n{existing or ''}\n"
                            f"{_format_ckpt_entry(state_hash_str, vfs_path, at_timestamp, content)}")
            stored_kind = "base"

    return stored_kind

# snap helper：每个 WT 在自身 VFS 视图内独立快照

def _snap_expand_wt(workspace, wt_uid: str):
    """Each WT snaps independently within its own VFS view."""
    return wt_uid, None

# snap create

async def _snap_create(workspace, pipe_data, args, kwargs, ctx) -> str:
    message: Optional[str] = None
    target_pattern: Optional[str] = None

    for k in ("-m", "--message", "--msg"):
        if k in kwargs and kwargs[k] is not None:
            message = str(kwargs[k])
            break

    pos_args = [str(a).strip() for a in args
                if isinstance(a, str) and a.strip() and not str(a).startswith("-")]
    if message is None and pos_args:
        message = pos_args[0]
    if message is None and pipe_data:
        message = str(pipe_data).strip()

    for k in ("-p", "--path"):
        if k in kwargs and kwargs[k] is not None:
            target_pattern = str(kwargs[k])
            break
    if target_pattern is None:
        for a in pos_args:
            if a != message and ("/" in a or a.endswith(".md") or "*" in a):
                target_pattern = a
                break

    if not message:
        return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] snap requires a message"
    if not target_pattern:
        target_pattern = "/**/*.md"

    branch = str(kwargs.get("--branch", kwargs.get("-b", ctx.worktree_name or DEFAULT_BRANCH))).strip() or ctx.worktree_name or DEFAULT_BRANCH
    tag = str(kwargs.get("--tag", kwargs.get("-t", ""))).strip()
    explicit_parent = str(kwargs.get("--parent", "")).strip() or None
    author_cfg = _author()
    # Use active_worktree_uid as primary source.
    # ctx.wt_uid may be genesis even when user is operating on a cfork/bfork
    # whose wt_uid is a different uid string (unlike int 0 which was shared).
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    if wt_uid == "genesis" and ctx.wt_uid != "genesis":
        wt_uid = ctx.wt_uid
    expand_wt, scope_targets = _snap_expand_wt(workspace, wt_uid)

    # -- Expand glob --
    try:
        abs_pattern = ctx.resolve_path(target_pattern) if target_pattern.startswith("/") \
            else ctx.resolve_path("/" + target_pattern)
    except Exception:
        abs_pattern = target_pattern
    try:
        files = workspace.store.expand_glob(expand_wt, abs_pattern, False)
    except Exception as e:
        return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] glob expansion failed: {e}"
    files = sorted(set(f for f in files if f.endswith(".md")))
    files = _filter_owned_files(workspace, wt_uid, files)
    if not files:
        return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] no files matched pattern: {target_pattern}"

    # -- Resolve parent (--parent validation → S002) --
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    snap_md_entries = _parse_snap_entries(snap_md_text) if snap_md_text else []

    if explicit_parent:
        if not _find_entry(snap_md_entries, explicit_parent):
            return f"- mdio_error: [{ErrCode.SNAP_NO_PARENT}] parent {_short_hash(explicit_parent, 12)}... not found"
        parent = explicit_parent
    else:
        parent = _get_branch_head(workspace, wt_uid, branch)
        if not parent:
            parent = _get_branch_head(workspace, wt_uid, branch)

    parent_entry = _find_entry(snap_md_entries, parent) if parent else None
    parent_notes = parent_entry.get("notes", {}) if parent_entry else {}

    # -- Hash files, detect changes --
    notes: Dict[str, str] = {}
    changed_files: List[Tuple[str, str, str]] = []
    skipped_info: List[str] = []

    for fp in files:
        content = _read_full_text(workspace, wt_uid, fp)
        sh = state_hash(content)
        notes[fp] = sh
        if parent and parent_notes.get(fp) == sh:
            skipped_info.append(f"- mdio_info: {fp} unchanged (skipped)")
            continue
        changed_files.append((fp, sh, content))

    if parent and not changed_files:
        result = f"- mdio_info: no changes since parent {_short_hash(parent)}, snap skipped"
        if skipped_info:
            result = "\n".join(skipped_info) + "\n" + result
        return result

    # -- Re-read HEAD for concurrency safety (before computing hash) --
    at_str = _utc_now_iso()
    if not explicit_parent:
        snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
        parent = _get_branch_head(workspace, wt_uid, branch) or ""

    # -- Compute snap hash (AFTER re-reading parent for TOCTOU safety) --
    fs_str = build_file_states(notes)
    new_hash = snap_hash(parent or "", author_cfg, message, at_str, fs_str)

    # -- Write ckpt entries (with delta when beneficial) --
    is_tagged = bool(tag)
    for fp, sh, content in changed_files:
        try:
            ph = parent_notes.get(fp) if parent_notes else ""
            _append_ckpt_entry(workspace, wt_uid, sh, fp, content, at_str,
                              parent_hash=ph, is_tagged=is_tagged)
        except Exception as e:
            return (f"- mdio_error: [{ErrCode.SNAP_CKPT_ERR}] "
                    f"failed to write ckpt for {fp}: {e}")

    # -- Build snap entry --
    entry_lines = [f"### {new_hash}"]
    if parent:
        entry_lines.append(f"- parent: {parent}")
    entry_lines.append(f"- author: {author_cfg}")
    entry_lines.append(f"- message: {message}")
    entry_lines.append(f"- at: {at_str}")
    if tag:
        entry_lines.append(f"- tag: {tag}")
    entry_lines.append("#### notes")
    for fp in sorted(notes.keys()):
        entry_lines.append(f"- {fp}: {notes[fp]}")
    snap_entry_text = "\n".join(entry_lines) + "\n"

    # -- Update snap.md via MDIO selectors --
    try:
        existing = _read_full_text(workspace, wt_uid, SNAP_MD)
        md_path = f"TIME.{branch}"
        if not existing or existing.strip() == "# TIME":
            _write_full_text(workspace, wt_uid, SNAP_MD,
                            f"# TIME\n\n## {branch}\n- HEAD: {new_hash}\n")
        elif f"## {branch}" not in existing:
            branch_block = f"\n## {branch}\n- HEAD: {new_hash}\n"
            workspace.write_locator(wt_uid, SNAP_MD,
                SelectorBuilder().build(),
                "+>", branch_block,
                auto_scaffold=True, god=True, suppress_hooks=True)
        else:
            workspace.write_locator(wt_uid, SNAP_MD,
                by_tag(md_path, "HEAD"),
                "/>", new_hash,
                auto_scaffold=True, god=True, suppress_hooks=True)
        workspace.write_locator(wt_uid, SNAP_MD,
            SelectorBuilder().build(),
            "+>", snap_entry_text,
            auto_scaffold=True, god=True, suppress_hooks=True)
    except Exception as e:
        return f"- mdio_error: [{ErrCode.SNAP_CKPT_ERR}] failed to update snap.md: {e}"

    # -- Return --
    result_lines = list(skipped_info)
    n = len(notes)
    if n == 1:
        result_lines.append(f"snap: {new_hash}")
    else:
        result_lines.append(f"snap: {new_hash} ({n} files)")
    return "\n".join(result_lines)


# snap --list

async def _snap_list(workspace, pipe_data, args, kwargs, ctx) -> str:
    target = kwargs.get("-p")
    if target is None:
        for a in args:
            s = str(a).strip()
            if s and not s.startswith("-"):
                target = s
                break
    if target is None and pipe_data:
        target = str(pipe_data).strip()
    if not target:
        wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
        snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
        entries = _parse_snap_entries(snap_md_text)
        if not entries: return f"- mdio_info: [snap] no snapshots found"
        lines = []
        for entry in entries:
            tag_str = f" [{entry['tag']}]" if entry.get("tag") else ""
            n = len(entry.get("notes", {}))
            lines.append(
                f"- {_short_hash(entry['hash'], 12)}  {entry.get('at', '')[:16]}  "
                f"{entry.get('message', '')}{tag_str}  ({n}f)")
        return "\n".join(lines)
    try:
        abs_target = ctx.resolve_path(str(target))
    except Exception:
        abs_target = str(target)
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)
    matching: List[str] = []
    for entry in entries:
        if abs_target in entry.get("notes", {}):
            tag_str = f" [{entry['tag']}]" if entry.get("tag") else ""
            matching.append(
                f"- {_short_hash(entry['hash'], 12)}  {entry.get('at', '')[:16]}  "
                f"{entry.get('message', '')}{tag_str}"
            )
    if not matching:
        return f"- mdio_info: [snap] no snaps found for '{abs_target}'"
    return "\n".join(matching)

# snap --show

async def _snap_show(workspace, pipe_data, args, kwargs, ctx) -> str:
    snap_id = None
    for a in args:
        s = str(a).strip()
        if s and not s.startswith("-"):
            snap_id = s
            break
    if snap_id is None and pipe_data:
        snap_id = str(pipe_data).strip()
    if not snap_id:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap --show requires a snap ID"
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)
    try:
        entry = _find_entry(entries, snap_id)
    except MdioError:
        raise
    if not entry:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap '{snap_id[:12]}...' not found"
    lines = [
        f"snap: {entry['hash']}",
        f"- parent: {entry.get('parent') or '(root)'}",
        f"- author: {entry.get('author', '-')}",
        f"- message: {entry.get('message', '-')}",
        f"- at: {entry.get('at', '-')}",
    ]
    tag = entry.get("tag", "")
    if tag:
        lines.append(f"- tag: {tag}")
    notes = entry.get("notes", {})
    if notes:
        lines.append(f"- notes: {len(notes)} files")
        for fp, sh in sorted(notes.items()):
            lines.append(f"  - {fp}: {sh}")
    return "\n".join(lines)

# snap --diff

async def _snap_diff(workspace, pipe_data, args, kwargs, ctx) -> str:
    """no args: diff current files vs HEAD. 1 arg: that snap vs parent. 2 args: two snaps."""
    ids: List[str] = []
    for a in args:
        s = str(a).strip()
        if s and not s.startswith("-"):
            ids.append(s)

    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)
    heads = _parse_branch_heads(snap_md_text)

    if not ids:
        a_val = kwargs.get("-a"); b_val = kwargs.get("-b")
        if a_val and not isinstance(a_val, bool): ids.append(str(a_val))
        if b_val and not isinstance(b_val, bool): ids.append(str(b_val))

    if len(ids) == 0:
        head = heads.get(ctx.worktree_name or DEFAULT_BRANCH)
        if not head: return f"- mdio_info: [snap] no snap history to diff"
        head_entry = _find_entry(entries, head) if entries else None
        if not head_entry: return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] HEAD snap not found"
        return await _diff_current_vs_snap(workspace, ctx, head_entry)

    if len(ids) == 1:
        id_a = ids[0]
        entry = _find_entry(entries, id_a) if entries else None
        if not entry:
            return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap '{id_a[:12]}' not found"
        parent = entry.get("parent", "")
        if not parent:
            return await _diff_current_vs_snap(workspace, ctx, entry)
        id_b = id_a; id_a = parent
    else:
        id_a, id_b = ids[0], ids[1]

    try:
        entry_a = _find_entry(entries, id_a)
        entry_b = _find_entry(entries, id_b)
    except MdioError:
        raise
    if not entry_a:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap '{id_a[:12]}' not found"
    if not entry_b:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap '{id_b[:12]}' not found"

    lca = _find_common_ancestor(entries, entry_a["hash"], entry_b["hash"])
    if lca is None:
        return (f"- mdio_error: [{ErrCode.SNAP_NO_ANCESTOR}] "
                f"no common ancestor between {_short_hash(id_a)} and {_short_hash(id_b)}")

    notes_a = entry_a.get("notes", {})
    notes_b = entry_b.get("notes", {})
    all_files = sorted(set(notes_a.keys()) & set(notes_b.keys()))

    if not all_files:
        only_a = set(notes_a.keys()) - set(notes_b.keys())
        only_b = set(notes_b.keys()) - set(notes_a.keys())
        lines: List[str] = []
        for fp in sorted(only_a):
            lines.append(f"# {fp} (removed)")
        for fp in sorted(only_b):
            lines.append(f"# {fp} (added)")
        if not lines:
            return (f"- mdio_info: [snap] no differences between "
                    f"{_short_hash(id_a)} and {_short_hash(id_b)}")
        return "\n".join(lines)

    output: List[str] = []
    for fp in all_files:
        sh_a = notes_a.get(fp)
        sh_b = notes_b.get(fp)
        if sh_a == sh_b:
            continue
        content_a = _read_ckpt_for_hash(workspace, wt_uid, sh_a,
                                        entry_a.get("at", "")) if sh_a else ""
        content_b = _read_ckpt_for_hash(workspace, wt_uid, sh_b,
                                        entry_b.get("at", "")) if sh_b else ""
        if content_a == content_b:
            continue
        output.append(f"# {fp}")
        output.append("```diff")
        diff_text = _mrc.snap_diff_ffi(content_a or "", content_b or "")
        if diff_text:
            output.append(diff_text.rstrip("\n"))
        output.append("```")

    if not output:
        return (f"- mdio_info: [snap] no differences between "
                f"{_short_hash(id_a)} and {_short_hash(id_b)}")
    return "\n\n".join(output)

async def _diff_current_vs_snap(workspace, ctx, snap_entry) -> str:
    """Diff current working files against a snap's content."""
    notes = snap_entry.get("notes", {})
    if not notes:
        return f"- mdio_info: [snap] snap has no files to diff"
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    expand_wt, _ = _snap_expand_wt(workspace, wt_uid)
    output: List[str] = []
    for fp in sorted(notes.keys()):
        sh = notes[fp]
        current = _read_full_text(workspace, wt_uid, fp)
        snap_content = _read_ckpt_for_hash(workspace, wt_uid, sh, snap_entry.get("at", ""))
        if snap_content is None: snap_content = ""
        if current == snap_content: continue
        output.append(f"# {fp}")
        output.append("```diff")
        diff_text = _mrc.snap_diff_ffi(snap_content or "", current or "")
        if diff_text: output.append(diff_text.rstrip("\n"))
        output.append("```")
    if not output: return f"- mdio_info: [snap] no differences from HEAD"
    return "\n\n".join(output)


async def _snap_status(workspace, pipe_data, args, kwargs, ctx) -> str:
    """Show what changed since last snap (HEAD)."""
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    expand_wt, _ = _snap_expand_wt(workspace, wt_uid)
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    heads = _parse_branch_heads(snap_md_text)
    head = heads.get(ctx.worktree_name or DEFAULT_BRANCH)
    if not head: return f"- mdio_info: [snap] no snap history yet"
    entries = _parse_snap_entries(snap_md_text)
    head_entry = _find_entry(entries, head)
    if not head_entry: return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] HEAD snap not found"
    head_notes = head_entry.get("notes", {})
    all_paths = set(head_notes.keys())
    try:
        current_files = _filter_owned_files(workspace, wt_uid,
            workspace.store.expand_glob(expand_wt, "/**/*.md", False))
        all_paths.update(current_files)
    except Exception: pass

    lines = [f"- snap: HEAD {_short_hash(head)}"]
    changed, unchanged, added, deleted = [], [], [], []
    for fp in sorted(all_paths):
        cur = _read_full_text(workspace, expand_wt, fp)
        cur_sh = state_hash(cur) if cur else ""
        head_sh = head_notes.get(fp)
        if not cur and head_sh: deleted.append(fp)
        elif not head_sh and cur: added.append(fp)
        elif head_sh != cur_sh: changed.append(fp)
        else: unchanged.append(fp)

    if changed: lines.append(f"- changed: {len(changed)} file(s)");
    for f in changed: lines.append(f"  M {f}")
    if added: lines.append(f"- added: {len(added)} file(s)")
    for f in added: lines.append(f"  A {f}")
    if deleted: lines.append(f"- deleted: {len(deleted)} file(s)")
    for f in deleted: lines.append(f"  D {f}")
    if not changed and not added and not deleted:
        lines.append("- (no changes since last snap)")
    return "\n".join(lines)


async def _snap_log(workspace, pipe_data, args, kwargs, ctx) -> str:
    """Show snap chain from HEAD to root."""
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)
    heads = _parse_branch_heads(snap_md_text)
    head = heads.get(ctx.worktree_name or DEFAULT_BRANCH)
    if not head: return f"- mdio_info: [snap] no snap history yet"
    lines = []
    cur = head
    while cur:
        entry = _find_entry(entries, cur)
        if not entry: break
        tag_str = f" [{entry.get('tag', '')}]" if entry.get("tag") else ""
        n = len(entry.get("notes", {}))
        lines.append(f"- {entry['hash'][:12]}  {entry.get('at','')[:16]}  {entry.get('message','')}{tag_str}  ({n}f)")
        cur = entry.get("parent", "")
        if cur: lines[-1] += f"  ← {cur[:8]}"
    return "\n".join(lines) if lines else f"- mdio_info: [snap] empty log"


async def _snap_undo(workspace, pipe_data, args, kwargs, ctx) -> str:
    """Delete HEAD snap. Requires -f flag for confirmation."""
    branch = str(kwargs.get("--branch", kwargs.get("-b", ctx.worktree_name or DEFAULT_BRANCH))).strip() or ctx.worktree_name or DEFAULT_BRANCH
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)
    heads = _parse_branch_heads(snap_md_text)
    head = heads.get(branch)
    if not head: return f"- mdio_info: [snap] nothing to undo on branch '{branch}'"
    force = "-f" in kwargs
    if not force:
        head_entry = _find_entry(entries, head) if entries else None
        msg = head_entry.get("message", "") if head_entry else ""
        return (f"- mdio_info: [snap] will undo HEAD {_short_hash(head)} (\"{msg}\") "
                f"on branch '{branch}'. Use -f to confirm.")
    return await _delete_one_snap(workspace, wt_uid, snap_md_text, entries, head, force)


# snap --restore

async def _snap_restore(workspace, pipe_data, args, kwargs, ctx) -> str:
    snap_id: Optional[str] = None
    target_path: Optional[str] = None
    for a in args:
        s = str(a).strip()
        if not s or s.startswith("-"):
            continue
        if snap_id is None:
            snap_id = s
        elif target_path is None:
            target_path = s
    if not snap_id:
        rid = kwargs.get("--restore") or kwargs.get("-r")
        if rid and not isinstance(rid, bool):
            snap_id = str(rid)
    if not snap_id:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap --restore requires a snap ID"
    if not target_path:
        target_path = kwargs.get("-p") or kwargs.get("--path")
        if target_path and not isinstance(target_path, str):
            target_path = None
    restore_all = not target_path

    hook_trigger = "-hook_trigger" in kwargs

    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)
    try:
        entry = _find_entry(entries, snap_id)
    except MdioError:
        raise
    if not entry:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap '{snap_id[:12]}...' not found"

    if restore_all:
        notes = entry.get("notes", {})
        if not notes:
            return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] snap has no files"
        restored = []
        for fp, fp_sh in notes.items():
            content = _read_ckpt_for_hash(workspace, wt_uid, fp_sh, entry.get("at", ""))
            if content is None: continue
            if state_hash(content) != fp_sh: continue
            try:
                _write_full_text(workspace, ctx.wt_uid, fp, content,
                                suppress_hooks=not hook_trigger)
                restored.append(fp)
            except Exception: pass
        return (f"- mdio_success: restored {len(restored)} file(s) from {_short_hash(snap_id)}"
                f"{' (hooks enabled)' if hook_trigger else ''}")

    try:
        abs_target = ctx.resolve_path(str(target_path))
    except Exception:
        abs_target = str(target_path)
    if "*" in abs_target or "?" in abs_target:
        try:
            expanded = workspace.store.expand_glob(ctx.wt_uid, abs_target, False)
            if not expanded:
                return f"- mdio_error: [{ErrCode.SNAP_NO_FILE}] no files matched: {target_path}"
            restored = []
            for fp in expanded:
                fp_sh = entry.get("notes", {}).get(fp)
                if not fp_sh:
                    fp_sh = next((v for k, v in entry.get("notes", {}).items()
                                  if k.endswith(fp) or fp.endswith(k)), None)
                if not fp_sh: continue
                content = _read_ckpt_for_hash(workspace, wt_uid, fp_sh, entry.get("at", ""))
                if content is None: continue
                actual_hash = state_hash(content)
                if actual_hash != fp_sh: continue
                try:
                    _write_full_text(workspace, ctx.wt_uid, fp, content,
                                    suppress_hooks=not hook_trigger)
                    restored.append(fp)
                except Exception: pass
            return (f"- mdio_success: restored {len(restored)} file(s) from "
                    f"{_short_hash(snap_id)}{' (hooks enabled)' if hook_trigger else ''}")
        except Exception:
            pass

    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)
    try:
        entry = _find_entry(entries, snap_id)
    except MdioError:
        raise
    if not entry:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap '{snap_id[:12]}...' not found"

    notes = entry.get("notes", {})
    state_h = notes.get(abs_target)
    if not state_h:
        for note_path, nhash in notes.items():
            if note_path.endswith(abs_target) or abs_target.endswith(note_path):
                state_h = nhash
                break
    if not state_h:
        return (f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] "
                f"file '{abs_target}' not in snap '{snap_id[:12]}...'")

    ckpt_content = _read_ckpt_for_hash(workspace, wt_uid, state_h,
                                       entry.get("at", ""))
    if ckpt_content is None:
        return (f"- mdio_error: [{ErrCode.SNAP_CKPT_ERR}] "
                f"ckpt data missing for state_hash '{_short_hash(state_h, 16)}...'")

    actual_hash = state_hash(ckpt_content)
    if actual_hash != state_h:
        return (f"- mdio_error: [{ErrCode.SNAP_CKPT_ERR}] "
                f"ckpt integrity check failed: "
                f"expected {_short_hash(state_h)}, got {_short_hash(actual_hash)}")

    file_exists = False
    try: file_exists = workspace.store.path_exists(ctx.wt_uid, abs_target)
    except Exception: pass
    needs_hook_note = hook_trigger and not file_exists

    try:
        _write_full_text(workspace, ctx.wt_uid, abs_target, ckpt_content,
                        suppress_hooks=not hook_trigger)
    except Exception as e:
        return f"- mdio_error: [{ErrCode.SNAP_PERM}] restore failed for '{abs_target}': {e}"

    hook_note = ""
    if needs_hook_note: hook_note = ", hook may need re-registration"
    elif hook_trigger: hook_note = " (hooks enabled)"
    return (f"- mdio_success: {abs_target} restored from {_short_hash(snap_id)}"
            f"{hook_note}")

# snap --del（DAG 保护 + 自动重 parent）

def _find_child_snaps(entries: List[Dict[str, Any]], snap_hash: str) -> List[Dict[str, Any]]:
    """Find all snaps that have snap_hash as their parent."""
    return [e for e in entries if e.get("parent", "") == snap_hash]

async def _snap_del(workspace, pipe_data, args, kwargs, ctx) -> str:
    """Delete a snap by hash, tag, or entire branch."""
    force = "-f" in kwargs
    tag = kwargs.get("--tag", "")
    branch = kwargs.get("--branch", "")
    del_id = kwargs.get("--del")
    if isinstance(del_id, bool):
        del_id = None
    if del_id is None and args:
        del_id = str(args[0])

    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)

    if tag:
        targets = [e for e in entries if e.get("tag", "") == str(tag)]
        if not targets:
            return f"- mdio_info: [snap] no snap found with tag '{tag}'"
        if len(targets) > 1 and not force:
            hashes = ", ".join(e["hash"][:12] for e in targets)
            return (f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] "
                    f"{len(targets)} snaps match tag '{tag}': {hashes}. Use -f to delete all.")
        for t in targets:
            result = await _delete_one_snap(workspace, wt_uid, snap_md_text, entries,
                                            t["hash"], force)
            snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
            entries = _parse_snap_entries(snap_md_text)
        return f"- mdio_success: deleted {len(targets)} snap(s) by tag '{tag}'"

    if branch:
        branch_entries = []
        heads = _parse_branch_heads(snap_md_text)
        head = heads.get(branch)
        if not head:
            return f"- mdio_info: [snap] branch '{branch}' has no HEAD"
        if not force:
            return (f"- mdio_info: [snap] will delete branch '{branch}' and all its snaps. "
                    f"Use -f to confirm.")
        seen = set()
        cur = head
        while cur:
            entry = _find_entry(entries, cur)
            if not entry or cur in seen:
                break
            seen.add(cur)
            branch_entries.append(entry)
            cur = entry.get("parent", "")
        for e in branch_entries:
            result = await _delete_one_snap(workspace, wt_uid, snap_md_text, entries,
                                            e["hash"], True)
            snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
            entries = _parse_snap_entries(snap_md_text)
        workspace.write_locator(
            wt_uid, SNAP_MD,
            drill(f"TIME.{branch}"),
            "->", "",
            auto_scaffold=True, god=True, suppress_hooks=True)
        return f"- mdio_success: deleted branch '{branch}' ({len(branch_entries)} snaps)"

    if not del_id:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap --del requires a snap ID, --tag, or --branch"
    entry = _find_entry(entries, del_id)
    if not entry:
        return (f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] "
                f"snap '{_short_hash(del_id, 12)}...' not found")
    if not force:
        children = _find_child_snaps(entries, del_id)
        child_note = f" ({len(children)} children)" if children else ""
        return (f"- mdio_info: [snap] will delete snap {_short_hash(del_id, 12)} "
                f"(\"{entry.get('message', '')}\"){child_note}. Use -f to confirm.")
    return await _delete_one_snap(workspace, wt_uid, snap_md_text, entries, del_id, force)


def _find_snap_branch(workspace, wt_uid: str, snap_hash: str) -> str:
    """Find which ## branch contains a given ### snap hash, using selectors."""
    ir = SelectorBuilder().drill("TIME").any_child().build()
    try:
        branches = workspace.read_locator(wt_uid, SNAP_MD, ir, proj="tag")
    except Exception:
        return DEFAULT_BRANCH
    for branch in branches:
        branch_name = str(branch).strip()
        if not branch_name or branch_name == "TIME":
            continue
        try:
            chunks = workspace.read_locator(
                wt_uid, SNAP_MD,
                drill(f"TIME.{branch_name}.{snap_hash}"),
            )
            if chunks:
                return branch_name
        except Exception:
            continue
    return DEFAULT_BRANCH


async def _delete_one_snap(workspace, wt_uid, snap_md_text, entries, snap_id, force):
    """Delete a single snap with DAG protection, using MDIO selectors."""
    entry = _find_entry(entries, snap_id)
    if not entry:
        return (f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] "
                f"snap '{_short_hash(snap_id, 12)}...' not found")

    children = _find_child_snaps(entries, entry["hash"])
    if children and not force:
        child_list = ", ".join(e["hash"][:12] for e in children[:5])
        more = f" (+{len(children) - 5} more)" if len(children) > 5 else ""
        return (f"- mdio_error: [{ErrCode.SNAP_NO_PARENT}] "
                f"snap {_short_hash(entry['hash'], 12)} has {len(children)} dependent snap(s): "
                f"{child_list}{more}. Use -f to force delete + auto-reparent.")

    deleted_hash = entry['hash']
    new_parent = entry.get("parent", "")
    branch = _find_snap_branch(workspace, wt_uid, deleted_hash)
    md_path = f"TIME.{branch}.{deleted_hash}"

    if children and force:
        for child in children:
            child_branch = _find_snap_branch(workspace, wt_uid, child["hash"])
            workspace.write_locator(wt_uid, SNAP_MD,
                by_tag(f"TIME.{child_branch}.{child['hash']}", "parent"),
                "/>", new_parent,
                auto_scaffold=True, god=True, suppress_hooks=True)

    workspace.write_locator(wt_uid, SNAP_MD,
        drill(md_path),
        "->", "",
        auto_scaffold=True, god=True, suppress_hooks=True)

    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    heads = _parse_branch_heads(snap_md_text)
    for br, h in heads.items():
        if h == deleted_hash:
            workspace.write_locator(wt_uid, SNAP_MD,
                by_tag(f"TIME.{br}", "HEAD"),
                "/>", new_parent if new_parent else "",
                auto_scaffold=True, god=True, suppress_hooks=True)

    reparent_note = f" ({len(children)} children reparented)" if children and force else ""
    return f"- mdio_success: deleted snap {_short_hash(deleted_hash, 12)}{reparent_note}"


# snap --verify / --gc（完整性审计）

async def _snap_verify(workspace, pipe_data, args, kwargs, ctx) -> str:
    """Full integrity audit: check all state_hashes exist in ckpt and match content."""
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)
    if not entries:
        return "- mdio_info: [snap] no snapshots to verify"

    referenced: Dict[str, set] = {}
    for e in entries:
        for fp, sh in e.get("notes", {}).items():
            referenced.setdefault(sh, set()).add(e["hash"])

    ckpt_sections: Dict[str, tuple] = {}
    delta_refs: Dict[str, set] = {}
    try:
        ckpt_files = workspace.store.expand_glob(wt_uid, f"{CKPT_DIR}/*.md", True)
    except Exception:
        ckpt_files = []
    missing, corrupted, orphans = [], [], []
    for cf in sorted(ckpt_files):
        ckpt_text = _read_full_text(workspace, wt_uid, cf)
        for m in re.finditer(r"^## ([0-9a-f]{64})$", ckpt_text, re.MULTILINE):
            h = m.group(1)
            parsed = _parse_ckpt_content(ckpt_text, h)
            if parsed is not None:
                ckpt_sections[h] = (cf.replace(CKPT_DIR + "/", ""), parsed)
            else:
                info = _parse_ckpt_entry_info(ckpt_text, h)
                if info and info["kind"] == "delta":
                    parent = info["data"]
                    delta_refs.setdefault(parent, set()).add(h)
                    reconstructed = _reconstruct_ckpt(workspace, wt_uid, h)
                    if reconstructed is not None:
                        ckpt_sections[h] = (cf.replace(CKPT_DIR + "/", ""), reconstructed)
                    else:
                        missing.append((h, set()))
                        continue
                else:
                    start = m.end()
                    rest = ckpt_text[start:]
                    next_sec = re.search(r"^## [0-9a-f]{64}$", rest, re.MULTILINE)
                    end = start + next_sec.start() if next_sec else len(ckpt_text)
                    parsed = ckpt_text[start:end].strip()
                    ckpt_sections[h] = (cf.replace(CKPT_DIR + "/", ""), parsed)

    verified = 0
    for h, snaps in referenced.items():
        if h not in ckpt_sections:
            missing.append((h, snaps))
        else:
            _, content = ckpt_sections[h]
            if state_hash(content) != h:
                corrupted.append((h, snaps))
            else:
                verified += 1

    for h, (month, _) in ckpt_sections.items():
        in_notes = h in referenced
        in_delta = h in delta_refs and len(delta_refs[h]) > 0
        if not in_notes and not in_delta:
            orphans.append((h, month))

    lines = []
    if not missing and not corrupted and not orphans:
        return f"- snap: verified {verified} state_hashes across {len(entries)} snaps — OK"

    if missing:
        for h, snaps in missing:
            lines.append(f"- missing: state {_short_hash(h)} (in snap(s) {', '.join(s[:12] for s in snaps)})")
    if corrupted:
        for h, snaps in corrupted:
            lines.append(f"- corrupted: state {_short_hash(h)} in ckpt (content hash mismatch, in snap(s) {', '.join(s[:12] for s in snaps)})")
    if orphans:
        lines.append(f"- orphan: {len(orphans)} unreferenced entries in ckpt")
        for h, month in orphans[:10]:
            lines.append(f"  {_short_hash(h, 16)} in {month}")
        if len(orphans) > 10:
            lines.append(f"  ... and {len(orphans) - 10} more")

    return f"- snap: {verified} ok, {len(missing)} missing, {len(corrupted)} corrupted, {len(orphans)} orphans\n" + "\n".join(lines)


async def _snap_gc(workspace, pipe_data, args, kwargs, ctx) -> str:
    """Garbage collect orphan ckpt entries. Requires -f."""
    if "-f" not in kwargs:
        return "- mdio_info: [snap] will remove unreferenced ckpt entries. Use -f to execute."

    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    entries = _parse_snap_entries(snap_md_text)
    referenced: set = set()
    for e in entries:
        for _, sh in e.get("notes", {}).items():
            referenced.add(sh)

    try:
        ckpt_files = workspace.store.expand_glob(wt_uid, f"{CKPT_DIR}/*.md", True)
    except Exception:
        ckpt_files = []

    for cf in sorted(ckpt_files):
        ckpt_text = _read_full_text(workspace, wt_uid, cf)
        for m in re.finditer(r"^- delta_from: ([0-9a-f]{64})$", ckpt_text, re.MULTILINE):
            referenced.add(m.group(1))
    cleaned = 0
    removed_hashes: List[str] = []
    for cf in sorted(ckpt_files):
        ckpt_text = _read_full_text(workspace, wt_uid, cf)
        modified = False
        for m in re.finditer(r"^## ([0-9a-f]{64})$", ckpt_text, re.MULTILINE):
            h = m.group(1)
            if h not in referenced:
                start = m.start()
                rest = ckpt_text[m.end():]
                next_sec = re.search(r"^## [0-9a-f]{64}$", rest, re.MULTILINE)
                end = m.end() + next_sec.start() if next_sec else len(ckpt_text)
                ckpt_text = ckpt_text[:start] + ckpt_text[end:]
                modified = True
                cleaned += 1
                removed_hashes.append(h)
        if modified:
            ckpt_text = re.sub(r"\n{3,}", "\n\n", ckpt_text)
            _write_full_text(workspace, wt_uid, cf, ckpt_text)

    # 磁盘已删除的条目必须同步失效：否则进程内仍命中缓存 ⇒ _append_ckpt_entry
    # 误判「已存」而 skip，notes 里的 hash 在磁盘上没有对应 ckpt（重启后爆）。
    # 见 BUGS 复盘 B5/A2。
    for h in removed_hashes:
        _ckpt_text_cache.pop((wt_uid, h), None)

    return f"- mdio_success: cleaned {cleaned} orphan ckpt entries"


# worktree checkout（全量替换编排）

async def _snap_checkout(workspace, ctx, target_hash: str, force: bool = False) -> str:
    """Full-state replacement: restore all files from a snap into the current worktree.

    1. Find target snap entry
    2. Check uncommitted changes (unless force)
    3. Compute diff: to_create, to_update, to_delete
    4. Rebuild files from ckpt → write_locator
    5. Delete removed files
    6. Update branch HEAD
    """
    branch = ctx.worktree_name or DEFAULT_BRANCH
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    if wt_uid == "genesis" and ctx.wt_uid != "genesis":
        wt_uid = ctx.wt_uid

    # Look for the snap: try WT-local snap.md first, then genesis.
    # (snaps created before cfork exist in genesis.)
    def _resolve_snap_entry():
        snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
        entries = _parse_snap_entries(snap_md_text)
        entry = _find_entry(entries, target_hash)
        if entry:
            return entry, snap_md_text, entries, wt_uid
        if wt_uid != "genesis":
            snap_md_text = _read_full_text(workspace, "genesis", SNAP_MD)
            entries = _parse_snap_entries(snap_md_text)
            entry = _find_entry(entries, target_hash)
            if entry:
                return entry, snap_md_text, entries, "genesis"
        return None, None, None, None

    target_entry, snap_md_text, entries, source_wt = _resolve_snap_entry()
    if not target_entry:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] snap '{_short_hash(target_hash)}...' not found"

    expand_wt, _ = _snap_expand_wt(workspace, wt_uid)
    target_notes = target_entry.get("notes", {})

    # ── 1. Get current file list ──
    # 检查与删除必须读同一份现值：此处曾让脏检查用 "*.md"（VFS 路径恒带前导
    # "/" 且 glob 的 * 不跨 "/" ⇒ 恒命中 0），删除集另用 "/**/*.md"，于是
    # 「检查放行、删除照删」——未提交的新文件被静默删除。见 BUGS 复盘 A1。
    try:
        current_files = set(_filter_owned_files(workspace, wt_uid,
            workspace.store.expand_glob(expand_wt, "/**/*.md", False)))
    except Exception:
        current_files = set()

    # ── 2. Check uncommitted changes ──
    if not force:
        heads = _parse_branch_heads(snap_md_text)
        head_hash = heads.get(branch)
        if head_hash:
            head_entry = _find_entry(entries, head_hash)
            if head_entry:
                head_notes = head_entry.get("notes", {})
                changed = []
                for fp, h_sh in head_notes.items():
                    cur = _read_full_text(workspace, expand_wt, fp)
                    cur_sh = state_hash(cur) if cur else ""
                    if cur_sh != h_sh:
                        changed.append(fp)
                # 与 step 1 共用同一份 current_files；未提交文件按「存在」计
                # （0 字节文件同样算改动，不能被静默删除）。
                for fp in sorted(current_files):
                    if fp not in head_notes:
                        changed.append(fp)
                if changed:
                    return (f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] "
                            f"uncommitted changes in {len(changed)} file(s). "
                            f"Commit or use --force to discard:\n" +
                            "\n".join(f"  M {f}" for f in changed[:10]) +
                            (f"\n  ... and {len(changed) - 10} more" if len(changed) > 10 else ""))

    target_files = set(target_notes.keys())

    to_create = target_files - current_files
    to_update = set()
    for fp in target_files & current_files:
        cur = _read_full_text(workspace, expand_wt, fp)
        cur_sh = state_hash(cur) if cur else ""
        if cur_sh != target_notes[fp]:
            to_update.add(fp)
    to_delete = current_files - target_files

    # ── 3. Rebuild files ──
    restored = 0
    errors = []

    for fp in sorted(to_create | to_update):
        fp_sh = target_notes[fp]
        content = _read_ckpt_for_hash(workspace, source_wt, fp_sh,
                                       target_entry.get("at", ""))
        if content is None:
            errors.append(f"  missing ckpt: {fp} ({_short_hash(fp_sh)})")
            continue
        if state_hash(content) != fp_sh:
            errors.append(f"  integrity fail: {fp}")
            continue
        try:
            _write_full_text(workspace, wt_uid, fp, content, suppress_hooks=True)
            restored += 1
        except Exception as e:
            errors.append(f"  write fail: {fp} — {e}")

    # ── 4. Delete removed files ──
    deleted = 0
    for fp in sorted(to_delete):
        try:
            workspace.rm(wt_uid, fp, recursive=False, force=True, verbose=False)
            deleted += 1
        except Exception:
            pass

    # ── 5. Update branch HEAD ──
    try:
        md_path = f"TIME.{branch}"
        workspace.write_locator(
            wt_uid, SNAP_MD,
            by_tag(md_path, "HEAD"),
            "/>", target_hash,
            auto_scaffold=True, god=True, suppress_hooks=True)
    except Exception as e:
        errors.append(f"  HEAD update fail: {e}")

    # ── Result ──
    result_parts = [f"- worktree: checked out {_short_hash(target_hash)}"]
    if restored:
        result_parts.append(f"- restored: {restored} file(s)")
    if deleted:
        result_parts.append(f"- deleted: {deleted} file(s)")
    if errors:
        result_parts.append(f"- errors: {len(errors)}")
        result_parts.extend(errors)

    return "\n".join(result_parts)


# worktree 层

# worktrees.md I/O

def _read_worktrees_md(workspace, wt_uid: str = None) -> str:
    """Read worktrees.md from a specific WT. Default: genesis (backward compat)."""
    if wt_uid is None:
        wt_uid = GENESIS_WT
    return _read_full_text(workspace, wt_uid, WORKTREES_MD)


def _parse_worktree_entries(workspace, wt_uid: str = None) -> List[Dict]:
    """Read worktrees.md via selector and parse into list of {name, uid, rule, path}.

    Uses drill('WORKTREES').any_child() to navigate to each ## entry,
    then extracts heading names and - key: value pairs.
    解析走新路径（tojson 信封 + bullets_to_kv）；旧手写行解析（parse_kv_lines）已迁出。
    """
    if wt_uid is None:
        wt_uid = GENESIS_WT
    ir = SelectorBuilder().drill("WORKTREES").any_child().build()
    try:
        children = workspace.read_locator(wt_uid, WORKTREES_MD, ir)
    except Exception:
        return []
    entries = []
    for chunk in children:
        lines = str(chunk).split("\n")
        if not lines:
            continue
        name = lines[0].strip().lstrip("#").strip()
        entry = {"name": name}
        body = "\n".join(lines[1:])
        if not body.endswith("\n"):
            # 防御性归一：裸 to_json_text 在末行是 `- k:`（冒号结尾且无换行）时会投影越界
            # panic（PanicException 不是 Exception 子类，兜不住）。
            # VFS 读回的 chunk 目前恒以换行结尾（store 落盘即归一），对 kv 提取无影响。
            body += "\n"
        entry.update(bullets_to_kv(json.loads(workspace.to_json_text(body))))
        entries.append(entry)
    return entries


def _format_worktree_entry(name: str, uid: str, rule: str = "",
                            path: str = "", doc_subdir: str = "",
                            wal_rel: str = "") -> str:
    """条目字段：name / uid / rule / path。

    注：external_root 不再是条目字段——它与 `path` 是同值副本，
    core 侧解析时对缺此字段的条目用 path 兜底。
    """
    lines = [f"## {name}", f"- uid: {uid}"]
    if rule:
        lines.append(f"- rule: {rule}")
    if path:
        lines.append(f"- path: {path}")
    # 三根分层的布局名（只在分层时写；不写 = 继承 store 默认 = 旧扁平行）
    if doc_subdir:
        lines.append(f"- doc_subdir: {doc_subdir}")
    if wal_rel:
        lines.append(f"- wal_rel: {wal_rel}")
    return "\n".join(lines) + "\n"


def _resolve_worktree_name(workspace, name: str) -> Optional[Dict]:
    """B10: wt 名 → 条目。唯一实现 = workspace.resolve_worktree_entry（Q8：本地薄壳）。"""
    return workspace.resolve_worktree_entry(name)


def _append_worktree_entry(workspace, name: str, uid: str,
                            parent_wt: str = GENESIS_WT,
                            rule: str = "", path: str = "",
                            doc_subdir: str = "", wal_rel: str = "") -> None:
    """B10: write entry to PARENT WT's worktrees.md, not genesis."""
    entry_text = _format_worktree_entry(name, uid, rule, path=path,
                                        doc_subdir=doc_subdir, wal_rel=wal_rel)
    existing = _read_worktrees_md(workspace, parent_wt)
    if not existing or existing.strip() == "":
        _write_full_text(workspace, parent_wt, WORKTREES_MD,
                         f"# WORKTREES\n\n{entry_text}")
    else:
        _append_to_file(workspace, parent_wt, WORKTREES_MD, f"\n{entry_text}")


def write_rule_to_file(workspace, store, wt_uid: str) -> bool:
    """把 wt 当前的 rule 写回父 WT 的 worktrees.md（文本级，rule 写回的唯一实现）。

    返回 True = 已写回；False = 该 wt 没有 worktrees.md 条目或文件不可读
    （genesis 没有条目：它的 bindings 来自 store 级配置，rule 无处落盘）。
    调用方据此避免「假成功」——见 BUGS 复盘 B2。

    rule="" 派生的条目没有 - rule: 行，selector 方式（tag_eq("rule")）会未命中并
    把整个文件写坏（WT 消失）。本实现：定位条目段 → 替换/插入 rule 行 → 写回全文。
    gateway mount 与 CLI mount 共用本函数。见 BUGS 复盘 B2/B1。
    """
    parent = store.parent_uid(wt_uid) or GENESIS_WT
    entries = _parse_worktree_entries(workspace, parent)
    entry = next((e for e in entries if e.get("uid") == wt_uid), None)
    if not entry:
        return False

    remote = store.rule_remote_entries(wt_uid)
    current_rule = entry.get("rule", "")
    bind = [p for p in current_rule.split() if not p.startswith("/share/")]
    remote_paths = [p for p, _ in remote]
    new_rule = " ".join(bind + remote_paths)

    try:
        text = store.get_text(parent, WORKTREES_MD) or ""
    except Exception:
        return False
    if not text.strip():
        return False
    lines = text.split("\n")
    start = None
    for i, ln in enumerate(lines):
        if ln.startswith("## ") and ln[3:].strip() == entry["name"]:
            start = i
            break
    if start is None:
        return False
    end = len(lines)
    for i in range(start + 1, len(lines)):
        if lines[i].startswith("## "):
            end = i
            break
    seg = [ln for ln in lines[start:end] if not ln.startswith("- rule:")]
    if new_rule:
        # 插到条目内最后一条 "- " 行之后、尾部空行之前：Rust 侧 read_worktree_children
        # 按块内容解析，rule 行不能落在尾部空行之外（否则被挤出内容块）。
        insert_at = len(seg)
        while insert_at > 0 and not seg[insert_at - 1].startswith("- "):
            insert_at -= 1
        seg.insert(insert_at, f"- rule: {new_rule}")
    new_text = "\n".join(lines[:start] + seg + lines[end:])
    workspace.write_locator(
        parent, WORKTREES_MD, SelectorBuilder().build(),
        "/>", new_text, auto_scaffold=True, god=True, suppress_hooks=True,
    )
    return True


def _delete_worktree_entry(workspace, name: str) -> None:
    """B10: delete entry from the PARENT WT's worktrees.md.
    Uses tree.parent_uid() to find which worktrees.md holds the entry.
    Falls back to genesis for backward compat."""
    # Find the WT's uid first, then get its parent
    entry = _resolve_worktree_name(workspace, name)
    if not entry or not entry.get("uid"):
        return
    uid = entry["uid"]
    try:
        parent = workspace.store.parent_uid(uid)
        wt_uid = parent if parent else GENESIS_WT
    except Exception:
        wt_uid = GENESIS_WT
    workspace.write_locator(
        wt_uid, WORKTREES_MD,
        drill(f"WORKTREES.{name}"),
        "->", "",
        auto_scaffold=True, god=True, suppress_hooks=True)




# WT sub-commands

async def _wt_bfork(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree bfork "name" --rule "/workspace" """
    name: Optional[str] = None
    for a in args:
        s = str(a).strip()
        if s and not s.startswith("-"):
            name = s
            break
    if not name:
        name = str(kwargs.get("--name", kwargs.get("-n", ""))).strip()
    if not name:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] bfork requires a name"

    if not name.replace("_", "").replace("-", "").isalnum():
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] worktree name must be alphanumeric: '{name}'"

    existing = _resolve_worktree_name(workspace, name)
    if existing:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] worktree '{name}' already exists"

    rule = str(kwargs.get("--rule", kwargs.get("-r", ""))).strip()
    if not rule:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] bfork requires --rule"

    try:
        wt_uid = workspace.derive_worktree(ctx.wt_uid, rule, name)
    except Exception as e:
        return f"- mdio_error: [{ErrCode.VFS_USURP}] bfork failed: {e}"

    try:
        _append_worktree_entry(workspace, name, wt_uid,
                               parent_wt=ctx.wt_uid, rule=rule)
    except Exception as e:
        try:
            await workspace.dispose_plugins_wt(wt_uid, strict=True)
            workspace.drop_derived_worktree(wt_uid)
        except Exception:
            pass
        return f"- mdio_error: [{ErrCode.SNAP_CKPT_ERR}] failed to register worktree: {e}"

    workspace.active_worktree_name = name
    workspace.active_worktree_uid = wt_uid

    return f"- worktree: bfork '{name}' created"


async def _wt_use(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree use "name" — switch to existing worktree."""
    name: Optional[str] = None
    for a in args:
        s = str(a).strip()
        if s and not s.startswith("-"):
            name = s
            break
    if not name:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] use requires a worktree name"

    # genesis 是根 wt，不在 worktrees.md 里 ⇒ 单独当「回根」处理。
    # 少了这一支：从子 wt 切不回 genesis，而 MdioRuntime.exec 未给 wt_uid 时用的正是
    # active_worktree_uid ⇒ 服务端会一直停在子 wt（实测）。
    if name == GENESIS_WT:
        workspace.active_worktree_name = None
        workspace.active_worktree_uid = GENESIS_WT
        return f"- worktree: switched to '{GENESIS_WT}'"

    entry = _resolve_worktree_name(workspace, name)
    if not entry:
        return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] worktree '{name}' not found"

    uid = entry.get("uid")
    if uid is None:
        return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] worktree '{name}' missing uid"

    workspace.active_worktree_name = name
    workspace.active_worktree_uid = uid

    return f"- worktree: switched to '{name}'"


async def _wt_current(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree current — output current worktree name."""
    name = ctx.worktree_name
    if name:
        return name
    return "(genesis)"


async def _wt_list(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree list — list all worktrees, * marks current.
    B10: collects entries from ALL WTs' worktrees.md files."""
    entries = []
    for uid, _name, _sr, _ep in workspace.store.list_worktrees():
        entries.extend(_parse_worktree_entries(workspace, uid))
    if not entries:
        return "- (no worktrees)"

    lines = []
    current_name = ctx.worktree_name

    # Use RuleManager for type classification when available
    try:
        from mdio_py.runtime.rule_manager import RuleManager
        rule_mgr = RuleManager(workspace.store)
        _use_rule_mgr = True
    except Exception:
        rule_mgr = None
        _use_rule_mgr = False

    for e in entries:
        marker = "*" if e.get("name") == current_name else " "
        name = e.get("name", "?")
        entry_path = e.get("path", "")
        entry_rule = e.get("rule", "")

        if _use_rule_mgr and rule_mgr is not None:
            e_uid = e.get("uid", "")
            kind = rule_mgr.get_type(e_uid) if e_uid else "bfork"
        else:
            kind = "bfork" if entry_rule else ("project" if entry_path else "cfork")

        label = f"{marker} {name} ({kind})"
        if entry_path:
            label += f" → {entry_path}"
        lines.append(f"- {label}")

    return "\n".join(lines)


async def _wt_show(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree show <hash|name> — snap details or worktree details.

    Hash takes priority: if target looks like a hex string or is found
    in snap.md, treat as hash. Otherwise resolve as worktree name.
    """
    target: Optional[str] = None
    for a in args:
        s = str(a).strip()
        if s and not s.startswith("-"):
            target = s
            break

    if not target:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] show requires a hash or name"

    is_hex_hash = len(target) >= 12 and all(c in '0123456789abcdef' for c in target.lower())
    if is_hex_hash:
        wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
        snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
        entries = _parse_snap_entries(snap_md_text)
        try:
            if _find_entry(entries, target):
                return await _snap_show(workspace, pipe_data, args, kwargs, ctx)
        except Exception:
            pass

    entry = _resolve_worktree_name(workspace, target)
    if entry:
        rule = entry.get("rule", "")
        path = entry.get("path", "")
        # 使用 RuleManager.get_type() 判定类型
        try:
            from mdio_py.runtime.rule_manager import RuleManager
            rule_mgr = RuleManager(workspace.store)
            e_uid = entry.get("uid", "")
            kind = rule_mgr.get_type(e_uid) if e_uid else "bfork"
        except Exception:
            kind = "bfork" if rule else ("project" if path else "cfork")
        lines = [
            f"- name: {entry.get('name', '?')}",
            f"- uid: {entry.get('uid', '?')}",
            f"- kind: {kind}",
        ]
        if rule:
            lines.append(f"- rule: {rule}")
        if path:
            lines.append(f"- path: {path}")
        return "\n".join(lines)

    return await _snap_show(workspace, pipe_data, args, kwargs, ctx)


async def _wt_commit(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree commit -am "msg" — snapshot current worktree state."""
    am_val = kwargs.pop("-am", None)
    if am_val and not isinstance(am_val, bool):
        kwargs["-m"] = str(am_val)

    return await _snap_create(workspace, pipe_data, args, kwargs, ctx)


async def _wt_log(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree log — show commit history for current worktree."""
    return await _snap_log(workspace, pipe_data, args, kwargs, ctx)


async def _wt_status(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree status — show current worktree + HEAD + uncommitted changes."""
    wt_name = ctx.worktree_name
    if not wt_name:
        return await _snap_status(workspace, pipe_data, args, kwargs, ctx)

    lines = [f"- worktree: {wt_name}"]
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    expand_wt, _scope_targets = _snap_expand_wt(workspace, wt_uid)
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    heads = _parse_branch_heads(snap_md_text)
    branch = wt_name
    head = heads.get(branch)

    if not head:
        lines.append("- (no commits yet)")
        return "\n".join(lines)

    entries = _parse_snap_entries(snap_md_text)
    head_entry = _find_entry(entries, head)
    if head_entry:
        msg = head_entry.get("message", "")
        lines.append(f"- HEAD: {_short_hash(head)} \"{msg}\"")
    else:
        lines.append(f"- HEAD: {_short_hash(head)}")

    head_notes = head_entry.get("notes", {}) if head_entry else {}
    all_paths = set(head_notes.keys())
    try:
        current_files = _filter_owned_files(workspace, wt_uid,
            workspace.store.expand_glob(expand_wt, "/**/*.md", False))
        all_paths.update(current_files)
    except Exception:
        pass

    changed, added, deleted = [], [], []
    for fp in sorted(all_paths):
        cur = _read_full_text(workspace, expand_wt, fp)
        cur_sh = state_hash(cur) if cur else ""
        head_sh = head_notes.get(fp)
        if not cur and head_sh:
            deleted.append(fp)
        elif not head_sh and cur:
            added.append(fp)
        elif head_sh != cur_sh:
            changed.append(fp)

    if changed:
        lines.append(f"- changed: {len(changed)} file(s)")
        for f in changed:
            lines.append(f"  M {f}")
    if added:
        lines.append(f"- added: {len(added)} file(s)")
        for f in added:
            lines.append(f"  A {f}")
    if deleted:
        lines.append(f"- deleted: {len(deleted)} file(s)")
        for f in deleted:
            lines.append(f"  D {f}")
    if not changed and not added and not deleted:
        lines.append("- (clean)")

    return "\n".join(lines)


async def _wt_diff(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree diff — diff working files vs HEAD."""
    return await _snap_diff(workspace, pipe_data, args, kwargs, ctx)


async def _wt_verify(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree verify — integrity audit of all snapshots."""
    return await _snap_verify(workspace, pipe_data, args, kwargs, ctx)


async def _wt_del(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree del <hash> [-f] — delete a snap."""
    return await _snap_del(workspace, pipe_data, args, kwargs, ctx)


async def _wt_cfork(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree cfork "name" [--rule "/docs"] [--from <hash>]

    Cfork = clean fork: allocates new key prefix for independent file storage.
    --rule: copy matching files from parent into the new worktree.
    --from: two-step sugar = cfork + checkout.
    """
    name: str | None = None
    for a in args:
        s = str(a).strip()
        if s and not s.startswith("-"):
            name = s
            break
    if not name:
        name = str(kwargs.get("--name", kwargs.get("-n", ""))).strip()
    if not name:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] cfork requires a name"

    if not name.replace("_", "").replace("-", "").isalnum():
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] worktree name must be alphanumeric: '{name}'"

    existing = _resolve_worktree_name(workspace, name)
    if existing:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] worktree '{name}' already exists"

    from_hash = str(kwargs.get("--from", "")).strip()
    copy_from = str(kwargs.get("--rule", kwargs.get("-r", ""))).strip()

    try:
        wt_uid = workspace.derive_worktree(ctx.wt_uid, "")
    except Exception as e:
        return f"- mdio_error: [{ErrCode.VFS_USURP}] cfork failed: {e}"

    copied_count = 0
    if copy_from:
        try:
            for r in copy_from.split(";"):
                r = r.strip()
                if not r:
                    continue
                parts = r.split(":")
                src = parts[0].strip()
                if not src.startswith("/"):
                    src = ctx.resolve_path(src) if hasattr(ctx, 'resolve_path') else "/" + src
                try:
                    src_files = workspace.store.expand_glob(ctx.wt_uid, f"{src}/*.md", False)
                except Exception:
                    src_files = []
                for fp in src_files:
                    content = _read_full_text(workspace, ctx.wt_uid, fp)
                    if not content:
                        continue
                    dest = fp
                    if fp.startswith(src):
                        dest = fp[len(src):]
                    if not dest.startswith("/"):
                        dest = "/" + dest
                    try:
                        _write_full_text(workspace, wt_uid, dest, content,
                                        suppress_hooks=True)
                        copied_count += 1
                    except Exception:
                        pass
        except Exception as e:
            pass

    _append_worktree_entry(workspace, name, wt_uid,
                           parent_wt=ctx.wt_uid)

    workspace.active_worktree_name = name
    workspace.active_worktree_uid = wt_uid

    result_lines = [f"- worktree: cfork '{name}' created"]
    if copied_count:
        result_lines.append(f"- copied: {copied_count} file(s)")

    if from_hash:
        checkout_ctx = type(ctx)(wt_uid=ctx.wt_uid, vfs_path="/", active_doc=None, worktree_name=name)
        checkout_result = await _snap_checkout(workspace, checkout_ctx, from_hash, force=False)
        result_lines.append(checkout_result)

    return "\n".join(result_lines)


async def _wt_checkout(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree checkout <hash> [--force] — full-state replacement."""
    target_hash: str | None = None
    for a in args:
        # D15 的边角：全数字 hash 会被语法当成 NUMBER（int），前导 0 已丢。
        # 按 12/64 位快照 hash 的已知长度补零还原，保证“裸 hash 一律可用”，
        # 不需要用户记“有时加引号”。
        if isinstance(a, int) and not isinstance(a, bool):
            target_hash = f"{a:012d}" if len(str(a)) <= 12 else f"{a:064d}"
            break
        # 运行时可能已把 NUMBER 转成字符串（前导 0 已丢）；短全数字串按 12 位补零。
        s = str(a).strip()
        if s.isdigit() and len(s) < 12:
            target_hash = f"{int(s):012d}"
            break
        if s and not s.startswith("-"):
            target_hash = s
            break
    if not target_hash:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] checkout requires a snap hash"

    force = "-f" in kwargs or "--force" in kwargs

    return await _snap_checkout(workspace, ctx, target_hash, force=force)


async def _wt_rm(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree rm "name" [-f] — delete a worktree."""
    name: str | None = None
    for a in args:
        s = str(a).strip()
        if s and not s.startswith("-"):
            name = s
            break
    if not name:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] rm requires a worktree name"

    entry = _resolve_worktree_name(workspace, name)
    if not entry:
        return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] worktree '{name}' not found"

    uid = entry.get("uid")
    if uid is None:
        return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] worktree '{name}' missing uid"

    rule = entry.get("rule", "")
    entry_path = entry.get("path", "")
    is_cfork = not rule and not entry_path  # cfork: no rule, no path
    force = "-f" in kwargs or "--force" in kwargs

    if not force:
        pass  # uncommitted changes check deferred (needs uid-based file access)

    try:
        await workspace.dispose_plugins_wt(uid, strict=True)
        workspace.drop_worktree([uid])
    except Exception as e:
        return f"- mdio_error: [{ErrCode.VFS_USURP}] failed to drop worktree: {e}"

    _delete_worktree_entry(workspace, name)

    if workspace.active_worktree_name == name:
        workspace.active_worktree_name = None
        workspace.active_worktree_uid = "genesis"

    return f"- worktree: deleted '{name}'"


async def _wt_gc(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree gc -f — garbage collect orphan ckpt entries (global)."""
    return await _snap_gc(workspace, pipe_data, args, kwargs, ctx)


async def _wt_undo(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree undo [--force] — checkout HEAD.parent."""
    branch = ctx.worktree_name or DEFAULT_BRANCH
    wt_uid = getattr(workspace, 'active_worktree_uid', None) or ctx.wt_uid or "genesis"
    snap_md_text = _read_full_text(workspace, wt_uid, SNAP_MD)
    heads = _parse_branch_heads(snap_md_text)
    head_hash = heads.get(branch)

    if not head_hash:
        return f"- mdio_info: [snap] no commit to undo on '{branch}'"

    entries = _parse_snap_entries(snap_md_text)
    head_entry = _find_entry(entries, head_hash)

    if not head_entry:
        return f"- mdio_error: [{ErrCode.SNAP_NO_SNAP}] HEAD snap not found"

    parent_hash = head_entry.get("parent", "")
    if not parent_hash:
        return f"- mdio_info: [snap] HEAD has no parent — nothing to undo"

    force = "-f" in kwargs or "--force" in kwargs

    return await _snap_checkout(workspace, ctx, parent_hash, force=force)


# Dispatch table

async def _wt_mount(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree mount --path /share/<name> --from iroh://<ticket> [--name <target>] [--as <name>]
    worktree mount --path / --from iroh://<ticket> --as <name>  → 创建新 WT + remote = /

    注意：位置参数形态（worktree mount /share/x）在 CLI 文法里进不来（裸路径被当作
    读表达式），必须用 --path。见 BUGS 复盘 B2。
    """
    from mdio_py.runtime.rule_manager import RuleManager

    path: Optional[str] = None
    ticket: Optional[str] = None
    target_name: Optional[str] = None
    as_name: Optional[str] = None

    for a in args:
        s = str(a).strip()
        if s.startswith("/"):
            path = s
        elif s.startswith("iroh://") or s.startswith("iroh-share://"):
            ticket = s
        elif s and not s.startswith("-"):
            target_name = s

    path = kwargs.get("--path", kwargs.get("-p", path))
    ticket = kwargs.get("--from", kwargs.get("-f", ticket))
    target_name = kwargs.get("--name", kwargs.get("-n", target_name))
    as_name = kwargs.get("--as", as_name)

    if not path or not ticket:
        return (f"- mdio_error: [{ErrCode.PARSE_TOKEN}] mount requires /share/<name> "
                f"and --from iroh://<ticket>（位置参数形态不可用，请改用 --path）")

    # load / → 创建新 WT + rule = /
    if path == "/":
        if not as_name:
            return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] mount / requires --as <name>"
        # 原实现先 derive_worktree 再调 workspace._switch_context —— 该方法不存在，
        # 结果是「报错 + 留下一个 rebuild 后消失的幽灵 WT」。这里先做前置检查：不满足
        # 就快速失败、不做任何改动。
        if not hasattr(workspace, "_switch_context"):
            return (f"- mdio_error: [{ErrCode.RUST_IO}] mount / not implemented: "
                    f"Workspace has no _switch_context (remote download flow is TODO); "
                    f"nothing changed")
        try:
            new_uid = workspace.derive_worktree(ctx.wt_uid, "")
            # 标记为 project (有 remote 的 WT)
            rule_mgr = RuleManager(workspace.store)
            rule_mgr.add_remote(new_uid, "/", as_name)  # TODO: full remote flow needs iroh download
            workspace._switch_context(new_uid, as_name)
            return f"- worktree: created '{as_name}' with remote /"
        except Exception as e:
            return f"- mdio_error: [{ErrCode.RUST_IO}] failed to create WT: {e}"

    if not path.startswith("/share/"):
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] mount path must start with /share/ (got: {path})"

    entry_id = as_name or path.rstrip("/").rsplit("/", 1)[-1]
    if not entry_id:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] invalid remote path: {path}"

    # Resolve target WT
    if target_name:
        target_uid = workspace.resolve_worktree_name(target_name)
        if not target_uid:
            return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] worktree '{target_name}' not found"
    else:
        target_uid = workspace.active_worktree_uid

    # Add remote segment：内存 + 文本级写回 worktrees.md（与 gateway mount 共用
    # write_rule_to_file），不调 tree_ensure —— gateway 侧已探明重建路径不可靠，
    # 重启恢复由 bootstrap 的 tree_ensure 承担。原实现只调 RuleManager(store)，
    # 而它的两个 if 都不成立 ⇒ 只改内存、下次 rebuild 即丢。见 BUGS 复盘 B2。
    try:
        workspace.store.rule_add_remote(target_uid, path, entry_id)
        if not write_rule_to_file(workspace, workspace.store, target_uid):
            # 目标 wt 没有 worktrees.md 条目（genesis 就是这种：bindings 来自 store
            # 级配置，rule 无处落盘）⇒ 回滚内存改动并明确报错，不再假成功。
            workspace.store.rule_remove_remote(target_uid, path)
            return (f"- mdio_error: [{ErrCode.RUST_IO}] worktree '{target_uid}' has no "
                    f"worktrees.md entry to persist the rule; nothing changed")
    except Exception as e:
        return f"- mdio_error: [{ErrCode.RUST_IO}] failed to add remote: {e}"

    return f"- worktree: added remote {path} to {target_name or target_uid}"



async def _wt_share(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree share <path> [--as <name>] — 分享本地子树。"""
    share_path: Optional[str] = None
    as_name: Optional[str] = None

    for a in args:
        s = str(a).strip()
        if s.startswith("/"):
            share_path = s
        elif s and not s.startswith("-"):
            as_name = s

    share_path = kwargs.get("--path", kwargs.get("-p", share_path))
    as_name = kwargs.get("--as", kwargs.get("-n", as_name))

    if not share_path:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] share requires a path to share"

    target_uid = workspace.active_worktree_uid
    name = as_name or share_path.strip("/").rsplit("/", 1)[-1]

    # Write gateway.md share entry: local_prefix + shared_token
    from mdio_py.gateway.api.gateway import _make_token, iroh_share  # type: ignore
    # iroh_share handles the gateway.md writing + token registration
    # For CLI share, we create a simple gateway entry manually
    gw_path = "/.mds/gateway.md"
    entry_title = name
    token_hex = _make_token()  # CSPRNG（R12）
    import time
    created_str = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    entry_text = (
        f"\n## {entry_title}\n"
        f"- local_prefix: {share_path}\n"
        f"- shared_token: {token_hex}\n"
        f"- created: {created_str}\n"
    )
    from mdio_py.core.selector_builder import SelectorBuilder
    try:
        workspace.write_locator(
            target_uid, gw_path, SelectorBuilder().build(),
            "+>", entry_text, auto_scaffold=True,
        )
    except Exception as e:
        return f"- mdio_error: [{ErrCode.RUST_IO}] share failed: {e}"

    return f"- worktree: shared {share_path} as '{name}' (token: {token_hex})"


async def _wt_unmount(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree unmount --path /share/<name> [--name <wt>] — 断开远程 folder（文件留本地，并停掉投影）。

    位置参数形态不可用：裸路径会被语法解析成读表达式，必须用 --path（与 mount 同款）。
    owner 由 path 在 rule / gateway.md / Active 三档来源按归一化路径解析，**不默认
    active_worktree_uid**（D35 的教训：拿 active 当缺省会把删除打到错树）。
    - 有 _gateway_store（serve/HTTP 进程）：复用其 unmount_by_path——同时删 rule 段、
      删 gateway.md 条目、停对应 handler，并做读回校验；
    - 无 _gateway_store（纯 exec/本地规则模式）：临时装配一个 GatewayStore 做同样的
      文件清理（重启不会从 worktrees.md/gateway.md 复活）；本进程没有 handler 可停，
      运行中的 gateway 进程需由它自己的 reconcile 感知文件变化。

    旧的 `unload`（只删规则段）与 `offload`（删规则 + 停 handler）的并集语义不变；
    `--name` 保留旧的定向能力（写的是 worktree 名，不是挂载点名）。
    """
    path: Optional[str] = None
    target_name: Optional[str] = None

    for a in args:
        s = str(a).strip()
        if s.startswith("/share/"):
            path = s
        elif s and not s.startswith("-"):
            target_name = s

    path = kwargs.get("--path", kwargs.get("-p", path))
    target_name = kwargs.get("--name", kwargs.get("-n", target_name))

    if not path:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] unmount requires --path /share/<name>"

    # 显式 --name 交给 GatewayStore 解析：它同时支持 worktree 名/uid/路径，
    # 且会校验 path 是否真属于该树。这里不自己 resolve_worktree_name——那会
    # 把 genesis（无 worktrees.md 条目）这类合法 uid 误判成 not found。
    # 单一收口：所有删除入口都走 GatewayStore.unmount_by_path（D35 后该方法是
    # rule/gateway.md/Active 三档状态的唯一清理器，且自己按 path 解析 owner）。
    # 纯 exec 进程没有常驻 gateway store，用临时实例做同一套文件清理；它没有
    # handler 可停，但能保证 worktrees.md/gateway.md 干净、重启不复活。
    from mdio_py.gateway.store import GatewayStore, GatewayTargetError
    gs = getattr(workspace, "_gateway_store", None)
    if gs is None:
        gs = GatewayStore(workspace.store, workspace)
    try:
        result = gs.unmount_by_path(path, wt_name=target_name)
    except GatewayTargetError as e:
        return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] unmount failed: {e}"
    except Exception as e:
        return f"- mdio_error: [{ErrCode.RUST_IO}] unmount failed: {e}"

    return (f"- worktree: unmounted {result.get('path', path)} from "
            f"{result.get('wt_uid', target_name)}（文件保留为 local）")


async def _wt_load(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree load "<本地目录>" [--rule R] [--pwt PARENT] — 作为子 wt 注册。"""
    path: Optional[str] = None
    for a in args:
        s = str(a).strip()
        if s and not s.startswith("-"):
            path = s
            break
    path = kwargs.get("--path", kwargs.get("-p", path))
    if not path:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] load requires a local directory path"
    rule = str(kwargs.get("--rule", kwargs.get("-r", "")) or "").strip()
    parent = str(kwargs.get("--pwt", kwargs.get("-pwt", ctx.wt_uid)) or ctx.wt_uid).strip()
    try:
        parent_uid = workspace.resolve_worktree_ref(parent)
        if not workspace.is_descendant_or_self(parent_uid, ctx.wt_uid):
            return (
                f"- mdio_error: [{ErrCode.PARSE_TOKEN}] parent '{parent}' must be "
                f"self or a descendant of {ctx.wt_uid}"
            )
        uid, is_new = workspace.load_worktree(str(path), rule, parent_wt=parent_uid)
    except Exception as e:
        return f"- mdio_error: [{ErrCode.RUST_IO}] load failed: {e}"
    return f"- worktree: loaded '{path}' (uid {uid}{'，new' if is_new else '，已存在'})"


async def _wt_offload(workspace, pipe_data, args, kwargs, ctx) -> str:
    """worktree offload "<name|uid|path>" — 卸载当前 wt 的后代。"""
    target: Optional[str] = None
    for a in args:
        s = str(a).strip()
        if s and not s.startswith("-"):
            target = s
            break
    target = kwargs.get("--name", kwargs.get("-n", target))
    if not target:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] offload requires a worktree name/uid/path"
    try:
        uid = workspace.resolve_worktree_ref(str(target))
    except Exception as e:
        return f"- mdio_error: [{ErrCode.SNAP_NO_MATCH}] {e}"
    if uid == GENESIS_WT:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] genesis 不可 offload"
    if uid == ctx.wt_uid or not workspace.is_descendant_or_self(uid, ctx.wt_uid):
        return (
            f"- mdio_error: [{ErrCode.PARSE_TOKEN}] offload target '{target}' must be "
            f"a descendant of {ctx.wt_uid}"
        )
    try:
        await workspace.dispose_plugins_wt(uid, strict=True)
        workspace.offload_worktree(uid)
    except Exception as e:
        return f"- mdio_error: [{ErrCode.RUST_IO}] offload failed: {e}"
    return f"- worktree: offloaded '{target}' (uid {uid})"
_WT_DISPATCH = {
    "bfork": _wt_bfork,
    "cfork": _wt_cfork,
    "use": _wt_use,
    "current": _wt_current,
    "list": _wt_list,
    "show": _wt_show,
    "mount": _wt_mount,
    "unmount": _wt_unmount,
    "share": _wt_share,
    "load": _wt_load,
    "offload": _wt_offload,
    "commit": _wt_commit,
    "checkout": _wt_checkout,
    "undo": _wt_undo,
    "log": _wt_log,
    "status": _wt_status,
    "diff": _wt_diff,
    "verify": _wt_verify,
    "del": _wt_del,
    "rm": _wt_rm,
    "gc": _wt_gc,
}


# Main entry point: worktree()

async def worktree(workspace, pipe_data, args, kwargs, ctx):
    """WORKTREE — worktree + snap orchestration."""
    sub: Optional[str] = None
    sub_args = list(args)

    # `worktree bfork name --rule /path` arrives as:
    #   args[0] = FuncCall(name='bfork', args=[FuncCall(name='name', kwargs={...})])
    if sub_args and hasattr(sub_args[0], 'name'):
        nested = sub_args.pop(0)
        sub = str(nested.name).strip().lower()
        for na in getattr(nested, 'args', []):
            if hasattr(na, 'name'):
                sub_args.append(str(na.name))
            else:
                sub_args.append(str(na).strip())
        merged = {}
        for na in getattr(nested, 'args', []):
            if hasattr(na, 'kwargs'):
                for k, v in na.kwargs.items():
                    merged[str(k)] = v
        for k, v in getattr(nested, 'kwargs', {}).items():
            merged[str(k)] = v
        for k, v in kwargs.items():
            merged[str(k)] = v
        kwargs = merged
    elif sub_args and not str(sub_args[0]).startswith("-"):
        sub = str(sub_args.pop(0)).strip().lower()

    if not sub:
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] worktree requires a sub-command: {', '.join(sorted(_WT_DISPATCH.keys()))}"

    handler = _WT_DISPATCH.get(sub)
    if not handler:
        avail = ", ".join(sorted(_WT_DISPATCH.keys()))
        return f"- mdio_error: [{ErrCode.PARSE_TOKEN}] unknown worktree sub-command '{sub}'. Available: {avail}"

    try:
        return await handler(workspace, pipe_data, sub_args, kwargs, ctx)
    except MdioError:
        raise
    except Exception as e:
        return f"- mdio_error: [{ErrCode.INTERP_CRASH}] worktree {sub}: {e}"


# Legacy snap() wrapper (deprecated)

async def snap(workspace, pipe_data, args, kwargs, ctx):
    """DEPRECATED: use `worktree commit` instead.

    Still works on genesis using the 'main' branch.
    All sub-flags continue to work.
    """
    print("[mdio] snap is deprecated, use 'worktree' instead", flush=True)

    # snap() always operates on genesis — force genesis ctx
    from dataclasses import replace
    ctx = replace(ctx, wt_uid="genesis", worktree_name=None)

    target = kwargs.get("--list") or kwargs.get("-l")
    if target is not None and not isinstance(target, bool):
        kwargs["--list"] = True
    show_id = kwargs.get("--show") or kwargs.get("-s")
    if show_id is not None and not isinstance(show_id, bool):
        kwargs["--show"] = True
        if not args:
            args = [show_id]
    diff_val = kwargs.get("--diff") or kwargs.get("-d")
    restore_id = kwargs.get("--restore") or kwargs.get("-r")
    if restore_id is not None and not isinstance(restore_id, bool):
        kwargs["--restore"] = True
        if not args:
            args = [restore_id]
    list_val = kwargs.get("--list") or kwargs.get("-l")
    if list_val is not None and not isinstance(list_val, bool):
        kwargs["--list"] = True
        if not args:
            args = [list_val]

    try:
        if "--verify" in kwargs:
            return await _snap_verify(workspace, pipe_data, args, kwargs, ctx)
        elif "--gc" in kwargs:
            return await _snap_gc(workspace, pipe_data, args, kwargs, ctx)
        elif "--status" in kwargs:
            return await _snap_status(workspace, pipe_data, args, kwargs, ctx)
        elif "--log" in kwargs:
            return await _snap_log(workspace, pipe_data, args, kwargs, ctx)
        elif "--undo" in kwargs:
            return await _snap_undo(workspace, pipe_data, args, kwargs, ctx)
        elif "--del" in kwargs:
            return await _snap_del(workspace, pipe_data, args, kwargs, ctx)
        elif "--list" in kwargs:
            return await _snap_list(workspace, pipe_data, args, kwargs, ctx)
        elif "--diff" in kwargs:
            return await _snap_diff(workspace, pipe_data, args, kwargs, ctx)
        elif "--show" in kwargs:
            return await _snap_show(workspace, pipe_data, args, kwargs, ctx)
        elif "--restore" in kwargs:
            return await _snap_restore(workspace, pipe_data, args, kwargs, ctx)
        else:
            return await _snap_create(workspace, pipe_data, args, kwargs, ctx)
    except MdioError as e:
        return e.to_markdown()
    except Exception as e:
        return f"- mdio_error: [{ErrCode.SNAP_CKPT_ERR}] {e}"
