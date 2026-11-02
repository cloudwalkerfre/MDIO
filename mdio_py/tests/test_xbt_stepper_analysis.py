import sys
import os
import asyncio
import uvloop
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))


# 🧬 原子执行追踪剖析器 (Atomic Execution Tracing Profiler)
# 定位各组件 CPU 耗时分布 —— telemetry 版(§5 profiler 迁移模板)

# 探针注入用 tele.trace(惰性:无激活 session 零开销;激活后进 Rust 时钟
# 采样);分析视角是消费方的(tele.data_fn 定制 polars 聚合);session 用
# rt.telemetry 包住测试主体(退出自动 report_session:终端 + 四产物)。

import polars as pl
import mdio_py.telemetry as tele

import mdio_py.core as core_module
import mdio_py.core.optimizer as opt_module

# --- 1. 深潜 Parser (把内部耗时全端出来) ---
from mdio_py.core.mdio_transformer import MdioTransformer
from lark import Lark
import mdio_py

import pytest

# telemetry 探针需要 core 编译时带 telemetry 特性；release/`fast` 档 wheel 不带 ⇒ 跳过（与布局无关）
_HAS_TELEMETRY = hasattr(getattr(mdio_py, "mdio_core", None), "ProjectStore") and \
    hasattr(mdio_py.mdio_core.ProjectStore, "telemetry_session_create")
pytestmark = pytest.mark.skipif(
    not _HAS_TELEMETRY,
    reason="core 未编译 telemetry（release/`fast` 档构建），本文件是 telemetry 探针专用",
)

Lark.parse = tele.trace("1_Compiler/1.Lark")(Lark.parse)
MdioTransformer.transform = tele.trace("1_Compiler/2.Transformer")(MdioTransformer.transform)
core_module.parse_code = tele.trace("1_Compiler/0.parse_code_TOTAL")(core_module.parse_code)
opt_module.optimize_ir = tele.trace("1_Compiler/3.Opt.optimize_ir")(opt_module.optimize_ir)

mdio_py.parse_code = core_module.parse_code
mdio_py.optimize_ir = opt_module.optimize_ir

# --- 2. 深潜 Interpreter (切开黑盒) ---
from mdio_py.core.interpreter import Interpreter

Interpreter.run = tele.trace("2_Interpreter/0.Total_Run")(Interpreter.run)
Interpreter.visit_pipeline = tele.trace("2_Interpreter/1.visit_pipeline")(Interpreter.visit_pipeline)
Interpreter._evaluate_args = tele.trace("2_Interpreter/2.evaluate_args")(Interpreter._evaluate_args)

# --- 3. Rust FFI 边界 (真实物理耗时) ---
from mdio_py.runtime.workspace import Workspace

Workspace.write_locator = tele.trace("3_Rust_FFI/1.write_locator")(Workspace.write_locator)
Workspace.read_locator = tele.trace("3_Rust_FFI/2.read_locator")(Workspace.read_locator)
Workspace.query_memory = tele.trace("3_Rust_FFI/3.query_memory")(Workspace.query_memory)
Workspace.expand_path_pattern = tele.trace("3_Rust_FFI/4.expand_pattern")(Workspace.expand_path_pattern)

# --- 4. XBT 状态机 (业务拓扑开销) ---
from mdio_py.plugins.xbt.stepper import XBTStepper
import mdio_py.plugins.xbt.stepper as stepper_mod

