"""fdc 首读成本分解探查(手动脚本——非 pytest 目标)。

场景 = 典型连续读穿插写:
  - 文档: 3 章节(S1/S2/S3),每轮在文尾追加一行(S1/S2 区间不变)。
  - 每轮: 读#1 = Drill S1(写后新快照——段构建 + sec 格构建 + pass2 全付),
          读#2 = Drill S1(同快照同区间——缓存命中),
          读#3 = Drill S3(同快照新区间——sec 格命中,pass2 重付),
          写   = 文尾追加一行。
  - 会话外预热 6 轮(pyo3 桥冷启动 ~50ms 不进场),会话内 40 轮,
    分析侧 k>=5 再取中位(暖态口径)。

输出: /tmp/fdc_trace.json(trace 事件原始 JSON)+ stdout 分解表。
用法: cd mdio_py && UV_CACHE_DIR=$PWD/../.uv-cache uv run --no-sync python telemetry/fdc_decomp.py
"""

import json
import sys

from mdio_py import mdio_core as mrc
from mdio_py.telemetry import TelemetrySession, trace_event_cap
from mdio_py.core.selector_builder import SelectorBuilder

INIT = (
    "# S1\n\n"
    "a: 1\n"
    "b: 2\n"
    "c: 3\n"
    "- item 1\n"
    "- item 2\n"
    "\n# S2\n\n"
    "e: 5\n"
    "f: 6\n"
    "\n# S3\n\n"
    "g: 7\n"
    "h: 8\n"
)

def build_init(scale: int) -> str:
    """规模刻度:scale 个章节,每章 2 内容行 + 标题(软换行段落折叠
    保持块结构同型;S1/S3 目标恒存在)。"""
    if scale <= 3:
        return INIT
    parts = ["# S1\n\na: 1\nb: 2\n"]
    for i in range(2, scale + 1):
        parts.append(f"\n# S{i}\n\ne{i}: 5\nf{i}: 6\n")
    return "".join(parts)

N_SESSIONS = 3
N_ROUNDS_PER_SESSION = 6
WARM_K = 2  # 每会话内前 2 轮作暖态过渡(分析时剔除);总稳态样本 = 3×4 = 12
SCALE = int(__import__("os").environ.get("FDC_SCALE", "3"))  # 章节数(规模刻度)


def main() -> None:
    store = mrc.ProjectStore("/mem")
    store.mk("genesis", "/test", parents=False, verbose=False)
    ir_root = SelectorBuilder().build()
    s1 = SelectorBuilder().drill("S1")
    s3 = SelectorBuilder().drill("S3")

    store.write_locator("genesis", "/test/doc.md", ir_root, "+>", build_init(SCALE),
                        True, True, False, False)

    def read(sel):
        return store.read_locator("genesis", "/test/doc.md", sel.build())

    def write(k: int):
        return store.write_locator("genesis", "/test/doc.md", ir_root, "+>",
                                   f"z{k}: {k}\n", True, True, False, False)

    # ── 会话外预热:桥冷启动/分配器/首快照,不进 trace ──
    for k in range(6):
        read(s1)
        read(s1)
        read(s3)
        write(k)

    # ── 分会话采集:每会话事件数必须远低于 4096 cap(trace 满则尾部
    # 静默丢弃——异步 worker 的根 span 是最后写入的,首当其冲)。会话
    # 各自 epoch 从 0 起——ts 不可跨会话合并,逐会话建树后聚合。 ──
    sessions_evs = []
    for s in range(N_SESSIONS):
        sess = TelemetrySession(store, f"fdc_decomp_{s}", mode="trace")
        with sess:
            for r in range(N_ROUNDS_PER_SESSION):
                k = s * N_ROUNDS_PER_SESSION + r
                with sess.span("read_op", k=k, seq=1, tgt="S1"):
                    read(s1)
                with sess.span("read_op", k=k, seq=2, tgt="S1"):
                    read(s1)
                with sess.span("read_op", k=k, seq=3, tgt="S3"):
                    read(s3)
                with sess.span("write_op", k=k):
                    write(k)
            trace = store.telemetry_report_trace_json()
        sess_evs = json.loads(trace).get("traceEvents", [])
        sessions_evs.append(sess_evs)
        cap = trace_event_cap()
        assert len(sess_evs) < cap, \
            f"session {s}: trace cap {cap} reached — 尾部事件被静默丢弃,数据不可信"
        print(f"[driver] session {s}: {len(sess_evs)} events")

    with open("/tmp/fdc_trace.json", "w") as f:
        # 逐会话保存(list-of-lists)——ts 各自 epoch,跨会话不可合并;
        # 供外部逐会话 dump(事件池本身按 drop 序,ts 非单调)。
        f.write(json.dumps({"sessions": sessions_evs}))

    analyze(sessions_evs)


