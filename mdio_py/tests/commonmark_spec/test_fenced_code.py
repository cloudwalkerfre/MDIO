"""§4.5 Fenced code blocks — 29 examples (CM spec §4.5).

CM spec rules:
  - Fence = ≥3 consecutive backticks (`) or tildes (~), cannot mix
  - Opening fence preceded by ≤3 spaces of indent
  - Info string = text after opening fence, trimmed; backtick fence info must not contain backticks
  - Closing fence: same char as opening, ≥ opening length, ≤3 spaces leading indent,
    trailing spaces/tabs allowed (and ignored)
  - Content indent stripping: up to N spaces removed from each content line,
    where N = spaces before opening fence
  - Unclosed fence → closed by end of container/document
  - Can interrupt paragraphs without blank line
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_parse_nested, ast_has_type

H1, H2 = "Leaf blocks", "Fenced code blocks"

# helpers

def _block_types(ast):
    """Return top-level block type list from flat AST."""
    return [b.get("nt") for b in (ast or []) if isinstance(b, dict)]

def _code_tag(ast):
    """Extract code_fence info string from flat AST. Returns info string or None."""
    for b in (ast or []):
        if not isinstance(b, dict):
            continue
        if b.get("nt") == "c":
            tag = b.get("t","")
            if tag:
                return tag
            # Check children for nested code_fence with info string
            for child in b.get("ct", []):
                if isinstance(child, dict) and child.get("nt") == "c":
                    tag = child.get("t","")
                    if tag:
                        return tag
    return None

def _fence_content(ast):
    """Extract fenced code content (cd field) from first code_fence node."""
    for b in (ast or []):
        if isinstance(b, dict) and b.get("nt") == "c":
            return b.get("cd", "")
    return ""


def _inline_code_content(ast):
    """Extract inline code content from ast: returns code text or None."""
    def _search(node):
        if isinstance(node, dict):
            if node.get("nt") == "cd":
                return node.get("cd","")
            for v in node.values():
                if isinstance(v, (dict, list)):
                    r = _search(v)
                    if r: return r
        elif isinstance(node, list):
            for c in node:
                r = _search(c)
                if r: return r
        return None
    return _search(ast)


# Tests

@pytest.mark.asyncio
async def test_fc_00_basic_backtick_fence(cm_workspace):
    """N=0: 3 backticks produce a fenced code block. Content is literal HTML."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "should be a code_block"
    assert _fence_content(ast) == "<\n >\n", f"content 应原样保留: {_fence_content(ast)!r}"

@pytest.mark.asyncio
async def test_fc_01_basic_tilde_fence(cm_workspace):
    """N=1: 3 tildes also produce a fenced code block."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "tilde fence should be a code_block"

@pytest.mark.asyncio
async def test_fc_02_two_backticks_not_fence(cm_workspace):
    """N=2: `` is fewer than 3 → not a fenced code block, it's an inline code span."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert not ast_has_type(ast, "c"), "2 backticks should NOT be a code_block"
    assert ast_has_type(ast, "cd"), "should be an inline code span"
    code = _inline_code_content(ast)
    assert code is not None
    assert code == "foo", f"code content should be 'foo', got {code!r}"

@pytest.mark.asyncio
async def test_fc_03_backtick_open_tilde_close_no_match(cm_workspace):
    """N=3: Opening ``` + closing ~~~ → char mismatch → tilde line is code content."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    # CM: ``` open is closed by the closing ```, ~~~ line is inside. Or unclosed;
    # the spec example shows <pre><code>aaa\n~~~\n</code></pre> — tilde line is content
    assert ast_has_type(ast, "c"), "opening ``` should create a code_block"

@pytest.mark.asyncio
async def test_fc_04_tilde_open_backtick_close_no_match(cm_workspace):
    """N=4: Opening ~~~ + closing ``` → char mismatch → backtick line is code content."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "opening ~~~ should create a code_block"

@pytest.mark.asyncio
async def test_fc_05_fence_longer_than_closing(cm_workspace):
    """N=5: Opening ```` (4) + closing ``` (3) → closing shorter, not enough → unclosed."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    # Waiting for 4+ backticks; closing ``` (3) is too short → content until closing ```` (6)
    # Actually: 4 open, 3 mid, 6 close → content is "aaa\n```\n"
    assert ast_has_type(ast, "c"), "opening ```` should still create a code_block"

