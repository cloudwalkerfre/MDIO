// Hook Fusion Benchmark — 8 Scenarios
//
// Each scenario compares Shadow (gold standard: full replay on old+new snapshots)
// against Fusion (PathIndex HookIndex) and asserts identical dispatch sets.

use std::time::Instant;
use mdio_core::engine::{MdioEngine, MdioSnapshot};
use mdio_core::selector::{Step, FilterExpr, TypeName};
use mdio_core::projection::BlockType;
use mdio_core::hook::*;
use mdio_core::selector::{CmpOp, Value, SingleSlice};
use mdio_core::projection::BlockProj;

fn key_tail(key: &str) -> Vec<Step> {
    vec![
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(key.to_string()) }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ]
}

fn build_large_doc(sections: usize) -> String {
    let mut doc = String::new();
    for s in 0..sections {
        doc.push_str(&format!("# Section {}\n\n", s));
        doc.push_str(&format!("- host_{}: prod_value_{}\n", s, s));
        for i in 0..10 {
            doc.push_str(&format!("- key_{}: value_{}\n", i, i));
        }
        if s % 5 == 0 {
            doc.push_str("```rust\nfn example() {\n    println!(\"hello\");\n}\n```\n\n");
        }
        if s % 3 == 0 {
            doc.push_str(&format!("> Blockquote text in section {}\n> More quoted text\n\n", s));
        }
        doc.push_str("Some paragraph text for this section.\n\n");
    }
    doc
}

fn build_small_doc(sections: usize) -> String { build_large_doc(sections) }
fn make_snap(text: &str) -> MdioSnapshot { MdioEngine::new(text).make_snapshot() }
fn make_engine(text: &str) -> MdioEngine { MdioEngine::new(text) }

/// Extract ripple zone from engine after edit + make_snapshot.
fn ripple_from_engine(engine: &MdioEngine, fallback_line: usize) -> ((usize, usize), Vec<u64>) {
    match &engine.state.ripple_diff {
        Some(d) => ((d.ripple_line_start, d.ripple_line_end), d.ripple_frame_actions.clone()),
        None => ((fallback_line, fallback_line.saturating_add(1)), vec![]),
    }
}


struct ShadowHook { hook_id: u64, steps: Vec<Step>, is_always: bool }

fn shadow_detect(hooks: &[ShadowHook], old_snap: &MdioSnapshot, new_snap: &MdioSnapshot) -> Vec<u64> {
    let mut triggered = Vec::new();
    for hook in hooks {
        let old_r = old_snap.select_nodes(&hook.steps);
        let new_r = new_snap.select_nodes(&hook.steps);
        let mut is_changed = old_r.len() != new_r.len();
        if !is_changed {
            for (o_r, n_r) in old_r.iter().zip(new_r.iter()) {
                if old_snap.byte_slice(o_r.byte_range.clone()) != new_snap.byte_slice(n_r.byte_range.clone()) {
                    is_changed = true; break;
                }
            }
        }
        if is_changed { triggered.push(hook.hook_id); }
    }
    triggered
}

fn assert_same(fusion: &[HookEvent], shadow: &[u64], label: &str) {
    let mut f: Vec<u64> = fusion.iter().map(|e| e.hook_id).collect();
    let mut s = shadow.to_vec();
    f.sort(); s.sort();
    assert_eq!(f, s, "{}: fusion {:?} != shadow {:?}", label, f, s);
}

#[test]
fn s1_exact_hit() {
    let text = build_large_doc(500);
    let snap = make_snap(&text);
    let mut index = HookIndex::new();
    let mut shadow_hooks = Vec::new();
    for s in 0..500 {
        let steps = vec![Step::Drill(format!("Section {}", s)), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(format!("host_{}", s)) }), Step::Slice(vec![SingleSlice::Index(0)])];
        index.register(s as u64, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: s as u64, steps, is_always: false });
    }
    let mut engine = make_engine(&text);
    let target = "prod_value_250";
    let pos = text.find(target).unwrap();
    engine.edit(pos, pos + target.len(), "staging_value");
    let new_snap = engine.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&engine, 1);
    let write_steps = vec![Step::Drill("Section 250".to_string()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("host_250".to_string()) }), Step::Slice(vec![SingleSlice::Index(0)])];

    let t0 = Instant::now();
    let fusion_events = index.detect_selector_write(0, &write_steps, &new_snap, rs, re, &[]);
    let fus_us = t0.elapsed().as_micros();
    let t0 = Instant::now();
    let sh = shadow_detect(&shadow_hooks, &snap, &new_snap);
    let sh_us = t0.elapsed().as_micros();
    assert_same(&fusion_events, &sh, "S1");
    assert_eq!(sh, vec![250]);
    println!("S1 Exact: shadow={}μs fusion={}μs speedup={:.0}x", sh_us, fus_us, sh_us as f64 / fus_us.max(1) as f64);
}

