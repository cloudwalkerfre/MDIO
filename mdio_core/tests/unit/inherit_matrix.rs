// inherit_matrix — 继承清单行为矩阵
//
// 目的：把"当前跨代继承行为"变成可执行断言（11 项 × 2 条链），
// 为"继承清单数据化"提供零行为变更的验收基线。
// include! 进 cache.rs 的 tests 模块（外层已 use super::*——本文件不重复导入）。
//
// 断言口径：内容链（守卫通过）期望共享的用 Arc::ptr_eq / is_some；
// 结构链（structural_change=true）期望重建的用 is_none / !ptr_eq。

use crate::projection::Snapshot;
use crate::session::MdioSession;
use dashmap::DashSet;

fn inherit_session(text: &str) -> MdioSession {
    MdioSession::new(1, text, 1, "test".to_string(), None, None, None, Arc::new(DashSet::new()), Arc::new(crate::session::DetectClock::new()))
}

fn node_of(snap: &Arc<Snapshot>) -> Arc<CacheNode> {
    let slot = snap.cache_slot.upgrade().expect("slot");
    ensure_node(&slot, snap.clone())
}

/// 暖基：把可物化项全部物化到 snap 上。
fn warm_all(snap: &Arc<Snapshot>) {
    let n = snap.line_len();
    let _ = structure_for_range(snap, 0, n); // full + block_starts + segment_memo
    let _ = kernel_root_table(snap);
    let _ = kernel_root_blocks_table(snap);
    let _ = kernel_block_spans_full(snap);
    let _ = old_block_index_of_own(snap);
    let _ = kernel_block_starts(snap);
}

// ── T1-a · 内容链（守卫通过）─────────────────────────────────────────

#[test]
fn inherit_matrix_content_chain_shares_expected_items() {
    let session = inherit_session("# H\n\npara a\n\npara b\n\n");
    session.execute_transaction(None, true, |ctx| {
        let start = ctx.state.engine.byte_of_line(2) + 5;
        ctx.state.engine.edit(start, start + 1, "x");
    });
    let snap1 = session.publisher.load();
    // 强持旧槽：否则下一次 publish 的 reaper 半钩子按 strong_count==2 取走
    // 旧代 full/render（reaper_take_if_last），继承断言会假红。
    let slot1 = snap1.cache_slot.upgrade().expect("slot1");
    let node1 = ensure_node(&slot1, snap1.clone());
    warm_all(&snap1);

    // 第二次纯内容编辑：守卫通过 → 透传。
    session.execute_transaction(None, true, |ctx| {
        let start = ctx.state.engine.byte_of_line(2) + 5;
        ctx.state.engine.edit(start, start + 1, "y");
    });
    let snap2 = session.publisher.load();
    let slot2 = snap2.cache_slot.upgrade().expect("slot2");
    let node2 = ensure_node(&slot2, snap2.clone());

    // ── 快照级 ──
    let bc1 = snap1.block_cache.clone().expect("bc1");
    let bc2 = snap2.block_cache.clone().expect("bc2");
    assert!(Arc::ptr_eq(&bc1, &bc2), "block_cache 应跨透传共享同一壳");
    let prev = snap2.prev_snapshot.upgrade().expect("prev");
    assert!(Arc::ptr_eq(&snap1, &prev), "prev_snapshot 应指向直接前驱");
    // [归位 S2] 断言对象 = 格入口（同源：BlockSpans 格）。
    let oi1 = old_block_index_of_own(&snap1).expect("oidx1");
    let oi2 = old_block_index_of_own(&snap2).expect("oidx2");
    assert!(Arc::ptr_eq(&oi1, &oi2), "old_block_index 应跨透传共享（格继承）");
    assert!(snap2.segment_memo.get().is_none(), "segment_memo 不继承（每次 snapshot 新建空壳）");
    let c1 = snap1.cells.clone().expect("cells1");
    let c2 = snap2.cells.clone().expect("cells2");
    assert!(!Arc::ptr_eq(&c1, &c2), "cells 表壳不直接继承（经 prev_snapshot 懒继承）");

    // ── 节点级 ──
    let f1 = node1.cached_full().expect("full1");
    let f2 = node2.cached_full().expect("full2");
    assert!(Arc::ptr_eq(&f1, &f2), "full 应跨透传共享（仅取已物化值）");
    let s1 = kernel_block_starts(&snap1).expect("bs1");
    let s2 = kernel_block_starts(&snap2).expect("bs2");
    assert!(Arc::ptr_eq(&s1, &s2), "block_starts 应跨透传共享");
    // [归位 S3] 断言对象 = 格入口（同格）。
    let rt1 = snap1.root_table_cell(false);
    let rt2 = snap2.root_table_cell(false);
    assert!(Arc::ptr_eq(&rt1, &rt2), "root_table 应跨透传共享（格继承）");
    let rbt1 = snap1.root_table_cell(true);
    let rbt2 = snap2.root_table_cell(true);
    assert!(Arc::ptr_eq(&rbt1, &rbt2), "root_blocks_table 应跨透传共享（格继承）");
    let sp1 = kernel_block_spans_full(&snap1).expect("sp1");
    let sp2 = kernel_block_spans_full(&snap2).expect("sp2");
    assert!(Arc::ptr_eq(&sp1, &sp2), "block_spans 应跨透传共享");

    // ── render：不经节点继承（走 overlay 通道）──
    match &*node2 {
        CacheNode::Base { render, .. } => assert!(
            render.load_full().is_none(),
            "节点级 render 不继承（字节归属走 ContentOverlay）"
        ),
        CacheNode::Edit { .. } => panic!("content chain must produce Base"),
    }
}

