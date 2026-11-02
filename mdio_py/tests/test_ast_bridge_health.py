"""AstBridge 的流量读数与停摆判定（离线）。

停摆判据须同时看「上游有没有发」（enqueued）与「下游有没有播」（broadcasted/errors），
以及起跑前丢了多少。
"""

import asyncio

from mdio_py.gateway.ast_bridge import AstBridge


class FakeStore:
    def __init__(self):
        self.cb = None

    def register_ast_callback(self, cb):
        self.cb = cb


def _pkt(doc_id: int, st: int = 4) -> bytes:
    """帧头 13 字节 + st（第 14 字节）+ 载荷——消费者只读 docId 与 st。"""
    return b"\x00" * 9 + doc_id.to_bytes(4, "little") + bytes([st]) + b"\x00\x00\x00\x00" + b"{}"


def test_dropped_early_before_loop_bound():
    b = AstBridge(FakeStore())
    b._on_packet(_pkt(7))
    h = b.health()
    assert h["dropped_early"] == 1, h        # 起跑前到达：记账，不假装没发生
    assert h["enqueued"] == 0 and h["loop_bound"] is False, h


def test_bind_loop_closes_the_early_window():
    """启动路径先 bind_loop ⇒ 构造之后来的包直接入队，不进入 dropped_early。"""

    async def main():
        b = AstBridge(FakeStore())
        b.bind_loop()
        b._on_packet(_pkt(9))
        await asyncio.sleep(0)
        return b

    b = asyncio.run(main())
    h = b.health()
    assert h["loop_bound"] is True and h["dropped_early"] == 0 and h["enqueued"] == 1, h


def test_flow_counters_and_not_stalled():
    async def main():
        b = AstBridge(FakeStore())
        seen = []

        async def bcast(doc_id, packet):
            seen.append(doc_id)

        task = asyncio.get_running_loop().create_task(b.consumer(bcast))
        await asyncio.sleep(0)               # 让 consumer 绑定 loop
        assert b.health()["loop_bound"] is True
        for _ in range(3):
            b._on_packet(_pkt(11))
        for _ in range(30):
            await asyncio.sleep(0)
            if b.broadcasted == 3:
                break
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        return b, seen

    b, seen = asyncio.run(main())
    h = b.health()
    assert h["enqueued"] == 3 and h["broadcasted"] == 3 and h["errors"] == 0, h
    assert h["backlog"] == 0 and h["stalled"] is False, h
    assert seen == [11, 11, 11]


def test_stalled_when_backlog_and_never_broadcast():
    async def main():
        b = AstBridge(FakeStore())

        async def noop(doc_id, packet):
            return None

        task = asyncio.get_running_loop().create_task(b.consumer(noop))
        await asyncio.sleep(0)               # 绑定 loop
        task.cancel()                        # 消费者退场 ⇒ 包只进不出
        try:
            await task
        except asyncio.CancelledError:
            pass
        b._on_packet(_pkt(5))
        await asyncio.sleep(0)               # 让 call_soon_threadsafe 落地
        return b

    b = asyncio.run(main())
    h = b.health(stall_after=0)
    assert h["backlog"] == 1 and h["stalled"] is True, h
    assert h["age_since_broadcast"] is None, h      # 从未广播过


def test_broadcast_error_counted_and_not_stalled():
    async def main():
        b = AstBridge(FakeStore())

        async def bad(doc_id, packet):
            raise RuntimeError("peer 炸了")

        task = asyncio.get_running_loop().create_task(b.consumer(bad))
        await asyncio.sleep(0)
        b._on_packet(_pkt(3))
        for _ in range(30):
            await asyncio.sleep(0)
            if b.errors == 1:
                break
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        return b

    b = asyncio.run(main())
    h = b.health()
    assert h["errors"] == 1 and "RuntimeError" in (h["last_error"] or ""), h
    assert h["backlog"] == 0 and h["stalled"] is False, h
