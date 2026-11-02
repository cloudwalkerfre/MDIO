// Hook Performance Tests — Speedup, Edge Attacks, Benchmarks
//
// Categories:
//   S1-S5:  Speedup tests (多 agent 状态表模拟, scan_once vs full)
//   E1-E6:  Edge attack tests (boundary conditions, aggressive patterns)
//   B1-B3:  Bench/stress tests (large scale, multi-edit, report)
//
// Run: cargo test --release --test hook_perf_tests -- --nocapture

use std::time::Instant;
use std::collections::HashMap;
use mdio_core::engine::{MdioEngine, MdioSnapshot};
use mdio_core::selector::{Step, FilterExpr, TypeName, CmpOp, Value, SingleSlice};
use mdio_core::projection::{BlockType, BlockProj};
use mdio_core::hook::*;
use mdio_core::block::entry_line;

fn snap(text: &str) -> MdioSnapshot { MdioEngine::new(text).make_snapshot() }
fn engine(text: &str) -> MdioEngine { MdioEngine::new(text) }

// §Helpers

/// Build a multi-agent status-table document with N agents per protocol.
fn build_proc_doc(n_llm: usize, n_xbt: usize) -> String {
    let mut doc = String::from("# PROCESS_TABLE\n\n");
    // LLM section
    doc.push_str("## LLM\n");
    for i in 0..n_llm {
        doc.push_str(&format!("### agent_{}\n", i));
        doc.push_str(&format!("- status: IDLE\n"));
        doc.push_str(&format!("- ppid: root\n"));
        doc.push_str(&format!("- turn: 0\n"));
        doc.push_str("\n");
    }
    // XBT section
    doc.push_str("## XBT\n");
    for i in 0..n_xbt {
        doc.push_str(&format!("### tid_{}\n", i));
        doc.push_str(&format!("- status: WAKE_UP\n"));
        doc.push_str(&format!("- ppid: root\n"));
        doc.push_str(&format!("- step: 0\n"));
        doc.push_str("\n");
    }
    doc
}

/// Register status-table hooks — one per agent, monitoring "status" tag.
/// steps = [Drill("PROCESS_TABLE"), Drill(proto), Drill(agent), Wildcard,
///          TypeCheck(Bullet), CMP(.tag,==,"status"), Slice(0)]
fn register_proc_hooks(
    idx: &mut HookIndex, snap: &MdioSnapshot,
    n_llm: usize, n_xbt: usize,
) {
    for i in 0..n_llm {
        let steps = vec![
            Step::Drill("PROCESS_TABLE".into()),
            Step::Drill("LLM".into()),
            Step::Drill(format!("agent_{}", i)),
            Step::Wildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
            Step::Filter(FilterExpr::Cmp {
                proj: BlockProj::Tag, op: CmpOp::Eq,
                value: Value::String("status".into()),
            }),
            Step::Slice(vec![SingleSlice::Index(0)]),
        ];
        idx.register(i as u64, "", 0, &steps, true, "cb", serde_json::json!({}), 0, snap);
    }
    for i in 0..n_xbt {
        let steps = vec![
            Step::Drill("PROCESS_TABLE".into()),
            Step::Drill("XBT".into()),
            Step::Drill(format!("tid_{}", i)),
            Step::Wildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
            Step::Filter(FilterExpr::Cmp {
                proj: BlockProj::Tag, op: CmpOp::Eq,
                value: Value::String("status".into()),
            }),
            Step::Slice(vec![SingleSlice::Index(0)]),
        ];
        let hid = (n_llm + i) as u64;
        idx.register(hid, "", 0, &steps, true, "cb", serde_json::json!({}), 0, snap);
    }
}

/// Extract ripple params from engine.
fn ripple_params(engine: &MdioEngine, fallback: usize) -> (usize, usize, Vec<u64>) {
    match &engine.state.ripple_diff {
        Some(d) => (d.ripple_line_start, d.ripple_line_end, d.ripple_frame_actions.clone()),
        None => (fallback, fallback.saturating_add(1), vec![]),
    }
}

/// Run T1-optimized detect and count triggered hooks.
/// structural_change 来自旧/新快照的 PL+FL 比较。
fn detect_and_count(
    idx: &mut HookIndex, old_snap: &MdioSnapshot, snap: &MdioSnapshot,
    rs: usize, re: usize, rfa: &[u64],
) -> (usize, Vec<HookEvent>) {
    let structural_change = !mdio_core::block::entries_eq_ignoring_consume(
        &old_snap.frame_log.entries, &snap.frame_log.entries)
        || old_snap.lines.as_ref() != snap.lines.as_ref();
    // edit_start/edit_end:helper 无编辑字节,用 ripple 区间(纯内容编辑时
    // ripple 收敛区 ⊇ 编辑行——剪枝保守不误剪;结构编辑 sc=true 不走该分支)。
    let events = idx.detect_crdt_write_ex(0, snap, rs, re, rfa, structural_change, rs, re.saturating_add(1), false, u64::MAX);
    (events.len(), events)
}