// ── T1-b · 结构链（structural_change=true）────────────────────────────

#[test]
fn inherit_matrix_structural_chain_rebuilds_expected_items() {
    let session = inherit_session("# S\n\nbody\n");
    // 保初代快照存活：Edit 节点的 old_snap 是 Weak，初代若死则
    // materialize_old_zone 返 None → 旧侧三表建不出来。
    let _snap0 = session.publisher.load();
    session.execute_transaction(None, true, |ctx| {
        let len = ctx.state.engine.byte_len();
        ctx.state.engine.edit(len, len, "- item\n");
    });
    let snap1 = session.publisher.load();
    let slot1 = snap1.cache_slot.upgrade().expect("slot1");
    let node1 = ensure_node(&slot1, snap1.clone());
    warm_all(&snap1);
    // [归位 S3] 断言对象 = 格入口（同格）。
    let old_rt = snap1.root_table_cell(false);
    let old_rbt = snap1.root_table_cell(true);
    let old_bs = kernel_block_spans_full(&snap1).expect("bs1");
    let old_bc = snap1.block_cache.clone().expect("bc1");

    // 结构编辑：再追加一行 → structural_change=true → 提升 Edit。
    session.execute_transaction(None, true, |ctx| {
        let len = ctx.state.engine.byte_len();
        ctx.state.engine.edit(len, len, "- item2\n");
    });
    let snap2 = session.publisher.load();
    let node2 = node_of(&snap2);

    assert!(snap2.prev_snapshot.upgrade().is_none(), "结构轮不得挂前驱");
    // [归位 S2] 断言"不共享"（结构轮格重建，Arc 不同）。
    let oi1 = old_block_index_of_own(&snap1).expect("oidx1");
    let oi2 = old_block_index_of_own(&snap2).expect("oidx2");
    assert!(!Arc::ptr_eq(&oi1, &oi2), "结构轮不得共享旧块跨度格");
    let bc2 = snap2.block_cache.clone().expect("bc2");
    assert!(!Arc::ptr_eq(&old_bc, &bc2), "结构轮 block_cache 应为新壳");
    assert!(node2.cached_full().is_none(), "Edit 节点 full 惰性（不继承旧 full）");
    match &*node2 {
        CacheNode::Edit { .. } => {}
        CacheNode::Base { .. } => panic!("structural chain must produce Edit"),
    }
    let new_rt = kernel_root_table(&snap2).expect("rt2");
    let new_rbt = kernel_root_blocks_table(&snap2).expect("rbt2");
    let new_bs = kernel_block_spans_full(&snap2).expect("bs2");
    assert!(!Arc::ptr_eq(&old_rt, &new_rt), "结构轮 root_table 必须重建");
    assert!(!Arc::ptr_eq(&old_rbt, &new_rbt), "结构轮 root_blocks_table 必须重建");
    assert!(!Arc::ptr_eq(&old_bs, &new_bs), "结构轮 block_spans 必须重建");
}

// ── T1-c · 边界：继承只取"已物化"值（§5.4 仅 load 不物化）────────────

