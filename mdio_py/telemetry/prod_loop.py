"""prod_loop 生产环驱动(手动脚本——非 pytest 目标)。

场景 = 已确认的生产环设计(1 写 + 4 读面,光标游走 5 种编辑 kind):
  - 文档: commonmark.md 真实结构。trace 环用前 400 行分片(实测: 9810 行
    一次冷渲染 = 恰 4096 事件撞 cap——trace 模式无法承载全文档;分片保留
    全部 5 类真实结构);FJ 全量渲染面走 bench 模式(全文 9810 行,
    per-phase 10 万样本 cap,无树无 cap 问题)。
  - trace 会话 × 3,每会话 12 轮(k 连续 0..35):
      W(k)   write_locator ~> 补丁(kind ∈ para/list/quote/fence/heading 轮流,
             目标从驱动镜像按结构池轮转;光标处编辑 = 目标节点字节域内
             字面替换,与 UI 光标编辑同形)
      R1(k)  hook_op 等待 Detect worker 事件(注册 1 个真实钩子
             drill("What is Markdown?"),事件回调直证生产输出)
      R3(k)  read_op seq=3: Drill 光标所在节(写后新快照首读)
      R4(k)  read_op seq=4: 同节重读(中间无更新——订阅重读面)
  - gap trace 会话: 6 轮,环内加 FJ(get_ast_packet_async 等待回调)——测
    FJ/Path worker 与 op 窗口的间隙关系(分片规模,cold render ~170 事件)。
  - bench 会话(全文): 30 轮 W + fj_op(FJ 全量渲染总价,真实 9810 行规模)
    + 每轮行为对拍(packet == 同步 to_ast_json,去 timestamp/md)。
  - 行为判据(运行时硬断言):
    R1: 钩子事件数 == 期望(编辑行落在钩子节内 ⟺ 至少 1 事件;节外
        ⟺ 0;边界 2 行盲区跳过并计数)。
    R2: FJ packet 与同步全量 AST 逐键相等(timestamp/md 除外)。
    R3/R4: 同轮两次 Drill 结果逐字节一致,R4 应命中 fdc_cache_hit。
  - 纪律: cap 守卫(4096);会话内 warmup k<3 剔除;per-session 纪元
    (ts 各自从 0,跨会话不可合并)。

输出: /tmp/prod_loop_trace_<si>.json + /tmp/prod_loop_gap.json +
      /tmp/prod_loop_bench.json + stdout 矩阵。
用法: cd mdio_py && UV_CACHE_DIR=$PWD/../.uv-cache uv run --no-sync python telemetry/prod_loop.py
"""

import json
import sys
import threading
import time
from pathlib import Path

from mdio_py import mdio_core as mrc
from mdio_py.telemetry import TelemetrySession, trace_event_cap
from mdio_py.core.selector_builder import SelectorBuilder

ROOT = Path(__file__).resolve().parents[2]
CM_FULL = (ROOT / "commonmark.md").read_text(encoding="utf-8")
CM_SLICE = "\n".join(CM_FULL.splitlines()[:400]) + "\n"

N_SESSIONS = 4
N_ROUNDS_PER_SESSION = 9
WARM_K = 2  # 会话内前 2 轮过渡(分析剔除);稳态样本 = 4 × 7 = 28/面
# 轮数预算实证:单轮 298 事件(选择器候选匹配 sel_ncat/sel_match 为主喷口,
# 每查询 ~70×2),9 轮 ≈ 3200(含标题轮 Path 渲染尖峰) < 4096 cap。
KINDS = ["para", "list", "quote", "fence", "heading"]
HOOK_TARGET = "What is Markdown?"  # 稳定节(驱动永不改其标题;有内容面)
HOOK_CB = "prod_loop_hook_cb"
EVENT_TIMEOUT_S = 0.01  # 事件等待上限(检测 µs 级;10ms = 千倍裕量)


def section_scan(lines):
    """驱动镜像侧章节扫描: [(start_line, heading_text)]。

    仅驱动自身的文本镜像使用(目标定位),引擎侧身份一律走引擎查询。
    """
    secs = []
    cur = None
    for i, ln in enumerate(lines):
        s = ln.strip()
        if s.startswith("#") and s.lstrip("#").lstrip():
            name = s.lstrip("#").strip()
            if cur is not None:
                secs.append(cur)
            cur = (i, name)
    if cur is not None:
        secs.append(cur)
    return secs


class Mirror:
    """驱动文本镜像:与引擎文档同步的字节副本(驱动确定性编辑的记账面)。"""

    def __init__(self, text: str):
        self.text = text

    def lines(self):
        return self.text.splitlines()

    def sections(self):
        return section_scan(self.lines())

    def sec_range(self, heading_name):
        """章节行区间 [start, end) —— 镜像侧。"""
        secs = self.sections()
        for idx, (s, n) in enumerate(secs):
            if n == heading_name:
                e = secs[idx + 1][0] if idx + 1 < len(secs) else len(self.lines())
                return (s, e)
        return None

    def count_in(self, old, sec_range):
        s, e = sec_range
        start = sum(len(l) + 1 for l in self.lines()[:s])
        end = sum(len(l) + 1 for l in self.lines()[:e])
        return self.text.count(old, start, max(end, start))

    def apply_patch(self, old, new, sec_range):
        """镜像侧复现引擎补丁(节内首个字面替换)——确定性唯一。"""
        s, e = sec_range
        start = sum(len(l) + 1 for l in self.lines()[:s])
        end = sum(len(l) + 1 for l in self.lines()[:e])
        pos = self.text.find(old, start, max(end, start))
        assert pos >= 0, f"mirror patch miss: {old!r} in {sec_range}"
        self.text = self.text[:pos] + new + self.text[pos + len(old):]


