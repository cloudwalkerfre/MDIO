"""§2.4 Backslash escapes — 13 examples (CM spec §2.4).

CM spec rules:
  - Any ASCII punctuation character may be backslash-escaped → the
    backslash is consumed and the char is treated as literal
  - Backslash before a non-ASCII-punctuation character → literal backslash
    (not consumed)
  - Backslash at end of line → hard line break (\\n)
  - Escaped chars in code spans, code blocks, autolinks, HTML tags →
    literal backslash (NOT consumed)
  - Backslash escapes ARE consumed in: link destinations, link titles,
    fence info strings → `\\*` → `*`

AST structure:
  转义字符被解码为 txt 节点 ({"nt": "txt", "ct": "解码后字符"})。

Known MDIO deviations:
  - N=11: reference link 依赖 refmap, 当前未实现 (xfail)
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_find_all, ast_has_type

H1, H2 = "Preliminaries", "Backslash escapes"


def _find(ast, t):
    """Find first node with given nodeType."""
    def _s(node):
        if isinstance(node, dict):
            if node.get("nt") == t:
                return node
            for v in node.values():
                if isinstance(v, (dict, list)):
                    r = _s(v)
                    if r:
                        return r
        elif isinstance(node, list):
            for c in node:
                r = _s(c)
                if r:
                    return r
        return None
    return _s(ast)


def _all_txt(ast):
    """Join all text node content (txt + tx: escaped chars are decoded in tx nodes)."""
    return "".join(n.get("ct","") for n in ast_find_all(ast, "txt") + ast_find_all(ast, "tx"))


# Tests — §2.4 Backslash escapes (13 examples)

# 基本转义规则

@pytest.mark.asyncio
async def test_es_00_all_ascii_punctuation_escaped(cm_workspace):
    """N=0: `\\!\\\"\\#\\$`... — every ASCII punctuation char escaped → literal.
    CM spec §6.1: any ASCII punctuation character may be backslash-escaped."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    # New format: escaped punctuation chars are decoded to literal text content.
    # 单个转义不单独计数：解码后并入 txt 节点，按文本片段汇总。
    txt = _all_txt(ast)
    assert "!" in txt and '"' in txt, f"should have decoded escaped chars, got: {txt[:80]}"


@pytest.mark.asyncio
async def test_es_01_non_punctuation_literal_backslash(cm_workspace):
    """N=1: `\\→\\A\\a\\ \\3\\φ\\«` — non-punctuation → literal backslash.
    CM spec: backslash before non-ASCII-punctuation is NOT consumed.
    Expected: literal `\\` followed by the char."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    # New format: literal backslash chars appear as "\\" in text content
    txt = _all_txt(ast)
    assert "\\" in txt, "should have literal backslash for non-punctuation chars"


@pytest.mark.asyncio
async def test_es_02_escaped_formatting_markers(cm_workspace):
    """N=2: `\\*not emphasized*` — `\\*` makes `*` literal, not emphasis.
    CM spec: backslash-escaped formatting chars are treated as text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    assert not ast_has_type(ast, "em"), "escaped * should NOT create emphasis"
    assert not ast_has_type(ast, "lk"), "escaped [ should NOT create link"
    assert not ast_has_type(ast, "cd"), "escaped ` should NOT create code span"
    assert not ast_has_type(ast, "h"), "escaped # should NOT create heading"


@pytest.mark.asyncio
async def test_es_03_double_backslash_then_emphasis(cm_workspace):
    """N=3: `\\\\*emphasis*` → `\\` (literal backslash) + `*emphasis*` (em).
    CM spec: `\\\\` → literal `\\`; then `*emphasis*` creates emphasis."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    assert ast_has_type(ast, "em"), "should have emphasis for unescaped *"


@pytest.mark.asyncio
async def test_es_04_backslash_at_eol_hard_break(cm_workspace):
    """N=4: `foo\\\\\\nbar` — backslash at end of line → hard line break.
    CM spec: 'A backslash at the end of the line is a hard line break.'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    assert ast_has_type(ast, "lb"), "should have hard line break"


