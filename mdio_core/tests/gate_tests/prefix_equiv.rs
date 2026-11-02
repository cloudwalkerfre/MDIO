//! ★前缀等价钉（8.1 门 #6）：prefix_cut 逐字段（bytes/edge——cols 由
//! bytes 同源推导不单比）vs 今日四种实现（markers_on_line/content_face/写面
//! peel/peel_markers），语料逐行。alt（Adjacency）不比对——
//! 其等价性由 8.2 的 peel_width==0 两投影门承担。
//! 用法：MDIO_DUMP_ROOT=<目录> cargo test --test prefix_equiv --release -- --nocapture

use mdio_core::block::{
    BlockFrame, FrameKind, entry_consume, entry_frame, entry_is_body_mut, entry_is_pop, entry_line,
};
use mdio_core::effect::{PrefixKinds, PrefixLead, PrefixStop, PrefixTail};
use mdio_core::infer::prefix_cut;

/// 钉①锚（生产 markers_on_line 已迁 prefix_cut 后，实现留此对拍——
/// 锚不随迁移消失：折叠与实现对拍逐字段 0 分歧的保证不变）。
fn markers_on_line_ref(
    text: &str,
    stack: &[BlockFrame],
    entries: &[u64],
    line: u32,
) -> (usize, usize) {
    let mut bq_left = stack
        .iter()
        .filter(|f| f.kind() == FrameKind::BlockQuote)
        .count();
    let start = entries.partition_point(|&e| entry_line(e) < line);
    let mut list_left = 0usize;
    for &e in &entries[start..] {
        if entry_line(e) != line {
            break;
        }
        if entry_is_pop(e) {
            continue;
        }
        let k = entry_frame(e).kind();
        if k != FrameKind::BlockQuote && mdio_core::effect::is_list_container(k) {
            list_left += 1;
        }
    }
    let (layers, _) = mdio_core::block::peel(text);
    let mut cut = 0usize;
    for layer in layers.iter() {
        let ok = match layer.kind {
            FrameKind::BlockQuote if bq_left > 0 => {
                bq_left -= 1;
                true
            }
            k if mdio_core::effect::is_list_container(k) && list_left > 0 => {
                list_left -= 1;
                true
            }
            _ => false,
        };
        if ok {
            cut = layer.end;
        } else {
            break;
        }
    }
    let edge: usize = stack
        .iter()
        .filter(|f| mdio_core::effect::is_list_container(f.kind()))
        .map(|f| (f.indent() as usize).saturating_sub(cut))
        .sum();
    (cut, edge)
}
use mdio_core::projection::State;
use std::path::{Path, PathBuf};

fn collect(dir: &Path, acc: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, acc);
        } else if p.extension().map(|x| x == "md").unwrap_or(false) {
            acc.push(p);
        }
    }
}

/// 行 k 事件前的栈重放（钉侧独立——push/pop 逐条）。
fn replay_stack_before(entries: &[u64], k: u32) -> Vec<BlockFrame> {
    let mut stack: Vec<BlockFrame> = Vec::new();
    for &e in entries {
        if entry_line(e) >= k {
            break;
        }
        if entry_is_body_mut(e) {
            continue;
        }
        if entry_is_pop(e) {
            stack.pop();
        } else {
            stack.push(entry_frame(e));
        }
    }
    stack
}

/// 行 k 的开层条目（push——bit15=0）。
fn line_pushes(entries: &[u64], k: u32) -> Vec<u64> {
    entries
        .iter()
        .filter(|&&e| entry_line(e) == k && !entry_is_body_mut(e) && !entry_is_pop(e))
        .copied()
        .collect()
}

