// D6 测试分类（当前状态——D6⑤d 桥拆除后）：
//   升级保障 23 条【已随 D6⑤d 删除】——对照旧 infer/旧机械的逐值等价
//     （record_fold_equals_build_container_records / end_for_rule_equals_legacy_span_end /
//      line_view_equals_row_view / block_fold_equals_build_block_records /
//      subtree_and_depth0_equivalence / section_intervals_equivalence /
//      locate_block_equals_block_at / span_of_regions / view_arrays_depth_and_chain /
//      tree_fold_equals_build_attribution_tree / node_face_equals_snapshot_read_side /
//      zone_query_equivalence / frame_push_queries_equivalence /
//      expand_zone_containers_equivalence / is_poppush_equals_legacy_pair /
//      is_poppush_kindonly_equals_legacy / perf_gate_new_vs_old(#[ignore]) 等）
//   真测试 17 条【常驻】——机械契约(walk_equals_forward_replay/partition_contract/
//     zone_edge_cases/zone_continuation_property/walk_lines_mechanics/
//     walk_lines_empty_lines/generated_corpus_equivalence) + 表语义直读
//     (end_for_rule_spec_semantics/reclassify_branch_spec/
//     view_arrays_naive_projections) + 双形态互证(depth_prefix_equivalence/
//     three_way_view_guard) + 审计三件套(frame_kind_enum_audit/
//     table_column_set_audit/reclassify_rewrite_arms_audit) +
//     parent_chain_sanity + ci_axis_differs_from_parent_relative_index +
//     restore_build_scan_closure(转正总验收)
    // 对照协议（inferview_plan_3 的三个对照层）：
    //   ① 机械等价：旧 infer / block 逐值断言（当前阶段）
    //   ② 规范等价：sdl spec 求值对照（reclassify 建起后启用）
    //   ③ 闭环比对：trilogy restore 往返恒等（树/记录建起后的转正总验收）
    // 基准约定：FrameSpans 列视图不作为对照基准；
    //   行级事实的基准 = build_view_arrays（生产查询面）与 RowView 17 原语。

    use super::*;
    use crate::block::{BlockFrame, FrameKind};

    fn snap_of(md: &str) -> crate::projection::Snapshot {
        let mut st = crate::projection::State::new(md);
        st.snapshot()
    }

    /// 语料：覆盖列表/BQ/表格/fence/复合开行/约束关闭/refdef/comment。
    fn corpus() -> Vec<&'static str> {
        vec![
            "# H1\n\npara with *em*\n",
            "- a\n- b\n  - c\n\n- d\n",
            "> quote\n> ```\n> code\n> ```\n> tail\n",
            "| a | b |\n|---|---|\n| 1 | 2 |\n",
            "1. one\n2. two\n\n   para\n",
            "> - x\n> - y\nplain\n",
            "```rust\nfn main(){}\n```\n",
            "> ```\n> code\nplain text\n",
            "- a\n\n  [ref]: /url\n\n- d\n",
            "<!-- c -->\ncomment body\n--> \n",
        ]
    }

    /// 边界语料：空/纯空行/单行/裸 marker/空容器。
    fn corpus_edge() -> Vec<&'static str> {
        vec![
            "",
            "\n",
            "\n\n\n",
            "single line",
            "> ",
            "- ",
            "```\n```",
            ">\n> x",
            "| a |\n",
        ]
    }

    /// 确定性生成语料（LCG——可复现的随机 markdown 形状）。
    fn gen_corpus(seed: u64, count: usize) -> Vec<String> {
        let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut rnd = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as u64
        };
        (0..count).map(|_| {
            let nlines = 1 + (rnd() % 40) as usize;
            let mut s = String::new();
            let mut list_depth = 0usize;
            let mut in_fence = false;
            for _ in 0..nlines {
                let r = rnd() % 22;
                if in_fence {
                    s.push_str("code line\n");
                    if r < 3 { in_fence = false; s.push_str("```\n"); }
                    continue;
                }
                if list_depth > 0 && r < 4 { list_depth -= 1; }
                match r {
                    0..=2 => s.push_str("# Heading\n"),
                    3..=5 => s.push_str("paragraph text\n"),
                    6..=9 => {
                        let ind = "  ".repeat(list_depth);
                        s.push_str(&format!("{ind}- item\n"));
                        if rnd() % 3 == 0 { list_depth += 1; }
                    }
                    10..=11 => {
                        let ind = "  ".repeat(list_depth);
                        s.push_str(&format!("{ind}1. num\n"));
                    }
                    12 => s.push_str("> quote\n"),
                    13..=14 => {
                        if in_fence { in_fence = false; s.push_str("```\n"); }
                        else { s.push_str("```\n"); in_fence = true; }
                    }
                    15 => s.push_str("| a | b |\n|---|---|\n"),
                    16 => s.push_str("\n"),
                    17 => s.push_str("***\n"),
                    18 => s.push_str("[r]: /url \"t\"\n"),
                    19 => s.push_str("<!-- c -->\nbody\n-->\n"),
                    20 => s.push_str("<div>\nhtml\n</div>\n"),
                    21 => {
                        let ind = "  ".repeat(list_depth);
                        s.push_str(&format!("{ind}> - nested\n"));
                    }
                    _ => s.push_str("more paragraph\n"),
                }
            }
            if in_fence { s.push_str("```\n"); }
            s
        }).collect()
    }

    /// 机械等价①：ReplayFold == forward_replay（全量 + 全部 zone 组合）。
    #[test]
    fn walk_equals_forward_replay() {
        for md in corpus() {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let mut expect = Vec::new();
            crate::block::forward_replay(entries, &mut expect);
            let mut got = ReplayFold::default();
            walk_entries(entries, &mut got);
            assert_eq!(got.stack, expect, "full walk: {md:?}");
            let n = snap.line_len() as u32;
            for start in 0..=n {
                for end in start..=n {
                    let mut expect2 = Vec::new();
                    let p = partition(entries, start);
                    let e2 = partition(entries, end);
                    crate::block::forward_replay(&entries[..p], &mut expect2);
                    crate::block::forward_replay(&entries[p..e2], &mut expect2);
                    let mut got2 = ReplayFold::default();
                    walk_zone(entries, (start, end), &mut got2);
                    assert_eq!(got2.stack, expect2, "zone ({start},{end}): {md:?}");
                }
            }
        }
    }

    /// 地基性质①：partition 契约 == 朴素计数（含越界行号）。
    #[test]
    fn partition_contract() {
        for md in corpus().iter().chain(corpus_edge().iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            for line in 0..=(n + 2) {
                let naive = entries.iter()
                    .filter(|&&e| crate::block::entry_line(e) < line)
                    .count();
                assert_eq!(partition(entries, line), naive, "line {line}: {md:?}");
            }
        }
    }

    /// 地基性质②：walk_zone((0,n)) == walk_entries；逆序 zone 空产出；越界 zone。
    #[test]
    fn zone_edge_cases() {
        for md in corpus_edge() {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            let mut f = ReplayFold::default();
            walk_zone(entries, (0, n), &mut f);
            let mut g = ReplayFold::default();
            walk_entries(entries, &mut g);
            assert_eq!(f.stack, g.stack, "full-zone: {md:?}");
            let mut r = ReplayFold::default();
            walk_zone(entries, (3, 1), &mut r);
            assert_eq!(r.stack, g.stack, "reversed zone: {md:?}");
            let mut o = ReplayFold::default();
            walk_zone(entries, (0, n + 5), &mut o);
            assert_eq!(o.stack, g.stack, "overrun zone: {md:?}");
        }
    }

    /// 地基性质③：续走律——(0,b) 后补走 [b,c) 段 == 一次走 (0,c)。
    #[test]
    fn zone_continuation_property() {
        for md in corpus().iter().chain(corpus_edge().iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            for b in 0..=n {
                for c in b..=n {
                    let mut once = ReplayFold::default();
                    walk_zone(entries, (0, c), &mut once);
                    let mut chained = ReplayFold::default();
                    walk_zone(entries, (0, b), &mut chained);
                    let pb = partition(entries, b);
                    let pc = partition(entries, c);
                    walk_entries(&entries[pb..pc], &mut chained);
                    assert_eq!(chained.stack, once.stack, "chain ({b},{c}): {md:?}");
                }
            }
        }
    }

    /// 机械契约：per_line 每行恰一次、行事件之后、栈与手工游标 oracle 一致。
    #[test]
    fn walk_lines_mechanics() {
        for md in corpus().iter().chain(corpus_edge().iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            for start in 0..=n {
                for end in start..=n {
                    let mut expect: Vec<(u32, Vec<BlockFrame>)> = Vec::new();
                    let mut stack: Vec<BlockFrame> = Vec::new();
                    let p0 = partition(entries, start);
                    crate::block::forward_replay(&entries[..p0], &mut stack);
                    let mut ei = p0;
                    for li in start..end {
                        let le = partition(entries, li + 1);
                        crate::block::forward_replay(&entries[ei..le], &mut stack);
                        ei = le;
                        expect.push((li, stack.clone()));
                    }
                    let mut fold = ReplayFold::default();
                    let mut prev = PrevSlot::default();
                    let mut got: Vec<(u32, Vec<BlockFrame>)> = Vec::new();
                    walk_lines(entries, &snap.lines, (start, end), &mut prev,
                        &mut fold, |f, li, _t| got.push((li as u32, f.stack.clone())));
                    assert_eq!(got, expect, "walk_lines ({start},{end}): {md:?}");
                }
            }
        }
    }

    /// 机械等价②：确定性生成语料（全量等价 + 抽样 zone 等价）。
    #[test]
    fn generated_corpus_equivalence() {
        for md in gen_corpus(0xC0FFEE, 300) {
            let snap = snap_of(&md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            let mut expect = Vec::new();
            crate::block::forward_replay(entries, &mut expect);
            let mut got = ReplayFold::default();
            walk_entries(entries, &mut got);
            assert_eq!(got.stack, expect, "gen full: {md:?}");
            let mut state = 0xDEADBEEF_u64.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            for _ in 0..30 {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let a = ((state >> 33) % (n as u64 + 1)) as u32;
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                let b = a + (((state >> 33) % (n as u64 + 1 - a as u64)) as u32);
                let mut expect2 = Vec::new();
                let p = partition(entries, a);
                let e2 = partition(entries, b);
                crate::block::forward_replay(&entries[..p], &mut expect2);
                crate::block::forward_replay(&entries[p..e2], &mut expect2);
                let mut got2 = ReplayFold::default();
                walk_zone(entries, (a, b), &mut got2);
                assert_eq!(got2.stack, expect2, "gen zone ({a},{b}): {md:?}");
            }
        }
    }

    fn all_kinds() -> [FrameKind; 10] {
        use FrameKind::*;
        [BlockQuote, BulletList, OrderedList, CodeFence, HtmlBlock,
            Table, Comment, HtmlElement, RefDef, TableRow]
    }





    /// P1 结算原子②：表语义直读（独立于实现——防双漂移）。
    #[test]
    fn end_for_rule_spec_semantics() {
        for kind in all_kinds() {
            for pop_pl in 0..27u8 {
                let bt: BlockType = unsafe { std::mem::transmute(pop_pl) };
                for pop_line in [0usize, 1, 2, 7, 100] {
                    let got = end_for_rule(kind, pop_line, bt);
                    match crate::effect::effect_for(kind).closing_pl {
                        crate::effect::ClosingPl::NoClose => {
                            assert_eq!(got, pop_line, "{kind:?} NoClose @ {pop_line}")
                        }
                        crate::effect::ClosingPl::Specific(pl) => {
                            assert_eq!(
                                got,
                                pop_line + (bt == pl) as usize,
                                "{kind:?} Specific({pl:?}) vs {bt:?} @ {pop_line}"
                            )
                        }
                    }
                }
            }
        }
    }

    /// 8 列构建器：RecordFold + build_view_arrays 的便捷组合。
    fn view_arrays_of(
        entries: &[u64],
        lines: &[crate::block::PackedLineInfo],
        zone: (u32, u32),
    ) -> ViewArrays {
        let mut fold = RecordFold::new(entries, lines);
        walk_zone(entries, zone, &mut fold);
        let (recs, arena) = fold.finish();
        build_view_arrays(&recs, &arena, entries, zone)
    }

    /// P3 性质①：五列 == 对记录集的朴素投影（独立实现形状）。
    #[test]
    fn view_arrays_naive_projections() {
        for md in corpus().iter().chain(corpus_edge().iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            for start in 0..=n {
                for end in start..=n {
                    let mut fold = RecordFold::new(entries, &snap.lines);
                    walk_zone(entries, (start, end), &mut fold);
                    let (recs, arena) = fold.finish();
                    let va = build_view_arrays(&recs, &arena, entries, (start, end));
                    let (zs, ze) = (start as usize, end as usize);
                    // 朴素投影（独立形状）
                    let mut pm: Vec<Vec<u32>> = vec![Vec::new(); ze - zs];
                    let mut bm: Vec<Vec<FrameKind>> = vec![Vec::new(); ze - zs];
                    let mut bd = vec![false; ze - zs];
                    let mut ch = vec![false; ze - zs];
                    let mut st: Vec<Vec<u32>> = vec![Vec::new(); ze - zs];
                    for (rid, r) in recs.iter().enumerate() {
                        for l in r.push_lines_of(&arena) {
                            let l = l as usize;
                            if l >= zs && l < ze {
                                pm[l - zs].push(rid as u32);
                            }
                        }
                        for l in r.body_mut_lines_of(&arena) {
                            let l = l as usize;
                            if l >= zs && l < ze {
                                bm[l - zs].push(r.frame.kind());
                            }
                        }
                        for ev in r.pop_push_events_of(&arena) {
                            let l = ev.line as usize;
                            if l >= zs && l < ze {
                                if ev.is_change {
                                    ch[l - zs] = true;
                                } else {
                                    bd[l - zs] = true;
                                }
                            }
                        }
                        let s = r.start as usize;
                        if s >= zs && s < ze {
                            st[s - zs].push(rid as u32);
                        }
                    }
                    for l in 0..(ze - zs) {
                        assert_eq!(va.push_row(l), pm[l].as_slice(),
                            "push_map[{l}] ({start},{end}): {md:?}");
                        assert_eq!(va.body_mut_row(l), bm[l].as_slice(),
                            "bm[{l}] ({start},{end}): {md:?}");
                        assert_eq!(va.start_row(l), st[l].as_slice(),
                            "st[{l}] ({start},{end}): {md:?}");
                    }
                    let bd_v2: Vec<bool> = (0..(ze - zs)).map(|l| va.boundary_at(l)).collect();
                    let ch_v2: Vec<bool> = (0..(ze - zs)).map(|l| va.change_at(l)).collect();
                    assert_eq!(bd_v2, bd, "bd ({start},{end}): {md:?}");
                    assert_eq!(ch_v2, ch, "ch ({start},{end}): {md:?}");
                }
            }
        }
    }



    /// 逐行驱动 BuildView（块遍同构——栈/游标/行内计数），返回第 line 行的视图。
    fn build_view_at<'a>(
        recs: &'a [ContainerRec],
        entries: &'a [u64],
        lines: &'a [crate::block::PackedLineInfo],
        va: &'a ViewArrays,
        tc: &'a [bool],
        zone: (u32, u32),
        line: u32,
        stack: &mut Vec<u32>,
        ei: &mut usize,
        last_rel: &mut usize,
        line_count: &mut usize,
    ) -> BuildView<'a> {
        let zs = zone.0 as usize;
        while *ei < entries.len() && entry_line(entries[*ei]) as usize <= line as usize {
            let e = entries[*ei];
            if entry_is_body_mut(e) {
                *ei += 1;
                continue;
            }
            if entry_is_pop(e) {
                stack.pop();
            } else {
                let rel = entry_line(e) as usize - zs;
                if rel != *last_rel {
                    *last_rel = rel;
                    *line_count = 0;
                }
                let lo = va.push_offsets[rel] as usize;
                stack.push(va.push_values[lo + *line_count]);
                *line_count += 1;
            }
            *ei += 1;
        }
        BuildView {
            records: recs,
            entries,
            lines,
            push_offsets: &va.push_offsets,
            push_values: &va.push_values,
            table_confirmed: tc,
            zs,
            ze: zone.1 as usize,
            stack: stack.clone(),
            ei: *ei,
            line: line as usize,
            // is_leaf 用 va 查表（与块遍 S4 同款——helper 原先写死 false 是
            // 伪形态——D3 三向守卫抓出：表头行 leaf=true 才能 end_for=None）
            leaf: crate::block::frame_kind(lines[line as usize].node_type()).map_or(false, |k| {
                !(va.push_row(line as usize - zs).iter()
                    .any(|&rid| recs[rid as usize].frame.kind() == k)
                    || va.body_mut_row(line as usize - zs).contains(&k))
            }),
        }
    }



    /// P4 规范语义：四路分派与表语义直读（独立于实现的手工锚）。
    #[test]
    fn reclassify_branch_spec() {
        // BQ 内容层：`> foo` → Paragraph；`> # H` → Header1（A0 分类）。
        // rd 撤销：`[x]:\n` 冒号后空 → Paragraph；`[x]: /u` → RefDef 保持。
        // 伪表：`|a| = b` 无确认分隔 → Paragraph；确认表行 → TableRow 保持。
        // #542：`[a\nb]: /u` 两行 → 第二行 Blank（跨行 rd 不产段落）。
        let docs = vec![
            "> foo\n",
            "> # H\n",
            "[x]:\n",
            "[x]: /u\n",
            "|a| = b\n",
            "| a |\n| - |\n| b |\n",
            "[a\nb]: /u\n",
        ];
        let expect: Vec<(usize, BlockType)> = vec![
            (0, BlockType::Paragraph),  // > foo
            (0, BlockType::Header1),    // > # H
            (0, BlockType::Paragraph),  // [x]: 冒号后空 → 撤销
            (0, BlockType::RefDef),     // [x]: /u → 保持
            (0, BlockType::Paragraph),  // 伪表 → 撤销
            (2, BlockType::TableRow),   // 确认表数据行 → 保持（行1是分隔行）
            (1, BlockType::Blank),      // #542 第二行 → Blank
        ];
        for (md, (line, want)) in docs.iter().zip(expect.iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            let mut fold = RecordFold::new(entries, &snap.lines);
            walk_zone(entries, (0, n), &mut fold);
            let (recs, arena) = fold.finish();
            let va = build_view_arrays(&recs, &arena, entries, (0, n));
            let tc = table_confirmed_flags(entries, (0, n));
            let mut stack: Vec<u32> = Vec::new();
            let mut ei = partition(entries, 0);
            let mut last_rel = usize::MAX;
            let mut line_count = 0usize;
            let mut prev_rt: Option<BlockType> = None;
            for i in 0..n {
                let view = build_view_at(
                    &recs, entries, &snap.lines, &va, &tc, (0, n),
                    i, &mut stack, &mut ei, &mut last_rel, &mut line_count,
                );
                let pl = snap.lines[i as usize].node_type();
                let rt = reclassify(pl, &snap.lines, &snap.rope, i as usize, view.zs, view.ze, Some(&view), prev_rt, None, None);
                if i == *line as u32 {
                    assert_eq!(rt, *want, "{md:?} line {i}");
                }
                prev_rt = Some(rt);
            }
        }
    }



    /// 地基补充：walk_lines 空 lines 显式契约（per_line 仍每行恰一次，lt=Paragraph）。
    #[test]
    fn walk_lines_empty_lines() {
        for md in corpus().iter().chain(corpus_edge().iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            for start in 0..=n {
                for end in start..=n {
                    let mut fold = ReplayFold::default();
                    let mut prev = PrevSlot::default();
                    let mut calls: Vec<(u32, BlockType)> = Vec::new();
                    walk_lines(entries, &[], (start, end), &mut prev, &mut fold, |_f, li, lt| {
                        calls.push((li as u32, lt));
                    });
                    // 每行恰一次 + GET 退化为 Paragraph + 区间外无回调。
                    assert_eq!(calls.len(), (end - start) as usize, "count ({start},{end}): {md:?}");
                    for (li, lt) in &calls {
                        assert!(li >= &start && li < &end, "range {li}: {md:?}");
                        assert_eq!(*lt, BlockType::Paragraph, "fallback {li}: {md:?}");
                    }
                    // 栈状态与手工 oracle 一致（前缀 + 区间段）。
                    let mut expect = Vec::new();
                    let p0 = partition(entries, start);
                    let p1 = partition(entries, end);
                    crate::block::forward_replay(&entries[..p0], &mut expect);
                    crate::block::forward_replay(&entries[p0..p1], &mut expect);
                    assert_eq!(fold.stack, expect, "stack ({start},{end}): {md:?}");
                }
            }
        }
    }



    /// P9 闭环比对：restore_pl(新树) == scan PL；
    /// restore_fl(新树) == scan 帧流（restore ∘ build == scan）。
    #[test]
    fn restore_build_scan_closure() {
        let mut docs: Vec<String> = corpus().iter().map(|s| s.to_string()).collect();
        docs.extend(corpus_edge().iter().map(|s| s.to_string()));
        docs.extend(gen_corpus(0x7E57, 300));
        docs.extend(gen_corpus(0xABCD1234, 300));
        for topo in [
            &crate::topology::CONTAINER_TOPOLOGY,
            &crate::topology::HEADINGSECTION_TOPOLOGY,
        ] {
            for md in &docs {
                let snap = snap_of(md);
                let entries = &snap.frame_log.entries;
                let n = snap.line_len() as u32;
                let mut fold = RecordFold::new(entries, &snap.lines);
                walk_zone(entries, (0, n), &mut fold);
                let (recs, arena) = fold.finish();
                let blocks = build_block_records(entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n));
                let lv = LineView::new(&recs, &blocks, entries, &snap.lines, &snap.rope, (0, n), &arena);
                let tree = build_attribution_tree(&recs, &blocks, &lv, &arena, (0, n), topo);
                // restore ∘ build == scan：PL 全等（CF + HS 双拓扑——HS 章节壳
                // 只重组 children，节点覆盖区间与 rec/blk 引用不变——预期恒等）。
                let pl = restore_pl(&tree, &blocks, &recs, &lv, &snap.rope, &snap.lines, n as usize);
                for i in 0..n as usize {
                    assert_eq!(pl[i], snap.lines[i].node_type(), "pl {i}: {} {md:?}", topo.name);
                }
                // restore ∘ build == scan：帧流重放等价（无时间箭头——
                // restore 产规范序（Pop 先/Push 次/BMut 恒末），不镜像 scan
                // 动作序——正逆同列律）：
                // ① 逐行事件多重集全等（kind × op 计数）；
                // ② 每行行尾栈 kind 序列全等（重放终态）。
                let fl = restore_fl(&tree, &recs, &blocks, &snap.lines, n as usize);
                let mut expect: Vec<(u32, FrameKind, u8)> = Vec::new();
                for &e in entries {
                    let k = crate::block::entry_frame(e).kind();
                    let op = if crate::block::entry_is_pop(e) {
                        1u8
                    } else if crate::block::entry_is_body_mut(e) {
                        2u8
                    } else {
                        0u8
                    };
                    expect.push((crate::block::entry_line(e), k, op));
                }
                let ms = |v: &[(u32, FrameKind, u8)]| -> std::collections::HashMap<(u32, u8, u8), usize> {
                    let mut m = std::collections::HashMap::new();
                    for &(l, k, op) in v {
                        *m.entry((l, k as u8, op)).or_insert(0) += 1;
                    }
                    m
                };
                assert_eq!(ms(&fl), ms(&expect), "fl 多重集: {} {md:?}", topo.name);
                let stacks = |v: &[(u32, FrameKind, u8)]| -> Vec<Vec<u8>> {
                    let mut out: Vec<Vec<u8>> = Vec::new();
                    let mut stack: Vec<FrameKind> = Vec::new();
                    let mut cur = 0u32;
                    for &(l, k, op) in v {
                        while cur < l {
                            out.push(stack.iter().map(|k| *k as u8).collect());
                            cur += 1;
                        }
                        match op {
                            0 => stack.push(k),
                            1 => { let _ = stack.pop(); }
                            _ => {}
                        }
                    }
                    while cur < n {
                        out.push(stack.iter().map(|k| *k as u8).collect());
                        cur += 1;
                    }
                    out
                };
                assert_eq!(stacks(&fl), stacks(&expect), "fl 行尾栈: {} {md:?}", topo.name);
            }
        }
    }



    /// C1 物化等价：depth_prefix == depth_before 全语料逐行（含越界）。
    #[test]
    fn depth_prefix_equivalence() {
        for md in corpus().iter().chain(corpus_edge().iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len();
            let col = depth_prefix(entries, n);
            for li in 0..n {
                assert_eq!(
                    col[li] as usize,
                    snap.open_depth_at(li),
                    "depth {li}: {md:?}"
                );
            }
            assert_eq!(col[n] as usize, snap.eof_open_depth(), "trailing depth: {md:?}");
        }
        for md in gen_corpus(0xD00D, 100) {
            let snap = snap_of(&md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len();
            let col = depth_prefix(entries, n);
            for li in 0..n {
                assert_eq!(
                    col[li] as usize,
                    snap.open_depth_at(li),
                    "gen depth {li}: {md:?}"
                );
            }
            assert_eq!(col[n] as usize, snap.eof_open_depth(), "gen trailing depth: {md:?}");
        }
    }

    /// L0 深度列对拍：段视图 `LineView.depth_at` 与快照列 `Snapshot.depth_at`
    /// 逐行一致（两个视角同源）。
    #[test]
    fn close_depth_equals_next_open_depth() {
        for md in corpus().iter().chain(corpus_edge().iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            let mut fold = RecordFold::new(entries, &snap.lines);
            walk_zone(entries, (0, n), &mut fold);
            let (recs, arena) = fold.finish();
            let blocks = build_block_records(entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n));
            let lv = LineView::new(&recs, &blocks, entries, &snap.lines, &snap.rope, (0, n), &arena);
            for li in 0..snap.line_len() {
                // LineView.depth_at = 该行事件后的链深；L0 列 = 行首（事件前）
                // 深度。二者满足“行尾深度 == 下一行行首深度”的进位关系。
                let want = if li + 1 == snap.line_len() {
                    snap.eof_open_depth()
                } else {
                    snap.open_depth_at(li + 1)
                };
                assert_eq!(
                    lv.close_depth_at(li),
                    want,
                    "depth carry mismatch @{li}: {md:?}"
                );
            }
        }
    }

    /// D3 三向守卫：depth_at / end_for 在三种访问形态一致——
    /// ① 物化（LineView 列直读）② 懒（BuildView 构建期影子栈）③ 全扫
    /// （手工游标 oracle——独立重放）。
    #[test]
    fn three_way_view_guard() {
        for md in corpus().iter().chain(corpus_edge().iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            let mut fold = RecordFold::new(entries, &snap.lines);
            walk_zone(entries, (0, n), &mut fold);
            let (recs, arena) = fold.finish();
            let blocks = build_block_records(entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n));
            let lv = LineView::new(&recs, &blocks, entries, &snap.lines, &snap.rope, (0, n), &arena);
            let va = build_view_arrays(&recs, &arena, entries, (0, n));
            let tc = table_confirmed_flags(entries, (0, n));
            let mut stack: Vec<u32> = Vec::new();
            let mut ei = partition(entries, 0);
            let mut last_rel = usize::MAX;
            let mut line_count = 0usize;
            for i in 0..n as usize {
                let bv = build_view_at(&recs, entries, &snap.lines, &va, &tc, (0, n),
                    i as u32, &mut stack, &mut ei, &mut last_rel, &mut line_count);
                // ③ 全扫 oracle：行 i 事件后栈深（独立重放——body_mut 不改栈结构）
                let mut oracle: Vec<BlockFrame> = Vec::new();
                let p = partition(entries, i as u32 + 1);
                for &e in &entries[..p] {
                    if entry_is_body_mut(e) {
                        continue;
                    } else if entry_is_pop(e) {
                        oracle.pop();
                    } else {
                        oracle.push(entry_frame(e));
                    }
                }
                // depth 三向（LineView = 行事件后深度列——与 oracle 同语义）
                assert_eq!(lv.close_depth_at(i), oracle.len(), "depth 物化 {i}: {md:?}");
                assert_eq!(bv.sim_open_depth_at(i), oracle.len(), "depth 懒 {i}: {md:?}");
                // end_for 三向（物化 vs 懒——oracle 经块记录直读对照）
                assert_eq!(lv.end_for(i), bv.end_for(i), "end_for {i}: {md:?}");
            }
        }
    }

    /// D2 审计①（§3.1 判据的长期守卫）：infer.rs 源文件内的 FrameKind
    /// 多枚举行 ⊆ 词法白名单（机械内零类型枚举——词法分派合法）。
    /// 新增多枚举行 → 本测试红 → 要么表化要么登记白名单（带分类注释）。
    #[test]
    fn frame_kind_enum_audit() {
        let src = include_str!("../../src/infer.rs"); // 相对单测文件——../../src 指回生产源
        let allowed: &[&str] = &[
            // 词法：容器前缀交替剥除（BQ/marker 邻接判定——peel 层序）
            "FrameKind::BulletList | FrameKind::OrderedList => {",
            // 词法：content_code 的 is_marker_line 判定
            "FrameKind::BulletList | FrameKind::OrderedList",
            // 审计基准自身：table_column_set_audit 的对照断言（审计测试的
            // matches! 枚举是表列的对照基准——合法）
            "let want_loose = matches!(fk, FrameKind::BulletList | FrameKind::OrderedList)",
            // section_intervals 的列表内标题判定（章节边界过滤——词法帧类）
            "crate::block::FrameKind::BulletList | crate::block::FrameKind::OrderedList)",
            // prev 语义边界的 item-open 形状判定（P5——旧 FL 帧类投影）
            "has_list_push |= matches!(k, FrameKind::BulletList | FrameKind::OrderedList);",
            // hs_scan_headings 的可含标题容器判定（自 incr 迁入的工具链——
            // 容器帧进入判定：BQ/BLT/OL 才进入内部扫标题）
            "FrameKind::BlockQuote | FrameKind::BulletList | FrameKind::OrderedList",
            // hs_scan_headings 的 in_list 标记（同迁入工具链——heading 的
            // 列表态判定——词法帧类）
            "matches!(fk, FrameKind::BulletList | FrameKind::OrderedList)",
            // get_node_end_from_lines 的叶子/帧分派（自 engine 迁入的工具链——
            // HtmlElement/TableRow 需查 Push 区分 leaf 与 frame）
            "if kind == crate::block::FrameKind::HtmlElement || kind == crate::block::FrameKind::TableRow {",
        ];
        for (i, line) in src.lines().enumerate() {
            // 排除字符串字面量行（白名单数组自身——审计只扫代码枚举）
            if line.matches("FrameKind::").count() >= 2 && !line.trim().starts_with('"') {
                let trimmed = line.trim();
                let ok = allowed.iter().any(|a| trimmed.starts_with(a));
                assert!(
                    ok,
                    "审计: 行 {} 的 FrameKind 多枚举行未登记白名单: {}",
                    i + 1,
                    trimmed
                );
            }
        }
    }

    /// D2 审计②（表列集合固化——防表列漂移）：has_loose == {BLT, OL}、
    /// has_pop_push == {BLT, OL, TableRow}、is_list_item == {Bullet,
    /// Checklist, OrderedListItem}——「该表化类」的历史枚举集合与表列同集。
    #[test]
    fn table_column_set_audit() {
        for k in 0..10u8 {
            let fk: FrameKind = unsafe { std::mem::transmute(k) };
            let want_loose = matches!(fk, FrameKind::BulletList | FrameKind::OrderedList);
            assert_eq!(crate::effect::decl_for(fk).has_loose, want_loose, "has_loose {fk:?}");
            let want_pp = matches!(
                fk,
                FrameKind::BulletList | FrameKind::OrderedList | FrameKind::TableRow
            );
            assert_eq!(crate::effect::decl_for(fk).has_pop_push, want_pp, "has_pop_push {fk:?}");
        }
        for t in 0..27u8 {
            let bt: BlockType = unsafe { std::mem::transmute(t) };
            let want_item = matches!(
                bt,
                BlockType::Bullet | BlockType::Checklist | BlockType::OrderedListItem
            );
            assert_eq!(
                crate::effect::block_decl(bt).is_list_item,
                want_item,
                "is_list_item {bt:?}"
            );
        }
    }

    /// 表化审计（§3.1 判据）：reclassify_rewrite 的守卫快照分派臂
    /// == BLOCK_TYPE_DECLS 中声明 context_fallback 的行集合。表加行
    /// （新后向确认类型）时本测试红，强制同步——快照默认禁止的自动化守卫。
    #[test]
    fn reclassify_rewrite_arms_audit() {
        let mut table_rows: Vec<BlockType> = Vec::new();
        for t in 0..27u8 {
            let bt: BlockType = unsafe { std::mem::transmute(t) };
            if crate::effect::block_decl(bt).context_fallback.is_some() {
                table_rows.push(bt);
            }
        }
        assert_eq!(table_rows, vec![BlockType::TableRow, BlockType::RefDef]);
    }

    /// P6 记录父链健全：chain[d] 的 parent 恒为 chain[d-1]。
    /// （ci 断言见 `ci_axis_differs_from_parent_relative_index`；
    /// ci 不是路径分量。）
    #[test]
    fn parent_chain_sanity() {
        for md in corpus().iter().chain(corpus_edge().iter()) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            let mut fold = RecordFold::new(entries, &snap.lines);
            walk_zone(entries, (0, n), &mut fold);
            let (recs, arena) = fold.finish();
            let blocks = build_block_records(entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n));
            let lv = LineView::new(&recs, &blocks, entries, &snap.lines, &snap.rope, (0, n), &arena);
            for li in 0..n {
                let chain = lv.chain_at(li as usize);
                for (d, &rid) in chain.iter().enumerate() {
                    if d > 0 {
                        assert_eq!(
                            recs[rid as usize].parent,
                            chain[d - 1],
                            "parent {li}.{d}: {md:?}"
                        );
                    }
                }
            }
        }
    }


    /// [对账判官·特征化] `ci` 轴 ≠ 路径分量轴。
    ///
    /// 事实：`ContainerRec.ci` 是**按深度的全局容器计数器**（infer.rs:702-703；
    /// 注释 infer.rs:259-260 原文"跨父容器共享——非父 children 序"）；而 patch
    /// path 的分量是**父内序号**（ast_path_of，incr.rs:3393-3396 用
    /// `children.position`）。两轴在"同一深度上多个父各带容器子"时分叉——
    /// 最小反例 `> - a\n\npara\n\n> - b\n` 的第二个 BQ 列表（深度 1）：
    /// ci=1 而父内序号=0。
    ///
    /// 本判官把"单链一致 / 分叉形状必分叉"钉成可执行事实：将来若把 ci 改成
    /// 父内序号（或给 fold 加种子让 zone 视图产出绝对序号），这里翻红即提醒
    /// 下游接线（path_to_root / align_container_path 的 ci 分量）需同步复核。
    #[test]
    fn ci_axis_differs_from_parent_relative_index() {
        fn divergences(md: &str) -> (usize, usize, Vec<String>) {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            let mut fold = RecordFold::new(entries, &snap.lines);
            walk_zone(entries, (0, n), &mut fold);
            let (recs, arena) = fold.finish();
            let blocks = build_block_records(
                entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n));
            let lv = LineView::new(&recs, &blocks, entries, &snap.lines, &snap.rope, (0, n), &arena);
            // 父内序号：根按记录创建序；其余按 parent.children 位置。
            let mut pos = vec![usize::MAX; recs.len()];
            let mut root_i = 0usize;
            for (i, r) in recs.iter().enumerate() {
                if r.parent == u32::MAX {
                    pos[i] = root_i;
                    root_i += 1;
                }
            }
            for p in recs.iter() {
                for (k, c) in p.children_of(&arena).enumerate() {
                    pos[c as usize] = k;
                }
            }
            let (mut total, mut diverge) = (0usize, 0usize);
            let mut samples = Vec::new();
            for li in 0..n as usize {
                let chain = lv.chain_at(li);
                for (d, &rid) in chain.iter().enumerate() {
                    let ci = lv.child_idx_at(li, d);
                    let want = pos[rid as usize];
                    total += 1;
                    if ci != want {
                        diverge += 1;
                        if samples.len() < 6 {
                            samples.push(format!(
                                "line={li} d={d} ci={ci} pos={want} rid={rid} parent={}",
                                recs[rid as usize].parent));
                        }
                    }
                }
            }
            (total, diverge, samples)
        }
        let (t_single, d_single, _) = divergences("> - a\n> - b\n");
        let (t_split, d_split, s) = divergences("> - a\n\npara\n\n> - b\n");

        assert_eq!(d_single, 0, "单链形状两轴应一致");
        assert!(d_split > 0, "同深度多父各带容器子：ci 轴与父内序号轴必须分叉");
    }

    /// 章节区间契约（深度感知语义）：容器/列表内的标题只截断同容器深度的
    /// 章节，不截断外层文档章节；只有同容器深度的 ≤ 级标题才截断。
    #[test]
    fn section_intervals_heading_truncation_contract() {
        let cases: [(&str, Vec<(usize, usize, u8, BlockType)>); 2] = [
            (
                "# S\n> # H4\n> x\n> ### B3\n",
                vec![(0, 4, 1, BlockType::Header1),
                     (1, 4, 1, BlockType::Header1),
                     (3, 4, 3, BlockType::Header3)],
            ),
            (
                "# S\n> # H4\n> x\n### B3\n",
                vec![(0, 4, 1, BlockType::Header1),
                     (1, 3, 1, BlockType::Header1),
                     (3, 4, 3, BlockType::Header3)],
            ),
        ];
        let heading = crate::topology::HEADINGSECTION_TOPOLOGY.heading_candidates;
        for (md, want) in cases {
            let snap = snap_of(md);
            let entries = &snap.frame_log.entries;
            let n = snap.line_len() as u32;
            let mut fold = RecordFold::new(entries, &snap.lines);
            walk_zone(entries, (0, n), &mut fold);
            let (recs, arena) = fold.finish();
            let blocks = build_block_records(entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n));
            let got = section_intervals(&blocks, &recs, &arena, heading, n as usize);
            assert_eq!(got, want, "secs: {md:?}");
        }
    }



