//! tree/attrs 冒烟测试：检查 Table / HtmlBlock / RefDef / Comment 的
//! CF 树与 attrs 投影面。

use mdio_core::projection::State;
use mdio_core::topology::ProjectionMode;

fn ast(doc: &str) -> String {
    let mut st = State::new(doc);
    let snap = st.snapshot();
    snap.to_ast_object_mode(ProjectionMode::Cf)
}

#[test]
fn k10_tree_attrs_smoke() {
    // Table: tb node, cols attr, tr child.
    let table = ast("| h |\n|---|\n| x |\n");
    assert!(table.contains("\"nt\":\"tb\""), "table: {table}");
    assert!(table.contains("\"cols\":\"h\""), "table cols: {table}");
    assert!(table.contains("\"nt\":\"tr\""), "table row: {table}");

    // HtmlBlock: hb node, tagname, raw body.
    let html = ast("<div>\nraw\n\n");
    assert!(html.contains("\"nt\":\"hb\""), "html: {html}");
    assert!(html.contains("\"t\":\"div\""), "html tag: {html}");
    assert!(html.contains("<div>"), "html raw open: {html}");
    assert!(html.contains("raw"), "html raw body: {html}");

    // HtmlElement appears in debug tree (AST hides it under hb raw).
    let mut he = State::new("<div>\n<span>\n</span>\n");
    let he_snap = he.snapshot();
    let dbg = he_snap.debug_block_tree();
    assert!(dbg.contains("html_block") || dbg.contains("HtmlBlock"), "he tree: {dbg}");
    assert!(dbg.contains("html_element") || dbg.contains("HtmlElement") || dbg.contains("span"), "he tree: {dbg}");

    // RefDef: rd node + label/url attrs.
    let rd = ast("[a]: /url\n");
    assert!(rd.contains("\"nt\":\"rd\""), "refdef: {rd}");
    assert!(rd.contains("\"t\":\"a\""), "refdef label: {rd}");
    assert!(rd.contains("\"u\":\"/url\""), "refdef url: {rd}");

    // RefDef title attr.
    let rd_title = ast("[a]: /url \"title\"\n");
    assert!(rd_title.contains("\"ti\":\"title\""), "refdef title: {rd_title}");

    // Table tentative: no tb node, paragraphs only.
    let tent = ast("| h |\n| x |\n");
    assert!(!tent.contains("\"nt\":\"tb\""), "tentative should not be tb: {tent}");
    assert!(tent.contains("\"nt\":\"p\""), "tentative p: {tent}");

    // Header-only table: tb, no tr child.
    let empty = ast("| h |\n|---|\n");
    assert!(empty.contains("\"nt\":\"tb\""), "header-only tb: {empty}");
    assert!(!empty.contains("\"nt\":\"tr\""), "header-only no tr: {empty}");

    // Nested HtmlElement raw body.
    let nested = ast("<div>\n<section>\n<b>\n</b>\n</section>\n</div>\n");
    assert!(nested.contains("<section>"), "nested section: {nested}");
    assert!(nested.contains("<b>"), "nested b: {nested}");

    // Comment: cm node.
    let cm = ast("<!--\nc\n-->\n");
    assert!(cm.contains("\"nt\":\"cm\""), "comment: {cm}");
}

fn count(hay: &str, needle: &str) -> usize {
    hay.match_indices(needle).count()
}

#[test]
fn k10_table_row_tree_smoke() {
    // Two data rows are two sibling tr nodes under one tb.
    let two = ast("| h |\n|---|\n| x |\n| y |\n");
    assert_eq!(count(&two, "\"nt\":\"tb\""), 1, "two rows tb: {two}");
    assert_eq!(count(&two, "\"nt\":\"tr\""), 2, "two rows tr: {two}");
    assert!(two.contains("\"ct\":\"x\\t\""), "row x: {two}");
    assert!(two.contains("\"ct\":\"y\\t\""), "row y: {two}");

    // Header-only: tb exists, no tr.
    let empty = ast("| h |\n|---|\n");
    assert_eq!(count(&empty, "\"nt\":\"tb\""), 1, "header-only tb: {empty}");
    assert_eq!(count(&empty, "\"nt\":\"tr\""), 0, "header-only tr: {empty}");

    // Tentative: neither tb nor tr.
    let tent = ast("| h |\n| x |\n");
    assert_eq!(count(&tent, "\"nt\":\"tb\""), 0, "tentative tb: {tent}");
    assert_eq!(count(&tent, "\"nt\":\"tr\""), 0, "tentative tr: {tent}");
}
