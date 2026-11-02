"""验证 Backend 行为：write_locator / read_locator / query_memory 的现有 FFI 接口行为不变。"""
import pytest


async def test_fs_backend_rw(runtime):
    """现有 write_locator + read_locator 流程不受影响。"""
    await runtime.exec('"# HELLO\nworld" | /> /test_rw.md -f -god', wt_uid="genesis")
    result = await runtime.exec('$ /test_rw.md::HELLO', wt_uid="genesis")
    assert "world" in result


async def test_memory_backend_query(runtime):
    """query_memory ($$) 继续正常工作。"""
    result = await runtime.exec('$$"# T\n- k: v"::T._b:k', wt_uid="genesis")
    assert "v" in result


def test_create_dir_all_eliminated():
    """mdio_core/src 生产代码无 std::fs::create_dir_all（telemetry.rs 与 bin 除外）。"""
    import pathlib
    src_dir = pathlib.Path(__file__).resolve().parent.parent.parent / "mdio_core" / "src"
    bad = []
    for p in src_dir.rglob("*.rs"):
        if p.name == "telemetry.rs" or "bin" in p.parts:
            continue
        text = p.read_text()
        # 只检查生产代码；文件末尾的 #[cfg(test)] mod tests 属于测试实现，
        # 其中 telemetry 产物落盘允许 create_dir_all。
        if "mod tests {" in text:
            text = text[:text.rfind("mod tests {")]
        if "std::fs::create_dir_all" in text:
            bad.append(str(p))
    assert not bad, f"Found std::fs::create_dir_all:\n{bad}"

if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))

