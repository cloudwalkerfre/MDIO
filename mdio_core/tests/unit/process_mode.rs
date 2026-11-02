// omp S2/S3 判官：进程级 ProcessMode 判官（进程唯一模式语义）。
//
// 语义（omp_process_mode.md §2）：进程内模式唯一；生产通道 = 进程环境变量
// （启动时读取一次）；`set_mode` 仅测试/诊断。
//
// 判官设计说明（实现中确立，替代 per-doc 时代的矩阵）：
//   - 切换语义（t1）与"初始默认来自 env"两个判官都放在**独立 integration 文件**
//     （`tests/omp_process_mode_switch.rs` / `tests/omp_process_mode_env.rs`）——
//     每个 integration 文件独立进程，`set_mode` 的进程全局写不越界。
//     实测：t1 留在本文件（shared lib 二进制）时，并行跑会与并发读
//     `process_mode()` 的用例互相干扰，失败集合每次不同；单线程与逐条隔离全绿。
//   - patches_for_mode 的 Bf 回落（分派契约）与 per-doc 无关，保留。

use crate::store::patches_for_mode;
use crate::topology::ProjectionMode;

/// t1（切换语义）已迁出到独立 integration 文件
/// `tests/omp_process_mode_switch.rs`：`set_mode` 写进程全局，放在 shared lib
/// 二进制里会与并发读 `process_mode()` 的用例互相干扰（实测并行 sporadically 红、
/// 单线程全绿；`MDIO_PROJ_MODE=cf` 对照同样红 ⇒ 先天竞态）。

/// t2：渲染层分派契约（保留自 per-doc 判官）——同一 IncrTask：
/// Hs → hs_patches；Cf / Bf → incr_patches（CF 事实层）。
#[test]
fn patches_for_mode_dispatches_by_mode() {
    let mut state = crate::projection::State::new("# H\n\ntext\n");
    let old = std::sync::Arc::new(state.snapshot());
    let pos = "# H\n\n".len();
    state.edit(pos, pos, "X");
    let snap = std::sync::Arc::new(state.snapshot());
    let diff = state.ripple_diff.take().expect("ripple diff");
    let task = crate::incr::IncrTask {
        snap: snap.clone(),
        doc_id: 7,
        snap_old: old.clone(),
        delta: diff.delta,
        edit_ranges: crate::cache::edit_actions_arc(vec![(pos, pos)]),
        ripple_line_start: diff.ripple_line_start,
        ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    assert_eq!(
        patches_for_mode(&task, ProjectionMode::Hs),
        crate::incr::hs_patches(&task),
        "Hs → hs_patches"
    );
    assert_eq!(
        patches_for_mode(&task, ProjectionMode::Cf),
        crate::incr::incr_patches(&task),
        "Cf → incr_patches"
    );
    assert_eq!(
        patches_for_mode(&task, ProjectionMode::Bf),
        crate::incr::incr_patches(&task),
        "Bf（非 Hs）回落 incr_patches（CF 事实层）"
    );
}
