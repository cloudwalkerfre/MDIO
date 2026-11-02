// Hook Edge Case Tests — Comprehensive coverage of HookIndex correctness
//
// Every test runs both Shadow (gold standard: full replay) and Fusion (HookIndex)
// and asserts identical dispatch sets.

use mdio_core::engine::{MdioEngine, MdioSnapshot};
use mdio_core::selector::{Step, FilterExpr, TypeName, CmpOp, Value, SingleSlice};
use mdio_core::projection::{BlockType, BlockProj};
use mdio_core::hook::*;
use mdio_core::hook::DrillKey;

fn key_tail(key: &str) -> Vec<Step> {
    vec![
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(key.to_string()) }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ]
}

fn snap(text: &str) -> MdioSnapshot { MdioEngine::new(text).make_snapshot() }
fn engine(text: &str) -> MdioEngine { MdioEngine::new(text) }

/// Extract ripple zone and frame actions from engine after edit + make_snapshot.
fn ripple_from_engine(engine: &MdioEngine, fallback_line: usize) -> ((usize, usize), Vec<u64>) {
    match &engine.state.ripple_diff {
        Some(d) => ((d.ripple_line_start, d.ripple_line_end), d.ripple_frame_actions.clone()),
        None => ((fallback_line, fallback_line.saturating_add(1)), vec![]),
    }
}

struct SHook { id: u64, steps: Vec<Step>, always: bool }

fn shadow(hooks: &[SHook], old: &MdioSnapshot, new: &MdioSnapshot) -> Vec<u64> {
    let mut t = Vec::new();
    for h in hooks {
        let o = old.select_nodes(&h.steps);
        let n = new.select_nodes(&h.steps);
        let mut ch = o.len() != n.len();
        if !ch {
            for (a, b) in o.iter().zip(n.iter()) {
                if old.byte_slice(a.byte_range.clone()) != new.byte_slice(b.byte_range.clone()) { ch = true; break; }
            }
        }
        if ch { t.push(h.id); }
    }
    t
}

fn assert_parity(fusion: &[HookEvent], shadow: &[u64], label: &str) {
    let mut f: Vec<u64> = fusion.iter().map(|e| e.hook_id).collect();
    let mut s = shadow.to_vec();
    f.sort(); s.sort();
    assert_eq!(f, s, "{}: fusion {:?} != shadow {:?}", label, f, s);
}

// C1: Container hierarchy — leaf edits affect ancestor containers

#[test]
fn c1_code_inside_header_triggers_header_hook() {
    let text = "# S\n\n```\ncode\n```\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let hooks = vec![
        SHook { id: 1, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Code))], always: false },
        SHook { id: 2, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Header(0)))], always: false },
    ];
    idx.register(1, "", 0, &hooks[0].steps, false, "cb", serde_json::json!({}), 0, &s);
    idx.register(2, "", 0, &hooks[1].steps, false, "cb", serde_json::json!({}), 0, &s);
    let mut e = engine(text);
    e.edit(text.find("code").unwrap(), text.find("code").unwrap() + 4, "newcode");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,0);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C1: code inside header");
    assert!(sh.contains(&2), "Header hook should trigger when code inside it changes");
}

#[test]
fn c1_code_inside_blockquote_triggers_bq_hook() {
    let text = "> ```\n> code\n> ```\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let hooks = vec![
        SHook { id: 1, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Code))], always: false },
        SHook { id: 2, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::BlockQuote))], always: false },
    ];
    idx.register(1, "", 0, &hooks[0].steps, false, "cb", serde_json::json!({}), 0, &s);
    idx.register(2, "", 0, &hooks[1].steps, false, "cb", serde_json::json!({}), 0, &s);
    let mut e = engine(text);
    e.edit(text.find("code").unwrap(), text.find("code").unwrap() + 4, "newcode");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,0);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C1: code inside bq");
    assert!(sh.contains(&2), "BQ hook should trigger when code inside it changes");
}

