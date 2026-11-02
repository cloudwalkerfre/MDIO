import json


DOC = """## HIS
### main
- user:
  > first
- assistant:
  > second
"""

NEST = """## HIS
### main
- user:
  > first
    - child:
        > c1
- assistant:
  > second
"""


def _src(text=DOC):
    return json.dumps(text, ensure_ascii=False)


async def test_filter_true_keeps_all(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b -pj raw" -f ( _ | fmt "true" )'
    )
    assert "first" in out and "second" in out
    assert "mdio_error" not in out


async def test_filter_false_deletes_all(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b -pj raw" -f ( _ | fmt "false" )'
    )
    assert "first" not in out and "second" not in out
    assert "### main" in out


async def test_filter_uppercase_true_keeps(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b -pj raw" -f ( _ | fmt "TRUE" )'
    )
    assert "first" in out and "second" in out


async def test_filter_uppercase_false_deletes(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b -pj raw" -f ( _ | fmt "FALSE" )'
    )
    assert "first" not in out and "second" not in out
    assert "### main" in out


async def test_filter_no_hit_returns_source(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main.none -pj raw" -f ( $ /nope.md:: -pj raw )'
    )
    assert "first" in out and "second" in out
    assert "mdio_error" not in out


async def test_filter_rejects_nonboolean_predicate(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b -pj raw" -f ( _ | fmt "yes" )'
    )
    assert "mdio_error" in out


async def test_filter_active_doc_pathless(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b -pj raw" -f ( _ | $ main._b[0] -pj tag | fmt "true" )'
    )
    assert "mdio_error" not in out
    assert "first" in out and "second" in out


async def test_filter_nested_false_deletes_subtree(runtime_mem):
    out = await runtime_mem.exec(
        _src(NEST) + ' | filter -sel "main..*_b -pj raw" -f ( _ | fmt "false" )'
    )
    assert "first" not in out and "c1" not in out and "second" not in out
    assert "### main" in out


async def test_filter_empty_bullet_deletion(runtime_mem):
    doc = "## HIS\n### main\n- user:\n- assistant:\n  > second\n"
    out = await runtime_mem.exec(
        _src(doc) + ' | filter -sel "main._b -pj raw" -f ( _ | fmt "false" )'
    )
    assert "### main" in out
    assert "- user:" not in out
    assert "second" not in out


async def test_filter_rejects_path_sel(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "/wl.md::main._b -pj raw" -f ( _ | fmt "true" )'
    )
    assert "mdio_error" in out


async def test_filter_rejects_separate_pj(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b" -pj raw -f ( _ | fmt "true" )'
    )
    assert "mdio_error" in out


async def test_filter_rejects_n(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b -pj raw" -n __x -f ( _ | fmt "true" )'
    )
    assert "mdio_error" in out


async def test_filter_rejects_mark(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b -pj mark" -f ( _ | fmt "true" )'
    )
    assert "mdio_error" in out


async def test_filter_requires_sel(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | filter -f ( _ | fmt "true" )')
    assert "mdio_error" in out and "-sel" in out


async def test_filter_rejects_string_f(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | filter -sel "main._b -pj raw" -f "fmt true"'
    )
    assert "mdio_error" in out and "exec block" in out
