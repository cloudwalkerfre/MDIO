"""管道流协议（v4）测试：classify 方言 + 流式写目标 + hook 恰好一次 + 流表。

设计归属验证：
- 写目标遇 asyncgen 自动流式（零 kwarg）
- 流中每 chunk suppress 写 → 仅流末非 suppress 笔触发 hook 恰好一次
- bash 文本方言：- mdio_error: 尾巴自动分流 error（不进文档）
- 终态写 /.mds/streams.md 流表
"""

import asyncio

import pytest

from mdio_py import MdioRuntime
from mdio_py.core.mdio_ir import SelectorIR
from mdio_py.core.selector_builder import by_tag
from mdio_py.core.stream import classify, stream_write, write_stream_record


# ── 1. classify 方言表驱动 ──────────────────────────────────────

@pytest.mark.parametrize("item,kind,payload", [
    # 元组方言：chunk
    (("content", "hi"), "chunk", "hi"),
    (("chunk", "hi"), "chunk", "hi"),
    (("text", "hi"), "chunk", "hi"),
    # 元组方言：error
    (("error", "boom"), "error", "boom"),
    # 元组方言：event 旁路
    (("thinking", "hmm"), "event", ("thinking", "hmm")),
    (("tool", {"id": "1"}), "event", ("tool", {"id": "1"})),
    (("progress", 0.5), "event", ("progress", 0.5)),
    # 元组方言：未知 ctype → error（防脏写）
    (("bogus_ctype", "x"), "error", "unknown stream ctype: 'bogus_ctype'"),
    # 文本方言
    ("plain text", "chunk", "plain text"),
    ("- mdio_error: [@BASH x] failed — boom", "error", "- mdio_error: [@BASH x] failed — boom"),
    # 其他形态 → event 旁路
    (None, "event", None),
    (123, "event", 123),
])
def test_classify(item, kind, payload):
    got_kind, got_payload = classify(item)
    assert got_kind == kind
    assert got_payload == payload


# ── 2. write_stream_record（假 workspace 记录调用）─────────────

class _FakeWS:
    def __init__(self):
        self.calls = []

    def write_locator(self, wt_uid, vfs_path, ir, op, text,
                      auto_scaffold=True, god=False, auto_level=False,
                      suppress_hooks=False, proj=None):
        self.calls.append({
            "path": vfs_path, "op": op, "text": text,
            "suppress": suppress_hooks, "wt_uid": wt_uid,
        })


def test_write_stream_record_single_transaction():
    ws = _FakeWS()
    write_stream_record(ws, "genesis", "r1", "failed", "boom\nline2",
                        meta={"target": "/x.md", "op": "+>", "actor": "test"},
                        batches=3, total_bytes=9)
    assert len(ws.calls) == 1, "流表应为单次事务写入"
    c = ws.calls[0]
    assert c["path"] == "/.mds/streams.md"
    assert c["op"] == "+>"
    assert c["suppress"] is True, "流表记录自身必须 suppress（防涟漪）"
    assert "- status: failed" in c["text"]
    assert "boom line2" in c["text"], "reason 应单行化（换行转空格）"
    assert "- batches: 3" in c["text"]
    assert "- bytes: 9" in c["text"]
    assert "- target: /x.md" in c["text"]


# ── 3. 集成：bash 流式写目标（自动流式，零 kwarg）──────────────

async def test_stream_write_append(runtime: MdioRuntime) -> None:
    await runtime.exec("mk -f /s1.md", wt_uid="genesis")
    res = await runtime.exec(
        '"echo L1; echo L2; echo L3" | @BASH.stream | +> /s1.md',
        wt_uid="genesis",
    )
    assert "stream completed" in str(res), res
    doc = runtime.store.get_text("genesis", "/s1.md")
    assert "L1" in doc and "L2" in doc and "L3" in doc, doc
    streams = runtime.store.get_text("genesis", "/.mds/streams.md")
    assert "- status: completed" in streams, streams


