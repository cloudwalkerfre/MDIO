use super::*;


#[test]
fn test_pack_unpack_roundtrip() {
    let types = [
        BlockType::Header1, BlockType::Header2, BlockType::Header3,
        BlockType::Header4, BlockType::Header5, BlockType::Header6,
        BlockType::Bullet, BlockType::CodeFenceOpen, BlockType::CodeFenceClose,
        BlockType::Blockquote, BlockType::OrderedListItem,
        BlockType::Paragraph, BlockType::CodeContent, BlockType::Comment,
        BlockType::ThematicBreak, BlockType::Blank,
        BlockType::TableRow, BlockType::TableSeparator,
    ];
    for &t in &types {
        let packed = PackedLineInfo::pack(t);
        assert!(packed.is_valid());
        assert_eq!(packed.node_type(), t);
    }
}

#[test]
fn test_invalid_dummy() {
    let dummy = PackedLineInfo::invalid();
    assert!(!dummy.is_valid());
    let valid = PackedLineInfo::pack(BlockType::Paragraph);
    assert_ne!(dummy.0, valid.0);
}

#[test]
fn test_scan_block_headers() {
    assert_eq!(scan_block(b"# Title", &[], None).0, BlockType::Header1);
    assert_eq!(scan_block(b"## Title", &[], None).0, BlockType::Header2);
    assert_eq!(scan_block(b"###### Title", &[], None).0, BlockType::Header6);
    assert_eq!(scan_block(b"####### Title", &[], None).0, BlockType::Paragraph); // Too many #
    assert_eq!(scan_block(b"#Tag", &[], None).0, BlockType::Paragraph); // No space
}

#[test]
fn test_scan_block_bullet() {
    assert_eq!(scan_block(b"- Item", &[], None).0, BlockType::Bullet);
    assert_eq!(scan_block(b"* Item", &[], None).0, BlockType::Bullet);
    assert_eq!(scan_block(b"+ Item", &[], None).0, BlockType::Bullet);
    assert_eq!(scan_block(b"-Item", &[], None).0, BlockType::Paragraph); // No space
    assert_eq!(scan_block(b"*Item", &[], None).0, BlockType::Paragraph); // No space
    // _ is not a bullet marker (CommonMark)
    assert_eq!(scan_block(b"_ text", &[], None).0, BlockType::Paragraph);
}

#[test]
fn test_scan_block_thematic_break() {
    assert_eq!(scan_block(b"---", &[], None).0, BlockType::ThematicBreak);
    assert_eq!(scan_block(b"***", &[], None).0, BlockType::ThematicBreak);
    assert_eq!(scan_block(b"___", &[], None).0, BlockType::ThematicBreak);
    assert_eq!(scan_block(b"----", &[], None).0, BlockType::ThematicBreak);
    assert_eq!(scan_block(b"*****", &[], None).0, BlockType::ThematicBreak);
    assert_eq!(scan_block(b"---   ", &[], None).0, BlockType::ThematicBreak); // trailing spaces
    assert_eq!(scan_block(b"- - -", &[], None).0, BlockType::ThematicBreak); // spaces between markers
    assert_eq!(scan_block(b"* * *", &[], None).0, BlockType::ThematicBreak); // spaces between markers
    assert_eq!(scan_block(b"--", &[], None).0, BlockType::Paragraph); // only 2
    assert_eq!(scan_block(b"--- text", &[], None).0, BlockType::Paragraph); // extra content
    assert_eq!(scan_block(b"+++", &[], None).0, BlockType::Paragraph); // + is not thematic break
    assert_eq!(scan_block(b"===", &[], None).0, BlockType::Paragraph); // = is not thematic break
}

#[test]
fn test_scan_block_with_stack_plus_plus_plus() {
    // +++ should be Paragraph, not ThematicBreak (not a valid HR marker)
    let (t, a) = scan_block(b"+++", &[], None);
    assert_eq!(t, BlockType::Paragraph, "+++ standalone should be Paragraph, got {:?}", t);
    assert!(matches!(a, ScanAction::Continue), "+++ should be Continue, got {:?}", a);

    // +++ after paragraph should still be Paragraph (not setext)
    let (t, a) = scan_block(b"+++", &[], Some(BlockType::Paragraph));
    assert_eq!(t, BlockType::Paragraph, "+++ after paragraph should be Paragraph, got {:?}", t);

    // === should be Paragraph standalone (only = is HsSep, which requires prev Paragraph)
    let (t, _) = scan_block(b"===", &[], None);
    assert_eq!(t, BlockType::Paragraph, "=== standalone should be Paragraph, got {:?}", t);
}

#[test]
fn test_scan_block_fence() {
    let bt_fence = BlockFrame::new(FrameKind::CodeFence, 0, BlockFrame::encode_cf_body(b'`', 3));
    let tilde_fence = BlockFrame::new(FrameKind::CodeFence, 0, BlockFrame::encode_cf_body(b'~', 3));
    // Backtick fence — opening
    assert_eq!(scan_block(b"```", &[], None).0, BlockType::CodeFenceOpen);
    assert_eq!(scan_block(b"```rust", &[], None).0, BlockType::CodeFenceOpen);
    // Inside backtick fence
    assert_eq!(scan_block(b"```", &[bt_fence], None).0, BlockType::CodeFenceClose);
    assert_eq!(scan_block(b"code", &[bt_fence], None).0, BlockType::CodeContent);
    // Tilde fence — opening
    assert_eq!(scan_block(b"~~~", &[], None).0, BlockType::CodeFenceOpen);
    assert_eq!(scan_block(b"~~~python", &[], None).0, BlockType::CodeFenceOpen);
    // Inside tilde fence
    assert_eq!(scan_block(b"~~~", &[tilde_fence], None).0, BlockType::CodeFenceClose);
    assert_eq!(scan_block(b"code", &[tilde_fence], None).0, BlockType::CodeContent);
    // Fences don't mix
    assert_eq!(scan_block(b"```", &[tilde_fence], None).0, BlockType::CodeContent);
    assert_eq!(scan_block(b"~~~", &[bt_fence], None).0, BlockType::CodeContent);
}

#[test]
fn test_scan_block_indented_code() {
    assert_eq!(scan_block(b"    code", &[], None).0, BlockType::IndentedCode);
    assert_eq!(scan_block(b"        code", &[], None).0, BlockType::IndentedCode);
    assert_eq!(scan_block(b"   code", &[], None).0, BlockType::Paragraph);
}

#[test]
fn test_scan_block_comment() {
    let cmt = BlockFrame::new(FrameKind::Comment, 0, BlockFrame::encode_spare_body());
    assert_eq!(scan_block(b"<!-- comment -->", &[], None).0, BlockType::Comment);
    assert_eq!(scan_block(b"<!-- comment", &[], None).0, BlockType::Comment);
    assert_eq!(scan_block(b" comment -->", &[cmt], None).0, BlockType::Comment);
    assert_eq!(scan_block(b" no comment", &[cmt], None).0, BlockType::Comment);
}

#[test]
fn test_scan_block_blank() {
    assert_eq!(scan_block(b"", &[], None).0, BlockType::Blank);
    assert_eq!(scan_block(b"   ", &[], None).0, BlockType::Blank);
    // All blank lines → Blank
    assert_eq!(scan_block(b"    ", &[], None).0, BlockType::Blank);
    assert_eq!(scan_block(b"\t\t", &[], None).0, BlockType::Blank);
    assert_eq!(scan_block(b"    \t", &[], None).0, BlockType::Blank);
    // In code block, blank line should be CodeContent
    let bt = BlockFrame::new(FrameKind::CodeFence, 0, BlockFrame::encode_cf_body(b'`', 3));
    let tilde = BlockFrame::new(FrameKind::CodeFence, 0, BlockFrame::encode_cf_body(b'~', 3));
    let cmt = BlockFrame::new(FrameKind::Comment, 0, BlockFrame::encode_spare_body());
    assert_eq!(scan_block(b"    ", &[bt], None).0, BlockType::CodeContent);
    assert_eq!(scan_block(b"    ", &[tilde], None).0, BlockType::CodeContent);
    assert_eq!(scan_block(b"    ", &[cmt], None).0, BlockType::Comment);
}

#[test]
fn test_char_lut_values() {
    assert_eq!(BLOCK_DISPATCH[b'#' as usize], 1);
    assert_eq!(BLOCK_DISPATCH[b'-' as usize], 2);
    assert_eq!(BLOCK_DISPATCH[b'*' as usize], 2);
    assert_eq!(BLOCK_DISPATCH[b'+' as usize], 2);
    assert_eq!(BLOCK_DISPATCH[b'_' as usize], 2);
    assert_eq!(BLOCK_DISPATCH[b'`' as usize], 3);
    assert_eq!(BLOCK_DISPATCH[b'~' as usize], 3);
    assert_eq!(BLOCK_DISPATCH[b'<' as usize], 4);
    assert_eq!(BLOCK_DISPATCH[b'>' as usize], 5);
    for d in b'0'..=b'9' {
        assert_eq!(BLOCK_DISPATCH[d as usize], 6);
    }
    assert_eq!(BLOCK_DISPATCH[b'a' as usize], 0);
    assert_eq!(BLOCK_DISPATCH[b' ' as usize], 0);
}

#[test]
fn test_scan_block_blockquote() {
    assert_eq!(scan_block(b"> text", &[], None).0, BlockType::Blockquote);
    assert!(matches!(scan_block(b"> text", &[], None).1, ScanAction::Push(_)));
    // >= without space after > IS a blockquote (CommonMark §5.1 option b)
    assert_eq!(scan_block(b">= text", &[], None).0, BlockType::Blockquote);
    // bare > without content is still blockquote
    assert_eq!(scan_block(b">", &[], None).0, BlockType::Blockquote);
    // Blank line after > is just Blank (aggregation handled by get_node_end)
    assert_eq!(scan_block(b"", &[], None).0, BlockType::Blank);
}

#[test]
fn test_scan_block_ordered_item() {
    assert_eq!(scan_block(b"1. item", &[], None).0, BlockType::OrderedListItem);
    assert_eq!(scan_block(b"42. answer", &[], None).0, BlockType::OrderedListItem);
    // Not an ordered list item
    assert_eq!(scan_block(b"2023year", &[], None).0, BlockType::Paragraph);
    assert_eq!(scan_block(b"1.item", &[], None).0, BlockType::Paragraph); // no space
    assert_eq!(scan_block(b"1 . item", &[], None).0, BlockType::Paragraph); // no space after digit
}

#[test]
fn test_pack_unpack_new_types() {
    let types = [BlockType::Blockquote, BlockType::OrderedListItem,
                 BlockType::HsHead1, BlockType::HtmlBlock, BlockType::IndentedCode];
    for &t in &types {
        let packed = PackedLineInfo::pack(t);
        assert!(packed.is_valid());
        assert_eq!(packed.node_type(), t);
    }
}

