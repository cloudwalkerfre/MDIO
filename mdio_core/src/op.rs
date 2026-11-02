//! MDIO OP — read/write/find/tree command semantics.
//!
//! Owns MDIO command behavior that consumes selector evidence and projection
//! snapshots. Read-side commands operate over an already-loaded Snapshot.
//! Write planning produces edit plans for the outer transaction layer to apply.

use std::sync::OnceLock;

use regex;
use serde::Serialize;
use serde_json;
use tiktoken_rs::CoreBPE;

use crate::block::{strip_list_marker, trim_start, BlockType, BlockNode, is_header, header_level};
use crate::projection::Snapshot;
use crate::selector::{NodeCategory, node_category};
// reverse_pathfinding 使用 new_infer LineView。
use crate::block::BlockProj;
use crate::selector::{Step, head_exists, is_simple_selector, extract_head_parts};
use crate::telemetry_span;


// Tree / Report Types


#[derive(Serialize, Default, Clone)]
pub struct TreeSummary {
    pub node_type: String,
    pub name: String,
    pub chars: usize,
    pub tokens: usize,
    pub lines: usize,
    pub children: Vec<TreeSummary>,
    pub anon_bullets: usize,
    pub paragraphs: usize,
    pub code_blocks: usize,
}

#[derive(Serialize)]
pub struct TreeResponse {
    pub file_chars: usize,
    pub file_tokens: usize,
    pub file_lines: usize,
    pub nodes: Vec<TreeSummary>,
}


// HS Tree -> TreeSummary helpers


fn tree_node_byte_range(snap: &Snapshot, start: usize, end: usize) -> std::ops::Range<usize> {
    let s = snap.byte_of_line(start);
    let e = if end < snap.line_len() {
        snap.byte_of_line(end)
    } else {
        snap.byte_len()
    };
    s..e
}

fn tree_node_name(
    snap: &Snapshot,
    face: &crate::infer::NodeProj<'_>,
    nd: &crate::infer::TreeNode,
) -> (String, String) {
    let bt = nd.block_type;
    if is_header(bt) {
        let lvl = header_level(bt).unwrap_or(1);
        let name = Snapshot::with_line_str(snap.line_text(nd.start as usize), |s| {
            let full = crate::block::str_view(crate::block::strip_bq(s.as_bytes(), false));
            crate::block::extract_heading_text(full).trim_end().to_string()
        });
        ("head".into(), format!("{} {}", crate::effect::atx_mark_text(lvl), name))
    } else if bt == BlockType::Bullet || bt == BlockType::Checklist {
        let tmp = snap.build_node(nd.start as usize, nd.end as usize, bt, nd.loose());
        let k = face.list_key(&tmp).map(|k| format!(":{}", k)).unwrap_or_else(|| "-".into());
        ("key".into(), k)
    } else if bt == BlockType::OrderedListItem {
        let name = Snapshot::with_line_str(snap.line_text(nd.start as usize), |s| {
            let trimmed = strip_list_marker(s, bt);
            let prefix_len = s.len() - trimmed.len();
            let marker = &s[..prefix_len].trim();
            format!(":{}", marker.trim_end_matches(&['.', ')'][..]))
        });
        ("key".into(), name)
    } else if bt == BlockType::Blockquote {
        ("bq".into(), "blockquote".into())
    } else if bt == BlockType::CodeFenceOpen {
        let lang = Snapshot::with_line_str(snap.line_text(nd.start as usize), |s| {
            let trimmed = trim_start(s);
            let info = crate::block::extract_fence_lang(trimmed).trim();
            if info.is_empty() { "code".to_string() } else { format!("code:{}", info) }
        });
        ("code".into(), lang)
    } else if bt == BlockType::Paragraph {
        ("p".into(), "paragraph".into())
    } else {
        ("other".into(), "Selection".into())
    }
}

/// 递归汇总 AttrTreeNew 的 children，返回 (展示节点, 匿名 bullet 数, paragraph 数, code 数)。
/// 列表容器壳会被扁平化：不生成 bl/od 节点，直接展开 item leaves。
fn summarize_children(
    snap: &Snapshot,
    face: &crate::infer::NodeProj<'_>,
    tree: &crate::infer::AttrTreeNew,
    parent: usize,
    depth: usize,
    max_depth: usize,
    detail: usize,
) -> (Vec<TreeSummary>, usize, usize, usize) {
    let mut children = Vec::new();
    let mut anon = 0usize;
    let mut paras = 0usize;
    let mut codes = 0usize;
    for &c in tree.nodes[parent].children_of(tree) {
        let nd = &tree.nodes[c as usize];
        // 列表容器壳：扁平化
        if let Some(fk) = nd.kind {
            if matches!(fk, crate::block::FrameKind::BulletList | crate::block::FrameKind::OrderedList) {
                let (sub, a, p, cb) = summarize_children(snap, face, tree, c as usize, depth, max_depth, detail);
                children.extend(sub);
                anon += a;
                paras += p;
                codes += cb;
                continue;
            }
        }
        // 代码围栏：计数，不建节点
        if nd.block_type == BlockType::CodeFenceOpen {
            if detail >= 2 { codes += 1; }
            continue;
        }
        // 段落：计数，不建节点（除非作为顶层选中节点，由调用方处理）
        if nd.block_type == BlockType::Paragraph {
            if detail >= 2 { paras += 1; }
            continue;
        }
        // 标题：建 head 节点
        if is_header(nd.block_type) && nd.kind.is_none() && nd.blk_idx().is_none() {
            if depth < max_depth {
                let (nt, name) = tree_node_name(snap, face, nd);
                let mut node = TreeSummary { node_type: nt, name, ..Default::default() };
                let range = tree_node_byte_range(snap, nd.start as usize, nd.end as usize);
                fill_stats_by_range(snap, &mut node, range, nd.end as usize - nd.start as usize);
                let (sub, a, p, cb) = summarize_children(snap, face, tree, c as usize, depth + 1, max_depth, detail);
                node.children = sub;
                node.anon_bullets = a;
                node.paragraphs = p;
                node.code_blocks = cb;
                children.push(node);
            }
            continue;
        }
        // BQ/其他容器壳：建节点
        if nd.kind.is_some() {
            if detail >= 1 {
                let (nt, name) = tree_node_name(snap, face, nd);
                let mut node = TreeSummary { node_type: nt, name, ..Default::default() };
                let range = tree_node_byte_range(snap, nd.start as usize, nd.end as usize);
                fill_stats_by_range(snap, &mut node, range, nd.end as usize - nd.start as usize);
                let (sub, a, p, cb) = summarize_children(snap, face, tree, c as usize, depth + 1, max_depth, detail);
                node.children = sub;
                node.anon_bullets = a;
                node.paragraphs = p;
                node.code_blocks = cb;
                children.push(node);
            }
            continue;
        }
        // item leaf：key 节点或匿名计数
        if nd.is_item() {
            let (nt, name) = tree_node_name(snap, face, nd);
            if nt == "key" && name != "-" {
                if detail >= 1 {
                    let mut node = TreeSummary { node_type: nt, name, ..Default::default() };
                    let range = tree_node_byte_range(snap, nd.start as usize, nd.end as usize);
                    fill_stats_by_range(snap, &mut node, range, nd.end as usize - nd.start as usize);
                    let (sub, a, p, cb) = summarize_children(snap, face, tree, c as usize, depth + 1, max_depth, detail);
                    node.children = sub;
                    node.anon_bullets = a;
                    node.paragraphs = p;
                    node.code_blocks = cb;
                    children.push(node);
                }
            } else if detail >= 2 {
                anon += 1;
            }
            continue;
        }
        // 其他叶子：忽略
    }
    (children, anon, paras, codes)
}

fn tree_node_to_summary(
    snap: &Snapshot,
    face: &crate::infer::NodeProj<'_>,
    tree: &crate::infer::AttrTreeNew,
    idx: usize,
    max_depth: usize,
    detail: usize,
) -> TreeSummary {
    let nd = &tree.nodes[idx];
    let (nt, name) = tree_node_name(snap, face, nd);
    let mut node = TreeSummary { node_type: nt, name, ..Default::default() };
    let range = tree_node_byte_range(snap, nd.start as usize, nd.end as usize);
    fill_stats_by_range(snap, &mut node, range, nd.end as usize - nd.start as usize);
    let (sub, a, p, cb) = summarize_children(snap, face, tree, idx, 0, max_depth, detail);
    node.children = sub;
    node.anon_bullets = a;
    node.paragraphs = p;
    node.code_blocks = cb;
    node
}

fn find_tree_node(tree: &crate::infer::AttrTreeNew, node: &BlockNode) -> Option<usize> {
    tree.nodes.iter().enumerate().find(|(_, tn)| {
        tn.start as usize == node.start_line
            && tn.end as usize == node.end_line
            && tn.block_type == node.node_type
    }).map(|(i, _)| i)
}


// Token Encoder


pub(crate) fn get_bpe() -> &'static CoreBPE {
    static BPE: OnceLock<CoreBPE> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().unwrap())
}


// Scaffold / Write Helpers


pub struct ScaffoldInsert {
    pub line: usize,
    pub text: String,
}

