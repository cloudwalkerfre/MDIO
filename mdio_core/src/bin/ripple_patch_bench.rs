//! ripple-patch-bench —— 随机编辑 → ripple 与 patch JSON 生成的分段基准
//!
//! 测量链：随机 edit → State::make_snapshot（ripple 增量重扫+收敛对账）
//!         → IncrTask → incr_patches（diff_patches_mode → Patch 流）
//! 直连核心（无 session/worker/Python 桥）——两段精确计时。
//!
//! 用法: cargo run --release --bin ripple-patch-bench -- [doc] [runs] [scale] [seed]
//!   doc   默认 commonmark.md; runs 默认 60; scale 文档拼接份数(默认 1); seed 默认 42
//! 输出: 每轮 (ripple_ms, patch_ms, patch_kb, patch_count) + 统计(median/p90/max) + CSV

use std::time::Instant;
use mdio_core::MdioEngine;
use mdio_core::incr::{IncrTask, incr_patches};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let doc_path = args.get(1).map(|s| s.as_str()).unwrap_or("commonmark.md");
    let runs: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60);
    let scale: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let seed: u64 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(42);

    let base = std::fs::read_to_string(doc_path)
        .unwrap_or_else(|_| panic!("doc not found: {doc_path}"));
    let mut text = String::with_capacity(base.len() * scale);
    for _ in 0..scale { text.push_str(&base); }
    let n_bytes = text.len();
    let n_lines = text.lines().count();
    println!("doc: {n_bytes} bytes, {n_lines} lines, scale={scale}, runs={runs}, seed={seed}");

    let mut rng = StdRng::seed_from_u64(seed);
    let mut rows: Vec<(f64, f64, usize, usize)> = Vec::with_capacity(runs); // (ripple_ms, patch_ms, patch_kb, patch_count)

    // 冷启动(页缓存/分配器)
    {
        let mut e = MdioEngine::new(&text);
        let _ = e.make_snapshot();
    }

    for i in 0..runs {
        // ── 1. 编辑前快照（old side）──
        let mut engine = MdioEngine::new(&text);
        let old_snap = std::sync::Arc::new(engine.make_snapshot());

        // ── 2. 随机 edit（位置 + 操作类型）──
        let (start, end, ins) = random_edit(&mut rng, text.len());
        engine.edit(start, end, &ins);
        text.replace_range(start..end, &ins);

        // ── 3. ripple（make_snapshot 内增量重扫 + 收敛）──
        let t0 = Instant::now();
        let new_snap = std::sync::Arc::new(engine.make_snapshot());
        let ripple_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // ── 4. patch JSON（diff_patches_mode → Patch 流）──
        let diff = match engine.state.ripple_diff.take() {
            Some(d) => d,
            None => { rows.push((ripple_ms, 0.0, 0, 0)); continue; }
        };
        let task = IncrTask {
            snap: new_snap.clone(),
            doc_id: 0,
            snap_old: old_snap,
            delta: diff.delta,
            edit_ranges: std::sync::Arc::new(vec![(start as u32, end as u32)]),
            ripple_line_start: diff.ripple_line_start,
            ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        let t1 = Instant::now();
        let patches = incr_patches(&task);
        let patch_ms = t1.elapsed().as_secs_f64() * 1000.0;
        // P 帧的**线路字节数**——逐字对齐 store 帧的真实序列化：
        // 帧 = `[` + op 用 `,` 分隔 + `]`，每个 op 的模板与转义都从这里原样镜像。
        // 字段写的线上形态是标准 6902 路径：`replace "<path>/<attr>"`。
        // 只统计字节，不参与计时（打点在 patch_ms 之后）。
        // 注：若 store.rs 的帧模板变更，此处须同步（否则 patch_kb 失真）。
        let mut json_len = 2usize; // 帧首尾 "[", "]"
        for (i, p) in patches.iter().enumerate() {
            if i > 0 { json_len += 1; } // op 之间的 ","
            json_len += match p {
                mdio_core::incr::Patch::Add { path, value } => format!(
                    r#"{{"op":"add","path":"{}","value":{}}}"#,
                    path.replace('\\', "\\\\").replace('"', "\\\""), value).len(),
                mdio_core::incr::Patch::Remove { path } => format!(
                    r#"{{"op":"remove","path":"{}"}}"#,
                    path.replace('\\', "\\\\").replace('"', "\\\"")).len(),
                mdio_core::incr::Patch::Replace { path, value } => format!(
                    r#"{{"op":"replace","path":"{}","value":{}}}"#,
                    path.replace('\\', "\\\\").replace('"', "\\\""), value).len(),
                mdio_core::incr::Patch::ReplaceAttr { path, attr, value } => format!(
                    r#"{{"op":"replace","path":"{}/{}","value":{}}}"#,
                    path.replace('\\', "\\\\").replace('"', "\\\""),
                    attr.replace('\\', "\\\\").replace('"', "\\\""), value).len(),
            };
        }
        let patch_kb = json_len / 1024;
        rows.push((ripple_ms, patch_ms, patch_kb, patches.len()));

        if i < 8 {
            println!("  run {i:3}: ripple={ripple_ms:8.3}ms patch={patch_ms:8.3}ms json={patch_kb:5}KB ops={:3} rls={} rle={}",
                patches.len(), diff.ripple_line_start, diff.ripple_line_end);
        }
    }

    // ── 5. 统计 ──
    fn stat(idx: usize, label: &str, unit: &str, rows: &[(f64, f64, usize, usize)]) {
        let mut v: Vec<f64> = rows.iter().map(|r| {
            match idx { 0 => r.0, 1 => r.1, 2 => r.2 as f64, _ => r.3 as f64 }
        }).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = v[v.len() / 2];
        let p90 = v[((v.len() as f64 * 0.9) as usize).min(v.len() - 1)];
        let max = *v.last().unwrap();
        let avg = v.iter().sum::<f64>() / v.len() as f64;
        println!("{label:12} avg={avg:9.3} median={med:9.3} p90={p90:9.3} max={max:9.3}  ({unit})");
    }
    println!("\n=== 统计 ({runs} 随机 edits) ===");
    stat(0, "ripple", "ms", &rows);
    stat(1, "patch_json", "ms", &rows);
    stat(2, "patch_kb", "KB", &rows);
    stat(3, "patch_ops", "ops", &rows);
    let tot: f64 = rows.iter().map(|r| r.0 + r.1).sum();
    println!("  ripple+patch 合计 avg = {:.3} ms/次", tot / rows.len() as f64);

    // CSV（便于后续分析/回归）
    let csv_path = format!(".telemetry/ripple_patch_bench_{}_{}_{}.csv",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        scale, seed);
    let mut csv = String::from("run,ripple_ms,patch_ms,patch_kb,patch_ops\n");
    for (i, r) in rows.iter().enumerate() {
        csv.push_str(&format!("{i},{:.6},{:.6},{},{}\n", r.0, r.1, r.2, r.3));
    }
    std::fs::create_dir_all(".telemetry").ok();
    let _ = std::fs::write(&csv_path, csv);
    println!("[CSV] {csv_path}");
}