/// 9.1 交叉点测量：最大语料文档上的写查询工作量——
/// prefix_cut 折叠调用数 vs 文档行数/节点数——同一行跨查询的重复折叠率
/// 即立项判据（≥3 次/行 = 跨相位复用值得立项）。
#[test]
fn cross_point_measurement() {
    use mdio_core::block::BlockProj;
    let root = std::env::var("MDIO_DUMP_ROOT")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../worktrees/genesis/wt").to_string());
    let mut files = Vec::new();
    collect(Path::new(&root), &mut files);
    files.sort();
    let mut biggest: Option<(u64, PathBuf)> = None;
    for f in &files {
        if let Ok(meta) = std::fs::metadata(f) {
            if biggest.as_ref().map_or(true, |(s, _)| meta.len() > *s) {
                biggest = Some((meta.len(), f.clone()));
            }
        }
    }
    let Some((_, path)) = biggest else { return };
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let mut st = State::new(&text);
    let snap = st.snapshot();
    let n = snap.line_len();
    // 节点枚举（写查询目标）
    let entries = &snap.frame_log.entries;
    let mut fold = mdio_core::infer::RecordFold::new(entries, &snap.lines);
    mdio_core::infer::walk_zone(entries, (0, n as u32), &mut fold);
    let (recs, arena) = fold.finish();
    let blocks = mdio_core::infer::build_block_records(
        entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
    let np = mdio_core::infer::NodeProj::new(&snap.rope, &snap.lines, entries);
    let mut writes = 0usize;
    for b in &blocks {
        let node = snap.build_node(b.start as usize, b.end as usize, b.block_type, b.loose());
        // 写查询（content 写）+ 两面读（Content/Container）——语料轻量集成通过。
        let _ = snap.compute_write_from_node(&node, "content", "x");
        let _ = snap.read_block_proj(&node, BlockProj::Content);
        let _ = np.node_container(&node);
        writes += 1;
    }
    // （9.1 测量探针 PREFIX_CUT_COUNT/WRITE_GEOM_NONE_COUNT 已摘除——
    // 测量结论已入账：0.30 次/行不立项、geometry:None 74.4% 不立项。）
}

/// ★Container 一致性钉（8.4 门 #8）：新「直取前缀」(All,Chain) vs 旧「raw−content
/// 后缀相减＋NodeSpan 特例」逐节点等价——语料全节点。
#[test]
fn container_consistency_pin() {
    use mdio_core::block::BlockProj;
    let root = std::env::var("MDIO_DUMP_ROOT")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../worktrees/genesis/wt").to_string());
    let mut files = Vec::new();
    collect(Path::new(&root), &mut files);
    files.sort();
    let mut total_nodes = 0usize;
    let mut fails = 0usize;
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let mut st = State::new(&text);
        let snap = st.snapshot();
        let entries = &snap.frame_log.entries;
        let n = snap.line_len();
        let mut fold = mdio_core::infer::RecordFold::new(entries, &snap.lines);
        mdio_core::infer::walk_zone(entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(
            entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        for b in &blocks {
            total_nodes += 1;
            let node = snap.build_node(
                b.start as usize, b.end as usize, b.block_type, b.loose());
            let np = mdio_core::infer::NodeProj::new(
                &snap.rope, &snap.lines, &snap.frame_log.entries);
            let new = np.node_container(&node);
            // 参照实现（suffix 相减＋NodeSpan 特例——钉参照）
            let raw = snap.byte_slice(node.byte_range.clone()).to_string();
            let content = snap.read_block_proj(&node, BlockProj::Content);
            let old = if content.is_empty() || content == raw {
                String::new()
            } else if matches!(
                mdio_core::effect::line_spec(node.node_type).content_rule,
                mdio_core::effect::ContentRule::NodeSpan
            ) {
                let heading_end = snap.rope.byte_of_line(node.start_line + 1).min(node.byte_range.end);
                if heading_end > node.byte_range.start {
                    snap.rope.byte_slice(node.byte_range.start..heading_end).to_string()
                } else {
                    let r = raw.trim_end_matches('\n');
                    let c = content.trim_end_matches('\n');
                    if r.starts_with(&c) { String::new() }
                    else if r.ends_with(&c) { r[..r.len() - c.len()].to_string() }
                    else { String::new() }
                }
            } else {
                let r = raw.trim_end_matches('\n');
                let c = content.trim_end_matches('\n');
                if r.starts_with(&c) { String::new() }
                else if r.ends_with(&c) { r[..r.len() - c.len()].to_string() }
                else { String::new() }
            };
            if new != old {
                fails += 1;
                if fails <= 5 {
                    eprintln!(
                        "[CTP] container 分歧 {:?} {:?}: new={:?} old={:?} start={} range={:?} pw={} geo={:?} raw={:?} content={:?}",
                        f.file_name().unwrap_or_default(),
                        b.block_type,
                        new,
                        old,
                        node.start_line,
                        node.byte_range,
                        node.peel_width,
                        node.geometry,
                        raw,
                        content
                    );
                }
            }
        }
    }
    eprintln!("[CTP] container 一致性钉：{total_nodes} 节点——分歧 {fails}");
    assert_eq!(fails, 0, "node_container 直取前缀与旧实现分歧 {fails} 处");
}

#[test]
fn prefix_equiv_corpus() {
    let root = std::env::var("MDIO_DUMP_ROOT")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../worktrees/genesis/wt").to_string());
    let mut files = Vec::new();
    collect(Path::new(&root), &mut files);
    files.sort();
    let mut total_lines = 0usize;
    let mut fails = 0usize;
    let mut n_files = 0usize;
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let mut st = State::new(&text);
        let snap = st.snapshot();
        let entries = &snap.frame_log.entries;
        let n_lines = snap.line_len();
        for k in 0..n_lines {
            total_lines += 1;
            let line: String = snap.line_text(k).to_string();
            let stack = replay_stack_before(entries, k as u32);
            let pushes = line_pushes(entries, k as u32);

            // ① markers_on_line（(All, Counted)）：bytes＋edge
            let (cut_m, edge_m) = markers_on_line_ref(&line, &stack, entries, k as u32);
            let c1 = prefix_cut(&line, &stack, &pushes, PrefixKinds::All, PrefixStop::Counted, PrefixLead::SkipWs, PrefixTail::None);
            if c1.bytes != cut_m || c1.edge != edge_m {
                fails += 1;
                if fails <= 5 {
                    eprintln!(
                        "[PEQ] markers_on_line 分歧 {:?}:{}: cut {} vs {} edge {} vs {}",
                        f.file_name().unwrap_or_default(),
                        k,
                        c1.bytes,
                        cut_m,
                        c1.edge,
                        edge_m
                    );
                }
            }

            // ⑥ 生产输入形态对拍（P0-b）：生产站点构造＝链 rebuild_stack_at(line+1)
            // （含本行自开）＋切片 line_entries_at（本行条目）——与独立参照
            // markers_on_line_ref(同链) 逐字段一致。P0 曾因三站传整表逃过 ①
            // （① 用行前栈，只验折叠本体）；本钉直测生产输入构造，再传整表必红。
            let chain_after = mdio_core::block::rebuild_stack_at(entries, k as u32 + 1);
            let slice_at = mdio_core::infer::line_entries_at(entries, k as u32);
            let c6 = prefix_cut(&line, &chain_after, slice_at, PrefixKinds::All, PrefixStop::Counted, PrefixLead::SkipWs, PrefixTail::None);
            let (cut_m2, edge_m2) = markers_on_line_ref(&line, &chain_after, entries, k as u32);
            if c6.bytes != cut_m2 || c6.edge != edge_m2 {
                fails += 1;
                if fails <= 5 {
                    eprintln!(
                        "[PEQ] 生产输入形态分歧 {:?}:{}: cut {} vs {} edge {} vs {}",
                        f.file_name().unwrap_or_default(),
                        k,
                        c6.bytes,
                        cut_m2,
                        c6.edge,
                        edge_m2
                    );
                }
            }

            // ② content_face（(BqOnly, None)＋FinalSkip）：bytes == peel_width
            // （peel_peeled 无链输入——无条件剥 BQ 层到首个非 BQ；钉实证
            // Chain 计数在 fence 内容行会漏剥——站映射修正为 None）
            let stripped = mdio_core::block::peel_peeled(&line, 0).rest_after_bq();
            let peel_width = stripped.as_ptr() as usize - line.as_ptr() as usize;
            let c2 = prefix_cut(&line, &stack, &pushes, PrefixKinds::BqOnly, PrefixStop::None, PrefixLead::SkipWs, PrefixTail::FinalSkip);
            if c2.bytes != peel_width {
                fails += 1;
                if fails <= 5 {
                    eprintln!(
                        "[PEQ] content_face 分歧 {:?}:{} line={:?}: {} vs {}",
                        f.file_name().unwrap_or_default(),
                        k,
                        line,
                        c2.bytes,
                        peel_width
                    );
                }
            }

            // ③ 写面 #8（(All, None)）：bytes == peel(l).last().end（链前奏）
            let (layers_p, _rem) = mdio_core::block::peel(&line);
            let full_cut = layers_p.last().map(|l| l.end).unwrap_or(0);
            let c3 = prefix_cut(&line, &stack, &pushes, PrefixKinds::All, PrefixStop::None, PrefixLead::SkipWs, PrefixTail::None);
            if c3.bytes != full_cut {
                fails += 1;
                if fails <= 5 {
                    eprintln!(
                        "[PEQ] 写面 peel 分歧 {:?}:{}: {} vs {}",
                        f.file_name().unwrap_or_default(),
                        k,
                        c3.bytes,
                        full_cut
                    );
                }
            }

            // ⑥ 位词法一致性钉（决策 1 实证）：行上开层 push 的 consume 位
            // == 该层词法宽，或 == 词法宽 − 层起点前导空白（位=光标帧宽：
            // 列表剥壳预剥 ws 后，位不含该 ws）。两者关系全语料对拍钉住。
            let (layers_p6, _) = mdio_core::block::peel(&line);
            let mut li = 0usize;
            for f in &stack {
                if li < layers_p6.len() && layers_p6[li].kind == f.kind() {
                    li += 1;
                }
            }
            let mut pi = 0usize;
            while li < layers_p6.len() && pi < pushes.len() {
                let l = &layers_p6[li];
                let k = entry_frame(pushes[pi]).kind();
                if k == l.kind {
                    let w = l.end - l.start;
                    let c = entry_consume(pushes[pi]);
                    let ws = line[l.start..]
                        .bytes()
                        .take_while(|&b| b == b' ' || b == b'\t')
                        .count();
                    if c != w && w - c != ws {
                        fails += 1;
                        if fails <= 5 {
                            eprintln!(
                                "[PEQ] 位词法分歧 {:?}:{:?} line={:?} kind={:?}: consume={c} 词法宽={w} ws={ws}",
                                f.file_name().unwrap_or_default(),
                                k,
                                line,
                                l.kind
                            );
                        }
                    }
                    pi += 1;
                    li += 1;
                } else {
                    pi += 1; // 非层 push（CF/Table 等）——跳过
                }
            }

            // ⑤ dedent_continuation_bq 的 keep_indent 剥层（(BqOnly,None,None)）：
            // bytes == len − rest_after_bq_keep_indent().len()（钉背书 #6 走链）
            let kept = mdio_core::block::peel_peeled(&line, 0).rest_after_bq_keep_indent();
            let keep_width = line.len() - kept.len();
            let c5 = prefix_cut(&line, &stack, &pushes, PrefixKinds::BqOnly, PrefixStop::None, PrefixLead::SkipWs, PrefixTail::None);
            if c5.bytes != keep_width {
                fails += 1;
                if fails <= 5 {
                    eprintln!(
                        "[PEQ] keep_indent 分歧 {:?}:{} line={:?}: {} vs {}",
                        f.file_name().unwrap_or_default(),
                        k,
                        line,
                        c5.bytes,
                        keep_width
                    );
                }
            }

            // ④ peel_markers（n 层·仅列表层——BQ 层 break）：PerLayerTrim；
            // peel_markers 遇 BQ 即停（无 5 臂）——前缀含 BQ 的行跳过
            // （退化语义由站迁移淘汰，不比）。开层条目须带真实 consume 位
            // （折叠读位——与 step_layer 同宽——位词法同源）。
            let n_layers = layers_p.len();
            for n in 0..=n_layers.min(4) {
                if layers_p[..n].iter().any(|l| l.kind == FrameKind::BlockQuote) {
                    continue;
                }
                let stack_n: Vec<BlockFrame> = Vec::new();
                let pushes_n: Vec<u64> = (0..n)
                    .map(|j| {
                        let l = &layers_p[j];
                        mdio_core::block::with_consume(
                            BlockFrame::new(l.kind, 0, 0).0 as u64,
                            l.end - l.start,
                        )
                    })
                    .collect();
                let c4 = prefix_cut(&line, &stack_n, &pushes_n, PrefixKinds::All, PrefixStop::Counted, PrefixLead::SkipWs, PrefixTail::PerLayerTrim);
                let want = line.len() - mdio_core::block::peel_markers(&line, n).len();
                if c4.bytes != want {
                    fails += 1;
                    if fails <= 5 {
                        eprintln!(
                            "[PEQ] peel_markers 分歧 {:?}:{} n={n} line={:?}: {} vs {}",
                            f.file_name().unwrap_or_default(),
                            k,
                            line,
                            c4.bytes,
                            want
                        );
                    }
                }
            }
        }
        n_files += 1;
    }
    eprintln!(
        "[PEQ] prefix 等价钉：{} 文件 {} 行——分歧 {}",
        n_files, total_lines, fails
    );
    assert_eq!(fails, 0, "prefix_cut 与既有实现分歧 {fails} 处");
}

/// 分歧形状可达性观察钉（#9 前置·量定义钉死配套）：row_pushes_at（非 Pop
/// 计数）与严格开层数（非 Pop 且非 body_mut）在 body_mut∧push 同行时差 1。
/// 全语料实测该形状**可达**（`> - x` 型 Composite primary=ContinueAs+nested）：
/// 旧「不可达」假设证伪——严格源（entries_pushes_at）语义安全由 lib 全量
/// （ic_reported_shapes_match_cm 等）+prefix 等价钉承担；本钉只上报可达数，
/// 防后人重新假设不可达。
#[test]
fn body_mut_push_relation_pin() {
    let root = std::env::var("MDIO_DUMP_ROOT")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../worktrees/genesis/wt").to_string());
    let mut files = Vec::new();
    collect(Path::new(&root), &mut files);
    files.sort();
    let mut total_lines = 0usize;
    let mut cooccur = 0usize;
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let mut st = State::new(&text);
        let snap = st.snapshot();
        let entries = &snap.frame_log.entries;
        let n_lines = snap.line_len();
        for k in 0..n_lines {
            total_lines += 1;
            let mut bm = 0usize;
            let mut push = 0usize;
            for &e in entries {
                if entry_line(e) != k as u32 {
                    continue;
                }
                if entry_is_body_mut(e) {
                    bm += 1;
                } else if !entry_is_pop(e) {
                    push += 1;
                }
            }
            if bm > 0 && push > 0 {
                cooccur += 1;
            }
        }
    }
    eprintln!(
        "[RCH] 分歧形状可达性钉：{} 行——body_mut∧push 同行 {} 处",
        total_lines, cooccur
    );
}

