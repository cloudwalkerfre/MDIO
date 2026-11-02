"""CrdtScope — 每个声明了 gateway 的 WT 持有一个实例。

管理该 WT 的 peer 集合和文档级订阅。
broadcast(source) 跳过来源 peer——这是唯一的防回环规则。
不区分 WS 和 iroh peer。"""

from __future__ import annotations
import os
import asyncio
import logging
from collections import defaultdict
from .peer import Peer


# ── 广播打点（走 logger，默认 WARNING 级别：不打）──────────────────
# 这些是 INFO 级诊断（投递结局 no_scope / no_subscribers）。无头、无 gateway 场景下
# 每次写入都会触发一次投递，直接 print 会刷屏淹没日志；排错时把 level 调成
# logging.INFO（或 logging.DEBUG）即可看到丢弃记录。
_bcast_log = logging.getLogger("mdio.bcast")
_bcast_log.setLevel(logging.WARNING)


# ── 投递唯一出口 ────────────────────────────────────────────────
# kind 决定回声策略：TextOp 跳过来源（发起端已本地应用），其余一律不跳过。
KIND_TEXT = "TextOp"
KIND_AST = "Ast"
KIND_RELINEAGE = "Relineage"
KIND_FILE = "FileOp"


async def deliver(lookup, doc_id: int, frame: bytes, kind: str,
                  origin: str | None = None, store=None) -> str:
    """唯一投递出口。按「文档的归属 wt」找 scope，按 kind 决定回声策略，返回结局。

    - lookup：GatewayStore（权威表）或 iroh 的 scope_lookup 可调用体
    - store：Rust ProjectStore（用于 doc_wt_uid）；lookup 是 GatewayStore 时可省
    - origin：**只接受真实 peer id 或 None**（伪 id 一律传 None）
    - 结局："queued:N"（已入队 N 条连接，**不代表对端已应用**）/ "no_subscribers" / "no_scope"
    """
    # 投递索引在 GatewayStore 上（doc → {peer}，**不查 wt**）⇒ 有它就委托。
    if hasattr(lookup, "deliver") and hasattr(lookup, "_doc_subs"):
        return await lookup.deliver(doc_id, frame, kind, origin=origin)
    st = store if store is not None else getattr(lookup, "_store", None)
    uid = None
    if st is not None:
        try:
            uid = st.doc_wt_uid(doc_id) or None
        except Exception:
            uid = None
    uid = uid or "genesis"
    if hasattr(lookup, "get_scope"):
        scope = lookup.get_scope(uid)
        if scope is None and hasattr(lookup, "_get_or_create_scope"):
            scope = lookup._get_or_create_scope(uid)
    elif callable(lookup):
        scope = lookup(uid)
    else:
        scope = None
    if scope is None:
        _bcast_log.info("[BCAST-MISS] doc=%s uid=%s kind=%s ⇒ 无 scope，%dB 丢弃",
                        doc_id, uid, kind, len(frame))
        return "no_scope"
    n = len(scope.subscriptions.get(doc_id, ()))
    if n == 0:
        _bcast_log.info("[BCAST-MISS] doc=%s uid=%s kind=%s ⇒ 无订阅者，%dB 丢弃",
                        doc_id, uid, kind, len(frame))
        return "no_subscribers"
    source = origin if (kind == KIND_TEXT and origin) else ""
    await scope.broadcast(frame, doc_id, source_peer_id=source)
    return f"queued:{n}"


async def push_relineage(store, scope, doc_ids, reason: str = "") -> int:
    """血统变更后**主动**推送换代：给每个 doc 的订阅者广播一帧 DN_OPEN（当前 checkpoint）。

    为什么必须有这条推送：血统换了以后，旧副本的集成结果不是"报错"而是**永久静默**
    （cola 返回 None → 进 pending，见 research/2026-09-21-lineage-single-source-model.md §1）。
    被动等"下一次服务端写入"才暴露太晚——换血统的那一刻就把新 checkpoint 推下去，
    客户端凭帧里的血统号立刻整篇重取（editor-core 的 DnOpen/DnSync 前置检查）。

    幂等：客户端发现血统号没变就跳过重取（DnOpen 分支）；这里对没有订阅者的 doc 不做事。
    返回实际推送的帧数。**不是轮询**——只在血统确实变了的那一处调用。
    """
    sent = 0
    for doc_id in doc_ids:
        did = int(doc_id)
        pkt = store.build_dn_open(did, 0)
        if not pkt:
            continue
        # 走唯一出口（kind=Relineage ⇒ 不抑制任何来源）；无订阅者时它自己记结局
        r = await deliver(scope, did, bytes(pkt), KIND_RELINEAGE, store=store)
        if r.startswith("queued:"):
            sent += 1
    if sent:
        _trace(f"[RELINEAGE] reason={reason} docs={[int(d) for d in doc_ids]} 推送换代 {sent} 帧")
    return sent


