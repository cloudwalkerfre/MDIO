"""MDIO Telemetry — Python-side wrapper for the Rust telemetry framework.

Module-level trace() and span() are **lazy** — they only activate
when a TelemetrySession is pushed onto the thread-local stack.
No event loop, no MdioRuntime, no store needed at definition time.

Usage:
    from mdio_py.telemetry import trace, span, TelemetrySession

    # Module-level — safe, no runtime needed
    @trace("1_Compiler/1.Lark")
    def lark_parse(s): ...

    @trace("write_locator", category="Rust_FFI")
    def write_locator(...): ...

    # Inside test / async main:
    sess = TelemetrySession(store, "my_bench", mode="bench")
    with sess:   # activates — thread-local push
        ...       # all @trace + with span() now recorded
    # sess.__exit__ — deactivates, pops from thread-local
    sess.report()
"""
import os, json, functools, inspect, threading, webbrowser, time

# Thread-local session stack

_tls = threading.local()

def _stack():
    if not hasattr(_tls, 'sessions'):
        _tls.sessions = []
    return _tls.sessions

def _active():
    s = _stack()
    return s[-1] if s else None

# Polars 分析层 — data_fn / render_fn 槽，缺省即用

def _builtin_data(df_raw: "pl.DataFrame") -> "pl.DataFrame":
    """缺省提取：group_by(name) → cnt, total_ms, avg/p50/p99 → 排序。"""
    import polars as pl
    return (df_raw.group_by("name")
            .agg([
                pl.col("dur_ns").count().alias("cnt"),
                (pl.col("dur_ns").sum() / 1_000_000.0).round(2).alias("total_ms"),
                (pl.col("dur_ns").mean() / 1_000.0).round(1).alias("avg_us"),
                (pl.col("dur_ns").median() / 1_000.0).round(1).alias("p50_us"),
                (pl.col("dur_ns").quantile(0.99) / 1_000.0).round(1).alias("p99_us"),
            ])
            .sort("total_ms", descending=True))

def _builtin_render(df: "pl.DataFrame") -> str:
    """缺省渲染：tabulate grid。"""
    try:
        import tabulate
        return tabulate.tabulate(df.to_numpy(), headers=df.columns, tablefmt="grid")
    except ImportError:
        return str(df)

def _builtin_header(df: "pl.DataFrame") -> str:
    """固定 header — df_out → str, 不可替换。"""
    try:
        total_str = f"{df['total_ms'].sum():.1f}"
    except Exception:
        total_str = "?"
    try:
        cnt = len(df)
        tops = df.head(3)
        hotspots = "  ·  ".join(
            f"{n} {v:.0f}µs" for n, v in zip(
                tops["name"].to_list(),
                (tops["total_ms"] / tops["cnt"] * 1_000).to_list()))
    except Exception:
        # 自定义 data_fn 输出无 name 列时降级 — 不强制同时定制 header_fn
        cnt = len(df)
        hotspots = "(自定义聚合)"
    return f"""============================================================
  bench · {cnt} phases · total {total_str}ms
  Hotspots: {hotspots}
============================================================"""

def _builtin_footer(df: "pl.DataFrame", wall_ms: float) -> str:
    """固定 footer — df_out, wall_ms → str, 不可替换。"""
    import platform, os
    cpu  = os.cpu_count() or 1
    arch = platform.machine()
    return f"  {len(df)} rows · wall: {wall_ms:.1f}ms · {arch} · {cpu} cores · release"

def _builtin_layer_data(layer_rows: list[dict]) -> "pl.DataFrame":
    """layer 样本 → 按 layer_name 聚合 avg/p50/p99 (pipeline latency 段)。"""
    import polars as pl
    df = pl.DataFrame(layer_rows)
    return (df.group_by("layer_name")
            .agg([
                pl.col("dur_ns").count().alias("cnt"),
                (pl.col("dur_ns").mean() / 1_000.0).round(1).alias("avg_us"),
                (pl.col("dur_ns").median() / 1_000.0).round(1).alias("p50_us"),
                (pl.col("dur_ns").quantile(0.99) / 1_000.0).round(1).alias("p99_us"),
            ])
            .sort("avg_us", descending=True))

def trace_event_cap() -> int:
    """读取 trace 事件上限（与 Rust MDIO_TELEMETRY_MAX_EVENTS 同源）。

    供分析脚本做 cap 守卫；缺省/非法值回退 4096。
    """
    raw = os.environ.get("MDIO_TELEMETRY_MAX_EVENTS", "4096")
    try:
        return max(1, int(raw))
    except Exception:
        return 4096


# 用户可覆盖的槽 — 设一次，覆盖内置
data_fn        = _builtin_data
render_fn      = _builtin_render
layer_fn       = _builtin_layer_data
header_fn      = _builtin_header
footer_fn      = _builtin_footer

# report() — 三段式终端报告

