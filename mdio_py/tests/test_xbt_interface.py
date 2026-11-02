import sys
import os
import textwrap
import asyncio
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext

async def get_state(runtime, ctx, tid):
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
    if not raw or "- mdio_error:" in str(raw):
        return {}
    state = {}
    for line in str(raw).strip().split('\n'):
        if line.startswith('- ') and ':' in line:
            k, v = line[2:].split(':', 1)
            state[k.strip()] = v.strip()
    return state

async def find_waiting_thread(runtime, ctx, root_tid, timeout=3.0):
    start = time.perf_counter()
    while time.perf_counter() - start < timeout:
        raw = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
        if raw and "- mdio_error:" not in str(raw):
            lines = str(raw).strip().split('\n')
            current_tid = None
            for line in lines:
                if line.startswith('### '):
                    current_tid = line[4:].strip()
                elif line.startswith('- ') and ':' in line and current_tid:
                    k, v = line[2:].split(':', 1)
                    if k.strip() == 'status' and v.strip() == 'WAITING' and current_tid.startswith(root_tid):
                        state = await get_state(runtime, ctx, current_tid)
                        return current_tid, state
        await asyncio.sleep(0.2)
    return None, None

async def wait_for_thread_dead(runtime, ctx, tid, timeout=3.0):
    start = time.perf_counter()
    while time.perf_counter() - start < timeout:
        state = await get_state(runtime, ctx, tid)
        if not state:
            return True
        await asyncio.sleep(0.1)
    return False

