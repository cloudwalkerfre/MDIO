"""iroh transport —— 每连接一个 IrohPeer。

每个连接一个 IrohPeer，每收到一个双向 stream 就是一个完整的请求-响应周期。
没有状态机，没有 fire-and-forget，没有闭包捕获陷阱。
"""

from __future__ import annotations
import asyncio, json, struct, time
from typing import Any

from .peer import Peer, IrohPeer
from .crdt_scope import CrdtScope
from mdio_py.gateway.handlers.crdt import _ast_enabled, submit_ast_packet

HEADER_SIZE = 13

# 帧分类：第一字节判定
CRDT_TAGS = frozenset({0x01, 0x02, 0x03, 0x04, 0x08, 0x09, 0x81, 0x82, 0x83})


# Frame classifier — one function to rule them all

def classify_frame(frame: bytes, is_token_fn=None) -> str:
    """返回帧类型: 'token' | 'doc_map' | 'crdt' | 'unknown'。
    is_token_fn: Callable[[bytes], bool] — GatewayStore 注入的 token 判断闭包
    """
    if not frame:
        return "unknown"
    tag = frame[0]
    if tag in CRDT_TAGS:
        return "crdt"
    if frame.startswith(b"{"):
        try:
            data = json.loads(frame)
            if data.get("type") == "doc_map":
                return "doc_map"
        except Exception:
            pass
    if is_token_fn is not None:
        if is_token_fn(frame):
            return "token"
        if b"|" in frame[:128]:
            t = frame.split(b"|", 1)[0]
            if is_token_fn(t):
                return "token"
    return "unknown"


# Core dispatch — WS 和 iroh 共享

def _gws_of(lookup):
    """步骤 2：lookup 若是 GatewayStore（有 4 个写口）就返回它，否则 None（旧接线）。"""
    return lookup if hasattr(lookup, "subscribe") and hasattr(lookup, "attach_peer") else None


async def _deliver_ast(pkg, lookup, doc_id, origin, store):
    """AST 帧的唯一出口包装（lambda 里不能写 import）。kind=Ast ⇒ 不抑制来源。"""
    from mdio_py.gateway.crdt_scope import deliver, KIND_AST
    return await deliver(lookup, doc_id, pkg, KIND_AST, origin=origin, store=store)


