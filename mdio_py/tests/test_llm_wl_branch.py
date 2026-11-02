"""-wl run-scoped + 单 branch 创建（LLM 侧）回归。"""

import asyncio
import json

import pytest

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.mdio_ir import SelectorIR
from mdio_py.plugins.llm import arrow_handoff
from mdio_py.plugins.llm.llm import LLMPlugin


def _ctx():
    return RuntimeContext(wt_uid="genesis")


async def _seed_doc(rt):
    await rt.exec("mk -f /b.md", wt_uid="genesis")
    await rt.exec('"## HIS\\n### main\\n- user:\\n  > r0\\n- assistant:\\n  > r1\\n" | /> /b.md:: -pj r',
                  wt_uid="genesis")


async def _raw(rt):
    return str(await rt.exec("$ /b.md::HIS -pj raw", wt_uid="genesis"))


async def _setup(rt, ctx, monkeypatch=None):
    ws = rt._workspace
    await _seed_doc(rt)
    p = LLMPlugin()
    r = await p(ws, "", ["dev", "init"], ctx, {"-m": "FAKE", "-his": "/b.md::HIS", "-tool": "false"})
    assert "initialized" in str(r), r
    return ws, p


async def test_branch_create_bare_and_run_scoped(runtime_llm, monkeypatch):
    rt = runtime_llm
    ctx = _ctx()
    ws, p = await _setup(rt, ctx)

    async def fake_secrets(self, ws_, alias):
        return {"model": "fake", "api_key": "x", "backend": "fake"}

    async def fake_infer(self, messages, secrets, is_stream, tools=None, num_retries=1):
        yield ("content", "answer")

    monkeypatch.setattr(LLMPlugin, "_get_god_secrets", fake_secrets)
    monkeypatch.setattr(LLMPlugin, "_infer_and_stream", fake_infer)

    r = await p(ws, "", ["dev"], ctx,
                {"-wl": "wl_a", "-branch": True, "-branch_target": "HIS.b_x"})
    out = json.loads(str(r))
    assert out["wl"] == "wl_a" and out["section"] == "b_x"

    raw = await _raw(rt)
    assert "+ wl_a:" in raw and "type: branch" in raw and "path: HIS.b_x" in raw
    assert "### b_x" in raw
    # 末尾 host：挂到最后一条记录，不新增 - user:，不复制父线记录
    assert raw.count("- user:") == 1, raw
    assert "- thinking:" in raw, raw
    assert raw.index("> r1") < raw.index("+ wl_a:") < raw.index("### b_x"), raw
    assert "> r0" not in raw[raw.find("### b_x"):]
    # 子线继承父线上下文（thinking marker 不进 messages）
    msgs = await arrow_handoff.handoff(raw, "wl_a", ws, ctx)
    assert [m.get("content") for m in msgs] == ["r0", "r1"], msgs

    r2 = await asyncio.wait_for(
        p(ws, "hello", ["dev", "run"], ctx, {"-wl": "wl_a"}), timeout=20)
    assert str(r2) == "answer", r2
    doc = await _raw(rt)
    assert "> hello" in doc[doc.find("### b_x"):]
    assert "> answer" in doc[doc.find("### b_x"):]
    cfg = str(await rt.exec("$ /.mds/llm.md::AGENTS.dev:wl -pj ct", wt_uid="genesis"))
    assert "wl_a" not in cfg


async def test_branch_create_idx_anchor(runtime_llm):
    rt = runtime_llm
    ctx = _ctx()
    ws, p = await _setup(rt, ctx)
    r = await p(ws, "", ["dev"], ctx,
                {"-wl": "wl_c", "-branch": "0", "-branch_target": "HIS.b_target"})
    assert "wl_c" in str(r), r
    raw = await _raw(rt)
    i_user = raw.find("- user:")
    i_ctl = raw.find("+ wl_c:")
    i_assistant = raw.find("- assistant:")
    assert i_user < i_ctl < i_assistant, raw

    r2 = await p(ws, "", ["dev"], ctx,
                 {"-wl": "wl_d", "-branch": "9", "-branch_target": "HIS.b_d"})
    assert "out of range" in str(r2), r2


async def test_branch_default_target_idempotent_and_conflict(runtime_llm):
    rt = runtime_llm
    ctx = _ctx()
    ws, p = await _setup(rt, ctx)
    r = await p(ws, "", ["dev"], ctx, {"-wl": "wl_e", "-branch": True})
    out = json.loads(str(r))
    assert out["section"] == "b_wl_e", out
    raw = await _raw(rt)
    assert "### b_wl_e" in raw

    r2 = await p(ws, "", ["dev"], ctx, {"-wl": "wl_e", "-branch": True})
    assert "wl_e" in str(r2), r2
    r3 = await p(ws, "", ["dev"], ctx,
                 {"-wl": "wl_e", "-branch": True, "-branch_target": "HIS.b_other"})
    assert "mdio_error" in str(r3), r3


