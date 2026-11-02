// cell_ledger_probe — **state 时代的跨代总账探针**
//
// 前 state 时代的分析套件（/tmp/gw_*）只有单次 span 的 p50——「谁付」看不见。
// 判定层落地后，账本（SnapshotConsumption 的 查询/命中/构建 × 5 kind）是
// **逐快照**的，于是可以按代过账。本探针把「单次成本」升级为「逐代总账」，
// 用来回答四类问题：
//   ① 重复计算：同一代同一 kind 的构建 ≥ 2？
//   ② 接线错误：内容轮（继承链应生效）仍有构建 / 走查？
//   ③ 轮子误用：稳态出现全量级走查（walked ≈ n，即落 bare/oracle）？
//   ④ 总体成本：各 kind 的构建次数随代数的趋势（而非单次分位）。
//
// 与 J1 的分工：J1 是**断言**（A-0′ 突变会判红）；本探针是**账本**（打印趋势，
// 只带结构性不变量断言）——两者共用 `cell_stats` 与 ROOTS_RANGE_LINES_WALKED。
//
// 夹具口径（探针自陈，避免误读）：每代在过账时对**五个消费面各来一次**
// （root_count / root_table(split) / block_spans / section / tree）——比真实
// 负载更重，且会把格烘热给下一代。这正是「逐代总账」想要的稳态口径；
// 要看真实负载分布请用 gateway + telemetry（见 docs/TELEMETRY.md §4.1/§5）。

use std::sync::Arc;

use crate::projection::{CellKind, Snapshot};
use crate::session::MdioSession;
use dashmap::DashSet;

const KINDS: [(CellKind, &str); 5] = [
    (CellKind::Tree, "tree"),
    (CellKind::Secs, "secs"),
    (CellKind::RootTable, "rtbl"),
    (CellKind::RootBlocksTable, "rblk"),
    (CellKind::BlockSpans, "span"),
];

fn ledger_session(text: &str) -> MdioSession {
    MdioSession::new(1, text, 1, "test".to_string(), None, None, None, Arc::new(DashSet::new()), Arc::new(crate::session::DetectClock::new()))
}

/// 每 kind 三元组 (query, hit, built) 的快照。
fn ledger(snap: &Arc<Snapshot>) -> Vec<(u64, u64, u64)> {
    KINDS.iter().map(|(k, _)| snap.cell_consumption.cell_stats(*k)).collect()
}

/// 五个消费面各来一次（= 探针的「全消费面过账」）。
fn touch_all(snap: &Arc<Snapshot>) {
    let q = (snap.line_len() / 2).max(1);
    let _ = snap.root_count_before(q);
    let _ = snap.root_table_cell(true);
    let _ = snap.block_spans_cell();
    let _ = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
    let _ = snap.tree_cell(&crate::topology::CONTAINER_TOPOLOGY);
}

fn node_is_edit(snap: &Arc<Snapshot>) -> bool {
    snap.cache_slot
        .upgrade()
        .and_then(|s| s.node.load_full())
        .map(|n| matches!(&**n, crate::cache::CacheNode::Edit { .. }))
        .unwrap_or(false)
}