#[test]
fn inherit_matrix_passthrough_never_materializes_old_full() {
    let session = inherit_session("# H\n\npara a\n\n");
    session.execute_transaction(None, true, |ctx| {
        let start = ctx.state.engine.byte_of_line(2) + 5;
        ctx.state.engine.edit(start, start + 1, "x");
    });
    let snap1 = session.publisher.load();
    let n1 = node_of(&snap1); // 建 Base 但**不**物化 full
    assert!(n1.cached_full().is_none(), "前置：旧 full 未物化");

    session.execute_transaction(None, true, |ctx| {
        let start = ctx.state.engine.byte_of_line(2) + 5;
        ctx.state.engine.edit(start, start + 1, "y");
    });
    let snap2 = session.publisher.load();
    let node2 = node_of(&snap2);
    assert!(
        node2.cached_full().is_none(),
        "透传不得拖前驱物化：旧 full 未物化时新节点 full 必须为 None"
    );
}

// ── T4 · 同源副本：old_block_index ≡ block_spans ─────────────────────

#[test]
fn t4_old_block_index_equals_block_spans_with_slot() {
    let mut state = State::new("# S\n\nbody\n- a\n- b\n");
    let mut snap = Arc::new(state.snapshot());
    let slot = Arc::new(CacheSlot::new());
    Arc::get_mut(&mut snap).unwrap().set_cache_slot(Arc::downgrade(&slot));
    slot.node.store(Some(Arc::new(CacheNode::new_base(snap.clone()))));

    let spans = kernel_block_spans_full(&snap).expect("spans");
    let oidx = old_block_index_of_own(&snap).expect("oidx");
    assert!(
        Arc::ptr_eq(&spans, &oidx),
        "有槽时 old_block_index 应与 block_spans 是同一 Arc（A3-3 收敛）"
    );
}

#[test]
fn t4_old_block_index_without_slot_self_builds() {
    let mut state = State::new("# S\n\nbody\n- a\n- b\n");
    let snap = Arc::new(state.snapshot()); // 无 cache_slot
    // [归位 S2 翻转] 存储迁入派生格层后，无槽也由格构建体给出答案
    // （派生格层契约「到达即建」），两侧同源同 Arc——不再有"前者缺席、
    // 后者自建"的不对称。
    let spans = kernel_block_spans_full(&snap).expect("格入口无槽也应可建");
    let oidx = old_block_index_of_own(&snap).expect("oidx 无槽也应可建");
    assert!(Arc::ptr_eq(&spans, &oidx), "无槽时两者必须同源同 Arc（同源副本合并）");
}
// ── J2（S4）· 格层惰性：查格不得拖前驱物化 ──────────────────────────

#[test]
fn j2_cell_query_never_materializes_predecessor() {
    let session = inherit_session("# H\n\npara a\n\npara b\n\n");
    session.execute_transaction(None, true, |ctx| {
        let start = ctx.state.engine.byte_of_line(2) + 5;
        ctx.state.engine.edit(start, start + 1, "x");
    });
    let snap1 = session.publisher.load();
    let slot1 = snap1.cache_slot.upgrade().expect("slot1");
    let node1 = ensure_node(&slot1, snap1.clone());
    assert!(node1.cached_full().is_none(), "前置：前驱 full 未物化");

    // 第 2 代（同类内容编辑 ⇒ 守卫通过 ⇒ 透传链）
    session.execute_transaction(None, true, |ctx| {
        let start = ctx.state.engine.byte_of_line(2) + 5;
        ctx.state.engine.edit(start, start + 1, "y");
    });
    let snap2 = session.publisher.load();
    // 在 snap2 上分别走 Build 臂（可能构建**自己**的全量）与 UseIfReady 臂
    let _ = snap2.block_spans_cell();
    let _ = snap2.block_spans_cell_if_ready();
    // 断言：前驱仍未被拖物化（T1-c 管节点级；本判官管**格层查询**）
    assert!(node1.cached_full().is_none(), "格层查询不得拖前驱物化（J2）");
}

// ── J1（S4）· 代序列成本：稳态「新侧一次构建、旧侧零构建、走查 zone 级」────