// ── Phase A：build_structure_parts / with_structure 等价 ──

#[test]
fn build_structure_parts_equals_manual_pipeline() {
    let samples = [
        "",
        "# H\n",
        "- a\n- b\n",
        "> q\n\np\n",
        "| a |\n| - |\n| b |\n",
        "# S\n> # H4\n> x\n### B3\n",
    ];
    for md in samples {
        let snap = snap_of(md);
        let entries = &snap.frame_log.entries;
        let lines = &snap.lines;
        let rope = &snap.rope;
        let n = snap.line_len() as u32;
        let zone = (0, n);

        // 新组合原子
        let (recs_new, blocks_new, va_new, arena_new) = build_structure_parts(entries, lines, &snap.open_depth, rope, zone);
        let view_new = LineView::with_arrays(&recs_new, &blocks_new, entries, lines, rope, zone, va_new, &arena_new);

        // 旧手工三件套
        let mut fold = RecordFold::new(entries, lines);
        walk_zone(entries, zone, &mut fold);
        let (recs_old, arena_old) = fold.finish();
        let blocks_old = build_block_records(entries, lines, &snap.open_depth, rope, &recs_old, &arena_old, zone);
        let view_old = LineView::new(&recs_old, &blocks_old, entries, lines, rope, zone, &arena_old);

        assert!(crate::cache::records_structurally_eq(&recs_new, &arena_new, &recs_old, &arena_old),
            "records mismatch for {md:?}");
        assert_eq!(blocks_new, blocks_old, "blocks mismatch for {md:?}");

        let n = n as usize;
        for i in 0..n {
            assert_eq!(view_new.close_depth_at(i), view_old.close_depth_at(i), "depth {i} {md:?}");
            assert_eq!(view_new.resolved_type_at(i), view_old.resolved_type_at(i), "rt {i} {md:?}");
            assert_eq!(view_new.is_block_start(i), view_old.is_block_start(i), "bs {i} {md:?}");
            assert_eq!(view_new.end_for(i), view_old.end_for(i), "end {i} {md:?}");
        }
    }
}

