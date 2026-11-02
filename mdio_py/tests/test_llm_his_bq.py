"""HIS 格式回归：记录 + 字段 + 引用载荷。

对齐仓库既有夹具：`runtime_llm`（临时 runtime + /.mds/llm.md + /.mds/proc.md）、`sandbox_id`。
不联网；真模型那一轮见 test_llm_live_smoke.py（需 MDIO_LIVE_LLM=1）。

规范形（记录 = 列 0 的 bullet，tag = 角色、值恒空；字段 = 缩进 2 的子 bullet；
载荷 = 引用容器里的**缩进代码块**，内容行 "  > " + 4 空格、空行裸 "  >"）。
"""

import asyncio
import json

import pytest

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.llm.llm import (
    HIS_CONTENT_COL,
    LLMPlugin,
    _append_record,
    _find_unpaired_tool_calls,
    _parse_his_records,
    _read_his,
    _read_last_assistant,
    _rebuild_messages,
)

NASTY = "第一段\n- 列表行\n> 引用行\n\n第二段"


def _ctx():
    return RuntimeContext(wt_uid="genesis")


def _cfg(his="HIS"):
    return {"his": his, "m": "FAKE", "sysp": "", "tool": "true", "retry": "1"}


def _write_basic(rt, agent="t1"):
    """六条记录：user / thinking / assistant(恶劣载荷) / tool_call / tool_result / assistant。"""
    ws, cfg, ctx = rt._workspace, _cfg(), _ctx()
    _append_record(ws, agent, "HIS", "user", "看一下日志，按频率排", cfg, ctx)
    _append_record(ws, agent, "HIS", "thinking", "先读 ERRORS 节", cfg, ctx)
    _append_record(ws, agent, "HIS", "assistant", NASTY, cfg, ctx)
    _append_record(ws, agent, "HIS", "tool_call",
                   '{"pipeline": "$ /logs/today.md::ERRORS"}', cfg, ctx,
                   fields={"name": "execute_mdio", "id": "call_8f3a"})
    _append_record(ws, agent, "HIS", "tool_result", "- mdio_success: +> /a.md", cfg, ctx,
                   fields={"name": "execute_mdio", "id": "call_8f3a"})
    _append_record(ws, agent, "HIS", "assistant", "建议先修 E_CONN_REFUSED", cfg, ctx)
    return ws


async def test_shape_is_record_field_payload(runtime_llm, sandbox_id):
    """写出的文档符合规范形：记录值恒空、载荷行只有 "  > " 与裸 "  >"、无尾空格。"""
    ws = _write_basic(runtime_llm)
    raw = _read_his(ws, "t1", "HIS", _cfg(), _ctx())

    rec_lines = [ln for ln in raw.split("\n") if ln.startswith("- ")]
    assert [ln for ln in rec_lines if not ln.endswith(":")] == [], rec_lines
    assert len(rec_lines) == 6, rec_lines

    payload_lines = [ln for ln in raw.split("\n")
                     if ln.startswith(" " * HIS_CONTENT_COL + ">")]
    assert payload_lines, raw
    assert all(ln == "  >" or ln.startswith("  > ") for ln in payload_lines), payload_lines
    assert [ln for ln in payload_lines if ln.endswith(" ")] == [], payload_lines


async def test_parse_records(runtime_llm, sandbox_id):
    """记录解析：角色序、字段、载荷逐字节（含多行、空行、"- " 行、"> " 行）。"""
    ws = _write_basic(runtime_llm)
    recs = _parse_his_records(_read_his(ws, "t1", "HIS", _cfg(), _ctx()), workspace=ws)

    assert [r["role"] for r in recs] == [
        "user", "thinking", "assistant", "tool_call", "tool_result", "assistant"]
    assert recs[2]["content"] == NASTY
    assert recs[2]["fields"] == {}
    assert recs[3]["fields"] == {"name": "execute_mdio", "id": "call_8f3a"}
    assert recs[3]["content"] == '{"pipeline": "$ /logs/today.md::ERRORS"}'
    assert recs[4]["fields"]["id"] == "call_8f3a"
    assert recs[4]["content"] == "- mdio_success: +> /a.md"


