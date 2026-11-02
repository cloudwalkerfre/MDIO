"""§6.3 Links — 90 examples (CM spec §6.3).

CM spec rules (read §6.3 in full):
  - Inline link: [text](url "title") — url may be <>-wrapped, title in "" '' ()
  - Reference link: [text][label], [text][], [text] (shortcut)
  - Link text may contain inline formatting (em, code, image)
  - Destination: no spaces unless <>-wrapped, balanced parens allowed
  - Label matching: case-insensitive Unicode fold
  - Backslash escapes active in link destination and title
  - Blank line terminates refdef; refdef cannot interrupt paragraph

Selector: cm_example(rt, "Inlines", "Links", N)
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_has_type

H1, H2 = "Inlines", "Links"


def _find(ast, t):
    def _s(node):
        if isinstance(node, dict):
            if node.get("nt") == t: return node
            for v in node.values():
                if isinstance(v, (dict, list)):
                    r = _s(v)
                    if r: return r
        elif isinstance(node, list):
            for c in node:
                r = _s(c)
                if r: return r
        return None
    return _s(ast)


def _link_url(link):
    return link.get("h","")


def _link_title(link):
    return link.get("ti","")


# §1 Inline links — basic forms (ex 0-5)

@pytest.mark.asyncio
async def test_links_00_inline_with_title(cm_workspace):
    """N=0: [link](/uri "title") — basic inline link with title."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "/uri"
    assert _link_title(link) == "title"

@pytest.mark.asyncio
async def test_links_01_inline_no_title(cm_workspace):
    """N=1: [link](/uri) — inline link without title."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "/uri"

@pytest.mark.asyncio
async def test_links_02_empty_text(cm_workspace):
    """N=2: [](./target.md) — empty link text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_03_empty_dest(cm_workspace):
    """N=3: [link]() — empty destination."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None

@pytest.mark.asyncio
async def test_links_04_empty_angle_dest(cm_workspace):
    """N=4: [link](<>) — empty angle-bracketed destination."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_05_empty_text_and_dest(cm_workspace):
    """N=5: []() — both empty."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")


# §2 Destination restrictions (ex 6-12)

@pytest.mark.asyncio
async def test_links_06_space_no_angle_not_link(cm_workspace):
    """N=6: [link](/my uri) — space without <> → NOT a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_07_space_in_angle_url(cm_workspace):
    """N=7: [link](</my uri>) — space allowed in angle-bracketed URL.
    CM spec: href='/my uri'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "/my uri", f"href: {_link_url(link)!r}"

@pytest.mark.asyncio
async def test_links_08_newline_no_angle_not_link(cm_workspace):
    """N=8: [link](foo\nbar) — newline → NOT a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_09_newline_in_angle_not_link(cm_workspace):
    """N=9: [link](<foo\nbar>) — newline even in <> → NOT a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_10_paren_in_angle_url(cm_workspace):
    """N=10: [a](<b)c>) — ) allowed inside angle-bracketed URL.
    CM spec: href='b)c'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "b)c", f"href: {_link_url(link)!r}"

@pytest.mark.asyncio
async def test_links_11_escaped_angle_not_link(cm_workspace):
    r"""N=11: [link](<foo\>) — escaped > → NOT a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_12_unmatched_angle_not_link(cm_workspace):
    """N=12: [a](<b)c / [a](<b)c> / [a](<b>c) — unmatched <> → NOT links.
    MDIO: last line [a](<b>c) incorrectly creates a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk"), "unmatched angle brackets should not create links"


# §3 Parentheses in destination (ex 13-18)

@pytest.mark.asyncio
async def test_links_13_escaped_paren(cm_workspace):
    """N=13: [link](\\(foo\\)) — escaped parens in URL.
    CM spec: href='(foo)'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "(foo)", f"href: {_link_url(link)!r}"

@pytest.mark.asyncio
async def test_links_14_balanced_paren(cm_workspace):
    """N=14: [link](foo(and(bar))) — balanced parens in URL.
    CM spec: href='foo(and(bar))'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "foo(and(bar))", f"href: {_link_url(link)!r}"

@pytest.mark.asyncio
async def test_links_15_unbalanced_paren_not_link(cm_workspace):
    """N=15: [link](foo(and(bar)) — unbalanced paren → NOT a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_16_escaped_paren_complex(cm_workspace):
    """N=16: [link](foo\\(and\\(bar\\)) — \\-escaped parens should be link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_17_paren_in_angle(cm_workspace):
    """N=17: [link](<foo(and(bar)>) — paren in angle URL → link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_18_escaped_paren_and_colon(cm_workspace):
    """N=18: [link](foo\\)\\:) — escaped ) and :."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 18)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")


