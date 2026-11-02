"""§5.1 Block quotes — 25 examples (CM spec §5.1).

selector: $"Container blocks"."Block quotes"._code[.tag.re("example")][N]
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse_nested, ast_has_type, ast_to_html

H1, H2 = "Container blocks", "Block quotes"


def _types(ast):
    return [b.get("nt") for b in ast if isinstance(b, dict)]

def _txts(ast):
    r = []
    def _w(n):
        if isinstance(n, dict) and n.get("nt") == "txt":
            c = n.get("ct","")
            if c: r.append(c)
        if isinstance(n, dict):
            # Include content field for block leaf nodes.
            # Split by newlines so assertions like 'bar' in _txts() work
            # against individual line content.
            ct = n.get("ct")
            if ct and n.get("nt") not in ("txt","sb","hr"):
                # ct 双型:字符串拆行 / 数组(内容流)逐元素。
                if isinstance(ct, str):
                    for line in ct.split('\n'):
                        line = line.strip()
                        if line: r.append(line)
                elif isinstance(ct, list):
                    for it in ct:
                        if isinstance(it, str):
                            if it.strip(): r.append(it.strip())
                        elif isinstance(it, dict):
                            _w(it)
            for v in n.values():
                if isinstance(v, (dict, list)): _w(v)
        elif isinstance(n, list):
            for c in n: _w(c)
    _w(ast); return r


def _all(node, node_type):
    """Recursively find ALL nodes of given type (including nested)."""
    results = []
    if isinstance(node, dict) and node.get("nt") == node_type:
        results.append(node)
    if isinstance(node, dict):
        for v in node.values():
            if isinstance(v, (list, dict)):
                results.extend(_all(v, node_type))
    elif isinstance(node, list):
        for c in node:
            results.extend(_all(c, node_type))
    return results


# Tests

@pytest.mark.asyncio
async def test_bq_00_basic_heading_and_paragraph(cm_workspace):
    """N=0: BQ with H1 + paragraph. `> ` prefix on each line."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq"), "should be a blockquote"
    assert ast_has_type(ast, "h"), "BQ should contain a heading"

@pytest.mark.asyncio
async def test_bq_01_no_space_after_gt(cm_workspace):
    """N=1: `>#` (no space) → valid BQ marker."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    assert ast_has_type(ast, "h")

@pytest.mark.asyncio
async def test_bq_01_content_stripped(cm_workspace):
    """BQ heading 渲染为 head 节点，content 已剥离。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse_nested(md)

    # Heading inside BQ should be a proper head node with "Foo" content,
    # not "># Foo" inline text.
    def _head_content(n, level, text):
        if isinstance(n, dict) and n.get("nt") == "h":
            return n.get("lv") == level and n.get("t","") == text
        if isinstance(n, dict):
            for v in n.values():
                if isinstance(v, (dict, list)) and _head_content(v, level, text): return True
        elif isinstance(n, list):
            for c in n:
                if _head_content(c, level, text):
                    return True
        return False
    assert _head_content(ast, 1, "Foo"), "BQ heading should be H1 with content 'Foo'"

@pytest.mark.asyncio
async def test_bq_02_three_space_indent(cm_workspace):
    """N=2: ≤3 spaces before `>` → valid BQ."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")

@pytest.mark.asyncio
async def test_bq_03_four_space_indent(cm_workspace):
    """N=3: 4 spaces → indented code block, NOT a BQ."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast, "ic"), "4-space indent should be indented code"

@pytest.mark.asyncio
async def test_bq_04_lazy_continuation(cm_workspace):
    """N=4: `baz` without `>` is lazy paragraph continuation inside BQ."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    assert ast_has_type(ast, "h")
    assert "baz" in _txts(ast), "lazy 'baz' should be inside BQ"

@pytest.mark.asyncio
async def test_bq_05_mixed_lazy_explicit(cm_workspace):
    """N=5: Mix of `>` and non-`>` lines → all in same BQ paragraph."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    print(f"\n[MDBUG] input={repr(md)}", flush=True)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    for w in ("bar", "baz", "foo"):
        assert w in _txts(ast), f"'{w}' should be in BQ"

@pytest.mark.asyncio
async def test_bq_06_tb_not_lazy(cm_workspace):
    """N=6: After `> foo`, `---` without `>` is thematic break, NOT BQ text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq"), "should have BQ with 'foo'"
    assert ast_has_type(ast,"hr"), "--- should be thematic break"
    assert "---" not in _txts(ast)[:5], "--- should NOT be BQ text"

@pytest.mark.asyncio
async def test_bq_07_list_in_bq_vs_outside(cm_workspace):
    """N=7: `> - foo` → BQ+list. `- bar` (no `>`) → separate list outside BQ.

    Nested AST preserves bullet structure inside/outside BQ."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq"), "should have BQ"
    assert ast_has_type(ast,"b"), "should have bullet list"
    # 差异集⑥ 例 8：`- bar` 0 缩进 marker 应打断——
    # 在 BQ 外新列表（item leaf 的 ct 剥 marker——断言用内容文本"bar"）
    assert "bar" not in _txts(_all(ast, "bq")[0]), \
        "例 8: bar 不应是 BQ 段落文本（lazy 行应打断——0 缩进 marker）"
    outside = [n for n in ast if isinstance(n, dict) and n.get("nt") != "bq"]
    assert any("bar" in _txts(n) for n in outside), "例 8: bar 应在 BQ 外新列表"