#[test]
fn s2_unrelated_edit() {
    let text = build_large_doc(500);
    let snap = make_snap(&text);
    let mut index = HookIndex::new();
    let mut shadow_hooks = Vec::new();
    for s in 0..500 {
        let steps = vec![Step::Drill(format!("Section {}", s)), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("status".to_string()) }), Step::Slice(vec![SingleSlice::Index(0)])];
        index.register(s as u64, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: s as u64, steps, is_always: false });
    }
    let mut engine = make_engine(&text);
    let target = "prod_value_250";
    let pos = text.find(target).unwrap();
    engine.edit(pos, pos + target.len(), "new");
    let new_snap = engine.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&engine, 1);
    let write_steps = vec![Step::Drill("Section 250".to_string()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("host_250".to_string()) }), Step::Slice(vec![SingleSlice::Index(0)])];

    let fusion_events = index.detect_selector_write(0, &write_steps, &new_snap, rs, re, &[]);
    let sh = shadow_detect(&shadow_hooks, &snap, &new_snap);
    assert_same(&fusion_events, &sh, "S2");
    assert!(sh.is_empty());
}

#[test]
fn s3_zero_hit() {
    let text = build_large_doc(500);
    let snap = make_snap(&text);
    let mut index = HookIndex::new();
    let mut shadow_hooks = Vec::new();
    for s in 0..500 {
        let steps = vec![Step::Drill("NonExistent".to_string()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(format!("key_{}", s)) }), Step::Slice(vec![SingleSlice::Index(0)])];
        index.register(s as u64, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: s as u64, steps, is_always: false });
    }
    let mut engine = make_engine(&text);
    let target = "prod_value_250";
    let pos = text.find(target).unwrap();
    engine.edit(pos, pos + target.len(), "new");
    let new_snap = engine.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&engine, 1);
    let write_steps = vec![Step::Drill("Section 250".to_string()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("host_250".to_string()) }), Step::Slice(vec![SingleSlice::Index(0)])];

    let fusion_events = index.detect_selector_write(0, &write_steps, &new_snap, rs, re, &[]);
    let sh = shadow_detect(&shadow_hooks, &snap, &new_snap);
    assert_same(&fusion_events, &sh, "S3");
    assert!(sh.is_empty());
}

#[test]
fn s4_recursive_filter() {
    let text = build_small_doc(50);
    let snap = make_snap(&text);
    let mut index = HookIndex::new();
    let mut shadow_hooks = Vec::new();
    for i in 0..10 {
        let steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Code))];
        index.register(i, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: i, steps, is_always: false });
    }
    for i in 10..20 {
        let steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Paragraph))];
        index.register(i, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: i, steps, is_always: false });
    }
    let mut engine = make_engine(&text);
    let target = "println!";
    let pos = text.find(target).unwrap();
    engine.edit(pos, pos + target.len(), "eprintln!");
    let new_snap = engine.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&engine, 1);
    let write_steps = vec![Step::Drill("Section 0".to_string()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("host_0".to_string()) }), Step::Slice(vec![SingleSlice::Index(0)])];

    let fusion_events = index.detect_selector_write(0, &write_steps, &new_snap, rs, re, &[]);
    let sh = shadow_detect(&shadow_hooks, &snap, &new_snap);
    assert_same(&fusion_events, &sh, "S4");
}

#[test]
fn s5_deep_drill() {
    let mut text = String::new();
    for s in 0..500 {
        text.push_str(&format!("# Section {}\n\n", s));
        text.push_str(&format!("## Sub {}\n\n", s));
        text.push_str(&format!("### SubSub {}\n\n", s));
        text.push_str(&format!("- host_{}: value_{}\n", s, s));
    }
    let snap = make_snap(&text);
    let mut index = HookIndex::new();
    let mut shadow_hooks = Vec::new();
    for s in 0..100 {
        let steps = vec![Step::Drill(format!("Section {}", s)), Step::Drill(format!("Sub {}", s)),
            Step::Drill(format!("SubSub {}", s)), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(format!("host_{}", s)) }), Step::Slice(vec![SingleSlice::Index(0)])];
        index.register(s as u64, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: s as u64, steps, is_always: false });
    }
    let mut engine = make_engine(&text);
    let target = "value_50";
    let pos = text.find(target).unwrap();
    engine.edit(pos, pos + target.len(), "new_value");
    let new_snap = engine.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&engine, 1);
    let write_steps = vec![Step::Drill("Section 50".to_string()), Step::Drill("Sub 50".to_string()),
        Step::Drill("SubSub 50".to_string()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("host_50".to_string()) }), Step::Slice(vec![SingleSlice::Index(0)])];

    let fusion_events = index.detect_selector_write(0, &write_steps, &new_snap, rs, re, &[]);
    let sh = shadow_detect(&shadow_hooks, &snap, &new_snap);
    assert_same(&fusion_events, &sh, "S5");
    assert_eq!(sh, vec![50]);
}