/// Offset ATX # levels in payload text, code blocks unaffected.
pub fn process_payload(text: &str, target_depth: u8) -> String {
    let mut result = String::with_capacity(text.len());
    let mut in_fence = false;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if crate::block::line_geometry(BlockType::CodeFenceOpen, trimmed.as_bytes()).is_some() {
            in_fence = !in_fence;
            result.push_str(line);
            result.push('\n');
            continue;
        }
        if in_fence {
            result.push_str(line);
            result.push('\n');
            continue;
        }

        let adjusted = shift_atx_heading(line, target_depth);
        result.push_str(&adjusted);
        result.push('\n');
    }
    result
}

use std::ops::Range;

pub(crate) fn reverse_pathfinding(snap: &Snapshot, start_line: usize) -> String {
    reverse_pathfinding_local(snap, start_line)
}

/// 全量 oracle：P2b 判官参照（仅测试）。
#[cfg(test)]
fn reverse_pathfinding_full_oracle(snap: &Snapshot, start_line: usize) -> String {
    if start_line >= snap.line_len() { return String::new(); }
    let n = snap.lines.len() as u32;
    let seg = crate::cache::Segment::build(snap, 0, n as usize);
    let spans = crate::infer::LineView::with_arrays(&seg.records, &seg.blocks,
        &snap.frame_log.entries, &snap.lines, &snap.rope, (0, n), seg.view.clone(), &seg.arena);
    let face = snap.node_proj();
    reverse_pathfinding_impl(snap, &spans, &face, start_line)
}

/// 目标行的局部解析 zone：depth-0 祖先起，容器/块闭包止于目标块尾。
/// 不解析 zone 前的 prefix，也不解析 zone 后的 suffix。
fn local_zone_for_line(snap: &Snapshot, line: usize) -> (usize, usize) {
    let heads = crate::infer::header_lines(&snap.lines, &snap.rope, 0, line);
    let has_container = snap
        .frame_log
        .entries
        .iter()
        .any(|e| !crate::block::entry_is_body_mut(*e));
    local_zone_for_line_facts(snap, line, &heads, has_container)
}

/// local_zone_for_line 的事实注入版（run_find 预扫描复用：heads=全量表、
/// has_container 一次算——逐命中不再重扫标题与 FL）。
fn local_zone_for_line_facts(
    snap: &Snapshot,
    line: usize,
    heads: &[(usize, u8)],
    has_container: bool,
) -> (usize, usize) {
    let n = snap.line_len();
    if line >= n {
        return (0, n);
    }
    // 有标题：从“最近前驱标题”开始解析（只进当前章节）。
    // 无标题且无容器：从目标 depth-0 行开始，prefix sibling 计数用行级 PL。
    // 无标题但有容器：从 0 开始，优先保证 resolved 类型稳定。
    let prefix_last = heads[..heads.partition_point(|&(p, _)| p < line)].last();
    let zs = if let Some(&(p, _)) = prefix_last {
        p.min(line)
    } else if has_container {
        0
    } else {
        snap.snap_to_depth0(line) as usize
    };
    let ce0 = (line + 1).min(n);
    let (_, ce) = crate::infer::expand_zone_to_containers_fl(
        &snap.frame_log.entries,
        &snap.lines,
        &snap.open_depth,
        n,
        zs,
        ce0,
    );
    let (zs, ce) = crate::infer::expand_zone_to_blocks_fl(
        &snap.frame_log.entries,
        &snap.lines,
        &snap.open_depth,
        n,
        zs,
        ce,
    );
    let (zs, ce) = crate::cache::zone_align_to_blocks(snap, zs, ce.min(n));
    (zs, ce)
}

/// 局部路径实现：局部 LineView 负责最近结构父块，行级标题表负责祖先标题。
fn reverse_pathfinding_local(snap: &Snapshot, start_line: usize) -> String {
    let heads = crate::infer::header_lines(&snap.lines, &snap.rope, 0, start_line);
    let has_container = snap
        .frame_log
        .entries
        .iter()
        .any(|e| !crate::block::entry_is_body_mut(*e));
    reverse_pathfinding_local_facts(snap, start_line, &heads, has_container)
}

/// reverse_pathfinding_local 的事实注入版（run_find 预扫描复用：heads=全量表、
/// has_container 一次算——逐命中不再重扫标题与 FL）。heads 按 start_line 前缀切片，
/// 与 header_lines(0, start_line) 逐条同域。
fn reverse_pathfinding_local_facts(
    snap: &Snapshot,
    start_line: usize,
    heads: &[(usize, u8)],
    has_container: bool,
) -> String {
    if start_line >= snap.line_len() { return String::new(); }
    let heads = &heads[..heads.partition_point(|&(p, _)| p < start_line)];
    let (zs, ce) = local_zone_for_line_facts(snap, start_line, heads, has_container);
    // 共享路由（修复 P2a）：编辑链存活的 zone 段走 kernel 三域拼接（零解析/δ 平移），
    // 链断/跨 mid/无 slot 回落裸建——内容等价由 prc_kernel_op_equivalence_gates 与
    // reverse_pathfinding_local_matches_full_oracle 两条钉背书。
    let seg: std::sync::Arc<crate::cache::Segment> =
        crate::selector::shared_structure_for_range(snap, zs, ce).unwrap_or_else(|| {
            std::sync::Arc::new(crate::cache::Segment::build(snap, zs, ce))
        });
    let spans = crate::infer::LineView::with_arrays(
        &seg.records,
        &seg.blocks,
        &snap.frame_log.entries,
        &snap.lines,
        &snap.rope,
        (seg.start_line as u32, seg.end_line as u32),
        seg.view.clone(),
        &seg.arena,
    );
    let face = snap.node_proj();
    let face = &face;
    let is_head = |line: usize| -> bool {
        heads.binary_search_by_key(&line, |&(p, _)| p).is_ok()
    };
    let resolved = |line: usize| -> BlockType {
        if line >= zs && line < ce {
            spans.resolved_type_at(line)
        } else {
            snap.line_type_at(line)
        }
    };
    let mut parts = Vec::new();
    let hit_info = resolved(start_line);
    let mut current_level = 7u8;
    let mut struct_parent_idx: Option<usize> = None;
    let mut struct_parent_type: Option<BlockType> = None;
    let mut peek = start_line;
    loop {
        let t = resolved(peek);
        if node_category(Some(&spans), peek, t) == NodeCategory::Block {
            struct_parent_idx = Some(peek);
            struct_parent_type = Some(t);
            break;
        }
        if is_header(t) { break; }
        if peek == zs || peek == 0 { break; }
        peek -= 1;
    }
    if let (Some(idx), Some(nt)) = (struct_parent_idx, struct_parent_type) {
        if nt == BlockType::Bullet || nt == BlockType::Checklist {
            if let Some(key) = extract_inline_key_with(snap, face, idx, nt) {
                parts.push(format!(":{}", key));
            } else {
                let mut sibling_count = 0;
                let mut s_peek = idx;
                while s_peek > 0 {
                    s_peek -= 1;
                    let t = resolved(s_peek);
                    if t == BlockType::Bullet || t == BlockType::Checklist {
                        sibling_count += 1;
                    } else if is_head(s_peek) { break; }
                }
                parts.push(format!("[{}]", sibling_count));
            }
        } else if nt == BlockType::OrderedListItem {
            let mut sibling_count = 0;
            let mut s_peek = idx;
            while s_peek > 0 {
                s_peek -= 1;
                let t = resolved(s_peek);
                if is_head(s_peek) { break; }
                if t == BlockType::OrderedListItem { sibling_count += 1; }
            }
            parts.push(format!(".*[ol][{}]", sibling_count));
        } else if nt == BlockType::Blockquote {
            let mut sibling_count = 0;
            let mut s_peek = idx;
            while s_peek > 0 {
                s_peek -= 1;
                let t = resolved(s_peek);
                if is_head(s_peek) { break; }
                if t == BlockType::Blockquote { sibling_count += 1; }
            }
            parts.push(format!(".*[bq][{}]", sibling_count));
        } else if nt == BlockType::CodeFenceOpen {
            let mut sibling_count = 0;
            let mut s_peek = idx;
            while s_peek > 0 {
                s_peek -= 1;
                let t = resolved(s_peek);
                if is_head(s_peek) { break; }
                if t == BlockType::CodeFenceOpen { sibling_count += 1; }
            }
            parts.push(format!(".*[code][{}]", sibling_count));
        } else if nt == BlockType::Paragraph {
            let mut sibling_count = 0;
            let mut s_peek = idx;
            while s_peek > 0 {
                s_peek -= 1;
                let t = resolved(s_peek);
                if is_head(s_peek) { break; }
                if t == BlockType::Paragraph { sibling_count += 1; }
            }
            parts.push(format!(".*[p][{}]", sibling_count));
        }
    }
    if is_header(hit_info) {
        current_level = header_level(hit_info).unwrap_or(7);
        if let Some(name) = extract_inline_head_resolved(snap, start_line, hit_info) {
            parts.push(name);
        }
    }
    for &(i, lvl) in heads.iter().rev() {
        if lvl < current_level {
            let mut t = resolved(i);
            if !is_header(t) {
                // 行级 header_lines 识别出的 setext 标题：PL 首行是
                // Paragraph，块级类型才是 HsHead1/2。
                t = if lvl == 1 {
                    BlockType::HsHead1
                } else {
                    BlockType::HsHead2
                };
            }
            if let Some(name) = extract_inline_head_resolved(snap, i, t) {
                parts.push(name);
            }
            current_level = lvl;
            if lvl == 1 { break; }
        }
    }
    parts.reverse();
    let mut res = String::new();
    for p in parts {
        if p.starts_with(':') || p.starts_with('[') {
            res.push_str(&p);
        } else {
            if !res.is_empty() { res.push('.'); }
            res.push_str(&p);
        }
    }
    res
}

