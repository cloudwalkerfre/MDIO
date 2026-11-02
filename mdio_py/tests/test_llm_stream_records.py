"""LLM 流式记录写入的离线测试。

覆盖：
1. `_append_record(part=True)` 把续写 chunk 追加到上一条记录的 bq 载荷；
2. `.stream` 的 `_handle_busy` 分支按 chunk 写 thinking/assistant，并在 tool 事件处收尾。

全部用 fake `_infer_and_stream` + 临时 MdioRuntime，不触网。
"""

import asyncio

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.llm.llm import LLMPlugin, _append_record, _ensure_wl_hook, _read_state

CFG = {
    "his": "/chat/t.md::HIS",
    "m": "FAKE",
    "sysp": "",
    "tool": "false",
    "retry": "1",
}


async def _seed_chat(rt, sid):
    await rt.exec("mk -f /chat/t.md", wt_uid=sid)
    await rt.exec('"## HIS\\n" | +> /chat/t.md::', wt_uid=sid)


async def _fake_secrets(ws_, alias):
    return {"model": "fake", "api_key": "x", "backend": "fake"}


async def test_append_record_part_appends_chunks_to_bq(runtime_llm, sandbox_id):
    """首写建记录；part 续写进同一条记录的 bq，而不是另起段落。"""
    rt = runtime_llm
    ctx = RuntimeContext(wt_uid=sandbox_id)
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    _append_record(ws, "cba", CFG["his"], "thinking", "t1", CFG, ctx)
    _append_record(ws, "cba", CFG["his"], "thinking", "t2", CFG, ctx, part=True)
    _append_record(ws, "cba", CFG["his"], "assistant", "a", CFG, ctx)
    _append_record(ws, "cba", CFG["his"], "assistant", "b", CFG, ctx, part=True)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert doc.count("- thinking:") == 1, doc
    assert doc.count("- assistant:") == 1, doc
    # 无尾换行的 chunk 走 inline 拼接：part 续写不是另起一行
    assert "  > t1t2" in doc, doc
    assert "  > ab" in doc, doc
    assert doc.index("- thinking:") < doc.index("- assistant:"), doc


async def test_append_record_part_stays_in_outer_bq_after_nested_quote(runtime_llm, sandbox_id):
    """助手内容含嵌套引用时，part 续写必须落在外层 bq，不能进内层引用。

    若 part 选「递归最后一个 bq」，会把 > > 之后的标题/正文吸进引用。
    """
    rt = runtime_llm
    ctx = RuntimeContext(wt_uid=sandbox_id)
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    _append_record(ws, "cba", CFG["his"], "assistant",
                   "开头。\n\n> 引用句。", CFG, ctx)
    _append_record(ws, "cba", CFG["his"], "assistant", "\nTAIL", CFG, ctx, part=True)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert "  > > 引用句。" in doc, doc
    assert "  > TAIL" in doc, doc
    assert "引用句。TAIL" not in doc, doc


async def test_handle_busy_stream_after_nested_quote_keeps_blocks_outside(runtime_llm, sandbox_id):
    """真实流式序列回归：> > 引用后接 ** / 换行 / ## 标题，标题不能落进内层引用。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [
        ("content", "开头。\n\n> 引用句。"),
        ("content", "**"),
        ("content", "\n## 引用后的标题\n"),
        ("content", "\n"),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert "  > > 引用句。**" in doc, doc
    assert "  > ## 引用后的标题" in doc, doc
    assert "  > > ## 引用后的标题" not in doc, doc
    assert "**##" not in doc, doc


async def test_handle_busy_stream_blank_chunk_closes_nested_quote(runtime_llm, sandbox_id):
    """> > 引用后的空行 chunk 必须落成独立空行，后续 ## 标题才不会被吞进引用。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [
        ("content", "开头。\n\n> 引用句。"),
        ("content", "\n\n"),
        ("content", "## 引用后的标题\n"),
        ("content", "正文。"),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    lines = [l.rstrip() for l in doc.splitlines()]
    i = lines.index("  > > 引用句。")
    assert lines[i + 1] == "  >", doc
    assert lines[i + 2] == "  > ## 引用后的标题", doc
    assert lines[i + 3] == "  > 正文。", doc


