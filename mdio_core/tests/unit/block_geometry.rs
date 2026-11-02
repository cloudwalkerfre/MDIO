use super::*;

#[test]
fn atx_header1() {
    let g = line_geometry(BlockType::Header1, b"# Title").unwrap();
    assert_eq!(g.mark_text_end, 1);
    assert_eq!(g.mark_end, 1);
    assert_eq!(g.tag_range, 2..7);
    assert_eq!(g.frame_body, 0);
    assert_eq!(g.content_offset, 1);
}

#[test]
fn atx_header3_with_trailing_hash() {
    let g = line_geometry(BlockType::Header3, b"### Title ##").unwrap();
    assert_eq!(g.mark_text_end, 3);
    assert_eq!(g.tag_range, 4..9); // "Title"
}

#[test]
fn atx_header_no_space_not_header() {
    assert!(line_geometry(BlockType::Header1, b"#notitle").is_none());
    assert!(line_geometry(BlockType::Header2, b"##notitle").is_none());
}

#[test]
fn atx_header1_closing_hash_stripped() {
    let g = line_geometry(BlockType::Header1, b"# Foo #").unwrap();
    assert_eq!(g.tag_range, 2..5); // "Foo"
}

#[test]
fn setext_head1() {
    let g = line_geometry(BlockType::HsHead1, b"My Title").unwrap();
    assert_eq!(g.mark_text_end, 0);
    assert_eq!(g.mark_end, 0);
    assert_eq!(g.tag_range, 0..8);
    assert_eq!(g.frame_body, 0);
    assert_eq!(g.content_offset, 0);
}

#[test]
fn bullet_basic() {
    let g = line_geometry(BlockType::Bullet, b"- key: value").unwrap();
    assert_eq!(g.mark_text_end, 1);
    assert_eq!(g.mark_end, 2);
    assert_eq!(g.tag_range, 2..5); // "key"
    assert_eq!(g.content_offset, 2);
}

#[test]
fn bullet_no_key() {
    let g = line_geometry(BlockType::Bullet, b"- plain text").unwrap();
    assert_eq!(g.mark_text_end, 1);
    assert_eq!(g.mark_end, 2);
    assert_eq!(g.tag_range, 0..0);
    assert_eq!(g.content_offset, 2);
}

#[test]
fn bullet_bracket_key() {
    let g = line_geometry(BlockType::Bullet, b"- [tag] value").unwrap();
    assert_eq!(g.tag_range, 2..7); // "[tag]" (includes both brackets)
}

#[test]
fn bullet_asterisk() {
    let g = line_geometry(BlockType::Bullet, b"* item").unwrap();
    assert_eq!(g.mark_text_end, 1);
    assert_eq!(g.mark_end, 2);
}

#[test]
fn bullet_plus() {
    let g = line_geometry(BlockType::Bullet, b"+ item").unwrap();
    assert_eq!(g.mark_text_end, 1);
    assert_eq!(g.mark_end, 2);
}

#[test]
fn checklist_plain() {
    // "- [ ] item"  — need at least 6 bytes for CKL
    let g = line_geometry(BlockType::Checklist, b"- [ ] item").unwrap();
    assert_eq!(g.mark_text_end, 1);
    assert_eq!(g.mark_end, 6);
    assert_eq!(g.tag_range, 3..4); // 状态字符 " "（C3：无键 ck tag = 状态字符）
}

#[test]
fn checklist_done() {
    let g = line_geometry(BlockType::Checklist, b"- [x] item").unwrap();
    assert_eq!(g.mark_end, 6);
    assert_eq!(g.tag_range, 3..4); // 状态字符 "x"（C3）
}

#[test]
fn checklist_with_colon_key() {
    let g = line_geometry(BlockType::Checklist, b"- [ ] key: value").unwrap();
    assert_eq!(g.mark_end, 6);
    assert_eq!(g.tag_range, 6..9); // "key" after checkbox
}

#[test]
fn ordered_list() {
    let g = line_geometry(BlockType::OrderedListItem, b"123. item").unwrap();
    assert_eq!(g.mark_text_end, 4); // "123."
    assert_eq!(g.mark_end, 5);      // "123. "
    assert_eq!(g.tag_range, 0..3);  // "123"
    assert_eq!(g.content_offset, 5);
}

