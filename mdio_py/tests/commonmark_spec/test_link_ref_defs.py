"""§4.7 Link reference definitions — 27 examples (CM spec §4.7).

CM spec rules:
  - refdef = [label]: destination [title], preceded by ≤3 spaces indent
  - label must be on one line, matched case-insensitive (Unicode fold)
  - destination: plain (no < >, no spaces) or <angle-bracketed>
  - title: "..." | '...' | (...), optional, must be separated from destination by whitespace
  - destination/title each allow ≤1 line ending internally
  - blank line terminates the refdef
  - no non-whitespace characters after refdef
  - refdef can come before OR after usage
  - refdef does NOT interrupt a paragraph
  - multiple matching definitions → first wins

AST structure:
  link node: {"nt": "lk", "ct": [...], "h": url, "ti": title}
  refdef node: {"nt": "rd", "u": url, "ti": title}
"""

import pytest, json
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_parse_nested, ast_to_html, ast_has_type

H1, H2 = "Leaf blocks", "Link reference definitions"


# helpers

def _find(ast, t):
    """Find first node with given nodeType, return it."""
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

def _links(ast):
    """Return all link nodes."""
    r = []
    def _w(n):
        if isinstance(n, dict) and n.get("nt") == "lk":
            r.append(n)
        if isinstance(n, dict):
            for v in n.values():
                if isinstance(v, (dict, list)):
                    _w(v)
        elif isinstance(n, list):
            for c in n:
                _w(c)
    _w(ast)
    return r

def _link_url(link):
    """Extract href from link dict."""
    return link.get("h","")

def _link_title(link):
    """Extract title from link dict."""
    return link.get("ti","")

def _link_children(link):
    """Extract child nodes from link dict."""
    return link.get("ct", [])

def _link_texts(link):
    """All text content from link children."""
    r = []
    def _w(n):
        if isinstance(n, dict) and n.get("nt") == "txt":
            c = n.get("ct","")
            if c: r.append(c)
        if isinstance(n, dict):
            for v in n.values():
                if isinstance(v, (dict, list)):
                    _w(v)
        elif isinstance(n, list):
            for c in n:
                _w(c)
    _w(_link_children(link))
    return r

def _txts(ast):
    """Recursively collect all text content strings."""
    r = []
    def _w(n):
        if isinstance(n, dict) and n.get("nt") == "txt":
            c = n.get("ct","")
            if c: r.append(c)
        if isinstance(n, dict):
            ct = n.get("ct")
            if ct and n.get("nt") not in ("txt","sb","hr"):
                # ct 双型:字符串直接收;数组递归元素。
                if isinstance(ct, str):
                    r.append(ct.rstrip('\n'))
                elif isinstance(ct, list):
                    for it in ct:
                        if isinstance(it, str):
                            r.append(it.rstrip('\n'))
                        elif isinstance(it, dict):
                            _w(it)
            for v in n.values():
                if isinstance(v, (dict, list)):
                    _w(v)
        elif isinstance(n, list):
            for c in n:
                _w(c)
    _w(ast)
    return r

def _block_types(ast):
    """Top-level block type list from flat AST."""
    return [b.get("nt") for b in (ast or []) if isinstance(b, dict)]


# Tests — §4.7 Link reference definitions (27 examples)

# §4.7 基本形式

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_00_basic_single_line(cm_workspace):
    """N=0: [foo]: /url "title" + [foo] → link with href/title."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have a link"
    assert _link_url(link) == "/url", f"url should be /url, got {_link_url(link)!r}"
    assert _link_title(link) == "title", f"title should be 'title', got {_link_title(link)!r}"
    assert "foo" in "".join(_link_texts(link))

@pytest.mark.xfail(
    reason="MDIO 偏差: multiline refdef 不识别。"
           "refdef_matches() 仅匹配单行 [label]: dest title 模式。"
           "URL 在下一行（包括 ≤1 line ending）的情况未处理。"
)
@pytest.mark.asyncio
async def test_rd_01_multiline_indented_url_title(cm_workspace):
    """N=1: indented [foo]: + /url on next line + 'title' on third → link.
    CM spec allows ≤1 line ending between label: and destination."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have a link"
    assert _link_url(link) == "/url"
    assert _link_title(link) == "the title"
    assert "foo" in "".join(_link_texts(link))

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented ([Foo*bar\\]] shortcut not resolved to lk)")
@pytest.mark.asyncio
async def test_rd_02_escaped_chars_in_label(cm_workspace):
    """N=2: [Foo*bar\\]]:my_(url) 'title...' → link. Label 含 * 和转义 ]."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have a link"
    assert _link_url(link) == "my_(url)", f"url: {_link_url(link)!r}"
    assert _link_title(link) == "title (with parens)", f"title: {_link_title(link)!r}"
    assert "Foo*bar]" in "".join(_link_texts(link))

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_03_angle_url_multiline(cm_workspace):
    """N=3: [Foo bar]: + <my url> + 'title' → link.
    Angle-bracketed destination may contain spaces, URL on next line."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have a link"
    assert "url" in _link_url(link), f"url should contain 'url': {_link_url(link)!r}"
    assert _link_title(link) == "title"

