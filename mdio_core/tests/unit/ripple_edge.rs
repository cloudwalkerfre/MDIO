use super::*;
use crate::block::{entry_is_pop, entry_frame, entry_line, FrameKind};

/// Helper: build engine with text, get line types as a Vec for inspection.
fn line_types(engine: &MdioEngine) -> Vec<BlockType> {
    engine.state.lines.iter().map(|p| p.node_type()).collect()
}

/// Helper: count FrameLog entries of each kind for inspection.
fn count_entries(engine: &MdioEngine) -> (usize, usize) {
    let pushes = engine.state.frame_log.entries.iter()
        .filter(|&&e| !entry_is_pop(e)).count();
    let pops = engine.state.frame_log.entries.iter()
        .filter(|&&e| entry_is_pop(e)).count();
    (pushes, pops)
}

// ── Fence open/close edits ──

#[test]
fn test_edit_insert_fence_open() {
    let mut engine = MdioEngine::new("text\nmore");
    let pos = engine.byte_of_line(0);
    engine.edit(pos, pos, "```\n");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::CodeFenceOpen, "line 0 should be fence open");
    assert_eq!(types[1], BlockType::CodeContent, "line 1 should be code content");
    assert_eq!(types[2], BlockType::CodeContent, "line 2 should be code content");
}

#[test]
fn test_edit_insert_fence_close() {
    let mut engine = MdioEngine::new("```\ncode\nmore");
    let pos = engine.byte_of_line(2);
    engine.edit(pos, pos, "```\n");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::CodeFenceOpen);
    assert_eq!(types[1], BlockType::CodeContent);
    assert_eq!(types[2], BlockType::CodeFenceClose, "inserted fence marker closes the open fence");
    let entries_at_2: Vec<_> = engine.state.frame_log.entries.iter()
        .filter(|&&e| entry_line(e) == 2).collect();
    assert_eq!(entries_at_2.len(), 1, "single Pop entry (no re-open)");
}

#[test]
fn test_edit_delete_fence_open() {
    let mut engine = MdioEngine::new("```\ncode\nmore\n```\nafter");
    let end = engine.byte_of_line(1);
    engine.edit(0, end, "");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::Paragraph, "was code, now paragraph");
    assert_eq!(types[1], BlockType::Paragraph, "was code, now paragraph");
}

#[test]
fn test_edit_delete_fence_close() {
    let mut engine = MdioEngine::new("```\ncode\n```\nafter");
    let start = engine.byte_of_line(2);
    let end = engine.byte_of_line(3);
    engine.edit(start, end, "");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[1], BlockType::CodeContent, "still inside unclosed fence");
}

#[test]
fn test_edit_splice_change_to_fence() {
    let mut engine = MdioEngine::new("text\nmore\nend");
    let start = engine.byte_of_line(1);
    let end = engine.byte_of_line(2);
    engine.edit(start, end, "```\n");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types.len(), 3, "should still be 3 lines");
    assert_eq!(types[0], BlockType::Paragraph, "text still paragraph");
    assert_eq!(types[1], BlockType::CodeFenceOpen, "changed to fence open");
}

// ── Ripple correctness: multi-edit convergence ──

#[test]
fn test_double_edit_before_ripple() {
    let mut engine = MdioEngine::new("a\nb\nc\nd");
    let p1 = engine.byte_of_line(1);
    engine.edit(p1, p1, "- ");
    let p3 = engine.byte_of_line(3);
    engine.edit(p3, p3, "- ");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[1], BlockType::Bullet, "line 1 became bullet");
    assert_eq!(types[3], BlockType::Bullet, "line 3 became bullet");
}