#[test]
fn ordered_list_paren() {
    let g = line_geometry(BlockType::OrderedListItem, b"42) item").unwrap();
    assert_eq!(g.mark_text_end, 3); // "42)"
    assert_eq!(g.mark_end, 4);
    assert_eq!(g.tag_range, 0..2);
}

#[test]
fn code_fence_open() {
    let g = line_geometry(BlockType::CodeFenceOpen, b"```rust").unwrap();
    assert_eq!(g.mark_text_end, 3);
    assert_eq!(g.mark_end, 3);
    assert_eq!(g.tag_range, 3..7); // "rust"
}

#[test]
fn code_fence_tilde() {
    let g = line_geometry(BlockType::CodeFenceOpen, b"~~~python").unwrap();
    assert_eq!(g.mark_text_end, 3);
    assert_eq!(g.tag_range, 3..9);
}

#[test]
fn code_fence_no_lang() {
    let g = line_geometry(BlockType::CodeFenceOpen, b"```").unwrap();
    assert_eq!(g.tag_range, 3..3); // empty
}

#[test]
fn code_fence_close() {
    let g = line_geometry(BlockType::CodeFenceClose, b"```").unwrap();
    assert_eq!(g.mark_text_end, 3);
    assert_eq!(g.tag_range, 3..3);
}

#[test]
fn blockquote() {
    let g = line_geometry(BlockType::Blockquote, b"> Quote").unwrap();
    assert_eq!(g.mark_text_end, 1);
    assert_eq!(g.mark_end, 1);
    assert_eq!(g.tag_range, 0..0);
    assert_eq!(g.content_offset, 1);
}

#[test]
fn html_block_div() {
    let g = line_geometry(BlockType::HtmlBlock, b"<div>").unwrap();
    assert!(g.mark_text_end > 0);
    assert_eq!(g.tag_range, 1..4); // "div"
}

#[test]
fn html_element() {
    let g = line_geometry(BlockType::HtmlElement, b"<p class='x'>").unwrap();
    assert!(g.mark_text_end > 0);
    assert_eq!(g.tag_range, 1..2); // "p"
}

#[test]
fn comment() {
    let g = line_geometry(BlockType::Comment, b"<!--").unwrap();
    assert_eq!(g.mark_text_end, 4);
    assert_eq!(g.mark_end, 4);
    assert_eq!(g.tag_range, 0..0);
    assert_eq!(g.content_offset, 0);
}

#[test]
fn table_row() {
    let g = line_geometry(BlockType::TableRow, b"| a | b |").unwrap();
    assert_eq!(g.mark_text_end, 1);
    assert_eq!(g.mark_end, 1);
    assert_eq!(g.tag_range, 0..0);
}

#[test]
fn table_sep() {
    let g = line_geometry(BlockType::TableSeparator, b"| --- |").unwrap();
    assert_eq!(g.mark_text_end, 1);
}

#[test]
fn paragraph_none() {
    assert!(line_geometry(BlockType::Paragraph, b"plain text").is_none());
}

#[test]
fn thematic_break_none() {
    assert!(line_geometry(BlockType::ThematicBreak, b"---").is_none());
}

#[test]
fn indented_code_none() {
    assert!(line_geometry(BlockType::IndentedCode, b"    code").is_none());
}

// ── range_content tests ──

#[test]
fn range_content_heading() {
    assert!(range_content(BlockType::Header1, "Title").is_none());
    assert_eq!(range_content(BlockType::Header1, "Title\nBody\n"), Some(6..11));
}

#[test]
fn range_content_fence() {
    assert_eq!(
        range_content(BlockType::CodeFenceOpen, "```\ncode\n```\n"),
        Some(4..9)
    );
    // 未闭合 CF（无闭围栏）：内容到文档尾（有尾 \n 的最后一个 \n 是内容行的，
    // 不能按 rfind 截断）——内容必须保留（fence-break 回归）。
    assert_eq!(
        range_content(BlockType::CodeFenceOpen, "```\ncode\n"),
        Some(4..9)
    );
    assert_eq!(
        range_content(BlockType::CodeFenceOpen, "```\ncode\nafter\n"),
        Some(4..15)
    );
}

