"""task_supervisor —— 长驻后台任务的**监护**（崩溃可见 + 有界重起 + 可查读数）。

背景：`attach_gateway` 起的两条长驻任务（AstBridge 消费者、GatewayStore tick）必须是监护壳 ——
一旦异常退出，进程照样跑、exec 照样好，只是**再也没有补丁广播**："文档全白、而 exec 全好"是这种停摆的现场。
裸 task 的失败在 asyncio 里只会在 GC 时打一句 "Task exception was never retrieved"（常常没人看）。

纪律（与客户端侧同一条）：
  · **正常返回不重起**（循环自己决定结束 = 结束；只有异常才算事故）；
  · **CancelledError 直接放行**（关机取消不是事故，不许重起）；
  · 重起**退避**（0.5s 起、翻倍、封顶 8s）—— 崩了立刻再崩的任务不会变成忙循环；
  · 每次事故**大声打**（任务名 + 异常 + 第几次），并把读数留在 `snapshot()` 里供 `/control/status` 查。
"""

from __future__ import annotations

import asyncio
import os
import threading
import time
import traceback
from typing import Any, Awaitable, Callable

__all__ = ["supervise", "TaskSupervisor", "fault_gate"]

_FAULT_LOCK = threading.Lock()
_FAULT_BUDGET: dict[str, int] | None = None


def _fault_budget() -> dict[str, int]:
    """解析 env 闸门 `MDIO_FAULT_TASKS="ast_bridge:1,tick:0"`（只读一次）。"""
    global _FAULT_BUDGET
    if _FAULT_BUDGET is None:
        budget: dict[str, int] = {}
        for item in (os.environ.get("MDIO_FAULT_TASKS") or "").split(","):
            item = item.strip()
            if not item or ":" not in item:
                continue
            name, _, n = item.partition(":")
            try:
                budget[name.strip()] = int(n)
            except ValueError:
                pass
        _FAULT_BUDGET = budget
    return _FAULT_BUDGET


def fault_gate(name: str) -> None:
    """**故障注入闸门**（默认关）：任务第 N 次起跑前抛一次，用来活体验证监护壳的重起路径。

    为什么需要它：重起/退避路径只有单测覆盖 —— 要"真杀一次桥、看它自己回来"，
    得有一条**确定性**的注入点；去 kill 进程杀的是整个网关，不是这一条任务。
    用法：`MDIO_FAULT_TASKS=ast_bridge:1` 起网关 ⇒ 该任务第一次起跑前抛，监护壳接住并重起，
    `/control/status` 的 `tasks.<名>.restarts` 会变成 1。
    """
    with _FAULT_LOCK:
        budget = _fault_budget()
        n = budget.get(name, 0)
        if n <= 0:
            return
        budget[name] = n - 1
    raise RuntimeError(f"fault-injected crash of {name}（MDIO_FAULT_TASKS）")


class TaskSupervisor:
    """一条长驻任务的监护壳：`await sup.run()` 在 task 里跑。"""

    def __init__(
        self,
        name: str,
        factory: Callable[[], Awaitable[Any]],
        *,
        retry_delay: float = 0.5,
        max_delay: float = 8.0,
        max_restarts: int | None = None,
        log: Callable[[str], None] | None = None,
    ) -> None:
        self.name = name
        self._factory = factory
        self._retry_delay = max(0.0, float(retry_delay))
        self._max_delay = max(self._retry_delay, float(max_delay))
        self._max_restarts = max_restarts
        self._log = log or (lambda msg: print(msg, flush=True))
        self.restarts = 0
        self.running = False
        self.last_error: str | None = None
        self.last_error_at: float | None = None
        self.started_at: float | None = None
        self.stopped_at: float | None = None

    async def run(self) -> None:
        delay = self._retry_delay
        while True:
            self.running = True
            if self.started_at is None:
                self.started_at = time.time()
            try:
                await self._factory()
                self.running = False
                self.stopped_at = time.time()
                self._log(f"[supervisor] {self.name} 正常结束")
                return
            except asyncio.CancelledError:
                self.running = False
                self.stopped_at = time.time()
                raise
            except Exception as e:  # noqa: BLE001 —— 监护的意义就是接住一切
                self.running = False
                self.restarts += 1
                self.last_error = f"{type(e).__name__}: {e}"
                self.last_error_at = time.time()
                self._log(
                    f"[supervisor] {self.name} 异常退出（第 {self.restarts} 次）：{self.last_error}\n"
                    + traceback.format_exc()
                )
                if self._max_restarts is not None and self.restarts > self._max_restarts:
                    self._log(f"[supervisor] {self.name} 超过重起上限（{self._max_restarts}），不再重起")
                    self.stopped_at = time.time()
                    return
                await asyncio.sleep(delay)
                delay = min(self._max_delay, max(self._retry_delay, delay * 2))
            finally:
                self.running = False

    def snapshot(self) -> dict:
        """可查读数（`/control/status` 用）：活着吗、崩过几次、最后一条错是什么。"""
        return {
            "name": self.name,
            "running": self.running,
            "restarts": self.restarts,
            "last_error": self.last_error,
            "last_error_at": self.last_error_at,
            "started_at": self.started_at,
            "stopped_at": self.stopped_at,
        }


def supervise(name: str, factory: Callable[[], Awaitable[Any]], **kwargs) -> tuple[TaskSupervisor, asyncio.Task]:
    """建监护壳并作为 task 起跑：返回 (supervisor, task)，调用方留 task 以便关机取消。"""
    sup = TaskSupervisor(name, factory, **kwargs)
    task = asyncio.get_running_loop().create_task(sup.run(), name=f"supervised:{name}")
    return sup, task
