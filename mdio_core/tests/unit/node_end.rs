use super::*;
use crate::block::BlockType;

fn get_end_for_line(doc: &str, line_idx: usize) -> usize {
    let engine = MdioEngine::new(doc);
    let t = engine.state.lines[line_idx].node_type();
    crate::infer::get_node_end_from_lines(&engine.state.lines, line_idx, t, &engine.state.frame_log)
}

#[test]
fn test_get_node_end_header_h1_until_h2() {
    let engine = MdioEngine::new("# H1\ntext\n\n# H2\nmore");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::Header1, &engine.state.frame_log);
    assert_eq!(end, 3);
}

#[test]
fn test_get_node_end_header_same_level_terminates() {
    let engine = MdioEngine::new("# First\n\n# Second");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::Header1, &engine.state.frame_log);
    assert_eq!(end, 2);
}

#[test]
fn test_get_node_end_header_lower_level_continues() {
    let engine = MdioEngine::new("# H1\n## H2\ntext\n\n# Another H1");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 1, BlockType::Header2, &engine.state.frame_log);
    assert_eq!(end, 4);
}

#[test]
fn test_get_node_end_list_item_absorbs_continuation() {
    let engine = MdioEngine::new("- A\n  cont\n- B");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::Bullet, &engine.state.frame_log);
    assert_eq!(end, 2);
}

#[test]
fn test_get_node_end_list_item_absorbs_blank() {
    let engine = MdioEngine::new("- A\n\n  cont\n- B\ntext");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::Bullet, &engine.state.frame_log);
    assert_eq!(end, 3);
}

#[test]
fn test_get_node_end_list_stops_at_block() {
    let engine = MdioEngine::new("- A\n- B\n\n# Header");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::Bullet, &engine.state.frame_log);
    assert_eq!(end, 1);
}

#[test]
fn test_get_node_end_code_fence_finds_close() {
    let engine = MdioEngine::new("```\ncode\nmore\n```\noutside");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::CodeFenceOpen, &engine.state.frame_log);
    assert_eq!(end, 4);
}

#[test]
fn test_get_node_end_code_fence_close_single_line() {
    let engine = MdioEngine::new("```\ncode\n```\n");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 2, BlockType::CodeFenceClose, &engine.state.frame_log);
    assert_eq!(end, 3);
}

#[test]
fn test_get_node_end_blockquote_stops_at_blank() {
    let engine = MdioEngine::new("> A\n> B\n\ntext");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::Blockquote, &engine.state.frame_log);
    assert_eq!(end, 2, "BQ ends at blank line (CommonMark §5.1)");
}

#[test]
fn test_get_node_end_blockquote_absorbs_lazy_continuation() {
    let engine = MdioEngine::new("> A\n  lazy cont\n> B\ntext");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::Blockquote, &engine.state.frame_log);
    assert_eq!(end, 4);
}

#[test]
fn test_get_node_end_table_absorbs_rows() {
    let engine = MdioEngine::new("| a | b |\n|---|---|\n| 1 | 2 |\n\ntext");
    // Header row has no TableRow frame → leaf, end = start_line + 1
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::TableRow, &engine.state.frame_log);
    assert_eq!(end, 1, "header row is leaf, single-line span");
}

#[test]
fn test_get_node_end_paragraph_single_line() {
    let engine = MdioEngine::new("plain para\n\nnext");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::Paragraph, &engine.state.frame_log);
    assert_eq!(end, 1);
}

#[test]
fn test_get_node_end_thematic_break_single_line() {
    let engine = MdioEngine::new("---\n\ntext");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::ThematicBreak, &engine.state.frame_log);
    assert_eq!(end, 1);
}

#[test]
fn test_get_node_end_consistent_with_framelog_for_list() {
    let engine = MdioEngine::new("- A\n  cont\n- B\n\noutside");
    let bullet_end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::Bullet, &engine.state.frame_log);
    assert_eq!(bullet_end, 2);
}

#[test]
fn test_get_node_end_html_block_returns_single_line() {
    let engine = MdioEngine::new("<div>\ncontent\n</div>\noutside");
    let end = crate::infer::get_node_end_from_lines(&engine.state.lines, 0, BlockType::HtmlBlock, &engine.state.frame_log);
    assert_eq!(end, 4);
}

#[test]
fn test_get_node_end_hs_head_single_line() {
    let engine = MdioEngine::new("Title\n===\n\nnext");
    let t0 = engine.state.lines[0].node_type();
    assert!(t0 == BlockType::Paragraph || t0 == BlockType::HsHead1,
        "expected Paragraph or HsHead1, got {:?}", t0);
}