@pytest.mark.asyncio
async def test_fc_06_tilde_fence_shorter_close_valid(cm_workspace):
    """N=6: Opening ~~~~ (4) + closing ~~~ (3) → closing shorter, not enough → unclosed."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c")

@pytest.mark.asyncio
async def test_fc_07_unclosed_by_eof(cm_workspace):
    """N=7: Single ``` line with no closing → code block closed by end of document."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "unclosed fence should be a code_block"

@pytest.mark.asyncio
async def test_fc_08_five_open_three_close(cm_workspace):
    """N=8: ````` open then later ``` → 3 < 5 → not enough, no close, eof closes it."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c")

@pytest.mark.asyncio
async def test_fc_09_bq_contains_fenced_code(cm_workspace):
    """N=9: Blockquote containing a fenced code block, then a paragraph outside.

    CM: <blockquote><pre><code>aaa</code></pre></blockquote><p>bbb</p>

    Nested AST shows code_block inside BQ — structure preserved where flat AST flattens it."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse_nested(md)
    assert ast_has_type(ast, "bq"), "outer block should be blockquote"
    assert ast_has_type(ast, "c"), "nested AST shows code_block inside BQ"

@pytest.mark.asyncio
async def test_fc_10_code_block_with_empty_lines(cm_workspace):
    """N=10: Code block with blank lines (content includes spaces)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "code block should persist through blank lines"

@pytest.mark.asyncio
async def test_fc_11_empty_code_block(cm_workspace):
    """N=11: ``` immediately followed by ``` → empty code block."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "empty code block should still be a code_block"

@pytest.mark.asyncio
async def test_fc_12_one_space_indent_fence(cm_workspace):
    """N=12: Opening fence with 1 space indent → code_block, content indent stripped.
    CM spec: '<pre><code>aaa\\naaa\\n</code></pre>' (1 space stripped from ' aaa')"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "1-space indented fence should be a code_block"
    assert _fence_content(ast) == "aaa\naaa\n", f"1 空格应剥离: {_fence_content(ast)!r}"

@pytest.mark.asyncio
async def test_fc_13_two_space_indent_fence(cm_workspace):
    """N=13: Opening fence with 2 spaces indent → code_block, up to 2 spaces stripped from content.
    CM spec: '<pre><code>aaa\\naaa\\naaa\\n</code></pre>' ('  aaa' → 'aaa')"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "2-space indented fence should be a code_block"
    assert _fence_content(ast) == "aaa\naaa\naaa\n", f"2 空格应剥离: {_fence_content(ast)!r}"

@pytest.mark.asyncio
async def test_fc_14_three_space_indent_fence(cm_workspace):
    """N=14: Opening fence with 3 spaces indent → code_block, up to 3 spaces stripped from content.
    CM spec: '<pre><code>aaa\\n aaa\\naaa\\n</code></pre>' (3/4/2 spaces stripped)"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "3-space indented fence should be a code_block"
    assert _fence_content(ast) == "aaa\n aaa\naaa\n", f"3 空格应剥离: {_fence_content(ast)!r}"

@pytest.mark.asyncio
async def test_fc_15_four_space_indent_too_many(cm_workspace):
    """N=15: 4 spaces indent → indented code block (NOT a fenced code block).
    The fence markers become literal content of the indented code block."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic"), (
        f"4-space indented ```` should be indented code_block, got unknown.\n"
        f"md: {md!r}"
    )

@pytest.mark.asyncio
async def test_fc_16_indent_no_match_closing_fence_ok(cm_workspace):
    """N=16: Closing fence indented 2 spaces (≤3), opening fence 0 → valid close.
    CM spec: '<pre><code>aaa\\n</code></pre>'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "2-space indented closing fence should close the block"
    assert _fence_content(ast) == "aaa\n", f"content 应为 aaa: {_fence_content(ast)!r}"

@pytest.mark.asyncio
async def test_fc_17_indented_open_unindented_close(cm_workspace):
    """N=17: Opening 3-space indent, closing 2-space indent → both ≤3 → ok.
    CM spec: '<pre><code>aaa\\n</code></pre>'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c")
    assert _fence_content(ast) == "aaa\n", f"content 应为 aaa: {_fence_content(ast)!r}"

@pytest.mark.asyncio
async def test_fc_18_closing_fence_four_space_indent(cm_workspace):
    """N=18: Closing fence with 4 spaces indent → NOT a closing fence → part of code content."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 18)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "4-space indented closing fence stays as code content"

@pytest.mark.asyncio
async def test_fc_19_space_in_fence_not_valid(cm_workspace):
    """N=19: `` ``` ```` ``  — internal space → not a fence → inline code span."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 19)
    ast = ast_parse(md)
    assert not ast_has_type(ast, "c"), "fence with internal space should NOT be a code_block"
    assert ast_has_type(ast, "cd"), "should be an inline code span (`` ``` `` → inline code)"

