"""§4.6 HTML blocks — 46 examples, 6 tested (CM spec §4.6).

CM spec rules:
  - There are 7 types of HTML blocks, each with different start/end conditions
  - Type 1: <script>, <pre>, <style>, <textarea> (case-insensitive)
  - Type 2: <!-- (HTML comment)
  - Type 3: <? (processing instruction)
  - Type 4: <! followed by ASCII letter (declaration)
  - Type 5: <![CDATA[
  - Type 6: specific tag names (div, table, etc.) + optional closing /
    End condition: blank line or end of container
  - Type 7: complete open/close tag with only whitespace after → does not
    need blank line to end
  - HTML block content is passed through RAW — no inline or block parsing
  - HTML blocks can contain blank lines unless they terminate the block
  - HTML blocks can start in containers (blockquotes, list items)
  - HTML blocks cannot interrupt paragraphs

Test approach:
  Uses ast_to_html() comparison for correct-by-construction verification.
  Each test asserts get_rendered_html == expected_CM_spec_html.
  Known MDIO deviations are marked xfail with root cause.

Non-verifiable spec examples:
  DTO-bound — the CM spec uses `←` for tab and `<URL>` placeholder in
  HTML blocks, which are not reproducible in the validation environment.
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_parse_nested, ast_to_html


def check_example(md_input, expected):
    """CM strict comparison: rendered MDIO HTML vs expected CM spec HTML."""
    ast = ast_parse(md_input)
    result = ast_to_html(ast)
    assert result.strip() == expected.strip()


# Tests — §4.6 HTML blocks (selected examples)

# Type 6 HTML blocks (end on blank line)

@pytest.mark.asyncio
async def test_hb_00_type6_table_raw_passthrough(cm_workspace):
    """N=0: <table><tr><td><pre>**Hello**,_world_.</pre>...</table> → type 6.
    CM spec §4.6: HTML block content is NOT parsed as inline markdown.
    **Hello** and _world_ should be literal text, not emphasis."""
    rt = MdioRuntime(cm_workspace)
    md_input, expected = await cm_example(rt, "Leaf blocks", "HTML blocks", 0)
    check_example(md_input, expected)


@pytest.mark.asyncio
async def test_hb_02_type6_div_raw_passthrough(cm_workspace):
    """N=2: <div>...*hello*...<foo><a> → type 6 HTML block.
    *hello* should be literal text, not inline emphasis."""
    rt = MdioRuntime(cm_workspace)
    md_input, expected = await cm_example(rt, "Leaf blocks", "HTML blocks", 2)
    check_example(md_input, expected)


# Type 7 HTML blocks (end on same line)

@pytest.mark.asyncio
async def test_hb_05_type7_multiline_attributes(cm_workspace):
    """N=5: <div id=\"foo\"\\n  class=\"bar\">\\n</div> → type 7 with multiline.
    CM spec: complete open/close tag, only whitespace after → single-line block."""
    rt = MdioRuntime(cm_workspace)
    md_input, expected = await cm_example(rt, "Leaf blocks", "HTML blocks", 5)
    check_example(md_input, expected)


# Type 1 HTML blocks (end on matching close tag)

@pytest.mark.asyncio
async def test_hb_19_type1_del_raw_passthrough(cm_workspace):
    """N=19: <del>\\n*foo*\\n</del> → type 1 HTML block.
    *foo* should be literal text, NOT emphasis."""
    rt = MdioRuntime(cm_workspace)
    md_input, expected = await cm_example(rt, "Leaf blocks", "HTML blocks", 19)
    check_example(md_input, expected)


@pytest.mark.asyncio
async def test_hb_24_type1_script_raw_passthrough(cm_workspace):
    """N=24: <script type=\"text/javascript\">...</script> → type 1.
    Script content should be raw passthrough."""
    rt = MdioRuntime(cm_workspace)
    md_input, expected = await cm_example(rt, "Leaf blocks", "HTML blocks", 24)
    check_example(md_input, expected)


# HtmlElement containers

@pytest.mark.asyncio
async def test_hb_elements_div_contains_paragraph(cm_workspace):
    """HtmlBlock type 6: <div>\\n<p>Hello</p>\\n</div> → hb 整体节点
    （CM §4.6 raw 透传——同行 Open+Close 配对 → Inline token → 无
    HtmlElement 帧——he 子节点不存在——hb 整体 ct=raw）"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("<div>\n<p>Hello</p>\n</div>\n")
    assert "hb" in str(ast)
    assert "he" not in str(ast)