# §4 Fragment/encoding/title variants (ex 19-29)

@pytest.mark.asyncio
async def test_links_19_fragment_url(cm_workspace):
    """N=19: three links with fragments #..."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 19)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_20_backslash_in_url(cm_workspace):
    """N=20: [link](foo\\bar) — backslash in URL → percent-encoded."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 20)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_21_entity_in_url(cm_workspace):
    """N=21: [link](foo%20b&auml;) — entity in URL → decoded then percent-encoded."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 21)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_22_title_as_dest(cm_workspace):
    """N=22: [link]("title") — quotes become URL (no title, just destination)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 22)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_23_title_variants(cm_workspace):
    """N=23: three links with "" '' () title delimiters."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 23)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_24_escaped_quote_in_title(cm_workspace):
    """N=24: title with escaped quote."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 24)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_25_nbsp_not_title(cm_workspace):
    """N=25: NBSP before quote → not a title separator."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 25)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_26_title_internal_quote_cut(cm_workspace):
    """N=26: title with internal unescaped quotes → NOT a valid link.
    MDIO: creates a link with truncated title (deviation)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 26)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_27_title_with_quotes(cm_workspace):
    """N=27: single-quote title containing double quotes."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 27)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_28_multiline_inline_link(cm_workspace):
    """N=28: [link](   /uri\n  "title"  ) — multiline inline link.
    CM spec: <p><a href="/uri" title="title">link</a></p>"""
    rt = MdioRuntime(cm_workspace)
    md, expected = await cm_example(rt, H1, H2, 28)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None, "multiline inline link should parse"
    assert _link_url(link) == "/uri", f"href: {_link_url(link)!r}"
    assert _link_title(link) == "title", f"title: {_link_title(link)!r}"

@pytest.mark.asyncio
async def test_links_29_space_after_closing_not_link(cm_workspace):
    """N=29: [link] (/uri) — space after ] → NOT a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 29)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")


# §5 Link text with nested brackets/inline (ex 30-44)

@pytest.mark.asyncio
async def test_links_30_nested_brackets_text(cm_workspace):
    """N=30: [link [foo [bar]]](/uri) — nested brackets in link text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 30)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_31_unbalanced_brackets_not_link(cm_workspace):
    """N=31: [link] bar](/uri) — unbalanced → NOT a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 31)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_32_inner_link_takes_priority(cm_workspace):
    """N=32: [link [bar](/uri) — inner [bar] becomes link, outer is text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 32)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None

@pytest.mark.asyncio
async def test_links_33_escaped_bracket_in_text(cm_workspace):
    """N=33: [link \\[bar](/uri) — escaped [ in text, outer becomes link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 33)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_34_inline_formatting_in_text(cm_workspace):
    """N=34: link with *foo **bar** `#`* in text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 34)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert ast_has_type(ast, "em") and ast_has_type(ast, "st") and ast_has_type(ast, "cd")

@pytest.mark.asyncio
async def test_links_35_image_in_link_text(cm_workspace):
    """N=35: [![moon](moon.jpg)](/uri) — image inside link text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 35)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")
    assert ast_has_type(ast,"im")

@pytest.mark.asyncio
async def test_links_36_nested_links_text_not_link(cm_workspace):
    """N=36: [foo [bar](/uri)](/uri) — inner link only, outer becomes text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 36)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_37_triple_nested_innermost_wins(cm_workspace):
    """N=37: [foo *[bar [baz](/uri)](/uri)*](/uri) — innermost link wins."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 37)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_38_nested_image_not_link(cm_workspace):
    """N=38: ![[[foo](uri1)](uri2)](uri3) — image whose alt contains a link.
    CM spec: <img src="uri3" alt="[foo](uri2)" /> — alt 渲染树含 link(foo→uri1)，
    属性纯文本化后为 '[foo](uri2)'（参考实现 commonmark 验证）。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 38)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")
    assert ast_has_type(ast,"lk"), "image alt 内应有 link（foo→uri1）"

@pytest.mark.asyncio
async def test_links_39_em_bracket_binding(cm_workspace):
    """N=39: *[foo*](/uri) — emphasis markers don't cross bracket boundaries."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 39)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_40_brackets_take_priority(cm_workspace):
    """N=40: [foo *bar](baz*) — brackets take priority over emphasis."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 40)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_41_no_link_from_emphasis(cm_workspace):
    """N=41: *foo [bar* baz] — emphasis wins, no link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 41)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_42_html_tag_blocks_link(cm_workspace):
    """N=42: [foo <bar attr="](baz)"> — HTML tag inside, not a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 42)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_43_code_span_blocks_link(cm_workspace):
    """N=43: [foo`](/uri)` — code span takes priority, no link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 43)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")
    assert ast_has_type(ast,"cd")