def report(df_raw: "pl.DataFrame", wall_ms: float = 0, layer_rows: list[dict] | None = None,
           mode: str | None = None) -> str:
    """header_fn(df_out) + render_fn(df_out) + footer_fn(df_out, wall_ms).

    layer_rows (layer_stats 样本) 非空时追加 pipeline latency 段 (layer_fn 槽) 与
    各端上报状态行。mode ("trace"/"bench") 传入时替换默认 header 的模式词。
    """
    df_out = data_fn(df_raw)
    out = f"{header_fn(df_out)}\n{render_fn(df_out)}\n{footer_fn(df_out, wall_ms)}"
    if mode and "bench ·" in out:
        out = out.replace("bench ·", f"{mode} ·", 1)
    if layer_rows:
        try:
            ldf = layer_fn(layer_rows)
            out += f"\n\n=== Pipeline Latency (layers) ===\n{render_fn(ldf)}\n{_flush_status(layer_rows)}"
        except Exception:
            pass
    return out


def _flush_status(layer_rows: list[dict]) -> str:
    """各端上报状态 — ✓ 该端有样本, pending 尚未 flush。"""
    names = {r.get("layer_name", "") for r in layer_rows}
    py_ok = any(n.startswith("PY_") or n.startswith("RS_") for n in names)
    js_ok = any(n.startswith("JS_") or n.startswith("WASM_") for n in names)
    def mark(ok: bool) -> str:
        return "✓" if ok else "pending"
    return f"  PY + RS: {mark(py_ok)}  |  JS + WASM: {mark(js_ok)}"

# HTML report — html_render_fn slot, notebook kit default

html_render_fn = None  # None → _builtin_html_fallback


def _builtin_html_fallback(df_out):
    """Plain HTML table (dark theme).  Override html_render_fn for notebook kit output."""
    cols = df_out.columns
    th = "".join(f'<th style="text-align:left;padding:4px 10px;border-bottom:1px solid #30363d;color:#8b949e;font-size:12px">{c}</th>' for c in cols)
    trs = "\n".join(
        "<tr>" + "".join(f'<td style="padding:4px 10px;border-bottom:1px solid #21262d">{v}</td>' for v in row) + "</tr>"
        for row in df_out.iter_rows())
    return (f'<table style="border-collapse:collapse;font-family:monospace;font-size:13px;width:100%">'
            f'<caption style="padding:8px;color:#8b949e;font-size:12px">{len(df_out)} phases</caption>'
            f'<thead><tr>{th}</tr></thead><tbody>{trs}</tbody></table>')


def report_html(df_raw, wall_ms=0, title="MDIO Telemetry"):
    """Generate HTML via html_render_fn slot.  Default: plain table.

    Set ``mdio_py.telemetry.html_render_fn`` to use the notebook kit.
    """
    df_out = data_fn(df_raw)
    if df_out is None or len(df_out) == 0:
        return "<html><body><p>No telemetry data.</p></body></html>"
    render = html_render_fn or _builtin_html_fallback
    body = render(df_out)
    return f"<!DOCTYPE html><html><head><meta charset=utf-8><title>{title}</title></head><body>{body}</body></html>"

# Micro HTTP server + JupyterLite fromURL

_TELEMETRY_PORT = 18766

_telemetry_server_ref = None  # (server, thread, shutdown_event)

# JupyterLite 站选择 (实测：fromURL 扩展与 pyodide kernel 的有无如下):
# - jupyter.org/try-jupyter: fromURL 扩展 ✓ + pyodide kernel ✓ (完整)
# - jupyterlab-open-url-param.readthedocs.io: fromURL ✓, 无 kernel
# - jupyterlite.github.io/demo 与 jupyterlite.readthedocs.io/_static/lab: kernel ✓, 无 fromURL 扩展
JUPYTERLITE_LAB = "https://jupyter.org/try-jupyter/lab/index.html"

# 构建产物 (Observable kit) 的代码块折叠注入 — serve 时变换, 不动产物文件
_OBS_COLLAPSE_INJECT = """
<style>
  details.obs-collapse > summary { cursor:pointer; font-size:12px; color:#8b949e; padding:2px 4px; user-select:none; }
  details.obs-collapse > summary::before { content:'\\u25B8 '; }
  details.obs-collapse[open] > summary::before { content:'\\u25BE '; }
  details.obs-collapse:not([open]) > pre { display: none !important; }
</style>
<script>
(function () {
  var wrap = function () {
    document.querySelectorAll("main > pre").forEach(function (p) {
      if (p.dataset.wrapped) return;
      p.dataset.wrapped = "1";
      var d = document.createElement("details");
      d.className = "obs-collapse";
      var s = document.createElement("summary");
      s.textContent = "analysis code";
      p.before(d); d.appendChild(s); d.appendChild(p);
    });
  };
  new MutationObserver(wrap).observe(document.body, { childList: true, subtree: true });
  wrap();
})();
</script>
"""


