"""§Images / Autolinks / Hard breaks / Soft breaks — CM spec inline sections.

CM spec sections covered:
  - Images (§6.4, 22 ex): ![alt](url), ![alt][ref], !\\[escaped]
  - Autolinks (§6.5, 19 ex): <url>, <email> — absolute URI or email in <>
  - Hard line breaks (§6.7, 15 ex): 2+ trailing spaces or backslash at EOL
  - Soft line breaks (§6.8, 2 ex): newline without trailing spaces

Selector: cm_example(rt, H1, H2, N) with .tag.re("example") filter.

Assertion strategy: from expected HTML, determine if example should have
image/autolink/link/lb/p etc. Check MDIO AST matches.
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_has_type, ast_find_all


# Images (§6.4, 22 ex)

@pytest.mark.asyncio
async def test_images_0_basic_inline(cm_workspace):
    """N=0: ![foo](/url "title") — basic inline image.
    CM spec: <img src="/url" alt="foo" title="title" />"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 0)
    ast = ast_parse(md)
    imgs = ast_find_all(ast, "im")
    assert len(imgs) == 1, f"应有 1 个 image: {len(imgs)}"
    img = imgs[0]
    assert img.get("s") == "/url", f"src: {img.get('s')!r}"
    assert img.get("alt") == "foo", f"alt: {img.get('alt')!r}"
    assert img.get("ti") == "title", f"title: {img.get('ti')!r}"


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_1_ref_with_emphasis(cm_workspace):
    """N=1: ![foo *bar*] with refdef — ref image with emphasis in alt text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 1)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")
    assert ast_has_type(ast, "em"), "emphasis in alt text"


@pytest.mark.asyncio
async def test_images_2_nested_image(cm_workspace):
    """N=2: ![foo ![bar](/url)](/url2) — nested image syntax is invalid.
    MDIO only creates one image, outer syntax broken."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 2)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.asyncio
async def test_images_3_image_with_link(cm_workspace):
    """N=3: ![foo [bar](/url)](/url2) — image with link in alt text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 3)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")
    assert ast_has_type(ast,"lk"), "link in alt text"


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_4_full_ref(cm_workspace):
    """N=4: ![foo *bar*][] — full reference image."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 4)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_5_collapsed_ref(cm_workspace):
    """N=5: ![foo *bar*][foobar] — collapsed ref with different label."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 5)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.asyncio
async def test_images_6_direct_url(cm_workspace):
    """N=6: ![foo](train.jpg) — direct URL image."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 6)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.asyncio
async def test_images_7_with_text_and_whitespace(cm_workspace):
    """N=7: My ![foo bar](/path...) — image with preceding text and whitespace in URL."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 7)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.asyncio
async def test_images_8_angle_bracket_url(cm_workspace):
    """N=8: ![foo](<url>) — angle-bracketed destination."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 8)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.asyncio
async def test_images_9_empty_alt(cm_workspace):
    """N=9: ![](/url) — empty alt text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 9)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_10_ref_label(cm_workspace):
    """N=10: ![foo][bar] — reference with explicit label."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 10)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_11_case_insensitive_ref(cm_workspace):
    """N=11: ![foo][bar] / [BAR]: /url — case-insensitive ref."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 11)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_12_shortcut_ref(cm_workspace):
    """N=12: ![foo][] — shortcut reference."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 12)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_13_emphasis_ref(cm_workspace):
    """N=13: ![*foo* bar][] — image ref with emphasis in alt."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 13)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_14_case_insensitive_alt(cm_workspace):
    """N=14: ![Foo][] / [foo]: — case-insensitive label matching."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 14)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_15_brackets_after(cm_workspace):
    """N=15: ![foo] \\n[] — brackets after image, not a second image."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 15)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_16_full_reference(cm_workspace):
    """N=16: ![foo] with refdef — full reference image."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 16)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_17_emphasis_full_ref(cm_workspace):
    """N=17: ![*foo* bar] with refdef — emphasis in alt text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 17)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.asyncio
async def test_images_18_double_bracket_not_image(cm_workspace):
    """N=18: ![[foo]] — double bracket, NOT an image.
    Expected HTML: <p>![[foo]]</p>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 18)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"im"), "double bracket should not be an image"


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference image resolution not implemented")
@pytest.mark.asyncio
async def test_images_19_case_insensitive_full(cm_workspace):
    """N=19: ![Foo] / [foo]: — case-insensitive full ref."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 19)
    ast = ast_parse(md)
    assert ast_has_type(ast,"im")


@pytest.mark.asyncio
async def test_images_20_escaped_bracket_not_image(cm_workspace):
    """N=20: !\\[foo] — escaped `[`, NOT an image.
    Expected HTML: <p>![foo]</p>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 20)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"im"), "escaped bracket should not be an image"