/// 随机编辑：插入/删除/替换/列表/标题 —— 覆盖内容编辑与结构编辑。
fn random_edit(rng: &mut StdRng, len: usize) -> (usize, usize, String) {
    let pos = rng.gen_range(0..len.max(1));
    let op = rng.gen_range(0..6);
    match op {
        0 => { // 段落插入（内容编辑——行数+）
            (pos, pos, format!("inserted paragraph {}\n\n", rng.gen_range(0..100)))
        }
        1 => { // 删除区间（行数-）
            let end = (pos + rng.gen_range(0..(len - pos).max(1))).min(len);
            (pos, end, String::new())
        }
        2 => { // 文本替换（内容编辑——行数不变）
            let end = (pos + rng.gen_range(0..80.min(len - pos).max(1))).min(len);
            (pos, end, format!("replacement text {}\n", rng.gen_range(0..1000)))
        }
        3 => { // 列表项插入（结构编辑）
            (pos, pos, format!("- item {}\n- item {}\n", rng.gen_range(0..100), rng.gen_range(0..100)))
        }
        4 => { // 标题插入（结构编辑）
            (pos, pos, format!("# Heading {}\n\n", rng.gen_range(0..1000)))
        }
        _ => { // 引用块（结构编辑）
            (pos, pos, format!("> quote line {}\n> quote line {}\n\n", rng.gen_range(0..100), rng.gen_range(0..100)))
        }
    }
}