@pytest.mark.asyncio
async def test_fc_20_tilde_fence_internal_space_in_close(cm_workspace):
    """N=20: ~~~~~~ open aaa ~~~ ~~ → closing `~~~ ~~` has internal space → not a close → eof closes."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 20)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c")

@pytest.mark.asyncio
async def test_fc_21_fence_interrupts_paragraph(cm_workspace):
    """N=21: Paragraph → fenced code → paragraph, no blank lines needed."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 21)
    ast = ast_parse(md)
    types = _block_types(ast)
    assert "p" in types, "should have paragraph blocks"
    assert ast_has_type(ast, "c"), "fenced code should interrupt paragraph"

@pytest.mark.asyncio
async def test_fc_22_headings_before_and_after_fence(cm_workspace):
    """N=22: Setext heading → fenced code → ATX heading, no blank lines."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 22)
    ast = ast_parse(md)
    assert ast_has_type(ast, "h"), "should have heading block"
    assert ast_has_type(ast, "c"), "should have code_block"

@pytest.mark.asyncio
async def test_fc_23_info_string_ruby(cm_workspace):
    """N=23: ```ruby → info string 'ruby' extracted. Content is the Ruby code."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 23)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "```ruby should be a code_block"
    info = _code_tag(ast)
    assert info == "ruby", f"info string should be 'ruby', got {info!r}"

@pytest.mark.asyncio
async def test_fc_24_complex_info_string(cm_workspace):
    """N=24: ~~~~    ruby startline=3 → info = 'ruby startline=3 $%@#$' (trimmed).
    First word 'ruby' is the language."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 24)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "~~~~ with info string should be a code_block"
    info = _code_tag(ast)
    # info string should start with 'ruby'
    assert info is not None and "ruby" in info, f"info should contain 'ruby', got {info!r}"

@pytest.mark.asyncio
async def test_fc_25_semicolon_info_string(cm_workspace):
    """N=25: ````; → info string ';' → language-; class."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 25)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "```` should be a code_block"
    info = _code_tag(ast)
    assert info == ";", f"info string should be ';', got {info!r}"

@pytest.mark.asyncio
async def test_fc_26_info_string_cannot_contain_backtick(cm_workspace):
    """N=26: ``` aa ``` → info string 'aa ```' contains backtick → NOT a fenced code block.
    It's an inline code span followed by paragraph text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 26)
    ast = ast_parse(md)
    assert not ast_has_type(ast, "c"), (
        "info string containing backtick → should NOT be a code_block"
    )
    assert ast_has_type(ast, "cd"), "should be inline code span"

@pytest.mark.asyncio
async def test_fc_27_tilde_fence_info_contains_backtick(cm_workspace):
    """N=27: ~~~ aa ``` ~~~ → tilde fence, info string CAN contain backticks (tilde fence
    has no restriction). content is 'foo'."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 27)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "tilde fence should be a code_block even with backtick in info"
    info = _code_tag(ast)
    # info should contain 'aa ``` ~~~' or at least 'aa'
    assert info is not None and "aa" in info, f"info should contain 'aa', got {info!r}"

@pytest.mark.asyncio
async def test_fc_28_closing_fence_cannot_have_info_string(cm_workspace):
    """N=28: ``` open, then ``` aaa (closing fence with text) → not a valid close.
    The closing ``` is the next line, so content is '``` aaa'."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 28)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "should be a code_block (``` aaa is content, not closing)"
