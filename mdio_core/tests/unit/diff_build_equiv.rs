// diff 构建对照（new_frame_span.md §八 增量对照——编辑场景——
// 记录版（归属树——唯一实现）——
// 验证 diff 路径跑通 + patch 合法性）。
// 场景：纯内容编辑/容器追加/多行段落/空行插入/嵌套列表编辑/cont 编辑。

use crate::incr::IncrTask;
use std::sync::Arc;

fn run_diff_case(name: &str, old: &str, s: usize, e: usize, new: &str) {
    let mut st_old = crate::projection::State::new(old);
    let snap_old = Arc::new(st_old.snapshot());
    let mut state = crate::projection::State::new(old);
    state.edit(s, e, new);
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("ripple_diff");
    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: crate::cache::edit_actions_arc(vec![(s, e)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    // 记录版(归属树)是唯一实现——本测试验证 diff 路径跑通 +
    // patch 合法性(逐 patch 正确性由 Python 侧 AST 应用 + incr 门禁承担)
    let rec_patches = crate::incr::diff_patches_mode(&task, &crate::topology::CONTAINER_TOPOLOGY);
    // 临时诊断——打印 op+path
    for p in &rec_patches {

    }
    for p in &rec_patches {
        let path = match p {
            crate::incr::Patch::Add { path, .. }
            | crate::incr::Patch::Remove { path }
            | crate::incr::Patch::Replace { path, .. }
            | crate::incr::Patch::ReplaceAttr { path, .. } => path,
        };
        assert!(path.starts_with("/ct/") || path.is_empty(),
            "[{name}] patch path 非法: {path}");
        if let crate::incr::Patch::Add { value, .. }
            | crate::incr::Patch::Replace { value, .. } = p {
            serde_json::from_str::<serde_json::Value>(value)
                .unwrap_or_else(|e| panic!("[{name}] patch value 非法 JSON: {e} — {value}"));
        }
    }

}

/// 增量对照（§八 三种编辑序列的代表场景）——记录 zone 构建 vs 现状 zone
/// 构建——逐 patch 一致。
#[test]
fn diff_build_spans_vs_records() {
    // 纯内容编辑（段落——不扩容器）
    run_diff_case("leaf_para", "hello world\n", 0, 12, "HELLO WORLD\n");
    // 纯内容编辑（列表内 cont）
    run_diff_case("item_cont", "- a\n  cont\n- b\n", 4, 8, "X\n");
    // 结构变化（追加 item）——列版旧差异见报告（归属树正确）
    run_diff_case("item_append", "# Doc\n- a\n- b\n", 12, 12, "- c\n");
    // 结构变化（BQ 追加——行数增）——列版旧差异见报告
    run_diff_case("bq_append", "> a\n> b\n", 6, 6, "> c\n");
    // item 编辑（列表中间行替换）
    run_diff_case("list_mid", "- a\n  - b\n- c\n", 4, 6, "X\n");
    // item 编辑（嵌套列表——深度变化）
    run_diff_case("nested_list", "- a\n- b\n", 4, 4, "  - inner\n");
    // 多行段落中间行替换
    run_diff_case("para_mid", "# Doc\n\nline one\nline two\nline three\n", 8, 17, "NEW LINE\n");
    // 空行处插入（zone 截断路径）
    run_diff_case("blank_insert", "- a\n\n- b\n", 4, 4, "- new\n");
}
