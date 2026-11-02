import pytest
from mdio_py import MdioRuntime


@pytest.mark.asyncio
async def test_malicious_wt_uid(tmp_path):
    """测试 2.1 + 2.2: 伪造句柄和越权访问应被系统拦截"""
    (tmp_path / "wt" / "admin_secret.md").write_text("# PASSWORD\n- root: 123456")
    (tmp_path / "public_zone").mkdir()

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

    # 2.1 伪造句柄 (wt_uid=9999 → invalid uid)
    res = await runtime.exec('$/admin_secret.md::PASSWORD', active_doc="/", wt_uid="9999")
    assert "not found" in res.lower() or "R201" in res, \
        f"伪造句柄应被拦截，实际: {res}"

    # 2.2 越权访问
    actor1_id = runtime._workspace.derive_worktree("genesis", "/public_zone")
    res2 = await runtime.exec('$/admin_secret.md::PASSWORD', active_doc="/", wt_uid=actor1_id)
    assert "not found" in res2.lower() or "R201" in res2, \
        f"越权访问应被拦截，实际: {res2}"


@pytest.mark.asyncio
async def test_exec_block_anti_poison(tmp_path):
    """测试 3: 嵌套 ExecBlock 崩溃不应污染 VFS 文件名"""
    (tmp_path / "wt" / "admin_secret.md").write_text("# PASSWORD\n- root: 123456")

    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

    malicious = '"毒数据" | +> $($("我瞎写的" | 无效指令))'
    res = await runtime.exec(malicious, active_doc="/", wt_uid="genesis")
    assert "mdio_error" in str(res).lower() or "P002" in str(res), \
        f"内层崩溃应触发防护，实际: {res}"

    bad_files = list(tmp_path.rglob("*- mdio_error*"))     # 文档根在 wt/ 下，rglob 覆盖
    assert not bad_files, f"错误字符串不应落盘为文件名: {bad_files}"


@pytest.mark.asyncio
@pytest.mark.skip(reason="pre-existing: system_error.md path in Plan 3 VFS")
async def test_fatal_parse_crash_log(tmp_path):
    """测试 4: 致命语法崩溃应被记录到 /system_error.md"""
    runtime = MdioRuntime(str(tmp_path))
    await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

    bad_code = "这是完全无法被 Lark AST 吞噬的混乱符文 :: { $| }"
    await runtime.exec(bad_code, active_doc="/", wt_uid="genesis")

    sys_log = tmp_path / "wt" / "system_error.md"
    assert sys_log.exists(), "/system_error.md 应存在"

    sys_text = runtime.store.get_text("genesis", "/system_error.md")
    assert "mdio_error" in sys_text and ("P001" in sys_text or "P002" in sys_text or "解析" in sys_text), \
        f"日志应包含崩溃追溯，实际: {sys_text[:200]}"

if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))

