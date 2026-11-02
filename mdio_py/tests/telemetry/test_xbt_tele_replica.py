"""Telemetry 1:1 复刻 — 完整对标 test_xbt_stepper_analysis.py。
只替换打点工具 (PROFILER → trace/TelemetrySession)，测试逻辑一字不动。
"""
import sys, os, asyncio, time, json, polars as pl
from functools import wraps
from collections import defaultdict

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py.telemetry import trace, TelemetrySession

import mdio_py.core as core_module
import mdio_py.core.optimizer as opt_module
from mdio_py.core.mdio_transformer import MdioTransformer
from lark import Lark
import mdio_py

# 拆解解析开销：是死在状态机，还是死在 Transformer 构建对象？
orig_parse = Lark.parse
Lark.parse = trace("1_Compiler/1.Lark (Raw Regex & AST)")(orig_parse)
MdioTransformer.transform = trace("1_Compiler/2.Transformer (Obj Build)")(MdioTransformer.transform)
core_module.parse_code = trace("1_Compiler/0.parse_code_TOTAL")(core_module.parse_code)
opt_module.optimize_ir = trace("1_Compiler/3.Opt.optimize_ir")(opt_module.optimize_ir)

mdio_py.parse_code = core_module.parse_code
mdio_py.optimize_ir = opt_module.optimize_ir

from mdio_py.core.interpreter import Interpreter
Interpreter.run = trace("2_Interpreter/0.Total_Run")(Interpreter.run)
Interpreter.visit_pipeline = trace("2_Interpreter/1.visit_pipeline")(Interpreter.visit_pipeline)
Interpreter._evaluate_args = trace("2_Interpreter/2.evaluate_args (Params)")(Interpreter._evaluate_args)

from mdio_py.runtime.workspace import Workspace
Workspace.write_locator = trace("3_Rust_FFI/1.write_locator")(Workspace.write_locator)
Workspace.read_locator = trace("3_Rust_FFI/2.read_locator")(Workspace.read_locator)
Workspace.query_memory = trace("3_Rust_FFI/3.query_memory")(Workspace.query_memory)
Workspace.expand_path_pattern = trace("3_Rust_FFI/4.expand_pattern (*/?)")(Workspace.expand_path_pattern)

from mdio_py.plugins.xbt.stepper import XBTStepper
import mdio_py.plugins.xbt.parser as xbt_parser
XBTStepper.step = trace("4_XBT_Core/1.step")(XBTStepper.step)
XBTStepper._bubble_up = trace("4_XBT_Core/2._bubble_up")(XBTStepper._bubble_up)
xbt_parser.parse_xbt = trace("4_XBT_Core/3.parse_xbt_ast")(xbt_parser.parse_xbt)

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext

async def thread_exists(runtime, ctx, tid: str):
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
    return bool(raw and "- mdio_error:" not in raw)

async def meta_d_daemon(runtime, ctx, stop_event):
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
        await asyncio.sleep(0)

async def test_pure_jit_topology():
    print("\n🧪 [TEST] 全 JIT 并发编排性能测试 (纳米级原子切片探针版)...")
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    await runtime.exec('mk -f /fake_meta.md', ctx=ctx)
    await runtime.exec('mk -f /log.md', ctx=ctx)
    await runtime.exec('"# LOGS" | +> /log.md:: -f', ctx=ctx)

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
        >> [] {{ "- E executed" | +> /log.md::LOGS }}
        >> [] {{ "- F executed" | +> /log.md::LOGS }}
        >> [] {{ "- G executed" | +> /log.md::LOGS }}
        >> []
            == []
                >> [] {{ "- H executed" | +> /log.md::LOGS }}
                >> [] {{ "- I executed" | +> /log.md::LOGS }}
                >> [] {{ "- G_again executed" | +> /log.md::LOGS }}
            == []
                == [] {{ "- K executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K1 executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K2 executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K3 executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K4 executed" | +> /log.md::LOGS }}
                == [ {{ "1" | test -le "0" }} ] {{ "- K5 executed" | +> /log.md::LOGS }}
            == []
                >> [] {{ "- L executed" | +> /log.md::LOGS }}
                >> [] {{ "- M executed" | +> /log.md::LOGS }}
                >> [ {{ "1" | test -le "0" }} ] {{ "- M2 executed" | +> /log.md::LOGS }}
        >> [] {{ "- N executed" | +> /log.md::LOGS }}
        >> [] {{ "- O executed" | +> /log.md::LOGS }}
        >> [ {{ "1" | test -le "0" }} ]
            ?? [ {{ "1" | test -le "0" }} ] {{ "- P executed" | +> /log.md::LOGS }}
            ?? [ {{ "1" | test -gt "0" }} ] {{ "- P executed" | +> /log.md::LOGS }}
"""
    
    # 把脚本包装入容器，使用 ```xbt code block
    blueprint_path = "/fake_meta.md::TEST"
    await runtime.exec(f'"# TEST" | +> /fake_meta.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{blueprint_script}\n```"
    await runtime.exec(f'"""{fenced}""" | +> {blueprint_path}', ctx=ctx)

    stop_event = asyncio.Event()
    daemon_task = asyncio.create_task(meta_d_daemon(runtime, ctx, stop_event))

    tid = "tid_jit_test"
    print(f"\n📌 [TEST] 调用 Syscall 发车：")
    t0 = time.perf_counter()

    with runtime.telemetry("xbt_replica"):
        # Spawn 传入的是 Head 容器目标
        await runtime.exec(f'"" | @XBT -spawn "{tid}" -bp "{blueprint_path}" -ppid "root" -ptr "0"', ctx=ctx)

        print("⏳ [TEST] 极限压测拉起中...")
        deadline = time.perf_counter() + 5.0  # 5s 壁钟超时
        while True:
            if not await thread_exists(runtime, ctx, tid):
                break
            if time.perf_counter() > deadline:
                stop_event.set()
                raise TimeoutError("引擎发生死锁或耗时过长！")
            await asyncio.sleep(0)

        t1 = time.perf_counter()
        total_time = (t1 - t0) * 1000
        stop_event.set()
        await daemon_task

    print(f"\n✅ 测试通过 (wall: {total_time:.1f}ms)\n")

if __name__ == "__main__":
    asyncio.run(test_pure_jit_topology())
