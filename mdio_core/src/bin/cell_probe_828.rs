//! cell_probe_828 —— cell plan(M0/判官)测量探针(telemetry 化版)
//!
//! TESTBED 纪律
//! 本文件是"专盯一点"的简易实现——忽视条件、忽视限制(无 slot 链/守卫裁决/
//! 内核路由/代际门/域密封/并发/生命周期)。**绝不可以生产照搬,照搬必炸;
//! 严禁复制粘贴进生产代码。** 只取测量结果,不取实现形态。
//!
//! 用法: cargo run --release -p mdio_core --features telemetry --bin cell-probe-828
//!
//! 纪律(docs/TELEMETRY.md):①单一时钟——时长全部走 telemetry_span!(cputicks),
//! 不混 Instant;②session 即场景——phase 名编码 尺寸×模式×测量面;
//! ③数据落盘——export_phase_json → .telemetry/cell_probe_828/export.json,
//! 聚合(Rust 侧 p50/p90/cnt)读落盘数据;④对拍类判定(等价/差异计数)是
//! 事件而非时长,保持计数输出。
//!
//! 测量面:
//!   [时长 phase × 循环] 树成本(合成 100/1000/9813 × Cf/Hs + 真文档)、段成本、
//!     守卫成本、P2 核心件(HS 全量段/标题/节区间 vs CF zone/zone 段)、
//!     hook 行级 scope 成本
//!   [计数面] hook 换源 gate(反例+随机对照)、selector 语义 gate 差异分类、
//!     归因解剖(跨界 vs 完整)、守卫推断四象限(trinity 对拍)

use mdio_core::engine::MdioEngine;
use mdio_core::hook::compute_scope_range;
use mdio_core::infer::{
    build_attribution_tree, build_structure_parts, deepest_section, expand_zone_to_blocks_fl,
    section_intervals, LineView,
};
use mdio_core::selector::Step;
use mdio_core::telemetry::{export_phase_json, set_session, TelemetryInner, TelemetryMode};
use mdio_core::topology::{CONTAINER_TOPOLOGY, HEADINGSECTION_TOPOLOGY};
use std::sync::Arc;

fn next_rand(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// 记录结构同一性（arena 化后 ContainerRec 不 derive Eq——逐字段 + 列切片比较）。
fn recs_struct_eq(
    a: &[mdio_core::infer::ContainerRec],
    aa: &mdio_core::infer::RecArena,
    b: &[mdio_core::infer::ContainerRec],
    ab: &mdio_core::infer::RecArena,
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).all(|(ra, rb)| {
        ra.frame == rb.frame
            && ra.start == rb.start
            && ra.end == rb.end
            && ra.flags == rb.flags
            && ra.anchor == rb.anchor
            && ra.parent == rb.parent
            && ra.ci == rb.ci
            && ra.children_of(aa).collect::<Vec<_>>() == rb.children_of(ab).collect::<Vec<_>>()
            && ra.push_lines_of(aa).collect::<Vec<_>>() == rb.push_lines_of(ab).collect::<Vec<_>>()
            && ra.body_mut_lines_of(aa).collect::<Vec<_>>()
                == rb.body_mut_lines_of(ab).collect::<Vec<_>>()
            && ra.pop_push_events_of(aa).collect::<Vec<_>>()
                == rb.pop_push_events_of(ab).collect::<Vec<_>>()
    })
}

fn synth_doc(lines: usize, seed: u64) -> String {
    let tokens: &[&str] = &[
        "plain paragraph text\n",
        "some **bold** words\n",
        "# Heading\n",
        "## Sub\n",
        "- item\n",
        "- [ ] task\n",
        "1. first\n",
        "  - nested\n",
        "> quote\n",
        "```rust\nlet x = 1;\n```\n",
        "| a | b |\n|---|---|\n| c | d |\n",
        "\n",
        "    indented code\n",
        "foo\n===\n",
        "bar\n---\n",
    ];
    let mut rng = seed;
    let mut doc = String::new();
    let mut i = 0usize;
    while i < lines {
        let tok = tokens[next_rand(&mut rng) as usize % tokens.len()];
        doc.push_str(tok);
        i += tok.lines().count();
    }
    doc
}