async def test_stream_failure_keeps_partial_and_no_error_tail(
        runtime: MdioRuntime) -> None:
    await runtime.exec("mk -f /s2.md", wt_uid="genesis")
    res = await runtime.exec(
        '"echo OK; exit 3" | @BASH.stream | +> /s2.md',
        wt_uid="genesis",
    )
    assert "stream failed" in str(res), res
    doc = runtime.store.get_text("genesis", "/s2.md")
    assert "OK" in doc, "失败前已写内容应保留"
    assert "mdio_error" not in doc, "错误尾巴（文本方言 error）不得进文档"
    streams = runtime.store.get_text("genesis", "/.mds/streams.md")
    assert "- status: failed" in streams, streams
    assert "reason" in streams, streams


async def test_stream_hook_fires_exactly_once(runtime: MdioRuntime) -> None:
    # hook 需要结构锚目标（空 steps 不匹配任何行）：建 # T 文档
    await runtime.exec("mk -f /s3.md", wt_uid="genesis")
    await runtime.exec('"# T\\n\\n- seed: 1" | /> /s3.md', wt_uid="genesis")

    fired = []
    runtime.store.register_hook_dispatcher(lambda cb, ctx: fired.append(cb))
    # 挂 # T 头（section 语义：section 内任何变更触发——含流式 append）
    from mdio_py.core import parse_code
    steps = parse_code("$T")[0].atoms[0].selector.steps
    runtime.store.register_hook("genesis", "/s3.md", SelectorIR(steps=steps),
                                "noop", {}, True)

    res = await runtime.exec(
        '"echo H1; echo H2; echo H3" | @BASH.stream | +> /s3.md',
        wt_uid="genesis",
    )
    assert "stream completed" in str(res), res

    for _ in range(50):
        if fired:
            break
        await asyncio.sleep(0.05)
    assert len(fired) == 1, f"hook 应恰好触发一次（流中 suppress、仅流末 flush 触发），实际 {len(fired)}"


async def _reg_head_hook(runtime, path, fired):
    """挂文档首 head（# T）hook：section 内任何变更触发。返回 hook 是否就绪。"""
    from mdio_py.core import parse_code
    runtime.store.register_hook_dispatcher(lambda cb, ctx: fired.append(cb))
    steps = parse_code("$T")[0].atoms[0].selector.steps
    runtime.store.register_hook("genesis", path, SelectorIR(steps=steps),
                                "noop", {}, True)
    await asyncio.sleep(0.2)


async def test_stream_failure_hook_fires_exactly_once(runtime: MdioRuntime) -> None:
    """异常终态（failed）：流末 flush 笔仍非 suppress → hook 恰好一次。"""
    await runtime.exec("mk -f /s4.md", wt_uid="genesis")
    await runtime.exec('"# T\\n\\n- seed: 1" | /> /s4.md', wt_uid="genesis")
    fired = []
    await _reg_head_hook(runtime, "/s4.md", fired)

    res = await runtime.exec(
        '"echo OK; exit 3" | @BASH.stream | +> /s4.md',
        wt_uid="genesis",
    )
    assert "stream failed" in str(res), res

    for _ in range(50):
        if fired:
            break
        await asyncio.sleep(0.05)
    assert len(fired) == 1, f"failed 终态也应恰好触发一次 hook，实际 {len(fired)}"


async def test_stream_pure_error_no_content_no_hook(runtime: MdioRuntime) -> None:
    """纯错误流（无任何内容 chunk）：文档没变 → hook 自判不触发（detect = suppress
    期变更累积 + 非 suppress 笔触发投递；零变更不投递）。错误感知走 exec 返回 + 流表。"""
    await runtime.exec("mk -f /s5.md", wt_uid="genesis")
    await runtime.exec('"# T\\n\\n- seed: 1" | /> /s5.md', wt_uid="genesis")
    before_doc = runtime.store.get_text("genesis", "/s5.md")
    fired = []
    await _reg_head_hook(runtime, "/s5.md", fired)

    res = await runtime.exec(
        '"exit 3" | @BASH.stream | +> /s5.md',
        wt_uid="genesis",
    )
    assert "stream failed" in str(res), res

    await asyncio.sleep(1.0)
    assert len(fired) == 0, f"文档未变则 hook 不应触发，实际 {len(fired)}"
    doc = runtime.store.get_text("genesis", "/s5.md")
    assert doc == before_doc, f"文档应零变化（零写零污染），got: {doc[-60:]!r}"


