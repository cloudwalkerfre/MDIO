"""arrow std op: md/current-wt docs -> shared DuckDB runtime -> SQL/script.

With pipe data: one md document. With no pipe data: docs metadata is selected
first (default select path from docs), then selected
docs are loaded via snapshot handoff into temp sessions. Parsed relations gain a
path key. User-facing views: docs, nodes, blocks (semantic parent/span), outline,
sections, section_member, pj, span, meta; edges/children/descendants are lower-level
relations. A script is temp setup (CREATE TEMP TABLE/VIEW/MACRO, INSERT) plus a
final SELECT; intermediate SELECTs are allowed and discarded. Default output renders
node ids per doc path through R0 render_md_selected; -rows/-json are explicit exits.
"""
from __future__ import annotations

import json
import os
import posixpath
import re
import uuid

from ..core.mem_slot import current_temp_frame
from .arrow_runtime import ArrowRuntimeError, RuntimeClosed, RuntimeFailed, get_runtime


def _err(msg: str) -> str:
    return f"- mdio_error: [arrow] {msg}"


def _is_err(x) -> bool:
    return isinstance(x, str) and x.startswith("- mdio_error:")


def _cmd_of(args, kwargs):
    if kwargs.get("-duckdb"):
        return kwargs["-duckdb"]
    if args:
        return args[0]
    return None


def _ext_path():
    p = os.environ.get("MDIO_FRAME_DUCKDB_EXT", "")
    if p:
        return p
    here = os.path.dirname(os.path.abspath(__file__))
    packaged = os.path.abspath(os.path.join(here, "..", "ext", "mdio_frame.duckdb_extension"))
    if os.path.isfile(packaged):
        return packaged
    return os.path.abspath(os.path.join(here, "..", "..", "..", "..",
                                       "mdio_core", "ext", "arrow", "mdio_frame.duckdb_extension"))


def _sql_lit(value) -> str:
    return "'" + str(value).replace("'", "''") + "'"


def _load_store_docs(workspace, wt=None):
    """Load current-wt documents through the VFS store API.

    Returns (docs, error) where docs = [(path, name, md)].  This is deliberately
    not a disk walk: expand_glob is the same VFS view (local + bound parent
    docs) that selectors see.
    """
    store = getattr(workspace, "store", None)
    if store is None:
        return [], "ARROW_NO_STORE: workspace has no store"
    wt = wt or getattr(workspace, "active_worktree_uid", None) or "genesis"
    try:
        paths = store.expand_glob(wt, "/**/*.md", False)
    except Exception as e:
        return [], f"ARROW_STORE_FAILED: {type(e).__name__}: {e}"
    docs = []
    for path in sorted(paths):
        try:
            if store.is_dir(wt, path):
                continue
            md = store.get_text(wt, path)
        except Exception as e:
            return [], f"ARROW_STORE_FAILED: {path}: {type(e).__name__}: {e}"
        docs.append((str(path), posixpath.basename(str(path)), "" if md is None else str(md)))
    if not docs:
        return [], f"ARROW_NO_DOCS: no markdown files under wt {wt!r}"
    return docs, None




def _load_store_docs_meta(workspace, wt=None):
    """Return docs(path,name,folder,stem) from the active worktree VFS; no md."""
    store = getattr(workspace, "store", None)
    if store is None:
        return [], "ARROW_NO_STORE: workspace has no store"
    wt = wt or getattr(workspace, "active_worktree_uid", None) or "genesis"
    try:
        paths = store.expand_glob(wt, "/**/*.md", False)
    except Exception as e:
        return [], f"ARROW_STORE_FAILED: {type(e).__name__}: {e}"
    rows = []
    for path in sorted(paths):
        try:
            if store.is_dir(wt, path):
                continue
        except Exception as e:
            return [], f"ARROW_STORE_FAILED: {path}: {type(e).__name__}: {e}"
        p = str(path)
        name = posixpath.basename(p)
        rows.append((p, name, posixpath.dirname(p), posixpath.splitext(name)[0]))
    return rows, None