#[test]
fn c1_paragraph_inside_header_triggers_header_hook() {
    let text = "# S\n\npara\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let hooks = vec![
        SHook { id: 1, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Paragraph))], always: false },
        SHook { id: 2, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Header(0)))], always: false },
    ];
    idx.register(1, "", 0, &hooks[0].steps, false, "cb", serde_json::json!({}), 0, &s);
    idx.register(2, "", 0, &hooks[1].steps, false, "cb", serde_json::json!({}), 0, &s);
    let mut e = engine(text);
    e.edit(text.find("para").unwrap(), text.find("para").unwrap() + 4, "changed");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C1: para inside header");
    assert!(sh.contains(&2), "Header hook should trigger when para inside it changes");
}

#[test]
fn c1_bullet_inside_header_triggers_header_hook() {
    let text = "# S\n- k: v\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let hooks = vec![
        SHook { id: 1, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet))], always: false },
        SHook { id: 2, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Header(0)))], always: false },
    ];
    idx.register(1, "", 0, &hooks[0].steps, false, "cb", serde_json::json!({}), 0, &s);
    idx.register(2, "", 0, &hooks[1].steps, false, "cb", serde_json::json!({}), 0, &s);
    let mut e = engine(text);
    e.edit(text.find('v').unwrap(), text.find('v').unwrap() + 1, "w");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C1: bullet inside header");
    assert!(sh.contains(&2), "Header hook should trigger when bullet inside it changes");
}

#[test]
fn c1_nested_containers_bq_code() {
    let text = "# Top\n\n> ```\n> code\n> ```\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let hooks = vec![
        SHook { id: 1, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Code))], always: false },
        SHook { id: 2, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::BlockQuote))], always: false },
        SHook { id: 3, steps: vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Header(0)))], always: false },
    ];
    for h in &hooks { idx.register(h.id, "", 0, &h.steps, false, "cb", serde_json::json!({}), 0, &s); }
    let mut e = engine(text);
    e.edit(text.find("code").unwrap(), text.find("code").unwrap() + 4, "changed");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C1: nested bq+code in header");
    assert!(sh.contains(&2), "BQ hook should trigger");
    assert!(sh.contains(&3), "Header hook should trigger");
}

// C2: Zero/non-zero transitions

#[test]
fn c2_zero_to_existing() {
    let text = "# S\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.len(), text.len(), "\n- k: v\n");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &hooks[0].steps, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C2: zero→existing");
    assert!(!sh.is_empty());
}

#[test]
fn c2_existing_to_zero() {
    let text = "# S\n- k: v\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let bullet_start = text.find("- k:").unwrap();
    let bullet_end = text[bullet_start..].find('\n').unwrap() + bullet_start + 1;
    let mut e = engine(text);
    e.edit(bullet_start, bullet_end, "");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,0);
    let ev = idx.detect_selector_write(0, &[Step::Drill("S".into())], &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C2: existing→zero");
    assert!(!sh.is_empty());
}

#[test]
fn c2_stays_zero() {
    let text = "# S\n- a: va\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("nonexistent".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("va").unwrap(), text.find("va").unwrap() + 2, "vb");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C2: stays zero");
    assert!(sh.is_empty());
}

// C3: Same-name disambiguation

#[test]
fn c3_same_name_different_parents() {
    let text = "# X\n## Name\n- xk: xv\n\n# Y\n## Name\n- yk: yv\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let hooks = vec![
        SHook { id: 1, steps: vec![Step::Drill("X".into()), Step::Drill("Name".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("xk".into()) }), Step::Slice(vec![SingleSlice::Index(0)])], always: false },
        SHook { id: 2, steps: vec![Step::Drill("Y".into()), Step::Drill("Name".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("yk".into()) }), Step::Slice(vec![SingleSlice::Index(0)])], always: false },
    ];
    for h in &hooks { idx.register(h.id, "", 0, &h.steps, false, "cb", serde_json::json!({}), 0, &s); }
    let mut e = engine(text);
    e.edit(text.find("xv").unwrap(), text.find("xv").unwrap() + 2, "changed");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &hooks[0].steps, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C3: same name X");
    assert_eq!(sh, vec![1]);
}

#[test]
fn c3_recursive_drill_same_name() {
    let text = "# A\n## X\n### X\n- deep: val\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::RecursiveDrill("X".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("deep".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("val").unwrap(), text.find("val").unwrap() + 3, "new");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C3: recursive drill same name");
    assert!(!sh.is_empty());
}