// Regression tests — full type coverage

/// All 25 BlockType discriminants, in numeric order.
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
fn test_all_types_pack_unpack() {
    for &t in &ALL_TYPES {
        let packed = PackedLineInfo::pack(t);
        assert!(packed.is_valid(), "pack({:?}) should be valid", t);
        assert_eq!(packed.node_type(), t, "roundtrip failed for {:?}", t);
    }
}

#[test]
fn test_all_types_in_bounds() {
    // Every discriminant must be < 64 for TYPE_MASK = 0x3F
    for &t in &ALL_TYPES {
        let d = t as u8;
        assert!(d < 64, "{:?} discriminant {} should be < 64", t, d);
    }
}

#[test]
fn test_no_indent_blank_line() {
    let cases: &[&[u8]] = &[b"", b"   ", b"    ", b"\t\t", b"    \t"];
    for &input in cases {
        let (t, _) = scan_block(input, &[], None);
        assert!(matches!(t, BlockType::Blank),
            "got {:?} for input {:?}", t, input);
    }
}

#[test]
fn test_indented_code_from_scan() {
    let (t, _) = scan_block(b"    code", &[], None);
    assert_eq!(t, BlockType::IndentedCode);
}

#[test]
fn test_thematic_break_not_setext_context() {
    // --- without preceding paragraph → ThematicBreak
    let (t, _) = scan_block(b"---", &[], None);
    assert_eq!(t, BlockType::ThematicBreak);
}

#[test]
fn test_header_level_values() {
    // Each header's discriminant must align with header_level()
    let expected: &[(BlockType, u8)] = &[
        (BlockType::Header1, 1), (BlockType::Header2, 2), (BlockType::Header3, 3),
        (BlockType::Header4, 4), (BlockType::Header5, 5), (BlockType::Header6, 6),
    ];
    for &(t, expected_level) in expected {
        let d = t as u8;
        assert_eq!(d - HEADER_LO + 1, expected_level,
            "{:?} discriminant {} should map to level {}", t, d, expected_level);
    }
    // HEADER_LO..=HEADER_HI range
    for &(t, _) in expected {
        let d = t as u8;
        assert!(d >= HEADER_LO && d <= HEADER_HI,
            "{:?} ({}) outside HEADER range", t, d);
    }
}

#[test]
fn test_html_block_from_legacy() {
    assert_eq!(scan_block(b"<div>", &[], None).0, BlockType::HtmlBlock);
    assert_eq!(scan_block(b"<script>", &[], None).0, BlockType::HtmlBlock);
    assert_eq!(scan_block(b"<!-- comment -->", &[], None).0, BlockType::Comment);
}

// Container-aware scanner tests

const BQ_PARA_OPEN: u8 = 0x80; // old-spec compat

/// Test helper: convert old-format spec to new body encoding.
fn frame(kind: FrameKind, threshold: u8, old_spec: u8) -> BlockFrame {
    let body = match kind {
        FrameKind::BulletList => BlockFrame::encode_blt_body(old_spec),
        FrameKind::OrderedList => BlockFrame::encode_ol_body(old_spec),
        FrameKind::CodeFence => {
            let fc = if old_spec & 0x40 != 0 { b'~' } else { b'`' };
            let len = old_spec & 0x3F;
            BlockFrame::encode_cf_body(fc, len)
        }
        FrameKind::BlockQuote => 0, // BQ body 恒 0（无 para_open 位）
        FrameKind::HtmlBlock => BlockFrame::encode_html_body(old_spec),
        _ => old_spec, // Table, Comment: raw body
    };
    BlockFrame::new(kind, threshold, body)
}

#[test]
fn test_stack_empty_normal() {
    // No stack → normal LUT dispatch
    let (t, a) = scan_block(b"# Title", &[], None);
    assert_eq!(t, BlockType::Header1);
    assert!(matches!(a, ScanAction::Continue));

    let (t, a) = scan_block(b"- item", &[], None);
    assert_eq!(t, BlockType::Bullet);
    assert!(matches!(a, ScanAction::Push(_)));

    let (t, a) = scan_block(b"> quote", &[], None);
    assert_eq!(t, BlockType::Blockquote);
    assert!(matches!(a, ScanAction::Push(_)));
}

#[test]
fn test_stack_code_fence_continuation() {
    let stack = vec![frame(FrameKind::CodeFence, 0, 3)]; // 3 backticks, bit6=0
    // Content inside fence
    let (t, a) = scan_block(b"some code", &stack, None);
    assert_eq!(t, BlockType::CodeContent);
    assert!(matches!(a, ScanAction::Continue));
    // Closing fence
    let (t, a) = scan_block(b"```", &stack, None);
    assert_eq!(t, BlockType::CodeFenceClose);
    assert!(matches!(a, ScanAction::Pop));
    // Tilde fence with backtick frame shouldn't close
    let (t, a) = scan_block(b"~~~", &stack, None);
    assert_eq!(t, BlockType::CodeContent);
}

#[test]
fn test_stack_comment_continuation() {
    let stack = vec![frame(FrameKind::Comment, 0, 0)];
    let (t, a) = scan_block(b"still in comment", &stack, None);
    assert_eq!(t, BlockType::Comment);
    assert!(matches!(a, ScanAction::Continue));
    // Closing -->
    let (t, a) = scan_block(b"end of comment -->", &stack, None);
    assert_eq!(t, BlockType::Comment);
    assert!(matches!(a, ScanAction::Pop));
}

#[test]
fn test_stack_bq_continuation() {
    // BQ with para_open = true
    let stack = vec![frame(FrameKind::BlockQuote, 0, BQ_PARA_OPEN)];
    // Another > line — M1: BQ continuation returns Paragraph outer_type
    let (t, a) = scan_block(b"> more quote", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Continue));
    // Lazy continuation (no >, para_open)——生产语义 prev=前一行 PL（`> foo\nlazy`）
    let (t, a) = scan_block(b"lazy continuation", &stack, Some(BlockType::Blockquote));
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Continue));
    // BQ with para_open = false (paragraph finalized)
    let stack2 = vec![frame(FrameKind::BlockQuote, 0, 0x00)];
    let (t, a) = scan_block(b"should break out", &stack2, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
}

#[test]
fn test_stack_blt_continuation() {
    // Bullet list with threshold = 2
    let stack = vec![frame(FrameKind::BulletList, 2, b'-')];
    // Indented content inside bullet: indent >= threshold
    let (t, a) = scan_block(b"  continued", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Continue));
    // Indent < threshold → Pop
    let (t, a) = scan_block(b"outside list", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
    // New bullet at same indent → PopPush
    let (t, a) = scan_block(b"- new item", &stack, None);
    assert_eq!(t, BlockType::Bullet);
    assert!(matches!(a, ScanAction::PopPush(_)));
}

#[test]
fn test_stack_ol_continuation() {
    let stack = vec![frame(FrameKind::OrderedList, 3, b'.')];
    // Indented content inside ordered list
    let (t, a) = scan_block(b"   continued OL", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Continue));
    // Below threshold
    let (t, a) = scan_block(b"outside", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
}

#[test]
fn test_stack_table_continuation() {
    // Confirmed table (body bit0=1, header_cols=2) — data row pushes TableRow
    let body = BlockFrame::encode_table_body(2) | 1;
    let stack = vec![frame(FrameKind::Table, 0, body)];
    let (t, a) = scan_block(b"| a | b |", &stack, None);
    assert_eq!(t, BlockType::TableRow);
    assert!(matches!(a, ScanAction::Push(_)));
    // Separator in confirmed table → Pass (ends table)
    let (t, a) = scan_block(b"|---|---|", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
    // Non-table line → Pop
    let (t, a) = scan_block(b"outside table", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));

    // Tentative table (body bit0=0), data row → Pass (abandon)
    let stack = vec![frame(FrameKind::Table, 0, BlockFrame::encode_table_body(2))];
    let (t, a) = scan_block(b"| a | b |", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
    // Tentative table, matching separator → confirm
    let (t, a) = scan_block(b"|---|---|", &stack, None);
    assert_eq!(t, BlockType::TableSeparator);
    assert!(matches!(a, ScanAction::ContinueAs(_)));
}

#[test]
fn test_stack_blank_handling() {
    // Blank in code fence → CodeContent
    let stack = vec![frame(FrameKind::CodeFence, 0, 3)]; // 3 backticks, bit6=0
    let (t, a) = scan_block(b"   ", &stack, None);
    assert_eq!(t, BlockType::CodeContent);
    assert!(matches!(a, ScanAction::Continue));
    // Blank in BLT → Continue (keeps list alive for tight/loose)
    let stack = vec![frame(FrameKind::BulletList, 2, b'-')];
    let (t, a) = scan_block(b"   ", &stack, None);
    assert_eq!(t, BlockType::Blank);
    assert!(matches!(a, ScanAction::Continue));
    // Blank with empty stack → Continue
    let (t, a) = scan_block(b"   ", &[], None);
    assert_eq!(t, BlockType::Blank);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_indented_code_at_document_level() {
    // indent >= 4 with empty stack → IndentedCode
    let (t, a) = scan_block(b"    code", &[], None);
    assert_eq!(t, BlockType::IndentedCode);
    assert!(matches!(a, ScanAction::Continue));
    // indent >= 4 with BQ stack (para_open) → lazy continuation stays in BQ
    let stack = vec![frame(FrameKind::BlockQuote, 0, BQ_PARA_OPEN)];
    // 生产语义：prev 是前一行 PL（`> foo\n    indented in bq`——prev=Blockquote）；
    // prev=None 只出现在空行后（BQ lazy 拒绝——边界 4）
    let (t, a) = scan_block(b"    indented in bq", &stack, Some(BlockType::Blockquote));
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Continue),
        "indent >= 4 with para_open should Continue (lazy continuation), got {:?}", a);
}

#[test]
fn test_nested_bullet_detection() {
    // indent 4 > frame.indent(2) → same-kind nesting → Push (nested), not PopPush (sibling)
    let stack = vec![frame(FrameKind::BulletList, 2, b'-')];
    let (t, a) = scan_block(b"    - nested", &stack, None);
    assert_eq!(t, BlockType::Bullet);
    assert!(matches!(a, ScanAction::Push(_)), "nested same-kind list should Push");
}

#[test]
fn test_tilde_fence() {
    let stack = vec![frame(FrameKind::CodeFence, 0, 0x43)]; // 0x40|3 = 3 tildes, bit6=1
    let (t, a) = scan_block(b"code", &stack, None);
    assert_eq!(t, BlockType::CodeContent);
    let (t, a) = scan_block(b"~~~", &stack, None);
    assert_eq!(t, BlockType::CodeFenceClose);
    assert!(matches!(a, ScanAction::Pop));
    // Backtick fence with tilde frame shouldn't close
    let (t, a) = scan_block(b"```", &stack, None);
    assert_eq!(t, BlockType::CodeContent);
}

