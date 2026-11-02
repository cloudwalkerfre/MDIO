"""§4.2 ATX headings — 18 examples (CM spec §4.2).

CM spec rules:
  - Opening: 1-6 `#` followed by space or tab (or end of line) → H1-H6
  - 7+ `#` is NOT a heading (paragraph text)
  - `#` without space/tab after (e.g. `#5bolt`, `#hashtag`) is NOT a heading
  - Closing `#` optional, length need not match opening, trailing spaces allowed
  - Closing `#` must be preceded by space/tab, otherwise part of content
  - Backslash-escaped `#` does NOT form heading; escape active before content
  - ≤3 spaces indent → still heading; ≥4 spaces → indented code block
  - ATX heading can interrupt paragraph; content inline elements (em, etc.) work
  - Leading/trailing whitespace stripped from heading content

AST structure:
  head node: ["head", level, mark?, content]
  - level at [1]: integer 1-6
  - mark is at [2]: string of opening `#` characters (e.g. "##", "###")
  - content at [3]: string if plain text, list if inline children
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_find_all, ast_to_html

H1, H2 = "Leaf blocks", "ATX headings"


# Tests — §4.2 ATX headings (18 examples)


@pytest.mark.asyncio
async def test_atx_00_six_headings_h1_through_h6(cm_workspace):
    """N=0: # foo, ## foo, ... ###### foo → 6 headings at levels 1-6.
    CM spec §4.2: 1-6 unescaped `#` chars → H1-H6; level = number of `#`."""
    rt = MdioRuntime(cm_workspace)
    md, expected = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert len(ast) == 6
    for i, block in enumerate(ast):
        assert block.get("nt") == "h"
        assert block.get("lv") == i + 1
        assert block.get("t","") == "foo"
    # HTML 渲染对比 (端到端)
    assert ast_to_html(ast).strip() == expected.strip(), (
        f"HTML 渲染不一致: {ast_to_html(ast)!r} vs {expected!r}"
    )


@pytest.mark.asyncio
async def test_atx_1_seven_hashes(cm_workspace):
    """N=1: ####### foo → 7 个 # 不是 heading。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"


@pytest.mark.asyncio
async def test_atx_2_no_space(cm_workspace):
    """N=2: #5 bolt / #hashtag → 无空格不是 heading。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert len(ast) == 2
    assert all(b.get("nt") == "p" for b in ast)


@pytest.mark.asyncio
async def test_atx_3_escaped_hash(cm_workspace):
    """N=3: \\## foo → 转义 # 不产生 heading。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    # CM spec：\# 转义后不形成 heading，是 paragraph
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"
    assert not any(isinstance(b, dict) and b.get("nt") == "h" for b in ast)


@pytest.mark.asyncio
async def test_atx_4_inline(cm_workspace):
    """N=4: # foo *bar* \\*baz\\* → heading 内 inline emphasis。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert len(ast) == 1
    head = ast[0]
    assert head.get("nt") == "h"
    assert head.get("lv") == 1
    # 标题行内写在 Tag 面自己的键 t 下（数组）；Content 面 ct 恒空。
    inlines = head.get("t", [])
    assert isinstance(inlines, list), f"t 应为行内数组, got {type(inlines)}"
    # 应有 em 节点
    has_em = any(isinstance(n, dict) and n.get("nt") == "em" for n in inlines)
    assert has_em
    # 空 Content 面写成空串而非省略该键。
    assert head.get("ct") == "", f"标题 Content 面应为空串, got {head.get('ct')!r}"


@pytest.mark.asyncio
async def test_atx_5_spaces_stripped(cm_workspace):
    """N=5: #          foo               → 首尾空格去除。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "h"
    assert ast[0].get("lv") == 1
    assert ast[0].get("t") == "foo"