fn reverse_pathfinding_impl(
    snap: &Snapshot,
    spans: &crate::infer::LineView<'_>,
    face: &crate::infer::NodeProj<'_>,
    start_line: usize,
) -> String {
    let mut parts = Vec::new();
    let hit_info = spans.resolved_type_at(start_line);
    let mut current_level = 7u8;
    let mut struct_parent_idx: Option<usize> = None;
    let mut struct_parent_type: Option<BlockType> = None;
    let mut peek = start_line;
    // 逐行回溯（非 node_end 推进）——comment 需帧判定（内容行 Skip——
    // 与 walk 一致）。LineView / NodeProj 由调用方复用。
    loop {
        let t = spans.resolved_type_at(peek);
        if node_category(Some(spans), peek, t) == NodeCategory::Block {
            struct_parent_idx = Some(peek);
            struct_parent_type = Some(t);
            break;
        }
        if is_header(t) { break; }
        if peek == 0 { break; }
        peek -= 1;
    }
    if let (Some(idx), Some(nt)) = (struct_parent_idx, struct_parent_type) {
        if nt == BlockType::Bullet || nt == BlockType::Checklist {
            if let Some(key) = extract_inline_key_with(snap, face, idx, nt) {
                parts.push(format!(":{}", key));
            } else {
                let mut sibling_count = 0;
                let mut s_peek = idx;
                while s_peek > 0 {
                    s_peek -= 1;
                    let t = spans.resolved_type_at(s_peek);
                    if t == BlockType::Bullet || t == BlockType::Checklist {
                        sibling_count += 1;
                    } else if is_header(t) { break; }
                }
                parts.push(format!("[{}]", sibling_count));
            }
        } else if nt == BlockType::OrderedListItem {
            let mut sibling_count = 0;
            let mut s_peek = idx;
            while s_peek > 0 {
                s_peek -= 1;
                let t = spans.resolved_type_at(s_peek);
                if is_header(t) { break; }
                if t == BlockType::OrderedListItem { sibling_count += 1; }
            }
            parts.push(format!(".*[ol][{}]", sibling_count));
        } else if nt == BlockType::Blockquote {
            let mut sibling_count = 0;
            let mut s_peek = idx;
            while s_peek > 0 {
                s_peek -= 1;
                let t = spans.resolved_type_at(s_peek);
                if is_header(t) { break; }
                if t == BlockType::Blockquote { sibling_count += 1; }
            }
            parts.push(format!(".*[bq][{}]", sibling_count));
        } else if nt == BlockType::CodeFenceOpen {
            let mut sibling_count = 0;
            let mut s_peek = idx;
            while s_peek > 0 {
                s_peek -= 1;
                let t = spans.resolved_type_at(s_peek);
                if is_header(t) { break; }
                if t == BlockType::CodeFenceOpen { sibling_count += 1; }
            }
            parts.push(format!(".*[code][{}]", sibling_count));
        } else if nt == BlockType::Paragraph {
            let mut sibling_count = 0;
            let mut s_peek = idx;
            while s_peek > 0 {
                s_peek -= 1;
                let t = spans.resolved_type_at(s_peek);
                if is_header(t) { break; }
                if t == BlockType::Paragraph { sibling_count += 1; }
            }
            parts.push(format!(".*[p][{}]", sibling_count));
        }
    }
    if is_header(hit_info) {
        current_level = header_level(hit_info).unwrap_or(7);
        if let Some(name) = extract_inline_head_resolved(snap, start_line, hit_info) {
            parts.push(name);
        }
    }
    for i in (0..start_line).rev() {
        let t = spans.resolved_type_at(i);
        if is_header(t) {
            let lvl = header_level(t).unwrap_or(7);
            if lvl < current_level {
                if let Some(name) = extract_inline_head_resolved(snap, i, t) {
                    parts.push(name);
                }
                current_level = lvl;
                if lvl == 1 { break; }
            }
        }
    }
    parts.reverse();
    let mut res = String::new();
    for p in parts {
        if p.starts_with(':') || p.starts_with('[') {
            res.push_str(&p);
        } else {
            if !res.is_empty() { res.push('.'); }
            res.push_str(&p);
        }
    }
    res
}

/// 单遍空白归一：\r\n→' '、\r→' '、\n→' '——与 replace("\r\n")+replace("\n")+replace("\r")
/// 逐字符等价（省 2 次分配；\n\r 仍得两个空格，同原序）。
fn normalize_snippet_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => {
                out.push(' ');
                if it.peek() == Some(&'\n') {
                    it.next();
                }
            }
            '\n' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

pub fn run_find(snap: &Snapshot, steps: &[crate::selector::Step], query_configs: &[(String, bool, bool)], ctx_b: usize, ctx_a: usize) -> Vec<String> {
    let mut results = Vec::new();
    if query_configs.is_empty() {
        results.push("- mdio_error: [find] missing queries".to_string());
        return results;
    }
    let compiled: Vec<Result<regex::Regex, (String, String)>> = query_configs.iter().map(|(query_raw, is_regex, ignore_case)| {
        let pattern = if *is_regex { query_raw.clone() } else { regex::escape(query_raw) };
        regex::RegexBuilder::new(&pattern)
            .case_insensitive(*ignore_case)
            .build()
            .map_err(|e| (query_raw.clone(), e.to_string()))
    }).collect();
    let target_ranges = crate::selector::select_ranges(snap, steps);
    // 预扫描一次化（修复：逐命中三扫标题+全 FL 扫 → 一次建表+逐命中二分）：
    // heads_all 与 has_container 是快照的确定性事实，逐命中重算语义不变。
    let face = snap.node_proj();
    let heads_all = crate::infer::header_lines(&snap.lines, &snap.rope, 0, snap.line_len());
    let has_container = snap
        .frame_log
        .entries
        .iter()
        .any(|e| !crate::block::entry_is_body_mut(*e));
    // 同 hit_line 的路径字符串是纯函数——命中内多匹配共享一次回溯（纯 memo）。
    let mut path_cache: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let mut scratch_text: Vec<u8> = Vec::new();
    let mut scratch_snip: Vec<u8> = Vec::new();
    for re_res in &compiled {
        let re = match re_res {
            Ok(r) => r,
            Err((raw, e)) => {
                results.push(format!("- mdio_error: [find] invalid regex pattern '{}' — {}", raw, e));
                continue;
            }
        };
        for range in &target_ranges {
            let text = face.byte_range_str_borrow(range.clone(), &mut scratch_text);
            for mat in re.find_iter(text) {
                let abs_start = range.start + mat.start();
                let abs_end = range.start + mat.end();
                let hit_line = snap.line_of_byte(abs_start);
                let path_str = match path_cache.get(&hit_line) {
                    Some(p) => p.clone(),
                    None => {
                        let p = reverse_pathfinding_local_facts(snap, hit_line, &heads_all, has_container);
                        path_cache.insert(hit_line, p.clone());
                        p
                    }
                };
                let mut safe_start = abs_start.saturating_sub(ctx_b);
                while safe_start > 0 && !snap.is_char_boundary(safe_start) {
                    safe_start -= 1;
                }
                let mut safe_end = (abs_end + ctx_a).min(snap.byte_len());
                while safe_end < snap.byte_len() && !snap.is_char_boundary(safe_end) {
                    safe_end += 1;
                }
                let snippet_raw = face.byte_range_str_borrow(safe_start..safe_end, &mut scratch_snip);
                let clean_snippet = normalize_snippet_ws(snippet_raw);
                let final_snippet = format!("{}{}{}",
                    if safe_start > range.start { "..." } else { "" },
                    clean_snippet.trim(),
                    if safe_end < range.end { "..." } else { "" }
                );
                let json_snippet = serde_json::to_string(&final_snippet).unwrap_or_else(|_| "\"<Unserializable Snippet>\"".to_string());
                results.push(format!("- path: {}, {}", path_str, json_snippet));
            }
        }
    }
    results
}