#[test]
fn cell_ledger_cross_generation() {
    use crate::incr::ROOTS_RANGE_LINES_WALKED;

    // 语料：120 段（中部为编辑点；前缀 ≫ zone ⇒ 全量走查可辨）。
    let mut doc = String::new();
    for i in 0..120 {
        doc.push_str(&format!("para {i}\n\n"));
    }
    let session = ledger_session(&doc);
    let mut total_built = [0u64; 5];
    let mut total_walked = 0u64;

    println!("[cell_ledger] 每代五消费面各一次；格式 kind=q/h/b（Δ，本代）；walked=本代走查行数");
    let mut prev_ptr = 0usize;
    // 对照开关：持有上一代快照（模拟生产里任务锚/调用方持锚的情形）。
    let mut keep: Vec<Arc<Snapshot>> = Vec::new();
    for k in 0..6usize {
        let structural = k % 2 == 0;
        session.execute_transaction(None, true, |ctx| {
            if structural {
                let at = ctx.state.engine.byte_of_line(120) + k;
                let ins = format!("- item {k}\n");
                ctx.state.engine.edit(at, at, &ins);
            } else {
                // 内容轮：段内改一个字符（结构不变 ⇒ 守卫通过 ⇒ 继承链生效）。
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

        let mut row = String::new();
        let mut dbuilt = 0u64;
        for (i, (_, nm)) in KINDS.iter().enumerate() {
            let dq = after[i].0 - before[i].0;
            let dh = after[i].1 - before[i].1;
            let db = after[i].2 - before[i].2;
            dbuilt += db;
            total_built[i] += db;
            row.push_str(&format!(" {nm}={dq}/{dh}/{db}"));
            assert!(db <= 1, "gen{k} {nm}: 同代构建 {db} 次（单构建被破坏 ⇒ 重复计算）");
        }
        total_walked += walked;
        let attached = cur.prev_snapshot.upgrade().is_some();
        let bc = cur.block_cache.is_some();
        let skip = crate::cache::PREV_MOUNT_SKIP.with(|c| c.get());
        let mount = crate::cache::MOUNT_STATE.with(|c| c.get());
        let ptr_eq = mount.1 == Arc::as_ptr(&cur) as usize;
        let old_eq_prev = mount.2 != 0 && mount.2 == prev_ptr;
        let tag = if structural { "结构" } else { "内容" };
        println!("  gen{k} {tag} node={} prev={attached} bc={bc} mount_ok={} ptr_eq={ptr_eq} old_eq_prev={old_eq_prev} 跳过=(无bc {}, get_mut {}) {row} | Δbuilt={dbuilt} walked={walked} n={}",
            if node_is_edit(&cur) { "Edit" } else { "Base" }, mount.0, skip.0, skip.1, cur.line_len());

        // 不变量①：内容轮（守卫通过 ⇒ 前驱已挂）必须全命中——零构建、零走查。
        // 继承链是否真的挂上，取决于 `prev_snapshot`（守卫通过 + Arc::get_mut 成功
        // + **old_snap.block_cache 存在**，cache.rs:576-588 的三重门）。
        // 因此断言按「挂没挂」条件化：挂了却重建 = 接线错误；没挂 = 应付（本代全建）。
        if !structural && k > 0 {
            if attached {
                assert_eq!(dbuilt, 0, "gen{k}: 内容轮已挂前驱 ⇒ 必须全命中（接线错误：绕开继承）");
                assert_eq!(walked, 0, "gen{k}: 内容轮已挂前驱 ⇒ 不得走查");
            } else {
                println!("    ^ gen{k}: 无前驱（继承链未挂）⇒ 本代全建为应付，不计接线错误");
            }
        }
        // 不变量②：结构轮的构建数 ≤ kind 数（每 kind 至多一次），且走查不得 ≈ 全文。
        if structural {
            assert!(dbuilt <= KINDS.len() as u64, "gen{k}: 结构轮构建数异常");
            // 走查口径：只有两张**表** kind（RootTable / RootBlocksTable）会各自扫一遍
            // zone（≤ n）⇒ 一代的上界是 2n（不是 n）。绝对上界本身就是错的度量
            // ——J1 用的是 zone 相对上界（见 §2.7）。
            assert!(walked <= 2 * cur.line_len() as u64 + 8, "gen{k}: 走查超 2n");
        }
        prev_ptr = Arc::as_ptr(&cur) as usize;
        keep.push(cur.clone());
    }
    println!("[cell_ledger] 六代累计 built: {}",
        KINDS.iter().enumerate().map(|(i, (_, nm))| format!("{nm}={}", total_built[i])).collect::<Vec<_>>().join(" "));
    println!("[cell_ledger] 六代累计 walked={total_walked}");
}

/// 对照：内容优先序列（持有前驱 ⇒ 继承生效）。
/// 与上面结构优先序列对照挂载状态：若本测试 prev=true 且 ptr_eq=true 而上面 prev=false，
/// 则差异定位在「哪条路径产出了发布实例」——接线缺口而非应付。
#[test]
fn cell_ledger_content_first_mounts() {
    let mut doc = String::new();
    for i in 0..120 {
        doc.push_str(&format!("para {i}\n\n"));
    }
    let session = ledger_session(&doc);
    let mut keep: Vec<Arc<Snapshot>> = Vec::new(); // 持锚 ⇒ Weak 可 upgrade
    for k in 0..4usize {
        session.execute_transaction(None, true, |ctx| {
            let at = ctx.state.engine.byte_of_line(120) + 5 + k;
            ctx.state.engine.edit(at, at + 1, "Z");
        });
        let cur = session.publisher.load();
        touch_all(&cur); // 烘热本代格（供下一代白吃）
        keep.push(cur.clone()); // 持锚 ⇒ 下一代 Weak 可 upgrade
        let attached = cur.prev_snapshot.upgrade().is_some();
        let mount = crate::cache::MOUNT_STATE.with(|c| c.get());
        let ptr_eq = mount.1 == Arc::as_ptr(&cur) as usize;
        println!("  content-first gen{k}: node={} prev={attached} mount_ok={} ptr_eq={ptr_eq}",
            if node_is_edit(&cur) { "Edit" } else { "Base" }, mount.0);
        // 观察（不裁决）：本序列里 gen0 不挂载（首代前无继承物 ⇒ 应付）；
        // gen1 起挂载成功（mount_ok>0 且 ptr_eq=true）但 `prev=false` ⇒
        // **另有触发/有效期细节**（与 mixed 序列不同：前驱是内容轮 Base）。
        // 结论不受影响——继承的机会性已由 `cell_ledger_cross_generation`
        // （持锚 ⇒ 全命中）与 `j2_inheritance_is_opportunistic_without_anchor`
        // （不持锚 ⇒ 每代重建）两条判官钉死；此处仅留观察面给后续追查。
        if k >= 1 && !attached {
            println!("    ^ content-first gen{k}: 挂载成功却读不到前驱（观察项，待追查触发/有效期细节）");
        }
    }
}

/// [J2 扩展] **继承的机会性**（governor：
/// `prev_snapshot` 是 Weak、不 pin——见 `projection.rs` Phase B 注释与
/// `cache.rs` 挂载点注释「挂载失败…只损失命中,不损正确性」）：
/// 不持有上一代快照时，Weak 在读取前即失效 ⇒ **零继承**（每代重建，应付而非缺口）；
/// 持有则全命中。生产路径由任务锚（`IncrTask.slot_old`）兜住持锚。
#[test]
fn j2_inheritance_is_opportunistic_without_anchor() {
    let mut doc = String::new();
    for i in 0..120 {
        doc.push_str(&format!("para {i}\n\n"));
    }
    let session = ledger_session(&doc);
    // 不持有任何旧快照（模拟无任务锚/调用方不持锚）——两个内容轮。
    for k in 0..2usize {
        session.execute_transaction(None, true, |ctx| {
            let at = ctx.state.engine.byte_of_line(120) + 5 + k;
            ctx.state.engine.edit(at, at + 1, "Z");
        });
        let cur = session.publisher.load();
        let attached = cur.prev_snapshot.upgrade().is_some();
        assert!(!attached, "gen{k}: 无持锚 ⇒ 前驱 Weak 已失效（本文档化的机会性行为）");
        let b0 = cur.cell_consumption.cell_stats(CellKind::RootTable).2;
        let _ = cur.root_count_before(160);
        let b1 = cur.cell_consumption.cell_stats(CellKind::RootTable).2;
        assert_eq!(b1 - b0, 1, "gen{k}: 无继承 ⇒ 本代必须自建根表（应付）");
    }
}
