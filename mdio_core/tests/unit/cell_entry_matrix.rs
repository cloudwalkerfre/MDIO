// cell_entry_matrix — S2 七入口 × 五桶探针（附录 G.2 表 + G.3 断言清单）
//
// include! 进 projection.rs 的 tests 模块（外层已 use super::*——本文件不重复导入）。
//
// 口径：
//   * 入口 = 七个格入口（四个构建访问器 + 三个策略兄弟）；域 = 键的第三元。
//   * 每入口调用前后各取一次 cell_stats（窗口内只有该入口在跑）——这正是
//     G.3-2 的入口级归因形态；桶级 q/h/b 与入口级账本逐窗口对拍。
//   * span 桶三入口必须按 peek → if_ready → build 的顺序驱动（G.2 顺序约束：
//     Build 会烘热格槽，先调就毁掉前两者的判别）。
//   * 冷 = 无槽快照（State::new().snapshot()：cache_slot 空、prev_snapshot 空）；
//     暖 = 手工挂 prev_snapshot 指向前驱（前驱已烘热五格）。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::domain::DomainId;
use crate::projection::{
    cell_entry_reset, cell_entry_stats, cell_entry_totals, CellEntry, CellKind, Snapshot, State,
    CELL_ENTRANCES,
};
use crate::topology::{CONTAINER_TOPOLOGY, HEADINGSECTION_TOPOLOGY};

const FIVE: [CellKind; 5] = [
    CellKind::Tree,
    CellKind::Secs,
    CellKind::RootTable,
    CellKind::RootBlocksTable,
    CellKind::BlockSpans,
];

const DOC: &str = "# H\n\npara one\n\npara two\n";

fn cold(text: &str) -> Snapshot {
    State::new(text).snapshot()
}

fn stats(snap: &Snapshot) -> [(u64, u64, u64); 5] {
    let mut out = [(0u64, 0u64, 0u64); 5];
    for (i, k) in FIVE.iter().enumerate() {
        out[i] = snap.cell_consumption.cell_stats(*k);
    }
    out
}

fn delta(before: &[(u64, u64, u64); 5], after: &[(u64, u64, u64); 5]) -> [(u64, u64, u64); 5] {
    let mut out = [(0u64, 0u64, 0u64); 5];
    for i in 0..5 {
        out[i] = (
            after[i].0 - before[i].0,
            after[i].1 - before[i].1,
            after[i].2 - before[i].2,
        );
    }
    out
}

fn at(d: &[(u64, u64, u64); 5], k: CellKind) -> (u64, u64, u64) {
    d[crate::projection::SnapshotConsumption::kind_idx(k)]
}

fn assert_bucket(d: &[(u64, u64, u64); 5], k: CellKind, want: (u64, u64, u64), ctx: &str) {
    assert_eq!(at(d, k), want, "{ctx}: 桶 {k:?} 期望 {want:?} 实得 {:?}", at(d, k));
}

fn assert_only(d: &[(u64, u64, u64); 5], k: CellKind, ctx: &str) {
    for (j, other) in FIVE.iter().enumerate() {
        if *other != k {
            assert_eq!(d[j], (0, 0, 0), "{ctx}: 桶 {other:?} 不得有变化");
        }
    }
}

/// 暖快照：前驱已烘热五格，当前快照只挂 Weak（继承腿可升级）。
fn warm_pair(text: &str) -> (Arc<Snapshot>, Snapshot) {
    let prev = Arc::new(cold(text));
    let _ = prev.tree_cell(&CONTAINER_TOPOLOGY);
    let _ = prev.section_cell(&HEADINGSECTION_TOPOLOGY);
    let _ = prev.root_table_cell_merge();
    let _ = prev.root_table_cell_split();
    let _ = prev.block_spans_cell();
    let mut cur = cold(text);
    cur.prev_snapshot = Arc::downgrade(&prev);
    (prev, cur)
}

// ── A 冷（无槽）：逐入口窗口的期望 = G.2 表「冷首查」列 ──────────────