def _trace(msg: str) -> None:
    if os.environ.get("MDIO_WS_TRACE") == "1":
        print(msg, flush=True)


class CrdtScope:
    """WT 级别的 CRDT 参与者管理。

    一个 WT 一个 scope。scope 内管理：
    - peers: 所有连接的参与者（WS 和 iroh 不区分）
    - subscriptions: doc_id → 订阅该文档的 peer_id 集合
    """

    wt_uid: str
    peers: dict[str, Peer]
    subscriptions: dict[int, set[str]]

    def __init__(self, wt_uid: str):
        self.wt_uid = wt_uid
        self.peers = {}
        self.subscriptions = defaultdict(set)
        # 广播发送失败的「连续」计数（成功即清零）。只用于告警/观测，
        # 不作为摘 peer 的依据——摘 peer 是连接生命周期的职责。见 BUGS 复盘 A4。
        self._send_failures: dict[str, int] = {}

    # ── Peer 生命周期 ──────────────────────────────────────────

    def add_peer(self, peer: Peer) -> None:
        """注册一个新 peer（WS 或 iroh）。"""
        _trace(f"[SCOPE] add_peer {peer.peer_id} wt={self.wt_uid} peers={len(self.peers) + 1}")
        self.peers[peer.peer_id] = peer

    def remove_peer(self, peer_id: str) -> None:
        """移除 peer 并清理其所有订阅。"""
        _n = sum(1 for s in self.subscriptions.values() if peer_id in s)
        _trace(f"[SCOPE] remove_peer {peer_id} wt={self.wt_uid} 连带清掉订阅数={_n} ← 若这里清的是活连接，订阅就没了")
        self.peers.pop(peer_id, None)
        self._send_failures.pop(peer_id, None)
        for doc_subs in self.subscriptions.values():
            doc_subs.discard(peer_id)

    # ── 订阅管理 ───────────────────────────────────────────────

    def subscribe(self, peer_id: str, doc_id: int) -> None:
        """peer 订阅某个文档的广播。"""
        self.subscriptions[doc_id].add(peer_id)
        _trace(f"[SCOPE] subscribe {peer_id} doc={doc_id} wt={self.wt_uid} subs={len(self.subscriptions[doc_id])}")

    def unsubscribe(self, peer_id: str, doc_id: int) -> None:
        """peer 取消订阅。"""
        subs = self.subscriptions.get(doc_id)
        if subs:
            subs.discard(peer_id)
            _trace(f"[SCOPE] unsubscribe {peer_id} doc={doc_id} wt={self.wt_uid} 剩余={len(subs)} ← 注意：客户端对一个文档有两个会话，退掉任一都会到这里")

    # ── 广播 ───────────────────────────────────────────────────

    async def broadcast(self, frame: bytes, doc_id: int,
                        source_peer_id: str) -> None:
        """向订阅了 doc_id 的所有 peer 并发广播帧。

        跳过 source_peer_id——这是唯一的防回环规则。
        source 可以是 ws_abc 或 iroh_xyz，语义一致。
        并发 send：WS 瞬时完成，iroh 走 open_uni() 单向流，QUIC 原生流控。
        """
        subs = self.subscriptions.get(doc_id, set())
        # 排错看 [BCAST-MISS] / [ADMIT]
        if not subs:
            return

        async def _send_one(pid: str) -> None:
            if pid == source_peer_id:
                return
            peer = self.peers.get(pid)
            if peer is None:
                return
            try:
                await peer.send(frame)
            except Exception as e:
                # 广播路径不得改状态：一次发送失败（对端缓冲满 / QUIC 流被重置）
                # 不等于该连接已死；摘 peer 是连接生命周期的职责（WS reader 退出 /
                # iroh stop / handshake_fail）。这里只做连续失败计数 + 告警。
                n = self._send_failures.get(pid, 0) + 1
                self._send_failures[pid] = n
                print(f"⚠️ [SCOPE] send failed peer={pid} wt={self.wt_uid} doc={doc_id} "
                      f"failures={n}: {e!r}", flush=True)
            else:
                self._send_failures.pop(pid, None)

        await asyncio.gather(
            *(_send_one(pid) for pid in subs),
            return_exceptions=True,
        )