@pytest.mark.asyncio
async def test_bq_08_indented_code_in_bq(cm_workspace):
    """N=8: 5 spaces after `>` → indented code block inside BQ."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    assert ast_has_type(ast, "ic"), "BQ should contain indented code"

@pytest.mark.asyncio
async def test_bq_09_fenced_code_in_bq(cm_workspace):
    """N=9: `> ``` ` → fenced code block inside BQ. Nested AST shows code_block child."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    assert ast_has_type(ast, "c"), "nested AST shows code_fence inside BQ"

@pytest.mark.asyncio
async def test_bq_10_indented_paragraph(cm_workspace):
    """N=10: `    - bar` inside BQ → paragraph continuation, NOT a sublist."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    assert "- bar" in _txts(ast), "indented '- bar' should be BQ paragraph text"

@pytest.mark.asyncio
async def test_bq_11_empty_bq(cm_workspace):
    """N=11: `>` alone → empty blockquote."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")

@pytest.mark.asyncio
async def test_bq_12_multiple_empty(cm_workspace):
    """N=12: `>\n>  \n> ` → single empty BQ."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")

@pytest.mark.asyncio
async def test_bq_13_content_then_empty(cm_workspace):
    """N=13: `>`, `> foo`, `>  ` → BQ with paragraph then trailing empty line."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")

@pytest.mark.asyncio
async def test_bq_14_blank_line_separates(cm_workspace):
    """N=14: Blank line → two separate BQs."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse_nested(md)
    ts = _types(ast)
    assert ts.count("bq") == 2, f"expected 2 BQs, got {ts}"

@pytest.mark.asyncio
async def test_bq_15_single_paragraph(cm_workspace):
    """N=15: Two `>` lines → single paragraph in BQ."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    for w in ("foo", "bar"):
        assert w in _txts(ast), f"'{w}' should be in BQ"

@pytest.mark.asyncio
async def test_bq_16_empty_gt_separates_paragraphs(cm_workspace):
    """N=16: `>`, `> foo`, `>`, `> bar` → two paragraphs in same BQ."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")

@pytest.mark.asyncio
async def test_bq_17_paragraph_then_bq(cm_workspace):
    """N=17: `foo` paragraph then `> bar` BQ, no blank line needed."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse_nested(md)
    ts = _types(ast)
    assert "p" in ts, f"should have paragraph, got {ts}"
    assert "bq" in ts, f"should have BQ, got {ts}"

@pytest.mark.asyncio
async def test_bq_18_tb_separates_bqs(cm_workspace):
    """N=18: Thematic break between two BQs."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 18)
    ast = ast_parse_nested(md)
    ts = _types(ast)
    assert "hr" in ts, f"should have TB, got {ts}"
    assert ts.count("bq") == 2, f"should have 2 BQs, got {ts}"

@pytest.mark.asyncio
async def test_bq_19_lazy_single_line(cm_workspace):
    """N=19: `> bar\nbaz` → `baz` is lazy continuation."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 19)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    for w in ("bar", "baz"):
        assert w in _txts(ast), f"'{w}' should be in BQ"

@pytest.mark.asyncio
async def test_bq_20_blank_line_ends_bq(cm_workspace):
    """N=20: Blank line → BQ ends, paragraph follows."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 20)
    ast = ast_parse_nested(md)
    ts = _types(ast)
    assert "bq" in ts
    assert "p" in ts, f"should have paragraph after BQ, got {ts}"

@pytest.mark.asyncio
async def test_bq_21_empty_gt_ends_bq(cm_workspace):
    """N=21: `> bar\n>\nbaz` → empty `>` ends BQ, `baz` is paragraph."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 21)
    ast = ast_parse_nested(md)
    ts = _types(ast)
    assert "bq" in ts
    assert "p" in ts, f"should have paragraph after BQ, got {ts}"

@pytest.mark.asyncio
async def test_bq_22_triple_nested(cm_workspace):
    """N=22: `> > > foo\nbar` → three nested BQs. Nested AST preserves 3 levels."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 22)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    bq_nodes = list(_all(ast, "bq"))
    assert len(bq_nodes) >= 3, f"expected ≥3 nested BQs, got {len(bq_nodes)}: {bq_nodes}"

@pytest.mark.asyncio
async def test_bq_23_compact_triple(cm_workspace):
    """N=23: `>>> foo\n> bar\n>>baz` → three nested BQs in nested AST."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 23)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq")
    bq_nodes = list(_all(ast, "bq"))
    assert len(bq_nodes) >= 3, f"expected ≥3 nested BQs, got {len(bq_nodes)}: {bq_nodes}"

@pytest.mark.asyncio
async def test_bq_24_code_vs_paragraph(cm_workspace):
    """N=24: `>     code` → indented code in BQ. Two BQs separated by blank line."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 24)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast,"bq"), "should have BQ"
    assert ast_has_type(ast, "ic"), "BQ should contain indented code block"


@pytest.mark.asyncio
async def test_bq_all_html_matches_cm_expected(cm_workspace):
    """25 例逐例整段 HTML 全等（与 ATX/Setext/Thematic breaks 三段同一强度）。

    逐例整段 HTML 全等可覆盖引用内的缩进码内容、空引用等形态；
    引用族与「引用内缩进码」族的回归由官方例子挡住。
    """
    rt = MdioRuntime(cm_workspace)
    bad = []
    for i in range(25):  # 本段用例数 25（test_bq_NN）
        md, want_html = await cm_example(rt, H1, H2, i)
        got = ast_to_html(ast_parse_nested(md)).strip()
        if got != want_html.strip():
            bad.append(f"#{i:02d} mdio={got!r} CM={want_html.strip()!r}")
    assert not bad, "引用段与 CM 期望不符：\n" + "\n".join(bad)
