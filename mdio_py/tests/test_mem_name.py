"""$$ -n/--name named mem 回归。"""

import json

from mdio_py.core import parse_code
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.mem_slot import (begin_exec_frame, current_temp_frame,
                                   end_exec_frame, register_temp_doc)


def _lit(s: str) -> str:
    return json.dumps(s, ensure_ascii=False)


async def test_name_roundtrip_and_write(runtime_llm):
    text = "## HIS\n### main\n- user:\n  > old\n"
    code = (
        _lit(text) + " | $$ -n __topo; "
        + _lit("- assistant:\n  > new\n") + " | +> __topo::HIS.main; "
        + "$ __topo::HIS.main -pj raw"
    )
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "- user:\n  > old" in out
    assert "- assistant:\n  > new" in out


async def test_name_with_selector_names_input_not_result(runtime_llm):
    text = "## HIS\n### main\n- user:\n  > hello\n"
    code = (
        _lit(text) + " | $$ ..*_b -n __topo; "
        + "$ __topo:: -pj raw"
    )
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "## HIS" in out
    assert "### main" in out
    assert "- user:" in out


async def test_name_duplicate_overwrites(runtime_llm):
    first = "## HIS\n### main\n- user:\n  > first\n"
    second = "## HIS\n### main\n- user:\n  > second\n"
    code = (
        _lit(first) + " | $$ -n __topo; "
        + _lit(second) + " | $$ -n __topo; "
        + "$ __topo::HIS.main -pj raw"
    )
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "second" in out
    assert "first" not in out


async def test_name_is_exec_scoped(runtime_llm):
    text = "## HIS\n### main\n- user:\n  > hello\n"
    await runtime_llm.exec(_lit(text) + " | $$ -n __topo", wt_uid="genesis")
    out = await runtime_llm.exec("$ __topo::HIS.main -pj raw", wt_uid="genesis")
    assert "mdio_error" in out or "File not found" in out


async def test_map_f_sees_exec_frame(runtime_llm):
    text = "## HIS\n### main\n- user:\n  > hello\n"
    map_input = "- x: 1\n"
    code = (
        _lit(text) + " | $$ -n __topo; "
        + _lit(map_input) + " | map -sel '..*_b' -f ( _ | $ __topo::HIS.main -pj raw )"
    )
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "- mdio_error" not in out
    assert "- user:" in out




async def test_map_can_write_named_mem(runtime_llm):
    ext = "## HIS\n### main\n- assistant:\n  > ext\n"
    map_input = "- x: 1\n"
    code = (
        _lit(ext) + " | $$ -n __out; "
        + _lit(map_input)
        + " | map -sel '..*_b' -f ( _ | "
        + _lit("- user:\n  > via-map\n")
        + " | +> __out::HIS.main ); "
        + "$ __out::HIS.main -pj raw"
    )
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "via-map" in out


async def test_map_reads_named_mem_and_own_source(runtime_llm):
    ext = "## HIS\n### main\n- assistant:\n  > external\n"
    map_input = "- x: 1\n"
    code = (
        _lit(ext) + " | $$ -n __ext; "
        + _lit(map_input)
        + " | map -sel '..*_b' -f ( _ | $ __ext::HIS.main -pj raw )"
    )
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "external" in out
    assert "mdio_error" not in out


async def test_replace_and_delete(runtime_llm):
    text = "## HIS\n### main\n- user:\n  > old\n"
    code = (
        _lit(text) + " | $$ -n __topo; "
        + _lit("- assistant:\n  > replaced\n") + " | /> __topo::HIS.main; "
        + "$ __topo::HIS.main -pj raw"
    )
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "replaced" in out
    assert "old" not in out

    code2 = (
        _lit(text) + " | $$ -n __topo; "
        + "-> __topo::HIS.main._b; "
        + "$ __topo::HIS.main._b -pj tag"
    )
    out2 = await runtime_llm.exec(code2, wt_uid="genesis")
    assert "user" not in out2


async def test_empty_input_can_be_named(runtime_llm):
    code = _lit("") + " | $$ -n __empty; $ __empty:: -pj raw"
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "- mdio_error" not in out


async def test_long_name_flag(runtime_llm):
    text = "## HIS\n### main\n- user:\n  > hello\n"
    code = _lit(text) + " | $$ --name __topo; $ __topo::HIS.main -pj raw"
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "- user:\n  > hello" in out


async def test_temp_doc_folder_cleaned_after_exec(runtime_llm):
    text = "## HIS\n### main\n- user:\n  > hello\n"
    await runtime_llm.exec(_lit(text) + " | $$ -n __topo", wt_uid="genesis")
    tree = json.loads(runtime_llm._workspace.temp_store.get_vfs_tree("genesis", True).decode())
    assert not any(str(n.get("path", "")).startswith("/.tmp/") for n in tree)
    main_tree = json.loads(runtime_llm.store.get_vfs_tree("genesis", True).decode())
    assert not any(str(n.get("path", "")).startswith("/.tmp/") for n in main_tree)


async def test_hook_temp_target_rejected(runtime_llm):
    text = "## HIS\n### main\n- user:\n  > hello\n"
    code = (_lit(text) + " | $$ -n __topo; "
            + "hook -target __topo::HIS.main -cb '" + 'x' + "' -is_always")
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "mdio_error" in out


async def test_foreign_tmp_path_rejected(runtime_llm):
    out = await runtime_llm.exec("$ /.tmp/not-current/x.md -pj raw", wt_uid="genesis")
    assert "mdio_error" in out


async def test_pathless_temp_doc_read_write(runtime_llm):
    token = begin_exec_frame(runtime_llm._workspace)
    path = None
    try:
        frame = current_temp_frame()
        assert frame is not None
        path = register_temp_doc("__doc", "# OUT\n- user:\n  > hello\n")
        ctx = RuntimeContext(wt_uid="genesis", vfs_path=frame.root, active_doc=path)

        out = await runtime_llm._interpreter.run(
            parse_code("$ OUT._b[0] -pj content"), ctx=ctx)
        assert out == "hello"

        await runtime_llm._interpreter.run(
            parse_code('"## extra" | +> OUT._b'), ctx=ctx)
        doc = runtime_llm._workspace.temp_store.get_text("genesis", path)
        assert "## extra" in doc
    finally:
        end_exec_frame(token)
        end_exec_frame(token)

    assert path is not None
    assert not runtime_llm._workspace.temp_store.path_exists("genesis", path)


async def test_multi_hit_write(runtime_llm):
    text = "## HIS\n### main\n- user:\n  > a\n- user:\n  > b\n"
    code = (
        _lit(text) + " | $$ -n __topo; "
        + "-> __topo::HIS.main._b; "
        + "$ __topo::HIS.main._b -pj tag"
    )
    out = await runtime_llm.exec(code, wt_uid="genesis")
    assert "user" not in out
