//! Multi-round telemetry benchmark. Writes JSON for Polars analysis.
//! `cargo run --release --features "telemetry,inline" --bin stream_bench`
use std::time::Instant;
use std::path::Path;
use std::sync::Arc;
use mdio_core::MdioEngine;
// spans 计时段用 new_infer LineView 构建。
use mdio_core::telemetry::{TelemetryInner, TelemetryMode, set_session, span};

fn find_commonmark() -> String {
    for p in &["../commonmark.md", "../../commonmark.md", "../../../commonmark.md",
               concat!(env!("CARGO_MANIFEST_DIR"), "/../commonmark.md")] {
        if Path::new(p).exists() { return p.to_string(); }
    }
    panic!("not found");
}

fn main() {
    let text = std::fs::read_to_string(&find_commonmark()).unwrap();
    let n_iters = 15;

    // Warmup
    for _ in 0..5 {
        let mut engine = MdioEngine::new(&text);
        let _ = engine.make_snapshot().to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
    }

    let mut all_phases: Vec<serde_json::Value> = Vec::new();

    for iter in 0..n_iters {
        let sess = Arc::new(TelemetryInner::from_env("stream-bench".into(), TelemetryMode::Bench));
        let _guard = set_session(&sess);

        let t0 = Instant::now();
        let mut engine = MdioEngine::new(&text);
        let parse_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let _top = span("render");
        let snap = engine.make_snapshot();
        let n = snap.lines.len();
        {
            let _s = span("G");
            let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
            mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
            let (recs, arena) = fold.finish();
            let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
            let _spans = mdio_core::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries, &snap.lines, &snap.rope, (0, n as u32), &arena);
        }
        {
            let _s = span("write");
            let mut buf = Vec::with_capacity((text.len() as f64 * 3.5) as usize + 4096);
            // 归属树渲染
            let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
            mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
            let (recs, arena) = fold.finish();
            let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
            let view = mdio_core::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries, &snap.lines, &snap.rope, (0, n as u32), &arena);
            let tree = mdio_core::infer::build_attribution_tree(&recs, &blocks, &view, &arena, (0, n as u32),
                &mdio_core::topology::CONTAINER_TOPOLOGY);
            buf.push(b'[');
            mdio_core::incr::render_tree_roots(&mut buf, &snap, &view, &tree,
                &mdio_core::topology::CONTAINER_TOPOLOGY);
            buf.push(b']');
            let _json = unsafe { String::from_utf8_unchecked(buf) };
        }
        drop(_top);

        let render_ms = Instant::now().duration_since(t0).as_secs_f64() * 1000.0 - parse_ms;

        let phase_json = mdio_core::telemetry::export_phase_json(&sess);
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&phase_json) {
            for mut item in arr {
                item.as_object_mut().unwrap().insert("iter".into(), serde_json::json!(iter));
                item.as_object_mut().unwrap().insert("parse_ms".into(), serde_json::json!(parse_ms));
                item.as_object_mut().unwrap().insert("render_ms".into(), serde_json::json!(render_ms));
                all_phases.push(item);
            }
        }
        eprintln!("iter {}: parse={:.2}ms render={:.2}ms", iter, parse_ms, render_ms);
    }

    let out = serde_json::to_string(&all_phases).unwrap();
    std::fs::write("/tmp/stream_phases.json", &out).unwrap();
    println!("wrote {} records to /tmp/stream_phases.json", all_phases.len());
}
