import json


DOC = """## HIS
### main
- user:
  > first
- assistant:
  > second
"""


def _src(text=DOC):
    return json.dumps(text, ensure_ascii=False)


async def test_map_content_replace(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b -pj ct" -f ( _ | fmt "X" )')
    assert "- user:\n  > X" in out
    assert "- assistant:\n  > X" in out
    assert "first" not in out and "second" not in out


async def test_map_raw_replace(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b -pj raw" -f ( _ | fmt "## changed" )')
    assert "## changed" in out
    assert "first" not in out and "second" not in out


async def test_map_raw_result_keeps_line_boundary(runtime_mem):
    doc = "# T\n\n## S\n- a: 1\n- b: 2\n"
    out = await runtime_mem.exec(
        _src(doc) + ' | map -sel "S._b[0] -pj raw" -f ( _ | fmt "X" )'
    )
    assert "X\n- b: 2" in out
    assert "X- b: 2" not in out


async def test_map_tag_replace(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b -pj tag" -f ( _ | fmt "changed" )')
    assert "- changed:" in out
    assert "- user:" not in out and "- assistant:" not in out


async def test_map_no_hit_returns_source(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main.none -pj ct" -f ( _ | fmt "X" )')
    assert "first" in out and "second" in out
    assert "mdio_error" not in out


async def test_map_multiple_items(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b -pj ct" -f ( _ | fmt "Y" )')
    assert out.count("> Y") == 2


async def test_map_requires_sel(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -f ( _ | fmt "X" )')
    assert "mdio_error" in out and "-sel" in out


async def test_map_rejects_path_sel(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "/wl.md::main._b -pj ct" -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_map_rejects_separate_pj(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b" -pj ct -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_map_rejects_n(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b -pj ct" -n __x -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_map_rejects_old_t_flag(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -t "main._b" -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_map_rejects_string_f(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b -pj ct" -f "fmt X"')
    assert "mdio_error" in out and "exec block" in out


async def test_map_exec_error(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b -pj ct" -f ( $ /nope.md:: -pj raw )')
    assert "mdio_error" in out


async def test_map_invalid_selector(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b[" -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_map_slice_selector(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b[0] -pj ct" -f ( _ | fmt "X" )')
    assert "- user:\n  > X" in out
    assert "- assistant:\n  > second" in out


async def test_map_mark_rejected(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | map -sel "main._b -pj mark" -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_map_active_doc_pathless(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | map -sel "main._b -pj raw" -f ( _ | $ main._b[0] -pj raw )'
    )
    assert "mdio_error" not in out
    assert "first" in out


NEST = """## HIS
### main
- user:
  > first
    - child:
        > c1
- assistant:
  > second
"""


async def test_map_rejects_overlapping_items(runtime_mem):
    out = await runtime_mem.exec(
        _src(NEST) + ' | map -sel "main..*_b -pj ct" -f ( _ | fmt "X" )'
    )
    assert "mdio_error" in out


async def test_trio_temp_docs_cleaned(runtime_mem):
    code = (
        _src() + ' | map -sel "main._b -pj ct" -f ( _ | fmt "M" ); '
        + _src() + ' | foreach -sel "main._b -pj ct" -f ( _ | fmt "F" ); '
        + _src() + ' | filter -sel "main._b -pj raw" -f ( _ | fmt "false" )'
    )
    out = await runtime_mem.exec(code)
    assert "mdio_error" not in out
    tree = json.loads(runtime_mem._workspace.temp_store.get_vfs_tree("genesis", True).decode())
    assert not any(str(n.get("path", "")).startswith("/.tmp/") for n in tree)


MULTI = """## HIS
### main
- user:
  - id: u1
  > first
- assistant:
  > second
- user:
  > third
"""

DUP = """## HIS
### main
- user:
  > dup
- assistant:
  > dup
"""

DOC2 = """## OTHER
### main
- assistant:
  > external
"""


def _dup(text=DOC):
    return json.dumps(text, ensure_ascii=False)


async def test_map_slice_preserves_unselected(runtime_mem):
    out = await runtime_mem.exec(
        _dup(MULTI) + ' | map -sel "main._b[0:2] -pj ct" -f ( _ | fmt "X" )'
    )
    assert "- id: u1" in out
    assert "  > X" in out
    assert "  > third" in out


async def test_map_duplicate_old_content_is_scoped(runtime_mem):
    out = await runtime_mem.exec(
        _dup(DUP) + ' | map -sel "main._b[0] -pj ct" -f ( _ | fmt "changed" )'
    )
    assert out.count("changed") == 1
    assert out.count("dup") == 1


async def test_map_multiline_result(runtime_mem):
    out = await runtime_mem.exec(
        _dup() + ' | map -sel "main._b[0] -pj ct" -f ( _ | fmt "line1\\nline2" )'
    )
    assert "line1" in out and "line2" in out
    assert "first" not in out


async def test_map_nested_child_only(runtime_mem):
    out = await runtime_mem.exec(
        _dup(NEST) + ' | map -sel "main._b[0]._b -pj ct" -f ( _ | fmt "CHILD" )'
    )
    assert "first" in out
    assert "CHILD" in out
    assert "second" in out


async def test_map_no_hit_does_not_execute(runtime_mem):
    out = await runtime_mem.exec(
        _dup() + ' | map -sel "main.none -pj ct" -f ( $ /nope.md:: -pj raw )'
    )
    assert "first" in out
    assert "mdio_error" not in out


async def test_map_empty_source(runtime_mem):
    out = await runtime_mem.exec(
        json.dumps("", ensure_ascii=False) + ' | map -sel "main._b -pj ct" -f ( _ | fmt "X" )'
    )
    assert out == ""


async def test_map_external_mem_value(runtime_mem):
    code = (
        _dup(DOC2) + " | $$ -n __ext; "
        + _dup() + ' | map -sel "main._b[0] -pj raw" -f ( _ | $ __ext::main._b[0] -pj raw )'
    )
    out = await runtime_mem.exec(code)
    assert "external" in out
    assert "mdio_error" not in out