#[test]
fn test_html_block_continuation() {
    // Type 2 (<!-- -->) HTML block
    let stack = vec![frame(FrameKind::HtmlBlock, 0, 2)]; // spec=2 = block_type
    let (t, a) = scan_block(b"still in html", &stack, None);
    assert_eq!(t, BlockType::HtmlContent);
    assert!(matches!(a, ScanAction::Continue));
    // End marker
    let (t, a) = scan_block(b"ends here -->", &stack, None);
    assert_eq!(t, BlockType::HtmlContent);
    assert!(matches!(a, ScanAction::Pop));
}

#[test]
fn test_html_block_type1_end() {
    // Type 1 (script/pre/style) ends with </
    let stack = vec![frame(FrameKind::HtmlBlock, 0, 1)];
    let (t, a) = scan_block(b"  </script>", &stack, None);
    assert_eq!(t, BlockType::HtmlContent);
    assert!(matches!(a, ScanAction::Pop));
}

#[test]
fn test_comment_single_line() {
    // Comment that opens and closes on the same line: no frame pushed
    let (t, a) = scan_block(b"<!-- brief -->", &[], None);
    assert_eq!(t, BlockType::Comment);
    assert!(matches!(a, ScanAction::Continue)); // not Push — single-line
}

#[test]
fn test_comment_multi_line() {
    // Comment that doesn't close on the same line: frame pushed
    let (t, a) = scan_block(b"<!-- long", &[], None);
    assert_eq!(t, BlockType::Comment);
    assert!(matches!(a, ScanAction::Push(_)));

    // Comment body: any line inside <!-- ... -->
    let stack = vec![frame(FrameKind::Comment, 0, 0)];
    let (t, a) = scan_block(b"  still comment", &stack, None);
    assert_eq!(t, BlockType::Comment);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_ordered_list_new_item() {
    let stack = vec![frame(FrameKind::OrderedList, 3, b'.')];
    // New item at same indent → PopPush
    let (t, a) = scan_block(b"2. second item", &stack, None);
    assert_eq!(t, BlockType::OrderedListItem);
    assert!(matches!(a, ScanAction::PopPush(_)));
}

#[test]
fn test_list_item_with_checklist() {
    let stack = vec![frame(FrameKind::BulletList, 2, b'-')];
    let (t, a) = scan_block(b"- [x] done", &stack, None);
    assert_eq!(t, BlockType::Checklist);
    assert!(matches!(a, ScanAction::PopPush(_)));
}

#[test]
fn test_thematic_break_inside_list() {
    // Thematic break at indent 0 in a list → Pop
    let stack = vec![frame(FrameKind::BulletList, 2, b'-')];
    let (t, a) = scan_block(b"---", &stack, None);
    assert_eq!(t, BlockType::Paragraph); // indent 0 < threshold 2 → Pop
    assert!(matches!(a, ScanAction::Pop));
}

#[test]
fn test_header_inside_container() {
    // # inside a BLT: indent 0 < threshold → Pop, then caller retries at doc level
    let stack = vec![frame(FrameKind::BulletList, 2, b'-')];
    let (t, a) = scan_block(b"# Breaking header", &stack, None);
    assert_eq!(t, BlockType::Paragraph); // classify: indent=0 < threshold → Pop
    assert!(matches!(a, ScanAction::Pop));
    // After Pop, caller retries with empty stack:
    let (t, a) = scan_block(b"# Breaking header", &[], None);
    assert_eq!(t, BlockType::Header1);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_multi_pop_scenario() {
    // Stack: BQ(para_open=false) → BLT
    let stack = vec![
        frame(FrameKind::BlockQuote, 0, 0x00),
        frame(FrameKind::BulletList, 2, b'-'),
    ];
    // Line outside both: indent=0 < BLT threshold → Pop BLT
    let (t, a) = scan_block(b"outside", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
    // After popping BLT, retry with just BQ:
    let stack1 = vec![frame(FrameKind::BlockQuote, 0, 0x00)];
    let (t, a) = scan_block(b"outside", &stack1, None);
    // para_open=false, so non->, non-blank → Pop BQ
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
    // After both pops, at document level:
    let (t, a) = scan_block(b"outside", &[], None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_open_code_fence_no_stack() {
    let (t, a) = scan_block(b"```rust", &[], None);
    assert_eq!(t, BlockType::CodeFenceOpen);
    let frame = match a {
        ScanAction::Push(f) => f,
        _ => panic!("expected Push"),
    };
    assert_eq!(frame.kind(), FrameKind::CodeFence);
    assert_eq!(frame.fence_char(), b'`');
    assert_eq!(frame.fence_len(), 3);
}

#[test]
fn test_open_blockquote_no_stack() {
    let (t, a) = scan_block(b"> quote", &[], None);
    assert_eq!(t, BlockType::Blockquote);
    let frame = match a {
        ScanAction::Push(f) => f,
        _ => panic!("expected Push"),
    };
    assert_eq!(frame.kind(), FrameKind::BlockQuote);
}

#[test]
fn test_html_block_type6_blank_ends() {
    // Type 6 HTML blocks end on blank line——空行 Pop 的 t=Blank(官方例
    // 2850:结束空行不是块内容——end_for_rule 的 Specific(HtmlContent)
    // 不再含入块)。
    let stack = vec![frame(FrameKind::HtmlBlock, 0, 6)];
    let (t, a) = scan_block(b"   ", &stack, None);
    assert_eq!(t, BlockType::Blank);
    assert!(matches!(a, ScanAction::Pop));
}

#[test]
fn test_html_block_type1_blank_continues() {
    // Type 1 HTML blocks (script/pre/style) continue through blank lines
    let stack = vec![frame(FrameKind::HtmlBlock, 0, 1)];
    let (t, a) = scan_block(b"   ", &stack, None);
    assert_eq!(t, BlockType::HtmlContent);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_commonmark_smoke() {
    let path = find_cm();
    let text = std::fs::read_to_string(&path).unwrap();
    let mut stack: Vec<BlockFrame> = Vec::new();
    let mut types: Vec<BlockType> = Vec::new();
    for line in text.lines() {
        loop {
            let (t, a) = scan_block(line.as_bytes(), &stack, None);
            types.push(t);
            match a {
                ScanAction::Pop => { stack.pop(); continue; }
                ScanAction::Push(f) => { stack.push(f); break; }
                ScanAction::PopPush(f) => { stack.pop(); stack.push(f); break; }
                _ => break,
            }
        }
    }

    assert!(types.len() > 9000);
}
fn find_cm() -> String {
    for p in &["../commonmark.md", "../../commonmark.md", "../../../commonmark.md",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../commonmark.md")] {
        if std::path::Path::new(p).exists() { return p.to_string(); }
    }
    panic!("commonmark.md not found");
}

// New type implementation tests

#[test]
fn test_indented_code_from_stack_scanner() {
    let (t, a) = scan_block(b"    code", &[], None);
    assert_eq!(t, BlockType::IndentedCode);
    assert!(matches!(a, ScanAction::Continue));
    // Tab-based indent
    let (t, _) = scan_block(b"\tcode", &[], None);
    assert_eq!(t, BlockType::IndentedCode);
    // 3 spaces → Paragraph
    let (t, _) = scan_block(b"   text", &[], None);
    assert_eq!(t, BlockType::Paragraph);
}

// ── is_structural guards on container continuations ──

#[test]
fn test_indent_4_fence_does_not_close() {
    let bt = BlockFrame::new(FrameKind::CodeFence, 0, BlockFrame::encode_cf_body(b'`', 3));
    // 4-space-indented ``` inside a code fence must not close it
    let (t, a) = scan_block(b"    ```", &[bt], None);
    assert_eq!(t, BlockType::CodeContent);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_indent_4_heading_in_bq_is_lazy_continuation() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    // 4-space-indented # heading inside BQ with open paragraph (prev 词法——
    // 段落打开由 prev 判定) must be lazy continuation
    let (t, _a) = scan_block(b"    # heading", &[bq], Some(BlockType::Paragraph));
    assert_eq!(t, BlockType::Paragraph);
    // BQ formal fix: lazy continuation no longer uses Continue action
}

// ── NestedResolver 统一 admit 窗口 ──

#[test]
fn test_same_line_list_nested_window() {
    // n=4：内层 marker 成立 → Composite，两个 list 帧。
    let (t, a) = scan_block(b"-    - x", &[], None);
    assert_eq!(t, BlockType::Bullet);
    let ScanAction::Composite { nested, .. } = a else {
        panic!("expected Composite, got {a:?}");
    };
    assert_eq!(nested.len(), 1, "nested frames: {nested:?}");
    assert_eq!(nested[0].kind(), FrameKind::BulletList);

    // n=5..8：item 首内容 = IC，不产生嵌套帧。
    for n in 5..=8 {
        let mut bytes = Vec::from(&b"-"[..]);
        bytes.extend(std::iter::repeat_n(b' ', n));
        bytes.extend_from_slice(b"- x");
        let (t, a) = scan_block(&bytes, &[], None);
        assert_eq!(t, BlockType::IndentedCode, "n={n} should be first-block IC");
        assert!(matches!(a, ScanAction::Push(_)), "n={n}: {a:?}");
    }
}

#[test]
fn test_same_line_ol_nested_window() {
    // `1.` 后总 4 空格 → 嵌套成立；总 5 空格 → IC。
    let (t, a) = scan_block(b"1.    - x", &[], None);
    assert_eq!(t, BlockType::Bullet);
    assert!(matches!(a, ScanAction::Composite { .. }), "{a:?}");
    let (t, a) = scan_block(b"1.     - x", &[], None);
    assert_eq!(t, BlockType::IndentedCode);
    assert!(matches!(a, ScanAction::Push(_)), "{a:?}");
}

#[test]
fn test_bq_continuation_residual_indent_heading() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    // `> # H` 之后 `>   # H2`：剥 BQ 前缀后 residual=2，必须是标题而非段落。
    let (t, a) = scan_block(b">   # H2", &[bq], Some(BlockType::Header1));
    assert_eq!(t, BlockType::Header1, "BQ 续行标题被降级");
    assert!(matches!(a, ScanAction::Continue), "{a:?}");
}

#[test]
fn test_empty_item_first_content_window() {
    // `-` 真空 item：5 空格内容是文本，6 空格是 IC（cmark oracle）。
    let item = BlockFrame::new(FrameKind::BulletList, 63, 0);
    let (t, _a) = scan_block(b"     baz", &[item], Some(BlockType::Bullet));
    assert_eq!(t, BlockType::Paragraph, "5 空格应并入 item 文本");
    let (t, _a) = scan_block(b"      baz", &[item], Some(BlockType::Bullet));
    assert_eq!(t, BlockType::IndentedCode, "6 空格应是 IC");
}

#[test]
fn test_bq_continuation_nested_bq_no_pop() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    // residual=3 的嵌套 `>` 必须被当前 BQ 认领，不能 Pop 后重开。
    let (t, a) = scan_block(b">    > x", &[bq], Some(BlockType::Header1));
    assert!(matches!(a, ScanAction::Composite { .. }), "expected Composite, got {a:?}");
    let ScanAction::Composite { nested, .. } = a else { unreachable!() };
    assert_eq!(nested.len(), 1, "nested frames: {nested:?}");
    assert_eq!(nested[0].kind(), FrameKind::BlockQuote);
    assert_ne!(t, BlockType::Blockquote, "BQ 续行嵌套的 PL 应为内容层语义");
}

#[test]
fn test_indent_4_pipe_does_not_continue_table() {
    let tbl = BlockFrame::new(FrameKind::Table, 0, BlockFrame::encode_spare_body());
    // 4-space-indented | inside a table must end the table
    let (t, a) = scan_block(b"    | cell", &[tbl], None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
}

#[test]
fn test_cross_kind_nesting_ol_inside_blt() {
    // OL inside BLT — BQ formal fix: cross-kind nesting uses PopPush
    let stack = vec![frame(FrameKind::BulletList, 2, b'-')];
    let (t, _a) = scan_block(b"  1. ordered inside", &stack, None);
    assert_eq!(t, BlockType::OrderedListItem);
}

#[test]
fn test_cross_kind_nesting_blt_inside_ol() {
    // BLT inside OL — BQ formal fix: cross-kind nesting uses PopPush
    let stack = vec![frame(FrameKind::OrderedList, 3, b'.')];
    let (t, _a) = scan_block(b"   - bullet inside", &stack, None);
    assert_eq!(t, BlockType::Bullet);
}

#[test]
fn test_same_kind_still_poppush() {
    // Same-kind item still PopPush
    let stack = vec![frame(FrameKind::BulletList, 2, b'-')];
    let (t, a) = scan_block(b"- sibling", &stack, None);
    assert_eq!(t, BlockType::Bullet);
    assert!(matches!(a, ScanAction::PopPush(_)));
}

#[test]
fn test_html_block_detection_div() {
    let (t, _) = scan_block(b"<div>", &[], None);
    assert_eq!(t, BlockType::HtmlBlock);
}

#[test]
fn test_html_block_detection_script() {
    let (t, _) = scan_block(b"<script>", &[], None);
    assert_eq!(t, BlockType::HtmlBlock);
}

#[test]
fn test_html_block_detection_pre() {
    let (t, _) = scan_block(b"<pre>", &[], None);
    assert_eq!(t, BlockType::HtmlBlock);
}

#[test]
fn test_comment_not_html_block() {
    // <!-- should remain Comment, not become HtmlBlock
    let (t, _) = scan_block(b"<!-- comment -->", &[], None);
    assert_eq!(t, BlockType::Comment);
}

#[test]
fn test_html_block_detection_then_push_frame() {
    // Stack scanner: <div> should Push HtmlBlock frame
    let (t, a) = scan_block(b"<div>", &[], None);
    assert_eq!(t, BlockType::HtmlBlock);
    match a {
        ScanAction::Push(f) => {
            assert_eq!(f.kind(), FrameKind::HtmlBlock);
            assert!(f.block_type() > 0 && f.block_type() <= 7, "block_type should be 1-7, got {}", f.block_type());
        }
        _ => panic!("expected Push, got {:?}", a),
    }
}

#[test]
fn test_html_type7_no_interrupt_paragraph() {
    // Type 7 (<span>) after paragraph → Paragraph (does NOT interrupt)
    let prev = Some(BlockType::Paragraph);
    let (t, a) = scan_block(b"<span>", &[], prev);
    assert_eq!(t, BlockType::Paragraph,
        "type 7 HTML should NOT interrupt paragraph, got {:?}", t);
    assert!(matches!(a, ScanAction::Continue),
        "expected Continue, got {:?}", a);

    // Type 6 (<div>) after paragraph → HtmlBlock (DOES interrupt)
    let (t, a) = scan_block(b"<div>", &[], prev);
    assert_eq!(t, BlockType::HtmlBlock,
        "type 6 HTML should interrupt paragraph, got {:?}", t);
    assert!(matches!(a, ScanAction::Push(_)),
        "expected Push, got {:?}", a);

    // Type 7 without paragraph context → normal HtmlBlock Push
    let (t, a) = scan_block(b"<span>", &[], None);
    assert_eq!(t, BlockType::HtmlBlock,
        "type 7 without paragraph should be HtmlBlock, got {:?}", t);
    assert!(matches!(a, ScanAction::Push(_)),
        "expected Push, got {:?}", a);
}

#[test]
fn test_html_content_inside_block() {
    let stack = vec![frame(FrameKind::HtmlBlock, 0, 6)]; // type 6
    let (t, a) = scan_block(b"some content", &stack, None);
    assert_eq!(t, BlockType::HtmlContent);
    assert!(matches!(a, ScanAction::Continue));
}

// Hs (setext) heading tests

#[test]
fn test_hs_heading_equals_after_paragraph() {
    // "Title\n===\n" — === after Paragraph → HsSep (M3: no RewritePrev)
    let (t, a) = scan_block(b"===", &[], Some(BlockType::Paragraph));
    assert_eq!(t, BlockType::HsSep);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_hs_heading_hyphen_after_paragraph() {
    // "Title\n---\n" — --- after Paragraph → HsSep (M3: no RewritePrev)
    let (t, a) = scan_block(b"---", &[], Some(BlockType::Paragraph));
    assert_eq!(t, BlockType::HsSep);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_hs_single_hyphen_after_paragraph() {
    // "Title\n-\n" — single - after Paragraph → HsSep(M3: no RewritePrev——
    // 仓库规范原文"The underlining can be any length",单 - 也是下划线)
    let (t, a) = scan_block(b"-", &[], Some(BlockType::Paragraph));
    assert_eq!(t, BlockType::HsSep);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_hs_hyphen_not_after_paragraph() {
    // --- without preceding Paragraph → ThematicBreak
    let (t, a) = scan_block(b"---", &[], None);
    assert_eq!(t, BlockType::ThematicBreak);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_hs_equals_not_after_paragraph() {
    // === without preceding Paragraph → Paragraph (not a setext candidate)
    let (t, a) = scan_block(b"===", &[], None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_hs_after_blank_is_thematic_break() {
    // "Title\n\n---" — blank between paragraph and underline → ThematicBreak
    let (t, a) = scan_block(b"---", &[], Some(BlockType::Blank));
    // prev_line_type is Blank → no match → ThematicBreak
    assert_eq!(t, BlockType::ThematicBreak);
}

#[test]
fn test_hs_with_trailing_spaces() {
    // "===\t  " — equals with trailing whitespace → still HsSep (M3: no RewritePrev)
    let (t, a) = scan_block(b"=== \t", &[], Some(BlockType::Paragraph));
    assert_eq!(t, BlockType::HsSep);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_hs_mixed_content_not_setext() {
    // "=== text" — equals with non-whitespace content → Paragraph
    let (t, a) = scan_block(b"=== text", &[], Some(BlockType::Paragraph));
    assert_eq!(t, BlockType::Paragraph);
}

#[test]
fn test_hs_underline_inside_bq_is_lazy_continuation() {
    // ">\n===\n" — === inside BQ → lazy continuation, not setext
    let stack = vec![frame(FrameKind::BlockQuote, 0, BQ_PARA_OPEN)]; // para_open=true
    let (t, a) = scan_block(b"===", &stack, None);
    // BQ check returns first, classification handled by check_bq_continuation
    assert_eq!(t, BlockType::Paragraph);
}

// Scenario tests — simulate rebuild_all with multi-pop loop

/// Run a sequence through scan_block with the multi-pop loop,
/// returning the final BlockType for each line.
fn simulate_scan(lines: &[&[u8]]) -> Vec<BlockType> {
    let mut stack: Vec<BlockFrame> = Vec::new();
    let mut types = Vec::new();
    for &line in lines {
        let mut final_type = None;
        loop {
            let (t, a) = scan_block(line, &stack, None);
            final_type = Some(t);
            match a {
                ScanAction::Pop => { stack.pop(); continue; }
                ScanAction::Push(f) => { stack.push(f); break; }
                ScanAction::PopPush(f) => { stack.pop(); stack.push(f); break; }
                ScanAction::ContinueAs(f) => {
                    if let Some(top) = stack.last_mut() { *top = f; }
                    break;
                }
                ScanAction::Composite { primary, nested, .. } => {
                    match primary {
                        CompositePrimary::Push(f) => { stack.push(f); }
                        CompositePrimary::Pop => { stack.pop(); }
                        CompositePrimary::PopPush(f) => { stack.pop(); stack.push(f); }
                        CompositePrimary::Continue | CompositePrimary::ContinueAs(_) => {}
                    }
                    for nf in nested { stack.push(nf); }
                    break;
                }
                _ => break,
            }
        }
        types.push(final_type.unwrap());
    }
    types
}
fn final_depth(lines: &[&[u8]]) -> usize {
    let mut stack: Vec<BlockFrame> = Vec::new();
    for &line in lines {
        loop {
            let (_, a) = scan_block(line, &stack, None);
            match a {
                ScanAction::Pop => { stack.pop(); continue; }
                ScanAction::Push(f) => { stack.push(f); break; }
                ScanAction::PopPush(f) => { stack.pop(); stack.push(f); break; }
                _ => break,
            }
        }
    }
    stack.len()
}

#[test]
fn test_scenario_list_lazy_continuation() {
    let types = simulate_scan(&[b"- A", b"B"]);
    assert_eq!(types[0], BlockType::Bullet);
    assert_eq!(types[1], BlockType::Paragraph);
    assert_eq!(final_depth(&[b"- A", b"B"]), 0);
}

#[test]
fn test_scenario_list_indented_continuation() {
    let types = simulate_scan(&[b"- A", b"   B", b"-  C", b" D"]);
    assert_eq!(types[0], BlockType::Bullet);
    assert_eq!(types[1], BlockType::Paragraph);
    assert_eq!(types[2], BlockType::Bullet);
    assert_eq!(types[3], BlockType::Paragraph);
    assert_eq!(final_depth(&[b"- A", b"   B", b"-  C", b" D"]), 0);
}

#[test]
fn test_scenario_fence_inside_bq() {
    // Composite: Push(BQ) + nested Push(CF) → outer_type is Blockquote
    let types = simulate_scan(&[b"> ```", b"paragraph"]);
    assert_eq!(types[0], BlockType::Blockquote);
    // ⑥ 复核（符合性比对——官方例 238 + cmark 反证）：
    // BQ 内 fence 的无前缀行被约束链拒（弹 CF——弹 BQ——顶层 Paragraph——
    // 官方 `<p>foo</p>`——CF 豁免方向错——已回滚——恢复
    // "Bug A 语义"（无前缀行不属于 BQ 内 CF——顶层））
    assert_eq!(types[1], BlockType::Paragraph);
}

#[test]
fn test_scenario_blank_separates_paragraphs() {
    let types = simulate_scan(&[b"A", b"", b"B"]);
    assert_eq!(types[0], BlockType::Paragraph);
    assert_eq!(types[1], BlockType::Blank);
    assert_eq!(types[2], BlockType::Paragraph);
}

#[test]
fn test_scenario_mixed_fence_chars() {
    let types = simulate_scan(&[b"```", b"code", b"~~~", b"more", b"```"]);
    assert_eq!(types[0], BlockType::CodeFenceOpen);
    assert_eq!(types[1], BlockType::CodeContent);
    assert_eq!(types[2], BlockType::CodeContent);
    assert_eq!(types[3], BlockType::CodeContent);
    assert_eq!(types[4], BlockType::CodeFenceOpen);
    assert_eq!(final_depth(&[b"```", b"code", b"~~~", b"more", b"```"]), 1);
}

#[test]
fn test_scenario_thematic_break_vs_setext() {
    let types = simulate_scan(&[b"Header", b"---", b"this"]);
    assert_eq!(types[0], BlockType::Paragraph);
    assert_eq!(types[1], BlockType::ThematicBreak);
    assert_eq!(types[2], BlockType::Paragraph);
}

#[test]
fn test_full_sequence_ol_with_nested_blt() {
    let types = simulate_scan(&[
        b"1. top", b"   * nested", b"   still nested", b"2. second",
    ]);
    assert_eq!(types[0], BlockType::OrderedListItem);
    assert_eq!(types[1], BlockType::Bullet);
    assert_eq!(types[2], BlockType::Paragraph);
    assert_eq!(types[3], BlockType::OrderedListItem);
}

#[test]
fn test_full_sequence_html_block() {
    let types = simulate_scan(&[
        b"<div>", b"content", b"still", b"", b"outside",
    ]);
    assert_eq!(types[0], BlockType::HtmlBlock);
    assert_eq!(types[1], BlockType::HtmlContent);
    assert_eq!(types[2], BlockType::HtmlContent);
    assert_eq!(types[3], BlockType::Blank);
    assert_eq!(types[4], BlockType::Paragraph);
}

// tight/loose tests — per-frame spec bit7 tracking

/// Simulate a sequence through the engine's multi-pop loop
/// (including bit7 set/clear on Continue), returning final stack frames.
fn simulate_scan_with_loose(lines: &[&[u8]]) -> Vec<BlockFrame> {
    let mut stack: Vec<BlockFrame> = Vec::new();
    for &line in lines {
        let mut final_type = None;
        loop {
            let (t, a) = scan_block(line, &stack, None);
            final_type = Some(t);
            match a {
                ScanAction::Pop => { stack.pop(); continue; }
                ScanAction::Push(f) => { stack.push(f); break; }
                ScanAction::PopPush(f) => { stack.pop(); stack.push(f); break; }
                ScanAction::ContinueAs(f) => {
                    if let Some(top) = stack.last_mut() { *top = f; }
                    break;
                }
                ScanAction::Continue => {
                    if let Some(top) = stack.last_mut() {
                        match top.kind() {
                            FrameKind::BulletList | FrameKind::OrderedList => {
                                if t == BlockType::Blank {
                                    *top = BlockFrame(top.0 | LOOSE_BIT);
                                } else if t == BlockType::Paragraph {
                                    *top = BlockFrame(top.0 & !LOOSE_BIT);
                                }
                            }
                            _ => {}
                        }
                    }
                    break;
                }
                _ => break,
            }
        }
    }
    stack
}

#[test]
fn test_tight_list_no_blank() {
    // - A\n- B  →  no blank between items → tight
    let stack = simulate_scan_with_loose(&[b"- A", b"- B"]);
    assert_eq!(stack.len(), 1); // BLT still on stack
    assert!(!stack[0].loose()); // tight
}

#[test]
fn test_loose_list_with_blank() {
    // - A\n\n- B  →  blank between items → loose
    let stack = simulate_scan_with_loose(&[b"- A", b"", b"- B"]);
    assert_eq!(stack.len(), 1);
    assert!(stack[0].loose()); // loose
}

#[test]
fn test_intra_item_blank_stays_tight() {
    // - A\n  cont\n\n  cont2\n- B  →  blank inside item, then more text
    let stack = simulate_scan_with_loose(&[b"- A", b"  cont", b"", b"  cont2", b"- B"]);
    assert_eq!(stack.len(), 1);
    assert!(!stack[0].loose()); // tight (blank cleared by cont2)
}

#[test]
fn test_blank_in_blt_returns_continue() {
    let stack = vec![frame(FrameKind::BulletList, 2, b'-')];
    let (t, a) = scan_block(b"   ", &stack, None);
    assert_eq!(t, BlockType::Blank);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_blank_in_oli_returns_continue() {
    let stack = vec![frame(FrameKind::OrderedList, 3, b'.')];
    let (t, a) = scan_block(b"   ", &stack, None);
    assert_eq!(t, BlockType::Blank);
    assert!(matches!(a, ScanAction::Continue));
}

// ── frame.rs tests (merged) ──

fn blt_frame(threshold: u8, bullet_char: u8) -> BlockFrame {
    BlockFrame::new(FrameKind::BulletList, threshold, BlockFrame::encode_blt_body(bullet_char))
}

fn ol_frame(threshold: u8, delim: u8) -> BlockFrame {
    BlockFrame::new(FrameKind::OrderedList, threshold, BlockFrame::encode_ol_body(delim))
}

fn cf_frame(fence_char: u8, len: u8) -> BlockFrame {
    BlockFrame::new(FrameKind::CodeFence, 0, BlockFrame::encode_cf_body(fence_char, len))
}

fn bq_frame() -> BlockFrame {
    BlockFrame::new(FrameKind::BlockQuote, 0, 0) // body 恒 0（无 para_open 位）
}

fn html_frame(block_type: u8) -> BlockFrame {
    BlockFrame::new(FrameKind::HtmlBlock, 0, BlockFrame::encode_html_body(block_type))
}

#[test]
fn test_frame_size() {
    assert_eq!(std::mem::size_of::<BlockFrame>(), 2);
}

#[test]
fn test_frame_kind_roundtrip() {
    for kind in &[
        FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::OrderedList,
        FrameKind::CodeFence, FrameKind::HtmlBlock, FrameKind::Table, FrameKind::Comment,
    ] {
        let f = BlockFrame::new(*kind, 0, 0);
        assert_eq!(f.kind(), *kind);
        // entry_is_pop is on u64 entries, not BlockFrame; Push entries have bit63=0
        let e = push_entry(0, f);
        assert!(!entry_is_pop(e));
    }
}

#[test]
fn test_frame_indent_roundtrip() {
    for indent in &[0u8, 1, 3, 7, 15, 31, 63] {
        let f = BlockFrame::new(FrameKind::BulletList, *indent, 0);
        assert_eq!(f.indent(), *indent);
    }
}

#[test]
fn test_frame_body_roundtrip() {
    for body in &[0u8, 1, 7, 15, 31, 63] {
        let f = BlockFrame::new(FrameKind::BlockQuote, 0, *body);
        assert_eq!(f.body(), *body);
    }
}

#[test]
fn test_entry_is_pop() {
    let f = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    let push = push_entry(0, f);
    assert!(!entry_is_pop(push));
    let pop = pop_entry(0, f);
    assert!(entry_is_pop(pop));
}

#[test]
fn test_blt_bullet_encoding() {
    let cases: &[(u8, u8)] = &[(b'-', 0), (b'*', 1), (b'+', 2)];
    for &(ch, expected_code) in cases {
        let body = BlockFrame::encode_blt_body(ch);
        let f = BlockFrame::new(FrameKind::BulletList, 4, body);
        assert_eq!(f.bullet_code(), expected_code);
        assert_eq!(f.kind(), FrameKind::BulletList);
        assert_eq!(f.indent(), 4);
    }
}

#[test]
fn test_ol_delim_encoding() {
    let f = BlockFrame::new(FrameKind::OrderedList, 3, BlockFrame::encode_ol_body(b'.'));
    assert_eq!(f.delim_code(), 0);
    assert_eq!(f.kind(), FrameKind::OrderedList);
    assert_eq!(f.indent(), 3);

    let f = BlockFrame::new(FrameKind::OrderedList, 5, BlockFrame::encode_ol_body(b')'));
    assert_eq!(f.delim_code(), 1);
    assert_eq!(f.indent(), 5);
}

#[test]
fn test_cf_encoding() {
    let f = cf_frame(b'`', 3);
    assert_eq!(f.fence_char(), b'`');
    assert_eq!(f.fence_len(), 3);
    assert_eq!(f.kind(), FrameKind::CodeFence);

    let f = cf_frame(b'~', 5);
    assert_eq!(f.fence_char(), b'~');
    assert_eq!(f.fence_len(), 5);

    let f = cf_frame(b'`', 34);
    assert_eq!(f.fence_len(), 34);
}

#[test]
fn test_bq_encoding() {
    // BQ body 恒 0。
    let f = bq_frame();
    assert_eq!(f.body(), 0);
    assert_eq!(f.kind(), FrameKind::BlockQuote);
}

#[test]
fn test_html_encoding() {
    for bt in 1u8..=7u8 {
        let f = html_frame(bt);
        assert_eq!(f.block_type(), bt);
        assert_eq!(f.kind(), FrameKind::HtmlBlock);
    }
}

#[test]
fn test_loose_bit() {
    let mut f = blt_frame(2, b'-');
    assert!(!f.loose());

    let f_loose = BlockFrame(f.0 | LOOSE_BIT);
    assert!(f_loose.loose());
    assert_eq!(f_loose.kind(), FrameKind::BulletList);

    let f_tight = BlockFrame(f_loose.0 & !LOOSE_BIT);
    assert!(!f_tight.loose());
}

#[test]
fn test_push_entry_roundtrip() {
    let f = blt_frame(4, b'-');
    let e = push_entry(5, f);
    assert_eq!(entry_line(e), 5);
    assert!(!entry_is_pop(e));
    assert_eq!(entry_frame(e).0, f.0);
}

#[test]
fn test_pop_entry_roundtrip() {
    let f = blt_frame(4, b'-');
    let e = pop_entry(10, f);
    assert_eq!(entry_line(e), 10);
    assert!(entry_is_pop(e));
    assert_eq!(entry_frame(e).0, f.0);
}

#[test]
fn test_empty_framelog() {
    let log = FrameLog::new();
    assert_eq!(log.prefix_len(0), 0);
    assert!(log.rebuild_stack(0).is_empty());
}

#[test]
fn test_push_pop_roundtrip() {
    let mut log = FrameLog::new();
    log.record_push(2, blt_frame(4, b'-'));
    log.record_pop(10, blt_frame(4, b'-'));

    assert!(log.rebuild_stack(2).is_empty());

    let stack = log.rebuild_stack(3);
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0].kind(), FrameKind::BulletList);
    assert_eq!(stack[0].indent(), 4);
    assert_eq!(stack[0].bullet_code(), 0);

    assert!(log.rebuild_stack(11).is_empty());
}

#[test]
fn test_prefix_len_boundary() {
    let mut log = FrameLog::new();
    log.record_push(5, bq_frame());
    log.record_pop(10, bq_frame());

    assert_eq!(log.prefix_len(4), 0);
    assert_eq!(log.prefix_len(5), 0);
    assert_eq!(log.prefix_len(6), 1);
    assert_eq!(log.prefix_len(10), 1);
    assert_eq!(log.prefix_len(11), 2);
}

#[test]
fn test_multi_level_nesting() {
    let mut log = FrameLog::new();
    log.record_push(1, bq_frame());
    log.record_push(2, blt_frame(3, b'-'));
    log.record_pop(8, blt_frame(3, b'-'));
    log.record_pop(9, bq_frame());

    let stack = log.rebuild_stack(3);
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[0].kind(), FrameKind::BlockQuote);
    assert_eq!(stack[1].kind(), FrameKind::BulletList);

    let stack = log.rebuild_stack(9);
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0].kind(), FrameKind::BlockQuote);

    assert!(log.rebuild_stack(10).is_empty());
}

#[test]
fn test_pop_on_empty_stack_does_not_panic() {
    let mut log = FrameLog::new();
    log.record_pop(0, blt_frame(0, b'-'));
    log.record_pop(1, bq_frame());
    assert!(log.rebuild_stack(2).is_empty());
}

#[test]
fn test_forward_backward_replay_roundtrip() {
    let entries = vec![
        push_entry(1, bq_frame()),
        push_entry(2, blt_frame(3, b'-')),
        pop_entry(5, blt_frame(3, b'-')),
        push_entry(6, cf_frame(b'`', 3)),
        pop_entry(9, cf_frame(b'`', 3)),
        pop_entry(10, bq_frame()),
    ];

    let mut stack = Vec::new();
    forward_replay(&entries, &mut stack);
    assert!(stack.is_empty(), "stack should be empty after full forward replay");

    let mut stack2 = Vec::new();
    forward_replay(&entries, &mut stack2);
    assert!(stack2.is_empty());

    backward_replay(&entries, &mut stack2);
    assert!(stack2.is_empty(), "already empty, backward should stay empty");
}

#[test]
fn test_replay_roundtrip_with_partial() {
    let mut log = FrameLog::new();
    log.record_push(1, bq_frame());
    log.record_push(2, blt_frame(3, b'-'));
    log.record_pop(5, blt_frame(3, b'-'));
    log.record_push(6, cf_frame(b'`', 3));

    let stack = log.rebuild_stack(7);
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[0].kind(), FrameKind::BlockQuote);
    assert_eq!(stack[1].kind(), FrameKind::CodeFence);

    let start = log.prefix_len(7);
    let mut stack2: Vec<BlockFrame> = Vec::new();
    forward_replay(&log.entries[..start], &mut stack2);
    assert_eq!(stack2.len(), 2);

    backward_replay(&log.entries[..start], &mut stack2);
    assert!(stack2.is_empty());
}

#[test]
fn test_forward_backward_with_body_mut() {
    // Push BQ, body_mut (clear para_open), Push BLT, body_mut (set LOOSE_BIT), Pop BLT, Pop BQ
    let entries = vec![
        push_entry(1, bq_frame()),
        body_mut_entry(2, bq_frame()),          // bare > cleared para_open
        push_entry(3, blt_frame(2, b'-')),
        body_mut_entry(4, BlockFrame(blt_frame(2, b'-').0 | LOOSE_BIT)), // blank set loose
        pop_entry(5, blt_frame(2, b'-')),
        pop_entry(7, bq_frame()),
    ];

    // Full forward → empty stack
    let mut stack = Vec::new();
    forward_replay(&entries, &mut stack);
    assert!(stack.is_empty());

    // Forward then backward roundtrip → empty
    let mut stack2 = Vec::new();
    forward_replay(&entries, &mut stack2);
    assert!(stack2.is_empty());
    backward_replay(&entries, &mut stack2);
    assert!(stack2.is_empty(), "roundtrip with body_mut should yield empty stack");

    // Partial: forward up to line 4 (before Pop), body_mut should be applied
    let mut stack3 = Vec::new();
    let slice = &entries[..4]; // Push BQ, body_mut, Push BLT, body_mut LOOSE
    forward_replay(slice, &mut stack3);
    assert_eq!(stack3.len(), 2);
    assert!(stack3[1].loose(), "body_mut should have set LOOSE_BIT");

    // Reverse back → empty
    backward_replay(slice, &mut stack3);
    assert!(stack3.is_empty(), "partial backward with body_mut should yield empty");
}

#[test]
fn test_rebuild_stack_at() {
    let entries = vec![
        push_entry(1, bq_frame()),
        push_entry(2, blt_frame(3, b'-')),
        pop_entry(5, blt_frame(3, b'-')),
        pop_entry(6, bq_frame()),
    ];
    let s = rebuild_stack_at(&entries, 7);
    assert!(s.is_empty());
}

#[test]
fn test_poppush_two_entries() {
    let mut log = FrameLog::new();
    let old_frame = blt_frame(2, b'-');
    let new_frame = blt_frame(2, b'*');
    log.record_push(2, old_frame);
    log.record_pop(3, old_frame);
    log.record_push(3, new_frame);

    assert!(log.rebuild_stack(2).is_empty());
    let stack = log.rebuild_stack(3);
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0].bullet_code(), 0);

    let stack = log.rebuild_stack(4);
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0].bullet_code(), 1);
}

#[test]
fn test_boundary_for_basic() {
    let mut log = FrameLog::new();
    log.record_push(2, blt_frame(4, b'-'));
    log.record_pop(10, blt_frame(4, b'-'));

    assert_eq!(log.item_tail_boundary(2, FrameKind::BulletList, &[]), Some(10));
    assert_eq!(log.item_tail_boundary(5, FrameKind::BulletList, &[]), Some(10));
}

#[test]
fn test_boundary_for_unclosed() {
    let mut log = FrameLog::new();
    log.record_push(2, bq_frame());
    assert_eq!(log.item_tail_boundary(2, FrameKind::BlockQuote, &[]), None);
}

#[test]
fn test_boundary_for_poppush() {
    let mut log = FrameLog::new();
    let f1 = blt_frame(2, b'-');
    let f2 = blt_frame(2, b'*');
    log.record_push(2, f1);
    log.record_pop(5, f1);
    log.record_push(5, f2);
    log.record_pop(10, f2);

    // PopPush at line 5: Pop closes old before L=5, end=5
    assert_eq!(log.item_tail_boundary(2, FrameKind::BulletList, &[]), Some(5));
}

#[test]
fn test_boundary_for_nested() {
    let mut log = FrameLog::new();
    log.record_push(1, bq_frame());
    log.record_push(2, blt_frame(3, b'-'));
    log.record_pop(5, blt_frame(3, b'-'));
    log.record_pop(6, bq_frame());

    assert_eq!(log.item_tail_boundary(2, FrameKind::BulletList, &[]), Some(5));
    assert_eq!(log.item_tail_boundary(1, FrameKind::BlockQuote, &[]), Some(6));
}

#[test]
fn test_body_mut_entry_replay() {
    // BQ 无 body_mut（无 para_open 位）——用 BLT LOOSE 验证
    // body_mut 重放机制（通用——帧更新）
    let loose_on = |base: BlockFrame| BlockFrame::new(base.kind(), base.indent(), base.body() | 0x08);
    let entries = &[
        push_entry(0, blt_frame(2, b'-')),
        body_mut_entry(1, loose_on(blt_frame(2, b'-'))), // LOOSE 置位
        pop_entry(3, blt_frame(2, b'-')),
    ];
    // Rebuild stack at line 2 (after body_mut, before Pop)
    let stack = FrameLog::new().rebuild_stack_from(entries, 2);
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0].kind(), FrameKind::BulletList);
    assert!(stack[0].loose(), "body_mut should have set LOOSE_BIT");

    // Rebuild stack at line 0 (before body_mut)
    let stack0 = FrameLog::new().rebuild_stack_from(entries, 0);
    assert!(stack0.is_empty());

    // Rebuild stack at line 1 (at body_mut — prefix_len excludes it)
    let stack1 = FrameLog::new().rebuild_stack_from(entries, 1);
    assert_eq!(stack1.len(), 1);
    assert!(!stack1[0].loose(), "body_mut at line 1 not yet replayed");

    // boundary_for should skip body_mut entries
    let mut log = FrameLog::new();
    log.record_push(0, bq_frame());
    log.entries.push(body_mut_entry(1, bq_frame()));
    log.record_push(2, blt_frame(2, b'-'));
    log.record_pop(5, blt_frame(2, b'-'));
    log.record_pop(8, bq_frame());
    assert_eq!(log.item_tail_boundary(0, FrameKind::BlockQuote, &[]), Some(8)); // BQ uses Pop line (not +1)
}

#[test]
fn test_indented_code_no_interrupt_paragraph() {
    // CommonMark §4.4: indented code cannot interrupt a paragraph
    let prev = Some(BlockType::Paragraph);
    let (t, a) = scan_block(b"    code", &[], prev);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Continue));
    // Without paragraph, indent >= 4 IS indented code
    let (t, a) = scan_block(b"    code", &[], None);
    assert_eq!(t, BlockType::IndentedCode);
    assert!(matches!(a, ScanAction::Continue));
}

