"""Lifecycle tests for shared arrow DuckDB runtime."""
from __future__ import annotations

import asyncio
import os

import pytest

from mdio_py.core.mem_slot import begin_exec_frame, end_exec_frame
from mdio_py.std.arrow_ops import _ext_path, arrow
from mdio_py.std.arrow_runtime import DuckDBRuntime, RuntimeClosed


class _Store:
    def __init__(self) -> None:
        self.events: list[str] = []

    def mk(self, *args, **kwargs):
        return None

    def rm(self, *args, **kwargs):
        self.events.append("rm")

    def get_text(self, *args, **kwargs):
        return ""


class _Workspace:
    def __init__(self) -> None:
        self.temp_store = _Store()


def _require_ext() -> str:
    ext = _ext_path()
    if not os.path.isfile(ext):
        pytest.skip("mdio_frame.duckdb_extension not built")
    return ext


def test_runtime_reuses_doc_and_closes() -> None:
    ext = _require_ext()
    rt = DuckDBRuntime("test-exec", ext)
    try:
        d1 = rt.open_doc("# A\n\nhello\n")
        d2 = rt.open_doc("# A\n\nhello\n")
        assert d2 is d1
        assert rt.doc_count == 1
        with rt.query(d1) as cur:
            assert cur.execute("select count(*) from mdio_nodes(?)", [d1.sid]).fetchone()[0] >= 1
            assert cur.execute("select mdio_render_selected(?, ?)", [d1.sid, "[0]"]).fetchone()[0]
    finally:
        rt.close()
    assert rt.state == "closed"
    with pytest.raises(RuntimeClosed):
        rt.open_doc("x")


def test_last_doc_closes_runtime() -> None:
    ext = _require_ext()
    rt = DuckDBRuntime("test-exec", ext)
    d = rt.open_doc("# A\n")
    rt.drop_doc(d.key)
    assert rt.state == "closed"
    with pytest.raises(RuntimeClosed):
        rt.open_doc("# B\n")
    rt.close()


def test_arrow_uses_one_runtime_and_temp_views_per_cursor() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n\nhello\n\n- item\n"
        out1 = asyncio.run(arrow(ws, md, [], {"-duckdb": "select node from nodes order by node"}, None))
        assert not out1.startswith("- mdio_error:")
        out2 = asyncio.run(arrow(ws, md, [], {"-duckdb": "select node, type from nodes order by node", "-rows": "10"}, None))
        assert "\"columns\"" in out2
        frame = token.frame
        assert frame is not None
        rt = frame.duckdb_runtime
        assert rt is not None
        assert rt.doc_count == 1
    finally:
        end_exec_frame(token)
    assert token.frame is not None
    assert token.frame.duckdb_runtime is None


def test_end_exec_frame_closes_runtime_before_temp_root() -> None:
    class _RT:
        def __init__(self, events):
            self.events = events

        def close(self):
            self.events.append("runtime.close")

    class _LocalStore(_Store):
        def rm(self, *args, **kwargs):
            self.events.append("root.rm")

    store = _LocalStore()
    ws = _Workspace()
    ws.temp_store = store
    token = begin_exec_frame(ws)
    frame = token.frame
    assert frame is not None
    frame.duckdb_runtime = _RT(store.events)
    end_exec_frame(token)
    assert store.events == ["runtime.close", "root.rm"]



def _script_out(ws, md: str, sql: str, **kwargs) -> str:
    return asyncio.run(arrow(ws, md, [], {"-duckdb": sql, **kwargs}, None))


def test_script_temp_table_setup_and_final_rows() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n\nhello\n\n- item\n"
        out = _script_out(
            ws,
            md,
            "create temp table t as select node from nodes; "
            "select node from t order by node",
            **{"-rows": "10"},
        )
        assert "\"columns\"" in out
        assert "\"node\"" in out
    finally:
        end_exec_frame(token)


def test_script_intermediate_select_is_allowed_and_discarded() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n"
        out = _script_out(ws, md, "select 0 as node, 1 as a; select 0 as node, 2 as b", **{"-rows": "10"})
        assert "\"b\"" in out
        assert "\"a\"" not in out
        assert "\"2\"" in out
    finally:
        end_exec_frame(token)


def test_script_temp_objects_do_not_leak_to_next_arrow() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n\nhello\n"
        out1 = _script_out(
            ws,
            md,
            "create temp table t as select 1 as x; select 0 as node, count(*) as n from t",
            **{"-rows": "10"},
        )
        assert "\"n\"" in out1
        out2 = _script_out(ws, md, "select * from t", **{"-rows": "10"})
        assert out2.startswith("- mdio_error:")
    finally:
        end_exec_frame(token)