def kind_targets(m: Mirror, kind: str, exclude_headings=()):
    """结构池:该 kind 全部可用编辑目标 (line_idx, old, new, sec_name)。

    硬约束: old 在其章节内唯一(引擎补丁 = 节内首个字面匹配——唯一性
    保证落点 = 目标行,驱动镜像与引擎同为首匹配、同步可证)。
    """
    lines = m.lines()
    secs = m.sections()
    sec_by_line = []
    for i in range(len(lines)):
        name = ""
        for idx, (s, n) in enumerate(secs):
            if s <= i and (idx + 1 == len(secs) or secs[idx + 1][0] > i):
                name = n
                break
        sec_by_line.append(name)
    out = []
    in_fence = False
    for i, ln in enumerate(lines):
        st = ln.strip()
        if st.startswith("```") or st.startswith("~~~"):
            in_fence = not in_fence
            continue
        cand = None
        if kind == "para" and not in_fence and st and len(st) >= 40 \
                and not st.startswith(("#", ">", "-", "*", "+", "|", "=")) \
                and sec_by_line[i]:
            pos = len(ln) // 2
            old = ln[max(0, pos - 8):pos + 4]
            if len(old) >= 10 and "\n" not in old:
                cand = (i, old, old + "x", sec_by_line[i])
        elif kind == "list" and (st.startswith("- ") or st.startswith("* ") or st.startswith("+ ")) \
                and sec_by_line[i] and len(ln) >= 12:
            cand = (i, ln, ln + " x", sec_by_line[i])
        elif kind == "quote" and st.startswith("> ") and sec_by_line[i] \
                and len(ln) >= 12:
            cand = (i, ln, ln + " x", sec_by_line[i])
        elif kind == "fence" and in_fence and st and len(st) >= 12 \
                and sec_by_line[i]:
            cand = (i, ln, ln + " x", sec_by_line[i])
        elif kind == "heading" and not in_fence and st.startswith("#") \
                and sec_by_line[i] and sec_by_line[i] not in exclude_headings \
                and len(ln) >= 5:
            cand = (i, ln, ln + "x", sec_by_line[i])
        if cand is not None:
            _, old, _, sec_name = cand
            if m.count_in(old, m.sec_range(sec_name)) == 1:
                out.append(cand)
    return out


class HookCounter:
    """钩子事件直证:Detect dispatch → Python 回调计数(生产输出面)。"""

    def __init__(self, store):
        self.store = store
        self.lock = threading.Lock()
        self.events = []  # (monotonic, cb_code)
        self.done = threading.Event()
        self.store.register_hook_dispatcher(self._on_trigger)

    def _on_trigger(self, cb_code: str, ctx_dict: dict):
        with self.lock:
            self.events.append((time.monotonic(), cb_code))
        self.done.set()

    def reset(self):
        with self.lock:
            self.events.clear()
        self.done.clear()

    def wait_events(self, timeout_s):
        """返回等待期内到达的事件数(仅计数本驱动注册的 cb)。"""
        self.done.wait(timeout_s)
        with self.lock:
            n = sum(1 for _, cb in self.events if cb == HOOK_CB)
        return n


def fj_wait(store, doc_id, timeout_s=5.0):
    """FJ 软读: get_ast_packet_async + 回调等待;返回 AST JSON bytes 或 None。

    回调帧 = 13 字节帧头(tag=DN_AST + len + peer + doc)+ b'F' + JSON
    (store.rs build_ast_frame)——驱动侧剥帧头与 'F' 前缀取 JSON。
    """
    box = {"frame": None}
    done = threading.Event()

    def _on_frame(frame):
        box["frame"] = bytes(frame) if frame else None
        done.set()

    ok = store.get_ast_packet_async(doc_id, _on_frame)
    assert ok, "get_ast_packet_async failed"
    done.wait(timeout_s)
    frame = box["frame"]
    if frame is None:
        return None
    payload = frame[13:]
    assert payload[:1] == b"F", f"unexpected AST frame payload head: {payload[:4]!r}"
    return payload[1:]


# trace 会话事件保存按原路径写 /tmp/prod_loop_trace_N.json。


