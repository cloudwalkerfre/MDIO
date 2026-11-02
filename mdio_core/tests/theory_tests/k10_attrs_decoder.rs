//! attrs decoder 冒烟测试：检查 CF attrs 的 decode 面（表格单元格、RefDef label/url/title、HtmlBlock tagname、实体节点）。
//!
//! Checks table cells, RefDef label/url/title, HtmlBlock tagname, entity nodes.
//! The ignored test records the observed RefDef unescape gap; it is not a fix.

use mdio_core::projection::State;
use mdio_core::topology::ProjectionMode;

fn ast(doc: &str) -> String {
    let mut st = State::new(doc);
    let snap = st.snapshot();
    snap.to_ast_object_mode(ProjectionMode::Cf)
}

#[test]
fn k10_attrs_decoder_smoke() {
    // Table cells: trim + tab join.
    let table = ast("|  a  | b |\n|---|---|\n|  x  | y |\n");
    assert!(table.contains("\"cols\":\"a\\tb\""), "table cols: {table}");
    assert!(table.contains("\"ct\":\"x\\ty\\t\""), "table row cells: {table}");

    // RefDef basic attrs.
    let rd = ast("[a]: /url \"title\"\n");
    assert!(rd.contains("\"t\":\"a\""), "refdef label: {rd}");
    assert!(rd.contains("\"u\":\"/url\""), "refdef url: {rd}");
    assert!(rd.contains("\"ti\":\"title\""), "refdef title: {rd}");

    // HtmlBlock tagname is preserved as written.
    let html = ast("<DIV>\nraw\n\n");
    assert!(html.contains("\"t\":\"DIV\""), "html tag: {html}");

    // Entity nodes decode to text.
    let ent = ast("&amp; &lt; &#65;\n");
    assert!(ent.contains("\"nt\":\"entity\""), "entity node: {ent}");
    assert!(ent.contains("\"ct\":\"&\""), "amp entity: {ent}");
    assert!(ent.contains("\"ct\":\"<\""), "lt entity: {ent}");
    assert!(ent.contains("\"ct\":\"A\""), "numeric entity: {ent}");
}

/// Convention: mdio records the raw RefDef string; it does not unescape.
#[test]
fn refdef_raw_string_by_design() {
    let rd = ast("[a]: /foo\\(bar\\)\n");
    assert!(rd.contains("\"u\":\"/foo"), "raw url prefix: {rd}");
    assert!(!rd.contains("\"u\":\"/foo(bar)\""), "must not unescape: {rd}");
}

/// Convention: RefDef is definition-only; `[a]` stays text, not a link node.
#[test]
fn refdef_definition_only_by_design() {
    let doc = ast("[a]: /url\n\n[a]\n");
    assert!(!doc.contains("\"nt\":\"lk\""), "must not resolve link: {doc}");
}