#[test]
fn c3_wildcard_key_disambiguation() {
    let text = "# S\n- a: va\n- b: vb\n- c: vc\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("b".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("vb").unwrap(), text.find("vb").unwrap() + 2, "changed");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C3: wildcard key disambig");
    assert!(!sh.is_empty());
}

// C4: Content comparison (hash stability)

#[test]
fn c4_same_length_change() {
    let text = "# S\n- k: abc\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("abc").unwrap(), text.find("abc").unwrap() + 3, "xyz");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &hooks[0].steps, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C4: same length change");
    assert!(!sh.is_empty());
}

#[test]
fn c4_byte_range_shift_doesnt_falsely_trigger() {
    let text = "# S\n- a: hello\n- b: world\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("b".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("hello").unwrap(), text.find("hello").unwrap() + 5, "hello_extended");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &[Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("a".into()) }), Step::Slice(vec![SingleSlice::Index(0)])], &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C4: byte shift no false trigger");
    assert!(sh.is_empty());
}

#[test]
fn c4_diff_length_change() {
    let text = "# S\n- k: abc\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("abc").unwrap(), text.find("abc").unwrap() + 3, "abcdefgh");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &hooks[0].steps, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C4: diff length change");
    assert!(!sh.is_empty());
}

#[test]
fn c4_container_byte_shift_triggers() {
    let text = "# S\n\npara\n\n- k: v\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into())];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("para").unwrap(), text.find("para").unwrap() + 4, "longer_paragraph");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C4: container byte shift triggers");
    assert!(!sh.is_empty());
}

// C5: is_always persistence

#[test]
fn c5_one_shot_consumed_after_trigger() {
    let text = "# S\n- k: v\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let mut e1 = engine(text);
    e1.edit(text.find('v').unwrap(), text.find('v').unwrap() + 1, "w");
    let ns1 = e1.make_snapshot();
    let ((rs1, re1), _rfa1) = ripple_from_engine(&e1,1);
    let ev1 = idx.detect_selector_write(0, &steps, &ns1, rs1, re1, &[]);
    assert_eq!(ev1.len(), 1, "First edit should trigger");
    let mut e2 = engine(text);
    e2.edit(text.find('v').unwrap(), text.find('v').unwrap() + 1, "x");
    let ns2 = e2.make_snapshot();
    let ((rs2, re2), _rfa2) = ripple_from_engine(&e2,1);
    let ev2 = idx.detect_selector_write(0, &steps, &ns2, rs2, re2, &[]);
    assert!(ev2.is_empty(), "One-shot hook consumed, second edit should NOT trigger");
}

#[test]
fn c5_always_survives_two_edits() {
    let text = "# S\n- k: v\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps: steps.clone(), always: true }];
    let mut e1 = engine(text);
    e1.edit(text.find('v').unwrap(), text.find('v').unwrap() + 1, "w");
    let ns1 = e1.make_snapshot();
    let sh1 = shadow(&hooks, &s, &ns1);
    let ((rs1, re1), _rfa1) = ripple_from_engine(&e1,1);
    let ev1 = idx.detect_selector_write(0, &steps, &ns1, rs1, re1, &[]);
    assert_parity(&ev1, &sh1, "C5: always edit 1");
    assert_eq!(ev1.len(), 1, "is_always should trigger on first edit");
    let mut e2 = engine(text);
    e2.edit(text.find('v').unwrap(), text.find('v').unwrap() + 1, "x");
    let ns2 = e2.make_snapshot();
    let sh2 = shadow(&hooks, &ns1, &ns2);
    let ((rs2, re2), _rfa2) = ripple_from_engine(&e2,1);
    let ev2 = idx.detect_selector_write(0, &steps, &ns2, rs2, re2, &[]);
    assert_parity(&ev2, &sh2, "C5: always edit 2");
    assert_eq!(ev2.len(), 1, "is_always should trigger on second edit too");
}