def _start_telemetry_server() -> str:
    """Start a micro HTTP server serving .telemetry/ on 127.0.0.1.

    Returns base URL.  Idempotent.  Daemon 线程随进程退出 (gateway 内常驻).
    """
    global _telemetry_server_ref
    if _telemetry_server_ref is not None:
        _server, _thread, _shutdown = _telemetry_server_ref
        if _thread.is_alive():
            return f"http://127.0.0.1:{_server.server_address[1]}"

    import http.server, socket, urllib.parse

    root = _telemetry_root()
    os.makedirs(root, exist_ok=True)
    # kit 报告绑定: 打开 kit 时 URL 带 ?session=&ts= → data/session.json 返回该次报告数据
    kit_binding = [None]  # 闭包状态: (session, ts)

    class _Handler(http.server.SimpleHTTPRequestHandler):
        def __init__(self, *a, **kw):
            super().__init__(*a, directory=root, **kw)

        def do_GET(self):
            path = self.path.split("?", 1)[0]
            # kit 页面带绑定参数 → 记录
            if path.rstrip("/").endswith("/.observable/dist/index.html") and "?" in self.path:
                qs = urllib.parse.parse_qs(self.path.split("?", 1)[1])
                if qs.get("session") and qs.get("ts"):
                    kit_binding[0] = (qs["session"][0], qs["ts"][0])
            # 绑定报告数据: data/session.json → .telemetry/<session>/data_<ts>.json
            if path.rstrip("/").endswith("/.observable/dist/data/session.json") and kit_binding[0]:
                sess, ts = kit_binding[0]
                fp = os.path.join(_telemetry_root(), _sanitize(sess), f"data_{ts}.json")
                if os.path.exists(fp):
                    import json as _json, platform
                    rows = _json.load(open(fp, encoding="utf-8"))
                    # data_<ts>.json 是裸数组 → 包装为 kit 的 session.json 结构
                    content = _json.dumps({
                        "rows": rows,
                        "layers": [],
                        "session": sess,
                        "wall_ms": 0,
                        "total_ms": sum(r.get("total_ms", 0) for r in rows),
                        "phases": len(rows),
                        "arch": platform.machine(),
                        "cpu": os.cpu_count() or 1,
                    })
                    data = content.encode("utf-8")
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                    return
            # 构建产物 index.html: 注入代码折叠支持 (serve 时变换, 不动产物)
            if path.rstrip("/").endswith("/.observable/dist/index.html"):
                try:
                    with open(self.translate_path(path), "rb") as f:
                        content = f.read().decode("utf-8")
                    content = content.replace("</body>", _OBS_COLLAPSE_INJECT + "</body>")
                    data = content.encode("utf-8")
                    self.send_response(200)
                    self.send_header("Content-Type", "text/html; charset=utf-8")
                    self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    self.wfile.write(data)
                    return
                except Exception:
                    pass
            super().do_GET()

        def translate_path(self, path):
            # /notebook/... → package 内 Observable kit (treemap/sunburst + data)
            if path.startswith("/notebook/"):
                nb = os.path.join(_notebook_root(), path[len("/notebook/"):])
                # 构建产物 (.observable/dist) 的 FileAttachment data 请求 →
                # 映射到 notebook/data (活数据, 无软链依赖, dist 重建不受影响)
                if "/.observable/dist/data/" in path:
                    return os.path.join(_notebook_root(), "data", path.rsplit("/data/", 1)[1])
                return nb
            return super().translate_path(path)

        def end_headers(self):
            self.send_header("Access-Control-Allow-Origin", "*")
            super().end_headers()

        def log_message(self, format, *args):
            pass

    port = _TELEMETRY_PORT
    for _ in range(10):
        try:
            server = http.server.HTTPServer(("127.0.0.1", port), _Handler)
            break
        except socket.error:
            port += 1
    else:
        raise RuntimeError("no free port for telemetry server")

    server.timeout = 2

    shutdown_event = threading.Event()

    def _serve():
        server.timeout = 2
        while not shutdown_event.is_set():
            try:
                server.handle_request()
            except Exception:
                pass
        server.server_close()

    t = threading.Thread(target=_serve, daemon=True)
    t.start()
    _telemetry_server_ref = (server, t, shutdown_event)
    return f"http://127.0.0.1:{port}"


def open_ipynb(ipynb_path: str, launch: bool = True):
    """Open .ipynb in JupyterLite via fromURL.

    Starts a micro HTTP server, constructs the fromURL, opens browser.
    Notebook auto-loads — zero drag-and-drop.
    """
    base = _start_telemetry_server()
    rel = os.path.relpath(ipynb_path, _telemetry_root())

    from urllib.parse import quote
    file_url = f"{base}/{quote(rel)}"
    jl_url = f"{JUPYTERLITE_LAB}?fromURL={quote(file_url, safe='')}"

    print(f"  JupyterLite: {jl_url}（仅进程存活期有效）")
    if launch:
        try:
            webbrowser.open(jl_url)
        except Exception:
            print(f"    (open manually in browser)")


def notebook_kit_url(session: str | None = None, ts: str | None = None) -> str:
    """Observable kit URL (treemap 动画 + sunburst) — 数据来自 package 内 data/.

    session/ts 指定时绑定该次报告数据 (server 端 data/session.json 返回
    .telemetry/<session>/data_<ts>.json), 否则显示最新报告.
    """
    base = _start_telemetry_server()
    url = f"{base}/notebook/.observable/dist/index.html"
    if session and ts:
        from urllib.parse import quote
        url += f"?session={quote(session)}&ts={quote(ts)}"
    return url


def open_notebook_kit(launch: bool = True, session: str | None = None, ts: str | None = None):
    """Open the Observable notebook kit in the browser."""
    url = notebook_kit_url(session, ts)
    print(f"  Notebook Kit: {url}（仅进程存活期有效）")
    if launch:
        try:
            webbrowser.open(url)
        except Exception:
            print(f"    (open manually in browser)")

