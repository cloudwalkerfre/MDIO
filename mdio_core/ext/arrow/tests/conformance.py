"""v1a conformance smoke: run with the repo venv and a built mdio_frame extension.

This is intentionally a separate, executable acceptance check: cargo test covers
FacadeCore; this script covers the DuckDB adapter surface and lifecycle.
"""
from __future__ import annotations

import json
import os
from pathlib import Path

import duckdb

HERE = Path(__file__).resolve()
EXT = Path(os.environ.get("MDIO_FRAME_DUCKDB_EXT") or (HERE.parents[1] / "mdio_frame.duckdb_extension"))
assert EXT.is_file(), f"extension not found: {EXT}"


def connect():
    con = duckdb.connect(config={"allow_unsigned_extensions": "true"})
    con.execute("LOAD '" + str(EXT).replace("'", "''") + "'")
    return con


def open_doc(con, text):
    return int(con.execute("select mdio_open(?)", [text]).fetchone()[0])


def main():
    con = connect()
    # empty
    sid = open_doc(con, "")
    assert con.execute("select count(*) from mdio_faces(?)", [sid]).fetchone()[0] == 0
    assert con.execute("select count(*) from mdio_roots(?)", [sid]).fetchone()[0] == 0

    # same-line identity + relation
    sid = open_doc(con, "- - foo\n")
    assert con.execute("select count(*) from mdio_faces(?)", [sid]).fetchone()[0] == 2
    assert con.execute("select count(*) from mdio_nodes(?)", [sid]).fetchone()[0] == 2
    assert [d[0] for d in con.execute("describe select * from mdio_nodes(?)", [sid]).fetchall()] == [
        "node", "level", "frame_ordinal", "section", "can_read", "can_write", "role", "type"]
    types = {r[0] for r in con.execute("select type from mdio_nodes(?)", [sid]).fetchall()}
    assert types == {"bullet"}, types
    assert con.execute("select count(*) from mdio_edges(?)", [sid]).fetchone()[0] == 1
    assert con.execute("select count(*) from mdio_roots(?)", [sid]).fetchone()[0] == 1
    edges = con.execute("select parent, child from mdio_edges(?)", [sid]).fetchall()
    roots = [n for (n,) in con.execute("select node from mdio_roots(?)", [sid]).fetchall()]
    assert set(roots) & {p for (p, _c) in edges}, (roots, edges)
    assert [d[0] for d in con.execute("describe select * from mdio_edges(?)", [sid]).fetchall()] == ["parent", "child", "pos"]
    assert [d[0] for d in con.execute("describe select * from mdio_span(?)", [sid]).fetchall()] == [
        "node", "start_line", "end_line", "byte_start", "byte_end"]

    # CRLF is just line endings; the same node count as LF
    sid = open_doc(con, "a\r\n\r\nb\r\n")
    assert con.execute("select count(*) from mdio_faces(?)", [sid]).fetchone()[0] == 2

    # multi-line span
    sid = open_doc(con, "> a\n> b\n")
    assert con.execute("select max(end_line - start_line) from mdio_span(?)", [sid]).fetchone()[0] > 0

    # sections + parent
    sid = open_doc(con, "# A\n\ntext\n\n## B\n\nmore\n")
    secs = con.execute("select section, parent_section from mdio_sections(?) order by start", [sid]).fetchall()
    assert len(secs) == 2 and secs[0][1] is None and secs[1][1] == secs[0][0], secs

    # descendants closure depth
    sid = open_doc(con, "- a\n  - b\n    - c\n")
    assert con.execute("select max(depth) from mdio_descendants(?)", [sid]).fetchone()[0] >= 2

    # meta
    assert con.execute("select topology, schema_version from mdio_meta()").fetchone() == ("hs", 2)

    # R0 render API: full md, selected roots, selected empty, out-of-range
    render_text = "# A\n\ntext\n"
    sid = open_doc(con, render_text)
    assert con.execute("select mdio_render_all(?)", [sid]).fetchone()[0] == render_text
    assert con.execute("select mdio_render_selected(?, ?)", [sid, "[]"]).fetchone()[0] == ""
    assert con.execute("select mdio_render_selected(?, ?)", [sid, "[-1]"]).fetchone()[0] == render_text
    roots_json = json.dumps([int(r[0]) for r in con.execute("select node from mdio_roots(?) order by pos", [sid]).fetchall()])
    assert con.execute("select mdio_render_selected(?, ?)", [sid, roots_json]).fetchone()[0] == render_text
    try:
        con.execute("select mdio_render_selected(?, ?)", [sid, "[999999]"]).fetchone()
        raise AssertionError("out-of-range node accepted")
    except duckdb.Error:
        pass

    # projection pushdown
    sid = open_doc(con, "# A\n\ntext\n")
    plan = con.execute("explain select node from mdio_faces(?)", [sid]).fetchall()[0][1]
    assert "Projections: node" in plan, plan
    plan2 = con.execute("explain select node, content from mdio_faces(?)", [sid]).fetchall()[0][1]
    assert "content" in plan2, plan2

    # close lifecycle
    sid = open_doc(con, "a\n")
    assert con.execute("select mdio_close(?)", [sid]).fetchone()[0] is True
    try:
        con.execute("select * from mdio_faces(?)", [sid]).fetchall()
        raise AssertionError("closed session still readable")
    except duckdb.Error:
        pass

    con.close()
    print("v1b conformance: OK")


if __name__ == "__main__":
    main()
