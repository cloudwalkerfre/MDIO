// omp 判官：进程模式初始默认 = 进程 env（独立进程判官）。
//
// 设计原因：process_mode 的"初始默认"只能在进程内无 set_mode 干扰时断言；
// 本文件作为独立 integration test（cargo 对每个 integration 文件各起一个
// 进程），进程内不存在 set_mode 调用——process_mode() 首读即 env 解析值。
// 判官进程用法：
//   默认进程（无 MDIO_PROJ_MODE）→ 断言 Hs
//   env-cf 进程（MDIO_PROJ_MODE=cf）→ 断言 Cf（显式切 Cf，A/B 用）
//   env-hs 进程（MDIO_PROJ_MODE=hs）→ 断言 Hs（双模回归覆盖）

use mdio_core::topology::{process_mode, ProjectionMode};

#[test]
fn process_mode_initial_default_matches_process_env() {
    let expect = match std::env::var("MDIO_PROJ_MODE").as_deref() {
        Ok("cf") => ProjectionMode::Cf,
        _ => ProjectionMode::Hs,
    };
    assert_eq!(
        process_mode(),
        expect,
        "进程初始默认必须等于 MDIO_PROJ_MODE 解析（无 set_mode 干扰时首读即 env）"
    );
}
