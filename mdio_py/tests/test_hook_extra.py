import asyncio
import tempfile
from pathlib import Path

import pytest
from mdio_py import MdioRuntime

async def test_chain_trigger():
    """钩子链式触发：A 修改 B，B 触发 C"""
    print("\n🧪 [Extended] Chain Trigger Test")
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        (root / "wt").mkdir(exist_ok=True)
        workspace = root / "wt"          # 文档根恒为 <根>/wt（定则）；夹具写 workspace
        (workspace / "chain.md").write_text(
            "# TARGET\n- level1: start\n- level2: middle\n", encoding="utf-8"
        )
        # 预先创建 LOGS 节点，确保写入目标存在
        (workspace / "log.md").write_text("# LOGS\n", encoding="utf-8")

        runtime = MdioRuntime(str(root))
        await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

        # 注册钩子，写入到已存在的 LOGS 节点
        register = r'''
        hook -target /chain.md::TARGET._b:level1 -cb '"触发level1" | fmt -t "钩子1执行: {}" | +> /log.md::LOGS ; "触发level1" | /> /chain.md::TARGET._b:level2' -is_always ;
        hook -target /chain.md::TARGET._b:level2 -cb '"触发level2" | fmt -t "钩子2执行: {}" | +> /log.md::LOGS' -is_always ;
        '''
        await runtime.exec(register, active_doc="/chain.md", wt_uid="genesis")

        # 触发 level1 变化
        await runtime.exec('"changed" | /> /chain.md::TARGET._b:level1', active_doc="/chain.md", wt_uid="genesis")
        await asyncio.sleep(0.2)  # 等待异步处理

        log = runtime.store.get_text("genesis", "/log.md")
        chain = runtime.store.get_text("genesis", "/chain.md")

        # 验证链式结果
        assert "钩子1执行: 触发level1" in log, "钩子1未执行或未写入日志"
        assert "钩子2执行: 触发level2" in log, "钩子2未执行或未写入日志"
        assert "level2: 触发level1" in chain, "钩子1未正确更新 level2"
        assert log.count("钩子2执行: 触发level2") == 1, "钩子2触发次数异常"

        print("✅ Chain trigger test passed")

async def test_callback_error_handling():
    """错误钩子不应阻塞其他钩子"""
    print("\n🧪 [Extended] Callback Error Handling Test")
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        (root / "wt").mkdir(exist_ok=True)
        workspace = root / "wt"          # 文档根恒为 <根>/wt（定则）；夹具写 workspace
        (workspace / "err.md").write_text("# ERR\n- key: val\n", encoding="utf-8")
        (workspace / "log_ok.md").write_text("# OK\n", encoding="utf-8")

        runtime = MdioRuntime(str(root))
        await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

        # Register error hook and normal hook
        register = r'''
        hook -target /err.md::ERR._b:key -cb '"error" | _nonexistent_func' -is_always ;
        hook -target /err.md::ERR._b:key -cb '"normal" | +> /log_ok.md::OK' -is_always ;
        '''
        await runtime.exec(register, active_doc="/err.md", wt_uid="genesis")

        # Trigger
        await runtime.exec('"new" | /> /err.md::ERR._b:key', active_doc="/err.md", wt_uid="genesis")
        await asyncio.sleep(0.2)

        log_ok = runtime.store.get_text("genesis", "/log_ok.md")
        # Normal hook should have executed
        assert "normal" in log_ok, "Normal hook was not triggered due to error hook failure"
        # Also check that error hook didn't crash the system (verify system still responsive)
        await runtime.exec('"test" | +> /log_ok.md::', active_doc="/err.md", wt_uid="genesis")
        final_log = runtime.store.get_text("genesis", "/log_ok.md")
        assert "test" in final_log, "System is unresponsive after error"
        print("✅ Error handling test passed")

async def test_hook_lifecycle_management():
    """使用 -deldoc 清理钩子"""
    print("\n🧪 [Extended] Hook Lifecycle (Del) Test")
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        (root / "wt").mkdir(exist_ok=True)
        workspace = root / "wt"          # 文档根恒为 <根>/wt（定则）；夹具写 workspace
        (workspace / "life.md").write_text("# LIFE\n- data: init\n", encoding="utf-8")
        (workspace / "log.md").write_text("# LOG\n", encoding="utf-8")

        runtime = MdioRuntime(str(root))
        await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

        # Register a persistent hook
        register = r'''
        hook -target /life.md::LIFE._b:data -cb '"triggered" | +> /log.md::LOG' -is_always ;
        '''
        await runtime.exec(register, active_doc="/life.md", wt_uid="genesis")

        # First trigger
        await runtime.exec('"changed1" | /> /life.md::LIFE._b:data', active_doc="/life.md", wt_uid="genesis")
        await asyncio.sleep(0.2)
        log1 = runtime.store.get_text("genesis", "/log.md")
        assert log1.count("triggered") == 1

        # Delete all hooks on the doc
        await runtime.exec('hook -deldoc /life.md', active_doc="/life.md", wt_uid="genesis")

        # Second trigger
        await runtime.exec('"changed2" | /> /life.md::LIFE._b:data', active_doc="/life.md", wt_uid="genesis")
        await asyncio.sleep(0.2)
        log2 = runtime.store.get_text("genesis", "/log.md")
        assert log2.count("triggered") == 1, "Hook should not trigger again after deletion"
        print("✅ Lifecycle management test passed")

@pytest.mark.skip(reason="fuse timing-dependent — needs investigation; passes under old runner")
async def test_fuse_default_behavior():
    """验证默认熔断器在循环中工作（轻量验证）"""
    print("\n🧪 [Extended] Fuse Default Behavior (Smoke Test)")
    with tempfile.TemporaryDirectory() as tmpdir:
        root = Path(tmpdir)
        (root / "wt").mkdir(exist_ok=True)
        workspace = root / "wt"          # 文档根恒为 <根>/wt（定则）；夹具写 workspace
        (workspace / "fuse.md").write_text("# FUSE\n- A: init\n- B: init\n", encoding="utf-8")

        runtime = MdioRuntime(str(root))
        await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

        register = r'''
        hook -target /fuse.md::FUSE._b:A -cb '"bounce_a" | +> /fuse.md::FUSE._b:B' -is_always ;
        hook -target /fuse.md::FUSE._b:B -cb '"bounce_b" | +> /fuse.md::FUSE._b:A' -is_always ;
        '''
        await runtime.exec(register, active_doc="/fuse.md", wt_uid="genesis")

        # Trigger
        await runtime.exec('"ignite!" | +> /fuse.md::FUSE._b:A', active_doc="/fuse.md", wt_uid="genesis")
        await asyncio.sleep(0.5)

        sys_err = runtime.store.get_text("genesis", "/system_error.md")
        # The fuse should have triggered and written an error message
        assert "熔断" in sys_err or "fuse" in sys_err.lower() or "风暴" in sys_err, "Fuse did not trigger"
        print("✅ Fuse default behavior test passed")

async def run_all_extended_tests():
    await test_chain_trigger()
    await test_callback_error_handling()
    await test_hook_lifecycle_management()
    await test_fuse_default_behavior()

if __name__ == "__main__":
    asyncio.run(run_all_extended_tests())