XBTStepper.step = tele.trace("4_XBT_Core/1.step")(XBTStepper.step)
XBTStepper._bubble_up = tele.trace("4_XBT_Core/2._bubble_up")(XBTStepper._bubble_up)
# 值导入修复:stepper.py `from ...parser import parse_xbt` 绑定在 import 时——
# 必须替换 stepper 模块命名空间里的名字(step 内部按模块全局查找)。
stepper_mod.parse_xbt = tele.trace("4_XBT_Core/3.parse_xbt")(stepper_mod.parse_xbt)
XBTStepper._handle_action = tele.trace("4_XBT_Core/4._handle_action")(XBTStepper._handle_action)
XBTStepper._handle_control = tele.trace("4_XBT_Core/5._handle_control")(XBTStepper._handle_control)
XBTStepper._fork_children = tele.trace("4_XBT_Core/6._fork_children")(XBTStepper._fork_children)
XBTStepper._read_state = tele.trace("4_XBT_Core/7._read_state")(XBTStepper._read_state)
XBTStepper._update_fields = tele.trace("4_XBT_Core/8._update_fields")(XBTStepper._update_fields)
XBTStepper._read_blueprint = tele.trace("4_XBT_Core/9._read_blueprint")(XBTStepper._read_blueprint)
XBTStepper._eval_condition = tele.trace("4_XBT_Core/10._eval_condition")(XBTStepper._eval_condition)
stepper_mod.resolve_pointer = tele.trace("4_XBT_Core/11.resolve_pointer")(stepper_mod.resolve_pointer)

# --- 定制分析(消费方视角,§5 可插拔管道)---
# data_fn: df_raw {name_hash, name, dur_ns} → 按 name 聚合(原报告的扩展:
# cnt/总量/均耗 + p50/p90 分布)。输入永远是 df_raw,聚合从 dur_ns 算起。
def xbt_analysis(df):
    return (df.group_by("name")
            .agg([
                pl.col("dur_ns").count().alias("cnt"),
                (pl.col("dur_ns").sum() / 1_000_000).round(2).alias("total_ms"),
                (pl.col("dur_ns").mean() / 1_000.0).round(2).alias("avg_us"),
                (pl.col("dur_ns").median() / 1_000.0).round(1).alias("p50_us"),
                (pl.col("dur_ns").quantile(0.9) / 1_000.0).round(1).alias("p90_us"),
                (pl.col("dur_ns").max() / 1_000.0).round(1).alias("max_us"),
            ])
            .sort("total_ms", descending=True))

# --- 定制渲染:原报告的表格观感(组件/次数/总耗时/均耗 + 分布)---
def xbt_render(df):
    out = []
    out.append(f"{'组件 / 探针位置':<45} | {'调用次数':<10} | {'总耗时(ms)':<10} | {'均耗/次(μs)':<10} | {'p50(μs)':<9} | {'p90(μs)':<9}")
    out.append("-" * 100)
    for r in df.to_dicts():
        out.append(f"{str(r['name']):<45} | {r['cnt']:<10} | {r['total_ms']:<10.2f} | {r['avg_us']:<10.2f} | {r['p50_us']:<9.1f} | {r['p90_us']:<9.1f}")
    out.append("-" * 100)
    return "\n".join(out)

def xbt_header(df):
    return ("=" * 60 + "\n"
            "    MDIO Engine 绝对内核剖析报告 (telemetry 版)\n" +
            "=" * 60)

tele.data_fn = xbt_analysis
tele.render_fn = xbt_render
tele.header_fn = xbt_header


# 以下是原本的测试执行代码 (无需改动)

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
    print("\n🧪 [TEST] 全 JIT 并发编排性能测试 (telemetry 探针版)...")
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

    blueprint_path = "/fake_meta.md::TEST"
    await runtime.exec(f'"# TEST" | +> /fake_meta.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{blueprint_script}\n```"
    await runtime.exec(f'"""{fenced}""" | +> {blueprint_path}', ctx=ctx)

    stop_event = asyncio.Event()
    daemon_task = asyncio.create_task(meta_d_daemon(runtime, ctx, stop_event))

    tid = "tid_jit_test"
    print(f"\n📌 [TEST] 调用 Syscall 发车：")
    t0 = time.perf_counter()

    # telemetry session 包住压测主体(bench 模式:分布统计;退出自动报告)
    with runtime.telemetry("xbt_stepper_analysis"):
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

    print(f"\n✅ 测试通过 (wall {total_time:.2f} ms),报告见上方 telemetry 输出与 .telemetry/xbt_stepper_analysis/ 四产物。\n")

if __name__ == "__main__":
    uvloop.run(test_pure_jit_topology())
