// 原子组装探针——对"切缝诊断"的支持/反对两侧做实测钉证。
//
// A(支持): 编辑后 tail 记录/块的 id 偏移 Δ 为常数(事件基承重声明:
//    "tail 事件同事件同折叠 ⟹ tail 记录序不变,id=旧id+Δ")。
// B(修正): zone 段的 BlockRec.container_ref 是 zone 局部 id——复用进全量
//    必须 +p 重定基(p=前缀记录数);同时证明 +p 常数足够(不需 HashMap)。
// C(修正): 6MB 全量 fold 的分段计时(l1 records / l2b view / l2a blocks)——
//    records+view refold 的真实成本与"blocks 是唯一读文本层"的成本定位。

use super::*;
use crate::block::entry_line;
use std::time::Instant;

fn next_rand(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn tokens() -> &'static [&'static str] {
    &[
        "plain paragraph\n", "some **bold** text\n", "# Heading\n", "## Sub\n",
        "- item\n", "- [ ] task\n", "1. first\n", "  - nested\n", "> quote\n",
        "> > nested\n", "```rust\nlet x = 1;\n```\n", "~~~\ncode\n~~~\n",
        "| a | b |\n|---|---|\n| c | d |\n", "[ref]: /url\n", "\n",
        "    indented code\n", "\tcode\n", "foo\n===\n", "foo\n---\n",
        "> foo\n> ===\n", "- a\n\n  cont\n",
    ]
}

fn random_edit(rng: &mut u64, doc: &str) -> (usize, usize, String) {
    let toks = tokens();
    let len = doc.len();
    if len == 0 {
        return (0, 0, toks[next_rand(rng) as usize % toks.len()].to_string());
    }
    match next_rand(rng) % 3 {
        0 => {
            let pos = crate::projection::snap_down_char_boundary(
                &crop::Rope::from(doc), next_rand(rng) as usize % (len + 1));
            (pos, pos, toks[next_rand(rng) as usize % toks.len()].to_string())
        }
        1 => {
            let mut s = crate::projection::snap_down_char_boundary(
                &crop::Rope::from(doc), next_rand(rng) as usize % len);
            let mut e = (s + 1 + next_rand(rng) as usize % (len - s)).min(len);
            e = crate::projection::snap_down_char_boundary(&crop::Rope::from(doc), e).max(s);
            (s, e, String::new())
        }
        _ => {
            let mut s = crate::projection::snap_down_char_boundary(
                &crop::Rope::from(doc), next_rand(rng) as usize % len);
            let mut e = (s + 1 + next_rand(rng) as usize % (len - s)).min(len);
            e = crate::projection::snap_down_char_boundary(&crop::Rope::from(doc), e).max(s);
            (s, e, toks[next_rand(rng) as usize % toks.len()].to_string())
        }
    }
}

