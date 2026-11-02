use super::*;

#[test] fn test_hash_deterministic() { assert_eq!(hash_name("a"), hash_name("a")); }
#[test] fn test_hash_different() { assert_ne!(hash_name("a"), hash_name("b")); }

#[test]
fn test_span_noop() { let g = span("t"); assert!(g.session.is_none()); drop(g); }

#[test]
fn test_span_session() {
    let s = Arc::new(TelemetryInner::new("t".into(), TelemetryMode::Trace));
    let _g = set_session(&s);
    drop(span("p"));
    assert_eq!(s.events.lock().len(), 1);
}

#[test]
fn test_nested() {
    let s = Arc::new(TelemetryInner::new("n".into(), TelemetryMode::Trace));
    let _g = set_session(&s);
    let o = span("o"); let i = span("i"); drop(i); drop(o);
    let ev = s.events.lock();
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[0].parent_hash, hash_name("o"));
    assert_eq!(ev[1].parent_hash, 0);
}

#[test]
fn test_meta() {
    let s = Arc::new(TelemetryInner::new("m".into(), TelemetryMode::Trace));
    let _g = set_session(&s);
    drop(span("p").kv("doc", "42"));
    let ev = s.events.lock();
    assert!(ev[0].meta_len > 0);
}

#[test]
fn test_bench() {
    let s = Arc::new(TelemetryInner::new("b".into(), TelemetryMode::Bench));
    let _g = set_session(&s);
    for _ in 0..10 { drop(span("bp")); }
    assert_eq!(s.accumulator.lock().as_ref().unwrap().durations[&hash_name("bp")].len(), 10);
}

#[test]
fn test_bench_phase_cap() {
    // 每 phase 上限 100k 样本 — 超出静默丢弃 (p99 精度足够)
    let mut acc = PhaseAccumulator::new();
    for i in 0..(MAX_BENCH_SAMPLES_PER_PHASE + 1000) {
        acc.record(42, i as u64);
    }
    assert_eq!(acc.durations[&42].len(), MAX_BENCH_SAMPLES_PER_PHASE);
}

#[test]
fn test_guard_restore() {
    let s1 = Arc::new(TelemetryInner::new("1".into(), TelemetryMode::Trace));
    let s2 = Arc::new(TelemetryInner::new("2".into(), TelemetryMode::Trace));
    let _g1 = set_session(&s1);
    let g2 = set_session(&s2);
    assert_eq!(current_session().unwrap().name, "2");
    drop(g2);
    assert_eq!(current_session().unwrap().name, "1");
}

#[test] fn test_clear() {
    let s = Arc::new(TelemetryInner::new("c".into(), TelemetryMode::Trace));
    let _g = set_session(&s); drop(span("p"));
    assert_eq!(s.events.lock().len(), 1); s.clear();
    assert_eq!(s.events.lock().len(), 0);
}

#[test] fn test_phase_idempotent() {
    let s = Arc::new(TelemetryInner::new("r".into(), TelemetryMode::Trace));
    assert_eq!(s.register_phase("x"), s.register_phase("x"));
}

#[test] fn test_kv_no_session() {
    let g = span("k").kv("a", "b"); assert!(g.session.is_none()); drop(g);
}

#[test]
fn test_overflow() {
    let mut inner = TelemetryInner::new("o".into(), TelemetryMode::Trace);
    inner.max_events = 1;
    let s = Arc::new(inner);
    let _g = set_session(&s);
    drop(span("a")); drop(span("b"));
    assert_eq!(s.events.lock().len(), 1);
    assert_eq!(s.events_dropped.load(Ordering::Relaxed), 1);
}
#[test] fn test_json_output() {
    let sess = Arc::new(TelemetryInner::new("json_test".into(), TelemetryMode::Trace));
    let _g = set_session(&sess); drop(span("phase_a").kv("key", "val"));
    let json = report_json_trace(&sess);
    assert!(json.contains("traceEvents") && json.contains("phase_a"));
}
#[test]
fn test_json_trace_has_parent() {
    // Chrome trace JSON 的 args 必须带父 span 名 — 树结构不丢
    let sess = Arc::new(TelemetryInner::new("parent_json".into(), TelemetryMode::Trace));
    let _g = set_session(&sess);
    { let _o = span("outer"); { let _i = span("inner"); } }
    let json = report_json_trace(&sess);
    assert!(json.contains(r#""parent":"outer""#), "inner 事件应带 parent=outer: {}", json);
}
#[test] fn test_export_phase_json_trace_mode() {
    // Trace 模式 (accumulator=None) 也必须能从 events 导出 phase 数据
    let s = Arc::new(TelemetryInner::new("trace_phase".into(), TelemetryMode::Trace));
    let _g = set_session(&s);
    drop(span("p1"));
    drop(span("p2"));
    let out = export_phase_json(&s);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "p1");
    assert_eq!(arr[1]["name"], "p2");
}