def test_script_rejects_unsafe_or_non_final_statements() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n"
        for sql in [
            "create table t as select 1; select 1",
            "copy (select 1) to '/tmp/arrow_script_should_not_exist'; select 1",
            "create temp table t as select 1",
        ]:
            out = _script_out(ws, md, sql, **{"-rows": "10"})
            assert out.startswith("- mdio_error:"), (sql, out)
            assert "ARROW_SCRIPT_REJECTED" in out or "ARROW_SQL_REJECTED" in out, (sql, out)
    finally:
        end_exec_frame(token)


def test_script_temp_macro_and_insert() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n"
        out = _script_out(
            ws,
            md,
            "create temp table t(x integer); "
            "create temp macro inc(v) as v + 1; "
            "insert into t values (inc(41)); "
            "select 0 as node, x from t",
            **{"-rows": "10"},
        )
        assert "\"41\"" not in out
        assert "\"42\"" in out
    finally:
        end_exec_frame(token)


def test_script_default_md_requires_node() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n\nhello\n"
        out = _script_out(
            ws,
            md,
            "create temp table t as select node from nodes; select node from t order by node",
        )
        assert not out.startswith("- mdio_error:")
    finally:
        end_exec_frame(token)


def test_script_leading_comments_are_allowed() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n"
        out = _script_out(
            ws,
            md,
            "-- setup\ncreate temp table t as select 1 as x; "
            "/* final */\nselect 0 as node, x from t",
            **{"-rows": "10"},
        )
        assert "\"x\"" in out
        assert "\"1\"" in out
    finally:
        end_exec_frame(token)


class _DocStore:
    def __init__(self, docs):
        self.docs = dict(docs)

    def expand_glob(self, wt_uid, pattern, include_hidden=False):
        return list(self.docs)

    def is_dir(self, wt_uid, path):
        return False

    def get_text(self, wt_uid, path):
        return self.docs[path]


def test_arrow_store_mode_docs_and_cross_doc() -> None:
    _require_ext()
    ws = _Workspace()
    ws.active_worktree_uid = "genesis"
    ws.store = _DocStore({
        "/notes/a.md": "# A\n\nalpha body\n",
        "/notes/b.md": "# B\n\nbeta body\n",
    })
    token = begin_exec_frame(ws)
    try:
        docs_out = asyncio.run(arrow(
            ws, None, [],
            {"-duckdb": "select 0 as node, path, name from docs order by path", "-rows": "10"},
            None,
        ))
        assert "\"path\"" in docs_out, docs_out
        assert "\"/notes/a.md\"" in docs_out, docs_out
        assert "\"/notes/b.md\"" in docs_out, docs_out


        blocks_out = asyncio.run(arrow(
            ws, None, [],
            {"-duckdb": "select path, type, count(*) as n, 0 as node from blocks group by path, type order by path, type",
             "-rows": "10"},
            None,
        ))
        assert "\"blocks\"" not in blocks_out
        assert "\"/notes/a.md\"" in blocks_out and "\"/notes/b.md\"" in blocks_out, blocks_out
        outline_out = asyncio.run(arrow(
            ws, None, [],
            {"-duckdb": "select path, section, level, 0 as node from outline order by path, section",
             "-rows": "10"},
            None,
        ))
        assert "\"outline\"" not in outline_out
        assert "\"/notes/a.md\"" in outline_out, outline_out

        nodes_out = asyncio.run(arrow(
            ws, None, [],
            {"-duckdb": "select path, min(node) as node, count(*) as n from nodes group by path order by path",
             "-rows": "10"},
            None,
        ))
        assert "\"/notes/a.md\"" in nodes_out, nodes_out
        assert "\"/notes/b.md\"" in nodes_out, nodes_out

        md = asyncio.run(arrow(
            ws, None, [],
            {"-duckdb": "select path, node from nodes where type = 'head' and level = 1 order by path, node"},
            None,
        ))
        assert "## /notes/a.md" in md, md
        assert "## /notes/b.md" in md, md
        assert "alpha body" in md, md
        assert "beta body" in md, md
    finally:
        end_exec_frame(token)





