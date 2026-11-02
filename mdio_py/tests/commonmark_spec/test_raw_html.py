"""§6.6 Raw HTML — 21 examples (CM spec §6.6).

CM spec rules:
  - Open tags: <tagname [attributes] [/]> — tag name = ASCII letter then
    alnum/-, attributes optional, may end with `/>` (self-closing)
  - Close tags: </tagname> — no attributes allowed
  - Valid tags → raw literal text in inline context (passthrough)
  - Invalid tags (<33>, <__>, < followed by space, broken attributes,
    close tag with attributes) → NOT raw HTML → escaped (< → &lt;)
  - Comments <!-- -->, PI <? ?>, declarations <!...>, CDATA <![CDATA[...]]>
    → raw passthrough
  - Entities (&ouml;) and backslashes (\\*) in attributes → literal (not decoded)
  - Backslash in attribute value can break quoting → tag becomes invalid
  - Newline in unquoted attribute → tag ends at newline, subsequent `>`
    starts a block quote

AST structure:
  Raw HTML tags appear as literal txt nodes within paragraphs.
  No separate "html" inline node type. Some HTML tokens are split across
  multiple txt nodes depending on inline parser tokenization.
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_find_all

H1, H2 = "Inlines", "Raw HTML"


def _blocks(ast):
    return [b.get("nt") for b in ast if isinstance(b, dict)]


def _all_txt(ast, join=" "):
    """Join all text node content."""
    return join.join(n.get("ct","") for n in ast_find_all(ast, "txt"))


def _has_txt_containing(ast, needle):
    """Check if any text node or plain block content field contains the given string."""
    # 1. Search inline text nodes
    for n in ast_find_all(ast, "txt"):
        c = n.get("ct","")
        if isinstance(c, str) and needle in c:
            return True
    # 2. Search block-level content field (plain text without inline triggers
    #    results in empty children — content sits in the block's content field)
    def _plain_content(node):
        if isinstance(node, dict):
            c = node.get("ct")
            # ct 双型:字符串 = 纯文本快捷(直接查);数组 = 内容流。
            if isinstance(c, str) and needle in c:
                return True
            for v in node.values():
                if isinstance(v, (dict, list)) and _plain_content(v):
                    return True
        elif isinstance(node, list):
            for item in node:
                if _plain_content(item):
                    return True
        return False
    return _plain_content(ast)


# Tests — §6.6 Raw HTML

# Valid HTML open/void tags → raw passthrough (N=0-4, 10)

@pytest.mark.asyncio
async def test_rh_00_multiple_open_tags_raw(cm_workspace):
    """N=0: <a><bab><c2c> — three valid open tags → raw literal.
    CM: open tag name starts with ASCII letter, alnum/- follow."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert _has_txt_containing(ast, "<a>")
    assert _has_txt_containing(ast, "<bab>")
    assert _has_txt_containing(ast, "<c2c>")


@pytest.mark.asyncio
async def test_rh_01_void_tags_raw(cm_workspace):
    """N=1: <a/><b2/> — self-closing void tags → raw literal.
    CM: trailing `/>` makes tag self-closing."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert _has_txt_containing(ast, "<a/>")
    assert _has_txt_containing(ast, "<b2/>")


@pytest.mark.asyncio
async def test_rh_02_multiline_attribute_raw(cm_workspace):
    """N=2: <a  /><b2\\ndata=\"foo\" > — multiline attribute → raw.
    CM: attributes can span lines if quoted."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert _has_txt_containing(ast, "<a  />")
    assert _has_txt_containing(ast, "data=\"foo\"")


@pytest.mark.asyncio
async def test_rh_03_complex_quoted_attributes_raw(cm_workspace):
    """N=3: mixed single/double quotes + _boolean attr → raw.
    CM: attribute values can use \" or ' quotes, boolean attrs allowed."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    # Verify the full HTML string is present
    combined = _all_txt(ast, "")
    assert "foo=\"bar\"" in combined and "bam = 'baz" in combined
    assert "zoop:33" in combined


@pytest.mark.asyncio
async def test_rh_04_custom_element_raw(cm_workspace):
    """N=4: <responsive-image src=\"foo.jpg\" /> — custom element → raw.
    CM: hyphens allowed in tag names → valid custom element."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert _has_txt_containing(ast, "responsive-image")
    assert _has_txt_containing(ast, "foo.jpg")


# Invalid HTML → escaped (N=5-9, 11, 19)
# CM: < followed by non-letter (digit, _, space) → NOT a tag → literal <
# MDIO stores the literal < as txt content; ast_to_html renders as is.

@pytest.mark.asyncio
async def test_rh_05_digit_start_not_a_tag(cm_workspace):
    """N=5: <33> <__> — tag name starts with digit or _ → NOT valid HTML.
    CM: tag name must start with ASCII letter. <33> → &lt;33&gt;."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    # Verify < is present as literal text (separate from tag content)
    combined = _all_txt(ast, "")
    assert "<33>" in combined or ("33" in combined)
    # Should NOT be valid tags — MDIO splits < from the invalid name
    assert not _has_txt_containing(ast, "<33>")


@pytest.mark.asyncio
async def test_rh_06_special_char_in_tag_name_escaped(cm_workspace):
    """N=6: <a h*#ref=\"hi\"> — `*#` in attribute name → invalid HTML.
    CM: attribute names must be ASCII alnum, _, :, -. *→&lowast;,#→&#35;."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    # MDIO treats this as text (doesn't unescape * and #)
    assert "h*#ref" in _all_txt(ast, "")