fn snap_parts(
    snap: &mdio_core::engine::MdioSnapshot,
) -> (
    Vec<mdio_core::infer::ContainerRec>,
    Vec<mdio_core::infer::BlockRec>,
    mdio_core::infer::ViewArrays,
    mdio_core::infer::RecArena,
) {
    build_structure_parts(
        &snap.frame_log.entries,
        &snap.lines,
        &snap.open_depth,
        &snap.rope,
        (0, snap.line_len() as u32),
    )
}

fn zone_parts(
    snap: &mdio_core::engine::MdioSnapshot,
    zs: usize,
    ce: usize,
) -> (
    Vec<mdio_core::infer::ContainerRec>,
    Vec<mdio_core::infer::BlockRec>,
    mdio_core::infer::ViewArrays,
    mdio_core::infer::RecArena,
) {
    build_structure_parts(
        &snap.frame_log.entries,
        &snap.lines,
        &snap.open_depth,
        &snap.rope,
        (zs as u32, ce as u32),
    )
}

/// 构建快照 + 预热后返回(Arc, 全量 parts, LineView 借用不方便——按需重建)。
fn prepared(doc: &str) -> (Arc<mdio_core::engine::MdioSnapshot>, String) {
    let mut eng = MdioEngine::new(doc);
    let snap = Arc::new(eng.make_snapshot());
    let _ = snap_parts(&snap); // 预热(页缓存/分配器)
    (snap, doc.to_string())
}

fn probe_tree_and_guard_cost() {
    // 树成本:合成 100/1000/9813 + 真文档(如果可读)
    let docs: Vec<(String, String)> = {
        let mut v = vec![
            ("syn100".to_string(), synth_doc(100, 0xC011_8280)),
            ("syn1000".to_string(), synth_doc(1000, 0xC011_8280)),
            ("syn9813".to_string(), synth_doc(9813, 0xC011_8280)),
        ];
        if let Ok(real) = std::fs::read_to_string("commonmark.md") {
            v.push(("real".to_string(), real));
        }
        v
    };
    for (tag, doc) in &docs {
        let snap = {
            let mut eng = MdioEngine::new(doc);
            let s = Arc::new(eng.make_snapshot());
            let _ = snap_parts(&s);
            s
        };
        let n = snap.line_len() as u32;
        let n_lines = snap.line_len();
        for _ in 0..300 {
            let seg_name = format!("tree_seg_{tag}");
            let cf_name = format!("tree_cf_{tag}");
            let hs_name = format!("tree_hs_{tag}");
            let (_recs, _blocks, _va, _arena) = {
                let _s = mdio_core::telemetry_span!(&seg_name);
                snap_parts(&snap)
            };
            let view = {
                let _s = mdio_core::telemetry_span!(&cf_name);
                let (recs, blocks, va, arena) = snap_parts(&snap);
                let view = LineView::with_arrays(
                    &recs, &blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
                    (0, n), va, &arena,
                );
                let _tree = build_attribution_tree(&recs, &blocks, &view, &arena, (0, n), &CONTAINER_TOPOLOGY);
            };
            let _ = view;
            {
                let _s = mdio_core::telemetry_span!(&hs_name);
                let (recs, blocks, va, arena) = snap_parts(&snap);
                let view = LineView::with_arrays(
                    &recs, &blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
                    (0, n), va, &arena,
                );
                let _tree = build_attribution_tree(&recs, &blocks, &view, &arena, (0, n), &HEADINGSECTION_TOPOLOGY);
            }
        }
    }
    // 守卫成本:真文档 + 合成 9813
    for (tag, doc) in [("syn9813".to_string(), synth_doc(9813, 0x9A8D_8281))]
        .into_iter()
        .chain(std::fs::read_to_string("commonmark.md").map(|d| ("real".to_string(), d)).into_iter())
    {
        let mut eng = MdioEngine::new(&doc);
        let old_snap = eng.make_snapshot();
        let mid = doc.len() / 2;
        let word_end = doc[mid..].find(|c: char| c == '\n').map(|p| mid + p).unwrap_or(doc.len());
        eng.edit(mid, word_end.min(doc.len()), "changed");
        let new_snap = eng.make_snapshot();
        let byte_len_old = old_snap.byte_len();
        let (s, e) = (mid.min(byte_len_old.saturating_sub(1)), word_end.min(byte_len_old));
        let min_line = old_snap.line_of_byte(s);
        let max_line = old_snap.line_of_byte(e.saturating_sub(1).max(s));
        let zs = old_snap.snap_to_depth0(min_line.saturating_sub(2));
        let ce_raw = (max_line + 3).min(old_snap.line_len()).max(zs);
        let (_z2, ce2) = expand_zone_to_blocks_fl(&old_snap.frame_log.entries, &old_snap.lines, &old_snap.open_depth, old_snap.line_len(), zs, ce_raw);
        for _ in 0..300 {
            {
                let name = format!("guard_old_{tag}");
                let _s = mdio_core::telemetry_span!(&name);
                let _ = zone_parts(&old_snap, zs, ce2);
            }
            {
                let name = format!("guard_new_{tag}");
                let _s = mdio_core::telemetry_span!(&name);
                let _ = zone_parts(&new_snap, zs, ce2.min(new_snap.line_len()));
            }
        }
    }
}

