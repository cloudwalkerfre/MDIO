"""proc.md 运行实例（#### wl）回归：状态隔离、hook/step、close 遍历、caller 回写。"""

import asyncio

import pytest

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.llm.llm import (
    LLMPlugin,
    _append_record,
    _ensure_wl_hook,
    _read_state,
    _write_proc,
)


def _ctx():
    return RuntimeContext(wt_uid="genesis")


async def test_proc_instance_isolation_and_multiline(runtime_llm):
    """每个 #### wl 只读自己的直接字段；多行 cb 用 bq 存取不截断。"""
    ws, ctx = runtime_llm._workspace, _ctx()
    _write_proc(ws, "dev", "main", ctx, status="IDLE", turn="0",
                cb="第一行\n第二行")
    _write_proc(ws, "dev", "wl_a", ctx, status="BUSY", turn="7")

    main = _read_state(ws, "dev", "main", ctx)
    a = _read_state(ws, "dev", "wl_a", ctx)
    assert main["status"] == "IDLE" and main["turn"] == "0"
    assert main["cb"] == "第一行\n第二行"
    assert a["status"] == "BUSY" and a["turn"] == "7"
    assert a.get("cb", "") == ""

    raw = str(await runtime_llm.exec("$ /.mds/proc.md -pj raw", wt_uid="genesis"))
    assert "#### main" in raw and "#### wl_a" in raw
    assert "  > 第一行" in raw and "  > 第二行" in raw


async def test_hook_driven_run_creates_instances_and_close_traverses(runtime_llm, monkeypatch):
    """真实 hook/step 链：一个 agent 两条 wl 各自跑完；close 遍历全部实例复位。"""
    async def fake_secrets(self, ws_, alias):
        return {"model": "fake", "api_key": "x", "backend": "fake"}

    async def fake_infer(self, messages, secrets, is_stream, tools=None, num_retries=1):
        yield ("content", "ok")

    monkeypatch.setattr(LLMPlugin, "_get_god_secrets", fake_secrets)
    monkeypatch.setattr(LLMPlugin, "_infer_and_stream", fake_infer)

    rt = runtime_llm
    ws, ctx = rt._workspace, _ctx()
    plugin = LLMPlugin()
    r = await plugin(ws, "", ["dev", "init"], ctx, {"-m": "FAKE", "-tool": "false"})
    assert "initialized" in str(r), r

    for wl, msg in (("wl_a", "hi a"), ("wl_b", "hi b")):
        r = await plugin(ws, "", ["dev"], ctx, {"-wl": wl})
        assert "updated" in str(r), r
        r = await asyncio.wait_for(plugin(ws, msg, ["dev", "run"], ctx, {}), timeout=20)
        assert str(r) == "ok", r
        st = _read_state(ws, "dev", wl, ctx)
        assert st["status"] == "DONE", st

    raw = str(await rt.exec("$ /.mds/proc.md -pj raw", wt_uid="genesis"))
    assert "#### main" in raw and "#### wl_a" in raw and "#### wl_b" in raw

    r = await plugin(ws, "", ["dev", "close"], ctx, {})
    assert "closed" in str(r), r
    for wl in ("main", "wl_a", "wl_b"):
        st = _read_state(ws, "dev", wl, ctx)
        assert st["status"] == "IDLE", (wl, st)
        assert str(st.get("sys_hook_id", "0")) == "0", (wl, st)


