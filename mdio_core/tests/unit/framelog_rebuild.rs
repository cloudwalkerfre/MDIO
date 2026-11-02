use super::*;
use crate::block::entry_line;

#[test]
fn test_rebuild_all_populates_frame_log() {
    let engine = MdioEngine::new("- item\n  continued\n\noutside");
    let log = &engine.state.frame_log;
    assert!(!log.entries.is_empty(), "FrameLog should have entries for a list document");
    assert!(log.entries.iter().any(|&e| entry_line(e) == 0));
}

#[test]
fn test_frame_log_entries_for_list() {
    let engine = MdioEngine::new("- item1\n  cont\n- item2\n\npost");
    let log = &engine.state.frame_log;
    assert!(log.entries.len() >= 2, "Expected at least Push+Pop for list, got {}", log.entries.len());
}

#[test]
fn test_frame_log_code_fence() {
    let engine = MdioEngine::new("```rust\ncode\nmore\n```\noutside");
    let log = &engine.state.frame_log;
    assert!(log.entries.len() >= 2, "Code fence: Push+Pop expected, got {}", log.entries.len());
}

#[test]
fn test_frame_log_ripple_after_edit() {
    let mut engine = MdioEngine::new("- item1\n- item2");
    let log_before = engine.state.frame_log.entries.len();
    engine.edit(engine.byte_of_line(1), engine.byte_of_line(1), "  cont\n");
    engine.ensure_ast();
    let log_after = engine.state.frame_log.entries.len();
    assert!(log_after > 0, "FrameLog should be non-empty after ripple");
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32),
        "All FrameLog line indices must be within document bounds");
}

#[test]
fn test_snapshot_works_with_frame_log() {
    let mut engine = MdioEngine::new("# Title\n\n- item\n- item2\n\ntext");
    let snap = engine.make_snapshot();
    let nodes = snap.select_nodes(&[]);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].byte_range, 0..snap.rope.byte_len());
}
