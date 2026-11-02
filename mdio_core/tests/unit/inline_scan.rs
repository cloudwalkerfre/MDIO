use super::*;

#[test]
fn proj_frame_bit_layout() {
    let pf = InlineFrame::pair_delim(0, 2);
    assert_eq!(pf.geo_kind(), GeoKind::PairDelim);
    assert_eq!(pf.opener_use(), 2);
    assert_eq!(pf.closer_use(), 2);
    assert_eq!(pf.delim_byte(), 0);
    assert_eq!(pf.inline_kind(), InlineType::Strong);

    let pf = InlineFrame::pair_delim(0, 1);
    assert_eq!(pf.inline_kind(), InlineType::Emph);

    let pf = InlineFrame::pair_delim(2, 2);
    assert_eq!(pf.inline_kind(), InlineType::Strikethrough);
}

#[test]
fn fenced_frame_bits() {
    let pf = InlineFrame::fenced(0, 5);
    assert_eq!(pf.geo_kind(), GeoKind::Fenced);
    assert_eq!(pf.fenced_sub(), 0);
    assert_eq!(pf.fence_len(), 5);
}

#[test]
fn bracket_bits() {
    let pf = InlineFrame::bracket(true, false);
    assert!(pf.is_image());
    assert!(!pf.is_ref());
    assert_eq!(pf.inline_kind(), InlineType::Image);

    let pf = InlineFrame::bracket(false, true);
    assert!(!pf.is_image());
    assert!(pf.is_ref());
    assert_eq!(pf.inline_kind(), InlineType::Link);
}

#[test]
fn plain_and_escape() {
    assert_eq!(InlineFrame::plain(0).inline_kind(), InlineType::Text);
    assert_eq!(InlineFrame::plain(1).inline_kind(), InlineType::SoftBreak);
    assert_eq!(InlineFrame::plain(2).inline_kind(), InlineType::LineBreak);
    assert_eq!(InlineFrame::escape().inline_kind(), InlineType::Escaped);
    assert_eq!(InlineFrame::entity().inline_kind(), InlineType::Entity);
}

#[test]
fn emph_state_fields() {
    let s = EmphState { can_open: true, can_close: false, tombstone: false, num_chars: 2, delim_code: 0 };
    assert!(s.can_open);
    assert!(!s.can_close);
    assert_eq!(s.num_chars, 2);
    assert_eq!(s.delim_code, 0);
    assert!(!s.tombstone);

    let mut s2 = EmphState { can_open: true, can_close: false, tombstone: false, num_chars: 2, delim_code: 0 };
    s2.tombstone = true;
    assert!(s2.tombstone);
    s2.num_chars = 1;
    assert_eq!(s2.num_chars, 1);
}

#[test]
fn scan_delims_basic() {
    let (n, open, close) = scan_delims_flanking("*foo", 0, b'*', FlankingRule::Standard);
    assert_eq!(n, 1);
    assert!(open);
    assert!(!close);

    let (n, open, close) = scan_delims_flanking("foo*", 3, b'*', FlankingRule::Standard);
    assert_eq!(n, 1);
    assert!(!open);
    assert!(close);

    let (n, _, _) = scan_delims_flanking("**foo", 0, b'*', FlankingRule::Standard);
    assert_eq!(n, 2);
}

#[test]
fn scan_delims_underscore() {
    let (n, open, close) = scan_delims_flanking("_foo_", 0, b'_', FlankingRule::Underscore);
    assert_eq!(n, 1);
    assert!(open);
    assert!(!close);
}

#[test]
fn scan_delims_tilde() {
    let (n, open, close) = scan_delims_flanking("~~foo~~", 0, b'~', FlankingRule::Tilde);
    assert_eq!(n, 2);
    assert!(open);
    assert!(!close);

    let (n, open, close) = scan_delims_flanking("~foo~", 0, b'~', FlankingRule::Tilde);
    assert_eq!(n, 1);
    assert!(!open);
    assert!(!close);
}

#[test]
fn normalize_label_basic() {
    assert_eq!(normalize_label("  Foo  Bar  "), "foo bar");
    assert_eq!(normalize_label("HELLO"), "hello");
    assert_eq!(normalize_label("a\nb\tc"), "a b c");
}

