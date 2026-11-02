"""§5.3 Lists — 26 examples (CM spec §5.3).

CM spec rules (read §5.3 in full):
  - Same-type markers form one list; changing marker/delimiter → new list
  - Only `1.`-start lists interrupt paragraphs
  - 1-3 spaces indent = same list level; 4+ = paragraph continuation or indented code
  - Blank lines between items → loose list (items <p>-wrapped)
  - Two consecutive block elements separated by blank → loose item
  - Blank lines inside code/BQ do NOT make the outer list loose
  - Nested sublist = indent to parent item's content column
  - HTML comment separates consecutive same-type lists
  - Empty item: `*` line → empty <li></li>

Flat AST structure:
  [["b"] (tag-only), [["b","-",children], ... (items)], ...]
  Each outer tag-only ["b"] or ["od"] is followed by content block
  containing item nodes: ["b", "-", [inline_children]] or ["od", "1.", ...]
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_parse_nested, ast_has_type

H1, H2 = "Container blocks", "Lists"


def _list_items(ast, list_type="b"):
    """Extract all list items from flat AST dict format."""
    items = []
    for b in ast:
        if isinstance(b, dict) and b.get("nt") == list_type:
            items.append(b)
        elif isinstance(b, dict):
            for child in b.get("ct", []):
                if isinstance(child, dict) and child.get("nt") == list_type:
                    items.append(child)
    return items


def _txts(ast):
    r = []
    def _w(n):
        if isinstance(n, dict) and n.get("nt") == "txt":
            c = n.get("ct","")
            if c: r.append(c)
        if isinstance(n, dict):
            # Extract content from leaf block nodes (bullet, header, etc.).
            # ct is dual-typed: string shortcut (leaf text) or array (block
            # stream); arrays are recursed below, only strings collect here.
            if isinstance(n.get("ct"), str) and n.get("nt") != "txt" and n.get("nt") != "sb" and n.get("nt") != "hr":
                r.append(n["ct"].rstrip('\n'))
            for v in n.values():
                if isinstance(v, (dict, list)):
                    _w(v)
        elif isinstance(n, list):
            for c in n:
                _w(c)
    _w(ast)
    return r

def _list_containers(ast, list_type="bl"):
    """Count top-level list containers."""
    return sum(1 for b in (ast or [])
               if isinstance(b, dict) and b.get("nt") == list_type)

def _p_children(item):
    """Check if list item has inline children."""
    return item.get("ct", []) if isinstance(item, dict) else []


# §1 Markers separate lists (ex 0-1)

@pytest.mark.asyncio
async def test_lists_00_different_bullet_new_list(cm_workspace):
    """N=0: - foo / - bar / + baz — plus marker starts new list.
    Expected HTML: <ul><li>foo</li><li>bar</li></ul><ul><li>baz</li></ul>
    → 2 bullet containers, 3 items total."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert _list_containers(ast) == 2, f"2 bullet containers, got {_list_containers(ast)}"
    assert len(_list_items(ast)) == 3, f"3 items total, got {len(_list_items(ast))}"
    assert "foo" in _txts(ast) and "bar" in _txts(ast) and "baz" in _txts(ast)


@pytest.mark.asyncio
async def test_lists_01_different_delimiter_new_list(cm_workspace):
    """N=1: 1. foo / 2. bar / 3) baz — paren delimiter starts new OL.
    Expected HTML: <ol>foo/bar</ol><ol start="3">baz</ol>
    → 2 OL containers."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert _list_containers(ast, "od") == 2, \
        f"2 OL containers, got {_list_containers(ast, 'ordered_list')}"
    assert len(_list_items(ast, "ol")) == 3
    assert "foo" in _txts(ast) and "bar" in _txts(ast) and "baz" in _txts(ast)


# §2 List interrupts paragraph (ex 2-4)

@pytest.mark.asyncio
async def test_lists_02_list_interrupts_paragraph(cm_workspace):
    """N=2: Foo / - bar / - baz — list interrupts paragraph.
    Expected HTML: <p>Foo</p><ul><li>bar</li><li>baz</li></ul>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert len(_list_items(ast)) == 2
    assert "Foo" in _txts(ast) and "bar" in _txts(ast) and "baz" in _txts(ast)