# to_ipynb — 固定 analysis + user space

def to_ipynb(df_raw, title="MDIO Telemetry", wall_ms=0, layer_rows: list[dict] | None = None):
    """Generate .ipynb with fixed analysis cells + user exploration space.

    *df_raw* is the raw span DataFrame (columns: name, dur_ns, ...).
    *layer_rows* (layer_stats 样本) 非空时追加 Pipeline Latency 分析 cell.

    Cell layout:
      0. Markdown — session summary
      1. Code — load embedded raw data, display df_raw
      2. Code — three-section analysis on df_raw (group_by → df_phase)
      3. Code — pipeline latency (layers), 仅当 layer_rows 非空
      4. Markdown — separator / user space
      5. Code — empty, user starts here
    """
    import json as _json
    rows = df_raw.to_dicts() if hasattr(df_raw, 'to_dicts') else df_raw
    total_ns = sum(r.get("dur_ns", 0) for r in rows)
    n_spans = len(rows)
    data_json = _json.dumps(rows)
    layer_json = _json.dumps(layer_rows or [])

    summary_extra = f" · **{len(layer_rows)}** layer samples" if layer_rows else ""
    cells = [
        # Cell 0: summary
        {
            "cell_type": "markdown",
            "metadata": {},
            "source": [
                f"# MDIO Telemetry: {title}\n",
                f"\n",
                f"**{n_spans}** spans · total **{total_ns / 1_000_000:.1f} ms** · wall **{wall_ms:.0f} ms**{summary_extra}\n",
                f"\n",
                f"> Raw data is embedded in the next cell (columns: name, dur_ns, ...). "
                f"All analysis runs in-browser via Pyodide.\n",
            ],
        },
        # Cell 1: load raw data
        {
            "cell_type": "code",
            "metadata": {},
            "outputs": [],
            "source": [
                "import json, polars as pl\n",
                "\n",
                f"data   = json.loads({_json.dumps(data_json)})\n",
                "df_raw = pl.DataFrame(data)\n",
                "df_raw\n",
            ],
        },
        # Cell 2: three-section analysis
        {
            "cell_type": "code",
            "metadata": {},
            "outputs": [],
            "source": [
                "# ── 1. By-phase summary ──\n",
                "df_phase = (\n",
                "    df_raw.group_by(\"name\")\n",
                "    .agg([\n",
                "        pl.col(\"dur_ns\").count().alias(\"count\"),\n",
                "        (pl.col(\"dur_ns\").sum() / 1_000_000).round(2).alias(\"total_ms\"),\n",
                "        (pl.col(\"dur_ns\").mean() / 1_000).round(1).alias(\"avg_us\"),\n",
                "        (pl.col(\"dur_ns\").median() / 1_000).round(1).alias(\"p50_us\"),\n",
                "        (pl.col(\"dur_ns\").quantile(0.99) / 1_000).round(1).alias(\"p99_us\"),\n",
                "    ])\n",
                "    .sort(\"total_ms\", descending=True)\n",
                ")\n",
                "df_phase\n",
                "\n",
                "# ── 2. Bar chart (avg µs per phase) ──\n",
                "import matplotlib.pyplot as plt\n",
                "top = df_phase.head(20).reverse()\n",
                "fig, ax = plt.subplots(figsize=(10, max(4, len(top) * 0.25)))\n",
                "ax.barh(top[\"name\"].to_list(), top[\"avg_us\"].to_list(), color=\"#e94560\")\n",
                "ax.set_xlabel(\"avg µs\")\n",
                "plt.tight_layout()\n",
                "plt.show()\n",
                "\n",
                "# ── 3. Top 10 by total time ──\n",
                "df_phase.head(10)\n",
            ],
        },
    ]
    if layer_rows:
        cells.append({
            "cell_type": "code",
            "metadata": {},
            "outputs": [],
            "source": [
                "# ── 4. Pipeline Latency (layers) ──\n",
                f"layer_data = json.loads({_json.dumps(layer_json)})\n",
                "df_layer = pl.DataFrame(layer_data)\n",
                "layer_summary = (df_layer.group_by(\"layer_name\")\n",
                "    .agg([\n",
                "        pl.col(\"dur_ns\").count().alias(\"cnt\"),\n",
                "        (pl.col(\"dur_ns\").mean() / 1_000).round(1).alias(\"avg_us\"),\n",
                "        (pl.col(\"dur_ns\").median() / 1_000).round(1).alias(\"p50_us\"),\n",
                "        (pl.col(\"dur_ns\").quantile(0.99) / 1_000).round(1).alias(\"p99_us\"),\n",
                "    ])\n",
                "    .sort(\"avg_us\", descending=True))\n",
                "layer_summary\n",
                "\n",
                "top_layer = layer_summary.head(15).reverse()\n",
                "fig2, ax2 = plt.subplots(figsize=(10, max(3, len(top_layer) * 0.25)))\n",
                "ax2.barh(top_layer[\"layer_name\"].to_list(), top_layer[\"avg_us\"].to_list(), color=\"#4ecdc4\")\n",
                "ax2.set_xlabel(\"avg µs\")\n",
                "plt.tight_layout()\n",
                "plt.show()\n",
            ],
        })
    cells.extend([
        # Cell 3: separator
        {
            "cell_type": "markdown",
            "metadata": {},
            "source": [
                "---\n",
                "## Your Analysis\n",
                "\n",
                "Add your own cells below. `df_raw` contains all span events.\n",
                "Use `df_phase` for pre-aggregated per-phase data.\n",
                "`df_layer` / `layer_summary` (if present) hold cross-stack pipeline latency.\n",
                "\n",
                "```python\n",
                "# Example: filter by category\n",
                "# df_raw.filter(pl.col(\"name\").str.starts_with(\"Rust\"))\n",
                "```\n",
            ],
        },
        # Cell 4: user entry point
        {
            "cell_type": "code",
            "metadata": {},
            "outputs": [],
            "source": [
                "# Your analysis here\n",
            ],
        },
    ])

    nb = {
        "cells": cells,
        "metadata": {
            "kernelspec": {
                "display_name": "Python 3 (Pyodide)",
                "language": "python",
                "name": "python",
            },
            "language_info": {"name": "python", "version": "3.12.0"},
        },
        "nbformat": 4,
        "nbformat_minor": 5,
    }
    return _json.dumps(nb, indent=1, ensure_ascii=False)