#[test]
fn test_ripple_converges_before_end() {
    let mut engine = MdioEngine::new("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
    let pos = engine.byte_of_line(2);
    engine.edit(pos, pos, "# ");
    engine.ensure_ast();
    assert_eq!(line_types(&engine)[2], BlockType::Header1);
    assert_eq!(line_types(&engine)[9], BlockType::Paragraph);
}

// ── FrameLog consistency after edits ──

#[test]
fn test_framelog_push_pop_balanced_after_edit() {
    let mut engine = MdioEngine::new("```\ncode\n```\nafter");
    let (pushes, pops) = count_entries(&engine);
    assert!(pushes >= 1 && pops >= 1, "Push/Pop should exist for a closed fence: pushes={}, pops={}", pushes, pops);
    let pos = engine.byte_of_line(1);
    engine.edit(pos, pos, "extra\n");
    engine.ensure_ast();
    let (pushes2, pops2) = count_entries(&engine);
    assert!(pushes2 >= 1 && pops2 >= 1, "Push/Pop should still exist after edit: pushes={}, pops={}", pushes2, pops2);
    for &e in &engine.state.frame_log.entries {
        assert!(entry_line(e) < engine.line_len() as u32,
            "entry line {} out of bounds (total lines={})", entry_line(e), engine.line_len());
    }
}

#[test]
fn test_framelog_no_stale_entries_after_fence_edit() {
    let mut engine = MdioEngine::new("```\ncode\n```\nafter");
    let orig_pushes: Vec<u32> = engine.state.frame_log.entries.iter()
        .filter(|&&e| !entry_is_pop(e))
        .map(|&e| entry_line(e))
        .collect();
    let end = engine.byte_of_line(1);
    engine.edit(0, end, "~~~\n");
    engine.ensure_ast();
    for &e in &engine.state.frame_log.entries {
        assert!(entry_line(e) < engine.line_len() as u32,
            "entry line {} out of bounds", entry_line(e));
    }
}

// ── Container boundary edits ──

#[test]
fn test_edit_at_blockquote_boundary() {
    // BQ re-parse: BQ 内容行为 Paragraph, BQ 容器由 FL Push/Pop 管理.
    let mut engine = MdioEngine::new("text\nmore\noutside");
    let pos = engine.byte_of_line(0);
    engine.edit(pos, pos, "> ");
    engine.ensure_ast();
    let snap = engine.make_snapshot();
    assert!(snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bq"), "BQ not found after edit");
}

#[test]
fn test_edit_remove_list_marker() {
    let mut engine = MdioEngine::new("- item\n  cont\noutside");
    engine.edit(0, 2, "");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::Paragraph, "was bullet, now paragraph");
}

// ── List nesting edits ──

#[test]
fn test_edit_indent_changes_nesting() {
    let mut engine = MdioEngine::new("- a\n  - b\n- c");
    let pos = engine.byte_of_line(2);
    engine.edit(pos, pos, "  ");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::Bullet);
    assert_eq!(types[1], BlockType::Bullet);
    assert_eq!(types[2], BlockType::Bullet);
}

// ── Hs (setext) heading edit ──

#[test]
fn test_edit_insert_setext_underline() {
    // M3: RewritePrev removed. HsHead1 is walk_blocks forward-look, not u8.
    let mut engine = MdioEngine::new("Title\nnext");
    let pos = engine.byte_of_line(1);
    engine.edit(pos, pos, "===\n");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::Paragraph, "stays Paragraph at u8 level");
    assert_eq!(types[1], BlockType::HsSep, "underline is HsSep");
    // walk_blocks via to_ast should render as heading
    let ast = engine.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(ast.contains("\"nt\":\"h\""), "forward-look should produce header node: {}", ast);
}

// ── Table edits ──

#[test]
fn test_edit_break_table() {
    let mut engine = MdioEngine::new("| a | b |\n|---|---|\n| 1 | 2 |");
    let start = engine.byte_of_line(1);
    let end = engine.byte_of_line(2);
    engine.edit(start, end, "plain\n");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::TableRow, "first row still table");
    assert_eq!(types[1], BlockType::Paragraph, "was separator, now paragraph");
    assert_eq!(types[2], BlockType::TableRow, "third row starts new table");
}

// ── HTML block edits ──

#[test]
fn test_edit_add_html_block() {
    let mut engine = MdioEngine::new("text\nmore");
    let pos = engine.byte_of_line(0);
    engine.edit(pos, pos, "<div>\n");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::HtmlBlock);
    assert_eq!(types[1], BlockType::HtmlContent);
}

// ── Smoke: commonmark full ripple ──

#[test]
fn test_commonmark_multi_edit_smoke() {
    let path = {
        let mut p = None;
        for candidate in &["../commonmark.md", "../../commonmark.md",
                           concat!(env!("CARGO_MANIFEST_DIR"), "/../commonmark.md")] {
            if std::path::Path::new(candidate).exists() { p = Some(candidate.to_string()); break; }
        }
        p.unwrap_or_else(|| panic!("commonmark.md not found"))
    };
    let text = std::fs::read_to_string(&path).unwrap();
    let mut engine = MdioEngine::new(&text);
    let total = engine.line_len();
    let p1 = engine.byte_of_line(total / 4);
    engine.edit(p1, p1, "- inserted\n");
    engine.ensure_ast();
    assert_eq!(engine.line_len(), total + 1, "one line added");
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32),
        "all entries within bounds after edit 1");
    let p2_start = engine.byte_of_line(total / 2);
    let p2_end = engine.byte_of_line(total / 2 + 1);
    engine.edit(p2_start, p2_end, "");
    engine.ensure_ast();
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32),
        "all entries within bounds after edit 2");
    let p3_start = engine.byte_of_line(total * 3 / 4);
    let p3_end = engine.byte_of_line(total * 3 / 4 + 1);
    engine.edit(p3_start, p3_end, "```\n");
    engine.ensure_ast();
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32),
        "all entries within bounds after edit 3");
    let snap = engine.make_snapshot();
    let nodes = snap.select_nodes(&[]);
    assert_eq!(nodes.len(), 1);
}

