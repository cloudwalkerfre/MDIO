/// block 与 inline JSON 的回归测试台。
/// Run: cargo test --test bug_testbed -- --nocapture
use mdio_core::projection::State;
use mdio_core::projection::Snapshot;
use mdio_core::topology::ProjectionMode;

// inline children 为空

#[test]
fn p1_bullet_inline() {
    let mut s = State::new("- **bold** bullet\n");
    let snap = s.snapshot();
    eprintln!("=== P1: bullet inline ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p1_bq_inline() {
    let mut s = State::new("> **bold** in BQ\n");
    let snap = s.snapshot();
    eprintln!("=== P1: BQ inline ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p1_checklist_inline() {
    let mut s = State::new("- [x] **done** task\n");
    let snap = s.snapshot();
    eprintln!("=== P1: checklist inline ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// BQ auto-paragraph 双重渲染

#[test]
fn p2_bq_heading_double() {
    let mut s = State::new("> ## Heading in BQ\n");
    let snap = s.snapshot();
    eprintln!("=== P2: BQ > H2 ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p2_bq_list_double() {
    let mut s = State::new("> - list in BQ\n");
    let snap = s.snapshot();
    eprintln!("=== P2: BQ > BLT ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p2_bq_fence_double() {
    let mut s = State::new("> ```\n> code\n> ```\n");
    let snap = s.snapshot();
    eprintln!("=== P2: BQ > CF ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p2_bq_multichild() {
    let mut s = State::new("> ## H2\n> plain paragraph\n");
    let snap = s.snapshot();
    eprintln!("=== P2: BQ > H2 + P ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// PopPush -> Push 退化

#[test]
fn p3_same_marker_same_indent() {
    let mut s = State::new("- a\n- b\n- c\n");
    let snap = s.snapshot();
    eprintln!("=== P3: same marker same indent ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p3_bq_list_poppush() {
    let mut s = State::new("> - a\n> - b\n");
    let snap = s.snapshot();
    eprintln!("=== P3: BQ > BLT PopPush ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p3_checklist_poppush() {
    let mut s = State::new("- [x] a\n- [ ] b\n- [x] c\n");
    let snap = s.snapshot();
    eprintln!("=== P3: checklist PopPush ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p3_star_dash_crosskind() {
    let mut s = State::new("* star\n  - dash\n");
    let snap = s.snapshot();
    eprintln!("=== P3: *→- crosskind ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// mark/content 泄漏

#[test]
fn p4_bq_prefix_in_ol() {
    let mut s = State::new("> 1. ordered item\n");
    let snap = s.snapshot();
    eprintln!("=== P4: BQ > prefix in OL ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p4_nested_bq_prefix() {
    let mut s = State::new("> > double BQ\n");
    let snap = s.snapshot();
    eprintln!("=== P4: nested BQ prefix ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// 嵌套容器边界

#[test]
fn p5_nested_bq_vanishes() {
    let mut s = State::new("> > nested BQ\n");
    let snap = s.snapshot();
    eprintln!("=== P5: nested BQ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p5_bq_in_list() {
    let mut s = State::new("- a\n  > bq inside list\n");
    let snap = s.snapshot();
    eprintln!("=== P5: BQ in list ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn p5_bq_bq_nested_close() {
    let mut s = State::new("> outer\n> > nested\n> back\n");
    let snap = s.snapshot();
    eprintln!("=== P5: [BQ,BQ] nested close ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// RefDef 未知

#[test]
fn p7_refdef_unknown() {
    let mut s = State::new("[foo]: /url \"title\"\n\ntext\n");
    let snap = s.snapshot();
    eprintln!("=== P7: RefDef ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Combined: BQ > BLT > OL + inline

#[test]
fn combined_bq_blt_ol() {
    let mut s = State::new("> * star\n>   1. ordered **bold**\n> * star2\n");
    let snap = s.snapshot();
    eprintln!("=== Combined: BQ>BLT>OL ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Table I: admit matrix — BLT continuation checks

#[test]
fn table1_para_cont() {
    let mut s = State::new("- a\n  paragraph continuation\n");
    let snap = s.snapshot();
    eprintln!("=== TableI: L2(Paragraph) x K1 ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn table1_codefence_reject() {
    let mut s = State::new("- c\n  ```\n  fence in list\n  ```\n");
    let snap = s.snapshot();
    eprintln!("=== TableI: L2(CodeFence) x K1 ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn table1_bq_fence_composite() {
    // BQ > CF: L3(CodeFence) x K2 — should be Composite(ContinueAs(BQ)+Push(CF))
    let mut s = State::new("> ```\n> fence in BQ\n> ```\n> after fence\n");
    let snap = s.snapshot();
    eprintln!("=== TableI: L3(CodeFence) x K2 ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn table1_htmlblock_swallows() {
    // L5(HtmlBlock) x K1 — html_block in list swallows subsequent
    let mut s = State::new("- a\n  <div>html in list</div>\n- b\n");
    let snap = s.snapshot();
    eprintln!("=== TableI: L5(HtmlBlock) x K1 ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Table II: body variant bugs

#[test]
fn table2_bullet_code_crosskind() {
    // `*` BLT 内 `-` → OtherListItem → cross-kind Push
    let mut s = State::new("* star\n  - dash cross\n");
    let snap = s.snapshot();
    eprintln!("=== TableII: bullet_code *→- crosskind ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn table2_plus_marker() {
    let mut s = State::new("- dash\n+ plus in dash list\n");
    let snap = s.snapshot();
    eprintln!("=== TableII: + marker in BLT ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn table2_loose_list() {
    let mut s = State::new("- item one\n\n- item two\n");
    let snap = s.snapshot();
    eprintln!("=== TableII: LOOSE_BIT loose list ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Table III: depth combination bugs

#[test]
fn table3_bq_blt_cf_fence_close() {
    // [BQ, BLT, CF] — CF closes, should return to BLT/BQ
    let mut s = State::new("> - a\n>   ```\n>   fence code\n>   ```\n> - after fence\n");
    let snap = s.snapshot();
    eprintln!("=== TableIII: [BQ,BLT,CF] fence close ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn table3_bq_cf_close_return_bq() {
    let mut s = State::new("> ```\n> code in BQ\n> ```\n> after fence in BQ\n");
    let snap = s.snapshot();
    eprintln!("=== TableIII: [BQ,CF] close return BQ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Variant: BQ>BLT paragraph continuation

#[test]
fn variant_bq_blt_para_cont() {
    let mut s = State::new("> - item\n>   paragraph continuation\n> - item2\n");
    let snap = s.snapshot();
    eprintln!("=== Variant: BQ>BLT para continuation ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Combined: BQ > H2 + CF + Table

#[test]
fn combined_bq_h2_cf_table() {
    let mut s = State::new("> ## BQ Heading\n> ```js\n> code **bold**\n> ```\n> | A | B |\n> |---|---|\n> | 1 | 2 |\n");
    let snap = s.snapshot();
    eprintln!("=== Combined: BQ>H2+CF+Table ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Table in BQ

#[test]
fn table_in_bq() {
    let mut s = State::new("> | A | B |\n> |---|---|\n> | 1 | 2 |\n");
    let snap = s.snapshot();
    eprintln!("=== Table in BQ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Comment multi-line

#[test]
fn comment_multiline() {
    let mut s = State::new("<!-- after blank\nstill in comment\n-->\n");
    let snap = s.snapshot();
    eprintln!("=== Comment multi-line ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// HtmlBlock type 3,4,5 — inline close failure

#[test]
fn htmlblock_type3_pi() {
    let mut s = State::new("before type3\n\n<?xml version=\"1.0\"?>\n\nafter type3\n");
    let snap = s.snapshot();
    eprintln!("=== HtmlBlock type3 ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_type4_doctype() {
    let mut s = State::new("before type4\n\n<!DOCTYPE html>\n\nafter type4\n");
    let snap = s.snapshot();
    eprintln!("=== HtmlBlock type4 ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_type5_cdata() {
    let mut s = State::new("before type5\n\n<![CDATA[ some data ]]>\n\nafter type5\n");
    let snap = s.snapshot();
    eprintln!("=== HtmlBlock type5 ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// ThematicBreak adjacent

#[test]
fn thematic_break_adjacent() {
    let mut s = State::new("***\n___\n");
    let snap = s.snapshot();
    eprintln!("=== TB adjacent ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// OL numbers

#[test]
fn ol_numbers() {
    let mut s = State::new("99. big number\n1. small number\n0. zero start\n");
    let snap = s.snapshot();
    eprintln!("=== OL numbers ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Setext in list

#[test]
fn setext_in_list() {
    let mut s = State::new("- a\n  ===\n");
    let snap = s.snapshot();
    eprintln!("=== Setext in list ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// BQ inline link

#[test]
fn bq_inline_link() {
    let mut s = State::new("> [link text](https://example.com)\n");
    let snap = s.snapshot();
    eprintln!("=== BQ inline link ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Tilde fence in BQ

#[test]
fn tilde_fence_in_bq() {
    let mut s = State::new("> ~~~\n> tilde code\n> ~~~\n");
    let snap = s.snapshot();
    eprintln!("=== Tilde fence in BQ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}


#[test]
fn gap_ref_links() {
    // Reference + shortcut links depend on RefMap
    let mut s = State::new("[ref]: https://x.com \"title\"\n\n[link text][ref]\n[shortcut]\n");
    let snap = s.snapshot();
    eprintln!("=== GAP: ref links ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn gap_table_inline() {
    let mut s = State::new("| **bold** | `code` |\n|----------|--------|\n| *italic* | [link](url) |\n");
    let snap = s.snapshot();
    eprintln!("=== GAP: table inline ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn gap_htmlblock_type7() {
    let mut s = State::new("<p>\ntype7 content\n</p>\n\nafter\n");
    let snap = s.snapshot();
    eprintln!("=== GAP: HtmlBlock type7 ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn gap_multi_blank_in_list() {
    let mut s = State::new("- a\n\n\n- b\n");
    let snap = s.snapshot();
    eprintln!("=== GAP: multi blank in list ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn gap_incr_pipeline_add() {
    // P7: incr_patches pipeline test
    use mdio_core::incr::{incr_patches, Patch, IncrTask};
    use mdio_core::block::PackedLineInfo;
    use std::sync::Arc;
    let mut state = State::new("# H\n- a\n- b\n");
    let snap_old = Arc::new(state.snapshot());
    state.edit(7, 7, "- x\n"); // insert "- x\n" between "- a\n" and "- b\n"
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("ripple_diff");
    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(7, 7)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    // 探针类保持 CF-only——纯 eprintln 诊断、无断言,双跑无价值。
    let patches = incr_patches(&task);
    eprintln!("=== GAP: incr pipeline ADD ===");
    eprintln!("snapshot FL entries: {:?}", task.snap.frame_log.entries.iter().map(|&e| {
        let tag = if mdio_core::block::entry_is_body_mut(e) { "BM" }
            else if mdio_core::block::entry_is_pop(e) { "Pop" } else { "Push" };
        format!("{}@{}:{:?}", tag, mdio_core::block::entry_line(e), mdio_core::block::entry_frame(e).kind())
    }).collect::<Vec<_>>());
    for (i, p) in patches.iter().enumerate() {
        match p {
            Patch::Add { path, value: _ } => eprintln!("  patch[{}]: ADD {}", i, path),
            Patch::Remove { path } => eprintln!("  patch[{}]: REMOVE {}", i, path),
            Patch::Replace { path, value: _ } => eprintln!("  patch[{}]: REPLACE {}", i, path),
            Patch::ReplaceAttr { path, attr, .. } => {
                eprintln!("  patch[{}]: RATTR {path} {attr}", i)
            }
        }
    }
}

#[test]
fn gap_incr_pipeline_edit_content() {
    // P7: content-only edit should produce REPLACE, not ADD+REMOVE
    use mdio_core::incr::{incr_patches, Patch, IncrTask};
    use mdio_core::block::PackedLineInfo;
    use std::sync::Arc;
    let mut state = State::new("# H\n- old\n");
    let snap_old = Arc::new(state.snapshot());
    state.edit(6, 9, "new"); // "- old" → "- new"
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("ripple_diff");
    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(6, 9)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    // 探针类保持 CF-only——纯 eprintln 诊断、无断言,双跑无价值。
    let patches = incr_patches(&task);
    eprintln!("=== GAP: incr pipeline content edit ===");
    eprintln!("snapshot AST: {}", task.snap.to_ast_object_mode(ProjectionMode::Cf));
    for (i, p) in patches.iter().enumerate() {
        match p {
            Patch::Add { path, value: _ } => eprintln!("  patch[{}]: ADD {}", i, path),
            Patch::Remove { path } => eprintln!("  patch[{}]: REMOVE {}", i, path),
            Patch::Replace { path, value: _ } => eprintln!("  patch[{}]: REPLACE {}", i, path),
            Patch::ReplaceAttr { path, attr, .. } => {
                eprintln!("  patch[{}]: RATTR {path} {attr}", i)
            }
        }
    }
}

// item 数变化：容器不变，但 item 计数必须反映真实变化

#[test]
fn theory_item_count_mismatch_insert() {
    // Insert "- x\n" into "- a\n- b\n- c" → 3 items → 4 items
    // Container (BLT) stays the same → matched. But item count changes.
    // Theory predicts: S_Ed covers the new line, but the OLD item "c" at idx 2
    // shifted to idx 3 — requires item-level ADD for the new last item.
    use mdio_core::incr::{incr_patches, Patch, IncrTask};
    use mdio_core::block::PackedLineInfo;
    use std::sync::Arc;
    let mut state = State::new("- a\n- b\n- c\n");
    let snap_old = Arc::new(state.snapshot());
    state.edit(4, 4, "- x\n"); // insert between a and b
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("ripple_diff");
    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(4, 4)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    // 探针类保持 CF-only——纯 eprintln 诊断、无断言,双跑无价值。
    let patches = incr_patches(&task);
    eprintln!("=== THEORY: item count insert (3→4) ===");
    eprintln!("snapshot AST: {}", task.snap.to_ast_object_mode(ProjectionMode::Cf));
    for (i, p) in patches.iter().enumerate() {
        match p {
            Patch::Add { path, value: _ } => eprintln!("  patch[{}]: ADD {}", i, path),
            Patch::Remove { path } => eprintln!("  patch[{}]: REMOVE {}", i, path),
            Patch::Replace { path, value: _ } => eprintln!("  patch[{}]: REPLACE {}", i, path),
            Patch::ReplaceAttr { path, attr, .. } => {
                eprintln!("  patch[{}]: RATTR {path} {attr}", i)
            }
        }
    }
    // item 级 ADD 应覆盖新增的第 4 个 item。
    let add_count = patches.iter().filter(|p| matches!(p, Patch::Add { .. })).count();
    let replace_count = patches.iter().filter(|p| matches!(p, Patch::Replace { .. })).count();
    eprintln!("  ADD={} REPLACE={} (theory: ADD>=1 for new item)", add_count, replace_count);
}

#[test]
fn theory_item_count_mismatch_delete() {
    // Delete "- b\n" from "- a\n- b\n- c" → 3 items → 2 items
    // Container matched. Old item "c" at idx 2 should be deleted.
    use mdio_core::incr::{incr_patches, Patch, IncrTask};
    use mdio_core::block::PackedLineInfo;
    use std::sync::Arc;
    let mut state = State::new("- a\n- b\n- c\n");
    let snap_old = Arc::new(state.snapshot());
    state.edit(4, 8, ""); // delete "- b\n"
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("ripple_diff");
    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(4, 8)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    // 探针类保持 CF-only——纯 eprintln 诊断、无断言,双跑无价值。
    let patches = incr_patches(&task);
    eprintln!("=== THEORY: item count delete (3→2) ===");
    eprintln!("snapshot AST: {}", task.snap.to_ast_object_mode(ProjectionMode::Cf));
    for (i, p) in patches.iter().enumerate() {
        match p {
            Patch::Add { path, value: _ } => eprintln!("  patch[{}]: ADD {}", i, path),
            Patch::Remove { path } => eprintln!("  patch[{}]: REMOVE {}", i, path),
            Patch::Replace { path, value: _ } => eprintln!("  patch[{}]: REPLACE {}", i, path),
            Patch::ReplaceAttr { path, attr, .. } => {
                eprintln!("  patch[{}]: RATTR {path} {attr}", i)
            }
        }
    }
    let remove_count = patches.iter().filter(|p| matches!(p, Patch::Remove { .. })).count();
    eprintln!("  REMOVE={} (theory: REMOVE>=1 for deleted item)", remove_count);
}

// Loose list — block structure + inline children

#[test]
fn loose_list_structure() {
    // Tight vs loose: blank line between items → LOOSE_BIT → paragraph wrapper
    let mut s = State::new("- tight a\n- tight b\n");
    let snap_tight = s.snapshot();
    eprintln!("=== Loose: tight list ===");
    eprintln!("AST: {}", snap_tight.to_ast_object_mode(ProjectionMode::Cf));

    let mut s2 = State::new("- loose a\n\n- loose b\n");
    let snap_loose = s2.snapshot();
    eprintln!("=== Loose: loose list ===");
    eprintln!("AST: {}", snap_loose.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn loose_list_with_inline() {
    // Loose list with inline content — verifies P1 interaction with LOOSE_BIT
    let mut s = State::new("- **bold** loose\n\n- *italic* loose\n");
    let snap = s.snapshot();
    eprintln!("=== Loose: inline in loose list ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn loose_list_nested() {
    // Nested loose list
    let mut s = State::new("- outer\n\n  - inner loose\n");
    let snap = s.snapshot();
    eprintln!("=== Loose: nested loose list ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Fence char cross-testing

#[test]
fn fence_backtick_in_tilde() {
    // ``` inside ~~~ fence — should be CodeContent, not close
    let mut s = State::new("~~~\ncode\n```\nmore code\n~~~\n");
    let snap = s.snapshot();
    eprintln!("=== Fence: ``` inside ~~~ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn fence_tilde_in_backtick() {
    // ~~~ inside ``` fence — should be CodeContent
    let mut s = State::new("```\ncode\n~~~\nmore code\n```\n");
    let snap = s.snapshot();
    eprintln!("=== Fence: ~~~ inside ``` ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn fence_bq_tilde() {
    // ~~~ fence inside BQ — P2 double render + fence close
    let mut s = State::new("> ~~~\n> tilde code\n> ~~~\n> after\n");
    let snap = s.snapshot();
    eprintln!("=== Fence: BQ > ~~~ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn fence_bq_backtick_vs_tilde() {
    // Two separate fences in BQ — backtick then tilde
    let mut s = State::new("> ```\n> bt code\n> ```\n> ~~~\n> tilde code\n> ~~~\n");
    let snap = s.snapshot();
    eprintln!("=== Fence: BQ > ``` then ~~~ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn fence_nested_list_backtick() {
    // ``` fence inside nested BLT
    let mut s = State::new("- a\n  - b\n    ```\n    fence code\n    ```\n");
    let snap = s.snapshot();
    eprintln!("=== Fence: nested BLT > CF ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn fence_nested_list_tilde() {
    // ~~~ fence inside nested BLT
    let mut s = State::new("- a\n  - b\n    ~~~\n    tilde code\n    ~~~\n");
    let snap = s.snapshot();
    eprintln!("=== Fence: nested BLT > ~~~ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn fence_bq_tilde_full() {
    // BQ > BLT > ~~~ fence — triple nesting with tilde
    let mut s = State::new("> - item\n>   ~~~\n>   tilde code\n>   ~~~\n> - after\n");
    let snap = s.snapshot();
    eprintln!("=== Fence: BQ > BLT > ~~~ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

// Complex BQ nesting — visual inspection

#[test]
fn complex_bq_bq_bq_inline() {
    // Triple nested BQ with inline
    let mut s = State::new("> outer\n> > mid\n> > > inner **bold**\n> > back to mid\n> back to outer\n");
    let snap = s.snapshot();
    eprintln!("=== Complex: BQ>BQ>BQ inline ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn complex_bq_blt_ol() {
    // BQ > BLT > OL with inline code and emphasis
    let mut s = State::new("> - a\n>   1. ordered `code`\n>   2. item *italic*\n> - b\n");
    let snap = s.snapshot();
    eprintln!("=== Complex: BQ>BLT>OL ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn complex_bq_bq_mix_close() {
    // BQ>BQ with mixed content and nested closing
    let mut s = State::new("> out1\n> > **nested bold**\n> > mid para\n> out2\n");
    let snap = s.snapshot();
    eprintln!("=== Complex: BQ>BQ mix close ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn complex_bq_cf_bq() {
    // BQ > CF — fence inside BQ
    let mut s = State::new("> ```\n> code\n> ```\n> after fence\n");
    let snap = s.snapshot();
    eprintln!("=== Complex: BQ>CF ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn full_md_review() {
    let md = "##H1

top **bold** `code` *italic*

- **bold** bullet
- `code` bullet
- plain

1. **bold** ol
2. normal

> **bold** bq
> `code` bq

> - bq bullet **bold**
>   bq para

- outer
  1. inner ol
  - inner bullet

| L | C | R |
|:---|:---:|---:|
| a | b | c |

<?xml?>

after t3

<!DOCTYPE>

after t4
";
    let mut s = State::new(md);
    eprintln!("=== Full MD Review ===");
    eprintln!("{}", s.snapshot().to_ast_object_mode(ProjectionMode::Cf));
}

// HtmlBlock type coverage — remaining gaps

#[test]
fn htmlblock_type1_script() {
    let mut s = State::new("<script>\nscript content **bold**\n</script>\n\nafter script\n");
    let snap = s.snapshot();
    eprintln!("=== Html: type1 script ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_type1_pre() {
    let mut s = State::new("<pre>\npre content\n</pre>\n\nafter pre\n");
    let snap = s.snapshot();
    eprintln!("=== Html: type1 pre ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_type1_style() {
    let mut s = State::new("<style>\nbody { color: red; }\n</style>\n\nafter style\n");
    let snap = s.snapshot();
    eprintln!("=== Html: type1 style ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_type1_in_bq() {
    // type1 inside BQ — P2 double render + html block
    let mut s = State::new("> <pre>\n> code in bq\n> </pre>\n> after\n");
    let snap = s.snapshot();
    eprintln!("=== Html: type1 pre in BQ ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_type1_in_list() {
    // type1 inside list — should reject (per TableI L5×K1)
    let mut s = State::new("- a\n  <pre>\n  code in list\n  </pre>\n- b\n");
    let snap = s.snapshot();
    eprintln!("=== Html: type1 pre in list ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_type2_comment_fallback() {
    // Verify <!-- --> is handled as Comment, not HtmlBlock type2
    let mut s = State::new("<!-- this is a comment -->\n\ntext after\n");
    let snap = s.snapshot();
    eprintln!("=== Html: type2 <!-- --> ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_type6_div() {
    let mut s = State::new("<div>\ndiv content\n</div>\n\nafter div\n");
    let snap = s.snapshot();
    eprintln!("=== Html: type6 div ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_type6_blank_close() {
    // type6 closes on blank line — verify </div> is NOT swallowed
    let mut s = State::new("<div>\ncontent\n\n</div>\n\nafter\n");
    let snap = s.snapshot();
    eprintln!("=== Html: type6 blank close ===");
    eprintln!("AST: {}", snap.to_ast_object_mode(ProjectionMode::Cf));
}

#[test]
fn htmlblock_all_types_summary() {
    eprintln!("=== Html: ALL TYPES SUMMARY ===");
    // type1: <pre> — explicit close tag
    let mut s1 = State::new("<pre>\nt1\n</pre>\n\ntext\n");
    eprintln!("type1: {}", s1.snapshot().to_ast_object_mode(ProjectionMode::Cf));
    // type3: <? — inline close ?>
    let mut s3 = State::new("<?xml?>\n\ntext\n");
    eprintln!("type3: {}", s3.snapshot().to_ast_object_mode(ProjectionMode::Cf));
    // type4: <!LETTER — inline close >
    let mut s4 = State::new("<!DOCTYPE html>\n\ntext\n");
    eprintln!("type4: {}", s4.snapshot().to_ast_object_mode(ProjectionMode::Cf));
    // type5: <![CDATA[ — inline close ]]>
    let mut s5 = State::new("<![CDATA[data]]>\n\ntext\n");
    eprintln!("type5: {}", s5.snapshot().to_ast_object_mode(ProjectionMode::Cf));
    // type6: <div> — blank line close
    let mut s6 = State::new("<div>\ndata\n</div>\n\ntext\n");
    eprintln!("type6: {}", s6.snapshot().to_ast_object_mode(ProjectionMode::Cf));
    // type7: <p> — blank line close
    let mut s7 = State::new("<p>\ndata\n</p>\n\ntext\n");
    eprintln!("type7: {}", s7.snapshot().to_ast_object_mode(ProjectionMode::Cf));
}

// Full document smoke test

#[test]
fn full_doc_smoke() {
    let md = "# **bold** heading\n\n> **Q** `code` *italic*\n\n- **bold** `code` ~~strike~~\n1. *italic* [link](/url \"tip\")\n- [x] **done**\n\n<div class=\"red\">html</div>\n\n<https://autolink>\n\n```py\nx=1\n```\n\n|a|b|\n|-|-|\n|1|2|\n\n&lt;div&gt; entity\n\nline one  \nline two\n\n\n##H1\n\ntop **bold** `code` *italic*\n\n- **bold** bullet\n- `code` bullet\n- plain\n\n1. **bold** ol\n2. normal\n\n> **bold** bq\n> `code` bq\n\n> - bq bullet **bold**\n>   bq para\n\n- outer\n  1. inner ol\n  - inner bullet\n\n| L | C | R |\n|:---|:---:|---:|\n| a | b | c |\n\n<?xml?>\n\nafter t3\n\n<!DOCTYPE>\n\nafter t4\n";
    let mut s = State::new(md);
    let json = s.snapshot().to_ast_object_mode(ProjectionMode::Cf);
    eprintln!("{}", json);
}

// P7 clean: BLT→OL 整行替换，行数不变

#[test]
fn p7_clean_blt_to_ol() {
    // Old: 3 bullets → New: 3 OL items. Same line count, clean replacement.
    use mdio_core::incr::{patches_for_mode, Patch, IncrTask};
    use mdio_core::block::PackedLineInfo;
    use std::sync::Arc;
    let mut state = State::new("- a\n- b\n- c\n");
    let snap_old = Arc::new(state.snapshot());
    let fl_old: Arc<Vec<u64>> = Arc::new(state.frame_log.entries.clone());
    let old_lines: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
    let old_len = state.rope.byte_len();

    eprintln!("=== P7 clean: 3 bullets → 3 OL items ===");
    eprintln!("old FL = {:?}", fl_old.iter().map(|&e| {
        let tag = if mdio_core::block::entry_is_body_mut(e) { "BM" }
            else if mdio_core::block::entry_is_pop(e) { "Pop" } else { "Push" };
        format!("{}@{}:{:?}", tag, mdio_core::block::entry_line(e), mdio_core::block::entry_frame(e).kind())
    }).collect::<Vec<_>>());
    eprintln!("old PL = {:?}", old_lines);
    eprintln!("old byte_len = {}", old_len);

    state.edit(0, old_len, "1. A\n2. B\n3. C\n");
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("ripple_diff");
    eprintln!("ripple_actions = {:?}", diff.ripple_frame_actions.iter().map(|&e| {
        let tag = if mdio_core::block::entry_is_body_mut(e) { "BM" }
            else if mdio_core::block::entry_is_pop(e) { "Pop" } else { "Push" };
        format!("{}@{}:{:?}", tag, mdio_core::block::entry_line(e), mdio_core::block::entry_frame(e).kind())
    }).collect::<Vec<_>>());
    eprintln!("new FL = {:?}", snap.frame_log.entries.iter().map(|&e| {
        let tag = if mdio_core::block::entry_is_body_mut(e) { "BM" }
            else if mdio_core::block::entry_is_pop(e) { "Pop" } else { "Push" };
        format!("{}@{}:{:?}", tag, mdio_core::block::entry_line(e), mdio_core::block::entry_frame(e).kind())
    }).collect::<Vec<_>>());
    eprintln!("new PL = {:?}", (0..snap.line_len()).map(|i| snap.lines[i].0).collect::<Vec<_>>());
    eprintln!("delta = {}", diff.delta);

    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(0, old_len)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };

    // 断言类双跑——同一用例按 mode 自动切换适配(无标题
    // 文档 HS≡CF,双跑等价;标题文档自动走壳操作)。
    for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
        let patches = patches_for_mode(&task, mode);
        eprintln!("[{mode:?}] {} patches:", patches.len());
        for (i, p) in patches.iter().enumerate() {
            match p {
                Patch::Add { path, value: _ } => eprintln!("  [{}] ADD {}", i, path),
                Patch::Remove { path } => eprintln!("  [{}] REMOVE {}", i, path),
                Patch::Replace { path, value: _ } => eprintln!("  [{}] REPLACE {}", i, path),
                Patch::ReplaceAttr { path, attr, .. } => eprintln!("  [{}] RATTR {path} {attr}", i),
            }
        }

        let has_replace = patches.iter().any(|p| matches!(p, Patch::Replace { .. }));
        eprintln!("[{mode:?}] has_replace={}", has_replace);

        // When REMOVE+ADD share same path, they merge into REPLACE
        assert!(has_replace, "MISSING patch — old BLT not replaced with new OL");
    }
}

// P7 边界：BQ 内容编辑

#[test]
fn p7_bq_content_edit() {
    use mdio_core::incr::{incr_patches, Patch, IncrTask};
    use mdio_core::block::PackedLineInfo;
    use std::sync::Arc;
    let mut state = State::new("> **BQ** text\n");
    let snap_old = Arc::new(state.snapshot());

    state.edit(3, 5, "XXX"); // "BQ" → "XXX"
    let snap = state.snapshot();
    eprintln!("BQ: FL={:?} to_ast={}",
        snap.frame_log.entries.iter().map(|&e| format!("{}@{}:{:?}",
            if mdio_core::block::entry_is_pop(e){"Pop"}else{"Push"}, mdio_core::block::entry_line(e), mdio_core::block::entry_frame(e).kind())).collect::<Vec<_>>(),
        snap.to_ast_object_mode(ProjectionMode::Cf));

    let diff = state.ripple_diff.take().expect("ripple_diff");
    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(3, 5)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    // 探针类保持 CF-only——纯 eprintln 诊断、无断言,双跑无价值。
    let patches = incr_patches(&task);
    for (i, p) in patches.iter().enumerate() {
        match p {
            Patch::Add { path, value: _ } => eprintln!("  [{}] ADD {}", i, path),
            Patch::Remove { path } => eprintln!("  [{}] REMOVE {}", i, path),
            Patch::Replace { path, value } => eprintln!("  [{}] REPLACE {} val={}", i, path,
                serde_json::to_string(value).unwrap_or_default().chars().take(200).collect::<String>()),
            Patch::ReplaceAttr { path, attr, .. } => {
                eprintln!("  [{}] RATTR {path} {attr}", i)
            }
        }
    }
}

// P7 边界：BLT→OL 容器替换

#[test]
fn p7_blt_to_ol_replacement() {
    // Old: 4 bullets → New: 2 OL items
    use mdio_core::incr::{incr_patches, Patch, IncrTask};
    use mdio_core::block::PackedLineInfo;
    use std::sync::Arc;
    let mut state = State::new("- a\n- **NEW**\n- b\n- c\n");
    let snap_old = Arc::new(state.snapshot());
    let fl_old: Arc<Vec<u64>> = Arc::new(state.frame_log.entries.clone());
    let old_lines: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
    eprintln!("P7 edge: old FL = {:?}", fl_old.iter().map(|&e| {
        let tag = if mdio_core::block::entry_is_body_mut(e) { "BM" }
            else if mdio_core::block::entry_is_pop(e) { "Pop" } else { "Push" };
        format!("{}@{}:{:?}", tag, mdio_core::block::entry_line(e), mdio_core::block::entry_frame(e).kind())
    }).collect::<Vec<_>>());
    eprintln!("P7 edge: old_lines = {:?}", old_lines);

    state.edit(0, 14, "1. X\n2. Y"); // replace all content
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("ripple_diff");
    eprintln!("P7 edge: ripple_actions = {:?}", diff.ripple_frame_actions.iter().map(|&e| {
        let tag = if mdio_core::block::entry_is_body_mut(e) { "BM" }
            else if mdio_core::block::entry_is_pop(e) { "Pop" } else { "Push" };
        format!("{}@{}:{:?}", tag, mdio_core::block::entry_line(e), mdio_core::block::entry_frame(e).kind())
    }).collect::<Vec<_>>());
    eprintln!("P7 edge: new FL = {:?}", snap.frame_log.entries.iter().map(|&e| {
        let tag = if mdio_core::block::entry_is_body_mut(e) { "BM" }
            else if mdio_core::block::entry_is_pop(e) { "Pop" } else { "Push" };
        format!("{}@{}:{:?}", tag, mdio_core::block::entry_line(e), mdio_core::block::entry_frame(e).kind())
    }).collect::<Vec<_>>());

    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(0, 14)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };

    // 探针类保持 CF-only——纯 eprintln 诊断、无断言,双跑无价值。
    let patches = incr_patches(&task);
    eprintln!("P7 edge: {} patches", patches.len());
    for (i, p) in patches.iter().enumerate() {
        match p {
            Patch::Add { path, value: _ } => eprintln!("  [{}] ADD {}", i, path),
            Patch::Remove { path } => eprintln!("  [{}] REMOVE {}", i, path),
            Patch::Replace { path, value: _ } => eprintln!("  [{}] REPLACE {}", i, path),
            Patch::ReplaceAttr { path, attr, .. } => eprintln!("  [{}] RATTR {path} {attr}", i),
        }
    }

    // Expected: REMOVE old BLT, ADD new OL with 2 items
    let has_remove = patches.iter().any(|p| matches!(p, Patch::Remove { .. }));
    let has_add = patches.iter().any(|p| matches!(p, Patch::Add { .. }));
    eprintln!("P7 edge: has_remove={} has_add={}", has_remove, has_add);
}