async def test_structured_write_roundtrip(runtime_llm, sandbox_id):
    """结构化记录写进去、原样读回来（含空载荷、emoji、载荷内出现记录形状）。"""
    cases = [
        {"role": "user", "fields": {}, "payload": "你好"},
        {"role": "assistant", "fields": {}, "payload": "带 emoji 🎯 与全角：，。\n尾行"},
        {"role": "thinking", "fields": {}, "payload": ""},
        {"role": "tool_call", "fields": {"name": "execute_mdio", "id": "call_8f3a"},
         "payload": '{"pipeline": "$ /a.md::X"}'},
        {"role": "tool_result", "fields": {"name": "execute_mdio", "id": "call_8f3a"},
         "payload": "- mdio_success: +> /a.md\n\n- E_TIMEOUT: 17"},
        {"role": "assistant", "fields": {}, "payload": "- user:\n  > 嵌套形状也照原样读回"},
    ]
    ws = runtime_llm._workspace
    for c in cases:
        _append_record(ws, "t2", "HIS", c["role"], c["payload"], _cfg(), _ctx(), fields=c["fields"])

    got = [{"role": r["role"], "fields": r["fields"], "payload": r["content"]}
           for r in _parse_his_records(_read_his(ws, "t2", "HIS", _cfg(), _ctx()), workspace=ws)]
    assert got == cases, json.dumps(got, ensure_ascii=False)


async def test_rebuild_and_pairing(runtime_llm, sandbox_id):
    """重建 messages：角色序、载荷逐字节、thinking 归后一条 assistant、tool 按 id 配对。"""
    ws = _write_basic(runtime_llm)
    raw = _read_his(ws, "t1", "HIS", _cfg(), _ctx())

    msgs = _rebuild_messages(raw, workspace=ws)
    assert [m["role"] for m in msgs] == ["user", "assistant", "tool", "assistant"]
    assert msgs[1]["content"] == NASTY
    assert msgs[1]["reasoning_content"] == "先读 ERRORS 节"
    assert msgs[1]["tool_calls"][0]["id"] == "call_8f3a"
    assert msgs[1]["tool_calls"][0]["function"]["name"] == "execute_mdio"
    assert msgs[2]["tool_call_id"] == "call_8f3a"
    assert msgs[2]["content"] == "- mdio_success: +> /a.md"
    assert _find_unpaired_tool_calls(raw, workspace=ws) == []

    # 再写一条没有结果的 call ⇒ 应被配对逻辑抓到
    _append_record(ws, "t1", "HIS", "tool_call", '{"pipeline": "$ /x.md::"}', _cfg(), _ctx(),
                   fields={"name": "execute_mdio", "id": "call_beef"})
    raw2 = _read_his(ws, "t1", "HIS", _cfg(), _ctx())
    unpaired = _find_unpaired_tool_calls(raw2, workspace=ws)
    assert len(unpaired) == 1 and unpaired[0]["tc_id"] == "call_beef", unpaired

    assert _read_last_assistant(ws, "t1", "HIS", _cfg(), _ctx()) == "建议先修 E_CONN_REFUSED"


async def test_tool_result_dedup_by_id(runtime_llm, sandbox_id):
    """同一个 tc_id 的 tool_result 只产出一条 tool 消息（异步路径上会被写两次）。"""
    ws, cfg, ctx = runtime_llm._workspace, _cfg(), _ctx()
    _append_record(ws, "t9", "HIS", "user", "跑一下", cfg, ctx)
    _append_record(ws, "t9", "HIS", "assistant", "我来跑", cfg, ctx)
    _append_record(ws, "t9", "HIS", "tool_call", '{"pipeline": "$ /a.md::X"}', cfg, ctx,
                   fields={"name": "execute_mdio", "id": "call_dup"})
    _append_record(ws, "t9", "HIS", "tool_result", "第一次结果", cfg, ctx,
                   fields={"name": "execute_mdio", "id": "call_dup"})
    _append_record(ws, "t9", "HIS", "tool_result", "第二次结果", cfg, ctx,
                   fields={"name": "execute_mdio", "id": "call_dup"})

    msgs = _rebuild_messages(_read_his(ws, "t9", "HIS", cfg, ctx), workspace=ws)
    tools = [m for m in msgs if m["role"] == "tool"]
    assert len(tools) == 1 and tools[0]["content"] == "第一次结果", tools


