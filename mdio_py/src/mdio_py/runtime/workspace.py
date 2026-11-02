import json
import logging
import os
from typing import Any, List
from mdio_py.plugins import register_plugins, PluginBase
from mdio_py.core.error import MdioError, ErrCode
from mdio_py.core.events import HookManager


MAX_WORKTREE_LEVELS = 8


def _default_proj(ir, proj):
    """tag-target ⇒ content 默认投影（唯一实现）；否则透传。"""
    if (proj is None or proj == "raw") and hasattr(ir, 'is_tag_target') and ir.is_tag_target():
        return "content"
    return proj

class Workspace:
    def __init__(self, storage_root: str, mrc_module,
                 doc_subdir: str = None, wal_rel: str = None):
        # 布局名一律从坐标文件取（单一来源）；没有 `wt/` 时直接报错，不兜底。
        from mdio_py.runtime.worktree_paths import doc_subdir_for, wal_rel_for
        doc_subdir = doc_subdir or doc_subdir_for(storage_root)
        wal_rel = wal_rel or wal_rel_for(storage_root)
        # core 只认 1 个布局参数时回落到单参构造。
        try:
            self._store = mrc_module.ProjectStore(storage_root, doc_subdir, wal_rel)
        except TypeError:
            self._store = mrc_module.ProjectStore(storage_root)
        self._temp_store = None
        # force WAL recovery for worktrees.md, then rebuild tree
        try:
            self._store.get_text("genesis", "/.mds/worktrees.md")
        except Exception:
            pass
        self._store.tree_ensure()
        # create Backends and load files for discovered project WTs
        try:
            self._store.restore_project_worktrees()
        except Exception:
            pass
        self._plugins = {}
        self.active_tasks: dict[str, dict] = {}        # key: str (wt_uid)
        self.active_worktree_name: str | None = None
        self.active_worktree_uid: str = "genesis"    # str (uid)
        register_plugins(self)
        self.hook_manager = HookManager(self)

    def register_plugin(self, plugin: PluginBase):
        self._plugins[plugin.protocol_name] = plugin

    def get_plugin(self, protocol_name: str) -> Any:
        return self._plugins.get(protocol_name)

    async def dispose_plugins_wt(self, wt_uid: str, *, strict: bool = False) -> None:
        errors: list[BaseException] = []
        for plugin in list(self._plugins.values()):
            fn = getattr(plugin, "dispose_wt", None)
            if fn is None:
                continue
            try:
                await fn(self, wt_uid)
            except Exception as e:
                errors.append(e)
                print(f"⚠️ [dispose] plugin {getattr(plugin, 'protocol_name', '?')} wt={wt_uid[:8]} failed: {e}")
        if errors and strict:
            raise ExceptionGroup(f"plugin dispose failed for wt {wt_uid}", errors)

    @property
    def store(self):
        return self._store

    @property
    def temp_store(self):
        """One in-memory document store per runtime for /.tmp/<exec_id>/ docs."""
        if self._temp_store is None:
            import mdio_py.mdio_core as mrc
            self._temp_store = mrc.ProjectStore("/mem")
        return self._temp_store

    def resolve_document(self, wt_uid: str, vfs_path: str):
        """Return (store, wt_uid, path) for a document address.

        Only /.tmp/ is routed to the in-memory temp store.  Temp docs must
        live under the current exec frame's root; a missing/foreign frame is
        rejected rather than falling back to VFS.
        """
        path = str(vfs_path or "")
        if not path.startswith("/.tmp/"):
            return self._store, wt_uid, vfs_path

        from mdio_py.core.mem_slot import current_temp_frame
        frame = current_temp_frame()
        if frame is None:
            raise MdioError(
                "interpreter", ErrCode.INTERP_CRASH,
                f"临时文档路径没有当前 exec frame: {path}",
            )
        if not (path.startswith(frame.root) or path == frame.root.rstrip("/")):
            raise MdioError(
                "interpreter", ErrCode.INTERP_CRASH,
                f"临时文档路径不在当前 exec frame 下: {path}",
            )
        return self.temp_store, "genesis", path

    def worktree_level(self, wt_uid: str) -> int:
        """层数：genesis 为 1，子 wt 依次 +1。"""
        if not wt_uid or wt_uid == "genesis":
            return 1
        level = 1
        cur = wt_uid
        seen = set()
        while cur and cur != "genesis" and cur not in seen:
            seen.add(cur)
            parent = self._store.parent_uid(cur)
            if not parent:
                break
            level += 1
            cur = parent
        return level

    def ensure_can_create_child(self, parent_wt: str) -> None:
        """创建子 wt 前检查层数；只允许到 MAX_WORKTREE_LEVELS。"""
        level = self.worktree_level(parent_wt)
        if level + 1 > MAX_WORKTREE_LEVELS:
            raise ValueError(
                f"worktree level limit {MAX_WORKTREE_LEVELS} exceeded "
                f"(parent level {level})"
            )

    def is_descendant_or_self(self, wt_uid: str, ancestor: str) -> bool:
        """wt_uid 是否等于 ancestor 或位于 ancestor 的子树内。"""
        if not wt_uid or not ancestor:
            return False
        cur = wt_uid
        seen = set()
        while cur and cur not in seen:
            if cur == ancestor:
                return True
            seen.add(cur)
            cur = self._store.parent_uid(cur)
        return False

    def resolve_worktree_ref(self, ref: str) -> str:
        """把 wt 名称、uid 或本地路径解析成 uid；空串/genesis 返回 genesis。"""
        text = str(ref or "").strip()
        if not text or text == "genesis":
            return "genesis"
        if self._store.has_worktree(text):
            return text
        entry = self.resolve_worktree_entry(text)
        if entry and entry.get("uid"):
            return str(entry["uid"])
        if text.startswith("/"):
            uid = self.match_worktree(text)
            if uid:
                return uid
        raise ValueError(f"worktree not found: {text}")

    def derive_worktree(self, parent_wt: str, mnt_rules: str, name: str = "") -> str:
        """Derive a worktree (rule-defined). Returns wt_uid (uid string).

        name: 可选 worktree 名（worktree bfork 命令名传入；缺省空串）。
        """
        parent_uid = self.resolve_worktree_ref(parent_wt)
        self.ensure_can_create_child(parent_uid)
        uid = self._store.derive_worktree(parent_uid, name, mnt_rules)
        return uid

    def drop_derived_worktree(self, wt_uid: str, clean_files: bool = False):
        """Drop a derived worktree. Cleans sandbox tasks, hooks, arena, sessions, VFS mem."""
        sandbox_tasks = self.active_tasks.pop(wt_uid, {})
        for pid, task in sandbox_tasks.items():
            task.cancel()
        try:
            self._store.clear_worktree_hooks(wt_uid)
            self._store.drop_worktree([wt_uid])
        except Exception:
            pass
        self._reset_active_if_dropped([wt_uid])
        gs = getattr(self, '_gateway_store', None)
        if gs is not None:
            gs.deactivate_wt(wt_uid)

    def drop_worktree(self, wt_uids: list[str]):
        """删除 WT 及其子孙。"""
        try:
            self._store.drop_worktree(wt_uids)
        except Exception:
            pass
        self._reset_active_if_dropped(wt_uids)
        # 删除 WT → 拆除 gateway 投影
        gs = getattr(self, '_gateway_store', None)
        if gs is not None:
            for uid in wt_uids:
                gs.deactivate_wt(uid)

    def _reset_active_if_dropped(self, dropped_wt_uids: list[str]):
        """Reset active_worktree_uid if it was dropped."""
        if self.active_worktree_uid in dropped_wt_uids:
            self.active_worktree_name = None
            self.active_worktree_uid = "genesis"

    def read_locator(self, wt_uid: str, vfs_path: str, ir, proj=None):
        try:
            proj = _default_proj(ir, proj)
            store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
            return store.read_locator(doc_wt, doc_path, ir, proj=proj)
        except MdioError:
            raise
        except Exception as e:
            raise MdioError("rust", ErrCode.RUST_IO, f"Read blocked({vfs_path}): {e}", cause=e)

    def query_memory(self, text: str, ir, proj=None):
        try:
            proj = _default_proj(ir, proj)
            return self._store.query_memory(text, ir, proj=proj)
        except MdioError:
            raise
        except Exception as e:
            raise MdioError("rust", ErrCode.RUST_IO, f"Memory Query Failed: {e}", cause=e)

    def query_memory_ranges(self, text: str, ir, proj=None):
        """map 算子用:文本 → selector 命中节点 (字节区间, 投影面值) 列表。"""
        try:
            proj = _default_proj(ir, proj)
            return self._store.query_memory_ranges(text, ir, proj=proj)
        except MdioError:
            raise
        except Exception as e:
            raise MdioError("rust", ErrCode.RUST_IO, f"Memory Range Query Failed: {e}", cause=e)

    def write_locator(self, wt_uid: str, vfs_path: str, ir, op: str, text: str, auto_scaffold: bool = True, god: bool = False, auto_level: bool = False, suppress_hooks: bool = False, proj=None):
        proj = _default_proj(ir, proj)
        if str(vfs_path or "").startswith("/.tmp/"):
            try:
                store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
                # Named-mem semantics: a write with no selector hit is an error.
                if not store.read_locator(doc_wt, doc_path, ir, proj=None):
                    raise MdioError(
                        "rust", ErrCode.RUST_IO,
                        f"目标节点不存在: {vfs_path}",
                    )
                store.write_locator(doc_wt, doc_path, ir, op, text,
                                    True, True, auto_level, True, proj=proj)
                return b""
            except MdioError:
                raise
            except Exception as e:
                raise MdioError("rust", ErrCode.RUST_IO, f"Write blocked({vfs_path}): {e}", cause=e)
        # HS + patch granularity=proj 的字段写是**标准 6902 路径形态**
        # （{"op":"replace","path":"<元素路径>/<字段名>","value":…}）；标题结构变化等窄例外
        # 仍会回退整块 replace，写入口提示该边界。
        if (
            os.environ.get("MDIO_PROJ_MODE") == "hs"
            and os.environ.get("MDIO_PATCH_GRANULARITY", "").lower() == "proj"
        ):
            logging.getLogger("mdio").info(
                "MDIO_PROJ_MODE=hs 与 MDIO_PATCH_GRANULARITY=proj 同时启用："
                "字段写走标准路径形态（replace <元素路径>/<字段名>）；"
                "标题结构变化等窄例外回退整块 replace。"
            )
        # 服务端内部耗时: exec 进入 Rust 执行(含 diff/P-patch 生成) → packet 返回 → 广播触发
        from mdio_py.telemetry import _TelemetrySpan as _wl_span
        with _wl_span(self._store, "rust_to_patch_out"):
            try:
                result = self._store.write_locator(wt_uid, vfs_path, ir, op, text, auto_scaffold, god, auto_level, suppress_hooks, proj=proj)
            except MdioError:
                raise
            except Exception as e:
                raise MdioError("rust", ErrCode.RUST_IO, f"Write blocked: {e}", cause=e)
            doc_id, packet = result

            # GatewayStore 回调——收束为一行
            gateway_store = getattr(self, '_gateway_store', None)
            if gateway_store and doc_id:
                gateway_store.on_document_written(wt_uid, vfs_path, doc_id)

            # 广播 CRDT 更新 —— 走唯一出口（kind=TextOp，origin=None：服务端写不抑制任何 peer）
            if packet:
                async def _broadcast():
                    from mdio_py.gateway.crdt_scope import deliver, KIND_TEXT
                    with _wl_span(self._store, "gw_broadcast", doc_id=doc_id):
                        await deliver(getattr(self, "_gateway_store", None) or self, doc_id, packet,
                                      KIND_TEXT, store=self._store)
                try:
                    import asyncio
                    asyncio.get_running_loop().create_task(_broadcast())
                except Exception:
                    pass
        return packet

    def path_exists(self, wt_uid: str, vfs_path: str) -> bool:
        store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
        return store.path_exists(doc_wt, doc_path)

    def head_exists(self, wt_uid: str, vfs_path: str, ir) -> bool:
        store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
        return store.head_exists(doc_wt, doc_path, ir)

    def rm(self, wt_uid: str, vfs_path: str, recursive: bool, force: bool, verbose: bool):
        store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
        return store.rm(doc_wt, doc_path, recursive, force, verbose)

    def mk(self, wt_uid: str, vfs_path: str, parents: bool, verbose: bool):
        store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
        return store.mk(doc_wt, doc_path, parents, verbose)

    def resolve_worktree_entry(self, name: str):
        """全仓唯一「wt 名 → 条目」解析。返回条目 dict 或 None。"""
        from mdio_py.std.worktree_ops import _parse_worktree_entries
        for uid, _n, _s, _e in self._store.list_worktrees():
            for e in _parse_worktree_entries(self, uid):
                if e.get("name") == name:
                    return e
        return None

    def resolve_worktree_name(self, name: str) -> str | None:
        """name → uid（resolve_worktree_entry 的薄壳）。"""
        entry = self.resolve_worktree_entry(name)
        return entry.get("uid") if entry else None

    def read_section_chunks(self, wt_uid: str, vfs_path: str, heading: str, *, children: bool = False) -> List[str]:
        """裸块读取的显式化：raw 投影——自解析 KV 用。

        children=False：drill(heading)（节自身，含其内容行——CF 拓扑下节的正文归它）；
        children=True ：drill(heading).any_child()（直接子块）。
        值读取请走 read_locator（tag-target 默认 content）。
        """
        from mdio_py.core.selector_builder import SelectorBuilder
        b = SelectorBuilder().drill(heading)
        if children:
            b.any_child()
        return self.read_locator(wt_uid, vfs_path, b.build(), proj="raw")

    def read_full_text(self, wt_uid: str, vfs_path: str) -> str:
        """全文读（唯一实现）：get_text + 异常兜空。"""
        try:
            store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
            return store.get_text(doc_wt, doc_path) or ""
        except MdioError:
            raise
        except Exception:
            return ""

    def to_json_text(self, text: str) -> str:
        """文本 → 整档 JSON 信封（复用既有 FFI parse_to_ast）。

        边界：引擎在“末行是 `- k:`（空值）且**无尾换行**”时投影会越界 panic
        （实测 `byte index out of bounds`），而 PanicException 不是
        Exception 子类、调用方 `except Exception` 兜不住。VFS 落盘文本恒以换行结尾，
        只有字面量管道可能缺尾换行 ⇒ 这里补一个（对 VFS 文本是恒等变换）。
        """
        if text and not text.endswith("\n"):
            text += "\n"
        return self._store.parse_to_ast(text)

    def to_ast_json_doc(self, wt_uid: str, vfs_path: str) -> str:
        """文件 → 整档 JSON 信封（薄封装既有 FFI to_ast_json）；取不到内容即报错"""
        store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
        raw = store.to_ast_json(doc_wt, doc_path)
        if not raw:
            raise ValueError(f"no AST for {vfs_path}")
        return raw

    def expand_path_pattern(self, wt_uid: str, virtual_pattern: str, include_hidden: bool = False) -> List[str]:
        from mdio_py.core import split_selector_addr
        addr = split_selector_addr(virtual_pattern)   # 统一切分入口
        if addr.file_path is not None:
            virtual_pattern = addr.file_path
        try:
            store, doc_wt, _ = self.resolve_document(wt_uid, virtual_pattern)
            return store.expand_glob(doc_wt, virtual_pattern, include_hidden)
        except MdioError:
            raise
        except Exception:
            return []

    def run_find(self, wt_uid: str, vfs_path: str, ir, query_configs: list, ctx_b: int, ctx_a: int):
        store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
        return store.run_find(doc_wt, doc_path, ir, query_configs, ctx_b, ctx_a)

    def run_tree(self, wt_uid: str, vfs_path: str, ir, depth: int, detail: int):
        store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
        return store.run_tree(doc_wt, doc_path, ir, depth, detail)

    def heading_at_line(self, wt_uid: str, vfs_path: str, line_no: int) -> str:
        store, doc_wt, doc_path = self.resolve_document(wt_uid, vfs_path)
        return store.heading_at_line(doc_wt, doc_path, line_no)

    # ── Project worktree lifecycle ──────────────────────────

    def register_worktree_at(self, parent_wt: str, project_root: str,
                              scheme: str = "fs", name: str = "project",
                              rule: str = "", doc_subdir: str = None,
                              wal_rel: str = None) -> str:
        """Register a new worktree from an independent Backend root. Returns wt_uid (uid).

        project_root: **用户项目根**（= worktrees.md 的 path 字段；mdio 可 load 任意多个）。
        rule: 可选子侧挂载(父 VFS 路径段,与 bfork 同语法;空串=无绑定)。
        布局名：调用方没给就从坐标文件取（单一来源；core 不认识 wt/.wal 这些名字）。
        """
        parent_uid = self.resolve_worktree_ref(parent_wt)
        self.ensure_can_create_child(parent_uid)
        if not doc_subdir or not wal_rel:
            from mdio_py.runtime.worktree_paths import doc_subdir_for, wal_rel_for
            doc_subdir = doc_subdir or doc_subdir_for(project_root)
            wal_rel = wal_rel or wal_rel_for(project_root)
        try:
            uid = self._store.register_worktree(parent_uid, name, project_root, scheme, rule,
                                                doc_subdir, wal_rel)
        except TypeError:   # core 只认单参构造
            uid = self._store.register_worktree(parent_uid, name, project_root, scheme, rule)
        return uid

    def match_worktree(self, project_root: str) -> str | None:
        """search all WTs' worktrees.md for the longest prefix match.
        project_root: 用户项目根（匹配 worktrees.md 的 path 字段）。"""
        import os
        from mdio_py.std.worktree_ops import _parse_worktree_entries

        def _is_subpath(parent: str, child: str) -> bool:
            p = parent.rstrip('/')
            c = child.rstrip('/')
            if p == c:
                return True
            if c.startswith(p) and c[len(p):len(p)+1] == '/':
                return True
            return False

        normalized_root = os.path.realpath(project_root)
        best_uid = None
        best_path_len = 0
        for uid, _n, _s, _e in self._store.list_worktrees():
            for e in _parse_worktree_entries(self, uid):
                entry_path = e.get("path", "")
                if not entry_path:
                    continue
                if _is_subpath(entry_path, normalized_root):
                    if best_uid is None or len(entry_path) > best_path_len:
                        best_uid = e.get("uid", "")
                        best_path_len = len(entry_path)
        return best_uid

    def load_worktree(self, project_root: str, rule: str = "",
                      name: str = "", parent_wt: str = "genesis") -> tuple[str, bool]:
        """Load a worktree for the given **用户项目根**（= worktrees.md 的 path 字段）.
        rule: 可选子侧挂载(注册新 WT 时生效;已存在 WT 不受影响)。
        name: 可选条目名(仅注册新 WT 时用);缺省沿用项目根 basename。
        parent_wt: 父 wt 名称/uid/本地路径；缺省 genesis。
        Returns (wt_uid, is_new). is_new=True means a new worktree was registered."""
        import os
        from mdio_py.std.worktree_ops import (_parse_worktree_entries,
                                               _format_worktree_entry, _append_worktree_entry,
                                               WORKTREES_MD, GENESIS_WT)

        parent_id = self.resolve_worktree_ref(parent_wt)
        normalized_root = os.path.realpath(project_root)
        if not os.path.isdir(normalized_root):
            raise ValueError(f"worktree root does not exist: {normalized_root}")
        existing = self.match_worktree(normalized_root)
        entries = _parse_worktree_entries(self)
        if existing is not None:
            try:
                # 先 flush:tree_ensure 从 vfs_op 读 worktrees.md,新内容此刻只在 session
                self._store.flush_worktree_sessions("genesis")
                self._store.tree_invalidate()
                self._store.tree_ensure()
                self._store.restore_project_worktrees()
            except Exception:
                pass
            existing_parent = self._store.parent_uid(existing) or "genesis"
            if existing_parent != parent_id:
                raise ValueError(
                    f"worktree path already loaded under {existing_parent}: {normalized_root}"
                )
            return (existing, False)

        self.ensure_can_create_child(parent_id)

        name = name or os.path.basename(normalized_root.rstrip('/')) or "default"
        # genesis 是 synthetic 根，不在 worktrees.md 里；名字唯一性必须把它算进去，
        # 否则加载 basename=genesis 的项目会写出第二个 ## genesis，重启重建树时自我递归。
        existing_names = {GENESIS_WT} | {e.get("name", "") for e in entries}
        base_name = name
        counter = 1
        while name in existing_names:
            name = f"{base_name}_{counter}"
            counter += 1

        # 项目入口直接使用项目根；布局恒定（文档 `<根>/wt`、运行态 `<根>/.wal`），并**写进条目**
        from mdio_py.runtime.worktree_paths import doc_subdir_for, wal_rel_for
        _dsub = doc_subdir_for(normalized_root)
        _wrel = wal_rel_for(normalized_root)
        new_wt_uid = self.register_worktree_at(parent_id, normalized_root, "fs", name=name, rule=rule,
                                               doc_subdir=_dsub, wal_rel=_wrel)
        _append_worktree_entry(self, name, new_wt_uid,
                               parent_wt=parent_id, path=normalized_root, rule=rule,
                               doc_subdir=_dsub, wal_rel=_wrel)
        self.active_worktree_name = name
        self.active_worktree_uid = new_wt_uid
        return (new_wt_uid, True)

    def offload_worktree(self, wt_uid: str) -> None:
        """flush → Backend.offload → drop_worktree → 删 worktrees.md 条目。"""
        from mdio_py.std.worktree_ops import (_parse_worktree_entries, _delete_worktree_entry)
        if wt_uid == "genesis":
            return
        active_was_removed = (
            self.active_worktree_uid == wt_uid
            or self.is_descendant_or_self(self.active_worktree_uid, wt_uid)
        )
        try:
            self._store.offload_worktree(wt_uid)
        except Exception as e:
            raise MdioError("rust", ErrCode.RUST_IO, f"Offload failed: {e}", cause=e)
        for e in _parse_worktree_entries(self):
            if e.get("uid") == wt_uid:
                _delete_worktree_entry(self, e["name"])
                break
        if active_was_removed:
            self.active_worktree_name = None
            self.active_worktree_uid = "genesis"