@pytest.mark.xfail(reason="MDIO: refmap deferred — shortcut [foo] not resolved as link")
@pytest.mark.asyncio
async def test_images_21_escaped_bang_not_image(cm_workspace):
    """N=21: \\![foo] — escaped `!`, NOT an image, but link IS active.
    Expected HTML: <a href="/url">foo</a> (link, no image)"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Images", 21)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"im"), "escaped ! should not be an image"
    assert ast_has_type(ast,"lk"), "unescaped [foo] should be a link"


# Autolinks (§6.5, 19 ex)

@pytest.mark.asyncio
async def test_autolinks_0_http(cm_workspace):
    """N=0: <http://foo.bar.baz> — basic http autolink."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 0)
    ast = ast_parse(md)
    links = ast_find_all(ast, "al")
    assert len(links) == 1, f"应有 1 个 autolink: {len(links)}"
    assert links[0].get("h") == "http://foo.bar.baz", f"href: {links[0].get('h')!r}"


@pytest.mark.asyncio
async def test_autolinks_1_https_with_query(cm_workspace):
    """N=1: <https://foo.bar.baz/test?q=hello&id=22> — https with query."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 1)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al")


@pytest.mark.asyncio
async def test_autolinks_2_irc_scheme(cm_workspace):
    """N=2: <irc://foo.bar:2233/baz> — irc scheme."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 2)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al")


@pytest.mark.asyncio
async def test_autolinks_3_mailto(cm_workspace):
    """N=3: <MAILTO:FOO@BAR.BAZ> — mailto scheme.
    CM spec: href='mailto:FOO@BAR.BAZ'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 3)
    ast = ast_parse(md)
    links = ast_find_all(ast, "al")
    assert len(links) == 1, f"应有 1 个 autolink: {len(links)}"
    assert links[0].get("h") == "mailto:FOO@BAR.BAZ", f"href: {links[0].get('h')!r}"


@pytest.mark.asyncio
async def test_autolinks_4_plus_scheme(cm_workspace):
    """N=4: <a+b+c:d> — scheme with + (valid per RFC 3986)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 4)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al")


@pytest.mark.asyncio
async def test_autolinks_5_custom_scheme(cm_workspace):
    """N=5: <made-up-scheme://foo,bar> — CM allows any scheme, should be autolink.
    MDIO: doesn't recognize this as autolink (deviation)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 5)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al"), "custom URI scheme should be autolink"


@pytest.mark.asyncio
async def test_autolinks_6_relative_path(cm_workspace):
    """N=6: <https://../> — relative path in URL."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 6)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al")


@pytest.mark.asyncio
async def test_autolinks_7_localhost(cm_workspace):
    """N=7: <localhost:5001/foo> — localhost without scheme."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 7)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al")


@pytest.mark.asyncio
async def test_autolinks_8_space_disallowed(cm_workspace):
    """N=8: <https://foo.bar/baz bim> — space not allowed in autolink.
    MDIO: incorrectly creates autolink (deviation)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 8)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"al"), "autolink URL should not contain space"


@pytest.mark.asyncio
async def test_autolinks_9_escaped_chars(cm_workspace):
    """N=9: <https://example.com/\\[\\> — backslash-escaped chars."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 9)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al")


@pytest.mark.asyncio
async def test_autolinks_10_email(cm_workspace):
    """N=10: <foo@bar.example.com> — email autolink.
    CM spec: href='mailto:foo@bar.example.com'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 10)
    ast = ast_parse(md)
    links = ast_find_all(ast, "al")
    assert len(links) == 1, f"应有 1 个 autolink: {len(links)}"
    assert links[0].get("h") == "mailto:foo@bar.example.com", f"href: {links[0].get('h')!r}"


@pytest.mark.asyncio
async def test_autolinks_11_email_special_chars(cm_workspace):
    """N=11: <foo+special@Bar.baz-bar0.com> — email with special chars."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 11)
    ast = ast_parse(md)
    assert ast_has_type(ast,"al")


@pytest.mark.asyncio
async def test_autolinks_12_escaped_plus_not_email(cm_workspace):
    """N=12: <foo\\+@bar.example.com> — escaped +, NOT an email autolink."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 12)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"al"), "escaped + should not be autolink"


@pytest.mark.asyncio
async def test_autolinks_13_empty_brackets(cm_workspace):
    """N=13: <> — empty brackets, NOT an autolink."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 13)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"al"), "<> should not be autolink"


@pytest.mark.asyncio
async def test_autolinks_14_space_after_open(cm_workspace):
    """N=14: < https://foo.bar > — space after <, NOT an autolink."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 14)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"al"), "space after < should not be autolink"


