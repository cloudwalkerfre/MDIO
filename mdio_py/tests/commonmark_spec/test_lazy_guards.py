"""差异集守卫测试：lazy 打断、约束链、item lazy、RefDef 后 lazy、列表词法盲区。

覆盖：
- 列⑥ lazy 打断（thematic 4 缩进 / 标题 / number-1）
- 列① 约束链（Comment/HtmlBlock 吞内容、HtmlBlock 空行吞）
- 列② item lazy / 嵌套 marker / Inside 段落
- 列③ RefDef 后 lazy
- 列⑤ 列表词法盲区（RefDef/表格后 lazy）
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace  # noqa: F401
from .ast_helpers import ast_parse_nested, ast_has_type


def _txts(ast):
    r = []
    def _w(n):
        if isinstance(n, dict) and n.get("nt") == "txt":
            c = n.get("ct","")
            if c: r.append(c)
        if isinstance(n, dict):
            # 块叶节点的 ct 按行拆(断言单行文本归属);ct 双型:字符串拆行 /
            # 数组(内容流/拆段)逐元素(字符串直接收,字典递归)。
            ct = n.get("ct")
            if ct and n.get("nt") not in ("txt","sb","hr"):
                if isinstance(ct, str):
                    for line in ct.split('\n'):
                        line = line.strip()
                        if line: r.append(line)
                elif isinstance(ct, list):
                    for it in ct:
                        if isinstance(it, str):
                            if it.strip(): r.append(it.strip())
                        elif isinstance(it, dict):
                            _w(it)
            for v in n.values():
                if isinstance(v, (dict, list)): _w(v)
        elif isinstance(n, list):
            for c in n: _w(c)
    _w(ast); return r


def _all(node, node_type):
    results = []
    if isinstance(node, dict) and node.get("nt") == node_type:
        results.append(node)
    if isinstance(node, dict):
        for v in node.values():
            if isinstance(v, (list, dict)):
                results.extend(_all(v, node_type))
    elif isinstance(node, list):
        for c in node:
            results.extend(_all(c, node_type))
    return results


def _bq_direct_paragraphs(ast):
    """BQ 直接子级的段落节点（不含列表内）——检查 lazy 行是否被 BQ 误吸。"""
    bq = _all(ast, "bq")[0]
    return [c for c in bq.get("ct", []) if isinstance(c, dict) and c.get("nt") == "p"]


def _top_nodes(ast):
    return [n for n in ast if isinstance(n, dict)]


# 列⑥ lazy 打断

@pytest.mark.asyncio
async def test_lazy_interrupt_thematic_0indent(cm_workspace):
    """`> foo\n---` — thematic 打断（BQ 外 hr）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n---")
    assert ast_has_type(ast, "hr"), "--- 应打断为 BQ 外 thematic break"
    assert "---" not in _txts(_all(ast, "bq")[0]), "--- 不应是 BQ 文本"

@pytest.mark.asyncio
async def test_item_after_thematic_in_bq(cm_workspace):
    """`> - item\n> ---` — setext 下划线需紧跟
    段落文本行（CM §4.2——item 行非段落）——--- 是 thematic（非 setext）——
    prev_para 粒度（含 Blockquote 不含 Bullet）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> - item\n> ---")
    assert ast_has_type(ast, "hr"), "--- 在 item 后应是 thematic break"
    assert not ast_has_type(ast, "h"), "--- 不应是 setext 下划线（item 行非段落）"

@pytest.mark.asyncio
async def test_lazy_interrupt_thematic_4indent(cm_workspace):
    """`> foo\n    ---` — 4 缩进 thematic 是 IC 形态（CM thematic
    开行限缩进 ≤3）——应 lazy 段落文本。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n    ---")
    assert "---" in _txts(_all(ast, "bq")[0]), \
        "4 缩进 --- 应是 BQ 段落文本（IC 形态不能中断段落——CM §4.1）"
    assert not ast_has_type(ast, "ic"), "不应打断成顶层 IC"

