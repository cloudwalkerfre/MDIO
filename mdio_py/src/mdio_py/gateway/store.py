"""GatewayStore — 生命周期管理器。

遵循 MDIO 通用模式：gateway.md + rule 段 → reconcile() → Active + Handler。
"""

from __future__ import annotations

import asyncio
import base64
import os      # 【实验计时】MDIO_FRAME_TIMING 门控
import time    # 【实验计时】perf_counter 差值
import json
import logging
import random
import string
import struct
import time
import zlib
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from mdio_py import mdio_core as _mrc
    from mdio_py.gateway.crdt_scope import CrdtScope
    from mdio_py.gateway.iroh_transport import ConnectionHandler
    from mdio_py.gateway.types import Active as _Active, RemoteDecl as _RD

# 广播打点（与 crdt_scope 同一个 logger，默认 WARNING 级别：不打）——
# 这是 INFO 级诊断，排错时把 level 调成 logging.INFO 即可看到丢弃记录。
_bcast_log = logging.getLogger("mdio.bcast")
_bcast_log.setLevel(logging.WARNING)

HEADER_SIZE = 13
UP_FILE = 0x06
TICK_INTERVAL = 5  # 秒
EXPIRY_DAYS = 7


def doc_channel(doc_id: int) -> str:
    """文档帧的通道名：与「订阅登记在文档上、投递不查 wt」同构（方案 §5）。"""
    return f"doc:{int(doc_id)}"


def _kv_from_md_text(text: str, *, workspace=None, store=None) -> dict:
    """gateway.md 片段 → {键: 文本}（新路径：tojson 信封 + bullets_to_kv）。

    三档（写清是为了对拍可解释，见 tests/test_gateway_kv_migration.py）：
      ① workspace 在 → workspace.to_json_text(text)（runtime/workspace.py:209，**新路径**）；
      ② 只有 store   → store.parse_to_ast(text)（同一引擎出口的等价 FFI，离线装配用）；
      ③ 两者都没有   → 旧 parse_kv_lines（只服务 _parse 的 /mem 装配——该路已搁置、
                       按业主要求不动，故保留旧语义）。
    旧实现永不抛；引擎/JSON 异常在这里退化成空表，不让 reconcile 整轮炸掉。
    """
    from mdio_py.core.ast_utils import bullets_to_kv
    if workspace is None and store is None:
        from mdio_py.core.text_utils import parse_kv_lines   # ③ 旧语义（搁置路径专用）
        return parse_kv_lines(text)
    try:
        raw = (workspace.to_json_text(text) if workspace is not None
               else store.parse_to_ast(text))
        return bullets_to_kv(json.loads(raw))
    except Exception:
        return {}