// ── Complex multi-span tests ──

#[test]
fn test_complex_trace() {
    let s = Arc::new(TelemetryInner::new("complex_trace".into(), TelemetryMode::Trace));
    let _g = set_session(&s);

    // Simulate: vfs_resolve → session_open → execute_txn → crdt_apply → doc_flush
    {
        let _a = span("vfs_resolve").kv("path", "/doc.md");
        // inside vfs_resolve: tree lookup + arena access
        { let _b = span("tree_lookup"); }
        { let _b = span("arena_get_meta"); }
    }
    {
        let _a = span("session_open");
        // inside session_open: alloc + init
        { let _b = span("engine_init"); }
        { let _b = span("publisher_init"); }
    }
    {
        let _a = span("execute_txn");
        {
            let _b = span("crdt_apply");
            { let _c = span("decode_update"); }
            { let _c = span("merge_ops"); }
        }
        { let _b = span("hook_detect"); }
    }
    {
        let _a = span("doc_flush");
        { let _b = span("rope_to_text"); }
        { let _b = span("vfs_write"); }
    }

    let json = report_json_trace(&s);
    assert!(json.contains("vfs_resolve"));
    assert!(json.contains("session_open"));
    assert!(json.contains("execute_txn"));
    assert!(json.contains("crdt_apply"));
    assert!(json.contains("doc_flush"));
    assert_eq!(s.events.lock().len(), 14);
}

#[test]
fn test_complex_bench() {
    let s = Arc::new(TelemetryInner::new("complex_bench".into(), TelemetryMode::Bench));
    let _g = set_session(&s);

    // Run 5 rounds of the same pipeline
    for _ in 0..5 {
        let _a = span("full_pipeline");
        { let _b = span("vfs_resolve"); }
        { let _b = span("session_open"); }
        {
            let _b = span("execute_txn");
            { let _c = span("crdt_apply"); }
        }
        { let _b = span("doc_flush"); }
    }

    let acc = s.accumulator.lock();
    let acc = acc.as_ref().unwrap();
    assert_eq!(acc.durations[&hash_name("full_pipeline")].len(), 5);
    assert_eq!(acc.durations[&hash_name("vfs_resolve")].len(), 5);
    assert_eq!(acc.durations[&hash_name("execute_txn")].len(), 5);
    assert_eq!(acc.durations[&hash_name("crdt_apply")].len(), 5);
    assert_eq!(acc.durations[&hash_name("doc_flush")].len(), 5);
}

#[test]
fn test_py_span_exit_bench_dispatch() {
    // Verify that py_span_exit writes to accumulator in bench mode
    let s = Arc::new(TelemetryInner::new("py_bench".into(), TelemetryMode::Bench));
    let name = "python_span";

    // simulate py_span_enter
    py_span_enter(&s, name);
    // simulate py_span_exit
    py_span_exit(&s, name, vec![]);

    // should be in accumulator, not events
    let acc = s.accumulator.lock();
    let acc = acc.as_ref().unwrap();
    assert!(acc.durations.contains_key(&hash_name(name)),
        "py_span_exit should write to accumulator in bench mode");
    assert_eq!(acc.durations[&hash_name(name)].len(), 1);
    // events should be empty
    assert_eq!(s.events.lock().len(), 0);
}

// ---- Layer breakdown tests ----

#[test]
fn test_layer_breakdown_empty() {
    let s = Arc::new(TelemetryInner::new("empty_layers".into(), TelemetryMode::Trace));
    assert_eq!(export_layer_stats_json(&s), "[]");
}

#[test]
fn test_layer_breakdown_with_data() {
    let s = Arc::new(TelemetryInner::new("layer_test".into(), TelemetryMode::Trace));
    s.add_layer_sample("RS_proc", 1000, SampleBucket::Edit, 4, 1);
    s.add_layer_sample("RS_proc", 2000, SampleBucket::Edit, 4, 2);
    s.add_layer_sample("PY_dispatch", 500, SampleBucket::Edit, 4, 1);
    s.add_layer_sample("RS_proc", 300, SampleBucket::Probe, 0x30, 0);

    let parsed: serde_json::Value = serde_json::from_str(&export_layer_stats_json(&s)).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 4);
    assert_eq!(arr[0]["layer_name"], "RS_proc");
    assert_eq!(arr[0]["bucket"], "edit");
    assert_eq!(arr[1]["doc_id"], 2);
    assert_eq!(arr[2]["layer_name"], "PY_dispatch");
    assert_eq!(arr[3]["bucket"], "probe");
    assert_eq!(arr[3]["tag"], 0x30);
}