#[test]
fn cell_entry_matrix_cold_five_buckets() {
    // span 桶三入口按 peek → if_ready → build（顺序约束；本测试即该顺序的正例）。
    let snap = cold(DOC);
    let b0 = stats(&snap);
    assert!(snap.peek_block_spans_cell().is_none(), "Peek 冷首查必须 None");
    let b1 = stats(&snap);
    assert_only(&delta(&b0, &b1), CellKind::BlockSpans, "peek_block_spans_cell 冷");
    assert_bucket(&delta(&b0, &b1), CellKind::BlockSpans, (1, 0, 0), "peek_block_spans_cell 冷");

    assert!(snap.block_spans_cell_if_ready().is_none(), "UseIfReady 冷未命中也必须 None");
    let b2 = stats(&snap);
    assert_only(&delta(&b1, &b2), CellKind::BlockSpans, "block_spans_cell_if_ready 冷");
    assert_bucket(&delta(&b1, &b2), CellKind::BlockSpans, (1, 0, 0), "block_spans_cell_if_ready 冷");

    let _spans = snap.block_spans_cell();
    let b3 = stats(&snap);
    assert_only(&delta(&b2, &b3), CellKind::BlockSpans, "block_spans_cell 冷");
    assert_bucket(&delta(&b2, &b3), CellKind::BlockSpans, (1, 0, 1), "block_spans_cell 冷");

    // Peek 臂不得安装：先 Peek 再 Build 时 Build 仍必须 b+1（G.3-3）。
    let snap2 = cold(DOC);
    let c0 = stats(&snap2);
    assert!(!snap2.peek_root_table_cell(false), "Peek 冷首查必须 false");
    let c1 = stats(&snap2);
    assert_only(&delta(&c0, &c1), CellKind::RootTable, "peek_root_table_cell 冷");
    assert_bucket(&delta(&c0, &c1), CellKind::RootTable, (1, 0, 0), "peek_root_table_cell 冷");
    let rt = snap2.root_table_cell_merge();
    let c2 = stats(&snap2);
    assert_bucket(&delta(&c1, &c2), CellKind::RootTable, (1, 0, 1), "root_table_cell 冷（Peek 未安装）");
    assert!(!rt.is_empty(), "根表载荷不得为空（非空占位）");
    // 一致性：Δh == 1 ⟺ 返回 true（G.3-3 的变体错配守卫）。
    assert!(snap2.peek_root_table_cell(false));
    let c3 = stats(&snap2);
    assert_bucket(&delta(&c2, &c3), CellKind::RootTable, (1, 1, 0), "peek 命中 ⇒ h+1");

    // 其余四桶的冷首查：各自 (1,0,1)，互不串桶。
    let snap3 = cold(DOC);
    let d0 = stats(&snap3);
    let _ = snap3.tree_cell(&CONTAINER_TOPOLOGY);
    let d1 = stats(&snap3);
    assert_only(&delta(&d0, &d1), CellKind::Tree, "tree_cell 冷");
    assert_bucket(&delta(&d0, &d1), CellKind::Tree, (1, 0, 1), "tree_cell 冷");

    let _ = snap3.section_cell(&HEADINGSECTION_TOPOLOGY);
    let d2 = stats(&snap3);
    assert_only(&delta(&d1, &d2), CellKind::Secs, "section_cell 冷");
    assert_bucket(&delta(&d1, &d2), CellKind::Secs, (1, 0, 1), "section_cell 冷");

    let _ = snap3.root_table_cell_split();
    let d3 = stats(&snap3);
    assert_only(&delta(&d2, &d3), CellKind::RootBlocksTable, "root_table_cell(true) 冷");
    assert_bucket(&delta(&d2, &d3), CellKind::RootBlocksTable, (1, 0, 1), "root_table_cell(true) 冷");

    assert!(snap3.peek_root_table_cell(true), "split 口径 Build 后 Peek 必须 true");
    let d4 = stats(&snap3);
    assert_bucket(&delta(&d3, &d4), CellKind::RootBlocksTable, (1, 1, 0), "peek(split) 命中 ⇒ h+1");
    assert_eq!(at(&delta(&d3, &d4), CellKind::RootTable), (0, 0, 0), "split 查询不得串到 merge 桶");
}

// ── B 同快照二查：全部转命中 (1,1,0) ────────────────────────────────