def run_trace_loop(store, m, hook_counter, doc_path, sess_name, k_start, n_rounds,
                   with_fj=False, doc_id=None, kinds=KINDS, hook_active=True):
    """一轮 = W + R1 + (FJ?) + R3 + R4。返回 (sess_evs, 行为判据账本)。"""
    ir_root = SelectorBuilder().build()
    ledger = {"hook_checked": 0, "hook_blind": 0, "drill_checked": 0}
    sess = TelemetrySession(store, sess_name, mode="trace")
    rotate = 0
    with sess:
        for r in range(n_rounds):
            k = k_start + r
            kind = kinds[rotate % len(kinds)]
            rotate += 1
            targets = kind_targets(m, kind, exclude_headings=("Introduction", HOOK_TARGET))
            assert targets, f"kind {kind} pool empty"
            t = targets[rotate % len(targets)]
            line_idx, old, new, sec_name = t
            sec_range = m.sec_range(sec_name)
            assert m.count_in(old, sec_range) == 1, \
                f"k={k} kind={kind}: old not unique in section {sec_name}: {old!r}"
            ir = SelectorBuilder().drill(sec_name).build()
            # ── W ──
            hook_counter.reset()
            with sess.span("write_op", k=k, kind=kind, tgt=sec_name):
                doc_id = store.write_locator("genesis", doc_path, ir, "~>",
                                             json.dumps([{"old": old, "new": new}]),
                                             True, True, False, False)[0]
            m.apply_patch(old, new, sec_range)
            if kind == "heading":
                # 标题改名:镜像章节表随新名更新(后续 R3 用新名 Drill)
                pass  # sec_range 由 Mirror.sections() 每次重扫,自动拿新名
            # ── R1:等待 Detect 事件(钩子节内编辑 ⟺ 事件 ≥ 1) ──
            with sess.span("hook_op", k=k, kind=kind):
                n_ev = hook_counter.wait_events(EVENT_TIMEOUT_S)
            # 期望:编辑行落在钩子节内(留 2 行边界盲区——ripple 边界形态
            # 驱动侧不可推演,不硬断言)
            hook_range = m.sec_range(HOOK_TARGET)
            expect_fire = hook_range[0] + 1 < line_idx < hook_range[1] - 1
            if not hook_active:
                pass  # 该文档未注册钩子(如 gap 分片)——无事件断言面
            elif line_idx <= hook_range[0] + 1 or line_idx >= hook_range[1] - 1:
                ledger["hook_blind"] += 1
            elif expect_fire:
                assert n_ev >= 1, f"k={k} kind={kind} line={line_idx}: 钩子节内编辑应触发"
                ledger["hook_checked"] += 1
            else:
                assert n_ev == 0, f"k={k} kind={kind} line={line_idx}: 节外编辑不应触发 (n={n_ev})"
                ledger["hook_checked"] += 1
            # ── R2(可选:gap 会话) ──
            if with_fj:
                with sess.span("fj_op", k=k):
                    pkt = fj_wait(store, doc_id)
                assert pkt is not None, f"k={k}: fj packet stale/None"
            # ── R3 / R4:光标所在节 Drill(新快照首读 + 无更新重读) ──
            tag = sec_name if kind != "heading" else new.strip().lstrip("#").strip()
            dr = SelectorBuilder().drill(tag)
            with sess.span("read_op", k=k, seq=3, tgt=tag):
                r3 = store.read_locator("genesis", doc_path, dr.build())
            with sess.span("read_op", k=k, seq=4, tgt=tag):
                r4 = store.read_locator("genesis", doc_path, dr.build())
            assert r3 and r3 == r4, f"k={k}: R3/R4 不一致 ({len(r3)} vs {len(r4)})"
            ledger["drill_checked"] += 1
        trace = store.telemetry_report_trace_json()
    sess_evs = json.loads(trace).get("traceEvents", [])
    cap = trace_event_cap()
    if len(sess_evs) >= cap:
        from collections import Counter
        c = Counter(e.get("name") for e in sess_evs)
        top = ", ".join(f"{n}:{v}" for n, v in c.most_common(12))
        raise SystemExit(
            f"{sess_name}: trace cap {cap} reached ({len(sess_evs)} events) — "
            f"top spans: {top}")
    return sess_evs, ledger


def run_bench_loop(store, m, doc_path, n_rounds):
    """bench 会话(全文 9810 行): W + fj_op(FJ 全量渲染总价)+ R2 行为对拍。"""
    ir_root = SelectorBuilder().build()
    sess = TelemetrySession(store, "prod_loop_bench", mode="bench")
    ledger = {"ast_checked": 0}
    rotate = 0
    with sess:
        for k in range(n_rounds):
            kind = KINDS[rotate % len(KINDS)]
            rotate += 1
            targets = kind_targets(m, kind, exclude_headings=("Introduction", HOOK_TARGET))
            assert targets, f"bench kind {kind} pool empty"
            t = targets[rotate % len(targets)]
            line_idx, old, new, sec_name = t
            sec_range = m.sec_range(sec_name)
            assert m.count_in(old, sec_range) == 1, f"bench k={k}: old not unique"
            ir = SelectorBuilder().drill(sec_name).build()
            with sess.span("write_op", k=k, kind=kind, tgt=sec_name):
                doc_id = store.write_locator("genesis", doc_path, ir, "~>",
                                             json.dumps([{"old": old, "new": new}]),
                                             True, True, False, False)[0]
            m.apply_patch(old, new, sec_range)
            with sess.span("fj_op", k=k, kind=kind):
                pkt = fj_wait(store, doc_id)
            assert pkt is not None, f"bench k={k}: fj packet None"
            # R2 行为对拍: FJ 渲染 == 同步全量 AST(去 timestamp/md)
            sync_ast = json.loads(store.to_ast_json("genesis", doc_path))
            pkt_obj = json.loads(pkt.decode("utf-8"))
            for obj in (sync_ast, pkt_obj):
                obj.pop("timestamp", None)
                obj.pop("md", None)
            assert pkt_obj == sync_ast, f"bench k={k}: FJ packet != 同步全量 AST"
            ledger["ast_checked"] += 1
        rows_raw = store.telemetry_export_phase_json()
    rows = json.loads(rows_raw)
    print(f"[bench] {len(rows)} phase rows")
    return rows, ledger


