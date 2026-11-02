import json


DOC = """## HIS
### main
- user:
  > first
- assistant:
  > second
"""

DOC2 = """## OTHER
### main
- assistant:
  > external
"""


def _src(text=DOC):
    return json.dumps(text, ensure_ascii=False)


async def test_foreach_concat_results(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b -pj ct" -f ( _ | fmt "X" )')
    assert out == "XX"


async def test_foreach_item_md_has_loc(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b -pj raw" -f ( _ )')
    assert "# item" in out
    assert "- loc: main._b[0]" in out
    assert "- loc: main._b[1]" in out
    assert "__foreach" not in out
    assert "## md" in out


async def test_foreach_default_content(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b -pj ct" -f ( _ )')
    assert "first" in out and "second" in out


async def test_foreach_raw_face(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b -pj raw" -f ( _ )')
    assert "- user:" in out and "  > first" in out
    assert "- assistant:" in out and "  > second" in out


async def test_foreach_tag_face(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b -pj tag" -f ( _ )')
    assert "user" in out and "assistant" in out


async def test_foreach_external_named_mem(runtime_mem):
    code = (
        _src(DOC2)
        + " | $$ -n __ext; "
        + _src()
        + ' | foreach -sel "main._b -pj ct" -f ( _ | $ __ext::main._b[0] -pj raw )'
    )
    out = await runtime_mem.exec(code)
    assert "external" in out
    assert "mdio_error" not in out


async def test_foreach_requires_sel(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -f ( _ | fmt "X" )')
    assert "mdio_error" in out and "-sel" in out


async def test_foreach_rejects_old_t_flag(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -t "main._b" -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_foreach_rejects_string_f(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b -pj ct" -f "fmt X"')
    assert "mdio_error" in out and "exec block" in out


async def test_foreach_empty_hits(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main.none -pj ct" -f ( _ | fmt "X" )')
    assert out == ""


async def test_foreach_exec_error(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b -pj ct" -f ( $ /nope.md:: -pj raw )')
    assert "mdio_error" in out


async def test_foreach_invalid_selector(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b[" -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_foreach_rejects_path_sel(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "/wl.md::main._b -pj ct" -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_foreach_rejects_separate_pj(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b" -pj ct -f ( _ | fmt "X" )')
    assert "mdio_error" in out


async def test_foreach_item_order(runtime_mem):
    out = await runtime_mem.exec(_src() + ' | foreach -sel "main._b -pj raw" -f ( _ )')
    assert out.index("- idx: 0") < out.index("- idx: 1")


async def test_foreach_reads_item_via_input_mem(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | foreach -sel "main._b -pj raw" -f ( $ __input_mem::item.md -pj raw )'
    )
    assert "- user:" in out and "  > first" in out
    assert "- assistant:" in out and "  > second" in out
    assert "mdio_error" not in out


async def test_foreach_active_doc_pathless(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | foreach -sel "main._b -pj raw" -f ( _ | $ main._b[0] -pj raw )'
    )
    assert "mdio_error" not in out
    assert "first" in out


async def test_foreach_rejects_n(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | foreach -sel "main._b -pj ct" -n __x -f ( _ | fmt "X" )'
    )
    assert "mdio_error" in out


async def test_foreach_external_named_mem_write(runtime_mem):
    code = (
        _src(DOC2)
        + " | $$ -n __ext; "
        + _src()
        + ' | foreach -sel "main._b -pj ct" -f ( _ | "- user:\\n  > via-foreach\\n" | +> __ext::main ); '
        + "$ __ext::main -pj raw"
    )
    out = await runtime_mem.exec(code)
    assert "via-foreach" in out
    assert "mdio_error" not in out


async def test_foreach_empty_input(runtime_mem):
    out = await runtime_mem.exec(
        json.dumps("", ensure_ascii=False)
        + ' | foreach -sel "main._b -pj ct" -f ( _ | fmt "X" )'
    )
    assert out == ""


NEST = """## HIS
### main
- user:
  > first
    - child:
        > c1
- assistant:
  > second
"""


async def test_foreach_recursive_items(runtime_mem):
    out = await runtime_mem.exec(
        _src(NEST) + ' | foreach -sel "main..*_b -pj raw" -f ( _ )'
    )
    assert out.count("- loc: main..*_b[") == 3
    assert "c1" in out


async def test_foreach_slice_items(runtime_mem):
    out = await runtime_mem.exec(
        _src(NEST) + ' | foreach -sel "main._b[0:1] -pj raw" -f ( _ | fmt "S" )'
    )
    assert out == "S"


async def test_foreach_multiline_payload(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | foreach -sel "main._b -pj raw" -f ( _ )'
    )
    assert out.count("## md") == 2


async def test_foreach_no_hit_does_not_execute(runtime_mem):
    out = await runtime_mem.exec(
        _src() + ' | foreach -sel "main.none -pj ct" -f ( $ /nope.md:: -pj raw )'
    )
    assert out == ""


async def test_foreach_external_mem_value(runtime_mem):
    code = (
        _src(DOC2) + " | $$ -n __ext; "
        + _src() + ' | foreach -sel "main._b -pj raw" -f ( _ | $ __ext::main._b[0] -pj raw )'
    )
    out = await runtime_mem.exec(code)
    assert "external" in out
    assert "mdio_error" not in out
