"""Peer — CRDT 协议的统一参与者抽象。

WS 和 iroh 是 Peer 的两种 transport 实现。没有语义差异——
都是"一个可以收发 CRDT 帧的东西"。

IrohPeer 持有 doc_map，send 时自动重写帧头的 doc_id：
  A 侧 local doc_id=42 → 重写为 B 侧 remote doc_id=99
接收方向无需重写——B 发来的帧已带 A 侧的 doc_id。
"""

from __future__ import annotations
import struct
from typing import Callable, Awaitable

SendFn = Callable[[bytes], Awaitable[None]]
HEADER_SIZE = 13


class Peer:
    """CRDT 参与者。"""
    peer_id: str
    transport: str          # "ws" | "iroh"
    _raw_send: SendFn       # 底层 transport 发送

    def __init__(self, peer_id: str, transport: str, send: SendFn):
        self.peer_id = peer_id
        self.transport = transport
        self._raw_send = send

    async def send(self, frame: bytes) -> None:
        """发送 CRDT 帧。子类可覆盖以改写帧头。"""
        # 真正的写 socket 时刻（channel 订阅者后台任务 → peer.send → send_bytes）。
        # env 门控 MDIO_FRAME_TIMING=1，默认关；t= 与 [FT] rx/pj 的 t= 同钟，可算排队与总往返。
        import os as _ft_os
        if _ft_os.environ.get("MDIO_FRAME_TIMING") == "1":
            import time as _ft_time
            _ft_t0 = _ft_time.perf_counter()
            await self._raw_send(frame)
            _ft_t1 = _ft_time.perf_counter()
            print(f"[FT] tx kind=peer t={_ft_time.time():.6f} bytes={len(frame)} send={(_ft_t1 - _ft_t0) * 1000:.2f}ms", flush=True)
            return
        await self._raw_send(frame)

    def __repr__(self) -> str:
        return f"Peer({self.peer_id}, {self.transport})"


class WsPeer(Peer):
    """WebSocket transport peer。"""

    def __init__(self, peer_id: str, send: SendFn):
        super().__init__(peer_id, "ws", send)


class IrohPeer(Peer):
    """iroh QUIC transport peer。

    持有 doc_map（path → {local, remote, hash}），send 时自动
    将帧头的 local doc_id 重写为 remote doc_id。

    doc_map 在 mount 时（iroh_mount）由远端 DN_TREE 的
    [{path, id, hash}] + 本地 register 后的 local doc_id 建立。
    """

    def __init__(self, peer_id: str, send: SendFn,
                 doc_map: dict[str, dict] | None = None):
        super().__init__(peer_id, "iroh", send)
        self.doc_map: dict[str, dict] = doc_map or {}
        self._remote_to_local: dict[int, int] = {}  # A→B
        self._local_to_remote: dict[int, int] = {}  # B→A
        self._path_to_local: dict[str, int] = {}
        self._rebuild_index()

    def _rebuild_index(self):
        self._remote_to_local.clear()
        self._local_to_remote.clear()
        self._path_to_local.clear()
        for path, info in self.doc_map.items():
            local = info.get("local")    # B 侧 doc_id
            remote = info.get("remote")  # A 侧 doc_id
            if local is not None and remote is not None:
                self._remote_to_local[remote] = local  # A→B
                self._local_to_remote[local] = remote  # B→A
                self._path_to_local[path] = local

    def update_doc_map(self, doc_map: dict[str, dict]):
        self.doc_map = doc_map
        self._rebuild_index()

    async def send(self, frame: bytes) -> None:
        """发送帧，自动双向重写 doc_id。

        A→B: A 的 doc_id → B 的 local（_remote_to_local）
        B→A: B 的 local → A 的 remote（_local_to_remote）
        """
        if not self._remote_to_local and not self._local_to_remote:
            await self._raw_send(frame)
            return
        if len(frame) < HEADER_SIZE:
            await self._raw_send(frame)
            return
        src_id = struct.unpack_from('<I', frame, 9)[0]
        # 先尝试 B→A (local → remote), 再尝试 A→B (remote → local)
        dst_id = self._local_to_remote.get(src_id) or self._remote_to_local.get(src_id)
        if dst_id is None:
            await self._raw_send(frame)
            return
        new_frame = bytearray(frame)
        struct.pack_into('<I', new_frame, 9, dst_id)
        await self._raw_send(bytes(new_frame))
