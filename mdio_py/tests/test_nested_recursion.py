"""嵌套递归语义测试 — 区域律（header/bullet 同律）下，`..`（递归钻取/通配）
递归钻入所有容器（列表 / BQ / 深层嵌套），嵌套 bullet 是独立 ItemLeaf，且每个
item 节点的 span = 区域（marker 行 + 内容 + 嵌套子树）——content = 区域 − ctn
层，与 header 的章节正文同律（父含子 + 子独立枚举 = 树真相）。"""

from mdio_py import MdioRuntime
from mdio_py.core import parse_code
from mdio_py.core.selector_builder import SelectorBuilder

NESTED_MD = """# CTX
## data
- top_key: top
    - nest_key: nest
        - deep_key: deep
> bq_text
> - bq_item: 1
>   - bq_nest: 2
"""


def _vfs_write(store, vpath, content):
    store.mk("genesis", vpath, parents=True, verbose=False)
    store.write_locator("genesis", vpath, SelectorBuilder().build(), "/>", content,
                        auto_scaffold=True, god=True)


def _reader(runtime, vpath):
    def read(selector, proj=None):
        ir = parse_code(selector)[0].atoms[0]
        raw = runtime.store.read_locator("genesis", vpath, ir.selector, proj=proj)
        return [s.strip() for s in raw]
    return read


async def test_recursive_drill_into_nested_list(runtime, tmp_path):
    """`..` 递归钻取命中嵌套列表 item；item ct = 区域 − ctn 层（带子树）。"""
    vpath = "/nested_drill.md"
    _vfs_write(runtime.store, vpath, NESTED_MD)
    read = _reader(runtime, vpath)

    cases = [
        # (selector, proj, expected) — content = region − ctn 层（区域律）
        ("$CTX..nest_key", "content", ['nest\n      - deep_key: deep']),   # L2 含子树
        ("$CTX..deep_key", "content", ['deep']),                           # L3 叶子
        ("$CTX..bq_item", "content", ['1\n- bq_nest: 2']),                 # BQ 内列表含子树
    ]
    for sel, proj, exp in cases:
        got = read(sel, proj)
        assert got == exp, f"{sel}: got {got!r}, expected {exp!r}"


async def test_recursive_wildcard_all_bullets(runtime, tmp_path):
    """`..*[bullet]` 递归通配应返回全部 5 个 bullet（含嵌套，当前只返回顶层）。"""
    vpath = "/nested_wildcard.md"
    _vfs_write(runtime.store, vpath, NESTED_MD)
    read = _reader(runtime, vpath)

    bullets = read("$CTX..*[bullet]", "content")
    assert len(bullets) == 5, f"expected 5 bullets, got {len(bullets)}: {bullets}"
    keys = ['top', 'nest', 'deep', '1', '2']
    for k in keys:
        assert any(k in b for b in bullets), f"missing bullet value {k!r}: {bullets}"


async def test_top_level_bullet_region_contains_subtree(runtime, tmp_path):
    """区域律：顶层 bullet 的 content = 区域 − ctn 层——含嵌套子树
    （与 header 章节正文同律——父含子 + 子独立枚举 = 树真相）。"""
    vpath = "/nested_region.md"
    _vfs_write(runtime.store, vpath, NESTED_MD)
    read = _reader(runtime, vpath)

    got = read("$CTX..top_key", "content")
    assert got == ['top\n  - nest_key: nest\n      - deep_key: deep'], \
        f"top_key region content should contain subtree, got {got!r}"


async def test_old_flat_expectation_flips(runtime, tmp_path):
    """旧 flat 语义的"钻不进嵌套"断言应在新语义下翻转。

    对应现有 test_selector.py 测试 33（$CTX..metadata._b:invisible → []）
    的旧期望：metadata 在嵌套列表内，旧语义找不到 → []；
    新语义应命中嵌套 bullet。
    """
    vpath = "/nested_flip.md"
    _vfs_write(runtime.store, vpath, NESTED_MD)
    read = _reader(runtime, vpath)

    got = read("$CTX..nest_key", "content")
    assert got == ['nest\n      - deep_key: deep'], f"nested bullet must be found, got {got!r}"


async def test_path_independent_span(runtime, tmp_path):
    """路径无关性：同一 item 经 `..` / `..*` / `.*` 的 content 全等
    （区域律单一事实——span 不依赖到达路径）。"""
    vpath = "/path_indep.md"
    _vfs_write(runtime.store, vpath, NESTED_MD)
    read = _reader(runtime, vpath)

    via_recursive = read("$CTX..nest_key", "content")
    via_wildcard = read("$CTX..*[.tag == 'nest_key']", "content")
    via_direct = read("$CTX.data.*[.tag == 'top_key']", "content")
    via_drill = read("$CTX..top_key", "content")
    assert via_recursive == via_wildcard == ['nest\n      - deep_key: deep'], \
        f"recursive paths diverge: {via_recursive!r} vs {via_wildcard!r}"
    assert via_direct == via_drill == ['top\n  - nest_key: nest\n      - deep_key: deep'], \
        f"direct vs drill diverge: {via_direct!r} vs {via_drill!r}"


MATRIX_MD = """# S
> - a: 1
>   # H_in
>   - b: 2
"""


