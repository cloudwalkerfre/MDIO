"""§6.1 Code spans — 22 examples (CM spec §6.1).

selector: $"Inlines"."Code spans"._code[.tag.re("example")][N]

CM spec rules:
  - Code span = run of n backtick chars, then content, then matching n backtick chars
  - Single leading+trailing space stripped (one from each side) if BOTH sides have a space
  - Newlines → spaces
  - Multiple spaces preserved (not collapsed)
  - Backslashes inside code span are NOT processed (literal)
  - Code span takes priority over emphasis, link, HTML
  - Autolink/HTML tag takes priority over code span
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_has_type, ast_find_all

H1, H2 = "Inlines", "Code spans"


def _find_code(ast):
    """Find first inline_code node, returns code content string or None."""
    def _search(node):
        if isinstance(node, dict):
            if node.get("nt") == "cd":
                return node.get("cd", "")
            for v in node.values():
                if isinstance(v, (dict, list)):
                    r = _search(v)
                    if r: return r
        elif isinstance(node, list):
            for item in node:
                r = _search(item)
                if r: return r
        return None
    return _search(ast)


def _find_all_codes(ast):
    """Find all inline_code nodes, returns list of code content strings."""
    codes = []
    def _search(node):
        if isinstance(node, dict):
            if node.get("nt") == "cd":
                codes.append(node.get("cd", ""))
            for v in node.values():
                if isinstance(v, (dict, list)):
                    _search(v)
        elif isinstance(node, list):
            for item in node:
                _search(item)
    _search(ast)
    return codes


# Tests

@pytest.mark.asyncio
async def test_cs_00_basic(cm_workspace):
    """N=0: `foo` → code span content='foo'."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "foo"

@pytest.mark.asyncio
async def test_cs_01_double_backtick(cm_workspace):
    """N=1: `` foo ` bar `` → double backtick wraps internal single backtick."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "foo ` bar"

@pytest.mark.asyncio
async def test_cs_02_backtick_content(cm_workspace):
    """N=2: ` `` ` → content is two backticks, spaces stripped."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "``"

@pytest.mark.asyncio
async def test_cs_03_double_space_stripping(cm_workspace):
    """N=3: `  ``  ` → CM strips one space from each side → ' `` '."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == " `` "

@pytest.mark.asyncio
async def test_cs_04_single_side_space(cm_workspace):
    """N=4: ` a` → 仅左边有空格，不剥离 → ' a'."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == " a"

@pytest.mark.asyncio
async def test_cs_05_both_side_space(cm_workspace):
    """N=5: ` b ` → 两侧各一个空格，各剥一个 → 'b'."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "b"

@pytest.mark.asyncio
async def test_cs_06_two_spaces(cm_workspace):
    """N=6: ` ` + `  ` → two code spans with space content ' ' and '  '."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    codes = _find_all_codes(ast)
    assert len(codes) == 2, f"expected 2 code spans, got {len(codes)}"
    assert codes[0] == " "
    assert codes[1] == "  "

@pytest.mark.asyncio
async def test_cs_07_newline_space_preservation(cm_workspace):
    """N=7: ``\nfoo\nbar  \nbaz\n`` → newlines→spaces, spaces preserved."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "foo bar   baz"

@pytest.mark.asyncio
async def test_cs_08_trailing_space(cm_workspace):
    """N=8: ``\nfoo \n`` → trailing space preserved → 'foo '."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "foo "

@pytest.mark.asyncio
async def test_cs_09_multiline_spaces(cm_workspace):
    """N=9: `foo   bar \nbaz` → spaces preserved → 'foo   bar  baz'."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "foo   bar  baz"

@pytest.mark.asyncio
async def test_cs_10_escaped_backtick_in_span(cm_workspace):
    r"""N=10: `foo\`bar` → backslash is literal in code span, span closes at first `."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "foo\\"

@pytest.mark.asyncio
async def test_cs_11_double_encloses_single(cm_workspace):
    """N=11: ``foo`bar`` → double backtick encloses internal single backtick."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "foo`bar"

@pytest.mark.asyncio
async def test_cs_12_single_encloses_double(cm_workspace):
    """N=12: ` foo `` bar ` → single backtick encloses double backtick content."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "foo `` bar"

@pytest.mark.asyncio
async def test_cs_13_code_span_priority_over_emph(cm_workspace):
    """N=13: *foo`*` → code span takes priority over emphasis."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "*"

@pytest.mark.asyncio
async def test_cs_14_code_span_priority_over_link(cm_workspace):
    """N=14: [not a `link](/foo`) → code span takes priority over link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "link](/foo"

@pytest.mark.asyncio
async def test_cs_15_html_in_code_span(cm_workspace):
    """N=15: `<a href="`">` → code span wraps HTML attribute."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == '<a href="'

@pytest.mark.asyncio
async def test_cs_16_html_tag_priority(cm_workspace):
    """N=16: <a href="`">` → HTML tag opens first, no code span."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    assert _find_code(ast) is None, "should be HTML tag, not code span"

@pytest.mark.asyncio
async def test_cs_17_url_in_code_span(cm_workspace):
    """N=17: `<https://foo.bar.`baz>` → code span wraps URL prefix."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "<https://foo.bar."

@pytest.mark.asyncio
async def test_cs_18_autolink_priority(cm_workspace):
    """N=18: <https://foo.bar.`baz>` → autolink, no code span."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 18)
    ast = ast_parse(md)
    assert _find_code(ast) is None, "should be autolink, not code span"

@pytest.mark.asyncio
async def test_cs_19_triple_backtick_literal(cm_workspace):
    """N=19: ```foo`` → 3 bt opener + 2 bt closer → mismatch → literal text, no code span."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 19)
    ast = ast_parse(md)
    assert _find_code(ast) is None, (
        "3 bt opener + 2 bt closer = mismatch, should be literal text not code span"
    )

@pytest.mark.asyncio
async def test_cs_20_unclosed_backtick(cm_workspace):
    """N=20: `foo → unclosed → literal text, no code span."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 20)
    ast = ast_parse(md)
    assert _find_code(ast) is None

@pytest.mark.asyncio
async def test_cs_21_partial_match(cm_workspace):
    """N=21: `foo``bar`` → first ` unclosed, ``bar`` is valid code span."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 21)
    ast = ast_parse(md)
    code = _find_code(ast)
    assert code is not None
    assert code == "bar"


@pytest.mark.asyncio
async def test_cs_22_code_span_with_trailing_backtick(cm_workspace):
    """# Inlines 引言 example: `hi`lo` — `hi` 是 code span, 末尾 ` 是 literal。
    CM spec: '<p><code>hi</code>lo`</p>'
    该 example 位于 # Inlines (H1) 分区, 不属于任何 H2 章节, cm_example 不可提取, 用内联输入。"""
    ast = ast_parse("`hi`lo`")
    assert ast_has_type(ast, "cd"), "`hi` 应解析为 inline code span"
    code = _find_code(ast)
    assert code == "hi", f"code 内容应为 hi: {code!r}"
    assert ast_has_type(ast, "p"), "整体应是 paragraph"
    txt = "".join(n.get("ct","") for n in ast_find_all(ast, "txt"))
    assert "lo`" in txt, f"lo` 应是文本: {txt!r}"