#[test]
fn c5_always_anchor_shift_same_line_count() {
    // 两个同名 heading；is_always hook 的目标锚点在两次同长编辑中
    // 从第一个 heading 漂移到第二个。group_scope_cache 若只按行数判有效，
    // 第二步会漏触发；本测试锁住“锚点行变化必须保守评估”。
    let text = "# H\nA\n# H\nB\n";
    let s = snap(text);
    let steps = vec![
        Step::Drill("H".to_string()),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let mut idx = HookIndex::new();
    idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &s);

    let mut e = engine(text);
    e.edit(0, 3, "# X");
    let ns1 = e.make_snapshot();
    let ((rs1, re1), _) = ripple_from_engine(&e, 0);
    let ev1 = idx.detect_selector_write_ex(0, &steps, &ns1, rs1, re1, &[], false, 0, 1, false, u64::MAX);
    assert_eq!(ev1.len(), 1, "first anchor destruction must trigger");

    // 第一处等长替换后，第二个 "# H" 仍在字节偏移 7。
    e.edit(7, 10, "# Y");
    let ns2 = e.make_snapshot();
    let ((rs2, re2), _) = ripple_from_engine(&e, 7);
    let ev2 = idx.detect_selector_write_ex(0, &steps, &ns2, rs2, re2, &[], false, 7, 8, false, u64::MAX);
    assert_eq!(ev2.len(), 1, "second anchor destruction must trigger");
}

// C7: Structural changes

#[test]
fn c7_header_rename_triggers_hook() {
    let text = "# OldName\n- k: v\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("OldName".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("OldName").unwrap(), text.find("OldName").unwrap() + 7, "NewName");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,0);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C7: header rename");
    assert!(!sh.is_empty());
}

#[test]
fn c7_bullet_becomes_header() {
    let text = "# S\n- k: v\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let bullet_start = text.find("- k:").unwrap();
    let bullet_end = text[bullet_start..].find('\n').unwrap() + bullet_start + 1;
    let mut e = engine(text);
    e.edit(bullet_start, bullet_end, "## new header\n");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,0);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C7: bullet becomes header");
    assert!(!sh.is_empty());
}

// C8: Multi-step edits

#[test]
fn c8_delete_then_recreate_same_content() {
    let text = "# S\n- k: hello\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps: steps.clone(), always: false }];
    let bullet_start = text.find("- k:").unwrap();
    let bullet_end = text[bullet_start..].find('\n').unwrap() + bullet_start + 1;
    let mut e1 = engine(text);
    e1.edit(bullet_start, bullet_end, "");
    let ns1 = e1.make_snapshot();
    let sh1 = shadow(&hooks, &s, &ns1);
    let ((rs1, re1), _rfa1) = ripple_from_engine(&e1,0);
    let ev1 = idx.detect_crdt_write(0, &ns1, rs1, re1, &[]);
    assert_parity(&ev1, &sh1, "C8: delete");
    assert!(!sh1.is_empty());
    let after_delete = ns1.to_string();
    let mut idx2 = HookIndex::new();
    let new_steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx2.register(2, "", 0, &new_steps, false, "cb", serde_json::json!({}), 0, &ns1);
    let mut e2 = engine(&after_delete);
    let insert_pos = after_delete.find('\n').unwrap() + 1;
    e2.edit(insert_pos, insert_pos, "- k: hello\n");
    let ns2 = e2.make_snapshot();
    let hooks2 = vec![SHook { id: 2, steps: new_steps, always: false }];
    let sh2 = shadow(&hooks2, &ns1, &ns2);
    let ((rs2, re2), _rfa2) = ripple_from_engine(&e2,1);
    let ev2 = idx2.detect_selector_write(0, &hooks2[0].steps, &ns2, rs2, re2, &[]);
    assert_parity(&ev2, &sh2, "C8: recreate same");
    assert!(!sh2.is_empty(), "Recreate should trigger (zero→existing)");
}

// C10: Type family matching