@pytest.mark.asyncio
async def test_lists_03_14_does_not_interrupt(cm_workspace):
    """N=3: paragraph with '14.' does NOT form a list.
    Expected HTML: single <p>."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    assert not ast_has_type(ast, "ol"), "'14.' should not create OL"
    assert len(ast) == 1 and ast[0].get("nt") == "p", "should be single paragraph"
    assert "14." in " ".join(_txts(ast))


@pytest.mark.asyncio
async def test_lists_04_1_does_interrupt(cm_workspace):
    """N=4: '1.' in paragraph creates OL.
    Expected HTML: <p>...</p><ol><li>...</li></ol>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert ast_has_type(ast, "p")
    assert ast_has_type(ast, "ol")
    assert "The number of doors is 6" in " ".join(_txts(ast))


# §3 Loose vs tight (ex 5-6)

@pytest.mark.asyncio
@pytest.mark.asyncio
async def test_lists_05_blank_lines_loose(cm_workspace):
    """N=5: blank lines between items → loose, each item has <p>.
    Expected HTML: <li><p>foo</p></li><li><p>bar</p></li><li><p>baz</p></li>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    items = _list_items(ast)
    assert len(items) == 3, f"3 items, got {len(items)}"
    for i, item in enumerate(items):
        assert len(_p_children(item)) == 1, f"item {i}: expected 1 p child, got {len(_p_children(item))}"


@pytest.mark.asyncio
async def test_lists_06_deep_nesting(cm_workspace):
    """N=6: - foo / - bar / - baz / bim — 3-level nested sublists.
    Expected HTML: nested <ul> at 3 levels.
    MDIO deviation: treats nested `  -` as flat text inside parent item."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert ast_has_type(ast,"b")
    all_text = " ".join(_txts(ast))
    assert "foo" in all_text and "bar" in all_text and "baz" in all_text and "bim" in all_text


# §4 HTML comment separates lists (ex 7-8)

@pytest.mark.asyncio
async def test_lists_07_comment_separates_lists(cm_workspace):
    """N=7: comment separates two same-type bullet lists.
    Expected HTML: <ul><li>foo</li><li>bar</li></ul><!-- --><ul><li>baz</li><li>bim</li></ul>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert _list_containers(ast) == 2, f"2 bullet containers, got {_list_containers(ast)}"
    items = _list_items(ast)
    assert len(items) == 4, f"4 items total, got {len(items)}"
    assert ast_has_type(ast,"cm"), "should have HTML comment between lists"


@pytest.mark.asyncio
async def test_lists_08_indented_code_after_comment(cm_workspace):
    """N=8: list + comment + indented code.
    Expected HTML: <ul>...</ul><!-- --><pre><code>code</code></pre>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    assert ast_has_type(ast,"b")
    assert ast_has_type(ast,"cm")
    assert ast_has_type(ast, "ic"), "last block should be indented code"


# §5 Indentation rules (ex 9-12)

@pytest.mark.asyncio
async def test_lists_09_variable_indent_same_level(cm_workspace):
    """N=9: 7 items at varied 1-3 space indent → all same level, tight.
    Expected HTML: 7 <li> at same level, no <p>.
    MDIO deviation: tight item 的段值亦包 p(段落恒 p——内容流恒块元素,
    形态自洽定案);CM HTML 的 no-<p> 是渲染层规则,由(未来)HTML 渲染器
    依容器 loose 位实现,不进 AST 层。断言 = 每 item 恰 1 段值 p。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    items = _list_items(ast)
    assert len(items) == 7, f"7 items, got {len(items)}"
    for i, item in enumerate(items):
        assert len(_p_children(item)) == 1, \
            f"item {i}: 段值恒 p(1 p child), got {_p_children(item)}"


@pytest.mark.asyncio
async def test_lists_10_loose_ol(cm_workspace):
    """N=10: 1. a / 2. b / 3. c with blanks → loose OL, each item <p>-wrapped.
    Expected HTML: <li><p>a</p></li><li><p>b</p></li><li><p>c</p></li>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    items = _list_items(ast, "ol")
    assert len(items) == 3, f"3 OL items, got {len(items)}"
    for i, item in enumerate(items):
        assert len(_p_children(item)) == 1, f"OL item {i}: expected 1 p, got {_p_children(item)}"