@pytest.mark.asyncio
async def test_links_44_autolink_not_inline_link(cm_workspace):
    """N=44: [foo<https://...?search=](uri)> — autolink binds tighter, no inline link.
    Expected HTML has <a> from autolink, not from inline [text](url) syntax."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 44)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al")
    assert not ast_has_type(ast,"lk"), "autolink, not inline link"


# §6 Reference links — full/collapsed/shortcut (ex 45-57)

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_45_full_ref(cm_workspace):
    """N=45: [foo][bar] + [bar]: /url "title" — full reference link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 45)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "/url"
    assert _link_title(link) == "title"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_46_nested_brackets_ref(cm_workspace):
    """N=46: [link [foo [bar]]][ref] + refdef."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 46)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_47_escaped_bracket_ref(cm_workspace):
    """N=47: [link \\[bar][ref] + refdef."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 47)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_48_inline_formatting_ref(cm_workspace):
    """N=48: [link *foo **bar** `#`*][ref] — inline formatting in ref text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 48)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_49_image_in_ref(cm_workspace):
    """N=49: [![moon](moon.jpg)][ref] — image in ref link text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 49)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_50_inner_link_takes_priority_ref(cm_workspace):
    """N=50: [foo [bar](/uri)][ref] + [ref]: /uri — inner link wins."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 50)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_51_nested_em_and_link_ref(cm_workspace):
    """N=51: [foo *bar [baz][ref]*][ref] — emphasis inside ref."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 51)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_52_em_binding_ref(cm_workspace):
    """N=52: *[foo*][ref] — brackets bind tighter than emphasis."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 52)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_53_ref_then_star(cm_workspace):
    """N=53: [foo *bar][ref]* — link then trailing star outside."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 53)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_54_html_in_text_not_link(cm_workspace):
    """N=54: [foo <bar attr="][ref]"> + refdef — HTML in text, not link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 54)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_55_code_span_in_text(cm_workspace):
    """N=55: [foo`][ref]` — code span takes priority, no link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 55)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_56_autolink_not_ref_link(cm_workspace):
    """N=56: [foo<https://...>][ref]> — autolink binds tighter, no ref link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 56)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al")
    assert not ast_has_type(ast,"lk"), "autolink, not ref link"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_57_collapsed_ref(cm_workspace):
    """N=57: [foo][] + [foo]: /url — collapsed reference."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 57)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")


# §7 Unicode and case folding (ex 58-59)

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_58_unicode_case_fold(cm_workspace):
    """N=58: [ẞ] + [SS]: /url — Unicode case folding (ẞ → ss → SS)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 58)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk"), "ẞ should match SS via Unicode case fold"

@pytest.mark.xfail(
    reason="MDIO 偏差: 多行 refdef 不识别。"
           "`[Foo\\n  bar]: /url` — 多行 label 不解析。同 RD #7a。"
)
@pytest.mark.asyncio
async def test_links_59_multiline_refdef(cm_workspace):
    """N=59: [Foo\n  bar]: /url + [Baz][Foo bar] — multiline refdef label."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 59)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk"), "multiline refdef should be recognized"


# §8 Shortcut links / edge cases (ex 60-89)

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_60_shortcut_ref(cm_workspace):
    """N=60: [foo] + [foo]: /url — shortcut reference link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 60)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_61_shortcut_with_space(cm_workspace):
    """N=61: [foo] + [foo]: /url — space after text is OK."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 61)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_62_trailing_text_in_brackets(cm_workspace):
    """N=62: [foo] bar — text after bracket, link still works."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 62)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_63_code_span_not_ref_link(cm_workspace):
    """N=63: `[foo]` — code span, not shortcut reference."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 63)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_64_bracket_in_ref_label_not_link(cm_workspace):
    """N=64: [foo][ref[] + [ref[]: /uri — NOT a link, [ in label."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 64)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_65_bracket_in_label_not_ref(cm_workspace):
    """N=65: [foo][ref[bar]] — `[` in ref label → NOT a valid ref link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 65)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_66_empty_shortcut_not_link(cm_workspace):
    """N=66: [] + []: /url — empty shortcut, NOT a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 66)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_67_shortcut_case_insensitive(cm_workspace):
    """N=67: [Foo] + [foo]: /url — case-insensitive match."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 67)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_68_spaces_normalized(cm_workspace):
    """N=68: [foo] + [  foo  ]: /url — label whitespace normalized."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 68)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.asyncio
async def test_links_69_empty_text_shortcut_not_link(cm_workspace):
    """N=69: [] + []: /uri — empty text, NOT a link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 69)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk"), "[] with empty text should not be a link"