async def test_infer_and_stream_yields_thinking_per_chunk(monkeypatch):
    """流式 thinking 逐 chunk 产出，不额外补整段 thinking。"""
    import mdio_py.plugins.llm.llm as llm

    class Ev:
        def __init__(self, kind, text="", tool=None, error=None):
            self.kind = kind
            self.text = text
            self.tool = tool
            self.error = error

    class FakeBackend:
        async def stream(self, req):
            yield Ev("thinking", "t1")
            yield Ev("thinking", "t2")
            yield Ev("content", "a")
            yield Ev("content", "b")

    monkeypatch.setattr(llm, "_backend_for", lambda name: FakeBackend())
    plugin = LLMPlugin()
    events = [
        e async for e in plugin._infer_and_stream(
            [], {"backend": "fake", "model": "m"}, True
        )
    ]
    assert events == [
        ("thinking", "t1"),
        ("thinking", "t2"),
        ("content", "a"),
        ("content", "b"),
    ], events


async def _install_fake_stream(plugin, yields):
    async def fake_infer(messages, secrets, is_stream, tools=None, num_retries=1):
        for item in yields:
            yield item

    async def fake_secrets(ws_, alias):
        return {"model": "fake", "api_key": "x", "backend": "fake"}

    plugin._infer_and_stream = fake_infer
    plugin._get_god_secrets = fake_secrets
    q = asyncio.Queue()
    plugin._stream_queues[("cba", "main")] = q
    return q


