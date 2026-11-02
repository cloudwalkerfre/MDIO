"""v1b Python-op conformance: default md, -rows/-json, and Result validation."""
from __future__ import annotations

import asyncio
import json
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "mdio_py" / "src"))

import mdio_py.std.arrow_ops as ao


def _fake_create(_name, text):
    p = tempfile.mktemp(prefix="arrow_op_", suffix=".md")
    with open(p, "w", encoding="utf-8") as f:
        f.write(text)
    return p


def _fake_read(path):
    with open(path, encoding="utf-8") as f:
        return f.read()


ao.create_temp_doc = _fake_create
ao.read_temp_doc = _fake_read
from mdio_py.core.mem_slot import begin_exec_frame, end_exec_frame
class _TS:
    def mk(self, *a, **k): return None
    def rm(self, *a, **k): return None
class _W:
    temp_store = _TS()
_FRAME_TOKEN = begin_exec_frame(_W())
ao.current_temp_frame = lambda: _FRAME_TOKEN.frame


def run(text, sql, **kwargs):
    return asyncio.run(ao.arrow(None, text, [], {"-duckdb": sql, **kwargs}, None))


def main():
    text = "# A\n\n- x\n  - y\n"

    rows = json.loads(run(text, "select node, role, type from nodes order by node", **{"-rows": True}))
    assert rows["columns"] == ["node", "role", "type"], rows
    assert len(rows["rows"]) == 3, rows

    js = json.loads(run(text, "select node, raw from nodes order by node", **{"-json": True}))
    assert js[0]["node"] == 0 and js[0]["raw"].startswith("# A"), js

    default = run(text, "select node from doc_root")
    assert default == text, repr(default)

    for doc in ["- - foo\n", "> - bar\n", "> a\n> b\n", "# A\n\ntext\n\n## B\n\nmore\n"]:
        got = run(doc, "select node from doc_roots order by pos")
        assert got == doc, (repr(doc), repr(got))

    corpus = Path(__file__).resolve().parent / "corpus"
    for path in sorted(corpus.glob("*.md")):
        if path.name in {"phantom.md", "crlf.md"}:
            # phantom is NON_WF; crlf canonical render may normalize blank lines.
            continue
        doc = path.read_text(encoding="utf-8")
        got = run(doc, "select node from doc_roots order by pos")
        assert got == doc, (path.name, repr(doc), repr(got))

    no_node = run(text, "select raw from nodes", **{"-rows": True})
    assert '"raw"' in no_node and "ARROW_INVALID_RESULT" not in no_node, no_node

    two_faces = run(text, "select node, raw, content from nodes", **{"-rows": True})
    assert "ARROW_INVALID_RESULT" in two_faces, two_faces

    rejected = run(text, "select 1; copy (select 1) to '/tmp/arrow_should_not_exist.csv'", **{"-rows": True})
    assert "ARROW_SCRIPT_REJECTED" in rejected or "ARROW_SQL_REJECTED" in rejected, rejected
    rejected = run(text, "create table x(a integer)", **{"-rows": True})
    assert "ARROW_SCRIPT_REJECTED" in rejected or "ARROW_SQL_REJECTED" in rejected, rejected
    with_q = run(text, "with x as (select 1 as a) select * from x", **{"-rows": True})
    assert "ARROW_SQL_REJECTED" not in with_q, with_q

    print("v1b python-op conformance: OK")


if __name__ == "__main__":
    main()