fn probe_p2_components() {
    let Ok(doc) = std::fs::read_to_string("commonmark.md") else { return };
    let mut eng = MdioEngine::new(&doc);
    let old_snap = Arc::new(eng.make_snapshot());
    let pos = doc.len() * 3 / 5;
    let end = doc[pos..].find('\n').map(|p| pos + p).unwrap_or(doc.len());
    eng.edit(pos, end.min(doc.len()), "changed");
    let new_snap = Arc::new(eng.make_snapshot());
    let Some(diff) = eng.state.ripple_diff.take() else { return };
    let task = mdio_core::incr::IncrTask {
        snap: new_snap.clone(),
        doc_id: 0,
        snap_old: old_snap.clone(),
        delta: diff.delta,
        edit_ranges: mdio_core::edit_actions_arc(vec![(pos, end.min(doc.len()))]),
        ripple_line_start: diff.ripple_line_start,
        ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    for _ in 0..300 {
        {
            let _s = mdio_core::telemetry_span!("p2_hs_full_pair");
            let _ = snap_parts(&old_snap);
            let _ = snap_parts(&new_snap);
        }
        {
            let _s = mdio_core::telemetry_span!("p2_hs_heads_secs");
            let (recs, blocks, _va, _arena) = snap_parts(&new_snap);
            let heads: Vec<_> = blocks.iter()
                .filter(|b| HEADINGSECTION_TOPOLOGY.heading_candidates.contains(&b.block_type))
                .map(|b| (b.start as usize, 0u8)).collect();
            let _ = heads;
            let _ = section_intervals(&blocks, &recs, &_arena, HEADINGSECTION_TOPOLOGY.heading_candidates, new_snap.line_len());
        }
        {
            let _s = mdio_core::telemetry_span!("p2_cf_zone");
            let lm = mdio_core::incr::LineMap::new(&task);
            let _ = mdio_core::incr::compute_zone(&task, &lm);
        }
        {
            let _s = mdio_core::telemetry_span!("p2_cf_zone_seg");
            let lm = mdio_core::incr::LineMap::new(&task);
            let zone = mdio_core::incr::compute_zone(&task, &lm);
            let _ = zone_parts(&old_snap, zone.zs_o, zone.ce_o);
            let _ = zone_parts(&new_snap, zone.zs_n, zone.ce_n);
        }
    }
}

fn probe_hook_row_scope_cost() {
    // hook 行级 scope 成本(随机文档 × 可 Drill 标题)
    let tokens: &[&str] = &[
        "plain paragraph\n", "# Heading\n", "## Sub\n", "- item\n", "> quote\n",
        "> # inner\n", "> foo\n> ===\n", "- foo\n  ===\n", "\n",
    ];
    let mut rng: u64 = 0x8280_5555;
    let mut docs = Vec::new();
    while docs.len() < 200 {
        let mut doc = String::new();
        let lc = 4 + (next_rand(&mut rng) % 14) as usize;
        for _ in 0..lc {
            doc.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
        }
        let mut eng = MdioEngine::new(&doc);
        let snap = eng.make_snapshot();
        let (recs, blocks, _va, _arena) = snap_parts(&snap);
        let secs = section_intervals(&blocks, &recs, &_arena, HEADINGSECTION_TOPOLOGY.heading_candidates, snap.line_len());
        if secs.is_empty() { continue; }
        let tags: Vec<String> = secs.iter().map(|&(s, _, _, _)| {
            snap.rope.line(s).to_string()
                .trim_start_matches(['#', ' ', '>'])
                .split_whitespace().next().unwrap_or("").to_string()
        }).filter(|t| !t.is_empty()).collect();
        if tags.is_empty() { continue; }
        docs.push((snap, tags));
    }
    for (snap, tags) in &docs {
        for t in tags.iter().take(3) {
            let steps = vec![Step::Drill(t.clone())];
            {
                let _s = mdio_core::telemetry_span!("hook_row_scope");
                let _ = compute_scope_range(&steps, snap);
            }
        }
    }
}

// ═══ 计数面(判定差异/象限——不是时长,不打 span) ═══

fn probe_hook_gate() {
    println!("\n=== hook 换源 gate:行级 vs 块级(反例 + 随机对照) ===");
    let cases: &[(&str, &str, &str)] = &[
        ("bq-atx", "> # H1\n> body\n> ## H2\n> more\n", "H1"),
        ("bq-setext", "> foo\n> ===\n> body\n\npara\n", "foo"),
        ("cross-bq", "# A\n> # B\n> > # C\n\n> # D\n", "A"),
        ("nested", "# A\n\ntext\n## B\n\n- a\n# C\n", "A"),
    ];
    for &(name, doc, tag) in cases {
        let mut eng = MdioEngine::new(doc);
        let snap = eng.make_snapshot();
        let (recs, blocks, _va, _arena) = snap_parts(&snap);
        let secs = section_intervals(&blocks, &recs, &_arena, HEADINGSECTION_TOPOLOGY.heading_candidates, snap.line_len());
        let anchor_line = blocks.iter()
            .find(|b| HEADINGSECTION_TOPOLOGY.heading_candidates.contains(&b.block_type)
                && snap.rope.line(b.start as usize).to_string().contains(tag))
            .map(|b| b.start as usize);
        let Some(al) = anchor_line else { continue };
        let row_scope = compute_scope_range(&vec![Step::Drill(tag.to_string())], &snap);
        let block_scope = secs.iter().find(|&&(s, _, _, _)| s == al).map(|&(s, e, _, _)| (s, e));
        let sec_nodes: Vec<(usize, usize, u8, usize)> = secs.iter().enumerate()
            .map(|(i, &(s, e, lv, _))| (s, e, lv, i)).collect();
        let oracle = deepest_section(&sec_nodes, al, al + 1);
        match (row_scope, block_scope) {
            (None, Some((bs, be))) => println!("{name:>10}: 行级 None(漏评估) | 块级 [{bs},{be}) | oracle={oracle:?}"),
            (Some((rs, re)), Some((bs, be))) => println!(
                "{name:>10}: 行级 [{rs},{re}) vs 块级 [{bs},{be}) | 等价={} | oracle={oracle:?}",
                rs == bs && re == be
            ),
            (Some((rs, re)), None) => println!("{name:>10}: 行级 [{rs},{re}) | 块级 None | oracle={oracle:?}"),
            (None, None) => println!("{name:>10}: 双侧 None"),
        }
    }
    // 随机对照
    let tokens: &[&str] = &[
        "plain paragraph\n", "# Heading\n", "## Sub\n", "### Deep\n", "- item\n",
        "> quote\n", "> # inner\n", "> foo\n> ===\n", "- foo\n  ===\n",
        "```rust\nlet x = 1;\n```\n", "| a | b |\n|---|---|\n| c | d |\n", "\n",
    ];
    let mut rng: u64 = 0x8280_3333;
    let mut total = 0usize;
    let mut same = 0usize;
    let mut row_bigger = 0usize;
    let mut row_smaller = 0usize;
    let mut row_none = 0usize;
    for _case in 0..300u64 {
        let mut doc = String::new();
        let lc = 4 + (next_rand(&mut rng) % 14) as usize;
        for _ in 0..lc {
            doc.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
        }
        let mut eng = MdioEngine::new(&doc);
        let snap = eng.make_snapshot();
        let (recs, blocks, _va, _arena) = snap_parts(&snap);
        let secs = section_intervals(&blocks, &recs, &_arena, HEADINGSECTION_TOPOLOGY.heading_candidates, snap.line_len());
        if secs.is_empty() { continue; }
        for &(s, e, _, _) in &secs {
            let line_text = snap.rope.line(s).to_string();
            let tag = line_text.trim_start_matches(['#', ' ', '>']).to_string();
            let tag = tag.split_whitespace().next().unwrap_or("").to_string();
            if tag.is_empty() { continue; }
            let row = compute_scope_range(&vec![Step::Drill(tag.clone())], &snap);
            total += 1;
            match row {
                None => row_none += 1,
                Some((rs, re)) if rs == s && re == e => same += 1,
                Some((rs, re)) if rs == s && re > e => row_bigger += 1,
                Some(_) => row_smaller += 1,
            }
        }
    }
    println!(
        "随机对照:标题={total} 等价={same}({:.1}%) 偏大={row_bigger}({:.1}%) 偏小/错起={row_smaller}({:.1}%) None={row_none}({:.1}%)",
        same as f64 / total.max(1) as f64 * 100.0,
        row_bigger as f64 / total.max(1) as f64 * 100.0,
        row_smaller as f64 / total.max(1) as f64 * 100.0,
        row_none as f64 / total.max(1) as f64 * 100.0,
    );
}

fn probe_selector_gate() {
    println!("\n=== selector 语义 gate:局部段 secs vs 全量截断(分类) ===");
    let tokens: &[&str] = &[
        "plain paragraph\n", "# Heading\n", "## Sub\n", "- item\n", "> quote\n",
        "```rust\nlet x = 1;\n```\n", "| a | b |\n|---|---|\n| c | d |\n", "\n",
        "foo\n===\n", "- foo\n  ===\n", "> # h\n> - x\n> ===\n",
    ];
    let mut rng: u64 = 0x8280_0001;
    let mut total_zones = 0usize;
    let mut diff_zones = 0usize;
    let mut lost = 0usize;
    let mut added = 0usize;
    let mut merge_only = 0usize;
    let mut cross = 0usize;
    let mut cross_diff = 0usize;
    let mut intact_diff = 0usize;
    for _case in 0..500u64 {
        let mut doc = String::new();
        let lc = 4 + (next_rand(&mut rng) % 12) as usize;
        for _ in 0..lc {
            doc.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
        }
        let mut eng = MdioEngine::new(&doc);
        let snap = eng.make_snapshot();
        let n = snap.line_len();
        let (recs, blocks, _va, _arena) = snap_parts(&snap);
        let full_secs = section_intervals(&blocks, &recs, &_arena, HEADINGSECTION_TOPOLOGY.heading_candidates, n);
        for _ in 0..3 {
            let zs = next_rand(&mut rng) as usize % n.max(1);
            let ce = (zs + 1 + (next_rand(&mut rng) as usize % (n - zs).max(1))).min(n);
            if ce <= zs { continue; }
            let zs0 = snap.snap_to_depth0(zs);
            let (lrecs, lblocks, _lva, _larena) = zone_parts(&snap, zs0, ce);
            let local_secs = section_intervals(&lblocks, &lrecs, &_larena, HEADINGSECTION_TOPOLOGY.heading_candidates, ce);
            let full_cut: Vec<(usize, usize)> = full_secs.iter()
                .filter(|&&(s, e, _, _)| s >= zs0 && s < ce && e > s)
                .map(|&(s, e, _, _)| (s, e.min(ce)))
                .collect();
            let local_cut: Vec<(usize, usize)> = local_secs.iter()
                .map(|&(s, e, _, _)| (s, e.min(ce)))
                .collect();
            let crosses = recs.iter().any(|r| {
                let rs = r.start as usize;
                let re = r.end_usize().unwrap_or(n);
                (rs < zs0 && re > ce) || (rs >= zs0 && rs < ce && re > ce)
            });
            total_zones += 1;
            if crosses { cross += 1; }
            if full_cut != local_cut {
                diff_zones += 1;
                if crosses { cross_diff += 1; } else { intact_diff += 1; }
                let full_starts: std::collections::HashSet<usize> =
                    full_cut.iter().map(|&(s, _)| s).collect();
                let local_starts: std::collections::HashSet<usize> =
                    local_cut.iter().map(|&(s, _)| s).collect();
                let l = full_starts.difference(&local_starts).count();
                let a = local_starts.difference(&full_starts).count();
                if l > 0 { lost += 1; }
                if a > 0 { added += 1; }
                if l == 0 && a == 0 { merge_only += 1; }
            }
        }
    }
    println!(
        "zones={total_zones} 差异={diff_zones}({:.1}%) 分类:丢失={lost} 新增={added} 仅尾差={merge_only} | 跨界容器区间={cross} 跨界差异={cross_diff}({:.1}%) 完整区间差异={intact_diff}({:.1}%)",
        diff_zones as f64 / total_zones.max(1) as f64 * 100.0,
        cross_diff as f64 / cross.max(1) as f64 * 100.0,
        intact_diff as f64 / (total_zones - cross).max(1) as f64 * 100.0,
    );
}

fn probe_guard_quadrants() {
    println!("\n=== 守卫推断四象限(trinity 对拍)+ 编辑类型细分 ===");
    use mdio_core::block::BlockType as BT;
    let entrance = |t: BT| -> bool {
        matches!(t, BT::Paragraph | BT::Blockquote | BT::HsSep | BT::RefDef | BT::TableRow)
    };
    let make_doc = |i: usize| -> String {
        format!(
            "| a | b |\n|---|---|\n| c{i} | d |\n\n```rust\nlet x{i} = 1;\n```\n\nplain paragraph text {i}\n\n> foo{i}\n> ===\n\nbody {i}\n"
        )
    };
    let mut rng: u64 = 0x8280_7777;
    let mut total = 0usize;
    let mut fast_pass = 0usize;
    let mut over_conservative = 0usize;
    let mut slow_correct = 0usize;
    let mut miss = 0usize;
    let mut by_kind = [0usize; 3];
    let mut fast_by_kind = [0usize; 3];
    for _case in 0..500u64 {
        let i = next_rand(&mut rng) as usize % 50;
        let doc = make_doc(i);
        let mut eng = MdioEngine::new(&doc);
        let old_snap = eng.make_snapshot();
        let kind = next_rand(&mut rng) % 3;
        let (start, end) = match kind {
            0 => { let p = doc.find("| c").unwrap() + 2; (p, p + 1) }
            1 => { let p = doc.find("let x").unwrap() + 4; (p, p + 1) }
            _ => { let p = doc.find("plain paragraph text").unwrap(); (p, p + 5) }
        };
        eng.edit(start, end, if kind == 0 { "X" } else if kind == 1 { "y" } else { "PLAIN" });
        let new_snap = eng.make_snapshot();
        if old_snap.lines.as_ref() != new_snap.lines.as_ref()
            && old_snap.lines.as_slice() != new_snap.lines.as_slice()
        { continue; }
        if !mdio_core::block::entries_eq_ignoring_consume(
            &old_snap.frame_log.entries, &new_snap.frame_log.entries) { continue; }
        total += 1;
        let min_line = old_snap.line_of_byte(start.min(old_snap.byte_len().saturating_sub(1)));
        let max_line = old_snap.line_of_byte(end.saturating_sub(1).max(start));
        let zs = old_snap.snap_to_depth0(min_line.saturating_sub(2));
        let ce_raw = (max_line + 3).min(old_snap.line_len()).max(zs);
        let (_z2, ce2) = expand_zone_to_blocks_fl(&old_snap.frame_log.entries, &old_snap.lines, &old_snap.open_depth, old_snap.line_len(), zs, ce_raw);
        let (orecs, oblocks, ova, oarena) = zone_parts(&old_snap, zs, ce2);
        let (nrecs, nblocks, nva, narena) = zone_parts(&new_snap, zs, ce2.min(new_snap.line_len()));
        let a_same = recs_struct_eq(&orecs, &oarena, &nrecs, &narena) && oblocks == nblocks && ova == nva;
        let lo = min_line.saturating_sub(2);
        let hi = (max_line + 2).min(old_snap.line_len());
        let b_same = !(lo..hi).any(|li| entrance(old_snap.lines[li].node_type()));
        match (a_same, b_same) {
            (true, true) => fast_pass += 1,
            (true, false) => over_conservative += 1,
            (false, false) => slow_correct += 1,
            (false, true) => miss += 1,
        }
        by_kind[kind as usize] += 1;
        if a_same && b_same { fast_by_kind[kind as usize] += 1; }
    }
    println!(
        "纯内容={total} 快通={fast_pass} 保守={over_conservative} 慢路={slow_correct} 漏判={miss}(必须0)  快通率={:.1}%",
        fast_pass as f64 / total.max(1) as f64 * 100.0
    );
    for (k, name) in ["表格单元格", "代码内容", "段落文本"].iter().enumerate() {
        println!(
            "  {name:>6}: {}/{} 快通({:.1}%)",
            fast_by_kind[k], by_kind[k],
            fast_by_kind[k] as f64 / by_kind[k].max(1) as f64 * 100.0
        );
    }
}

/// 聚合:读落盘 JSON → 按 name 分组 → cnt / total_ms / p50 / p90。
fn aggregate_and_print(json: &str) {
    let rows: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    let mut groups: std::collections::BTreeMap<String, Vec<u64>> = std::collections::BTreeMap::new();
    for r in &rows {
        let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let dur = r.get("dur_ns").and_then(|v| v.as_u64()).unwrap_or(0);
        groups.entry(name).or_default().push(dur);
    }
    let mut table: Vec<(String, usize, f64, u64, u64)> = groups.into_iter().map(|(name, mut ds)| {
        ds.sort_unstable();
        let cnt = ds.len();
        let total = ds.iter().sum::<u64>() as f64 / 1e6;
        let p50 = ds[cnt / 2] / 1000;
        let p90 = ds[(cnt as f64 * 0.9) as usize % cnt.max(1)] / 1000;
        (name, cnt, total, p50, p90)
    }).collect();
    table.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    println!("\n=== 聚合(p50/p90 单位 µs,total 单位 ms——单一时钟 cputicks) ===");
    println!("{:<22} {:>6} {:>10} {:>9} {:>9}", "name", "cnt", "total_ms", "p50_us", "p90_us");
    for (name, cnt, total, p50, p90) in &table {
        println!("{name:<22} {cnt:>6} {total:>10.2} {p50:>9} {p90:>9}");
    }
}

/// [11] 守卫等价判官基线:现状行为 = 会话生产路径(写线程建槽+首读裁决)
/// 的 to_ast_object_mode 字节 == 同文本无槽冷路径字节。
/// 同一判官重跑 = 发布即定型 vs 首读裁决等价。
fn probe_guard_equiv_baseline() {
    println!("\n=== [11] 守卫等价判官基线(现状首读裁决行为,阶段 B 后同判官重跑) ===");
    use mdio_core::session::MdioSession;
    let cases: &[(&str, &str)] = &[
        ("plain", "# S\n\nhello world\n"),
        ("code", "# S\n\n```rust\nlet x = 1;\n```\n"),
        ("table", "# S\n\n| a | b |\n|---|---|\n| c | d |\n"),
        ("setext", "# S\n\nfoo\n===\n"),
    ];
    for &(name, doc) in cases {
        let sess = MdioSession::new(1, doc, 1, "k".to_string(), None, None, None, Arc::new(dashmap::DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
        // 先整文写入(结构)，再纯内容编辑——守卫(首读裁决)在 to_ast_object_mode 时触发
        sess.execute_transaction(None, true, |ctx| {
            let _ = ctx.apply_mdio_op(vec![0..doc.len()], "/>", doc);
        });
        let pos = doc.find("x = 1").or_else(|| doc.find("hello")).or_else(|| doc.find("| c")).unwrap_or(0);
        let end = pos + 5;
        sess.execute_transaction(None, true, |ctx| {
            let _ = ctx.apply_mdio_op(vec![pos..end], "r>", "X = 1");
        });
        let snap = sess.publisher.load();
        let cached = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
        let text_now = snap.rope.to_string();
        let cold = {
            let mut e2 = MdioEngine::new(&text_now);
            e2.make_snapshot()
        }.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
        let cv: serde_json::Value = serde_json::from_str(&cached).unwrap();
        let ov: serde_json::Value = serde_json::from_str(&cold).unwrap();
        let eq = cv["ch"] == ov["ch"];
        println!("  {name:>6}: 会话路径 == 无槽冷路径: {eq}");
        assert!(eq, "guard baseline must hold: {name}");
    }
}

/// [12] 任务锚存在性基线:生产 path_tx 的 IncrTask 带新侧 slot 锚(现状)。
/// DetectTask 现状无锚字段——补锚的基线证据。
fn probe_task_anchor_baseline() {
    println!("\n=== [12] 任务锚存在性基线(IncrTask.slot / DetectTask 现状) ===");
    use mdio_core::session::MdioSession;
    let (ptx, prx) = crossbeam::channel::unbounded();
    let (dtx, drx) = crossbeam::channel::unbounded();
    let hook_docs = Arc::new(dashmap::DashSet::new());
    hook_docs.insert(1u32);
    let sess = MdioSession::new(1, "", 1, "k".to_string(), None, Some(dtx), Some(ptx), hook_docs, Arc::new(mdio_core::session::DetectClock::new()));
    let doc = "# S\n\n- a\n\nbody\n";
    sess.execute_transaction(None, false, |ctx| {
        let _ = ctx.apply_mdio_op(vec![0..0], "/>", doc);
    });
    let task = prx.try_recv().expect("IncrTask queued");
    println!(
        "  IncrTask.slot.is_some() = {}(新侧锚,现状已带——阶段 B 需加旧侧锚)",
        task.slot.is_some()
    );
    match drx.try_recv() {
        Ok(mdio_core::store::DetectTask::SelectorWrite { .. })
        | Ok(mdio_core::store::DetectTask::CrdtWrite { .. }) => {
            println!("  DetectTask 收到(现状无 slot 字段——阶段 B 补锚的基线)");
        }
        _ => println!("  DetectTask 未收到(注册路径或 hook_docs 门控)"),
    }
}

/// [13] 兜底正确性(慢消费退化行为面):无槽快照 select_nodes == 会话生产快照 select_nodes。
fn probe_fallback_equiv() {
    println!("\n=== [13] 兜底正确性:无槽 vs 会话生产快照 select_nodes 一致 ===");
    let mut doc = String::new();
    for s in 0..20 {
        doc.push_str(&format!("# Section {s}\n\n"));
        for i in 0..50 {
            doc.push_str(&format!("- item {s}-{i}: v\n"));
        }
        doc.push('\n');
    }
    let mut eng = MdioEngine::new(&doc);
    let snap = eng.make_snapshot();
    let insert_at = doc.find("- item 5-0").unwrap();
    eng.edit(insert_at, insert_at, "> quote new\n");
    let new_snap = eng.make_snapshot();
    let no_slot = {
        let mut e2 = MdioEngine::new(&new_snap.rope.to_string());
        e2.make_snapshot()
    };
    let steps = |s: usize| vec![
        mdio_core::selector::Step::Drill(format!("Section {s}")),
        mdio_core::selector::Step::Wildcard,
    ];
    let mut mismatches = 0;
    for s in 0..20 {
        let a = mdio_core::selector::select_nodes(&new_snap, &steps(s));
        let b = mdio_core::selector::select_nodes(&no_slot, &steps(s));
        if a.len() != b.len() { mismatches += 1; }
    }
    println!("  20 节 Drill 链:无槽 vs 快照节点数不一致 = {mismatches}(必须 0)");
}

fn main() {
    let sess = Arc::new(TelemetryInner::from_env("cell_probe_828".to_string(), TelemetryMode::Bench));
    let _guard = set_session(&sess);
    probe_tree_and_guard_cost();
    probe_p2_components();
    probe_hook_row_scope_cost();
    drop(_guard);
    // 计数面(判定差异,非时长)
    probe_hook_gate();
    probe_selector_gate();
    probe_guard_quadrants();
    probe_guard_equiv_baseline();
    probe_task_anchor_baseline();
    probe_fallback_equiv();
    // 导出落盘 + 聚合
    let json = export_phase_json(&sess);
    let dir = std::path::Path::new(".telemetry/cell_probe_828");
    let _ = std::fs::create_dir_all(dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let path = dir.join(format!("export_{ts}.json"));
    std::fs::write(&path, &json).expect("write export json");
    println!("\n落盘: {}", path.display());
    aggregate_and_print(&json);
}