/// 判官：同型**结构**编辑 K 代（新侧恒 Edit 节点 ⇒ 根表必须自建），逐代重放
/// `root_offsets` 的两侧调用次序，量 `root_count_before` 的成本形态：
///   ① 两侧值 == oracle（`roots_in_range(0,q)` 合并口径）；
///   ② 稳态（k≥1）新侧恰好 +1 构建、旧侧 +0 构建（预热链的兑现面）；
///   ③ 走查行数 ≤ 64（zone 级）——**前缀级即失败**（语料前缀 160 行 ≫ 64）。
///
/// 红基线（A-0′ 病历，本仓库实测）：若把新侧计数换成"复用旧侧值"（新侧格
/// 永不物化），下一代旧侧查询落 `rtb_chain_break` 全量 oracle，`root_count`
/// p50 13.90µs → 140.06µs（10×）——断言②（旧侧 +1）与③（走查前缀级）同时判红。
/// 判官骨架 = 既有 `root_count_query_is_local`（tests/unit/incr.rs，现 `#[ignore]`
/// 且自陈红：`rtb_edit 0→1 / walked 599`）。
///
/// 口径声明：写走生产 `execute_transaction`，查询走生产 `Snapshot::root_count_before`；
/// 合成面只有"谁在何时调它"（判官替代 Path worker 的两处调用点）。
#[test]
fn j1_generation_sequence_cost_stays_zone_bounded() {
    use crate::incr::{roots_in_range, ROOTS_RANGE_LINES_WALKED};
    use crate::projection::CellKind;

    // 语料：120 段；编辑 = **中部**插入一个列表项（结构编辑 ⇒ 新侧 Edit）。
    // 中部的意义：若新侧落 bare/chain-break 兜底，走查 = 全文；若走三截，
    // 走查 = zone（ripple 侧 zone_both_sides）——两者量级可辨。
    let mut doc = String::new();
    for i in 0..120 {
        doc.push_str(&format!("para {i}\n\n"));
    }

    let session = inherit_session(&doc);
    let q_line = 160usize;
    let mut prev = session.publisher.load();

    for k in 0..4usize {
        session.execute_transaction(None, true, |ctx| {
            let at = ctx.state.engine.byte_of_line(120) + k; // 中部（para 60 一带）
            let ins = format!("- item {k}\n");
            ctx.state.engine.edit(at, at, &ins);
        });
        let cur = session.publisher.load();

        let is_edit = matches!(&*node_of(&cur), CacheNode::Edit { .. });

        // 旧侧（先，与 root_offsets 的调用次序一致）。
        let (_, _, ob0) = prev.cell_consumption.cell_stats(CellKind::RootTable);
        ROOTS_RANGE_LINES_WALKED.with(|c| c.set(0));
        let got_old = prev.root_count_before(q_line).expect("old idx");
        let w_old = ROOTS_RANGE_LINES_WALKED.with(|c| c.get());
        let (_, _, ob1) = prev.cell_consumption.cell_stats(CellKind::RootTable);
        assert_eq!(got_old, roots_in_range(&prev, 0, q_line).len(), "gen{k}: 旧侧值 == oracle");

        // 新侧。
        let (_, _, nb0) = cur.cell_consumption.cell_stats(CellKind::RootTable);
        ROOTS_RANGE_LINES_WALKED.with(|c| c.set(0));
        let got_new = cur.root_count_before(q_line).expect("new idx");
        let w_new = ROOTS_RANGE_LINES_WALKED.with(|c| c.get());
        let (_, _, nb1) = cur.cell_consumption.cell_stats(CellKind::RootTable);
        assert_eq!(got_new, roots_in_range(&cur, 0, q_line).len(), "gen{k}: 新侧值 == oracle");

        // ③ 成本形态（稳态 k≥1 是量测面；k=0 是冷启，zone 尚未收敛）。
        if k >= 1 {
            assert_eq!(ob1 - ob0, 0, "gen{k}: 旧侧上一代已物化 ⇒ 本轮必须命中（不得重建）——A-0′ 病征");
            assert_eq!(w_old, 0, "gen{k}: 旧侧命中不得有走查，实测 {w_old}");
            if is_edit {
                assert_eq!(nb1 - nb0, 1, "gen{k}: 结构新侧恰好构建一次（预热链的付款面）");
                let zone = node_of(&cur).materialize_zone();
                let zlen = zone.1.saturating_sub(zone.0);

                assert!(w_new <= zlen.max(1) as u64 + 8,
                    "gen{k}: 新侧走查必须 zone 级（zone={zlen}，实测 {w_new}，全文 n={}）", cur.line_len());
            } else {
                assert_eq!(nb1 - nb0, 0, "gen{k}: 内容链新侧必须继承（0 构建）");
                assert_eq!(w_new, 0, "gen{k}: 内容链新侧不得有走查，实测 {w_new}");
            }
        }
        prev = cur;
    }
}
// ── O2 · 格槽位并发语义 ──────────────────────────────────────────────