@pytest.mark.asyncio
async def test_lazy_interrupt_heading(cm_workspace):
    """`> foo\n# h` — ATX 标题可中断段落（CM §4.2）——应打断。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n# h")
    assert ast_has_type(ast, "h"), "# h 应打断为标题（ATX 可中断段落）"

@pytest.mark.asyncio
async def test_lazy_interrupt_ordered_1(cm_workspace):
    """`> foo\n1. bar` — number-1 规则（CM §5.3：1. 可打断段落）——
    应打断为 BQ 外 OL。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n1. bar")
    assert ast_has_type(ast, "ol"), "1. bar 应打断为 BQ 外有序列表（number-1）"

@pytest.mark.asyncio
async def test_lazy_interrupt_ordered_2(cm_workspace):
    """`> foo\n2. bar` — number≠1 不能打断段落（CM §5.3）——
    lazy 段落文本。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n2. bar")
    assert "2. bar" in _txts(_all(ast, "bq")[0]), "2. bar 应是 BQ 段落文本"
    assert not ast_has_type(ast, "ol"), "不应打断（number≠1）"

# 列① 约束链 — Comment/HtmlBlock 内容行无守卫（吞内容——数据丢失级）

@pytest.mark.asyncio
async def test_comment_content_guard_bq(cm_workspace):
    """`> <!--\n> x\nbar` — Comment 内容行无守卫——bar 会被吞
    （数据丢失）——bar 应是顶层段落。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> <!--\n> x\nbar")
    assert any("bar" in _txts(n) for n in _all(ast, "p")), \
        "bar 不能被吞（Comment 内容行需守卫——CM §4.6）"

@pytest.mark.asyncio
async def test_htmlblock_content_guard_bq(cm_workspace):
    """`> <div>\n> x\nbar` — HtmlBlock(type 6) 内容行无守卫——
    bar 会被吞（数据丢失）——bar 应是顶层段落。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> <div>\n> x\nbar")
    hbs = _all(ast, "hb")
    assert hbs, "应有 HtmlBlock"
    assert not any("bar" in _txts(h) for h in hbs), "bar 不应被 HtmlBlock 吞"
    assert any("bar" in _txts(n) for n in _all(ast, "p")), "bar 应是段落"

@pytest.mark.asyncio
async def test_htmlblock_blank_guard_bq(cm_workspace):
    """`> <script>\n> x\n\nbar` — type 1-5 空行被 HtmlBlock 吞
    （数据丢失）——空行应结束容器链——bar 顶层段落。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> <script>\n> x\n\nbar")
    hbs = _all(ast, "hb")
    assert hbs, "应有 HtmlBlock"
    assert not any("bar" in _txts(h) for h in hbs), "bar 不应在 HtmlBlock 内容里（空行应结束容器链）"

@pytest.mark.asyncio
async def test_comment_content_guard_list(cm_workspace):
    """`- <!--\n  x\nbar` — 列表内 Comment 内容行无守卫——
    bar 会被吞——bar 应是顶层段落。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("- <!--\n  x\nbar")
    assert any("bar" in _txts(n) for n in _all(ast, "p")), \
        "bar 不能被吞（列表内 Comment 内容行需守卫）"

@pytest.mark.asyncio
async def test_htmlblock_content_guard_list(cm_workspace):
    """`- <div>\n  x\nbar` — 列表内 HtmlBlock 内容行无守卫——
    bar 会被吞——bar 应是顶层段落。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("- <div>\n  x\nbar")
    hbs = _all(ast, "hb")
    assert hbs, "应有 HtmlBlock"
    assert not any("bar" in _txts(h) for h in hbs), "bar 不应被 HtmlBlock 吞"
    assert any("bar" in _txts(n) for n in _all(ast, "p")), "bar 应是段落"

# 列② item lazy / 嵌套 marker / Inside 段落