#[test]
fn test_bq_thematic_break_pops_bq() {
    // CommonMark §4.3 + §5.1: thematic break without > prefix closes the BQ
    let stack = vec![bq_frame()]; // BQ with para_open
    let (t, a) = scan_block(b"---", &stack, None);
    // Should Pop — close BQ, then rescan as ThematicBreak at document level
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
    // Heading also closes the BQ
    let (t, a) = scan_block(b"# heading", &stack, None);
    assert_eq!(t, BlockType::Paragraph);
    assert!(matches!(a, ScanAction::Pop));
}

// ── HTML block diagnostics ──

#[test]
fn diag_html_block_all() {
    use crate::projection::{project_text, Snapshot};
    // A: Type 6 div with emphasis - should be raw text

    let snap = project_text("<div>\n*hello*\n</div>\n");
    for li in 0..snap.line_len() {

    }
    let ast_str = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);


    // B: Table with blank line inside — boundary test

    let md = "<table><tr><td>\n<pre>\n**Hello**,\n\n_world_.\n</pre>\n</td></tr></table>\n";
    let snap = project_text(md);
    for li in 0..snap.line_len() {
        let t = snap.line_type_at(li);
        let txt = crate::projection::Snapshot::with_line_str(snap.line_text(li), |s| s.chars().take(30).collect::<String>());

    }
    let ast_str = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);

    let ast_nested = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);


    // C: Type 1 script — should have raw text content

    let snap = project_text("<script>\nvar x = 1;\n</script>\n");
    for li in 0..snap.line_len() {

    }
    let ast_str = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);

}