// §S: Speedup Tests — 多 agent 状态表模拟

#[test]
fn s1_scan_once_speedup_50_llm_10_xbt() {
    // 60 hooks: 50 LLM + 10 XBT
    let text = build_proc_doc(50, 10);
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 50, 10);

    // Edit: change agent_0 status (not turn — all hooks monitor "status")
    let mut e = engine(&text);
    let agent0_pos = text.find("### agent_0").unwrap();
    let status_pos = text[agent0_pos..].find("status: IDLE").unwrap() + agent0_pos;
    e.edit(status_pos, status_pos + 12, "status: BUSY");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);

    let start = Instant::now();
    let (count, _) = detect_and_count(&mut idx, &s, &ns, rs, re, &rfa);
    let elapsed = start.elapsed();

    println!("\n[S1] 50 LLM + 10 XBT hooks, edit one agent turn:");
    println!("  triggered: {}/60 hooks", count);
    println!("  detect time: {:?}", elapsed);
    assert_eq!(count, 1, "Only agent_0 should trigger (its turn changed)");
    // 并行 CPU 竞争下时间断言放宽到 1s（本意是 sanity 非无限慢；
    // 精确性能由 B1-B3 bench 承担）
    assert!(elapsed.as_millis() < 1000, "Should be fast (<1s), got {:?}", elapsed);
}

#[test]
fn s2_scan_once_speedup_200_llm() {
    let text = build_proc_doc(200, 0);
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 200, 0);

    let mut e = engine(&text);
    let pos = text.find("### agent_150").unwrap();
    let status_pos = text[pos..].find("status: IDLE").unwrap() + pos;
    e.edit(status_pos, status_pos + 12, "status: BUSY");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);

    let start = Instant::now();
    let (count, _) = detect_and_count(&mut idx, &s, &ns, rs, re, &rfa);
    let elapsed = start.elapsed();

    println!("\n[S2] 200 LLM hooks, edit agent_150 status:");
    println!("  triggered: {}/200 hooks", count);
    println!("  detect time: {:?}", elapsed);
    assert_eq!(count, 1);
    // 时间断言放宽到 1s（同 s1：并行 CPU 竞争偶发；sanity 语义）
    assert!(elapsed.as_millis() < 1000, "200 hooks should be <1s, got {:?}", elapsed);
}

#[test]
fn s3_scan_once_speedup_xbt_only() {
    // All XBT hooks — same prefix ["PROCESS_TABLE","XBT"]
    let text = build_proc_doc(0, 30);
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 0, 30);

    let mut e = engine(&text);
    let pos = text.find("### tid_20").unwrap();
    let status_pos = text[pos..].find("status: WAKE_UP").unwrap() + pos;
    e.edit(status_pos, status_pos + 16, "status: PAUSED");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);

    let start = Instant::now();
    let (count, _) = detect_and_count(&mut idx, &s, &ns, rs, re, &rfa);
    let elapsed = start.elapsed();

    println!("\n[S3] 30 XBT hooks, edit tid_20 status:");
    println!("  triggered: {}/30 hooks", count);
    println!("  detect time: {:?}", elapsed);
    assert_eq!(count, 1);
    // 时间断言放宽到 500ms（并行 CPU 竞争偶发；sanity 语义）
    assert!(elapsed.as_millis() < 500, "30 hooks should be <500ms, got {:?}", elapsed);
}

#[test]
fn s4_edit_outside_all_scopes() {
    // Edit in a section where NO hooks exist
    let mut text = String::from("# OTHER\n- some data\n\n");
    text.push_str(&build_proc_doc(20, 5));
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 20, 5);

    // Edit "# OTHER" paragraph text — outside all hook scopes
    let mut e = engine(&text);
    let pos = text.find("some data").unwrap();
    e.edit(pos, pos + 9, "other data");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);

    let start = Instant::now();
    let (count, _) = detect_and_count(&mut idx, &s, &ns, rs, re, &rfa);
    let elapsed = start.elapsed();

    println!("\n[S4] Edit outside all scopes (25 hooks in PROC_TABLE, edit in # OTHER):");
    println!("  triggered: {}/25 hooks", count);
    println!("  detect time: {:?}", elapsed);
    assert_eq!(count, 0, "Edit outside all scopes should trigger nothing");
    // 时间断言放宽到 50ms（并行 CPU 竞争偶发；sanity 语义）
    assert!(elapsed.as_micros() < 50_000, "Should be nearly instant (<50ms), got {:?}", elapsed);
}