@pytest.mark.asyncio
async def test_atx_6_indent_ok(cm_workspace):
    """N=6: ≤3 空格缩进仍是 heading。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert len(ast) == 3
    levels = [b.get("lv") for b in ast]
    assert levels == [3, 2, 1]
    assert all(b.get("t") == "foo" for b in ast)


@pytest.mark.asyncio
async def test_atx_07_four_spaces_indent_code_block(cm_workspace):
    """N=7: ····# foo → 4 空格缩进 → indented code block（非 heading）。
    CM spec §4.2: 'Four spaces of indentation is too many.'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "ic", f"expected code_block, got {ast[0].get('nt')!r}"


@pytest.mark.asyncio
async def test_atx_08_indent_in_paragraph_not_heading(cm_workspace):
    """N=8: foo\\n····# bar → 段落内缩进 # 是 lazy continuation。
    CM spec §4.2: indented `#` inside paragraph is paragraph text, not heading.
    Expected HTML: <p>foo\n# bar</p> — leading spaces before `#` are stripped."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"
    assert not any(isinstance(b, dict) and b.get("nt") == "h" for b in ast)

    # CM spec expects "# bar" without leading spaces in paragraph content
    txts = [n.get("ct","") for n in ast_find_all(ast, "txt")]
    combined = "".join(txts)
    if "#" in combined:
        bar_part = [t for t in txts if "bar" in t]
        if bar_part:
            assert bar_part[0] == "# bar", (
                f"CM spec 期望 '# bar'（无前导空格），MDIO: {bar_part[0]!r}"
            )


@pytest.mark.asyncio
async def test_atx_9_closing_optional(cm_workspace):
    """N=9: ## foo ## / ### bar ### → closing # 可选。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    assert len(ast) == 2
    assert ast[0].get("lv") == 2 and ast[0].get("t") == "foo"
    assert ast[1].get("lv") == 3 and ast[1].get("t") == "bar"


@pytest.mark.asyncio
async def test_atx_10_closing_length_unmatched(cm_workspace):
    """N=10: closing # 长度不需要匹配 opening。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    assert len(ast) == 2
    assert ast[0].get("lv") == 1 and ast[0].get("t") == "foo"
    assert ast[1].get("lv") == 5 and ast[1].get("t") == "foo"


@pytest.mark.asyncio
async def test_atx_11_closing_trailing_spaces(cm_workspace):
    """N=11: closing # 后空格正常。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("lv") == 3 and ast[0].get("t") == "foo"


@pytest.mark.asyncio
async def test_atx_12_closing_followed_by_text(cm_workspace):
    """N=12: ### foo ### b → ### b 是 content 一部分。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("lv") == 3
    assert ast[0].get("t") == "foo ### b"


@pytest.mark.asyncio
async def test_atx_13_closing_must_have_space(cm_workspace):
    """N=13: # foo# → 无空格 closing # 是 content。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("lv") == 1
    assert ast[0].get("t") == "foo#"


@pytest.mark.asyncio
async def test_atx_14_escaped_closing(cm_workspace):
    """N=14: ### foo \\### → 转义 # 不计入 closing sequence。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse(md)
    assert len(ast) == 3
    assert all(b.get("nt") == "h" for b in ast)
    assert [b.get("lv") for b in ast] == [3, 2, 1]


@pytest.mark.asyncio
async def test_atx_15_interrupt_with_tb(cm_workspace):
    """N=15: **** / ## foo / **** → heading 在 thematic breaks 间。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    assert len(ast) == 3
    assert ast[0].get("nt") == "hr"
    assert ast[1].get("nt") == "h" and ast[1].get("lv") == 2
    assert ast[2].get("nt") == "hr"


@pytest.mark.asyncio
async def test_atx_16_interrupt_para(cm_workspace):
    """N=16: heading 可打断 paragraph。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    assert len(ast) == 3
    assert ast[0].get("nt") == "p"
    assert ast[1].get("nt") == "h" and ast[1].get("lv") == 1
    assert ast[2].get("nt") == "p"


@pytest.mark.asyncio
async def test_atx_17_empty(cm_workspace):
    """N=17: ## / # / ### ### → 空 heading。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse(md)
    assert len(ast) == 3
    assert all(b.get("nt") == "h" for b in ast)
    assert [b.get("lv") for b in ast] == [2, 1, 3]
