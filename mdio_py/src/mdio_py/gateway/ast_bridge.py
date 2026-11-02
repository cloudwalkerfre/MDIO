"""AstBridge — AST Dispatch Thread 的 Python 异步桥接器。

这条桥上接 Rust（Path Worker 回调 `_on_packet`）、下接 WS 广播（`consumer`），
中间只有一个 asyncio.Queue。它停摆时进程、exec、WS 全都正常，**只是没有补丁** ——
"文档全白而 exec 全好"是这种停摆的现场。所以：
  · 入队 / 广播 / 错误三个计数 + 各自的时间戳：判"停摆"必须同时看**上游有没有发**与**下游有没有播**；
  · `_loop` 在 **consumer 起跑时**绑定到运行中的 loop：
    抓早了或抓到别的 loop ⇒ `call_soon_threadsafe` 全部失败 ⇒ **一条补丁都进不来**而毫无异常；
  · 起跑前到达的包记一笔 `dropped_early`（启动窗口内的静默丢失，不该假装没发生）。
"""

import asyncio
import time


class AstBridge:
    def __init__(self, store):
        self.store = store
        self.queue: asyncio.Queue = asyncio.Queue()
        self._loop = None            # 在 consumer 起跑时绑定（见 consumer 的注释）
        self._up_sync_times = {}     # doc_id → perf_counter when UpSync was received
        # ── 流量读数：上游入队 / 下游广播 / 广播异常 ──
        self.enqueued = 0
        self.broadcasted = 0
        self.errors = 0
        self.dropped_early = 0
        self.last_enqueue_at = None
        self.last_broadcast_at = None
        self.last_error = None
        store.register_ast_callback(self._on_packet)

    def bind_loop(self, loop=None):
        """把事件循环绑到**当下正在跑的那条**（启动路径上直接在 running loop 里调）。

        为什么要显式绑一次：`__init__` 里注册了 Rust 回调，而消费者起跑前有一个窗口 ——
        这期间来的包只能记进 `dropped_early`（丢了）。在 attach 时先绑好，窗口就没了
        （实测：故障注入重启时 `dropped_early=1`，正是这个窗口里丢的一包）。
        """
        self._loop = loop if loop is not None else asyncio.get_running_loop()
        return self._loop

    def mark_up_sync(self, doc_id: int):
        self._up_sync_times[doc_id] = time.perf_counter()

    def _on_packet(self, packet: bytes):
        """Rust 侧回调（可能在别的线程）：入队到 asyncio 队列。"""
        loop = self._loop
        if loop is None:
            self.dropped_early += 1     # 消费者还没起跑 —— 记账，不静默
            return
        self.enqueued += 1
        self.last_enqueue_at = time.time()
        loop.call_soon_threadsafe(self.queue.put_nowait, packet)

    async def consumer(self, broadcast_fn):
        # **绑定当下的运行中 loop**（不是构造时的那条）：绑定时机见 `bind_loop`。
        self._loop = asyncio.get_running_loop()
        while True:
            packet = await self.queue.get()
            try:
                doc_id = int.from_bytes(packet[9:13], "little")
                t_process = time.perf_counter()
                t_recv = self._up_sync_times.pop(doc_id, None)
                gap = f" gap={((t_process - t_recv) * 1000):.1f}ms" if t_recv else ""
                # print(f"[AstBridge] broadcast doc={doc_id}{gap}")  # 热路径 print 注释
                _out = await broadcast_fn(doc_id, packet)
                self.broadcasted += 1
                self.last_broadcast_at = time.time()
            except Exception as e:
                self.errors += 1
                self.last_error = f"{type(e).__name__}: {e}"
                print(f"[AstBridge] broadcast error: {e}")
            finally:
                self.queue.task_done()

    def health(self, stall_after: float = 5.0) -> dict:
        """可查读数（`/control/status` 用）+ **停摆判定**。

        `stalled` 判据 = 有包入队、却还有积压（入队 - 广播 - 出错 > 0）且最近一次广播已超过 stall_after
        （或从未广播过）。只判 task 活着不够 —— 上游停发时消费者只是闲着，那不该叫 stalled。
        """
        now = time.time()
        age = None if self.last_broadcast_at is None else round(now - self.last_broadcast_at, 3)
        backlog = self.enqueued - self.broadcasted - self.errors
        stalled = bool(backlog > 0 and (age is None or age > stall_after))
        return {
            "enqueued": self.enqueued,
            "broadcasted": self.broadcasted,
            "errors": self.errors,
            "dropped_early": self.dropped_early,
            "backlog": backlog,
            "queue_size": self.queue.qsize(),
            "last_enqueue_at": self.last_enqueue_at,
            "last_broadcast_at": self.last_broadcast_at,
            "age_since_broadcast": age,
            "last_error": self.last_error,
            "loop_bound": self._loop is not None,
            "stalled": stalled,
        }