@pytest.mark.asyncio
async def test_autolinks_15_single_letter_scheme(cm_workspace):
    """N=15: <m:abc> — single-letter scheme, NOT an autolink.
    MDIO: incorrectly creates autolink (deviation)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 15)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"al"), "single-letter scheme should not be autolink"


@pytest.mark.asyncio
async def test_autolinks_16_no_scheme_no_at(cm_workspace):
    """N=16: <foo.bar.baz> — no scheme, no @, NOT an autolink."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 16)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"al"), "no scheme no @ should not be autolink"


@pytest.mark.asyncio
async def test_autolinks_17_no_brackets(cm_workspace):
    """N=17: https://example.com — no brackets, NOT an autolink."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 17)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"al"), "no brackets should not be autolink"


@pytest.mark.asyncio
async def test_autolinks_18_email_no_brackets(cm_workspace):
    """N=18: foo@bar.example.com — no brackets, NOT an autolink."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Autolinks", 18)
    ast = ast_parse(md)
    assert not ast_has_type(ast,"al"), "no brackets should not be autolink"


# Hard line breaks (§6.7, 15 ex)

@pytest.mark.asyncio
async def test_hard_line_breaks_0_two_spaces(cm_workspace):
    """N=0: foo  \\nbaz — two trailing spaces = hard break."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 0)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "lb"), "two trailing spaces should produce lb"


@pytest.mark.asyncio
async def test_hard_line_breaks_1_backslash(cm_workspace):
    """N=1: foo\\\\nbaz — backslash = hard break."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 1)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "lb")


@pytest.mark.asyncio
async def test_hard_line_breaks_2_many_spaces(cm_workspace):
    """N=2: foo       \\nbaz — many trailing spaces = hard break."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 2)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "lb")


@pytest.mark.asyncio
async def test_hard_line_breaks_3_leading_spaces(cm_workspace):
    """N=3: leading spaces on next line stripped."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 3)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "lb")


@pytest.mark.asyncio
async def test_hard_line_breaks_4_backslash_leading_spaces(cm_workspace):
    """N=4: backslash + leading spaces on next line."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 4)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "lb")


@pytest.mark.asyncio
async def test_hard_line_breaks_5_in_emphasis(cm_workspace):
    """N=5: *foo  \\nbar* — hard break inside emphasis."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 5)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "lb")
    assert ast_has_type(ast, "em"), "emphasis should contain the break"


@pytest.mark.asyncio
async def test_hard_line_breaks_6_backslash_in_emphasis(cm_workspace):
    """N=6: *foo\\\\nbaz* — backslash hard break inside emphasis."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 6)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "lb")


@pytest.mark.asyncio
async def test_hard_line_breaks_7_in_code_span(cm_workspace):
    """N=7: `code  \\nspan` — spaces in code span are preserved."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 7)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "cd")


@pytest.mark.asyncio
async def test_hard_line_breaks_8_backslash_in_code(cm_workspace):
    """N=8: `code\\\\nbaz` — backslash preserved in code span."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 8)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "cd")


@pytest.mark.asyncio
async def test_hard_line_breaks_9_in_html_attr(cm_workspace):
    """N=9: <a href="foo  \\nbar"> — not a break in HTML attribute."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 9)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")


@pytest.mark.asyncio
async def test_hard_line_breaks_10_backslash_in_html(cm_workspace):
    """N=10: backslash not active in HTML attribute."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 10)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")


@pytest.mark.asyncio
async def test_hard_line_breaks_11_single_backslash(cm_workspace):
    """N=11: foo\\\\ — backslash at end of document, no following char."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 11)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")


@pytest.mark.asyncio
async def test_hard_line_breaks_12_trailing_spaces_only(cm_workspace):
    """N=12: foo  — trailing spaces at end, no next line."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 12)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")


@pytest.mark.asyncio
async def test_hard_line_breaks_13_heading_backslash(cm_workspace):
    """N=13: ### foo\\\\ — backslash at end of heading, not a hard break.
    Expected HTML: <h3>foo\\</h3>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 13)
    ast = ast_parse(md)
    assert ast_has_type(ast, "h"), "heading, not paragraph with hard break"


@pytest.mark.asyncio
async def test_hard_line_breaks_14_heading_spaces(cm_workspace):
    """N=14: ### foo  — spaces at end of heading, not a break.
    Expected HTML: <h3>foo</h3>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Hard line breaks", 14)
    ast = ast_parse(md)
    assert ast_has_type(ast, "h"), "heading, not paragraph with hard break"


# Soft line breaks (§6.8, 2 ex)

@pytest.mark.asyncio
async def test_soft_line_breaks_0_basic(cm_workspace):
    """N=0: foo\\nbaz — newline without trailing spaces = soft break."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Soft line breaks", 0)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "sb"), "newline without spaces should be soft break"


@pytest.mark.asyncio
async def test_soft_line_breaks_1_leading_space(cm_workspace):
    """N=1: foo \\n baz — space before newline, leading space on next line."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Soft line breaks", 1)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "sb")