// ── List xfail diagnostics ──

#[test]
fn diag_list_12_indent_code() {
    use crate::projection::project_text;
    // test_lists_12: CM spec ex 12 — blank lines, indented code (5-space indent)
    let md = "1.  a\n\n  2.  b\n\n    3.  c\n";

    let snap = project_text(md);
    for li in 0..snap.line_len() {
        let t = snap.line_type_at(li);
        let txt = crate::projection::Snapshot::with_line_str(snap.line_text(li), |s| s.chars().take(30).collect::<String>());

    }
    let ast_str = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);

    let ast_nested = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);

}

#[test]
fn diag_list_19_bq_in_item() {
    use crate::projection::project_text;
    // test_lists_19: CM spec ex 19 — 2-space indent before >
    let md = "* a\n  > b\n  >\n* c\n";

    let snap = project_text(md);
    for li in 0..snap.line_len() {
        let t = snap.line_type_at(li);
        let txt = crate::projection::Snapshot::with_line_str(snap.line_text(li), |s| s.chars().take(30).collect::<String>());

    }
    let ast_str = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);

    let ast_nested = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);

}

#[test]
fn diag_list_20_bq_code_in_item() {
    use crate::projection::project_text;
    // test_lists_20: CM spec ex 20 — 2-space indent for BQ/code inside item
    let md = "- a\n  > b\n  ```\n  c\n  ```\n- d\n";

    let snap = project_text(md);
    for li in 0..snap.line_len() {
        let t = snap.line_type_at(li);
        let txt = crate::projection::Snapshot::with_line_str(snap.line_text(li), |s| s.chars().take(30).collect::<String>());

    }
    let ast_str = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);

    let ast_nested = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);

}