pub fn run_tree(snap: &Snapshot, steps: &[Step], max_depth: usize, detail_level: usize) -> String {
    let _span = telemetry_span!("run_tree", "step_count" => steps.len());
    if snap.line_len() == 0 {
        let response = TreeResponse { file_chars: 0, file_tokens: 0, file_lines: 0, nodes: vec![] };
        return serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
    }
    let nodes = crate::selector::select_nodes(snap, steps);
    // A5 全量树归口(#39):HS 全量树查格(到达即建)——格内段 + 树一次
    // 构建、逐快照共享;视图由格段经 with_arrays 重建(借用形态)。
    let cell = snap.tree_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
    let seg = &cell.seg;
    let tree = &cell.tree;
    let face = snap.node_proj();
    let mut results = Vec::new();
    for node in nodes {
        if node.node_type == BlockType::Blank {
            let mut root_node = TreeSummary { node_type: "root".to_string(), name: "root".to_string(), ..Default::default() };
            fill_stats_by_range(snap, &mut root_node, node.byte_range.clone(), node.end_line - node.start_line);
            let mut children = Vec::new();
            let mut a_s = 0usize;
            let mut p = 0usize;
            let mut c_b = 0usize;
            for &r in &tree.roots {
                let summary = tree_node_to_summary(snap, &face, &tree, r as usize, max_depth, detail_level);
                a_s += summary.anon_bullets;
                p += summary.paragraphs;
                c_b += summary.code_blocks;
                children.push(summary);
            }
            root_node.children = children;
            root_node.anon_bullets = a_s;
            root_node.paragraphs = p;
            root_node.code_blocks = c_b;
            results.push(root_node);
        } else if let Some(idx) = find_tree_node(&tree, &node) {
            results.push(tree_node_to_summary(snap, &face, &tree, idx, max_depth, detail_level));
        } else {
            let nt = node.node_type;
            let (node_type_str, name_str) = if is_header(nt) {
                let lvl = header_level(nt).unwrap_or(1);
                ("head".to_string(), format!("{} {}", crate::effect::atx_mark_text(lvl), extract_inline_head(snap, node.start_line).unwrap_or_default()))
            } else if nt == BlockType::Bullet || nt == BlockType::Checklist {
                let k = extract_inline_key(snap, node.start_line, nt).map(|k| format!(":{}", k)).unwrap_or_else(|| "-".to_string());
                ("key".to_string(), k)
            } else if nt == BlockType::OrderedListItem {
                let order_name = Snapshot::with_line_str(snap.line_text(node.start_line), |s| {
                    let trimmed = strip_list_marker(s, nt);
                    let prefix_len = s.len() - trimmed.len();
                    let marker = &s[..prefix_len].trim();
                    format!(":{}", marker.trim_end_matches(&['.', ')'][..]))
                });
                ("key".to_string(), order_name)
            } else if nt == BlockType::Blockquote {
                ("bq".to_string(), "blockquote".to_string())
            } else if nt == BlockType::CodeFenceOpen {
                let lang = Snapshot::with_line_str(snap.line_text(node.start_line), |s| {
                    let trimmed = trim_start(s);
                    let info = crate::block::extract_fence_lang(trimmed).trim();
                    if info.is_empty() { "code".to_string() } else { format!("code:{}", info) }
                });
                ("code".to_string(), lang)
            } else if nt == BlockType::Paragraph {
                ("p".to_string(), "paragraph".to_string())
            } else {
                ("other".to_string(), "Selection".to_string())
            };
            let mut top_node = TreeSummary { node_type: node_type_str, name: name_str, ..Default::default() };
            fill_stats_by_range(snap, &mut top_node, node.byte_range.clone(), node.end_line - node.start_line);
            results.push(top_node);
        }
    }
    let file_text = snap.to_string();
    let bpe = get_bpe();
    let response = TreeResponse {
        file_chars: file_text.chars().count(),
        file_tokens: bpe.encode_with_special_tokens(&file_text).len(),
        file_lines: snap.line_len(),
        nodes: results,
    };
    serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string())
}

pub fn extract_scaffold_key(steps: &[Step]) -> Option<&str> {
    use crate::selector::{FilterExpr, CmpOp, TypeName, SingleSlice, Value};
    let len = steps.len();
    if len == 0 { return None; }
    // 融合后: 最后一步是 TraversalFilter, 检查其内含的 CMP(.tag)
    if let Step::TraversalFilter { filters, .. } = steps.last()? {
        let has_typecheck = filters.iter().any(|f| {
            matches!(f, FilterExpr::TypeCheck(TypeName::Bullet))
        });
        if !has_typecheck { return None; }
        for f in filters {
            if let FilterExpr::Cmp { proj, op, value }
                = f
            {
                if *proj == crate::block::BlockProj::Tag && *op == CmpOp::Eq {
                    return match value { Value::String(s) => Some(s.as_str()), _ => None };
                }
            }
        }
        return None;
    }
    // 未融合: 最后是 Slice(Index(N)), 倒数第二是 Filter(CMP(.tag))
    if len < 3 { return None; }
    // 最后一步：Slice(Index(_))
    match steps.last()? {
        Step::Slice(specs) if matches!(specs.first(), Some(SingleSlice::Index(_))) => {}
        _ => return None,
    }
    // 倒数第二步：Filter(Cmp { proj: Tag, op: Eq, value: String(key) })
    match steps.get(len - 2)? {
        Step::Filter(FilterExpr::Cmp { proj, op, value })
            if *proj == crate::block::BlockProj::Tag
            && *op == CmpOp::Eq =>
        {
            let key = match value { Value::String(s) => s.as_str(), _ => return None };
            // 确认前面有 TypeCheck(Bullet) + Wildcard/RecursiveWildcard
            let has_typecheck = steps[..len - 2].iter().any(|s| {
                matches!(s, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)))
            });
            if has_typecheck { Some(key) } else { None }
        }
        _ => None,
    }
}

/// 写面 scaffold 续行缩进单源（值取几何阈值表 LIST_CONTENT_MAX）。
fn scaffold_continuation_indent() -> String {
    " ".repeat(crate::effect::geo::LIST_CONTENT_MAX)
}

/// multiline payload 续行缩进（bullet key 内容块语义）。
fn indent_scaffold_payload(payload: &str) -> String {
    if payload.contains('\n') {
        let indent = scaffold_continuation_indent();
        payload.replace('\n', &format!("\n{}", indent))
    } else {
        payload.to_string()
    }
}

pub fn build_scaffold(snap: &Snapshot, steps: &[Step], operator: &str, payload: &str, existing_depth: usize) -> Option<ScaffoldInsert> {
    if operator != "+>" && operator != "/>" { return None; }
    let head_parts = extract_head_parts(steps);
    let key = extract_scaffold_key(steps);
    // 仅允许纯 Drill 或 Drill + key 模式，其他过滤模式不 scaffold
    let is_key_mode = key.is_some();
    if !is_simple_selector(steps) && !is_key_mode { return None; }
    let bullet = crate::effect::SCAFFOLD_BULLET_MARK;
    if snap.line_len() == 0 {
        let mut final_text = String::new();
        let clean_payload = payload.trim();
        for (i, part) in head_parts.iter().enumerate() {
            final_text.push_str(&format!("{} {}\n", crate::effect::atx_mark_text(i as u8 + 1), part));
        }
        if let Some(k) = key {
            let indented = indent_scaffold_payload(clean_payload);
            final_text.push_str(&format!("{}{}: {}\n", bullet, k, indented));
        } else if !clean_payload.is_empty() {
            if clean_payload.starts_with('-') || clean_payload.starts_with('#') {
                final_text.push_str(clean_payload);
                if !clean_payload.ends_with('\n') { final_text.push('\n'); }
            } else {
                final_text.push_str(&format!("{}{}\n", bullet, clean_payload));
            }
        }
        if final_text.is_empty() { return None; }
        return Some(ScaffoldInsert { line: 0, text: final_text });
    }
    let mut missing_lines = Vec::new();
    for i in existing_depth..head_parts.len() {
        missing_lines.push(format!("{} {}", crate::effect::atx_mark_text(i as u8 + 1), head_parts[i]));
    }
    let clean_payload = payload.trim();
    if let Some(k) = key {
        let indented = indent_scaffold_payload(clean_payload);
        missing_lines.push(format!("{}{}: {}", bullet, k, indented));
    } else if !clean_payload.is_empty() {
        if clean_payload.starts_with('-') || clean_payload.starts_with('#') {
            missing_lines.push(clean_payload.to_string());
        } else {
            missing_lines.push(format!("{}{}", bullet, clean_payload));
        }
    }
    if missing_lines.is_empty() { return None; }
    let insert_line = if existing_depth == 0 {
        snap.line_len()
    } else {
        let head_steps: Vec<Step> = head_parts[0..existing_depth]
            .iter()
            .map(|p| Step::Drill(p.clone()))
            .collect();
        #[cfg(feature = "telemetry")] let _sp_anchor = crate::telemetry_span!("op_scaffold_anchor");
        let nodes = crate::selector::select_nodes(snap, &head_steps);
        #[cfg(feature = "telemetry")] drop(_sp_anchor);
        if nodes.is_empty() { return None; }
        nodes[0].end_line
    };
    let final_text = format!("\n{}\n", missing_lines.join("\n"));
    Some(ScaffoldInsert { line: insert_line, text: final_text })
}

pub(crate) fn deepest_existing_head_depth(snap: &Snapshot, head_parts: &[String]) -> usize {
    let mut steps = Vec::new();
    for (i, part) in head_parts.iter().enumerate() {
        steps.push(Step::Drill(part.clone()));
        if !head_exists(snap, &steps) {
            return i;
        }
    }
    head_parts.len()
}

pub(crate) fn fill_stats_by_range(snap: &Snapshot, node: &mut TreeSummary, range: Range<usize>, lines: usize) {
    let slice = snap.byte_slice(range);
    node.chars = slice.chars().count();
    node.tokens = get_bpe().encode_with_special_tokens(&slice.to_string()).len();
    node.lines = lines;
}

pub(crate) fn extract_inline_key(snap: &Snapshot, line_idx: usize, t: BlockType) -> Option<String> {
    let face = snap.node_proj();
    extract_inline_key_with(snap, &face, line_idx, t)
}