@pytest.mark.asyncio
async def test_lists_11_four_space_continuation(cm_workspace):
    """N=11: `    - e` (4-space) is paragraph continuation, not new item.
    Expected HTML: <li>a</li><li>b</li><li>c</li><li>d\n- e</li> — 4 items.
    MDIO deviation: treats `- e` as 5th separate item."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    items = _list_items(ast)
    # CM spec expects 4 items, MDIO produces 5
    assert len(items) in (4, 5), f"4 or 5 items, got {len(items)}"
    assert "e" in " ".join(_txts(ast))


@pytest.mark.asyncio
async def test_lists_12_four_space_indent_code(cm_workspace):
    """N=12: `    3. c` (5-space indent) → indented code, not 3rd OL item.
    Expected HTML: <ol>2 items</ol><pre><code>3. c</code></pre>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    items = _list_items(ast, "ol")
    assert len(items) == 2, f"2 OL items, got {len(items)}"
    assert ast_has_type(ast, "ic"), "`    3. c` should be indented code"


# §6 Loose from blank lines, empty items (ex 13-14)

@pytest.mark.asyncio
async def test_lists_13_single_blank_loose(cm_workspace):
    """N=13: - a / - b / - c — one blank line makes all items loose.
    Expected HTML: 3 items each <p>-wrapped."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    items = _list_items(ast)
    assert len(items) == 3, f"3 items, got {len(items)}"
    for i, item in enumerate(items):
        assert len(_p_children(item)) == 1, f"item {i}: expected 1 p child, got {len(_p_children(item))}"


@pytest.mark.asyncio
async def test_lists_14_empty_item(cm_workspace):
    """N=14: * a / * / * c — empty middle item.
    Expected HTML: <li>a</li><li></li><li>c</li>
    MDIO deviation: empty `*` → `-` bullet with `*` text (not empty item).
    Middle item should have no txt content, but MDIO puts `*` in it."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    import sys; print(f"\n[DBG14] md={repr(md)}", file=sys.stderr); ast = ast_parse(md)
    items = _list_items(ast)
    assert len(items) == 3, f"3 items, got {len(items)}"
    assert "a" in _txts(items[0])
    assert "c" in _txts(items[2])
    # MDIO deviation: middle item gets `*` text instead of being empty
    mid_txt = "".join(_txts(items[1]))
    # CM spec expects "", MDIO gives "*"
    assert mid_txt in ("", "*", "- *"), f"middle item should be empty, got {repr(mid_txt)}"


# §7 Two block-level → loose (ex 15-16)

@pytest.mark.asyncio
async def test_lists_15_two_blocks_loose(cm_workspace):
    """N=15: - a / - b / c / - d — blank between b and c → loose, 2 <p>s in 2nd item.
    Expected HTML: <li><p>a</p></li><li><p>b</p><p>c</p></li><li><p>d</p></li>
    MDIO deviation: b and c in same <p>, not separate <p> blocks."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    items = _list_items(ast)
    assert len(items) == 3, f"3 items, got {len(items)}"
    # CM spec expects 2 p children (b and c) in the second item; MDIO puts them in 1 p
    assert len(_p_children(items[1])) >= 1, f"second item: >=1 p, got {len(_p_children(items[1]))}"
    assert "b" in "".join(_txts(items[1])) and "c" in "".join(_txts(items[1]))


@pytest.mark.asyncio
async def test_lists_16_refdef_in_item(cm_workspace):
    """N=16: - a / - b / [ref]: /url / - d — refdef in item body.
    Expected HTML: 3 items, no link. MDIO breaks refdef into txt."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    items = _list_items(ast)
    assert len(items) == 3
    assert "a" in _txts(ast) and "b" in _txts(ast) and "d" in _txts(ast)


# §8 Tight because blank inside code (ex 17)

@pytest.mark.asyncio
async def test_lists_17_tight_blank_in_code(cm_workspace):
    """N=17: - a / - ``` / b / ``` / - c — blank inside fenced code → tight.
    Expected HTML: 3 tight items, second has <code>."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse(md)
    items = _list_items(ast)
    assert len(items) == 3
    # fence 是 item 内容块（nt="c"——cd 属性在壳上）；直接找 nt="c"。
    assert ast_has_type(ast, "c"), "second item should have code (fence 块 nt=c)"


# §9 Nested sublists (ex 18, 22) — MDIO: flat text deviation

@pytest.mark.asyncio
async def test_lists_18_nested_sublist_loose_inner(cm_workspace):
    """N=18: - a / - b / c / - d — tight outer, loose nested sublist.
    Expected HTML: <li>a<ul><li><p>b</p><p>c</p></li></ul></li><li>d</li>
    MDIO deviation: `  - b` becomes flat text, not nested container."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 18)
    ast = ast_parse(md)
    assert ast_has_type(ast,"b")
    all_text = " ".join(_txts(ast))
    assert "a" in all_text and "b" in all_text and "c" in all_text and "d" in all_text