async def test_branch_target_cross_doc_rejected_and_unknown_wl_run(runtime_llm):
    rt = runtime_llm
    ctx = _ctx()
    ws, p = await _setup(rt, ctx)
    r = await p(ws, "", ["dev"], ctx,
                {"-wl": "wl_z", "-branch": True, "-branch_target": "/other.md::HIS.x"})
    assert "same md" in str(r), r

    r2 = await asyncio.wait_for(
        p(ws, "hello", ["dev", "run"], ctx, {"-wl": "wl_missing"}), timeout=20)
    assert "wl not found" in str(r2), r2

async def test_branch_default_his_doc(runtime_llm, monkeypatch):
    """默认 HIS（/.mds/llm.md::AGENTS.dev.HIS，深标题）也能建 branch 并跑。"""
    async def fake_secrets(self, ws_, alias):
        return {"model": "fake", "api_key": "x", "backend": "fake"}

    async def fake_infer(self, messages, secrets, is_stream, tools=None, num_retries=1):
        yield ("content", "deep-answer")

    monkeypatch.setattr(LLMPlugin, "_get_god_secrets", fake_secrets)
    monkeypatch.setattr(LLMPlugin, "_infer_and_stream", fake_infer)

    rt = runtime_llm
    ctx = _ctx()
    ws = rt._workspace
    p = LLMPlugin()
    r = await p(ws, "", ["dev", "init"], ctx, {"-m": "FAKE", "-tool": "false"})
    assert "initialized" in str(r), r
    r = await p(ws, "", ["dev"], ctx, {"-wl": "wl_deep", "-branch": True})
    assert "wl_deep" in str(r), r
    r2 = await asyncio.wait_for(
        p(ws, "hello", ["dev", "run"], ctx, {"-wl": "wl_deep"}), timeout=20)
    assert str(r2) == "deep-answer", r2

    assert str(r2) == "deep-answer", r2


async def test_branch_idx_anchor_keeps_host_payload_and_existing_gate(runtime_llm):
    """idx anchor 挂在已有门/payload 的 host 上：payload 与旧门不被重排，各线各自可读。"""
    rt = runtime_llm
    ctx = _ctx()
    ws = rt._workspace
    seeded = (
        "## HIS\n### main\n"
        "- user:\n"
        "    + old:\n"
        "        - type: branch\n"
        "        - path: /b.md::HIS.b_old\n"
        "    > r0\n"
        "- assistant:\n"
        "  > r1\n"
        "### b_old\n"
        "- user:\n"
        "  > old0\n"
    )
    ws.write_locator("genesis", "/b.md", SelectorIR(file_path=None, steps=[]),
                     "/>", seeded, auto_scaffold=True)
    p = LLMPlugin()
    r = await p(ws, "", ["dev", "init"], ctx,
                {"-m": "FAKE", "-his": "/b.md::HIS", "-tool": "false"})
    assert "initialized" in str(r), r
    r = await p(ws, "", ["dev"], ctx,
                {"-wl": "wl_new", "-branch": "0", "-branch_target": "HIS.b_new"})
    assert "wl_new" in str(r), r
    raw = await _raw(rt)
    # 旧门和新门都在 host 记录下，payload 仍是记录直属载荷（不被缩进重排吞掉）
    assert "+ old:" in raw and "+ wl_new:" in raw, raw
    assert "### b_old" in raw and "### b_new" in raw, raw
    assert "> r0" in raw and "> old0" in raw, raw
    assert [m.get("content") for m in await arrow_handoff.handoff(raw, "", ws, ctx)] == ["r0", "r1"]
    assert [m.get("content") for m in await arrow_handoff.handoff(raw, "old", ws, ctx)] == ["old0"]
    assert [m.get("content") for m in await arrow_handoff.handoff(raw, "wl_new", ws, ctx)] == []


async def test_branch_parent_derived_from_wl_prefix(runtime_llm):
    """b0_1 的 parent 自动取已存在的 b0，不依赖 config 默认线。"""
    rt = runtime_llm
    ctx = _ctx()
    ws, p = await _setup(rt, ctx)
    r = await p(ws, "", ["dev"], ctx, {"-wl": "b0", "-branch": True})
    assert "b0" in str(r), r
    r = await p(ws, "", ["dev"], ctx, {"-wl": "b0_1", "-branch": True})
    assert "b0_1" in str(r), r
    raw = await _raw(rt)
    i_b0 = raw.find("### b_b0")
    i_child = raw.find("+ b0_1:")
    i_b01 = raw.find("### b_b0_1")
    assert i_b0 < i_child < i_b01, raw

