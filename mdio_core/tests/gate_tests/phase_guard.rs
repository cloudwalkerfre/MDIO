#![deny(unfulfilled_lint_expectations)]
//! 相位拆分判官（层 a：lint 级不变式 + 类型级静态断言；零子进程）。
//!
//! 机制：`#[expect(lint)]` 的期望必须被兑现，否则本 crate 顶部的 deny 把它升级为硬错。
//! 两个探针函数故意**不接线**（只在类型层与 lint 层存在）：一旦有人把它们接进生产路径，
//! `dead_code` 不再触发 ⇒ 期望落空 ⇒ 编译失败，强制回到相位清单复核。
//!
//! 相位违规的「必须编译失败」由 effect.rs 上 `LineResolveView` 的 compile_fail doctest 承担
//! （层 b，跑 `cargo test --doc`）；本文件承担层 a（lint 不变式）与类型级正向断言。

use mdio_core::effect::{LineResolveView, ResolveView};
use mdio_core::infer::{BuildView, LineView};

/// 层 (a) 探针一：行相位视图只能问行相位原语（合成用例，不接生产）。
#[expect(dead_code, reason = "synthetic phase probe; must stay unwired")]
fn line_phase_probe(v: &dyn LineResolveView) -> bool {
    v.table_confirmed_at(0)
}

/// 层 (a) 探针二：类型级正向断言——两个视图都必须实现行相位，且 LineView 必须实现结构就绪相位。
/// （反向命题「BuildView 不实现 ResolveView」不可用稳定 Rust 的负实现表达，由 compile_fail doctest 判。）
#[expect(dead_code, reason = "compile-time assertions; never called at runtime")]
fn phase_split_static_assertions() {
    fn assert_line<T: LineResolveView + ?Sized>() {}
    fn assert_post<T: ResolveView + ?Sized>() {}
    assert_line::<LineView<'static>>();
    assert_line::<BuildView<'static>>();
    assert_post::<LineView<'static>>();
}