# cap 断言保留在上方主流程。


def mechanism_assert(mz) -> None:
    """机制四支路逐轮硬断言(因果树归因 + 读侧直证槽号):

    state(读#1 depth-0) == "asm" ⟺
        (a) 旧槽有 full 取物在读前,且取物后、首读前没有重建(asm 且
            ts+dur <= 读 ts);或
        (b) 旧槽在 trace 可见历史中从未被组装过(无物化史 → 无从取起,
            首读仍是缺席 → asm)。
    其余 → hit/absent。归因全部来自因果树(k/seq 由 op 子树携带),
    不用窗口启发式;跨会话物化史按 (sess, ts) 全序判定。

    违例即机制模型与数据不符 —— 直接失败,不许带着过期结论继续。
    """
    import polars as pl

    n_checked = 0
    unverified = 0
    # 物化史(跨会话全序):slot -> [(sess, end_ts)] state=asm 的 old_of_inner
    asm_hist = {}
    for row in mz.filter((pl.col("span") == "old_of_inner") & (pl.col("state") == "asm")).iter_rows(named=True):
        asm_hist.setdefault(str(row["slot_id"]), []).append((row["sess"], row["ts"] + row["dur_us"]))
    # 每轮首读的直证槽号(读侧 oidx_slot/oidx_fallback 的 slot_id kv)
    reads = mz.filter((pl.col("span") == "read_op") & (pl.col("seq") == "1"))
    oidx_rows = mz.filter(pl.col("span").is_in(["oidx_slot", "oidx_fallback"]))
    takes = mz.filter((pl.col("span") == "reaper_take") & (pl.col("bag").is_in(["full", "both"])))
    st0 = mz.filter((pl.col("span") == "old_of_inner") & (pl.col("depth") == "0") & (pl.col("seq") == "1"))
    for rr in reads.iter_rows(named=True):
        k = rr["k"]
        if k % N_ROUNDS_PER_SESSION < WARM_K:
            continue
        sess, ts = rr["sess"], rr["ts"]
        sub = oidx_rows.filter((pl.col("sess") == sess) & (pl.col("k") == k) & (pl.col("seq") == "1"))
        if sub.height == 0:
            continue  # 读侧未触旧索引(无旧侧依赖轮),不参与断言
        sid = str(sub["slot_id"][0])
        take_list = takes.filter(
            (pl.col("sess") == sess) & (pl.col("slot_id") == sid) & (pl.col("ts") < ts)
        )["ts"].to_list()
        taken_before = bool(take_list)
        rebuilt_done = False
        if taken_before:
            t0 = max(take_list)
            for rr2 in mz.filter(
                (pl.col("sess") == sess) & (pl.col("span") == "old_of_inner")
                & (pl.col("slot_id") == sid) & (pl.col("state") == "asm")
            ).iter_rows(named=True):
                if rr2["ts"] >= t0 and rr2["ts"] + rr2["dur_us"] <= ts:
                    rebuilt_done = True
                    break
        ever_assembled = any(
            p_sess < sess or (p_sess == sess and end <= ts)
            for (p_sess, end) in asm_hist.get(sid, [])
        )
        sub0 = st0.filter((pl.col("sess") == sess) & (pl.col("k") == k))
        state = None if sub0.height == 0 else sub0["state"][0]
        if not taken_before and not ever_assembled:
            # 可观察面盲区:asm(从未组装支路)与 hit(物化史在 warmup/
            # 更早会话的盲区)都合法——不硬断言,计入 unverified。
            unverified += 1
            continue
        # 硬断言两支:取物在前且无重建 ⟹ asm;其余 ⟹ hit/absent。
        expect_asm = taken_before and not rebuilt_done
        if expect_asm:
            assert state == "asm", (
                f"机制违例 sess={sess} k={k}: 取物在前且无重建,"
                f"期望 asm 实得 {state} (sid={sid} taken={taken_before} "
                f"rebuilt={rebuilt_done} ever={ever_assembled})")
            n_checked += 1
        else:
            assert state in (None, "hit"), (
                f"机制违例 sess={sess} k={k}: 无取物或已被重建,"
                f"期望 hit/absent 实得 {state} (sid={sid} taken={taken_before} "
                f"rebuilt={rebuilt_done} ever={ever_assembled})")
            n_checked += 1
    print(f"[mechanism] 断言通过: {n_checked} 轮,盲区跳过 {unverified} 轮"
          f"(取物在前且无重建→asm;其余→hit/absent;归因=因果树+读侧槽号)")