def _save_report(df_raw, wall_ms: float, session_name: str, layer_rows: list[dict] | None = None,
                 mode: str | None = None) -> tuple[str, str, str, str]:
    """Write .md, .html, .json, .ipynb; return (terminal_str, html_path, ipynb_path, ts)."""
    import time, platform
    out = report(df_raw, wall_ms, layer_rows, mode)
    df_out = data_fn(df_raw)
    ts = hex(int(time.time() * 1000))[2:]
    session_dir = os.path.join(_telemetry_root(), _sanitize(session_name))
    os.makedirs(session_dir, exist_ok=True)

    # .md — terminal report
    md_path = os.path.join(session_dir, f"report_{ts}.md")
    with open(md_path, "w") as f:
        f.write(out)

    # .html — df_out 表格 (深色主题; 可换 html_render_fn 定制)
    html = report_html(df_raw, wall_ms, session_name)
    html_path = os.path.join(session_dir, f"report_{ts}.html")
    with open(html_path, "w") as f:
        f.write(html)

    # .json — aggregated data
    json_path = os.path.join(session_dir, f"data_{ts}.json")
    with open(json_path, "w") as f:
        json.dump(df_out.to_dicts(), f)

    # .ipynb — raw span data, fixed analysis + user space (+ layer 分析 cell)
    ipynb_path = os.path.join(session_dir, f"report_{ts}.ipynb")
    with open(ipynb_path, "w") as f:
        f.write(to_ipynb(df_raw, session_name, wall_ms, layer_rows))

    # Notebook Kit static data
    nb_data = os.path.join(_notebook_root(), "data", "session.json")
    try:
        os.makedirs(os.path.dirname(nb_data), exist_ok=True)
        layers_agg = layer_fn(layer_rows).to_dicts() if layer_rows else []
        with open(nb_data, "w") as f:
            json.dump({
                "rows": df_out.to_dicts(),
                "layers": layers_agg,           # 跨栈 layer 聚合 (layer_fn 槽)
                "session": session_name,
                "wall_ms": wall_ms,
                "total_ms": df_out["total_ms"].sum(),
                "phases": len(df_out),
                "arch": platform.machine(),
                "cpu": os.cpu_count() or 1,
            }, f)
    except OSError:
        pass
    _aggregate_sessions()
    _prune_reports(session_dir)
    print(f"  saved: {md_path}")
    print(f"         {html_path}")
    print(f"         {json_path}")
    print(f"         {ipynb_path}")
    return out, html_path, ipynb_path, ts

def _prune_reports(session_dir: str, keep: int = 5):
    """同一 session 目录保留最近 keep 份报告 (各类型按 mtime 截断).

    防累积混淆 (旧产物被误打开) + history.json 无限增长。
    """
    import glob as _glob
    for pattern in ("report_*.md", "report_*.html", "report_*.json", "report_*.ipynb", "data_*.json"):
        files = sorted(_glob.glob(os.path.join(session_dir, pattern)), key=os.path.getmtime)
        for f in files[:-keep]:
            try:
                os.remove(f)
            except OSError:
                pass

def _aggregate_sessions():
    """Build time-series history.json across all sessions for animated treemap."""
    import glob as _glob, json as _json
    root = _telemetry_root()
    pattern = os.path.join(root, "*", "data_*.json")
    files = sorted(_glob.glob(pattern), key=os.path.getmtime)
    if len(files) < 1:
        return
    phase_map = {}
    sessions = []
    for fp in files:
        try:
            with open(fp) as f:
                rows = _json.load(f)
        except Exception:
            continue
        sdir = os.path.basename(os.path.dirname(fp))
        fname = os.path.basename(fp)
        tstag = fname.replace("data_", "").replace(".json", "")
        total = sum(r["total_ms"] for r in rows if isinstance(r, dict) and "total_ms" in r)
        sessions.append({"session": sdir, "ts": tstag, "total_ms": round(total, 2), "phases": len(rows)})
        for r in rows:
            # 自定义 data_fn 的产物可能无 name/total_ms 列 — 跳过不污染聚合
            if not isinstance(r, dict) or "name" not in r or "total_ms" not in r:
                continue
            phase_map.setdefault(r["name"], []).append(round(r["total_ms"], 2))
    n = len(sessions)
    series = [{"name": name, "category": name.split("/")[0], "values": vals + [0] * (n - len(vals))}
              for name, vals in phase_map.items() if any(v > 0 for v in vals)]
    hist_path = os.path.join(_notebook_root(), "data", "history.json")
    try:
        os.makedirs(os.path.dirname(hist_path), exist_ok=True)
        with open(hist_path, "w") as f:
            _json.dump({"sessions": sessions, "series": series}, f)
    except OSError:
        pass