#[test]
fn range_content_comment() {
    assert_eq!(
        range_content(BlockType::Comment, "<!-- hide -->"),
        Some(4..10)
    );
}

// ── extract_mark tests ──

#[test]
fn extract_mark_head() {
    assert_eq!(extract_mark(BlockType::Header3, "### Title"), "###");
}

#[test]
fn extract_mark_bullet() {
    assert_eq!(extract_mark(BlockType::Bullet, "- key: val"), "-");
}

#[test]
fn extract_mark_fence() {
    assert_eq!(extract_mark(BlockType::CodeFenceOpen, "```rust"), "```");
}

#[test]
fn extract_mark_indented_code() {
    assert_eq!(extract_mark(BlockType::IndentedCode, "    code"), "    ");
}

#[test]
fn extract_mark_none_for_para() {
    assert_eq!(extract_mark(BlockType::Paragraph, "text"), "");
}

// ── extract_tag tests ──

#[test]
fn extract_tag_heading() {
    assert_eq!(extract_tag(BlockType::Header2, "## My Title ###"), "My Title");
}

#[test]
fn extract_tag_bullet_key() {
    assert_eq!(extract_tag(BlockType::Bullet, "- hello: world"), "hello");
}

#[test]
fn extract_tag_ol_number() {
    assert_eq!(extract_tag(BlockType::OrderedListItem, "42. item"), "42");
}

#[test]
fn extract_tag_fence_lang() {
    assert_eq!(extract_tag(BlockType::CodeFenceOpen, "```rust"), "rust");
}

// ── measure tests ──

#[test]
fn measure_bullet() {
    let m = measure(BlockType::Bullet, b"- key: val").unwrap();
    assert_eq!(m.prefix_bytes, 2);
    // frame_body for '-' bullet is 0 (code 0 << 4)
    assert_eq!(m.frame_body, 0);
}

#[test]
fn measure_paragraph_none() {
    assert!(measure(BlockType::Paragraph, b"plain text").is_none());
}

// ── strip tests ──

#[test]
fn strip_bullet_all() {
    let r = strip(BlockType::Bullet, b"- key: val", StripMode::All).unwrap();
    assert_eq!(r, b"key: val");
}

#[test]
fn strip_bq_exact() {
    let r = strip(BlockType::Blockquote, b"> quote", StripMode::Exact).unwrap();
    assert_eq!(r, b"quote");
}

#[test]
fn strip_bq_exact_no_space() {
    let r = strip(BlockType::Blockquote, b">quote", StripMode::Exact).unwrap();
    assert_eq!(r, b"quote");
}

// ── format_payload tests ──

#[test]
fn format_payload_bq_content() {
    let s = format_payload(BlockType::Blockquote, "content", "hello\nworld", "", None, &[], &[]);
    assert_eq!(s, "> hello\n> world");
}

#[test]
fn format_payload_bl_content() {
    // #88（注销"续行加固定 4 空格"设定——payload 原样——
    // 读端 dedent 对顶层剥 content_offset=2——固定 4 不对称剩 2（气泡
    // 空白）——研究结论: 取消设定（0 缩进续行由 LazyParagraph 并入——
    // CM §5.2——含 marker 续行按 markdown 打断——与 CM 一致）
    let s = format_payload(BlockType::Bullet, "content", "hello\nworld", "", None, &[], &[]);
    assert_eq!(s, "hello\nworld");
}

#[test]
fn format_payload_ol_tag() {
    let s = format_payload(BlockType::OrderedListItem, "tag", "99", "  42. item", None, &[], &[]);
    assert_eq!(s, "  99. item");
}

#[test]
fn format_payload_bl_tag_empty() {
    let s = format_payload(BlockType::Bullet, "tag", "", "  - key: value", None, &[], &[]);
    assert!(s.contains("- value"));
}

#[test]
fn format_payload_bl_tag_bracket() {
    let s = format_payload(BlockType::Bullet, "tag", "[done]", "  - value", None, &[], &[]);
    assert_eq!(s, "  - [done] value");
}

