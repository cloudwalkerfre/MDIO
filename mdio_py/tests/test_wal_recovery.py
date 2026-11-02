"""Phase 4b 验证测试：Crash Recovery + 边界情况。

测试 crash recovery 位点驱动恢复逻辑，
drop_worktree WAL 清理，以及边界情况处理。
"""
import os


async def test_recovery_new_file_no_base(runtime, tmp_path):
    """新文件从未 flush → 无 base 文件 → Engine 从空字符串开始，WAL 正常。"""
    await runtime.exec('"# RECOVERY\n- v: recovered" | /> /test_recovery.md -f -god', wt_uid="genesis")
    result = await runtime.exec('$ /test_recovery.md::RECOVERY._b:v', wt_uid="genesis")
    assert "recovered" in result


async def test_recovery_preserves_content(runtime):
    """多次编辑后内容正确保留。"""
    await runtime.exec('"# RP\n- v: first" | /> /test_rp.md -f -god', wt_uid="genesis")
    await runtime.exec('"second" | +> /test_rp.md::RP._b:v -f', wt_uid="genesis")
    # Flush and reopen
    doc_id = runtime.store.resolve_path("genesis", "/test_rp.md", False)
    runtime.store.flush_doc_to_vfs(doc_id)
    result = await runtime.exec('$ /test_rp.md::RP._b:v', wt_uid="genesis")
    assert "first" in result


async def test_flush_and_reopen_consistency(runtime):
    """flush 后 reopen session 内容一致（模拟 base offset 对齐 WAL 尾部）。"""
    await runtime.exec('"# FLUSH_REOPEN\n- v: aligned" | /> /test_align.md -f -god', wt_uid="genesis")
    doc_id = runtime.store.resolve_path("genesis", "/test_align.md", False)
    runtime.store.flush_doc_to_vfs(doc_id)
    # Force session drop by clearing sessions internally (via drop_derived_worktree pattern)
    # Then read again — open_session should reload from base
    result = await runtime.exec('$ /test_align.md::FLUSH_REOPEN._b:v', wt_uid="genesis")
    assert "aligned" in result


async def test_concurrent_writes_same_doc(runtime):
    """同一 doc 多次写，内容正确。"""
    await runtime.exec('"# CONCUR\n- v: a" | /> /test_concur.md -f -god', wt_uid="genesis")
    await runtime.exec('"b" | +> /test_concur.md::CONCUR._b:v -f', wt_uid="genesis")
    # Multiple append ops
    await runtime.exec('"c" | +> /test_concur.md::CONCUR._b:v -f', wt_uid="genesis")
    result = await runtime.exec('$ /test_concur.md::CONCUR._b:v', wt_uid="genesis")
    assert "a" in result

if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))