@pytest.mark.asyncio
async def test_item_lazy_in_bq(cm_workspace):
    """`> - item\nlazy` — 0 缩进 lazy 行应并入 item 段落
    （CM §5.2 最近段落）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> - item\nlazy")
    assert not any("lazy" in _txts(p) for p in _bq_direct_paragraphs(ast)), \
        "lazy 不应是 BQ 直接子级段落（应并入 item）"
    assert any("lazy" in _txts(b) for b in _all(ast, "b")), "lazy 应在 item 段落内"

@pytest.mark.asyncio
@pytest.mark.xfail(reason="known divergence: 2-space nested marker inside BQ not yet supported", strict=False)
async def test_nested_marker_in_bq(cm_workspace):
    """`> - foo\n  - bar` — 2 缩进 marker（Inside）应嵌套在 item
    内（嵌套列表——数据归属级）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> - foo\n  - bar")
    assert any(ast_has_type(b, "bl") for b in _all(ast, "b")), \
        "`  - bar` 应嵌套在 item 内（Inside marker——list_lazy_candidate 豁免）"

@pytest.mark.asyncio
async def test_inside_paragraph_in_bq(cm_workspace):
    """`> - foo\n  bar` — 2 缩进段落（Inside）应并入 item 段落。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> - foo\n  bar")
    assert not any("bar" in _txts(p) for p in _bq_direct_paragraphs(ast)), \
        "bar 不应是 BQ 直接子级段落（应并入 item）"
    assert any("bar" in _txts(b) for b in _all(ast, "b")), "bar 应在 item 内"

# 列③ RefDef 后 lazy（BQ）— lazy 只覆盖段落

@pytest.mark.asyncio
async def test_bq_refdef_lazy(cm_workspace):
    """`> [a]: b\nlazy` — RefDef 不是段落（CM §5.1 lazy 只覆盖
    段落）——lazy 应结束 BQ 成顶层段落。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> [a]: b\nlazy")
    assert not any("lazy" in _txts(p) for p in _bq_direct_paragraphs(ast)), \
        "lazy 不应是 BQ 段落（RefDef 后无 lazy continuation）"
    assert any(n.get("nt") == "p" and "lazy" in _txts(n) for n in _top_nodes(ast)), \
        "lazy 应是顶层段落"

# 列⑤ 列表词法盲区（RefDef/表格后 lazy）

@pytest.mark.asyncio
async def test_list_refdef_lazy(cm_workspace):
    """`- a\n  [a]: b\nbar` — RefDef 后无段落——bar 应列表结束
    成顶层段落（词法 prev 盲区）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("- a\n  [a]: b\nbar")
    assert any(n.get("nt") == "p" and "bar" in _txts(n) for n in _top_nodes(ast)), \
        "bar 应是顶层段落（RefDef 后无 lazy continuation）"

@pytest.mark.asyncio
async def test_list_table_lazy(cm_workspace):
    """`- a\n  | x |\n  |---|\nbar` — 表格后无段落——bar 应列表
    结束成顶层段落（TableSeparator 词法盲区）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("- a\n  | x |\n  |---|\nbar")
    assert any(n.get("nt") == "p" and "bar" in _txts(n) for n in _top_nodes(ast)), \
        "bar 应是顶层段落（表格后无 lazy continuation）"

# 空行结束 BQ（CommonMark：无 > 前缀的空行不弹回）

@pytest.mark.asyncio
async def test_bq_blank_splits_bq(cm_workspace):
    """`> - foo\n\n> - bar` — 无前缀空行结束当前 BQ，后一行开启新 BQ；
    CommonMark 与参考实现一致：两个 BQ，各一个 item。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> - foo\n\n> - bar")
    assert len(_all(ast, "bq")) == 2, "空行应结束 BQ，产生两个 BQ"
    assert len(_all(ast, "b")) == 2, "每个 BQ 各一个 item"

@pytest.mark.asyncio
async def test_bq_blank_two_bqs(cm_workspace):
    """`> foo\n\n> bar` — 段落场景空行无 > 中断 BQ——两个 BQ
    （BQ 顶空行 Pop——现有逻辑保持）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n\n> bar")
    assert len(_all(ast, "bq")) == 2, "应两个 BQ（段落间空行中断）"


