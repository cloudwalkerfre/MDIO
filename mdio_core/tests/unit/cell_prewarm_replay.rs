// cell_prewarm_replay — S2 prewarm 判据（附录 G.5 + G.7-4）
//
// 两件：
//   ① 生产次序重放：先旧侧 root_count_before、后新侧；断言
//      旧侧 Δb(RootTable)==0 且 walked==0（上一代新侧构建的预热兑现）；
//      新侧结构代 Δb==1 且 walked ≤ zone_len + 8；新侧内容代 Δb==0、walked==0；
//      至少连续 2 代（否则「旧侧命中」无前代构建可对账）。
//      A-0′ 病征（G.5-⑤，不构造、只做反证面）：若新侧每代 Δb==0，则下一代
//      旧侧 Δb==1 且 walked ≈ 前缀（病历 13.90µs → 140.06µs）——本条断言
//      「新侧结构代 Δb==1」正是让该病征无法成立的机械面。
//   ② W0 跨代明细冻结（G.7-4）：把 W0 探针的每代 (q,h,b) × 5 桶与 walked
//      固化成测试内快照（W0 只给了累计值 978；单跑不必再跑 cargo 复算）。
//      本测试**不改** W0 原探针（G.3-1），只是并列的独立窗口。

use std::sync::Arc;

use crate::projection::{CellKind, Snapshot};
use crate::session::MdioSession;
use dashmap::DashSet;

const FIVE: [CellKind; 5] = [
    CellKind::Tree,
    CellKind::Secs,
    CellKind::RootTable,
    CellKind::RootBlocksTable,
    CellKind::BlockSpans,
];

fn session(text: &str) -> MdioSession {
    MdioSession::new(1, text, 1, "test".to_string(), None, None, None, Arc::new(DashSet::new()), Arc::new(crate::session::DetectClock::new()))
}

