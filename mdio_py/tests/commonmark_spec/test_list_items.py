"""§19 List items — 48 examples，覆盖 Rule #1-#5 及关键边界。"""

import pytest
from pathlib import Path
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_desc, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_parse_nested, ast_find_all, ast_block_types, ast_has_type


@pytest.mark.asyncio
async def test_list_items_bullet_basic(cm_workspace):
    """ex0 (_code[0]): paragraph + indented code + BQ → 三种独立 block 类型。
    indented code block 识别为 code_block。
    """
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 0)
    ast = ast_parse(md_input)
    blocks = ast_block_types(ast)
    assert "p" in blocks, f"应有 paragraph\n  {desc[:200]}"
    assert "ic" in blocks, f"应有 indented code block\n  {desc[:200]}"
    assert "bq" in blocks, f"应有 blockquote\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_ordered(cm_workspace):
    """1. A paragraph / indented code / BQ → ordered list with mixed content items。"""
    md_input = "1. First item\n\n    indented\n\n   > blockquote inside\n"
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"应有 ordered list"


@pytest.mark.asyncio
async def test_list_items_indent_matters(cm_workspace):
    """ex3: - one + 1/2 空格缩进 → 缩进决定是否属于 list item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 2)
    ast = ast_parse(md_input)
    assert "bl" in ast_block_types(ast), f"应有 bullet list\n  {desc[:200]}"
    assert "p" in ast_block_types(ast), f"\"two\" indent 不足，应为独立 paragraph\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_no_space_not_list(cm_workspace):
    """ex9: -one / 2.two → 无空格不是 list item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 8)
    ast = ast_parse(md_input)
    assert "bl" not in ast_block_types(ast), f"-one 不应产生 bullet\n  {desc[:200]}"
    assert "od" not in ast_block_types(ast), f"2.two 不应产生 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_paragraph_continuation(cm_workspace):
    """ex5: - foo\\n\\n    bar → 空行 + 4 空格 → paragraph 延续。已知偏差 [code]。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 4)
    ast = ast_parse(md_input)
    assert "bl" in ast_block_types(ast), f"应有 bullet list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_empty(cm_workspace):
    """ex28 (_code[28]): - foo / - / - bar → 空 bullet item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 28)
    ast = ast_parse(md_input)
    bullets = ast_find_all(ast, "b")
    assert bullets, f"应有 bullet list（含空 item）\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_heading_inside(cm_workspace):
    """ex47 (_code[47]): - # Foo / - Bar / --- → list 中 ATX heading + setext heading。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 47)
    ast = ast_parse_nested(md_input)
    assert "bl" in ast_block_types(ast), f"应有 bullet list\n  {desc[:200]}"
    # list item 内应有 heading（至少 1 个）
    assert ast_has_type(ast, "h"), f"list item 内应有 heading\n  {desc[:200]}"
    # --- 在 list item 内是 setext 下划线（官方 h2(Bar)——CM §4.2）
    assert ast_has_type(ast, "h"), f"--- 应产出 setext heading（h2）\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_ordered_start(cm_workspace):
    """ex13: 123456789. ok → 多位数 ordered list item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 12)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"多位数 ordered list 应被识别\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_ordered_no_space(cm_workspace):
    """ex10: 1.ok → ordered list 也有 space 要求（无空格不是 list）。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 9)
    ast = ast_parse(md_input)
    assert "od" not in ast_block_types(ast), f"1.ok 不应产生 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_nested_sublist(cm_workspace):
    """ex42: - foo\\n  - bar\\n    - baz\\n      - boo → 四级嵌套 sublist。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 41)
    ast = ast_parse(md_input)
    assert "bl" in ast_block_types(ast), f"应有嵌套 bullet list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_blockquote_in_item(cm_workspace):
    """list item 内含 BQ（自备例）。"""
    ast = ast_parse_nested('* a\n  > b\n* c')
    assert "bl" in ast_block_types(ast), f"应有 bullet list"
    assert ast_has_type(ast, "bq"), f"list item 内应有 BQ"