# Bug A 最小复现回归

@pytest.mark.asyncio
async def test_bugA_fence_blank_keeps_bar(cm_workspace):
    """`> ```\n> code\n\nbar` — 空行结束
    容器链——bar 必须是独立块（不在 fence 内——cd 只含 code）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> ```\n> code\n\nbar")
    cds = [c.get("cd", "") for c in _all(ast, "c")]
    assert cds, "应有 fence 块"
    assert all("bar" not in cd for cd in cds), "bar 不应在 fence 内（cd）"
    assert any("bar" in _txts(n) for n in _all(ast, "p")), "bar 应是独立段落"

@pytest.mark.asyncio
async def test_refdef_fields(cm_workspace):
    """`[id]: /url/id "定义"` — rd 节点必须
    有 t/u/ti（tag/url/title——Tag/Content 投影对 RefDef）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested('[id]: /url/id "定义"')
    rds = _all(ast, "rd")
    assert rds, "应有 rd 节点"
    rd = rds[0]
    assert rd.get("t") == "id", "t 字段（tag）"
    assert rd.get("u") == "/url/id", "u 字段（url）"
    assert rd.get("ti") == "定义", "ti 字段（title）"

# BQ lazy 上下文的打断差异
# CM §4.6 / §5.2：HTML type 7 / 空列表项 不能打断段落——lazy 类推（加 marker 后是段落
# 延续）——当前 lazy_interrupts 无 type 7 / 空 item 检查（差异）。
# 顶层对照：para_modifiers 已处理。

@pytest.mark.asyncio
async def test_lazy_type7_not_interrupt(cm_workspace):
    """`> foo\n<span>` — HTML type 7 不能打断段落（CM §4.6）
    （`Foo\n<a href="bar">` 例——lazy 类推：加 marker 后是段落延续）——
    应 lazy 文本（hb——lazy_interrupts 的 HtmlBlock→Always 无 type 7 检查）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n<span>")
    assert "<span>" in "".join(_txts(_all(ast, "bq")[0])), \
        "span 应是 BQ 段落文本（type 7 不打断——CM §4.6）"
    assert not ast_has_type(ast, "hb"), "不应打断为 HtmlBlock"

@pytest.mark.asyncio
async def test_lazy_empty_item_not_interrupt(cm_workspace):
    """`> foo\n- ` — 空列表项不能打断段落（CM §5.2）
    （`foo\n*` 例——lazy 类推）——应 lazy 文本（空列表——
    lazy_interrupts 的 IndentLe3 无空 item 检查）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n- ")
    assert "-" in "".join(_txts(_all(ast, "bq")[0])), \
        "- 应是 BQ 段落文本（空 item 不打断——CM §5.2——_txts strip 后为单个 -）"
    assert not ast_has_type(ast, "bl"), "不应打断为列表"

@pytest.mark.asyncio
async def test_top_type7_not_interrupt(cm_workspace):
    """`foo\n<span>` — type 7 不打断（inline——
    apply_para_modifiers 的 type 7 → Paragraph——与 lazy 的差异定位依据）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("foo\n<span>")
    assert not ast_has_type(ast, "hb"), "顶层 type 7 不应打断为 HtmlBlock"
    assert "<span>" in "".join(_txts(_all(ast, "p"))), "span 应是段落文本"

@pytest.mark.asyncio
async def test_top_empty_item_not_interrupt(cm_workspace):
    """`foo\n*` — 空 item 不打断（段落文本——
    classify_at_top 的空 item 检查——与 lazy 的差异定位依据）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("foo\n*")
    assert not ast_has_type(ast, "bl"), "顶层空 item 不应打断为列表"
    assert "*" in "".join(_txts(_all(ast, "p"))), "* 应是段落文本"