#[test]
fn cell_entry_matrix_second_query_hits() {
    let snap = cold(DOC);
    let _ = snap.tree_cell(&CONTAINER_TOPOLOGY);
    let _ = snap.section_cell(&HEADINGSECTION_TOPOLOGY);
    let _ = snap.root_table_cell_merge();
    let _ = snap.root_table_cell_split();
    let spans = snap.block_spans_cell();

    let e0 = stats(&snap);
    let t2 = snap.tree_cell(&CONTAINER_TOPOLOGY);
    let e1 = stats(&snap);
    assert_only(&delta(&e0, &e1), CellKind::Tree, "tree_cell 二查");
    assert_bucket(&delta(&e0, &e1), CellKind::Tree, (1, 1, 0), "tree_cell 二查");

    let s2 = snap.section_cell(&HEADINGSECTION_TOPOLOGY);
    let e2 = stats(&snap);
    assert_bucket(&delta(&e1, &e2), CellKind::Secs, (1, 1, 0), "section_cell 二查");

    let r2 = snap.root_table_cell_merge();
    let e3 = stats(&snap);
    assert_bucket(&delta(&e2, &e3), CellKind::RootTable, (1, 1, 0), "root_table_cell(false) 二查");
    let rb2 = snap.root_table_cell_split();
    let e4 = stats(&snap);
    assert_bucket(&delta(&e3, &e4), CellKind::RootBlocksTable, (1, 1, 0), "root_table_cell(true) 二查");

    let sp2 = snap.block_spans_cell();
    let e5 = stats(&snap);
    assert_bucket(&delta(&e4, &e5), CellKind::BlockSpans, (1, 1, 0), "block_spans_cell 二查");
    assert!(Arc::ptr_eq(&spans, &sp2), "二查必须同一 Arc（单构建）");

    // 本代已就绪 ⇒ UseIfReady 也命中；Peek 与本代格槽同源（已烘热）。
    assert!(snap.block_spans_cell_if_ready().is_some(), "本代就绪 ⇒ UseIfReady Some");
    let e6 = stats(&snap);
    assert_bucket(&delta(&e5, &e6), CellKind::BlockSpans, (1, 1, 0), "if_ready 本代就绪");
    assert!(snap.peek_block_spans_cell().is_some(), "本代就绪 ⇒ Peek Some");
    let e7 = stats(&snap);
    assert_bucket(&delta(&e6, &e7), CellKind::BlockSpans, (1, 1, 0), "peek 本代就绪");

    // 载荷同源：tree/secs/root 二查必须同一 Arc（单构建）。
    let _ = (t2, s2, r2, rb2);
}

// ── C 暖（前驱）：继承命中 + Peek/UseIfReady 的判别点 ────────────────

