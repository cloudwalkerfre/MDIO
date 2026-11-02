"""§4.3 Setext headings — 27 examples (CM spec §4.3)."""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_find_all, ast_has_type, ast_to_html

H1, H2 = "Leaf blocks", "Setext headings"

# 类型名映射 — 新格式
HEADER = "h"
PARA = "p"
EM = "em"
SB = "sb"
HR = "hr"
BQ = "bq"
BULLET = "b"
CF = "c"
IC = "ic"
TEXT = "txt"


def _block_type(b):
    return b.get("nt")


@pytest.mark.asyncio
async def test_sh_00_basic_h1_and_h2_with_emphasis(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, expected = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert len(ast) == 2
    b1, b2 = ast[0], ast[1]
    assert _block_type(b1) == HEADER and b1["lv"] == 1
    assert _block_type(b2) == HEADER and b2["lv"] == 2
    assert ast_has_type(b1, EM)
    assert ast_has_type(b2, EM)
    # HTML 渲染对比 (端到端)
    assert ast_to_html(ast).strip() == expected.strip(), (
        f"HTML 渲染不一致: {ast_to_html(ast)!r} vs {expected!r}"
    )


@pytest.mark.asyncio
async def test_sh_01_multiline_emphasis(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert len(ast) == 1
    head = ast[0]
    assert _block_type(head) == HEADER and head["lv"] == 1
    assert ast_has_type(head, EM)
    assert ast_has_type(head, SB)


@pytest.mark.asyncio
async def test_sh_02_indented_content(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert len(ast) == 1
    head = ast[0]
    assert _block_type(head) == HEADER and head["lv"] == 1
    assert ast_has_type(head, EM)


@pytest.mark.asyncio
async def test_sh_03_underline_length_flexibility(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    assert len(ast) == 2
    assert _block_type(ast[0]) == HEADER and ast[0]["lv"] == 2
    assert _block_type(ast[1]) == HEADER and ast[1]["lv"] == 1
    assert ast[0]["t"] == "Foo"
    assert ast[1]["t"] == "Foo"


@pytest.mark.asyncio
async def test_sh_04_indentation_tolerance(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert len(ast) == 3
    for b in ast:
        assert _block_type(b) == HEADER
    assert ast[0]["lv"] == 2 and ast[0]["t"] == "Foo"
    assert ast[1]["lv"] == 2 and ast[1]["t"] == "Foo"
    assert ast[2]["lv"] == 1 and ast[2]["t"] == "Foo"


@pytest.mark.asyncio
async def test_sh_05_four_space_indent_code_block(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert ast_has_type(ast, IC) or ast_has_type(ast, CF), f"expected code block: {[_block_type(b) for b in ast]}"
    assert ast_has_type(ast, HR)


@pytest.mark.asyncio
async def test_sh_06_underline_with_trailing_spaces(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == HEADER and ast[0]["lv"] == 2
    assert ast[0]["t"] == "Foo"


@pytest.mark.asyncio
async def test_sh_07_four_space_underline_not_setext(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == PARA
    assert not ast_has_type(ast, HEADER)


@pytest.mark.asyncio
async def test_sh_08_space_between_underline_chars(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    assert len(ast) == 3
    assert _block_type(ast[0]) == PARA
    assert _block_type(ast[1]) == PARA
    assert _block_type(ast[2]) == HR
    assert not ast_has_type(ast, HEADER)


@pytest.mark.asyncio
async def test_sh_09_trailing_spaces_on_content(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == HEADER and ast[0]["lv"] == 2
    assert ast[0]["t"] == "Foo"


@pytest.mark.asyncio
async def test_sh_10_backslash_at_end_of_content(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == HEADER and ast[0]["lv"] == 2


@pytest.mark.asyncio
async def test_sh_11_html_tag_with_underline_pattern(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    blocks = [_block_type(b) for b in ast]
    assert len(ast) == 4, f"expected 4 blocks, got {len(ast)}: {blocks}"
    assert blocks[0] == HEADER and blocks[2] == HEADER
    assert blocks[1] == PARA and blocks[3] == PARA


@pytest.mark.asyncio
async def test_sh_12_bq_plus_underline_not_heading(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    assert ast_has_type(ast, BQ)
    assert ast_has_type(ast, HR)
    assert not ast_has_type(ast, HEADER)


@pytest.mark.asyncio
async def test_sh_13_bq_multiline_not_setext(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    assert ast_has_type(ast, BQ)
    assert not ast_has_type(ast, HEADER)
    assert not ast_has_type(ast, HR)


@pytest.mark.asyncio
async def test_sh_14_list_item_plus_underline_not_heading(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse(md)
    assert ast_has_type(ast, BULLET) or ast_has_type(ast, "bl")
    assert ast_has_type(ast, HR)
    assert not ast_has_type(ast, HEADER)


@pytest.mark.asyncio
async def test_sh_15_multiline_setext_heading(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == HEADER and ast[0]["lv"] == 2
    txt_nodes = ast_find_all(ast, TEXT)
    txts = [n.get("ct", "") for n in txt_nodes]
    assert "Foo" in txts and "Bar" in txts, f"both lines should be in heading: {txts}"


@pytest.mark.asyncio
async def test_sh_16_braided_thematic_breaks_and_setext(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    blocks = [_block_type(b) for b in ast]
    assert HR in blocks
    head_count = blocks.count(HEADER)
    assert head_count >= 2, f"expected ≥2 headings, got {head_count}: {blocks}"
    assert PARA in blocks


@pytest.mark.asyncio
async def test_sh_17_underline_without_content(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == PARA
    assert not ast_has_type(ast, HEADER)


@pytest.mark.asyncio
async def test_sh_18_two_thematic_breaks(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 18)
    ast = ast_parse(md)
    assert len(ast) == 2
    for b in ast:
        assert _block_type(b) == HR
    assert not ast_has_type(ast, HEADER)


@pytest.mark.asyncio
async def test_sh_19_list_then_long_underline(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 19)
    ast = ast_parse(md)
    assert ast_has_type(ast, BULLET) or ast_has_type(ast, "bl")
    assert ast_has_type(ast, HR)
    assert not ast_has_type(ast, HEADER)


@pytest.mark.asyncio
async def test_sh_20_four_space_code_not_setext(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 20)
    ast = ast_parse(md)
    assert ast_has_type(ast, IC) or ast_has_type(ast, CF), f"expected code block: {[_block_type(b) for b in ast]}"
    assert ast_has_type(ast, HR)
    assert not ast_has_type(ast, HEADER)


@pytest.mark.asyncio
async def test_sh_21_bq_plus_long_underline(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 21)
    ast = ast_parse(md)
    assert ast_has_type(ast, BQ)
    assert ast_has_type(ast, HR)
    assert not ast_has_type(ast, HEADER)


@pytest.mark.asyncio
async def test_sh_22_escaped_greater_in_setext_content(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 22)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == HEADER and ast[0]["lv"] == 2
    assert ">" in str(ast)


# 补齐用例 — underline 受干扰的边界语义 (N=23-26)

@pytest.mark.asyncio
async def test_sh_23_blank_line_interrupts(cm_workspace):
    """N=23: Foo\\n\\nbar\\n---\\nbaz → 空行中断 → Foo 是段落, bar 才是 H2。
    CM spec: '<p>Foo</p>\\n<h2>bar</h2>\\n<p>baz</p>'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 23)
    ast = ast_parse(md)
    types = [_block_type(b) for b in ast]
    assert types == [PARA, HEADER, PARA], f"got {types}"
    assert ast[1]["lv"] == 2


@pytest.mark.asyncio
async def test_sh_24_blank_line_before_underline(cm_workspace):
    """N=24: Foo\\nbar\\n\\n---\\n\\nbaz → --- 前有空行 → thematic break。
    CM spec: '<p>Foo\\nbar</p>\\n<hr />\\n<p>baz</p>'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 24)
    ast = ast_parse(md)
    types = [_block_type(b) for b in ast]
    assert types == [PARA, HR, PARA], f"got {types}"


@pytest.mark.asyncio
async def test_sh_25_thematic_break_not_underline(cm_workspace):
    """N=25: Foo\\nbar\\n* * *\\nbaz → * * * 是 thematic break。
    CM spec: '<p>Foo\\nbar</p>\\n<hr />\\n<p>baz</p>'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 25)
    ast = ast_parse(md)
    types = [_block_type(b) for b in ast]
    assert types == [PARA, HR, PARA], f"got {types}"


@pytest.mark.asyncio
async def test_sh_26_escaped_underline(cm_workspace):
    """N=26: Foo\\nbar\\n\\\\---\\nbaz → 反斜杠转义 → --- 是文本。
    CM spec: '<p>Foo\\nbar\\n---\\nbaz</p>' (单一段落)"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 26)
    ast = ast_parse(md)
    assert len(ast) == 1, f"应只有一个 paragraph, got {[_block_type(b) for b in ast]}"
    assert _block_type(ast[0]) == PARA
