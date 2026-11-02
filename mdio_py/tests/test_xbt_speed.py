import sys
import os
import textwrap
import asyncio
import uvloop
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext

async def thread_exists(runtime, ctx, tid: str):
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
    return bool(raw and "- mdio_error:" not in raw)

async def test_xbt_benchmark():
    print("\n🧪 [BENCHMARK] @XBT 引擎极限调度性能压测 (完美单轨态)")

    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    print("📁 构建极短路径靶心...")
    
    await runtime.exec('mk -f /fake_meta.md', ctx=ctx)
    await runtime.exec('mk -f /b.md', ctx=ctx)
    await runtime.exec('"# C" | +> /b.md:: -f', ctx=ctx)
    await runtime.exec('0 | +> /b.md::C._b:h', ctx=ctx)
    await runtime.exec(f'"# BENCH" | +> /fake_meta.md:: -f', ctx=ctx)

    jit = '{{ "x" | +> /b.md::C._b:h }}'
    
    blueprint_content = textwrap.dedent(f"""
    >> []
        >> []
            == {jit}
            == {jit}
            == {jit}
            == {jit}
            == {jit}
        >> []
            == []
                >> {jit}
                >> {jit}
                >> {jit}
                >> {jit}
            == []  
                >> {jit}
                >> {jit}
                >> {jit}
                >> {jit}
            == []
                >> {jit}
                >> {jit}
                >> {jit}
                >> {jit}
        >> []
            >> {jit}
    """).strip()

    fenced = f"```xbt\n{blueprint_content}\n```"
    await runtime.exec(f'"""{fenced}""" | +> /fake_meta.md::BENCH', ctx=ctx)

    tid = "tid_bench"
    
    print(f"\n🚀 [BENCHMARK] AST 灌入缓存，倒计时发车...")
    start_time = time.perf_counter()

    await runtime.exec(f'"" | @XBT -spawn "{tid}" -bp "/fake_meta.md::BENCH" -ppid "root" -ptr "0"', ctx=ctx)

    check_rounds = 0
    while await thread_exists(runtime, ctx, tid):
        check_rounds += 1
        await asyncio.sleep(0.005)
        await asyncio.sleep(0.005)
        if check_rounds > 2000:
            raise TimeoutError("压测超时！引擎死锁！")

    end_time = time.perf_counter()
    elapsed_ms = (end_time - start_time) * 1000
    
    print("\n✅ [BENCHMARK] 狂飙结束！没有发生死锁！")
    print(f"==================================================")
    print(f" ⏱️ 最终真实物理耗时 (18+并发，单轴无碎片的纯净态):")
    print(f"      --->  {elapsed_ms:.2f} ms  <---")
    print(f"==================================================")
    
    val = await runtime.exec('$/b.md::C._b:h', ctx=ctx)
    print(f"\n📄 高频落锁靶心查验 (应有满屏的 x): \n{val.strip() if val else 'NULL'}")
    x_count = str(val).count("x")
    print(f"🎯 合计命中 {x_count} 次并发修改！(应为 1+5+12=18 次)")
    assert x_count == 18, "并发数据流错乱丢失！"

if __name__ == "__main__":
    uvloop.run(test_xbt_benchmark())