def test_arrow_store_mode_preselection_by_docs_sql() -> None:
    _require_ext()
    ws = _Workspace()
    ws.active_worktree_uid = "genesis"
    ws.store = _DocStore({
        "/notes/a.md": "# A\n\nalpha body\n",
        "/notes/b.md": "# B\n\nbeta body\n",
    })
    token = begin_exec_frame(ws)
    try:
        out = asyncio.run(arrow(
            ws, None, [],
            {
                "-duckdb": "select path, node from nodes where type = 'head' order by path, node",
                "-rows": "10",
            },
            None,
        ))
        assert "\"/notes/a.md\"" in out, out
        assert "\"/notes/b.md\"" in out, out
    finally:
        end_exec_frame(token)

def test_arrow_store_mode_empty_store_errors() -> None:
    _require_ext()
    ws = _Workspace()
    ws.active_worktree_uid = "genesis"
    ws.store = _DocStore({})
    token = begin_exec_frame(ws)
    try:
        out = asyncio.run(arrow(ws, None, [], {"-duckdb": "select 1", "-rows": "10"}, None))
        assert out.startswith("- mdio_error:"), out
        assert "ARROW_NO_DOCS" in out, out
    finally:
        end_exec_frame(token)



def _bm25_setup() -> str:
    return (
        "create temp table search_docs as "
        "select n.node, -1::BIGINT as doc_id, n.section, n.type, "
        "coalesce(h.tag, '') as heading, "
        "coalesce(n.tag, '') as tag, "
        "coalesce(n.content, '') as text "
        "from nodes n "
        "left join sections s on s.section = n.section "
        "left join nodes h on h.node = s.header_node "
        "where (n.role in ('content', 'item_leaf') "
        "       or (n.role = 'shell' and n.type in ('bullet', 'ol'))) "
        "and (n.can_read & 8) <> 0 "
        "and length(trim(coalesce(n.content, ''))) > 0; "
        "PRAGMA create_fts_index("
        "'search_docs', 'node', 'text', 'heading', 'tag', "
        "tokenizer = 'opensearch_standard', "
        "stemmer = 'none', stopwords = 'none', "
        "overwrite = 1, layered_search = 1); "
    )


def _bm25_md() -> str:
    return (
        "# Incident\n\n"
        "## Symptom\n\n"
        "Checkout p99 latency reached 2.4s after the release.\n\n"
        "## Root cause\n\n"
        "The payment connection pool max_connections was reduced from 100 to 20.\n\n"
        "- retry policy: five attempts\n"
        "- rollback: restore pool size to 100\n"
    )


def test_runtime_preloads_fts() -> None:
    _require_ext()
    rt = DuckDBRuntime("test-fts", _ext_path())
    try:
        if not rt.fts_available:
            pytest.skip(f"fts extension unavailable: {rt.fts_error}")
        assert rt.fts_available
    finally:
        rt.close()


def test_script_bm25_candidate_and_fine_md() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = _bm25_md()
        coarse = (
            _bm25_setup()
            + "with hits as ("
            "  select docname::BIGINT as node, score, rank "
            "  from fts_main_search_docs.search_layered_bm25("
            "    'connection pool', top_k := 20, "
            "    fields := 'text,heading,tag'"
            "  )"
            "), scope as ("
            "  select section as scope_id, node from search_docs where section >= 0"
            "), agg as ("
            "  select sc.scope_id, max(h.score) as score, count(*) as matched "
            "  from hits h join scope sc using (node) "
            "  group by sc.scope_id"
            ") "
            "select s.header_node as node, a.scope_id, hh.tag as heading, "
            "a.score, a.matched "
            "from agg a "
            "join sections s on s.section = a.scope_id "
            "join nodes hh on hh.node = s.header_node "
            "order by a.score desc"
        )
        out = _script_out(ws, md, coarse, **{"-rows": "10"})
        assert "\"scope_id\"" in out, out
        assert "\"matched\"" in out, out
        assert "\"Root cause\"" in out, out

        fine = (
            _bm25_setup()
            + "select s.header_node as node from sections s "
            "where s.section = ("
            "  select section from search_docs "
            "  where text ilike '%connection pool%' limit 1"
            ")"
        )
        md_out = _script_out(ws, md, fine)
        assert "Root cause" in md_out, md_out
        assert "connection pool" in md_out, md_out
    finally:
        end_exec_frame(token)


def test_script_rejects_non_fts_pragma() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        out = _script_out(ws, "# A\n", "pragma database_size; select 1", **{"-rows": "10"})
        assert out.startswith("- mdio_error:"), out
        assert "ARROW_SCRIPT_REJECTED" in out, out
    finally:
        end_exec_frame(token)