async def test_caller_wl_writes_back_to_parent_instance(runtime_llm):
    """子实例完成时，tool_result 回写父 agent 的对应 wl，并唤醒父实例。"""
    rt = runtime_llm
    ws, ctx = rt._workspace, _ctx()
    await rt.exec("mk -f /p.md", wt_uid="genesis")
    await rt.exec('"## HIS\n" | +> /p.md::', wt_uid="genesis")
    await rt.exec("mk -f /c.md", wt_uid="genesis")
    await rt.exec('"## HIS\n" | +> /c.md::', wt_uid="genesis")

    plugin = LLMPlugin()
    r = await plugin(ws, "", ["dev", "init"], ctx,
                     {"-m": "FAKE", "-his": "/p.md::HIS", "-tool": "false"})
    assert "initialized" in str(r), r

    child_cfg = {"his": "/c.md::HIS", "m": "FAKE"}
    _write_proc(ws, "dev", "wl_a", ctx, status="BUSY", turn="1", step="1")
    _write_proc(ws, "sub", "main", ctx, status="DONE",
                caller="dev", caller_wl="wl_a", tc_id="tc_1")
    _append_record(ws, "sub", "/c.md::HIS", "assistant", "sub-ok", child_cfg, ctx)

    LLMPlugin()._handle_terminal_sync(
        ws, "sub", "main",
        _read_state(ws, "sub", "main", ctx), child_cfg,
    )

    parent_his = str(await rt.exec("$ /p.md::HIS -pj raw", wt_uid="genesis"))
    assert "sub-ok" in parent_his, parent_his
    parent_state = _read_state(ws, "dev", "wl_a", ctx)
    assert parent_state["status"] == "BUSY", parent_state

async def test_two_wl_instances_step_concurrently(runtime_llm, monkeypatch):
    """同一 agent 的两条 wl 实例可以并行执行 _step，各自写各自状态。"""
    async def fake_secrets(self, ws_, alias):
        return {"model": "fake", "api_key": "x", "backend": "fake"}

    async def fake_infer(self, messages, secrets, is_stream, tools=None, num_retries=1):
        await asyncio.sleep(0.05)
        yield ("content", "done")

    monkeypatch.setattr(LLMPlugin, "_get_god_secrets", fake_secrets)
    monkeypatch.setattr(LLMPlugin, "_infer_and_stream", fake_infer)

    rt = runtime_llm
    ws, ctx = rt._workspace, _ctx()
    plugin = LLMPlugin()
    r = await plugin(ws, "", ["dev", "init"], ctx, {"-m": "FAKE", "-tool": "false"})
    assert "initialized" in str(r), r

    for wl in ("wl_a", "wl_b"):
        _ensure_wl_hook(ws, "dev", wl, ctx)
        _write_proc(ws, "dev", wl, ctx, status="BUSY", turn="1", step="0",
                    cb="", wt="genesis")

    await asyncio.gather(
        plugin._step(ws, "dev", "wl_a"),
        plugin._step(ws, "dev", "wl_b"),
    )
    for wl in ("wl_a", "wl_b"):
        st = _read_state(ws, "dev", wl, ctx)
        assert st["status"] == "DONE", (wl, st)

    assert st["status"] == "DONE", (wl, st)


async def test_multiline_field_transitions_do_not_leave_stale_bq(runtime_llm):
    """多行 bq 字段在整段替换、收敛为单行、删除三种操作后，不残留旧 bq 行或重复 bullet。"""
    ws, ctx = runtime_llm._workspace, _ctx()
    _write_proc(ws, "dev", "main", ctx, cb="l1\nl2")
    _write_proc(ws, "dev", "main", ctx, cb="l3\nl4")
    assert _read_state(ws, "dev", "main", ctx)["cb"] == "l3\nl4"

    _write_proc(ws, "dev", "main", ctx, cb="single")
    assert _read_state(ws, "dev", "main", ctx)["cb"] == "single"
    raw = str(await runtime_llm.exec("$ /.mds/proc.md -pj raw", wt_uid="genesis"))
    assert raw.count("- cb:") == 1, raw
    assert "  > l1" not in raw and "  > l3" not in raw, raw

    _write_proc(ws, "dev", "main", ctx, cb="")
    assert not _read_state(ws, "dev", "main", ctx).get("cb")
    raw2 = str(await runtime_llm.exec("$ /.mds/proc.md -pj raw", wt_uid="genesis"))
    assert "- cb:" not in raw2, raw2

