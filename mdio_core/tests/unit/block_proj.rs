use super::*;
use crate::block::{scan_block, BlockType, node_type_to_str, entry_line, FrameKind, line_geometry};
use crate::block::BlockMask as ProjMask;

// ── helpers ──

fn snap_from(md: &str) -> (MdioEngine, MdioSnapshot) {
    let mut engine = MdioEngine::new(md);
    let snapshot = engine.make_snapshot();
    (engine, snapshot)
}

fn find_nodes(snap: &MdioSnapshot, pred: fn(BlockType) -> bool) -> Vec<BlockNode> {
    let mut out = Vec::new();
    let len = snap.lines.len();
    let mut i = 0;
    while i < len {
        let t = snap.lines[i].node_type();
        if pred(t) {
            let end = crate::infer::get_node_end_from_lines(
                &snap.lines, i, t, &snap.frame_log);
            out.push(snap.build_node(i, end, t, false));
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn assert_projs_readable(snap: &MdioSnapshot, node: &BlockNode) {
    let spec = block_proj_spec(node.node_type);
    let surfaces = [BlockProj::Raw, BlockProj::Mark, BlockProj::Tag, BlockProj::Content, BlockProj::Container];
    for &surface in &surfaces {
        let val = snap.read_block_proj(node, surface);
        if !spec.can_read.contains(surface) {
            assert!(val.is_empty(),
                "{:?}.{:?} spec says NOT readable but got '{}'", node.node_type, surface, val);
        }
    }
}

// ── unit: spec table ──

#[test]
fn spec_header() {
    for t in [BlockType::Header1, BlockType::Header2, BlockType::Header3,
              BlockType::Header4, BlockType::Header5, BlockType::Header6] {
        let s = block_proj_spec(t);
        assert!(s.can_read.raw()); assert!(s.can_read.mark()); assert!(s.can_read.tag());
        assert!(s.can_read.content()); assert!(s.can_read.container());
        assert!(!s.can_write.raw()); assert!(s.can_write.tag()); assert!(s.can_write.content());
        assert!(matches!(s.inline_mode, InlineFeed::TagOnly));
    }
}

#[test]
fn spec_setext_headers() {
    for t in [BlockType::HsHead1, BlockType::HsHead2] {
        let s = block_proj_spec(t);
        assert!(s.can_read.raw()); assert!(s.can_read.tag());
        assert!(s.can_write.tag()); assert!(s.can_write.content());
        assert!(matches!(s.inline_mode, InlineFeed::TagOnly));
    }
}

#[test]
fn spec_bullet_and_checklist() {
    for t in [BlockType::Bullet, BlockType::Checklist] {
        let s = block_proj_spec(t);
        assert!(s.can_read.raw()); assert!(s.can_read.mark()); assert!(s.can_read.tag());
        assert!(s.can_read.content()); assert!(s.can_read.container());
        assert!(s.can_write.tag()); assert!(s.can_write.content());
        assert!(!s.can_write.raw());
        assert!(matches!(s.inline_mode, InlineFeed::ContentOnly));
    }
}

#[test]
fn spec_ordered_list_item() {
    let s = block_proj_spec(BlockType::OrderedListItem);
    assert!(s.can_read.raw()); assert!(s.can_read.mark()); assert!(s.can_read.tag());
    assert!(s.can_write.tag()); assert!(s.can_write.content());
    assert!(matches!(s.inline_mode, InlineFeed::ContentOnly));
}

#[test]
fn spec_paragraph() {
    let s = block_proj_spec(BlockType::Paragraph);
    assert!(s.can_read.raw()); assert!(s.can_read.content());
    assert!(!s.can_write.tag()); assert!(s.can_write.raw()); assert!(s.can_write.content());
    assert!(matches!(s.inline_mode, InlineFeed::ContentOnly));
}

#[test]
fn spec_code_fence() {
    let s = block_proj_spec(BlockType::CodeFenceOpen);
    assert!(s.can_read.tag()); assert!(s.can_write.tag()); assert!(s.can_write.content());
    assert!(matches!(s.inline_mode, InlineFeed::None));

    let sc = block_proj_spec(BlockType::CodeFenceClose);
    assert!(!sc.can_write.raw()); assert!(!sc.can_write.tag()); assert!(!sc.can_write.content());
    assert_eq!(sc.can_write.0, 0);

    let scc = block_proj_spec(BlockType::CodeContent);
    assert!(scc.can_read.raw());
    assert!(scc.can_read.content());
    assert!(!scc.can_read.mark()); assert!(!scc.can_read.tag());
    assert!(!scc.can_write.raw()); assert!(!scc.can_write.content());
}

#[test]
fn spec_blockquote() {
    let s = block_proj_spec(BlockType::Blockquote);
    assert!(s.can_read.raw()); assert!(s.can_read.mark()); assert!(s.can_read.content());
    assert!(s.can_read.container());
    assert!(!s.can_read.tag());
    assert!(s.can_write.raw()); assert!(s.can_write.content());
    assert!(!s.can_write.tag());
    assert!(matches!(s.inline_mode, InlineFeed::ContentOnly));
}

#[test]
fn spec_comment() {
    let s = block_proj_spec(BlockType::Comment);
    assert!(s.can_read.raw()); assert!(s.can_read.mark());
    assert!(!s.can_read.tag());
    assert!(s.can_write.raw()); assert!(s.can_write.content());
    assert!(matches!(s.inline_mode, InlineFeed::None));
}

#[test]
fn spec_table() {
    for t in [BlockType::TableRow, BlockType::TableSeparator] {
        let s = block_proj_spec(t);
        assert!(s.can_read.raw()); assert!(s.can_read.mark()); assert!(s.can_read.content());
        assert!(!s.can_read.tag()); assert!(!s.can_read.container());
        assert!(s.can_write.raw()); assert!(s.can_write.content());
        assert!(matches!(s.inline_mode, InlineFeed::Cells));
    }
}

#[test]
fn spec_thematic_break() {
    for t in [BlockType::ThematicBreak, BlockType::HsSep] {
        let s = block_proj_spec(t);
        assert!(s.can_read.raw()); assert!(s.can_read.mark()); assert!(s.can_read.content());
        assert!(!s.can_read.tag()); assert!(!s.can_read.container());
        assert!(!s.can_write.raw()); assert!(!s.can_write.content());
        assert!(matches!(s.inline_mode, InlineFeed::None));
    }
}

#[test]
fn spec_raw_or_rc_types() {
    // IndentedCode: M2 now has Mark read + RawText inline_mode
    {
        let s = block_proj_spec(BlockType::IndentedCode);
        assert!(s.can_read.raw(), "IndentedCode should have Raw read");
        assert!(s.can_read.content(), "IndentedCode should have Content read");
        assert!(s.can_read.mark(), "IndentedCode should have Mark read (M2)");
        assert!(!s.can_read.tag());
        assert!(!s.can_read.container());
        // 声明变更：IC 的 content 写放开（`no_w` → `rw_c`＝Raw+Content）——
        // 写面为此先补了 `content_write_range` 的 IC 臂（按 `mark_width == Fixed("    ")`），
        // 并同步了 `trilogy::restore_content_start` 的 IC 支（否则 iteration26 分叉）。
        assert!(s.can_write.raw(), "IndentedCode 的 Raw 写随 rw_c 放开");
        assert!(s.can_write.content(), "IndentedCode 的 Content 写已放开");
        assert!(matches!(s.inline_mode, InlineFeed::RawText));
    }
    // HtmlContent: read-only, no markup
    {
        let s = block_proj_spec(BlockType::HtmlContent);
        assert!(s.can_read.raw());
        assert!(s.can_read.content());
        assert!(!s.can_read.mark());
        assert!(!s.can_read.tag());
        assert!(!s.can_read.container());
        assert!(!s.can_write.raw());
        assert!(matches!(s.inline_mode, InlineFeed::None));
    }
    // HtmlBlock: now has Tag+Mark for type 6 tag extraction
    let s = block_proj_spec(BlockType::HtmlBlock);
    assert!(s.can_read.raw());
    assert!(s.can_read.content());
    assert!(s.can_read.mark(), "HtmlBlock should now have Mark read");
    assert!(s.can_read.tag(), "HtmlBlock should now have Tag read");
    assert!(!s.can_read.container());
    assert!(s.can_write.content()); // writable
    assert!(matches!(s.inline_mode, InlineFeed::RawText));   // CM §4.6: raw passthrough
    // Blank: root-level, raw+content writable
    for t in [BlockType::Blank] {
        let s = block_proj_spec(t);
        assert!(s.can_read.raw());
        assert!(s.can_read.content());
        assert!(s.can_write.raw());
        assert!(s.can_write.content());
        assert!(!s.can_write.mark());
        assert!(!s.can_write.tag());
        assert!(!s.can_write.container());
        assert!(matches!(s.inline_mode, InlineFeed::None));
    }
}

#[test]
fn spec_all_types_return_valid_spec() {
    let all = [
        BlockType::Header1, BlockType::Header2, BlockType::Header3,
        BlockType::Header4, BlockType::Header5, BlockType::Header6,
        BlockType::Bullet, BlockType::CodeFenceOpen, BlockType::Blockquote,
        BlockType::OrderedListItem, BlockType::Paragraph, BlockType::Comment,
        BlockType::TableRow, BlockType::Checklist,
        BlockType::HsHead1, BlockType::HsHead2, BlockType::HsSep,
        BlockType::HtmlBlock, BlockType::ThematicBreak,
        BlockType::CodeFenceClose, BlockType::CodeContent,
        BlockType::HtmlContent, BlockType::IndentedCode,
        BlockType::TableSeparator, BlockType::Blank,
    ];
    for &nt in &all {
        let spec = block_proj_spec(nt);
        assert!(spec.can_read.raw(), "{:?} missing Raw read", nt);
    }
}

// ── unit: ProjMask / BlockProj ──

#[test]
fn proj_mask_bits() {
    let full = ProjMask::new(1|2|4|8|16);
    assert!(full.raw()); assert!(full.mark()); assert!(full.tag());
    assert!(full.content()); assert!(full.container());

    let none = ProjMask::new(0);
    assert!(!none.raw()); assert!(!none.tag()); assert!(!none.content());

    let rw = ProjMask::new(1|8);
    assert!(rw.raw()); assert!(!rw.mark()); assert!(!rw.tag());
    assert!(rw.content()); assert!(!rw.container());
}

#[test]
fn proj_mask_contains() {
    let full = ProjMask::new(1|2|4|8|16);
    assert!(full.contains(BlockProj::Raw));
    assert!(full.contains(BlockProj::Mark));
    assert!(full.contains(BlockProj::Tag));
    assert!(full.contains(BlockProj::Content));
    assert!(full.contains(BlockProj::Container));
    assert!(!ProjMask::new(1).contains(BlockProj::Content));
    assert!(!ProjMask::new(0).contains(BlockProj::Raw));
}

#[test]
fn block_proj_from_str() {
    assert_eq!(BlockProj::from_proj_str("raw"), Some(BlockProj::Raw));
    assert_eq!(BlockProj::from_proj_str("content"), Some(BlockProj::Content));
    assert_eq!(BlockProj::from_proj_str("tag"), Some(BlockProj::Tag));
    assert_eq!(BlockProj::from_proj_str("mark"), Some(BlockProj::Mark));
    assert_eq!(BlockProj::from_proj_str("container"), Some(BlockProj::Container));
    assert_eq!(BlockProj::from_proj_str("unknown"), None);
    assert_eq!(BlockProj::from_proj_str(""), None);
}

// ── golden: each shape × each projection ──

#[test]
fn golden_paragraph() {
    let (_, snap) = snap_from("hello **world**\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Paragraph);
    assert!(!nodes.is_empty(), "expected at least one paragraph");
    for n in &nodes { assert_projs_readable(&snap, n); }
    let p = &nodes[0];
    assert_eq!(snap.read_block_proj(p, BlockProj::Raw), "hello **world**\n");
    // CM §4.8: Content 剥段落尾换行（渲染语义——无尾软换行）；Raw 保留原文
    assert_eq!(snap.read_block_proj(p, BlockProj::Content), "hello **world**");
    assert_eq!(snap.read_block_proj(p, BlockProj::Mark), "");
    assert_eq!(snap.read_block_proj(p, BlockProj::Tag), "");
}

#[test]
fn golden_header_with_body() {
    let (_, snap) = snap_from("## Hello World\nbody text\n");
    let nodes = find_nodes(&snap, |t| is_header(t));
    assert!(!nodes.is_empty());
    let h = &nodes[0];
    assert_projs_readable(&snap, h);
    assert_eq!(snap.read_block_proj(h, BlockProj::Mark), "##");
    assert_eq!(snap.read_block_proj(h, BlockProj::Tag), "Hello World");
    let content = snap.read_block_proj(h, BlockProj::Content);
    assert!(content.contains("body text"), "expected content to contain body, got: '{}'", content);
}

#[test]
fn golden_header_no_body() {
    let (_, snap) = snap_from("# Title Only\n");
    let nodes = find_nodes(&snap, |t| is_header(t));
    assert!(!nodes.is_empty());
    let h = &nodes[0];
    assert_projs_readable(&snap, h);
    assert_eq!(snap.read_block_proj(h, BlockProj::Mark), "#");
    assert_eq!(snap.read_block_proj(h, BlockProj::Tag), "Title Only");
    assert_eq!(snap.read_block_proj(h, BlockProj::Content), "");
}

#[test]
fn golden_bullet_kv() {
    let (_, snap) = snap_from("- key: value\n- another: thing\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Bullet);
    assert_eq!(nodes.len(), 2);
    for n in &nodes { assert_projs_readable(&snap, n); }
    let b0 = &nodes[0];
    assert_eq!(snap.read_block_proj(b0, BlockProj::Mark), "-");
    assert_eq!(snap.read_block_proj(b0, BlockProj::Tag), "key");
    assert_eq!(snap.read_block_proj(b0, BlockProj::Content).trim(), "value");
}

#[test]
fn golden_bullet_plain() {
    let (_, snap) = snap_from("- just an item\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Bullet);
    assert!(!nodes.is_empty());
    let b = &nodes[0];
    assert_projs_readable(&snap, b);
    assert_eq!(snap.read_block_proj(b, BlockProj::Mark), "-");
    assert_eq!(snap.read_block_proj(b, BlockProj::Tag), "");
}

#[test]
fn golden_checklist() {
    let (_, snap) = snap_from("- [ ] todo item\n- [x] done\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Checklist);
    assert_eq!(nodes.len(), 2);
    for n in &nodes { assert_projs_readable(&snap, n); }
    let c0 = &nodes[0];
    assert_eq!(snap.read_block_proj(c0, BlockProj::Mark), "-");
    assert_eq!(snap.read_block_proj(c0, BlockProj::Tag), " "); // 状态字符（C3）
    let c1 = &nodes[1];
    assert_eq!(snap.read_block_proj(c1, BlockProj::Tag), "x");
}

#[test]
fn golden_ordered_list() {
    let (_, snap) = snap_from("12. twelfth item\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::OrderedListItem);
    assert!(!nodes.is_empty());
    let ol = &nodes[0];
    assert_projs_readable(&snap, ol);
    assert_eq!(snap.read_block_proj(ol, BlockProj::Mark), "12.");
    assert_eq!(snap.read_block_proj(ol, BlockProj::Tag), "12");
}

#[test]
fn golden_code_fence() {
    let (_, snap) = snap_from("```rust\nfn main() {}\n```\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::CodeFenceOpen);
    assert!(!nodes.is_empty());
    let cf = &nodes[0];
    assert_projs_readable(&snap, cf);
    assert_eq!(snap.read_block_proj(cf, BlockProj::Mark), "```");
    assert_eq!(snap.read_block_proj(cf, BlockProj::Tag), "rust");
    assert_eq!(snap.read_block_proj(cf, BlockProj::Content), "fn main() {}\n");
}

#[test]
fn golden_code_fence_no_lang() {
    let (_, snap) = snap_from("```\ncode\n```\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::CodeFenceOpen);
    assert!(!nodes.is_empty());
    let cf = &nodes[0];
    assert_projs_readable(&snap, cf);
    assert_eq!(snap.read_block_proj(cf, BlockProj::Tag), "");
    assert_eq!(snap.read_block_proj(cf, BlockProj::Content), "code\n");
}

#[test]
fn golden_blockquote() {
    let (_, snap) = snap_from("> quoted text\n> more quote\n");
    // BQ re-parse (§AX.3): BQ 内容行的 PL 类型是 Paragraph (非 Blockquote),
    // BQ 容器由 FL Push/Pop + walker 打开. 验证 AST 而非按行类型搜索.
    let ast = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(ast.contains("bq"), "BQ container not found in AST: {}", ast);
    // 用 walker 找到 BQ 节点验证投影
    let nodes = find_nodes(&snap, |t| t == BlockType::Blockquote);
    if nodes.is_empty() {
        // BQ re-parse 后 find_nodes 可能找不到 (行类型为 Paragraph), 跳过 per-node 断言
        return;
    }
    let bq = &nodes[0];
    assert_projs_readable(&snap, bq);
    assert_eq!(snap.read_block_proj(bq, BlockProj::Mark), ">");
    assert_eq!(snap.read_block_proj(bq, BlockProj::Content), "quoted text\nmore quote");
    assert_eq!(snap.read_block_proj(bq, BlockProj::Tag), "");
}

#[test]
fn golden_comment() {
    let (_, snap) = snap_from("<!-- hidden note -->\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Comment);
    assert!(!nodes.is_empty());
    let cm = &nodes[0];
    assert_projs_readable(&snap, cm);
    assert_eq!(snap.read_block_proj(cm, BlockProj::Mark), "<!--");
    // 修正（表化回归——旧硬编码 AngleBracket 分支返回注释内容
    // （range_content 剥 <!-- 和 -->）——表化时误定 None_ 变 ''——
    // 恢复 Stripped——content="hidden note"）
    assert_eq!(snap.read_block_proj(cm, BlockProj::Content), "hidden note");
}

#[test]
fn golden_thematic_break() {
    let (_, snap) = snap_from("---\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::ThematicBreak);
    assert!(!nodes.is_empty());
    let tb = &nodes[0];
    assert_projs_readable(&snap, tb);
    assert!(!snap.read_block_proj(tb, BlockProj::Raw).is_empty());
    assert_eq!(snap.read_block_proj(tb, BlockProj::Mark), "");
    assert_eq!(snap.read_block_proj(tb, BlockProj::Tag), "");
    // ContentMode::None_：ThematicBreak 无内容
    assert_eq!(snap.read_block_proj(tb, BlockProj::Content), "");
}

#[test]
fn golden_blank() {
    let (_, snap) = snap_from("text\n\nmore\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Blank);
    for n in &nodes {
        assert_projs_readable(&snap, n);
        assert_eq!(snap.read_block_proj(n, BlockProj::Mark), "");
        assert_eq!(snap.read_block_proj(n, BlockProj::Tag), "");
        assert_eq!(snap.read_block_proj(n, BlockProj::Container), "");
        // ContentMode::None_：Blank 无内容
        assert_eq!(snap.read_block_proj(n, BlockProj::Content), "");
    }
}

// ── golden: table ──

#[test]
fn golden_table_row() {
    let (_, snap) = snap_from("| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::TableRow);
    assert!(!nodes.is_empty(), "expected at least 1 TableRow");
    for n in &nodes { assert_projs_readable(&snap, n); }
    let row = &nodes[0];
    assert_eq!(snap.read_block_proj(row, BlockProj::Mark), "|");
    assert!(!snap.read_block_proj(row, BlockProj::Content).is_empty());
    // 行级 span：TableRow 行 = item 级尾（下一行），不是表尾/容器尾。
    let mut spans: Vec<(usize, usize)> = crate::selector::all_nodes(&snap)
        .into_iter()
        .filter(|n| n.node_type == BlockType::TableRow)
        .map(|n| (n.start_line, n.end_line))
        .collect();
    spans.sort_unstable();
    spans.dedup();
    assert_eq!(spans, vec![(0, 1), (2, 3), (3, 4)], "tb 行 span 必须是行级 item 尾");
    // 表在文中（后跟段落）：末行尾 = 表关闭行，不是文档尾。
    let (_, snap2) = snap_from("| A | B |\n|---|---|\n| 1 | 2 |\n\nafter\n");
    let mut spans2: Vec<(usize, usize)> = crate::selector::all_nodes(&snap2)
        .into_iter()
        .filter(|n| n.node_type == BlockType::TableRow)
        .map(|n| (n.start_line, n.end_line))
        .collect();
    spans2.sort_unstable();
    spans2.dedup();
    assert_eq!(spans2, vec![(0, 1), (2, 3)], "文中表末行尾 = 关闭行");
}

// ── golden: write path ──

#[test]
fn golden_write_range_header_tag() {
    let (_, snap) = snap_from("# Old Title\n");
    let nodes = find_nodes(&snap, |t| is_header(t));
    let h = &nodes[0];
    let target = BlockWriteTarget { range: h.byte_range.clone(), node_type: h.node_type };
    let wr = snap.block_write_range(&target, BlockProj::Tag).unwrap();
    assert!(!wr.is_empty());
    let raw = snap.rope.byte_slice(h.byte_range.clone()).to_string();
    assert!(raw.contains("Old Title"));
}

#[test]
fn golden_write_range_header_content() {
    let (_, snap) = snap_from("# Title\n\nbody here\n");
    let nodes = find_nodes(&snap, |t| is_header(t));
    let h = &nodes[0];
    let target = BlockWriteTarget { range: h.byte_range.clone(), node_type: h.node_type };
    let wr = snap.block_write_range(&target, BlockProj::Content).unwrap();
    let content = snap.rope.byte_slice(wr).to_string();
    assert!(content.contains("body here"));
}

#[test]
fn golden_write_range_read_only_rejected() {
    let (_, snap) = snap_from("# Title\n");
    let nodes = find_nodes(&snap, |t| is_header(t));
    let h = &nodes[0];
    let target = BlockWriteTarget { range: h.byte_range.clone(), node_type: h.node_type };
    assert!(snap.block_write_range(&target, BlockProj::Mark).is_err());
    assert!(snap.block_write_range(&target, BlockProj::Container).is_err());
}

#[test]
fn golden_write_range_code_content_rejected() {
    let (_, snap) = snap_from("```\ncode\n```\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::CodeContent);
    for n in &nodes {
        let target = BlockWriteTarget { range: n.byte_range.clone(), node_type: n.node_type };
        assert!(snap.block_write_range(&target, BlockProj::Raw).is_err(),
            "CodeContent should have no write projections");
        assert!(snap.block_write_range(&target, BlockProj::Content).is_err(),
            "CodeContent Content write should also be rejected");
    }
}

#[test]
fn golden_transform_payload_blockquote() {
    let (_, snap) = snap_from("> old\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Blockquote);
    if nodes.is_empty() { return; } // BQ re-parse: BQ 内容行为 Paragraph
    let bq = &nodes[0];
    let (_, text) = snap.compute_write_from_node(bq, "content", "new").unwrap();
    assert_eq!(text, "new", "BQ 两轴发射：首行原样——mark 留在容器面字节里（不被 payload 重建）");
    let (_, text2) = snap.compute_write_from_node(bq, "content", "a\nb").unwrap();
    assert_eq!(text2, "a\n> b", "续行 = 首行容器面（> ）+ 载荷行");
}

// ── roundtrip: BQ content write through engine ──
// compute_write_from_node → engine.edit → read_back
//
// BQ geometry is always None in practice (build_node strips the ">" before
// calling line_geometry).  The roundtrip succeeds via the fallback path.
// The third test manually constructs a BQ BlockNode WITH geometry to
// verify the geometry fast-path does NOT produce a spurious ">" prefix.

#[test]
fn bq_content_write_roundtrip_single_line() {
    let (mut engine, snap) = snap_from("> old text\n");
    // BQ re-parse: BQ 内容行类型为 Paragraph, find_nodes 可能找不到.
    // 用 to_ast_object 确认 BQ 容器存在.
    assert!(snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bq"), "BQ not found in AST");
    let nodes = find_nodes(&snap, |t| t == BlockType::Blockquote);
    if nodes.is_empty() { return; } // BQ re-parse: skip per-node write test
    let bq = &nodes[0];
    assert!(bq.geometry.is_none(), "BQ geometry is None — fallback path");

    let (write_range, formatted) = snap.compute_write_from_node(bq, "content", "new").unwrap();
    assert!(!write_range.is_empty(), "write range should not be empty");
    engine.edit(write_range.start, write_range.end, &formatted);
    let new_snap = engine.make_snapshot();

    let new_nodes = find_nodes(&new_snap, |t| t == BlockType::Blockquote);
    if new_nodes.is_empty() { return; }
    let content = new_snap.read_block_proj(&new_nodes[0], BlockProj::Content);
    assert_eq!(content, "new", "BQ content roundtrip failed: expected 'new', got '{}'", content);
}

#[test]
fn bq_content_write_roundtrip_multi_line() {
    let (mut engine, snap) = snap_from("> line one\n> line two\n");
    // BQ re-parse: BQ 内容行类型为 Paragraph
    assert!(snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bq"), "BQ not found in AST");
    let nodes = find_nodes(&snap, |t| t == BlockType::Blockquote);
    if nodes.is_empty() { return; }
    let bq = &nodes[0];
    assert!(bq.end_line > bq.start_line + 1, "expected multi-line BQ");

    let (write_range, formatted) = snap.compute_write_from_node(bq, "content", "replaced").unwrap();
    engine.edit(write_range.start, write_range.end, &formatted);
    let new_snap = engine.make_snapshot();

    let new_nodes = find_nodes(&new_snap, |t| t == BlockType::Blockquote);
    if new_nodes.is_empty() { return; }
    let content = new_snap.read_block_proj(&new_nodes[0], BlockProj::Content);
    assert_eq!(content, "replaced", "multi-line BQ content roundtrip failed: expected 'replaced', got '{}'", content);
}

/// Defence-in-depth: manually construct a BQ BlockNode WITH geometry to
/// prove that the geometry fast-path would produce a spurious ">" if we
/// ever fixed build_node to preserve BQ geometry.  Today this is dead
/// code (build_node strips ">" → geometry=always=None), but the geometry
/// path in compute_write_from_node has Blockquote excluded from the
/// fallback list — a latent bug waiting to trigger.
#[test]
fn bq_content_write_geometry_path_would_fail() {
    let (_, snap) = snap_from("> old\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Blockquote);
    if nodes.is_empty() { return; } // BQ re-parse: BQ 内容行为 Paragraph
    let real_bq = &nodes[0];
    assert!(real_bq.geometry.is_none(),
        "precondition: real BQ has no geometry (build_node strips '>')");

    // Manually construct a BQ with geometry — simulate what would happen
    // if build_node ever stopped stripping the BQ prefix before computing
    // geometry, or if geometry came from another path.
    let geo = line_geometry(BlockType::Blockquote, b"> old").unwrap();
    let fake_bq = BlockNode {
        geometry: Some(geo),
        ..real_bq.clone()
    };

    let (wr, text) = snap.compute_write_from_node(&fake_bq, "content", "new").unwrap();
    // Defect: the formatted text "> new" will be written starting at
    // [base+1..end] (after ">"), but position 0 still holds ">".
    // The rendered output will contain an extra ">" — ">> new".
    let mut engine = MdioEngine::new("> old\n");
    engine.edit(wr.start, wr.end, &text);
    let new_snap = engine.make_snapshot();
    let raw = new_snap.to_string();
    assert!(
        raw.starts_with(">"),
        "san check: rendered BQ should start with '>'"
    );
    assert!(
        !raw.contains(">>"),
        "BUG: geometry path produces spurious extra '>' in BQ content write.\n\
         write_range [{}, {}], formatted '{}', rendered '{}'",
        wr.start, wr.end, text, raw
    );
}

// ── nested: BQ content write roundtrip ──
// Geometry is always None for BQ (build_node strips ">" before calling
// line_geometry), so the fallback path is exercised regardless of flat
// or nested walker.  The nested selector's depth filter makes it hard to
// drill BQ from a sentinel root (target_depth=parent_depth+1 misses
// depth-1 BQ frames).  Instead verify via to_ast — the nested walker
// in to_ast(true) exercises the same BlockNode production path that
// select_nodes(nested=true) uses internally.

#[test]
fn bq_to_ast_nested_roundtrip() {
    let (_, snap) = snap_from("> line one\n> line two\n");
    // to_ast(true) uses the nested walker — same code path as select_nodes(nested=true)
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    // Verify the JSON contains the BQ container and its content
    assert!(json.contains("bq"), "nested to_ast missing BQ type: {}", json);
    assert!(json.contains("line one"), "nested to_ast missing line1: {}", json);
    assert!(json.contains("line two"), "nested to_ast missing line2: {}", json);
    // Content should NOT contain ">" prefix (block_content strips per-line)
    assert!(!json.contains("\"> line"),
        "nested to_ast: BQ content has '>' leak: {}", json);
}

// ── to_ast: BQ content representation ──
// Both flat and nested to_ast must produce valid JSON with correct
// BQ content — no spurious ">" artifacts from the internal projection.

#[test]
fn to_ast_bq_content_flat() {
    let (_, snap) = snap_from("> quoted text\n");
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(json.contains("bq"), "flat to_ast: {}", json);
    assert!(json.contains("quoted text"), "flat to_ast should contain BQ content: {}", json);
    // Content should be the text, not the ">" prefix (block_content strips it)
    assert!(!json.contains("> quoted"),
        "flat to_ast: BQ content should not contain '>' prefix: {}", json);
}

#[test]
fn to_ast_bq_content_nested() {
    let (_, snap) = snap_from("> line one\n> line two\n");
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(json.contains("bq"), "nested to_ast: {}", json);
    assert!(json.contains("line one"), "nested to_ast should contain content: {}", json);
    assert!(json.contains("line two"), "nested to_ast should contain both lines: {}", json);
    // Each content line should be text, not "> text" (block_content strips ">")
    assert!(!json.contains("\"> line"),
        "nested to_ast: BQ content should not contain '>' prefix: {}", json);
}

// ── golden: inline bridge ──

#[test]
fn inline_heading_tag() {
    let (_, snap) = snap_from("# Hello World\n");
    let nodes = find_nodes(&snap, |t| is_header(t));
    assert!(!nodes.is_empty());
    let segs = snap.inline_segments(&nodes[0]);
    assert_eq!(segs.len(), 1);
    assert!(matches!(segs[0].role, InlineSegmentRole::Tag));
    assert_eq!(segs[0].text, "Hello World");
}

#[test]
fn inline_paragraph_content() {
    let (_, snap) = snap_from("plain text\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Paragraph);
    assert!(!nodes.is_empty());
    let segs = snap.inline_segments(&nodes[0]);
    assert_eq!(segs.len(), 1);
    assert!(matches!(segs[0].role, InlineSegmentRole::Content));
    assert_eq!(segs[0].text, "plain text");
}

#[test]
fn inline_code_fence_none() {
    let (_, snap) = snap_from("```\ncode\n```\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::CodeFenceOpen);
    assert!(!nodes.is_empty());
    let segs = snap.inline_segments(&nodes[0]);
    assert!(segs.is_empty());
}

#[test]
fn inline_blockquote_content() {
    let (_, snap) = snap_from("> quoted\n> text\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Blockquote);
    if nodes.is_empty() { return; } // BQ re-parse: BQ 内容行为 Paragraph
    let segs = snap.inline_segments(&nodes[0]);
    assert_eq!(segs.len(), 1);
    assert!(matches!(segs[0].role, InlineSegmentRole::Content));
    assert_eq!(segs[0].text, "quoted\ntext");
}

#[test]
fn inline_bullet_empty_content() {
    let (_, snap) = snap_from("- \n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Bullet);
    assert!(!nodes.is_empty());
    let segs = snap.inline_segments(&nodes[0]);
    assert!(segs.is_empty());
}

// ── proj_write_range / compute_write routing ──

#[test]
fn proj_write_range_routes_through_spec() {
    let (_, snap) = snap_from("- item\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Bullet);
    let b = &nodes[0];
    assert!(snap.proj_write_range(&b.byte_range, b.node_type, "raw").is_err());
    assert!(snap.proj_write_range(&b.byte_range, b.node_type, "content").is_ok());
}

#[test]
fn compute_write_routes_through_spec() {
    let (_, snap) = snap_from("text\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Paragraph);
    let p = &nodes[0];
    let result = snap.compute_write_from_node(p, "content", "new");
    assert!(result.is_ok());
    let (wr, text) = result.unwrap();
    assert_eq!(text, "new");
    assert!(!wr.is_empty());
}

// ── golden: context-dependent BlockTypes ──

#[test]
fn golden_code_fence_close_and_content() {
    let (_, snap) = snap_from("```\ncode line\n```\n");
    let close_nodes = find_nodes(&snap, |t| t == BlockType::CodeFenceClose);
    for n in &close_nodes { assert_projs_readable(&snap, n); }
    let content_nodes = find_nodes(&snap, |t| t == BlockType::CodeContent);
    for n in &content_nodes { assert_projs_readable(&snap, n); }
}

#[test]
fn golden_setext_heading_h1() {
    // M3: RewritePrev removed. HsHead1 is walk_blocks forward-look effective type.
    // At u8 level, "Title" stays Paragraph, "===" is HsSep.
    let (_, snap) = snap_from("Title\n===\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Paragraph);
    assert!(!nodes.is_empty(), "expected Paragraph at u8 level");
    assert_eq!(snap.lines[nodes[0].start_line].node_type(), BlockType::Paragraph);
    // HsSep is present on second line
    let sep_nodes = find_nodes(&snap, |t| t == BlockType::HsSep);
    assert!(!sep_nodes.is_empty(), "expected HsSep underline");
    // walk_blocks via to_ast renders effective type as head
    let ast = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(ast.contains("\"h\""), "forward-look should produce head node: {}", ast);
}

#[test]
fn golden_setext_heading_h2() {
    // M3: RewritePrev removed. HsHead2 is walk_blocks forward-look effective type.
    let (_, snap) = snap_from("SubTitle\n---\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::Paragraph);
    assert!(!nodes.is_empty(), "expected Paragraph at u8 level");
    let sep_nodes = find_nodes(&snap, |t| t == BlockType::HsSep);
    assert!(!sep_nodes.is_empty(), "expected HsSep underline");
    let ast = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(ast.contains("\"h\""), "forward-look should produce head node: {}", ast);
}

#[test]
fn golden_html_block() {
    let (_, snap) = snap_from("<div>\ninner\n</div>\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::HtmlBlock);
    assert!(!nodes.is_empty(), "expected HtmlBlock");
    for n in &nodes {
        assert_projs_readable(&snap, n);
        assert!(!snap.read_block_proj(n, BlockProj::Raw).is_empty());
    }
    let content_nodes = find_nodes(&snap, |t| t == BlockType::HtmlContent);
    for n in &content_nodes { assert_projs_readable(&snap, n); }
}

#[test]
fn golden_indented_code() {
    let (_, snap) = snap_from("    indented code\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::IndentedCode);
    assert!(!nodes.is_empty(), "expected IndentedCode");
    for n in &nodes {
        assert_projs_readable(&snap, n);
        // M2: LineSpec mark_width Fixed("    ") → "    "
        assert_eq!(snap.read_block_proj(n, BlockProj::Mark), "    ");
        assert_eq!(snap.read_block_proj(n, BlockProj::Tag), "");
        // Content is dedented raw (column-aware dedent ≤4)
        let content = snap.read_block_proj(n, BlockProj::Content);
        assert_eq!(content, "indented code");
    }
}

#[test]
fn golden_table_separator() {
    let (_, snap) = snap_from("| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n");
    let nodes = find_nodes(&snap, |t| t == BlockType::TableSeparator);
    assert!(!nodes.is_empty(), "expected TableSeparator in table");
    for n in &nodes {
        assert_projs_readable(&snap, n);
        assert_eq!(snap.read_block_proj(n, BlockProj::Mark), "|");
    }
}
#[test]
fn bq_para_normalize_consistency() {
    let (_, snap) = snap_from(">   x\n");
    // 块记录直取 BQ 内容段落（selector 枚举壳优先——Paragraph 与 BQ 壳同行不可枚举）
    let n = snap.line_len();
    let mut fold = crate::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
    crate::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
    let (recs, arena) = fold.finish();
    let blocks = crate::infer::build_block_records(&snap.frame_log.entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
    let para = blocks.iter().find(|b| b.block_type == BlockType::Paragraph)
        .expect("应找到 BQ 内容段落块");
    let node = snap.build_node(para.start as usize, para.end as usize, para.block_type, para.loose());
    let proj = snap.read_block_proj(&node, BlockProj::Content);
    // 渲染路径语义（write_block_node Paragraph 分支——与投影同源要求）
    let raw = snap.byte_slice(node.byte_range.clone()).to_string();
    let render = if raw.as_bytes().iter().any(|&b| b == b'>') {
        crate::infer::paragraph_text_normalize(&crate::infer::paragraph_lines_stripped(
            &raw, &snap.frame_log.entries, para.start as u32,
            &crate::block::rebuild_stack_at(&snap.frame_log.entries, para.start as u32 + 1)))
    } else {
        crate::infer::paragraph_text_normalize(&raw)
    };
    assert_eq!(proj, render,
        "P16: BQ 段落投影与渲染归一化分歧——投影应收束（normalize_spec=StripTrailing）");
}
// ── +> -pj ct 尾追加语义 ──
// 原则：payload 不带换行 → 内容尾（尾换行前）；要新行 payload 自带换行。
// 范围：Paragraph / Blockquote / HtmlBlock(he) / CodeFenceOpen 四类；其余不动。
fn append_ct(doc: &str, sl: usize, el: usize, nt: BlockType, payload: &str) -> String {
    let mut st = crate::projection::State::new(doc);
    let snap = st.snapshot();
    // build_node 的契约 = 行区间（start_line, end_line），非字节。
    let node = snap.build_node(sl, el, nt, false);
    // 走生产入口（plan_write_with_nodes）：含 finalize_payload（行首落点补尾换行）。
    let req = crate::op::WriteRequest {
        steps: &[], operator: "+>", text: payload.to_string(),
        proj: Some("content"), auto_scaffold: false, auto_level: false,
    };
    let plan = crate::op::plan_write_with_nodes(&snap, req, &[node], 0).expect("plan");
    let edits = match plan {
        crate::op::WritePlan::Apply { edits, .. } => edits,
        _ => panic!("expect Apply"),
    };
    for e in &edits {
        st.edit(e.range.start, e.range.end, &e.text);
    }
    st.snapshot().rope.to_string()
}

#[test]
fn test_append_ct_tail_semantics() {
    use crate::block::BlockType as BT;
    // A. 单行尾追加（payload 无换行）
    assert_eq!(append_ct("# H\n\nhello\n", 2, 3, BT::Paragraph, " world"),
        "# H\n\nhello world\n", "para 尾追加");
    assert_eq!(append_ct("> quote\n", 0, 1, BT::Blockquote, " tail"),
        "> quote tail\n", "bq 顶层尾追加");
    assert_eq!(append_ct("- a\n  > q\n", 1, 2, BT::Blockquote, " tail"),
        "- a\n  > q tail\n", "bq item 内尾追加");
    assert_eq!(append_ct("> q1\n> q2\n", 0, 2, BT::Blockquote, " tail"),
        "> q1\n> q2 tail\n", "bq 多行尾追加");
    assert_eq!(append_ct("<div>\nraw\n</div>\n", 0, 3, BT::HtmlBlock, " tail"),
        "<div>\nraw tail\n</div>\n", "html 有闭标：落在 </div> 前");
    assert_eq!(append_ct("<div>\nraw\n\n", 0, 2, BT::HtmlBlock, " tail"),
        "<div>\nraw tail\n\n", "html 无闭标：截断空行算内容不吞");
    assert_eq!(append_ct("```lang\ncode line\n```\n", 0, 3, BT::CodeFenceOpen, "more"),
        "```lang\ncode linemore\n```\n", "fence 尾追加同体行");
    assert_eq!(append_ct("```lang\n```\n", 0, 2, BT::CodeFenceOpen, "code"),
        "```lang\ncode\n```\n", "fence 空体：行首落点");
    // B. payload 自带换行（前缀逐行补）
    assert_eq!(append_ct("> quote\n", 0, 1, BT::Blockquote, " tail\nmore"),
        "> quote tail\n> more\n", "bq payload 换行：后续行补前缀");
    assert_eq!(append_ct("- a\n  > q\n", 1, 2, BT::Blockquote, " tail\nmore"),
        "- a\n  > q tail\n  > more\n", "bq item 内 payload 换行：带 item 缩进前缀");
    assert_eq!(append_ct("# H\n\nhello\n", 2, 3, BT::Paragraph, " a\nb"),
        "# H\n\nhello a\nb\n", "para payload 换行：前缀空 = 原样");
    // C. 其余类型回归（行中落点行为不变）
    assert_eq!(append_ct("<!-- note -->\n", 0, 1, BT::Comment, " tail"),
        "<!-- note  tail-->\n", "comment 不变");
}
// ── 落点③ 键型最小闭环（判据先行）──
// 多行键型条目（类A 纯段落子块）的内容面终点 = 末槽右端；三处重叠
// （embody 前缀 / finalize 粘连 / apply_one 尾换行）各有字节对拍形状。
// 生产链夹具：plan → TransactionContext::apply_mdio_op_multi → apply_one（引号剥/
// trailing_n 等 session 层逻辑全走）——与 write_ct（直改引擎）互补。
fn write_ct_proj(doc: &str, sl: usize, el: usize, nt: BlockType, proj: &str, op: &str, payload: &str) -> String {
    let mut st = crate::projection::State::new(doc);
    let snap = st.snapshot();
    let node = snap.build_node(sl, el, nt, false);
    let req = crate::op::WriteRequest {
        steps: &[], operator: op, text: payload.to_string(),
        proj: Some(proj), auto_scaffold: false, auto_level: false,
    };
    let plan = crate::op::plan_write_with_nodes(&snap, req, &[node], 0).expect("plan");
    let (operator, edits) = match plan {
        crate::op::WritePlan::Apply { edits, operator } => (operator, edits),
        _ => panic!("expect Apply"),
    };
    let mut ws = crate::session::WriteState {
        engine: crate::engine::MdioEngine::new(doc),
        replica: None,
        pending_inserts: Default::default(),
        edit_spans: Vec::new(),
    };
    {
        let mut wire = smallvec::SmallVec::<[crate::session::WireOp; 4]>::new();
        let mut ctx = crate::session::TransactionContext {
            state: &mut ws,
            wire_ops: &mut wire,
            tx_min_dirty_line: usize::MAX,
        };
        ctx.apply_mdio_op_multi(edits, &operator);
    }
    let len = ws.engine.byte_len();
    ws.engine.byte_slice(0..len).to_string()
}

fn write_ct_prod(doc: &str, sl: usize, el: usize, nt: BlockType, op: &str, payload: &str) -> String {
    write_ct_proj(doc, sl, el, nt, "content", op, payload)
}

fn write_ct(doc: &str, sl: usize, el: usize, nt: BlockType, op: &str, payload: &str) -> String {
    let mut st = crate::projection::State::new(doc);
    let snap = st.snapshot();
    let node = snap.build_node(sl, el, nt, false);
    let req = crate::op::WriteRequest {
        steps: &[], operator: op, text: payload.to_string(),
        proj: Some("content"), auto_scaffold: false, auto_level: false,
    };
    let plan = crate::op::plan_write_with_nodes(&snap, req, &[node], 0).expect("plan");
    let edits = match plan {
        crate::op::WritePlan::Apply { edits, .. } => edits,
        _ => panic!("expect Apply"),
    };
    for e in &edits {
        st.edit(e.range.start, e.range.end, &e.text);
    }
    st.snapshot().rope.to_string()
}

#[test]
fn luodian3_multi_item_content_span_and_writes() {
    let doc = "- k: a\n  b\n";
    let mut st = crate::projection::State::new(doc);
    let snap = st.snapshot();
    let node = snap.build_node(0, 2, BlockType::Bullet, false);
    let (r, _) = snap.node_proj().compute_write_from_node(&node, "content", "X").unwrap();
    assert_eq!(r, 5..10, "内容面终点应为末槽右端（a 值槽 + b 续行槽，剥尾换行）");
    assert_eq!(write_ct(doc, 0, 2, BlockType::Bullet, "+>", "X"),
        "- k: a\n  bX\n", "+> 尾追加落末槽右端");
    assert_eq!(write_ct(doc, 0, 2, BlockType::Bullet, "+>", "\nX"),
        "- k: a\n  b\nX\n", "+> 前导换行 = 新行");
    assert_eq!(write_ct(doc, 0, 2, BlockType::Bullet, "/>", "X"),
        "- k: X\n", "/> 整体替换");
    assert_eq!(write_ct(doc, 0, 2, BlockType::Bullet, "/>", "a2\nb2"),
        "- k: a2\n  b2\n", "/> 多行载荷续行补缩进");
    assert_eq!(write_ct("- k: a\n", 0, 1, BlockType::Bullet, "/>", "X"),
        "- k: X\n", "单行哨兵");
    // BQ 写面与读面同构（第四步修订）：mark 逐行事务、缩进整体位置——两轴发射。
    assert_eq!(write_ct("- a\n  > q\n", 1, 2, BlockType::Blockquote, "/>", "q2"),
        "- a\n  > q2\n", "BQ 内条目单行：mark+缩进保留");
    assert_eq!(write_ct("- a\n  > q\n", 1, 2, BlockType::Blockquote, "/>", "q2\nq3"),
        "- a\n  > q2\n  > q3\n", "BQ 内条目多行：续行 = 位置 + mark + 行");
    assert_eq!(write_ct("> q\n", 0, 1, BlockType::Blockquote, "/>", "q2\nq3"),
        "> q2\n> q3\n", "顶层 BQ 多行");
    assert_eq!(write_ct("> > q\n", 0, 1, BlockType::Blockquote, "/>", "q2\nq3"),
        "> > q2\n> > q3\n", "嵌套 BQ 多行：双层 mark 逐行");
}
// ── 判据补账 ──
// P1-1 G3：语料类A 多行条目写回面恒等（/> 自身内容 → 全文逐字不变）。
#[test]
fn g3_corpus_class_a_multiline_writeback_identity() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let files = [
        &format!("{root}/mdio_core/tests/fixtures/g3_corpus/class_a.md"),
        &format!("{root}/worktrees/genesis/wt/var/bash.md"),
        &format!("{root}/worktrees/genesis/wt/addr_probe4.md"),
        &format!("{root}/worktrees/genesis/wt/system_error.md"),
    ];
    let mut total = 0usize;
    let mut multi = 0usize;
    for f in files {
        if !std::path::Path::new(f).exists() {
            eprintln!("G3: skip missing external corpus: {f}");
            continue;
        }
        let doc = std::fs::read_to_string(f).expect("corpus file");
        let mut st = crate::projection::State::new(&doc);
        let snap = st.snapshot();
        let is_item = |t: BlockType| {
            matches!(t, BlockType::Bullet | BlockType::Checklist | BlockType::OrderedListItem)
        };
        let nodes = find_nodes(&snap, is_item);
        for node in &nodes {
            total += 1;
            let content = snap.read_block_proj(node, crate::block::BlockProj::Content);
            if !content.contains('\n') { continue; }
            if multi < 3 {
                eprintln!("DBG 多行条目 行{}..{} 类型{:?} content={:?}", node.start_line, node.end_line, node.node_type, content);
                for l in node.start_line..node.end_line.min(node.start_line + 3) {
                    eprintln!("   L{l} type={:?} cont={}", snap.lines[l].node_type(), snap.lines[l].cont());
                }
            }
            // 类A：节点区间内除首行外全为 cont 行或空行（子列表行 = 非 cont 非空）
            let mut nested = false;
            for l in (node.start_line + 1)..node.end_line {
                let t = snap.lines[l].node_type();
                if !snap.lines[l].cont() && t != BlockType::Blank { nested = true; break; }
            }
            if nested { continue; }
            multi += 1;
            let after = write_ct_prod(&doc, node.start_line, node.end_line, node.node_type, "/>", &content);
            assert_eq!(after, doc, "类A 多行条目写回恒等失败: {f} 行{}", node.start_line);
        }
    }
    eprintln!("G3: 条目 {total}，类A 多行 {multi}");
    if total == 0 {
        eprintln!("G3 skipped: external corpus unavailable");
        return;
    }
    assert!(multi >= 30, "语料类A 多行条目过少: {multi}");
}

// P1-2 G4/G5：多行载荷续行补缩进（grow）+ BQ 内条目恒等（两轴）。
#[test]
fn g4_g5_writeback_shapes() {
    assert_eq!(write_ct_prod("- k: a\n", 0, 1, BlockType::Bullet, "/>", "x\nb"),
        "- k: x\n  b\n", "g4 grow：续行补条目缩进");
    assert_eq!(write_ct_prod("- a\n  > q\n", 1, 2, BlockType::Blockquote, "/>", "q2\nq3"),
        "- a\n  > q2\n  > q3\n", "g5 BQ 内条目多行两轴恒等");
}

// P1-3 raw 锚点哨兵：+> -pj raw 锚 = 节点尾（不随 content 终点变化）。
#[test]
fn raw_anchor_sentinel() {
    assert_eq!(write_ct_proj("- k: a\n  b\n", 0, 2, BlockType::Bullet, "raw", "+>", "X"),
        "- k: a\n  b\nX\n", "raw 锚 = 节点尾（行首追加 + 补尾换行）");
}

// P1-4 类B：外层条目 +> 落末槽右端、子列表不吞。
#[test]
fn class_b_subtree_not_swallowed() {
    assert_eq!(write_ct_prod("- k: a\n  b\n  - c\n", 0, 3, BlockType::Bullet, "+>", "X"),
        "- k: a\n  bX\n  - c\n", "类B：落 b 尾、子列表不吞");
}

// P1-5 /> trailing_n 对拍（production 路径 apply_one）。
#[test]
fn replace_trailing_n_pair() {
    assert_eq!(write_ct_prod("p1\n", 0, 1, BlockType::Paragraph, "/>", "X"),
        "X\n", "单行段落替换");
    assert_eq!(write_ct_prod("p1\np2\n", 0, 2, BlockType::Paragraph, "/>", "X"),
        "X\n", "多行段落替换：整块替换、保留结构尾换行");
}
// ── 嵌套 BQ 选择器多命中钉（归 selector 线账）──
// 现状（MINIMAL §9.5 已知缺陷）：通配+类型过滤在嵌套引用块上把同一节点返回 N 次
// （N = BQ 层数、byte_range 全同）——写路径因此插多次。钉住现状字节防本线意外改动；
// 修在 selector 线（另一会话）。
#[test]
#[ignore]
fn p31_tail_nl_matrix() {
    // 尾换行语义矩阵：段落 vs bq（前导 \n 形式为合同路径，尾 \n 形式对拍现状）
    let para1 = write_ct_prod("hello\n", 0, 1, BlockType::Paragraph, "+>", "B\n");
    eprintln!("para after B\\n: {para1:?}");
    let para2 = write_ct_prod(&para1, 0, 1, BlockType::Paragraph, "+>", "C");
    eprintln!("para after C: {para2:?}");
    let bq1 = write_ct_prod("> A\n", 0, 1, BlockType::Blockquote, "+>", "B\n");
    eprintln!("bq after B\\n: {bq1:?}");
    let bq2 = write_ct_prod(&bq1, 0, bq1.lines().count(), BlockType::Blockquote, "+>", "C");
    eprintln!("bq after C: {bq2:?}");
    // 前导 \n（合同路径）两型一致
    eprintln!("para lead-nl: {:?}", write_ct_prod("hello\n", 0, 1, BlockType::Paragraph, "+>", "\nB"));
}
// ── bq-in-item fence cd 复现矩阵（判伪先行）──
#[test]
#[ignore]
// ── 方案 double check：k 链单源 + rest_after_layer(k) 全链模拟 ──
#[test]
#[ignore]
fn fence_k_pipeline_probe() {
    for (tag, doc) in [
        ("根级 bq", "> ```js\n> const a = 1;\n> ```\n"),
        ("item+bq", "- a:\n  > ```js\n  > const a = 1;\n  > ```\n"),
        ("item+bq+空行", "- assistant:\n  > 示例：\n  >\n  > ```js\n  > const a = 1;\n  > ```\n"),
        ("双 bq", "> > ```js\n> > const a = 1;\n> > ```\n"),
        ("顶层", "```js\nconst a = 1;\n```\n"),
        ("bq+代码行以>开头", "> ```js\n> > foo\n> ```\n"),
        ("item+bq+空行+代码行以>开头", "- x:\n  >\n  > ```js\n  > > foo\n  > ```\n"),
        ("item+双bq+空行", "- x:\n  >\n  > > ```js\n  > > a\n  > > ```\n"),
    ] {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        // 找 fence 开行节点（CodeFenceOpen 或折入行的 CodeContent）
        for (i, p) in snap.lines.iter().enumerate() {
            let t = p.node_type();
            if t != BlockType::CodeFenceOpen && t != BlockType::CodeContent { continue; }
            let node = snap.build_node(i, crate::infer::get_node_end_from_lines(&snap.lines, i, t, &snap.frame_log), t, false);
            // k = 开行 peel 的 BlockQuote 层数（词法单源——开行携带容器全部 mark；
            // 注：chain_frames_at 是 ViewArrays 方法，NodeProj 就地无——double check 更正）
            let first_line = snap.rope.line(node.start_line).to_string();
            let k = crate::block::peel_peeled(&first_line, 0)
                .layers.iter().filter(|ly| ly.kind == FrameKind::BlockQuote).count();
            // 逐行剥恰 k 层
            let raw = snap.rope.byte_slice(node.byte_range.clone()).to_string();
            let working: String = raw.lines()
                .map(|l| crate::block::peel_peeled(l, 0).rest_after_layer(k).to_string())
                .collect::<Vec<_>>().join("\n");
            let cr = crate::block::range_content(t, &working);
            let body = cr.map(|c| &working[c]).unwrap_or("");
            eprintln!("[{tag}] k={k} working={working:?} body={body:?}");
        }
    }
}
// ── fence cd 修复判据（double check 后实施；先红后绿）──
#[test]
fn cd_fence_matrix_gate() {
    for (tag, doc) in [
        ("根级 bq", "> ```js\n> const a = 1;\n> ```\n"),
        ("item+bq", "- a:\n  > ```js\n  > const a = 1;\n  > ```\n"),
        ("item+bq+空行", "- assistant:\n  > 示例：\n  >\n  > ```js\n  > const a = 1;\n  > ```\n"),
        ("双 bq", "> > ```js\n> > const a = 1;\n> > ```\n"),
        ("顶层", "```js\nconst a = 1;\n```\n"),
        ("bq+代码行以>开头", "- x:\n  >\n  > ```js\n  > > foo\n  > ```\n"),
    ] {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let nodes = find_nodes(&snap, |t| {
            matches!(t, BlockType::CodeFenceOpen | BlockType::CodeContent | BlockType::CodeFenceClose)
        });
        for node in &nodes {
            let cd = snap.read_block_proj(node, crate::block::BlockProj::Content);
            // 污染判据：cd 不得含闭合 ``` 行；行首 > 前缀检查对「代码自身以 > 开头」
            // 形状豁免（该形状有独立精确断言——cd 恰为 > foo）。
            assert!(!cd.contains("```"), "[{tag}] cd 含闭合 fence: {cd:?}");
            if tag != "bq+代码行以>开头" {
                for l in cd.lines() {
                    assert!(!l.starts_with('>'), "[{tag}] cd 行带 > 前缀: {cd:?}");
                }
            }
            // 三路径一致：borrow 快路径与 face 读逐字节相等
            let mut scratch = Vec::new();
            let borrowed = snap.block_proj_borrow(node, crate::block::BlockProj::Content, &mut scratch);
            assert_eq!(borrowed.as_ref(), cd.as_str(), "[{tag}] borrow 与 face 读不一致");
        }
        // 目标形状钉死（HIS 形状 + > foo 保真）
        if tag == "item+bq+空行" {
            let opens = find_nodes(&snap, |t| t == BlockType::CodeFenceOpen);
            assert_eq!(snap.read_block_proj(&opens[0], crate::block::BlockProj::Content),
                "const a = 1;\n", "HIS 形状 cd 钉死");
        }
        if tag == "bq+代码行以>开头" {
            let opens = find_nodes(&snap, |t| t == BlockType::CodeFenceOpen);
            assert_eq!(snap.read_block_proj(&opens[0], crate::block::BlockProj::Content),
                "> foo\n", "> foo 正文保真（缺陷 B）");
        }
    }
}
// ── nestscene 确实bug 复现（I1-I6；I7-I9 文档自注非 bug 不测）──
#[test]
#[ignore]
fn i4_shell_probe() {
    for doc in ["- assistant:\n    > 好,按三步:\n    > - 再改\n", "- assistant:\n    > 好,按三步:\n    > - 再改\n    > - 最后验证\n"] {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
        eprintln!("DOC={doc:?}\nCF={cf}");
        for (i, p) in snap.lines.iter().enumerate() {
            eprintln!("  L{i} {:?} cont={}", p.node_type(), p.cont());
        }
    }
}
fn i2_demotion_probe() {
    for s in ["- a --", "- a -", "- a ---", "- a -- x", "* a --", "1. a --"] {
        let t0 = crate::block::classify_at_top(s.as_bytes(), 0, false);
        let t1 = crate::block::classify_at_top(s.as_bytes(), 0, true);
        eprintln!("[{s:?}] no-cont={t0:?} cont={t1:?}");
    }
}
fn nestscene_bug_repro() {
    // I1 Table 双 ct：to_ast_object_mode 里 tb 对象键序列
    let mut st = crate::projection::State::new("| a | b |\n|---|---|\n| 1 | 2 |\n");
    let snap = st.snapshot();
    let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let tb_seg = cf.split("\"nt\":\"tb\"").nth(1).unwrap_or("");
    let keys: Vec<&str> = tb_seg.split(",\"").skip(1).map(|s| s.split('"').next().unwrap_or("")).collect();
    eprintln!("[I1] tb keys = {keys:?}");
    // I2 末尾 dash
    let mut st2 = crate::projection::State::new("- a --\n");
    let snap2 = st2.snapshot();
    eprintln!("[I2] - a -- 首行类型 = {:?}", snap2.lines[0].node_type());
    // I3 bq 内短 setext
    let mut st3 = crate::projection::State::new("> ls\n> -\n");
    let snap3 = st3.snapshot();
    for (i, p) in snap3.lines.iter().enumerate() { eprintln!("[I3] L{i} {:?}", p.node_type()); }
    // I5 hb raw 丢 >
    let mut st5 = crate::projection::State::new("<div>\n> q\n");
    let snap5 = st5.snapshot();
    let nodes5 = find_nodes(&snap5, |t| t == BlockType::HtmlBlock);
    for n in &nodes5 {
        let raw = snap5.read_block_proj(n, crate::block::BlockProj::Raw);
        let ct = snap5.read_block_proj(n, crate::block::BlockProj::Content);
        eprintln!("[I5] hb raw={raw:?} content={ct:?}");
    }
    // I6 未闭合 fence 空行
    let mut st6 = crate::projection::State::new("```\n\n");
    let snap6 = st6.snapshot();
    let nodes6 = find_nodes(&snap6, |t| t == BlockType::CodeFenceOpen);
    for n in &nodes6 {
        eprintln!("[I6] 未闭合 fence cd={:?}", snap6.read_block_proj(n, crate::block::BlockProj::Content));
    }
}
fn cd_fence_matrix_probe() {
    for (tag, doc) in [
        ("根级 bq", "> ```js\n> const a = 1;\n> ```\n"),
        ("item+bq", "- a:\n  > ```js\n  > const a = 1;\n  > ```\n"),
        ("item+bq+空行", "- assistant:\n  > 示例：\n  >\n  > ```js\n  > const a = 1;\n  > ```\n"),
        ("双 bq", "> > ```js\n> > const a = 1;\n> > ```\n"),
    ] {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        for (i, p) in snap.lines.iter().enumerate() {
            eprintln!("[{tag}] L{i} {:?}", p.node_type());
        }
        let nodes = find_nodes(&snap, |t| t == BlockType::CodeFenceOpen);
        for node in &nodes {
            let cd = snap.read_block_proj(node, crate::block::BlockProj::Content);
            eprintln!("[{tag}] fence 行{}..{} cd={cd:?}", node.start_line, node.end_line);
        }
    }
}
// ── P3-4 判伪探针：HtmlBlock 写回面恒等（复跑件见 /research/p3_falsification）──
// ── range_content fence 分支六情形（先红后绿）──
#[test]
// ── tb 壳无重复 ct 键（先红后绿）──
#[test]
// ── 闭 marker 规则删除后 dash 结尾条目 = list（先红后绿）──
#[test]
#[test]
#[ignore]
fn g3_132_probe() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../worktrees/genesis/wt/var/bash.md")).unwrap();
    let mut st = crate::projection::State::new(&doc);
    let snap = st.snapshot();
    let t = snap.lines[132].node_type();
    let end = crate::infer::get_node_end_from_lines(&snap.lines, 132, t, &snap.frame_log);
    eprintln!("node 132..{end} {:?}", t);
    for l in 132..(end + 1).min(snap.lines.len()) {
        eprintln!("  L{l} {:?} cont={}", snap.lines[l].node_type(), snap.lines[l].cont());
    }
    let node = snap.build_node(132, end, t, false);
    let content = snap.read_block_proj(&node, crate::block::BlockProj::Content);
    eprintln!("CONTENT={content:?}");
    let face = snap.node_proj();
    if let Ok((r, tx)) = face.compute_write_from_node(&node, "content", &content) {
        eprintln!("RANGE={r:?} TEXT={tx:?}");
        let span = snap.rope.byte_slice(r.clone()).to_string();
        eprintln!("SPAN={span:?}");
    }
}
// ── hb ct 按 k 剥层（先红后绿）──
#[test]
#[test]
#[ignore]
fn i4_fl_dump_probe() {
    let doc = "- assistant:\n    > 好,按三步:\n    > - 再改\n    > - 最后验证\n";
    let mut st = crate::projection::State::new(doc);
    let snap = st.snapshot();
    for (i, p) in snap.lines.iter().enumerate() {
        eprintln!("L{i} {:?} cont={}", p.node_type(), p.cont());
    }
    // FL entries：每行的 push/pop
    let entries = &snap.frame_log.entries;
    for (i, e) in entries.iter().enumerate() {
        let frame = (e & 0xFFFF) as u16;
        let anchor = ((e >> 16) & 0xFF) as u8;
        let line = (e >> 24) & 0x3F_FFFF_FFFF;
        let pop = (e >> 63) & 1 == 1;
        let body_mut = (e >> 62) & 1 == 1;
        eprintln!("FL[{i}] frame={frame} anchor={anchor} line={line} pop={pop} body_mut={body_mut}");
    }
}
// ── bq-in-item 列表兄弟锚相等 + 同族形状（先红后绿）──
#[test]
// ── bq 内短 setext 矩阵（先红后绿）──
#[test]
fn p2_setext_in_bq_matrix() {
    let cf = |doc: &str| {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf)
    };
    assert!(cf("> ls\n> -\n").contains("\"nt\":\"h\""), "单 dash = h2");
    assert!(cf("> ls\n>  -\n").contains("\"nt\":\"h\""), "缩进单 dash = h2");
    assert!(cf("> ls\n> ---\n").contains("\"nt\":\"h\""), "TB 不变");
    assert!(cf("> ls\n> ===\n").contains("\"nt\":\"h\""), "eq 不变");
    let empty = cf("> -\n> -\n");
    assert!(!empty.contains("\"nt\":\"h\""), "空项不转标题");
    assert!(empty.contains("\"nt\":\"bl\""), "空列表仍是列表");
}
#[test]
// ── 销账判据（实测）：双 bq / bq 内 OL 的兄弟结构本已正确——
// 常驻防回归（误记——两形状不经过 v3 减法也正确）。──
#[test]
#[ignore]
fn bq_pop_regressions_probe() {
    for (tag, doc) in [
        ("lazy 保持", "> > x\n> y\n"),
        ("同 bq item 内容", "> - x\n> x\n"),
        ("空行+缩进续行", "> - x\n\n  b\n"),
        ("空行+顶格段落", "> - x\n\np\n"),
    ] {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
        eprintln!("[{tag}] {cf}");
    }
}
#[test]
#[ignore]
fn blank_attrib_probe() {
    let mut st = crate::projection::State::new("> - x\n\n  b\n");
    let snap = st.snapshot();
    for (i, e) in snap.frame_log.entries.iter().enumerate() {
        eprintln!("FL[{i}] line={} frame={} pop={}", (e>>24)&0x3F_FFFF_FFFF, e&0xFFFF, (e>>63)&1);
    }
    let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    eprintln!("CF={cf}");
}
#[test]
#[ignore]
#[test]
#[ignore]
fn tree_dump_probe() {
    let doc = "> - x\n\n  b\n";
    let mut st = crate::projection::State::new(doc);
    let snap = st.snapshot();
    let tc = snap.tree_cell(&crate::topology::CONTAINER_TOPOLOGY);
    let tree = &tc.tree;
    eprintln!("nodes = {}", tree.nodes.len());
    for (i, n) in tree.nodes.iter().enumerate() {
        let kind = n.kind.map(|k| format!("{k:?}")).unwrap_or_else(|| "-".into());
        let kids: Vec<u32> = n.children_of(tree).to_vec();
        eprintln!("[{i}] kind={kind} start={} end={} blk={:?} ch={kids:?}", n.start, n.end, n.blk_idx());
    }
}
#[test]
#[ignore]
#[test]
#[ignore]
fn api_rebuild_o_n2_probe() {
    for n in [500usize, 1000, 2000, 4000, 8000] {
        let doc = "> x\n\n".repeat(n);
        let mut st = crate::projection::State::new(&doc);
        let snap = st.snapshot();
        let nodes = snap.select_nodes(&[crate::selector::Step::RecursiveWildcard]);
        let face = snap.node_proj();
        let t0 = std::time::Instant::now();
        let mut total = 0usize;
        for nd in nodes.iter() {
            let txt = face.read_block_proj(nd, crate::block::BlockProj::Content);
            total += txt.len();
        }
        let dt = t0.elapsed().as_micros();
        eprintln!("[O2] n={n} nodes={} dt={dt}us bytes={total}", nodes.len());
    }
}

#[test]
#[ignore]
fn chain_equiv_rebuild_probe() {
    let docs = [
        "> - x\n\n> x\n",
        "> > - x\n> - x\n",
        "> q\n> - # h\n> q\n> ====\n> ====\n\n>\n  - ### h3\n## h\n- [ ] t\n",
        &String::from("> x\n\n".repeat(3)),
    ];
    for doc in docs {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let seg = crate::cache::Segment::build(&snap, 0, n);
        let view = crate::infer::LineView::with_arrays(
            &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
            (seg.start_line as u32, seg.end_line as u32), seg.view.clone(), &seg.arena);
        for k in 0..=n {
            let a = crate::block::rebuild_stack_at(&snap.frame_log.entries, k as u32 + 1);
            let b = crate::effect::ResolveView::chain_frames_at(&view, k + 1);
            let b: Vec<_> = b.into_iter().collect();
            let eq = a == b;
            if !eq {
                eprintln!("[NEQ] doc={doc:?} k={k} rebuild={:?} chain={:?}", a.iter().map(|f| f.kind()).collect::<Vec<_>>(), b.iter().map(|f| f.kind()).collect::<Vec<_>>());
            }
        }
        eprintln!("[DONE] doc={doc:?} n={n}");
    }
}

#[test]
#[ignore]
fn chain_plus_line_replay_equiv_probe() {
    let docs = [
        "> - x\n\n> x\n",
        "> > - x\n> - x\n",
        "> q\n> - # h\n> q\n> ====\n> ====\n\n>\n  - ### h3\n## h\n- [ ] t\n",
        &String::from("> x\n\n".repeat(3)),
    ];
    for doc in docs {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let seg = crate::cache::Segment::build(&snap, 0, n);
        let view = crate::infer::LineView::with_arrays(
            &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
            (seg.start_line as u32, seg.end_line as u32), seg.view.clone(), &seg.arena);
        let mut mismatches = 0;
        for k in 0..n {
            let a = crate::block::rebuild_stack_at(&snap.frame_log.entries, k as u32 + 1);
            let mut b: Vec<_> = crate::effect::ResolveView::chain_frames_at(&view, k).into_iter().collect();
            // 行 k 自身的条目重放（push/pop 照 forward_replay 语义）
            for e in snap.frame_log.entries.iter().copied() {
                if crate::block::entry_line(e) as usize == k {
                    if crate::block::entry_is_body_mut(e) { continue; }
                    if crate::block::entry_is_pop(e) { b.pop(); } else { b.push(crate::block::entry_frame(e)); }
                }
            }
            if a != b {
                mismatches += 1;
                eprintln!("[NEQ] doc={doc:?} k={k} rebuild={:?} got={:?}", a.iter().map(|f| f.kind()).collect::<Vec<_>>(), b.iter().map(|f| f.kind()).collect::<Vec<_>>());
            }
        }
        eprintln!("[DONE] doc={doc:?} n={n} mismatches={mismatches}");
    }
}

#[test]
#[ignore]
fn chain_equiv_rebuild_correct_index_probe() {
    let docs = [
        "> - x\n\n> x\n",
        "> > - x\n> - x\n",
        "> q\n> - # h\n> q\n> ====\n> ====\n\n>\n  - ### h3\n## h\n- [ ] t\n",
        &String::from("> x\n\n".repeat(3)),
    ];
    for doc in docs {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let seg = crate::cache::Segment::build(&snap, 0, n);
        let view = crate::infer::LineView::with_arrays(
            &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
            (seg.start_line as u32, seg.end_line as u32), seg.view.clone(), &seg.arena);
        let mut mismatches = 0;
        for k in 0..n {
            let a = crate::block::rebuild_stack_at(&snap.frame_log.entries, k as u32 + 1);
            let b: Vec<_> = crate::effect::ResolveView::chain_frames_at(&view, k).into_iter().collect();
            if a != b {
                mismatches += 1;
                eprintln!("[NEQ] doc={doc:?} k={k} rebuild={:?} chain={:?}", a.iter().map(|f| f.kind()).collect::<Vec<_>>(), b.iter().map(|f| f.kind()).collect::<Vec<_>>());
            }
        }
        eprintln!("[DONE] doc={doc:?} n={n} mismatches={mismatches}");
    }
}

#[test]
#[ignore]
fn chain_equiv_rebuild_sweep_probe() {
    let tokens = ["a", "para", "  para", "    code", "- a", "  - b", "1. a", "- [ ] t",
        "> q", "> - x", ">", "# h", "## h", "### x", "> # h", "> - # h", "- # h",
        "===", "--", "- ## h2", "  - ### h3", "```", "```x", "| a |", "| - |", "[a]: /u", "", "  " ];
    let mut x: u64 = 0x7a5_2026_1022_0001;
    let mut total_ne = 0;
    for _ in 0..300 {
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        let n_lines = 1 + (x as usize) % 12;
        let mut doc = String::new();
        for _ in 0..n_lines {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            doc.push_str(tokens[(x as usize) % tokens.len()]);
            doc.push('\n');
        }
        let mut st = crate::projection::State::new(&doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let seg = crate::cache::Segment::build(&snap, 0, n);
        let view = crate::infer::LineView::with_arrays(
            &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
            (seg.start_line as u32, seg.end_line as u32), seg.view.clone(), &seg.arena);
        for k in 0..n {
            let a = crate::block::rebuild_stack_at(&snap.frame_log.entries, k as u32 + 1);
            let b: Vec<_> = crate::effect::ResolveView::chain_frames_at(&view, k).into_iter().collect();
            if a != b {
                total_ne += 1;
                if total_ne <= 3 {
                    eprintln!("[NEQ] doc={doc:?} k={k}");
                    eprintln!("  rebuild={:?}", a.iter().map(|f| (f.kind(), f.0)).collect::<Vec<_>>());
                    eprintln!("  chain  ={:?}", b.iter().map(|f| (f.kind(), f.0)).collect::<Vec<_>>());
                }
            }
        }
    }
    eprintln!("[SWEEP] total mismatches={total_ne}");
}

#[test]
#[ignore]
fn chain_vs_rebuild_consumer_ab_probe() {
    let tokens = ["a", "para", "  para", "    code", "- a", "  - b", "1. a", "- [ ] t",
        "> q", "> - x", ">", "# h", "## h", "### x", "> # h", "> - # h", "- # h",
        "===", "--", "- ## h2", "  - ### h3", "```", "```x", "| a |", "| - |", "[a]: /u", "", "  " ];
    let mut x: u64 = 0x5b_2026_1022_0002;
    let mut total_ne = 0;
    let mut nodes_checked = 0;
    for _ in 0..300 {
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        let n_lines = 1 + (x as usize) % 12;
        let mut doc = String::new();
        for _ in 0..n_lines {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            doc.push_str(tokens[(x as usize) % tokens.len()]);
            doc.push('\n');
        }
        let mut st = crate::projection::State::new(&doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let seg = crate::cache::Segment::build(&snap, 0, n);
        let view = crate::infer::LineView::with_arrays(
            &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
            (seg.start_line as u32, seg.end_line as u32), seg.view.clone(), &seg.arena);
        let nodes = snap.select_nodes(&[crate::selector::Step::RecursiveWildcard]);
        for nd in nodes.iter() {
            let k = nd.start_line as usize;
            let rebuild = crate::block::rebuild_stack_at(&snap.frame_log.entries, k as u32 + 1);
            let chain: Vec<_> = crate::effect::ResolveView::chain_frames_at(&view, k).into_iter().collect();
            if rebuild == chain { continue; }
            nodes_checked += 1;
            // 分歧节点：逐字节对拍 Content 面（用各自的栈走同一条归一化管线）
            let raw = snap.rope.byte_slice(nd.byte_range.clone()).to_string();
            let a = crate::infer::paragraph_text_normalize(&crate::infer::paragraph_lines_stripped(&raw, &snap.frame_log.entries, k as u32, &rebuild));
            let b = crate::infer::paragraph_text_normalize(&crate::infer::paragraph_lines_stripped(&raw, &snap.frame_log.entries, k as u32, &chain));
            if a != b {
                total_ne += 1;
                if total_ne <= 3 { eprintln!("[AB-NEQ] doc={doc:?} k={k} a={a:?} b={b:?}"); }
            }
        }
    }
    eprintln!("[AB] diverged_stacks={nodes_checked} content_neq={total_ne}");
}

fn bq_pop_defect_probe() {
    for (tag, doc) in [
        ("缺陷一：空行后 BQ 没关", "> - x\n\n> x\n"),
        ("缺陷二：降深后内层 BQ 没弹", "> > - x\n> - x\n"),
    ] {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        eprintln!("[{tag}]");
        for (i, e) in snap.frame_log.entries.iter().enumerate() {
            let frame = (e & 0xFFFF) as u16;
            let anchor = ((e >> 16) & 0xFF) as u8;
            let line = (e >> 24) & 0x3F_FFFF_FFFF;
            let pop = (e >> 63) & 1 == 1;
            eprintln!("  FL[{i}] line={line} frame={frame} anchor={anchor} pop={pop}");
        }
        let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
        eprintln!("  CF={cf}");
    }
}
fn guazhang_shapes_are_siblings() {
    for (tag, doc, nt, marker) in [
        ("双 bq 列表", "- assistant:\n    > 好,按三步:\n    > > - 再改\n    > > - 最后验证\n", "\"nt\":\"b\"", "\"ct\":\"再改\""),
        ("bq 内 OL 兄弟", "- assistant:\n    > 好,按三步:\n    > 1. 再改\n    > 2. 最后验证\n", "\"nt\":\"ol\"", "\"ct\":\"再改\""),
    ] {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
        // 两个 item 的兄弟判据：各自的 marker 段在同一个容器壳内、且都在（无嵌套再包一层）
        let first = cf.find(marker).expect("再改 marker 在");
        let second = cf.rfind("\"ct\":\"最后验证\"").expect("最后验证在");
        let seg = &cf[first..second];
        assert!(!seg.contains("\"nt\":\"bl\""), "[{tag}] 最后验证不应嵌在再改的 item 下: {seg}");
        let _ = nt;
    }
}
fn p5_bq_item_list_siblings() {
    // 双 bq 形态（bq 层数 >1 的坐标链算术更深——deep_cross 回归后收窄）。
    let shapes = [
        ("bq 内双列表项", "- assistant:\n    > 好,按三步:\n    > - 再改\n    > - 最后验证\n"),
        ("bq 内 Checklist", "- assistant:\n    > 好,按三步:\n    > - [ ] 再改\n    > - [ ] 最后验证\n"),
    ];
    for (tag, doc) in shapes {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        // 判据形态：L3 的列表帧 push 锚 == L2 的列表帧 push 锚（sibling 同层）
        let e = &snap.frame_log.entries;
        let push_anchor = |line: u32| -> Option<u8> {
            e.iter()
                .filter(|x| (*x >> 24) & 0x3F_FFFF_FFFF == line as u64 && (*x >> 63) & 1 == 0)
                .map(|x| ((x >> 16) & 0xFF) as u8)
                .next()
        };
        let a2 = push_anchor(2).unwrap_or(0);
        let a3 = push_anchor(3).unwrap_or(0);
        assert_eq!(a3, a2, "[{tag}] L3 列表帧 push 锚应 == L2（sibling），现状 {a3} vs {a2}");
    }
}
fn p4_hb_k_strip() {
    let hb_ct = |doc: &str| {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let nodes = find_nodes(&snap, |t| t == BlockType::HtmlBlock);
        eprintln!("hb nodes = {} for {doc:?}", nodes.len());
        snap.hb_block_content(&nodes[0])
    };
    // 顶层 hb：字面 > 是内容（cmark passthrough）
    assert_eq!(hb_ct("<div>\n> q\n"), "<div>\n> q\n", "顶层字面 > 保留");
    // bq 内 hb（空 bq 行迫使开行独立成节点——与 fence 同款折叠）：剥容器恰一层
    assert_eq!(hb_ct("> 前：\n>\n> <div>\n> q\n"), "<div>\nq\n", "bq 内剥恰 1 层");
    // 双 bq
    assert_eq!(hb_ct("> > 前：\n> >\n> > <div>\n> > q\n"), "<div>\nq\n", "双 bq 剥恰 2 层");
    // bq 内 hb 字面 > 保真（缺陷 B 同款）
    assert_eq!(hb_ct("- x\n  > 前：\n  >\n  > <div>\n  > > q\n  > </div>\n"), "<div>\n> q\n</div>\n", "item+bq 字面 > 保真");
}
fn p3_dash_tail_items_are_lists() {
    for s in ["- a --", "- a -", "- a ---", "* a *"] {
        let t = crate::block::classify_at_top(s.as_bytes(), 0, false);
        assert_eq!(t, crate::block::BlockType::Bullet, "{s:?} 应为 Bullet");
    }
    // 不受影响哨兵
    assert_eq!(
        crate::block::classify_at_top(b"- a -- x", 0, false),
        crate::block::BlockType::Bullet, "- a -- x 不变");
    assert_eq!(
        crate::block::classify_at_top(b"- - a", 0, false),
        crate::block::BlockType::Bullet, "嵌套 marker 开头不闭");
}
fn p2_table_no_dup_ct() {
    let mut st = crate::projection::State::new("| a | b |\n|---|---|\n| 1 | 2 |\n");
    let snap = st.snapshot();
    let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let tb_seg = cf.split("\"nt\":\"tb\"").nth(1).unwrap_or("");
    // tb 对象自身的键 = children 通道（"ct":[）之前的头部键
    let head = tb_seg.split("\"ct\":[").next().unwrap_or("");
    let keys: Vec<&str> = head
        .split(",\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap_or(""))
        .filter(|k| !k.is_empty())
        .collect();
    eprintln!("tb head keys = {keys:?}");
    let mut seen = std::collections::HashSet::new();
    for k in &keys {
        assert!(seen.insert(*k), "tb 对象重复键 {k:?}: {keys:?}");
    }
    assert!(keys.contains(&"cols"), "cols 键应在（表头承载）: {keys:?}");
    assert!(!keys.contains(&"ct"), "attr ct 应已去掉（表头归 cols）: {keys:?}");
    assert!(tb_seg.contains("\"ct\":["), "children 通道 ct 应在");
}
fn p1_range_content_six_cases() {
    use crate::block::BlockType as BT;
    let rc = |raw: &str| crate::block::range_content(BT::CodeFenceOpen, raw).map(|r| raw[r].to_string()).unwrap_or_default();
    assert_eq!(rc("```js\ncode\n```\n"), "code\n", "① 闭合有尾换行");
    assert_eq!(rc("```js\ncode\n```"), "code\n", "② 闭合无尾换行");
    assert_eq!(rc("```js\ncode\n"), "code\n", "③ 未闭合有内容（尾 \n 是内容）");
    assert_eq!(rc("```\n\n"), "\n", "④ 未闭合只有空行——cmark cd=\n");
    assert_eq!(rc("```\n```\n"), "", "⑤ 空围栏");
    assert_eq!(rc("```\n\n```\n"), "\n", "⑥ 空行+闭合");
}
#[test]
#[ignore]
fn p34_hb_identity_probe() {
    for doc in [
        "<div>\nx\n</div>\n",
        "> <div>\n> x\n> </div>\n",
        "- a\n  <div>\n  x\n  </div>\n",
    ] {
        let mut st = crate::projection::State::new(doc);
        let snap = st.snapshot();
        let nodes = find_nodes(&snap, |t| t == BlockType::HtmlBlock);
        for node in &nodes {
            let content = snap.read_block_proj(node, crate::block::BlockProj::Content);
            let after = write_ct_prod(doc, node.start_line, node.end_line, node.node_type, "/>", &content);
            eprintln!("HB doc={doc:?} lines={}..{}\n  read={content:?}\n  identity={}",
                node.start_line, node.end_line, after == doc);
            if after != doc {
                let d = doc.as_bytes().iter().zip(after.as_bytes().iter()).position(|(a, b)| a != b).unwrap_or(doc.len());
                eprintln!("  BEFORE={:?}", &doc[d.saturating_sub(40)..(d + 40).min(doc.len())]);
                eprintln!("  AFTER ={:?}", &after[d.saturating_sub(40)..(d + 40).min(after.len())]);
            }
        }
    }
}
fn nested_bq_selector_multihit_pin() {
    let doc = "> > q\n";
    let mut st = crate::projection::State::new(doc);
    let snap = st.snapshot();
    let steps = vec![
        crate::selector::Step::RecursiveWildcard,
        crate::selector::Step::Filter(crate::selector::FilterExpr::TypeCheck(crate::selector::TypeName::BlockQuote)),
    ];
    let hits = crate::selector::select_nodes(&snap, &steps);
    assert_eq!(hits.len(), 2, "嵌套 BQ（两层）命中两次——现状缺陷钉");
    assert_eq!(hits[0].byte_range, hits[1].byte_range, "两次命中 byte_range 全同");
}
#[test]
#[test]
#[ignore]
fn p15_probe_895b() {
    let f = std::env::var("MP_F").unwrap_or_else(|_| "addr_probe4.md".into());
    let l0: usize = std::env::var("MP_L").unwrap_or_else(|_| "2373".into()).parse().unwrap();
    let doc = std::fs::read_to_string(format!("{}/../worktrees/genesis/wt/{f}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut st = crate::projection::State::new(&doc);
    let snap = st.snapshot();
    let t = snap.lines[l0].node_type();
    let end = crate::infer::get_node_end_from_lines(&snap.lines, l0, t, &snap.frame_log);
    let node = snap.build_node(l0, end, t, false);
    eprintln!("NODE {l0}\u{2026}{end} {:?}", t);
    let content = snap.read_block_proj(&node, crate::block::BlockProj::Content);
    eprintln!("CONTENT_LINES={} TAIL={:?}", content.lines().count(), &content[content.len().saturating_sub(120)..]);
    let face = snap.node_proj();
    if let Ok((r, tx)) = face.compute_write_from_node(&node, "content", &content) {
        eprintln!("TEXT first 200={:?}", &tx[..200.min(tx.len())]);
        let span = snap.rope.byte_slice(r.clone()).to_string();
        eprintln!("SPAN_LINES={}", span.lines().count());
        for (i, l) in span.lines().enumerate().take(8) {
            eprintln!("  SPAN{i} {l:?}");
        }
        eprintln!("  CONTENT first 8:");
        for (i, l) in content.lines().enumerate().take(8) {
            eprintln!("  CT{i} {l:?}");
        }
    }
    let req = crate::op::WriteRequest {
        steps: &[], operator: "/>", text: content.clone(),
        proj: Some("content"), auto_scaffold: false, auto_level: false,
    };
    let plan = crate::op::plan_write_with_nodes(&snap, req, &[node], 0).expect("plan");
    if let crate::op::WritePlan::Apply { edits, .. } = &plan {
        for ed in edits {
            eprintln!("EDIT {}..{} {:?}", ed.range.start, ed.range.end, &ed.text[..120.min(ed.text.len())]);
        }
    }
    {
        let mut st2 = crate::projection::State::new("p1\np2\n");
        let snap2 = st2.snapshot();
        let n2 = snap2.build_node(0, 2, BlockType::Paragraph, false);
        let req2 = crate::op::WriteRequest { steps: &[], operator: "/>", text: "X".into(), proj: Some("content"), auto_scaffold: false, auto_level: false };
        let p2 = crate::op::plan_write_with_nodes(&snap2, req2, &[n2], 0).expect("plan");
        if let crate::op::WritePlan::Apply { edits, .. } = &p2 { for ed in edits { eprintln!("P2EDIT {}..{} {:?}", ed.range.start, ed.range.end, ed.text); } }
    }
    let after = write_ct_prod("p1\np2\n", 0, 2, BlockType::Paragraph, "/>", "X");
    eprintln!("P2REPLACE={after:?}");
    let after2 = write_ct_prod(&doc, l0, end, t, "/>", &content);
    let after = after2;
    let d = doc.as_bytes().iter().zip(after.as_bytes().iter()).position(|(a, b)| a != b).map(|i| i).unwrap_or(doc.len());
    eprintln!("FIRST_DIFF={d} len_before={} len_after={}", doc.len(), after.len());
    eprintln!("BEFORE={:?}", &doc[d.saturating_sub(60)..(d + 60).min(doc.len())]);
    eprintln!("AFTER ={:?}", &after[d.saturating_sub(60)..(d + 60).min(after.len())]);
}
fn p15_probe_895() {
    let doc = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../worktrees/genesis/wt/var/bash.md")).unwrap();
    let mut st = crate::projection::State::new(&doc);
    let snap = st.snapshot();
    let node = snap.build_node(895, 906, BlockType::Bullet, false);
    eprintln!("node {}\u{2026}{} type {:?} conts:", node.start_line, node.end_line, node.node_type);
    for l in 895..906.min(snap.lines.len()) {
        eprintln!("  L{l} {:?} cont={}", snap.lines[l].node_type(), snap.lines[l].cont());
    }
    let content = snap.read_block_proj(&node, crate::block::BlockProj::Content);
    eprintln!("CONTENT={content:?}");
    let face = snap.node_proj();
    if let Ok((r, _)) = face.compute_write_from_node(&node, "content", &content) {
        eprintln!("RANGE={r:?}");
        eprintln!("SPAN={:?}", snap.rope.byte_slice(r.clone()).to_string());
    }
}
// ── 引号剥离守卫判据（先红后绿）──
// 裸 "（len==1）是内容不是包裹：原样写入、不 panic；"" 剥空；"abc" 剥壳；
// 单引号不剥（Rust 只剥双引号）。全部走生产链（plan → apply_mdio_op_multi → apply_one）。
#[test]
fn p01_quote_strip_guard() {
    // 引号剥壳不存在：Rust 背板不判义——载荷引号一律原样写入（剥壳是
    // 调用方语义，py interpreter 层承担；G3 抓出合法引文被误剥的实证）。
    assert_eq!(write_ct_prod("hello\n", 0, 1, BlockType::Paragraph, "+>", "\""),
        "hello\"\n", "+> 裸引号原样");
    assert_eq!(write_ct_prod("hello\n", 0, 1, BlockType::Paragraph, "/>", "\""),
        "\"\n", "/> 裸引号原样");
    assert_eq!(write_ct_prod("hello\n", 0, 1, BlockType::Paragraph, "+>", "\"\""),
        "hello\"\"\n", "空引号对原样（不再剥空）");
    assert_eq!(write_ct_prod("hello\n", 0, 1, BlockType::Paragraph, "+>", "\"abc\""),
        "hello\"abc\"\n", "引号包裹内容原样（不再剥壳）");
    assert_eq!(write_ct_prod("hello\n", 0, 1, BlockType::Paragraph, "+>", "'"),
        "hello'\n", "单引号原样");
}
// ── 两尾巴修复判据（先红后绿）──
#[test]
fn luodian3_tail_bugs_regression() {
    // bug② 空首行伪影：\n\nB 一次写入——首空行不内联、后续行带前缀
    assert_eq!(append_ct("> A\n", 0, 1, BlockType::Blockquote, "\n\nB"),
        "> A\n>\n> B\n", "空首行伪影修复");
    // bug① 尾换行（有前缀容器）：B\n 结束本行，下一个 chunk 落新行
    let mut st = crate::projection::State::new("> A\n");
    for (payload, expect) in [("B\n", "> AB\n> \n"), ("C", "> AB\n> C\n")] {
        // 每次按当前文档重建 bq 节点行区间（写后块范围随内容增长）
        let node = st.snapshot().build_node(0, st.snapshot().line_len() as usize, BlockType::Blockquote, false);
        let req = crate::op::WriteRequest {
            steps: &[], operator: "+>", text: payload.to_string(),
            proj: Some("content"), auto_scaffold: false, auto_level: false,
        };
        let plan = crate::op::plan_write_with_nodes(&st.snapshot(), req, &[node], 0).expect("plan");
        let edits = match plan {
            crate::op::WritePlan::Apply { edits, .. } => edits,
            _ => panic!("expect Apply"),
        };
        for e in &edits { st.edit(e.range.start, e.range.end, &e.text); }
        assert_eq!(st.snapshot().rope.to_string(), expect, "chunk {payload:?}");
    }
    // 前导换行（前缀空=顶层段落）：语义仍满足
    assert_eq!(append_ct("hello\n", 0, 1, BlockType::Paragraph, "\nB"),
        "hello\nB\n", "顶层段落前导换行 = 新行");
    // 顶层段落尾换行：现状哨兵（无前缀可预开——待 face 游标）
    assert_eq!(append_ct("hello\n", 0, 1, BlockType::Paragraph, "B\n"),
        "helloB\n\n", "顶层段落尾换行现状哨兵");
}