#[test]
fn test_export_layer_stats_json() {
    let s = Arc::new(TelemetryInner::new("export_test".into(), TelemetryMode::Trace));
    s.add_layer_sample("RS_proc", 1500, SampleBucket::Edit, 4, 42);

    let json = export_layer_stats_json(&s);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["layer_name"], "RS_proc");
    assert_eq!(arr[0]["dur_ns"], 1500);
    assert_eq!(arr[0]["bucket"], "edit");
    assert_eq!(arr[0]["doc_id"], 42);
}

#[test]
fn test_merge_js_data() {
    let s = Arc::new(TelemetryInner::new("merge_test".into(), TelemetryMode::Trace));
    let js = r#"{"JS":{"send_gap":[100,200],"recv_to_render":[5000]}}"#;
    merge_js_data(&s, js);
    let stats = s.layer_stats.lock();
    let names: Vec<&str> = stats.iter().map(|x| x.layer_name.as_str()).collect();
    assert!(names.contains(&"JS_send_gap"));
    assert!(names.contains(&"JS_recv_to_render"));
    assert_eq!(stats.len(), 3);
}

#[test]
fn test_report_all_writes_files_and_clears() {
    let s = Arc::new(TelemetryInner::new("report_all_test".into(), TelemetryMode::Trace));
    let _g = set_session(&s);
    drop(span("op"));

    let out = report_all(&s, "/tmp/_mdio_telemetry_test_");
    // Rust 出口: 终端报告由 Python report_session 完成 — 返回空串
    assert!(out.is_empty(), "Terminal report delegated to Python");
    // 非空 trace 数据 → 写 Chrome trace JSON
    let jdir = "/tmp/_mdio_telemetry_test_/report_all_test";
    let n = std::fs::read_dir(jdir).map(|d| d.count()).unwrap_or(0);
    assert!(n >= 1, "Should write trace JSON");
    // 消费性: report 后 session 清空
    assert_eq!(s.events.lock().len(), 0);
}

#[test]
fn test_from_tag_all_protocol() {
    // Probe
    assert_eq!(SampleBucket::from_tag(0x30), SampleBucket::Probe);
    assert_eq!(SampleBucket::from_tag(0xB0), SampleBucket::Probe);
    // Ctrl: OpenDoc, CloseDoc, UpTree, DnTree, DnOpen, DnErr
    assert_eq!(SampleBucket::from_tag(0x01), SampleBucket::Ctrl);
    assert_eq!(SampleBucket::from_tag(0x02), SampleBucket::Ctrl);
    assert_eq!(SampleBucket::from_tag(0x03), SampleBucket::Ctrl);
    assert_eq!(SampleBucket::from_tag(0x81), SampleBucket::Ctrl);
    assert_eq!(SampleBucket::from_tag(0x82), SampleBucket::Ctrl);
    assert_eq!(SampleBucket::from_tag(0xFF), SampleBucket::Ctrl);
    // Edit: all CRDT and unknown
    assert_eq!(SampleBucket::from_tag(0x04), SampleBucket::Edit);  // UP_SYNC
    assert_eq!(SampleBucket::from_tag(0x83), SampleBucket::Edit);  // DN_SYNC
    assert_eq!(SampleBucket::from_tag(0x05), SampleBucket::Edit);  // UndoReq
    assert_eq!(SampleBucket::from_tag(0x06), SampleBucket::Edit);  // RedoReq
    assert_eq!(SampleBucket::from_tag(0x00), SampleBucket::Edit);  // unknown
    assert_eq!(SampleBucket::from_tag(0x10), SampleBucket::Edit);  // unknown
}

// ── cputicks vs Instant benchmark ──────────────────────────

