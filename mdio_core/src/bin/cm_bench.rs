//! MDIO benchmark —— 三条真实出口（不含任何编辑）。
//! `cargo run --release --bin cm-bench`
//!
//! 模式口径：三条出口**一律跟随进程模式**（`topology::process_mode()`，默认 Hs；
//! `MDIO_PROJ_MODE=cf` 可切 Cf 做 A/B），报告头会打印实际模式 —— 避免量到 Cf
//! 却当成生产（Hs）口径。
//!
//! 路径口径（每条输出都带标签，任何一条不得单独当成"生产速度"）：
//!   纯引擎直用 : MdioEngine::new(text).make_snapshot().to_ast_object_mode(process_mode())
//!                 （无 store/session——单测、嵌入方的真出口；参照口径）
//!   host 冷启动 : ProjectStore(reset_doc 写入) → 首次 get_ast（生产真出口）
//!   host 热路径 : 同会话第二次 get_ast（① 整段 memo 稳态）
//! 方法：1 warmup + N timed rounds（>20MB 文件 N=1），报 min。
//! 阶段分解不在此处打点：需要阶段账时用 telemetry 构建跑
//!   CM_TELE=1 CM_REPORT=1（deep 子项加 MDIO_TELE_DEEP=1；大文档把
//!   MDIO_TELEMETRY_MAX_EVENTS 调大防事件截断）——
//!   报告层级由 telemetry 从 events 因果父链**自动推导**（见
//!   telemetry::report_text），任何新 span 自动入树，无需维护层级表。

use std::path::Path;
use std::time::Instant;
use mdio_core::MdioEngine;

// ===== 可选 telemetry 采集 =====
// 一个（文件 × rep × 场景）一个 bench session；session 名即场景维度。
#[cfg(feature = "telemetry")]
mod collect {
    use mdio_core::telemetry::{self, TelemetryInner, TelemetryMode};
    use std::sync::Arc;

    /// 每个场景：导出行 JSON ＋ 该场景的 session（供内置报告读 events 父链）。
    pub struct Sink { items: Vec<(serde_json::Value, Arc<TelemetryInner>)> }

    pub fn maybe() -> Option<Sink> {
        let on = std::env::var_os("CM_TELE");
        if on.is_some() {
            // 先付 cputicks 频率校准税（~50ms，OnceLock 每进程一次）——否则它会被
            // 记进第一个 span 所在的场景。
            let t0 = std::time::Instant::now();
            let _ = telemetry::ticks_ns();
            eprintln!("[CM_TELE] clock warmup {:.1}ms（校准税已付在窗口外）",
                      t0.elapsed().as_secs_f64() * 1000.0);
        }
        on.map(|_| Sink { items: Vec::new() })
    }

    pub fn run<T>(sink: Option<&mut Sink>, file: &str, rep: usize, stage: &str, f: impl FnOnce() -> T) -> T {
        let Some(sink) = sink else { return f() };
        // CM_REPORT=1 ⇒ 强制 Trace：内置报告要从 events 的因果父链推导树，
        // Bench 模式的 events 只留 kv span（树只剩 stage 一层）。
        let explicit_trace = matches!(std::env::var("CM_TELE_MODE").as_deref(), Ok("trace"));
        let report = std::env::var("CM_REPORT").is_ok();
        let mode = if explicit_trace || report {
            TelemetryMode::Trace
        } else {
            TelemetryMode::Bench
        };
        let sess = Arc::new({
            let mut cfg = mdio_core::telemetry::TelemetryConfig::from_env();
            if report && std::env::var("MDIO_TELEMETRY_MAX_EVENTS").is_err() {
                // 报告模式默认抬高事件上限：默认 4096 连 TEST.md 的 render 都截断
                // （根 stage 被挤掉 → 树退化为孤儿列表）。显式设过 env 则尊重之。
                cfg.max_events = 2_000_000;
            }
            TelemetryInner::new_with_config(format!("cm|{file}|{stage}|{rep}"), mode, cfg)
        });
        let guard = telemetry::set_session(&sess);
        let out = {
            let _sp = telemetry::span("stage")
                .kv("file", file).kv("stage", stage).kv("rep", rep);
            f()
        };
        drop(guard);
        if let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(&telemetry::export_phase_json(&sess)) {
            let is_trace = matches!(mode, TelemetryMode::Trace);
            let mut rec = serde_json::json!({
                "file": file, "rep": rep, "stage": stage,
                "mode": if is_trace { "trace" } else { "bench" },
                // 投影模式（口径留痕）：三条出口一律跟随进程模式（默认 hs）
                "proj_mode": mdio_core::topology::process_mode().name(),
                "rows": rows,
            });
            // chrome 只在**显式** trace 时导出（report 强制 trace 时省掉这份大文件）
            if explicit_trace {
                rec["chrome"] = serde_json::Value::String(telemetry::report_json_trace(&sess));
            }
            sink.items.push((rec, sess));
        }
        out
    }

