"""XBT 黑板机制：默认自动创建/复用、显式 -bb ensure、fork 继承。"""

import asyncio
import tempfile

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext

F = chr(96) * 3


def _ctx():
    return RuntimeContext(wt_uid="genesis")


def _rt():
    return MdioRuntime(tempfile.mkdtemp())


async def _write_plan(rt, ctx, bp):
    await rt.exec("mk -f /plan.md", ctx=ctx)
    await rt.exec('"# PLAN" | +> /plan.md:: -f', ctx=ctx)
    await rt.exec('"""' + F + "xbt\n" + bp.strip() + "\n" + F + '""" | +> /plan.md::PLAN', ctx=ctx)


async def _read(rt, ctx, code):
    return str(await rt.exec(code, ctx=ctx))


async def _wait_proc(rt, ctx, tid, needle, timeout=6.0):
    end = asyncio.get_event_loop().time() + timeout
    while asyncio.get_event_loop().time() < end:
        raw = await _read(rt, ctx, "$/.mds/proc.md::PROCESS_TABLE.XBT")
        if needle in raw:
            return raw
        await asyncio.sleep(0.05)
    return ""


def _hanging_bp():
    return """- PLAN:
    >> [] 挂起动作
"""


async def test_spawn_default_bb_created_and_returned():
    rt = _rt()
    ctx = _ctx()
    await _write_plan(rt, ctx, _hanging_bp())
    res = str(await rt.exec('@XBT -spawn "bb_default" -bp "/plan.md::PLAN"', ctx=ctx))
    assert "- mdio_info: /XBT_BlackBoard/bb_default.md::BlackBoard" in res, res
    doc = await _read(rt, ctx, "$ /XBT_BlackBoard/bb_default.md::")
    assert "# BlackBoard" in doc, doc
    raw = await _wait_proc(rt, ctx, "bb_default", "bb:")
    assert "/XBT_BlackBoard/bb_default.md::BlackBoard" in raw, raw[:500]
    await rt.exec('@XBT -kill "bb_default" -cascade true', ctx=ctx)


async def test_spawn_explicit_bb_ensures_missing():
    rt = _rt()
    ctx = _ctx()
    await _write_plan(rt, ctx, _hanging_bp())
    res = str(await rt.exec(
        '@XBT -spawn "bb_explicit" -bp "/plan.md::PLAN" -bb "/custom/bb.md::Board"', ctx=ctx))
    assert "- mdio_info: /custom/bb.md::Board" in res, res
    doc = await _read(rt, ctx, "$ /custom/bb.md::")
    assert "# Board" in doc, doc
    raw = await _wait_proc(rt, ctx, "bb_explicit", "bb:")
    assert "/custom/bb.md::Board" in raw, raw[:500]
    await rt.exec('@XBT -kill "bb_explicit" -cascade true', ctx=ctx)


async def test_spawn_reuses_existing_default_bb():
    rt = _rt()
    ctx = _ctx()
    await rt.exec("mk -f /XBT_BlackBoard/reuse.md", ctx=ctx)
    await rt.exec('"# BlackBoard" | +> /XBT_BlackBoard/reuse.md::', ctx=ctx)
    await rt.exec('"- keep: yes" | +> /XBT_BlackBoard/reuse.md::BlackBoard', ctx=ctx)
    await _write_plan(rt, ctx, _hanging_bp())
    await rt.exec('@XBT -spawn "reuse" -bp "/plan.md::PLAN"', ctx=ctx)
    doc = await _read(rt, ctx, "$ /XBT_BlackBoard/reuse.md::")
    assert "keep: yes" in doc, doc
    await rt.exec('@XBT -kill "reuse" -cascade true', ctx=ctx)


async def test_fork_inherits_parent_bb():
    rt = _rt()
    ctx = _ctx()
    bp = """- PLAN:
    >> []
        == [] 动作A
        == [] 动作B
"""
    await _write_plan(rt, ctx, bp)
    await rt.exec('@XBT -spawn "bb_fork" -bp "/plan.md::PLAN"', ctx=ctx)
    raw = await _wait_proc(rt, ctx, "bb_fork", "bb_fork_0_1")
    assert "bb_fork_0_1" in raw, raw[:800]
    assert raw.count("/XBT_BlackBoard/bb_fork.md::BlackBoard") >= 3, raw[:800]
    await rt.exec('@XBT -kill "bb_fork" -cascade true', ctx=ctx)