#[test]
fn bench_clock_overhead() {
    use std::time::Instant;
    use cputicks::Ticks;

    const WARMUP: usize = 1000;
    const ROUNDS: usize = 10_000_000;

    for _ in 0..WARMUP { let _ = Instant::now(); }
    for _ in 0..WARMUP { let _ = Ticks::now(); }

    let t0 = Instant::now();
    for _ in 0..ROUNDS { let _ = Instant::now(); }
    let instant_ns = t0.elapsed().as_nanos() as f64 / ROUNDS as f64;

    let t0 = Instant::now();
    for _ in 0..ROUNDS { let _ = Ticks::now(); }
    let ticks_ns = t0.elapsed().as_nanos() as f64 / ROUNDS as f64;

    let i0 = Instant::now();
    for _ in 0..ROUNDS { let _ = i0.elapsed(); }
    let ielapsed_ns = i0.elapsed().as_nanos() as f64 / ROUNDS as f64;

    let t0 = Ticks::now();
    for _ in 0..ROUNDS { let _ = t0.elapsed(); }
    let telapsed_ns = t0.elapsed().as_duration().as_nanos() as f64 / ROUNDS as f64;

    let span_r = ROUNDS / 100;
    let tt0 = Instant::now();
    for _ in 0..span_r { let s = Instant::now(); let _ = s.elapsed().as_nanos(); }
    let span_instant = tt0.elapsed().as_nanos() as f64 / span_r as f64;

    let tt0 = Instant::now();
    for _ in 0..span_r { let s = Ticks::now(); let _ = s.elapsed().as_duration().as_nanos(); }
    let span_ticks = tt0.elapsed().as_nanos() as f64 / span_r as f64;









}

// ── span 自身计时界(S5b——上轮升级的纪律面:测量靠 span 自身
// cputicks,禁外部计时器;裸语句 span(dur≈0)即红) ──

#[test]
fn span_self_measure_bounds() {
    let s = Arc::new(TelemetryInner::new("self_measure".into(), TelemetryMode::Bench));
    let _g = set_session(&s);
    {
        // let 绑定保活(§3 生命周期);black_box 每迭代防循环惯用式折叠——
        // 50M 依赖链 ≈ 几十 ms 级,落 [10ms, 10s] 宽括号不 flaky。
        let _sp = span("burn_50m");
        let mut x: u64 = 0;
        for _ in 0..50_000_000 {
            x = std::hint::black_box(x).wrapping_add(1);
        }
        std::hint::black_box(x);
    }
    let acc = s.accumulator.lock();
    let acc = acc.as_ref().unwrap();
    let durs = &acc.durations[&hash_name("burn_50m")];
    assert_eq!(durs.len(), 1);
    let d = durs[0];
    assert!(
        (10_000_000..=10_000_000_000).contains(&d),
        "span self-measure out of bounds: {d}ns"
    );
}

/// 时钟一致性硬闸(插装正确性的总闸):ticks_now/ticks_to_ns 自计时
/// 与墙钟 Instant 对拍,比率必须落 [0.7, 1.3]。若 ticks 计数器与换算
/// 频率不一致(如 CNTVCT ticks ÷ mach_absolute_time 频率 ≈ 41.7×
/// 错标),此闸必红——这是 span_self_measure_bounds 宽括号抓不住的
/// 系统性刻度错误。两段同规模黑盒加运算分别计墙钟与 tick,避开单次
/// 计时开销;带宽 0.7-1.3 覆盖 CPU 频率波动与调度噪声。
#[test]
fn clock_ticks_consistent_with_wall() {
    const N: u64 = 30_000_000;
    let mut x: u64 = 0;
    let wall = std::time::Instant::now();
    for _ in 0..N {
        x = std::hint::black_box(x).wrapping_add(1);
    }
    let wall_ns = wall.elapsed().as_nanos() as f64;
    std::hint::black_box(x);
    let t0 = ticks_ns();
    let mut y: u64 = 0;
    for _ in 0..N {
        y = std::hint::black_box(y).wrapping_add(1);
    }
    let tick_ns = (ticks_ns() - t0) as f64;
    std::hint::black_box(y);
    let ratio = tick_ns / wall_ns;
    assert!(
        (0.7..=1.3).contains(&ratio),
        "clock mismatch: tick_ns={tick_ns:.0} wall_ns={wall_ns:.0} ratio={ratio:.3}"
    );
}

/// ticks 单调性闸:连续 200k 次读取严格不减(CNTVCT 系统级单调;
/// 若出现回退 = 计数器/换核问题,时序树会塌)。
#[test]
fn clock_ticks_monotonic() {
    let mut prev = ticks_ns();
    for _ in 0..200_000 {
        let cur = ticks_ns();
        assert!(cur >= prev, "ticks went backwards: {prev} -> {cur}");
        prev = cur;
    }
}