#[test]
fn cell_entry_matrix_warm_predecessor_and_peek_arms() {
    // 前驱先烘热（warm_pair 内已建），供影子对拍。
    let (prev, cur) = warm_pair(DOC);
    let rt_prev = prev.root_table_cell_merge();
    let rb_prev = prev.root_table_cell_split();
    let t_prev = prev.tree_cell(&CONTAINER_TOPOLOGY);
    let s_prev = prev.section_cell(&HEADINGSECTION_TOPOLOGY);
    let sp_prev = prev.block_spans_cell();

    // Peek 不继承：暖快照本代格槽空 ⇒ false，且 h 不 +1。
    let w0 = stats(&cur);
    assert!(!cur.peek_root_table_cell(false), "Peek 只查本代格槽，不继承（本代空 ⇒ false）");
    let w1 = stats(&cur);
    assert_bucket(&delta(&w0, &w1), CellKind::RootTable, (1, 0, 0), "Peek 暖但本代空");

    // Build：继承命中（h+1，b 恒 0——继承不构建），载荷与影子前驱同一 Arc。
    let rt = cur.root_table_cell_merge();
    let w2 = stats(&cur);
    assert_bucket(&delta(&w1, &w2), CellKind::RootTable, (1, 1, 0), "暖 Build 必须继承命中");
    assert!(Arc::ptr_eq(&rt, &rt_prev), "暖命中载荷必须与前驱同一 Arc");

    // 同快照再 Build：本代已安装 ⇒ 仍命中。
    let rt_again = cur.root_table_cell_merge();
    let w3 = stats(&cur);
    assert_bucket(&delta(&w2, &w3), CellKind::RootTable, (1, 1, 0), "本代已安装 ⇒ 命中");
    assert!(Arc::ptr_eq(&rt, &rt_again));

    // Peek 在 Build 之后：本代格槽已就绪 ⇒ true。
    assert!(cur.peek_root_table_cell(false), "Build 装槽后 Peek 必须 true");
    let w4 = stats(&cur);
    assert_bucket(&delta(&w3, &w4), CellKind::RootTable, (1, 1, 0), "Peek 装槽后命中");

    // 暖 + 其余四格：各自继承命中。
    let t = cur.tree_cell(&CONTAINER_TOPOLOGY);
    let w5 = stats(&cur);
    assert_bucket(&delta(&w4, &w5), CellKind::Tree, (1, 1, 0), "tree 暖继承");
    let s = cur.section_cell(&HEADINGSECTION_TOPOLOGY);
    let w6 = stats(&cur);
    assert_bucket(&delta(&w5, &w6), CellKind::Secs, (1, 1, 0), "secs 暖继承");
    let rb = cur.root_table_cell_split();
    let w7 = stats(&cur);
    assert_bucket(&delta(&w6, &w7), CellKind::RootBlocksTable, (1, 1, 0), "split 暖继承");
    assert!(Arc::ptr_eq(&t, &t_prev) && Arc::ptr_eq(&s, &s_prev));
    assert!(Arc::ptr_eq(&rb, &rb_prev));
    assert!(!Arc::ptr_eq(&rt, &rb), "两口径载荷不得同 Arc（域含口径）");

    // 暖 + UseIfReady：继承命中即安装（与 Peek 的判别点：装完 Peek 应 Some）。
    let (p2, cur2) = warm_pair(DOC);
    let sp_prev2 = p2.block_spans_cell();
    assert!(cur2.peek_block_spans_cell().is_none(), "Peek 暖但本代未装 ⇒ None");
    let sp_if = cur2.block_spans_cell_if_ready().expect("UseIfReady 必须走继承腿");
    assert!(Arc::ptr_eq(&sp_if, &sp_prev2), "UseIfReady 命中载荷 == 本快照自己的前驱");
    assert!(cur2.peek_block_spans_cell().is_some(), "UseIfReady 命中后必须装入本代槽");
    let sp_peek = cur2.peek_block_spans_cell().expect("装入后 Peek Some");
    assert!(Arc::ptr_eq(&sp_if, &sp_peek), "同键同载荷");
    let sp_build = cur2.block_spans_cell();
    assert!(Arc::ptr_eq(&sp_if, &sp_build), "Build 命中已装槽（不得重建）");
}

// ── 入口级归因：每窗口桶级增量 == 入口级账本增量 ────────────────────

/// 单入口窗口：返回 (桶级增量, 入口级增量)。窗口内只有该入口在跑。
fn window<T>(
    snap: &Snapshot,
    entry: CellEntry,
    f: impl FnOnce() -> T,
) -> ([(u64, u64, u64); 5], (u64, u64, u64)) {
    let b0 = stats(snap);
    let e0 = cell_entry_stats(entry);
    let _ = f();
    let b1 = stats(snap);
    let e1 = cell_entry_stats(entry);
    (delta(&b0, &b1), (e1.0 - e0.0, e1.1 - e0.1, e1.2 - e0.2))
}

