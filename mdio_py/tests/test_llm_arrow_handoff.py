"""arrow-only handoff 回归：单次 arrow -json + Python walk 产出的 messages。

覆盖：baseline/branch/join、compact 摘要、tool/thinking 折叠、载荷保真、失败回退、源文档只读。
"""

import pytest

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.llm import arrow_handoff, llm
from mdio_py.plugins.llm.llm import _rebuild_messages

pytestmark = pytest.mark.skipif(
    not arrow_handoff.available(), reason="arrow 扩展/duckdb 不可用")

DOC_BRANCH = """## HIS
### main
- user:
  > 起步
- user:
    + go:
        - type: branch
        - path: /wl.md::HIS.b1
  > 换线
- assistant:
  > 主线尾部残留
- user:
    + jn:
        - type: join
        - path: /wl.md::HIS.b2
  > 拉入
- assistant:
  > join 之后主线继续
### b1
- assistant:
  > B1 分支内容
### b2
- assistant:
  > B2 拉入内容
"""

DOC_COMPACT = """## HIS
### main
- user:
  > 第一轮任务
- assistant:
  > 第一轮回复
- tool_result:
    - id: t1
  > 工具结果一
- user:
    * ts1:
        - type: compact
        - next: 3
        > 前三轮摘要A
        > 前三轮摘要B
  > 继续任务
- assistant:
  > 好的
"""

TOOL_DOC = """## HIS
### main
- user:
  > 跑一下
- assistant:
  > 开始
- tool_call:
    - name: execute_mdio
    - id: call_01
    - tool: pipeline
  > $ /README.md::..*[h] -pj tag
- tool_result:
    - name: execute_mdio
    - id: call_01
  > mdio_success: ok
"""

THINK_DOC = """## HIS
### main
- user:
  > 问
- thinking:
  > 先想一下
- assistant:
  > 答
"""

NESTED_BQ_DOC = """## HIS
### main
- user:
  > # 交接
  >
  > > 引用行
  >
  > 尾部
- assistant:
  > 收到
"""

ESCAPE_DOC = """## HIS
### main
- assistant:
  > 示例："# PROJ\\n- port: 5432"
- user:
  > 继续
"""


def _ctx():
    return RuntimeContext(wt_uid="genesis")


async def _write(rt, text, path="/wl.md"):
    from mdio_py.core.mdio_ir import SelectorIR
    rt._workspace.write_locator("genesis", path, SelectorIR(file_path=None, steps=[]),
                                "/>", text, auto_scaffold=True)
    return str(await rt.exec("$ %s::HIS -pj raw" % path, wt_uid="genesis"))


async def test_branch_and_join(runtime_llm):
    his = await _write(runtime_llm, DOC_BRANCH)
    ws = runtime_llm._workspace
    go = await arrow_handoff.handoff(his, "go", ws, _ctx())
    assert [m.get("content") for m in go] == ["起步", "B1 分支内容"]
    jn = await arrow_handoff.handoff(his, "jn", ws, _ctx())
    assert [m.get("content") for m in jn] == [
        "起步", "换线", "主线尾部残留", "B2 拉入内容", "join 之后主线继续"]


async def test_compact_summary(runtime_llm):
    his = await _write(runtime_llm, DOC_COMPACT)
    msgs = await arrow_handoff.handoff(his, "ts1", runtime_llm._workspace, _ctx())
    assert msgs[0]["content"] == "前三轮摘要A\n前三轮摘要B"


async def test_tool_and_thinking_fold(runtime_llm):
    his = await _write(runtime_llm, TOOL_DOC)
    msgs = await arrow_handoff.handoff(his, "", runtime_llm._workspace, _ctx())
    assert [m["role"] for m in msgs] == ["user", "assistant", "tool"]
    assert msgs[2]["tool_call_id"] == "call_01"
    his = await _write(runtime_llm, THINK_DOC)
    msgs = await arrow_handoff.handoff(his, "", runtime_llm._workspace, _ctx())
    assert msgs[1]["reasoning_content"] == "先想一下"


async def test_nested_bq_payload(runtime_llm):
    his = await _write(runtime_llm, NESTED_BQ_DOC)
    msgs = await arrow_handoff.handoff(his, "", runtime_llm._workspace, _ctx())
    assert msgs[0]["content"].startswith("# 交接")


async def test_arrow_failure_falls_back_to_raw_his(runtime_llm, monkeypatch):
    his = await _write(runtime_llm, DOC_BRANCH)
    ws = runtime_llm._workspace
    ref = _rebuild_messages(his, workspace=ws)

    async def boom(*args, **kwargs):
        raise RuntimeError("boom")

    monkeypatch.setattr(arrow_handoff, "handoff", boom)
    got = await llm._messages_for_turn(his, "go", ws, _ctx())
    assert got == ref


async def test_literal_backslash_n_payload_is_preserved(runtime_llm):
    his = await _write(runtime_llm, ESCAPE_DOC)
    msgs = await arrow_handoff.handoff(his, "", runtime_llm._workspace, _ctx())
    assert [m.get("role") for m in msgs] == ["assistant", "user"]
    assert msgs[0]["content"] == '示例："# PROJ\\n- port: 5432"'


async def test_source_is_untouched(runtime_llm):
    his = await _write(runtime_llm, DOC_BRANCH)
    await arrow_handoff.handoff(his, "go", runtime_llm._workspace, _ctx())
    after = str(await runtime_llm.exec("$ /wl.md::HIS -pj raw", wt_uid="genesis"))
    assert after == his


async def test_bad_gate_path_raises(runtime_llm):
    doc = DOC_BRANCH.replace("- path: /wl.md::HIS.b1", "- path: /ref.md::mem")
    his = await _write(runtime_llm, doc)
    with pytest.raises(ValueError):
        await arrow_handoff.handoff(his, "go", runtime_llm._workspace, _ctx())


async def test_empty_his_returns_empty(runtime_llm):
    assert await arrow_handoff.handoff("", "wl", runtime_llm._workspace, _ctx()) == []
    assert await llm._messages_for_turn("", "wl", runtime_llm._workspace, _ctx()) == []