async def dispatch_action(
    action: Any,
    doc_id: int,
    peer: Peer,
    store: Any,
    gateway_mgr_or_scope_lookup: Any,
    reply_fn: Any,
    expected_wt_uid: str | None = None,
):
    """处理 store.handle_packet 返回的 ServerAction。
    gateway_mgr_or_scope_lookup: GatewayManager（旧）或 scope_lookup 函数（新）
    expected_wt_uid: 认证绑定的 wt_uid（R11 订阅归属校验用——跨 WT 订阅拒绝）
    """
    # 1. Reply（Direct 或 DirectAndSub）
    if action.data and (action.is_direct or action.is_subscribe):
        await reply_fn(bytes(action.data))

    # 2. Subscribe（DirectAndSub）
    if action.is_subscribe:
        wt_uid = store.doc_wt_uid(doc_id)
        if wt_uid is None:
            print(f"[ADMIT] iroh 不登记订阅 doc={doc_id} ⇒ 该文档没有归属 wt", flush=True)
            return
        # 新路径：scope_lookup 函数
        if callable(gateway_mgr_or_scope_lookup) and not hasattr(gateway_mgr_or_scope_lookup, 'get_scope'):
            scope = gateway_mgr_or_scope_lookup(wt_uid)
        else:
            scope = gateway_mgr_or_scope_lookup.get_scope(wt_uid)
        if scope is None:
            print(f"[ADMIT] iroh 不登记订阅 doc={doc_id} wt={wt_uid} ⇒ 没有 scope", flush=True)
            return
        # C1：与 WS 统一判据 —— **可达性为主**；R11 降为快速前置（归属不符直接拒，省一次取树）。
        if expected_wt_uid is not None and scope.wt_uid != expected_wt_uid:
            print(f"[ADMIT] iroh 不登记订阅 doc={doc_id} ⇒ R11 前置不符（连接 wt={expected_wt_uid} / scope wt={scope.wt_uid}）", flush=True)
            return
        # 可达性主判据（与 WS 订阅分支同源：get_vfs_tree(wt, True) 的 id 集；判不了 ⇒ 放行订阅 = 接受 + 空订阅）
        from mdio_py.gateway.handlers.crdt import _reachable_doc_ids
        _ids = _reachable_doc_ids(store, expected_wt_uid or scope.wt_uid)
        if _ids is not None and int(doc_id) not in _ids:
            print(f"[ADMIT] iroh 不登记订阅 doc={doc_id} ⇒ 不在连接 wt={expected_wt_uid or scope.wt_uid} 的可达集（与 WS 同判据）", flush=True)
            # 拒绝必须协议可见：回 DnErr（引擎收会话 + 宿主可见）。peer 从请求帧头 +5 回填（与 WS B1 同形）。
            import struct
            _peer = struct.unpack_from("<I", bytes(action.data), 5)[0] if action.data and len(action.data) >= 9 else 0
            _reason = f"doc={doc_id} 不在连接 wt 的可达集".encode()
            await reply_fn(struct.pack("<BIII", 0xFF, len(_reason), _peer, doc_id) + _reason)
            return
        _gws = _gws_of(gateway_mgr_or_scope_lookup)
        if _gws is not None:
            _gws.attach_peer(peer, scope)
            _gws.subscribe(peer.peer_id, doc_id, scope)
            await _gws.channel_subscribe(peer.peer_id, doc_id)   # R6：iroh 文档帧同形走通道
        else:
            if peer.peer_id not in scope.peers:
                scope.add_peer(peer)
            scope.subscribe(peer.peer_id, doc_id)

    # 3. Broadcast
    if action.is_broadcast and action.data:
        wt_uid = store.doc_wt_uid(doc_id)
        if wt_uid:
            if callable(gateway_mgr_or_scope_lookup) and not hasattr(gateway_mgr_or_scope_lookup, 'get_scope'):
                scope = gateway_mgr_or_scope_lookup(wt_uid)
            else:
                scope = gateway_mgr_or_scope_lookup.get_scope(wt_uid)
            if scope:
                # 唯一出口（kind=TextOp，origin=真实 peer ⇒ 跳过发起端）
                from mdio_py.gateway.crdt_scope import deliver, KIND_TEXT
                await deliver(gateway_mgr_or_scope_lookup, doc_id, bytes(action.data), KIND_TEXT,
                              origin=peer.peer_id, store=store)
            # DN_AST 通道
            # FJ 线程渲染：投递到 FJ 线程渲染，不阻塞本线程——
            # 完成回调经事件循环广播（语义保持：scope 广播给所有 peer）
            if _ast_enabled(store, wt_uid):
                if scope:
                    loop = asyncio.get_running_loop()
                    submit_ast_packet(
                        store, doc_id, loop,
                        lambda pkg: _deliver_ast(pkg, gateway_mgr_or_scope_lookup, doc_id, peer.peer_id, store))

    # 4. Unsubscribe
    if action.is_unsubscribe:
        wt_uid = store.doc_wt_uid(doc_id)
        if wt_uid:
            if callable(gateway_mgr_or_scope_lookup) and not hasattr(gateway_mgr_or_scope_lookup, 'get_scope'):
                scope = gateway_mgr_or_scope_lookup(wt_uid)
            else:
                scope = gateway_mgr_or_scope_lookup.get_scope(wt_uid)
            if scope:
                _gws = _gws_of(gateway_mgr_or_scope_lookup)
                if _gws is not None:
                    _gws.unsubscribe(peer.peer_id, doc_id, scope)
                else:
                    scope.unsubscribe(peer.peer_id, doc_id)

    # 5. FileAction — 由 GatewayStore._handle_file_action 处理
    if getattr(action, 'is_file_action', False):
        if hasattr(gateway_mgr_or_scope_lookup, '_handle_file_action'):
            gateway_mgr_or_scope_lookup._handle_file_action(action)


