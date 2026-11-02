"""HIS 业务 fixture 回归：一个覆盖主业务的测试 HIS。

覆盖：
- main 基线；branch 甩尾；join 回拉与重入；compact 原位摘要；prune identity；
- thinking/tool_call/tool_result 折叠；嵌套 tag（a / a_b / a_b_c）与不可达 tag；
- branch id 标注（只作元数据，不参与装配）；
- 编辑后新 branch 的读语义（fixture 里的 ed-1 只是普通 gate tag，不代表建线 API）。

本文件只做读端基线 + 写目标/流式/配置回归；编辑建线走 -branch 接口（test_llm_wl_branch.py）。
"""

from pathlib import Path

import pytest

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.mdio_ir import SelectorIR
from mdio_py.plugins.llm import arrow_handoff

FIXTURE = Path(__file__).parent / "fixtures" / "his_business_his.txt"


def _ctx():
    return RuntimeContext(wt_uid="genesis")


async def _write(rt):
    """把业务 fixture 原样写进临时 runtime，返回 HIS 原文。"""
    text = FIXTURE.read_text()
    from mdio_py.core.mdio_ir import SelectorIR as IR
    rt._workspace.write_locator("genesis", "/wl.md",
                                IR(file_path=None, steps=[]), "/>", text,
                                auto_scaffold=True)
    return str(await rt.exec("$ /wl.md::HIS -pj raw", wt_uid="genesis"))


async def _messages(rt, wl):
    his = await _write(rt)
    return await arrow_handoff.handoff(his, wl, rt._workspace, _ctx())


def _contents(msgs):
    return [m.get("content") for m in msgs]


BASE = [
    "原问题", "原回答", "带思考的回答", "mdio_success: ok",
    "换线", "主线尾部", "继续任务", "最终回答",
    "剪枝记录", "剪枝后的回答", "进入 a 线", "a 线尾部",
    "拉入 join", "join 后主线继续", "再拉一次", "最后",
]

GO = [
    "原问题", "原回答", "带思考的回答", "mdio_success: ok",
    "B1 内容", "B1 内部", "B1 尾部",
]

EDITED = ["原问题", "原回答", "编辑后的消息", "新回答"]

A_PARENT = BASE[:10] + ["A 内容", "A 切到 ab", "A 尾部"]

AB_PARENT = BASE[:10] + ["A 内容", "AB 内容", "AB 切到 abc", "AB 尾部"]

ABC_LINE = BASE[:10] + ["A 内容", "AB 内容", "ABC 内容"]

JOIN = BASE[:12] + ["JOIN 内容", "join 后主线继续", "JOIN 内容", "最后"]

TS1 = [
    "前三轮摘要", "带思考的回答", "mdio_success: ok",
    "换线", "主线尾部", "继续任务", "最终回答",
    "剪枝记录", "剪枝后的回答", "进入 a 线", "a 线尾部",
    "拉入 join", "join 后主线继续", "再拉一次", "最后",
]

WL_CASES = [
    ("baseline", "", BASE),
    ("unknown", "nope", BASE),
    ("branch_go", "go", GO),
    ("join_jn", "jn", JOIN),
    ("compact_ts1", "ts1", TS1),
    ("prune_pr1", "pr1", BASE),
    ("nested_a", "a", A_PARENT),
    ("nested_ab", "a_b", AB_PARENT),
    ("nested_abc", "a_b_c", ABC_LINE),
    ("nested_missing", "a_x", A_PARENT),
    ("edit_read", "ed-1", EDITED),
]


@pytest.mark.parametrize("name,wl,expected", WL_CASES, ids=[c[0] for c in WL_CASES])
async def test_business_worldline_outputs(runtime_llm, name, wl, expected):
    """每个世界线取到预期业务内容；branch/join/compact/prune/编辑线各司其职。"""
    msgs = await _messages(runtime_llm, wl)
    assert _contents(msgs) == expected


async def test_business_thinking_and_tool_fold(runtime_llm):
    """thinking 归位到 assistant，tool_call 与 tool_result 配对成 tool 消息。"""
    msgs = await _messages(runtime_llm, "")
    assert msgs[0]["role"] == "user" and msgs[0]["content"] == "原问题"
    assert msgs[1]["role"] == "assistant" and msgs[1]["content"] == "原回答"
    folded = msgs[2]
    assert folded["role"] == "assistant" and folded["content"] == "带思考的回答"
    assert folded["reasoning_content"] == "先想一下"
    assert len(folded["tool_calls"]) == 1
    assert folded["tool_calls"][0]["id"] == "call_01"
    tool = msgs[3]
    assert tool["role"] == "tool" and tool["tool_call_id"] == "call_01"
    assert tool["content"] == "mdio_success: ok"


async def test_business_compact_prune_controls_not_emitted(runtime_llm):
    """compact 摘要原位置换；prune 只删控制，正文保留；控制行不进 messages。"""
    compact = await _messages(runtime_llm, "ts1")
    assert compact[0] == {"role": "user", "content": "前三轮摘要"}
    assert all("原问题" not in str(m.get("content")) for m in compact)
    pruned = await _messages(runtime_llm, "pr1")
    assert _contents(pruned) == BASE
    for m in pruned:
        assert "+ " not in str(m.get("content"))
        assert "* pr1" not in str(m.get("content"))