@pytest.mark.asyncio
async def test_links_70_internal_newline_not_shortcut(cm_workspace):
    """N=70: [foo\nbar] — newline in text, not shortcut ref."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 70)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_71_collapsed_ref(cm_workspace):
    """N=71: [foo][] + [foo]: /url "title" — collapsed reference link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 71)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_72_shortcut_over_collapsed(cm_workspace):
    """N=72: [foo][] + [foo]: /url — both shortcut and collapsed match."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 72)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_73_collapsed_empty_label(cm_workspace):
    """N=73: [foo][] — collapsed ref to [foo]."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 73)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_74_full_explicit_label(cm_workspace):
    """N=74: [foo][FoO] — case-insensitive label."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 74)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_75_first_def_wins(cm_workspace):
    """N=75: [foo] with two refdefs — first one wins."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 75)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_76_label_match(cm_workspace):
    """N=76: [Foo] + [foo]: /url1 / [FOO]: /url2 — case-insensitive, first wins."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 76)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_77_space_in_label(cm_workspace):
    """N=77: [foo] [bar] — space between, not shortcut."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 77)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_78_separate_shortcuts(cm_workspace):
    """N=78: [foo]/[bar] — two separate shortcut refs."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 78)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_79_second_link_not_resolved(cm_workspace):
    """N=79: [foo][bar] + [foo]: /url1 — [bar] not defined, only foo gets link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 79)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_80_shortcut_text_after(cm_workspace):
    """N=80: [foo] bar + [foo]: /url — shortcut ref with text after."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 80)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "/url"

@pytest.mark.asyncio
async def test_links_81_escaped_bracket_not_link(cm_workspace):
    """N=81: \\[foo] + [foo]: /url — escaped bracket prevents link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 81)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_82_refdef_with_star_label(cm_workspace):
    """N=82: [foo*]: /url + *[foo*] — refdef label can contain *, link works."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 82)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_83_two_refdefs_one_used(cm_workspace):
    """N=83: [foo][bar] + [foo]: /url1 + [bar]: /url2 — explicit label uses [bar]."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 83)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "/url2"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_84_collapsed_with_def(cm_workspace):
    """N=84: [foo][] + [foo]: /url1 — collapsed ref to [foo]."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 84)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == "/url1"

@pytest.mark.asyncio
async def test_links_85_inline_empty_dest_over_refdef(cm_workspace):
    """N=85: [foo]() + [foo]: /url1 — inline () takes priority over refdef."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 85)
    ast = ast_parse(md)
    link = _find(ast, "lk")
    assert link is not None
    assert _link_url(link) == ""  # empty destination from inline ()

# §9 Edge cases — refdef vs inline, consecutive refs (ex 86-89)

@pytest.mark.xfail(
    reason="MDIO 偏差: 非法 inline link 不回退到 refdef。"
           "`[foo](not a link)` — 空格在 dest 中 → inline link 无效，"
           "应 fallback 到 `[foo]: /url1` refdef + 字面 `(not a link)`。"
           "inline.rs 未实现 inline→ref fallback。"
)
@pytest.mark.asyncio
async def test_links_86_invalid_inline_fallback_to_ref(cm_workspace):
    """N=86: [foo](not a link) + [foo]: /url1 — invalid inline → refdef fallback."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 86)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk"), "should fall back to refdef when inline dest invalid"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_87_consecutive_ref_links(cm_workspace):
    """N=87: [foo][bar][baz] + [baz]: /url — consecutive ref links."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 87)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk"), "should parse [baz] as link"

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_88_two_refs_from_consecutive(cm_workspace):
    """N=88: [foo][bar][baz] + [baz]: /url1 + [bar]: /url2 — consecutive ref links."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 88)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")

@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_links_89_consecutive_refs_multiple(cm_workspace):
    """N=89: [foo][bar][baz] + [baz]: /url1 + [foo]: /url2 — consecutive ref with defs."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 89)
    ast = ast_parse(md)
    assert ast_has_type(ast,"lk")