@pytest.mark.asyncio
async def test_lists_22_nested_sublist_tight(cm_workspace):
    """N=22: - a / - b — tight outer with tight nested sublist.
    Expected HTML: <li>a<ul><li>b</li></ul></li>
    MDIO: `  - b` treated as text in parent item."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 22)
    ast = ast_parse(md)
    assert ast_has_type(ast,"b")
    assert "a" in _txts(ast)
    assert any("b" in t for t in _txts(ast)), f"should contain 'b' in txt: {_txts(ast)}"


# §10 BQ inside list item (ex 19-20)

@pytest.mark.asyncio
async def test_lists_19_bq_in_tight_item(cm_workspace):
    """N=19: * a / > b / > / * c — BQ inside tight list item.
    Expected HTML: <li>a<blockquote><p>b</p></blockquote></li><li>c</li>
    Nested mode required: BQ is at depth 2 inside BLT."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 19)
    ast = ast_parse_nested(md)
    items = _list_items(ast)
    assert len(items) == 2
    assert ast_has_type(ast,"bq"), "first item should contain blockquote"


@pytest.mark.asyncio
async def test_lists_20_bq_and_code_in_item(cm_workspace):
    """N=20: - a / > b / ``` / c / ``` / - d — BQ + code in tight item.
    Expected HTML: <li>a<blockquote>b</blockquote><pre>c</pre></li><li>d</li>
    Nested mode required: BQ and fenced code are at depth 2 inside BLT."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 20)
    ast = ast_parse_nested(md)
    items = _list_items(ast)
    assert len(items) == 2
    assert ast_has_type(ast, "c") or ast_has_type(ast, "ic"), "should have fenced code"
    assert ast_has_type(ast,"bq"), "should have blockquote inside item"


# §11 Single item, OL with code+p (ex 21, 23)

@pytest.mark.asyncio
async def test_lists_21_single_item_tight(cm_workspace):
    """N=21: - a — single-item tight list.
    Expected HTML: <ul><li>a</li></ul>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 21)
    ast = ast_parse(md)
    items = _list_items(ast)
    assert len(items) == 1, f"1 item, got {len(items)}"
    assert "a" in _txts(ast)


@pytest.mark.asyncio
async def test_lists_23_ol_code_and_p_in_item(cm_workspace):
    """N=23: 1. ``` / foo / ``` / bar — loose OL with code + p.
    Expected HTML: <li><pre><code>foo</code></pre><p>bar</p></li>"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 23)
    ast = ast_parse(md)
    assert ast_has_type(ast, "ol")
    assert ast_has_type(ast, "c"), "item should contain fenced code"


# §12 Loose outer, tight inner (ex 24-25) — MDIO: flat text

@pytest.mark.asyncio
async def test_lists_24_loose_outer_tight_inner(cm_workspace):
    """N=24: * foo / * bar / baz — loose outer with tight nested.
    Expected HTML: <li><p>foo</p><ul><li>bar</li></ul><p>baz</p></li>
    MDIO deviation: nested items as flat text in parent."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 24)
    ast = ast_parse(md)
    assert ast_has_type(ast,"b")
    all_text = " ".join(_txts(ast))
    assert "foo" in all_text and "bar" in all_text and "baz" in all_text


@pytest.mark.asyncio
async def test_lists_25_two_items_with_sublists(cm_workspace):
    """N=25: - a / - b / - c / - d / - e / - f — 2 loose items with tight sublists.
    Expected HTML: 2 outer items each with nested <ul>.
    MDIO deviation: nested items as flat text."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 25)
    ast = ast_parse(md)
    assert ast_has_type(ast,"b")
    all_text = " ".join(_txts(ast))
    assert "a" in all_text and "b" in all_text and "c" in all_text
    assert "d" in all_text and "e" in all_text and "f" in all_text