def main() -> None:
    import os
    if os.environ.get("PROD_LOOP_READ_BENCH") == "1":
        # 缺口①:全文(9810 行)普通读面测量——trace cap 无法承载全文
        # 循环(单查询 ~4000 事件),改 bench 模式:span 级 p50 由 acc 行
        # 供给(无 cap 问题);warmup 面由会话外预热承担(桥冷启动/首快照
        # 不进会话),会话内 24 轮稳态 pooled p50。
        store = mrc.ProjectStore("/mem")
        store.mk("genesis", "/test", parents=False, verbose=False)
        ir_root = SelectorBuilder().build()
        full_path = "/test/cm_full.md"
        store.write_locator("genesis", full_path, ir_root, "+>", CM_FULL,
                            True, True, False, False)
        m = Mirror(CM_FULL)
        # 会话外预热:桥 + 首快照 + 一轮完整环(读/写/读)
        for _ in range(3):
            store.read_locator("genesis", full_path,
                               SelectorBuilder().drill("Introduction").build())
        warm_targets = kind_targets(m, "para")
        w_old, w_new = warm_targets[0][1], warm_targets[0][2]
        store.write_locator("genesis", full_path,
                            SelectorBuilder().drill(warm_targets[0][3]).build(),
                            "~>", json.dumps([{"old": w_old, "new": w_new}]),
                            True, True, False, False)
        m.apply_patch(w_old, w_new, m.sec_range(warm_targets[0][3]))
        store.read_locator("genesis", full_path,
                           SelectorBuilder().drill("Introduction").build())
        store.read_locator("genesis", full_path,
                           SelectorBuilder().drill("Introduction").build())
        sess = TelemetrySession(store, "prod_loop_read_bench", mode="bench")
        rotate = 0
        with sess:
            for k in range(24):
                kind = KINDS[rotate % len(KINDS)]
                rotate += 1
                targets = kind_targets(m, kind, exclude_headings=("Introduction",))
                assert targets, f"read-bench kind {kind} pool empty"
                t = targets[rotate % len(targets)]
                line_idx, old, new, sec_name = t
                sec_range = m.sec_range(sec_name)
                assert m.count_in(old, sec_range) == 1, f"k={k}: old not unique"
                ir = SelectorBuilder().drill(sec_name).build()
                with sess.span("write_op", k=k, kind=kind, tgt=sec_name):
                    store.write_locator("genesis", full_path, ir, "~>",
                                        json.dumps([{"old": old, "new": new}]),
                                        True, True, False, False)
                m.apply_patch(old, new, sec_range)
                tag = sec_name if kind != "heading" else new.strip().lstrip("#").strip()
                dr = SelectorBuilder().drill(tag)
                with sess.span("read_op", k=k, seq=3, tgt=tag):
                    r3 = store.read_locator("genesis", full_path, dr.build())
                with sess.span("read_op", k=k, seq=4, tgt=tag):
                    r4 = store.read_locator("genesis", full_path, dr.build())
                if not r3:
                    print(f"[READ-BENCH 异常] k={k} kind={kind} tag={tag!r}: R3 空(目标节不存在?)")
                    raise SystemExit(f"k={k}: R3 空")
                if r3 != r4:
                    for i, (a, b) in enumerate(zip(r3, r4)):
                        if a != b:
                            print(f"[READ-BENCH 异常] k={k} kind={kind} tag={tag!r} "
                                  f"first diff line {i}: r3={a[:80]!r} r4={b[:80]!r}")
                            break
                    raise SystemExit(f"k={k}: R3/R4 不一致(行为判官违例)")
            rows_raw = store.telemetry_export_phase_json()
        rows = json.loads(rows_raw)
        with open("/tmp/prod_loop_read_bench.json", "w") as f:
            f.write(json.dumps(rows))
        import polars as _pl
        rdf = _pl.DataFrame(rows, strict=False)
        print("=== 全文(9810 行)普通读面 span 级 p50(µs, acc 行 pooled) ===")
        faces = rdf.filter(_pl.col("name").is_in(
            ["read_op", "rl_total", "read_projection", "sel_total", "sel_fdc",
             "fdc_pass2", "fdc_prologue", "fdc_sec_intervals", "s4r_total",
             "fdc_p2_breakdown", "rl_depy", "vfs_resolve", "session_open",
             "sec_cell_intervals", "sec_cell_header_bounds", "fdc_settled_loop",
             "fdc_settled_map", "fdc_struct_clone", "sel_match", "sel_ncat_impl",
             "write_op", "wl_main_select"]))
        print(faces.group_by("name").agg(
            (_pl.col("dur_ns").median() / 1000).alias("p50_us"),
            (_pl.col("dur_ns").max() / 1000).alias("max_us"),
            _pl.col("dur_ns").count().alias("n"),
        ).sort("p50_us", descending=True))
        return

    if os.environ.get("PROD_LOOP_GAP_ONLY") == "1":
        # 裁决探查快速通道:只跑 gap 会话 + rr_verdict 普查(reuse_roots
        # 裁决路径 kv 探查——零行为,裁决后回撤探针)。
        store = mrc.ProjectStore("/mem")
        store.mk("genesis", "/test", parents=False, verbose=False)
        ir_root = SelectorBuilder().build()
        gap_slice = "\n".join(CM_FULL.splitlines()[:100]) + "\n"
        gap_path = "/test/cm_gap.md"
        store.write_locator("genesis", gap_path, ir_root, "+>", gap_slice,
                            True, True, False, False)
        m_gap = Mirror(gap_slice)
        hc = HookCounter(store)
        gap_evs, gap_ledger = run_trace_loop(
            store, m_gap, hc, gap_path, "prod_loop_gap", 100, 4, with_fj=True,
            kinds=["para", "quote", "fence"], hook_active=False,
        )
        import json as _json, polars as _pl
        rows = [{"name": e["name"], "ts": e["ts"], "args": e.get("args") or {}}
                for e in gap_evs]
        rdf = _pl.DataFrame(rows, strict=False)
        print("=== rr_verdict 裁决路径分布(gap 会话) ===")
        print(rdf.filter(_pl.col("name") == "rr_verdict").group_by(
            ["args"]).agg(_pl.col("ts").count().alias("n")).sort("n", descending=True))
        print("=== rr_verdict 逐轮时序 ===")
        print(rdf.filter(_pl.col("name") == "rr_verdict").select(["ts", "args"]))
        return

    store = mrc.ProjectStore("/mem")
    store.mk("genesis", "/test", parents=False, verbose=False)
    ir_root = SelectorBuilder().build()

    # ── 文档装载(驱动镜像 = 写入字节的原样副本) ──
    slice_path = "/test/cm_slice.md"
    full_path = "/test/cm_full.md"
    store.write_locator("genesis", slice_path, ir_root, "+>", CM_SLICE,
                        True, True, False, False)
    store.write_locator("genesis", full_path, ir_root, "+>", CM_FULL,
                        True, True, False, False)
    m_slice = Mirror(CM_SLICE)
    m_full = Mirror(CM_FULL)

    # ── 钩子: 稳定节 "What is Markdown?"(驱动永不改其标题)。 ──
    # is_always=True = 常驻监控形态(is_always=False 为一次性钩子——
    # hook.rs evaluate_and_dispatch: 触发即 self.remove——与"连续读穿插
    # 连续写"的每写检测面不符,生产常驻监控用 True)。
    hook_ir = SelectorBuilder().drill(HOOK_TARGET).build()
    hook_counter = HookCounter(store)
    hook_id = store.register_hook("genesis", slice_path, hook_ir, HOOK_CB, {}, True)
    assert hook_id != 0, "hook register failed"

    # ── 会话外预热(桥冷启动/分配器/首快照不进 trace) ──
    for _ in range(3):
        store.read_locator("genesis", slice_path, SelectorBuilder().drill("Introduction").build())
        store.to_ast_json("genesis", full_path)

    # ── trace 环 × 3(每会话 12 轮;文档状态跨会话承前) ──
    all_sessions = []
    ledgers = []
    for s in range(N_SESSIONS):
        sess_evs, ledger = run_trace_loop(
            store, m_slice, hook_counter, slice_path,
            f"prod_loop_t{s}", s * N_ROUNDS_PER_SESSION, N_ROUNDS_PER_SESSION,
        )
        path = f"/tmp/prod_loop_trace_{s}.json"
        with open(path, "w") as f:
            f.write(json.dumps({"traceEvents": sess_evs}))
        all_sessions.append(sess_evs)
        ledgers.append(ledger)
        print(f"[driver] trace session {s}: {len(sess_evs)} events; "
              f"hook_checked={ledger['hook_checked']} blind={ledger['hook_blind']} "
              f"drill_checked={ledger['drill_checked']}")

    # ── gap trace 会话(环内加 FJ——间隙关系面)。 ──
    # 100 行分片:FJ 冷渲染 ~500-600 事件/次(实测 400 行分片 ~2000/次,
    # 4 轮 × 900 预算 < cap;窗口/间隙机制与文档规模无关,结构轮 FJ
    # 全量成本面由 bench 全文会话承接)。
    gap_slice = "\n".join(CM_FULL.splitlines()[:100]) + "\n"
    gap_path = "/test/cm_gap.md"
    store.write_locator("genesis", gap_path, ir_root, "+>", gap_slice,
                        True, True, False, False)
    m_gap = Mirror(gap_slice)
    gap_evs, gap_ledger = run_trace_loop(
        store, m_gap, hook_counter, gap_path,
        "prod_loop_gap", 100, 4, with_fj=True,
        kinds=["para", "quote", "fence"], hook_active=False,
    )
    with open("/tmp/prod_loop_gap.json", "w") as f:
        f.write(json.dumps({"traceEvents": gap_evs}))
    print(f"[driver] gap session: {len(gap_evs)} events; hook_checked={gap_ledger['hook_checked']} "
          f"blind={gap_ledger['hook_blind']} drill_checked={gap_ledger['drill_checked']}")

    # ── bench 会话(全文 9810 行,FJ 全量渲染总价 + AST 对拍) ──
    bench_rows, bench_ledger = run_bench_loop(store, m_full, full_path, 30)
    with open("/tmp/prod_loop_bench.json", "w") as f:
        f.write(json.dumps(bench_rows))
    print(f"[driver] bench: ast_checked={bench_ledger['ast_checked']}")

    analyze(all_sessions, gap_evs, bench_rows, ledgers + [gap_ledger])


