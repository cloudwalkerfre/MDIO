// incr v2 — 递归 FC1 树 diff 的调试助手 (crate 内部, 全 pub(crate) 访问)
//
// 仅保留 dump_tree_nodes 供调试打印用 (非测试)。

use super::*;

/// 打印 build_tree 的树结构 (调试助手)。
#[allow(dead_code)]
pub(crate) fn dump_tree_nodes(nodes: &[DNode], indent: usize, out: &mut String) {
    for n in nodes {
        out.push_str(&format!("{}[{},{}] kind={:?} frame={:?} bt={:?} item={}\n",
            "  ".repeat(indent), n.start, n.end, n.kind,
            n.frame.map(|f| (f.kind(), f.indent(), f.body())), n.block_type, n.is_item));
        dump_tree_nodes(&n.children, indent + 1, out);
    }
}
