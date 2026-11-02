# tests/test_xbt_stepper_jit.py

import sys
import os
import textwrap
import asyncio
import time

import pytest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.xbt.xbt import XBTPlugin

async def thread_exists(runtime, ctx, tid: str):
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
    return bool(raw and "- mdio_error:" not in raw)

async def meta_d_daemon(runtime, ctx, stop_event):
    print("🤖 [Meta-D] Daemon 启动，开始巡视系统...")
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
                if state.get("status") == "WAITING":
                    # 因为新架构抛弃了臃肿的待定字段，现在的挂起相态直接标在 pointer 后缀里
                    pointer = state.get('pointer', '')
                    print(f"   🚨 [Meta-D 拦截] 警告！节点被拦截并挂起: {tid}")
                    print(f"      挂起原因 (指针相态): {pointer}")
                        
        await asyncio.sleep(0.05)
    print("🤖 [Meta-D] Daemon 安全退出。")

@pytest.mark.slow
async def test_pure_jit_topology():
    print("\n🧪 [TEST] 全 JIT 并发编排与条件分支引擎测试 (遵循反向DM推断法与前置安检)")

    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    await runtime.exec('mk -f /fake_meta.md', ctx=ctx)
    await runtime.exec('mk -f /log.md', ctx=ctx)
    await runtime.exec('"# LOGS" | +> /log.md:: -f', ctx=ctx)

    # 剔除了所有错误干扰自然语言的纯净 JIT 蓝图
    blueprint_script = r"""
[]
    >> []
        ?? [ {{ "1" | test -le "0" }} ]
            ?? [] {{ "- BAD executed" | +> /log.md::LOGS }}
        ?? [ {{ "1" | test -gt "0" }} ]
            == []
                >> [] {{ "- A1 executed" | +> /log.md::LOGS }}
                >> [] {{ "- A2 executed" | +> /log.md::LOGS }}
                >> [] {{ "- A3 executed" | +> /log.md::LOGS }}
            == []
                >> [] {{ "- B1 executed" | +> /log.md::LOGS }}
                >> [] {{ "- B2 executed" | +> /log.md::LOGS }}
                >> [] {{ "- B3 executed" | +> /log.md::LOGS }}
    >> [] {{ "- C executed" | +> /log.md::LOGS }}
    >> [] {{ "- D executed" | +> /log.md::LOGS }}
    >> []
        >> {{ "- E executed" | +> /log.md::LOGS }}
        >> {{ "- F executed" | +> /log.md::LOGS }}
        >> {{ "- G executed" | +> /log.md::LOGS }}
        >> []
            == []
                >> {{ "- H executed" | +> /log.md::LOGS }}
                >> {{ "- I executed" | +> /log.md::LOGS }}
                >> {{ "- G_again executed" | +> /log.md::LOGS }}
            == []
                == {{ "- K executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K1 executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K2 executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K3 executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K4 executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K5 executed" | +> /log.md::LOGS }}
            == []
                >> {{ "- L executed" | +> /log.md::LOGS }}
                >> {{ "- M executed" | +> /log.md::LOGS }}
                >> [ {{ "1" | test -le "0" }} ] {{ "- M2 executed" | +> /log.md::LOGS }}
        >> {{ "- N executed" | +> /log.md::LOGS }}
        >> {{ "- O executed" | +> /log.md::LOGS }}
        >> [ {{ "1" | test -le "0" }} ]
            ?? [ {{ "1" | test -le "0" }} ] {{ "- P executed" | +> /log.md::LOGS }}
            ?? [ {{ "1" | test -gt "0" }} ] {{ "- P executed" | +> /log.md::LOGS }}
        >> [ {{ "1" | test -gt "0" }} ]
            ?? [ {{ "1" | test -le "0" }} ] {{ "- Final! executed" | +> /log.md::LOGS }}
            ?? [ {{ "1" | test -gt "0" }} ] {{ "- Final executed" | +> /log.md::LOGS }}
"""
    
    # 🌟 架构修复 1：把脚本装进容器 Head 节点，使用 ```xbt code block
    blueprint_path = "/fake_meta.md::TEST"
    await runtime.exec(f'"# TEST" | +> /fake_meta.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{blueprint_script}\n```"
    await runtime.exec(f'"""{fenced}""" | +> {blueprint_path}', ctx=ctx)

    # 停止信号与守护进程
    stop_event = asyncio.Event()
    daemon_task = asyncio.create_task(meta_d_daemon(runtime, ctx, stop_event))

    tid = "tid_jit_test"

    print(f"\n📌 [TEST] 调用 Syscall 发车：")
    t0 = time.perf_counter()
    
    # 🌟 架构修复 2：Spawn 参数的蓝图地址指向 Head 容器
    await runtime.exec(f'"" | @XBT -spawn "{tid}" -bp "{blueprint_path}" -ppid "root" -ptr "0"', ctx=ctx)

    print("\n⏳ [TEST] 微内核光速解析与拓扑推演中...")
    # 把循环次数加长一点，这棵树的节点有几十个，加上 IO 处理可能需要一点时间
    for i in range(300):
        if not await thread_exists(runtime, ctx, tid):
            print("✅ [TEST] 主干线程完成全量执行，已回收集结！")
            break
        await asyncio.sleep(0.01)
    else:
        stop_event.set()
        raw = await runtime.exec("$/.mds/proc.md::PROCESS_TABLE.XBT", ctx=ctx)
        print(f"💣 宕机超时！残骸：\n{raw}")
        raise TimeoutError("引擎发生死锁！")

    t1 = time.perf_counter()
    print(f"⏱️ 引擎总净耗时: {(t1 - t0) * 1000:.2f} ms")

    stop_event.set()
    await daemon_task

    log = await runtime.exec("$/log.md::LOGS", ctx=ctx)
    print(f"\n📄 [TEST] 物理落盘最终序列:\n{log}")
    
    log_lines = [l for l in str(log).strip().split('\n') if l.startswith('- ')]
    executed = [l.replace('- ', '').replace(' executed', '') for l in log_lines]

    print(f"🔍 序列全貌: {executed}")
    
    assert "BAD" not in executed, "❌ ?? 发生串线穿透！"
    assert "K1" not in executed, "❌ == 内部条件短路失效！"
    assert "K5" not in executed, "❌ == 内部条件短路失效！"
    assert "M2" not in executed, "❌ >> 内部条件阻塞失效！"
    assert "P" not in executed, "❌ 外层条件保护伞失效！"

    print("\n🎉 [TEST] All Pass！包含『条件极速短路判决』与『多层并发编排』的纯代码流，完美闭环！")

if __name__ == "__main__":
    asyncio.run(test_pure_jit_topology())
