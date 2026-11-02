// keepalive — S0「禁止删除清单」的 keep-alive 出口断言
//
// 机制：\`const _: fn(..) = 资产路径;\` —— 编译期把被保资产的名字与签名各钉一次；
// 删除被保资产 ⇒ 本文件编译红（E0425/E0063/E0308）。选 const 而非 #[test]：
// ① 零用例增量（闸门 ⑤ 的库内计数保持不变）；② 不依赖运行时，无抖动。
// 宿主 = store.rs 的 #[cfg(test)] mod tests（新增 include，不改既有测试内容）。
// 清单来源：dsh-notes/S0-point-list.txt 与 dsh-notes/S0-forbidden-list.txt。
//
// pyo3 导出的 Rust 侧 keep-alive 不在本文件：\`mod python_bindings\` 内的
//   \`PyProjectStore\` 与 8 个 \`#[pyfunction]\` 都是**模块私有**（非 pub），
//   跨模块不可命名 ⇒ 只能写在 lib.rs 的该模块内部（S0 范围外，未改）。
//   该面的机械保护由 mdio_py/tests/test_pyo3_api_surface.py 的 dir() 快照承担
//   （附录 E.5 #4：pyo3 导出面以 Python 侧 dir(module) 快照为准）。

/// PendingWire：引用映射（§6.6 疑似域「引用映射」）的构造出口。
/// 0.8.4 生产零调用；0.8.3 / speedtest 分支的 block.rs、projection.rs 有真实调用
/// ⇒ 不是死代码，是待接线（跨分支调用者类别）。
#[allow(clippy::type_complexity)]
const _: fn(&str, &[crate::block::PackedLineInfo], &[usize]) -> crate::inline::RefMap =
    crate::inline::RefMap::from_indexed_text;

/// PendingWire：块跨度格的 Peek 臂样例位（§9.2「七个格入口」之一）。
/// S0 裁定保留（doc 自陈样例/诊断位），不进 S2 的删除面。
#[allow(clippy::type_complexity)]
const _: fn(&crate::projection::Snapshot) -> Option<std::sync::Arc<Vec<(u32, u32)>>> =
    crate::projection::Snapshot::peek_block_spans_cell;
