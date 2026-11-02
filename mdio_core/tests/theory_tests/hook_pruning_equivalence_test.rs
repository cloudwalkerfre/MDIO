// T1 hit_range 剪枝等价性测试。
// 每个 case：full = detect(..., structural_change=true)；
//           pruned = detect_ex(..., structural_change=实际 PL+FL 比较)。
// 纯内容编辑时两者必须事件恒等；结构编辑由 ex 的真实 flag 自然降级为全量。
use mdio_core::engine::{MdioEngine, MdioSnapshot};
use mdio_core::hook::{HookEvent, HookIndex};
use mdio_core::selector::{CmpOp, FilterExpr, SingleSlice, Step, TypeName, Value};
use mdio_core::projection::BlockProj;

fn bullet_steps(head: &str, tag: &str) -> Vec<Step> {
    vec![
        Step::Drill(head.into()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag,
            op: CmpOp::Eq,
            value: Value::String(tag.into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ]
}

fn register_case(idx: &mut HookIndex, snap: &MdioSnapshot) -> Vec<(u64, Vec<Step>)> {
    let mut hooks = Vec::new();
    let mut id = 1u64;
    let mut add = |steps: Vec<Step>| {
        idx.register(id, "", 0, &steps, true, "cb", serde_json::json!({}), 0, snap);
        hooks.push((id, steps));
        id += 1;
    };
    for (head, tag) in [("A", "a"), ("A", "b"), ("B", "x"), ("B", "y")] {
        add(bullet_steps(head, tag));
    }
    add(vec![Step::Drill("A".into()), Step::Drill("A2".into()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Slice(vec![SingleSlice::Index(0)])]);
    add(vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Code))]);
    add(vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Paragraph))]);
    add(vec![Step::Drill("Setext".into()), Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Paragraph)),
        Step::Slice(vec![SingleSlice::Index(0)])]);
    hooks
}

fn event_ids(events: &[HookEvent]) -> Vec<u64> {
    let mut v: Vec<u64> = events.iter().map(|e| e.hook_id).collect();
    v.sort();
    v
}

fn structural_change(old: &MdioSnapshot, new: &MdioSnapshot) -> bool {
    !mdio_core::block::entries_eq_ignoring_consume(
        &old.frame_log.entries, &new.frame_log.entries)
        || old.lines.as_ref() != new.lines.as_ref()
}

fn run_case(doc: &str, edits: &[(usize, usize, &str)]) {
    let initial = MdioEngine::new(doc).make_snapshot();
    let mut full = HookIndex::new();
    let mut pruned = HookIndex::new();
    let _hooks = register_case(&mut full, &initial);
    let _hooks2 = register_case(&mut pruned, &initial);

    let mut text = doc.to_string();
    for (edit_idx, &(s, e, rep)) in edits.iter().enumerate() {
        let old = MdioEngine::new(&text).make_snapshot();
        let mut eng = MdioEngine::new(&text);
        eng.edit(s, e, rep);
        let new = eng.make_snapshot();
        let d = eng.state.ripple_diff.as_ref().unwrap();
        let sc = structural_change(&old, &new);

        let full_ev = full.detect_crdt_write(0, &new, d.ripple_line_start, d.ripple_line_end, &d.ripple_frame_actions);
        // edit_start/edit_end = 编辑行区间(旧侧首字节行,新侧尾字节行+1)——与
        // session seam 的 edit_range 语义一致;传 (0,0) 会让 edit_range_allows_skip 恒
        // 剪(es=0,ee=0 ⟹ hL >= 0 恒真)——纯内容编辑时等价性被破坏。
        let els = old.line_of_byte(s.min(old.byte_len().saturating_sub(1)));
        let ele = new.line_of_byte(e.min(new.byte_len())).saturating_add(1);
        let pruned_ev = pruned.detect_crdt_write_ex(0, &new, d.ripple_line_start, d.ripple_line_end, &d.ripple_frame_actions, sc, els, ele, false, u64::MAX);
        assert_eq!(event_ids(&full_ev), event_ids(&pruned_ev),
            "doc={doc:?} edit#{edit_idx} ({s}..{e} -> {rep:?}) structural_change={sc}\nfull={:?}\npruned={:?}",
            full_ev.iter().map(|e| e.hook_id).collect::<Vec<_>>(),
            pruned_ev.iter().map(|e| e.hook_id).collect::<Vec<_>>());

        text = new.to_string();
    }
}

#[test]
fn content_edit_equivalence() {
    let doc = "# A\n- a: 1\n- b: 2\n\n## A2\n- c: 3\n\n# B\n- x: 10\n- y: 20\n\n```\ncode here\n```\n\nparagraph text\n";
    let e1 = (doc.find("1").unwrap(), doc.find("1").unwrap() + 1);
    let e2 = (doc.find("20").unwrap(), doc.find("20").unwrap() + 2);
    let e3 = (doc.find("code here").unwrap(), doc.find("code here").unwrap() + 4);
    let e4 = (doc.find("paragraph").unwrap(), doc.find("paragraph").unwrap() + 9);
    run_case(doc, &[
        (e1.0, e1.1, "alpha"),
        (e2.0, e2.1, "two"),
        (e3.0, e3.1, "code"),
        (e4.0, e4.1, "para"),
    ]);
}

#[test]
fn heading_rename_anchor_guard() {
    // 编辑祖先 Drill 锚点：即使 PL+FL 不变（纯标题文本重命名），
    // hit_range 在 ripple 之后的下游 hook 也不能被剪掉。
    let doc = "# A\n- a: 1\n\n## A2\n- c: 3\n";
    let a2 = doc.find("A2").unwrap();
    run_case(doc, &[(a2, a2 + 2, "Renamed")]);
}

#[test]
fn setext_scope_edit_equivalence() {
    let doc = "Setext\n=====\n\ncontent one\n\nmore\n";
    let pos = doc.find("content").unwrap();
    run_case(doc, &[(pos, pos + 7, "changed")]);
}

#[test]
fn structural_edit_uses_flag() {
    // 结构编辑：新增列表项（PL/FL 变化）。ex(true/false) 都不允许剪枝，
    // 事件应与 full 一致。
    let doc = "# A\n- a: 1\n";
    let insert = doc.find("- a: 1").unwrap();
    let old = MdioEngine::new(doc).make_snapshot();
    let mut full = HookIndex::new();
    let mut pruned = HookIndex::new();
    register_case(&mut full, &old);
    register_case(&mut pruned, &old);

    let mut eng = MdioEngine::new(doc);
    eng.edit(insert, insert, "- z: 9\n");
    let new = eng.make_snapshot();
    let d = eng.state.ripple_diff.as_ref().unwrap();
    let sc = structural_change(&old, &new);
    assert!(sc, "inserting a list item must be structural");
    let f = full.detect_crdt_write(0, &new, d.ripple_line_start, d.ripple_line_end, &d.ripple_frame_actions);
    let p = pruned.detect_crdt_write_ex(0, &new, d.ripple_line_start, d.ripple_line_end, &d.ripple_frame_actions, sc, insert, insert, false, u64::MAX);
    assert_eq!(event_ids(&f), event_ids(&p));
}