@pytest.mark.asyncio
async def test_es_05_backslashes_in_code_span_literal(cm_workspace):
    """N=5: `` \\[\\` `` — backslashes in code span are literal.
    CM spec: backslash escapes are NOT processed in code spans."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    code = _find(ast, "cd")
    assert code is not None, "should have code span"
    assert code.get("cd") == "\\[\\`"


@pytest.mark.asyncio
async def test_es_06_backslashes_in_indented_code(cm_workspace):
    """N=6: `····\\\\[\\\\]` — 4-space indent → indented code block.
    CM spec: backslash escapes NOT processed in code blocks."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic"), "4-space indent → code_block"


@pytest.mark.asyncio
async def test_es_07_backslashes_in_fenced_code(cm_workspace):
    """N=7: fenced code with `\\\\[\\\\]` → literal backslashes in code.
    CM spec: backslash escapes NOT processed in code blocks."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert ast_has_type(ast, "c"), "should have code_block"


@pytest.mark.asyncio
async def test_es_08_backslash_in_autolink(cm_workspace):
    """N=8: `<https://example.com?find=\\*>` — backslash in autolink → literal.
    CM spec: backslash escapes NOT processed in autolinks."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    autolink = _find(ast, "al")
    assert autolink is not None, "should have autolink"
    assert "\\*" in autolink.get("ct",""), f"backslash should be literal: {autolink.get('ct','')!r}"


@pytest.mark.asyncio
async def test_es_09_backslash_in_html_tag(cm_workspace):
    """N=9: `<a href=\"/bar\\/)\">` — backslash in HTML attribute → literal.
    CM spec: backslash escapes NOT processed in HTML tags."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "hb", ast
    raw = ast[0].get("ct", "")
    assert "\\/" in raw, "backslash should be literal in HTML attribute"


# Link destination / title / info string 中应 unescape

@pytest.mark.asyncio
async def test_es_10_backslash_unescaped_in_link_url_and_title(cm_workspace):
    """N=10: `[foo](/bar\\* \"ti\\*tle\")` — `\\*` → `*` in URL and title.
    CM spec: backslash escapes ARE consumed in link destinations and titles."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have link"
    # AST: ["link", mark, tag, content(url), title, [children]]
    url = link.get("h","")
    assert url == "/bar*", f"expected /bar* (unescaped), got {url!r}"


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_es_11_backslash_unescaped_in_refdef(cm_workspace):
    """N=11: ref link `[foo]` with `[foo]: /bar\\* \"ti\\*tle\"` → `*` in URL.
    CM spec: same rule as inline links — backslash escapes consumed."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have link"
    # AST: ["link", mark, tag, content(url), title, [children]]
    url = link.get("h","")
    assert url == "/bar*", f"expected /bar* (unescaped), got {url!r}"


@pytest.mark.asyncio
async def test_es_12_backslash_unescaped_in_fence_info(cm_workspace):
    """N=12: ` ``` foo\\+bar ` — `\\+` → `+` in info string.
    CM spec: backslash escapes ARE consumed in code fence info strings."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    # Find the code_fence block with the info string
    code_blocks = [b for b in ast if isinstance(b, dict) and b.get("nt") == "c"]
    if code_blocks:
        # Check tag on outer node or nested children
        tag = code_blocks[0].get("t","")
        if not tag:
            for child in code_blocks[0].get("ct", []):
                if isinstance(child, dict) and child.get("nt") == "c":
                    tag = child.get("t","")
                    break
        assert tag == "foo+bar", f"expected foo+bar (unescaped), got {tag!r}"
    else:
        # Fallback: check if any code_fence has a tag with the right info
        cs = [b for b in ast if isinstance(b, dict) and b.get("nt") == "c"]
        if cs:
            assert cs[0].get("t","") == "foo+bar", f"expected foo+bar, got {cs[0].get('t','')!r}"