class GatewayStore:
    """Gateway 的生命周期管理器。

    只响应 MD 文件变化，不响应运行时状态变化。
    Handler 自主管连接/重连，Active.state 是纯投影。
    """

    def __init__(self, store, workspace, iroh_endpoint=None):
        self._store = store
        self._workspace = workspace
        self._ep = iroh_endpoint   # 可为 None（startup 阶段），step 6 后更新
        self._actives: dict[str, "_Active"] = {}
        self._scopes: dict[str, "CrdtScope"] = {}
        # ── 步骤 2：投递索引（两张表）────────────────────────────
        # _peers      : peer_id → Peer（投递用；peer 的 wt 与投递无关）
        # _doc_subs   : doc_id  → {peer_id}（订阅登记在**文档**上）
        # CrdtScope 保留为「wt 分域索引」（peers/subscriptions），由下面 4 个方法双写维持——
        # _release_scope 的 ws_ 守卫、iroh 的 doc_map 传播等仍按 wt 视图读它。
        self._peers: dict[str, object] = {}
        self._doc_subs: dict[int, set[str]] = {}
        # ── R6 步骤 1：通道侧订阅句柄 ─────────────────────────────
        # 投递已交给 Litestar 通道（publish 一次、零 await）；这里只持有
        # 「(peer, doc) → Subscriber 上下文」句柄，**投递路径永不读它**，
        # 因此它坏掉也不会堵住任何一帧（通道内订阅与连接生命周期一致）。
        self._channels = None                       # ChannelsPlugin（attach_gateway 注入）
        self._chan_ctx: dict[tuple[str, int], object] = {}
        self._chan_skip = 0                         # §4.4#2：订阅被静默跳过的次数（loud 用）
        self._reconcile_enabled = True  # unmount/unshare 期间临时关掉
        self._tick_task = None          # attach_gateway 设置
        self._ast_task = None           # attach_gateway 设置
        self._shutting_down = False

    # 查询

    def get_scope(self, wt_uid: str) -> "CrdtScope | None":
        return self._scopes.get(wt_uid)

    def has_scope(self, wt_uid: str) -> bool:
        return wt_uid in self._scopes

    # 步骤 2：登记/注销与订阅——**唯一的 4 个写口**（同时维护投递索引与 wt 分域索引）
    # 调用方不再直接碰 _scopes[*].peers / .subscriptions。

    def attach_peer(self, peer, scope=None) -> None:
        """登记一个 peer（WS 或 iroh）。scope = 它的 wt 分域（可省略）。"""
        self._peers[peer.peer_id] = peer
        if scope is not None and peer.peer_id not in scope.peers:
            scope.add_peer(peer)

    def detach_peer(self, peer_id: str, scope=None) -> None:
        """摘一个 peer：投递表 + 它在所有文档上的订阅 + wt 分域（scope 给定时）。"""
        self._peers.pop(peer_id, None)
        for subs in self._doc_subs.values():
            subs.discard(peer_id)
        for s in self._scopes.values():
            s.remove_peer(peer_id)
        if scope is not None:
            scope.remove_peer(peer_id)

    def subscribe(self, peer_id: str, doc_id: int, scope=None) -> None:
        """订阅：登记在**文档**上（投递索引），同时更新 wt 分域（给定时）。"""
        self._doc_subs.setdefault(int(doc_id), set()).add(peer_id)
        if scope is not None:
            scope.subscribe(peer_id, int(doc_id))

    def unsubscribe(self, peer_id: str, doc_id: int, scope=None) -> None:
        subs = self._doc_subs.get(int(doc_id))
        if subs:
            subs.discard(peer_id)
            if not subs:
                self._doc_subs.pop(int(doc_id), None)
        if scope is not None:
            scope.unsubscribe(peer_id, int(doc_id))

    async def deliver(self, doc_id: int, frame: bytes, kind: str, origin: str | None = None) -> str:
        """**唯一投递出口**（步骤 2 版）：doc → {peer} → 各自的连接。**不查 wt。**

        结局："queued:N"（已入队 N 条连接，**不代表对端已应用**）/ "no_subscribers"。
        origin 只接受真实 peer id 或 None（伪 id 一律传 None）。
        """
        if self._channels is None:
            print(f"⚠️ [DELIVER] 通道插件未注入 ⇒ {len(frame)}B kind={kind} 无法投递", flush=True)
            return "no_channel"
        # §4.4#1（评审 R4-H1）：通道化时把"无订阅者"的判空丢了 ✗ —— 旧出口本来有它
        # （crdt_scope.py:53-56 的 [BCAST-MISS] + no_subscribers）。这里照旧分支补回来：
        # 否则 publish 到空通道 = 零痕迹丢弃，却仍报 published（违反"绝不静默"）。
        if not (self._doc_subs.get(int(doc_id)) or ()):
            _bcast_log.info("[BCAST-MISS] doc=%s kind=%s ⇒ 无订阅者，%dB 丢弃",
                            int(doc_id), kind, len(frame))
            return "no_subscribers"
        # 一次 publish：同步入队每个订阅者自己的流（§3.2 实证 put_nowait）⇒
        # 出口里没有循环、没有 await、没有任何 peer 能被别人等住（方案 §4.2）。
        self._channels.publish(frame, [doc_channel(int(doc_id))])
        # 【实验计时】「收到 exec/MC_UP 帧 → PJ 转发出去」的网关内间隔（env 门控 MDIO_FRAME_TIMING=1）
        try:
            if os.environ.get("MDIO_FRAME_TIMING") == "1":
                _t0 = getattr(self, "_ft_in_at", None)
                if _t0 is not None:
                    _now = time.perf_counter()
                    _d = (_now - _t0) * 1000
                    if _d < 1000:
                        # 【实验计时 T2】PJ 转发（Unique 出口）一路：in_to_deliver=入引擎→转发，
                        # rx_to_deliver=收帧→转发（含准入段与引擎），id 与 [FT] rx/eng/tx 同一帧。
                        _rx = getattr(self, "_ft_rx", 0.0)
                        _rid = getattr(self, "_ft_id", None)
                        _rx_ms = f" rx_to_deliver={(_now - _rx) * 1000:.2f}ms" if _rx else ""
                        print(f"[FT] pj id={_rid} doc={int(doc_id)} kind={kind} in_to_deliver={_d:.2f}ms{_rx_ms} t={time.time():.6f}", flush=True)
        except Exception:
            pass
        return "published"

    # R6 步骤 1：通道侧订阅（框架订阅者 + 每连接写任务）
    #   句柄只在这里被持有；投递（publish）完全不读它 ⇒ 僵尸/慢消费者
    #   无法再阻塞别人的帧；连接断开时由调用方 await 退订即自动停写任务。

    async def channel_subscribe(self, peer_id: str, doc_id: int) -> None:
        """把一个已登记的 peer 接进该文档的通道。幂等。"""
        plugin = self._channels
        peer = self._peers.get(peer_id)
        # §4.4#2：这里不静默 return ✗ —— "登记了却接不通"最难查，
        # 而此处正是每连接订阅的主路径（WS crdt.py / iroh / mount 三处都走它）。
        if plugin is None:
            self._chan_skip += 1
            print(f"⚠️ [CHAN] 订阅失败 peer={peer_id} doc={doc_id}：通道插件未注入（跳过 #{self._chan_skip}）", flush=True)
            return
        if peer is None:
            self._chan_skip += 1
            print(f"⚠️ [CHAN] 订阅失败 peer={peer_id} doc={doc_id}：peer 未登记 ⇒ 该连接收不到帧（跳过 #{self._chan_skip}）", flush=True)
            return
        key = (peer_id, int(doc_id))
        if key in self._chan_ctx:
            return
        sub = await plugin.subscribe([doc_channel(int(doc_id))])
        ctx = sub.run_in_background(peer.send)      # peer.send: Awaitable[None] ✓ 契约相符
        await ctx.__aenter__()
        self._chan_ctx[key] = ctx

    async def channel_unsubscribe(self, peer_id: str, doc_id: int) -> None:
        ctx = self._chan_ctx.pop((peer_id, int(doc_id)), None)
        if ctx is None:
            return
        try:
            await ctx.__aexit__(None, None, None)   # 停写任务（框架语义）
        except Exception as e:                       # 退订失败必须可见，不静默
            print(f"⚠️ [CHAN] 退订失败 peer={peer_id} doc={doc_id}: {e!r}", flush=True)

    async def channel_unsubscribe_all(self, peer_id: str) -> None:
        """连接结束时一次退干净（替代旧 detach_peer 里遍历 _doc_subs 的清理）。"""
        for key in [k for k in list(self._chan_ctx) if k[0] == peer_id]:
            await self.channel_unsubscribe(key[0], key[1])

    def deactivate_wt(self, wt_uid: str) -> None:
        """拆除指定 WT 的所有 gateway 投影——WT 删除时调用。"""
        prefix = f"{wt_uid}:"
        to_remove = [k for k in self._actives if k.startswith(prefix)]
        for key in to_remove:
            active = self._actives.pop(key, None)
            if active and active.handler:
                active.handler.stop()
        self._release_scope(wt_uid)

    def deactivate_all(self) -> None:
        """gateway off：停掉所有 WT 的 gateway 投影。只动 Active，不动声明 MD。"""
        self._shutting_down = True
        for uid, *_ in self._store.list_worktrees():
            self.deactivate_wt(uid)
        self._scopes.clear()

    def get_handler(self, wt_uid: str, entry_id: str) -> "ConnectionHandler | None":
        key = f"{wt_uid}:{entry_id}"
        active = self._actives.get(key)
        return active.handler if active else None

    def iter_scopes(self):
        return self._scopes.values()

    def is_token(self, token: bytes) -> bool:
        return self.lookup_token(token) is not None

    def lookup_token(self, token: bytes) -> "_Active | None":
        for active in self._actives.values():
            if active.decl.token.encode() == token:
                return active
        return None

    # 操作

    def mount(self, wt_uid: str, ticket: str, name: str, _ep_client=None, _conn=None, _doc_map=None) -> "_Active":
        """挂载 remote 段。先写文件，再创建 Handler + IrohPeer 注入 scope。§五"""
        from mdio_py.gateway.types import RemoteDecl, Active

        # 1. 解析声明
        decl = RemoteDecl.from_ticket(ticket, wt_uid, name)

        # 2-4. 创建空 Active
        key = f"{wt_uid}:{name}"
        scope = self._get_or_create_scope(wt_uid)
        active = Active(key=key, decl=decl, scope=scope, handler=None)

        # 5. 先注册（消除竞态）
        self._actives[key] = active

        # 6. 先写 rule（内存），再写 gateway.md
        existing = self._store.rule_remote_entries(wt_uid)
        if not any(p == decl.path for p, _ in existing):
            self._store.rule_add_remote(wt_uid, decl.path, name)
            # 只改内存 + 写文件，不调 tree_ensure：
            # ensure 从 worktrees.md 重建树（select_nodes 解析），当前构建下不可靠
            # （read_projection 能解析而 select_nodes 不能——Rust 层登记）。
            # 运行期内存态足够；重启恢复由 bootstrap tree_ensure 承担（另议）。
            self._write_rule_to_file(wt_uid)

        # 7. 写 gateway.md — 确保有 # GATEWAY 标题（bfork WT 初始化不创建此文件）
        self._reconcile_enabled = False
        try:
            now = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            entry_text = (
                f"## {name}\n"
                f"- remote_addr: {decl.addr}\n"
                f"- remote_token: {decl.token}\n"
                f"- local_prefix: {decl.local_prefix}\n"
                f"- remote_prefix: {decl.remote_prefix}\n"
                f"- mounted: {now}\n"
            )
            # 确保 # GATEWAY 标题存在，否则 _read_gateway_entries 的 drill("GATEWAY") 命中不了
            try:
                existing = self._store.get_text(wt_uid, "/.mds/gateway.md")
            except Exception:
                existing = ""
            if not existing.strip() or not existing.lstrip().startswith("# GATEWAY"):
                entry_text = f"# GATEWAY\n\n{entry_text}"
            else:
                entry_text = f"\n{entry_text}"
            from mdio_py.core.selector_builder import SelectorBuilder
            self._workspace.write_locator(
                wt_uid, "/.mds/gateway.md", SelectorBuilder().build(),
                "+>", entry_text, auto_scaffold=True, god=True, suppress_hooks=True,
            )
        finally:
            self._reconcile_enabled = True

        # 8. 注入下载连接（iroh_mount 传来，直连）
        if _ep_client is not None and _conn is not None:
            from mdio_py.gateway.iroh_transport import ConnectionHandler
            import asyncio
            handler = ConnectionHandler(initial_state="connected")
            handler._store = self._store
            handler._lookup = self.lookup_token
            handler._is_token = self.is_token
            handler._scope_lookup = lambda uid: self._scopes.get(uid)
            handler._gs = self  # doc_map 校正写回 gateway.md 用
            handler.configure(decl)
            handler.ep = _ep_client
            handler.addr = decl.addr
            handler._saved_token = decl.token.encode()
            if _doc_map:
                handler.doc_map = dict(_doc_map)
                # A3: doc_map 首次创建即入文件（设计 2.3：稳定映射必须持久化）
                self._update_mount_doc_map(wt_uid, name, _doc_map)
            active.handler = handler
            handler._active = active
            from mdio_py.gateway.peer import IrohPeer
            peer = IrohPeer(f"iroh_mount_{key}", handler.send, handler.doc_map)
            self.attach_peer(peer, scope)
            if _doc_map:
                for path, info in _doc_map.items():
                    self.subscribe(peer.peer_id, info.get("local"), scope)
                    # R6 步骤 1：mount 是同步调用点 ⇒ 通道订阅交给事件循环
                    #（WS/iroh 各自在自己的异步订阅点显式 await，不会与这里重复：
                    #  channel_subscribe 以 (peer, doc) 为键幂等）
                    try:
                        asyncio.ensure_future(
                            self.channel_subscribe(peer.peer_id, info.get("local")))
                    except Exception as e:
                        print(f"⚠️ [CHAN] mount 订阅调度失败 {peer.peer_id}: {e!r}", flush=True)
            asyncio.ensure_future(handler.run(_conn))

        # 9. Handler 就位后再 reconcile（此时 handler 非 None，不会重复创建）
        self.reconcile(wt_uid)
        return active

    def unmount(self, wt_uid: str, path: str) -> None:
        """卸载 remote 段。先删声明（文件），再销运行时。§五"""
        entry_id = path.rstrip("/").rsplit("/", 1)[-1]
        key = f"{wt_uid}:{entry_id}"
        # 2.5 步要用 decl.path —— 这里**只查不取**（真正的 pop 留到文件操作全部成功之后，
        # 保持「删除失败就不动运行时」的原意）。直接引用尚未赋值的 active ⇒ UnboundLocalError，
        # 调用方（/gateway/unmount）只看到 500，handler 却还挂着（实测）。
        existing = self._actives.get(key)

        # 文件操作期间 suppress reconcile——由显式清理接管
        self._reconcile_enabled = False
        try:
            # 1. 删 worktrees.md rule 段 + 写回文件（不调 tree_ensure，同 mount）
            self._store.rule_remove_remote(wt_uid, path)
            self._write_rule_to_file(wt_uid)

            # 2. 删 gateway.md entry
            self._rewrite_gateway_md(wt_uid, entry_id)

            # 2.5. 删 rule（与 share 对称：share 写 rule 供重启恢复）
            if existing is not None:
                self._store.rule_remove_remote(wt_uid, existing.decl.path)
                self._write_rule_to_file(wt_uid)
        finally:
            self._reconcile_enabled = True

        # 3-5. 销毁运行时（MD 意图：两步文件操作全部成功，才进这里）
        active = self._actives.pop(key, None)
        if active is None:
            return
        if active.handler:
            active.handler.stop()
        self._release_scope(wt_uid)

    def share(self, wt_uid: str, path: str, name: str) -> str:
        """分享本地路径为 remote ticket。Active(handler=None, state=Listener)。§五"""
        from mdio_py.gateway.types import RemoteDecl, Active

        # 1. 生成 token（R12：CSPRNG——random.choices 的 MT19937 非加密安全）
        import secrets
        token = secrets.token_urlsafe(12)

        # 2. 写 gateway.md（先写——reconcile 需要 gateway entry 存在才能 join 出 desired）
        entry_text = (
            f"\n## {name}\n"
            f"- shared_token: {token}\n"
            f"- local_prefix: {path}\n"
        )
        # 确保 # GATEWAY 标题存在（selector drill("GATEWAY") 需要；与 mount 一致）
        try:
            existing = self._store.get_text(wt_uid, "/.mds/gateway.md")
        except Exception:
            existing = ""
        if not existing.strip() or not existing.lstrip().startswith("# GATEWAY"):
            entry_text = f"# GATEWAY\n{entry_text}"
        from mdio_py.core.selector_builder import SelectorBuilder
        self._workspace.write_locator(
            wt_uid, "/.mds/gateway.md", SelectorBuilder().build(),
            "+>", entry_text, auto_scaffold=True, god=True, suppress_hooks=True,
        )

        # 3-5. 建 Active（无 addr → Listener，等对端连接）
        decl_path = f"/share/{name}"
        decl = RemoteDecl(
            wt_uid=wt_uid, path=decl_path, entry_id=name,
            token=token, addr=None, local_prefix=path, remote_prefix=None,
        )
        key = f"{wt_uid}:{name}"
        scope = self._get_or_create_scope(wt_uid)
        active = Active(key=key, decl=decl, scope=scope, handler=None)
        self._actives[key] = active

        # 4.5. 写 rule（重启恢复：reconcile 从 rule + gateway.md join 推导 share 条目；
        # 不写则重启后 Active/scope 丢失，对端连接 lookup_token 落空被拒）
        existing = self._store.rule_remote_entries(wt_uid)
        if not any(p == decl_path for p, _ in existing):
            self._store.rule_add_remote(wt_uid, decl_path, name)
            # 同 mount：不调 tree_ensure（Rust select_nodes 解析不可靠，登记）
            self._write_rule_to_file(wt_uid)

        # 6-7. 构建 ticket
        import iroh as _iroh
        ep_addr = str(_iroh.EndpointTicket.from_addr(self._ep.addr())) if self._ep else ""
        ticket_data = json.dumps({"addr": ep_addr, "token": token, "path": path})
        ticket_b64 = base64.urlsafe_b64encode(ticket_data.encode()).decode().rstrip("=")
        return f"iroh-share://{ticket_b64}"

    def unshare(self, wt_uid: str, path: str) -> None:
        """取消分享。只删 gateway.md entry。§五"""
        active = self._find_active_by_path(wt_uid, path)
        if active is None:
            return
        entry_id = active.decl.entry_id
        key = active.key

        # 文件操作期间 suppress reconcile
        self._reconcile_enabled = False
        try:
            self._rewrite_gateway_md(wt_uid, entry_id)
        finally:
            self._reconcile_enabled = True

        # 2-4. 销毁运行时
        self._actives.pop(key, None)
        if active.handler:
            active.handler.stop()
        self._release_scope(wt_uid)

    # 桥

    def reconcile(self, wt_uid: str) -> None:
        """从 gateway.md + rule 推导 desired，对账 current。"""
        from mdio_py.gateway.types import RemoteDecl, Active
        from mdio_py.gateway.iroh_transport import ConnectionHandler

        # 1-2. 推导 desired
        remote_entries = self._store.rule_remote_entries(wt_uid)
        if not remote_entries:
            desired: list["_RD"] = []
        else:
            gateway_entries = self._read_gateway_entries(wt_uid)
            if not gateway_entries:
                desired = []
            else:
                desired = RemoteDecl.from_files(wt_uid, remote_entries, gateway_entries)

        # 3. 当前集合
        current_keys = {k for k in self._actives if k.startswith(f"{wt_uid}:")}
        desired_keys = {f"{d.wt_uid}:{d.entry_id}" for d in desired}

        # 4. 新建 / 补建
        for decl in desired:
            key = f"{decl.wt_uid}:{decl.entry_id}"

            if key not in self._actives:
                if decl.addr is not None:
                    # mount 侧：创建 Handler
                    handler = ConnectionHandler(initial_state="reconnecting")
                    self._inject_handler(handler, decl, wt_uid)
                    # A3: 从 gateway.md 恢复 doc_map 基线（local/remote id 可能过时——
                    # _doc_map_stale 标记，重连后 _rebuild_doc_map 校正）
                    restored = self._doc_map_from_entries(gateway_entries, decl.entry_id)
                    if restored:
                        handler.doc_map = restored
                        handler._doc_map_stale = True
                    scope = self._get_or_create_scope(wt_uid)
                    active = Active(key=key, decl=decl, scope=scope, handler=handler)
                    self._actives[key] = active
                    handler._active = active
                    handler.start()
                else:
                    # share 侧：只建 Active
                    scope = self._get_or_create_scope(wt_uid)
                    self._actives[key] = Active(key=key, decl=decl, scope=scope, handler=None)

            elif self._actives[key].handler is None and decl.addr is not None:
                # mount 写文件后触发：Active 已有，补建 Handler
                handler = ConnectionHandler(initial_state="reconnecting")
                self._inject_handler(handler, decl, wt_uid)
                active = self._actives[key]
                active.handler = handler
                handler._active = active
                handler.start()

            # 没有 "state==Disconnected → reconnect" 分支

        # 5. 清理
        for key in (current_keys - desired_keys):
            active = self._actives.pop(key, None)
            if active is None:
                continue
            if active.handler:
                active.handler.stop()
            self._release_scope(wt_uid)

    def reconcile_all(self) -> None:
        for uid, _, _, _ in self._store.list_worktrees():
            self.reconcile(uid)

    # 维护

    def tick(self) -> None:
        """周期维护：phase 投影由 Handler 自主管理，tick 只做过期 GC。"""
        from mdio_py.gateway.types import RemotePhase

        for active in list(self._actives.values()):
            if active.handler is None:
                continue

            # PENDING 超时 → reconcile 清理
            if active.phase == RemotePhase.PENDING:
                age = getattr(active, '_created_at', None)
                if age is None:
                    active._created_at = time.time()
                    age = active._created_at
                if time.time() - age > EXPIRY_DAYS * 86400:
                    self.reconcile(active.decl.wt_uid)

    async def _tick_loop(self) -> None:
        while True:
            await asyncio.sleep(TICK_INTERVAL)
            self.tick()

    # 回调

    def on_document_written(self, wt_uid: str, path: str, doc_id: int) -> None:
        """workspace.write_locator 完成后的回调。"""
        if self._shutting_down:
            return
        # 控制文件变了 → reconcile（unmount/unshare 期间 suppress）
        if path in ("/.mds/gateway.md", "/.mds/worktrees.md"):
            if self._reconcile_enabled:
                self.reconcile(wt_uid)
            return

        # remote 子树写入 → FileOp 广播（通知远端）
        from mdio_py.gateway.types import RemotePhase

        for active in self._actives.values():
            if not active.key.startswith(f"{wt_uid}:"):
                continue
            if active.phase != RemotePhase.ACTIVE:
                continue
            prefix = active.decl.local_prefix.rstrip("/") if active.decl.local_prefix else ""
            if not path.startswith(prefix + "/") and path != prefix:
                continue

            try:
                relative = path[len(prefix):].lstrip("/") if prefix else path.lstrip("/")
                op_json = json.dumps({"New": {"relative_path": relative, "checkpoint": []}})
                import mdio_py.mdio_core as _mrc2
                payload = _mrc2.encode_file_op_ffi(op_json)
                if not payload:
                    continue

                crc = zlib.crc32(active.decl.entry_id.encode()) & 0xFFFFFFFF
                frame = bytearray()
                frame.append(UP_FILE)
                frame.extend(struct.pack('<I', len(payload)))
                frame.extend(struct.pack('<I', 0))  # peer
                frame.extend(struct.pack('<I', crc)) # pipe = crc32(entry_id)
                frame.extend(payload)

                async def _broadcast(f: bytes = bytes(frame), _self=self):
                    from mdio_py.gateway.crdt_scope import deliver, KIND_FILE
                    await deliver(_self, doc_id, f, KIND_FILE, store=_self._store)

                try:
                    asyncio.get_running_loop()
                    asyncio.ensure_future(_broadcast())
                except RuntimeError:
                    pass
            except Exception:
                pass

    def _on_handler_event(self, key: str, handler_state: str) -> None:
        """Handler 状态变化回调。phase 已由 Handler._notify_state_change 直接更新。"""
        pass

    def _handle_file_action(self, action):
        """WS 路径到达的 FileOp——crc32 反查 Active。§四"""
        crc_hex = action.file_action_entry_id
        op_json = action.file_action_op_json
        if not crc_hex or not op_json:
            return
        for active in self._actives.values():
            eid_crc = format(zlib.crc32(active.decl.entry_id.encode()) & 0xFFFFFFFF, '08x')
            if eid_crc == crc_hex:
                from mdio_py.gateway.iroh_transport import _exec_file_op
                _exec_file_op(self._store, active.decl.wt_uid, active.decl.local_prefix or "/", op_json)
                return

    # 内部辅助

    def _get_or_create_scope(self, wt_uid: str) -> "CrdtScope":
        if wt_uid not in self._scopes:
            from mdio_py.gateway.crdt_scope import CrdtScope
            self._scopes[wt_uid] = CrdtScope(wt_uid)
        return self._scopes[wt_uid]

    def _release_scope(self, wt_uid: str) -> None:
        """如果该 wt 下没有其他 Active，删除 scope。

        统一注册表后 scope 同时承载本地 WS 会话——unmount/unshare 不得
        误清 WS peer（ws_ 前缀），残留 iroh peer 随 scope 一并清理
        （连接已关，broadcast 失败时也会自清）。"""
        for k, a in self._actives.items():
            if k.startswith(f"{wt_uid}:"):
                return  # 还有活的 Active
        scope = self._scopes.get(wt_uid)
        if scope is None:
            return
        if any(pid.startswith("ws_") for pid in scope.peers):
            return  # 本地 WS 会话仍在——保留 scope
        self._scopes.pop(wt_uid, None)

    def _inject_handler(self, handler: "ConnectionHandler", decl: "_RD", wt_uid: str) -> None:
        """注入 Handler 所需依赖。ep 此时可能为 None（startup 阶段）。"""
        handler._store = self._store
        handler._lookup = self.lookup_token
        handler._is_token = self.is_token
        handler._scope_lookup = lambda uid: self._scopes.get(uid)
        handler._gs = self  # doc_map 校正写回 gateway.md 用
        handler.on_state_change = self._on_handler_event
        handler.configure(decl)
        handler.ep = self._ep

    @staticmethod
    def _parse_gateway_chunks(children, *, keep_untitled: bool = False,
                              workspace=None, store=None) -> list[dict]:
        """gateway.md 子块 → entry 列表（Q13：_parse / _read_gateway_entries 共享的唯一内核）。

        keep_untitled：旧 _read_gateway_entries 保留空标题条目、旧 _parse 丢弃——
        两处旧行为并存，参数化精确还原。
        workspace / store：kv 提取的引擎句柄（见 _kv_from_md_text 三档）；两者都缺时
        走旧 parse_kv_lines（_parse 的 /mem 装配——搁置中，按业主要求不动）。
        """
        entries = []
        for chunk in children:
            lines = str(chunk).split("\n")
            if not lines:
                continue
            name = lines[0].strip().lstrip("#").strip()
            if not name and not keep_untitled:
                continue
            entry = {"_title": name}
            # 与旧 parse_kv_lines 吃同一段文本（标题行以下），对拍才有意义
            entry.update(_kv_from_md_text("\n".join(lines[1:]),
                                          workspace=workspace, store=store))
            for line in lines[1:]:
                line = line.strip()
                if line.startswith("- ") and ":" not in line[2:]:
                    entry[line[2:].strip()] = True
            entries.append(entry)
        return entries

    @staticmethod
    def _parse(text: str) -> list[dict]:
        """用 MDIO selector 管线解析 gateway.md 文本为 entry 列表。"""
        if not text or not text.strip():
            return []
        # 确保有 # GATEWAY 顶级标题，否则 selector 的 drill("GATEWAY") 命中不了
        if not text.lstrip().startswith("# GATEWAY"):
            text = "# GATEWAY\n" + text
        from mdio_py import mdio_core
        from mdio_py.core.selector_builder import SelectorBuilder
        store = mdio_core.ProjectStore("/mem")
        ir = SelectorBuilder().drill("GATEWAY").any_child().build()
        try:
            children = store.query_memory(text, ir, proj="raw")
        except Exception:
            return []
        return GatewayStore._parse_gateway_chunks(children)

    def _read_gateway_entries(self, wt_uid: str) -> list[dict]:
        """Read gateway.md entries via selector。

        与 _parse 对齐：# GATEWAY 顶级标题缺失时自动补（selector 的
        drill("GATEWAY") 需要标题存在才能命中——share 早期写入无标题）。"""
        from mdio_py.core.selector_builder import SelectorBuilder
        try:
            text = self._store.get_text(wt_uid, "/.mds/gateway.md") or ""
        except Exception:
            return []
        if not text.strip():
            return []
        if not text.lstrip().startswith("# GATEWAY"):
            text = "# GATEWAY\n" + text
        ir = SelectorBuilder().drill("GATEWAY").any_child().build()
        try:
            children = self._store.query_memory(text, ir, proj="raw")
        except Exception:
            return []
        return self._parse_gateway_chunks(children, keep_untitled=True,
                                          workspace=self._workspace, store=self._store)   # 旧 _read_gateway_entries 保留空标题条目

    def _update_mount_doc_map(self, wt_uid: str, entry_id: str, doc_map: dict) -> None:
        """把 doc_map 持久化到 gateway.md 的 mount 条目（单行 JSON）。

        设计 gateway_manager_plan.md 2.3/3.1：doc_map 是稳定映射，必须入文件。
        落地修正：设计原文用缩进多行，与现有 selector 解析（- k: v 单行）不兼容，
        故用单行 JSON（已探针验证 selector 管线可原样解析）。
        写回触发 on_document_written → reconcile，active 已存在不会重建，无循环。
        """
        if not doc_map:
            return
        try:
            text = self._store.get_text(wt_uid, "/.mds/gateway.md") or ""
        except Exception:
            return
        if not text.strip():
            return
        lines = text.split("\n")
        start = None
        for i, ln in enumerate(lines):
            if ln.startswith("## ") and ln[3:].strip() == entry_id:
                start = i
                break
        if start is None:
            return
        end = len(lines)
        for i in range(start + 1, len(lines)):
            if lines[i].startswith("## "):
                end = i
                break
        new_line = f"- doc_map: {json.dumps(doc_map, ensure_ascii=False)}"
        seg = [ln for ln in lines[start:end] if not ln.startswith("- doc_map:")]
        seg.append(new_line)
        new_text = "\n".join(lines[:start] + seg + lines[end:])
        from mdio_py.core.selector_builder import SelectorBuilder
        self._workspace.write_locator(
            wt_uid, "/.mds/gateway.md", SelectorBuilder().build(),
            "/>", new_text, auto_scaffold=True, god=True, suppress_hooks=True,
        )

    @staticmethod
    def _doc_map_from_entries(gateway_entries: list[dict], entry_id: str) -> dict:
        """从 gateway.md entries 恢复指定条目的 doc_map（单行 JSON 解析）。"""
        for e in gateway_entries:
            if e.get("_title") != entry_id:
                continue
            raw = e.get("doc_map")
            if not raw:
                return {}
            try:
                parsed = json.loads(raw) if isinstance(raw, str) else raw
                return parsed if isinstance(parsed, dict) else {}
            except Exception:
                return {}
        return {}

    def _rewrite_gateway_md(self, wt_uid: str, entry_id: str) -> None:
        """Delete ## {entry_id} from gateway.md via selector."""
        from mdio_py.core.selector_builder import SelectorBuilder
        sel = SelectorBuilder().drill("GATEWAY").drill(entry_id).build()
        self._workspace.write_locator(
            wt_uid, "/.mds/gateway.md", sel,
            "->", "", auto_scaffold=False, god=True, suppress_hooks=True,
        )

    def _find_active_by_path(self, wt_uid: str, path: str) -> "_Active | None":
        """按路径查找 Active：先精确匹配 decl.path，再匹配 decl.local_prefix。"""
        for active in self._actives.values():
            if active.decl.wt_uid != wt_uid:
                continue
            if active.decl.path == path:
                return active
            if active.decl.local_prefix == path:
                return active
        return None

    def _write_rule_to_file(self, wt_uid: str) -> None:
        """将当前 rule 写回父 WT 的 worktrees.md 文件。

        实现已抽到 std/worktree_ops.write_rule_to_file（rule 写回的唯一实现，
        CLI mount 与 gateway mount 共用）；此处保留方法名以不动既有调用点。
        见 BUGS 复盘 B2/B1。
        """
        from mdio_py.std.worktree_ops import write_rule_to_file
        write_rule_to_file(self._workspace, self._store, wt_uid)

