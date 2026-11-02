import sys
import os
import textwrap
import asyncio

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.xbt.stepper import XBTStepper

async def read_thread_state(runtime, ctx, abs_pid: str) -> dict:
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{abs_pid}", ctx=ctx)
    if not raw or "- mdio_error:" in raw:
        return {}
    state = {}
    for line in raw.strip().split('\n'):
        line = line.strip()
        if line.startswith('- ') and ':' in line:
            k, v = line[2:].split(':', 1)
            state[k.strip()] = v.strip()
    return state

async def test_stepper_fake_meta_d():
    print("\n🧪 [TEST] 初始化内存运行时 & 虚拟操作系统...")
    runtime = MdioRuntime("/mem")
    workspace = runtime._workspace
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    print("📁 [TEST] 构建 VFS 文件结构...")
    await runtime.exec('mk -f /fake_meta.md', ctx=ctx)

    blueprint_path = "/fake_meta.md::TEST"
    script_content = textwrap.dedent("""
    >> []
        >> [] 模拟动作：打印日志
    """).strip()

    print(f"📝 [TEST] 烧录蓝图容器与附庸脚本: {blueprint_path}")
    await runtime.exec(f'"# TEST" | +> /fake_meta.md:: -f', ctx=ctx)
    await runtime.exec(f'"my_brain" | +> {blueprint_path}._b:meta -f', ctx=ctx)
    fenced = f"```xbt\n{script_content}\n```"
    await runtime.exec(f'"""{fenced}""" | +> {blueprint_path}', ctx=ctx)

    print("\n📋 [TEST] 步骤 0: 使用 SysCall 分配并激活根线程 tid_test...")
    tid = "tid_test"
    spawn_res = await runtime.exec(f'@XBT -spawn "{tid}" -bp "{blueprint_path}" -ppid "root" -ptr "0"', ctx=ctx)
    print(f"   SysCall 返回: {spawn_res}")
    
    init_state = await read_thread_state(runtime, ctx, tid)
    assert init_state.get("status") == "WAKE_UP", "Spawn 初始化必须为 WAKE_UP"
    assert init_state.get("blueprint") == blueprint_path

    print("\n🚀 [TEST] 步骤 1: 驱动大根 (Virtual Root) ...")
    result = await XBTStepper.step(workspace, ctx, tid)
    print(f"   Stepper 返回: {result}")
    
    parent_state = await read_thread_state(runtime, ctx, tid)
    assert parent_state.get("status") == "PENDING"
    
    child_tid = f"{tid}_0"
    child_state = await read_thread_state(runtime, ctx, child_tid)
    assert child_state.get("status") == "WAKE_UP"
    print("   ✅ 步骤 1 通过")
    
    print("\n🚀 [TEST] 步骤 2: 驱动大儿子 (Sequence Node) ...")
    result = await XBTStepper.step(workspace, ctx, child_tid)
    print(f"   Stepper 返回: {result}")
    
    action_tid = f"{child_tid}_0"
    action_state = await read_thread_state(runtime, ctx, action_tid)
    assert action_state.get("status") == "WAKE_UP"
    print("   ✅ 步骤 2 通过")
    
    print("\n🚀 [TEST] 第三次 step (动作节点，应挂起)...")
    result = await XBTStepper.step(workspace, ctx, action_tid)
    print(f"   Stepper 返回: {result}")
    
    action_state = await read_thread_state(runtime, ctx, action_tid)
    assert action_state.get("status") == "WAITING"
    assert action_state.get("phase") == "A_EXE"
    print("   ✅ 第三次 step 通过，完美挂起并冻结指针相态")
    
    print("\n🤖 [TEST] 步骤 4 & 5 合并: 模拟外部动作完成，调用 SysCall -resolve 跨时空起搏并瞬时冒泡...")
    resolve_res = await runtime.exec(f'"SUCCESS" | @XBT -resolve "{action_tid}"', ctx=ctx)
    print(f"   SysCall 返回: {resolve_res}")
    
    action_exists = await read_thread_state(runtime, ctx, action_tid)
    assert not action_exists, "action 节点在 resolve 后应从 proc.md 中删除"

    # 验证：子节点清理后，父节点被唤醒并收到子结果
    child_state_resumed = await read_thread_state(runtime, ctx, child_tid)
    assert child_state_resumed.get("status") == "WAKE_UP", "父节点状态应变为 WAKE_UP"
    assert child_state_resumed.get(f"res_{action_tid}") == "SUCCESS", "父节点应通过 res_<tid> 收到子结果"
    print("   ✅ 步骤 4 & 5 通过：引擎极速自动消化了中断恢复、向上气泡、自身死灭三个周期的演进！")
    
    print("\n🚀 [TEST] 步骤 6: Sequence 节点苏醒结算，继续向上冒泡 ...")
    result = await XBTStepper.step(workspace, ctx, child_tid)
    print(f"   Stepper 返回: {result}")
    
    child_exists = await read_thread_state(runtime, ctx, child_tid)
    assert not child_exists
    
    root_state_resumed = await read_thread_state(runtime, ctx, tid)
    assert root_state_resumed.get("status") == "WAKE_UP"
    assert root_state_resumed.get(f"res_{child_tid}") == "SUCCESS"
    print("   ✅ 步骤 6 通过")
    
    print("\n🚀 [TEST] 步骤 7: Virtual Root 节点苏醒，进入 Loop 寿命结算 ...")
    result = await XBTStepper.step(workspace, ctx, tid)
    print(f"   Stepper 返回: {result}")
    
    root_exists = await read_thread_state(runtime, ctx, tid)
    assert not root_exists, "根节点没有正确清理自身！"
    print("   ✅ 步骤 7 通过")
    
    print("\n🎉 [TEST] 全生命周期单步断言测试完美通过！系统边界与微指令状态机坚如磐石！")

if __name__ == "__main__":
    asyncio.run(test_stepper_fake_meta_d())
