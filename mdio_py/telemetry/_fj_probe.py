"""FJ 延迟探针:定位 wall-clock 摆动(550µs–106ms)发生在哪个环节。

三变量对照(同一进程,同文档):
  A. 遥测开(bench 会话)vs 遥测关 —— 隔离 span 录制成本/锁竞争;
  B. 写后渲染 vs 纯重复渲染(同快照 60 连发)—— 隔离写侧干扰与队列形态;
  C. 钩子注册 vs 无钩子 —— 隔离 Detect worker/分发线程干扰。
每轮驱动侧 Instant 记 wall;bench 会话内 fj_render_total 的 acc 分布
(min/p50/max)给出 span 内口径——wall max 尖峰而 acc 不尖 = 阻塞在
span 外(队列等待/回调前);两者同尖 = span 内。
用法: cd mdio_py && UV_CACHE_DIR=$PWD/../.uv-cache uv run --no-sync python telemetry/_fj_probe.py
"""
import json
import threading
import time
from pathlib import Path

from mdio_py import mdio_core as mrc
from mdio_py.telemetry import TelemetrySession
from mdio_py.core.selector_builder import SelectorBuilder

ROOT = Path(__file__).resolve().parents[2]
CM_FULL = (ROOT / "commonmark.md").read_text(encoding="utf-8")
IR_ROOT = SelectorBuilder().build()
WARM = 3
ROUNDS = 60


def fj_wait(store, doc_id, timeout_s=10.0):
    w, ok, _, _ = fj_wait_stamped(store, doc_id, timeout_s)
    return ok


def fj_wait_stamped(store, doc_id, timeout_s=10.0):
    """返回 (wall_us, cb_ts, wake_ts):切开 FJ 侧延迟 vs 主线程唤醒延迟。"""
    box = {"frame": None, "cb_ts": None}
    done = threading.Event()
    t0 = time.perf_counter()

    def _on_frame(frame):
        box["frame"] = bytes(frame) if frame else None
        box["cb_ts"] = time.perf_counter()
        done.set()

    ok = store.get_ast_packet_async(doc_id, _on_frame)
    assert ok
    done.wait(timeout_s)
    wake_ts = time.perf_counter()
    return ((wake_ts - t0) * 1e6, box["frame"] is not None,
            box["cb_ts"], wake_ts)


def round_wall(store, doc_id):
    t0 = time.perf_counter()
    ok = fj_wait(store, doc_id)
    return (time.perf_counter() - t0) * 1e6, ok