/// [O2 判官] 同键并发查询必须单构建：N 线程同时查同一格，全部拿到**同一 Arc**
/// （多构建会产出不同 Arc）。锁粒度修正（表锁只覆盖查/建槽、构建在槽的
/// OnceLock 内）不得破坏"同键单构建"语义——异键互不阻塞由网关 bench 实测
/// 与本节注释记账。
#[test]
fn o2_same_key_concurrent_query_is_single_build() {
    let mut state = State::new("# H\n\npara a\n\n- x\n- y\n\npara b\n");
    let mut snap = Arc::new(state.snapshot());
    let slot = Arc::new(CacheSlot::new_with_id(11));
    Arc::get_mut(&mut snap).unwrap().set_cache_slot(Arc::downgrade(&slot));
    slot.node.store(Some(Arc::new(CacheNode::new_base(snap.clone()))));

    let mut handles = Vec::new();
    for _ in 0..8 {
        let s = snap.clone();
        handles.push(std::thread::spawn(move || s.root_table_cell(false)));
    }
    let tables: Vec<_> = handles.into_iter().map(|h| h.join().expect("thread")).collect();
    for t in &tables[1..] {
        assert!(Arc::ptr_eq(&tables[0], t), "同键并发必须单构建（同一 Arc）");
    }
}

// ── S1 · 归位：格入口与节点级路径逐值一致（本阶段不切换消费面）────────

/// [归位 S1 判官] 新格入口（`root_table_cell` / `block_spans_cell`）与既有
/// 节点级路径**逐值一致**；同快照二次查询同 Arc；无槽时格可建而旧
/// `kernel_*` 路径仍 None（S1 只加新路径、不动旧路径）。
#[test]
fn s1_cell_entries_match_node_path() {
    // ① 有槽 Base：格值 == 节点级值（三张表逐一）。
    let mut state = State::new("# H\n\npara a\n\n- x\n- y\n\npara b\n");
    let mut snap = Arc::new(state.snapshot());
    let slot = Arc::new(CacheSlot::new_with_id(7));
    Arc::get_mut(&mut snap).unwrap().set_cache_slot(Arc::downgrade(&slot));
    slot.node.store(Some(Arc::new(CacheNode::new_base(snap.clone()))));

    let node_rt = kernel_root_table(&snap).expect("node root table");
    let cell_rt = snap.root_table_cell(false);
    assert_eq!(&*node_rt, &*cell_rt, "RootTable 格值必须与节点级逐值相等");

    let node_rbt = kernel_root_blocks_table(&snap).expect("node split table");
    let cell_rbt = snap.root_table_cell(true);
    assert_eq!(&*node_rbt, &*cell_rbt, "RootBlocksTable 格值必须与节点级逐值相等");

    let node_bs = kernel_block_spans_full(&snap).expect("node spans");
    let cell_bs = snap.block_spans_cell();
    assert_eq!(&*node_bs, &*cell_bs, "BlockSpans 格值必须与节点级逐值相等");

    assert!(Arc::ptr_eq(&cell_rt, &snap.root_table_cell(false)), "同快照二次查询应命中同 Arc");
    assert!(Arc::ptr_eq(&cell_bs, &snap.block_spans_cell()), "同快照二次查询应命中同 Arc");

    // ② 无槽：格可建（派生格层「到达即建」）。
    // [归位 S3 翻转] S1 阶段旧 `kernel_*` 路径无槽仍 None；S3 起三张表全部走格，
    // 薄转发恒有答案——原断言（None）随之翻转。
    let mut state2 = State::new("# S\n\nbody\n");
    let snap2 = Arc::new(state2.snapshot());
    assert!(!snap2.root_table_cell(false).is_empty(), "格入口无槽也应可建");
    assert!(!snap2.root_table_cell(true).is_empty(), "split 根格无槽也应可建");
    assert!(!snap2.block_spans_cell().is_empty(), "块跨度格无槽也应可建");
    assert!(kernel_root_table(&snap2).is_some(), "S3 起薄转发无槽也有答案");
    assert!(kernel_block_spans_full(&snap2).is_some(), "S3 起薄转发无槽也有答案");
}