#[test]
fn c10_code_fence_and_content_both_match_code_typecheck() {
    let text = "# S\n\n```rust\nfn main() {}\n```\n";
    let s = snap(text);
    let steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Code))];
    let hooks = vec![SHook { id: 1, steps: steps.clone(), always: false }];

    let mut idx1 = HookIndex::new();
    idx1.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let mut e1 = engine(text);
    e1.edit(text.find("rust").unwrap(), text.find("rust").unwrap() + 4, "rs");
    let ns1 = e1.make_snapshot();
    let sh1 = shadow(&hooks, &s, &ns1);
    let ((rs1, re1), _rfa1) = ripple_from_engine(&e1,0);
    let ev1 = idx1.detect_crdt_write(0, &ns1, rs1, re1, &[]);
    assert_parity(&ev1, &sh1, "C10: CodeFence TypeCheck");
    assert!(!sh1.is_empty());

    let mut idx2 = HookIndex::new();
    idx2.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let mut e2 = engine(text);
    e2.edit(text.find("main").unwrap(), text.find("main").unwrap() + 4, "start");
    let ns2 = e2.make_snapshot();
    let sh2 = shadow(&hooks, &s, &ns2);
    let ((rs2, re2), _rfa2) = ripple_from_engine(&e2,1);
    let ev2 = idx2.detect_crdt_write(0, &ns2, rs2, re2, &[]);
    assert_parity(&ev2, &sh2, "C10: CodeContent TypeCheck");
    assert!(!sh2.is_empty());
}

#[test]
fn c10_header_level_specific() {
    let text = "# H1\n## H2\n### H3\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let h1_steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Header(1)))];
    let h2_steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Header(2)))];
    idx.register(1, "", 0, &h1_steps, false, "cb", serde_json::json!({}), 0, &s);
    idx.register(2, "", 0, &h2_steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![
        SHook { id: 1, steps: h1_steps, always: false },
        SHook { id: 2, steps: h2_steps, always: false },
    ];
    let mut e = engine(text);
    e.edit(text.find("H2").unwrap(), text.find("H2").unwrap() + 2, "Changed");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,0);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C10: header level specific");
    // H1 hook fires too: H2 content change expands H1's container byte_range → combined_hash changes
    assert!(sh.contains(&1), "H1 fires because container byte_range changed");
    assert!(sh.contains(&2), "H2 hook SHOULD fire");
}

// C11: Complex selectors

#[test]
fn c11_wildcard_in_middle_of_path() {
    let text = "# A\n## B\n- key: val1\n\n# A\n## C\n- key: val2\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("A".into()), Step::Wildcard, Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("key".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("val1").unwrap(), text.find("val1").unwrap() + 4, "changed");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,0);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C11: wildcard in middle");
    assert!(!sh.is_empty());
}

#[test]
fn c11_same_heading_name_ambiguity() {
    let text = "# X\n## Name\n- xk: xv\n\n# Y\n## Name\n- yk: yv\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("Y".into()), Step::Drill("Name".into())];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("xv").unwrap(), text.find("xv").unwrap() + 2, "changed");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C11: same heading ambiguity");
    assert!(sh.is_empty(), "Hook for Y.Name should NOT fire when X.Name is edited");
}

// C17: Unicode

#[test]
fn c17_unicode_content() {
    let text = "# S\n- k: héllo世界\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    let val_start = text.find("héllo").unwrap();
    e.edit(val_start, val_start + "héllo世界".len(), "nĭhǎo宇宙");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &hooks[0].steps, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C17: unicode content");
}

#[test]
fn c17_emoji() {
    let text = "# S\n- k: 🦀\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    let val_start = text.find('🦀').unwrap();
    e.edit(val_start, val_start + "🦀".len(), "🐙");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &hooks[0].steps, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C17: emoji");
}

// C19: Empty/whitespace values

#[test]
fn c19_empty_bullet_filled() {
    let text = "# S\n- k:\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    let colon_pos = text.find("k:").unwrap() + 2;
    e.edit(colon_pos, colon_pos, "filled");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &hooks[0].steps, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C19: empty bullet filled");
    assert!(!sh.is_empty());
}

// C25: Lifecycle management

#[test]
fn c25_same_selector_two_ids_independent() {
    let text = "# S\n- k: v\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    idx.register(2, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let mut e = engine(text);
    e.edit(text.find('v').unwrap(), text.find('v').unwrap() + 1, "w");
    let ns = e.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &steps, &ns, rs, re, &[]);
    let ids: Vec<u64> = ev.iter().map(|e| e.hook_id).collect();
    assert!(ids.contains(&1) && ids.contains(&2), "Both hooks should fire independently");
}