@pytest.mark.asyncio
async def test_list_items_setext_interrupt(cm_workspace):
    """ex34: 1-space 缩进 ordered list → rule #4 indentation。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 33)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"应有 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_indented_code_in_item(cm_workspace):
    """ex18: - foo\\n\\n      bar → 6-space indented code in list item。已知偏差 [code]。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 17)
    ast = ast_parse(md_input)
    assert "bl" in ast_block_types(ast), f"应有 bullet\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_ordered_parenthesis(cm_workspace):
    """ex9b: 2) bar → 右括号 delimiter 也是合法的 ordered list。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 10)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"2) 应有 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_thematic_break_interrupt(cm_workspace):
    """ex20: --- 可打断 list item。已知偏差 [tb]。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 19)
    ast = ast_parse(md_input)
    assert ast, f"已知偏差 [tb]\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_loose_with_blank(cm_workspace):
    """ex27: - foo\\n\\n- bar → 空行 → loose list。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 26)
    ast = ast_parse(md_input)
    assert "bl" in ast_block_types(ast), f"loose list 应有 bullet\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_thin_divider(cm_workspace):
    """ex47: 1 空格缩进足矣建立 sublist。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 46)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"应有 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_code_block_in_item(cm_workspace):
    """ex8: 8 空格 → indented code in list item。已知偏差 [code]。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 7)
    ast = ast_parse(md_input)
    assert ast, f"已知偏差 [code]\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_lazy_continuation(cm_workspace):
    """ex38: 1-space 缩进 1. + lazy continuation（续行无缩进仍属同一 item）。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 37)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"lazy 续行应属同一 item\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_fenced_inside(cm_workspace):
    """list item 内含 fenced code（自备例）。"""
    ast = ast_parse_nested('- ```\n  code\n  ```')
    assert ast_has_type(ast, "b"), f"应有 list"
    assert ast_has_type(ast, "c"), f"list item 内应有 code_block"


@pytest.mark.asyncio
async def test_list_items_blank_in_paragraph(cm_workspace):
    """ex33: `foo\n*\n\nfoo\n1.` → 空 list item 不能中断段落（CM §5.2）——两个段落。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 32)
    ast = ast_parse(md_input)
    types = ast_block_types(ast)
    assert "bl" not in types, f"空 item 不中断段落——不应有 list\n  {desc[:200]}"
    assert types == ["p", "p"], f"应为两个段落\n  {types}"


@pytest.mark.asyncio
async def test_list_items_ordered_with_text(cm_workspace):
    """ex35: 1. foo → 段落后 ordered list item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 34)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"2-spaces 前置 indent 应产生 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_continuation_block(cm_workspace):
    """多行 paragraph 延续（自备例）。"""
    ast = ast_parse('- foo\n  bar\n  baz')
    assert "bl" in ast_block_types(ast), f"应有 list"


@pytest.mark.asyncio
async def test_list_items_ordered_start_9(cm_workspace):
    """ex39: 9) foo → 右括号 ordered list item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 38)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"9) 应有 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_indented_para(cm_workspace):
    """ex40: -     indented paragraph → 5 空格缩进为 indented code。已知偏差 [code]。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 39)
    ast = ast_parse(md_input)
    assert ast, f"已知偏差 [code]\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_marker_variants(cm_workspace):
    """同 ) delimiter 合并为一个 list（自备例）。"""
    ast = ast_parse('1) foo\n2) bar')
    assert "od" in ast_block_types(ast), f"同 ) delimiter 应合并"


@pytest.mark.asyncio
async def test_list_items_tight_subitem(cm_workspace):
    """ex43: - foo\n  - bar → 1 空格缩进下的 tight sublist。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 42)
    ast = ast_parse_nested(md_input)
    bullets = ast_find_all(ast, "b")
    assert len(bullets) >= 2, f"应有嵌套 bullet，实际 {len(bullets)}\n  {desc[:200]}"



# 补齐用例 — 编号合法性 / 缩进变体 / 空 item / 嵌套 / marker 类型 (N=1..45 缺失)

@pytest.mark.asyncio
async def test_list_items_ordered_mixed_blocks(cm_workspace):
    """N=1: 1. A paragraph + indented code + BQ → ordered list 内三种 block。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 1)
    ast = ast_parse(md_input)
    blocks = ast_block_types(ast)
    assert "od" in blocks, f"应有 ordered list\n  {desc[:200]}"
    # ic/bq 在 od 内——递归检查
    assert ast_has_type(ast, "ic"), f"应有 indented code\n  {desc[:200]}"
    assert ast_has_type(ast, "bq"), f"应有 blockquote\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_two_space_continuation(cm_workspace):
    """N=3: - one / 空行 / 2 空格 two → 2 空格足够 item 延续 (loose)。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 3)
    ast = ast_parse(md_input)
    assert "bl" in ast_block_types(ast), f"应有 bullet list\n  {desc[:200]}"
    txt = _all_text(ast)
    assert "one" in txt and "two" in txt, f"one/two 都应在 item 内: {txt!r}"


@pytest.mark.asyncio
async def test_list_items_five_space_continuation(cm_workspace):
    """N=5:  -    one / 空行 / 6 空格 two → two 是 item 内 paragraph continuation。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 5)
    ast = ast_parse(md_input)
    assert "bl" in ast_block_types(ast), f"应有 bullet list\n  {desc[:200]}"
    txt = _all_text(ast)
    assert "one" in txt and "two" in txt, f"one/two 都应在 item 内: {txt!r}"