# Compile check

def _check_telemetry_compiled():
    try:
        from . import mdio_core
        s = mdio_core.ProjectStore("/mem")
        has = "telemetry_session_create" in dir(s)
        del s
        return has
    except Exception:
        return False

TELEMETRY_COMPILED = _check_telemetry_compiled()
TELEMETRY_HINT = (
    "MDIO Telemetry is not compiled. "
    "Add 'telemetry' to [tool.maturin] features in pyproject.toml:\n"
    '  features = ["python", "telemetry"]\n'
    "Then rebuild: just build-core"
) if not TELEMETRY_COMPILED else ""

# Module-level: trace() decorator  (对标 Rust telemetry_span! 宏)

def trace(name: str, **meta):
    """Module-level lazy decorator.  Activate by pushing a TelemetrySession
    onto the thread-local stack (``with sess:``).

    ``**meta`` are stored as span metadata — available in Polars as columns.
    """
    def decorator(func):
        if inspect.iscoroutinefunction(func):
            @functools.wraps(func)
            async def wrapper(*a, **kw):
                sess = _active()
                if sess is None:
                    return await func(*a, **kw)
                with sess._span(name, **meta):
                    return await func(*a, **kw)
            return wrapper
        else:
            @functools.wraps(func)
            def wrapper(*a, **kw):
                sess = _active()
                if sess is None:
                    return func(*a, **kw)
                with sess._span(name, **meta):
                    return func(*a, **kw)
            return wrapper
    return decorator


# Module-level: span() context manager

class span:
    """Module-level lazy context manager.

    ``with span("name", key=val):`` — records if a session is active,
    noop otherwise.
    """
    def __init__(self, name: str, **meta):
        self._name = name
        self._meta = meta
        self._span = None

    def __enter__(self):
        sess = _active()
        if sess is not None:
            self._span = sess._span(self._name, **self._meta)
            self._span.__enter__()
        return self

    def __exit__(self, *args):
        if self._span is not None:
            self._span.__exit__(*args)


# TelemetrySession — explicit lifecycle

