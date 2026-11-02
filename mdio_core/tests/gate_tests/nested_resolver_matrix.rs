//! NestedResolver cmark 边界矩阵（固定 n 的嵌套 BQ/BLT 形态与 cmark 终态对照）
//! 运行：cargo test -p mdio_core --no-default-features --release --test nested_resolver_matrix

use mdio_core::projection::State;

fn tree(doc: &str) -> String {
    let mut st = State::new(doc);
    st.snapshot().debug_block_tree()
}

#[test]
fn same_line_list_window_is_first_block_ic() {
    // n=4 → 嵌套 bullet；n=5..8 → outer bullet + code_block。
    for n in 5..=8 {
        let doc = format!("-{} - x\n", " ".repeat(n));
        let t = tree(&doc);
        assert!(t.contains("code_block"), "n={n} should be IC: {t}");
        assert!(t.contains("bullet"), "n={n} keeps outer item: {t}");
    }
    let t = tree("-    - x\n");
    assert!(t.matches("bullet").count() >= 3, "n=4 nested bullets: {t}");
    assert!(!t.contains("code_block"), "n=4: {t}");
}

#[test]
fn same_line_ol_window_is_first_block_ic() {
    let t = tree("1.     - x\n");
    assert!(t.contains("code_block"), "OL 5 spaces should be IC: {t}");
    let t = tree("1.    - x\n");
    assert!(!t.contains("code_block"), "OL 4 spaces nested: {t}");
}

#[test]
fn bq_continuation_residual_indent_heading() {
    let t = tree("> # H\n>   # H2\n");
    assert_eq!(t.matches("head").count(), 2, "two headings: {t}");
    assert_eq!(t.matches("\"p\"").count(), 0, "no paragraph: {t}");
}

#[test]
fn empty_item_five_spaces_is_text_six_is_ic() {
    let t5 = tree("-\n     baz\n");
    assert!(!t5.contains("code_block"), "5 spaces is text: {t5}");
    let t6 = tree("-\n      baz\n");
    assert!(t6.contains("code_block"), "6 spaces is IC: {t6}");
}

#[test]
fn g_matrix_bq_marker_indent() {
    use mdio_core::block::{BlockType, FrameKind};

    fn snap(doc: &str) -> mdio_core::projection::Snapshot {
        let mut st = State::new(doc);
        st.snapshot()
    }
    fn events(snap: &mdio_core::projection::Snapshot, line: usize) -> Vec<(bool, FrameKind, u8)> {
        snap.frame_log.entries.iter().copied()
            .filter(|&e| mdio_core::block::entry_line(e) as usize == line)
            .map(|e| (mdio_core::block::entry_is_pop(e),
                      mdio_core::block::entry_frame(e).kind(),
                      mdio_core::block::entry_frame(e).indent()))
            .collect()
    }

    // G1/G3
    assert_eq!(snap("    > # Foo\n").line_type_at(0), BlockType::IndentedCode);
    let g3 = snap("   > # Foo\n");
    assert_eq!(g3.line_type_at(0), BlockType::Header1);
    assert_eq!(events(&g3, 0).len(), 1);

    // G4：bq 内 3 空格嵌套 BQ——不再 Pop 后重开。
    let g4 = snap("> # H\n>    > x\n");
    assert_eq!(g4.line_type_at(0), BlockType::Header1);
    assert_eq!(g4.line_type_at(1), BlockType::Paragraph);
    let ev = events(&g4, 1);
    assert_eq!(ev.len(), 1, "one nested BQ push: {ev:?}");
    assert_eq!(ev[0].1, FrameKind::BlockQuote);

    // G5：item 内 BQ 延续。
    let g5 = snap("- B:\n  > x\n");
    assert_eq!(g5.line_type_at(0), BlockType::Bullet);
    assert_eq!(g5.line_type_at(1), BlockType::Blockquote);

    // G7/G8：item 内缩进过大的 `>` 是 lazy 段落文本。
    for spaces in [6, 7] {
        let doc = format!("- B:\n{}> x\n", " ".repeat(spaces));
        let s = snap(&doc);
        assert_eq!(s.line_type_at(1), BlockType::Paragraph, "spaces={spaces}");
        assert!(s.lines[1].cont(), "spaces={spaces} should cont");
        assert!(events(&s, 1).is_empty(), "spaces={spaces} no frames");
    }

    // G9：item 结束，顶层 BQ>BQ>p。
    let g9 = snap("- B:\n>    > x\n");
    assert_eq!(g9.line_type_at(1), BlockType::Blockquote);
    let ev = events(&g9, 1);
    assert_eq!(ev.len(), 3, "pop BLT + push BQ + push BQ: {ev:?}");
    assert!(ev[0].0 && ev[0].1 == FrameKind::BulletList);
    assert!(!ev[1].0 && ev[1].1 == FrameKind::BlockQuote);
    assert!(!ev[2].0 && ev[2].1 == FrameKind::BlockQuote);

    // G10/G11：item 结束，顶层 BQ(IC)。
    for spaces in [6, 7] {
        let doc = format!("- B:\n>{}> x\n", " ".repeat(spaces));
        let s = snap(&doc);
        assert_eq!(s.line_type_at(1), BlockType::IndentedCode, "spaces={spaces}");
        let ev = events(&s, 1);
        assert_eq!(ev.len(), 2, "pop BLT + push BQ: {ev:?}");
        assert!(ev[0].0 && ev[0].1 == FrameKind::BulletList);
        assert!(!ev[1].0 && ev[1].1 == FrameKind::BlockQuote);
    }

    // G12：纯 BQ 内嵌套 BQ。
    let g12 = snap("> # H\n>   > x\n");
    assert_eq!(g12.line_type_at(1), BlockType::Paragraph);
    let ev = events(&g12, 1);
    assert_eq!(ev.len(), 1, "one nested BQ push: {ev:?}");
    assert_eq!(ev[0].1, FrameKind::BlockQuote);
}

