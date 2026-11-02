"""FJ 成本拆解 + 跨路径重叠模块对照(纯测试——零生产代码改动)。

方法:bench 会话按入口分组(每入口独立会话——bench 聚合无 kv 归组,
会话级隔离是唯一干净的分组面),同名 span 跨会话对比:
  S_fj   同快照 FJ 异步重复渲染(复用快路径面)
  S_sync 同快照同步 to_ast_json(同渲染层另一入口)
  S_read 同快照 Drill 读(读路径——树格/段格消费面对照)
  S_write 每轮写后 FJ(Edit 面——树格每版本重建面)
  S_share 格子共享验证:写一次(新快照)→ sync → fj → sync → fj,
          toast_build 出现次数 = 树格"首建一次、跨入口共享"的直接证据
重叠模块对照口径:toast_build(树格构建)/ s4r_total(段格入口)/
blk_node+rn_leaf+inl_total(渲染件)/ toast_render(无槽冷路径标志)。
两档规模:400 行分片(默认)与 9810 行全文(FJ_DECOMP_FULL=1)。
用法: cd mdio_py && UV_CACHE_DIR=$PWD/../.uv-cache uv run --no-sync python telemetry/_fj_decomp.py
"""
import json
import os
import threading
import time
from pathlib import Path

from mdio_py import mdio_core as mrc
from mdio_py.telemetry import TelemetrySession
from mdio_py.core.selector_builder import SelectorBuilder

ROOT = Path(__file__).resolve().parents[2]
CM_FULL = (ROOT / "commonmark.md").read_text(encoding="utf-8")
FULL = os.environ.get("FJ_DECOMP_FULL") == "1"
DOC = CM_FULL if FULL else "\n".join(CM_FULL.splitlines()[:400]) + "\n"
TAG = "full9810" if FULL else "slice400"
IR_ROOT = SelectorBuilder().build()
N = 30
WARM = 5


def fj(store, doc_id):
    box = {"frame": None}
    done = threading.Event()

    def _cb(frame):
        box["frame"] = bytes(frame) if frame else None
        done.set()

    assert store.get_ast_packet_async(doc_id, _cb)
    done.wait(10.0)
    return box["frame"] is not None


def stats(xs):
    xs = sorted(xs)
    return xs[0], xs[len(xs) // 2], xs[-1], len(xs)


def run_session(store, doc_path, doc_id, name, fn, n=N):
    sess = TelemetrySession(store, f"fjdc_{TAG}_{name}", mode="bench")
    walls = []
    with sess:
        for _ in range(WARM):
            fn(store, doc_id, doc_path)
        for _ in range(n):
            t0 = time.perf_counter()
            fn(store, doc_id, doc_path)
            walls.append((time.perf_counter() - t0) * 1e6)
        raw = json.loads(store.telemetry_export_phase_json())
    rows = [r for r in raw if r["src"] == "acc"]  # acc 口径(时间维度,无 kv 双写)
    return walls, rows


def span_table(rows, names):
    out = {}
    for nm in names:
        durs = sorted(r["dur_ns"] / 1000 for r in rows if r["name"] == nm)
        if durs:
            out[nm] = (durs[0], durs[len(durs) // 2], durs[-1], len(durs))
    return out


SPANS = ["toast_build", "s4r_total", "toast_render", "blk_node", "rn_leaf",
         "rn_shell", "inl_total", "walk_inline", "render_attrs", "walk_attrs",
         "guard_validate", "fdc_sec_intervals", "fdc_pass2"]


def main() -> None:
    store = mrc.ProjectStore("/mem")
    store.mk("genesis", "/test", parents=False, verbose=False)
    p = "/test/fjdc.md"
    store.write_locator("genesis", p, IR_ROOT, "+>", DOC, True, True, False, False)
    doc_id = store.write_locator("genesis", p, IR_ROOT, "~>",
                                 json.dumps([{"old": "CommonMark Spec",
                                              "new": "CommonMark Spec!"}]),
                                 True, True, False, False)[0]
    for _ in range(2):
        fj(store, doc_id)  # 桥预热(不进统计)

    # ── S_fj: 同快照 FJ 重复渲染 ──
    w1, r1 = run_session(store, p, doc_id, "fj", lambda s, d, pth: fj(s, d))
    # ── S_sync: 同快照同步渲染 ──
    w2, r2 = run_session(store, p, doc_id, "sync",
                         lambda s, d, pth: s.to_ast_json("genesis", pth))
    # ── S_read: 同快照 Drill 读 ──
    dr = SelectorBuilder().drill("Introduction")
    w3, r3 = run_session(store, p, doc_id, "read",
                         lambda s, d, pth: s.read_locator("genesis", pth, dr.build()))
    # ── S_write: 每轮写后 FJ ──
    def write_then_fj(s, d, pth):
        s.write_locator("genesis", pth, IR_ROOT, "~>",
                        json.dumps([{"old": "The point can be illustrated",
                                     "new": "The point can be illustrated"}]),
                        True, True, False, False)
        fj(s, d)
    w4, r4 = run_session(store, p, doc_id, "write", write_then_fj)

    # ── S_share: 树格跨入口共享(写一次 → sync/fj 交替) ──
    store.write_locator("genesis", p, IR_ROOT, "~>",
                        json.dumps([{"old": "CommonMark Spec!",
                                     "new": "CommonMark Spec"}]),
                        True, True, False, False)
    sess = TelemetrySession(store, f"fjdc_{TAG}_share", mode="bench")
    share_walls = []
    with sess:
        for _ in range(4):
            t0 = time.perf_counter()
            store.to_ast_json("genesis", p)
            share_walls.append(("sync", (time.perf_counter() - t0) * 1e6))
            t0 = time.perf_counter()
            fj(store, doc_id)
            share_walls.append(("fj", (time.perf_counter() - t0) * 1e6))
        raw = json.loads(store.telemetry_export_phase_json())
    tb = [r for r in raw if r["name"] == "toast_build"]
    s4r = [r for r in raw if r["name"] == "s4r_total"]
    print(f"[{TAG} share] toast_build n={len(tb)} s4r n={len(s4r)} "
          f"(8 次渲染中树格/段格实际构建次数——首建一次则共享成立)")

    print(f"\n===== {TAG} 入口 wall(µs, min/p50/max/n) =====")
    for nm, w in (("FJ异步重渲", w1), ("同步to_ast_json", w2),
                  ("Drill读", w3), ("写后FJ", w4)):
        print(f"  {nm}: {stats(w)}")

    print(f"\n===== {TAG} 重叠模块 span(µs, p50/n) × 入口会话 =====")
    hdr = f"{'span':<20}" + "".join(f"{nm[:12]:>14}" for nm in
                                    ("FJ重渲", "同步", "Drill读", "写后FJ"))
    print(hdr)
    for sp in SPANS:
        cells = []
        for rows in (r1, r2, r3, r4):
            t = span_table(rows, [sp]).get(sp)
            cells.append(f"{t[1]:>8.1f}/{t[3]:>4}" if t else f"{'-':>13}")
        if any(c != f"{'-':>13}" for c in cells):
            print(f"{sp:<20}" + "".join(cells))


if __name__ == "__main__":
    main()