#[test]
fn with_structure_provides_view() {
    let md = "- a\n- b\n";
    let snap = snap_of(md);
    let entries = &snap.frame_log.entries;
    let lines = &snap.lines;
    let rope = &snap.rope;
    let n = snap.line_len() as u32;
    let sum = with_structure(entries, lines, &snap.open_depth, rope, (0, n), |_records, _blocks, view| {
        (0..n as usize).filter(|&i| view.is_block_start(i)).count()
    });
    assert!(sum >= 2, "expected at least two block starts, got {sum}");
}


// ── Step 3：RecordFold 局部化守卫 ──

#[test]
fn has_pop_at_matches_full_pop_set() {
    for md in [
        "",
        "# H\n",
        "- a\n- b\n",
        "> q\n> r\n",
        "| a |\n| - |\n| b |\n",
    ] {
        let snap = snap_of(md);
        let entries = &snap.frame_log.entries;
        let full: std::collections::HashSet<u32> = entries.iter()
            .filter(|e| crate::block::entry_is_pop(**e))
            .map(|e| crate::block::entry_line(*e))
            .collect();
        for line in 0..=snap.line_len() as u32 {
            assert_eq!(
                has_pop_at(entries, line),
                full.contains(&line),
                "has_pop_at mismatch line={line} md={md:?}"
            );
        }
    }
}