    impl Sink {
        pub fn flush(&self) {
            if self.items.is_empty() { return; }
            let dir = std::env::var("CM_TELE_OUT").unwrap_or_else(|_| "/tmp/cmtele".into());
            if let Err(e) = std::fs::create_dir_all(&dir) {
                eprintln!("[CM_TELE] 建目录失败 {dir}: {e}");
                return;
            }
            let mut n = 0usize;
            for (it, _sess) in &self.items {
                let f = it["file"].as_str().unwrap_or("?");
                let s = it["stage"].as_str().unwrap_or("?");
                let r = it["rep"].as_u64().unwrap_or(0);
                let name = format!("{dir}/{f}__{s}__r{r}.json");
                match serde_json::to_string(it) {
                    Ok(txt) => { if std::fs::write(&name, txt).is_ok() { n += 1; } }
                    Err(e) => eprintln!("[CM_TELE] 序列化失败 {name}: {e}"),
                }
            }
            eprintln!("[CM_TELE] 写出 {n} 个场景文件 → {dir}/");
        }

        /// 阶段账报告（CM_REPORT=1）：**层级由 telemetry 从 events 因果父链自动
        /// 推导**（telemetry::report_text），无需声明层级常量；新 span 自动入树。
        /// 事件上限看 MDIO_TELEMETRY_MAX_EVENTS（默认 4096，大文档请调大，
        /// 截断会打印警告）。deep 子项需采集时带 MDIO_TELE_DEEP=1。
        pub fn report(&self) {
            if std::env::var("CM_REPORT").is_err() { return; }
            for (it, sess) in &self.items {
                let file = it["file"].as_str().unwrap_or("?");
                let stage = it["stage"].as_str().unwrap_or("?");
                println!("\n===== {file} | {stage}（deep 档数字含计数器膨胀）=====");
                print!("{}", telemetry::report_text(sess));
            }
        }
    }
}

#[cfg(not(feature = "telemetry"))]
#[allow(dead_code)]
mod collect {
    pub struct Sink;
    pub fn maybe() -> Option<Sink> { None }
    #[inline]
    pub fn run<T>(_sink: Option<&mut Sink>, _file: &str, _rep: usize, _stage: &str, f: impl FnOnce() -> T) -> T { f() }
    impl Sink { pub fn flush(&self) {} pub fn report(&self) {} }
}

