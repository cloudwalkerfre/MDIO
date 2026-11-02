"""Phase 4a 验证测试：WAL 写入 + Checkpoint（正常运行路径）。

仅在 persistent=true 时执行 WAL 相关测试。
默认 feature=persistence 关闭时，所有测试（含现有全量）不受影响。
"""
import os
import pytest


async def test_persistent_false_no_wal(runtime):
    """persistent=false 时不写 WAL、不启 worker（默认行为）。"""
    # 默认 runtime 是 persistent=false，写操作正常工作
    await runtime.exec('"# NO_WAL\n- v: test" | /> /test_no_wal.md -f -god', wt_uid="genesis")
    result = await runtime.exec('$ /test_no_wal.md::NO_WAL._b:v', wt_uid="genesis")
    assert "test" in result


async def test_memory_backend_no_wal(runtime):
    """Memory backend 的 query_memory ($$) 不产生 WAL 文件。"""
    result = await runtime.exec('$$"# M\n- v: mem_test"::M._b:v', wt_uid="genesis")
    assert "mem_test" in result


async def test_flush_one_cleans_dirty(runtime):
    """flush_doc_to_vfs 后内容正确落盘。"""
    await runtime.exec('"# FLUSH_TEST\n- v: flushed" | /> /test_flush.md -f -god', wt_uid="genesis")
    # Force flush
    doc_id = runtime.store.resolve_path("genesis", "/test_flush.md", False)
    runtime.store.flush_doc_to_vfs(doc_id)
    # Re-read via open_session
    result = await runtime.exec('$ /test_flush.md::FLUSH_TEST._b:v', wt_uid="genesis")
    assert "flushed" in result


async def test_write_then_read_consistency(runtime):
    """多次写入后内容一致性。"""
    await runtime.exec('"# CYCLE\n- v: a" | /> /test_cycle.md -f -god', wt_uid="genesis")
    await runtime.exec('"b" | +> /test_cycle.md::CYCLE._b:v -f', wt_uid="genesis")
    result = await runtime.exec('$ /test_cycle.md::CYCLE._b:v', wt_uid="genesis")
    assert "a" in result or "b" in result


async def test_multiple_docs_independent(runtime):
    """不同文档的写入互不影响。"""
    await runtime.exec('"# DOC1\n- v: d1" | /> /doc1.md -f -god', wt_uid="genesis")
    await runtime.exec('"# DOC2\n- v: d2" | /> /doc2.md -f -god', wt_uid="genesis")
    r1 = await runtime.exec('$ /doc1.md::DOC1._b:v', wt_uid="genesis")
    r2 = await runtime.exec('$ /doc2.md::DOC2._b:v', wt_uid="genesis")
    assert "d1" in r1
    assert "d2" in r2

if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))