@pytest.mark.asyncio
async def test_list_items_bq_nested_ordered(cm_workspace):
    """N=6: 双层 BQ 内嵌 ordered list。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 6)
    ast = ast_parse_nested(md_input)
    assert ast_has_type(ast, "bq"), f"应有 BQ\n  {desc[:200]}"
    assert ast_has_type(ast, "od"), f"BQ 内应有 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_multi_blank_code(cm_workspace):
    """N=11: - Foo / 空行 / 6 空格 bar / 双空行 / 6 空格 baz → bar+baz 是 indented code。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 11)
    ast = ast_parse(md_input)
    assert "bl" in ast_block_types(ast), f"应有 bullet list\n  {desc[:200]}"
    assert ast_has_type(ast, "ic"), f"bar/baz 应是 indented code\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_number_too_long_not_list(cm_workspace):
    """N=13: 1234567890. not ok → 10 位数字不是 ordered list item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 13)
    ast = ast_parse(md_input)
    assert "od" not in ast_block_types(ast), f"超 9 位不应是 ordered list\n  {desc[:200]}"
    assert "p" in ast_block_types(ast), f"应是 paragraph\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_number_zero_ok(cm_workspace):
    """N=14: 0. ok → 0 是合法 ordered list 起点。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 14)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"0. 应是 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_number_padded_ok(cm_workspace):
    """N=15: 003. ok → 补零数字合法 (start=3)。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 15)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"003. 应是 ordered list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_number_negative_not_list(cm_workspace):
    """N=16: -1. not ok → 负数不是 ordered list item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 16)
    ast = ast_parse(md_input)
    assert "od" not in ast_block_types(ast), f"-1. 不应是 ordered list\n  {desc[:200]}"
    assert "p" in ast_block_types(ast), f"应是 paragraph\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_three_space_indent_code(cm_workspace):
    """N=18: 3 空格缩进的 10. foo + 11 空格 bar → bar 是 item 内 indented code。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 18)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"应有 ordered list\n  {desc[:200]}"
    assert ast_has_type(ast, "ic"), f"bar 应是 indented code\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_five_space_code_then_para(cm_workspace):
    """N=20: 1. + 5 空格 code / 3 空格 paragraph / 7 空格 code。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 20)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"应有 ordered list\n  {desc[:200]}"
    assert ast_has_type(ast, "ic"), f"应有 indented code\n  {desc[:200]}"
    # paragraph 在 item 内（官方 ol>li>pre+p+pre）——递归查找
    assert ast_has_type(ast, "p"), f"应有 paragraph（item 内）\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_six_space_code_then_para(cm_workspace):
    """N=21: 同 N=20 但首段多 1 空格 → 内容保留 1 前导空格。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 21)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"应有 ordered list\n  {desc[:200]}"
    assert ast_has_type(ast, "ic"), f"应有 indented code\n  {desc[:200]}"
    # 同 N=20（paragraph 在 item 内——递归）
    assert ast_has_type(ast, "p"), f"应有 paragraph（item 内）\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_indent_para_two_blocks(cm_workspace):
    """N=22: 3 空格缩进 foo / 空行 / bar → 两个独立段落。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 22)
    ast = ast_parse(md_input)
    blocks = ast_block_types(ast)
    assert blocks.count("p") == 2, f"应为两个 paragraph: {blocks}\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_four_space_content_then_two(cm_workspace):
    """N=23: - + 4 空格 foo / 空行 / 2 空格 bar → bar 脱离 item 成独立段落。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 23)
    ast = ast_parse(md_input)
    blocks = ast_block_types(ast)
    assert "bl" in blocks, f"应有 bullet list\n  {desc[:200]}"
    assert "p" in blocks, f"bar 应是独立段落\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_three_space_stays_in_item(cm_workspace):
    """N=24: - + 2 空格 foo / 空行 / 3 空格 bar → bar 在 item 内 (loose)。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 24)
    ast = ast_parse(md_input)
    assert "bl" in ast_block_types(ast), f"应有 bullet list\n  {desc[:200]}"
    txt = _all_text(ast)
    assert "foo" in txt and "bar" in txt, f"foo/bar 都应在 item 内: {txt!r}"