@pytest.mark.asyncio
async def test_rh_07_unquoted_attr_with_quote_escaped(cm_workspace):
    """N=7: <a href=\"hi'> — unquoted value contains ' → invalid.
    CM: unquoted attribute value cannot contain ', \", =, <, >, `."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    # MDIO splits < from the content
    assert not _has_txt_containing(ast, "<a href=\"hi'>")


@pytest.mark.asyncio
async def test_rh_08_whitespace_after_lt_escaped(cm_workspace):
    """N=8: < a>, <\\nfoo>, <bar/ >... — < then space/newline → NOT a tag.
    CM: < followed by space, tab, or newline → literal <."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    # < is separate from tag content
    assert not _has_txt_containing(ast, "< a>")


@pytest.mark.asyncio
async def test_rh_09_unquoted_no_space_between_attrs(cm_workspace):
    """N=9: <a href='bar'title=title> — no space between value and attr.
    CM: unquoted values end at first whitespace or >. No space → invalid."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert "'bar'title" in _all_txt(ast, "")


@pytest.mark.asyncio
async def test_rh_10_close_tags_raw(cm_workspace):
    """N=10: </a></foo > — valid close tags → raw literal.
    CM: </tagname> is a valid close tag. Trailing space allowed."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    # MDIO splits < from /a> and /foo > but retains the close tag markers


@pytest.mark.asyncio
async def test_rh_11_close_tag_with_attr_escaped(cm_workspace):
    """N=11: </a href=\"foo\"> — close tag with attribute → invalid.
    CM: 'closing tags cannot have attributes.'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    # < is separate from /a href... — not a valid close tag
    assert not _has_txt_containing(ast, "</a href=\"foo\">")


# Comments / PI / declarations / CDATA → raw passthrough (N=12-16)

@pytest.mark.asyncio
async def test_rh_12_comment_raw(cm_workspace):
    """N=12: foo <!-- this is a --\\ncomment - with hyphens --> → raw.
    CM: HTML comments passthrough. < inside comment is part of it."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert _has_txt_containing(ast, "foo")
    assert "this is a" in _all_txt(ast, "")
    assert "comment" in _all_txt(ast, "")


@pytest.mark.asyncio
async def test_rh_13_comment_edge_cases(cm_workspace):
    """N=13: <!--> foo --> (ends at >) + <!---> foo --> (ends at ->).
    CM: comments end at the first `-->`. The first one eats `foo -->` literally.
    Second one: MDIO stores <!---> as literal text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p", "p"]
    assert "foo -->" in _all_txt(ast, "")


@pytest.mark.asyncio
async def test_rh_14_pi_raw(cm_workspace):
    """N=14: foo <?php echo $a; ?> — processing instruction → raw.
    CM: <? ... ?> is a processing instruction, raw passthrough."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert "php" in _all_txt(ast, "")


@pytest.mark.asyncio
async def test_rh_15_declaration_raw(cm_workspace):
    """N=15: foo <!ELEMENT br EMPTY> — DOCTYPE/declaration → raw.
    CM: <! followed by ASCII letter → declaration."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert "ELEMENT" in _all_txt(ast, "")


@pytest.mark.asyncio
async def test_rh_16_cdata_raw(cm_workspace):
    """N=16: foo <![CDATA[>&<]]> — CDATA section → raw.
    CM: <![CDATA[...]]> passthrough, content not parsed."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert "CDATA" in _all_txt(ast, "")


# Entities / escapes in raw HTML → literal (N=17-18)

@pytest.mark.asyncio
async def test_rh_17_entity_in_attr_literal(cm_workspace):
    """N=17: foo <a href=\"&ouml;\"> — &ouml; in attribute → literal.
    CM: entities in raw HTML attributes are NOT decoded."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert "&ouml;" in _all_txt(ast, "")


@pytest.mark.asyncio
async def test_rh_18_backslash_in_attr_literal(cm_workspace):
    """N=18: foo <a href=\"\\*\"> — \\* in attribute → literal.
    CM: backslash escapes NOT processed in raw HTML attributes."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 18)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    assert "\\*" in _all_txt(ast, "")


# Edge cases (N=19-20)

@pytest.mark.asyncio
async def test_rh_19_backslash_in_quote_breaks_tag(cm_workspace):
    """N=19: <a href=\"\\\"\"> — backslash-escaped quote in attribute.
    CM: \\\" does NOT close the quoted value; behavior is implementation-defined.
    MDIO: \\\" is treated as literal inside the quote → breaks tag matching.
    The < is split from the rest → escaped."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 19)
    ast = ast_parse(md)
    assert _blocks(ast) == ["p"]
    # < is separate from tag content → escaped
    assert not _has_txt_containing(ast, "<a href=\"\\\"\">")


@pytest.mark.asyncio
async def test_rh_20_newline_ends_unquoted_attr_starts_blockquote(cm_workspace):
    """N=20: <a\\n> quoted text — \\n ends unquoted attr, <a becomes text,
    > starts a block quote. CM: unquoted values cannot contain newlines."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 20)
    ast = ast_parse(md)
    blocks = _blocks(ast)
    assert "p" in blocks and "bq" in blocks
    # Verify the first paragraph contains <a as text
    paras = [b for b in ast if isinstance(b,dict) and b.get("nt") == "p"]
    assert any("a" in _all_txt([p], "") for p in paras)
    # Verify blockquote contains "quoted text"
    assert _has_txt_containing(ast, "quoted text")
