"""RuleManager — rule 的 Python 侧薄封装。

Rust FFI + worktrees.md 写回 (selector + />, Session 通道) + Gateway 联动。
对标 hook/snap/_append_worktree_entry 的 write_locator 范式。
"""
from __future__ import annotations
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from mdio_py.gateway.store import GatewayStore

class RuleManager:
    """rule 的 Python 侧薄封装。"""

    def __init__(self, store, gateway_mgr=None, workspace=None):
        self._store = store
        self._gateway_mgr = gateway_mgr
        self._workspace = workspace

    def remote_entries(self, wt_uid: str) -> list[tuple[str, str]]:
        return self._store.rule_remote_entries(wt_uid)

    def add_remote(self, wt_uid: str, path: str, entry_id: str):
        self._store.rule_add_remote(wt_uid, path, entry_id)
        if self._workspace is not None:
            self._sync_rule_to_file(wt_uid)
            self._store.tree_ensure()
        if self._gateway_mgr is not None:
            self._gateway_mgr.reconcile(wt_uid)

    def remove_remote(self, wt_uid: str, path: str):
        self._store.rule_remove_remote(wt_uid, path)
        if self._workspace is not None:
            self._sync_rule_to_file(wt_uid)
            self._store.tree_ensure()
        if self._gateway_mgr is not None:
            self._gateway_mgr.reconcile(wt_uid)

    def get_type(self, wt_uid: str) -> str:
        remote = self._store.rule_remote_entries(wt_uid)
        if remote:
            return "project"
        for w in self._store.list_worktrees():
            if w[0] == wt_uid:
                if w[3]:
                    return "project"
                break
        return "bfork"

    # ── worktrees.md 写回 ──

    def _sync_rule_to_file(self, wt_uid: str):
        """把该 WT 的 rule 写回 worktrees.md（delegate 到唯一实现）。

        不用 selector tag_eq("rule")：rule="" 的条目没有 - rule: 行，tag 匹配会未命中
        并把整个文件写坏（WT 消失）；这里 delegate 到 write_rule_to_file。
        """
        if self._workspace is None:
            return
        from mdio_py.std.worktree_ops import write_rule_to_file
        write_rule_to_file(self._workspace, self._store, wt_uid)
