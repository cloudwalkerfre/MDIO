"""wl 祖先前缀匹配回归：纯 branch 树、compact 继承、写目标/写盘。"""

import pytest

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.mdio_ir import SelectorIR
from mdio_py.plugins.llm import arrow_handoff, llm

pytestmark = pytest.mark.skipif(
    not arrow_handoff.available(), reason="arrow 扩展/duckdb 不可用")

NESTED = """## HIS
### main
- user:
    > m1
- assistant:
    > m2
- thinking:
    + a:
        - type: branch
        - path: /wl.md::HIS.b_a
### b_a
- user:
    > a0
- thinking:
    + a_1:
        - type: branch
        - path: /wl.md::HIS.b_a_1
    + a_2:
        - type: branch
        - path: /wl.md::HIS.b_a_2
- assistant:
    > a_common
### b_a_1
- user:
    > c1_rec
- assistant:
    > c1_ans
### b_a_2
- user:
    > c2_rec
- assistant:
    > c2_ans
"""

COMPACT = """## HIS
### main
- user:
    > m1
- assistant:
    > m2
- thinking:
    + a:
        - type: branch
        - path: /wl.md::HIS.b_a
### b_a
- user:
    > p1
- assistant:
    > p2
- thinking:
    * a:
        - type: compact
        - next: 2
        > 摘要P
    + a_1:
        - type: branch
        - path: /wl.md::HIS.b_a_1
- assistant:
    > p3
### b_a_1
- user:
    > c1_rec
- assistant:
    > c1_ans
"""


def _ctx():
    return RuntimeContext(wt_uid="genesis")


async def _write(rt, text):
    rt._workspace.write_locator("genesis", "/wl.md", SelectorIR(file_path=None, steps=[]),
                                "/>", text, auto_scaffold=True)
    return str(await rt.exec("$ /wl.md::HIS -pj raw", wt_uid="genesis"))


async def _contents(rt, text, wl):
    his = await _write(rt, text)
    msgs = await arrow_handoff.handoff(his, wl, rt._workspace, _ctx())
    return [m.get("content") for m in msgs]


async def test_nested_branch_tree(runtime_llm):
    assert await _contents(runtime_llm, NESTED, "a") == ["m1", "m2", "a0", "a_common"]
    assert await _contents(runtime_llm, NESTED, "a_1") == ["m1", "m2", "a0", "c1_rec", "c1_ans"]
    assert await _contents(runtime_llm, NESTED, "a_2") == ["m1", "m2", "a0", "c2_rec", "c2_ans"]
    assert await _contents(runtime_llm, NESTED, "") == ["m1", "m2"]


async def test_unknown_prefix_walks_existing_ancestors(runtime_llm):
    assert await _contents(runtime_llm, NESTED, "a_x") == ["m1", "m2", "a0", "a_common"]


async def test_write_targets_follow_prefix_chain(runtime_llm):
    his = await _write(runtime_llm, NESTED)
    secs = llm._parse_his_structure(his)["sections"]
    cases = {"": "main", "a": "b_a", "a_1": "b_a_1", "a_2": "b_a_2", "a_x": "b_a"}
    for wl, expected in cases.items():
        assert llm._walk_write_section(secs, wl) == expected, wl


async def test_append_record_writes_own_branch(runtime_llm):
    await _write(runtime_llm, NESTED)
    ws = runtime_llm._workspace
    llm._append_record(ws, "dev", "/wl.md::HIS", "user", "新记录",
                       {"wl": "a_1"}, _ctx())
    raw = str(await runtime_llm.exec("$ /wl.md -pj raw", wt_uid="genesis"))
    _, _, tail = raw.partition("### b_a\n")
    ba, _, rest = tail.partition("### b_a_1")
    assert "新记录" not in ba
    assert "新记录" in rest


async def test_child_inherits_ancestor_compact(runtime_llm):
    assert await _contents(runtime_llm, COMPACT, "a_1") == [
        "m1", "m2", "摘要P", "c1_rec", "c1_ans"]
    assert await _contents(runtime_llm, COMPACT, "a") == [
        "m1", "m2", "摘要P", "p3"]
