"""pyo3 导出面 keep-alive：Python 侧是真实消费者。

pyo3 的注册写在 Rust 侧 #[pymodule] 函数体里，Rust 侧工具与 dead_code 都看不见
「零调用导出」；导出面的权威快照只能取 Python 侧 dir(module)。本文件就是那份
快照判官——删任一被导出名 ⇒ 本测试红。

清单来源：本文件维护的跨进程契约面。
telemetry_* 组随构建 feature 出现/消失，故只在「该组任一存在」时要求整组。
"""
from mdio_py import mdio_core

# 模块级：8 个 #[pyfunction] + 2 个 pyclass
MODULE_NAMES = [
    "encode_file_op_ffi",
    "snap_apply_ffi",
    "snap_diff_ffi",
    "snap_hash_ffi",
    "snap_parse_all",
    "snap_parse_ckpt",
    "snap_parse_ckpt_full",
    "snap_state_hash",
    "ProjectStore",
    "PyServerAction",
]

# ProjectStore 方法：非 telemetry 组（出货构建恒有）
STORE_METHODS = [
    "apply_edit_selector",
    "bindings_info",
    "clear_doc_hooks",
    "clear_worktree_hooks",
    "debug_block_tree",
    "decode_checkpoint_text",
    "derive_worktree",
    "doc_wt_uid",
    "drop_worktree",
    "expand_glob",
    "flush_doc_to_vfs",
    "flush_worktree_sessions",
    "force_flush_file",
    "get_ast",
    "get_ast_packet",
    "get_ast_packet_async",
    "get_file_metadata",
    "get_text",
    "get_vfs_tree",
    "handle_packet",
    "has_worktree",
    "head_exists",
    "heading_at_line",
    "hook_count_for_worktree",
    "is_dir",
    "list_worktrees",
    "load_checkpoint",
    "mk",
    "offload_worktree",
    "parent_uid",
    "parse_to_ast",
    "parse_to_ast_nested",
    "path_exists",
    "query_memory",
    "read_locator",
    "register_ast_callback",
    "register_hook",
    "register_hook_by_uid",
    "register_hook_dispatcher",
    "register_worktree",
    "reload",
    "remove_hook",
    "reset_doc",
    "resolve_path",
    "restore_project_worktrees",
    "rm",
    "rule_add_remote",
    "rule_remote_entries",
    "rule_remove_remote",
    "run_find",
    "run_tree",
    "set_mode",
    "to_ast_json",
    "tree_ensure",
    "write_locator",
]

# PyServerAction getter 面（[小修] 原先不在快照里：9 个 getter 无机械保护）
SERVER_ACTION_GETTERS = [
    "is_broadcast",
    "is_direct",
    "is_subscribe",
    "is_unsubscribe",
    "channel",
    "data",
    "is_file_action",
    "file_action_entry_id",
    "file_action_op_json",
]

# ProjectStore 方法：telemetry 组（分析构建才有；见 mdio_core Cargo.toml [features]）
STORE_METHODS_TELEMETRY = [
    "telemetry_export_phase_json",
    "telemetry_get_layer_stats",
    "telemetry_merge_js_data",
    "telemetry_report",
    "telemetry_report_trace_json",
    "telemetry_session_active",
    "telemetry_session_create",
    "telemetry_session_destroy",
    "telemetry_session_mode",
    "telemetry_span_enter",
    "telemetry_span_exit",
]


def test_module_export_surface():
    present = set(dir(mdio_core))
    missing = [n for n in MODULE_NAMES if n not in present]
    assert not missing, f"pyo3 模块导出缺失（契约面，禁删）：{missing}"


def test_project_store_method_surface():
    present = set(dir(mdio_core.ProjectStore))
    missing = [n for n in STORE_METHODS if n not in present]
    assert not missing, f"ProjectStore 方法缺失（契约面，禁删）：{missing}"


def test_server_action_getter_surface():
    present = set(dir(mdio_core.PyServerAction))
    missing = [n for n in SERVER_ACTION_GETTERS if n not in present]
    assert not missing, f"PyServerAction getter 缺失（契约面，禁删）：{missing}"


def test_project_store_telemetry_group_is_all_or_nothing():
    present = set(dir(mdio_core.ProjectStore))
    hits = [n for n in STORE_METHODS_TELEMETRY if n in present]
    if not hits:
        return  # 出货构建（telemetry 关）：整组不存在，合法
    missing = [n for n in STORE_METHODS_TELEMETRY if n not in present]
    assert not missing, f"telemetry 组只出现部分成员（导出面不一致）：{missing}"