// ── Nested container edits ──

#[test]
fn test_nested_bq_blt_break_all() {
    let engine = MdioEngine::new("> - item\n\noutside");
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32));
    assert!(engine.state.frame_log.entries.iter().any(|&e| entry_is_pop(e)), "Pops exist");
}

#[test]
fn test_edit_break_inner_container_only() {
    let mut engine = MdioEngine::new("> - item\n> plain");
    engine.edit(2, 4, "");
    engine.ensure_ast();
    let snap = engine.make_snapshot();
    assert!(snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bq"), "BQ should exist");
    assert!(!snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bl"), "no more bullet");
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32));
}

#[test]
fn test_edit_add_indent_inside_list() {
    let mut engine = MdioEngine::new("- a\n- b");
    let p = engine.byte_of_line(1);
    engine.edit(p, p, "  ");
    engine.ensure_ast();
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32));
}

// ── Lazy continuation boundaries ──

#[test]
fn test_bq_lazy_para_open_transition() {
    // BQ re-parse (§AX.3): BQ 内容行类型为 Paragraph.
    let mut engine = MdioEngine::new("> quote\nlazy");
    assert!(engine.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bq"), "BQ not found");
    let engine2 = MdioEngine::new("> quote\n\nlazy");
    let types2 = line_types(&engine2);
    assert!(types2.iter().any(|&t| t == BlockType::Blank), "blank line present");
}

#[test]
fn test_indent_4_stops_bq_lazy() {
    let engine = MdioEngine::new("> quote\n> \n    code");
    let types = line_types(&engine);
    assert!(!types.is_empty());
}

// ── BlockType transition edits ──

#[test]
fn test_edit_paragraph_to_header() {
    let mut engine = MdioEngine::new("text\nmore");
    engine.edit(0, 0, "# ");
    engine.ensure_ast();
    assert_eq!(line_types(&engine)[0], BlockType::Header1);
    assert_eq!(line_types(&engine)[1], BlockType::Paragraph);
}

#[test]
fn test_edit_header_to_paragraph() {
    let mut engine = MdioEngine::new("# title\ntext");
    engine.edit(0, 2, "");
    engine.ensure_ast();
    assert_eq!(line_types(&engine)[0], BlockType::Paragraph);
}

#[test]
fn test_thematic_break_vs_setext_transition() {
    // Standalone --- → ThematicBreak
    let engine = MdioEngine::new("---");
    assert_eq!(line_types(&engine)[0], BlockType::ThematicBreak);
    // M3: RewritePrev removed. --- after Paragraph → HsSep at u8,
    // the preceding line stays Paragraph; forward-look handles HsHead2.
    let mut engine = MdioEngine::new("Title\n---");
    assert_eq!(line_types(&engine)[0], BlockType::Paragraph);
    assert_eq!(line_types(&engine)[1], BlockType::HsSep);
    // walk_blocks via to_ast should render as heading
    {
        let snap = engine.make_snapshot();
        let ast = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
        assert!(ast.contains("\"nt\":\"h\""), "forward-look should produce header node: {}", ast);
    }
}

#[test]
fn test_edit_checklist_to_bullet() {
    let mut engine = MdioEngine::new("- [x] done\n- [ ] todo");
    assert_eq!(line_types(&engine)[0], BlockType::Checklist);
    engine.edit(2, 6, "");
    engine.ensure_ast();
    assert_eq!(line_types(&engine)[0], BlockType::Bullet);
}

// ── Blank line effects ──

#[test]
fn test_blank_toggles_list_loose() {
    let engine = MdioEngine::new("- a\n\n- b");
    let stack = engine.state.frame_log.rebuild_stack(3);
    let blt = stack.last().unwrap();
    assert!(blt.loose(), "blank between items -> loose");
    let engine = MdioEngine::new("- a\n- b");
    let stack = engine.state.frame_log.rebuild_stack(2);
    let blt = stack.last().unwrap();
    assert!(!blt.loose(), "no blank -> tight");
}

#[test]
fn test_blank_inside_code_fence_is_code_content() {
    let engine = MdioEngine::new("```\ncode\n\nmore\n```\nafter");
    let types = line_types(&engine);
    assert_eq!(types[2], BlockType::CodeContent, "blank inside fence is CodeContent");
}

#[test]
fn test_blank_ends_bq() {
    // BQ re-parse (§AX.3): BQ 内容行类型为 Paragraph (非 Blockquote).
    // 验证 BQ 容器存在即可 (walker 通过 FL Push/Pop 打开容器).
    let mut engine = MdioEngine::new("> para\n\noutside");
    let snap = engine.make_snapshot();
    assert!(snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bq"), "BQ not found");
}

// ── Indent boundary ──

#[test]
fn test_indent_3_to_4_crosses_indented_code() {
    let engine = MdioEngine::new("    code");
    assert_eq!(line_types(&engine)[0], BlockType::IndentedCode);
    let engine = MdioEngine::new("   text");
    assert_eq!(line_types(&engine)[0], BlockType::Paragraph, "indent 3 is paragraph");
}

#[test]
fn test_tab_indent_boundary() {
    let engine = MdioEngine::new("\tcode");
    assert_eq!(line_types(&engine)[0], BlockType::IndentedCode, "tab = indent 4");
    let engine = MdioEngine::new(" \tcode");
    assert_eq!(line_types(&engine)[0], BlockType::IndentedCode, "space+tab = indent 4");
}

// ── Multi-pop scenarios ──

#[test]
fn test_line_breaks_two_containers() {
    let engine = MdioEngine::new("```\ncode\n```\noutside");
    let pop_count = engine.state.frame_log.entries.iter()
        .filter(|&&e| entry_is_pop(e)).count();
    assert_eq!(pop_count, 1, "one Pop at closing fence");
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32));
}