#[test]
fn scan_inline_plain_text() {
    let refmap = RefMap::new();
    let v = scan_inline("hello world", &refmap);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].frame.geo_kind(), GeoKind::Plain);
    assert_eq!(v[0].byte_start, 0);
    assert_eq!(v[0].byte_end, 11);
}

#[test]
fn scan_inline_emphasis_content() {
    let refmap = RefMap::new();
    let v = scan_inline("*foo* baz", &refmap);
    let text = inline_content(&v, "*foo* baz", &refmap);
    assert_eq!(text, "foo baz");
}

#[test]
fn scan_inline_double_emphasis() {
    let refmap = RefMap::new();
    let v = scan_inline("**bold** _italic_", &refmap);
    let text = inline_content(&v, "**bold** _italic_", &refmap);
    assert_eq!(text, "bold italic");
}

#[test]
fn scan_inline_code_span() {
    let refmap = RefMap::new();
    let v = scan_inline("a `code` span", &refmap);
    let text = inline_content(&v, "a `code` span", &refmap);
    assert_eq!(text, "a code span");
}

#[test]
fn scan_inline_inline_link() {
    let refmap = RefMap::new();
    let v = scan_inline("[link](https://x.com) text", &refmap);
    let text = inline_content(&v, "[link](https://x.com) text", &refmap);
    assert_eq!(text, "link text");
}

#[test]
fn scan_inline_strikethrough() {
    let refmap = RefMap::new();
    let v = scan_inline("~~old~~ new", &refmap);
    let text = inline_content(&v, "~~old~~ new", &refmap);
    assert_eq!(text, "old new");
}

#[test]
fn scan_inline_triple_asterisk() {
    let refmap = RefMap::new();
    let v = scan_inline("***foo***", &refmap);
    let text = inline_content(&v, "***foo***", &refmap);
    assert_eq!(text, "foo");
}

#[test]
fn scan_inline_nested_emphasis() {
    let refmap = RefMap::new();
    let v = scan_inline("**foo *bar* baz**", &refmap);
    let text = inline_content(&v, "**foo *bar* baz**", &refmap);
    assert_eq!(text, "foo bar baz");
}

#[test]
fn scan_inline_escape() {
    let refmap = RefMap::new();
    let v = scan_inline("a \\*escaped\\* star", &refmap);
    let text = inline_content(&v, "a \\*escaped\\* star", &refmap);
    assert_eq!(text, "a *escaped* star");
}

#[test]
fn scan_inline_entity_numeric() {
    let refmap = RefMap::new();
    let v = scan_inline("x &#62; y", &refmap);
    let text = inline_content(&v, "x &#62; y", &refmap);
    assert_eq!(text, "x #62 y");
}

#[test]
fn scan_inline_entity_named() {
    let refmap = RefMap::new();
    let v = scan_inline("a &amp; b &lt; c", &refmap);
    let text = inline_content(&v, "a &amp; b &lt; c", &refmap);
    assert_eq!(text, "a amp b lt c");
}

#[test]
fn scan_inline_entity_unknown() {
    let refmap = RefMap::new();
    let v = scan_inline("a &unknown; b", &refmap);
    let text = inline_content(&v, "a &unknown; b", &refmap);
    assert_eq!(text, "a &unknown; b");
}

#[test]
fn scan_inline_ref_link() {
    let mut refmap = RefMap::new();
    refmap.insert("test".into(), "https://x.com".into(), "".into());
    let v = scan_inline("a [text][test] b", &refmap);
    let text = inline_content(&v, "a [text][test] b", &refmap);
    assert_eq!(text, "a text b");
}

#[test]
fn scan_inline_shortcut_ref_link() {
    let mut refmap = RefMap::new();
    refmap.insert("text".into(), "https://x.com".into(), "".into());
    let v = scan_inline("a [text] b", &refmap);
    let text = inline_content(&v, "a [text] b", &refmap);
    assert_eq!(text, "a text b");
}

#[test]
fn virtual_inline_is_12_bytes() {
    assert_eq!(std::mem::size_of::<InlineNode>(), 12);
}