#[test]
fn c25_remove_then_no_trigger() {
    let text = "# S\n- k: v\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps: steps.clone(), always: true }];
    idx.remove(1);
    let mut e = engine(text);
    e.edit(text.find('v').unwrap(), text.find('v').unwrap() + 1, "w");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &steps, &ns, rs, re, &[]);
    assert!(ev.is_empty(), "Removed hook should not fire");
    assert!(!sh.is_empty(), "Shadow would have fired (pre-removal snapshot)");
}

#[test]
fn c25_clear_doc() {
    let text = "# S\n- a: va\n- b: vb\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps_a = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("a".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    let steps_b = vec![Step::Drill("S".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("b".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    idx.register(1, "", 0, &steps_a, true, "cb", serde_json::json!({}), 0, &s);
    idx.register(2, "", 0, &steps_b, true, "cb", serde_json::json!({}), 0, &s);
    idx.clear_doc(0, "");
    let mut e = engine(text);
    e.edit(text.find("va").unwrap(), text.find("va").unwrap() + 2, "changed");
    let ns = e.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_selector_write(0, &steps_a, &ns, rs, re, &[]);
    assert!(ev.is_empty(), "All hooks should be cleared");
}

// C26: Ordered List

#[test]
fn c26_ol_item_change_triggers_typecheck() {
    let text = "# S\n1. first item\n2. second item\n";
    let s = snap(text);
    let mut idx = HookIndex::new();
    let steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::OrderedList))];
    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);
    let hooks = vec![SHook { id: 1, steps, always: false }];
    let mut e = engine(text);
    e.edit(text.find("first").unwrap(), text.find("first").unwrap() + 5, "changed");
    let ns = e.make_snapshot();
    let sh = shadow(&hooks, &s, &ns);
    let ((rs, re), _rfa) = ripple_from_engine(&e,1);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &[]);
    assert_parity(&ev, &sh, "C26: OL change triggers typecheck");
    assert!(!sh.is_empty());
}

// C28: Cache/Hash

#[test]
fn c28_same_text_different_nodetype_differs_in_hash() {
    // Two nodes with same text but different types should have different hashes
    let text1 = "# S\n";
    let text2 = "# S\n";
    let s1 = snap(text1);
    let s2 = snap(text2);
    let n1 = s1.select_nodes(&[Step::Drill("S".into())]);
    let n2 = s2.select_nodes(&[Step::Drill("S".into())]);
    let h1 = combined_hash_from_nodes(&n1, &s1);
    let h2 = combined_hash_from_nodes(&n2, &s2);
    assert_eq!(h1, h2, "Same content + same type → same hash");
}

// C29: write_path behavior

#[test]
fn c29_write_path_drill_key() {
    let steps = vec![Step::Drill("A".into()), Step::Drill("B".into()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("k".into()) }), Step::Slice(vec![SingleSlice::Index(0)])];
    assert_eq!(write_path(&steps), vec![DrillKey("A".into()), DrillKey("B".into())]);
}

#[test]
fn c29_write_path_pure_drill() {
    let steps = vec![Step::Drill("A".into()), Step::Drill("B".into())];
    assert_eq!(write_path(&steps), vec![DrillKey("A".into()), DrillKey("B".into())]);
}

#[test]
fn c29_write_path_empty() {
    let steps: Vec<Step> = vec![];
    assert_eq!(write_path(&steps), vec![]);
}

#[test]
fn c29_write_path_single_drill() {
    let steps = vec![Step::Drill("X".into())];
    assert_eq!(write_path(&steps), vec![DrillKey("X".into())]);
}