#[test]
fn test_multi_pop_reclassify_header() {
    let engine = MdioEngine::new("```\ncode\n```\n# header");
    assert!(engine.state.frame_log.entries.iter().any(|&e| entry_is_pop(e)), "Pop exists for fence close");
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32));
}

// ── Nested cross-kind list edits ──

#[test]
fn test_cross_kind_list_nesting_ol_in_blt() {
    let engine = MdioEngine::new("- bullet\n  1. ordered");
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::Bullet);
    assert_eq!(types[1], BlockType::OrderedListItem, "OL nested inside BLT");
}

#[test]
fn test_blt_inside_ol_nesting() {
    let engine = MdioEngine::new("1. ordered\n   - bullet");
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::OrderedListItem);
    assert_eq!(types[1], BlockType::Bullet, "BLT nested inside OL");
}

// ── Document boundaries ──

#[test]
fn test_edit_at_line_zero() {
    let mut engine = MdioEngine::new("text\nmore");
    engine.edit(0, 0, "# ");
    engine.ensure_ast();
    assert_eq!(line_types(&engine)[0], BlockType::Header1);
}

#[test]
fn test_edit_at_end_to_close_fence() {
    let mut engine = MdioEngine::new("```\ncode");
    engine.edit(engine.byte_len(), engine.byte_len(), "\n```");
    engine.ensure_ast();
    let types = line_types(&engine);
    assert!(types.contains(&BlockType::CodeFenceClose) || types.contains(&BlockType::CodeFenceOpen),
        "should contain fence types");
}

#[test]
fn test_empty_document_edit() {
    let mut engine = MdioEngine::new("");
    engine.edit(0, 0, "# header");
    engine.ensure_ast();
    assert_eq!(line_types(&engine)[0], BlockType::Header1);
}

// ── PopPush scenarios ──

#[test]
fn test_list_poppush_entry_count() {
    let engine = MdioEngine::new("- a\n- b\n- c");
    let (pushes, pops) = (
        engine.state.frame_log.entries.iter().filter(|&&e| !entry_is_pop(e)).count(),
        engine.state.frame_log.entries.iter().filter(|&&e| entry_is_pop(e)).count(),
    );
    assert_eq!(pushes, 3, "3 Push entries (1 initial + 2 PopPush)");
    assert_eq!(pops, 2, "2 Pop entries (PopPush closes old items)");
}

#[test]
fn test_list_poppush_preserves_loose_flag() {
    let engine = MdioEngine::new("- a\n\n- b\n- c");
    let stack = engine.state.frame_log.rebuild_stack(4);
    let blt = stack.last().unwrap();
    assert!(blt.loose(), "loose after blank between items");
}