async def test_inline_value_is_not_a_record(runtime_llm, sandbox_id):
    """格式是「记录值恒空」：带内联值的 bullet 不是记录（这里只认记录 + 字段 + 载荷）。"""
    ws = runtime_llm._workspace
    text = "### HIS\n- user: 你好\n- assistant: 这样写不构成记录\n"
    assert _parse_his_records(text, workspace=ws) == []
    assert _rebuild_messages(text, workspace=ws) == []


async def test_turn_end_to_end(runtime_llm, sandbox_id):
    """状态机一轮：_handle_busy（tool_call）→ _handle_wait_tool（真管道）→ _handle_busy（收尾）。"""
    from mdio_py.plugins.llm.llm import _read_state

    ws = runtime_llm._workspace
    plugin = LLMPlugin()
    cfg, ctx, agent = _cfg(), _ctx(), "t3"

    calls = {"n": 0}

    async def fake_infer(messages, secrets, is_stream, tools=None, num_retries=1):
        calls["n"] += 1
        if calls["n"] == 1:
            yield ("content", "我先读一遍日志。")
            yield ("tool", {"id": "call_8f3a", "name": "execute_mdio",
                            "pipeline": "$ /.mds/llm.md::AGENTS.t3.HIS",
                            "raw_args": '{"pipeline": "$ /.mds/llm.md::AGENTS.t3.HIS"}'})
        else:
            yield ("thinking", "已经拿到结果")
            yield ("content", "建议先修 E_CONN_REFUSED")

    async def fake_secrets(ws_, alias):
        return {"model": "fake-model", "api_key": "x"}

    plugin._infer_and_stream = fake_infer
    plugin._get_god_secrets = fake_secrets

    _append_record(ws, agent, "HIS", "user", "看一下日志，按频率排", cfg, ctx)

    await plugin._handle_busy(ws, agent, "main", {"status": "BUSY", "turn": "1", "step": "0"}, cfg)
    assert _read_state(ws, agent, "main", ctx).get("status") == "WAIT_TOOL"

    await plugin._handle_wait_tool(ws, agent, "main", _read_state(ws, agent, "main", ctx), cfg)
    assert _read_state(ws, agent, "main", ctx).get("status") == "BUSY"

    await plugin._handle_busy(ws, agent, "main", _read_state(ws, agent, "main", ctx), cfg)
    assert _read_state(ws, agent, "main", ctx).get("status") == "DONE"

    raw = _read_his(ws, agent, "HIS", cfg, ctx)
    assert [r["role"] for r in _parse_his_records(raw, workspace=ws)] == [
        "user", "assistant", "tool_call", "tool_result", "thinking", "assistant"]
    msgs = _rebuild_messages(raw, workspace=ws)
    assert [m["role"] for m in msgs] == ["user", "assistant", "tool", "assistant"]
    assert msgs[3]["reasoning_content"] == "已经拿到结果"
    assert _find_unpaired_tool_calls(raw, workspace=ws) == []


async def test_his_stats_counts_thinking(runtime_llm, sandbox_id):
    ws = _write_basic(runtime_llm)
    stats = await LLMPlugin()._get_history_stats(ws, "t1", _cfg(), _ctx())
    assert stats["thinking"] == 1, stats


async def test_stream_closes_on_early_error(runtime_llm, sandbox_id):
    """早错（NO_SECRETS）时 `@LLM.x.stream` 必须**结束**：先推错误块，再终止。

    早错路径若只写 proc + notify、不碰 stream 队列，消费端 `async for` 会永久挂住；
    由 `_notify_done` 统一收尾 + `_stream_error` 推错误块。
    """
    rt, agent = runtime_llm, "nosec"
    await rt.exec(f'@LLM.{agent}.init -m "NOPE" -tool false -his "/h.md::A"', wt_uid=sandbox_id)
    stream = await rt.exec(f'"hi" | @LLM.{agent}.stream', wt_uid=sandbox_id, stream=True)

    async def _drain():
        return [ch async for ch in stream]

    chunks = await asyncio.wait_for(_drain(), timeout=10)      # 挂住就是回归
    assert chunks and chunks[0][0] == "error", chunks
    assert "NO_SECRETS" in chunks[0][1], chunks
