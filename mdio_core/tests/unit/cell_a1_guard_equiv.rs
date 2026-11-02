// cell_a1_guard_equiv — A-1 守卫差分判官（附录 G.5 末段；S2 交付项）
//
// 守卫（incr.rs::diff_patches_mode_with_ctx_g 计算并传入 root_offsets）：
//     reuse_new_block_start := zs_o == zs_n && zs_o + 1 < task.ripple_line_start
// 成立时 [0, zs] 前缀逐字节恒等 ⇒ 新侧块起点 = 旧侧块起点，生产**跳过**
// 新侧 block_start_before（直接复用 zs_o_a）。本判官守着该守卫路径
// （reuse_new_block_start 在测试里零引用，既有 block_start_before_matches_probe
// 只对拍函数本体、守卫路径根本不调它）。
//
// 本判官两条腿：
//   ① 差分：直接调 ZoneTransform::block_start_before(snap_new, zs_n) 与
//      被复用的 block_start_before(snap_old, zs_o) 比对——守卫成立时必须相等。
//   ② 行为见证（入口级账本）：走生产入口 patches_for_mode——守卫成立时新侧
//      跨度入口（UseIfReady）q == 0（确实跳过了查询）、旧侧 Build 入口 q ≥ 1；
//      守卫不成立时新侧必须 q ≥ 1（回退原路径）。span 在 Edit 走 UseIfReady
//      ⇒ b ≡ 0，白名单只能靠 q 区分「跳过」与「调用」（G.5-A1）。
//
// include! 进 incr.rs 的 incr_tests 模块（外层 use super::* —— 模块私有件可命名）。

use std::sync::Arc;

use crate::cache::{CacheNode, CacheSlot, CacheSlotHandle};
use crate::incr::{compute_zone, patches_for_mode, IncrTask, LineMap};
use crate::projection::{cell_entry_reset, cell_entry_stats, CellEntry, State};
use crate::topology::{ProjectionMode, CONTAINER_TOPOLOGY};

use super::ZoneTransform;

const CASES: &[(&str, &str, &str, &str)] = &[
    // (名字, 旧文档, 编辑目标子串, 替换文本)
    ("list_last_item", "- a\n- b\n- c\n", "c", "C"),
    ("para_last_line", "head\n\nline1\nline2\nline3\n\ntail\n", "line3", "LINE3"),
    ("quote_last_line", "> q1\n> q2\n> q3\n", "q3", "Q3"),
    ("para_first_line", "para alpha\npara beta\n", "alpha", "ALPHA"),
];

fn case_task(old_text: &str, s: usize, e: usize, ins: &str) -> IncrTask {
    let mut state = State::new(old_text);
    let mut old = Arc::new(state.snapshot());
    state.edit(s, e, ins);
    let mut new = Arc::new(state.snapshot());
    let diff = state.ripple_diff.take().expect("ripple diff");
    let anchor = old.line_of_byte(s.min(old.byte_len().saturating_sub(1)));
    let edit_ranges = crate::cache::edit_actions_arc(vec![(s, e)]);

    let old_slot = Arc::new(CacheSlot::new_with_id(1));
    Arc::get_mut(&mut old)
        .unwrap()
        .set_cache_slot(Arc::downgrade(&old_slot));
    old_slot
        .node
        .store(Some(Arc::new(CacheNode::new_base(old.clone()))));

    let new_slot = Arc::new(CacheSlot::new_with_id(2));
    Arc::get_mut(&mut new)
        .unwrap()
        .set_cache_slot(Arc::downgrade(&new_slot));
    let rle_e = (diff.ripple_line_end + 1).min(new.line_len());
    let node = CacheNode::new_edit_full(
        &old,
        new.clone(),
        (diff.ripple_line_start, rle_e),
        edit_ranges.clone(),
        diff.delta,
        anchor,
    );
    new_slot.node.store(Some(Arc::new(node)));

    IncrTask {
        snap: new,
        doc_id: 0,
        snap_old: old,
        delta: diff.delta,
        edit_ranges,
        ripple_line_start: diff.ripple_line_start,
        ripple_line_end: diff.ripple_line_end,
        slot: Some(CacheSlotHandle(new_slot)),
        slot_old: Some(CacheSlotHandle(old_slot)),
        reaper_tx: None, structural_change: false,
    }
}

#[test]
fn a1_guard_reuse_equals_new_side_block_start() {
    let mut holds = 0usize;
    let mut fails = 0usize;

    for (name, old_text, target, ins) in CASES {
        let s = old_text.find(target).unwrap_or_else(|| panic!("{name}: 找不到目标子串"));
        let e = s + target.len();
        let task = case_task(old_text, s, e, ins);
        let line_map = LineMap::new(&task);
        let zone = compute_zone(&task, &line_map);
        let guard = zone.zs_o == zone.zs_n
            && zone.zs_o.saturating_add(1) < task.ripple_line_start;

        // ② 行为见证：先跑生产入口（窗口内只有生产路径在跑），再读入口账。
        cell_entry_reset();
        let patches = patches_for_mode(&task, ProjectionMode::Cf);
        let if_ready_q = cell_entry_stats(CellEntry::SpansIfReady).0;
        let build_q = cell_entry_stats(CellEntry::BlockSpans).0;

        // ① 差分：被复用的旧侧值 vs 新侧独立计算值。
        let a_o = ZoneTransform::block_start_before(&task.snap_old, zone.zs_o);
        let a_n = ZoneTransform::block_start_before(&task.snap, zone.zs_n);

        println!(
            "[a1_guard] case={name} zs_o={} zs_n={} rls={} guard={guard} if_ready_q={if_ready_q} build_q={build_q} a_o={a_o} a_n={a_n} patches={}",
            zone.zs_o,
            zone.zs_n,
            task.ripple_line_start,
            patches.len()
        );

        if guard {
            holds += 1;
            assert_eq!(
                if_ready_q, 0,
                "{name}: 守卫成立 ⇒ 生产不得查询新侧跨度格（A-1 复用必须生效）"
            );
            assert!(build_q >= 1, "{name}: 旧侧块起点查询必须经 Build 入口（q 是存活见证）");
            assert_eq!(
                a_o, a_n,
                "{name}: 守卫成立时 block_start_before(new, zs_n) 必须等于被复用的 zs_o_a"
            );
        } else {
            fails += 1;
            assert!(
                if_ready_q >= 1,
                "{name}: 守卫不成立 ⇒ 新侧必须走原路径（UseIfReady q ≥ 1，实得 {if_ready_q}）"
            );
        }
    }

    assert!(holds >= 1, "判官必须覆盖守卫成立的场景（否则差分腿空跑）");
    assert!(fails >= 1, "判官必须覆盖守卫不成立的对照场景（否则 q==0 见证无判别力）");
}
