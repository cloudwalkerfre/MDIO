# tests/test_stepper_meta.py

import sys
import os
import asyncio

from pathlib import Path
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext

async def thread_exists(runtime, ctx, tid: str):
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
    return bool(raw and "- mdio_error:" not in raw)

async def meta_d_daemon(runtime, ctx, stop_event):
    print("🤖 [Meta-D] Daemon 启动，开始巡视系统线程表...")
    while not stop_event.is_set():
        raw_xbt = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
        if raw_xbt and "- mdio_error:" not in raw_xbt:
            current_tid = None
            threads = {}
            for line in raw_xbt.strip().split('\n'):
                if line.startswith('### '):
                    current_tid = line[4:].strip()
                    threads[current_tid] = {}
                elif line.startswith('- ') and ':' in line and current_tid:
                    k, v = line[2:].split(':', 1)
                    threads[current_tid][k.strip()] = v.strip()
            
            for tid, state in threads.items():
                if state.get("status") == "WAITING" and state.get("phase") == "A_EXE":
                    
                    prompt = await runtime.exec(f'@XBT -inspect "{tid}" -k "prompt"', ctx=ctx)
                    prompt = str(prompt).strip() if prompt else ""
                    
                    if "meta_log_" in prompt:
                        log_msg = prompt.replace("meta_log_", "") + " executed"
                        await runtime.exec(f'"- {log_msg}" | +> /log.md::LOGS', ctx=ctx)
                        
                        print(f"   🪄 [Meta-D] 探勘到 Action[{tid}] (prompt='{prompt}'), 正在注入干预...")
                        await runtime.exec(f'"SUCCESS" | @XBT -resolve "{tid}"', ctx=ctx)
                        
        await asyncio.sleep(0.05)
    print("🤖 [Meta-D] Daemon 安全退出。")


async def test_complex_meta_d():
    print("\n🧪 [TEST] 复杂并发 Meta-D (== 包含 >>) 全自动 Hook 事件流测试")

    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    print("📁 [TEST] 构建 VFS 空间初始隔离带...")
    # .mds/proc.md 系统初始化时已内建，建好业务骨架即可
    await runtime.exec('mk -f /fake_meta.md', ctx=ctx)
    await runtime.exec('mk -f /log.md', ctx=ctx)
    
    await runtime.exec('"# LOGS" | +> /log.md:: -f', ctx=ctx)

    # 回归纯净逻辑：无条件并发下派发动作
    blueprint_script = """
>> []
    == []
        >> []
            meta_log_A1
            meta_log_A2
        >> []
            meta_log_B1
            meta_log_B2
>> [] meta_log_C
    """
    
    # 🌟 架构升级：使用标准的容器化蓝图载入
    blueprint_path = "/fake_meta.md::TEST"
    await runtime.exec(f'"# TEST" | +> /fake_meta.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{blueprint_script}\n```"
    await runtime.exec(f'"""{fenced}""" | +> {blueprint_path}', ctx=ctx)

    stop_event = asyncio.Event()
    daemon_task = asyncio.create_task(meta_d_daemon(runtime, ctx, stop_event))

    tid = "tid_test"

    print(f"\n📌 [TEST] 物理指令执行：(-spawn) 向中断表挂载母舰线程！")
    # 🌟 -bp 指向纯净的架构父容器
    spawn_res = await runtime.exec(f'@XBT -spawn "{tid}" -bp "{blueprint_path}" -ppid "root" -ptr "0"', ctx=ctx)
    print(f"   SysCall Echo: {spawn_res}")

    print("\n⏳ [TEST] 引擎全速狂飙中...")
    for i in range(100):  
        if not await thread_exists(runtime, ctx, tid):
            print("✅ [TEST] 主干线程彻底落幕！")
            break
        await asyncio.sleep(0.05)
    else:
        stop_event.set()
        # 超时时把存活的残骸打出来，方便排雷
        raw = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
        print(f"💣 超时残骸：\n{raw}")
        raise TimeoutError("执行逾时，发生死局坍缩！")

    stop_event.set()
    await daemon_task

    log = await runtime.exec("$/log.md::LOGS", ctx=ctx)
    print(f"\n📄 [TEST] 最终战果烙片:\n{log}")
    
    log_lines = [l for l in str(log).strip().split('\n') if l.startswith('- ')]
    executed = [l.replace('- ', '').replace(' executed', '') for l in log_lines]

    print(f"🔍 落盘全貌: {executed}")
    
    assert "A1" in executed and "A2" in executed
    assert "B1" in executed and "B2" in executed
    assert "C" in executed

    assert executed.index("A1") < executed.index("A2"), "❌ A1未能在A2前开火！"
    assert executed.index("B1") < executed.index("B2"), "❌ B1未能在B2前越界！"
    assert executed[-1] == "C", "❌ C 不是最后一个被点名！"

    print("\n🎉 [TEST] All Pass！卸下枷锁的引擎配合 Meta-D 已完美通关！")

if __name__ == "__main__":
    asyncio.run(test_complex_meta_d())