// ── CodeFence char mismatch ──

#[test]
fn test_tilde_fence_backtick_inside_is_content() {
    let engine = MdioEngine::new("~~~\ncode\n```\nafter");
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::CodeFenceOpen);
    assert_eq!(types[1], BlockType::CodeContent);
    assert_eq!(types[2], BlockType::CodeContent, "``` inside ~~~ is content");
}

#[test]
fn test_fence_with_info_string() {
    let engine = MdioEngine::new("```rust\ncode\n```");
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::CodeFenceOpen, "fence open with info string");
    assert_eq!(types[1], BlockType::CodeContent);
    assert!(engine.state.frame_log.entries.iter().any(|&e| !entry_is_pop(e)), "has push entry");
    assert!(engine.state.frame_log.entries.iter().all(|&e| entry_line(e) < engine.line_len() as u32));
}

// ── Post-Bugfix regression tests ──

#[test]
fn test_bq_blank_keeps_bq_open() {
    let mut engine = MdioEngine::new("> para\n\ntext");
    // BQ re-parse: BQ 内容行类型为 Paragraph, AST 验证 BQ 容器存在
    let snap = engine.make_snapshot();
    assert!(snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bq"), "BQ not found");
    assert_eq!(snap.lines[1].node_type(), BlockType::Blank);
}

#[test]
fn test_bq_thematic_break_pops_bq() {
    // BQ re-parse: "---" after "> quote" 被 setext 识别 (BQ 内段落→HsHead2).
    // BQ 正确关闭, "text" 成为独立段落.
    let mut engine = MdioEngine::new("> quote\n---\ntext");
    let snap = engine.make_snapshot();
    assert!(snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bq"), "BQ not found");
}

#[test]
fn test_same_kind_list_nesting_is_push() {
    let engine = MdioEngine::new("- outer\n  - inner\n  - inner2\n- outer2");
    let pushes: Vec<_> = engine.state.frame_log.entries.iter()
        .filter(|&&e| !entry_is_pop(e))
        .collect();
    assert!(pushes.len() >= 3, "should have pushes for nested items");
}

#[test]
fn test_boundary_for_middle_list_item() {
    let engine = MdioEngine::new("- a\n- b\n- c\ntext");
    let types = line_types(&engine);
    assert_eq!(types[0], BlockType::Bullet);
    assert_eq!(types[1], BlockType::Bullet);
    assert_eq!(types[2], BlockType::Bullet);
    let end = engine.state.frame_log.item_tail_boundary(1, FrameKind::BulletList, &engine.state.lines);
    assert_eq!(end, Some(2), "middle item should end at next PopPush");
}

#[test]
fn test_bq_continue_as_para_open_transition() {
    // `> quote\n>\nlazy`——空 `>` 行段落关闭（CM 例 22：`> bar\n>\nbaz` → baz
    // 在 BQ 外）——lazy 拒（prev 归一 None——PrevHint::Blank——
    // prev 词法补内容层）→ BQ Pop——baz 顶层段落。
    let mut engine = MdioEngine::new("> quote\n>\nlazy");
    let snap = engine.make_snapshot();
    assert!(snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf).contains("bq"), "BQ not found");
    let pops = engine.state.frame_log.entries.iter()
        .filter(|&&e| entry_is_pop(e) && entry_frame(e).kind() == FrameKind::BlockQuote)
        .count();
    assert_eq!(pops, 1, "BQ should Pop at line 2 (baz 顶层——CM 例 22)");
}

#[test]
fn test_single_pipe_not_panic() {
    let mut engine = MdioEngine::new("|\n");
    let snap = engine.make_snapshot();
    let nodes = snap.select_nodes(&[Step::RecursiveWildcard]);

}

#[test]
fn test_plus_plus_plus_is_paragraph_not_thematic_break() {
    let mut engine = MdioEngine::new("text before\n+++\ntext after\n");
    let types = line_types(&engine);

    assert_eq!(types[0], BlockType::Paragraph, "line 0 'text before' should be Paragraph");
    assert_eq!(types[1], BlockType::Paragraph, "line 1 '+++' should be Paragraph, got {:?}", types[1]);
    assert_eq!(types[2], BlockType::Paragraph, "line 2 'text after' should be Paragraph");
    let snap = engine.make_snapshot();
    let nodes = snap.select_nodes(&[Step::RecursiveWildcard]);

    for (i, n) in nodes.iter().enumerate() {
        let text = snap.read_block_proj(n, BlockProj::Raw);

    }
    assert_eq!(nodes.len(), 1, "consecutive paragraphs merge into one node");
}

// ── Post-ripple-convergence-off-by-one regression ──

/// Edit that inserts framed content (sub-heading + bullet) under one sibling heading
/// must not corrupt bullet boundaries under the next sibling heading.
/// Regression test for: `prefix_len(conv_old)` → `prefix_len(conv_old + 1)`.
#[test]
fn test_sibling_list_boundary_after_convergence_edit() {
    let doc = "# AGENTS\n\
               ## libai\n\
               - m: DEEPSEEK_CHAT\n\
               - sysp: prompt\n\
               ## dufu\n\
               - m: DEEPSEEK_CHAT\n\
               - sysp: prompt\n";
    let mut engine = MdioEngine::new(doc);
    // line 0: # AGENTS (h1)
    // line 1: ## libai (h2)
    // line 2: - m: DEEPSEEK_CHAT
    // line 3: - sysp: prompt
    // line 4: ## dufu (h2)
    // line 5: - m: DEEPSEEK_CHAT
    // line 6: - sysp: prompt

    // Verify before-edit boundaries: dufu's first bullet ends at next bullet (line 6)
    let bl_end = engine.state.frame_log.item_tail_boundary(5, FrameKind::BulletList, &engine.state.lines);
    assert_eq!(bl_end, Some(6), "before edit, dufu first bullet should end at next bullet (line 6)");

    // Insert framed content under libai: a sub-heading with a bullet.
    // This is the exact scenario from the LLM plugin auto_scaffold history write.
    let insert_pos = engine.byte_of_line(4); // before ## dufu
    engine.edit(insert_pos, insert_pos, "\n### HIS\n- user: text\n");
    engine.ensure_ast();

    // After edit (+3 lines, delta=3), dufu's first bullet shifts from line 5 → line 8.
    // Its boundary should be at the next bullet (old line 6 → new line 9).
    let bl_end = engine.state.frame_log.item_tail_boundary(8, FrameKind::BulletList, &engine.state.lines);
    assert_eq!(bl_end, Some(9),
        "dufu first bullet boundary should be at line 9, got {:?}", bl_end);

    // Verify no stale BulletList from libai leaks past ## dufu.
    // rebuild_stack(8) replays entries with line < 8 — after ## dufu's pop.
    let stack = engine.state.frame_log.rebuild_stack(8);
    let bl_count = stack.iter().filter(|f| f.kind() == FrameKind::BulletList).count();
    assert_eq!(bl_count, 0,
        "no BulletList should leak past ## dufu after edit, stack={:?}", stack);

    // Verify no duplicate frame entries at convergence boundary.
    // convergence was at old line 4 (## dufu), shifted to new line 7.
    // That line should have at most one pop(BL) — the ripple entry,
    // NOT a duplicate from the old log.
    let line_7_pops = engine.state.frame_log.entries.iter()
        .filter(|&&e| entry_line(e) == 7 && entry_is_pop(e)
                    && entry_frame(e).kind() == FrameKind::BulletList)
        .count();
    assert!(line_7_pops <= 1,
        "at most one pop(BL) at convergence line 7, got {}", line_7_pops);
}

// ── Minimal reproduction: 2 headings, 2 keys on sibling ──

#[test]
fn test_two_heading_two_key_boundary() {
    let doc = "# R\n\n## H1\n- a: v1\n\n## H2\n- b: v2\n\n- c: v3\n";
    let mut engine = MdioEngine::new(doc);

    let h2_line = (0..engine.line_len())
        .find(|&i| engine.state.lines[i].node_type() == BlockType::Header2 && i > 3)
        .unwrap();
    engine.edit(engine.byte_of_line(h2_line), engine.byte_of_line(h2_line), "\n### S\n- x: ins\n");
    engine.ensure_ast();

    let new_h2 = (0..engine.line_len())
        .find(|&i| engine.state.lines[i].node_type() == BlockType::Header2 && i > h2_line)
        .unwrap();
    let bl = new_h2 + 1;
    let end = engine.state.frame_log.item_tail_boundary(bl as u32, FrameKind::BulletList, &engine.state.lines);
    assert_eq!(end, Some((bl + 2) as u32),
        "H2 first bullet L{} end at L{}, got {:?}", bl, bl + 2, end);
}

// ── P2 锚守卫回归：BQ 内不误触发 + tail 场景触发 ──
#[test]
fn p2_anchor_guard_rules() {
    // BQ 内 `>    3. c`（4 空格）剥 `> ` 后 effective=3 ≤ 锚 3 → 同列表
    // （CM 实测一致——不是结束）。
    let mut engine = crate::MdioEngine::new("> 1. a\n>\n>  2. b\n>\n>    3. c\n");
    let types = engine.state.lines.iter().map(|p| p.node_type()).collect::<Vec<_>>();
    assert_eq!(types[4], crate::block::BlockType::OrderedListItem,
        "BQ 内 4sp 应为同列表 item: {:?}", types);

    // tail 场景：`    3. c` 后接 `4. d`——守卫应触发（line 4 = IC）。
    let mut engine2 = crate::MdioEngine::new("1. a\n\n  2. b\n\n    3. c\n4. d\n");
    let types2 = engine2.state.lines.iter().map(|p| p.node_type()).collect::<Vec<_>>();
    assert_eq!(types2[4], crate::block::BlockType::IndentedCode,
        "tail 场景守卫应触发（line 4 应为 IC）: {:?}", types2);
}

// ── P2 边界回归：跨种类 marker 不触发守卫（新列表嵌套——CM 实测）──
#[test]
fn p2_cross_kind_no_guard() {
    // `- a\n\n    1. b`——跨种类（BLT→OL）不守卫——新列表嵌套
    // （CM：`<li><p>a</p><ol><li>b</li></ol></li>`——不是顶层 IC）。
    // 注：嵌套容器类型显示 bl（item 类型推断用容器层——跨种类嵌套的
    // marker 剥取/类型属 item 内容段模型远期缺口——此处断言嵌套结构）。
    let mut engine = crate::MdioEngine::new("- a\n\n    1. b\n");
    let types = engine.state.lines.iter().map(|p| p.node_type()).collect::<Vec<_>>();
    assert_eq!(types[2], crate::block::BlockType::OrderedListItem,
        "跨种类 marker 应嵌套（不触发守卫）: {:?}", types);
    let ast = engine.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);

    let bl_idx = ast.find("\"nt\":\"bl\"");
    let od_idx = ast.find("\"nt\":\"od\"");
    assert!(bl_idx.is_some() && od_idx.is_some() && od_idx > bl_idx,
        "跨种类 4sp 应嵌套（od 在 bl 内——CM 实测）: {}", &ast[..ast.len().min(300)]);

    // 同种类守卫仍生效（N12 对照）。
    let mut engine2 = crate::MdioEngine::new("1. a\n\n  2. b\n\n    3. c\n");
    let types2 = engine2.state.lines.iter().map(|p| p.node_type()).collect::<Vec<_>>();
    assert_eq!(types2[4], crate::block::BlockType::IndentedCode,
        "同种类守卫应触发（line 4 应为 IC）: {:?}", types2);
}