# additive search layer (BM25) tests


def test_search_blocks_excludes_headers_and_hits_text_only() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# T\n\nintro pool body\n\n## Sec\n\nheading only words here\n\n- item pool bullet\n"
        hdr = _script_out(
            ws, md,
            "select count(*) as n, 0 as node from search.blocks sb "
            "join nodes n on n.path = sb.path and n.node = sb.node "
            "where n.role = 'header'",
            **{"-rows": "10"},
        )
        assert not hdr.startswith("- mdio_error:"), hdr
        assert '"0"' in hdr, hdr

        pool = _script_out(
            ws, md, "select count(*) as n, 0 as node from search.hits('pool')",
            **{"-rows": "10"},
        )
        assert '"2"' in pool, pool

        sec = _script_out(
            ws, md, "select count(*) as n, 0 as node from search.hits('Sec')",
            **{"-rows": "10"},
        )
        assert '"0"' in sec, sec
    finally:
        end_exec_frame(token)


def test_search_result_columns_available() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        out = _script_out(
            ws, "# T\n\npool text\n",
            "select path, node, section, heading, parent, pos, type, tag, "
            "start_line, end_line from search.blocks order by node",
            **{"-rows": "10"},
        )
        for col in ('"path"', '"parent"', '"pos"', '"heading"', '"start_line"'):
            assert col in out, out
    finally:
        end_exec_frame(token)


def test_search_hits_multidoc_attribution() -> None:
    _require_ext()
    ws = _Workspace()
    ws.active_worktree_uid = "genesis"
    ws.store = _DocStore({
        "/notes/a.md": "# A\n\nalpha pool\n",
        "/notes/b.md": "# B\n\nbeta pool\n",
    })
    token = begin_exec_frame(ws)
    try:
        out = asyncio.run(arrow(
            ws, None, [],
            {"-duckdb": "select h.path, h.node, h.score from search.hits('pool') h order by h.path",
             "-rows": "10"},
            None,
        ))
        assert '"/notes/a.md"' in out, out
        assert '"/notes/b.md"' in out, out
    finally:
        end_exec_frame(token)


def test_search_hits_not_truncated() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# T\n\n" + "\n\n".join("pool item %d" % i for i in range(120)) + "\n"
        out = _script_out(
            ws, md, "select count(*) as n, 0 as node from search.hits('pool')",
            **{"-rows": "10"},
        )
        assert '"120"' in out, out
    finally:
        end_exec_frame(token)


def test_search_results_leaf_most_and_default_md() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# Root\n\nintro\n\n## Alpha\n\nalpha pool one\n\n### Nested\n\nnested pool two\n"
        rows = _script_out(
            ws, md,
            "select r.section, sec.heading, r.node, r.matched "
            "from search.results('pool') r "
            "join sections sec on sec.path = r.path and sec.section = r.section",
            **{"-rows": "10"},
        )
        assert '"Nested"' in rows, rows
        assert '"Alpha"' not in rows, rows

        md_out = _script_out(ws, md, "select r.path, r.node from search.results('pool') r")
        assert "nested pool two" in md_out, md_out
        assert "alpha pool one" not in md_out, md_out
    finally:
        end_exec_frame(token)


def test_search_results_node_is_header_node() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        out = _script_out(
            ws, "# T\n\n## Sec\n\npool body\n",
            "select count(*) as n, 0 as node from search.results('pool') r "
            "join sections s on s.path = r.path and s.section = r.section "
            "where r.node = s.header_node",
            **{"-rows": "10"},
        )
        assert '"1"' in out, out
    finally:
        end_exec_frame(token)


def test_search_hits_default_md_outputs_blocks() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        out = _script_out(
            ws, "# T\n\npool paragraph here\n",
            "select h.path, h.node from search.hits('pool') h",
        )
        assert not out.startswith("- mdio_error:"), out
        assert "pool paragraph here" in out, out
    finally:
        end_exec_frame(token)


def test_search_no_reference_builds_no_index() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        out = _script_out(
            ws, "# A\n\nhello\n",
            "select count(*) as n, 0 as node from duckdb_functions() "
            "where schema_name like 'fts_main_%'",
            **{"-rows": "10"},
        )
        assert '"0"' in out, out
    finally:
        end_exec_frame(token)