fn extract_inline_key_with(
    snap: &Snapshot,
    face: &crate::infer::NodeProj<'_>,
    line_idx: usize,
    t: BlockType,
) -> Option<String> {
    let node = snap.build_node(line_idx, (line_idx + 1).min(snap.line_len()), t, false);
    face.list_key(&node)
}

pub(crate) fn extract_inline_head(snap: &Snapshot, line_idx: usize) -> Option<String> {
    let face = snap.node_proj();
    extract_inline_head_with(snap, &face, line_idx)
}

fn extract_inline_head_with(
    snap: &Snapshot,
    _face: &crate::infer::NodeProj<'_>,
    line_idx: usize,
) -> Option<String> {
    let t = snap.line_type_at(line_idx);
    extract_inline_head_resolved(snap, line_idx, t)
}

/// 使用已解析的 BlockType 提取标题文本；setext/嵌套 BQ 均按 strip_bq 后提取。
fn extract_inline_head_resolved(snap: &Snapshot, line_idx: usize, t: BlockType) -> Option<String> {
    if !is_header(t) { return None; }
    let name = Snapshot::with_line_str(snap.line_text(line_idx), |s| {
        let full = crate::block::str_view(crate::block::strip_bq(s.as_bytes(), false));
        crate::block::extract_heading_text(full).trim_end().to_string()
    });
    if name.is_empty() { None } else { Some(name) }
}

/// If `line` is an ATX heading, shift its `#` count by `delta` (capped at 6).
fn shift_atx_heading(line: &str, delta: u8) -> String {
    let bytes = line.as_bytes();
    if bytes.is_empty() || bytes[0] != b'#' { return line.to_string(); }
    let mut level = 0u8;
    while (level as usize) < bytes.len() && level < 6 && bytes[level as usize] == b'#' {
        level += 1;
    }
    if level == 0 { return line.to_string(); }
    let end_of_hashes = level as usize;
    if end_of_hashes < bytes.len() && bytes[end_of_hashes] != b' ' && bytes[end_of_hashes] != b'\t' {
        return line.to_string();
    }
    let new_level = (level + delta).min(6) as usize;
    let body = if end_of_hashes < line.len() { &line[end_of_hashes..] } else { "" };
    crate::effect::atx_mark_text(new_level as u8) + body
}


// Write Planning Types


pub struct WriteRequest<'a> {
    pub steps: &'a [Step],
    pub operator: &'a str,
    pub text: String,
    pub proj: Option<&'a str>,
    pub auto_scaffold: bool,
    pub auto_level: bool,
}

pub struct WriteEdit {
    pub range: Range<usize>,
    pub text: String,
}

pub enum WritePlan {
    Apply {
        operator: String,
        edits: Vec<WriteEdit>,
    },
    PatchApply {
        edits: Vec<WriteEdit>,
    },
    MissingTarget,
}

#[derive(serde::Deserialize, Debug)]
pub(crate) struct PatchBlock {
    pub old: String,
    pub new: String,
}

/// `~>` patch JSON 的唯一解析入口。session 只消费解析后的结构，
/// 不做 serde；投影 scope / 行映射升级都发生在 op 层。
pub(crate) fn parse_patch_blocks(payload: &str) -> Result<Vec<PatchBlock>, OpError> {
    serde_json::from_str::<Vec<PatchBlock>>(payload)
        .map_err(|e| OpError { message: format!("invalid patch JSON: {}", e) })
}

/// `~>` 的编辑 scope。
///
/// 默认/raw：node.byte_range。
/// content/tag：使用与读投影一致的连续范围；**payload 保持 JSON 原样**，
/// 绝不经过 `format_payload`。会改坏 JSON 的类型（BQ/table）显式拒绝。
/// container/mark：投影铁律只读，拒绝。
fn patch_scope_edit(
    snap: &Snapshot,
    face: &crate::infer::NodeProj<'_>,
    node: &BlockNode,
    proj: Option<&str>,
    payload: &str,
) -> Result<WriteEdit, OpError> {
    let surface = proj.map(BlockProj::from_proj_str).unwrap_or(Some(BlockProj::Raw));
    let surface = match surface {
        Some(s) => s,
        None => return Err(OpError { message: format!("unknown patch projection: {:?}", proj) }),
    };
    let range = match surface {
        BlockProj::Raw => node.byte_range.clone(),
        BlockProj::Content => {
            // BQ/table 的 content payload 格式化会改 JSON；行映射器实现前拒绝。
            if crate::effect::payload_spec(node.node_type, "content")
                == crate::effect::PayloadSpec::BqPrefix
                || matches!(
                    crate::effect::line_spec(node.node_type).mark_width,
                    crate::effect::MarkWidth::Fixed("|")
                )
            {
                return Err(OpError {
                    message: "patch projection Content unsupported for this node (raw scope only)"
                        .to_string(),
                });
            }
            face.content_write_range(&node.byte_range, node.node_type)
                .map_err(|e| OpError { message: e })?
        }
        BlockProj::Tag => face
            .tag_write_range(&node.byte_range, node.node_type)
            .map_err(|e| OpError { message: e })?,
        BlockProj::Container | BlockProj::Mark => {
            return Err(OpError {
                message: format!("patch projection {:?} is read-only", surface),
            });
        }
    };
    Ok(WriteEdit { range, text: payload.to_string() })
}

/// raw scope 的 `~>` 预计算：解析 patch JSON，逐 node 模拟文本并把
/// 每个匹配替换成精确 absolute byte edit。session 收到 PatchApply 后
/// 只做 delete/insert，不搜索。
///
/// 语义与旧 session 搜索保持：
/// - 每个 node scope 内第一个字面匹配
/// - 找不到时 old/new 去头尾 `\n\r` 再找一次
/// - 多 patch 块按序应用，后面的块能看到前面块产生的新文本
/// - 空 old 忽略
pub(crate) fn plan_patch(
    snap: &Snapshot,
    nodes: &[BlockNode],
    proj: Option<&str>,
    payload: &str,
) -> Result<WritePlan, OpError> {
    let surface = proj.map(BlockProj::from_proj_str).unwrap_or(Some(BlockProj::Raw));
    match surface {
        Some(BlockProj::Raw) => {}
        Some(_) | None => return Ok(WritePlan::MissingTarget),
    }

    let patches = parse_patch_blocks(payload)?;
    let mut nodes = nodes.to_vec();
    nodes.sort_by_key(|n| n.byte_range.start);

    // 重叠 scope（父/子区域同时命中）时，预计算会脱离“前一个节点已改过的
    // 当前文本”；退回 session 搜索路径，保持旧语义。
    // 列表 item 节点的 raw patch scope 扩展到 item 子树（嵌套子项可见——
    // `# S\n> - a\n>   - b: 2` 对 a patch old=2 须命中 b）。
    let scope_of = |node: &BlockNode| -> std::ops::Range<usize> {
        if crate::effect::block_decl(node.node_type).is_list_item {
            let end_line = crate::infer::item_subtree_end(
                &snap.frame_log.entries, &snap.open_depth, node.start_line, snap.line_len());
            let end_byte = if end_line < snap.line_len() {
                snap.byte_of_line(end_line)
            } else {
                snap.byte_len()
            };
            node.byte_range.start..end_byte.max(node.byte_range.end)
        } else {
            node.byte_range.clone()
        }
    };
    let mut prev_end = 0usize;
    for node in &nodes {
        let scope = scope_of(node);
        if scope.start < prev_end {
            return Ok(WritePlan::Apply {
                operator: "~>".to_string(),
                edits: nodes
                    .iter()
                    .map(|n| WriteEdit {
                        range: n.byte_range.clone(),
                        text: payload.to_string(),
                    })
                    .collect(),
            });
        }
        prev_end = prev_end.max(scope.end);
    }

    let mut edits = Vec::new();
    let mut global_delta: isize = 0;
    for node in nodes {
        let scope = scope_of(&node);
        let base = scope.start as isize + global_delta;
        let mut cur = snap.byte_slice(scope.clone()).to_string();
        for block in &patches {
            if block.old.is_empty() {
                continue;
            }
            let (old, new) = if cur.find(&block.old).is_some() {
                (block.old.as_str(), block.new.as_str())
            } else {
                let old_trim = block.old.trim_matches(['\n', '\r']);
                let new_trim = block.new.trim_matches(['\n', '\r']);
                if cur.find(old_trim).is_some() {
                    (old_trim, new_trim)
                } else {
                    eprintln!("⚠️ [MDIO Patch Warning] Match failed! '{}' not found.", old_trim);
                    continue;
                }
            };
            let pos = cur.find(old).unwrap();
            let abs_start = base + pos as isize;
            let abs_end = abs_start + old.len() as isize;
            edits.push(WriteEdit {
                range: abs_start as usize..abs_end as usize,
                text: new.to_string(),
            });
            cur.replace_range(pos..pos + old.len(), new);
            global_delta += new.len() as isize - old.len() as isize;
        }
    }
    Ok(WritePlan::PatchApply { edits })
}

/// 目标首行实际 BQ 层前缀（`>` / `> ` 保留原宽；层之间夹着的
/// list marker 不计入——例如 `> - > x` 得到 `> > `）。
/// 单源 = block::bq_prefix_of_line（op/infer 写面共用）。
fn bq_outer_prefix(snap: &Snapshot, node: &BlockNode) -> String {
    Snapshot::with_line_str(snap.line_text(node.start_line), crate::block::bq_prefix_of_line)
}