#[test]
fn record_fold_walk_zone_ignores_prefix_records() {
    let md = "- a\n- b\n- c\n";
    let snap = snap_of(md);
    let entries = &snap.frame_log.entries;
    let lines = &snap.lines;

    let mut fold = RecordFold::new(entries, lines);
    record_fold_walk_zone(entries, (2, 3), &mut fold);
    let (recs, _arena) = fold.finish();
    assert!(
        recs.iter().all(|r| r.start as usize >= 2),
        "record_fold_walk_zone must not materialize prefix records: {recs:?}"
    );

    let mut full_fold = RecordFold::new(entries, lines);
    walk_zone(entries, (0, snap.line_len() as u32), &mut full_fold);
    let (full_recs, _arena) = full_fold.finish();
    assert!(full_recs.iter().any(|r| (r.start as usize) < 2));
}

/// 事件级守卫：zone 构建原子在 `on_zone_begin` 前不得触碰任何 prefix entry。
/// 与 `build_structure_parts` 的静态接线（record_fold_walk_zone 直调）共同
/// 构成 Step 8 的“计数 fold 不碰 prefix entries”门禁。
#[test]
fn record_fold_walk_zone_never_folds_prefix_entries() {
    struct PrefixTouchGuard {
        producing: bool,
        prefix_touches: usize,
        zone_events: usize,
    }
    impl crate::infer::EntryFold for PrefixTouchGuard {
        fn on_zone_begin(&mut self) {
            self.producing = true;
        }
        fn push(&mut self, _e: u64) {
            if self.producing { self.zone_events += 1; } else { self.prefix_touches += 1; }
        }
        fn pop(&mut self, _e: u64, _next: Option<u64>) {
            if self.producing { self.zone_events += 1; } else { self.prefix_touches += 1; }
        }
        fn body_mut(&mut self, _e: u64) {
            if self.producing { self.zone_events += 1; } else { self.prefix_touches += 1; }
        }
    }

    let md = "- a\n- b\n  - c\n- d\n";
    let snap = snap_of(md);
    let entries = &snap.frame_log.entries;
    let mut guard = PrefixTouchGuard { producing: false, prefix_touches: 0, zone_events: 0 };
    walk_zone_no_prefix(entries, (2, 4), &mut guard);
    assert_eq!(guard.prefix_touches, 0,
        "zone build must not fold prefix entries; zone_events={}", guard.zone_events);
    assert!(guard.zone_events > 0, "zone must contain structural events");
}