#[test]
fn edge_crlf_merged() {
    let refmap = RefMap::new();
    let v = scan_inline("a\r\nb", &refmap);
    let text = inline_content(&v, "a\r\nb", &refmap);
    assert_eq!(text, "a b");
}

#[test]
fn edge_backslash_at_eof() {
    let refmap = RefMap::new();
    let v = scan_inline("a\\", &refmap);
    let text = inline_content(&v, "a\\", &refmap);
    assert_eq!(text, "a\\");
}

#[test]
fn edge_empty_emphasis() {
    let refmap = RefMap::new();
    let v = scan_inline("** **", &refmap);
    let text = inline_content(&v, "** **", &refmap);
    assert_eq!(text, "** **");
}

#[test]
fn edge_cross_emphasis() {
    let refmap = RefMap::new();
    let v = scan_inline("*a _b* c_", &refmap);
    let text = inline_content(&v, "*a _b* c_", &refmap);
    assert_eq!(text, "a _b c_");
}

#[test]
fn edge_dollar_number() {
    let refmap = RefMap::new();
    let v = scan_inline("$100 bill", &refmap);
    let text = inline_content(&v, "$100 bill", &refmap);
    assert_eq!(text, "$100 bill");
}

#[test]
fn scan_inline_math() {
    let refmap = RefMap::new();
    let v = scan_inline("a $x=1$ b", &refmap);
    let text = inline_content(&v, "a $x=1$ b", &refmap);
    assert_eq!(text, "a x=1 b");
}

#[test]
fn scan_inline_display_math() {
    let refmap = RefMap::new();
    let v = scan_inline("$$x^2$$", &refmap);
    let text = inline_content(&v, "$$x^2$$", &refmap);
    assert_eq!(text, "x^2");
}

#[test]
fn scan_inline_consecutive_emphasis() {
    let refmap = RefMap::new();
    let v = scan_inline("*a* *b*", &refmap);
    let text = inline_content(&v, "*a* *b*", &refmap);
    assert_eq!(text, "a b");
}

#[test]
fn scan_inline_multi_backtick() {
    let refmap = RefMap::new();
    let v = scan_inline("`` `code` ``", &refmap);
    let text = inline_content(&v, "`` `code` ``", &refmap);
    assert_eq!(text, "`code`");
}

#[test]
fn empty_input() {
    let refmap = RefMap::new();
    let v = scan_inline("", &refmap);
    assert!(v.is_empty());
}

#[test]
fn unpaired_open_bracket_stays_literal() {
    let refmap = RefMap::new();
    let v = scan_inline("a [b c", &refmap);
    let text = inline_content(&v, "a [b c", &refmap);
    assert_eq!(text, "a [b c");
}

#[test]
fn invalid_escape_stays_literal() {
    let refmap = RefMap::new();
    let v = scan_inline(r"\a \b \c", &refmap);
    let text = inline_content(&v, r"\a \b \c", &refmap);
    assert_eq!(text, r"\a \b \c");
}

#[test]
fn entity_null_codepoint() {
    let refmap = RefMap::new();
    let v = scan_inline("&#0;", &refmap);
    let text = inline_content(&v, "&#0;", &refmap);
    assert_eq!(text, "#0");
}

#[test]
fn entity_surrogate() {
    let refmap = RefMap::new();
    let v = scan_inline("&#xD800;", &refmap);
    let text = inline_content(&v, "&#xD800;", &refmap);
    assert_eq!(text, "#xD800");
}

#[test]
fn image_shortcut_ref() {
    let mut refmap = RefMap::new();
    refmap.insert("alt".into(), "img.png".into(), "".into());
    let v = scan_inline("![alt]", &refmap);
    let text = inline_content(&v, "![alt]", &refmap);
    assert_eq!(text, "alt");
}

#[test]
fn image_full_ref() {
    let mut refmap = RefMap::new();
    refmap.insert("r".into(), "img.png".into(), "".into());
    let v = scan_inline("![alt][r]", &refmap);
    let text = inline_content(&v, "![alt][r]", &refmap);
    assert_eq!(text, "alt");
}