/// 行内插入文本：去掉写入边界归一化追加的尾部换行。
fn inline_insert_text(payload: &str) -> String {
    payload.trim_end_matches('\n').to_string()
}

/// 新起行插入文本：每行加外层前缀；空行使用无尾空格的 `>` 前缀；
/// 保留 payload 已有的尾换行。（单源 = block::embody_lines(inline_first=false)。）
fn prefixed_line_text(prefix: &str, payload: &str) -> String {
    crate::block::embody_lines(prefix, payload, false)
}

/// 追加点行结构感知的 payload 收尾（尾 \n 归一化下沉——FFI 层 range 盲的
/// blanket 追加改由 plan 层在插入点已知处决策）：
///   - 插入点在行首（前一字节为 \n）或文档尾 → 补尾 \n（payload 自成一行）；
///   - 插入点在行尾（下一字节为 \n）→ 不补（原 \n 保留，防双换行）；
///   - 行中（comment 标记间等）→ 不补（防换行污染行内结构）。
/// 注意：仅 +> 追加需要（apply_one 的 +> 分支无收尾逻辑）；/> 替换由
/// apply_one 的 has_trailing_nl 收尾（删除到原 \n 前 + payload trim——等价）。
fn finalize_payload(snap: &Snapshot, range: &std::ops::Range<usize>, text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        return text.to_string();
    }
    let at_line_start = range.end == 0
        || range.end >= snap.byte_len()
        || (range.end > 0 && snap.byte_at(range.end - 1) == b'\n');
    if at_line_start {
        format!("{}\n", text)
    } else {
        text.to_string()
    }
}

/// 尾追加锚点（+> -pj ct 对齐）：插入点 = 内容尾（尾换行之前）。
/// - Paragraph/Blockquote：块尾向前剥换行；
/// - HtmlBlock/HtmlElement：末行为闭合标签（</…> 行）→ 落其行首；否则剥尾换行
///   （type 6 空行截断的空行算内容，不吞）；
/// - CodeFenceOpen：体尾剥一个换行（体尾换行 = 分隔符非内容）；
/// - 其余类型：range.end（现状）。
/// 只服务 append_edit；content_write_range（/> 替换共享）保持原样。
fn content_tail_anchor(snap: &Snapshot, node: &BlockNode, range: &std::ops::Range<usize>) -> usize {
    use crate::block::BlockType as BT;
    let mut end = range.end;
    match node.node_type {
        BT::Paragraph | BT::Blockquote => {
            while end > range.start && snap.byte_at(end - 1) == b'\n' { end -= 1; }
        }
        BT::HtmlBlock | BT::HtmlElement => {
            if end > range.start {
                let last = snap.line_of_byte(end.saturating_sub(1));
                let line_str: String = snap.rope.line(last).chunks().collect();
                let t = line_str.trim();
                if t.starts_with("</") && t.ends_with('>') {
                    end = snap.byte_of_line(last);
                }
                // 有闭标：锚 = 闭标行首再剥尾换行（落在内容尾）；无闭标：直接剥。
                // 剥尾不吞空行——type 6 截断空行算内容。
                while end > range.start && snap.byte_at(end - 1) == b'\n' { end -= 1; }
            }
        }
        BT::CodeFenceOpen => {
            // 仅当存在体行（开行与闭行之间至少一行）时剥体尾换行；空体保持
            // 行首落点（否则会拼进开行本身——行中分支把 payload 接到 ```lang 尾）。
            if node.end_line > node.start_line + 2
                && end > range.start && snap.byte_at(end - 1) == b'\n'
            {
                end -= 1;
            }
        }
        _ => {}
    }
    end
}

/// `+>` 的投影插入规划。
///
/// - content：沿用覆盖语义的 content range 与 payload 格式化，插入点取 content_tail_anchor 锚
/// - raw：插入 node region end，payload 加目标实际 BQ 层前缀
/// - container：作为只读锚点，插入 content start（行首边界则新起行，行内则 inline）
/// - tag：插入 tag range.end（行内 append，去尾换行）
/// - mark：只读
pub(crate) fn append_edit(
    snap: &Snapshot,
    face: &crate::infer::NodeProj<'_>,
    node: &BlockNode,
    proj: &str,
    payload: &str,
) -> Result<WriteEdit, OpError> {
    use crate::block::BlockProj;
    let surface = BlockProj::from_proj_str(proj)
        .ok_or_else(|| OpError { message: format!("unknown projection: {}", proj) })?;

    match surface {
        BlockProj::Content => {
            // TableRow 特判：追加 = 末 cell 文本尾 + 原始 payload（不经过
            // transform_payload 的整行重建——重建文本只服务 /> 的整行替换语义，
            // 追加到行尾会破坏 cell 数）。复用读面同源 cell_ranges_bytes。
            if node.node_type == BlockType::TableRow {
                let raw = snap.byte_slice(node.byte_range.clone()).to_string();
                let (_, ranges) = crate::block::cell_ranges_bytes(raw.as_bytes());
                // trim_ascii 不含换行 ⟹ 尾随空 cell(含 \\n)会被计入——取最后一个
                // 非空白 cell 的文本尾作为追加锚点(cell 数不变, 结构保留)。
                if let Some(last) = ranges.iter().rev().find(|r| {
                    !raw.as_bytes()[r.start..r.end].iter().all(|&b|
                        b == b' ' || b == b'\t' || b == b'\n' || b == b'\r')
                }) {
                    let end = node.byte_range.start + last.end;
                    return Ok(WriteEdit { range: end..end, text: inline_insert_text(payload) });
                }
            }
            // 保留现有 content 覆盖语义（payload 格式化、首行/正文范围），
            // 只是把覆盖范围压成 end..end 的追加插入点。
            // 文本基 = 原始 payload（compute_write 的体现变换是 replace 语义；
            // append 的前缀适配由下方行界判定独立完成——避免双前缀）。
            let (range, _text) = face
                .compute_write_from_node(node, "content", payload)
                .map_err(|e| OpError { message: e })?;
            // 尾追加锚点（对齐原则）：
            // 插入点 = 内容尾（尾换行之前）；content_write_range（/> 替换共享）不动。
            let anchor = content_tail_anchor(snap, node, &range);
            // 落点行首（前一字节为换行——空内容例外条款）⟹ 全行前缀（现状）；
            // 落点行中：单行 payload 原样拼到内容尾（不取前缀——空文档脚手架等
            // 退化节点走此路，bq_outer_prefix 会越界）；payload 内换行则后续行补前缀。
            // 前缀只在真正需要时取，且对退化节点（空文档脚手架等 start_line 越界）
            // 返回空——bq_outer_prefix 会 line(start_line) 越界 panic。
            let outer = || -> String {
                if node.start_line < snap.line_len() {
                    bq_outer_prefix(snap, node)
                } else {
                    String::new()
                }
            };
            let prefix = outer();
            let text = if anchor > 0 && snap.byte_at(anchor - 1) == b'\n' {
                prefixed_line_text(&prefix, payload)
            } else if payload.contains('\n') {
                // 尾换行语义（写面收口计划第三步）：载荷以换行收尾 = 该行结束、
                // 下一 chunk 应落在新行——预开一行带容器前缀（parse 稳定编码：
                // 裸换行会被重解析吸收成块终止符）。前缀为空（顶层段落）时无
                // 法预开——尾换行保持现状（前导换行即新行，语义可满足）。
                let mut t = crate::block::embody_lines(&prefix, payload, true);
                if payload.ends_with('\n') && !prefix.is_empty() {
                    t.push_str(&prefix);
                }
                t
            } else {
                payload.to_string()
            };
            Ok(WriteEdit { range: anchor..anchor, text })
        }
        BlockProj::Raw => {
            let prefix = bq_outer_prefix(snap, node);
            let mut text = prefixed_line_text(&prefix, payload);
            let end = node.byte_range.end.min(snap.byte_len());
            if !text.is_empty() && end > 0 && snap.byte_at(end - 1) != b'\n' {
                text.insert(0, '\n');
            }
            Ok(WriteEdit { range: end..end, text })
        }
        BlockProj::Container => {
            match crate::effect::region_role(node.node_type) {
                crate::effect::RegionRole::Section | crate::effect::RegionRole::Cell => {}
                other => {
                    return Err(OpError {
                        message: format!("ctn append anchor unsupported for {:?} role", other),
                    });
                }
            }
            let content_start = face
                .content_write_range(&node.byte_range, node.node_type)
                .map_err(|e| OpError { message: e })?
                .start;
            let line = snap.line_of_byte(content_start);
            let line_start = snap.byte_of_line(line);
            let text = if content_start == line_start {
                prefixed_line_text(&bq_outer_prefix(snap, node), payload)
            } else {
                inline_insert_text(payload)
            };
            Ok(WriteEdit { range: content_start..content_start, text })
        }
        BlockProj::Tag => {
            let tag_range = face
                .tag_write_range(&node.byte_range, node.node_type)
                .map_err(|e| OpError { message: e })?;
            Ok(WriteEdit {
                range: tag_range.end..tag_range.end,
                text: inline_insert_text(payload),
            })
        }
        BlockProj::Mark => Err(OpError {
            message: "mark is read-only".to_string(),
        }),
    }
}