#[test]
fn record_fold_walk_zone_equals_walk_zone_for_depth0_zones() {
    let samples = ["", "# H\n", "- a\n- b\n", "> q\n\np\n"];
    for md in samples {
        let snap = snap_of(md);
        let entries = &snap.frame_log.entries;
        let lines = &snap.lines;
        let n = snap.line_len() as u32;

        let mut a = RecordFold::new(entries, lines);
        record_fold_walk_zone(entries, (0, n), &mut a);
        let (recs_a, arena_a) = a.finish();

        let mut b = RecordFold::new(entries, lines);
        walk_zone(entries, (0, n), &mut b);
        let (recs_b, arena_b) = b.finish();

        assert!(crate::cache::records_structurally_eq(&recs_a, &arena_a, &recs_b, &arena_b),
            "zone walker mismatch for {md:?}");
    }
}

    // ── cache_fix_824 §1:zone_ce_closure_fl 闭包语义单测 ──
    #[test]
    fn closure_container_extends_and_idempotent() {
        let s = snap_of("> a\n> b\n\nend\n");
        let e = &s.frame_log.entries;
        let l = &s.lines;
        let n = s.line_len();
        // cs=1 切入 BQ 内(入口锚 snap 后 0);ce=1 → ① 容器闭包应推至 BQ 闭合
        // (NoClose 型:端 = pop 行 2——fold 需含 pop 行 → ce=3)
        let r1 = zone_ce_closure_fl(e, l, &s.open_depth, n, 1, 1, 0, None, 1);
        assert_eq!(r1, 3, "container closure must extend ce past the BQ pop line");
        // 幂等:以闭包结果为输入应稳定
        let r2 = zone_ce_closure_fl(e, l, &s.open_depth, n, 1, r1, 0, None, 1);
        assert_eq!(r2, r1, "closure fixpoint must be idempotent");
    }

    #[test]
    fn closure_setext_walks_without_index() {
        let s = snap_of("foo\n===\nbar\n");
        let e = &s.frame_log.entries;
        let l = &s.lines;
        let n = s.line_len();
        // 冷路径(old_idx=None):② 深度守卫行走应纳入下划线行
        let r = zone_ce_closure_fl(e, l, &s.open_depth, n, 1, 1, 0, None, 1);
        assert_eq!(r, 2, "setext underline must be pulled into the zone");
    }

    #[test]
    fn closure_certified_early_stop() {
        let s = snap_of("foo\n===\nbar\n");
        let e = &s.frame_log.entries;
        let l = &s.lines;
        let n = s.line_len();
        // 认证:opener(0) ≤ anchor(0) 且旧块 (0,3) 跨旧切点 1 → 零前看,ce 不动
        let idx: &[(u32, u32)] = &[(0, 3)];
        let r = zone_ce_closure_fl(e, l, &s.open_depth, n, 1, 1, 0, Some(idx), 1);
        assert_eq!(r, 1, "certified merge must not extend ce");
    }

    #[test]
    fn closure_certification_falls_back_when_opener_after_anchor() {
        let s = snap_of("alpha\nfoo\n===\nbar\n");
        let e = &s.frame_log.entries;
        let l = &s.lines;
        let n = s.line_len();
        // 认证失败(opener=1 > anchor=0)→ 行走纳入下划线
        let idx: &[(u32, u32)] = &[(1, 3)];
        let r = zone_ce_closure_fl(e, l, &s.open_depth, n, 2, 2, 0, Some(idx), 2);
        assert_eq!(r, 3, "non-certified setext must walk to the underline");
    }

    /// sealed_interval_of(827 重写阶段 0):最外层区间链上移 + 空隙兜底 +
    /// 双侧容器密封——三形态契约(bq 内节拉入容器闭包 / 嵌套节上移 /
    /// 根级节恒等)。
    #[test]
    fn sealed_interval_of_contract() {
        // h3_in_bq 旧侧:节 (1,2) 在 bq (0,2) 内 → 密封到 (0,2)。
        let s = snap_of("> outer\n> ### deep\n");
        let r = sealed_interval_of(
            &s.frame_log.entries, &s.lines, &s.open_depth, s.line_len(), &[(1, 2)], 1);
        assert_eq!(r, (0, 2), "container-internal section must seal to container closure");
        // h3_in_bq 新侧:节 (2,3) 的 bq 之前有未闭合列表(行 2 事件前深度
        // 为 1)——锚保守回退到 0,密封域 (0,3)(契约 = 合法 depth-0 域,
        // 非最小域;路径由 off+comps 保证:sec 在域树 [2,0] → /ct/2/ct/0)。
        let s2 = snap_of("> outer\n- ins\n> ### deep\n");
        let r2 = sealed_interval_of(
            &s2.frame_log.entries, &s2.lines, &s2.open_depth, s2.line_len(), &[(2, 3)], 2);
        assert_eq!(r2, (0, 3), "sealing anchors conservatively to depth-0");
        // 嵌套 B 在 A 内:含 line 3 的节 (3,5) 沿包含链上移 → A (0,5) 密封恒等。
        let s3 = snap_of("# A\n\ntext\n\n# B\n\nmore\n");
        let r3 = sealed_interval_of(
            &s3.frame_log.entries, &s3.lines, &s3.open_depth, s3.line_len(), &[(0, 5), (3, 5)], 3);
        assert_eq!(r3, (0, 5), "nested section must walk up to outermost");
        // 空隙兜底:line 0 无节含 → 空隙 (0,2) 密封恒等。
        let s4 = snap_of("text\n\n# A\n");
        let r4 = sealed_interval_of(
            &s4.frame_log.entries, &s4.lines, &s4.open_depth, s4.line_len(), &[(2, 3)], 0);
        assert_eq!(r4, (0, 2), "gap fallback seals the inter-section gap");
        // 标题打断容器:节 (3,6) 起点处列表恰关闭(depth_before(s+1)==0)
        // → 左界不扩(扩入前一节内部会使 off+comps 双计)。
        let s5 = snap_of("# H\n\n- a\n# X\n\n> q\n");
        let r5 = sealed_interval_of(
            &s5.frame_log.entries, &s5.lines, &s5.open_depth, s5.line_len(), &[(0, 3), (3, 6)], 3);
        assert_eq!(r5, (3, 6), "interrupting container must not extend left into previous section");
    }