@pytest.mark.xfail(
    reason="MDIO 偏差: multiline title 不识别。"
           "refdef title 跨多行（'title\\nline1\\nline2\\n'）未处理。"
)
@pytest.mark.asyncio
async def test_rd_04_multiline_title_with_newlines(cm_workspace):
    """N=4: title spans multiple lines → link with multiline title."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have a link"
    assert _link_url(link) == "/url"
    title = _link_title(link)
    assert "\n" in title or "title" in title

@pytest.mark.asyncio
async def test_rd_05_blank_line_breaks_title(cm_workspace):
    """N=5: blank line in middle of title → NOT a refdef.
    Expected: 3 paragraphs, no link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk"), "blank line in title → NOT a refdef"
    p_count = len([b for b in _block_types(ast) if b == "p"])
    assert p_count == 3, f"expected 3 paragraphs, got {p_count}: {_block_types(ast)}"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_06_url_on_next_line(cm_workspace):
    """N=6: [foo]: + /url (next line) → link without title."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have a link"
    assert _link_url(link) == "/url", f"url: {_link_url(link)!r}"
    assert "foo" in "".join(_link_texts(link))

async def test_rd_07_destination_cannot_be_omitted(cm_workspace):
    """N=7: [foo]: + blank line + [foo] → no destination → NOT a refdef.
    Expected: 2 paragraphs（`[foo]:` 撤销为 p——官方
    `<p>[foo]:</p><p>[foo]</p>`——无 rd）."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert not ast_has_type(ast, "lk"), "no destination → NOT a refdef"
    blocks = _block_types(ast)
    # 空 body 的 rd 撤销为 p——官方 `<p>[foo]:</p>`
    assert "rd" not in blocks, f"空 body 不成立（撤销为 p）: {blocks}"
    assert blocks.count("p") == 2, f"should have 2 paragraphs: {blocks}"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_08_empty_angle_url(cm_workspace):
    """N=8: [foo]: <> → link to empty URL."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have a link"
    url = _link_url(link)
    assert url in ("", "<>"), f"url should be empty, got {url!r}"
    assert "foo" in "".join(_link_texts(link))

@pytest.mark.asyncio
async def test_rd_09_title_must_be_separated_by_whitespace(cm_workspace):
    """N=9: [foo]: <bar>(baz) → NOT a refdef.
    Expected: 2 paragraphs, no link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk"), (
        "no space between destination and title → NOT a refdef\n"
        f"AST: {json.dumps(ast, ensure_ascii=False)}")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_10_backslash_escapes_in_url_and_title(cm_workspace):
    """N=10: backslash escapes in URL + title → link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have a link"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_11_forward_ref_usage_before_definition(cm_workspace):
    """N=11: [foo] appears before [foo]: url → link (forward reference ok)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "forward ref should create link"
    assert _link_url(link) == "url", f"url: {_link_url(link)!r}"
    assert "foo" in "".join(_link_texts(link))

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_12_duplicate_refdef_first_wins(cm_workspace):
    """N=12: [foo]: first + [foo]: second → first definition wins."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have a link"
    assert _link_url(link) == "first", f"first should win, got {_link_url(link)!r}"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_13_case_insensitive_label(cm_workspace):
    """N=13: [FOO]: /url + [Foo] → case-insensitive match → link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "case-fold should create link"
    assert _link_url(link) == "/url"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_14_unicode_case_fold(cm_workspace):
    """N=14: [ΑΓΩ]: /φου + [αγω] → Unicode case fold → link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "Unicode fold should create link"
    assert _link_url(link) == "/φου"

# 非 refdef / 边界条件

# N=15: refdef-only example ([foo]: /url with no `.` separator in spec).
# cm_example cannot parse this — data format issue, not MDIO bug.

@pytest.mark.asyncio
async def test_rd_16_multiline_label_not_refdef(cm_workspace):
    """N=16: [\\nfoo\\n]: /url\\nbar → multiline label → NOT a refdef.
    Expected: visible text 'bar'."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk"), "multiline label → NOT a refdef"
    assert "bar" in "".join(_txts(ast))

@pytest.mark.asyncio
async def test_rd_17_text_after_title_not_refdef(cm_workspace):
    """N=17: [foo]: /url "title" ok → 'ok' after title → NOT a refdef."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk"), "text after title → NOT a refdef"
    assert "ok" in "".join(_txts(ast)), "'ok' should be visible"

@pytest.mark.asyncio
async def test_rd_18_refdef_without_title_then_text(cm_workspace):
    """N=18: [foo]: /url + "title" ok on next line → refdef (no title), then text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 18)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk"), "should not have a link"
    assert '"title" ok' in "".join(_txts(ast))