# stream_write 单元测试（recorder 注入——批序契约）


class _Rec:
    def __init__(self):
        self.calls = []

    def __call__(self, op, text, suppress):
        self.calls.append((op, text, suppress))


async def _agen(items):
    for it in items:
        yield it


async def test_stream_write_batch_order():
    """批序契约：[suppress x N-1, 非 suppress x 1]（延迟一拍）。"""
    rec = _Rec()
    st, rs, nb, by = await stream_write(_agen(["A", "B", "C"]), "+>", rec)
    assert st == "completed" and nb == 3 and by == 3
    assert rec.calls == [("+>", "A", True), ("+>", "B", True), ("+>", "C", False)], rec.calls


async def test_stream_write_single_chunk_no_suppress():
    """单 chunk：首笔即末笔——非 suppress（hook 一次且内容完整）。"""
    rec = _Rec()
    st, _, nb, _ = await stream_write(_agen(["X"]), "+>", rec)
    assert nb == 1 and rec.calls == [("+>", "X", False)], rec.calls


async def test_stream_write_empty_chunks_skipped():
    """空 chunk 跳过（不占 pending、不落笔）——尾随空 chunk 不影响末笔。"""
    rec = _Rec()
    st, _, nb, _ = await stream_write(_agen(["A", "", "B", ""]), "+>", rec)
    assert nb == 2 and rec.calls == [("+>", "A", True), ("+>", "B", False)], rec.calls


async def test_stream_write_pure_event_no_writes():
    """纯 event 流（thinking/tool）：零写零污染。"""
    rec = _Rec()
    st, _, nb, _ = await stream_write(_agen([("thinking", "hmm"), ("tool", {"id": "1"})]), "+>", rec)
    assert nb == 0 and rec.calls == [] and st == "completed", rec.calls


async def test_stream_write_error_terminal():
    """error 项 → failed；已到内容照落（末笔非 suppress——hook 仍一次）。"""
    rec = _Rec()
    st, rs, nb, _ = await stream_write(_agen(["OK", ("error", "boom")]), "+>", rec)
    assert st == "failed" and rs == "boom"
    assert rec.calls == [("+>", "OK", False)], rec.calls


async def test_stream_write_cancelled_raises_after_pending():
    """CancelledError：finally 落末笔（非 suppress）后 re-raise。"""
    rec = _Rec()

    async def _cancelled():
        yield "A"
        raise asyncio.CancelledError()

    with pytest.raises(asyncio.CancelledError):
        await stream_write(_cancelled(), "+>", rec)
    assert rec.calls == [("+>", "A", False)], rec.calls


async def test_stream_write_overwrite_op_first_batch():
    """/> 覆盖语义：首笔按原 op（/>），后续转 +>。"""
    rec = _Rec()
    st, _, nb, _ = await stream_write(_agen(["NEW", "MORE"]), "/>", rec)
    assert rec.calls == [("/>", "NEW", True), ("+>", "MORE", False)], rec.calls



# 状态级 hook 语义（按机制断言，不 mock 内部）