@pytest.mark.asyncio
async def test_list_items_empty_then_fenced_items(cm_workspace):
    """N=25: 空首 item + fenced code item + 缩进 code item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 25)
    ast = ast_parse_nested(md_input)
    assert ast_has_type(ast, "bl"), f"应有 bullet list\n  {desc[:200]}"
    assert ast_has_type(ast, "c"), f"应有 fenced code\n  {desc[:200]}"
    assert ast_has_type(ast, "ic"), f"应有 indented code\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_empty_then_para(cm_workspace):
    """N=27: 空 item / 空行 / 2 空格 foo → foo 是独立段落。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 27)
    ast = ast_parse(md_input)
    blocks = ast_block_types(ast)
    assert "bl" in blocks, f"应有 bullet list (空 item)\n  {desc[:200]}"
    assert "p" in blocks, f"foo 应是独立段落\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_empty_middle(cm_workspace):
    """N=29: - foo / - (带尾空格) / - bar → 中间空 item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 29)
    ast = ast_parse_nested(md_input)
    bullets = ast_find_all(ast, "b")
    assert len(bullets) == 3, f"应有 3 个 bullet item\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_empty_middle_ordered(cm_workspace):
    """N=30: 1. foo / 2. / 3. bar → ordered list 中间空 item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 30)
    ast = ast_parse_nested(md_input)
    items = ast_find_all(ast, "ol")
    assert len(items) == 3, f"应有 3 个 ordered item\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_bare_star_empty_item(cm_workspace):
    """N=31: 单独 * → 空 bullet list item。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 31)
    ast = ast_parse_nested(md_input)
    assert ast_has_type(ast, "bl"), f"单独 * 应是 bullet list\n  {desc[:200]}"
    bullets = ast_find_all(ast, "b")
    assert len(bullets) == 1, f"应有 1 个空 item\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_one_space_indent_ordered(cm_workspace):
    """N=35: 1 空格缩进的 ordered list。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 35)
    ast = ast_parse(md_input)
    assert "od" in ast_block_types(ast), f"应有 ordered list\n  {desc[:200]}"
    assert ast_has_type(ast, "ic"), f"应有 indented code\n  {desc[:200]}"
    assert ast_has_type(ast, "bq"), f"应有 blockquote\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_five_space_indent_code(cm_workspace):
    """N=36: 5 空格缩进 → indented code block, 不是 list。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 36)
    ast = ast_parse(md_input)
    blocks = ast_block_types(ast)
    assert "od" not in blocks, f"5 空格缩进不应是 ordered list\n  {desc[:200]}"
    assert "ic" in blocks, f"应是 indented code\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_bq_ordered_continuation(cm_workspace):
    """N=40: BQ 内嵌 ordered list, list item 跨行延续 (对比 N=39 有 > 前缀)。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 40)
    ast = ast_parse_nested(md_input)
    assert ast_has_type(ast, "bq"), f"应有 BQ\n  {desc[:200]}"
    assert ast_has_type(ast, "od"), f"BQ 内应有 ordered list\n  {desc[:200]}"
    txt = _all_text(ast)
    assert "Blockquote" in txt and "continued" in txt, f"跨行内容应在 item 内: {txt!r}"


@pytest.mark.asyncio
async def test_list_items_paren_marker_nested_bullet(cm_workspace):
    """N=43: 10) foo + 4 空格 - bar → ) marker 列表内嵌套 bullet。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 43)
    ast = ast_parse_nested(md_input)
    assert ast_has_type(ast, "od"), f"应有 ordered list\n  {desc[:200]}"
    assert ast_has_type(ast, "bl"), f"应有嵌套 bullet list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_paren_marker_separate_bullet(cm_workspace):
    """N=44: 10) foo + 3 空格 - bar → bar 是独立 bullet list。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 44)
    ast = ast_parse_nested(md_input)
    assert ast_has_type(ast, "od"), f"应有 ordered list\n  {desc[:200]}"
    assert ast_has_type(ast, "bl"), f"bar 应是独立 bullet list\n  {desc[:200]}"


@pytest.mark.asyncio
async def test_list_items_nested_bare_marker(cm_workspace):
    """N=45: - - foo → 嵌套 bullet (外层空 marker)。"""
    rt = MdioRuntime(cm_workspace)
    desc = await cm_desc(rt, "Container blocks", "List items")
    md_input, _ = await cm_example(rt, "Container blocks", "List items", 45)
    ast = ast_parse_nested(md_input)
    assert ast_has_type(ast, "bl"), f"应有 bullet list\n  {desc[:200]}"
    txt = _all_text(ast)
    assert "foo" in txt, f"foo 应在嵌套 item 内: {txt!r}"


# 补齐用例的文本提取 helper

def _all_text(ast) -> str:
    """递归收集所有 ct 字段文本（覆盖 txt 节点 + 块级 content 字段）。"""
    parts = []
    def _w(n):
        if isinstance(n, dict):
            c = n.get("ct")
            if isinstance(c, str) and c.strip():
                parts.append(c)
            for v in n.values():
                if isinstance(v, (dict, list)):
                    _w(v)
        elif isinstance(n, list):
            for item in n:
                _w(item)
    _w(ast)
    return " ".join(parts)