// ── P2 边界：守卫反转的增量路径（3sp→4sp——IC 应回归）──
#[test]
fn p2_guard_reverse_incremental() {
    let mut engine = crate::MdioEngine::new("1. a\n\n  2. b\n\n   3. c\n");
    let types0 = engine.state.lines.iter().map(|p| p.node_type()).collect::<Vec<_>>();

    assert_eq!(types0[4], crate::block::BlockType::OrderedListItem, "3sp = item");

    // 行 4 前插一个空格（3sp → 4sp）——守卫应触发 → IC
    let pos = engine.byte_of_line(4);
    engine.edit(pos, pos, " ");
    engine.ensure_ast();
    let types = engine.state.lines.iter().map(|p| p.node_type()).collect::<Vec<_>>();

    let ast = engine.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);

    assert_eq!(types[4], crate::block::BlockType::IndentedCode,
        "4sp 守卫应触发（line 4 应为 IC）: {:?}", types);
    assert!(ast.contains("\"nt\":\"ic\""), "AST 应有 ic: {}", ast);
}

// ── P2 回归：跨种类 scan 路径（Outside→Pop 同层 / Inside→Push 嵌套——CM 实测）──
#[test]
fn p2_cross_kind_scan_path() {
    use crate::block::{BlockFrame, FrameKind, scan_block, ScanAction};
    let blt = BlockFrame::new(FrameKind::BulletList, 2, 0);
    // Outside（M < 内容列）→ Pop（顶层重扫——同层新列表）
    let (t0, a0) = scan_block(b"1. b", &[blt], None);
    assert!(matches!(a0, ScanAction::Pop), "0sp 跨种类应 Pop: {:?}", a0);
    // Inside（M ≥ 内容列）→ Push（item 内嵌套——CM 实测）
    let (t2, a2) = scan_block(b"  1. b", &[blt], None);
    assert!(matches!(a2, ScanAction::Push(_)), "2sp 跨种类应 Push（嵌套）: {:?}", a2);
    assert_eq!(t2, crate::block::BlockType::OrderedListItem);
}




