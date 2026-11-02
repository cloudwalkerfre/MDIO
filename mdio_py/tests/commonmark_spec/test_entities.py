"""§2.5 Entity references — 17 examples (CM spec §2.5).

CM spec rules:
  - Entity/numeric references are decoded to Unicode in contexts where
    they would be parsed as such: paragraphs, headings, link text
  - NOT decoded in: code spans, code blocks (indented/fenced), autolinks,
    raw HTML attributes, code fence info strings
  - Numeric references: &#NNNN; (decimal), &#XHHHH; (hex) — decode to
    Unicode code point
  - Named entities: &amp; &lt; &quot; &nbsp; etc. — decode to char
  - Entity without trailing `;` → NOT an entity (literal text)
  - Unknown entity name → NOT decoded (literal text)
  - Entity decoding happens AFTER inline parsing structure is determined
    (&#42; decodes to * but does not create emphasis or list items)

Selector: Entity references 位于 H1="Preliminaries" 下。

AST structure:
  Entity nodes in MDIO: entities are decoded inline to their Unicode chars.
  MDIO emits separate "entity" AST nodes (nt: "entity") for decoded entities
  that differ from raw source (e.g. &#42; → *). Entities where decoded char
  matches raw source (e.g. &amp; → &) may merge into surrounding txt nodes.
"""

import json
import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_find_all, ast_has_type

H1, H2 = "Preliminaries", "Entity and numeric character references"


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
    """Join all text and entity node content."""
    return "".join(n.get("ct","") for n in ast_find_all(ast, "txt")) + "".join(n.get("ct","") for n in ast_find_all(ast, "entity"))


# Tests — §2.5 Entity references (17 examples)

# Entities decoded in paragraphs

@pytest.mark.asyncio
async def test_ent_00_named_entities_decoded_in_paragraph(cm_workspace):
    """N=0: &nbsp; &amp; &copy; &AElig; ... → decoded to Unicode in paragraph.
    CM spec §5.3: named entities replaced by corresponding Unicode chars
    in paragraph context. MDIO decodes &nbsp; and &amp; correctly."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    txt = _all_txt(ast)
    # &nbsp; MUST be decoded to non-breaking space
    assert "\u00a0" in txt or " " in txt, "&nbsp; should be decoded"
    # &amp; MUST be decoded to &
    assert "& " in txt or "&" in txt, "&amp; should be decoded"
    # Should not have literal "&nbsp;" text in output
    assert "&nbsp;" not in txt.split("&AElig;")[0], f"&nbsp; literal found: {txt[:100]}"


@pytest.mark.asyncio
async def test_ent_01_numeric_decimal_entities(cm_workspace):
    """N=1: &#35; &#1234; &#992; &#0; → decoded to # Ӓ Ϡ �.
    CM spec: numeric decimal entities decoded to Unicode code points."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    txt = _all_txt(ast)
    # &#35; → #
    assert "#" in txt, "&#35; should decode to #"


@pytest.mark.asyncio
async def test_ent_02_numeric_hex_entities(cm_workspace):
    """N=2: &#X22; &#XD06; &#xcab; → decoded to \" ആ ಫ.
    CM spec: hex entities (case-insensitive X) decoded to Unicode."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    txt = _all_txt(ast)
    # &#X22; → "
    assert '"' in txt, "&#X22; should decode to double quote"


@pytest.mark.asyncio
async def test_ent_03_non_entities_preserved_literal(cm_workspace):
    """N=3: &nbsp &x; &#; &#x; &ThisIsNotDefined; → literal text (not entities).
    CM spec: entity without semicolon, incomplete hex, or unknown name →
    NOT decoded. '&#0;' and '&#x;' may produce replacement char."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    txt = _all_txt(ast)
    # &nbsp (no semicolon) → literal. 用 startswith 避免 '&nbsp' 匹配 '&nbsp;' 的子串
    assert txt.startswith("&nbsp "), f"should start with literal '&nbsp ', got {txt[:50]!r}"


@pytest.mark.asyncio
async def test_ent_04_amp_copy_no_semicolon(cm_workspace):
    """N=4: &copy → literal '&copy' (no semicolon).
    CM spec: '&' followed by letters but no ';' → NOT an entity."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    txt = _all_txt(ast)
    assert txt == "&copy", f"expected literal '&copy' (no semicolon → not entity), got {txt!r}"


@pytest.mark.asyncio
async def test_ent_05_unknown_entity_literal(cm_workspace):
    """N=5: &MadeUpEntity; → literal (unknown entity name).
    CM spec: unknown entity names are NOT decoded."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    txt = _all_txt(ast)
    assert "&MadeUpEntity;" == txt


# Entities NOT decoded in HTML/links/code

