//! Probe — telemetry-based pipeline profiler for a known MD file.
//! `cargo run --release --bin probe --features "telemetry"`

use std::sync::Arc;
use std::time::Instant;
use mdio_core::MdioEngine;
use mdio_core::telemetry::{TelemetryInner, TelemetryMode, set_session, export_phase_json};

fn main() {
    let default = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/test_data/probe.md");
    let path = std::env::args().nth(1).unwrap_or_else(|| default.to_string());
    let text = std::fs::read_to_string(&path).unwrap();
    let fname = std::path::Path::new(&path).file_name().unwrap().to_string_lossy();
    let lines_count = text.lines().count();
    let byte_len = text.len();

    let sess = Arc::new(TelemetryInner::from_env("probe".into(), TelemetryMode::Bench));
    let _guard = set_session(&sess);

    println!("=== INPUT: {fname} ===");
    println!("lines: {lines_count}  bytes: {byte_len}");
    println!();

    // Warmup
    { let mut e = MdioEngine::new(&text); let _ = e.make_snapshot().to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf); }
    // Reset telemetry after warmup
    sess.clear();

    // Timed run
    let t_total = Instant::now();
    let t0 = Instant::now();
    let mut engine = MdioEngine::new(&text);
    let snap = engine.make_snapshot();
    let parse_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let n = snap.lines.len();
    let fl = &snap.frame_log.entries;

    let t0 = Instant::now();
    let mut fold = mdio_core::infer::RecordFold::new(fl, &snap.lines);
    mdio_core::infer::walk_zone(fl, (0, n as u32), &mut fold);
    let (recs, arena) = fold.finish();
    let blocks = mdio_core::infer::build_block_records(fl, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
    let spans = mdio_core::infer::LineView::new(&recs, &blocks, fl, &snap.lines, &snap.rope, (0, n as u32), &arena);
    let g_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t0 = Instant::now();
    let est = (snap.byte_len() as f64 * 3.5) as usize + 4096;
    let mut buf = Vec::with_capacity(est);
    // 归属树渲染
    let mut fold = mdio_core::infer::RecordFold::new(fl, &snap.lines);
    mdio_core::infer::walk_zone(fl, (0, n as u32), &mut fold);
    let (recs, arena) = fold.finish();
    let blocks = mdio_core::infer::build_block_records(fl, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
    let view = mdio_core::infer::LineView::new(&recs, &blocks, fl, &snap.lines, &snap.rope, (0, n as u32), &arena);
    let tree = mdio_core::infer::build_attribution_tree(&recs, &blocks, &view, &arena, (0, n as u32),
        &mdio_core::topology::CONTAINER_TOPOLOGY);
    buf.push(b'[');
    mdio_core::incr::render_tree_roots(&mut buf, &snap, &view, &tree,
        &mdio_core::topology::CONTAINER_TOPOLOGY);
    buf.push(b']');
    let json = unsafe { String::from_utf8_unchecked(buf) };
    let walk_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let total_ms = t_total.elapsed().as_secs_f64() * 1000.0;

    println!("=== WALL CLOCK ===");
    println!("parse:   {parse_ms:.3}ms");
    println!("g-phase: {g_ms:.3}ms");
    println!("walk:    {walk_ms:.3}ms");
    println!("total:   {total_ms:.3}ms");
    println!();

    println!("=== STRUCTURE ===");
    println!("PackedLineInfo lines: {n}");
    println!("FrameLog entries:     {}", fl.len());
    println!("JSON output bytes:    {}", json.len());
    println!("JSON/input ratio:     {:.1}x", json.len() as f64 / byte_len as f64);
    println!();

    println!("=== TELEMETRY PHASE REPORT ===");
    let phase_json = export_phase_json(&sess);
    if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&phase_json) {
        // Aggregate per-name: count, sum, min, max。
        // 只取 src=="acc"（时间口径）——带 kv 的 span 在 Bench 双写
        // （acc + events），不过滤会把同一样本计两次。
        let mut by_name: std::collections::BTreeMap<String, (u64, f64, f64, f64)> = std::collections::BTreeMap::new();
        for entry in &arr {
            if entry["src"].as_str().unwrap_or("") != "acc" { continue; }
            let name = entry["name"].as_str().unwrap_or("?").to_string();
            let dur_ns = entry["dur_ns"].as_f64().unwrap_or(0.0);
            let e = by_name.entry(name).or_insert((0, 0.0, f64::MAX, 0.0));
            e.0 += 1;
            e.1 += dur_ns;
            e.2 = e.2.min(dur_ns);
            e.3 = e.3.max(dur_ns);
        }
        println!("{:<40} {:>6} {:>10} {:>10} {:>10}", "phase", "count", "sum_ms", "min_ns", "max_ns");
        println!("{}", "-".repeat(80));
        for (name, (count, sum, min, max)) in &by_name {
            println!("{:<40} {:>6} {:>10.4} {:>10.0} {:>10.0}", name, count, sum / 1_000_000.0, min, max);
        }
    } else {
        println!("(no phase data)");
    }
    println!();

    // Expected vs actual analysis
    println!("=== EXPECTED COUNTS (from document structure) ===");
    println!("lines:               {lines_count}");
    println!("scan_block calls:    {lines_count} (once per line)");
    // count non-empty lines containing inline syntax
    let inline_lines = text.lines().filter(|l| {
        !l.is_empty() && l.as_bytes().iter().any(|&b| matches!(b, b'*'|b'_'|b'~'|b'`'|b'['|b']'|b'!'|b'<'|b'&'|b'\\'))
    }).count();
    println!("lines w/ inline:     {inline_lines}");
    println!("has_inline_syntax:   ~{} calls (per block with inline trigger)", inline_lines);
    println!();
    println!("Compare with telemetry counts above.");
}