def _exec_file_op(store, wt_uid: str, local_prefix: str, op_json: str) -> None:
    """执行单条 FileOp：New/Del/Mov → VFS 操作。两条路径共享。"""
    import json
    try:
        op = json.loads(op_json)
    except json.JSONDecodeError:
        return

    prefix = local_prefix.rstrip("/") if local_prefix else ""
    if "New" in op:
        rp = op["New"]["relative_path"]
        full = f"{prefix}/{rp}" if prefix else f"/{rp}"
        ckpt = bytes(op["New"].get("checkpoint", []))
        if ckpt:
            text = store.decode_checkpoint_text(ckpt) if hasattr(store, 'decode_checkpoint_text') else ""
            try:
                store.write_locator(wt_uid, full, {"steps": []}, "+>", text, True, True, False, False)
            except Exception:
                pass
    elif "Del" in op:
        rp = op["Del"]["relative_path"]
        full = f"{prefix}/{rp}" if prefix else f"/{rp}"
        try:
            store.rm(wt_uid, full, False, True, False)
        except Exception:
            pass
    elif "Mov" in op:
        old_rp = op["Mov"]["old_path"]
        new_rp = op["Mov"]["new_path"]
        old_full = f"{prefix}/{old_rp}" if prefix else f"/{old_rp}"
        new_full = f"{prefix}/{new_rp}" if prefix else f"/{new_rp}"
        try:
            text = store.get_text(wt_uid, old_full)
            if text:
                store.write_locator(wt_uid, new_full, {"steps": []}, "+>", text, True, True, False, False)
                store.rm(wt_uid, old_full, False, True, False)
        except Exception:
            pass



# Per-stream handler — one request, one response

