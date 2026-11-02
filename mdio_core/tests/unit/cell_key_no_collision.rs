// cell_key_no_collision — S2 键不相撞判据（附录 G.4）
//
// 两口径（合并 / 拆分）同区间同拓扑，只差域实例；若都映射到同一个「根」域
// 就会撞键：先查者占槽、后查者命中同一载荷（静默错值）。
//
// 判据形态（G.4 强制）：
//   * 必须用**新快照**（prev_snapshot = Weak::new ⇒ 继承腿不可用）+ **构建策略**，
//     否则继承命中会伪装成撞键；
//   * 必须用 built 计数判隔离，**不得用值比较**——某些文档两口径内容相同；
//   * 正反两序各做一遍（对称）+ 暖快照下两口径各自 ptr_eq 自己的前驱。
// 红灯含义（G.4）：第二次查询得 (1,1,0) ⇒ 命中了合并口径的槽（漏口径入键）；
// 得 (0,0,0) ⇒ 同键直复。

use std::sync::Arc;

use crate::projection::{CellKind, Snapshot, State};

const DOC: &str = "# H\n\npara one\n\npara two\n";
/// setext 拆分的语料：两口径内容不同（值比较的补充面，不是隔离判据）。
const DOC_SETEXT: &str = "# H\n\npara one\n===\n\npara two\n";

fn cold(text: &str) -> Snapshot {
    State::new(text).snapshot()
}

fn stats(snap: &Snapshot) -> [(u64, u64, u64); 5] {
    [
        snap.cell_consumption.cell_stats(CellKind::Tree),
        snap.cell_consumption.cell_stats(CellKind::Secs),
        snap.cell_consumption.cell_stats(CellKind::RootTable),
        snap.cell_consumption.cell_stats(CellKind::RootBlocksTable),
        snap.cell_consumption.cell_stats(CellKind::BlockSpans),
    ]
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

/// 断言只有 rtbl 桶变化，且变化恰为期望值；其余桶全 0。
fn only_rtbl(d: &[(u64, u64, u64); 5], want: (u64, u64, u64), ctx: &str) {
    assert_eq!(d[2], want, "{ctx}: rtbl 桶期望 {want:?} 实得 {:?}", d[2]);
    for (i, x) in d.iter().enumerate() {
        if i != 2 {
            assert_eq!(*x, (0, 0, 0), "{ctx}: 桶 {i} 不得有变化（撞键或串桶）");
        }
    }
}

fn only_rblk(d: &[(u64, u64, u64); 5], want: (u64, u64, u64), ctx: &str) {
    assert_eq!(d[3], want, "{ctx}: rblk 桶期望 {want:?} 实得 {:?}", d[3]);
    for (i, x) in d.iter().enumerate() {
        if i != 3 {
            assert_eq!(*x, (0, 0, 0), "{ctx}: 桶 {i} 不得有变化（撞键或串桶）");
        }
    }
}

// ── 正序：先合并口径，后拆分口径 ────────────────────────────────────

#[test]
fn cell_key_calibers_do_not_collide_fresh_forward() {
    let snap = cold(DOC);
    assert!(snap.prev_snapshot.upgrade().is_none(), "新快照：继承腿必须不可用");
    assert!(snap.cache_slot.upgrade().is_none(), "新快照：无槽 ⇒ 无节点级路径");

    let s0 = stats(&snap);
    let rt = snap.root_table_cell_merge();
    let s1 = stats(&snap);
    only_rtbl(&delta(&s0, &s1), (1, 0, 1), "先查合并口径：必须自建 rtbl");

    let rbt = snap.root_table_cell_split();
    let s2 = stats(&snap);
    only_rblk(&delta(&s1, &s2), (1, 0, 1), "后查拆分口径：必须产生自己的构建计数");
    assert_eq!(
        delta(&s1, &s2)[2],
        (0, 0, 0),
        "拆分口径不得被合并口径的槽吸收（Δrtbl 必须为 0）"
    );
    assert!(!Arc::ptr_eq(&rt, &rbt), "两口径必须各自成键（不得同 Arc）");
}

// ── 反序：先拆分口径，后合并口径（对称）────────────────────────────

#[test]
fn cell_key_calibers_do_not_collide_fresh_reverse() {
    let snap = cold(DOC);
    assert!(snap.prev_snapshot.upgrade().is_none());

    let s0 = stats(&snap);
    let rbt = snap.root_table_cell_split();
    let s1 = stats(&snap);
    only_rblk(&delta(&s0, &s1), (1, 0, 1), "先查拆分口径：必须自建 rblk");

    let rt = snap.root_table_cell_merge();
    let s2 = stats(&snap);
    only_rtbl(&delta(&s1, &s2), (1, 0, 1), "后查合并口径：必须产生自己的构建计数");
    assert_eq!(delta(&s1, &s2)[3], (0, 0, 0), "合并口径不得被拆分口径的槽吸收");
    assert!(!Arc::ptr_eq(&rt, &rbt));
}

// ── 暖快照：两口径各自命中自己的前驱（不串）─────────────────────────

#[test]
fn cell_key_calibers_do_not_collide_warm() {
    let prev = Arc::new(cold(DOC));
    let rt_prev = prev.root_table_cell_merge();
    let rb_prev = prev.root_table_cell_split();
    assert!(!Arc::ptr_eq(&rt_prev, &rb_prev));

    let mut cur = cold(DOC);
    cur.prev_snapshot = Arc::downgrade(&prev);

    let w0 = stats(&cur);
    let rt = cur.root_table_cell_merge();
    let w1 = stats(&cur);
    only_rtbl(&delta(&w0, &w1), (1, 1, 0), "暖：合并口径继承命中（h+1/b+0）");
    assert!(Arc::ptr_eq(&rt, &rt_prev), "合并口径必须命中自己的前驱");

    let rb = cur.root_table_cell_split();
    let w2 = stats(&cur);
    only_rblk(&delta(&w1, &w2), (1, 1, 0), "暖：拆分口径继承命中（h+1/b+0）");
    assert_eq!(delta(&w1, &w2)[2], (0, 0, 0), "拆分查询不得动 rtbl 桶");
    assert!(Arc::ptr_eq(&rb, &rb_prev), "拆分口径必须命中自己的前驱（不串）");
    assert!(!Arc::ptr_eq(&rt, &rb));
}

// ── 值比较陷阱（G.4 明令）──────────────────────────────────────────
//
// 某些文档（含 setext 形态）两口径内容**相同**——实测 "# H\n\npara one\n===\n\npara
// two\n" 上两口径逐值相等。因此隔离判据**只能是 built 计数 + !Arc::ptr_eq**
// （上面三条测试），绝不能用值比较；下面的常量保留为一处留档语料。

#[test]
fn cell_key_value_comparison_is_not_an_isolation_criterion() {
    // 留档：此语料两口径值相同——若把它们映到同一个域会「看起来没问题」，
    // 而 built 计数立刻判红（这正是 G.4 用计数而非值比较的原因）。
    let snap = cold(DOC_SETEXT);
    assert!(snap.prev_snapshot.upgrade().is_none());
    let rt = snap.root_table_cell_merge();
    let rbt = snap.root_table_cell_split();
    assert!(!Arc::ptr_eq(&rt, &rbt), "即使值相同，两口径也必须是两个独立载荷");
    assert_eq!(rt.as_slice(), rbt.as_slice(), "留档语料：两口径逐值相等（不是隔离判据）");
}
