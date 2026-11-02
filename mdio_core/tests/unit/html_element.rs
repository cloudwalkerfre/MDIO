#[cfg(test)]
mod html_element_tests {
    use crate::block::{
        tokenize_html_line, HtmlLineToken,
        extract_html_tagname, extract_html_mark, extract_html_tagname_from_bytes,
    };

    // ── M0: extract functions ──

    #[test]
    fn test_extract_html_tagname_open() {
        assert_eq!(extract_html_tagname("<div>"), "div");
        assert_eq!(extract_html_tagname("<p class='x'>"), "p");
        assert_eq!(extract_html_tagname("  <section>"), "section");
    }

    #[test]
    fn test_extract_html_tagname_close() {
        assert_eq!(extract_html_tagname("</div>"), "div");
        assert_eq!(extract_html_tagname("</span>"), "span");
    }

    #[test]
    fn test_extract_html_tagname_self_close() {
        assert_eq!(extract_html_tagname("<br/>"), "br");
        assert_eq!(extract_html_tagname("<img src='x.png'/>"), "img");
    }

    #[test]
    fn test_extract_html_tagname_custom() {
        assert_eq!(extract_html_tagname("<invented_myself>"), "invented_myself");
        assert_eq!(extract_html_tagname("<my-component>"), "my-component");
    }

    #[test]
    fn test_extract_html_tagname_empty() {
        assert_eq!(extract_html_tagname(""), "");
        assert_eq!(extract_html_tagname("not a tag"), "");
    }

    #[test]
    fn test_extract_html_mark_basic() {
        assert!(extract_html_mark("<p>").starts_with("<p"));
        assert!(extract_html_mark("<div class='x'>").starts_with("<div"));
    }

    #[test]
    fn test_extract_html_tagname_from_bytes_basic() {
        assert_eq!(extract_html_tagname_from_bytes(b"<div>"), "div");
        assert_eq!(extract_html_tagname_from_bytes(b"<p class='x'>"), "p");
    }

    // ── M1: tokenizer ──

    fn assert_tokens(line: &str, expected: Vec<(&str, &str)>) {
        // expected: Vec<(kind, tag)> where kind is "Open"/"Close"/"Inline"
        let tokens = tokenize_html_line(line.as_bytes());
        let result: Vec<(&str, &str)> = tokens.iter().map(|t| match t {
            HtmlLineToken::Open { tag } => ("Open", tag.as_str()),
            HtmlLineToken::Close { tag } => ("Close", tag.as_str()),
            HtmlLineToken::Inline => ("Inline", ""),
        }).collect();
        let mut passed = true;
        if result.len() != expected.len() {
            passed = false;
        } else {
            for (r, e) in result.iter().zip(expected.iter()) {
                if r.0 != e.0 || (r.0 != "Inline" && r.1 != e.1) {
                    passed = false;
                    break;
                }
            }
        }
        if !passed {
            panic!("\n  input:    {:?}\n  expected: {:?}\n  got:      {:?}", line, expected, result);
        }
    }

    #[test]
    fn test_tokenizer_basic() {
        assert_tokens("<div>", vec![("Open", "div")]);
        assert_tokens("</div>", vec![("Close", "div")]);
        assert_tokens("<br>", vec![("Inline", "")]);
        assert_tokens("<img/>", vec![("Inline", "")]);
        assert_tokens("<br />", vec![("Inline", "")]);
    }

    #[test]
    fn test_tokenizer_attributes() {
        assert_tokens("<div class='x'>", vec![("Open", "div")]);
        assert_tokens("<img src='x.png'/>", vec![("Inline", "")]);
        // Same-line pairing: <a> and </a> paired → both Inline
        assert_tokens("<a href='>'>link</a>", vec![("Inline", ""), ("Inline", "")]);
    }

    #[test]
    fn test_tokenizer_pairing() {
        assert_tokens("<p>text</p>", vec![("Inline", ""), ("Inline", "")]);
        assert_tokens("<div><p></p></div>",
            vec![("Inline", ""), ("Inline", ""), ("Inline", ""), ("Inline", "")]);
    }

    #[test]
    fn test_tokenizer_mixed() {
        assert_tokens("</div><p>", vec![("Close", "div"), ("Open", "p")]);
        assert_tokens("<div><section>", vec![("Open", "div"), ("Open", "section")]);
    }

    #[test]
    fn test_tokenizer_void_elements() {
        for el in &["br", "hr", "img", "input", "meta", "link", "area", "base",
                     "col", "embed", "source", "track", "wbr", "keygen", "param"] {
            let input = format!("<{}>", el);
            let tokens = tokenize_html_line(input.as_bytes());
            assert!(!tokens.is_empty(), "void element <{}> should produce tokens", el);
            assert!(matches!(tokens[0], HtmlLineToken::Inline), "<{}> should be Inline", el);
        }
    }

    #[test]
    fn test_tokenizer_non_tag() {
        assert!(tokenize_html_line(b"a < b && c > d").is_empty());
        assert!(tokenize_html_line(b"not a < tag").is_empty());
    }

    #[test]
    fn test_tokenizer_comment() {
        assert!(tokenize_html_line(b"<!-- comment --><div>")[0].tag() == Some("div"));
    }

    #[test]
    fn test_tokenizer_quoted_close_tag() {
        let tokens = tokenize_html_line(b"<a title=\"</div>\">text</a>");
        // Should NOT see </div> as close — it's inside an attribute
        let has_close_div = tokens.iter().any(|t| matches!(t, HtmlLineToken::Close { tag } if tag == "div"));
        assert!(!has_close_div, "quoted </div> should not be a Close token");
    }

    #[test]
    fn test_tokenizer_custom_names() {
        assert_tokens("<my-component>", vec![("Open", "my-component")]);
        assert_tokens("<todo-item>", vec![("Open", "todo-item")]);
    }

    // Helper for tokenizer tests
    impl HtmlLineToken {
        fn tag(&self) -> Option<&str> {
            match self {
                HtmlLineToken::Open { tag } | HtmlLineToken::Close { tag } => Some(tag),
                HtmlLineToken::Inline => None,
            }
        }
    }
}