// ── P2 回归：pending_loose 跨 ripple 起点恢复（同层→嵌套编辑——loose 不丢）──
#[test]
fn p2_pending_loose_ripple_restore() {
    // 全量对照：直接 4sp（嵌套——a 应 loose——p 包裹）
    let mut full = crate::MdioEngine::new("- a\n\n    1. b\n");
    let ast_full = full.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);

    // 增量：从同层编辑到嵌套——空行在 ripple 起点前——pending 需恢复
    let mut inc = crate::MdioEngine::new("- a\n\n 1. b\n");
    let pos = inc.byte_of_line(2);
    inc.edit(pos, pos, "    ");
    inc.ensure_ast();
    let ast1 = inc.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert_eq!(ast_full, ast1,
        "F/P 应一致（同层→嵌套编辑——a 的 loose p 在增量中保留）: {}", &ast1[..ast1.len().min(300)]);
}

// ── P2 回归：嵌套↔同层编辑的双向 F/P 一致（loose 添加/移除）──
#[test]
fn p2_nest_flat_bidirectional() {
    // 嵌套 → 同层（4sp → 1sp——loose 移除——a 变 tight 无 p）
    let mut full = crate::MdioEngine::new("- a\n\n 1. b\n");
    let ast_full = full.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let mut inc = crate::MdioEngine::new("- a\n\n    1. b\n");
    let pos = inc.byte_of_line(2);
    inc.edit(pos, pos + 4, "");
    inc.ensure_ast();
    let ast1 = inc.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert_eq!(ast_full, ast1, "嵌套→同层 F/P 应一致: {}", &ast1[..ast1.len().min(200)]);
}