# 世界线写目标（branch 线写盘/流式）

from mdio_py.plugins.llm import llm as llm_plugin


async def _raw(rt):
    return str(await rt.exec("$ /wl.md -pj raw", wt_uid="genesis"))


async def test_resolve_write_section_follows_wl(runtime_llm):
    """wl 决定写目标：无 wl→main；ed-1→b_ed；未知→main。"""
    await _write(runtime_llm)
    ws = runtime_llm._workspace
    ctx = _ctx()
    assert llm_plugin._resolve_write_section(ws, "dev", "/wl.md::HIS", {}, ctx) == "main"
    assert llm_plugin._resolve_write_section(ws, "dev", "/wl.md::HIS", {"wl": "ed-1"}, ctx) == "b_ed"
    assert llm_plugin._resolve_write_section(ws, "dev", "/wl.md::HIS", {"wl": "nope"}, ctx) == "main"


async def test_branch_line_write_targets_branch_not_main(runtime_llm):
    """wl=ed-1 时新 user 记录写进 b_ed，main 不动。"""
    await _write(runtime_llm)
    ws = runtime_llm._workspace
    llm_plugin._append_record(ws, "dev", "/wl.md::HIS", "user", "分支新用户",
                              {"wl": "ed-1"}, _ctx())
    raw = await _raw(runtime_llm)
    head, _, tail = raw.partition("### b_ed")
    assert "分支新用户" not in head
    assert "分支新用户" in tail
    assert "### b_ed" in raw


async def test_branch_line_stream_part_targets_branch(runtime_llm):
    """编辑线里的 assistant 流式 chunk 续写同一 branch 的直属 bq，不跑回 main。"""
    await _write(runtime_llm)
    ws = runtime_llm._workspace
    cfg = {"wl": "ed-1"}
    llm_plugin._append_record(ws, "dev", "/wl.md::HIS", "assistant", "第一段",
                              cfg, _ctx())
    llm_plugin._append_record(ws, "dev", "/wl.md::HIS", "assistant", "第二段",
                              cfg, _ctx(), part=True)
    raw = await _raw(runtime_llm)
    head, _, tail = raw.partition("### b_ed")
    assert "第一段" not in head and "第二段" not in head
    assert "第一段" in tail and "第二段" in tail
    assert "> 第一段第二段" in tail or "第一段\n  > 第二段" in tail


async def test_mainline_write_still_main(runtime_llm):
    """wl 为空时仍写 main，branch 线不被污染。"""
    await _write(runtime_llm)
    ws = runtime_llm._workspace
    llm_plugin._append_record(ws, "dev", "/wl.md::HIS", "user", "主线新用户",
                              {}, _ctx())
    raw = await _raw(runtime_llm)
    head, _, tail = raw.partition("### b_ed")
    assert "主线新用户" in head
    assert "主线新用户" not in tail


async def test_wl_config_single_bullet(runtime_llm):
    """wl 配置只保留一条：重复写不产生历史重复 bullet（切线的必要条件）。"""
    ws = runtime_llm._workspace
    llm_plugin._write_agent_wl(ws, "dev", "line-a")
    llm_plugin._write_agent_wl(ws, "dev", "line-b")
    raw = str(await runtime_llm.exec("$ /.mds/llm.md -pj raw", wt_uid="genesis"))
    assert raw.count("- wl:") == 1
    assert "line-b" in raw and "line-a" not in raw


async def test_wl_config_writes_agent_level_without_touching_his(runtime_llm):
    """wl 只按 agent 节写一条；HIS 内的历史 wl 行保持不变。"""
    from mdio_py.core.mdio_ir import SelectorIR
    ws = runtime_llm._workspace
    root = SelectorIR(file_path=None, steps=[])
    old = "# AGENTS\n## dev\n- m: X\n### HIS\n- wl: old1\n- wl: old2\n"
    ws.write_locator("genesis", "/.mds/llm.md", root, "/>", old, auto_scaffold=True)
    llm_plugin._write_agent_wl(ws, "dev", "line-new")
    raw = str(await runtime_llm.exec("$ /.mds/llm.md -pj raw", wt_uid="genesis"))
    assert "line-new" in raw and "- m: X" in raw
    assert "old1" in raw and "old2" in raw
    assert raw.count("- wl:") == 3


async def test_write_section_follows_branch_join_walk(runtime_llm):
    """写目标与读端同一套 wl 开集 + branch/join walk：不用最长 tag 猜测。"""
    await _write(runtime_llm)
    ws = runtime_llm._workspace
    ctx = _ctx()
    cases = {"": "main", "go": "b1", "jn": "main", "ts1": "main",
             "a": "b_a", "a_b": "b_ab", "a_b_c": "b_abc", "a_x": "b_a",
             "ed-1": "b_ed"}
    for wl, expected in cases.items():
        got = llm_plugin._resolve_write_section(ws, "dev", "/wl.md::HIS", {"wl": wl}, ctx)
        assert got == expected, (wl, got, expected)

