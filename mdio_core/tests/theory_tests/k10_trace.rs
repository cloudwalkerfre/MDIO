//! trace harness 冒烟测试：Full scanner + FrameLog EventPlan，
//! 覆盖 Table / Comment / HtmlBlock / HtmlElement / RefDef。

use mdio_core::block::{entry_frame, entry_is_body_mut, entry_line, FrameKind};
use mdio_core::projection::State;

fn kind_name(k: FrameKind) -> &'static str {
    match k {
        FrameKind::BlockQuote => "BlockQuote",
        FrameKind::BulletList => "BulletList",
        FrameKind::OrderedList => "OrderedList",
        FrameKind::CodeFence => "CodeFence",
        FrameKind::HtmlBlock => "HtmlBlock",
        FrameKind::HtmlElement => "HtmlElement",
        FrameKind::Comment => "Comment",
        FrameKind::RefDef => "RefDef",
        FrameKind::Table => "Table",
        FrameKind::TableRow => "TableRow",
    }
}

fn actual(doc: &str) -> Vec<(usize, char, &'static str)> {
    let mut st = State::new(doc);
    let snap = st.snapshot();
    snap.frame_log
        .entries
        .iter()
        .map(|&e| {
            let line = entry_line(e) as usize;
            let op = if entry_is_body_mut(e) {
                'B'
            } else if e >> 63 != 0 {
                'Q'
            } else {
                'P'
            };
            (line, op, kind_name(entry_frame(e).kind()))
        })
        .collect()
}

fn cases() -> Vec<(&'static str, &'static str, Vec<(usize, char, &'static str)>)> {
    vec![
        (
            "table_one_row",
            "| h |\n|---|\n| x |\n",
            vec![(0, 'P', "Table"), (1, 'B', "Table"), (2, 'P', "TableRow")],
        ),
        (
            "table_two_rows",
            "| h |\n|---|\n| x |\n| y |\n",
            vec![
                (0, 'P', "Table"),
                (1, 'B', "Table"),
                (2, 'P', "TableRow"),
                (3, 'Q', "TableRow"),
                (3, 'P', "TableRow"),
            ],
        ),
        (
            "table_tentative",
            "| h |\n| x |\n",
            vec![
                (0, 'P', "Table"),
                (1, 'Q', "Table"),
                (1, 'P', "Table"),
            ],
        ),
        (
            "table_header_only",
            "| h |\n|---|\n",
            vec![(0, 'P', "Table"), (1, 'B', "Table")],
        ),
        ("comment_single", "<!-- c -->\n", vec![]),
        (
            "comment_multi",
            "<!--\nc\n-->\n",
            vec![(0, 'P', "Comment"), (2, 'Q', "Comment")],
        ),
        (
            "html_type6_blank",
            "<div>\nraw\n\n",
            vec![(0, 'P', "HtmlBlock"), (2, 'Q', "HtmlBlock")],
        ),
        (
            "html_type1_script",
            "<script>\nx\n</script>\n",
            vec![(0, 'P', "HtmlBlock"), (2, 'Q', "HtmlBlock")],
        ),
        (
            "html_type3_pi",
            "<?php\nx\n?>\n",
            vec![(0, 'P', "HtmlBlock"), (2, 'Q', "HtmlBlock")],
        ),
        (
            "html_type4_declaration",
            "<!A\nx\n",
            vec![(0, 'P', "HtmlBlock")],
        ),
        (
            "html_type4_doctype_single",
            "<!DOCTYPE html>\n",
            vec![],
        ),
        (
            "html_type5_cdata",
            "<![CDATA[\nx\n]]>\n",
            vec![(0, 'P', "HtmlBlock"), (2, 'Q', "HtmlBlock")],
        ),
        (
            "html_type7_custom",
            "<custom-tag>\nraw\n\n",
            vec![(0, 'P', "HtmlBlock"), (2, 'Q', "HtmlBlock")],
        ),
        (
            "html_element",
            "<div>\n<span>\n</span>\n",
            vec![
                (0, 'P', "HtmlBlock"),
                (1, 'P', "HtmlElement"),
                (2, 'Q', "HtmlElement"),
            ],
        ),
        (
            "html_element_nested",
            "<div>\n<section>\n<b>\n</b>\n</section>\n</div>\n",
            vec![
                (0, 'P', "HtmlBlock"),
                (1, 'P', "HtmlElement"),
                (2, 'P', "HtmlElement"),
                (3, 'Q', "HtmlElement"),
                (4, 'Q', "HtmlElement"),
            ],
        ),
        ("refdef_single", "[a]: /url\n", vec![(0, 'P', "RefDef")]),
        (
            "refdef_multiline",
            "[a]:\n/url\np\n",
            vec![(0, 'P', "RefDef")],
        ),
        (
            "refdef_title",
            "[a]: /url \"title\"\n",
            vec![(0, 'P', "RefDef")],
        ),
        (
            "refdef_multiline_title",
            "[a]:\n/url\n\"title\"\n",
            vec![(0, 'P', "RefDef")],
        ),
    ]
}

#[test]
fn k10_trace_smoke() {
    for (name, doc, want) in cases() {
        let got = actual(doc);
        assert_eq!(got, want, "case {name} doc={doc:?}");
    }
}
