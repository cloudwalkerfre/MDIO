"""§6.2 Emphasis and strong emphasis — 132 examples (CM spec §6.2).

CM spec rules at a glance:
  - Left/Right-flanking delimiter runs rule whether `*`/`_`/`**`/`__` create emphasis
  - `_` between word characters → NOT an opener/closer (within-words rule)
  - `*` between word characters → IS an opener/closer
  - Emphasis: 1 delimiter → <em>; Strong: 2 → <strong>; 3 → both nested
  - Delimiter runs CAN be shared (`` ***foo*** `` → em+strong nested)
  - Backslash-escaped delimiters (`\\*`, `\\_`) are literal, not emphasis markers
  - Code spans, autolinks, HTML tags inside emphasis are supported

AST structure:
  em node:  {"nt": "em", "ct": [...]}
  st node:  {"nt": "st", "ct": [...]}
  txt node: {"nt": "txt", "ct": "text"}
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse, ast_has_type, ast_find_all


def _em_test(n, desc):
    """Create a per-example emphasis test.  Compares em/strong NODE COUNT
    against CM spec expected HTML (upgrade from mere existence check)."""
    async def _test(cm_workspace):
        rt = MdioRuntime(cm_workspace)
        md, expected = await cm_example(rt, "Inlines", "Emphasis and strong emphasis", n)
        ast = ast_parse(md)
        should_em = expected.count("<em>")
        should_st = expected.count("<strong>")
        em_nodes = ast_find_all(ast, "em")
        st_nodes = ast_find_all(ast, "st")
        assert len(em_nodes) == should_em, (
            f"N={n}: em count mismatch (expected {should_em}, got {len(em_nodes)})  md: {md!r}"
        )
        assert len(st_nodes) == should_st, (
            f"N={n}: strong count mismatch (expected {should_st}, got {len(st_nodes)})  md: {md!r}"
        )
    _test.__name__ = f"test_em_{n:03d}"
    _test.__doc__ = desc
    return pytest.mark.asyncio(_test)


# Tests

# §6.2 Left/Right-flanking delimiter runs

test_em_000 = _em_test(0,  "N=0: *foo bar* — `*` at start left-flanking, before space right-flanking → em")
test_em_001 = _em_test(1,  "N=1: a * foo bar* — `*` after space → not left-flanking → no em")
test_em_002 = _em_test(2,  'N=2: a*"foo"* — `*` after `a` left-flanking, before `"` not right-flanking → no em')
test_em_003 = _em_test(3,  "N=3: * a * — `*` before space → not right-flanking (also after space) → no em")
test_em_004 = _em_test(4,  "N=4: *$*alpha — Unicode punctuation `$` at start. Only first `*` flanking, others not → no em")
test_em_005 = _em_test(5,  "N=5: foo*bar* — `*` between word chars → both left+right flanking → em")
test_em_006 = _em_test(6,  "N=6: 5*6*78 — `*` between digits → both left+right flanking → em")
test_em_007 = _em_test(7,  "N=7: _foo bar_ — `_` at start left-flanking, before space not right-flanking → em")
test_em_008 = _em_test(8,  "N=8: _ foo bar_ — `_` followed by space → not right-flanking → no em")
test_em_009 = _em_test(9,  'N=9: a_"foo"_ — `_` after `a` not left-flanking (letter+`_`) → no em')
test_em_010 = _em_test(10, "N=10: foo_bar_ — `_` between word chars → NOT flanking → no em (within-word)")
test_em_011 = _em_test(11, "N=11: 5_6_78 — `_` between digits → NOT flanking → no em")
test_em_012 = _em_test(12, "N=12: пристаням_стремятся_ — Cyrillic word-internal `_` → NOT em (within-word).")
test_em_013 = _em_test(13, 'N=13: aa_"bb"_cc — `_` between `aa` and `"` left-flanking; before `cc` right-flanking → em')
test_em_014 = _em_test(14, "N=14: foo-_(bar)_ — `_` after `-` left-flanking; before `)` right-flanking → em")
test_em_015 = _em_test(15, "N=15: _foo* — `_` after nothing left-flanking; before `*` → em (shared?)")
test_em_016 = _em_test(16, "N=16: *foo bar * — `*` after space not left-flanking → no em")
test_em_017 = _em_test(17, "N=17: *foo bar\\n* — `*` across line → no em (newline not white/punct)")
test_em_018 = _em_test(18, "N=18: *(*foo) — `*` after `(` not right-flanking → no em")
test_em_019 = _em_test(19, "N=19: *(*foo*)* — outer `*` before `(` left-flanking; after `)` right-flanking → em")
test_em_020 = _em_test(20, "N=20: *foo*bar — `*foo*` em, then literal `bar`")

# `_` within-word vs `*` between-word

test_em_021 = _em_test(21, "N=21: _foo bar _ — `_` before space → not right-flanking → no em")
test_em_022 = _em_test(22, "N=22: _(_foo) — `_` after `(` not right-flanking → no em")
test_em_023 = _em_test(23, "N=23: _(_foo_)_ — outer `_` before `(` left-flanking; after `)` right-flanking → em")
test_em_024 = _em_test(24, "N=24: _foo_bar — `_` between word chars → NOT flanking → no em")
test_em_025 = _em_test(25, "N=25: _пристаням_стремятся — `_` between Cyrillic → NOT flanking → no em")
test_em_026 = _em_test(26, "N=26: _foo_bar_baz_ — multiple `_` → only outermost form em")
test_em_027 = _em_test(27, "N=27: _(bar)_. — `_` before `(` left-flanking; after `)` right-flanking → em")

# §6.2 Strong emphasis (double * or _)

test_em_028 = _em_test(28, "N=28: **foo bar** — `**` at start left-flanking, before space not right-flanking → strong")
test_em_029 = _em_test(29, "N=29: ** foo bar** — `**` followed by space → not right-flanking → no strong")
test_em_030 = _em_test(30, 'N=30: a**"foo"** — `**` after `a` left-flanking; before `"` → no strong')
test_em_031 = _em_test(31, "N=31: foo**bar** — `**` between word chars → strong")
test_em_032 = _em_test(32, "N=32: __foo bar__ — `__` → strong (same rules as `**`)")
test_em_033 = _em_test(33, "N=33: __ foo bar__ — `__` followed by space → not right-flanking → no strong")
test_em_034 = _em_test(34, "N=34: __\\nfoo bar__ — `__` across line → no strong")
test_em_035 = _em_test(35, 'N=35: a__"foo"__ — same pattern as N=30 with `_` → no strong')
test_em_036 = _em_test(36, "N=36: foo__bar__ — `__` between word chars → NOT flanking → no strong (within-word)")
test_em_037 = _em_test(37, "N=37: 5__6__78 — `__` between digits → NOT flanking → no strong")
test_em_038 = _em_test(38, "N=38: пристаням__стремятся__ — Cyrillic word-internal `__` → NOT strong (within-word).")
test_em_039 = _em_test(39, "N=39: __foo, __bar__, baz__ — `__foo` and `__bar__` → strong")
test_em_040 = _em_test(40, "N=40: foo-__(bar)__ — `__` between `-` and `(` → strong")

# Non-flanking `**` / `__`

test_em_041 = _em_test(41, "N=41: **foo bar ** — `**` before space → not right-flanking → no strong")
test_em_042 = _em_test(42, "N=42: **(**foo) — `**` at boundary → no strong")
test_em_043 = _em_test(43, "N=43: *(*foo*)* — outer `*` forms em → em+strong? No, just em")
test_em_044 = _em_test(44, "N=44: **Gomphocarpus...*Asclepias...** — complex strong with nested em")
test_em_045 = _em_test(45, 'N=45: **foo "*bar*" foo** — strong containing quoted text with em')
test_em_046 = _em_test(46, "N=46: **foo**bar — strong at word start")
test_em_047 = _em_test(47, "N=47: __foo bar __ — `__` before space → not right-flanking → no strong")
test_em_048 = _em_test(48, "N=48: __(__foo) — `__` at boundary → no strong")
test_em_049 = _em_test(49, "N=49: _(__foo__)_ — `_(` and `)_` with __ inside")
test_em_050 = _em_test(50, "N=50: __foo__bar — strong at document start")
test_em_051 = _em_test(51, "N=51: __пристаням__стремятся — Cyrillic `__` → NOT flanking → no strong")
test_em_052 = _em_test(52, "N=52: __foo__bar__baz__ — multiple `__` → strong on outer pair")
test_em_053 = _em_test(53, "N=53: __(bar)__. — `__` between `(` and `)` → strong")

# Links inside emphasis

test_em_054 = _em_test(54, "N=54: *foo [bar](/url)* — em containing link")
test_em_055 = _em_test(55, "N=55: *foo\\nbar* — em across lines")
test_em_056 = _em_test(56, "N=56: _foo __bar__ baz_ — em containing strong")
test_em_057 = _em_test(57, "N=57: _foo _bar_ baz_ — nested em inside em")
test_em_058 = _em_test(58, "N=58: __foo_ bar_ — mixed `__`/`_` → strong or em?")
test_em_059 = _em_test(59, "N=59: *foo *bar** — `*` em open, `*bar*` em, `*` close → nested em?")
test_em_060 = _em_test(60, "N=60: *foo **bar** baz* — em containing strong")
test_em_061 = _em_test(61, "N=61: *foo**bar**baz* — em start, strong inside, em close → em+strong")
test_em_062 = _em_test(62, "N=62: *foo**bar* — em start, ** → strong open, bar → text, * → em close")
test_em_063 = _em_test(63, "N=63: ***foo** bar* — 3 `*` → em+strong open, 2 `*` close, 1 `*` close → em+strong")
test_em_064 = _em_test(64, "N=64: *foo **bar*** — em, then strong+em?")
test_em_065 = _em_test(65, "N=65: *foo**bar*** — nested em, strong, em close sequence")
test_em_066 = _em_test(66, "N=66: foo***bar***baz — em+strong in mid-text")
test_em_067 = _em_test(67, "N=67: foo******bar*********baz — many `*` runs → strong (4+ pairs)")

# Complex nesting

test_em_068 = _em_test(68, "N=68: *foo **bar *baz* bim** bop* — multi-level nested em+strong+em")
test_em_069 = _em_test(69, "N=69: *foo [*bar*](/url)* — em containing link-with-em")
test_em_070 = _em_test(70, "N=70: ** is not an empty emphasis — literal `**`")
test_em_071 = _em_test(71, "N=71: **** is not an empty strong emphasis — literal `****`")
test_em_072 = _em_test(72, "N=72: **foo [bar](/url)** — strong containing link")
test_em_073 = _em_test(73, "N=73: **foo\\nbar** — strong across lines")
test_em_074 = _em_test(74, "N=74: __foo _bar_ baz__ — strong containing em")
test_em_075 = _em_test(75, "N=75: __foo __bar__ baz__ — strong containing strong")
test_em_076 = _em_test(76, "N=76: ____foo__ bar__ — 4 `_` → strong, then `__` → strong")
test_em_077 = _em_test(77, "N=77: **foo **bar**** — `**` strong open, `**` strong open, `****` strong+strong close → nested?")
test_em_078 = _em_test(78, "N=78: **foo *bar* baz** — strong containing em")
test_em_079 = _em_test(79, "N=79: **foo*bar*baz** — `**`, em `*bar*`, `baz**` → strong with em")
test_em_080 = _em_test(80, "N=80: ***foo* bar** — 3 `*` open, 1 `*` close → em+strong then em close")
test_em_081 = _em_test(81, "N=81: **foo *bar*** — strong, em close shared with strong?")
test_em_082 = _em_test(82, "N=82: **foo *bar **baz**\\nbim* bop** — multi-line nested")
test_em_083 = _em_test(83, "N=83: **foo [*bar*](/url)** — strong containing link-with-em")

# Underscore emphasis edge cases

test_em_084 = _em_test(84, "N=84: __ is not an empty emphasis — literal `__`")
test_em_085 = _em_test(85, "N=85: ____ is not an empty strong emphasis — literal `____`")
test_em_086 = _em_test(86, "N=86: foo *** — trailing `***` → no opener → no emphasis")
test_em_087 = _em_test(87, "N=87: foo *\\** — escaped `\\*` literal, outer `*...*` → em")
test_em_088 = _em_test(88, "N=88: foo *_* — em containing literal `_`")
test_em_089 = _em_test(89, "N=89: foo ***** — trailing `*****` → no opener → no emphasis")
test_em_090 = _em_test(90, "N=90: foo **\\*** — strong containing `\\*` → strong")
test_em_091 = _em_test(91, "N=91: foo **_** — strong containing `_`")
test_em_092 = _em_test(92, "N=92: **foo* — `**` strong open, `*` — no matching close → no strong?")
test_em_093 = _em_test(93, "N=93: *foo** — `*` em open, `**` → em")
test_em_094 = _em_test(94, "N=94: ***foo** — 3 `*` em+strong open, 2 `*` → strong close → strong only")
test_em_095 = _em_test(95, "N=95: ****foo* — 4 `*` → no opener (4 → can't be 3+1) → em from `*`?")
test_em_096 = _em_test(96, "N=96: **foo*** — strong open, 3 `*` close → strong+em close → strong only")
test_em_097 = _em_test(97, "N=97: *foo**** — em open, 4 `*` close → em only")
test_em_098 = _em_test(98, "N=98: foo ___ — trailing `___` → no opener → no emphasis")
test_em_099 = _em_test(99, "N=99: foo _\\__ — escaped `_` literal, outer `_..._` → em")
test_em_100 = _em_test(100, "N=100: foo _*_ — em containing `*`")
test_em_101 = _em_test(101, "N=101: foo _____ — trailing `_____` → no opener → no emphasis")
test_em_102 = _em_test(102, "N=102: foo __\\___ — escaped `_` literal, outer `__...__` → strong")
test_em_103 = _em_test(103, "N=103: foo __*__ — strong containing `*`")
test_em_104 = _em_test(104, "N=104: __foo_ — `__` strong open, `_` → em close → strong only")
test_em_105 = _em_test(105, "N=105: _foo__ — `_` em open, `__` → em only")
test_em_106 = _em_test(106, "N=106: ___foo__ — 3 `_` em+strong open, 2 `_` → strong close → strong")
test_em_107 = _em_test(107, "N=107: ____foo_ — 4 `_` → strong+strong? em+strong+em? em from `_`?")
test_em_108 = _em_test(108, "N=108: __foo___ — strong open, 3 `_` close → strong+em close → strong")
test_em_109 = _em_test(109, "N=109: _foo____ — em open, 4 `_` close → em only")
test_em_110 = _em_test(110, "N=110: **foo** — strong")
test_em_111 = _em_test(111, "N=111: *_foo_* — `*_` open, `_*` close → cross-delimiter not allowed → `*...*` em")
test_em_112 = _em_test(112, "N=112: __foo__ — strong")
test_em_113 = _em_test(113, "N=113: _*foo*_ — `_*` open, `*_` close → cross-delimiter not allowed → `_..._` em")
test_em_114 = _em_test(114, "N=114: ****foo**** — 4 `*` → strong+strong? → strong")
test_em_115 = _em_test(115, "N=115: ____foo____ — 4 `_` → strong")
test_em_116 = _em_test(116, "N=116: ******foo****** — 6 `*` → many pairs → strong")
test_em_117 = _em_test(117, "N=117: ***foo*** — 3 `*` both sides → em+strong nested")
test_em_118 = _em_test(118, "N=118: _____foo_____ — 5 `_` → em+strong+em? No, strong+em?")
test_em_119 = _em_test(119, "N=119: *foo _bar* baz_ — `*` and `_` cannot cross → `*...*` em")
test_em_120 = _em_test(120, "N=120: *foo __bar *baz bim__ bam* — complex crossing")
test_em_121 = _em_test(121, "N=121: **foo **bar baz** — outer strong, inner strong")
test_em_122 = _em_test(122, "N=122: *foo *bar baz* — em containing em")
test_em_123 = _em_test(123, "N=123: *[bar*](/url) — `*` inside `[...]` → not em (link context)")
test_em_124 = _em_test(124, "N=124: _foo [bar_](/url) — `_` inside `[...]` → not em")
test_em_125 = _em_test(125, "N=125: *<img src=\"foo\" title=\"*\"/> — `*` inside HTML → not em")
test_em_126 = _em_test(126, "N=126: **<a href=\"**\"> — `**` inside HTML attr → not strong")
test_em_127 = _em_test(127, "N=127: __<a href=\"__\"> — `__` inside HTML attr → not strong")
test_em_128 = _em_test(128, "N=128: *a `*`* — em containing code span with `*`")
test_em_129 = _em_test(129, "N=129: _a `_`_ — em containing code span with `_`")
test_em_130 = _em_test(130, "N=130: **a<https://foo.bar/?q=**> — `**` before autolink → not strong (autolink context)")


# 补齐用例 — N=131: autolink URL 内含 __ 不作为 emphasis delimiter

@pytest.mark.asyncio
async def test_em_131_autolink_priority_over_emphasis(cm_workspace):
    """N=131: __a<https://foo.bar/?q=__> — autolink 优先级高于 emphasis。
    CM spec: '<p>__a<a href="https://foo.bar/?q=__">https://foo.bar/?q=__</a></p>'
    autolink URL 内的 __ 不是 emphasis delimiter。"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, "Inlines", "Emphasis and strong emphasis", 131)
    ast = ast_parse(md)
    assert ast_has_type(ast, "al"), "URL 在 <> 内应解析为 autolink"
    assert not ast_has_type(ast, "st"), "URL 内的 __ 不应形成 strong"
