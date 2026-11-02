"""§4.4 Indented code blocks — 12 examples (CM spec §4.4).

selector: $"Leaf blocks"."Indented code blocks"._code[.tag.re("example")][N]

CM spec rules:
  - 4+ spaces or 1 tab at line start → indented code block
  - Blank lines inside don't end the block
  - Content is literal text, not parsed as markdown
  - Can interrupt paragraphs
  - Trailing blank lines stripped from output
  - One level of indentation (4 spaces) removed from each content line

值断言: 12 例逐例比对 ic 内容与 CM 期望 HTML 的 <pre><code> 文本——
不只断节点类型（例 09 的八空格应只剥四列）。
"""

import html as _html
import re

import pytest

from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_find_all, ast_has_type, ast_to_html

H1, H2 = "Leaf blocks", "Indented code blocks"


def _block_types(ast):
    return [b.get("nt") for b in ast if isinstance(b, dict)]


_PRE_CODE = re.compile(r"<pre><code[^>]*>(.*?)</code></pre>", re.S)


def _cm_code_texts(expected_html: str):
    """CM 期望 HTML 里每个 <pre><code> 的文本（反转义；去尾换行以便与 mdio 比）。"""
    return [_html.unescape(t).rstrip("\n") for t in _PRE_CODE.findall(expected_html)]


def _mdio_ic_texts(ast):
    """mdio AST 里每个 ic 节点的内容（短键 cd；去尾换行以便与 CM 比）。"""
    return [n.get("cd", "").rstrip("\n") for n in ast_find_all(ast, "ic")]



@pytest.mark.asyncio
async def test_ic_00_simple(cm_workspace):
    """N=0: Simple indented code block (4 spaces)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic"), "4-space indent should be indented code"

@pytest.mark.asyncio
async def test_ic_01_after_list(cm_workspace):
    """N=1: List item with paragraph continuation via 4-space indent — NOT indented code.

    CM: `<ul><li><p>foo</p><p>bar</p></li></ul>` — 4 spaces inside list = paragraph break."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert ast_has_type(ast,"b"), "should be a bullet list, not indented code"

@pytest.mark.asyncio
async def test_ic_02_after_ordered_list(cm_workspace):
    """N=2: Ordered list with sub-list — NOT indented code.

    CM: `<ol><li><p>foo</p><ul><li>bar</li></ul></li></ol>` — 4-space indent in OL = sub-list."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert ast_has_type(ast, "od"), "should be an ordered list, not indented code"

@pytest.mark.asyncio
async def test_ic_03_html_content(cm_workspace):
    """N=3: Indented code with HTML-like content — literal, not HTML."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic")

@pytest.mark.asyncio
async def test_ic_04_blank_lines(cm_workspace):
    """N=4: Indented code with blank lines inside — block continues."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic")

@pytest.mark.asyncio
async def test_ic_05_trailing_spaces(cm_workspace):
    """N=5: Indented code — trailing spaces in lines preserved."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic")

@pytest.mark.asyncio
async def test_ic_06_paragraph_then_indented(cm_workspace):
    """N=6: Paragraph then 4-space line — paragraph continuation, NOT indented code.

    CM: `<p>Foo\nbar</p>` — `    bar` is lazy paragraph continuation, not indented code."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    types = _block_types(ast)
    assert "p" in types, "should be a single paragraph"
    assert not ast_has_type(ast, "ic"), "should NOT be indented code"

@pytest.mark.asyncio
async def test_ic_07_indented_then_paragraph(cm_workspace):
    """N=7: Indented code then non-indented paragraph."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic"), "first block should be indented code"
    assert ast_has_type(ast, "p"), "second should be paragraph"

@pytest.mark.asyncio
async def test_ic_08_mixed_headings_and_code(cm_workspace):
    """N=8: Headings and indented code — headings detected, indented code → unknown.

    CM: `<h1>Heading</h1><pre><code>foo</code></pre><h2>Heading</h2><pre><code>foo</code></pre><hr />`"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    headings = [b for b in ast if isinstance(b, dict) and b.get("nt") == "h"]
    assert len(headings) == 2, f"expected 2 headings, got {len(headings)}: {headings}"
    assert ast_has_type(ast,"hr"), "should have thematic break"
    assert ast_has_type(ast, "ic"), "CM spec has indented code blocks between headings"

@pytest.mark.asyncio
async def test_ic_09_extra_indented(cm_workspace):
    """N=9: 8 spaces → indented code (content dedented by 4)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic")

@pytest.mark.asyncio
async def test_ic_10_empty_first_line(cm_workspace):
    """N=10: Empty first line → indented code block continues."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic")

@pytest.mark.asyncio
async def test_ic_11_trailing_spaces_line(cm_workspace):
    """N=11: Indented code line with trailing spaces."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic")


@pytest.mark.asyncio
async def test_ic_all_contents_and_html_match_cm_expected(cm_workspace):
    """12 例逐例比对：ic 内容 == CM 期望 HTML 的 <pre><code> 文本，且整段 HTML 全等。

    两条都断：内容级给出精确诊断，整段 HTML 全等与 ATX/Setext/Thematic breaks
    三段的判据同一强度（例 01/02 的嵌套列表形态由 _render_item 支撑）。
    """
    rt = MdioRuntime(cm_workspace)
    bad = []
    for i in range(12):  # 本段用例数 12（test_ic_NN）
        md, want_html = await cm_example(rt, H1, H2, i)
        ast = ast_parse(md)
        got_txt, want_txt = _mdio_ic_texts(ast), _cm_code_texts(want_html)
        if got_txt != want_txt:
            bad.append(f"#{i:02d} ic 内容 mdio={got_txt!r} CM={want_txt!r}")
            continue
        got_html = ast_to_html(ast).strip()
        if got_html != want_html.strip():
            bad.append(f"#{i:02d} HTML mdio={got_html!r} CM={want_html.strip()!r}")
    assert not bad, "ic 段与 CM 期望不符：\n" + "\n".join(bad)