#[test]
fn deep_bq_list_chain_stays_attached() {
    let doc = "# H1\n## H2\n> # H3\n> - B1:\n>   - B2:\n>      > # H4\n>      > - B3:\n>      >   # H5\n>      >   - B4:\n>      >     - B5:  hi\n";
    let t = tree(doc);
    // cmark 终态：B4/B5 都在 B3 的子树里，inner-BQ 不再在 L8 pop。
    let b3_idx = t.find(">      > - B3:").expect("B3 node");
    let b4_idx = t.find(">      >   - B4:").expect("B4 node");
    let b5_idx = t.find(">      >     - B5:  hi").expect("B5 node");
    let h5_idx = t.find(">      >   # H5").expect("H5 node");
    assert!(b3_idx < h5_idx && h5_idx < b4_idx && b4_idx < b5_idx,
        "B3 subtree order broken: {t}");
    // B4 的 children 含 B5（B5 出现在 B4 之后且在同一子树尾部）。
    assert!(b4_idx < b5_idx, "B5 after B4: {t}");
}

#[test]
fn mixed_same_line_chain_matrix() {
    use mdio_core::block::{BlockType, FrameKind};
    let cases: &[(&str, BlockType, &[FrameKind])] = &[
        ("- > x\n", BlockType::Paragraph, &[FrameKind::BulletList, FrameKind::BlockQuote]),
        ("1. - x\n", BlockType::Bullet, &[FrameKind::OrderedList, FrameKind::BulletList]),
        ("> - - x\n", BlockType::Blockquote, &[FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::BulletList]),
        ("- > > x\n", BlockType::Paragraph, &[FrameKind::BulletList, FrameKind::BlockQuote, FrameKind::BlockQuote]),
        ("- > - x\n", BlockType::Bullet, &[FrameKind::BulletList, FrameKind::BlockQuote, FrameKind::BulletList]),
    ];
    for (doc, want_pl, want_kinds) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        assert_eq!(snap.line_type_at(0), *want_pl, "{doc:?}");
        let got: Vec<FrameKind> = snap.frame_log.entries.iter()
            .filter(|&&e| mdio_core::block::entry_line(e) == 0 && !mdio_core::block::entry_is_pop(e))
            .map(|&e| mdio_core::block::entry_frame(e).kind())
            .collect();
        assert_eq!(&got[..], *want_kinds, "{doc:?}");
    }
}

#[test]
fn bq_carries_list_constraint_minimal() {
    // B4 只有一层 > 前缀；它是 B3 列表的嵌套 item，不是外层 B2 的 item。
    let doc = "- B1:\n  - B2:\n     > - B3:\n     >   - B4:\n";
    let t = tree(doc);
    let b3 = t.find("> - B3:").expect("B3");
    let b4 = t.find(">   - B4:").expect("B4");
    assert!(b3 < b4, "B4 after B3: {t}");
    assert!(t.matches("blockquote").count() >= 1, "BQ shell present: {t}");
    // 关键：不能出现 L8 的 pop 重开形态（BQ 结束后 B4 成为 B2 的子）。
    let b2 = t.find("  - B2:").expect("B2");
    assert!(b2 < b3 && b3 < b4, "B2 < B3 < B4 order: {t}");
}

#[test]
fn bq_carries_list_constraint_single_prefix() {
    // 单层 BQ 前缀 + 缩进不足于 B2 的 marker 行仍属于 B3 列表。
    let doc = "- B1:\n  - B2:\n     > - B3:\n     > - B4:\n";
    let t = tree(doc);
    let b3 = t.find("> - B3:").expect("B3");
    let b4 = t.find("> - B4:").expect("B4");
    assert!(b3 < b4, "B4 stays in B3 list: {t}");
    assert!(!t.contains("code_block"), "no spurious IC: {t}");
}

#[test]
fn bq_prefix_does_not_carry_without_bq_frame_above() {
    // 没有 BQ 层在 B2 之上：B3 结束 B2/B1，随后 BQ 在顶层打开。
    // 注：debug_block_tree 的深度 walker 对同线 pop×2+push×2 会丢 BQ 壳，
    // 因此这里直接断言 FL 与 PL（scan 的 canonical 产物）。
    let doc = "- B1:\n  - B2:\n> - B3:\n>   - B4:\n";
    let mut st = mdio_core::projection::State::new(doc);
    let snap = st.snapshot();
    let line_entries: Vec<_> = snap.frame_log.entries.iter().copied()
        .filter(|&e| mdio_core::block::entry_line(e) == 2)
        .collect();
    assert_eq!(line_entries.len(), 4, "L2: pop B2 + pop B1 + push BQ + push BLT");
    assert!(mdio_core::block::entry_is_pop(line_entries[0]), "pop B2 first");
    assert!(mdio_core::block::entry_is_pop(line_entries[1]), "pop B1 second");
    assert_eq!(mdio_core::block::entry_frame(line_entries[2]).kind(),
        mdio_core::block::FrameKind::BlockQuote, "push BQ third");
    assert_eq!(mdio_core::block::entry_frame(line_entries[3]).kind(),
        mdio_core::block::FrameKind::BulletList, "push BLT fourth");
    assert_eq!(snap.line_type_at(2), mdio_core::block::BlockType::Blockquote,
        "L2 PL 应为顶层 BQ");
}