// ── P2 回归：列表 item lazy 续行（CM §5.2——无缩进续行并入 item 段落）──
#[test]
fn p2_list_lazy_continuation() {
    use crate::block::BlockType as BT;

    // 1. 基本 lazy：`- a\nb`——b 并入 item（不 Pop 列表）
    let mut e = crate::MdioEngine::new("- a\nb\n");
    let t = e.state.lines.iter().map(|p| p.node_type()).collect::<Vec<_>>();
    assert_eq!(t[1], BT::Paragraph, "b 是 lazy 段落: {:?}", t);
    let ast = e.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(ast.matches("\"nt\":\"bl\"").count() == 1 && ast.contains("\"ct\":\"b\""),
        "lazy 行应在列表内: {}", &ast[..ast.len().min(200)]);

    // 2. lazy + 新 item：`- a\nb\n- c`——c 是外层新 item
    let mut e2 = crate::MdioEngine::new("- a\nb\n- c\n");
    let ast2 = e2.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(ast2.matches("\"nt\":\"b\"").count() >= 2,
        "c 应是新 item: {}", &ast2[..ast2.len().min(200)]);

    // 3. 空行后不 lazy：`- a\nb\n\nc`——c 列表外
    let mut e3 = crate::MdioEngine::new("- a\nb\n\nc\n");
    let ast3 = e3.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    // WC-1（字段键恒在）：段落现在多出 t:""，子串匹配要跟着形状走——语义不变（c 是列表外的根段落）。
    let p_after = ast3.rfind("\"nt\":\"p\",\"t\":\"\",\"ct\":\"c\"");
    let bl_end = ast3.rfind("\"nt\":\"bl\"");
    assert!(p_after.is_some() && bl_end.is_some() && p_after > bl_end,
        "空行后 c 应在列表外: {}", &ast3[..ast3.len().min(250)]);

    // 4. 嵌套 lazy：`- a\n  - b\nc`——c 并入嵌套 item
    let mut e4 = crate::MdioEngine::new("- a\n  - b\nc\n");
    let ast4 = e4.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(ast4.matches("\"nt\":\"bl\"").count() == 2,
        "嵌套列表保持: {}", &ast4[..ast4.len().min(250)]);

    // 5. 空 item 后不 lazy：`- a\n*\nb`——b 列表外（空 item 无段落）
    let mut e5 = crate::MdioEngine::new("- a\n*\nb\n");
    let ast5 = e5.make_snapshot().to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let b_pos = ast5.find("\"ct\":\"b\"");
    let last_bl = ast5.rfind("\"nt\":\"bl\"");
    assert!(b_pos.is_some() && last_bl.is_some() && b_pos > last_bl,
        "空 item 后 b 应在列表外: {}", &ast5[..ast5.len().min(250)]);
}