# 分析侧:因果树/归因逻辑与 fdc_decomp.session_records 同源(
# 两级消歧 + 父子记账 + 异步根时间窗归因);per-session 纪元,跨会话不合并。

OPS = ("write_op", "hook_op", "read_op", "fj_op")


def session_records(evs, sess_idx, warm_k, k_start=0):
    rows = []
    for e in evs:
        rows.append({
            "name": e["name"],
            "ts": e["ts"],
            "end": e["ts"] + e["dur"],
            "dur": e["dur"],
            "args": e.get("args") or {},
        })
    by_name = {}
    for i, r in enumerate(rows):
        by_name.setdefault(r["name"], []).append(i)
    EPS = 0.5
    parent_of = {}
    ambiguous = 0
    total_linked = 0
    orphan_missing = 0
    loose = 0
    boundary_noise = 0
    for i, r in enumerate(rows):
        pn = r["args"].get("parent")
        best = None
        if pn:
            total_linked += 1
            strict = [j for j in by_name.get(pn, [])
                      if rows[j]["ts"] <= r["ts"] and rows[j]["end"] >= r["end"]]
            fuzzy = [] if strict else [
                j for j in by_name.get(pn, [])
                if rows[j]["ts"] <= r["ts"] + EPS and rows[j]["end"] >= r["end"] - EPS
            ]
            if len(strict) == 1:
                best = strict[0]
            elif len(strict) > 1:
                if r["dur"] >= 1.0:
                    ambiguous += 1
                else:
                    boundary_noise += 1
                best = min(strict, key=lambda j: -rows[j]["ts"])
            elif len(fuzzy) == 1:
                best = fuzzy[0]
            elif len(fuzzy) > 1:
                boundary_noise += 1
                best = min(fuzzy, key=lambda j: -rows[j]["ts"])
            elif not by_name.get(pn):
                orphan_missing += 1
            else:
                loose += 1
        parent_of[i] = best
    if total_linked:
        print(f"[tree] session {sess_idx} 歧义率: {ambiguous}/{total_linked} "
              f"= {ambiguous / total_linked:.2%} (boundary={boundary_noise} "
              f"missing={orphan_missing} loose={loose})")
    children_of = {}
    for i, p in parent_of.items():
        children_of.setdefault(p, []).append(i)

    def subtree(i):
        out = [i]
        for c in children_of.get(i, []):
            out.extend(subtree(c))
        return out

    ops = [i for i, r in enumerate(rows) if r["name"] in OPS]
    recs = []
    mz_rows = []
    hit_rows = []
    for oi in ops:
        r = rows[oi]
        args = r["args"]
        k = int(args.get("k", -1))
        if k >= 0 and (k - k_start) < warm_k:
            continue
        kind = args.get("kind", "")
        seq = args.get("seq", "")
        tgt = args.get("tgt", "")
        for ci in subtree(oi):
            c = rows[ci]
            if c["name"] in ("mz_new_zone", "mz_mid", "old_full_of", "old_of_inner",
                             "reaper_take", "oidx_slot", "oidx_fallback",
                             "write_op", "hook_op", "read_op", "fj_op"):
                mz_rows.append({
                    "op": r["name"], "seq": seq, "tgt": tgt, "k": k, "sess": sess_idx,
                    "span": c["name"],
                    "zone": c["args"].get("zone", ""),
                    "old_up": c["args"].get("old_up", ""),
                    "src": c["args"].get("src", ""),
                    "took": c["args"].get("took", ""),
                    "moved": c["args"].get("moved", ""),
                    "bag": c["args"].get("bag", ""),
                    "slot_id": c["args"].get("slot_id", ""),
                    "depth": c["args"].get("depth", ""),
                    "state": c["args"].get("state", ""),
                    "ts": c["ts"],
                    "dur_us": c["dur"],
                })
                continue
            if c["name"] in ("fdc_cache_hit", "fdc_hit_rebuild"):
                hit_rows.append({
                    "op": r["name"], "seq": seq, "tgt": tgt, "k": k, "sess": sess_idx,
                    "span": c["name"], "dur_us": c["dur"],
                })
                continue
            recs.append({
                "op": r["name"], "seq": seq, "tgt": tgt, "k": k,
                "kind": kind, "sess": sess_idx,
                "span": c["name"], "dur_us": c["dur"],
            })
    # 异步 worker 归因(时间窗): 根 span 落在哪个 op 窗口 = 与谁并发。
    # hd 链(钩子检测)与 FJ/Path 同属 worker 线程根——hook_op 窗口等待
    # 事件即归因窗(线程局部 span 栈下 worker 根 parent=None,时间窗
    # 包含是唯一归因通道)。
    async_roots = [i for i, r in enumerate(rows)
                   if r["name"] in ("fj_render_total", "path_render_total",
                                    "hook_detect_work")]
    async_recs = []
    TAIL = 100.0  # µs:窗口缝隙(写返回后镜像记账/span 退出竞态)归后继 op 的尾容差
    for ar in async_roots:
        # 启动点归因(非全包含):worker 根常在写面启动、钩子等待面内收尾
        # (全包含要求 root.end <= op.end——主线程被事件唤醒的 µs 级竞态把
        # 收尾挤出窗口,全包含会漏归因到 gap)。启动点 ∈ 窗口 = 该面发起的
        # 异步工作;启动点落在窗口缝隙(≤TAIL 内随后继 op 启动)= 归后继
        # op;队列深度恒 1(每轮等待收口),无跨轮积压串扰。
        owner = None
        for oi in ops:
            o = rows[oi]
            k = int(o["args"].get("k", -1))
            if k >= 0 and (k - k_start) < warm_k:
                continue
            if o["ts"] <= rows[ar]["ts"] < o["end"]:
                owner = oi
                break
        if owner is None:
            for oi in ops:
                o = rows[oi]
                k = int(o["args"].get("k", -1))
                if k >= 0 and (k - k_start) < warm_k:
                    continue
                if rows[ar]["ts"] < o["ts"] <= rows[ar]["ts"] + TAIL:
                    owner = oi
                    break
        owner_k = int(rows[owner]["args"].get("k", -1)) if owner is not None else -1
        for ai in subtree(ar):
            if rows[ai]["name"] in ("reaper_take", "old_of_inner"):
                a = rows[ai]["args"]
                mz_rows.append({
                    "op": "async", "seq": "", "tgt": "", "k": owner_k, "sess": sess_idx,
                    "span": rows[ai]["name"],
                    "zone": "", "old_up": "", "src": "",
                    "took": a.get("took", ""),
                    "moved": a.get("moved", ""),
                    "bag": a.get("bag", ""),
                    "slot_id": a.get("slot_id", ""),
                    "depth": a.get("depth", ""),
                    "state": a.get("state", ""),
                    "ts": rows[ai]["ts"],
                    "dur_us": rows[ai]["dur"],
                })
            async_recs.append({
                "op": rows[owner]["name"] if owner is not None else "gap",
                "seq": rows[owner]["args"].get("seq", "") if owner is not None else "",
                "tgt": rows[owner]["args"].get("tgt", "") if owner is not None else "",
                "span": rows[ai]["name"], "dur_us": rows[ai]["dur"],
            })
    return recs, mz_rows, hit_rows, async_recs