// T3: heading scale —— 确认/证伪 A5（section_intervals_from_heads）

/// 构造 H 个标题元数据，尽量用尽 (level, depth, in_list) 组合，
/// 使 `section_intervals_from_heads` 的后继查找尽量长。
fn heading_meta_pressure(h: usize) -> Vec<crate::infer::HeadingMeta> {
    (0..h)
        .map(|i| crate::infer::HeadingMeta {
            start: i as u32,
            level: (1 + (i % 6)) as u8,
            block_type: crate::block::BlockType::Header1,
            depth: (i % 32) as u8,
            in_list: i % 2 == 0,
            bound: h as u32,
            container_ref: u32::MAX,
        })
        .collect()
}

/// A5 曲线：`section_intervals_from_heads` 对 H=500/2000 的后继查找。
/// 若文档断言的最坏 O(H²) 成立，4 倍标题数应约 16 倍时间；
/// 若 (level,depth,in_list) 组合有界（6×32×2）导致截断，曲线应接近线性。
#[test]
fn curve_section_intervals_from_heads() {
    let _perf_guard = crate::PERF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    use std::time::{Duration, Instant};
    let time = |h: usize| {
        let heads = heading_meta_pressure(h);
        let n = h;
        let mut best = Duration::from_secs(60);
        for _ in 0..5 {
            let t = Instant::now();
            let _ = crate::infer::section_intervals_from_heads(&heads, n);
            best = best.min(t.elapsed());
        }
        best
    };
    let t5 = time(500);
    let t20 = time(2_000);

    let ratio = t20.as_nanos() as f64 / t5.as_nanos() as f64;

    assert!(ratio < 10.0,
        "若超线性接近二次方，应超过 10 倍: 500={t5:?} 2000={t20:?}");
}



// T1: container-heavy prefix + tail —— 确认 A1/A2 前缀重放

/// 构造 container-heavy 前缀：每行 `> - x`（BQ 壳开一次 + 每行列表
/// PopPush 一对 entries）。返回 (doc, snapshot, entries 行数)。
fn container_prefix_case(n: usize) -> (String, crate::projection::Snapshot) {
    let mut doc = String::new();
    for _ in 0..n {
        doc.push_str("> - x\n");
    }
    let mut state = crate::projection::State::new(&doc);
    let snap = state.snapshot();
    (doc, snap)
}

/// L0 深度列收束后的回归曲线：`Snapshot::snap_to_depth0` 列直读回退，
/// 应随 container-heavy 前缀线性（不再是超线性重放）。
#[test]
fn curve_snap_to_depth0_after_col() {
    let _perf_guard = crate::PERF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    use std::time::{Duration, Instant};
    let time = |n: usize| {
        let (_doc, snap) = container_prefix_case(n);
        let mut best = Duration::from_secs(60);
        for _ in 0..5 {
            let t = Instant::now();
            let _ = snap.snap_to_depth0(n - 1);
            best = best.min(t.elapsed());
        }
        best
    };
    let t1 = time(1_000);
    let t4 = time(4_000);

    assert!(
        // 阈值 10（线性理论 4）：并行全量跑时 container-heavy 构建受抢 CPU 干扰，
        // 7.0 曾误红；旧「超线性重放」形态远高于 10，仍能判出回归。
        (t4.as_nanos() as f64) < (t1.as_nanos() as f64) * 10.0,
        "深度列直读后 snap_to_depth0 应线性（4x 输入 ≤6x 时间）: 1k={t1:?} 4k={t4:?}"
    );
}

/// [P0 §3 身份判官] `root_blocks_before` 的 start 集合必须 == 同区间 (0,end) 上
/// CF 归属树 roots 的 start 集合（**数量判官不够**——同名数量可掩盖身份错位）。
/// 定向样本 + 确定性伪随机拼装；end_line 全枚举。
#[test]
fn root_blocks_before_identity_matches_cf_tree_roots() {
    let frags: [&str; 8] = [
        "# H\n", "para\n", "- item\n", "> bq\n",
        "```\ncode\n```\n", "\n", "## H2\n", "1. ol\n",
    ];
    let mut docs: Vec<String> = vec![
        "# A\n\npara\n\n# C\n".to_string(),
        "# A\n\npara\n\n## B\n\nx\n\n# C\n".to_string(),
        "- a\n- b\n".to_string(),
        "> q\n>\n> r\n".to_string(),
        "```rust\ncode\n```\n".to_string(),
        "# A\n\n- x\n\n# C\n".to_string(),
        "para\n\n# A\n\npara2\n\n# C\n".to_string(),
        "# A\n- x\n".to_string(),
    ];
    let mut s: u64 = 0x9E3779B97F4A7C15;
    for _ in 0..48 {
        let mut d = String::new();
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let k = 1 + ((s >> 33) as usize % 6);
        for _ in 0..k {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            d.push_str(frags[(s >> 33) as usize % frags.len()]);
        }
        docs.push(d);
    }
    let mut checked = 0usize;
    let mut bad: Vec<(String, usize, Vec<usize>, Vec<usize>)> = Vec::new();
    for doc in &docs {
        let snap = crate::projection::State::new(doc).snapshot();
        let n = snap.line_len();
        for e in 0..=n {
            let rb = crate::infer::root_blocks_before(
                &snap.frame_log.entries, &snap.lines, &snap.rope, &snap.frame_log, e);
            let mut got: Vec<usize> = rb.iter().map(|x| x.0).collect();
            got.sort_unstable();
            let seg = crate::cache::Segment::build(&snap, 0, e);
            let view = crate::infer::LineView::with_arrays(
                &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
                (seg.start_line as u32, seg.end_line as u32), seg.view.clone(), &seg.arena);
            let tree = crate::infer::build_attribution_tree(
                &seg.records, &seg.blocks, &view, &seg.arena, (0, e as u32),
                &crate::topology::CONTAINER_TOPOLOGY);
            let mut want: Vec<usize> =
                tree.roots.iter().map(|&i| tree.nodes[i as usize].start as usize).collect();
            want.sort_unstable();
            if got != want { bad.push((doc.clone(), e, got, want)); }
            checked += 1;
        }
    }
    for (d, e, g, w) in bad.iter().take(8) {

    }
    assert!(bad.is_empty(), "root_blocks_before 身份判官: {} / {} 不一致", bad.len(), checked);
}



/// memo 钉（受检不变量 1）：build_node 的两个 memo
/// （peel_width / geometry）必须恒等于其组合式——
///   peel_width ≡ peel_peeled(行).rest_after_bq() 的前缀字节宽
///   geometry  ≡ line_geometry(nt, 剥后字节)
/// 全域（27 类型 × 形状集）比对——防止 memo 与算式漂移。
#[test]
fn memo_pin_geometry_peel_width_vs_composition() {
    let shapes: &[&str] = &[
        "- item", "- ", "-", "* item", "+ item", "- [x] done", "1. item", "1) item",
        "> x", "> > deep", "> - a", ">     code", "    code", "```rust", "# h",
        "---", "===", "<div>", "<script>", "| a |", "|--|", "[a]: /url",
        "text", "  text", "",
    ];
    let mut checked = 0usize;
    for t in 0..27usize {
        let nt: crate::block::BlockType = unsafe { std::mem::transmute(t as u8) };
        for &s in shapes {
            // P3-2 空文档守卫域外：rope 0 行时 build_node 短路 (0, None)——
            // 守卫语义而非漂移，钉的域排除之。
            if s.is_empty() { continue; }
            let mut state = crate::projection::State::new(s);
            let snap = state.snapshot();
            let node = snap.build_node(0, 0, nt, false);
            // memo 1：peel_width ≡ 全剥 BQ 前缀字节宽
            let stripped = crate::block::peel_peeled(s, 0).rest_after_bq();
            let pw = stripped.as_ptr() as usize - s.as_ptr() as usize;
            assert_eq!(node.peel_width, pw, "memo 钉 peel_width 漂移: {nt:?} '{s}'");
            // memo 2：geometry ≡ line_geometry(nt, 剥后字节)
            let recomputed = crate::block::line_geometry(nt, stripped.as_bytes());
            assert!(
                match (&node.geometry, &recomputed) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                },
                "memo 钉 geometry 漂移: {nt:?} '{s}' memo={:?} 重算={:?}",
                node.geometry, recomputed
            );
            checked += 1;
        }
    }
    eprintln!("[MEMO-PIN] 27×{}={} 对 memo≡重算 全部一致", shapes.len(), checked);
}

