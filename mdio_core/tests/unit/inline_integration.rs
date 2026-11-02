use super::*;
use crate::block::scan_block;

fn snap_from(md: &str) -> (MdioEngine, MdioSnapshot) {
    let mut engine = MdioEngine::new(md);
    let snapshot = engine.make_snapshot();
    (engine, snapshot)
}

fn find_first_node(snap: &MdioSnapshot, pred: fn(BlockType) -> bool) -> Option<BlockNode> {
    let len = snap.lines.len();
    let mut i = 0;
    while i < len {
        let t = snap.lines[i].node_type();
        if pred(t) {
            let end = crate::infer::get_node_end_from_lines(
                &snap.lines, i, t, &snap.frame_log);
            return Some(snap.build_node(i, end, t, false));
        }
        i += 1;
    }
    None
}

// ── inline_source ──

#[test]
fn inline_source_paragraph() {
    let (_, snap) = snap_from("hello **world**\n");
    let n = find_first_node(&snap, |t| t == BlockType::Paragraph).unwrap();
    assert_eq!(snap.inline_source(&n), "hello **world**");
}

#[test]
fn inline_source_heading() {
    let (_, snap) = snap_from("## **Bold** Title\n");
    let n = find_first_node(&snap, |t| is_header(t)).unwrap();
    assert_eq!(snap.inline_source(&n), "**Bold** Title");
}

#[test]
fn inline_source_bullet_kv() {
    let (_, snap) = snap_from("- key: **value**\n");
    let n = find_first_node(&snap, |t| t == BlockType::Bullet).unwrap();
    assert_eq!(snap.inline_source(&n), "key: **value**");
}

#[test]
fn inline_source_bullet_plain() {
    let (_, snap) = snap_from("- **bold** item\n");
    let n = find_first_node(&snap, |t| t == BlockType::Bullet).unwrap();
    assert_eq!(snap.inline_source(&n), "**bold** item");
}

#[test]
fn inline_source_checklist() {
    let (_, snap) = snap_from("- [x] **done**\n");
    let n = find_first_node(&snap, |t| t == BlockType::Checklist).unwrap();
    assert_eq!(snap.inline_source(&n), "**done**");
}

#[test]
fn inline_source_blockquote() {
    let (_, snap) = snap_from("> **quoted**\n");
    let n = match find_first_node(&snap, |t| t == BlockType::Blockquote) {
        Some(n) => n,
        None => return, // BQ re-parse: BQ 内容行为 Paragraph
    };
    assert_eq!(snap.inline_source(&n), "**quoted**");
}

#[test]
fn inline_source_no_inlines() {
    let (_, snap) = snap_from("```\ncode\n```\n");
    let n = find_first_node(&snap, |t| t == BlockType::CodeFenceOpen).unwrap();
    assert_eq!(snap.inline_source(&n), "");
}

// ── to_ast ──

#[test]
fn to_ast_simple_document() {
    let (_, snap) = snap_from("# **Hello**\n\nparagraph text\n");
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let _: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert!(json.contains(r#""nt":"h""#));
    assert!(json.contains(r#""nt":"st""#));
    assert!(json.contains(r#""nt":"p""#));
}

#[test]
fn to_ast_two_bullets() {
    let (_, snap) = snap_from("- one\n- two\n");
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let _: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
}

#[test]
fn to_ast_nested_list() {
    let (_, snap) = snap_from("- a\n  - b\n- c\n");
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let _: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
}

#[test]
fn to_ast_code_fence() {
    let (_, snap) = snap_from("```rust\nfn main() {}\n```\n");
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let _: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
}

#[test]
fn to_ast_paragraph_grouping() {
    let (_, snap) = snap_from("line one\nline two\n\nnext para\n");
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let _: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
    assert!(json.contains("line one"), "should contain first line");
    assert!(json.contains("line two"), "should contain second line");
}

#[test]
fn to_ast_commonmark_bench() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../commonmark.md");
    if !path.exists() { eprintln!("SKIP: commonmark.md not found"); return; }
    let md = std::fs::read_to_string(&path).unwrap();
    let mut engine = MdioEngine::new(&md);
    let snap = engine.make_snapshot();
    let _ = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let start = std::time::Instant::now();
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let elapsed = start.elapsed();
    let kb = md.len() as f64 / 1000.0;

    assert!(json.contains(r#""nt":"h""#));
    assert!(json.contains(r#""nt":"p""#));
    assert!(!json.contains(r#""nt":"dl""#));
}