async def test_handle_busy_stream_writes_thinking_then_assistant(runtime_llm, sandbox_id):
    """流式分支：HIS 顺序 thinking → assistant，内容逐 chunk 写入，队列照旧收到 content。"""
    rt = runtime_llm
    ctx = RuntimeContext(wt_uid=sandbox_id)
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    q = await _install_fake_stream(plugin, [
        ("thinking", "t1"),
        ("thinking", "t2"),
        ("content", "a"),
        ("content", "b"),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert doc.count("- thinking:") == 1, doc
    assert doc.count("- assistant:") == 1, doc
    # 无尾换行的 chunk 走 inline 拼接：part 续写不是另起一行
    assert "  > t1t2" in doc, doc
    assert "  > ab" in doc, doc
    assert doc.index("- thinking:") < doc.index("- assistant:"), doc

    items = []
    while not q.empty():
        items.append(q.get_nowait())
    assert items[:2] == ["a", "b"], items
    assert items[-1] is None, items
    assert _read_state(ws, "cba", "main", ctx).get("status") == "DONE"


async def test_handle_busy_stream_single_line_thinking_written_before_assistant(runtime_llm, sandbox_id):
    """单行（无换行）thinking + 多行 assistant：thinking 必须先在 HIS 落盘。

    行缓冲只在换行符触发写盘时，单行 thinking 会被压到流末尾才 flush；
    assistant 多行内容在流中即时写盘 ⇒ 记录顺序变成 assistant → thinking（chat 里 Reason 沉底）。
    """
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [
        ("thinking", "single line thought"),
        ("content", "answer line 1\n"),
        ("content", "answer line 2"),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert "- thinking:" in doc and "- assistant:" in doc, doc
    assert "  > single line thought" in doc, doc
    assert doc.index("- thinking:") < doc.index("- assistant:"), doc


async def test_handle_busy_stream_tool_branch_records_tool_call(runtime_llm, sandbox_id):
    """流式分支遇 tool：先落 thinking/assistant，再一次性写 tool_call 并置 WAIT_TOOL。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [
        ("thinking", "t1"),
        ("content", "a"),
        ("tool", {"id": "id1", "pipeline": "ls /x", "name": "execute_mdio",
                  "raw_args": "", "tool": "pipeline"}),
    ])
    await plugin._handle_busy(
        ws, "cba", "main",
        {"status": "BUSY", "turn": "1", "step": "0"}, CFG,
    )

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert "- tool_call:" in doc, doc
    assert "- name: execute_mdio" in doc, doc
    assert "- id: id1" in doc, doc
    assert "- tool: pipeline" in doc, doc
    assert "  > ls /x" in doc, doc
    assert doc.index("- thinking:") < doc.index("- assistant:") < doc.index("- tool_call:"), doc
    assert _read_state(ws, "cba", "main", RuntimeContext(wt_uid=sandbox_id)).get("status") == "WAIT_TOOL"


async def test_handle_busy_stream_skips_empty_content(runtime_llm, sandbox_id):
    """空 content chunk 不建空 assistant 记录；后续 chunk 正常写。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [("content", ""), ("content", "a")])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert doc.count("- assistant:") == 1, doc
    assert "  > a" in doc, doc



async def test_handle_busy_stream_error_keeps_written_chunks(runtime_llm, sandbox_id):
    """流中 error：已写 chunk 保留，proc 置 ERROR，错误元组照旧推给 stream 队列。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    q = await _install_fake_stream(plugin, [
        ("thinking", "t1"),
        ("content", "a"),
        ("error", "boom"),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert "- thinking:" in doc and "- assistant:" in doc, doc
    assert "  > t1" in doc and "  > a" in doc, doc
    items = []
    while not q.empty():
        items.append(q.get_nowait())
    assert ("error", "boom") in items, items
    assert items[-1] is None, items
    assert _read_state(ws, "cba", "main", RuntimeContext(wt_uid=sandbox_id)).get("status") == "ERROR"


async def test_handle_busy_stream_trailing_newline_preopens_line(runtime_llm, sandbox_id):
    """chunk 尾换行：预开下一行，下一个 chunk 落到新行。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [
        ("content", "A"),
        ("content", "B\n"),
        ("content", "C"),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert "  > AB" in doc, doc
    assert "  > C" in doc, doc
    assert doc.count("- assistant:") == 1, doc


async def test_handle_busy_stream_drops_trailing_blank_chunks(runtime_llm, sandbox_id):
    """模型收尾的纯空白 chunk 不落盘：不能留下尾部空 bq 行。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [
        ("content", "A"),
        ("content", "\n"),
        ("content", "\n"),
        ("content", " "),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert "  > A" in doc, doc
    assert doc.rstrip().endswith("> A"), doc


async def test_handle_busy_stream_drops_leading_blank_chunks(runtime_llm, sandbox_id):
    """流开头的空行 chunk 不建空记录/空 bq。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [
        ("content", "\n"),
        ("content", "\n"),
        ("content", "A"),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert "  > A" in doc, doc
    assert doc.rstrip().endswith("> A"), doc


async def test_handle_busy_stream_keeps_content_separate_around_blank_chunk(runtime_llm, sandbox_id):
    """内容中间的空白 chunk 不能被静默丢弃到把前后内容拼成一行。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [
        ("content", "A"),
        ("content", "\n\n"),
        ("content", "B"),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    payload_lines = [l.rstrip() for l in doc.splitlines() if l.startswith("  >")]
    assert "  > A" in payload_lines, doc
    assert "  > B" in payload_lines, doc
    assert "  > AB" not in doc, doc


async def test_close_stops_stream_and_resets_state(runtime_llm, sandbox_id):
    """stop：给 stream 队列推 None、proc 复位 IDLE、hook id 清零；再 ensure 能重注册。"""
    rt = runtime_llm
    ctx = RuntimeContext(wt_uid=sandbox_id)
    ws = rt._workspace
    plugin = LLMPlugin()

    r = await plugin(ws, "", ["cba", "init"], ctx, {"-m": "FAKE", "-tool": "false"})
    assert "initialized" in str(r), r

    q = asyncio.Queue()
    plugin._stream_queues[("cba", "main")] = q
    plugin._done_events[("cba", "main")] = asyncio.Event()
    r2 = await plugin(ws, "", ["cba", "close"], ctx, {})
    assert "closed" in str(r2), r2
    assert q.get_nowait() is None
    state = _read_state(ws, "cba", "main", ctx)
    assert state.get("status") == "IDLE", state
    assert str(state.get("sys_hook_id", "")) == "0", state

    _ensure_wl_hook(ws, "cba", "main", ctx)
    state2 = _read_state(ws, "cba", "main", ctx)
    assert str(state2.get("sys_hook_id", "0")) not in ("", "0"), state2


def test_input_text_none_is_empty():
    """空输入（None）不得变成字面量 "None" 写进 HIS。"""
    from mdio_py.plugins.llm.llm import _input_text

    assert _input_text(None) == ""
    assert _input_text("") == ""
    assert _input_text("  hi  ") == "hi"


async def test_handle_busy_stream_multiparagraph_no_duplicate(runtime_llm, sandbox_id):
    """段落边界到达后只覆盖最后一段：A/B 各出现一次，不把整篇重复写一遍。"""
    rt = runtime_llm
    ws = rt._workspace
    await _seed_chat(rt, sandbox_id)

    plugin = LLMPlugin()
    await _install_fake_stream(plugin, [
        ("content", "A"),
        ("content", "\n\n"),
        ("content", "B"),
    ])
    await plugin._handle_busy(ws, "cba", "main", {"status": "BUSY", "turn": "1", "step": "0"}, CFG)

    doc = await rt.exec("$ /chat/t.md::", wt_uid=sandbox_id)
    assert doc.count("A") == 1, doc
    assert doc.count("B") == 1, doc
    assert "  > A" in doc, doc
    assert "  > B" in doc, doc