def _load_store_docs_for_paths(workspace, paths, wt=None):
    """Load md only for selected VFS paths."""
    store = getattr(workspace, "store", None)
    if store is None:
        return [], "ARROW_NO_STORE: workspace has no store"
    wt = wt or getattr(workspace, "active_worktree_uid", None) or "genesis"
    out = []
    for path in paths:
        try:
            md = store.get_text(wt, path)
        except Exception as e:
            return [], f"ARROW_STORE_FAILED: {path}: {type(e).__name__}: {e}"
        out.append((str(path), posixpath.basename(str(path)), "" if md is None else str(md)))
    if not out:
        return [], "ARROW_NO_DOCS: selection produced no docs"
    return out, None


_FTS_PRAGMA = (
    "pragma create_fts_index('search_docs', 'search_id', 'text', "
    "tokenizer = 'opensearch_standard', stemmer = 'none', stopwords = 'none', "
    "overwrite = 1, layered_search = 1)"
)

_HITS_MACRO = """
create or replace macro search.hits(q) as table
select sd.path, sd.node, h.score
from fts_main_search_docs.search_layered_bm25(q, top_k := NULL) h
join search_docs sd on sd.search_id = h.docname::BIGINT
"""

_HITS_MACRO_EMPTY = """
create or replace macro search.hits(q) as table
select NULL::VARCHAR as path, NULL::BIGINT as node, NULL::DOUBLE as score
where false
"""

_RESULTS_MACRO = """
create or replace macro search.results(q) as table
with recursive h as (
  select path, node, score from search.hits(q)
),
scoped as (
  select d.path, d.section, h.node, h.score
  from h join search_docs d on d.path = h.path and d.node = h.node
),
agg as (
  select path, section, max(score) as score, count(*) as matched
  from scoped group by path, section
),
res as (
  select a.path, a.section, sec.header_node as node, a.score, a.matched
  from agg a join search_sections sec on sec.path = a.path and sec.section = a.section
),
anc as (
  select r.path, r.section, s.parent_section as cur
  from res r join search_sections s on s.path = r.path and s.section = r.section
  where s.parent_section is not null
  union all
  select a.path, a.section, s.parent_section
  from anc a join search_sections s on s.path = a.path and s.section = a.cur
  where a.cur is not null and s.parent_section is not null
)
select r.path, r.section, r.node, r.score, r.matched
from res r
where not exists (
  select 1 from anc a
  where a.path = r.path and a.cur = r.section
)
"""


def _mentions_search(cmd: str) -> bool:
    low = str(cmd).lower()
    return "search.blocks" in low or "search.hits" in low or "search.results" in low


def _mentions_search_fts(cmd: str) -> bool:
    low = str(cmd).lower()
    return "search.hits" in low or "search.results" in low


def _search_sessions_lit(docs) -> str:
    sessions = [{"path": str(path), "sid": int(sid)} for path, _name, _md, sid in docs]
    return _sql_lit(json.dumps(sessions, ensure_ascii=False))


_RELATION_DEPS = {
    "nodes": (),
    "edges": (),
    "children": ("edges",),
    "pj": ("nodes",),
    "descendants": (),
    "roots": (),
    "doc_root": ("docs",),
    "doc_roots": ("roots", "descendants"),
    "sections": ("nodes",),
    "section_member": (),
    "span": (),
    "blocks": ("nodes", "edges", "span"),
    "outline": ("sections",),
    "meta": (),
    "docs": (),
    "doc_sessions": (),
}
_ALWAYS_CREATED = {"docs", "doc_sessions"}
_FULL_SCAN_MARKERS = ("duckdb_views", "information_schema", "pragma", "explain")


def _script_relations(cmd) -> set:
    low = str(cmd).lower()
    if any(marker in low for marker in _FULL_SCAN_MARKERS):
        return set(_RELATION_DEPS)
    found = set()
    for name in _RELATION_DEPS:
        if re.search(r"(?<![a-z0-9_])" + re.escape(name) + r"(?![a-z0-9_])", low) is not None:
            found.add(name)
    changed = True
    while changed:
        changed = False
        for name in list(found):
            for dep in _RELATION_DEPS.get(name, ()):
                if dep not in found:
                    found.add(dep)
                    changed = True
    return found


def _views_for_script(cmd, docs):
    needed = _script_relations(cmd)
    if needed == set(_RELATION_DEPS):
        return _views(docs)
    out = []
    for ddl in _views(docs):
        m = re.match(r"create temp (?:table|view) ([a-z_]+)", ddl)
        name = m.group(1) if m else ""
        if name in _ALWAYS_CREATED or name in needed:
            out.append(ddl)
    return out