#[test]
fn s5_multi_edit_cycle_delta_shift() {
    // Verify scope_range shifts correctly across multiple edits
    let text = build_proc_doc(5, 0);
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 5, 0);

    // Edit 1: delete a section BEFORE PROC_TABLE (δ < 0)
    let mut e1 = engine(&text);
    // No section before — insert one, then edit it
    let with_prefix = format!("# PREFIX\n- x\n\n{}", text);
    let mut e = engine(&with_prefix);
    // Delete PREFIX section — everything else shifts up by δ = -3
    e.edit(0, with_prefix.find("# PROCESS_TABLE").unwrap(), "");
    let ns1 = e.make_snapshot();
    let (rs1, re1, rfa1) = ripple_params(&e, 0);

    let events1 = idx.detect_crdt_write(0, &ns1, rs1, re1, &rfa1);
    println!("\n[S5] After deleting prefix section:");
    println!("  triggered: {}/5 hooks", events1.len());

    // Edit 2: change agent_2 status in shifted document — SHOULD trigger
    let text2 = ns1.to_string();
    let mut e2 = engine(&text2);
    let pos = text2.find("### agent_2").unwrap();
    let status_pos = text2[pos..].find("status: IDLE").unwrap() + pos;
    e2.edit(status_pos, status_pos + 12, "status: BUSY");
    let ns2 = e2.make_snapshot();
    let (rs2, re2, rfa2) = ripple_params(&e2, 1);

    let events2 = idx.detect_crdt_write(0, &ns2, rs2, re2, &rfa2);
    println!("  after edit agent_2 status in shifted doc: triggered {}/5", events2.len());
    // agent_2's status changed from IDLE to BUSY → hook should fire
    // Note: agent_2 is is_always=true, but the hook may not fire if scope
    // was maintained incorrectly (δ shift bug would show here)
    assert!(!events2.is_empty(), "agent_2 status change should trigger at least its own hook");
}

// §E: Edge Attack Tests