class ConnectionHandler:
    """管理一个 iroh 连接。

    双向流（open_bi/accept_bi）：请求-响应（token 认证、UP_TREE、UP_OPEN 等）。
    单向流（open_uni/accept_uni）：fire-and-forget CRDT 广播，无 recv half 残留。
    两种流在同一 QUIC 连接上并发共存。
    """

    def __init__(self, initial_state: str = "connected"):
        # 重连状态机
        self._initial_state = initial_state
        self.state = initial_state
        self.pending: list[bytes] = []
        self.retry = 0
        self.ep = None
        self.addr = None
        self.alpn = b"mdio-net/1"
        self._saved_token: bytes | None = None
        self.doc_map: dict[str, dict] = {}
        # UP_TREE 子树过滤
        self.local_prefix: str | None = None
        self.wt_uid: str | None = None
        # GatewayStore 注入属性
        self.on_state_change = None       # Callable[[str, str], None] | None
        self._task = None                 # asyncio.Task | None
        self._background_tasks: set[asyncio.Task] = set()
        self._store = None                # ProjectStore | None
        self._active = None               # Active | None（通过 .scope 获取 CrdtScope）
        self._lookup = None               # Callable[[bytes], Active|None] | None
        self._is_token = None             # Callable[[bytes], bool] | None

    # 连接状态方法

    def configure(self, decl):
        """从 RemoteDecl 统一设置 Handler 配置。"""
        from mdio_py.gateway.types import RemoteDecl as _RD
        self.addr = decl.addr
        self._saved_token = decl.token.encode()
        self.wt_uid = decl.wt_uid
        self.entry_id = decl.entry_id
        self.local_prefix = decl.local_prefix
        self.remote_prefix = decl.remote_prefix
        self.doc_map = {}                # 新连接，重置映射

    def stop(self):
        """取消 asyncio task + 关闭连接 + 切换到 idle。"""
        if self._task is not None:
            self._task.cancel()
        for task in list(self._background_tasks):
            task.cancel()
        if hasattr(self, '_conn') and self._conn is not None:
            try:
                self._conn.close(0, b"stop")
            except Exception:
                pass
        # R13：清理 scope peer（死 peer 残留——send 非 connected 只 append 不抛）
        try:
            if (getattr(self, '_scope_lookup', None) is not None
                    and getattr(self, 'wt_uid', None) is not None
                    and hasattr(self, 'peer_id')):
                scope = self._scope_lookup(self.wt_uid)
                if scope is not None:
                    _gws = _gws_of(getattr(self, "_gs", None))
                    if _gws is not None:
                        # R6：通道订阅退干净（本方法同步 ⇒ 交给事件循环；失败不阻塞关闭）
                        try:
                            import asyncio as _aio
                            _aio.ensure_future(_gws.channel_unsubscribe_all(self.peer_id))
                        except Exception:
                            pass
                        _gws.detach_peer(self.peer_id)      # 投递表 + 全订阅 + 各 wt 分域
                    else:
                        scope.remove_peer(self.peer_id)
        except Exception:
            pass
        self.state = "idle"

    async def wait_stopped(self) -> None:
        """cancel 后等待 _task / _background_tasks 退出（幂等）。"""
        tasks = [t for t in (self._task, *self._background_tasks) if t is not None]
        if not tasks:
            return
        for t in tasks:
            if not t.done():
                t.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        self._task = None
        self._background_tasks.clear()

    def start(self) -> None:
        """启动 Handler 的自主状态机。由外部（GatewayStore.reconcile）调用。"""
        if self._task is None or self._task.done():
            self._task = asyncio.ensure_future(self._run_loop())

    def _notify_state_change(self, new_state: str) -> None:
        """状态转移时回调 GatewayStore._on_handler_event，同步更新 phase。"""
        if self._active is not None:
            self._active.phase = self._map_to_phase()
        if self.on_state_change is not None:
            wt = getattr(self, 'wt_uid', None)
            eid = getattr(self, 'entry_id', None)
            if wt and eid:
                key = f"{wt}:{eid}"
                self.on_state_change(key, new_state)

    def _map_to_phase(self):
        """将底层 iroh 状态映射为 Gateway 统一 Phase。
        ACTIVE = connected + doc_map 就绪。其余一律 PENDING。"""
        from mdio_py.gateway.types import RemotePhase
        if self.state == "connected" and self.doc_map:
            return RemotePhase.ACTIVE
        return RemotePhase.PENDING

    async def run(self, conn):
        """连接主循环——含重连状态机。

        每个新连接重置状态到 initial_state。
        iroh 框架可能复用同一 handler 处理多个连接。
        """
        self._conn = conn
        self.peer_id = f"iroh_{str(conn.remote_id())[:8]}"
        self.state = self._initial_state
        self.retry = 0
        self._accept_fail_count = 0
        self._task = asyncio.current_task()
        await self._run_loop()

    def _cleanup_connection(self):
        """关闭连接 + 清理 scope peer（握手失败/超时路径——不进入 idle 死循环）。

        R13：认证期间 _auth_token 注册的 peer 在连接死亡时显式移除——
        send() 非 connected 态只 append 不抛异常，broadcast 的自清不可靠。"""
        if hasattr(self, '_conn') and self._conn is not None:
            try:
                self._conn.close(0, b"handshake_fail")
            except Exception:
                pass
        try:
            if (getattr(self, '_scope_lookup', None) is not None
                    and getattr(self, 'wt_uid', None) is not None
                    and hasattr(self, 'peer_id')):
                scope = self._scope_lookup(self.wt_uid)
                if scope is not None:
                    _gws = _gws_of(getattr(self, "_gs", None))
                    if _gws is not None:
                        # R6：通道订阅退干净（本方法同步 ⇒ 交给事件循环；失败不阻塞关闭）
                        try:
                            import asyncio as _aio
                            _aio.ensure_future(_gws.channel_unsubscribe_all(self.peer_id))
                        except Exception:
                            pass
                        _gws.detach_peer(self.peer_id)      # 投递表 + 全订阅 + 各 wt 分域
                    else:
                        scope.remove_peer(self.peer_id)
        except Exception:
            pass
        self.state = "idle"

    async def _run_loop(self):
        while True:
            if self.state == "handshaking":
                # ── 握手态（A4 认证门禁）──
                # 只收 token 帧，其余全拒（门禁在 _handle_stream 内）；
                # 不启动单向流接收（结构性关闭未认证订阅——R14）；
                # 30s 总 deadline 包住 accept+handle 整个周期（R1：防 slowloris 钉住 recv）
                while self.state == "handshaking":
                    try:
                        async with asyncio.timeout(30):
                            bi = await self._conn.accept_bi()
                            await self._handle_stream(bi, self.peer_id)
                    except asyncio.TimeoutError:
                        # 认证超时——close 连接 + 终结任务（R2：share 侧无重连必要，
                        # 新连接由 creator 新建 handler；不进入 idle↔reconnecting 死循环）
                        self._cleanup_connection()
                        return
                    except Exception:
                        # accept/handle 异常——回状态机管辖（R3：不击穿任务）
                        self.state = "disconnected"
                        self._notify_state_change("disconnected")
                        break
                # 认证成功（_auth_token 置 connected）→ 主循环重新评估 → connected 分支
                continue
            elif self.state == "connected":
                # 双 accept 并发：双向流处理请求-响应，单向流处理 CRDT 推送
                bi_task = asyncio.ensure_future(self._accept_bi_loop())
                uni_task = asyncio.ensure_future(self._accept_uni_loop())
                done, pending = await asyncio.wait(
                    [bi_task, uni_task],
                    return_when=asyncio.FIRST_COMPLETED,
                )
                # 任一退出 → 连接断开，取消另一个
                for t in pending:
                    t.cancel()
                # 收集异常（忽略 CancelledError）
                for t in done:
                    try:
                        await t
                    except asyncio.CancelledError:
                        pass
                self.state = "disconnected"
                self._notify_state_change("disconnected")
                self.retry = 0

            elif self.state == "disconnected":
                backoff = min(1 << self.retry, 16)
                # uvloop timer 在 UniFFI 异常后失效 — 用 to_thread 绕开
                await asyncio.to_thread(time.sleep, backoff)
                self.state = "reconnecting"
                self._notify_state_change("reconnecting")

            elif self.state == "reconnecting":
                try:
                    if self.addr is None:
                        self.state = "idle"
                        self._notify_state_change("idle")
                        continue
                    import iroh as _iroh
                    self._conn = await self.ep.connect(self.addr, self.alpn)
                    if self._saved_token:
                        resp = await self._send_token_frame(self._conn, self._saved_token)
                        if resp != b"accepted":
                            # token 被拒（对端 unshare/重启后失效）：重试无意义，close 连接 + 结束任务。
                            try:
                                self._conn.close(0, b"auth_rejected")
                            except Exception:
                                pass
                            print(f"⚠️ [iroh] token rejected for {self.wt_uid or self.addr}: {resp[:40]!r}")
                            self.state = "idle"
                            self._notify_state_change("idle")
                            return
                    self.state = "syncing"
                    self._notify_state_change("syncing")
                    self.retry = 0
                except asyncio.TimeoutError:
                    # 等 accepted 超时——瞬态，backoff 重试
                    self.retry += 1
                    if self.retry >= 5:
                        self.state = "idle"
                        self._notify_state_change("idle")
                    else:
                        self.state = "disconnected"
                        self._notify_state_change("disconnected")
                except Exception:
                    self.retry += 1
                    if self.retry >= 5:
                        self.state = "idle"
                        self._notify_state_change("idle")
                    else:
                        self.state = "disconnected"
                        self._notify_state_change("disconnected")

            elif self.state == "syncing":
                to_send = list(self.pending)
                self.pending.clear()
                if to_send:
                    async def _flush_one(frame):
                        uni = await self._conn.open_uni()
                        await uni.write_all(frame)
                        await uni.finish()

                    try:
                        await asyncio.gather(
                            *(_flush_one(f) for f in to_send),
                            return_exceptions=False,
                        )
                    except Exception:
                        self.pending = to_send + self.pending
                        self.state = "disconnected"
                        self._notify_state_change("disconnected")
                        self.retry = 0
                        continue

                while self.pending and self.state == "syncing":
                    more = list(self.pending)
                    self.pending.clear()
                    try:
                        await asyncio.gather(
                            *(_flush_one(f) for f in more),
                            return_exceptions=False,
                        )
                    except Exception:
                        self.pending = more + self.pending
                        self.state = "disconnected"
                        self._notify_state_change("disconnected")
                        self.retry = 0
                        break
                if self.state == "syncing":
                    self.state = "connected"
                    self._notify_state_change("connected")
                    self.retry = 0
                    self.peer_id = f"iroh_{str(self._conn.remote_id())[:8]}"
                    # 首次连接后自动重建 doc_map（文件恢复的基线 stale 时也重建校正）
                    if self.wt_uid and self.local_prefix and (
                        not self.doc_map or getattr(self, "_doc_map_stale", False)
                    ):
                        task = asyncio.ensure_future(self._rebuild_doc_map())
                        self._background_tasks.add(task)
                        task.add_done_callback(self._background_tasks.discard)

            elif self.state == "idle":
                await asyncio.to_thread(time.sleep, 30)
                self.state = "reconnecting"
                self._notify_state_change("reconnecting")
                self.retry = 0

    async def _accept_bi_loop(self):
        """接受双向流（请求-响应：token 认证、UP_TREE、UP_OPEN 等）。"""
        fail_count = 0
        while True:
            try:
                bi = await self._conn.accept_bi()
                fail_count = 0
                await self._handle_stream(bi, self.peer_id)
            except Exception:
                fail_count += 1
                if fail_count >= 3:
                    raise

    async def _accept_uni_loop(self):
        """接受单向流（fire-and-forget CRDT 广播）。无 send half，不回复。"""
        while True:
            uni = await self._conn.accept_uni()
            frame = bytes(await uni.read_to_end(10_000_000))
            # defense-in-depth（R14）：单向流接收只在 connected 分支启动（握手态
            # 结构性无 uni accept）——若未来重构提前启动，此门禁兜底拒绝未认证订阅
            if self.state != "connected":
                continue
            tag = frame[0] if frame else 0
            # 单向流只有 CRDT 帧——其他类型（token/doc_map）走双向流
            if tag in CRDT_TAGS and self._store is not None:
                from .peer import IrohPeer
                async def _noop_reply(data): pass
                peer = IrohPeer(self.peer_id, _noop_reply, getattr(self, "doc_map", {}))
                action = self._store.handle_packet(frame, getattr(self, 'wt_uid', None))
                doc_id = struct.unpack_from("<I", frame, 9)[0]
                await dispatch_action(action, doc_id, peer, self._store,
                                      getattr(self, '_scope_lookup', None), _noop_reply,
                                      expected_wt_uid=getattr(self, 'wt_uid', None))

    async def _handle_stream(self, bi, peer_id):
        """处理单个双向 stream。"""
        frame = bytes(await bi.recv().read_to_end(10_000_000))
        tag = frame[0] if frame else 0
        # ── GATEWAY 全量拦截（入站）：每条进网关的帧 ──
        # tag 速查：0x01 UpTree 0x02 UpOpen 0x03 UpClose 0x04 UpSync 0x0E UpExec
        #           0x81 DnTree 0x82 DnOpen 0x83 DnSync 0x84 DnAst 0x85 DnExec

        async def reply(data):
            await bi.send().write_all(data)
            await bi.send().finish()

        # ── A4 认证门禁（R9 白名单）：仅 connected/syncing 处理业务帧 ──
        # 握手态只认 token 帧（R15：token 字符集为字母数字 ASCII，不与
        # CRDT_TAGS/JSON 冲突——classify 的 CRDT 优先判定无歧义）；
        # 无效 token（classify unknown）→ ERR:unauthorized（R6 语义：
        # _auth_token 的 invalid_token 分支在注入链下不可达）
        if self.state not in ("connected", "syncing"):
            if self.state == "handshaking" and classify_frame(frame, is_token_fn=self._is_token) == "token":
                await self._auth_token(frame, peer_id, reply)
                return
            await reply(b"ERR:unauthorized")
            return

        # UP_TREE 子树过滤（R8：无条件走 Python 分支——connected 态必有
        # local_prefix/wt_uid（configure 注入）；缺失时回空子树——任何路径
        # 不落 Rust refresh_tree_json("genesis") 分支）
        if tag == 0x01:
            if self.local_prefix is None or self.wt_uid is None:
                await reply(struct.pack("<BIII", 0x81, 2, 0, 0) + b"[]")
                return
            import json as _json
            wt = self.wt_uid
            if self._store is None:
                return
            tree_json = self._store.get_vfs_tree(wt, True)   # M4-1：含隐藏（/.mds 声明本体也是树上的节点）
            all_files = _json.loads(tree_json.decode()) if tree_json else []
            # M4-1（树旁路收口）：删除 /.mds 硬过滤——CRDT 树必须完整（P1：树本身承载一切），
            # 声明本体（/.mds/ui.md 等）与普通文档同是树上的节点；展示层要不要隐藏由壳自己决定，
            # 传输层不再替业务藏数据。
            filtered = [f for f in all_files
                        if f.get("path", "").startswith(self.local_prefix)]
            resp_data = _json.dumps(filtered).encode()
            header = struct.pack("<BIII", 0x81, len(resp_data), 0, 0)
            await reply(header + resp_data)
            return

        # FileOp: 直接用 handler 上下文
        if tag in (0x06, 0x86) and self.wt_uid is not None and self._store is not None:
            action = self._store.handle_packet(frame, getattr(self, 'wt_uid', None))
            if getattr(action, 'is_file_action', False):
                self._handle_file_op(action, self._store)
            return

        kind = classify_frame(frame, is_token_fn=self._is_token)

        if kind == "token":
            await self._auth_token(frame, peer_id, reply)
        elif kind == "doc_map":
            self._apply_doc_map(frame)
            await bi.send().write_all(b'')
            await bi.send().finish()
        elif kind == "crdt":
            if self._store is None:
                return
            peer = IrohPeer(peer_id, reply, getattr(self, "doc_map", {}))
            action = self._store.handle_packet(frame, getattr(self, 'wt_uid', None))
            doc_id = struct.unpack_from("<I", frame, 9)[0]
            await dispatch_action(action, doc_id, peer, self._store,
                                  getattr(self, '_scope_lookup', None), reply,
                                  expected_wt_uid=getattr(self, 'wt_uid', None))

    async def _auth_token(self, frame, peer_id, reply):
        """Token 认证——自注入 Active + 从 decl 自配置。§七

        A4 修订：
        - 幂等守卫（R7）：已认证（connected/syncing 且 _active 非 None）时重复
          token 帧直接 accepted 返回——防重跑 configure 清空 doc_map / 覆盖绑定
        - 终态化（R7/R10）：认证成功置 state="connected" 并通知真实终态；
          顺序不变量——add_peer 与 state 赋值之间不得插入 await（否则广播
          可能在认证完成前落入 pending，share 侧从不进 syncing 无法 flush）
        """
        token = frame.split(b"|", 1)[0] if b"|" in frame else frame

        if self._lookup is None:
            await reply(b"ERR:no_gateway")
            return

        if self.state in ("connected", "syncing") and self._active is not None:
            await reply(b"accepted")
            return

        active = self._lookup(token)
        if active is None:
            await reply(b"ERR:invalid_token")
            return
        active.handler = self                        # self-inject
        self.configure(active.decl)                  # 自配置
        peer = IrohPeer(peer_id, self.send, self.doc_map)
        _gws = _gws_of(getattr(self, "_gs", None))
        if _gws is not None:
            _gws.attach_peer(peer, active.scope)
        else:
            active.scope.add_peer(peer)
        self._active = active                        # 绑定（share 侧 creator 路径补设）
        self.state = "connected"                     # 终态（R7）
        self._notify_state_change("connected")       # 通知真实终态（原 notify(self.state)）
        await reply(b"accepted")

    async def send(self, frame: bytes):
        """被 CrdtScope.broadcast() 调用。单向流发送，无 recv half 残留。"""
        # ── GATEWAY 全量拦截（出站）：每条网关下发的帧 ──
        if self.state == "connected" and hasattr(self, '_conn'):
            try:
                uni = await self._conn.open_uni()
                await uni.write_all(frame)
                await uni.finish()
            except Exception:
                self.pending.append(frame)
                self.state = "disconnected"
                self._notify_state_change("disconnected")
                self.retry = 0
        else:
            self.pending.append(frame)

    async def _send_token_frame(self, conn, token: bytes) -> bytes:
        """发送 token 并等待响应。

        契约（R17）：token 响应 = 裸字节 b"accepted" 或 b"ERR:*"，无 13 字节
        帧头；精确匹配。超时/异常由调用方（状态机）处理。"""
        bi = await conn.open_bi()
        await bi.send().write_all(token)
        await bi.send().finish()
        return bytes(await asyncio.wait_for(bi.recv().read_to_end(1024), timeout=10))

    def _apply_doc_map(self, frame):
        """应用对端发来的 doc_map。只更新 self.doc_map。"""
        try:
            data = json.loads(frame)
            mapping = data.get("mapping", {})
            if mapping:
                self.doc_map = mapping
                # 更新同 scope 中所有 IrohPeer 的 doc_map
                if hasattr(self, '_scope_lookup') and self._scope_lookup is not None:
                    for uid in ([self.wt_uid] if self.wt_uid else []):
                        scope = self._scope_lookup(uid)
                        if scope:
                            for peer in scope.peers.values():
                                if hasattr(peer, 'update_doc_map'):
                                    peer.update_doc_map(mapping)
        except json.JSONDecodeError:
            pass
        except Exception as e:
            import sys
            print(f"[doc_map] apply error: {e}", file=sys.stderr)

    async def _rebuild_doc_map(self):
        """首次连接后：UP_TREE + 本地 hash 比对 → doc_map → 发送到对端。
        mount 侧独占（share 侧由对端发送 doc_map）。"""
        if self.ep is None or self.addr is None:
            return
        try:
            import iroh as _iroh
            import json as _json, struct as _struct
            _ep_client = await _iroh.Endpoint.bind(_iroh.EndpointOptions(preset=_iroh.preset_n0()))
            await _ep_client.online()
            try:
                conn = await _ep_client.connect(self.addr, self.alpn)
                # 发送 token（R5：校验 accepted——第三个发 token 路径与主连接对称）
                if self._saved_token:
                    bi = await conn.open_bi()
                    await bi.send().write_all(self._saved_token)
                    await bi.send().finish()
                    token_resp = bytes(await asyncio.wait_for(bi.recv().read_to_end(1024), timeout=10))
                    if token_resp != b"accepted":
                        return  # 认证失败——重建 doc_map 无意义
                # UP_TREE
                bi2 = await conn.open_bi()
                hdr = _struct.pack("<BIII", 0x01, 0, 0, 0)
                await bi2.send().write_all(hdr)
                await bi2.send().finish()
                tree_resp = bytes(await asyncio.wait_for(bi2.recv().read_to_end(65536), timeout=10))
                if tree_resp[0] != 0x81:
                    return
                remote_files = _json.loads(tree_resp[13:])
                # 本地文件树
                store = self._store
                if store is None:
                    return
                wt = self.wt_uid or "genesis"
                local_json = store.get_vfs_tree(wt, False)
                local_files = _json.loads(local_json.decode()) if local_json else []
                # local_prefix 过滤 + 按 path 匹配
                lp = (self.local_prefix or "").rstrip("/")
                remote_by_path = {}
                for f in remote_files:
                    rp = f.get("path", "")
                    if lp and not rp.startswith(lp):
                        continue
                    remote_by_path[rp] = f
                doc_map = {}
                for f in local_files:
                    lp_local = f.get("path", "")
                    if lp and not lp_local.startswith(lp):
                        continue
                    remote = remote_by_path.get(lp_local)
                    if remote:
                        doc_map[lp_local] = {
                            "local": f["id"],
                            "remote": remote["id"],
                            "hash": remote.get("hash", ""),
                        }
                if doc_map:
                    self.doc_map = doc_map
                    self._doc_map_stale = False
                    # 同步 scope 中所有 IrohPeer（修复：重建后不同步导致
                    # doc_id 重写失效——与 _apply_doc_map 的同步逻辑对齐）
                    if self.wt_uid and getattr(self, '_scope_lookup', None) is not None:
                        scope = self._scope_lookup(self.wt_uid)
                        if scope:
                            for peer in scope.peers.values():
                                if hasattr(peer, 'update_doc_map'):
                                    peer.update_doc_map(doc_map)
                    # 校正后持久化到 gateway.md（mount 条目 doc_map 字段）
                    gs = getattr(self, '_gs', None)
                    if gs is not None and getattr(self, 'entry_id', None):
                        try:
                            gs._update_mount_doc_map(self.wt_uid, self.entry_id, doc_map)
                        except Exception:
                            pass
                    # 发送 doc_map 到对端
                    payload = _json.dumps({"type": "doc_map", "mapping": doc_map}).encode()
                    bi3 = await conn.open_bi()
                    await bi3.send().write_all(payload)
                    await bi3.send().finish()
                    await asyncio.wait_for(bi3.recv().read_to_end(1024), timeout=5)
                    # doc_map 建立后，通过主连接发 UP_OPEN 在 A 侧注册 doc 订阅
                    for path, info in doc_map.items():
                        remote_id = info.get("remote")
                        if remote_id is not None:
                            hdr = _struct.pack("<BIII", 0x02, 0, 0, remote_id)
                            bi = await conn.open_bi()
                            await bi.send().write_all(hdr)
                            await bi.send().finish()
                            await asyncio.wait_for(bi.recv().read_to_end(65536), timeout=10)
            finally:
                await _ep_client.shutdown()
        except Exception:
            pass

    def _handle_file_op(self, action, store):
        """用 self.wt_uid 和 self.local_prefix 执行 FileOp。
        handler 自身上下文已知，无需 crc32 反查。delegate 到统一 _exec_file_op。
        """
        op_json = action.file_action_op_json
        if not op_json or not self.wt_uid:
            return
        _exec_file_op(store, self.wt_uid, self.local_prefix or "/", op_json)


# ProtocolCreator — 每连接一个 ConnectionHandler

# 兼容包装

# Keep old name as alias
IrohConnectionHandler = ConnectionHandler


def make_iroh_creator(gateway_store):
    """创建 iroh.ProtocolCreator。使用 GatewayStore 注入 Handler 依赖。"""
    import iroh as _iroh

    class _Creator(_iroh.ProtocolCreator):
        def create(self, endpoint):
            # A4：share 侧被动连接初始为握手态——只收 token 帧，认证成功才进 connected
            handler = ConnectionHandler(initial_state="handshaking")
            handler.ep = endpoint  # 供重连使用
            # GatewayStore 注入
            handler._store = gateway_store._store
            handler._lookup = gateway_store.lookup_token
            handler._is_token = gateway_store.is_token
            handler._scope_lookup = lambda uid: gateway_store._scopes.get(uid)
            handler.on_state_change = gateway_store._on_handler_event

            class _Handler(_iroh.ProtocolHandler):
                async def accept(self, conn):
                    await handler.run(conn)

                async def shutdown(self):
                    pass

            return _Handler()

    return _Creator()