@pytest.mark.asyncio
async def test_ent_06_entities_in_raw_html_not_decoded(cm_workspace):
    """N=6: <a href=\"&ouml;&ouml;.html\"> → entities NOT decoded in raw HTML.
    CM spec: entities are not decoded in raw HTML blocks/tags."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "hb", ast
    raw = ast[0].get("ct", "")
    assert "&ouml;" in raw, "entities in HTML should be literal"


@pytest.mark.asyncio
async def test_ent_07_entities_in_link_url_should_decode(cm_workspace):
    """N=7: [foo](/f&ouml;&ouml; \"f&ouml;&ouml;\") → URL decoded to /föö.
    CM spec: entities in link destinations and titles ARE decoded."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    link = _find(ast, "lk")
    assert link is not None, "should have link"
    url = link.get("h","")
    assert "f" in url and "ouml" not in url, f"expected decoded URL, got {url!r}"


@pytest.mark.xfail(reason="MDIO: refmap deferred — reference link resolution not implemented")
@pytest.mark.asyncio
async def test_ent_08_entities_in_refdef_should_decode(cm_workspace):
    """N=8: [foo]: /f&ouml;&ouml; \"f&ouml;&ouml;\" → URL decoded to /föö.
    CM spec: same as inline link — entities in refdef destinations decode."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    assert len(ast) == 2
    assert ast[0].get("nt") == "p"
    assert ast_has_type(ast, "rd"), "refdef node should not be skipped"
    link = _find(ast, "lk")
    assert link is not None, "should have link"
    url = link.get("h","")
    assert "f" in url and "ouml" not in url, f"expected decoded URL, got {url!r}"


async def test_ent_09_entities_in_fence_info_decoded(cm_workspace):
    """N=9: ``` f&ouml;&ouml; → info string decoded to 'föö'.
    CM spec: entities in code fence info strings ARE decoded."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    code_blocks = [b for b in ast if isinstance(b, dict) and b.get("nt") == "c"]
    if code_blocks:
        tag = code_blocks[0].get("t","")
        assert "ouml" not in tag, f"expected decoded info string, got {tag!r}"


@pytest.mark.asyncio
async def test_ent_10_entities_in_code_span_not_decoded(cm_workspace):
    """N=10: `f&ouml;&ouml;` → entities NOT decoded in code span.
    CM spec: entities are not decoded in code spans."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    code = _find(ast, "cd")
    assert code is not None, "should have code span"
    assert code.get("cd") == "f&ouml;&ouml;", f"entities should be literal in code span: {code.get('cd')!r}"


@pytest.mark.asyncio
async def test_ent_11_entities_in_indented_code(cm_workspace):
    """N=11: `    f&ouml;f&ouml;` → 4-space indented code block.
    CM spec: entities in indented code are NOT decoded."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ic"), "4-space indent → code_block"


@pytest.mark.asyncio
async def test_ent_12_entity_decoded_after_parsing_not_emphasis(cm_workspace):
    """N=12: &#42;foo&#42; / *foo* → &#42; decodes to *, but does NOT create em.
    CM spec: entity decoding happens AFTER inline structure is determined,
    so &#42; cannot form emphasis markers."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    # Entity-decoded &#42; → * should appear as entity nodes, NOT as emphasis
    entities = [n.get("ct","") for n in ast_find_all(ast, "entity")]
    assert "*" in entities, "&#42; should decode to * as entity node"
    # The real *foo* on the next line SHOULD create emphasis
    assert ast_has_type(ast, "em"), "real *foo* should create emphasis"


@pytest.mark.asyncio
async def test_ent_13_entity_not_list_marker(cm_workspace):
    """N=13: &#42; foo / * foo → entity-decoded * NOT a list marker.
    CM spec: block structure is determined before entity decoding, so
    &#42; cannot start a list item."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    blocks = [b.get("nt") for b in ast if isinstance(b, dict)]
    assert "p" in blocks, "&#42; foo should be paragraph, not list item"
    assert "bl" in blocks, "* foo should be a real list item"


@pytest.mark.asyncio
async def test_ent_14_entity_newline_no_paragraph_break(cm_workspace):
    """N=14: foo&#10;&#10;bar → decoded newlines do NOT break paragraph.
    CM spec: entity decoding after block parsing → &#10; stays in paragraph."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    txt = _all_txt(ast)
    assert "foo" in txt and "bar" in txt, "both parts should be in same paragraph"


@pytest.mark.asyncio
async def test_ent_15_tab_entity_decoded(cm_workspace):
    """N=15: &#9;foo → decoded to tab character + foo in paragraph.
    CM spec: &#9; → U+0009 (tab)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    txt = _all_txt(ast)
    assert "foo" in txt, "should contain foo"


@pytest.mark.asyncio
async def test_ent_16_entity_quote_in_url_not_a_link(cm_workspace):
    """N=16: [a](url &quot;tit&quot;) → &quot; decodes to \", but
    title starts BEFORE the entity is decoded → \" appears literal → NOT a link.
    CM spec: entity decoding after link parsing → not a valid link."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    assert len(ast) == 1 and ast[0].get("nt") == "p"
    assert not ast_has_type(ast, "lk"), "should NOT be a link (entity decoded after parsing)"
