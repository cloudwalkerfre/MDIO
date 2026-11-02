# tests/test_stepper_nested_simple.py

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
                    
                    prompt = await runtime.exec(f'"" | @XBT -inspect "{tid}" -k "prompt"', ctx=ctx)
                    prompt = str(prompt).strip() if prompt else ""
                    
                    if "meta_log_" in prompt:
                        log_msg = prompt.replace("meta_log_", "") + " executed"
                        # [Meta-D 侧] 将业务结果写到外部黑板
                        await runtime.exec(f'"- {log_msg}" | +> /log.md::LOGS', ctx=ctx)
                        
                        print(f"   🪄 [Meta-D] 探勘到 Action[{tid}] 挂起 (prompt='{prompt}')，解开枷锁...")
                        # [Meta-D 侧] 注入答案，令系统恢复流转
                        await runtime.exec(f'"SUCCESS" | @XBT -resolve "{tid}"', ctx=ctx)
                        
        await asyncio.sleep(0.05)
    print("🤖 [Meta-D] Daemon 安全退出。")


async def test_nested_sequence():
    print("\n🧪 [TEST] 嵌套 Sequence (>> 包含 >>) 全自动 OS 级跑测 (适配前置安检)")

    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    
    print("📁 [TEST] 格式化 VFS 分区并构建基础文件...")
    # .mds/proc.md 系统会自动生成，只需创建业务文件
    await runtime.exec('mk -f /fake_meta.md', ctx=ctx)
    await runtime.exec('mk -f /log.md', ctx=ctx)
    await runtime.exec('"# LOGS" | +> /log.md:: -f', ctx=ctx)

    blueprint_script = """
>> []
    >> [] meta_log_A
    >> [] meta_log_B
    """
    
    # 🌟 架构级调整：构建 Head 容器，将蓝图代码压入 ```xbt code block
    blueprint_path = "/fake_meta.md::TEST"
    await runtime.exec(f'"# TEST" | +> /fake_meta.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{blueprint_script}\n```"
    await runtime.exec(f'"""{fenced}""" | +> {blueprint_path}', ctx=ctx)

    stop_event = asyncio.Event()
    daemon_task = asyncio.create_task(meta_d_daemon(runtime, ctx, stop_event))

    tid = "tid_test"

    print(f"\n📌 [TEST] 调用 Syscall 分配执行树 (-spawn)")
    spawn_res = await runtime.exec(
        f'"" | @XBT -spawn "{tid}" -bp "{blueprint_path}" -ppid "root" -ptr "0"', 
        ctx=ctx
    )

    print("\n⏳ [TEST] 引擎全速狂飙中，验证深层队列层层穿透...")
    for i in range(100):
        if not await thread_exists(runtime, ctx, tid):
            print("✅ [TEST] 探测到根线程自尽！整个深空嵌套执行完毕。")
            break
        await asyncio.sleep(0.05)
    else:
        stop_event.set()
        raw = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
        print(f"💣 超时！残骸死锁现场：\n{raw}")
        raise TimeoutError("执行超时！")

    stop_event.set()
    await daemon_task

    log = await runtime.exec("$/log.md::LOGS", ctx=ctx)
    print(f"\n📄 [TEST] 执行流水:\n{log}")
    
    log_lines = [l for l in str(log).strip().split('\n') if l.startswith('- ')]
    executed = [l.replace('- ', '').replace(' executed', '') for l in log_lines]

    print(f"🔍 落盘序列: {executed}")
    
    assert "A" in executed and "B" in executed, "❌ A或B丢失执行权！"
    assert executed.index("A") < executed.index("B"), "❌ 嵌套死锁或越级！A必须严格在B之前执行。"
    
    print("\n🎉 [TEST] All Pass！多层递归队列完全畅通！")

if __name__ == "__main__":
    asyncio.run(test_nested_sequence())