class TelemetrySession:
    """A named telemetry session that collects span events.

    Push onto thread-local stack to activate module-level ``@trace`` /
    ``with span()`` probes::

        sess = TelemetrySession(store, "bench", mode="bench")
        with sess:
            ...  # all @trace / span() calls recorded
        sess.report()
    """

    def __init__(self, store, name: str, mode: str = "trace",
                 max_events: int | None = None,
                 max_meta_pool: int | None = None,
                 bench_samples_per_phase: int | None = None,
                 max_layer_samples: int | None = None):
        if not TELEMETRY_COMPILED:
            raise RuntimeError(TELEMETRY_HINT)
        self._store = store
        self._name = name
        self._mode = mode
        self._max_events = max_events
        self._max_meta_pool = max_meta_pool
        self._bench_samples_per_phase = bench_samples_per_phase
        self._max_layer_samples = max_layer_samples
        self._active = False
        self._cached_phase: str | None = None    # JSON snapshot on __exit__
        self._cached_layer: str | None = None
        try:
            # 覆盖已有 session (如 gateway auto) 前提示 — 数据将丢失
            if self._store.telemetry_session_active():
                print(f"TelemetrySession: 覆盖现有 session (旧数据丢弃) → {name}")
            self._store.telemetry_session_create(
                name,
                mode,
                max_events=self._max_events,
                max_meta_pool=self._max_meta_pool,
                bench_samples_per_phase=self._bench_samples_per_phase,
                max_layer_samples=self._max_layer_samples,
            )
            self._active = True
        except Exception as e:
            print(f"TelemetrySession: failed to create session ({e})")

    def __enter__(self):
        """Activate this session — push onto thread-local stack."""
        _stack().append(self)
        return self

    def __exit__(self, *args):
        """Deactivate — pop from thread-local stack, snapshot Rust data."""
        s = _stack()
        if s and s[-1] is self:
            s.pop()
        # Snapshot data now, before the store's Rust session gets overwritten
        try:
            self._cached_phase = self._store.telemetry_export_phase_json()
        except Exception:
            pass
        try:
            self._cached_layer = self._store.telemetry_get_layer_stats()
        except Exception:
            pass

    def _span(self, name: str, **meta):
        return _TelemetrySpan(self._store, name, **meta)

    # 实例级 API：TelemetrySession 上的 trace()/span()；模块级 API 见下方 @trace 与 span()

    def span(self, name: str, **meta):
        """Context manager (instance method — always active)."""
        return _TelemetrySpan(self._store, name, **meta)

    def trace(self, name: str):
        """Decorator (instance method — always active, requires pre-created session)."""
        store = self._store
        def decorator(func):
            if inspect.iscoroutinefunction(func):
                @functools.wraps(func)
                async def wrapper(*a, **kw):
                    with _TelemetrySpan(store, name):
                        return await func(*a, **kw)
                return wrapper
            else:
                @functools.wraps(func)
                def wrapper(*a, **kw):
                    with _TelemetrySpan(store, name):
                        return func(*a, **kw)
                return wrapper
        return decorator

    def measure(self, name: str, func, *args, **kwargs):
        with _TelemetrySpan(self._store, name):
            return func(*args, **kwargs)

    def report(self, open_html: bool = False, open_notebook: bool = False,
               open_kit: bool = False, group_by: str | None = None):
        if not self._active:
            print("TelemetrySession: no active session.")
            return
        if group_by:
            self._report_grouped(group_by)
            return
        df = self.to_polars()
        if df is None:
            print("TelemetrySession: no data.")
            return
        layer_rows = self.layer_stats()
        out, html_path, ipynb_path, ts = _save_report(df, 0, self._name, layer_rows or None, self._mode)
        if self._mode == "trace":
            try:
                trace = self._store.telemetry_report_trace_json()
                if '"traceEvents":[]' not in trace:
                    # 与四产物同 ts — 便于按时间戳配对
                    session_dir = os.path.join(_telemetry_root(), _sanitize(self._name))
                    os.makedirs(session_dir, exist_ok=True)
                    trace_path = os.path.join(session_dir, f"report_{ts}.json")
                    with open(trace_path, "w") as f:
                        f.write(trace)
            except Exception as e:
                print(f"TelemetrySession: trace JSON 写入失败 ({e})")
        print(out)
        open_ipynb(ipynb_path, launch=open_notebook)
        open_notebook_kit(launch=open_kit)

        if open_html:
            webbrowser.open("file://" + os.path.abspath(html_path))

    def _report_grouped(self, group_by: str):
        try:
            import polars as pl
        except ImportError:
            print("TelemetrySession: polars required"); return
        df = self.to_polars()
        if df is None: print("TelemetrySession: no data"); return
        if group_by not in df.columns:
            # layer_name/bucket 等 layer 列 — 回退到 layer_stats 聚合
            rows = self.layer_stats()
            if rows and any(group_by in r for r in rows):
                df = pl.DataFrame(rows)
            else:
                print(f"TelemetrySession: no column '{group_by}' in phase data (valid: {df.columns})")
                return
        report = df.group_by(group_by).agg([
            pl.col("dur_ns").count().alias("count"),
            (pl.col("dur_ns") / 1000).mean().round(1).alias("avg_us"),
            (pl.col("dur_ns") / 1000).median().round(1).alias("p50_us"),
            (pl.col("dur_ns") / 1000).quantile(0.99).round(1).alias("p99_us"),
            (pl.col("dur_ns").sum() / 1_000_000).round(2).alias("total_ms"),
        ]).sort("avg_us", descending=True)
        print(report)

    def layer_stats(self) -> list[dict]:
        if not self._active: return []
        try:
            raw = self._cached_layer or self._store.telemetry_get_layer_stats()
            return json.loads(raw)
        except Exception: return []

    def to_polars(self):
        """Return span event data as a Polars DataFrame.

        Uses cached snapshot from __exit__ if available;
        falls back to live store query (only works if the Rust session
        hasn't been overwritten by a subsequent session on the same store).
        """
        if not self._active: return None
        try:
            raw = self._cached_phase or self._store.telemetry_export_phase_json()
            rows = json.loads(raw)
            if not rows: return None
            import polars as pl
            return pl.DataFrame(rows)
        except Exception: return None

    def dump_json(self, path: str | None = None) -> str:
        """Export span event data as JSON. Uses cached snapshot from __exit__."""
        text = self._cached_phase or "[]"
        if text == "[]":
            try:
                text = self._store.telemetry_export_phase_json()
            except Exception:
                pass
        if path:
            with open(path, "w") as f: f.write(text)
        return text

    def close(self):
        """Close the session: release the store's Rust session + deactivate.

        A subsequent TelemetrySession re-creates a fresh session.
        """
        if not self._active:
            return
        self._active = False
        self._cached_phase = None
        self._cached_layer = None
        try:
            self._store.telemetry_session_destroy()
        except Exception:
            pass


# Runtime telemetry scope — context manager + decorator