pub fn validate_projection_write(
    snap: &Snapshot,
    steps: &[Step],
    proj: &str,
) -> Result<(), OpError> {
    let nodes = crate::selector::select_for_write(snap, steps);
    // sel 对齐/写面单源：与 plan_write 使用同一个 NodeProj 写面，
    // 不经 Snapshot 本地副本（双真相源漂移风险）。
    let face = snap.node_proj();
    for node in &nodes {
        face.proj_write_range(&node.byte_range, node.node_type, proj)
            .map_err(|e| OpError { message: e })?;
    }
    Ok(())
}

pub fn plan_write(snap: &Snapshot, req: WriteRequest<'_>) -> Result<WritePlan, OpError> {
    let _span = telemetry_span!("plan_write", "operator" => req.operator, "proj" => req.proj.unwrap_or("none"));
    let mut final_text = req.text;

    // 2. Get write targets
    let nodes = crate::selector::select_for_write(snap, req.steps);

    // 3. Empty targets: scaffold or missing
    if nodes.is_empty() {
        if req.auto_scaffold {
            let head_parts = extract_head_parts(req.steps);
            let existing_depth = deepest_existing_head_depth(snap, &head_parts);
            let scaffold_depth = (existing_depth + head_parts.len()) as u8;

            let mut scaffold_payload = final_text;
            if req.auto_level {
                scaffold_payload = process_payload(&scaffold_payload, scaffold_depth);
            }
            if let Some(scaffold) = build_scaffold(snap, req.steps, req.operator, &scaffold_payload, existing_depth) {
                let insert_byte = snap.byte_of_line(scaffold.line);
                return Ok(WritePlan::Apply {
                    operator: "+>".to_string(),
                    edits: vec![WriteEdit {
                        range: insert_byte..insert_byte,
                        text: scaffold.text,
                    }],
                });
            }
        }
        return Ok(WritePlan::MissingTarget);
    }

    // 4. raw patch 走预计算路径，不经 session 搜索。
    if req.operator == "~>" && req.proj.map_or(true, |p| p == "raw") {
        return plan_patch(snap, &nodes, req.proj, &final_text);
    }

    let face = snap.node_proj();
    let mut edits = Vec::new();
    for node in &nodes {
        let mut payload = final_text.clone();

        // patch JSON 不走 auto_level / 投影 payload 格式化。
        if req.operator != "~>" && is_header(node.node_type) && req.auto_level {
            let target_depth = header_level(node.node_type).unwrap_or(1);
            payload = process_payload(&payload, target_depth);
        }

        if req.operator == "~>" {
            edits.push(patch_scope_edit(snap, &face, node, req.proj, &payload)?);
        } else if let Some(p) = req.proj {
            let edit = if req.operator == "+>" {
                let mut e = append_edit(snap, &face, node, p, &payload)?;
                e.text = finalize_payload(snap, &e.range, &e.text);
                e
            } else {
                // /> 替换：尾 \n 由 apply_one 的 has_trailing_nl 收尾
                // （删除到原 \n 前 + payload trim——行尾范围净行为与 FFI 时代一致；
                // 行中范围（comment）无 \n 参与——正确内联）。
                let (r, transformed) = face
                    .compute_write_from_node(node, p, &payload)
                    .map_err(|e| OpError { message: e })?;
                WriteEdit { range: r, text: transformed }
            };
            edits.push(edit);
        } else {
            // raw 替换：finalize（append 规则——range 尾在行首/EOF 时补 \n）——
            // 恢复 FFI 时代 raw 写的行尾语义（空文档种子/无尾换行文档）。
            let r = node.byte_range.clone();
            edits.push(WriteEdit { range: r.clone(), text: finalize_payload(snap, &r, &payload) });
        }
    }

    Ok(WritePlan::Apply {
        operator: req.operator.to_string(),
        edits,
    })
}

/// Variant of [`plan_write`] that accepts pre-computed `nodes` from a prior
/// `select_for_write` call, avoiding a redundant AST traversal.
///
/// The caller must guarantee that `nodes` were computed from the same
/// document state as `snap` (or a snapshot that is byte-identical to it).
pub fn plan_write_with_nodes(
    snap: &Snapshot,
    req: WriteRequest<'_>,
    nodes: &[BlockNode],
    existing_depth: usize,
) -> Result<WritePlan, OpError> {
    let _span = telemetry_span!("plan_write_nodes", "operator" => req.operator, "node_count" => nodes.len());
    let final_text = req.text;

    if nodes.is_empty() {
        if req.auto_scaffold {
            let head_parts = extract_head_parts(req.steps);
            let scaffold_depth = (existing_depth + head_parts.len()) as u8;

            let mut scaffold_payload = final_text;
            if req.auto_level {
                scaffold_payload = process_payload(&scaffold_payload, scaffold_depth);
            }
            if let Some(scaffold) = build_scaffold(snap, req.steps, req.operator, &scaffold_payload, existing_depth) {
                let insert_byte = snap.byte_of_line(scaffold.line);
                return Ok(WritePlan::Apply {
                    operator: "+>".to_string(),
                    edits: vec![WriteEdit {
                        range: insert_byte..insert_byte,
                        text: scaffold.text,
                    }],
                });
            }
        }
        return Ok(WritePlan::MissingTarget);
    }

    if req.operator == "~>" && req.proj.map_or(true, |p| p == "raw") {
        return plan_patch(snap, nodes, req.proj, &final_text);
    }

    let face = snap.node_proj();
    let mut edits = Vec::new();
    for node in nodes {
        let mut payload = final_text.clone();

        // patch JSON 不走 auto_level / 投影 payload 格式化。
        if req.operator != "~>" && is_header(node.node_type) && req.auto_level {
            let target_depth = header_level(node.node_type).unwrap_or(1);
            payload = process_payload(&payload, target_depth);
        }

        if req.operator == "~>" {
            edits.push(patch_scope_edit(snap, &face, node, req.proj, &payload)?);
        } else if let Some(p) = req.proj {
            let edit = if req.operator == "+>" {
                let mut e = append_edit(snap, &face, node, p, &payload)?;
                e.text = finalize_payload(snap, &e.range, &e.text);
                e
            } else {
                // /> 替换：尾 \n 由 apply_one 的 has_trailing_nl 收尾
                // （删除到原 \n 前 + payload trim——行尾范围净行为与 FFI 时代一致；
                // 行中范围（comment）无 \n 参与——正确内联）。
                let (r, transformed) = face
                    .compute_write_from_node(node, p, &payload)
                    .map_err(|e| OpError { message: e })?;
                WriteEdit { range: r, text: transformed }
            };
            edits.push(edit);
        } else {
            // raw 替换：finalize（append 规则——range 尾在行首/EOF 时补 \n）——
            // 恢复 FFI 时代 raw 写的行尾语义（空文档种子/无尾换行文档）。
            let r = node.byte_range.clone();
            edits.push(WriteEdit { range: r.clone(), text: finalize_payload(snap, &r, &payload) });
        }
    }

    Ok(WritePlan::Apply {
        operator: req.operator.to_string(),
        edits,
    })
}


// OpError


#[derive(Debug)]
pub struct OpError {
    pub message: String,
}

impl From<String> for OpError {
    fn from(message: String) -> Self { OpError { message } }
}

