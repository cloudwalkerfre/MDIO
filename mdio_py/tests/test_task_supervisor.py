"""task_supervisor —— 长驻任务监护的判据（离线，真 asyncio，无插件依赖）。

为什么值得单测：这条壳是 R29「文档全白而 exec 全好」那类事故的**兜底**——它必须做到
① 正常返回不重起；② 异常才重起且**退避**（不许忙循环）；③ 关机取消不是事故；
④ 崩太多次要有上限并留下读数；⑤ 读数（snapshot）能被 /control/status 直接查。
"""

import asyncio
import time

import pytest

import mdio_py.gateway.task_supervisor as ts
from mdio_py.gateway.task_supervisor import TaskSupervisor, fault_gate


def test_restarts_on_crash_then_succeeds():
    calls = {"n": 0}

    async def factory():
        calls["n"] += 1
        if calls["n"] <= 2:
            raise RuntimeError("桥炸了 #%d" % calls["n"])
        return None

    async def main():
        sup = TaskSupervisor("t", factory, retry_delay=0.01, max_delay=0.02, log=lambda m: None)
        await sup.run()
        return sup

    sup = asyncio.run(main())
    assert calls["n"] == 3, calls
    assert sup.restarts == 2, sup.snapshot()
    assert sup.running is False
    assert sup.stopped_at is not None
    assert "RuntimeError" in (sup.last_error or "")


def test_normal_return_is_not_an_incident():
    async def factory():
        return None

    sup = asyncio.run(_run(factory))
    assert sup.restarts == 0, sup.snapshot()
    assert sup.running is False
    assert sup.last_error is None


async def _run(factory, **kw):
    sup = TaskSupervisor("t", factory, log=lambda m: None, **kw)
    await sup.run()
    return sup


def test_cancel_is_not_a_crash():
    started = asyncio.Event()

    async def factory():
        started.set()
        await asyncio.sleep(30)

    async def main():
        sup = TaskSupervisor("t", factory, log=lambda m: None)
        task = asyncio.get_running_loop().create_task(sup.run())
        await started.wait()
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        return sup, task

    sup, task = asyncio.run(main())
    assert task.cancelled(), "取消应当原样抛出 CancelledError"
    assert sup.restarts == 0, sup.snapshot()


def test_max_restarts_gives_up():
    calls = {"n": 0}

    async def factory():
        calls["n"] += 1
        raise ValueError("一直炸")

    sup = asyncio.run(_run(factory, retry_delay=0.01, max_restarts=2))
    # 初始 1 次 + 最多 2 次重起 = 3 次失败后放弃
    assert calls["n"] == 3, calls
    assert sup.restarts == 3, sup.snapshot()


def test_backoff_is_not_a_busy_loop():
    async def factory():
        raise ValueError("炸")

    t0 = time.monotonic()
    asyncio.run(_run(factory, retry_delay=0.05, max_delay=0.2, max_restarts=2))
    dt = time.monotonic() - t0
    # 两次重起之间至少睡 0.05 + 0.10（翻倍）——忙循环的耗时会远小于这个
    assert dt >= 0.14, "重起之间没有退避：%.3fs" % dt


def test_fault_gate_injects_once_then_passes(monkeypatch):
    """env 闸门：`ast_bridge:1` ⇒ 第一次抛、第二次放行；没点名的任务永远不抛。"""
    monkeypatch.setenv("MDIO_FAULT_TASKS", "ast_bridge:1, tick:0")
    monkeypatch.setattr(ts, "_FAULT_BUDGET", None)      # 重读 env
    with pytest.raises(RuntimeError, match="fault-injected"):
        fault_gate("ast_bridge")
    fault_gate("ast_bridge")                            # 预算用完 ⇒ 放行
    fault_gate("tick")                                  # 0 ⇒ 放行
    fault_gate("other")                                 # 没点名 ⇒ 放行


def test_fault_gate_off_by_default(monkeypatch):
    monkeypatch.delenv("MDIO_FAULT_TASKS", raising=False)
    monkeypatch.setattr(ts, "_FAULT_BUDGET", None)
    fault_gate("ast_bridge")                            # 不该抛


def test_fault_gate_drives_supervisor_restart(monkeypatch):
    """把注入接进监护壳：第一次起跑前被注入炸掉 ⇒ 重起一次后正常结束（restarts == 1）。"""
    monkeypatch.setenv("MDIO_FAULT_TASKS", "injected:1")
    monkeypatch.setattr(ts, "_FAULT_BUDGET", None)

    async def factory():
        fault_gate("injected")
        return None

    sup = asyncio.run(_run(factory, retry_delay=0.01))
    assert sup.restarts == 1, sup.snapshot()
    assert "fault-injected" in (sup.last_error or ""), sup.snapshot()
    assert sup.stopped_at is not None


def test_snapshot_fields():
    async def factory():
        return None

    sup = asyncio.run(_run(factory))
    snap = sup.snapshot()
    for k in ("name", "running", "restarts", "last_error", "last_error_at", "started_at", "stopped_at"):
        assert k in snap, k
    assert snap["name"] == "t"
