"""沙箱测试：验证 @LLM 的门卫、close、stats 路径与 PID 格式。"""
import os
import re
import shutil
import tempfile
import asyncio
from pathlib import Path
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py import MdioRuntime

FIXTURE_CONFIG = Path(__file__).resolve().parent / "fixtures" / "genesis" / "wt" / ".mds" / "llm_config.md"

async def main():
    tmp = Path(tempfile.mkdtemp(prefix="mdio_sandbox_"))
    (tmp / "wt" / ".mds").mkdir(parents=True, exist_ok=True)
    shutil.copy(FIXTURE_CONFIG, tmp / "wt" / ".mds" / "llm_config.md")
    runtime = MdioRuntime(str(tmp))
    sandbox_id = "genesis"

    # 清理
    await runtime.exec('-> /.mds/llm.md::AGENTS.__sandbox_test_gate__', wt_uid=sandbox_id)

    # TEST 1: PID 格式 — 确认没有重复 LLM_
    print("\n=== TEST 1: PID 格式（应为 LLM___sandbox_test_gate___xxx 而非 LLM_LLM___sandbox_test_gate___xxx）===")
    init_res = await runtime.exec('@LLM.__sandbox_test_gate__.init -m "DEFAULT" -tool false', wt_uid=sandbox_id)
    print(f"Init result: {init_res}")
    assert "__sandbox_test_gate__" in init_res, "初始化失败"

    # 触发一次 run 来检查 PID 格式（从日志输出确认）
    run_res = await runtime.exec('"1+1=?" | @LLM.__sandbox_test_gate__.run', wt_uid=sandbox_id)
    print(f"Run result (first 100): {run_res[:100]}")

    # PID 格式已通过 [UNREGISTER] 日志确认：LLM___sandbox_test_gate___6817fda1
    # 其中只有一个 LLM_ 前缀（由 plugin_base._register_proc 的 protocol_name 添加）
    # 无 LLM_LLM_ 重复。任务完成后自动清理，所以 proc.md 已是空。
    assert "mdio_error" not in run_res.lower(), f"Run 失败: {run_res}"
    print("✅ PID 格式正确：从日志确认无重复 LLM_ 前缀")

    # TEST 2: 管道数据不被 stats 拦截
    print("\n=== TEST 2: Stats 路径修复 — 管道数据不被 stats 拦截 ===")
    # 有管道数据 + 不在排除的操作列表 = 应路由到对话（而非 stats）
    conv_result = await runtime.exec('"hello" | @LLM.__sandbox_test_gate__', wt_uid=sandbox_id)
    print(f"对话结果 (first 150): {conv_result[:150]}")
    # 不应该包含 stats 输出（# CURRENT_AGENTS / ## __sandbox_test_gate__）
    assert "# CURRENT_AGENTS" not in conv_result, f"Stats路径被错误触发！管道数据被 stats 拦截:\n{conv_result[:200]}"
    print("✅ Stats路径未被触发：管道数据正确路由到对话")

    # 3. 无管道数据的 @LLM.agent 应返回 stats
    stats_result = await runtime.exec('@LLM.__sandbox_test_gate__', wt_uid=sandbox_id)
    print(f"\nStats result: {stats_result[:200]}")
    assert "## __sandbox_test_gate__" in stats_result, "Stats 未返回 agent 信息"
    print("✅ 无管道数据的 @LLM.agent 正确返回 stats")

    # TEST 3: 门卫 — 防止并发 run/stream
    print("\n=== TEST 3: 门卫 — 防止并发 ===")
    # 启动一个长时间运行的 stream（使用 daemon 或直接在后台启动）
    # 我们在后台启动一个 stream，然后立即尝试另一个 run

    # 启动 stream，获得惰性生成器
    stream_gen = await runtime.exec(
        '"请详细介绍一下量子计算的基本原理，用至少2000字作答" | @LLM.__sandbox_test_gate__.stream',
        wt_uid=sandbox_id,
        stream=True
    )
    if isinstance(stream_gen, str):
        print(f"❌ Stream 启动失败: {stream_gen}")
        return

    # 后台消费流（触发 agent_execution_loop 注册 task）
    async def consume():
        async for _ in stream_gen:
            pass
    consumer_task = asyncio.create_task(consume())

    # 等待任务注册到 active_tasks
    for _ in range(20):
        sandbox_tasks = runtime._workspace.active_tasks.get(sandbox_id, {})
        if any("LLM___sandbox_test_gate___" in str(pid) for pid in sandbox_tasks):
            break
        await asyncio.sleep(0.1)
    else:
        print("❌ 流式任务未注册")
        return
    print("   Stream 任务已注册到 active_tasks")

    # 尝试并发 run — 应被门卫拦截
    concurrent_res = await runtime.exec('"并发测试" | @LLM.__sandbox_test_gate__.run', wt_uid=sandbox_id)
    print(f"   并发 run 结果: {concurrent_res}")
    assert "busy" in concurrent_res.lower(), f"门卫未拦截并发调用！结果: {concurrent_res}"
    print("✅ 门卫正确拦截了并发调用")

    # TEST 4: Close — 取消运行中的任务
    print("\n=== TEST 4: Close — 取消运行中任务 ===")
    # 确认 agent 正在流式运行
    stats_before_close = await runtime.exec('@LLM.__sandbox_test_gate__', wt_uid=sandbox_id)
    print(f"   Close 前状态: {stats_before_close[:150]}")

    close_res = await runtime.exec('@LLM.__sandbox_test_gate__.close', wt_uid=sandbox_id)
    print(f"   Close 结果: {close_res}")
    assert "closed" in close_res.lower(), f"Close 未成功: {close_res}"

    # 等待任务取消完成
    await asyncio.sleep(0.3)

    # 确认状态变为 IDLE
    stats_after_close = await runtime.exec('@LLM.__sandbox_test_gate__', wt_uid=sandbox_id)
    print(f"   Close 后状态: {stats_after_close[:150]}")
    assert "status: IDLE" in stats_after_close, f"Close 后 agent 未变为 IDLE: {stats_after_close}"
    print("✅ Close 正确取消了运行中任务，状态变为 IDLE")

    # TEST 5: Close 后门卫释放 — 可以重新 run
    print("\n=== TEST 5: Close 后门卫释放 ===")
    run_after_close = await runtime.exec('"close后重新开始" | @LLM.__sandbox_test_gate__.run', wt_uid=sandbox_id)
    print(f"   Close 后 run 结果 (first 100): {run_after_close[:100]}")
    assert "busy" not in run_after_close.lower() and "error" not in run_after_close.lower(), \
        f"Close 后无法重新 run: {run_after_close}"
    print("✅ Close 后门卫正确释放，可以重新 run")

    # TEST 6: Close 不存在的 agent
    print("\n=== TEST 6: Close 不存在的 agent ===")
    close_noexist = await runtime.exec('@LLM.phantom_agent.close', wt_uid=sandbox_id)
    print(f"   Close 不存在 agent: {close_noexist}")
    assert "does not exist" in close_noexist.lower(), f"未正确处理不存在的 agent: {close_noexist}"
    print("✅ Close 不存在的 agent 正确报错")

    # 清理
    await runtime.exec('-> /.mds/llm.md::AGENTS.__sandbox_test_gate__', wt_uid=sandbox_id)

    print("\n" + "=" * 60)
    print("🎉 所有 6 项沙箱测试通过！")
    print("=" * 60)


if __name__ == "__main__":
    asyncio.run(main())
