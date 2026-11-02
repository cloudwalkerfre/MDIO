"""§4.1 Thematic breaks — 19 examples (CM spec §4.1)."""

import json
import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_has_type, ast_to_html

H1, H2 = "Leaf blocks", "Thematic breaks"

TB = "hr"
PARA = "p"
HEADER = "h"
EM = "em"
IC = "ic"
BULLET = "b"
BULLET_LIST = "bl"


def _block_type(b):
    return b.get("nt")


# §4.1 有效 TB

@pytest.mark.asyncio
async def test_tb_00_three_variants(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, expected = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert len(ast) == 3
    for b in ast:
        assert _block_type(b) == TB
    # HTML 渲染对比 (端到端)
    assert ast_to_html(ast).strip() == expected.strip(), (
        f"HTML 渲染不一致: {ast_to_html(ast)!r} vs {expected!r}"
    )


@pytest.mark.asyncio
async def test_tb_01_plus_not_tb(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == PARA


@pytest.mark.asyncio
async def test_tb_02_equals_not_tb(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == PARA


@pytest.mark.asyncio
async def test_tb_03_less_than_three_not_tb(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == PARA


@pytest.mark.asyncio
async def test_tb_04_indent_up_to_3_spaces_ok(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert len(ast) == 3
    for b in ast:
        assert _block_type(b) == TB


@pytest.mark.asyncio
async def test_tb_05_four_space_indent_code_block(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) in (IC, "c"), f"expected code block, got {_block_type(ast[0])!r}"


@pytest.mark.asyncio
async def test_tb_06_tb_in_paragraph_continuation(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == PARA
    assert not ast_has_type(ast, TB)


@pytest.mark.asyncio
async def test_tb_07_long_underscore_tb(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == TB


@pytest.mark.asyncio
async def test_tb_08_spaces_between_chars(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 8)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == TB


@pytest.mark.asyncio
async def test_tb_09_complex_space_tb(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 9)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == TB


@pytest.mark.asyncio
async def test_tb_10_long_dash_spaces(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 10)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == TB


@pytest.mark.asyncio
async def test_tb_11_trailing_spaces_on_tb(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 11)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == TB


@pytest.mark.asyncio
async def test_tb_12_extra_chars_not_tb(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 12)
    ast = ast_parse(md)
    assert len(ast) == 3
    for b in ast:
        assert _block_type(b) == PARA
    assert not ast_has_type(ast, TB)


@pytest.mark.asyncio
async def test_tb_13_emphasis_with_dash(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 13)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert _block_type(ast[0]) == PARA
    assert ast_has_type(ast, EM)
    assert not ast_has_type(ast, TB)


@pytest.mark.asyncio
async def test_tb_14_tb_between_lists(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 14)
    ast = ast_parse(md)
    blocks = [_block_type(b) for b in ast]
    assert any(b in (BULLET, BULLET_LIST) for b in blocks)
    assert TB in blocks


@pytest.mark.asyncio
async def test_tb_15_tb_between_paragraphs(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 15)
    ast = ast_parse(md)
    blocks = [_block_type(b) for b in ast]
    assert blocks == [PARA, TB, PARA], f"got {blocks}"


@pytest.mark.asyncio
async def test_tb_16_dash_underline_is_setext_not_tb(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 16)
    ast = ast_parse(md)
    blocks = [_block_type(b) for b in ast]
    assert blocks == [HEADER, PARA], f"got {blocks}"
    assert ast[0]["lv"] == 2
    assert not ast_has_type(ast, TB)


@pytest.mark.asyncio
async def test_tb_17_star_tb_between_lists(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 17)
    ast = ast_parse(md)
    assert ast_has_type(ast, BULLET) or ast_has_type(ast, BULLET_LIST)
    assert ast_has_type(ast, TB)


@pytest.mark.asyncio
async def test_tb_18_tb_inside_list_item(cm_workspace):
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 18)
    ast = ast_parse(md)
    items = [b for b in ast if isinstance(b, dict) and b.get("nt") in (BULLET, "bl")]
    container = next((b for b in ast if isinstance(b, dict) and b.get("nt") == "bl"), None)
    if container:
        items = [c for c in container.get("ct", []) if isinstance(c, dict) and c.get("nt") == BULLET]
    assert len(items) >= 2, f"expected >=2 bullet items: {json.dumps(ast, ensure_ascii=False)}"
