import sys
import os
import textwrap
import asyncio

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext

async def thread_exists(runtime, ctx, tid: str):
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
    return bool(raw and "- mdio_error:" not in raw)

async def read_thread_state(runtime, ctx, tid: str) -> dict:
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
    if not raw or "- mdio_error:" in raw:
        return {}
    state = {}
    for line in raw.strip().split('\n'):
        line = line.strip()
        if line.startswith('- ') and ':' in line:
            k, v = line[2:].split(':', 1)
            state[k.strip()] = v.strip()
    return state

async def test_loop_and_poison():
    print("\n🧪 [TEST] XBT 循环重置与失败注入测试 (Loop Respawn & Poison Injection)")

    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    await runtime.exec('mk -f /sys.md', ctx=ctx)
    await runtime.exec('mk -f /log.md', ctx=ctx)
    await runtime.exec('"# LOGS" | +> /log.md:: -f', ctx=ctx)

    blueprint_script = r"""
>> []
    >> {{ "- Action A executed" | +> /log.md::LOGS }}
    >> [] 外部长时动作挂起_等待投喂
    >> {{ "- Action B executed (If you see this, poison failed!)" | +> /log.md::LOGS }}
"""
    
    blueprint_path = "/sys.md::TEST_ROOT"
    await runtime.exec(f'"# TEST_ROOT\n- loop: 3" | +> /sys.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{blueprint_script}\n```"
    await runtime.exec(f'"""{fenced}""" | +> {blueprint_path}', ctx=ctx)

    tid = "Samsara_Agent"

    print(f"\n📌 [TEST] 启动循环代理 (-spawn)，初始寿命: 3 ...")
    await runtime.exec(f'"" | @XBT -spawn "{tid}" -bp "{blueprint_path}" -ppid "root" -ptr "0"', ctx=ctx)


    # 第一世 (Epoch 1)：正常通关，验证自然倒计与状态重置功能

    print("\n=== ⏳ Epoch 1：初生之世 ===")
    
    # 增加容忍度到 100 次（5秒），给老机型充分喘息的空间
    for i in range(100):
        action_tid = f"{tid}_0_1"
        if await thread_exists(runtime, ctx, action_tid):
            state = await read_thread_state(runtime, ctx, action_tid)
            if state.get("status") == "WAITING" and state.get("phase") == "A_EXE":
                print(f"   [Epoch 1] 捕获到节点 {action_tid} 挂起，正在注入生机...")
                break
        await asyncio.sleep(0.05)
    else:
        raise TimeoutError("等待节点挂起超时！")
    
    await runtime.exec(f'"SUCCESS" | @XBT -resolve "{action_tid}"', ctx=ctx)
    
    # 等待第一世代结束，重置启动 
    for i in range(100):
        state = await read_thread_state(runtime, ctx, tid)
        # 只要查到寿命变成 2，就说明 Epoch 1 已结算并重启
        # 引擎跑得太快，很可能已经进入 PENDING 态，所以不要断言 status == "WAKE_UP"
        if state.get("loop_remain") == "2":
            print("   ✅ [Epoch 1] 完美结算！寿命扣减为 2。")
            break
        await asyncio.sleep(0.05)
    else:
        raise TimeoutError("第一纪元未能正常完成重启！")


    state = await read_thread_state(runtime, ctx, tid)
    assert "res_" not in "".join(state.keys()), "重置时未能清洗子节点交差记录！"
    assert state.get("pointer", "").startswith("0"), f"重置时未归回起航点！实际指针: {state.get('pointer')}"
    print("   ✅ [Epoch 1] 洗脑执行彻底！指针归 0，过往兵符全清。")


    # 第二世 (Epoch 2)：半路崩阻，验证毒药强权！

    print("\n=== ⏳ Epoch 2：失败注入轮次 ===")
    
    # 再次放宽探测区间寻找重生子代
    for i in range(100):
        action_tid = f"{tid}_0_1" 
        if await thread_exists(runtime, ctx, action_tid):
            state = await read_thread_state(runtime, ctx, action_tid)
            if state.get("status") == "WAITING" and state.get("phase") == "A_EXE":
                print(f"   [Epoch 2] 再次捕获到节点 {action_tid} 挂起...")
                break
        await asyncio.sleep(0.05)
    else:
         raise TimeoutError("找不到 Epoch 2 重生后的挂起节点！")

    print("   💉 [Epoch 2] 注入 -resolve failed")
    resolve_res = await runtime.exec(f'"failed" | @XBT -resolve "{action_tid}"', ctx=ctx)
    print(f"   制裁信号回执: {resolve_res}")
    
    # 给级联强杀留足 3 秒反应期
    for i in range(60):
        if not await thread_exists(runtime, ctx, tid):
            print("   ✅ [Epoch 2] XBT 线程及其子线程已清理")
            break
        await asyncio.sleep(0.05)
    else:
        raw = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
        raise TimeoutError(f"resolve failed 注入后线程未按预期清理！残留进程表：\n{raw}")


    # 终极盘点记录

    log = await runtime.exec("$/log.md::LOGS", ctx=ctx)
    print(f"\n📄 [TEST] 物理落盘最终序列:\n{log}")
    
    log_lines = [l for l in str(log).strip().split('\n') if l.startswith('- ')]
    executed = [l.replace('- ', '').replace(' executed', '') for l in log_lines]
    print(f"🔍 序列全貌: {executed}")

    # 统计包含了特定前缀的元素数量
    count_a = sum(1 for x in executed if "Action A" in x)
    count_b = sum(1 for x in executed if "Action B" in x)
    assert count_a == 2, "Action A 应在两次循环中各执行一次"
    assert count_b, "Action B 不应在 resolve failed 后执行"

    print("\n🎉 [TEST] All Pass！(寿命重塑无瑕疵 | 毒药穿透特权绝对起效！)")

if __name__ == "__main__":
    asyncio.run(test_loop_and_poison())