async def test_hook_hash_roundtrip(runtime: MdioRuntime) -> None:
    """T1 状态级：目标 hash 往返——改→触发；同值重写→不触发；改回→再触发。"""
    from mdio_py.core import parse_code
    await runtime.exec("mk -f /h1.md", wt_uid="genesis")
    await runtime.exec('"# T\\n\\n- status: IDLE" | /> /h1.md', wt_uid="genesis")
    fired = []
    runtime.store.register_hook_dispatcher(lambda cb, ctx: fired.append(cb))
    steps = parse_code("$T._b:status")[0].atoms[0].selector.steps
    runtime.store.register_hook("genesis", "/h1.md", SelectorIR(steps=steps), "noop", {}, True)
    await asyncio.sleep(0.3)

    async def _wait_fired(n, timeout=3.0):
        deadline = asyncio.get_running_loop().time() + timeout
        while asyncio.get_running_loop().time() < deadline:
            if len(fired) >= n:
                return
            await asyncio.sleep(0.05)
        assert len(fired) >= n, f"期望触发 {n}，实际 {len(fired)}"

    runtime.store.write_locator("genesis", "/h1.md", by_tag("T", "status"), "/>", "UP",
                                auto_scaffold=True, god=True, proj="content")
    await _wait_fired(1)
    # 同值重写：无变化 → 不触发
    runtime.store.write_locator("genesis", "/h1.md", by_tag("T", "status"), "/>", "UP",
                                auto_scaffold=True, god=True, proj="content")
    await asyncio.sleep(0.5)
    assert len(fired) == 1, f"同值重写不应触发，实际 {len(fired)}"
    # 再改 → 触发；改回初始 → 再触发（往返语义）
    runtime.store.write_locator("genesis", "/h1.md", by_tag("T", "status"), "/>", "DOWN",
                                auto_scaffold=True, god=True, proj="content")
    await _wait_fired(2)
    runtime.store.write_locator("genesis", "/h1.md", by_tag("T", "status"), "/>", "IDLE",
                                auto_scaffold=True, god=True, proj="content")
    await _wait_fired(3)


async def test_hook_other_section_not_fired(runtime: MdioRuntime) -> None:
    """T3 隔离：watch A 区——写 B 区不触发（目标 hash 未变）。"""
    from mdio_py.core import parse_code
    await runtime.exec("mk -f /h2.md", wt_uid="genesis")
    await runtime.exec('"# T\\n\\n## A\\n\\n- status: IDLE\\n\\n## B\\n\\n- status: IDLE" | /> /h2.md',
                       wt_uid="genesis")
    fired = []
    runtime.store.register_hook_dispatcher(lambda cb, ctx: fired.append(cb))
    steps = parse_code("$T.A._b:status")[0].atoms[0].selector.steps
    runtime.store.register_hook("genesis", "/h2.md", SelectorIR(steps=steps), "noop", {}, True)
    await asyncio.sleep(0.3)
    runtime.store.write_locator("genesis", "/h2.md", by_tag("T.B", "status"), "/>", "GO",
                                auto_scaffold=True, god=True, proj="content")
    await asyncio.sleep(0.5)
    assert len(fired) == 0, f"B 区写不应触发 A 区 hook，实际 {len(fired)}"
    runtime.store.write_locator("genesis", "/h2.md", by_tag("T.A", "status"), "/>", "GO",
                                auto_scaffold=True, god=True, proj="content")
    await asyncio.sleep(0.6)
    assert len(fired) == 1, f"A 区写应触发，实际 {len(fired)}"


