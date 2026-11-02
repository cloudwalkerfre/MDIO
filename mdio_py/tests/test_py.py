import asyncio
import tempfile
from pathlib import Path
from mdio_py import MdioRuntime

# 1. 准备测试所需的基础 Markdown 数据（增加了 sales 数据用于后续测试）
TEST_MD_CONTENT = """# CTX
## DATA
- nums: [1, 2, 3, 4, 5]
- config: {"rate": 1.5, "offset": 10, "name": "V8_Test"}
- sales: [{"date": "2024-01-01", "product": "A", "amount": 100},
          {"date": "2024-01-01", "product": "B", "amount": 150},
          {"date": "2024-01-02", "product": "A", "amount": 200},
          {"date": "2024-01-02", "product": "B", "amount": 250}]

# RESULTS
"""


async def run_py_test():
    print("🚀 [TEST] 正在初始化 MDIO V8 测试环境...")

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        # 写入初始测试文件
        test_file_path = ws / "py_test.md"
        test_file_path.write_text(TEST_MD_CONTENT, encoding="utf-8")

        # 初始化 Runtime (会自动拉起 Rust Core 和加载插件)
        runtime = MdioRuntime(str(ws))

        print("\n" + "="*50)
        print("✅ 环境初始化完成，开始执行 @PY 插件测试链")
        print("="*50)

        
        # 测试 1：【无状态执行 + 管道数据处理】
        # 作用：测试读取配置列表，清洗为 JSON，喂给 Python，并返回结果
        
        print("\n▶️  [Test 1] 无状态执行 (列表推导)")
        cmd1 = ' $py_test.md::CTX.DATA._b:nums | _loads | @PY.run("[n * 2 for n in val]") '
        print(f"执行 MDI 脚本: {cmd1}")
        res1 = await runtime.exec(cmd1, "default.md")
        print(f"返回结果: {res1}")
        await runtime.exec(f'r"""\n{res1}\n""" | +>py_test.md::RESULTS.Test1', "default.md")

        
        # 测试 2：【状态会话：启动与赋值】
        # 作用：声明一个叫 `calc` 的 session，并将 config 设为全局变量
        
        print("\n▶️  [Test 2] 初始化带状态会话 (Session: calc)")

        # 使用分号明确分隔，保留代码缩进
        cmd2 = """
        $py_test.md::CTX.DATA._b:config | _loads | @PY.calc.run('''
        cfg = val;
        rate = cfg['rate'];
        print(f"环境初始化成功! 当前费率: {rate}, 模式: {cfg['name']}")
        ''')
        """

        cmd2_clean = cmd2.strip()
        print(f"执行 MDI 脚本: {cmd2_clean}")
        res2 = await runtime.exec(cmd2_clean, "default.md")
        print(f"返回结果: {res2}")
        await runtime.exec(f'r"""\n{res2}\n""" | +>py_test.md::RESULTS.Test2', "default.md")

        
        # 测试 3：【状态会话：上下文维持与计算】
        # 作用：验证 `calc` session 是否真正记住了 `rate` 这个变量
        
        print("\n▶️  [Test 3] 验证会话记忆 (调用上一步的 rate)")
        cmd3 = ' "100" | _loads | @PY.calc.run("val * rate") '
        print(f"执行 MDI 脚本: {cmd3}")
        res3 = await runtime.exec(cmd3, "default.md")
        print(f"返回结果: 100 * 1.5 = {res3}")
        await runtime.exec(f'r"""\n{res3}\n""" | +>py_test.md::RESULTS.Test3', "default.md")

        
        # 测试 4：【沙箱隔离与错误处理测试】
        # 作用：在另一个全新的 session 中尝试读取 rate，预期会触发 NameError，
        #       并正确返回 MdioError 错误信息。
        
        print("\n▶️  [Test 4] 会话隔离拦截与异常捕获 (Session: hacker)")
        cmd4 = ' @PY.hacker.run("print(rate)") '
        print(f"执行 MDI 脚本: {cmd4}")
        res4 = await runtime.exec(cmd4, "default.md")
        print(f"返回结果 (预期为 MDIO 错误日志):\n{res4}")
        await runtime.exec(f'r"""\n{res4}\n""" | +>py_test.md::RESULTS.Test4', "default.md")

        
        # 测试 5：【销毁会话资源】
        
        print("\n▶️  [Test 5] 销毁会话资源")
        cmd5 = ' @PY.calc.close '
        print(f"执行 MDI 脚本: {cmd5}")
        res5 = await runtime.exec(cmd5, "default.md")
        print(f"返回结果: {res5}")

        # ==================== 深度测试 ====================

        
        # 测试 6：【动态安装第三方包并验证】
        # 作用：在 `data_sci` session 中安装 pandas，并打印版本号
        
        print("\n▶️  [Test 6] 动态安装 pandas (Session: data_sci)")
        cmd6 = '''
        """await micropip.install('pandas'); import pandas; print(f"Pandas version: {pandas.__version__}")""" | @PY.data_sci.run
        '''
        cmd6_clean = cmd6.strip()
        print(f"执行 MDI 脚本: {cmd6_clean}")
        res6 = await runtime.exec(cmd6_clean, "default.md")
        print(f"返回结果: {res6}")
        await runtime.exec(f'r"""\n{res6}\n""" | +>py_test.md::RESULTS.Test6', "default.md")

        
        # 测试 7：【从管道传递销售数据，创建 DataFrame 并执行简单统计】
        # 作用：读取 sales 数据，转为 DataFrame，计算总销售额
        
        print("\n▶️  [Test 7] 使用 pandas 处理销售数据 (Session: data_sci)")
        cmd7 = """
        $py_test.md::CTX.DATA._b:sales | _loads | @PY.data_sci.run('''
        import pandas as pd
        df = pd.DataFrame(val)
        total = df['amount'].sum()
        print(f"总销售额: {total}")
        df
        ''')
        """
        cmd7_clean = cmd7.strip()
        print(f"执行 MDI 脚本: {cmd7_clean}")
        res7 = await runtime.exec(cmd7_clean, "default.md")
        print(f"返回结果: {res7}")
        await runtime.exec(f'r"""\n{res7}\n""" | +>py_test.md::RESULTS.Test7', "default.md")

        
        # 测试 8：【复杂分组聚合，并将结果写回 Markdown】
        # 作用：按 product 分组，计算总销售额，并将结果以 Strip 格式追加到 RESULTS
        
        print("\n▶️  [Test 8] 分组聚合并写回 Markdown")
        cmd8 = """
        $py_test.md::CTX.DATA._b:sales | _loads | @PY.data_sci.run('''
        import pandas as pd
        df = pd.DataFrame(val)
        grouped = df.groupby('product')['amount'].sum().reset_index()
        # 转换为 Markdown 风格的列表项
        result_lines = [f"- {row['product']}: {row['amount']}" for _, row in grouped.iterrows()]
        "\\n".join(result_lines)
        ''') | +>py_test.md::RESULTS.Test8
        """
        cmd8_clean = cmd8.strip()
        print(f"执行 MDI 脚本: {cmd8_clean}")
        res8 = await runtime.exec(cmd8_clean, "default.md")
        print(f"返回结果 (实际已写入文件，返回值通常为空): {res8}")

        
        # 测试 9：【验证会话隔离：变量隔离】
        # 作用：在 data_sci 中定义一个变量，isolated 中尝试读取，预期 NameError
        
        print("\n▶️  [Test 9] 会话隔离验证 - 变量隔离 (Session: isolated)")
        # 先在 data_sci 中定义一个变量
        await runtime.exec('''
            @PY.data_sci.run("data_sci_secret = 2024")
        ''', "default.md")
        # 然后在 isolated 中读取
        cmd9 = ' @PY.isolated.run("print(data_sci_secret)") '
        print(f"执行 MDI 脚本: {cmd9}")
        res9 = await runtime.exec(cmd9, "default.md")
        print(f"返回结果 (预期为 NameError):\n{res9}")
        await runtime.exec(f'r"""\n{res9}\n""" | +>py_test.md::RESULTS.Test9', "default.md")

        # ==================== 边缘与特殊场景测试 ====================

        
        # 测试 10：【空值处理】
        # 作用：验证管道输入 None、空字符串、空列表时，@PY 的行为
        
        print("\n▶️  [Test 10] 空值处理")
        # 10.1 None 输入
        cmd10_1 = ' @PY.run("val") '
        print(f"执行 MDI 脚本: {cmd10_1}")
        res10_1 = await runtime.exec(cmd10_1, "default.md")
        print(f"返回结果 (None 输入): {res10_1}")
        await runtime.exec(f'r"""\n{res10_1}\n""" | +>py_test.md::RESULTS.Test10_1', "default.md")

        # 10.2 空字符串输入
        cmd10_2 = ' "" | @PY.run("val") '
        print(f"执行 MDI 脚本: {cmd10_2}")
        res10_2 = await runtime.exec(cmd10_2, "default.md")
        print(f"返回结果 (空字符串): {res10_2}")
        await runtime.exec(f'r"""\n{res10_2}\n""" | +>py_test.md::RESULTS.Test10_2', "default.md")

        # 10.3 空列表输入
        cmd10_3 = ' "[]" | _loads | @PY.run("len(val)") '
        print(f"执行 MDI 脚本: {cmd10_3}")
        res10_3 = await runtime.exec(cmd10_3, "default.md")
        print(f"返回结果 (空列表长度): {res10_3}")
        await runtime.exec(f'r"""\n{res10_3}\n""" | +>py_test.md::RESULTS.Test10_3', "default.md")

        
        # 测试 11：【大型数据处理】
        # 作用：传入较大的列表，测试性能与稳定性
        
        print("\n▶️  [Test 11] 大型数据处理")
        cmd11 = '''
            "list(range(10000))" | _loads | @PY.run("sum(val)")
        '''
        print(f"执行 MDI 脚本: {cmd11}")
        res11 = await runtime.exec(cmd11, "default.md")
        print(f"返回结果 (0..9999 的和): {res11}")
        await runtime.exec(f'r"""\n{res11}\n""" | +>py_test.md::RESULTS.Test11', "default.md")

        
        # 测试 12：【超时与长任务】（需要插件支持超时，当前超时 60s，所以放一个 5s 的任务）
        # 作用：模拟耗时操作，验证超时机制（这里故意用 time.sleep 阻塞）
        
        print("\n▶️  [Test 12] 超时与长任务")
        cmd12 = '''
            @PY.timeout_test.run("import time; time.sleep(5); print('Done')")
        '''
        print(f"执行 MDI 脚本: {cmd12}")
        res12 = await runtime.exec(cmd12, "default.md")
        print(f"返回结果 (5秒后应正常返回): {res12}")
        await runtime.exec(f'r"""\n{res12}\n""" | +>py_test.md::RESULTS.Test12', "default.md")

        
        # 测试 13：【错误恢复】
        # 作用：在同一个 session 中先执行错误代码，再执行正确代码，验证会话状态是否被破坏
        
        print("\n▶️  [Test 13] 错误恢复 (Session: recovery)")
        # 13.1 先执行错误代码
        cmd13_1 = ' @PY.recovery.run("1/0") '
        print(f"执行 MDI 脚本: {cmd13_1}")
        res13_1 = await runtime.exec(cmd13_1, "default.md")
        print(f"返回结果 (预期错误): {res13_1}")
        # 13.2 再执行正确代码
        cmd13_2 = ' @PY.recovery.run("x = 42; x") '
        print(f"执行 MDI 脚本: {cmd13_2}")
        res13_2 = await runtime.exec(cmd13_2, "default.md")
        print(f"返回结果 (正确代码应继续执行): {res13_2}")
        await runtime.exec(f'r"""\n{res13_1}\n---\n{res13_2}\n""" | +>py_test.md::RESULTS.Test13', "default.md")

        
        # 测试 14：【并发调用】（简单模拟）
        # 作用：同时发起多个 @PY 调用，验证锁机制是否正常工作
        
        print("\n▶️  [Test 14] 并发调用 (同时启动两个任务)")
        async def concurrent_task(session_name, code):
            cmd = f' @PY.{session_name}.run("{code}") '
            return await runtime.exec(cmd, "default.md")

        task1 = concurrent_task("conc1", "import time; time.sleep(0.5); 'done1'")
        task2 = concurrent_task("conc2", "import time; time.sleep(0.5); 'done2'")
        results = await asyncio.gather(task1, task2, return_exceptions=True)
        print(f"并发结果: {results}")
        await runtime.exec(f'r"""\n{results}\n""" | +>py_test.md::RESULTS.Test14', "default.md")

        
        # 测试 15：【管道串联】
        # 作用：多个 @PY 调用通过管道连接，数据依次传递
        
        print("\n▶️  [Test 15] 管道串联")
        cmd15 = '''
            "5" | _loads | @PY.run("val * 2") | @PY.run("val + 10") | @PY.run(f"结果: {val}")
        '''
        print(f"执行 MDI 脚本: {cmd15}")
        res15 = await runtime.exec(cmd15, "default.md")
        print(f"返回结果: {res15}")
        await runtime.exec(f'r"""\n{res15}\n""" | +>py_test.md::RESULTS.Test15', "default.md")

        
        # 测试 16：【混合操作符】
        # 作用：@PY 与读写操作结合
        
        print("\n▶️  [Test 16] 混合操作符")
        # 先写一个初始值，然后用 Python 读取并修改，再写回
        await runtime.exec('"initial" | +>py_test.md::TEST16.value', "default.md")
        cmd16 = '''
            $py_test.md::TEST16.value | @PY.run("val.upper() + '!!!'") | +>py_test.md::TEST16.result
        '''
        print(f"执行 MDI 脚本: {cmd16}")
        res16 = await runtime.exec(cmd16, "default.md")
        print(f"返回结果: {res16}")
        final_val = await runtime.exec('$py_test.md::TEST16.result', "default.md")
        print(f"最终写入的值: {final_val}")

        
        # 测试 17：【特殊字符处理】
        # 作用：代码中包含引号、换行符、Unicode 字符
        
        print("\n▶️  [Test 17] 特殊字符处理")
        cmd17 = '''
            """print("He said \\"Hello\\"\\n你好🌍")""" | @PY.run
        '''
        print(f"执行 MDI 脚本: {cmd17}")
        res17 = await runtime.exec(cmd17, "default.md")
        print(f"返回结果: {res17}")
        await runtime.exec(f'r"""\n{res17}\n""" | +>py_test.md::RESULTS.Test17', "default.md")

        
        # 测试 18：【嵌套 MDIO 调用】（预期错误或特定行为）
        # 作用：在 Python 代码中尝试执行 MDIO 表达式，应如何反应？目前 @PY 不解析 MDIO，所以应原样返回。
        
        print("\n▶️  [Test 18] 嵌套 MDIO 调用")
        cmd18 = '''
            @PY.run('''"$CTX.DATA._b:nums"''')
        '''
        print(f"执行 MDI 脚本: {cmd18}")
        res18 = await runtime.exec(cmd18, "default.md")
        print(f"返回结果 (应为字符串 '$CTX.DATA._b:nums'): {res18}")
        await runtime.exec(f'r"""\n{res18}\n""" | +>py_test.md::RESULTS.Test18', "default.md")

        # ==================== 更多边缘测试 ====================

        
        # 测试 19：【超大文本输入（> 1MB）】
        # 作用：验证插件能处理大数据量，不会因缓冲区限制而崩溃
        
        print("\n▶️  [Test 19] 超大文本输入")
        large_text = "x" * (1024 * 1024 + 10)  # 1MB + 10 字节
        cmd19 = f' """{large_text}""" | @PY.run("len(val)") '
        print(f"执行 MDI 脚本: 计算输入文本长度")
        res19 = await runtime.exec(cmd19, "default.md")
        print(f"返回结果 (预期 1048586): {res19}")
        await runtime.exec(f'r"""\n{res19}\n""" | +>py_test.md::RESULTS.Test19', "default.md")

        
        # 测试 20：【并发访问同一 Session】
        # 作用：同时启动多个任务操作同一个 session，验证锁机制是否有效防止数据竞争
        
        print("\n▶️  [Test 20] 并发访问同一 Session (Session: shared)")
        # 先初始化一个计数器
        await runtime.exec('@PY.shared.run("counter = 0")', "default.md")

        async def increment():
            for _ in range(10):
                await runtime.exec('@PY.shared.run("counter += 1; counter")', "default.md")

        # 并发执行 5 个任务，每个递增 10 次
        tasks = [increment() for _ in range(5)]
        await asyncio.gather(*tasks)

        # 读取最终值
        res20 = await runtime.exec('@PY.shared.run("counter")', "default.md")
        print(f"最终计数器值 (预期 50): {res20}")
        await runtime.exec(f'r"""\n{res20}\n""" | +>py_test.md::RESULTS.Test20', "default.md")

        
        # 测试 21：【长时间运行任务 + 中途取消】
        # 作用：验证长时间任务可以被取消或超时，不会阻塞其他调用
        
        print("\n▶️  [Test 21] 长时间运行任务 + 取消")
        # 启动一个长时间运行的任务（但不等待它完成）
        long_task = asyncio.create_task(
            runtime.exec('@PY.long.run("import time; time.sleep(10); print(\'Done\')")', "default.md")
        )
        # 等待 2 秒后取消任务
        await asyncio.sleep(2)
        long_task.cancel()
        try:
            await long_task
        except asyncio.CancelledError:
            print("长时间任务已被取消（预期行为）")

        # 检查 session 是否仍可用
        res21 = await runtime.exec('@PY.long.run("42")', "default.md")
        print(f"取消后 session 仍可用，返回值: {res21}")
        await runtime.exec(f'r"""\n{res21}\n""" | +>py_test.md::RESULTS.Test21', "default.md")

        
        # 测试 22：【reset 操作】
        # 作用：重置 session 状态，验证变量被清空
        
        print("\n▶️  [Test 22] reset 操作 (Session: resettable)")
        await runtime.exec('@PY.resettable.run("x = 100")', "default.md")
        res_before = await runtime.exec('@PY.resettable.run("x")', "default.md")
        print(f"reset 前 x 的值: {res_before}")

        await runtime.exec('@PY.resettable.reset', "default.md")
        res_after = await runtime.exec('@PY.resettable.run("x")', "default.md")
        print(f"reset 后尝试读取 x (预期 NameError):\n{res_after}")
        await runtime.exec(f'r"""\n{res_after}\n""" | +>py_test.md::RESULTS.Test22', "default.md")

        
        # 测试 23：【jail_ctx 权限测试】
        # 作用：验证插件遵守 jail_ctx 沙盒限制（需在 Interpreter 中正确传递）
        # 注意：此测试需要 runtime.exec 正确传递 jail_ctx 参数，当前可能未实现，暂作演示
        
        print("\n▶️  [Test 23] jail_ctx 沙盒限制 (需 interpreter 支持)")
        # 假设我们以 jail_ctx="restricted" 执行，尝试读取系统文件（应被拦截）
        cmd23 = '@PY.run("open(\'/etc/passwd\').read()")'
        try:
            res23 = await runtime.exec(cmd23, "default.md", jail_ctx="restricted")
        except Exception as e:
            res23 = f"异常: {e}"
        print(f"越权读取结果 (预期被拦截): {res23}")
        await runtime.exec(f'r"""\n{res23}\n""" | +>py_test.md::RESULTS.Test23', "default.md")

        
        # 测试 24：【管道串联 + 中间 STD 函数】
        # 作用：将 @PY 与 _fmt, _split 等 STD 函数组合，验证数据流转
        
        print("\n▶️  [Test 24] 管道串联 + STD 函数")
        cmd24 = """
            $py_test.md::CTX.DATA._b:sales | _loads | @PY.data_sci.run('''
        import pandas as pd
        df = pd.DataFrame(val)
        df.groupby('product')['amount'].sum().to_dict()
        ''') | _dumps | @PY.run("f'产品销售额: {val}'")
        """
        cmd24_clean = cmd24.strip()
        print(f"执行 MDI 脚本: {cmd24_clean}")
        res24 = await runtime.exec(cmd24_clean, "default.md")
        print(f"返回结果: {res24}")
        await runtime.exec(f'r"""\n{res24}\n""" | +>py_test.md::RESULTS.Test24', "default.md")

        
        # 测试 25：【多重嵌套执行块 $(...)】
        # 作用：验证 $(...) 嵌套执行能在 @PY 参数中正确求值
        
        print("\n▶️  [Test 25] 多重嵌套执行块")
        cmd25 = """
            @PY.run($(_fmt('"{ $py_test.md::CTX.DATA._b:nums }"') | _loads | @PY.run("[x*3 for x in val]") | _dumps))
        """
        # 上述命令流程：
        # 1. 内层 $(_fmt(...)) 先执行，读取 nums，转为列表，乘以3，再转 JSON 字符串。
        # 2. 外层 @PY.run 接收该字符串作为代码执行，由于字符串是 "[3,6,9,12,15]"，执行它返回原列表。
        cmd25_clean = cmd25.strip()
        print(f"执行 MDI 脚本: {cmd25_clean}")
        res25 = await runtime.exec(cmd25_clean, "default.md")
        print(f"返回结果 (预期 [3,6,9,12,15]): {res25}")
        await runtime.exec(f'r"""\n{res25}\n""" | +>py_test.md::RESULTS.Test25', "default.md")

        # ==================== 结束 ====================

        print("\n" + "="*50)
        print("🎉 测试结束！以下是落地在 Rust 引擎中的最终 Markdown 结果：")
        print("="*50)

        # 读出整个文件看一眼
        await runtime.exec('$py_test.md', "default.md")
        raw_text = runtime.store.get_text("py_test.md")
        print(raw_text)


if __name__ == "__main__":
    asyncio.run(run_py_test())