def test_search_no_match_outputs_empty() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        out = _script_out(ws, "# A\n\nhello\n", "select h.path, h.node from search.hits('zzzqqq') h")
        assert not out.startswith("- mdio_error:"), out
        assert out == "", repr(out)
    finally:
        end_exec_frame(token)


def test_search_q_edge_literals() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n\npool text\n"
        for q in ("", "   ", "---", "pool's", "的的的", "a" * 300):
            sql = "select count(*) as n, 0 as node from search.hits('%s')" % q.replace("'", "''")
            out = _script_out(ws, md, sql, **{"-rows": "10"})
            assert not out.startswith("- mdio_error:"), (q, out)
    finally:
        end_exec_frame(token)


def test_search_fts_unavailable_reports_and_base_arrow_works() -> None:
    from mdio_py.std.arrow_runtime import get_runtime

    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        frame = token.frame
        rt = get_runtime(frame, _ext_path())
        rt.fts_available = False
        rt.fts_error = "simulated-fts-missing"

        out = _script_out(
            ws, "# A\n\npool text\n",
            "select h.path, h.node from search.hits('pool') h",
        )
        assert out.startswith("- mdio_error:"), out
        assert "ARROW_SEARCH_UNAVAILABLE" in out, out

        blocks = _script_out(
            ws, "# A\n\npool text\n",
            "select count(*) as n, 0 as node from search.blocks",
            **{"-rows": "10"},
        )
        assert not blocks.startswith("- mdio_error:"), blocks

        ok = _script_out(
            ws, "# A\n\npool text\n",
            "select node from nodes order by node",
            **{"-rows": "10"},
        )
        assert not ok.startswith("- mdio_error:"), ok
    finally:
        end_exec_frame(token)


def test_search_repeated_calls_same_frame() -> None:
    _require_ext()
    ws = _Workspace()
    token = begin_exec_frame(ws)
    try:
        md = "# A\n\npool one\n"
        a = _script_out(ws, md, "select count(*) as n, 0 as node from search.hits('pool')", **{"-rows": "10"})
        b = _script_out(ws, md, "select count(*) as n, 0 as node from search.results('pool')", **{"-rows": "10"})
        c = _script_out(ws, md, "select count(*) as n, 0 as node from search.hits('pool')", **{"-rows": "10"})
        assert '"1"' in a, a
        assert '"1"' in b, b
        assert '"1"' in c, c
    finally:
        end_exec_frame(token)


def test_arrow_store_mode_parallel_open_many_docs() -> None:
    _require_ext()
    ws = _Workspace()
    ws.active_worktree_uid = "genesis"
    ws.store = _DocStore({
        "/notes/d%02d.md" % i: "# D%d\n\nbody pool %d\n" % (i, i) for i in range(40)
    })
    token = begin_exec_frame(ws)
    try:
        out = asyncio.run(arrow(
            ws, None, [],
            {"-duckdb": "select count(distinct path) as n, 0 as node from search.blocks",
             "-rows": "10"},
            None,
        ))
        assert not out.startswith("- mdio_error:"), out
        assert '"40"' in out, out
        hits = asyncio.run(arrow(
            ws, None, [],
            {"-duckdb": "select count(*) as n, 0 as node from search.hits('pool')",
             "-rows": "10"},
            None,
        ))
        assert '"40"' in hits, hits
    finally:
        end_exec_frame(token)


def test_arrow_store_mode_uses_exec_ctx_worktree() -> None:
    from types import SimpleNamespace

    class _RecordingStore(_DocStore):
        def __init__(self, docs):
            super().__init__(docs)
            self.wts = []

        def expand_glob(self, wt_uid, pattern, include_hidden=False):
            self.wts.append(wt_uid)
            return list(self.docs)

        def get_text(self, wt_uid, path):
            self.wts.append(wt_uid)
            return self.docs[path]

    _require_ext()
    ws = _Workspace()
    ws.active_worktree_uid = "genesis"
    store = _RecordingStore({"/notes/a.md": "# A\n\nalpha pool\n"})
    ws.store = store
    token = begin_exec_frame(ws)
    try:
        out = asyncio.run(arrow(
            ws, None, [],
            {"-duckdb": "select count(*) as n, 0 as node from nodes", "-rows": "10"},
            SimpleNamespace(wt_uid="proj-wt"),
        ))
        assert not out.startswith("- mdio_error:"), out
        assert store.wts, store.wts
        assert set(store.wts) == {"proj-wt"}, store.wts
    finally:
        end_exec_frame(token)