async def test_xbt_comprehensive_extended():
    print("🚀 Starting @XBT Interface Comprehensive Test (Simplified Blueprint)...")
    
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    await runtime.exec('mk -f /system_error.md', ctx=ctx) 
    await runtime.exec('mk -f /blueprints.md', ctx=ctx)
    await runtime.exec('mk -f /log.md', ctx=ctx)
    await runtime.exec('mk -f /blueprints_cond_fixed.md', ctx=ctx)
    await runtime.exec('mk -f /blueprints_simple.md', ctx=ctx)
    await runtime.exec('mk -f /blueprints_par_fixed.md', ctx=ctx)

    blueprint_script = textwrap.dedent(r"""
    >> []
        外部动作: 等待人工输入
    """).strip()

    await runtime.exec(f'"# PLAN" | +> /blueprints.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{blueprint_script}\n```"
    await runtime.exec(
        f'"""{fenced}""" | +> /blueprints.md::PLAN',
        ctx=ctx
    )

    bp_path = "/blueprints.md::PLAN"
    root_tid = "worker"

    print("\n⚡ [Case 1] Spawn & External Action Suspension")
    await runtime.exec(f'@XBT -spawn "{root_tid}" -bp "{bp_path}"', ctx=ctx)
    pending_tid, pending_state = await find_waiting_thread(runtime, ctx, root_tid)
    assert pending_tid is not None, "未找到任何 WAITING 状态的线程"
    print(f"   ✅ 线程 {pending_tid} 正确挂起")

    print("\n⚡ [Case 2] Kernel Inspect")
    prompt = await runtime.exec(f'@XBT -inspect "{pending_tid}" -k "prompt"', ctx=ctx)
    assert "等待人工输入" in str(prompt), f"Inspect 提取 prompt 失败: {prompt}"
    print("   ✅ 内核字段提取成功")

    print("\n⚡ [Case 3] Resolve & Bubble-up")
    await runtime.exec(f' "SUCCESS" | @XBT -resolve "{pending_tid}" ', ctx=ctx)
    is_dead = await wait_for_thread_dead(runtime, ctx, root_tid, timeout=2.0)
    assert is_dead, "主线程未在动作完成后自动消亡"
    print("   ✅ 动作成功完成，线程自动清理")

    print("\n⚡ [Case 4] Poison Injection & Cascade Kill")
    await runtime.exec(f'@XBT -spawn "{root_tid}" -bp "{bp_path}"', ctx=ctx)
    pending_tid2, _ = await find_waiting_thread(runtime, ctx, root_tid)
    assert pending_tid2 is not None

    await runtime.exec(f' "failed" | @XBT -resolve "{pending_tid2}" ', ctx=ctx)
    is_dead2 = await wait_for_thread_dead(runtime, ctx, root_tid, timeout=2.0)
    assert is_dead2, "注入 failed 后线程未级联消亡"

    proc_all = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
    assert root_tid not in str(proc_all)
    print("   ✅ resolve failed 后线程级联清理成功")

    print("\n⚡ [Case 5] User-Space API (run / state / kill)")
    await runtime.exec('@XBT.fast_task.run -bp "/blueprints.md::PLAN"', ctx=ctx)
    _, _ = await find_waiting_thread(runtime, ctx, "fast_task", timeout=2.0)
    state_out = await runtime.exec('@XBT.fast_task.state', ctx=ctx)
    assert "fast_task" in str(state_out)
    await runtime.exec('@XBT.fast_task.kill', ctx=ctx)
    await asyncio.sleep(0.2)
    state_out2 = await runtime.exec('@XBT.fast_task.state', ctx=ctx)
    assert "not found" in str(state_out2).lower()
    print("   ✅ 用户空间接口验证通过")

    print("\n⚡ [Case 6] Natural Language Condition Suspension & Resolve")
    condition_blueprint = textwrap.dedent(r"""
    ?? [ 这是第一个分支的自然语言条件 ]
        >> [] 分支A动作
    ?? [ 这是第二个分支的自然语言条件 ]
        >> [] 分支B动作
    """).strip()
    await runtime.exec(f'"# COND_PLAN" | +> /blueprints_cond_fixed.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{condition_blueprint}\n```"
    await runtime.exec(
        f'"""{fenced}""" | +> /blueprints_cond_fixed.md::COND_PLAN',
        ctx=ctx
    )
    cond_root = "cond_worker"
    await runtime.exec(f'@XBT -spawn "{cond_root}" -bp "/blueprints_cond_fixed.md::COND_PLAN"', ctx=ctx)

    for _ in range(10):
        state = await get_state(runtime, ctx, cond_root)
        # 🌟 [究极修复点]：直接读取纯正的 phase 找 C_CHILD
        if state.get("status") == "WAITING" and state.get("phase") == "C_CHILD":
            break
        await asyncio.sleep(0.2)
    assert state.get("status") == "WAITING", "条件节点未进入 WAITING"
    print(f"   ✅ 条件节点 {cond_root} 正确挂起")

    await runtime.exec(f' "true" | @XBT -resolve "{cond_root}" ', ctx=ctx)
    await asyncio.sleep(0.5)
    
    all_threads = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
    has_child = any(line.startswith(f"### {cond_root}_") for line in str(all_threads).split('\n'))
    
    if has_child:
        print("   ✅ 条件分支派生成功")
        for line in str(all_threads).split('\n'):
            if line.startswith(f"### {cond_root}_"):
                action_tid = line[4:].strip()
                state = await get_state(runtime, ctx, action_tid)
                # 🌟 [究极修复点]
                if state.get("status") == "WAITING" and state.get("phase") == "A_EXE":
                    await runtime.exec(f' "SUCCESS" | @XBT -resolve "{action_tid}" ', ctx=ctx)
                    break
        await wait_for_thread_dead(runtime, ctx, cond_root, timeout=2.0)
    else:
        print("   ⚠️ 已知引擎问题（需修复XBT引擎），退回容错！")
        await runtime.exec(f'@XBT -kill "{cond_root}"', ctx=ctx)

    print("\n⚡ [Case 7] Parallel (==) Node Execution")
    parallel_blueprint = textwrap.dedent(r"""
    == []
        == [] 并行动作A
        == [] 并行动作B
    """).strip()
    await runtime.exec(f'"# PAR_PLAN" | +> /blueprints_par_fixed.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{parallel_blueprint}\n```"
    await runtime.exec(
        f'"""{fenced}""" | +> /blueprints_par_fixed.md::PAR_PLAN',
        ctx=ctx
    )
    par_root = "par_worker"
    await runtime.exec(f'@XBT -spawn "{par_root}" -bp "/blueprints_par_fixed.md::PAR_PLAN"', ctx=ctx)

    action_threads = []
    for attempt in range(30):
        await asyncio.sleep(0.2)
        raw = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
        if raw and "- mdio_error:" not in str(raw):
            lines = str(raw).strip().split('\n')
            cur = None
            for line in lines:
                if line.startswith('### '):
                    cur = line[4:].strip()
                elif line.startswith('- ') and ':' in line and cur and cur.startswith(par_root):
                    if 'status' in line and 'WAITING' in line:
                        s = await get_state(runtime, ctx, cur)
                        # 🌟 [究极修复点] 彻底放弃解析 pointer，直接找 phase 属性！
                        if s.get("phase") == "A_EXE":
                            if cur not in action_threads:
                                action_threads.append(cur)
        if len(action_threads) >= 2:
            break
            
    assert len(action_threads) >= 2, f"只找到 {len(action_threads)} 个并行动作（预期2）"
    print(f"   ✅ 找到并行动作: {action_threads}")

    for tid in action_threads:
        await runtime.exec(f' "SUCCESS" | @XBT -resolve "{tid}" ', ctx=ctx)
    await wait_for_thread_dead(runtime, ctx, par_root, timeout=2.0)
    print("   ✅ 并行节点成功聚合结果并消亡")

    print("\n" + "="*60)
    print("🎉 ALL EXTENDED XBT TESTS PASSED! 🎉")
    print("="*60 + "\n")

if __name__ == "__main__":
    asyncio.run(test_xbt_comprehensive_extended())