#[test]
fn format_payload_passthrough() {
    let s = format_payload(BlockType::Paragraph, "raw", "hello", "", None, &[], &[]);
    assert_eq!(s, "hello");
}

// ── skip_leading_whitespace NEON 加速: 对拍 + 定向反例 ──

fn ws_eq_check(bytes: &[u8]) {
    let base = skip_ws_baseline_pos(bytes);
    let got = skip_ws_pos(bytes);
    assert_eq!(base, got, "skip_ws mismatch: {:?}", bytes);
}

#[test]
fn skip_ws_directed_borrow_cases() {
    // 定向反例（>=8B 进入加速主循环）:
    // space + 0x21('!')——SWAR 版曾在此假阳性（NEON 精确, 必须通过）
    ws_eq_check(b" !abcdefg");
    ws_eq_check(b"  !xyz123");
    ws_eq_check(b"      !ab");
    ws_eq_check(b" a bcdefg");
    ws_eq_check(b"\t!abcdef");
    ws_eq_check(b"\t0abcdef");
    ws_eq_check(b"        x"); // 8 space 整窗
    ws_eq_check(b"        ");  // 8 space 无内容
    ws_eq_check(b"                x"); // 16 space 整窗
    ws_eq_check(b"                ");  // 16 space 无内容
    ws_eq_check(b"                 x"); // 17 space
}

#[test]
fn skip_ws_nbsp_chains() {
    // NBSP 连续链（probe 曾在此漏 continue）
    let mut s = Vec::new();
    s.extend_from_slice(&[0xC2, 0xA0, 0xC2, 0xA0, 0xC2, 0xA0]);
    s.extend_from_slice(b"x");
    ws_eq_check(&s);
    // tab + NBSP + space 混合
    let mut m = Vec::new();
    m.extend_from_slice(b"\t");
    m.extend_from_slice(&[0xC2, 0xA0]);
    m.extend_from_slice(b"  abcdef");
    ws_eq_check(&m);
    // 0xC2 后跟非 0xA0（U+0080 家族——不是 NBSP——必须 break）
    let mut n = Vec::new();
    n.extend_from_slice(b" ");
    n.push(0xC2); n.push(0x80);
    n.extend_from_slice(b"abcdefgh");
    ws_eq_check(&n);
    // 行尾的孤立 0xC2
    let mut o = vec![b' '; 20];
    o.push(0xC2);
    ws_eq_check(&o);
    // 0xC2 在 16B 窗口边界附近
    for off in 13..20usize {
        let mut p = vec![b' '; 40];
        p[off] = 0xC2;
        p[off + 1] = 0xA0;
        ws_eq_check(&p);
    }
}

#[test]
fn skip_ws_window_boundaries() {
    // 触发/停止位置在窗口边界 15/16/17/31/32
    for &(len, stop) in &[(16usize, 15usize), (17, 16), (32, 31), (33, 32), (40, 33)] {
        let mut s = vec![b' '; len.max(stop + 1)];
        s[stop] = b'x';
        ws_eq_check(&s);
    }
    // 尾部 1-15 字节（回退路径）
    for tail in 1..16usize {
        let mut s = vec![b' '; tail];
        ws_eq_check(&s);
        let mut t2 = vec![b' '; tail + 20];
        t2[20 + tail - 1] = b'x';
        ws_eq_check(&t2);
    }
    ws_eq_check(b"");
    ws_eq_check(b"   ");
    ws_eq_check(b"\t\t\t");
}

#[test]
fn skip_ws_fuzz_vs_baseline() {
    let mut seed = 0x9E3779B97F4A7C15u64;
    let mut buf = vec![0u8; 300];
    for _ in 0..5000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let n = ((seed >> 33) % 290) as usize + 10;
        for b in buf.iter_mut().take(n) {
            let k = (seed >> 33) % 100;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *b = match k {
                0..=30 => b' ',
                31..=40 => b'\t',
                41..=44 => 0xC2,
                45..=48 => 0xA0,
                49..=55 => 0x80 + ((seed >> 33) % 0x3F) as u8,
                _ => (seed >> 33) as u8 % 90 + 32,
            };
        }
        ws_eq_check(&buf[..n]);
    }
}