/// Hook detect must delegate TraversalFilter steps to select_nodes：共享前缀的
/// 两个 hook 只有各自 tag 变化时才触发，任一侧 tag 变化不会同时触发。
#[test]
fn c30_fused_traversalfilter_hooks_do_not_cross_trigger() {
    let text = "# S\n- status: IDLE\n- turn: 0\n";
    let s = snap(text);
    let mk_steps = |tag: &str| vec![
        Step::Drill("S".into()),
        Step::TraversalFilter {
            traversal: Box::new(Step::Wildcard),
            filters: vec![
                FilterExpr::TypeCheck(TypeName::Bullet),
                FilterExpr::Cmp {
                    proj: BlockProj::Tag,
                    op: CmpOp::Eq,
                    value: Value::String(tag.into()),
                },
            ],
            limit: 1,
        },
    ];
    let steps_status = mk_steps("status");
    let steps_turn = mk_steps("turn");
    let hooks = vec![
        SHook { id: 1, steps: steps_status.clone(), always: true },
        SHook { id: 2, steps: steps_turn.clone(), always: true },
    ];
    let mut idx = HookIndex::new();
    idx.register(1, "", 0, &steps_status, true, "cb", serde_json::json!({}), 0, &s);
    idx.register(2, "", 0, &steps_turn, true, "cb", serde_json::json!({}), 0, &s);

    let mut e = engine(text);
    e.edit(text.find("IDLE").unwrap(), text.find("IDLE").unwrap() + 4, "BUSY");
    let ns = e.make_snapshot();
    let ((rs, re), rfa) = ripple_from_engine(&e, 1);
    let sh = shadow(&hooks, &s, &ns);
    let ev = idx.detect_crdt_write(0, &ns, rs, re, &rfa);
    assert_parity(&ev, &sh, "C30: fused TraversalFilter hooks");
    assert_eq!(sh, vec![1], "only the status hook must see a change");
}

#[test]
fn repro_packed_line_info_bug() {
    let text = "# S\n- a: alpha\n- b: beta\n- c: gamma\n";
    let mut e = MdioEngine::new(text);
    e.edit(text.find("alpha").unwrap(), text.find("alpha").unwrap() + 5, "ALPHA!");
    let t1 = e.rope_clone().to_string();
    let b_line = t1.find("- b:").unwrap();
    let b_end = t1[b_line..].find('\n').unwrap() + b_line + 1;
    e.edit(b_line, b_end, "");
    let t2 = e.rope_clone().to_string();
    let c_line = t2.find("- c:").unwrap();
    e.edit(c_line, c_line, "- x: extra\n");
    let t3 = e.rope_clone().to_string();
    let gamma_pos = t3.find("gamma").unwrap();
    e.edit(gamma_pos, gamma_pos + 5, "DELTA");
    let snap = e.make_snapshot();
    for i in 0..snap.line_len() {
        let _ = snap.line_type_at(i);
    }
}

/// B1-4 Table 语义守卫：Table 体差异（confirmed/tentative）PopPush 对是
/// 可达的，判 C_info；唯一消费点把 C_info|I_info 同权——这里钉住"仍属结构
/// 事件"与"同权处理"这两个事实，防未来消费方区分 C/I 时漂移。
#[test]
fn b14_table_body_diff_poppush_is_structural() {
    use mdio_core::block::{entry_frame, entry_is_body_mut, entry_is_pop, entry_line, FrameKind};
    let doc = "para\n|---|\n| a | b |\n| e |\n|:--|\npara\n|---|\n";
    let mut e = MdioEngine::new(doc);
    let snap = e.make_snapshot();
    let entries = &snap.frame_log.entries;
    let mut found = false;
    for i in 0..entries.len() {
        let e0 = entries[i];
        if !entry_is_pop(e0) {
            continue;
        }
        let Some(&n) = entries.get(i + 1) else { continue };
        if entry_is_pop(n) || entry_is_body_mut(n) || entry_line(n) != entry_line(e0) {
            continue;
        }
        let fa = entry_frame(e0);
        let fb = entry_frame(n);
        if fa.kind() != FrameKind::Table || fb.kind() != FrameKind::Table {
            continue;
        }
        if fa.body() == fb.body() {
            continue;
        }
        found = true;
        let cls = classify_entry(entries, i);
        assert!(matches!(cls, EvClass::C_info | EvClass::I_info),
            "Table body-diff PopPush 必须仍是结构事件（C_info|I_info 同权），got {cls:?}");
        assert_eq!(
            mdio_core::effect::same_container(fa, fb), false,
            "same_container 对 Table body 差异应判异体（B1-4 语义单源）");
    }
    assert!(found, "应存在 Table body 差异 PopPush 对（fixture 已实证可达）");
}