async def test_stream_suppress_digest_deterministic(runtime: MdioRuntime) -> None:
    """确定性 FIFO 屏障测试（替代 sleep 时序测试）。

    用第二个 doc 的 hook 当"处理完成屏障"：同一线程先发 A 的 suppress
    检测任务、再发 B 的普通检测任务，全局 detect 通道 FIFO + worker 单线程
    ⟹ B hook 触发 ⟹ A 的该笔 suppress 任务已被评估（消化）。因此这里可以
    **证明**"流中 fired==0 / 末笔 fired==1"，而不是 sleep 后碰巧观察。
    """
    from mdio_py.core import parse_code

    # ── 目标 doc A + head hook ──
    await runtime.exec("mk -f /da1.md", wt_uid="genesis")
    await runtime.exec('"# T\\n\\n- seed: 1" | /> /da1.md', wt_uid="genesis")
    fired_a = []
    barrier = []

    def on_hook(cb, ctx):
        if cb == "barrier":
            barrier.append(cb)
        else:
            fired_a.append(cb)

    runtime.store.register_hook_dispatcher(on_hook)
    steps_a = parse_code("$T")[0].atoms[0].selector.steps
    runtime.store.register_hook("genesis", "/da1.md", SelectorIR(steps=steps_a),
                                "noop", {}, True)

    # ── 屏障 doc B + 任何变化必触发的 head hook ──
    await runtime.exec("mk -f /db1.md", wt_uid="genesis")
    await runtime.exec('"# B\\n\\n- seed: 1" | /> /db1.md', wt_uid="genesis")
    steps_b = parse_code("$B")[0].atoms[0].selector.steps
    runtime.store.register_hook("genesis", "/db1.md", SelectorIR(steps=steps_b),
                                "barrier", {}, True)

    submitted = []

    def write_a(op, text, suppress):
        runtime.store.write_locator(
            "genesis", "/da1.md", SelectorIR(steps=steps_a), op, text,
            auto_scaffold=True, god=True, auto_level=False,
            suppress_hooks=suppress, proj="content")
        submitted.append((op, text, suppress))

    async def wait_submitted(n, timeout=5.0):
        deadline = asyncio.get_running_loop().time() + timeout
        while asyncio.get_running_loop().time() < deadline:
            if len(submitted) >= n:
                return
            await asyncio.sleep(0.01)
        assert len(submitted) >= n, f"等待第 {n} 笔写提交，实际 {len(submitted)}"

    async def wait_barrier(n, timeout=5.0):
        deadline = asyncio.get_running_loop().time() + timeout
        while asyncio.get_running_loop().time() < deadline:
            if len(barrier) >= n:
                return
            await asyncio.sleep(0.01)
        assert len(barrier) >= n, f"等待屏障 {n}，实际 {len(barrier)}"

    async def barrier_push():
        before = len(barrier)
        runtime.store.write_locator(
            "genesis", "/db1.md", SelectorIR(steps=steps_b), "+>", "- b: x\n",
            auto_scaffold=True, god=True, auto_level=False,
            suppress_hooks=False, proj="content")
        await wait_barrier(before + 1)

    async def gen():
        yield "A1"
        yield "A2"
        yield "A3"  # 触发 A2 的 suppress 写；此时 A1/A2 已提交
        await wait_submitted(2)
        await barrier_push()          # 屏障 ⟹ A1/A2 的消化任务均已处理
        assert len(fired_a) == 0, f"流中 suppress 笔不得投递，实际 {len(fired_a)}"
        # 结束后 stream_write finally 落 A3（非 suppress）

    st, _, nb, _ = await stream_write(gen(), "+>", write_a)
    assert st == "completed" and nb == 3, (st, nb)

    await wait_submitted(3)
    await barrier_push()  # 屏障 ⟹ A3 的末笔检测任务已处理
    assert len(fired_a) == 1, f"流末应恰好触发一次，实际 {len(fired_a)}"

    # 再来一次屏障，确认没有迟到/重复投递。
    await barrier_push()
    assert len(fired_a) == 1, f"末笔之后不得再触发，实际 {len(fired_a)}"