#[test]
fn scan_inline_ref_link_with_title() {
    let mut refmap = RefMap::new();
    refmap.insert("r".into(), "url".into(), "mytitle".into());
    let v = scan_inline("[x][r]", &refmap);
    let link = v.iter().find(|vi| vi.frame.geo_kind() == GeoKind::Bracket).unwrap();
    let tag = project(link, "[x][r]", &refmap, InlineField::Tag);
    let title = project(link, "[x][r]", &refmap, InlineField::Title);
    assert_eq!(tag, "x");
    assert_eq!(title, "mytitle");
}

#[test]
fn nested_emphasis_with_link() {
    let refmap = RefMap::new();
    let v = scan_inline("***[link](url)***", &refmap);
    let text = inline_content(&v, "***[link](url)***", &refmap);
    assert_eq!(text, "link");
}

#[test]
fn bracket_after_shortcut_blocked() {
    let refmap = RefMap::new();
    let v = scan_inline("][text]", &refmap);
    let text = inline_content(&v, "][text]", &refmap);
    assert_eq!(text, "][text]");
}

#[test]
fn no_link_openers_blocks_subsequent() {
    let mut refmap = RefMap::new();
    refmap.insert("a".into(), "url1".into(), "".into());
    refmap.insert("b".into(), "url2".into(), "".into());
    let v = scan_inline("[a](url1) [b]", &refmap);
    let text = inline_content(&v, "[a](url1) [b]", &refmap);
    assert_eq!(text, "a b");
}

#[test]
fn scan_inline_autolink() {
    let refmap = RefMap::new();
    let v = scan_inline("<https://x.com>", &refmap);
    let text = inline_content(&v, "<https://x.com>", &refmap);
    assert_eq!(text, "https://x.com");
}

#[test]
fn emph_underscore_not_in_snake_case() {
    let refmap = RefMap::new();
    let v = scan_inline("snake_case", &refmap);
    let text = inline_content(&v, "snake_case", &refmap);
    assert_eq!(text, "snake_case");
}

#[test]
fn emph_asterisk_in_math_context() {
    let refmap = RefMap::new();
    let v = scan_inline("2 * 3 = 6", &refmap);
    let text = inline_content(&v, "2 * 3 = 6", &refmap);
    assert_eq!(text, "2 * 3 = 6");
}

#[test]
fn emph_multiple_underscores() {
    let refmap = RefMap::new();
    let v = scan_inline("__bold__", &refmap);
    let text = inline_content(&v, "__bold__", &refmap);
    assert_eq!(text, "bold");
}

#[test]
fn link_with_nested_brackets() {
    let refmap = RefMap::new();
    let v = scan_inline("[a [b] c](url)", &refmap);
    let text = inline_content(&v, "[a [b] c](url)", &refmap);
    assert_eq!(text, "a [b] c");
}

#[test]
fn code_span_collapses_whitespace() {
    let refmap = RefMap::new();
    let v = scan_inline("` a  b `", &refmap);
    let text = inline_content(&v, "` a  b `", &refmap);
    // CM §6.3: each side's space stripped (both present), internal spaces preserved
    assert_eq!(text, "a  b");
}

#[test]
fn escape_preserves_next_char() {
    let refmap = RefMap::new();
    let v = scan_inline(r"\* \_ \~ \[ \]", &refmap);
    let text = inline_content(&v, r"\* \_ \~ \[ \]", &refmap);
    assert_eq!(text, r"* _ ~ [ ]");
}

#[test]
fn entity_hex_numeric() {
    let refmap = RefMap::new();
    let v = scan_inline("&#x3E;", &refmap);
    let text = inline_content(&v, "&#x3E;", &refmap);
    assert_eq!(text, "#x3E");
}

#[test]
fn hard_line_break_two_spaces() {
    let refmap = RefMap::new();
    let v = scan_inline("a  \nb", &refmap);
    let text = inline_content(&v, "a  \nb", &refmap);
    assert_eq!(text, "a\nb");
}

#[test]
fn entity_invalid_returns_replacement() {
    let refmap = RefMap::new();
    let v = scan_inline("&#X0G;", &refmap);
    let text = inline_content(&v, "&#X0G;", &refmap);
    assert_eq!(text, "#X0G");
}