async def test_bq_list_header_face_matrix(runtime, tmp_path):
    """验收场景：BQ ⊃ BLT ⊃ header 五面（区域律 + 分层剥 + A2 同线双身份）。

    scan 结构事实（现状）：S ⊃ [BQ1(a), p(H_in), BQ2(b)]——两个 BQ 帧
    （H_in 中断 tight 列表，BQ 在 b 处重开）。A2 已修复：BQ 壳开行的
    item 身份同线恢复——`..*` 壳与 bullet 各自独立枚举、`..a` 可 drill。
    """
    vpath = "/face_matrix.md"
    _vfs_write(runtime.store, vpath, MATRIX_MD)
    read = _reader(runtime, vpath)

    # S：章节区——raw 全文；ct = 正文
    assert read("$S", None) == ['# S\n> - a: 1\n>   # H_in\n>   - b: 2']
    assert read("$S", "content") == ['- a: 1\n  # H_in\n  - b: 2']

    # BQ：容器区——单一壳（帧尾 = 文档尾——未闭合帧补结算）；ct 剥单层
    # = 有效内层 MD（A2 已收敛：壳与 item 不再分裂为两个 BQ）
    assert read("$S..*[bq]", None) == ['> - a: 1\n>   # H_in\n>   - b: 2']
    assert read("$S..*[bq]", "content") == ['- a: 1\n# H_in\n- b: 2']

    # A2：复合开行的 item 身份——bullet 独立枚举 + drill 可达
    assert read("$S..*[bullet]", None) == ['> - a: 1', '>   - b: 2']
    assert read("$S..*[bullet]", "content") == ['1', '2']
    assert read("$S..a", "content") == ['1']
    assert read("$S..b", "content") == ['2']

    # H_in：修点 4 后 = 标题（cmark：li 内 h1），区域律——raw/ct = 自身标题
    assert read("$S..*[head]", None) == ['>   # H_in\n>   - b: 2']
    assert read("$S..*[head]", "content") == ['- b: 2']


DEEP_MD = """# H1
## H2
> # H3
> - B1:
>   - B2:
>      > # H4
>      > - B3:
>      >   # H5
>      >   - B4:
>      >     - B5:  hi
"""


async def test_deep_cross_nesting_navigation(runtime, tmp_path):
    """H1→B5 跨嵌套导航（section/BQ/BLT/item 任意组合——通用枚举，
    零 per-structure 特判）。NestedResolver 收编后 B3/B4/B5/H5 均按 cmark
    终态挂在 B3 子树内，不再有 BQ 重开断链。"""
    vpath = "/deep_nav.md"
    _vfs_write(runtime.store, vpath, DEEP_MD)
    read = _reader(runtime, vpath)

    # B1/B2 的 item 身份已恢复（A2）；H4 标题独立枚举——从文档根递归
    # （H1 章节区间 = [H1, H3)——B1/B2 在章节外，须根级递归）
    raw_bullets = read("$..*[bullet]", None)
    assert any("> - B1:" in r for r in raw_bullets)
    assert any(">   - B2:" in r for r in raw_bullets)
    assert len(raw_bullets) == 5, f"B1..B5 all bullets: {raw_bullets}"
    heads = read("$..*[head]", None)
    assert any("H4" in h for h in heads)
    assert any("H5" in h for h in heads)
    # 结构断言：B4/B5 与 H5 都在 B3 子树内（cmark oracle）。
    b3_bullets = read("$..B3..*[bullet]", None)
    assert any(">      >   - B4:" in b for b in b3_bullets), b3_bullets
    assert any(">      >     - B5:  hi" in b for b in b3_bullets), b3_bullets
    assert any("H5" in h for h in read("$..B3..*[head]", None))

    # 深层嵌套的 selector 路径：直接子/递归钻取/递归通配都要可达。
    h3 = read("$H1.H2..H3", None)
    assert len(h3) == 1 and h3[0].startswith("> # H3") and "B5:  hi" in h3[0], h3
    assert len(read("$H1.H2..*", None)) >= 10
    assert any("B5" in b for b in read("$H1.H2..B5", None))

    # find / tree 同样覆盖深层嵌套。
    ir = parse_code("$H1.H2..B5")[0].atoms[0]
    hits = runtime.store.run_find(
        "genesis", vpath, ir.selector, [("hi", False, False)], 0, 0)
    assert hits and "H5:B5" in hits[0], hits
    tree = runtime.store.run_tree(
        "genesis", vpath, SelectorBuilder().build(), 10, 2)
    assert '"# H5"' in tree and '":B5"' in tree, tree

    # op 写：append / patch / delete / tag overwrite 在深层嵌套下保持前缀。
    def _write(selector, op, payload, proj=None):
        runtime.store.write_locator(
            "genesis", vpath, selector, op, payload,
            True, True, False, False, proj,
        )

    ir = parse_code("$H1.H2..B5")[0].atoms[0]
    _write(ir.selector, "+>", "!!", "content")
    assert read("$H1.H2..B5", "content") == ["hi!!"]

    # 重新写回原文档，避免前面 append 影响后续断言。
    _vfs_write(runtime.store, vpath, DEEP_MD)
    ir = parse_code("$H1.H2..B5")[0].atoms[0]
    _write(ir.selector, "~>", '[{"old":"hi","new":"HI"}]')
    assert read("$H1.H2..B5", None) == [">      >     - B5:  HI"]

    _vfs_write(runtime.store, vpath, DEEP_MD)
    ir = parse_code("$H1.H2..B4")[0].atoms[0]
    _write(ir.selector, "->", "")
    bullets = read("$..*[bullet]", None)
    assert len(bullets) == 3, bullets
    assert not any("B4" in b or "B5" in b for b in bullets), bullets

    _vfs_write(runtime.store, vpath, DEEP_MD)
    ir = parse_code("$H1.H2..B3")[0].atoms[0]
    _write(ir.selector, "/>", "B3X", "tag")
    assert read("$H1.H2..B3X", "tag") == ["B3X"]
    raw = read("$H1.H2..B3X", None)
    assert len(raw) == 1 and ">      > - B3X:" in raw[0], raw