class _TelemetryScope:
    """Returned by ``runtime.telemetry("name")``.

    Supports two usage patterns::

        # Decorator (recommended)
        @runtime.telemetry("my_bench")
        async def test(): ...

        # Context manager
        with runtime.telemetry("my_bench"):
            ...
    """
    def __init__(self, store, name: str,
                 max_events: int | None = None,
                 max_meta_pool: int | None = None,
                 bench_samples_per_phase: int | None = None,
                 max_layer_samples: int | None = None):
        self._store = store
        self._name = name
        self._max_events = max_events
        self._max_meta_pool = max_meta_pool
        self._bench_samples_per_phase = bench_samples_per_phase
        self._max_layer_samples = max_layer_samples
        self._t0 = None

    def __enter__(self):
        import time
        self._t0 = time.perf_counter()
        self._store.telemetry_session_create(
            self._name, "bench",
            max_events=self._max_events,
            max_meta_pool=self._max_meta_pool,
            bench_samples_per_phase=self._bench_samples_per_phase,
            max_layer_samples=self._max_layer_samples,
        )
        # 内部创建 TelemetrySession 推入栈，提供 _span() 给 trace()
        self._sess = TelemetrySession.__new__(TelemetrySession)
        self._sess._store = self._store
        self._sess._name = self._name
        self._sess._mode = "bench"
        self._sess._active = True
        _stack().append(self._sess)
        return self

    def __exit__(self, *args):
        import time
        wall_ms = (time.perf_counter() - self._t0) * 1000 if self._t0 else 0
        _stack().pop()
        out = report_session(self._store, self._name, wall_ms)
        print(out)

    def __call__(self, fn):
        import functools, inspect
        @functools.wraps(fn)
        def sync_wrapper(*a, **kw):
            with self:
                return fn(*a, **kw)
        @functools.wraps(fn)
        async def async_wrapper(*a, **kw):
            with self:
                return await fn(*a, **kw)
        return async_wrapper if inspect.iscoroutinefunction(fn) else sync_wrapper


# Internal span (FFI bridge)

class _TelemetrySpan:
    def __init__(self, store, name: str, **meta):
        self._store = store
        self._name = name
        self._meta = meta

    def __enter__(self):
        try:
            self._store.telemetry_span_enter(self._name)
        except Exception:
            pass
        return self

    def __exit__(self, *args):
        try:
            self._store.telemetry_span_exit(self._name, self._meta or None)
        except Exception:
            pass


def report_session(store, name: str = "gateway_report", wall_ms: float = 0) -> str:
    """从 store 的当前 session 生成完整 Python 报告 (polars 主线出口)。

    四产物 (.md/.html/.json/.ipynb) + Pipeline Latency 段 + 消费性 clear。
    gateway /telemetry/report 端点与脚本的统一入口。
    """
    raw = store.telemetry_export_phase_json()
    rows = json.loads(raw)
    if not rows:
        return "no telemetry data"
    import polars as pl
    df_raw = pl.DataFrame(rows)
    layer_rows = json.loads(store.telemetry_get_layer_stats())
    try:
        mode = store.telemetry_session_mode() or None
    except Exception:
        mode = None
    out, html_path, ipynb_path, _ts = _save_report(df_raw, wall_ms, name, layer_rows or None, mode)
    store.telemetry_report()  # 消费性 clear (对齐 Rust report_all 语义)
    return out

def _sanitize(s: str) -> str:
    return "".join(c if c.isalnum() or c in "-_" else "_" for c in s)

def telemetry_help(topic: str = ""):
    """Print telemetry usage guide. Topic: span / trace / session / report / bench / empty."""
    t = topic.lower()
    if t in ("span", "spans"):
        print("""  with span("name", key=val):  — 惰性上下文管理器
    仅在 TelemetrySession 激活时记录 (thread-local 栈), 否则零开销
    模块级: from mdio_py.telemetry import span""")
    elif t in ("trace", "decorator"):
        print("""  @trace("1_Compiler/1.Lark")  — 惰性装饰器
    def f(...): ...
    模块级: from mdio_py.telemetry import trace
    支持 sync/async; 无激活 session 时直接执行原函数""")
    elif t in ("session", "telemetrysession"):
        print("""  sess = TelemetrySession(store, "name", mode="bench")
  with sess:  ...        # 激活 (thread-local push)
  sess.report()          # 写 .md/.html/.json/.ipynb + notebook kit
  mode: "trace"(Chrome trace JSON) / "bench"(p50/p99 统计)""")
    elif t in ("report", "html", "notebook"):
        print("""  report(df_raw, wall_ms)  — 三段式终端报告 (data_fn/render_fn 槽)
  report_html(df_raw)     — html_render_fn 槽 (可换 notebook kit)
  _save_report / sess.report() 写 .telemetry/<session>/ 并更新 history.json""")
    elif t in ("bench", "benchmark"):
        print("""  @runtime.telemetry("bench_name")   — decorator 或上下文管理器
  async def test(): ...
  进入时创建 bench session, 退出自动报告 (Polars)""")
    else:
        print("""MDIO Telemetry — 三层全栈探针 (Rust 宏 / Python @trace+span / JS+WASM)
  TelemetrySession(store, name, mode)  → 显式生命周期
  runtime.telemetry(name)              → 自动报告作用域
  @trace / with span()                 → 惰性探针 (无 session 零开销)
  mode: trace → Chrome trace JSON (speedscope) | bench → Polars 统计
  topic: span / trace / session / report / bench — 查看专项用法""")

def _notebook_root() -> str:
    """Path to telemetry/notebook/ within the installed package."""
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), "notebook")

def _telemetry_root() -> str:
    """Find project/worktree root by walking up to .git, then .telemetry under it."""
    d = os.path.abspath(os.getcwd())
    while True:
        if os.path.isdir(os.path.join(d, ".git")):
            break
        parent = os.path.dirname(d)
        if parent == d:
            break
        d = parent
    return os.path.join(d, ".telemetry")