#[test]
fn edge_unmatched_backtick() {
    let refmap = RefMap::new();
    let v = scan_inline("a `code span", &refmap);
    let text = inline_content(&v, "a `code span", &refmap);
    assert_eq!(text, "a `code span");
}

#[test]
fn scan_inline_image() {
    let refmap = RefMap::new();
    let v = scan_inline("a ![alt](img.png) b", &refmap);
    let text = inline_content(&v, "a ![alt](img.png) b", &refmap);
    assert_eq!(text, "a alt b");
}

#[test]
fn virtual_inline_size() {
    assert_eq!(std::mem::size_of::<InlineNode>(), 12);
}

#[test]
fn parent_none_sentinel() {
    assert_eq!(PARENT_NONE, 0xFFFF);
}

#[test]
#[test]
fn refmap_basic() {
    let mut rm = RefMap::new();
    rm.insert("test".into(), "https://example.com".into(), "Title".into());
    assert_eq!(rm.resolve("test"), "https://example.com");
    assert_eq!(rm.resolve("missing"), "");
    assert!(rm.get("test").is_some());
}

#[test]
fn code_span_precedence_over_emphasis() {
    // Case 1: emphasis `*` opener has its potential closer swallowed by code span.
    // Input: *foo`*` — emphasis opener `*`, text "foo", code span containing `*`.
    // The `*` inside code span is never dispatched, so emphasis never closes.
    let refmap = RefMap::new();
    let v = scan_inline("*foo`*`", &refmap);
    let text = inline_content(&v, "*foo`*`", &refmap);
    assert_eq!(text, "*foo*");
}

#[test]
fn code_span_precedence_over_link_bracket() {
    // Case 2: backtick opens code span before `[`, so `[not a link]...` is code
    // span content, not a link.
    let refmap = RefMap::new();
    let v = scan_inline("`[not a link](/foo`", &refmap);
    let text = inline_content(&v, "`[not a link](/foo`", &refmap);
    assert_eq!(text, "[not a link](/foo");
}

#[test]
fn backtick_first_code_span_wins_over_angle() {
    // Case 3: backtick comes before `<`, so `<a href="` becomes code span content.
    let refmap = RefMap::new();
    let v = scan_inline("`<a href=\"`\">`", &refmap);
    let text = inline_content(&v, "`<a href=\"`\">`", &refmap);
    // code span content: `<a href="` then text: `">` then literal backtick
    // Adjacent double-quote: " from code span + " from text start
    assert_eq!(text, "<a href=\"\">`");
}

#[test]
fn angle_first_mdio_no_raw_html_tag_literal() {
    // `<a href=\"`\">` — the `<a href=\"`\">` portion is a valid raw HTML tag
    // (tag name `a`, attribute `href=\"`\"`). Backtick inside quoted attribute
    // is literal HTML. Trailing backtick is standalone literal text.
    let refmap = RefMap::new();
    let v = scan_inline("<a href=\"`\">`", &refmap);
    let text = inline_content(&v, "<a href=\"`\">`", &refmap);
    assert_eq!(text, "<a href=\"`\">`");
}

#[test]
fn code_span_before_bracket_no_link() {
    // Case 5: `foo`[bar] — code span then bracket pair with no destination.
    let refmap = RefMap::new();
    let v = scan_inline("`foo`[bar]", &refmap);
    let text = inline_content(&v, "`foo`[bar]", &refmap);
    assert_eq!(text, "foo[bar]");
}

#[test]
fn emphasis_opener_code_span_with_asterisk_inside() {
    // Case 6: `*` opens emphasis, but code span `*` consumes the closer.
    let refmap = RefMap::new();
    let v = scan_inline("*`*`", &refmap);
    let text = inline_content(&v, "*`*`", &refmap);
    assert_eq!(text, "**");
}

// ── CM spec coverage gap tests ──

#[test]
fn backslash_newline_hard_break() {
    // §6.7: backslash before newline → hard line break
    let refmap = RefMap::new();
    let v = scan_inline("a\\\nb", &refmap);
    let text = inline_content(&v, "a\\\nb", &refmap);
    assert_eq!(text, "a\nb");
}