def _search_prelude(cur, docs, need_fts: bool) -> None:
    """Hidden setup for the user-facing search layer (arrow-internal).

    The complete search rows (own-text included) and section rows are computed
    once by the parallel Rust table functions; SQL only exposes views/macros.
    """
    lit = _search_sessions_lit(docs)
    cur.execute("create schema if not exists search")
    cur.execute("drop view if exists search.blocks")
    cur.execute(
        "create temp table search_docs as "
        "select row_number() over () as search_id, * from mdio_blocks_all(" + lit + ")"
    )
    cur.execute(
        "create view search.blocks as "
        "select path, node, section, type, heading, tag, parent, pos, "
        "start_line, end_line, text from search_docs"
    )
    if not need_fts:
        return
    cur.execute(
        "create temp table search_sections as "
        "select path, section, parent_section, header_node from mdio_sections_all(" + lit + ")"
    )
    row = cur.execute("select count(*) from search_docs").fetchone()
    total = int(row[0]) if row and row[0] is not None else 0
    if total > 0:
        cur.execute(_FTS_PRAGMA)
        cur.execute(_HITS_MACRO)
    else:
        cur.execute(_HITS_MACRO_EMPTY)
    cur.execute(_RESULTS_MACRO)


def _views(docs):
    """Create docs + per-doc union views keyed by VFS path."""
    rows = ",\n".join(
        "(%s, %s, %s, %s)" % (
            _sql_lit(path), _sql_lit(name),
            _sql_lit(posixpath.dirname(str(path))),
            _sql_lit(posixpath.splitext(name)[0]),
        )
        for path, name, _md, _sid in docs
    )
    sessions = ",\n".join(
        "(%s, %d)" % (_sql_lit(path), sid) for path, _name, _md, sid in docs
    )

    def union(select_tpl):
        return "\nunion all\n".join(
            select_tpl.format(path=_sql_lit(path), sid=sid)
            for path, _name, _md, sid in docs
        )

    nodes = union(
        "select {path} as path, node, role, type, level, frame_ordinal, section, "
        "can_read, can_write, raw, mark, tag, content, container from mdio_faces({sid})"
    )
    sections = union("select {path} as path, * from mdio_sections({sid})")
    members = union("select {path} as path, node, section from mdio_section_member({sid})")
    descs = union(
        "select {path} as path, ancestor, descendant, depth from mdio_descendants({sid})"
    )
    roots = union("select {path} as path, node, pos from mdio_roots({sid})")
    edges = union("select {path} as path, parent, child, pos from mdio_edges({sid})")
    spans = union(
        "select {path} as path, node, start_line, end_line, byte_start, byte_end "
        "from mdio_span({sid})"
    )
    return (
        "create temp table docs(path VARCHAR, name VARCHAR, folder VARCHAR, stem VARCHAR);\n"
        "insert into docs values\n" + rows + ";",
        "create temp table doc_sessions(path VARCHAR, sid BIGINT);\n"
        "insert into doc_sessions values\n" + sessions + ";",
        "create temp view nodes as\n" + nodes,
        "create temp view pj as\n"
        "select path, node, 'raw' as pj, raw as value from nodes where (can_read & 1) <> 0\n"
        "union all select path, node, 'mark', mark from nodes where (can_read & 2) <> 0\n"
        "union all select path, node, 'tag', tag from nodes where (can_read & 4) <> 0\n"
        "union all select path, node, 'content', content from nodes where (can_read & 8) <> 0\n"
        "union all select path, node, 'container', container from nodes where (can_read & 16) <> 0",
        "create temp view edges as\n" + edges,
        "create temp view children as select path, parent, child, pos from edges",
        "create temp view descendants as\n" + descs,
        "create temp view roots as\n" + roots,
        "create temp view doc_root as select path, (-1)::BIGINT as node from docs",
        "create temp view doc_roots as\n"
        "select r.path, r.node, r.pos\n"
        "from roots r\n"
        "where not exists (\n"
        "  select 1 from descendants d\n"
        "  join roots a on a.path = d.path and a.node = d.ancestor\n"
        "  where d.path = r.path and d.descendant = r.node and d.ancestor <> r.node\n"
        ")",
        "create temp view sections as\n"
        "select s.path, s.section, s.parent_section, s.level,\n"
        "       s.start as start_line, s.\"end\" as end_line,\n"
        "       s.start, s.\"end\", s.depth, s.in_list,\n"
        "       s.header_node, s.header_node as node, coalesce(h.tag, '') as heading\n"
        "from (" + sections + ") s\n"
        "left join nodes h on h.path = s.path and h.node = s.header_node",
        "create temp view section_member as\n" + members,
        "create temp view span as\n" + spans,
        "create temp view blocks as\n"
        "select n.path, n.node, n.role, n.type, n.level, n.section, n.frame_ordinal,\n"
        "       s.start_line, s.end_line,\n"
        "       (select min(e.parent) from edges e where e.path = n.path and e.child = n.node) as parent,\n"
        "       n.content\n"
        "from nodes n left join span s on s.path = n.path and s.node = n.node",
        "create temp view outline as\n"
        "select path, section, parent_section, level, start_line, end_line, header_node, heading from sections",
        "create temp view meta as select * from mdio_meta()",
    )