fn ledger(snap: &Arc<Snapshot>) -> [(u64, u64, u64); 5] {
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

fn ledger_session_doc() -> String {
    let mut doc = String::new();
    for i in 0..120 {
        doc.push_str(&format!("para {i}\n\n"));
    }
    doc
}

/// 本代节点是不是 Edit（结构代）；Base = 内容代。
fn is_edit(snap: &Arc<Snapshot>) -> bool {
    snap.cache_slot
        .upgrade()
        .and_then(|s| s.node.load_full())
        .map(|n| matches!(&**n, crate::cache::CacheNode::Edit { .. }))
        .unwrap_or(false)
}

/// 本代 Edit 节点的 zone 长度（G.5-② 的走查上界）。
fn zone_len(snap: &Arc<Snapshot>) -> usize {
    snap.cache_slot
        .upgrade()
        .and_then(|s| s.node.load_full())
        .and_then(|n| match &**n {
            crate::cache::CacheNode::Edit { .. } => Some(n.materialize_zone()),
            _ => None,
        })
        .map(|(zs, ce)| ce.saturating_sub(zs))
        .unwrap_or(0)
}

// ── ① 生产次序重放 ─────────────────────────────────────────────────

#[test]
fn prewarm_replay_root_table_production_order() {
    use crate::incr::ROOTS_RANGE_LINES_WALKED;

    let doc = ledger_session_doc();
    let session = session(&doc);
    let q_line = 160usize;
    let mut prev: Option<Arc<Snapshot>> = None;
    let mut keep: Vec<Arc<Snapshot>> = Vec::new();
    let (mut structural_gens, mut content_gens) = (0usize, 0usize);

    for k in 0..6usize {
        let structural = k % 2 == 0;
        session.execute_transaction(None, true, |ctx| {
            if structural {
                let at = ctx.state.engine.byte_of_line(120) + k;
                let ins = format!("- item {k}\n");
                ctx.state.engine.edit(at, at, &ins);
            } else {
                let at = ctx.state.engine.byte_of_line(120) + 5 + k;
                ctx.state.engine.edit(at, at + 1, "Z");
            }
        });
        let cur = session.publisher.load();
        let edit = is_edit(&cur);

        // ① 旧侧（生产次序第一步：root_offsets 先算旧侧）。
        if let Some(old) = prev.as_ref() {
            let (q0, h0, b0) = old.cell_consumption.cell_stats(CellKind::RootTable);
            ROOTS_RANGE_LINES_WALKED.with(|c| c.set(0));
            let got_old = old.root_count_before(q_line).expect("old idx");
            let walked_old = ROOTS_RANGE_LINES_WALKED.with(|c| c.get());
            let (q1, h1, b1) = old.cell_consumption.cell_stats(CellKind::RootTable);
            assert!(got_old <= cur.line_len(), "旧侧计数必须可查询");
            assert_eq!(b1 - b0, 0, "gen{k}: 旧侧必须命中（上一代新侧构建的预热兑现）");
            assert_eq!(walked_old, 0, "gen{k}: 旧侧命中不得有走查，实测 {walked_old}");
            assert!(q1 - q0 >= 1 && h1 - h0 >= 1, "gen{k}: 旧侧 q/h 是存活见证");
        }

        // ② 新侧（生产次序第二步）。
        let (nq0, _nh0, nb0) = cur.cell_consumption.cell_stats(CellKind::RootTable);
        ROOTS_RANGE_LINES_WALKED.with(|c| c.set(0));
        let got_new = cur.root_count_before(q_line).expect("new idx");
        let walked_new = ROOTS_RANGE_LINES_WALKED.with(|c| c.get());
        let (nq1, _nh1, nb1) = cur.cell_consumption.cell_stats(CellKind::RootTable);
        assert!(got_new <= cur.line_len(), "新侧计数必须可查询");
        assert!(nq1 - nq0 >= 1, "gen{k}: 每个非 PendingWire 域每代 q ≥ 1（存活见证）");

        assert_eq!(structural, edit, "gen{k}: 结构代必须产出 Edit 节点（zone 界与三截维护的前提）");
        if edit {
            structural_gens += 1;
            assert_eq!(nb1 - nb0, 1, "gen{k}: 结构代新侧恰好构建一次（预热链的付款面）");
            let zlen = zone_len(&cur).max(1);
            assert!(
                walked_new <= zlen as u64 + 8,
                "gen{k}: 新侧走查必须 zone 级（zone={zlen}，实测 {walked_new}，全文 n={}）",
                cur.line_len()
            );
        } else {
            content_gens += 1;
            assert!(
                cur.prev_snapshot.upgrade().is_some(),
                "gen{k}: 内容代继承链必须挂上（否则本条无判别力）"
            );
            assert_eq!(nb1 - nb0, 0, "gen{k}: 内容代新侧必须继承（0 构建）");
            assert_eq!(walked_new, 0, "gen{k}: 内容代新侧不得有走查，实测 {walked_new}");
        }

        prev = Some(cur.clone());
        keep.push(cur.clone());
    }
    assert!(structural_gens >= 2, "至少连续 2 个结构代（G.5-④），实测 {structural_gens}");
    assert!(content_gens >= 2, "至少连续 2 个内容代（G.5-④），实测 {content_gens}");
}

// ── ② W0 跨代明细冻结（G.7-4）──────────────────────────────────────

/// 五个消费面各来一次（与 W0 探针同口径：root_count / root_table(split) /
/// block_spans / section / tree）——比真实负载更重，且把格烘热给下一代。
fn touch_all(snap: &Arc<Snapshot>) {
    let q = (snap.line_len() / 2).max(1);
    let _ = snap.root_count_before(q);
    let _ = snap.root_table_cell(true);
    let _ = snap.block_spans_cell();
    let _ = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
    let _ = snap.tree_cell(&crate::topology::CONTAINER_TOPOLOGY);
}

#[test]
fn w0_generation_ledger_snapshot_is_frozen() {
    use crate::incr::ROOTS_RANGE_LINES_WALKED;

    let doc = ledger_session_doc();
    let session = session(&doc);
    let mut keep: Vec<Arc<Snapshot>> = Vec::new();

    // 冻结明细：每代 (每桶 Δ(q,h,b), walked, prev 已挂)。桶序 = tree/secs/rtbl/rblk/span。
    // 数值来源：v3.7 W0 实测（与既有探针并列的独立窗口，不改原探针）。
    let expect: [([(u64, u64, u64); 5], u64, bool); 6] = [
        ([(1, 0, 1), (1, 0, 1), (1, 0, 1), (1, 0, 1), (1, 0, 1)], 478, false),
        ([(1, 1, 0), (1, 1, 0), (1, 1, 0), (1, 1, 0), (1, 1, 0)], 0, true),
        ([(1, 0, 1), (1, 0, 1), (1, 0, 1), (1, 0, 1), (1, 0, 1)], 250, false),
        ([(1, 1, 0), (1, 1, 0), (1, 1, 0), (1, 1, 0), (1, 1, 0)], 0, true),
        ([(1, 0, 1), (1, 0, 1), (1, 0, 1), (1, 0, 1), (1, 0, 1)], 250, false),
        ([(1, 1, 0), (1, 1, 0), (1, 1, 0), (1, 1, 0), (1, 1, 0)], 0, true),
    ];
    let mut total_built = [0u64; 5];
    let mut total_walked = 0u64;

    for (k, (want, want_walked, want_prev)) in expect.iter().enumerate() {
        let structural = k % 2 == 0;
        session.execute_transaction(None, true, |ctx| {
            if structural {
                let at = ctx.state.engine.byte_of_line(120) + k;
                let ins = format!("- item {k}\n");
                ctx.state.engine.edit(at, at, &ins);
            } else {
                let at = ctx.state.engine.byte_of_line(120) + 5 + k;
                ctx.state.engine.edit(at, at + 1, "Z");
            }
        });
        let cur = session.publisher.load();
        let before = ledger(&cur);
        ROOTS_RANGE_LINES_WALKED.with(|c| c.set(0));
        touch_all(&cur);
        let walked = ROOTS_RANGE_LINES_WALKED.with(|c| c.get());
        let after = ledger(&cur);
        let d = delta(&before, &after);
        let attached = cur.prev_snapshot.upgrade().is_some();
        for i in 0..5 {
            assert_eq!(d[i], want[i], "gen{k}: 桶 {i} 明细与 W0 冻结快照不符（实得 {:?}）", d[i]);
            total_built[i] += d[i].2;
        }
        assert_eq!(walked, *want_walked, "gen{k}: walked 与 W0 冻结快照不符");
        assert_eq!(attached, *want_prev, "gen{k}: prev 挂载状态与 W0 冻结快照不符");
        total_walked += walked;
        keep.push(cur.clone());
    }
    assert_eq!(total_built, [3, 3, 3, 3, 3], "六代累计 built 每桶 = 3");
    assert_eq!(total_walked, 978, "六代累计 walked = 978（v3.7 基线）");
}