pub fn read_projection(
    snap: &Snapshot,
    steps: &[Step],
    proj: Option<&str>,
) -> Result<Vec<String>, OpError> {
    let _span = telemetry_span!("read_projection", "step_count" => steps.len(), "proj" => proj.unwrap_or("raw"));
    let p = proj.unwrap_or("raw");
    let surface = BlockProj::from_proj_str(p)
        .ok_or_else(|| OpError { message: format!("unknown projection: {}", p) })?;
    let vnodes = crate::selector::select_nodes(snap, steps);
    let face = snap.node_proj();
    let mut results = Vec::with_capacity(vnodes.len());
    for node in &vnodes {
        results.push(face.read_block_proj(node, surface));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_of(text: &str) -> Snapshot {
        let mut state = crate::projection::State::new(text);
        state.snapshot()
    }

    #[test]
    fn reverse_pathfinding_local_zone_skips_flat_prefix() {
        let prefix = "p\n\n".repeat(50);
        let target = prefix.lines().count();
        let doc = format!("{prefix}target\n\n{}", "tail\n\n".repeat(50));
        let snap = snap_of(&doc);
        let (zs, ce) = super::local_zone_for_line(&snap, target);
        assert_eq!(zs, target, "flat no-container doc zone must start at depth-0 target block");
        assert!(ce < snap.line_len(), "flat zone must not include suffix");
    }

    #[test]
    fn reverse_pathfinding_local_zone_skips_other_sections() {
        let doc = format!("# A\n\n{}# B\n\nb one\n\n{}",
            "a\n\n".repeat(50), "tail\n\n".repeat(50));
        let snap = snap_of(&doc);
        // 目标行 = 文档中第二个 H1 之后的首个内容行。
        let target = snap.lines.iter().rposition(|l| l.node_type() == crate::block::BlockType::Header1).unwrap() + 1;
        let (zs, ce) = super::local_zone_for_line(&snap, target);
        assert_eq!(zs, target - 1, "zone must start at the nearest predecessor heading");
        assert!(ce < snap.line_len(), "zone must not include trailing suffix");
    }

    #[test]
    fn reverse_pathfinding_local_matches_full_oracle() {
        let docs = [
            "# S\n\n## A\n- a: 1\n- b: 2\n\npara\n",
            "# S\n> - a\n> - b\n\n# S2\n",
            "> # H4\n> - B3: x\n",
            "- a: 1\n- b: 2\n",
            "# T\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
            "foo\n===\n\ntext\n\n# H\n",
            "```rust\nlet x = 1;\n```\n\npara\n",
            "# A\n\nintro\n\n# B\n\nb one\n\n",
            "# A\n\npara\n\n## B\n\npara\n\n### C\n\nleaf\n",
        ];
        for doc in docs {
            let snap = snap_of(doc);
            for li in 0..=snap.line_len() {
                assert_eq!(
                    super::reverse_pathfinding_local(&snap, li),
                    super::reverse_pathfinding_full_oracle(&snap, li),
                    "local path mismatch\ndoc={doc:?}\nline={li}"
                );
            }
        }
        let tokens = [
            "# H1\n", "## H2\n", "### H3\n", "para\n", "  cont\n",
            "- a\n", "1. b\n", "> q\n", "```\nx\n```\n", "\n",
            "| a | b |\n| - | - |\n",
        ];
        let mut rng: u64 = 0x0A2B_0B2B;
        for case in 0..300u64 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let mut doc = String::new();
            let n_lines = 3 + (rng as usize % 10);
            for _ in 0..n_lines {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                doc.push_str(tokens[rng as usize % tokens.len()]);
            }
            let snap = snap_of(&doc);
            for li in 0..=snap.line_len() {
                let got = super::reverse_pathfinding_local(&snap, li);
                let want = super::reverse_pathfinding_full_oracle(&snap, li);
                assert_eq!(
                    got, want,
                    "random local path mismatch case={case}\ndoc={doc:?}\nline={li}"
                );
            }
        }
    }

    #[test]
    fn plan_patch_raw_precomputes_exact_byte_edit() {
        let snap = snap_of("# T\n- a: x\n");
        let nodes = crate::selector::select_nodes(&snap, &[Step::Drill("T".into())]);
        let plan = plan_patch(&snap, &nodes, None, r#"[{"old":"x","new":"X"}]"#).unwrap();
        let crate::op::WritePlan::PatchApply { edits } = plan else { panic!("not PatchApply") };
        assert_eq!(edits.len(), 1);
        assert_eq!(snap.byte_slice(edits[0].range.clone()).to_string(), "x");
        assert_eq!(edits[0].text, "X");
    }

    #[test]
    fn plan_patch_multi_block_sees_previous_edit() {
        let snap = snap_of("# T\n- a: x\n- b: y\n");
        let nodes = crate::selector::select_nodes(&snap, &[Step::Drill("T".into())]);
        let plan = plan_patch(
            &snap,
            &nodes,
            None,
            r#"[{"old":"a: x","new":"a: X"},{"old":"b: y","new":"b: Y"}]"#,
        )
        .unwrap();
        let crate::op::WritePlan::PatchApply { edits } = plan else { panic!("not PatchApply") };
        assert_eq!(edits.len(), 2);
        assert_eq!(snap.byte_slice(edits[0].range.clone()).to_string(), "a: x");
        assert_eq!(snap.byte_slice(edits[1].range.clone()).to_string(), "b: y");
    }

    /// Op equivalence gates: kernel-slot snapshots must produce the
    /// same reverse_pathfinding / run_find / run_tree output as the same
    /// Snapshot with a dangling Weak slot.
    #[test]
    fn prc_kernel_op_edit_chain_equivalence() {
        use crate::cache::CacheSlot;
        use std::sync::Arc;
        // 结构编辑 → Edit 链(op 消费的是 structure_for_range(0,n) 全量:
        // Edit 节点非精确 zone → fallback 全量构建)——输出必须与无槽一致。
        let doc = "# S\n\n## A\n- a: 1\n\npara\n";
        let mut st = crate::projection::State::new(doc);
        let snap0 = Arc::new(st.snapshot());
        let insert_at = doc.find("- a: 1").unwrap();
        st.edit(insert_at, insert_at, "- x: 9\n");
        let mut snap1 = Arc::new(st.snapshot());
        let diff = st.ripple_diff.take().expect("ripple diff");
        let n1 = snap1.line_len();
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(n1);
        let anchor = snap0.line_of_byte(insert_at.min(snap0.byte_len().saturating_sub(1)));
        let edit = crate::cache::CacheNode::new_edit_full(
            &snap0,
            snap1.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(insert_at, insert_at)]),
            diff.delta,
            anchor,
        );
        let slot = Arc::new(CacheSlot::new());
        // ArcSwapOption<Arc<CacheNode>>:store 需双层 Arc(与 seam 同款)。
        slot.node.store(Some(Arc::new(edit)));
        if let Some(s) = Arc::get_mut(&mut snap1) {
            s.set_cache_slot(Arc::downgrade(&slot));
        }

        // reverse_pathfinding: every line must match the no-slot fallback.
        // 对照 = st.snapshot()(State::snapshot 的 cache_slot 恒 Weak::new();
        // Snapshot clone 会复制 Weak 字段——不是无槽对照)。
        let snap1_noslot = st.snapshot();
        for li in 0..=snap1.line_len() {
            assert_eq!(
                super::reverse_pathfinding(&snap1, li),
                super::reverse_pathfinding(&snap1_noslot, li),
                "Edit-chain reverse_pathfinding mismatch for doc {doc:?} line {li}"
            );
        }
        let steps = vec![Step::Wildcard];
        let queries = vec![
            ("a".to_string(), false, false),
            ("^#".to_string(), true, false),
        ];
        assert_eq!(
            super::run_find(&snap1, &steps, &queries, 3, 3),
            super::run_find(&snap1_noslot, &steps, &queries, 3, 3),
            "Edit-chain run_find mismatch"
        );
        assert_eq!(
            super::run_tree(&snap1, &steps, 2, 2),
            super::run_tree(&snap1_noslot, &steps, 2, 2),
            "Edit-chain run_tree mismatch"
        );
    }

    #[test]
    fn prc_kernel_op_equivalence_gates() {
        use crate::cache::CacheSlot;
        use std::sync::Arc;

        let docs = [
            "# S\n\n## A\n- a: 1\n- b: 2\n\npara\n",
            "# S\n> - a\n> - b\n\n# S2\n",
            "> # H4\n> - B3: x\n",
            "- a: 1\n- b: 2\n",
            "# T\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
        ];

        for doc in docs {
            let mut st = crate::projection::State::new(doc);
            let snap = st.snapshot();

            let mut with_slot = snap.clone();
            let slot = Arc::new(CacheSlot::new());
            with_slot.set_cache_slot(Arc::downgrade(&slot));

            // reverse_pathfinding: same path strings for every line.
            for li in 0..=snap.line_len() {
                assert_eq!(
                    super::reverse_pathfinding(&with_slot, li),
                    super::reverse_pathfinding(&snap, li),
                    "reverse_pathfinding mismatch for doc {doc:?} line {li}"
                );
            }

            let steps = vec![Step::Wildcard];
            let queries = vec![
                ("a".to_string(), false, false),
                ("item".to_string(), false, false),
                ("^#".to_string(), true, false),
            ];

            // run_find: same find output (path + snippet).
            let with_find = super::run_find(&with_slot, &steps, &queries, 3, 3);
            let fallback_find = super::run_find(&snap, &steps, &queries, 3, 3);
            assert_eq!(with_find, fallback_find, "run_find mismatch for {doc:?}");

            // run_tree stays full fallback; kernel slot must not change output.
            let with_tree = super::run_tree(&with_slot, &steps, 2, 2);
            let fallback_tree = super::run_tree(&snap, &steps, 2, 2);
            assert_eq!(with_tree, fallback_tree, "run_tree mismatch for {doc:?}");
        }
    }


    // op 局部路径的 zone 定位前缀扫描


    /// 构造 flat 无容器文档（每行一个段落块），tail 查询
    /// `local_zone_for_line`。该函数先做 `header_lines(0,line)` 扫描全部
    /// 前缀行；无标题也把 0..line 走一遍，因此随查询行号线性增长。
    #[test]
    fn curve_local_zone_for_line() {
        let _perf_guard = crate::PERF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use std::time::{Duration, Instant};
        let build = |n: usize| {
            let mut doc = String::new();
            for i in 0..n {
                doc.push_str(&format!("p{i}\n\n"));
            }
            let snap = snap_of(&doc);
            (doc, snap)
        };
        let time = |n: usize| {
            let (_doc, snap) = build(n);
            let target = n - 1;
            let mut best = Duration::from_secs(60);
            for _ in 0..5 {
                let t = Instant::now();
                let _ = local_zone_for_line(&snap, target);
                best = best.min(t.elapsed());
            }
            best
        };
        let t5 = time(5_000);
        let t20 = time(20_000);
        // 阈值 1.4（线性理论 4.0）：并行全量跑时小样本抖动曾把 1.8 打成误红。
        assert!(t20.as_nanos() as f64 > t5.as_nanos() as f64 * 1.4,
            "local_zone_for_line 应随前缀行数线性（header_lines(0,line)）: 5k={t5:?} 20k={t20:?}");
    }
}