#[test]
fn backslash_crlf_hard_break() {
    // §6.7: backslash before CR+LF → hard line break.
    // scan_segment multi-byte merge: \<CRLF> → Plain(2) hard line break.
    let refmap = RefMap::new();
    let v = scan_inline("a\\\r\nb", &refmap);
    let text = inline_content(&v, "a\\\r\nb", &refmap);
    assert_eq!(text, "a\nb");
}

#[test]
fn dollar_no_closer_fallback() {
    // $ opener with no matching closer → literal text
    let refmap = RefMap::new();
    let v = scan_inline("a $math b", &refmap);
    let text = inline_content(&v, "a $math b", &refmap);
    assert_eq!(text, "a $math b");
}

#[test]
fn dollar_triple_fallback() {
    // 3 $ opener → find_closer requires exact match of 3 → content = "math"
    let refmap = RefMap::new();
    let v = scan_inline("$$$math$$$", &refmap);
    let text = inline_content(&v, "$$$math$$$", &refmap);
    assert_eq!(text, "math");
}

#[test]
fn angle_no_closer_fallback() {
    // < without closing > → literal text (消费 < 本身)
    let refmap = RefMap::new();
    let v = scan_inline("a <url b", &refmap);
    let text = inline_content(&v, "a <url b", &refmap);
    assert_eq!(text, "a <url b");
}

#[test]
fn angle_email_autolink() {
    // <email> → autolink
    let refmap = RefMap::new();
    let v = scan_inline("<foo@bar.com>", &refmap);
    let text = inline_content(&v, "<foo@bar.com>", &refmap);
    assert_eq!(text, "foo@bar.com");
}

#[test]
fn entity_missing_semicolon() {
    // & without closing ; → literal text
    let refmap = RefMap::new();
    let v = scan_inline("a &amp b", &refmap);
    let text = inline_content(&v, "a &amp b", &refmap);
    assert_eq!(text, "a &amp b");
}

#[test]
fn entity_numeric_missing_semicolon() {
    // &#... without closing ; → literal text
    let refmap = RefMap::new();
    let v = scan_inline("a &#62 b", &refmap);
    let text = inline_content(&v, "a &#62 b", &refmap);
    assert_eq!(text, "a &#62 b");
}

#[test]
fn image_no_closing_bracket() {
    // ![ without closing ] → literal text
    let refmap = RefMap::new();
    let v = scan_inline("![alt", &refmap);
    let text = inline_content(&v, "![alt", &refmap);
    assert_eq!(text, "![alt");
}

#[test]
fn emphasis_unicode_cjk() {
    // Emphasis inside CJK characters
    let refmap = RefMap::new();
    let v = scan_inline("你好*世界*再见", &refmap);
    let text = inline_content(&v, "你好*世界*再见", &refmap);
    assert_eq!(text, "你好世界再见");
}

#[test]
fn code_span_with_newline() {
    // §6.1: code span containing newline → newline replaced with space
    let refmap = RefMap::new();
    let v = scan_inline("`a\nb`", &refmap);
    let text = inline_content(&v, "`a\nb`", &refmap);
    assert_eq!(text, "a b");
}

#[test]
fn code_span_with_crlf() {
    // §6.1: code span containing CRLF → replaced with space
    let refmap = RefMap::new();
    let v = scan_inline("`a\r\nb`", &refmap);
    let text = inline_content(&v, "`a\r\nb`", &refmap);
    assert_eq!(text, "a b");
}

#[test]
fn code_span_all_spaces() {
    // §6.1: code span of all spaces — spaces preserved (not stripped)
    let refmap = RefMap::new();
    let v = scan_inline("`   `", &refmap);
    let text = inline_content(&v, "`   `", &refmap);
    assert_eq!(text, "   ");
}

#[test]
fn inline_link_empty_url() {
    // [text]() — empty destination, CM spec: creates link with empty URL
    let refmap = RefMap::new();
    let v = scan_inline("[text]()", &refmap);
    let text = inline_content(&v, "[text]()", &refmap);
    assert_eq!(text, "text");
}