fn main() {
    let data_dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_data"));
    let mut paths: Vec<(String, String, u64)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(data_dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map_or(false, |x| x == "md") {
                let sz = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                paths.push((p.file_name().unwrap().to_string_lossy().to_string(), p.to_string_lossy().to_string(), sz));
            }
        }
    }
    let cm = concat!(env!("CARGO_MANIFEST_DIR"), "/../commonmark.md");
    if Path::new(cm).exists() {
        paths.push(("commonmark.md".into(), cm.into(), std::fs::metadata(cm).map(|m| m.len()).unwrap_or(0)));
    }
    paths.sort_by_key(|(_, _, sz)| *sz);
    if let Ok(only) = std::env::var("CM_ONLY") {
        paths.retain(|(name, _, _)| name.contains(only.as_str()));
    }

    let mut tel = collect::maybe();

    // host 入口需要持久化开启（构造时一次判定；不影响纯引擎路径）。
    std::env::set_var("MDIO_PERSIST", "1");
    let store_dir = format!("/tmp/__cmbench_store_{}", std::process::id());
    let _ = std::fs::create_dir_all(&store_dir);
    let store = mdio_core::store::ProjectStore::new_with_layout(
        &store_dir, Some("wt".into()), Some(".wal".into())).ok();
    if store.is_none() {
        eprintln!("[cm_bench] store 建失败——只报 P3");
    }

    println!("=== cm_bench（三条真实出口 · 模式 {}）===", mdio_core::topology::process_mode().name());
    println!("P3 引擎直用 | P1 host 冷（首次 to_json）| P2 host 热（二次 to_json）");

    // 跨文件总表行（min-of-N）：文件 | inKB | lines | P3parse | P3render | P3合计 |
    // P1open | P1cold | P2hot | P3出 | P1出 | P2出 | 对拍
    let mut rows: Vec<(String, f64, usize, [f64; 5], f64, String)> = Vec::new();
    let mut idx = 0usize;
    for (name, path, _sz) in &paths {
        let text = match std::fs::read_to_string(path) { Ok(t) => t, Err(_) => continue };
        let lines = text.lines().count();
        let in_kb = text.len() as f64 / 1024.0;
        // warmup（排除首次触达/分配器/页缺失）
        {
            let mut e = MdioEngine::new(&text);
            let _ = e.make_snapshot().to_ast_object_mode(mdio_core::topology::process_mode());
        }
        let iters: usize = std::env::var("CM_REPS").ok().and_then(|v| v.parse().ok())
            .unwrap_or(if text.len() > 20_000_000 { 1 } else { 3 });
        let mut best = [f64::MAX; 5]; // p3_parse p3_render p1_open p1_cold p2_hot
        let mut out_kb = 0.0f64;
        let mut ct_eq = true;
        let mut len_eq = true;
        for rep in 0..iters {
            // ── 引擎直用 ──
            let snap = collect::run(tel.as_mut(), name, rep, "p3parse", || {
                let t0 = Instant::now();
                let mut eng = MdioEngine::new(&text);
                let s = eng.make_snapshot();
                best[0] = best[0].min(t0.elapsed().as_secs_f64() * 1000.0);
                s
            });
            let j3 = collect::run(tel.as_mut(), name, rep, "p3render", || {
                let t0 = Instant::now();
                let j = snap.to_ast_object_mode(mdio_core::topology::process_mode());
                best[1] = best[1].min(t0.elapsed().as_secs_f64() * 1000.0);
                j
            });
            out_kb = j3.len() as f64 / 1024.0;
            // ── host 冷 / host 热 ──
            if let Some(store) = store.as_ref() {
                let vpath = format!("/bench_{}_{}.md", idx, name.replace('/', "_"));
                let doc = collect::run(tel.as_mut(), name, rep, "p1open", || {
                    let t0 = Instant::now();
                    let _ = store.create_path("genesis", &vpath, false, false);
                    let d = store.reset_doc("genesis", &vpath, &text);
                    best[2] = best[2].min(t0.elapsed().as_secs_f64() * 1000.0);
                    d
                });
                if let Ok(doc_id) = doc {
                    let j1 = collect::run(tel.as_mut(), name, rep, "p1cold", || {
                        let t0 = Instant::now();
                        let j = store.get_ast(doc_id);
                        best[3] = best[3].min(t0.elapsed().as_secs_f64() * 1000.0);
                        j
                    });
                    let _ = collect::run(tel.as_mut(), name, rep, "p2hot", || {
                        let t0 = Instant::now();
                        let j = store.get_ast(doc_id);
                        best[4] = best[4].min(t0.elapsed().as_secs_f64() * 1000.0);
                        j
                    });
                    match (j1.as_ref(), &j3) {
                        (Some(a), b) => {
                            len_eq = a.len() == b.len();
                            // 大文件（>64MB 输出）只比长度，避免整串 serde 解析的开销
                            if len_eq && out_kb < 64.0 * 1024.0 {
                                let av: serde_json::Value = serde_json::from_str(a).unwrap_or(serde_json::Value::Null);
                                let bv: serde_json::Value = serde_json::from_str(b).unwrap_or(serde_json::Value::Null);
                                ct_eq = av["ct"] == bv["ct"];
                            }
                        }
                        (None, _) => { len_eq = false; ct_eq = false; }
                    }
                }
            }
        }
        idx += 1;
        let mb = |b_kb: f64, ms: f64| if ms > 0.0 { b_kb / ms } else { 0.0 }; // KB/ms == MB/s
        // >64MB 输出只比了长度——标签如实标 len，避免把「只查长度」说成「ct 一致」
        let eq = if ct_eq && len_eq { "一致" } else { "★不一致" };
        let eq_label = if out_kb < 64.0 * 1024.0 { "ct(P1vsP3)" } else { "len(P1vsP3)" };
        println!("{}  in={:.0}KB lines={} out={:.0}KB {}={}",
            name, in_kb, lines, out_kb, eq_label, eq);
        println!("  P3 引擎直用: parse {:.2}ms | to_json {:.2}ms | 合计 {:.2}ms | 出 {:.1}MB/s",
            best[0], best[1], best[0] + best[1], mb(out_kb, best[1]));
        println!("  P1 host 冷 : open(写md+会话) {:.2}ms | to_json首次 {:.2}ms | 出 {:.1}MB/s",
            best[2], best[3], mb(out_kb, best[3]));
        println!("  P2 host 热 : to_json二次 {:.2}ms | 出 {:.0}MB/s",
            best[4], mb(out_kb, best[4]));
        rows.push((name.clone(), in_kb, lines, best, out_kb, eq.to_string()));
    }
    // ── 跨文件总表 ──
    println!("\n===== 跨文件总表（min-of-N；MB/s = 出KB / 该列耗时）=====");
    println!("{:<26}{:>7}{:>8}{:>9}{:>10}{:>9}{:>9}{:>9}{:>8}{:>7}{:>7}{:>8}  {}",
        "文件", "inKB", "lines", "P3parse", "P3tojson", "P3合计", "P1open", "P1cold", "P2hot", "P3MB/s", "P1MB/s", "P2MB/s", "对拍");
    for (name, in_kb, lines, b, out_kb, eq) in &rows {
        let mb = |ms: f64| if ms > 0.0 { out_kb / ms } else { 0.0 };
        println!("{:<26}{:>7.0}{:>8}{:>8.2}{:>9.2}{:>9.2}{:>9.2}{:>8.2}{:>8.3}{:>7.1}{:>7.1}{:>7.0}  {}",
            name, in_kb, lines, b[0], b[1], b[0] + b[1], b[2], b[3], b[4],
            mb(b[1]), mb(b[3]), mb(b[4]), eq);
    }
    // 清理本次的 store 临时目录（每次运行一个独立 pid 目录）
    let _ = std::fs::remove_dir_all(&store_dir);
    if let Some(tel) = tel.as_mut() { tel.flush(); tel.report(); }
}