# 剩余差距：
#   表格打断（GFM——顶层实测打断——lazy 侧 TableRow Never）
#   01. 数值语义（CM §5.3 例外 1(b)：start number must be 1——数值——顶层
#       parse 数值打断——lazy 侧一位检查拒绝——双实现分裂）
#   4 缩进 fence/html（CM §4.5/4.6 打断开行限缩进 ≤3——Always 无上限——
#       4 缩进是 IC 形态——不能打断段落）

@pytest.mark.asyncio
async def test_lazy_interrupt_table(cm_workspace):
    """`> foo\n| a |\n|---|` — 表格可打断段落（GFM——顶层
    实测打断成 tb——容器内语义保持）——应打断为 BQ 外
    表格——TableRow lazy_interrupt=Never——lazy 文本。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n| a |\n|---|")
    assert ast_has_type(ast, "tb"), "表格行应打断（GFM——顶层对称）"

@pytest.mark.asyncio
async def test_lazy_interrupt_number_01(cm_workspace):
    """`> foo\n01. bar` — CM §5.3 例外 1(b)：start number
    must be 1——`01.` 的 start number 是 1（数值——顶层 parse 数值——
    打断成 od）——lazy 侧 ordered_number_is_one 一位检查（01. 两位——拒绝）
    ——应打断。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n01. bar")
    assert ast_has_type(ast, "od"), "01. 应打断（CM §5.3 start number=1——数值）"

@pytest.mark.asyncio
async def test_lazy_4indent_fence_not_interrupt(cm_workspace):
    """`> foo\n    ```` ` — CM §4.5 fence 开行限缩进 ≤3——
    4 缩进是 IC 形态——不能打断段落——应 lazy 文本（Always 表条目无
    缩进上限——会打断成顶层 IC）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n    ```\n    x")
    assert not ast_has_type(ast, "ic"), "4 缩进 fence 不应打断成 IC（CM §4.5）"
    assert "```" in "".join(_txts(_all(ast, "bq")[0])), "应 lazy 文本"

@pytest.mark.asyncio
async def test_lazy_4indent_html_not_interrupt(cm_workspace):
    """`> foo\n    <div>` — CM §4.6 HTML 开行限缩进 ≤3——
    应 lazy 文本（4 缩进不应打断成顶层 IC）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("> foo\n    <div>")
    assert not ast_has_type(ast, "ic"), "4 缩进 html 不应打断成 IC（CM §4.6）"
    assert "<div>" in "".join(_txts(_all(ast, "bq")[0])), "应 lazy 文本"

@pytest.mark.asyncio
async def test_top_table_interrupt(cm_workspace):
    """`foo\n| a |\n|---|` — 顶层表格打断成 tb。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("foo\n| a |\n|---|")
    assert ast_has_type(ast, "tb"), "顶层表格应打断"

@pytest.mark.asyncio
async def test_top_01_interrupt(cm_workspace):
    """`foo\n01. bar` — 顶层 01. 打断成 od（parse
    数值==1——CM start number 语义——顶层基准）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("foo\n01. bar")
    assert ast_has_type(ast, "od"), "顶层 01. 应打断（start number=1）"

@pytest.mark.asyncio
async def test_top_4indent_fence_lazy(cm_workspace):
    """`foo\n    ```` ` — 顶层 4 缩进 fence 是 lazy
    段落文本（IC downgrade——classify_at_top——几何先捕获）。"""
    rt = MdioRuntime(cm_workspace)
    ast = ast_parse_nested("foo\n    ```\n    x")
    assert not ast_has_type(ast, "ic"), "顶层 4 缩进 fence 不应打断成 IC"
    assert "```" in "".join(_txts(_all(ast, "p"))), "应 lazy 段落文本"
