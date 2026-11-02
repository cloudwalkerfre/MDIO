import asyncio
import textwrap
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

async def test_natural_lang_condition():
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    await runtime.exec('mk -f /test_nat.md', ctx=ctx)

    # 创建蓝图：一个 ?? 节点，两个分支均为自然语言条件
    blueprint = textwrap.dedent("""
    ?? [ 这是第一个分支：选择此分支 ]
        >> [] 动作A
    ?? [ 这是第二个分支的条件 ]
        >> [] 动作B
    """).strip()
    await runtime.exec(f'"# PLAN" | +> /test_nat.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{blueprint}\n```"
    await runtime.exec(f'"""{fenced}""" | +> /test_nat.md::PLAN', ctx=ctx)

    tid = "nat_worker"
    await runtime.exec(f'@XBT -spawn "{tid}" -bp "/test_nat.md::PLAN"', ctx=ctx)

    # 等待条件节点挂起
    for _ in range(10):
        state = await get_state(runtime, ctx, tid)
        if state.get("status") == "WAITING" and state.get("phase") == "C_CHILD":
            break
        await asyncio.sleep(0.2)
    else:
        raise AssertionError("条件节点未挂起")
    print("✅ 条件节点挂起成功")

    # 选择第一个分支
    await runtime.exec(f' "true" | @XBT -resolve "{tid}" ', ctx=ctx)

    # 等待动作节点出现并挂起
    action_tid = None
    for _ in range(10):
        raw = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
        if raw and "- mdio_error:" not in str(raw):
            lines = str(raw).strip().split('\n')
            cur = None
            for line in lines:
                if line.startswith('### '):
                    cur = line[4:].strip()
                elif line.startswith('- ') and ':' in line and cur and cur.startswith(tid):
                    if 'status' in line and 'WAITING' in line:
                        s = await get_state(runtime, ctx, cur)
                        if s.get("phase") == "A_EXE":
                            action_tid = cur
                            break
            if action_tid:
                break
        await asyncio.sleep(0.2)
    assert action_tid, "未找到动作节点"
    print(f"✅ 动作节点 {action_tid} 挂起")

    # 完成动作
    await runtime.exec(f' "SUCCESS" | @XBT -resolve "{action_tid}" ', ctx=ctx)
    await asyncio.sleep(0.5)

    # 验证线程消亡
    final_state = await get_state(runtime, ctx, tid)
    assert not final_state, "线程未清理"
    print("✅ 自然语言条件分支全流程通过")

if __name__ == "__main__":
    asyncio.run(test_natural_lang_condition())