/// A: tail 记录/块的 id 偏移 Δ 为常数(2000 随机例)。
#[test]
fn tail_id_offset_is_constant() {
    let mut rng: u64 = 0xA11C_E55F_0000_0001;
    let mut checked = 0usize;
    let mut with_tail = 0usize;
    for _case in 0..2000u64 {
        let mut doc = String::new();
        let line_count = (next_rand(&mut rng) % 10) as usize + 2;
        for _ in 0..line_count {
            doc.push_str(tokens()[next_rand(&mut rng) as usize % tokens().len()]);
        }
        let mut st = State::new(&doc);
        let old_snap = st.snapshot();
        let (s, e, text) = random_edit(&mut rng, &doc);
        st.edit(s, e, &text);
        let new_snap = st.snapshot();
        let Some(diff) = st.ripple_diff.take() else { continue };
        let (rls, rle, delta) = (diff.ripple_line_start, diff.ripple_line_end, diff.delta);

        let n_old = old_snap.line_len();
        let n_new = new_snap.line_len();
        let old_full = crate::infer::build_structure_parts(&old_snap.frame_log.entries, &old_snap.lines, &old_snap.open_depth, &old_snap.rope, (0, n_old as u32));
        let new_full = crate::infer::build_structure_parts(&new_snap.frame_log.entries, &new_snap.lines, &new_snap.open_depth, &new_snap.rope, (0, n_new as u32));

        // 旧侧 tail 起点 = rle 的旧侧映射(rle 是 conv 行;tail 从 conv+1 起)
        let conv_old = ((rle as isize - delta).max(0) as usize).min(n_old.saturating_sub(1));
        let tail_old_line = (conv_old + 1).min(n_old);
        let tail_new_line = (rle + 1).min(n_new);

        // 记录配对(按序——记录创建序 = push 行序;重复结构按序对齐,不靠
        // (kind,start) 命中——case 217 教训:重复结构下精确配对会错配)。
        let new_tail: Vec<_> = new_full.0.iter().enumerate()
            .filter(|(_, r)| (r.start as usize) >= tail_new_line)
            .collect();
        let old_tail: Vec<_> = old_full.0.iter().enumerate()
            .filter(|(_, r)| (r.start as usize) >= tail_old_line)
            .collect();
        if !new_tail.is_empty() || !old_tail.is_empty() {
            with_tail += 1;
            assert_eq!(
                new_tail.len(), old_tail.len(),
                "tail 记录数不同(增删恒等应同数): new={} old={} doc={doc:?} rls={rls} rle={rle} δ={delta}",
                new_tail.len(), old_tail.len()
            );
            let mut deltas: Vec<i64> = Vec::new();
            for ((nid, nr), (oid, or)) in new_tail.iter().zip(old_tail.iter()) {
                // 同形判定:kind/indent/anchor/loose 逐值;行字段平移 δ
                assert_eq!(nr.frame.kind(), or.frame.kind(), "tail 记录 kind 不同 doc={doc:?}");
                assert_eq!(nr.frame.indent(), or.frame.indent(), "tail indent 不同");
                assert_eq!(nr.anchor, or.anchor, "tail anchor 不同");
                assert_eq!(nr.frame.loose(), or.frame.loose(), "tail loose 位不同");
                assert_eq!(
                    nr.start as isize - delta, or.start as isize,
                    "tail start 不平移: doc={doc:?}"
                );
                let end_ok = match (nr.end, or.end) {
                    (Some(ne), Some(oe)) => ne.get() as isize == oe.get() as isize + delta,
                    (None, None) => true,
                    _ => false,
                };
                assert!(end_ok, "tail 记录 end 不平移: doc={doc:?}");
                deltas.push(*nid as i64 - *oid as i64);
            }
            let d0 = deltas[0];
            assert!(
                deltas.iter().all(|&d| d == d0),
                "tail 记录 id 偏移非常数: deltas={:?} doc={doc:?} rls={rls} rle={rle} δ={delta}",
                &deltas[..deltas.len().min(8)]
            );
        }
        checked += 1;
    }
    assert!(checked >= 1500, "样本不足: {checked}");
    eprintln!("[A] checked={checked} with_tail={with_tail} —— tail id 偏移 Δ 恒为常数");
}