#[test]
fn inline_link_angle_bracketed_url() {
    // [text](<url>) — angle-bracketed URL
    let refmap = RefMap::new();
    let v = scan_inline("[x](<https://x.com>)", &refmap);
    let text = inline_content(&v, "[x](<https://x.com>)", &refmap);
    assert_eq!(text, "x");
}

#[test]
fn soft_break_to_space() {
    // §6.8: soft line break → space
    let refmap = RefMap::new();
    let v = scan_inline("a\nb", &refmap);
    let text = inline_content(&v, "a\nb", &refmap);
    assert_eq!(text, "a b");
}

#[test]
fn no_link_openers_blocks_subsequent_bracket_as_text() {
    // no_link_openers prevents nested links only, not sequential ones.
    // After [a](url) resolves, the stack is empty → no_link_openers resets.
    // [test] (with refmap entry) resolves as a valid sequential link.
    let mut refmap = RefMap::new();
    refmap.insert("test".into(), "url".into(), "".into());
    let v = scan_inline("[a](url) [test]", &refmap);
    let text = inline_content(&v, "[a](url) [test]", &refmap);
    assert_eq!(text, "a test");
}

#[test]
fn asterisk_underscore_mixed_nesting() {
    // CM spec: * and _ can be nested independently
    let refmap = RefMap::new();
    let v = scan_inline("*a _b_ c*", &refmap);
    let text = inline_content(&v, "*a _b_ c*", &refmap);
    assert_eq!(text, "a b c");
}

// ── no_link_openers scope: nested bracket prevention ──

#[test]
fn no_link_openers_inline_link_prevents_nested_outer() {
    // [a [b](url)](url2) — inner inline link sets no_link_openers,
    // outer ] is rejected → outer [] become literal text, outer link never resolves.
    let refmap = RefMap::new();
    let v = scan_inline("[a [b](url)](url2)", &refmap);
    // Verify only 1 Bracket shell (inner [b](url)). Outer [ never resolves.
    let bracket_count = v.iter().filter(|vi| vi.frame.geo_kind() == GeoKind::Bracket).count();
    assert_eq!(bracket_count, 1,
        "Only inner [b](url) should create a Bracket; outer must be rejected by no_link_openers");
}

#[test]
fn no_link_openers_ref_link_blocks_nested_outer() {
    // [a [b][r]](url) with refmap={"r":"url_r"}
    // Inner ref link resolves → sets no_link_openers → outer ] is rejected.
    let mut refmap = RefMap::new();
    refmap.insert("r".into(), "url_r".into(), "".into());
    let v = scan_inline("[a [b][r]](url)", &refmap);
    let bracket_count = v.iter().filter(|vi| vi.frame.geo_kind() == GeoKind::Bracket).count();
    assert_eq!(bracket_count, 1,
        "Only inner ref link [b][r] should create a Bracket; outer ] must be rejected");
}

#[test]
fn no_link_openers_shortcut_link_blocks_nested_outer() {
    // [a [b]](url) with refmap={"b":"url_b"}
    // Inner shortcut link resolves → sets no_link_openers → outer ] is rejected.
    let mut refmap = RefMap::new();
    refmap.insert("b".into(), "url_b".into(), "".into());
    let v = scan_inline("[a [b]](url)", &refmap);
    let bracket_count = v.iter().filter(|vi| vi.frame.geo_kind() == GeoKind::Bracket).count();
    assert_eq!(bracket_count, 1,
        "Only inner shortcut link [b] should create a Bracket; outer ] must be rejected");
}

// ── skip_text_dispatch 加速实现的对拍与定向验证 ──

/// 确定性 LCG（与生产随机无关——纯测试用）
struct TfRng(u64);
impl TfRng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

fn check_dispatch_eq(bytes: &[u8], pos: usize) {
    let base = skip_text_dispatch_baseline(bytes, pos);
    let memchr_r = skip_text_dispatch_memchr(bytes, pos);
    assert_eq!(base, memchr_r, "memchr mismatch at pos={pos}");
    #[cfg(target_arch = "aarch64")]
    {
        let neon_r = skip_text_dispatch_neon(bytes, pos);
        assert_eq!(base, neon_r, "neon mismatch at pos={pos}");
    }
}