/// P0-b 生产站直呼钉（CTP 参照独立化的实质）：语料全部「容器内 IC 行」——
/// 生产站 ic_line_prefix_cut（含调用点输入形态）与独立参照 markers_on_line_ref
/// （链=rebuild_stack_at(line+1)）逐字段一致。P0 曾因三站传整表逃过所有门；
/// 本钉直呼生产站，输入形态错了必红。
#[test]
fn ic_cut_input_form_pin() {
    let root = std::env::var("MDIO_DUMP_ROOT")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../worktrees/genesis/wt").to_string());
    let mut files = Vec::new();
    collect(Path::new(&root), &mut files);
    files.sort();
    let mut total_ic = 0usize;
    let mut fails = 0usize;
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let mut st = State::new(&text);
        let snap = st.snapshot();
        let entries = &snap.frame_log.entries;
        let np = mdio_core::infer::NodeProj::new(&snap.rope, &snap.lines, entries);
        let n_lines = snap.line_len();
        for k in 0..n_lines {
            let chain_after = mdio_core::block::rebuild_stack_at(entries, k as u32 + 1);
            if chain_after.is_empty() {
                continue; // 无容器——生产站同样早退 (0,0)
            }
            if snap.line_type_at(k) != mdio_core::block::BlockType::IndentedCode {
                continue;
            }
            total_ic += 1;
            let line = snap.rope.line(k).to_string();
            let (cut_ref, edge_ref) = markers_on_line_ref(&line, &chain_after, entries, k as u32);
            let (cut_prod, indent_prod) = np.ic_line_prefix_cut(k);
            let edge_prod = indent_prod.saturating_sub(mdio_core::effect::geo::LIST_CONTENT_MAX);
            if cut_prod != cut_ref || edge_prod != edge_ref {
                fails += 1;
                if fails <= 5 {
                    eprintln!(
                        "[ICF] 生产站分歧 {:?}:{} line={:?}: cut {} vs {} edge {} vs {}",
                        f.file_name().unwrap_or_default(),
                        k,
                        line,
                        cut_prod,
                        cut_ref,
                        edge_prod,
                        edge_ref
                    );
                }
            }
        }
    }
    eprintln!(
        "[ICF] 容器内 IC 行输入形态钉：{total_ic} 行——分歧 {fails}"
    );
    assert_eq!(fails, 0, "ic_line_prefix_cut 与独立参照分歧 {fails} 处");
}