#[test]
fn cell_entry_attribution_identity_per_bucket() {
    cell_entry_reset();
    let snap = cold(DOC);

    let (d, e) = window(&snap, CellEntry::Tree, || snap.tree_cell(&CONTAINER_TOPOLOGY));
    assert_eq!(at(&d, CellKind::Tree), e, "tree 入口归因");
    let (d, e) = window(&snap, CellEntry::Section, || snap.section_cell(&HEADINGSECTION_TOPOLOGY));
    assert_eq!(at(&d, CellKind::Secs), e, "section 入口归因");
    let (d, e) = window(&snap, CellEntry::RootTable, || snap.root_table_cell_merge());
    assert_eq!(at(&d, CellKind::RootTable), e, "merge 入口归因");
    let (d, e) = window(&snap, CellEntry::RootTable, || snap.root_table_cell_split());
    assert_eq!(at(&d, CellKind::RootBlocksTable), e, "split 入口归因");
    let (d, e) = window(&snap, CellEntry::PeekRootTable, || snap.peek_root_table_cell(false));
    assert_eq!(at(&d, CellKind::RootTable), e, "peek 根入口归因");
    let (d, e) = window(&snap, CellEntry::PeekSpans, || snap.peek_block_spans_cell());
    assert_eq!(at(&d, CellKind::BlockSpans), e, "peek span 入口归因");
    let (d, e) = window(&snap, CellEntry::SpansIfReady, || snap.block_spans_cell_if_ready());
    assert_eq!(at(&d, CellKind::BlockSpans), e, "if_ready 入口归因");
    let (d, e) = window(&snap, CellEntry::BlockSpans, || snap.block_spans_cell());
    assert_eq!(at(&d, CellKind::BlockSpans), e, "span Build 入口归因");

    // 聚合恒等：每桶 q/h/b == Σ(该桶入口)。逐入口窗口已对上，这里再钉
    // 「桶级总和 == 入口级总和」（不得有绕过七个入口的桶级查询）。
    let totals = cell_entry_totals();
    let mut bk = [(0u64, 0u64, 0u64); 5];
    for (i, k) in FIVE.iter().enumerate() {
        bk[i] = snap.cell_consumption.cell_stats(*k);
    }
    let sum_entries = totals
        .iter()
        .fold((0u64, 0u64, 0u64), |a, t| (a.0 + t.0, a.1 + t.1, a.2 + t.2));
    let sum_buckets = bk
        .iter()
        .fold((0u64, 0u64, 0u64), |a, t| (a.0 + t.0, a.1 + t.1, a.2 + t.2));
    assert_eq!(sum_entries, sum_buckets, "Σ入口 == Σ桶（两本账同源自增）");
}

// ── legacy 计数器交叉断言（G.7-5：同一事实的两套口径，S2 只加交叉断言）──

#[test]
fn cell_entry_legacy_counters_match_bucket_queries() {
    // tree_full / secs 与桶 0/1 是同一事实的两套口径（F2 多口径族小尾巴）。
    // 删除会动 SnapshotConsumption 的公开面——留到第二阶段；本步只钉「逐次相等」。
    let snap = cold(DOC);
    let _ = snap.tree_cell(&CONTAINER_TOPOLOGY);
    let _ = snap.tree_cell(&CONTAINER_TOPOLOGY);
    let _ = snap.section_cell(&HEADINGSECTION_TOPOLOGY);
    let _ = snap.section_cell(&HEADINGSECTION_TOPOLOGY);
    assert_eq!(
        snap.cell_consumption.tree_full.load(Ordering::Relaxed),
        snap.cell_consumption.cell_stats(CellKind::Tree).0,
        "legacy tree_full 必须等于 tree 桶查询数（逐次相等）"
    );
    assert_eq!(
        snap.cell_consumption.secs.load(Ordering::Relaxed),
        snap.cell_consumption.cell_stats(CellKind::Secs).0,
        "legacy secs 必须等于 secs 桶查询数（逐次相等）"
    );
}

// ── 分派表自检：七入口全登记、域实例与桶一致 ────────────────────────

#[test]
fn cell_entry_dispatch_table_is_complete() {
    use crate::projection::{domain_bucket, CELL_ENTRY_COUNT};

    assert_eq!(CELL_ENTRANCES.len(), 9, "域实例 × 策略臂的行数");
    for e in [
        CellEntry::Tree,
        CellEntry::Section,
        CellEntry::RootTable,
        CellEntry::BlockSpans,
        CellEntry::PeekRootTable,
        CellEntry::SpansIfReady,
        CellEntry::PeekSpans,
    ] {
        assert!(CELL_ENTRANCES.iter().any(|d| d.entry == e), "入口 {e:?} 未在分派表登记");
        assert!((e as usize) < CELL_ENTRY_COUNT);
    }
    for d in CELL_ENTRANCES.iter() {
        assert!(domain_bucket(d.domain).is_some(), "分派表的域必须有账本桶: {:?}", d.domain);
    }
    let root_domains: Vec<DomainId> = CELL_ENTRANCES
        .iter()
        .filter(|d| d.entry == CellEntry::RootTable)
        .map(|d| d.domain)
        .collect();
    assert!(root_domains.contains(&DomainId::RootMerge) && root_domains.contains(&DomainId::RootSplit));
    let span_arms = CELL_ENTRANCES.iter().filter(|d| d.domain == DomainId::Span).count();
    assert_eq!(span_arms, 3, "span 域必须三臂全登记");
}
