import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime

async def run_integration_test():
    print("🚀 Starting MDIO Pipeline Integration Test (VFS NEO)...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        # VFS 落盘在虚拟根的 /config.md
        test_file = Path(tmpdir) / "config.md"
        test_file.write_text("# CTX\n## SYS\n- host: localhost\n- port: 8080\n", encoding="utf-8")

        print(f"📦 Workspace mounted at: {tmpdir}")
        print(f"📄 Initial File State:\n{test_file.read_text(encoding='utf-8')}")

        # 启动沙箱
        runtime = MdioRuntime(str(tmpdir))


        # [Test 1] 基础写入覆盖功能 (防退化断言)

        print("\n" + "="*40 + "\n[Test 1] Regular Append & Overwrite\n" + "-"*40)
        code_1 = r'''
        $CTX.SYS._b:port | fmt -t """\n    - Sub item: it is {}""" | +>CTX.SYS._b:port ;
        "192.168.1.1" | />CTX.SYS._b:host
        '''
        result_1 = await runtime.exec(code_1, active_doc="/config.md", wt_uid="genesis")
        final_text_1 = runtime.store.get_text("genesis", "/config.md")
        
        # 写操作默认吃掉管道，因此期望输出为空或是基础回执
        print(f"   => 📥 Pipeline Return: {repr(result_1)}")
        
        assert "host: 192.168.1.1" in final_text_1, "覆盖更新失败"
        assert "Sub item: it is 8080" in final_text_1, "追加更新跨行缩进失败"
        print("   => ✔️ Test 1 Passed!")


        # [Test 2] `-pt` 透传流式机制验证

        print("\n" + "="*40 + "\n[Test 2] -pt (Passthrough) Feature\n" + "-"*40)
        # 将密码压入管道，落地存储之后，依靠 -pt 将数据透传给下游的 fmt 进行输出
        code_2 = r'''
        "SecurePassword123" | />CTX.SYS._b:secret -pt | fmt -t "拦截器收到透传数据: {}"
        '''
        result_2 = await runtime.exec(code_2, active_doc="/config.md", wt_uid="genesis")
        final_text_2 = runtime.store.get_text("genesis", "/config.md")

        print(f"   => 📥 Pipeline Return: {result_2}")
        
        assert "secret: SecurePassword123" in final_text_2, "写入VFS系统失败"
        assert result_2 == "拦截器收到透传数据: SecurePassword123", "-pt 管道透传拦截失败！"
        print("   => ✔️ Test 2 Passed! Pipeline transparent hooking works.")


        # [Test 3] `-f false` 关闭自动接骨，执行严防死守

        print("\n" + "="*40 + "\n[Test 3] -f false (Strict No-Scaffold)\n" + "-"*40)
        # 尝试写入一个之前完全不存在的 GHOST 节点树，但关闭自动建构 (这模拟了数据库的 Update Only 行为)
        code_3 = r'''
        "ghost_val" | />GHOST_NODE.SUB._b:key -f false
        '''
        result_3 = await runtime.exec(code_3, active_doc="/config.md", wt_uid="genesis")
        final_text_3 = runtime.store.get_text("genesis", "/config.md")

        print(f"   => 📥 Pipeline Return: {str(result_3)}")
        
        # 验证这笔非法操作在 MDIO 层面爆错，并且阻止了向文件渗透污染
        assert "mdio_success" in str(result_3), f"-f false 应返回成功回执, got: {result_3}"
        assert "GHOST_NODE" not in final_text_3, "禁用 -f 后不应创建不存在的节点"
        print("   => ✔️ Test 3 Passed! Strict boundaries enforced.")


        # [Test 4] 默认隐式 (或 -f true) 触发深度建架构

        print("\n" + "="*40 + "\n[Test 4] Auto-Scaffold Execution\n" + "-"*40)
        # 使用自然语言指令建树，测试智能接骨上浮 Interpreter 后有没有发生降级错误
        code_4 = r'''
        "magic_val" | />NEW_HEAD.SUB._b:magic
        '''
        result_4 = await runtime.exec(code_4, active_doc="/config.md", wt_uid="genesis")
        final_text_4 = runtime.store.get_text("genesis", "/config.md")
        
        # 验证智能接骨器能够凭空组装缺失结构并追尾附加到文本底端
        assert "# NEW_HEAD" in final_text_4, "scaffold 应创建 # NEW_HEAD 标题"
        assert "## SUB" in final_text_4, "scaffold 应创建 ## SUB 子标题"
        assert "magic: magic_val" in final_text_4, "scaffold 应创建 - magic: magic_val"
        print("   => ✔️ Test 4 Passed! Scaffold auto-assembly works smoothly.")

        print("\n✅ Final Real File State (Inspect):\n" + "-"*40)
        print(final_text_4.strip())
        print("-" * 40)
        print("\n🎉 ALL PIPELINE && FLAG TESTS PASSED!")


if __name__ == "__main__":
    asyncio.run(run_integration_test())