// ── gauntlet 回归：空 item 不中断段落（CM §5.2）─ tab 构造内容非空 ──
#[test]
fn gauntlet_empty_item_no_interrupt() {
    use crate::block::scan_block;
    // 空 item（`*`）不能中断段落（CM §5.2——`foo\n*` 是段落）
    let (t1, a1) = scan_block(b"*", &[], Some(BlockType::Paragraph));
    assert_eq!(t1, BlockType::Paragraph, "空 item 不中断段落: {:?}", (t1, a1));
    // 有内容 marker 正常中断
    let (t2, _a2) = scan_block(b"- x", &[], Some(BlockType::Paragraph));
    assert_eq!(t2, BlockType::Bullet, "有内容 marker 中断段落");
    // 非 1 的 ordered 空 item 也段落（number 规则 + 空规则双重）
    let (t3, _a3) = scan_block(b"1.", &[], Some(BlockType::Paragraph));
    assert_eq!(t3, BlockType::Paragraph, "空 ordered item 不中断: {:?}", t3);
}

#[test]
fn gauntlet_tab_item_content() {
    use crate::block::{measure, BlockType};
    // tab 构造（`-\t\tfoo`）的 content_offset 列→字节（tab 展开）——不剥超
    let m = measure(BlockType::Bullet, b"-\t\tfoo");
    assert!(m.is_some());
    let m = m.unwrap();
    assert!(m.prefix_bytes < b"-\t\tfoo".len(),
        "content_offset 应不超行字节（tab 列→字节）: {:?}", m);
}