def analyze(all_sessions, gap_evs, bench_rows, ledgers) -> None:
    import polars as pl

    pl.Config.set_tbl_rows(200)
    pl.Config.set_fmt_str_lengths(120)

    def med(dfx):
        return dfx.group_by("span").agg(
            (pl.col("dur_us").min()).alias("min_us"),
            (pl.col("dur_us").median()).alias("p50_us"),
            (pl.col("dur_us").max()).alias("max_us"),
            pl.col("dur_us").count().alias("n"),
        ).sort("p50_us", descending=True)

    # ── trace 三会话聚合(逐会话建树 → 聚合) ──
    recs_all, mz_all, hit_all, async_all = [], [], [], []
    for si, evs in enumerate(all_sessions):
        recs, mz_rows, hit_rows, async_recs = session_records(
            evs, si, WARM_K, k_start=si * N_ROUNDS_PER_SESSION)
        recs_all.extend(recs)
        mz_all.extend(mz_rows)
        hit_all.extend(hit_rows)
        async_all.extend(async_recs)
    df = pl.DataFrame(recs_all, strict=False)
    mz_df = pl.DataFrame(mz_all, strict=False) if mz_all else None

    print("\n════════ §6 矩阵 Q1: 一轮生产环总价与构成(trace 环, p50 µs) ════════")
    faces = mz_df.filter(pl.col("span").is_in(OPS)).group_by(["op", "seq"]).agg(
        (pl.col("dur_us").min()).alias("min_us"),
        (pl.col("dur_us").median()).alias("p50_us"),
        (pl.col("dur_us").max()).alias("max_us"),
        pl.col("dur_us").count().alias("n"),
    ).sort("p50_us", descending=True)
    print(faces)
    w_p50 = mz_df.filter((pl.col("op") == "write_op") & (pl.col("span") == "write_op"))["dur_us"].median()
    r3_p50 = mz_df.filter((pl.col("op") == "read_op") & (pl.col("seq") == "3") & (pl.col("span") == "read_op"))["dur_us"].median()
    r4_p50 = mz_df.filter((pl.col("op") == "read_op") & (pl.col("seq") == "4") & (pl.col("span") == "read_op"))["dur_us"].median()
    if r3_p50 is not None and r4_p50 is not None:
        print(f"写 p50={w_p50}µs;读#1 p50={r3_p50}µs;读#2 p50={r4_p50}µs;"
              f"读#2/读#1 = {r4_p50 / r3_p50:.2f}")

    print("\n════════ §6 矩阵 Q2: R1 hook 检测读面成本(hd_* 引擎面, p50 µs) ════════")
    hd = df.filter(pl.col("op") == "hook_op")
    print(med(hd))
    # hd_* 在 Detect worker 线程(异步根 hook_detect_work 的子树)——启动点归因:
    # 检测常在写面启动(write_locator 未返回前 worker 已开跑)或钩子等待面内
    # 启动——两者同轮,都是 R1 引擎面。
    if async_all:
        adf_all = pl.DataFrame(async_all, strict=False)
        print("--- write/hook 窗口内异步子树全件(worker 工作总面) ---")
        print(med(adf_all.filter(pl.col("op").is_in(["hook_op", "write_op"]))))
        hd_async = adf_all.filter(
            pl.col("op").is_in(["hook_op", "write_op"])
            & pl.col("span").str.starts_with("hd")
            | (pl.col("op").is_in(["hook_op", "write_op"]) & pl.col("span").str.starts_with("hook_detect")))
        print("--- hd 链(Detect worker 本体) ---")
        print(med(hd_async))
        hk2 = adf_all.filter((pl.col("op").is_in(["hook_op", "write_op"])) & (pl.col("span") == "hook_detect_work"))["dur_us"].median()
        print(f"hook_detect_work(检测引擎总面) p50 = {hk2}µs")
        hd_by_owner = adf_all.filter(pl.col("span").str.starts_with("hd") | pl.col("span").str.starts_with("hook_detect")).group_by("op").agg(
            pl.col("dur_us").count().alias("n"))
        print("--- hd span 按启动面分布 ---")
        print(hd_by_owner)
    hk = mz_df.filter((pl.col("op") == "hook_op") & (pl.col("span") == "hook_op"))["dur_us"].median()
    print(f"hook_op 等待窗口 p50 = {hk}µs(驱动事件等待含裕量,引擎面成本以 hd_* 计)")

    print("\n════════ §6 矩阵 Q3: 第一次读贵在哪(fdc 链逐件,按物化三态分组) ════════")
    r3 = df.filter((pl.col("op") == "read_op") & (pl.col("seq") == "3"))
    print("--- 读#1 全量(不分态) ---")
    print(med(r3))
    if mz_df is not None:
        print("--- 写侧物化 span 普查(write_op 子树内的 mz 件——发布即定型面) ---")
        print(mz_df.filter(pl.col("op") == "write_op").group_by("span").agg(
            pl.col("dur_us").count().alias("n"),
            (pl.col("dur_us").median()).alias("p50_us"),
        ).sort("n", descending=True))
        print("--- 异步物化 span 普查(Path/FJ worker 子树内的 mz 件——物化面) ---")
        print(mz_df.filter(pl.col("op") == "async").group_by("span").agg(
            pl.col("dur_us").count().alias("n"),
            (pl.col("dur_us").median()).alias("p50_us"),
        ).sort("n", descending=True))
        print("--- 读#1 物化相关 span 普查(seq=3 子树内出现的 mz 件) ---")
        print(mz_df.filter(pl.col("seq") == "3").group_by("span").agg(
            pl.col("dur_us").count().alias("n"),
            (pl.col("dur_us").median()).alias("p50_us"),
        ).sort("n", descending=True))
        # 三态分组 = Path worker 首层物化状态(该节奏下旧链物化发生在
        # Path 异步物化面;写侧 compute_zone 走 diff 面不触旧链,读#1 拿
        # 已物化 full,s4r ~0.3µs 直通)
        st = mz_df.filter((pl.col("span") == "old_of_inner") & (pl.col("op") == "async") & (pl.col("depth") == "0"))
        print("--- Path 物化面首层状态分布 ---")
        print(st.group_by("state").agg(pl.col("dur_us").count().alias("n")))
        for state in ("asm", "hit", "none"):
            ks = st.filter(pl.col("state") == state)["k"].to_list()
            if not ks:
                continue
            sub = r3.filter(pl.col("k").is_in(ks))
            if sub.height:
                print(f"--- 读#1 分解(Path 物化 state={state}, n={sub.height}) ---")
                print(med(sub))

    print("\n════════ §6 矩阵 Q4: 第二次读(中间无更新)——R4 vs R3 ════════")
    r4 = df.filter((pl.col("op") == "read_op") & (pl.col("seq") == "4"))
    print("--- 读#2 分解 ---")
    print(med(r4))
    if hit_all:
        hdf = pl.DataFrame(hit_all, strict=False)
        print("--- fdc_cache_hit / fdc_hit_rebuild 命中面(读#2) ---")
        print(hdf.filter(pl.col("seq") == "4").group_by("span").agg(
            (pl.col("dur_us").median()).alias("p50_us"),
            pl.col("dur_us").count().alias("n")))

    print("\n════════ §6 矩阵 Q5: 异步 worker 间隙关系(gap 会话) ════════")
    gap_recs, gap_mz, _, gap_async = session_records(gap_evs, 99, 1, k_start=100)
    gdf = pl.DataFrame(gap_recs, strict=False)
    if gap_async:
        adf = pl.DataFrame(gap_async, strict=False)
        roots = adf.filter(pl.col("span").is_in(["fj_render_total", "path_render_total"]))
        print(roots.group_by(["op", "span"]).agg(
            (pl.col("dur_us").median()).alias("p50_us"),
            pl.col("dur_us").count().alias("n"),
        ).sort("p50_us", descending=True))
        print(f"[gap] 落在 op 窗口内 {roots.height} 个根;"
              f"gap(间隙)根 = {adf.filter((pl.col('op')=='gap') & (pl.col('span').is_in(['fj_render_total','path_render_total']))).height} 个")
    if gap_mz:
        gmz = pl.DataFrame(gap_mz, strict=False)
        print("--- gap 会话 FJ 软读(fj_op 等待 vs fj_render_total 引擎面) ---")
        print(gmz.filter(pl.col("span").is_in(["fj_op"])).group_by("span").agg(
            (pl.col("dur_us").min()).alias("min_us"),
            (pl.col("dur_us").median()).alias("p50_us"),
            (pl.col("dur_us").max()).alias("max_us"),
            pl.col("dur_us").count().alias("n"),
        ))

    print("\n════════ bench 全文(9810 行): FJ 全量渲染总价 ════════")
    bdf = pl.DataFrame(bench_rows, strict=False)
    fj_bench = bdf.filter(pl.col("name").is_in(["fj_op", "fj_render_total"]))
    print(fj_bench.group_by("name").agg(
        (pl.col("dur_ns").min() / 1000).alias("min_us"),
        (pl.col("dur_ns").median() / 1000).alias("p50_us"),
        (pl.col("dur_ns").max() / 1000).alias("max_us"),
        pl.col("dur_ns").count().alias("n"),
    ))
    if "doc_id" in bdf.columns:
        print("--- fj_render_total 按 doc_id 分布(查证渲染来源) ---")
        print(bdf.filter(pl.col("name") == "fj_render_total").group_by("doc_id").agg(
            pl.col("dur_ns").count().alias("n")))
    w_bench = bdf.filter(pl.col("name") == "write_op").group_by("name").agg(
        (pl.col("dur_ns").median() / 1000).alias("p50_us"),
        pl.col("dur_ns").count().alias("n"))
    print(w_bench)
    # warmup 剔除(带 k kv 的环面): k>=3 稳态口径
    if "k" in bdf.columns:
        warm = bdf.filter(pl.col("name").is_in(["fj_op", "write_op"]) & (pl.col("k") >= 3))
        print("--- 稳态(k>=3)口径 ---")
        print(warm.group_by("name").agg(
            (pl.col("dur_ns").median() / 1000).alias("p50_us"),
            pl.col("dur_ns").count().alias("n")))

    print("\n════════ 行为判据账本(运行时硬断言) ════════")
    hc = sum(l["hook_checked"] for l in ledgers)
    hb = sum(l["hook_blind"] for l in ledgers)
    dc = sum(l["drill_checked"] for l in ledgers)
    print(f"R1 钩子事件断言: {hc} 轮通过(盲区跳过 {hb});"
          f"R3/R4 逐字节一致断言: {dc} 轮通过")


if __name__ == "__main__":
    sys.exit(main())