/// B: zone 段 container_ref 是 zone 局部 id;复用进全量 = +p 常数重定基。
#[test]
fn zone_block_container_ref_needs_const_rebase() {
    // 容器语料:BQ 嵌套 + 列表,确保 zone 内有记录。
    let doc = "> a\n> - x\n> - y\n\n- p\n- q\n\n# h\n\ntail\n";
    let mut st = State::new(doc);
    let snap = st.snapshot();
    let n = snap.line_len();
    let full = crate::infer::build_structure_parts(&snap.frame_log.entries, &snap.lines, &snap.open_depth, &snap.rope, (0, n as u32));

    // 取一个 depth-0 的 zone:从列表容器起点(前有 BQ 记录——保证 p>0)。
    let zs = 4usize; // "- p" 行(BQ 区已关闭,depth-0)
    let ce = n;
    let zone = crate::infer::build_structure_parts(&snap.frame_log.entries, &snap.lines, &snap.open_depth, &snap.rope, (zs as u32, ce as u32));

    // p = 全量记录中创建序在 zs 之前的记录数(zone 从 depth-0 起 → zone 记录
    // 与全量的 [zs, ce) 段记录同序)
    let p = full.0.iter().filter(|r| (r.start as usize) < zs).count();

    // 对每一块(zone 与全量共有),验证 container_ref 相差恰好 p
    let mut checked = 0usize;
    for zb in &zone.1 {
        let fb = full.1.iter().find(|b| {
            b.start == zb.start && b.end == zb.end && b.block_type == zb.block_type
        });
        if let Some(fb) = fb {
            if zb.container_ref != u32::MAX && fb.container_ref != u32::MAX {
                assert_eq!(
                    fb.container_ref as i64 - zb.container_ref as i64,
                    p as i64,
                    "container_ref 差 ≠ p: 块 {:?} full={} zone={} p={p}",
                    zb.block_type, fb.container_ref, zb.container_ref
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "语料未产生可比对的容器块");
    assert!(p > 0, "zone 前必须有记录(p=0 则测试平凡)");
    eprintln!("[B] blocks checked={checked} p={p} —— zone 局部 id + 常数重定基成立(无需 HashMap)");
}

/// C: 6MB 级文档的分段计时(l1 records / l2b view / l2a blocks)。
#[test]
fn full_fold_phase_cost_split() {
    // 混合块大文档:~29 万行
    let unit = "# h\n\npara line\n\n- a\n- b\n\n> q\n\n```\ncode\n```\n\n";
    let reps = 20_000;
    let mut doc = String::with_capacity(unit.len() * reps);
    for _ in 0..reps {
        doc.push_str(unit);
    }
    let mut st = State::new(&doc);
    let snap = st.snapshot();
    let n = snap.line_len();
    let entries = &snap.frame_log.entries;
    let lines = &snap.lines;
    let rope = &snap.rope;
    eprintln!("[C] doc lines={n} entries={}", entries.len());

    // l1: RecordFold
    let mut best1 = f64::MAX;
    for _ in 0..3 {
        let t0 = Instant::now();
        let mut fold = crate::infer::RecordFold::new(entries, lines);
        crate::infer::walk_zone(entries, (0, n as u32), &mut fold);
        let (recs, _arena) = fold.finish();
        best1 = best1.min(t0.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&recs);
    }
    let (recs, arena) = {
        let mut fold = crate::infer::RecordFold::new(entries, lines);
        crate::infer::walk_zone(entries, (0, n as u32), &mut fold);
        fold.finish()
    };
    // l2b: view arrays
    let mut best2 = f64::MAX;
    for _ in 0..3 {
        let t0 = Instant::now();
        let va = crate::infer::build_view_arrays(&recs, &arena, entries, (0, n as u32));
        best2 = best2.min(t0.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&va);
    }
    let va = crate::infer::build_view_arrays(&recs, &arena, entries, (0, n as u32));
    let tc = crate::infer::table_confirmed_flags(entries, (0, n as u32));
    // l2a: block records(唯一读文本层)
    let mut best3 = f64::MAX;
    for _ in 0..3 {
        let t0 = Instant::now();
        let blocks = crate::infer::build_block_records_with(
            entries, lines, &snap.open_depth, rope, &recs, &arena, (0, n as u32), &va, &tc);
        best3 = best3.min(t0.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&blocks);
    }
    eprintln!(
        "[C] l1(records)={best1:.2}ms l2b(view)={best2:.2}ms l2a(blocks)={best3:.2}ms \
         —— records+view refold 成本 = {:.2}ms",
        best1 + best2
    );
}