// P7a 列坐标单源回归：peel/effective/classify_cf/container_action 的
// 字节偏移全部收敛到列（tab 展开）——锁住字节数当列用的三处实现。

#[test]
fn p7a_peel_offset_is_columns_with_tab_prefix() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    // peel_lower_prefixes 只剥 lower frames（top-1）——补一个栈顶帧，
    // 让 BQ 位于被剥层。
    let top = BlockFrame::new(FrameKind::BulletList, 0, 0);
    let stack = [bq, top];
    // `>` 列 1 + tab（列 1 起算）→ 列 4——旧字节偏移会返回 2。
    let (peeled, cols) = peel_lower_prefixes(b">\tx", &stack);
    assert_eq!(peeled, b"x");
    assert_eq!(cols, 4);
    // 只吃必选 1 空白——剩余 tab 是 peeled 内容缩进（列 4）。
    let (peeled, cols) = peel_lower_prefixes(b"> \tx", &stack);
    assert_eq!(peeled, b"\tx");
    assert_eq!(cols, 2);
    let facts = LineFacts::from(b"> \tx");
    assert_eq!(effective_cmp_after_peel(&facts, peeled, cols), 4);
}

#[test]
fn p7a_effective_cmp_counts_list_indent_before_bq_marker() {
    // 无下层 BQ 帧时，`  > b` 的前导 2 列是列表内容缩进，不是 BQ 标记缩进
    // （回归：直接忽略 has_gt 行的 facts.indent_col 会算成 0 → 弹列表）。
    let facts = LineFacts::from(b"  > b");
    let blt = BlockFrame::new(FrameKind::BulletList, 2, 0);
    let (peeled, cols) = peel_lower_prefixes(facts.content, &[blt]);
    assert_eq!(peeled, b"> b");
    assert_eq!(cols, 0);
    assert_eq!(effective_cmp_after_peel(&facts, peeled, cols), 2);
}

#[test]
fn p7a_classify_cf_line_indent_uses_columns() {
    // 非 structural 行：4 空格 + `>\t```——`>` 前缀从列 4 起 tab 展开后
    // 围栏起点在列 8；frame.indent=4 时窗口 [4,7] 必须拒绝（旧字节和 6 会误收）。
    let facts = LineFacts::from(b"    >\t```");
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    let cf = BlockFrame::new(FrameKind::CodeFence, 4, BlockFrame::encode_cf_body(b'`', 3));
    let rt = crate::block::ScanRuntime { stack: &[bq, cf], open_lines: &[], rope: None };
    let scene = classify_cf(&facts, &cf, &rt);
    assert!(matches!(scene, Some((BlockType::CodeContent, ScanAction::Continue))));
}

#[test]
fn p7a_container_action_takes_raw_columns() {
    // NBSP 缩进：列 1、字节 2——旧 dispatch_new_block 传字节会把 fence frame
    // 记成 indent=2。顶层进入点现在传 facts.indent_col。
    let (_t, action) = scan_block(b"\xC2\xA0```", &[], None);
    match action {
        ScanAction::Push(f) => assert_eq!(f.indent(), 1),
        other => panic!("expected fence Push, got {other:?}"),
    }
}


#[test]
fn p7a_resolver_rel_starts_at_raw_origin() {
    // `- \t> x`：bullet marker 的 marker_end_col=2，后续 tab 应从列 2
    // 展开（相对 2≤3）——从 0 算 tab 是 4 列，会把 BQ 嵌套误判成
    // first-block IC。
    let (t, action) = scan_block(b"- \t> x", &[], None);
    match action {
        ScanAction::Composite { nested, .. } => {
            assert_eq!(t, BlockType::Paragraph);
            assert_eq!(nested.first().map(|f| f.kind()), Some(FrameKind::BlockQuote));
        }
        other => panic!("expected BQ Composite, got t={t:?} action={other:?}"),
    }
}

#[test]
fn p7a_indent_capped_uses_relative_columns() {
    // tab 从 mark_end=2 展开到 4——相对宽度 2，不是 compute_indent 从 0 的 4。
    assert_eq!(indent_capped_at(b"\tfoo", 2), 2);
    // 普通空格仍按相对列 cap。
    assert_eq!(indent_capped_at(b"    foo", 2), 4);
}


// A5 打断组矩阵：interrupts_para 行为级协作断言
// ——规则单位（打断组）的修饰组合穷举——顶层/lazy 两消费点查同一函数——
// 表条目（InterruptRule）+ 几何（≤3）+ 修饰（空 item/number-1/type7）协作。

#[test]
fn interrupts_para_behavior_matrix() {
    use crate::block::BlockType;
    use crate::effect::can_interrupt as interrupts_para;
    use BlockType::*;
    // 几何：开行缩进 ≤3（CM §4.1/4.5/4.6——≥4 是 IC 形态——不打断）
    assert!(interrupts_para(ThematicBreak, b"---", 0), "thematic 0 缩进打断");
    assert!(interrupts_para(ThematicBreak, b"---", 3), "thematic 3 缩进打断（合法开行）");
    assert!(!interrupts_para(ThematicBreak, b"---", 4), "thematic 4 缩进不打断（IC 形态）");
    assert!(interrupts_para(CodeFenceOpen, b"```", 3), "fence 3 缩进打断（CM §4.5 ≤3）");
    assert!(!interrupts_para(CodeFenceOpen, b"```", 4), "fence 4 缩进不打断（A3 差距 3 修复）");
    assert!(interrupts_para(HtmlBlock, b"<div>", 3), "html type6 3 缩进打断");
    assert!(!interrupts_para(HtmlBlock, b"<div>", 4), "html 4 缩进不打断（A3 差距 3b）");
    // List 修饰：空 item（CM §5.2——`foo\n*` 例）
    assert!(!interrupts_para(Bullet, b"- ", 0), "空 item 不打断");
    assert!(!interrupts_para(Bullet, b"-", 0), "单 dash 空 item 不打断");
    assert!(!interrupts_para(OrderedListItem, b"1. ", 0), "空 OL item 不打断");
    assert!(interrupts_para(Bullet, b"- bar", 0), "非空 item 打断");
    // List 修饰：number-1 数值语义（CM §5.3 start number must be 1——
    // `01.` 的 start number 是 1——可打断——A2 统一裁决）
    assert!(interrupts_para(OrderedListItem, b"1. bar", 0), "1. 打断（number-1）");
    assert!(!interrupts_para(OrderedListItem, b"2. bar", 0), "2. 不打断（number≠1）");
    assert!(interrupts_para(OrderedListItem, b"01. bar", 0), "01. 打断（start number=1——数值）");
    assert!(!interrupts_para(OrderedListItem, b"10. bar", 0), "10. 不打断（start number=10）");
    assert!(interrupts_para(OrderedListItem, b"1) bar", 0), "1) 打断（')' 分隔符——数值 1）");
    // Html 修饰：type 7（CM §4.6——`Foo\n<a href="bar">` 例）
    assert!(!interrupts_para(HtmlBlock, b"<span>", 0), "type7 不打断");
    assert!(interrupts_para(HtmlBlock, b"<div>", 0), "type6 打断");
    assert!(interrupts_para(HtmlBlock, b"<script>", 0), "type1 打断");
    // No：段落内容/IC/RefDef（不能打断）
    assert!(!interrupts_para(Paragraph, b"text", 0), "Paragraph 不打断（非候选）");
    assert!(!interrupts_para(IndentedCode, b"    code", 0), "IC 不打断（CM §4.4）");
    assert!(!interrupts_para(RefDef, b"[a]: /url", 0), "RefDef 不打断（CM §4.7）");
    // Yes 本体：BQ/表格（CM §4.1——Block quotes can interrupt；GFM 表格）
    assert!(interrupts_para(Blockquote, b"> x", 0), "BQ 可打断（CM §4.1）");
    assert!(interrupts_para(TableRow, b"| a |", 0), "表格行可打断（GFM——A3 差距 1 修复）");
}

