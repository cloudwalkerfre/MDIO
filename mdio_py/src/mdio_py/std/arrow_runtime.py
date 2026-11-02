"""Shared DuckDB runtime for arrow: lazy, exec-scoped, doc-session leases.

One TempDocFrame owns at most one DuckDBRuntime. The runtime is created on the
first arrow query, keeps a single in-memory DuckDB database and a base
connection, and maps content-hash snapshot keys to mdio sessions. Queries run on
per-query cursors; doc/runtime leases keep deletion from racing in-flight work.
"""
from __future__ import annotations

import hashlib
import threading
from contextlib import contextmanager
from dataclasses import dataclass
from typing import Any, Iterator, Optional


class ArrowRuntimeError(RuntimeError):
    pass


class RuntimeClosed(ArrowRuntimeError):
    pass


class RuntimeFailed(ArrowRuntimeError):
    pass


@dataclass
class DocSession:
    key: str
    sid: int
    refs: int = 0
    closing: bool = False
    queries: int = 0


class DuckDBRuntime:
    """One in-memory DuckDB database per exec frame."""

    def __init__(self, exec_id: str, ext_path: str) -> None:
        self.exec_id = str(exec_id)
        self.ext_path = str(ext_path)
        self.state = "starting"
        self.docs: dict[str, DocSession] = {}
        self.active_queries = 0
        self.fts_available = False
        self.fts_error: Optional[str] = None
        self._lock = threading.RLock()
        self._cond = threading.Condition(self._lock)
        self.base = None
        try:
            import duckdb
            self.base = duckdb.connect(config={"allow_unsigned_extensions": "true"})
            self.base.execute("LOAD '" + self.ext_path.replace("'", "''") + "'")
            # FTS must be loaded before external access is disabled. It is
            # optional: plain arrow relations keep working if fts is absent.
            try:
                self.base.execute("LOAD fts")
                self.fts_available = True
            except Exception as fts_e:
                self.fts_error = f"{type(fts_e).__name__}: {fts_e}"
            self.base.execute("SET enable_external_access=false")
            self.state = "active"
        except Exception:
            self._close_base_locked()
            self.state = "failed"
            raise

    # -- lifecycle ---------------------------------------------------------
    def _ensure_active(self) -> None:
        if self.state == "closing":
            raise RuntimeClosed("ARROW_RUNTIME_CLOSED: runtime is closing")
        if self.state == "closed":
            raise RuntimeClosed("ARROW_RUNTIME_CLOSED: runtime is closed")
        if self.state == "failed":
            raise RuntimeFailed("ARROW_RUNTIME_FAILED: runtime failed")
        if self.state != "active":
            raise RuntimeFailed("ARROW_RUNTIME_FAILED: runtime not active")

    @property
    def doc_count(self) -> int:
        with self._lock:
            return len(self.docs)

    # -- validation / setup -------------------------------------------------
    def validate_script(self, cmd: str) -> tuple[list[Any], Optional[str]]:
        """Validate one arrow script: temp setup, optional probe SELECTs, final SELECT.

        Policy (not SQL grammar):
          - final statement must be SELECT/WITH
          - earlier SELECTs are allowed; their results are consumed and discarded
          - earlier CREATEs must be TEMP TABLE / TEMP VIEW / TEMP MACRO
          - earlier INSERTs are allowed (they can only target visible tables)
          - earlier PRAGMAs are limited to create_fts_index / drop_fts_index
          - ATTACH/COPY/LOAD/INSTALL/SET/CALL/other PRAGMA/non-temp CREATE are rejected
        """
        import duckdb
        with self._lock:
            self._ensure_active()
            try:
                stmts = list(self.base.extract_statements(cmd))
            except Exception as e:
                return [], f"ARROW_SQL_REJECTED: parse failed: {type(e).__name__}: {e}"
        if not stmts:
            return [], "ARROW_SCRIPT_REJECTED: empty script"
        if stmts[-1].type != duckdb.StatementType.SELECT:
            return [], "ARROW_SCRIPT_REJECTED: final statement must be SELECT/WITH"
        for stmt in stmts[:-1]:
            if stmt.type == duckdb.StatementType.SELECT:
                continue
            q = self._statement_head(stmt.query)
            if stmt.type == duckdb.StatementType.CREATE and q.startswith(
                (
                    "CREATE TEMP TABLE", "CREATE TEMPORARY TABLE",
                    "CREATE TEMP VIEW", "CREATE TEMPORARY VIEW",
                    "CREATE TEMP MACRO", "CREATE TEMPORARY MACRO",
                )
            ):
                continue
            if stmt.type == duckdb.StatementType.INSERT and q.startswith("INSERT INTO "):
                continue
            if stmt.type == duckdb.StatementType.PRAGMA and q.startswith(
                ("PRAGMA CREATE_FTS_INDEX", "PRAGMA DROP_FTS_INDEX")
            ):
                continue
            return [], f"ARROW_SCRIPT_REJECTED: unsupported setup statement: {stmt.query[:120]}"
        return stmts, None

    @staticmethod
    def _statement_head(query: str) -> str:
        """Strip leading SQL comments for policy checks; keep it simple."""
        s = str(query)
        while True:
            s = s.lstrip()
            if s.startswith("--"):
                i = s.find("\n")
                s = "" if i < 0 else s[i + 1:]
                continue
            if s.startswith("/*"):
                i = s.find("*/", 2)
                s = "" if i < 0 else s[i + 2:]
                continue
            return s.upper()

    def execute_setup(self, cursor: Any, stmt: Any) -> None:
        """Execute one validated setup statement on the query cursor."""
        import duckdb
        if stmt.type == duckdb.StatementType.SELECT:
            # Intermediate SELECT is allowed but has no output channel; fully
            # consume it so its side effects/errors are deterministic.
            cursor.execute(stmt.query).fetchall()
        else:
            cursor.execute(stmt.query)

    # -- docs --------------------------------------------------------------
    @staticmethod
    def snapshot_key(text: str) -> str:
        return hashlib.sha256(str(text).encode("utf-8")).hexdigest()

    def open_doc(self, text: str) -> DocSession:
        key = self.snapshot_key(text)
        with self._lock:
            self._ensure_active()
            doc = self.docs.get(key)
            if doc is not None and not doc.closing:
                return doc
            if self.base is None:
                self.state = "failed"
                raise RuntimeFailed("ARROW_RUNTIME_FAILED: no base connection")
            try:
                sid = int(self.base.execute("select mdio_open(?)", [str(text)]).fetchone()[0])
            except Exception:
                raise
            doc = DocSession(key=key, sid=sid)
            self.docs[key] = doc
            return doc

    @staticmethod
    def blob_key(blob: bytes) -> str:
        return "blob:" + hashlib.sha256(bytes(blob)).hexdigest()

    def open_doc_blob(self, blob: bytes) -> DocSession:
        key = self.blob_key(blob)
        with self._lock:
            self._ensure_active()
            doc = self.docs.get(key)
            if doc is not None and not doc.closing:
                return doc
            if self.base is None:
                self.state = "failed"
                raise RuntimeFailed("ARROW_RUNTIME_FAILED: no base connection")
            sid = int(self.base.execute("select mdio_open_snapshot(?)", [bytes(blob)]).fetchone()[0])
            doc = DocSession(key=key, sid=sid)
            self.docs[key] = doc
            return doc


    def _open_many(self, kind: str, payloads, threads: int = 4):
        """Open many docs in parallel (cursor per worker), register under the lock.

        Sessions are keyed exactly like open_doc/open_doc_blob; duplicate payloads
        share one DocSession. Returned list follows the input order.
        """
        payloads = list(payloads)
        if kind == "text":
            key_fn = self.snapshot_key
            sql = "select mdio_open(?)"
        else:
            key_fn = self.blob_key
            sql = "select mdio_open_snapshot(?)"
        keys = [key_fn(p) for p in payloads]
        with self._lock:
            self._ensure_active()
            if self.base is None:
                self.state = "failed"
                raise RuntimeFailed("ARROW_RUNTIME_FAILED: no base connection")
            known = {k for k in keys if (d := self.docs.get(k)) is not None and not d.closing}
        pending = [(i, p) for i, (k, p) in enumerate(zip(keys, payloads)) if k not in known]
        opened: dict[str, DocSession] = {}
        if pending:
            errors: list = []
            guard = threading.Lock()

            def worker(items):
                try:
                    cur = self.base.cursor()
                    try:
                        for idx, payload in items:
                            sid = int(cur.execute(sql, [payload]).fetchone()[0])
                            with guard:
                                opened.setdefault(keys[idx], DocSession(key=keys[idx], sid=sid))
                    finally:
                        try:
                            cur.close()
                        except Exception:
                            pass
                except BaseException as exc:  # noqa: BLE001 - propagate to caller
                    with guard:
                        errors.append(exc)

            n = max(1, min(int(threads), len(pending)))
            chunks = [pending[i::n] for i in range(n)]
            ts = [threading.Thread(target=worker, args=(ch,), daemon=True) for ch in chunks]
            for t in ts:
                t.start()
            for t in ts:
                t.join()
            if errors:
                raise RuntimeFailed(f"ARROW_RUNTIME_FAILED: parallel open failed: {errors[0]}")
        with self._lock:
            self._ensure_active()
            for k in keys:
                doc = self.docs.get(k)
                if doc is None or doc.closing:
                    doc = opened.get(k)
                    if doc is None:
                        raise RuntimeFailed(f"ARROW_RUNTIME_FAILED: missing opened doc {k}")
                    self.docs[k] = doc
            return [self.docs[k] for k in keys]

    def open_docs_texts(self, texts, threads: int = 4):
        return self._open_many("text", texts, threads)

    def open_docs_blobs(self, blobs, threads: int = 4):
        return self._open_many("blob", blobs, threads)

    @contextmanager
    def query(self, doc: DocSession) -> Iterator[Any]:
        with self._lock:
            self._ensure_active()
            if doc.closing:
                raise RuntimeClosed("ARROW_RUNTIME_CLOSED: document is closing")
            doc.refs += 1
            doc.queries += 1
            self.active_queries += 1
            try:
                cur = self.base.cursor()
            except Exception:
                doc.refs -= 1
                doc.queries -= 1
                self.active_queries -= 1
                self._cond.notify_all()
                raise
        try:
            yield cur
        finally:
            try:
                cur.close()
            except Exception:
                pass
            with self._lock:
                doc.refs -= 1
                doc.queries -= 1
                self.active_queries -= 1
                self._cond.notify_all()

    def drop_doc(self, key: str) -> None:
        """Close one doc session after its in-flight queries drain.

        Current exec-framed arrow uses content-hash docs.
        """
        with self._lock:
            doc = self.docs.get(key)
            if doc is None:
                return
            doc.closing = True
            while doc.refs > 0:
                self._cond.wait()
            self.docs.pop(key, None)
            if self.base is not None and self.state in ("active", "closing"):
                try:
                    self.base.execute("select mdio_close(?)", [doc.sid])
                except Exception:
                    pass
            if not self.docs and self.state == "active" and self.active_queries == 0:
                self.state = "closing"
                self._close_base_locked()
                self.state = "closed"
                self._cond.notify_all()

    def close(self) -> None:
        with self._lock:
            if self.state == "closed":
                return
            if self.state == "closing":
                while self.state != "closed":
                    self._cond.wait()
                return
            if self.state == "failed":
                self.state = "closed"
                self._cond.notify_all()
                return
            self.state = "closing"
            while self.active_queries > 0:
                self._cond.wait()
            docs = list(self.docs.values())
            self.docs.clear()
            self._close_docs_locked(docs)
            self._close_base_locked()
            self.state = "closed"
            self._cond.notify_all()

    def _close_docs_locked(self, docs: list[DocSession]) -> None:
        if self.base is None:
            return
        for doc in docs:
            try:
                self.base.execute("select mdio_close(?)", [doc.sid])
            except Exception:
                pass

    def _close_base_locked(self) -> None:
        try:
            if self.base is not None:
                self.base.close()
        except Exception:
            pass
        finally:
            self.base = None


def get_runtime(frame: Any, ext_path: str) -> DuckDBRuntime:
    """Return the frame-scoped runtime, creating it lazily."""
    if frame is None:
        raise RuntimeClosed("ARROW_RUNTIME_CLOSED: no exec frame")
    lock = getattr(frame, "_duckdb_runtime_lock", None)
    if lock is None:
        lock = threading.RLock()
        try:
            frame._duckdb_runtime_lock = lock
        except Exception:
            pass
    with lock:
        rt = getattr(frame, "duckdb_runtime", None)
        if rt is not None and rt.state in ("active", "closing"):
            return rt
        rt = DuckDBRuntime(getattr(frame, "exec_id", "unknown"), ext_path)
        frame.duckdb_runtime = rt
        return rt