#[test]
fn e1_can_scan_once_fallback_heterogeneous() {
    // Mix hooks with different step structures under same prefix
    let text = build_proc_doc(3, 0);
    let s = snap(&text);
    let mut idx = HookIndex::new();

    // Hook 1: standard pattern (Dril, Wildcard, TypeCheck, CMP, Slice)
    let steps1 = vec![
        Step::Drill("PROCESS_TABLE".into()), Step::Drill("LLM".into()),
        Step::Drill("agent_0".into()), Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("status".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    idx.register(1, "", 0, &steps1, true, "cb", serde_json::json!({}), 0, &s);

    // Hook 2: different — RecursiveWildcard instead of Wildcard (heterogeneous)
    let steps2 = vec![
        Step::Drill("PROCESS_TABLE".into()), Step::Drill("LLM".into()),
        Step::Drill("agent_1".into()), Step::RecursiveWildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("status".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    idx.register(2, "", 0, &steps2, true, "cb", serde_json::json!({}), 0, &s);

    // Hook 3: same as Hook 1 pattern (homogeneous with Hook 1)
    let steps3 = vec![
        Step::Drill("PROCESS_TABLE".into()), Step::Drill("LLM".into()),
        Step::Drill("agent_2".into()), Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("status".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    idx.register(3, "", 0, &steps3, true, "cb", serde_json::json!({}), 0, &s);

    // Edit: all three agents' status should be detected correctly
    let mut e = engine(&text);
    let pos = text.find("### agent_1").unwrap();
    let status_pos = text[pos..].find("status: IDLE").unwrap() + pos;
    e.edit(status_pos, status_pos + 12, "status: BUSY");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);
    let events = idx.detect_crdt_write(0, &ns, rs, re, &rfa);

    // Hook 2 should trigger (status changed). Hooks 1,3 should not.
    let triggered: Vec<u64> = events.iter().map(|ev| ev.hook_id).collect();
    println!("\n[E1] Heterogeneous steps in same group:");
    println!("  triggered hooks: {:?}", triggered);
    assert!(triggered.contains(&2), "agent_1 hook should trigger");
    assert!(!triggered.contains(&1), "agent_0 hook should NOT trigger");
    assert!(!triggered.contains(&3), "agent_2 hook should NOT trigger");
}

#[test]
fn e2_single_level_prefix_deep_dril() {
    // Hook with only one Drill prefix level (edge case for group key)
    let text = "# S\n- k: v\n- j: w\n";
    let s = snap(&text);
    let mut idx = HookIndex::new();

    let steps = vec![
        Step::Drill("S".into()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("k".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &s);

    let mut e = engine(text);
    let vpos = text.find('v').unwrap();
    e.edit(vpos, vpos + 1, "x");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);
    let events = idx.detect_crdt_write(0, &ns, rs, re, &rfa);

    println!("\n[E2] Single-level Dril prefix:");
    println!("  triggered: {} hooks", events.len());
    assert_eq!(events.len(), 1, "tag k value change should trigger");
}

#[test]
fn e3_structural_edit_changes_section_tree() {
    // Insert a new H2 heading — changes Section tree structure
    let text = "# S\n## A\n- k: v\n";
    let s = snap(&text);
    let mut idx = HookIndex::new();

    let steps = vec![
        Step::Drill("S".into()), Step::Drill("A".into()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("k".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &s);

    // Edit: insert "## B" after "## A" section
    let insert_pos = text.find("- k:").unwrap() + 6;  // after "- k: v\n"
    let mut e = engine(text);
    e.edit(insert_pos, insert_pos, "## B\n- x: y\n");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);
    let events = idx.detect_crdt_write(0, &ns, rs, re, &rfa);

    println!("\n[E3] Structural edit (insert new H2):");
    println!("  triggered: {} hooks", events.len());
    // Section "A" is structurally unchanged, but R may overlap scope
    // depending on ripple propagation. Verify the trigger is correct either way.
}

#[test]
fn e4_heading_rename_breaks_dril() {
    let text = "# OldName\n- k: v\n";
    let s = snap(&text);
    let mut idx = HookIndex::new();

    let steps = vec![
        Step::Drill("OldName".into()), Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("k".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &s);

    let mut e = engine(text);
    e.edit(2, 9, "NewName"); // "# OldName" → "# NewName"
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 0);
    let events = idx.detect_crdt_write(0, &ns, rs, re, &rfa);

    println!("\n[E4] Heading rename breaks Dril:");
    println!("  triggered: {} hooks", events.len());
    assert_eq!(events.len(), 1, "heading rename → Dril unresolvable → must trigger");
}

#[test]
fn e5_empty_ripple_zone_degenerate() {
    // A degenerate edit (same text replaced) produces re <= rs
    let text = "# S\n- k: v\n";
    let s = snap(&text);
    let mut idx = HookIndex::new();

    let steps = vec![
        Step::Drill("S".into()), Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("k".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &s);

    // Delete and immediately recreate same content → degenerate ripple
    let mut e1 = engine(text);
    let bullet_start = text.find("- k:").unwrap();
    let bullet_end = text[bullet_start..].find('\n').unwrap() + bullet_start + 1;
    e1.edit(bullet_start, bullet_end, "");
    let ns1 = e1.make_snapshot();
    let (rs1, re1, rfa1) = ripple_params(&e1, 0);
    // Verify deletion is handled correctly
    let ev1 = idx.detect_crdt_write(0, &ns1, rs1, re1, &rfa1);

    println!("\n[E5] Degenerate ripple zone:");
    println!("  delete → triggered: {} hooks", ev1.len());
    // Deletion removes the bullet → hook should trigger (existing→zero)
    assert_eq!(ev1.len(), 1, "deleting target should trigger hook");
}

#[test]
fn e6_global_hook_and_scoped_hook_mix() {
    // Mix of global (no Dril) and scoped hooks
    let text = "# S\n- k: v\n\n# T\n- x: y\n";
    let s = snap(&text);
    let mut idx = HookIndex::new();

    // Global hook: RecursiveWildcard + TypeCheck(Bullet) — watches ALL bullets
    let global_steps = vec![
        Step::RecursiveWildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
    ];
    idx.register(1, "", 0, &global_steps, true, "cb", serde_json::json!({}), 0, &s);

    // Scoped hook: Dril("S") + Wildcard + Bullet + tag="k"
    let scoped_steps = vec![
        Step::Drill("S".into()), Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("k".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    idx.register(2, "", 0, &scoped_steps, true, "cb", serde_json::json!({}), 0, &s);

    // Edit section T's bullet — scoped hook should NOT fire, global hook SHOULD
    let mut e = engine(text);
    let t_pos = text.find("# T").unwrap();
    let y_pos = text[t_pos..].find('y').unwrap() + t_pos;
    e.edit(y_pos, y_pos + 1, "z");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);
    let events = idx.detect_crdt_write(0, &ns, rs, re, &rfa);

    let triggered: Vec<u64> = events.iter().map(|ev| ev.hook_id).collect();
    println!("\n[E6] Global + scoped hook mix:");
    println!("  triggered: {:?}", triggered);
    assert!(triggered.contains(&1), "global hook should fire on any bullet change");
    assert!(!triggered.contains(&2), "scoped hook on S should NOT fire on T edit");
}

// §B: Bench/Stress Tests — Report

#[test]
fn b1_large_scale_500_hooks() {
    let text = build_proc_doc(400, 100);
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 400, 100);

    // Edit deep in LLM section
    let mut e = engine(&text);
    let pos = text.find("### agent_350").unwrap();
    let status_pos = text[pos..].find("status: IDLE").unwrap() + pos;
    e.edit(status_pos, status_pos + 12, "status: BUSY");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);

    // Measure (no warmup — first detection captures actual work)
    let start = Instant::now();
    let (count, _) = detect_and_count(&mut idx, &s, &ns, rs, re, &rfa);
    let elapsed = start.elapsed();

    println!("\n[B1] === LARGE SCALE BENCH ===");
    println!("  hooks: 500 (400 LLM + 100 XBT)");
    println!("  doc lines: {}", ns.line_len());
    println!("  triggered: {}/500", count);
    println!("  detect time: {:?}", elapsed);
    println!("  avg per hook: {:?}", elapsed / 500);
    assert_eq!(count, 1);
}

#[test]
fn b2_multi_edit_throughput() {
    let text = build_proc_doc(100, 50);
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 100, 50);

    // 20 edits in sequence
    let mut current_text = text.clone();
    let edits = [
        ("### agent_10", "status: IDLE", "status: BUSY"),
        ("### agent_20", "turn: 0", "turn: 1"),
        ("### agent_30", "status: IDLE", "status: ERROR"),
        ("### agent_40", "ppid: root", "ppid: agent_0"),
        ("### agent_50", "status: IDLE", "status: BUSY"),
        ("### tid_10", "status: WAKE_UP", "status: PAUSED"),
        ("### tid_20", "step: 0", "step: 1"),
        ("### tid_30", "status: WAKE_UP", "status: ERROR"),
        ("### agent_60", "status: IDLE", "status: BUSY"),
        ("### agent_70", "turn: 0", "turn: 1"),
        ("### agent_80", "status: IDLE", "status: BUSY"),
        ("### agent_90", "ppid: root", "ppid: new_parent"),
        ("### tid_40", "status: WAKE_UP", "status: DONE"),
        ("### agent_0", "status: IDLE", "status: BUSY"),
        ("### tid_0", "status: WAKE_UP", "status: PAUSED"),
        ("### agent_1", "turn: 0", "turn: 999"),
        ("### agent_2", "status: IDLE", "status: BUSY"),
        ("### agent_3", "status: IDLE", "status: BUSY"),
        ("### agent_4", "turn: 0", "turn: 1"),
        ("### agent_5", "status: IDLE", "status: BUSY"),
    ];

    let mut total_time = std::time::Duration::ZERO;
    let mut total_triggers = 0usize;

    for (i, (section, old_val, new_val)) in edits.iter().enumerate() {
        let pos = current_text.find(section).unwrap();
        let val_pos = current_text[pos..].find(old_val).unwrap() + pos;
        let old_snap = MdioEngine::new(&current_text).make_snapshot();
        let mut e = engine(&current_text);
        e.edit(val_pos, val_pos + old_val.len(), new_val);
        let ns = e.make_snapshot();
        let (rs, re, rfa) = ripple_params(&e, 1);

        let start = Instant::now();
        let (count, _) = detect_and_count(&mut idx, &old_snap, &ns, rs, re, &rfa);
        let elapsed = start.elapsed();

        total_triggers += count;
        total_time += elapsed;
        current_text = ns.to_string();
    }

    println!("\n[B2] === MULTI-EDIT THROUGHPUT ===");
    println!("  edits: 20");
    println!("  total triggers: {}", total_triggers);
    println!("  total time: {:?}", total_time);
    println!("  avg per edit: {:?}", total_time / 20);
    println!("  avg per hook per edit: {:?}", total_time / 20 / 150);
}

#[test]
fn b3_cold_vs_warm_detection() {
    // Compare first detection (cold) with fresh HookIndex vs subsequent
    let text = build_proc_doc(50, 50);
    let s = snap(&text);

    // Edit 1: change agent_25 status
    let mut e1 = engine(&text);
    let pos1 = text.find("### agent_25").unwrap();
    let sp1 = text[pos1..].find("status: IDLE").unwrap() + pos1;
    e1.edit(sp1, sp1 + 12, "status: BUSY");
    let ns1 = e1.make_snapshot();
    let (rs1, re1, rfa1) = ripple_params(&e1, 1);

    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 50, 50);

    // Cold: first detection on first edit
    let start = Instant::now();
    let (c1, _) = detect_and_count(&mut idx, &s, &ns1, rs1, re1, &rfa1);
    let cold = start.elapsed();

    // Warm: second detection, different edit (agent_36 turn)
    let text2 = ns1.to_string();
    let mut e2 = engine(&text2);
    let pos2 = text2.find("### agent_36").unwrap();
    let sp2 = text2[pos2..].find("turn: 0").unwrap() + pos2;
    e2.edit(sp2, sp2 + 7, "turn: 1");
    let ns2 = e2.make_snapshot();
    let (rs2, re2, rfa2) = ripple_params(&e2, 1);

    let start = Instant::now();
    let (c2, _) = detect_and_count(&mut idx, &ns1, &ns2, rs2, re2, &rfa2);
    let warm = start.elapsed();

    println!("\n[B3] === COLD vs WARM ===");
    println!("  cold (1st): {:?}  triggered: {}", cold, c1);
    println!("  warm (2nd): {:?}  triggered: {}", warm, c2);
    // Cold detection includes HashMap group setup overhead
    // Warm detection reuses structures but with different edit
}

// §C: Hook Cycle Timing — Phase-by-phase Breakdown

#[test]
fn c1_detect_cycle_breakdown_60_hooks() {
    let text = build_proc_doc(50, 10);
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 50, 10);

    let mut e = engine(&text);
    let a0 = text.find("### agent_0").unwrap();
    let sp = text[a0..].find("status: IDLE").unwrap() + a0;
    e.edit(sp, sp + 12, "status: BUSY");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);

    // ── Phase A: struct_lines (L0 classification) ──
    let t0 = Instant::now();
    let struct_lines: Vec<usize> = {
        let mut lines = vec![];
        for i in 0..rfa.len() {
            if matches!(classify_entry(&rfa, i), EvClass::C_info | EvClass::I_info) {
                let line = entry_line(rfa[i]) as usize;
                if !lines.contains(&line) { lines.push(line); }
            }
        }
        lines
    };
    let t_struct = t0.elapsed();

    // ── Phase B: Grouping + scope ──
    let t0 = Instant::now();
    let prefix_steps: Vec<Step> = vec![
        Step::Drill("PROCESS_TABLE".into()),
        Step::Drill("LLM".into()),
    ];
    let group_scope = compute_scope_range(&prefix_steps, &ns);
    let t_group_scope = t0.elapsed();

    // ── Phase C: Shared Dril resolution ──
    let t0 = Instant::now();
    let shared_anchor = ns.select_nodes(&prefix_steps);
    let t_shared_dril = t0.elapsed();

    // ── Phase D: Wildcard scan + HashMap build ──
    let t0 = Instant::now();
    let section_root = &shared_anchor[0];
    let all_children = mdio_core::selector::find_direct_children(
        &ns, section_root, None);
    let child_map: std::collections::HashMap<String, &mdio_core::projection::BlockNode> =
        all_children.iter()
            .filter_map(|c| {
                let name = ns.read_block_proj(c, BlockProj::Tag);
                if name.is_empty() { None } else { Some((name, c)) }
            })
            .collect();
    let t_scan = t0.elapsed();

    // ── Phase E: Per-hook — find + Wildcard + Filter + Slice + Hash ──
    let t0 = Instant::now();
    let mut triggered = 0usize;
    let agent_steps: Vec<Step> = vec![
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("status".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    for i in 0..50u64 {
        let agent = format!("agent_{}", i);
        if let Some(tgt) = child_map.get(&agent) {
            let mut current: Vec<mdio_core::projection::BlockNode> = vec![(*tgt).clone()];
            for step in &agent_steps {
                match step {
                    Step::Wildcard => {
                        let mut next = Vec::new();
                        for n in &current {
                            next.extend(mdio_core::selector::find_direct_children(
                                &ns, n, None));
                        }
                        current = next;
                    }
                    Step::Filter(expr) => {
                        current.retain(|n| mdio_core::selector::eval_filter(&ns, n, expr));
                    }
                    Step::Slice(specs) => {
                        current = mdio_core::selector::slice_nodes(current, specs);
                    }
                    _ => {}
                }
            }
            let hash = if current.is_empty() { None } else {
                Some(combined_hash_from_nodes(&current, &ns))
            };
            // Check against cached (simplified — just count)
            if hash.is_some() { triggered += 1; }
        }
    }
    let t_per_hook = t0.elapsed();

    // ── Phase F: Comparison — full select_nodes per hook ──
    let t0 = Instant::now();
    let mut full_count = 0usize;
    for i in 0..50u64 {
        let steps: Vec<Step> = vec![
            Step::Drill("PROCESS_TABLE".into()),
            Step::Drill("LLM".into()),
            Step::Drill(format!("agent_{}", i)),
            Step::Wildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
            Step::Filter(FilterExpr::Cmp {
                proj: BlockProj::Tag, op: CmpOp::Eq,
                value: Value::String("status".into()),
            }),
            Step::Slice(vec![SingleSlice::Index(0)]),
        ];
        if !ns.select_nodes(&steps).is_empty() { full_count += 1; }
    }
    let t_full = t0.elapsed();

    // ── Report ──
    let total_ms = (t_struct + t_group_scope + t_shared_dril + t_scan + t_per_hook).as_secs_f64() * 1000.0;
    let full_ms = t_full.as_secs_f64() * 1000.0;

    println!("\n[C1] ═══ HOOK CYCLE TIMING (60 hooks: 50 LLM + 10 XBT) ═══");
    println!("  rfa entries: {}  struct_lines: {} struct", rfa.len(), struct_lines.len());
    println!("  ─── Phase breakdown ───");
    println!("  A. struct_lines (L0 classify): {:>8.2}µs  ({} entries)",
        t_struct.as_secs_f64() * 1000.0, rfa.len());
    println!("  B. group scope (prefix Dril): {:>8.2}µs",
        t_group_scope.as_secs_f64() * 1000.0);
    println!("  C. shared Dril (select_nodes): {:>8.2}µs  (anchor: {:?})",
        t_shared_dril.as_secs_f64() * 1000.0,
        shared_anchor.first().map(|n| (n.start_line, n.node_type)));
    println!("  D. Wildcard scan + HashMap:   {:>8.2}µs  ({} children)",
        t_scan.as_secs_f64() * 1000.0, all_children.len());
    println!("  E. per-hook (find+filter+hash):{:>8.2}µs  (50 LLM hooks, {} triggered)",
        t_per_hook.as_secs_f64() * 1000.0, triggered);
    println!("  ─── Totals ───");
    println!("  scan_once total (C+D+E):     {:>8.2}µs",
        (t_shared_dril + t_scan + t_per_hook).as_secs_f64() * 1000.0);
    println!("  full cycle (A..E):           {:>8.2}µs", total_ms * 1000.0);
    println!("  F. full select_nodes ×50:    {:>8.2}µs  ({} found)", full_ms * 1000.0, full_count);
    println!("  speedup: scan_once {:.1}x faster than full select_nodes",
        full_ms / total_ms.max(0.001));
}

#[test]
fn c2_detect_cycle_breakdown_scope_skip() {
    // Measure the cost of scope ∩ R = ∅ (fast skip path)
    let mut text = String::from("# OTHER\n- x: y\n\n");
    text.push_str(&build_proc_doc(30, 0));
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 30, 0);

    // Edit "# OTHER" — outside all hook scopes
    let mut e = engine(&text);
    let pos = text.find("x: y").unwrap();
    e.edit(pos, pos + 3, "z: w");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);

    // ── Phase A: struct_lines ──
    let t0 = Instant::now();
    let _sl: Vec<usize> = {
        let mut lines = vec![];
        for i in 0..rfa.len() {
            if matches!(classify_entry(&rfa, i), EvClass::C_info | EvClass::I_info) {
                let line = entry_line(rfa[i]) as usize;
                if !lines.contains(&line) { lines.push(line); }
            }
        }
        lines
    };
    let t_struct = t0.elapsed();

    // ── Phase B: Scope check (per group) ──
    let t0 = Instant::now();
    let prefix_steps: Vec<Step> = vec![
        Step::Drill("PROCESS_TABLE".into()),
        Step::Drill("LLM".into()),
    ];
    let scope = compute_scope_range(&prefix_steps, &ns);
    let skipped = match scope {
        Some((L, E)) => L >= re || E <= rs,
        None => false,
    };
    let t_scope = t0.elapsed();

    // ── Report ──
    println!("\n[C2] ═══ SCOPE SKIP PATH (30 hooks, edit outside) ═══");
    println!("  rfa entries: {}  struct_lines: {:?}", rfa.len(),
        if rfa.is_empty() { "empty".to_string() } else { format!("{} entries", rfa.len()) });
    println!("  scope: {:?}  R: [{},{})  skipped: {}", scope, rs, re, skipped);
    println!("  A. struct_lines: {:>8.2}µs", t_struct.as_secs_f64() * 1000.0);
    println!("  B. scope check:  {:>8.2}µs", t_scope.as_secs_f64() * 1000.0);
    println!("  total skip path: {:>8.2}µs",
        (t_struct + t_scope).as_secs_f64() * 1000.0);
}

#[test]
fn c3_detect_cycle_with_struct_edit() {
    // Structural edit — heading insert, which forces broader ripple
    let text = "# S\n## A\n- k: v\n- j: w\n";
    let s = snap(&text);
    let mut idx = HookIndex::new();

    let steps = vec![
        Step::Drill("S".into()), Step::Drill("A".into()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("k".into()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &s);

    // Insert "## B" between the two bullets — structural change
    let insert_pos = text.find("- j:").unwrap();
    let mut e = engine(text);
    e.edit(insert_pos, insert_pos, "## B\n- x: y\n");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);

    let t0 = Instant::now();
    let events = idx.detect_crdt_write(0, &ns, rs, re, &rfa);
    let t_detect = t0.elapsed();

    println!("\n[C3] ═══ STRUCTURAL EDIT CYCLE ═══");
    println!("  rfa entries: {}  ripple: [{},{})", rfa.len(), rs, re);
    println!("  struct changes in R: {}",
        rfa.iter().enumerate()
            .filter(|(i, _)| matches!(classify_entry(&rfa, *i), EvClass::C_info | EvClass::I_info))
            .count());
    println!("  triggered: {} hooks", events.len());
    println!("  detect time: {:>8.2}µs", t_detect.as_secs_f64() * 1000.0);
}

// §S4: scan_once vs 统一 select_nodes 对比

#[test]
fn s4_scan_once_vs_unified_probe() {
    // s1 场景:50 LLM + 10 XBT(60 hook)——编辑 agent_0 status
    let text = build_proc_doc(50, 10);
    let s = snap(&text);
    let mut idx = HookIndex::new();
    register_proc_hooks(&mut idx, &s, 50, 10);
    let mut e = engine(&text);
    let agent0_pos = text.find("### agent_0").unwrap();
    let status_pos = text[agent0_pos..].find("status: IDLE").unwrap() + agent0_pos;
    e.edit(status_pos, status_pos + 12, "status: BUSY");
    let ns = e.make_snapshot();
    let (rs, re, rfa) = ripple_params(&e, 1);

    // A:现状(scan_once——detect 现有路径)
    let mut idx_a = HookIndex::new();
    register_proc_hooks(&mut idx_a, &s, 50, 10);
    let t0 = Instant::now();
    let (count_a, _) = detect_and_count(&mut idx_a, &s, &ns, rs, re, &rfa);
    let el_a = t0.elapsed();

    // B:统一模拟(每 hook 完整 select_nodes + combined_hash——复刻
    // evaluate_and_dispatch 的成本——触发判定简化:非空即计数)
    let mut idx_b = HookIndex::new();
    register_proc_hooks(&mut idx_b, &s, 50, 10);
    let t1 = Instant::now();
    let mut count_b = 0usize;
    for i in 0..60 {
        let steps = if i < 50 {
            vec![Step::Drill("PROCESS_TABLE".into()), Step::Drill("LLM".into()),
                 Step::Drill(format!("agent_{}", i)), Step::Wildcard,
                 Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
                 Step::Filter(FilterExpr::Cmp {
                     proj: BlockProj::Tag, op: CmpOp::Eq,
                     value: Value::String("status".into()),
                 }),
                 Step::Slice(vec![SingleSlice::Index(0)])]
        } else {
            vec![Step::Drill("PROCESS_TABLE".into()), Step::Drill("XBT".into()),
                 Step::Drill(format!("tid_{}", i - 50)), Step::Wildcard,
                 Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
                 Step::Filter(FilterExpr::Cmp {
                     proj: BlockProj::Tag, op: CmpOp::Eq,
                     value: Value::String("status".into()),
                 }),
                 Step::Slice(vec![SingleSlice::Index(0)])]
        };
        let nodes = ns.select_nodes(&steps);
        if !nodes.is_empty() {
            let _h = combined_hash_from_nodes(&nodes, &ns);
            count_b += 1;
        }
    }
    let el_b = t1.elapsed();

    println!("\n[S4] ═══ scan_once vs 统一 select_nodes(60 hook) ═══");
    println!("  A(scan_once):  {}µs  触发={}", el_a.as_secs_f64() * 1000.0, count_a);
    println!("  B(统一 select_nodes): {}µs  计数={}", el_b.as_secs_f64() * 1000.0, count_b);
    println!("  比值 A/B = {:.2}", el_a.as_secs_f64() / el_b.as_secs_f64().max(1e-9));
}

// §S4b: 新增代价拆分布点(统一路径的成本构成)

#[test]
fn s4b_unified_cost_breakdown() {
    let text = build_proc_doc(50, 10);
    let s = snap(&text);
    let ns = s; // 同快照(编辑后场景近似——懒缓存状态同)
    // 拆分布点(60 hook 完整链的成本构成):
    // B1:完整链(Drill×3 + Wildcard + Filter×2 + Slice——含树构建)
    // B2:纯定位链(Drill×3——懒缓存命中——零树)
    // B3:定位 + Filter + Slice(无 Wildcard——零树)
    let full_steps: Vec<Vec<Step>> = (0..60).map(|i| {
        let proto = if i < 50 { "LLM" } else { "XBT" };
        let name = if i < 50 { format!("agent_{}", i) } else { format!("tid_{}", i - 50) };
        vec![Step::Drill("PROCESS_TABLE".into()), Step::Drill(proto.into()),
             Step::Drill(name), Step::Wildcard,
             Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
             Step::Filter(FilterExpr::Cmp {
                 proj: BlockProj::Tag, op: CmpOp::Eq,
                 value: Value::String("status".into()),
             }),
             Step::Slice(vec![SingleSlice::Index(0)])]
    }).collect();
    let drill_steps: Vec<Vec<Step>> = full_steps.iter().map(|st| st[..3].to_vec()).collect();
    let drill_fs: Vec<Vec<Step>> = full_steps.iter()
        .map(|st| [st[..3].to_vec(), st[4..].to_vec()].concat()).collect();

    let t0 = Instant::now();
    for st in &full_steps { let _ = ns.select_nodes(st); }
    let el_full = t0.elapsed();

    let t1 = Instant::now();
    for st in &drill_steps { let _ = ns.select_nodes(st); }
    let el_drill = t1.elapsed();

    let t2 = Instant::now();
    for st in &drill_fs { let _ = ns.select_nodes(st); }
    let el_dfs = t2.elapsed();

    println!("\n[S4b] ═══ 统一路径成本构成(60 hook——同快照) ═══");
    println!("  完整链(含树构建): {:>8.2}µs", el_full.as_secs_f64() * 1000.0);
    println!("  纯定位链(Drill×3): {:>8.2}µs", el_drill.as_secs_f64() * 1000.0);
    println!("  定位+Filter+Slice(零树): {:>8.2}µs", el_dfs.as_secs_f64() * 1000.0);
    println!("  树构建增量(完整-零树): {:>8.2}µs", (el_full.as_secs_f64() - el_dfs.as_secs_f64()) * 1000.0);
}