#[test]
fn s6_crdt_write() {
    let text = build_large_doc(500);
    let snap = make_snap(&text);
    let mut index = HookIndex::new();
    let mut shadow_hooks = Vec::new();
    for i in 0..50 {
        let steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Code))];
        index.register(i, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: i, steps, is_always: false });
    }
    for i in 50..100 {
        let s = i - 50;
        let steps = vec![Step::Drill(format!("Section {}", s)), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(format!("host_{}", s)) }), Step::Slice(vec![SingleSlice::Index(0)])];
        index.register(i, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: i, steps, is_always: false });
    }
    let mut engine = make_engine(&text);
    let target = "println!";
    let pos = text.find(target).unwrap();
    engine.edit(pos, pos + target.len(), "eprintln!");
    let new_snap = engine.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&engine, 1);

    let fusion_events = index.detect_crdt_write(0, &new_snap, rs, re, &[]);
    let sh = shadow_detect(&shadow_hooks, &snap, &new_snap);
    assert_same(&fusion_events, &sh, "S6");
}

#[test]
fn s7_scaffold() {
    let text = build_small_doc(10);
    let snap = make_snap(&text);
    let mut index = HookIndex::new();
    let mut shadow_hooks = Vec::new();
    for s in 0..10 {
        let steps = vec![Step::Drill(format!("Section {}", s)), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(format!("host_{}", s)) }), Step::Slice(vec![SingleSlice::Index(0)])];
        index.register(s as u64, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: s as u64, steps, is_always: false });
    }
    let mut engine = make_engine(&text);
    let target = "prod_value_5";
    let pos = text.find(target).unwrap();
    engine.edit(pos, pos + target.len(), "new");
    let new_snap = engine.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&engine, 1);
    let write_steps = vec![Step::Drill("Section 5".to_string()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("host_5".to_string()) }), Step::Slice(vec![SingleSlice::Index(0)])];

    let fusion_events = index.detect_selector_write(0, &write_steps, &new_snap, rs, re, &[]);
    let sh = shadow_detect(&shadow_hooks, &snap, &new_snap);
    assert_same(&fusion_events, &sh, "S7");
}

#[test]
fn s8_mixed_1000() {
    let text = build_large_doc(500);
    let snap = make_snap(&text);
    let mut index = HookIndex::new();
    let mut shadow_hooks = Vec::new();

    for s in 0..500 {
        let steps = vec![Step::Drill(format!("Section {}", s)), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(format!("host_{}", s)) }), Step::Slice(vec![SingleSlice::Index(0)])];
        index.register(s as u64, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: s as u64, steps, is_always: false });
    }
    for i in 500..700 {
        let steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Code))];
        index.register(i, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: i, steps, is_always: false });
    }
    for i in 700..900 {
        let steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Paragraph))];
        index.register(i, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: i, steps, is_always: false });
    }
    for i in 900..1000 {
        let steps = vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Header(0)))];
        index.register(i, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap);
        shadow_hooks.push(ShadowHook { hook_id: i, steps, is_always: false });
    }

    let mut engine = make_engine(&text);
    let target = "println!";
    let pos = text.find(target).unwrap();
    engine.edit(pos, pos + target.len(), "eprintln!");
    let new_snap = engine.make_snapshot();
    let ((rs, re), _rfa) = ripple_from_engine(&engine, 1);
    let write_steps = vec![Step::Drill("Section 0".to_string()), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("host_0".to_string()) }), Step::Slice(vec![SingleSlice::Index(0)])];

    // Path A
    let fusion_events_a = index.detect_selector_write(0, &write_steps, &new_snap, rs, re, &[]);
    let sh = shadow_detect(&shadow_hooks, &snap, &new_snap);
    assert_same(&fusion_events_a, &sh, "S8 Path A");

    // Path B — separate index because one-shot hooks get consumed
    let mut index_b = HookIndex::new();
    let snap_b = make_snap(&text);
    for s in 0..500 {
        let steps = vec![Step::Drill(format!("Section {}", s)), Step::Wildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)), Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(format!("host_{}", s)) }), Step::Slice(vec![SingleSlice::Index(0)])];
        index_b.register(s as u64, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &snap_b);
    }
    for i in 500..700 {
        index_b.register(i, "", 0, &vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Code))], false, "cb", serde_json::json!({}), 0, &snap_b);
    }
    for i in 700..900 {
        index_b.register(i, "", 0, &vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Paragraph))], false, "cb", serde_json::json!({}), 0, &snap_b);
    }
    for i in 900..1000 {
        index_b.register(i, "", 0, &vec![Step::RecursiveWildcard, Step::Filter(FilterExpr::TypeCheck(TypeName::Header(0)))], false, "cb", serde_json::json!({}), 0, &snap_b);
    }
    let fusion_events_b = index_b.detect_crdt_write(0, &new_snap, rs, re, &[]);
    assert_same(&fusion_events_b, &sh, "S8 Path B");
}
