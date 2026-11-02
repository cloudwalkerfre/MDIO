"""Shared worktree operations for HTTP and WebSocket transports."""

from __future__ import annotations

import asyncio
from typing import Any


async def dispatch_worktree(runtime, method: str, params: dict[str, Any],
                            self_uid: str = "genesis") -> dict[str, Any]:
    """Dispatch a worktree method with the caller's current worktree as scope."""
    workspace = runtime._workspace
    op = str(method or "").removeprefix("worktree.")
    params = dict(params or {})
    self_uid = self_uid or "genesis"

    if op == "load":
        path = str(params.get("path") or "").strip()
        if not path:
            raise ValueError("worktree.load requires path")
        rule = str(params.get("rule") or "")
        name = str(params.get("name") or "")
        parent_ref = str(params.get("parent") or self_uid)
        parent_uid = workspace.resolve_worktree_ref(parent_ref)
        if not workspace.is_descendant_or_self(parent_uid, self_uid):
            raise ValueError(
                f"parent '{parent_ref}' must be self or a descendant of {self_uid}"
            )
        uid, is_new = await asyncio.to_thread(
            workspace.load_worktree, path, rule, name, parent_uid
        )
        return {"uid": uid, "is_new": is_new, "path": path, "parent": parent_uid}

    if op == "offload":
        selectors = [
            str(params.get("wt") or "").strip(),
            str(params.get("id") or "").strip(),
            str(params.get("path") or "").strip(),
        ]
        chosen = [value for value in selectors if value]
        if len(chosen) != 1:
            raise ValueError("worktree.offload requires exactly one of wt, id, path")
        target = chosen[0]
        if selectors[1]:
            uid = workspace.resolve_worktree_ref(selectors[1])
        elif selectors[2]:
            uid = workspace.match_worktree(selectors[2])
            if uid is None:
                raise ValueError(f"worktree not found: {selectors[2]}")
        else:
            uid = workspace.resolve_worktree_ref(selectors[0])
        if uid == "genesis":
            raise ValueError("genesis cannot be offloaded")
        if uid == self_uid or not workspace.is_descendant_or_self(uid, self_uid):
            raise ValueError(
                f"offload target '{target}' must be a descendant of {self_uid}"
            )
        await workspace.dispose_plugins_wt(uid, strict=True)
        await asyncio.to_thread(workspace.offload_worktree, uid)
        return {"uid": uid, "target": target}

    if op == "list":
        rows = []
        for uid, name, _alias, project_root in runtime.store.list_worktrees():
            rows.append({"uid": uid, "name": name, "path": project_root or ""})
        return {"worktrees": rows}

    if op == "current":
        return {
            "uid": workspace.active_worktree_uid,
            "name": workspace.active_worktree_name or "genesis",
        }

    raise ValueError(f"unsupported worktree method: {method}")
