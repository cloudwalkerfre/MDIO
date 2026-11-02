// omp 判官：切换语义（独立进程）。
//
// 为什么独立一个 integration 文件：
//   `set_mode` 写的是**进程全局**（ArcSwap）。放在 shared lib 测试二进制里，它会与
//   其它并发读取 `process_mode()` 的用例互相干扰 —— 并行跑
//   `cargo test --workspace --lib` 时偶发失败、且每次失败集合不同
//   （4 / 1 / 5 条），单线程与逐条隔离全绿；`MDIO_PROJ_MODE=cf` 对照下同样红
//   ⇒ 先天竞态，与缺省模式无关。
//   cargo 对每个 integration 文件各起一个进程 ⇒ 本文件的写不越界。
//
// 语义：进程内模式唯一；生产通道 = 进程环境变量
// （启动读一次），`set_mode` 仅测试/诊断。切换必须**复原初值**——缺省已可能是
// Hs，写死"恢复 Cf"不等于复原。

use mdio_core::topology::{process_mode, set_mode, ProjectionMode};

/// t1：切换语义——Hs → Bf → Cf 全部可设可读（幂等），最后**复原初值**。
#[test]
fn set_mode_switches_all_variants() {
    let original = process_mode();
    set_mode(ProjectionMode::Hs);
    assert_eq!(process_mode(), ProjectionMode::Hs, "set_mode(Hs) 后必须 Hs");
    set_mode(ProjectionMode::Bf);
    assert_eq!(process_mode(), ProjectionMode::Bf, "set_mode(Bf) 后必须 Bf");
    set_mode(ProjectionMode::Cf);
    assert_eq!(process_mode(), ProjectionMode::Cf, "set_mode(Cf) 后必须 Cf");
    set_mode(original);
    assert_eq!(
        process_mode(),
        original,
        "必须复原为进入本用例时的模式（缺省可能是 Hs）"
    );
}