// ── BQ lazy 打断的偏移语义（cmark 实证）──
// 修法：BQ frame.body 存本层内容列（push 链原点坐标系），打断判定取
// 「Σ 栈内全部 BQ body − 行绝对缩进」的偏移：offset <= 0 可打断（关 BQ、
// 块开父层），offset > 0 恒 lazy 段落文本（cmark：lazy 不承载块）。
#[test]
fn test_bq_lazy_interrupt_offset_semantics() {
    let types = |text: &str| -> Vec<BlockType> {
        let mut st = crate::projection::State::new(text);
        let snap = st.snapshot();
        (0..snap.line_len()).map(|i| snap.lines[i as usize].node_type()).collect()
    };
    // offset == 0（内容列上的 marker）：关 BQ，列表开在父层
    let m3 = types("- a\n  > q\n    - b\n");
    assert_eq!(m3[2], BlockType::Bullet, "m3: 内容列上的 marker 应打断");
    // offset < 0（更浅缩进的兄弟项）：关 BQ + 关 item，兄弟项独立
    let m4 = types("- user:\n    - a:\n      > q\n    - b:\n  > 帮我\n- c:\n");
    assert_eq!(m4[3], BlockType::Bullet, "m4: b: 应为兄弟项");
    assert_eq!(m4[5], BlockType::Bullet, "m4: c: 应为兄弟项");
    // offset == 0（标题，顶层 BQ）：关 BQ，标题开在文档层
    let n6 = types("> q\n  # h\n");
    assert_eq!(n6[1], BlockType::Header1, "n6: 内容列上的标题应打断");
    // f5（标题在 item 内）：断言 BQ 已在行 2 关闭（重放到行 3 的栈不再含 BQ）；
    // 该行的块分类是列表侧既有行为（与本次修复无关）。
    let mut st = crate::projection::State::new("- a\n  > q\n    # h\n");
    let snap = st.snapshot();
    let stack_after = snap.frame_log.rebuild_stack(3);
    assert!(!stack_after.iter().any(|f| f.kind() == FrameKind::BlockQuote),
        "f5: 行 2 处理完 BQ 应已关");
    // offset > 0：长得像 marker 也恒 lazy 文本（cmark：lazy 不承载块）
    let f1 = types("- a\n  > q\n      - b\n");
    assert_eq!(f1[2], BlockType::Paragraph, "f1: 深缩进 marker 应 lazy 文本");
    // offset < 0 的段落行仍 lazy（零缩进 lazy 不因几何而弹）
    let f4 = types("> q\nbar\n");
    assert_eq!(f4[1], BlockType::Paragraph, "f4: 零缩进 lazy 段落保留");
    // 嵌套 BQ：Σ 内容列——双层 BQ 在更浅 marker 下逐层关
    let n4 = types("> > q\n  - b\n");
    assert_eq!(n4[1], BlockType::Bullet, "n4: 双层 BQ 逐层关，列表到顶层");
    let n5 = types("- a\n  > > q\n    - b\n");
    assert_eq!(n5[2], BlockType::Bullet, "n5: 双层 BQ 逐层关，列表开在 item");
}

// ── wl4：更浅的 `>` 行关 BQ（前缀行也要列校验）──
// `- user` 内 ts1 内容列 6、BQ 开在 8：`> 继续任务` 缩进 4 应关 BQ/关 item、
// 在 user 层重开；6/8/10 才是同一 BQ 的续行（cmark 实测边界）。
#[test]
fn test_bq_shallower_prefixed_line_closes_bq() {
    let types = |text: &str| -> Vec<BlockType> {
        let mut st = crate::projection::State::new(text);
        let snap = st.snapshot();
        (0..snap.line_len()).map(|i| snap.lines[i as usize].node_type()).collect()
    };
    let base = "- user:\n    * ts1:\n        - type: compact\n        - next: 3\n        > 摘要1\n        > 摘要2\n";
    // 0/2/4：更浅——BQ 关闭，行成为更外层的 BQ 开行（Blockquote）
    for ind in [0usize, 2, 4] {
        let doc = format!("{base}{}> 继续任务\n", " ".repeat(ind));
        assert_eq!(types(&doc)[6], BlockType::Blockquote,
            "缩进 {ind}: 更浅的 > 行应关 BQ、在更外层重开");
    }
    // 6/8/10：到达外层 item 内容列——同一 BQ 段落续行（Paragraph）
    for ind in [6usize, 8, 10] {
        let doc = format!("{base}{}> 继续任务\n", " ".repeat(ind));
        assert_eq!(types(&doc)[6], BlockType::Paragraph,
            "缩进 {ind}: 同一 BQ 续行应吸收");
    }
}

#[test]
fn test_list_bq_table_forms_table() {
    let text = "- x\n    > | a | b |\n    > | - | - |\n    > | 1 | 2 |\n";
    let mut st = crate::projection::State::new(text);
    let snap = st.snapshot();
    let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(cf.contains("\"nt\":\"tb\""), "list 内 BQ 的 table 应成表，cf={cf}");
}

#[test]
fn test_list_bq_table_separator_confirms_not_pops() {
    let text = "- x\n    > | a | b |\n    > | - | - |\n    > | 1 | 2 |\n";
    let mut st = crate::projection::State::new(text);
    let snap = st.snapshot();
    let mut body_mut = false;
    let mut popped = false;
    for &e in &snap.frame_log.entries {
        if crate::block::entry_line(e) == 2 {
            if crate::block::entry_is_body_mut(e) { body_mut = true; }
            else if crate::block::entry_is_pop(e) { popped = true; }
        }
    }
    assert!(body_mut, "分隔行应确认 table（body_mut）");
    assert!(!popped, "分隔行不得关表（Pop）");
}

#[test]
fn test_top_level_bq_table_still_table() {
    let text = "> | a | b |\n> | - | - |\n> | 1 | 2 |\n";
    let mut st = crate::projection::State::new(text);
    let snap = st.snapshot();
    let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(cf.contains("\"nt\":\"tb\""), "顶层 BQ table 对照，cf={cf}");
    let mut body_mut = false;
    for &e in &snap.frame_log.entries {
        if crate::block::entry_line(e) == 1 && crate::block::entry_is_body_mut(e) { body_mut = true; }
    }
    assert!(body_mut, "顶层 BQ table 分隔行也应 body_mut");
}

#[test]
fn test_nested_bq_table_forms_table() {
    let text = "> > | a | b |\n> > | - | - |\n> > | 1 | 2 |\n";
    let mut st = crate::projection::State::new(text);
    let snap = st.snapshot();
    let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(cf.contains("\"nt\":\"tb\""), "双层 BQ table 应成表，cf={cf}");
    let mut body_mut = false;
    let mut popped = false;
    for &e in &snap.frame_log.entries {
        if crate::block::entry_line(e) == 1 {
            if crate::block::entry_is_body_mut(e) { body_mut = true; }
            else if crate::block::entry_is_pop(e) { popped = true; }
        }
    }
    assert!(body_mut, "双层 BQ 分隔行应确认 table（body_mut）");
    assert!(!popped, "双层 BQ 分隔行不得关表（Pop）");
}

#[test]
fn test_bq_extra_space_table_forms_table() {
    let text = ">  | a | b |\n>  | - | - |\n>  | 1 | 2 |\n";
    let mut st = crate::projection::State::new(text);
    let snap = st.snapshot();
    let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(cf.contains("\"nt\":\"tb\""), "> 两空格 BQ table 应成表，cf={cf}");
    let mut body_mut = false;
    for &e in &snap.frame_log.entries {
        if crate::block::entry_line(e) == 1 && crate::block::entry_is_body_mut(e) { body_mut = true; }
    }
    assert!(body_mut, "两空格 BQ 分隔行应确认 table");
}

#[test]
fn test_list_bq_fence_closes() {
    let text = "- x\n    > ```md\n    > a\n    > ```\n    >\n    > tail\n";
    let mut st = crate::projection::State::new(text);
    let snap = st.snapshot();
    assert_eq!(snap.lines[3].node_type(), BlockType::CodeFenceClose, "list 内 BQ 的 fence 应闭合");
}

#[test]
fn test_table_in_list_item_without_bq() {
    let text = "- x\n  | a | b |\n  | - | - |\n  | 1 | 2 |\n";
    let mut st = crate::projection::State::new(text);
    let snap = st.snapshot();
    let cf = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    assert!(cf.contains("\"nt\":\"tb\""), "list 内无 BQ 的 table 对照，cf={cf}");
}

#[test]
fn test_first_item_layer_push_skips_pop() {
    // PopPush 行 = pop + push 两条目；helper 必须跳过 pop（consume=0），
    // 否则非首条 item 的 marker consume 恒 0，tag 写把整行当 content。
    let text = "- a: 1\n- b: 2\n- c: 3\n";
    let mut st = crate::projection::State::new(text);
    let snap = st.snapshot();
    let entries = &snap.frame_log.entries;
    for line in 0..3u32 {
        let (k, c) = crate::block::first_item_layer_push(entries, line).expect("push at line");
        assert_eq!(k, FrameKind::BulletList);
        assert_eq!(c, 2, "line {line} consume 应为 marker 宽 2");
    }
}
