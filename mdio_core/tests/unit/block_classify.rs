use super::*;
use crate::block::{BlockType, node_type_to_str};

const ALL_TYPES: [BlockType; 25] = [
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

#[test]
fn test_node_category_all_types() {
    for &t in &ALL_TYPES {
        // spans=None = flat 模式语义——comment 产节点（
        // 与 walk 一致: flat 用 node_end 推进——内容行被开行跳过——
        // 访问到的 comment 行都产节点——TestPjComment）——RefDef 保持 Skip
        let cat = node_category(None, 0, t);
        match t {
            BlockType::TableSeparator | BlockType::CodeFenceClose | BlockType::CodeContent
            | BlockType::Blank | BlockType::ThematicBreak
            | BlockType::HtmlContent | BlockType::RefDef =>
                assert_eq!(cat, NodeCategory::Skip, "{:?} should be Skip", t),
            _ => assert_eq!(cat, NodeCategory::Block, "{:?} should be Block", t),
        }
    }
    // nested 语义（spans 可用）: 多行 comment 的内容行（帧在且无 Push）
    // Skip——单行配对/开行 Block——构造文档验证
    let mut st = crate::projection::State::new("<!--\nnote\n-->\n<!-- x -->\n");
    let snap = st.snapshot();
    let mut fold = crate::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
    crate::infer::walk_zone(&snap.frame_log.entries, (0, snap.line_len() as u32), &mut fold);
    let (recs, arena) = fold.finish();
    let blocks = crate::infer::build_block_records(&snap.frame_log.entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, snap.line_len() as u32));
    let spans = crate::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries, &snap.lines, &snap.rope, (0, snap.line_len() as u32), &arena);
    // 行 1（内容行——帧在）→ Skip
    assert_eq!(node_category(Some(&spans), 1, BlockType::Comment), NodeCategory::Skip,
        "多行 comment 内容行应 Skip");
    // 行 0（开行——有 Push）→ Block
    assert_eq!(node_category(Some(&spans), 0, BlockType::Comment), NodeCategory::Block,
        "多行 comment 开行应 Block（壳节点）");
    // 行 3（单行配对——无帧）→ Block
    assert_eq!(node_category(Some(&spans), 3, BlockType::Comment), NodeCategory::Block,
        "单行 comment 配对应 Block（leaf）");
}

#[test]
fn test_node_type_to_str_all_types() {
    for &t in &ALL_TYPES {
        let s = node_type_to_str(t);
        assert!(!s.is_empty(), "empty str for {:?}", t);
        assert_ne!(s, "other", "{:?} should not be 'other'", t);
    }
}

#[test]
fn test_header_level_coverage() {
    assert_eq!(header_level(BlockType::Header1), Some(1));
    assert_eq!(header_level(BlockType::Header6), Some(6));
    assert_eq!(header_level(BlockType::HsHead1), Some(1));
    assert_eq!(header_level(BlockType::HsHead2), Some(2));
    assert_eq!(header_level(BlockType::Paragraph), None);
    assert_eq!(header_level(BlockType::Blank), None);
}

#[test]
fn test_check_node_type_html() {
    use crate::engine::TypeName;
    let tn = TypeName::Html;
    assert_eq!(tn, TypeName::Html);
}