@pytest.mark.asyncio
async def test_rd_19_four_space_indent_not_refdef(cm_workspace):
    """N=19: 4-space indented [foo]: /url "title" → indented code block, NOT refdef."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 19)
    ast = ast_parse(md)
    assert ast_has_type(ast,"ic"), f"4-space indent → indented_code, got {_block_types(ast)}"
    assert not ast_has_type(ast,"lk"), "should NOT resolve link"

@pytest.mark.asyncio
async def test_rd_20_refdef_in_fenced_code(cm_workspace):
    """N=20: [foo]: /url inside ``` fenced code → NOT a refdef.
    Expected: code_block + paragraph [foo]."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 20)
    ast = ast_parse(md)
    assert ast_has_type(ast,"c"), "should have code_fence"
    assert not ast_has_type(ast,"lk"), "refdef in fenced code should NOT create link"

@pytest.mark.asyncio
async def test_rd_21_refdef_cannot_interrupt_paragraph(cm_workspace):
    """N=21: Foo + [bar]: /baz + [bar] → [bar]:/baz is paragraph text.
    Expected: no link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 21)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk"), (
        "refdef should NOT interrupt paragraph\n"
        f"AST: {json.dumps(ast, ensure_ascii=False)}")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_22_refdef_after_heading(cm_workspace):
    """N=22: # [Foo] + [foo]: /url + > bar → link in heading + refdef + BQ."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 22)
    ast = ast_parse(md)
    assert ast_has_type(ast,"head"), "should have heading"
    assert ast_has_type(ast,"bq"), "should have blockquote"
    link = _find(ast, "lk")
    assert link is not None, "heading should contain link"
    assert _link_url(link) == "/url"
    assert "Foo" in "".join(_link_texts(link))

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_23_refdef_then_setext_heading(cm_workspace):
    """N=23: [foo]: /url + bar + === + [foo] → refdef, then setext heading, then link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 23)
    ast = ast_parse(md)
    assert ast_has_type(ast,"head"), "should have heading"
    link = _find(ast, "lk")
    assert link is not None, "should have link"
    assert _link_url(link) == "/url"
    # heading text: flat AST may have string content at [3] or inline children at [3]
    head = _find(ast, "head")
    assert head is not None
    head_content = head[3] if len(head) > 3 else ""
    if isinstance(head_content, list):
        assert "bar" in "".join(_txts(head))
    else:
        assert "bar" in str(head_content)

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_24_refdef_equals_sign_in_paragraph(cm_workspace):
    """N=24: [foo]: /url + === + [foo] → refdef + paragraph with === + link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 24)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "should have link"
    assert _link_url(link) == "/url"
    # === should be in the same paragraph as the link
    p = _find(ast, "p")
    assert p is not None, "should have paragraph"
    all_texts = "".join(_txts(p))
    assert "===" in all_texts, f"'===' should be paragraph text, got texts: {all_texts!r}"
    assert "foo" in all_texts, f"link text 'foo' should be in paragraph: {all_texts!r}"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_25_multiple_refdefs_and_links(cm_workspace):
    """N=25: three consecutive refdefs + three links.
    CM expected: foo(/foo-url "foo"), bar(/bar-url "bar"), baz(/baz-url)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 25)
    ast = ast_parse(md)
    links = _links(ast)
    assert len(links) >= 3, f"expected ≥3 links, got {len(links)}"

    # Collect urls and texts
    link_info = []
    for l in links:
        url = _link_url(l)
        title = _link_title(l)
        text = "".join(_link_texts(l))
        link_info.append((text, url, title))

    # foo link
    foo = [li for li in link_info if li[0] == "foo"]
    assert foo, f"should have link for 'foo', got {link_info}"
    assert foo[0][1] == "/foo-url", f"foo url: {foo[0][1]!r}"
    assert foo[0][2] == "foo", f"foo title: {foo[0][2]!r}"

    # baz link (no title)
    baz = [li for li in link_info if li[0] == "baz"]
    assert baz, f"should have link for 'baz', got {link_info}"
    assert baz[0][1] == "/baz-url", f"baz url: {baz[0][1]!r}"
    assert baz[0][2] == "", f"baz should have no title: {baz[0][2]!r}"


# 补齐用例 — N=15 独立 refdef, N=26 BQ 内 refdef

@pytest.mark.asyncio
async def test_rd_15_standalone_refdef_no_output(cm_workspace):
    """N=15: [foo]: /url → 独立 refdef 不产生任何输出 block。
    (该 example 期望输出为空, `.` 后无换行直接关 fence — conftest 已兼容此格式)"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    # CM spec: 空输出 (refdef 本身不可见, 无 link 引用)
    assert ast_has_type(ast, "rd"), "refdef 节点应存在(数据层)"
    assert not ast_has_type(ast, "lk"), "无 link 引用 → 不应有 link"


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_rd_26_refdef_in_blockquote_applies(cm_workspace):
    """N=26: [foo] + BQ 内 [foo]: /url → 文档级 refmap, BQ 不隔离。
    CM spec: '<p><a href="/url">foo</a></p>\\n<blockquote>\\n</blockquote>'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 26)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "BQ 内的 refdef 应作用于文档级 [foo]"
    assert _link_url(link) == "/url"