def stats(xs):
    xs = sorted(xs)
    return (xs[0], xs[len(xs) // 2], xs[-1], len(xs))


def run_render_only(store, doc_id, label, telemetry):
    sess = TelemetrySession(store, f"fjprobe_{label}", mode="bench") if telemetry else None
    walls = []
    if sess is not None:
        with sess:
            for k in range(ROUNDS):
                w, ok, cb_ts, wake_ts = fj_wait_stamped(store, doc_id)
                assert ok, f"render-only {label} k={k} stale"
                if w > 1000:
                    t0_est = wake_ts - w * 1e-6
                    print(f"  [{label} k={k}] SPIKE wall={w:.0f}µs "
                          f"cb_lag={(cb_ts - t0_est) * 1e6:.0f}µs "
                          f"wake_lag={(wake_ts - cb_ts) * 1e6:.0f}µs")
                walls.append(w)
            acc = json.loads(store.telemetry_export_phase_json())
    else:
        for k in range(ROUNDS):
            w, ok, _, _ = fj_wait_stamped(store, doc_id)
            assert ok, f"render-only {label} k={k} stale"
            walls.append(w)
        acc = []
    fj_rows = [r for r in acc if r.get("name") == "fj_render_total"] if acc else []
    fj_durs = [r["dur_ns"] / 1000 for r in fj_rows]
    print(f"[render-only {label}] wall(µs) min/p50/max/n = "
          f"{stats(walls)} ; fj span acc min/p50/max/n = "
          f"{(stats(fj_durs) if fj_durs else 'n/a')}")
    if walls:
        print("  wall 序列(前 8):", [round(w, 0) for w in walls[:8]])
        print("  wall 序列(尾 4):", [round(w, 0) for w in walls[-4:]])
    if fj_durs:
        top = sorted(fj_durs, reverse=True)[:4]
        print("  fj dur 最大 4 个(µs):", [round(d, 1) for d in top])
        # 四段切分:50ms 落在渲染内/过期检查/attach/回调
        for nm in ("fj_render_inner", "fj_stale", "fj_attach", "fj_frame",
                   "toast_build", "toast_render", "render_attrs", "blk_node",
                   "rn_leaf", "rn_shell", "walk_inline", "push_esc", "inl_plain",
                   "s4r_total", "attr_read", "walk_attrs"):
            sub = sorted((r["dur_ns"] / 1000 for r in acc if r.get("name") == nm), reverse=True)
            if sub:
                print(f"    {nm}: max={sub[0]:.1f} p50={sub[len(sub)//2]:.1f} n={len(sub)}")


def main() -> None:
    import os as _os
    if _os.environ.get("FJ_ORDER") == "prewarm_span":
        # 代码页假说验证:首会话内、首渲染前,主线程先录一个短 span
        # (record_event 首次执行)。税若在此 span 付清 → 之后渲染零尖峰。
        store = mrc.ProjectStore("/mem")
        store.mk("genesis", "/test", parents=False, verbose=False)
        store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "+>",
                            "\n".join(CM_FULL.splitlines()[:100]) + "\n",
                            True, True, False, False)
        doc_s = store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "~>",
                                    json.dumps([{"old": "CommonMark Spec",
                                                 "new": "CommonMark Spec!"}]),
                                    True, True, False, False)[0]
        for _ in range(2):
            fj_wait(store, doc_s)
        sess = TelemetrySession(store, "fjprobe_prewarm", mode="bench")
        with sess:
            t0 = time.perf_counter()
            with sess.span("prewarm_span", k=0):
                pass
            pw = (time.perf_counter() - t0) * 1e6
            print(f"  [prewarm_span] wall={pw:.0f}µs")
            for k in range(60):
                w, ok, cb, wk = fj_wait_stamped(store, doc_s)
                assert ok
                if w > 1000:
                    print(f"  [prewarm k={k}] SPIKE wall={w:.0f}µs")
        print("[prewarm] 完成")
        return
    if _os.environ.get("FJ_ORDER") == "no_telemetry_first":
        # 最干净对照:全程无任何遥测会话,60 次渲染逐轮测 wall。
        # 尖峰若出现 = 与遥测无关(渲染面每进程一次性税);若零 = 与
        # 遥测会话绑定。
        store = mrc.ProjectStore("/mem")
        store.mk("genesis", "/test", parents=False, verbose=False)
        store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "+>",
                            "\n".join(CM_FULL.splitlines()[:100]) + "\n",
                            True, True, False, False)
        doc_s = store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "~>",
                                    json.dumps([{"old": "CommonMark Spec",
                                                 "new": "CommonMark Spec!"}]),
                                    True, True, False, False)[0]
        for k in range(60):
            w, ok, cb, wk = fj_wait_stamped(store, doc_s)
            assert ok
            if w > 1000:
                print(f"  [no_tel k={k}] SPIKE wall={w:.0f}µs")
        print("[no_tel] 完成")
        return
    if _os.environ.get("FJ_ORDER") == "sync_first":
        # 线程归属判别:进程内首个会话跑主线程同步渲染(to_ast_json)。
        # 若主线程首渲也尖峰 = 挂起不认线程(OS 层);若只有 worker 尖峰
        # = FJ/队列唤醒面专属。
        store = mrc.ProjectStore("/mem")
        store.mk("genesis", "/test", parents=False, verbose=False)
        store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "+>",
                            "\n".join(CM_FULL.splitlines()[:100]) + "\n",
                            True, True, False, False)
        doc_s = store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "~>",
                                    json.dumps([{"old": "CommonMark Spec",
                                                 "new": "CommonMark Spec!"}]),
                                    True, True, False, False)[0]
        for _ in range(2):
            fj_wait(store, doc_s)
        sess = TelemetrySession(store, "fjprobe_sync_first", mode="bench")
        with sess:
            for k in range(60):
                t0 = time.perf_counter()
                store.to_ast_json("genesis", "/test/fj_small.md")
                w = (time.perf_counter() - t0) * 1e6
                if w > 1000:
                    print(f"  [sync_first k={k}] SPIKE wall={w:.0f}µs")
        print("[sync_first] 完成")
        return
    if _os.environ.get("FJ_ORDER") == "startup_delay":
        # 启动期判别:首会话前空等 30s——尖峰若随延迟消失 = 进程启动期
        # 调度噪声;若仍在首任务 = 与会话机制绑定。
        store = mrc.ProjectStore("/mem")
        store.mk("genesis", "/test", parents=False, verbose=False)
        store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "+>",
                            "\n".join(CM_FULL.splitlines()[:100]) + "\n",
                            True, True, False, False)
        doc_s = store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "~>",
                                    json.dumps([{"old": "CommonMark Spec",
                                                 "new": "CommonMark Spec!"}]),
                                    True, True, False, False)[0]
        for _ in range(2):
            fj_wait(store, doc_s)
        print("[delay] 空等 30s 远离进程启动期 ...", flush=True)
        time.sleep(30.0)
        for si in range(2):
            sess = TelemetrySession(store, f"fjprobe_delay_{si}", mode="bench")
            with sess:
                for k in range(60):
                    w, ok, cb, wk = fj_wait_stamped(store, doc_s)
                    assert ok
                    if w > 1000:
                        print(f"  [delay sess={si} k={k}] SPIKE wall={w:.0f}µs")
            print(f"[delay] sess={si} 完成", flush=True)
            if si == 0:
                time.sleep(30.0)  # 会话间隔 30s
        return
        # 顺序交换实验:小文档会话在前——区分"文档规模"与"会话序位"。
        store = mrc.ProjectStore("/mem")
        store.mk("genesis", "/test", parents=False, verbose=False)
        store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "+>",
                            "\n".join(CM_FULL.splitlines()[:100]) + "\n",
                            True, True, False, False)
        doc_s = store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "~>",
                                    json.dumps([{"old": "CommonMark Spec",
                                                 "new": "CommonMark Spec!"}]),
                                    True, True, False, False)[0]
        for _ in range(2):
            fj_wait(store, doc_s)
        sess_s = TelemetrySession(store, "fjprobe_small_first", mode="bench")
        with sess_s:
            for k in range(60):
                w, ok, cb, wk = fj_wait_stamped(store, doc_s)
                assert ok
                if w > 1000:
                    print(f"  [small_first k={k}] SPIKE wall={w:.0f}µs")
        print("[small_first] 完成")
        return
    store = mrc.ProjectStore("/mem")
    store.mk("genesis", "/test", parents=False, verbose=False)
    store.write_locator("genesis", "/test/fj.md", IR_ROOT, "+>", CM_FULL,
                        True, True, False, False)
    doc_id = store.write_locator("genesis", "/test/fj.md", IR_ROOT, "~>",
                                 json.dumps([{"old": "CommonMark Spec",
                                              "new": "CommonMark Spec!"}]),
                                 True, True, False, False)[0]
    # 桥预热(不进任何统计)
    for _ in range(2):
        fj_wait(store, doc_id)
    print(f"doc_id={doc_id}")

    # ── B1: 纯重复渲染(同快照,不写)——遥测开 ──
    run_render_only(store, doc_id, "t_on", telemetry=True)
    # ── B1-E: trace 模式 60 次(ts 事件树——缝隙定位决定性实验) ──
    sess_t = TelemetrySession(store, "fjprobe_t_trace", mode="trace")
    with sess_t:
        for k in range(60):
            w, ok = round_wall(store, doc_id)
            assert ok
            if w > 1000:
                print(f"  [trace k={k}] SPIKE wall={w:.0f}µs")
        trace_raw = store.telemetry_report_trace_json()
    tev = json.loads(trace_raw).get("traceEvents", [])
    fj_roots = [e for e in tev if e["name"] == "fj_render_total"]
    print(f"[trace60] renders={len(fj_roots)} events={len(tev)} "
          f"wall_max={max((e['ts']+e['dur']) - e['ts'] for e in fj_roots):.1f}µs")
    # 首 2 次渲染的事件树:根与直接子件的 ts 对齐,看缝隙
    fj_sorted = sorted(fj_roots, key=lambda e: e["ts"])
    for i, root in enumerate(fj_sorted[:2]):
        print(f"  root[{i}] ts={root['ts']:.1f} dur={root['dur']:.1f}")
        kids = [e for e in tev if e.get("args", {}).get("parent") == "fj_render_total"
                and root["ts"] <= e["ts"] < root["ts"] + root["dur"]]
        for e in sorted(kids, key=lambda x: x["ts"]):
            print(f"      {e['name']}: ts={e['ts']:.1f} dur={e['dur']:.1f}")
    # ── B1-C: 第 2 次前插同步渲染(区分渲染层 vs FJ 任务面)——同 store 新文档 ──
    store.write_locator("genesis", "/test/fj2.md", IR_ROOT, "+>", CM_FULL,
                        True, True, False, False)
    doc2 = store.write_locator("genesis", "/test/fj2.md", IR_ROOT, "~>",
                               json.dumps([{"old": "CommonMark Spec",
                                            "new": "CommonMark Spec!"}]),
                               True, True, False, False)[0]
    for _ in range(2):
        fj_wait(store, doc2)
    sess_c = TelemetrySession(store, "fjprobe_t_sync", mode="bench")
    with sess_c:
        w1, _ = round_wall(store, doc2)
        store.to_ast_json("genesis", "/test/fj2.md")  # 同步渲染插在第 2 次 FJ 前
        for k in range(6):
            w, ok = round_wall(store, doc2)
            assert ok
            print(f"  [sync-insert k={k}] wall={w:.0f}µs")
        print(f"  [sync-insert 首渲] wall={w1:.0f}µs")
    # ── B1-F: 小文档(100 行)bench 会话——区分"会话早期"vs"大渲染×acc" ──
    store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "+>",
                        "\n".join(CM_FULL.splitlines()[:100]) + "\n",
                        True, True, False, False)
    doc_s = store.write_locator("genesis", "/test/fj_small.md", IR_ROOT, "~>",
                                json.dumps([{"old": "CommonMark Spec",
                                             "new": "CommonMark Spec!"}]),
                                True, True, False, False)[0]
    for _ in range(2):
        fj_wait(store, doc_s)
    sess_s = TelemetrySession(store, "fjprobe_t_small", mode="bench")
    spike_cpu = []
    with sess_s:
        for k in range(60):
            w, ok, cb_ts, wake_ts = fj_wait_stamped(store, doc_s)
            assert ok
            if w > 1000:
                t0_est = wake_ts - w * 1e-6
                print(f"  [small k={k}] SPIKE wall={w:.0f}µs "
                      f"cb_lag={(cb_ts - t0_est)*1e6:.0f}µs wake_lag={(wake_ts - cb_ts)*1e6:.0f}µs")
    print(f"  [small] spikes={len(spike_cpu)}")
    # ── B2: 纯重复渲染——遥测关 ──
    run_render_only(store, doc_id, "t_off", telemetry=False)

    # ── A: 写后渲染循环(60 轮,轮间小内容编辑)——遥测开 ──
    sess = TelemetrySession(store, "fjprobe_wr_on", mode="bench")
    walls = []
    with sess:
        for k in range(WARM):
            store.write_locator("genesis", "/test/fj.md", IR_ROOT, "~>",
                                json.dumps([{"old": f"para{k}placeholder",
                                             "new": f"para{k}placeholder"}]),
                                True, True, False, False)
            round_wall(store, doc_id)
        for k in range(ROUNDS):
            t0 = time.perf_counter()
            store.write_locator("genesis", "/test/fj.md", IR_ROOT, "~>",
                                json.dumps([{"old": "The point can be illustrated",
                                             "new": "The point can be illustrated"}]),
                                True, True, False, False)
            w, ok = round_wall(store, doc_id)
            assert ok, f"wr k={k} stale"
            walls.append((time.perf_counter() - t0) * 1e6)
        acc = json.loads(store.telemetry_export_phase_json())
    fj_rows = [r for r in acc if r.get("name") == "fj_render_total"]
    print(f"[wr_on] wall(µs) min/p50/max/n = {stats(walls)} ; "
          f"fj span acc min/p50/max/n = {stats([r['dur_ns'] / 1000 for r in fj_rows])}")
    # 逐轮序列(前 12 轮)——看摆动形态
    print("  per-round:", [round(w, 0) for w in walls[:12]])

    # ── C: 无钩子对照已含于上(本进程从未注册钩子)——补一组有钩子写后循环 ──
    events = []
    lock = threading.Lock()

    def _cb(cb_code, ctx):
        with lock:
            events.append(cb_code)

    store.register_hook_dispatcher(_cb)
    hook_id = store.register_hook("genesis", "/test/fj.md",
                                  SelectorBuilder().drill("Introduction").build(),
                                  "probe", {}, True)
    sess2 = TelemetrySession(store, "fjprobe_wr_hook", mode="bench")
    walls2 = []
    with sess2:
        for k in range(WARM):
            store.write_locator("genesis", "/test/fj.md", IR_ROOT, "~>",
                                json.dumps([{"old": f"hookwarm{k}placeholder",
                                             "new": f"hookwarm{k}placeholder"}]),
                                True, True, False, False)
            round_wall(store, doc_id)
        for k in range(ROUNDS):
            t0 = time.perf_counter()
            store.write_locator("genesis", "/test/fj.md", IR_ROOT, "~>",
                                json.dumps([{"old": "The point can be illustrated",
                                             "new": "The point can be illustrated"}]),
                                True, True, False, False)
            w, ok = round_wall(store, doc_id)
            assert ok, f"wr_hook k={k} stale"
            walls2.append((time.perf_counter() - t0) * 1e6)
        acc2 = json.loads(store.telemetry_export_phase_json())
    fj2 = [r for r in acc2 if r.get("name") == "fj_render_total"]
    print(f"[wr_hook] wall(µs) min/p50/max/n = {stats(walls2)} ; "
          f"fj span acc min/p50/max/n = {stats([r['dur_ns'] / 1000 for r in fj2])} "
          f"hook_events={len(events)}")


if __name__ == "__main__":
    main()