def _json_rows(cols, rows):
    return [dict(zip(cols, row)) for row in rows]


def _check_result(cols):
    faces = [c for c in ("raw", "mark", "tag", "content", "container") if c in cols]
    if len(faces) > 1:
        return "ARROW_INVALID_RESULT: result must expose at most one face; use pj or separate queries"
    return None


async def arrow(workspace, pipe_data, args, kwargs, ctx):
    if _is_err(pipe_data):
        return pipe_data

    cmd = _cmd_of(args, kwargs)
    if not isinstance(cmd, str) or not cmd.strip():
        return _err("用法:'md' | arrow -duckdb '<SQL>' [-rows [N] | -json]")

    want_rows = "-rows" in kwargs or "-json" in kwargs
    store_mode = pipe_data is None
    store_wt = (
        getattr(ctx, "wt_uid", None)
        or getattr(workspace, "active_worktree_uid", None)
        or "genesis"
    )

    frame = current_temp_frame()
    if frame is None:
        return _err("arrow 需要在 MdioRuntime.exec 的 temp doc frame 内执行")

    try:
        ext_path = _ext_path()
        if not os.path.isfile(ext_path):
            return _err(f"DuckDB 扩展不存在: {ext_path}")
        runtime = get_runtime(frame, ext_path)
        stmts, sql_err = runtime.validate_script(cmd)
        if sql_err:
            return _err(sql_err)

        snapshot_docs = None
        raw_docs = None
        if store_mode:
            meta, meta_err = _load_store_docs_meta(workspace, store_wt)
            if meta_err:
                return _err(meta_err)
            docs_sql = "select path from docs"
            if docs_sql is not None:
                meta_doc = runtime.open_doc("")
                with runtime.query(meta_doc) as pcur:
                    pcur.execute("create temp table docs(path VARCHAR, name VARCHAR, folder VARCHAR, stem VARCHAR)")
                    if meta:
                        pcur.executemany("insert into docs values (?, ?, ?, ?)", meta)
                    try:
                        srel = pcur.sql(docs_sql)
                        selected_paths = [str(r[0]) for r in srel.fetchall()]
                    except Exception as e:
                        return _err(f"ARROW_DOCS_REJECTED: {type(e).__name__}: {e}")
            else:
                selected_paths = [p for p, _n, _f, _s in meta]

            src_store = getattr(workspace, "store", None)
            tmp_store = getattr(workspace, "temp_store", None)
            if (selected_paths and src_store is not None and tmp_store is not None
                    and hasattr(tmp_store, "handoff_from")):
                try:
                    wt = store_wt
                    prefix = frame.root + "__handoff_" + uuid.uuid4().hex + "/"
                    batch = tmp_store.handoff_from(src_store, wt, selected_paths, prefix)
                    pairs = list(zip(selected_paths, batch))
                    if len(pairs) >= 16:
                        blobs = [blob for _src, (_dest, blob) in pairs]
                        sessions = runtime.open_docs_blobs(blobs, threads=4)
                        snapshot_docs = [
                            (src_path, posixpath.basename(src_path), doc)
                            for (src_path, _batch), doc in zip(pairs, sessions)
                        ]
                    else:
                        snapshot_docs = []
                        for src_path, (_dest, blob) in pairs:
                            doc = runtime.open_doc_blob(blob)
                            snapshot_docs.append((src_path, posixpath.basename(src_path), doc))
                except Exception as e:
                    return _err(f"ARROW_HANDOFF_FAILED: {type(e).__name__}: {e}")
            else:
                if docs_sql is not None:
                    raw_docs, doc_err = _load_store_docs_for_paths(workspace, selected_paths, store_wt)
                else:
                    raw_docs, doc_err = _load_store_docs(workspace, store_wt)
                if doc_err:
                    return _err(doc_err)
        else:
            text = pipe_data if isinstance(pipe_data, str) else str(pipe_data)
            raw_docs = [("input", "input", text)]

        docs = []
        primary_doc = None
        if snapshot_docs is not None:
            for path, name, doc in snapshot_docs:
                if primary_doc is None:
                    primary_doc = doc
                docs.append((path, name, "", doc.sid))
        else:
            if len(raw_docs) >= 16:
                sessions = runtime.open_docs_texts(
                    [md for _path, _name, md in raw_docs], threads=4
                )
                for (path, name, md), doc in zip(raw_docs, sessions):
                    if primary_doc is None:
                        primary_doc = doc
                    docs.append((path, name, md, doc.sid))
            else:
                for path, name, md in raw_docs:
                    doc = runtime.open_doc(md)
                    if primary_doc is None:
                        primary_doc = doc
                    docs.append((path, name, md, doc.sid))

        with runtime.query(primary_doc) as cur:
            search_mode = _mentions_search(cmd)
            for ddl in (_views_for_script(cmd, docs) if search_mode else _views(docs)):
                cur.execute(ddl)

            if search_mode:
                need_fts = _mentions_search_fts(cmd)
                if need_fts and not runtime.fts_available:
                    return _err(
                        "ARROW_SEARCH_UNAVAILABLE: "
                        + str(runtime.fts_error or "fts extension not loaded")
                    )
                try:
                    _search_prelude(cur, docs, need_fts)
                except Exception as e:
                    return _err(f"ARROW_SEARCH_FAILED: {type(e).__name__}: {e}")

            for stmt in stmts[:-1]:
                runtime.execute_setup(cur, stmt)
            rel = cur.sql(stmts[-1].query)
            cols = list(rel.columns)
            rows = rel.fetchall()
            if not want_rows:
                if "node" not in cols:
                    return _err("ARROW_INVALID_RESULT: 默认 md 需要 node 列（选择节点；全篇请用 roots）")
                ni = cols.index("node")
                pi = cols.index("path") if "path" in cols else None
                if pi is None and len(docs) > 1:
                    return _err("ARROW_INVALID_RESULT: 多文档默认 md 需要 path 列")
                single_path = docs[0][0] if len(docs) == 1 else ""
                groups = {}
                order = []
                for row in rows:
                    v = row[ni]
                    if v is None:
                        return _err("ARROW_INVALID_RESULT: node 不能为 NULL")
                    path = single_path if pi is None or row[pi] is None else str(row[pi])
                    if path not in groups:
                        groups[path] = []
                        order.append(path)
                    groups[path].append(int(v))
                sid_by_path = {path: sid for path, _name, _md, sid in docs}
                blocks = []
                for path in order:
                    sid = sid_by_path.get(path)
                    if sid is None:
                        return _err(f"ARROW_INVALID_RESULT: unknown doc path {path!r}")
                    md = cur.execute("select mdio_render_selected(?, ?)",
                                     [sid, json.dumps(groups[path])]).fetchone()[0]
                    if path and not (len(docs) == 1 and path == "input"):
                        blocks.append(f"## {path}\n\n{md}")
                    else:
                        blocks.append(str(md))
                return "\n\n".join(blocks)
            err = _check_result(cols)
            if err:
                return _err(err)
            rv = kwargs.get("-rows")
            limit = int(rv) if isinstance(rv, (int, str)) and not isinstance(rv, bool) and str(rv).isdigit() else 0
            if limit:
                rows = rows[:limit]
            if "-json" in kwargs:
                out = json.dumps(_json_rows(cols, rows), ensure_ascii=False, default=str)
            else:
                out = json.dumps({"columns": cols, "rows": [[str(v) if v is not None else None for v in row] for row in rows]},
                                 ensure_ascii=False)
            return out
    except (RuntimeClosed, RuntimeFailed, ArrowRuntimeError) as e:
        return _err(str(e))
    except Exception as e:
        return _err(f"{type(e).__name__}: {e}")