def analyze(sessions_evs) -> None:
    import polars as pl

    print(f"[driver] sessions {len(sessions_evs)}; "
          f"events per session {[len(s) for s in sessions_evs]}")

    def session_records(evs, sess_idx):
        """单会话事件 → (recs, kv_buckets, async_recs)——因果树 + op 归因。"""
        rows = []
        for e in evs:
            rows.append({
                "name": e["name"],
                "ts": e["ts"],
                "end": e["ts"] + e["dur"],
                "dur": e["dur"],
                "args": e.get("args") or {},
            })

        # ── 因果树:Rust 记录的父名(args["parent"])+ 最近包含同名消歧 ──
        # (框架的 span 栈已线程本地化——父链因果正确;同名 span 靠时间包含
        # 取最内层实例。区间嵌套无法区分并发线程,不再使用。)
        by_name = {}
        for i, r in enumerate(rows):
            by_name.setdefault(r["name"], []).append(i)
        # 时序包含容差:ts 由 Instant 回推、
        # dur 由 ticks 计时,两钟混用 → 嵌套边界 ±0.1µs 倒置。两级消歧:
        # 严格包含唯一 → 采纳;无严格但 ε 内唯一模糊 → 采纳(边界噪声);
        # 多候选 → 歧义;无候选 → 孤儿(真链接断)。
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
                        ambiguous += 1  # µs 级歧义(致命)
                    else:
                        boundary_noise += 1  # 亚 µs 零宽子落在边界(与 checker 同律)
                    best = min(strict, key=lambda j: -rows[j]["ts"])
                elif len(fuzzy) == 1:
                    best = fuzzy[0]
                elif len(fuzzy) > 1:
                    boundary_noise += 1  # 亚 µs 边界噪声(与 checker 同律,非违例)
                    best = min(fuzzy, key=lambda j: -rows[j]["ts"])
                elif not by_name.get(pn):
                    orphan_missing += 1  # 父名不存在 = 真链接断(致命)
                else:
                    loose += 1  # 回推 ts + 抢占膨胀伪影(报告不致命)
            parent_of[i] = best
        # 因果树歧义率审计:同父名多个包含候选 = 消歧选择面,错选风险面。
        # 每次会话建树时打印,率须近零(同名 span 嵌套极少)。
        if total_linked:
            print(f"[tree] session 歧义率: {ambiguous}/{total_linked} "
                  f"= {ambiguous / total_linked:.2%} (严格多候选;模糊边界噪声 {boundary_noise}; "
                  f"missing={orphan_missing} 真断; loose={loose} 伪影)")

        children_of = {}
        for i, p in parent_of.items():
            children_of.setdefault(p, []).append(i)

        def subtree(i):
            out = [i]
            for c in children_of.get(i, []):
                out.extend(subtree(c))
            return out

        # ── 每轮 op 的子树汇总 ──
        ops = [i for i, r in enumerate(rows) if r["name"] in ("read_op", "write_op")]
        recs = []
        kv_buckets = []
        mz_rows = []
        for oi in ops:
            r = rows[oi]
            args = r["args"]
            kind = r["name"]
            k = int(args.get("k", -1))
            if k % N_ROUNDS_PER_SESSION < WARM_K:
                continue
            seq = args.get("seq", "")
            tgt = args.get("tgt", "")
            for ci in subtree(oi):
                c = rows[ci]
                if c["name"] in ("mz_new_zone", "mz_mid", "old_full_of", "old_of_inner",
                                 "reaper_take", "oidx_slot", "oidx_fallback",
                                 "read_op", "write_op"):
                    mz_rows.append({
                        "op": kind, "seq": seq, "tgt": tgt, "k": k,
                        "sess": sess_idx,
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
                if c["name"] == "fdc_p2_breakdown":
                    a = c["args"]
                    kv_buckets.append({
                        "op": kind, "seq": seq, "tgt": tgt,
                        "li_ns": int(float(a.get("li_ns", 0))),
                        "idn_ns": int(float(a.get("idn_ns", 0))),
                        "bre_ns": int(float(a.get("bre_ns", 0))),
                        "bpn_ns": int(float(a.get("bpn_ns", 0))),
                        "mn_ns": int(float(a.get("mn_ns", 0))),
                        "bn_ns": int(float(a.get("bn_ns", 0))),
                        "iter": int(float(a.get("iter", 0))),
                        "skip": int(float(a.get("skip", 0))),
                        "emit": int(float(a.get("emit", 0))),
                        "l_seq": a.get("l_seq", ""),
                        "nl_end": int(float(a.get("nl_end", 0))),
                        "blocks_n": int(float(a.get("blocks_n", 0))),
                        "seg_lo": int(float(a.get("seg_lo", -1))),
                        "seg_hi": int(float(a.get("seg_hi", -1))),
                        "bs_first": int(float(a.get("bs_first", -1))),
                        "bs_last": int(float(a.get("bs_last", -1))),
                        "bre_calls": int(float(a.get("bre_calls", 0))),
                        "bs_seq": a.get("bs_seq", ""),
                        "q_start": int(float(a.get("q_start", -1))),
                        "q_end": int(float(a.get("q_end", -1))),
                    })
                    continue
                recs.append({
                    "op": kind, "seq": seq, "tgt": tgt, "k": k,
                    "span": c["name"], "dur_us": c["dur"],
                })

        # ── 异步 worker 归因(时间窗——与因果树正交):根 span 落在哪个 op 的
        # [ts,end) 内 = 写后渲染/补丁计算与谁并发;落在间隙则不计入任何 op。
        async_roots = [i for i, r in enumerate(rows)
                       if r["name"] in ("fj_render_total", "path_render_total")]
        async_recs = []
        for ar in async_roots:
            owner = None
            for oi in ops:
                o = rows[oi]
                k = int(o["args"].get("k", -1))
                if k % N_ROUNDS_PER_SESSION < WARM_K:
                    continue
                if o["ts"] <= rows[ar]["ts"] and o["end"] >= rows[ar]["end"]:
                    owner = oi
                    break
            for ai in subtree(ar):
                if rows[ai]["name"] in ("reaper_take", "old_of_inner"):
                    a = rows[ai]["args"]
                    mz_rows.append({
                        "op": "async", "seq": "", "tgt": "", "k": -1,
                        "sess": sess_idx,
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
        return recs, kv_buckets, async_recs, mz_rows

    recs_all = []
    kv_all = []
    async_all = []
    mz_all = []
    for si, evs in enumerate(sessions_evs):
        recs, kv_buckets, async_recs, mz_rows = session_records(evs, si)
        recs_all.extend(recs)
        kv_all.extend(kv_buckets)
        async_all.extend(async_recs)
        mz_all.extend(mz_rows)

    df = pl.DataFrame(recs_all, strict=False)
    if df.height == 0:
        print("[driver] no steady-state records")
        return

    import os
    pl.Config.set_tbl_rows(200)
    pl.Config.set_fmt_str_lengths(120)

    def med(dfx):
        return dfx.group_by("span").agg(
            (pl.col("dur_us").min()).alias("min_us"),
            (pl.col("dur_us").median()).alias("p50_us"),
            (pl.col("dur_us").max()).alias("max_us"),
            pl.col("dur_us").count().alias("n"),
        ).sort("p50_us", descending=True)

    print("\n=== 读#1(写后新快照, Drill S1) span 分解(p50 µs) ===")
    print(med(df.filter((pl.col("op") == "read_op") & (pl.col("seq") == "1") & (pl.col("tgt") == "S1"))))
    print("\n=== 读#2(同快照同区间, Drill S1) ===")
    print(med(df.filter((pl.col("op") == "read_op") & (pl.col("seq") == "2") & (pl.col("tgt") == "S1"))))
    print("\n=== 读#3(同快照新区间, Drill S3) ===")
    print(med(df.filter((pl.col("op") == "read_op") & (pl.col("seq") == "3") & (pl.col("tgt") == "S3"))))
    print("\n=== 写 ===")
    print(med(df.filter(pl.col("op") == "write_op")))

    if kv_all:
        kb = pl.DataFrame(kv_all, strict=False)
        print("\n=== fdc_p2_breakdown 桶(p50 ns, 读#1) ===")
        kb1 = kb.filter((pl.col("op") == "read_op") & (pl.col("seq") == "1"))
        print(kb1.select([pl.col(c).median().alias(c) for c in
                          ["li_ns", "idn_ns", "bre_ns", "bpn_ns", "mn_ns", "bn_ns"]]))
        print(kb1.select([
            pl.col("iter").median().alias("iter_p50"),
            pl.col("skip").median().alias("skip_p50"),
            pl.col("emit").median().alias("emit_p50"),
            pl.col("iter").count().alias("events"),
        ]))
        print("--- 读#1 每事件明细(全部 fdc_p2_breakdown 事件) ---")
        print(kb1.select(["iter", "skip", "emit", "li_ns", "idn_ns", "bre_ns", "bpn_ns", "mn_ns", "bn_ns"]))
        print("--- 读#1 pass2 行序列(l_seq / nl_end,机制查证) ---")
        print(kb1.select(["l_seq", "nl_end", "blocks_n", "seg_lo", "seg_hi", "bs_seq", "q_start", "q_end", "bre_calls"]))
        print("\n=== fdc_p2_breakdown 桶(p50 ns, 读#3) ===")
        kb3 = kb.filter((pl.col("op") == "read_op") & (pl.col("seq") == "3"))
        print(kb3.select([pl.col(c).median().alias(c) for c in
                          ["li_ns", "idn_ns", "bre_ns", "bpn_ns", "mn_ns", "bn_ns"]]))
        print(kb3.select([
            pl.col("iter").median().alias("iter_p50"),
            pl.col("skip").median().alias("skip_p50"),
            pl.col("emit").median().alias("emit_p50"),
            pl.col("iter").count().alias("events"),
        ]))
        print("--- 读#3 每事件明细 ---")
        print(kb3.select(["iter", "skip", "emit", "li_ns", "idn_ns", "bre_ns", "bpn_ns", "mn_ns", "bn_ns"]))

    # ── 顶层未归因:read_projection 减去直接子项 ──
    print("\n=== read_projection 直接子项归因(读#1) ===")
    r1 = df.filter((pl.col("op") == "read_op") & (pl.col("seq") == "1") & (pl.col("tgt") == "S1"))
    rp = r1.filter(pl.col("span") == "read_projection")["dur_us"].median()
    sel = r1.filter(pl.col("span") == "sel_total")["dur_us"].median()
    print(f"read_projection p50 = {rp}µs; sel_total p50 = {sel}µs; "
          f"余量(proj 面) ≈ {rp - sel if rp is not None and sel is not None else 'n/a'}µs")

    # ── 物化路径指纹(机制查证):每轮首读的 zone 材料化路径 ──
    if mz_all:
        mz = pl.DataFrame(mz_all, strict=False)
        mz1 = mz.filter((pl.col("seq") == "1"))
        print("\n=== 物化路径指纹(读#1 每轮: mz_new_zone.old_up/zone + old_full_of.src + mz_mid.zone) ===")
        print(mz1.select(["k", "span", "zone", "old_up", "src", "dur_us"]).sort("k"))
        # 链深直证:读#1 的 old_full_of/old_of_inner 逐层所见
        chain = mz.filter(
            (pl.col("span").is_in(["old_full_of", "old_of_inner"])) & (pl.col("seq") == "1")
        )
        print("\n=== 旧链深度直证(读#1 的 old_full_of / old_of_inner,逐层) ===")
        print(chain.select(["k", "span", "depth", "src", "state", "slot_id", "dur_us"]).sort(["k", "depth"]))
        # 真取物配对(只算 full 口径——render-only 取物不影响读方):
        # 槽 k+7 的 bag∈{full,both} 取物 ⟺ 读#1(k) 的 depth-0 asm
        mv = mz.filter((pl.col("span") == "reaper_take") & (pl.col("bag").is_in(["full", "both"]))).with_columns(
            (pl.col("slot_id").cast(pl.Int64)).alias("sid")
        )
        st = mz.filter((pl.col("span") == "old_of_inner") & (pl.col("seq") == "1") & (pl.col("depth") == "0"))
        mv_by_slot = mv.group_by(["sess", "sid"]).agg(pl.col("bag").count().alias("full_takes"))
        st2 = st.group_by(["sess", "k"]).agg(pl.col("state").first().alias("st0"))
        pair2 = st2.join(mv_by_slot, left_on=["sess", pl.col("k") + 7], right_on=["sess", "sid"], how="left")
        print("\n=== 真取物配对(full 口径: 槽 k+7 取物 ⟺ 读#1(k) depth-0 状态) ===")
        print(pair2.select(["sess", "k", "st0", "full_takes"]).sort(["sess", "k"]))

        # 机制断言(四支路,因果树归因 + 读侧直证槽号——分析结论的可执行形态)
        mechanism_assert(mz)

    # ── 异步 worker 归因(上一轮写的渲染/补丁,落在谁的时间窗) ──
    if async_all:
        ar_df = pl.DataFrame(async_all, strict=False)
        roots_df = ar_df.filter(pl.col("span").is_in(["fj_render_total", "path_render_total"]))
        print("\n=== 异步 worker(FJ 全量渲染 / Path 补丁,写后,落在各 op 时间窗) p50 µs ===")
        print(roots_df.group_by(["op", "seq", "tgt", "span"]).agg(
            (pl.col("dur_us").median()).alias("p50_us"),
            pl.col("dur_us").count().alias("n"),
        ).sort("p50_us", descending=True))
        print(f"[async] 落在 op 窗口内的根 span {roots_df.height} 个;"
              f" gap(间隙)根 span {ar_df.filter(pl.col('op') == 'gap').filter(pl.col('span').is_in(['fj_render_total','path_render_total'])).height} 个")


if __name__ == "__main__":
    sys.exit(main())