/// 层钉（受检不变量 3）：markers_on_line 的 cut
/// 随覆盖链深度单调增——嵌套每加一层（BQ 或列表 marker），前缀消费严格多一层。
#[test]
fn layer_pin_cut_monotone_with_chain_depth() {
    let cases: &[(&str, usize)] = &[
        ("- a", 2),
        ("> a", 2),
        ("- - a", 4),
        ("> - a", 4),
        ("> > a", 4),
        ("> > - a", 6),
    ];
    for &(s, want_cut) in cases {
        let mut state = crate::projection::State::new(s);
        let snap = state.snapshot();
        let entries = &snap.frame_log.entries;
        let stack = crate::block::rebuild_stack_at(entries, 1);
        let text = snap.rope.line(0).to_string();
        let pc = crate::infer::prefix_cut(
            &text, &stack, entries,
            crate::effect::PrefixKinds::All,
            crate::effect::PrefixStop::Counted,
            crate::effect::PrefixLead::SkipWs,
            crate::effect::PrefixTail::None);
        let cut = pc.bytes;
        assert_eq!(cut, want_cut, "层钉 cut 漂移: '{s}'");
    }
    eprintln!("[LAYER-PIN] 6 例 cut 随覆盖链深度单调 ✓");
}

/// 面钉（受检不变量 2）：写面内容起点 ≡ 统一入口
/// content_face 的组合起点（byte_of_line + peel_width + 声明 content_offset）
/// ——对 marker 类类型（item/IC）逐行比对；其余类型写面走整块语义（起点=行首）
/// 不在本钉域。钉住「面实现与坐标律一致」，防漂移。
#[test]
fn face_pin_write_start_vs_content_face() {
    let docs: &[&str] = &[
        "    code\n",
        ">     code\n",
        "- x\n      deep\n",
        "- assistant:\n  >         deep\n  >     普通\n",
        "> >     1\n> >     2\n",
        "- a\n  >     B\n",
    ];
    let mut checked = 0usize;
    for doc in docs {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let proj = snap.node_proj();
        for line in 0..snap.rope.line_len() {
            let nt = snap.line_type_at(line);
            if !matches!(nt,
                crate::block::BlockType::IndentedCode
                | crate::block::BlockType::Bullet
                | crate::block::BlockType::Checklist
                | crate::block::BlockType::OrderedListItem)
            {
                continue;
            }
            // 键值面（ColonKey/CheckboxColonKey）写面走「值槽起点」＝tag_range 几何
            // ——另一坐标身份（LineGeo.tag_range 声明），不在内容面钉域。
            if crate::effect::is_key_like(crate::effect::line_spec(nt).tag_rule) {
                continue;
            }
            let text = snap.rope.line(line).to_string();
            let (pw, geo) = crate::infer::content_face(&text, nt);
            let start_face = if nt == crate::block::BlockType::IndentedCode {
                // IC 内容面＝层轴律（prefix_cut(All,Counted) cut 不含 BQ 可选空白）＋4＋edge
                let stack = crate::block::rebuild_stack_at(
                    snap.frame_log.entries.as_slice(), line as u32 + 1);
                let pc = crate::infer::prefix_cut(
                    &text, &stack,
                    crate::infer::line_entries_at(snap.frame_log.entries.as_slice(), line as u32),
                    crate::effect::PrefixKinds::All,
                    crate::effect::PrefixStop::Counted,
                    crate::effect::PrefixLead::SkipWs,
                    crate::effect::PrefixTail::None);
                snap.rope.byte_of_line(line) + pc.bytes + crate::effect::geo::LIST_CONTENT_MAX + pc.edge
            } else {
                // item 内容面＝节点几何入口（rest_after_bq 含可选空白——memo 同源）
                snap.rope.byte_of_line(line) + pw
                    + geo.as_ref().map_or(0, |g| g.content_offset)
            };
            let body = text.trim_end_matches('\n').len();
            let rng = snap.rope.byte_of_line(line)..snap.rope.byte_of_line(line) + body;
            if let Ok(w) = proj.content_write_range(&rng, nt) {
                if w.start != rng.start {
                    assert_eq!(w.start, start_face, "面钉写面起点漂移: {nt:?} line {line} '{text:?}'");
                    checked += 1;
                }
            }
        }
    }
    eprintln!("[FACE-PIN] {checked} 行 写面起点 ≡ content_face 组合 ✓");
}

/// OL 跳标缺口修复定向门（待证⑤）：#9 迁移后嵌套 OL/OL-含-BL 的写面值槽起点。
/// `1. 2. a`：内层 OL item 值槽 = "a"（旧码 = "2. a"——含内层 marker）；
/// `1. - k: v`：内层 bullet 值槽 = "v"（旧码 = 整块回退——key 槽丢失）。
#[test]
fn ol_skip_marker_write_slot() {
    use crate::block::{BlockNode, BlockType};
    use crate::projection::State;
    let doc = "1. 2. a\n1. - k: v\n";
    let mut st = State::new(doc);
    let snap = st.snapshot();
    let np = crate::infer::NodeProj::new(&snap.rope, &snap.lines, &snap.frame_log.entries);
    // 内层 OL item（`2. a` @3..7）——Digits 值槽起点
    let node_oli = BlockNode {
        byte_range: 3..7,
        node_type: BlockType::OrderedListItem,
        ..BlockNode::dummy()
    };
    let w = np
        .content_write_range(&node_oli.byte_range, node_oli.node_type)
        .unwrap();
    assert_eq!(&doc[w.start..w.end], "a", "内层 OL 值槽起点");
    // 内层 bullet（`- k: v` @11..17）——ColonKey 值槽
    let node_blt = BlockNode {
        byte_range: 11..17,
        node_type: BlockType::Bullet,
        ..BlockNode::dummy()
    };
    let w = np
        .content_write_range(&node_blt.byte_range, node_blt.node_type)
        .unwrap();
    assert_eq!(&doc[w.start..w.end], "v", "内层 bullet 值槽");
}

/// P-ct 对齐：空值槽 + 子块的 bullet，值槽写范围应跨到子块末行
/// （与读面 `-pj ct` 一致）；同深度兄弟条目、单行值条目保持不变。
#[test]
fn bullet_child_blocks_content_slot_spans_children() {
    use crate::block::{BlockNode, BlockType};
    use crate::projection::State;
    let doc = "- a:\n  > x\n  > y\n- b: v\n";
    let mut st = State::new(doc);
    let snap = st.snapshot();
    let np = crate::infer::NodeProj::new(&snap.rope, &snap.lines, &snap.frame_log.entries);

    // 空值 + bq 子块：值槽 = 首行值槽起点 .. 子块末行（剥尾换行）
    let node_a = BlockNode {
        byte_range: 0..5,
        node_type: BlockType::Bullet,
        ..BlockNode::dummy()
    };
    let w = np.content_write_range(&node_a.byte_range, node_a.node_type).unwrap();
    assert_eq!(&doc[w.start..w.end], "\n  > x\n  > y", "子块应并入值槽跨度");

    // 同深度兄弟条目：值槽仍只有自己的 "v"
    let node_b = BlockNode {
        byte_range: 17..doc.len(),
        node_type: BlockType::Bullet,
        ..BlockNode::dummy()
    };
    let w = np.content_write_range(&node_b.byte_range, node_b.node_type).unwrap();
    assert_eq!(&doc[w.start..w.end], "v", "兄弟条目不得吞并");

    // 嵌套子列表不并入（content_write_keeps_sublist 语义）
    let doc2 = "- a:\n  - b\n  - c\n";
    let mut st2 = State::new(doc2);
    let snap2 = st2.snapshot();
    let np2 = crate::infer::NodeProj::new(&snap2.rope, &snap2.lines, &snap2.frame_log.entries);
    let node_a2 = BlockNode {
        byte_range: 0..5,
        node_type: BlockType::Bullet,
        ..BlockNode::dummy()
    };
    let w = np2.content_write_range(&node_a2.byte_range, node_a2.node_type).unwrap();
    assert_eq!(w.start, w.end, "sublist 之前值槽保持零宽（不吞子列表）");
}

/// P0 回归（生产输入形态钉）：line_entries 整表传参曾使列表预算扩为全文
/// 规模——文档任何位置有列表条目时，容器内 IC 行（`>     - …`）的 `- ` 被
/// 当层吃掉（读面错、写面范围同错）。钉：本行切片的
/// cut 只到容器前缀（2 字节），4 空格后的 `- ` 属内容。
#[test]
fn p0_line_entries_slice_regression() {
    let doc = "- outside\n\n> quote\n>     - 只要今天的\n";
    let mut st = crate::projection::State::new(doc);
    let snap = st.snapshot();
    let proj = snap.node_proj();
    // 行 3（0 基）= ">     - 只要今天的"：BQ 续行、内容为 IC；
    // 行 0 的 bullet push 使整表预算 ≥1（暴露条件③），本行切片预算 = 0。
    let (cut, indent) = proj.ic_line_prefix_cut(3);
    assert_eq!(cut, 2, "IC 内容行 cut 应为 BQ 前缀 2 字节——`- ` 是内容不是层");
    assert_eq!(indent, crate::effect::geo::LIST_CONTENT_MAX, "无列表边缘");
    let text = snap.rope.line(3).to_string();
    assert_eq!(&text[cut..], "    - 只要今天的", "IC 内容被吃");
}