@pytest.mark.perf
async def test_concurrent_write_digest_pure_increment(runtime: MdioRuntime) -> None:
    """方案语义（stream_suppress_digest）：流中并发非 suppress 写同 doc——
    edit 自身增量触发一次（不含流内容），流末再触发一次（共 2）。"""
    await runtime.exec("mk -f /s7.md", wt_uid="genesis")
    await runtime.exec('"# T\\n\\n- seed: 1" | /> /s7.md', wt_uid="genesis")
    fired = []
    await _reg_head_hook(runtime, "/s7.md", fired)

    async def _wait_fired(n, timeout=5.0):
        deadline = asyncio.get_running_loop().time() + timeout
        while asyncio.get_running_loop().time() < deadline:
            if len(fired) >= n:
                return
            await asyncio.sleep(0.05)
        assert len(fired) >= n, f"期望触发至少 {n}，实际 {len(fired)}"

    task = asyncio.create_task(runtime.exec(
        '"echo A; sleep 0.3; echo B; sleep 0.3; echo C" | @BASH.stream | +> /s7.md',
        wt_uid="genesis",
    ))
    await asyncio.sleep(0.5)  # A 已 suppress 落笔并消化（不投递）
    await runtime.exec('"Z" | +> /s7.md', wt_uid="genesis")  # 并发写同 doc（非 suppress）
    await _wait_fired(1)
    await asyncio.sleep(0.3)
    assert len(fired) == 1, f"并发 edit 应只报告自己的增量（1 次），实际 {len(fired)}"
    await asyncio.wait_for(task, timeout=10)
    await _wait_fired(2)
    await asyncio.sleep(0.3)
    assert len(fired) == 2, f"流末应再触发一次（共 2 次），实际 {len(fired)}"


async def test_stream_one_shot_hook_exactly_once_then_removed(runtime: MdioRuntime) -> None:
    """一次性 hook watch 流目标：流中 suppress 不误杀，流末恰好触发一次并移除。"""
    from mdio_py.core import parse_code
    await runtime.exec("mk -f /s8.md", wt_uid="genesis")
    await runtime.exec('"# T\\n\\n- seed: 1" | /> /s8.md', wt_uid="genesis")
    fired = []
    runtime.store.register_hook_dispatcher(lambda cb, ctx: fired.append(cb))
    steps = parse_code("$T")[0].atoms[0].selector.steps
    runtime.store.register_hook("genesis", "/s8.md", SelectorIR(steps=steps),
                                "noop", {}, False)
    await asyncio.sleep(0.3)

    res = await runtime.exec(
        '"echo A; echo B; echo C" | @BASH.stream | +> /s8.md',
        wt_uid="genesis",
    )
    assert "stream completed" in str(res), res
    deadline = asyncio.get_running_loop().time() + 5.0
    while asyncio.get_running_loop().time() < deadline and not fired:
        await asyncio.sleep(0.05)
    await asyncio.sleep(0.3)
    assert len(fired) == 1, f"一次性 hook 应恰好触发一次并移除，实际 {len(fired)}"

    await runtime.exec('"Z" | +> /s8.md', wt_uid="genesis")
    await asyncio.sleep(0.5)
    assert len(fired) == 1, f"一次性 hook 移除后不应再触发，实际 {len(fired)}"


async def test_stream_mid_register_digests_current_state(runtime: MdioRuntime) -> None:
    """流中间注册 hook：register 预评估消化当前态——从注册时刻起增量观察。"""
    from mdio_py.core import parse_code
    await runtime.exec("mk -f /s9.md", wt_uid="genesis")
    await runtime.exec('"# T\\n\\n- seed: 1" | /> /s9.md', wt_uid="genesis")

    task = asyncio.create_task(runtime.exec(
        '"echo A; sleep 0.25; echo B; sleep 0.25; echo C" | @BASH.stream | +> /s9.md',
        wt_uid="genesis",
    ))
    await asyncio.sleep(0.5)  # A/B 已 suppress 落笔；C 尚 pending

    fired = []
    runtime.store.register_hook_dispatcher(lambda cb, ctx: fired.append(cb))
    steps = parse_code("$T")[0].atoms[0].selector.steps
    runtime.store.register_hook("genesis", "/s9.md", SelectorIR(steps=steps),
                                "noop", {}, True)
    await asyncio.wait_for(task, timeout=10)

    deadline = asyncio.get_running_loop().time() + 5.0
    while asyncio.get_running_loop().time() < deadline and not fired:
        await asyncio.sleep(0.05)
    await asyncio.sleep(0.3)
    assert len(fired) == 1, f"流中间注册应从注册态起观察并只触发末笔增量，实际 {len(fired)}"