#[test]
fn dispatch_trigger_bytes_directed() {
    // 13 个触发字节各自: 行首 / 中间 / 重复 / 邻接非触发
    for &t in TRIGGER_BYTES.iter() {
        let mut s = vec![b'a'; 40];
        s[5] = t;
        check_dispatch_eq(&s, 0);
        check_dispatch_eq(&s, 3);
        s[20] = t;
        check_dispatch_eq(&s, 0);
        s[5] = b'x'; // 只留 20 处
        check_dispatch_eq(&s, 0);
        // 重复两个触发
        let mut d = vec![b'b'; 40];
        d[2] = t; d[3] = t;
        check_dispatch_eq(&d, 0);
        check_dispatch_eq(&d, 1);
        // 触发紧邻 0x80+ 字节（UTF-8 多字节——不应误触发）
        let mut m = vec![b'c'; 40];
        m[10] = t; m[11] = 0xE4; m[12] = 0xB8; m[13] = 0xAD;
        check_dispatch_eq(&m, 0);
        check_dispatch_eq(&m, 9);
    }
    // 非触发: 0x80+ 全段、0x7F、空白行
    let hi = vec![0xE4u8, 0xB8, 0xAD, 0xE4, 0xB8, 0xAD, 0xE4, 0xB8, 0xAD, 0xE4, 0xB8, 0xAD, 0xE4, 0xB8, 0xAD, 0xE4, 0xB8, 0xAD, 0xE4, 0xB8, 0xAD];
    check_dispatch_eq(&hi, 0);
    check_dispatch_eq(&hi, 5);
    let mut f = vec![0x7Fu8; 40];
    check_dispatch_eq(&f, 0);
    let spaces = vec![b' '; 60];
    check_dispatch_eq(&spaces, 0);
}

#[test]
fn dispatch_window_boundaries() {
    // 16 字节窗口边界: 触发在第 15/16/17/31/32/33 字节
    for &(len, tpos) in &[(16usize, 15usize), (16, 16), (17, 16), (32, 31), (33, 32), (40, 33), (8, 7), (9, 8)] {
        let mut s = vec![b'a'; len.max(tpos + 1)];
        s[tpos] = b'*';
        check_dispatch_eq(&s, 0);
        // 从中间 pos 起
        let mid = (tpos / 2).max(1);
        check_dispatch_eq(&s, mid);
    }
    // 尾部 1-15 字节（回退路径）
    for tail in 1..16usize {
        let mut s = vec![b'a'; tail];
        check_dispatch_eq(&s, 0);
        let mut t2 = vec![b'a'; tail + 20];
        t2[20 + tail - 1] = b'~';
        check_dispatch_eq(&t2, 0);
        check_dispatch_eq(&t2, 19);
    }
    // 空输入与 pos == len
    check_dispatch_eq(b"", 0);
    check_dispatch_eq(b"abcdef", 6);
}

#[test]
fn dispatch_fuzz_vs_baseline() {
    let mut r = TfRng(20260830);
    let mut buf = vec![0u8; 256];
    for _ in 0..5000 {
        let n = (r.next() % 250) as usize + 6;
        // 混合: 普通 ASCII / 触发字节 / 0x80+ 高字节
        for b in buf.iter_mut().take(n) {
            let k = r.next() % 100;
            *b = if k < 12 {
                TRIGGER_BYTES[(r.next() % 13) as usize]
            } else if k < 18 {
                (0x80 + (r.next() % 0x60)) as u8
            } else {
                (r.next() % 90 + 32) as u8
            };
        }
        let pos = (r.next() % n as u64) as usize;
        check_dispatch_eq(&buf[..n], pos);
    }
}

#[test]
fn dispatch_memchr_path_reaches_end() {
    // 无触发长文本——memchr 组合返回末尾
    let s = vec![b'x'; 200];
    assert_eq!(skip_text_dispatch_memchr(&s, 0), 200);
    let s2 = vec![b' '; 100];
    assert_eq!(skip_text_dispatch_memchr(&s2, 0), 100);
}