// ── 主树 HEAD 的两条行为守护（落地时从 `fe3ce598` 原样移植；台子重构后语义不变）──

/// P3-1：**空文档整写**（`mk -f /doc.md` 后 `"# A\n- v: 1" | /> /doc.md -god` 那条路径）
/// 不得 panic；空文档没有容器前缀可体现 ⇒ 原文直写。
/// 原崩点：「容器前缀体现」臂 guard 里的 `self.rope.line(node.start_line)`
/// （空 rope 只有 0 行，而整篇节点 `start_line = 0` ✗）。
#[test]
fn empty_doc_content_write_does_not_panic() {
    let mut st = crate::projection::State::new("");
    let snap = st.snapshot();
    // 整篇节点：**直接构造**（不走 build_node 的 clamp）——正是 P3-1 的路径。
    let node = crate::block::BlockNode {
        start_line: 0,
        end_line: 0,
        byte_range: 0..0,
        node_type: crate::block::BlockType::Paragraph,
        loose: false,
        geometry: None,
        peel_width: 0,
        frame_ordinal: 0,
    };
    let got = snap.compute_write_from_node(&node, "content", "# A\n- v: 1");
    assert!(got.is_ok(), "空文档整写不应 panic／报错：{got:?}");
    let (_r, text) = got.expect("上一行已断言 ok");
    assert_eq!(text, "# A\n- v: 1", "空文档应原文直写");
}

/// P3-2：**越界节点**不得 panic——`build_node` 的 clamp 上界必须是**最后一个合法下标**
/// （`len()-1`，不是 `len()` ✗），空 rope 走几何短路。
/// 原崩点：`build_node` 内的 `self.rope.line(start_line)`（行号被夹到 `len()` ⇒ 越界 ✗）。
#[test]
fn degenerate_node_never_panics() {
    let doc = "alpha\nbeta\n";
    let mut st = crate::projection::State::new(doc);
    let snap = st.snapshot();
    for (s, e) in [(9usize, 9usize), (2, 9), (0, 9)] {
        let n = snap.build_node(s, e, crate::block::BlockType::Paragraph, false);
        assert!(n.end_line >= n.start_line, "行区间必须有序：{s}..{e}");
        assert!(
            n.byte_range.start <= doc.len() && n.byte_range.end <= doc.len(),
            "字节范围必须落在文档内：{s}..{e} ⇒ {:?}",
            n.byte_range
        );
    }
    // 空 rope：几何短路（不读行），一切取零值。
    let mut st2 = crate::projection::State::new("");
    let snap2 = st2.snapshot();
    let n = snap2.build_node(0, 0, crate::block::BlockType::Paragraph, false);
    assert_eq!(n.byte_range, 0..0, "空 rope 的字节范围应为空");
    assert_eq!(n.peel_width, 0, "空 rope 的前缀宽应为 0");
    assert!(n.geometry.is_none(), "空 rope 不应计算几何");
}

/// A6：CF 归属树的结构 oracle——父链由容器记录的 parent 字段独立重建
/// （记录折叠产物，不读树构建代码）；逐块比较「树父块」与「记录父块」。
#[test]
fn tree_fold_block_nesting_oracle() {
    for md in corpus().iter().chain(corpus_edge().iter()) {
        let snap = snap_of(md);
        let n = snap.line_len();
        let seg = crate::cache::structure_for_range(&snap, 0, n);
        let tree = snap.tree_cell(&crate::topology::CONTAINER_TOPOLOGY).tree.clone();
        let recs = &seg.records;
        // 记录 → 块的反查（理想一一，重复则跳过该记录的比较）。
        let mut by_rec: std::collections::HashMap<usize, Option<usize>> = std::collections::HashMap::new();
        for (bi, b) in seg.blocks.iter().enumerate() {
            let r = b.container_ref as usize;
            if r >= recs.len() { continue; }
            by_rec.entry(r)
                .and_modify(|slot| { if *slot != Some(bi) { *slot = None; } })
                .or_insert(Some(bi));
        }
        // 期望父链：child 记录 parent → 父记录 → 父块。
        let mut expected: Vec<Option<usize>> = vec![None; seg.blocks.len()];
        for (bi, b) in seg.blocks.iter().enumerate() {
            let r = b.container_ref as usize;
            if r >= recs.len() { continue; }
            let pr = recs[r].parent;
            if pr == u32::MAX { continue; }
            if let Some(Some(pbi)) = by_rec.get(&(pr as usize)) {
                expected[bi] = Some(*pbi);
            }
        }
        // 实际父链：沿树走，隐式节点（blk 无）不打断父块链。
        let mut actual: Vec<Option<usize>> = vec![None; seg.blocks.len()];
        let mut stack: Vec<(u32, Option<usize>)> =
            tree.roots.iter().rev().map(|&r| (r, None)).collect();
        while let Some((ni, pblk)) = stack.pop() {
            let node = &tree.nodes[ni as usize];
            if let Some(b) = node.blk_idx() { actual[b] = pblk; }
            let child_pblk = node.blk_idx().or(pblk);
            for &c in node.children_of(&tree).iter().rev() {
                stack.push((c, child_pblk));
            }
        }
        for bi in 0..seg.blocks.len() {
            // item/隐式壳的父链是树重组产物；记录 parent 只声明容器父→子。
            let node_is_item = tree.nodes.iter().find(|nd| nd.blk_idx() == Some(bi)).map_or(false, |nd| nd.is_item() || nd.implicit());
            if node_is_item || expected[bi].is_none() { continue; }
            assert_eq!(actual[bi], expected[bi], "块 {bi} 父链不一致（记录 parent oracle）: {md:?}");
        }
    }
}

/// C1：container_end_for 双口径差异判官——LineView（该行起点记录的最大 end）
/// 与 BuildView（构建期栈顶记录的 end）在复合开行上应产生差异；
/// 判官只钉「差异存在且 LineView 不小于 BuildView」，不强行统一两口径。
#[test]
fn container_end_for_dual_caliber() {
    use crate::effect::LineResolveView;
    let mut diff = 0usize;
    let extra: [&str; 6] = ["- - x\n", "> - x\n> y\n", "1. - x\n", "> > a\n> > b\n", "> - x\n>\n> tail\n", "> - x\n\nafter\n"];
    for md in corpus().iter().chain(corpus_edge().iter()).chain(extra.iter()) {
        let snap = snap_of(md);
        let entries = &snap.frame_log.entries;
        let n = snap.line_len() as u32;
        let mut fold = RecordFold::new(entries, &snap.lines);
        walk_zone(entries, (0, n), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = build_block_records(entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n));
        let lv = LineView::new(&recs, &blocks, entries, &snap.lines, &snap.rope, (0, n), &arena);
        let va = build_view_arrays(&recs, &arena, entries, (0, n));
        let tc = table_confirmed_flags(entries, (0, n));
        let mut stack: Vec<u32> = Vec::new();
        let mut ei = partition(entries, 0);
        let mut last_rel = usize::MAX;
        let mut line_count = 0usize;
        for i in 0..n as usize {
            let bv = build_view_at(&recs, entries, &snap.lines, &va, &tc, (0, n),
                i as u32, &mut stack, &mut ei, &mut last_rel, &mut line_count);
            // table_confirmed_at 跨视图对拍（C1：两口径同源的表格确认位）。
            assert_eq!(lv.table_confirmed_at(i), bv.table_confirmed_at(i),
                "table_confirmed 双视图不一致 @{i}: {md:?}");
            let la = lv.container_end_for(i);
            let ba = bv.container_end_for(i);
            if let (Some(a), Some(b)) = (la, ba) {
                assert!(a >= b, "LineView 尾应 >= BuildView 尾 @{i}: {md:?}");
            }
            if la != ba { diff += 1; }
            // 定点：内层容器先闭、外层容器延续时两口径必然分叉。
            if *md == "> - x\n>\n> tail\n" && i == 0 {
                assert_eq!(la, Some(3), "LineView 应取外层 BQ 尾: {md:?}");
                assert_eq!(ba, Some(2), "BuildView 应取内层列表尾: {md:?}");
            }
        }
    }
    assert!(diff > 0, "复合开行上应出现两口径差异（判官未覆盖差异面）");
}
// ── G7（判据先行）──
// 空值条目冒号在行末、无尾换行（`- k:` 为末行）：tag_range 落在行末冒号上，
// byte(pos+1) 越界 panic。三入口：直调读内容面 / 写面快路径 / 生产信封。
#[test]
fn g7_probe_read_content_arm_no_panic() {
    let snap = snap_of("- k:");
    let node = snap.build_node(0, 1, crate::block::BlockType::Bullet, false);
    let _ = snap.read_block_proj(&node, crate::block::BlockProj::Content);
}

#[test]
fn g7_probe_write_face_no_panic() {
    let snap = snap_of("- k:");
    let node = snap.build_node(0, 1, crate::block::BlockType::Bullet, false);
    // 写面快路径 tag 分支与 content 分支都过一遍
    let _ = snap.node_proj().compute_write_from_node(&node, "tag", "x");
    let _ = snap.node_proj().compute_write_from_node(&node, "content", "x");
}

#[test]
fn g7_prod_probe_envelope_no_panic() {
    for mode in [crate::topology::ProjectionMode::Cf,
                 crate::topology::ProjectionMode::Hs,
                 crate::topology::ProjectionMode::Bf] {
        let _ = crate::projection::State::new("- k:").snapshot()
            .to_ast_object_envelope(mode, None);
    }
}
