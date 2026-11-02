"""Telemetry 新特性矩阵验证 — 全部基于 XBT replica 场景。"""
import sys, os, asyncio, time, json, polars as pl

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py.telemetry import trace, span, TelemetrySession, report
import mdio_py.telemetry as tele

import mdio_py.core as core_module
import mdio_py.core.optimizer as opt_module
from mdio_py.core.mdio_transformer import MdioTransformer
from lark import Lark
import mdio_py

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

BLUEPRINT_SCRIPT = r"""
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

async def run_xbt_bench(runtime, ctx, label="bench"):
    """共享的 XBT bench 逻辑，每次调用创建新的 session。"""
    await runtime.exec('mk -f /fake_meta.md', ctx=ctx)
    await runtime.exec('mk -f /log.md', ctx=ctx)
    await runtime.exec('"# LOGS" | +> /log.md:: -f', ctx=ctx)

    blueprint_path = "/fake_meta.md::TEST"
    await runtime.exec(f'"# TEST" | +> /fake_meta.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{BLUEPRINT_SCRIPT}\n```"
    await runtime.exec(f'"""{fenced}""" | +> {blueprint_path}', ctx=ctx)

    stop_event = asyncio.Event()
    daemon_task = asyncio.create_task(meta_d_daemon(runtime, ctx, stop_event))

    tid = "tid_feature_test"
    t0 = time.perf_counter()

    # ★ 特性 1: runtime.telemetry() context manager — 自动 push/pop + report
    with runtime.telemetry(label):
        await runtime.exec(f'"" | @XBT -spawn "{tid}" -bp "{blueprint_path}" -ppid "root" -ptr "0"', ctx=ctx)

        deadline = time.perf_counter() + 5.0
        while True:
            if not await thread_exists(runtime, ctx, tid):
                break
            if time.perf_counter() > deadline:
                stop_event.set()
                raise TimeoutError("引擎发生死锁或耗时过长！")
            await asyncio.sleep(0)

        t1 = time.perf_counter()
        stop_event.set()
        await daemon_task

    return (t1 - t0) * 1000


async def main():
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)


    # 特性 1: runtime.telemetry() 装饰器模式

    print("\n" + "="*60)
    print("  特性 1: @runtime.telemetry() 装饰器模式")
    print("="*60)

    @runtime.telemetry("decorator_mode")
    async def decorated_bench():
        return await run_xbt_bench(runtime, ctx, "decorator_mode")

    await decorated_bench()


    # 特性 2: 自定义 data_fn — category 级别聚合

    print("\n" + "="*60)
    print("  特性 2: 自定义 data_fn — 按 category 聚合 (top-level)")
    print("="*60)

    orig_data = tele.data_fn
    orig_render = tele.render_fn

    def category_data_fn(df_raw: pl.DataFrame) -> pl.DataFrame:
        """从 name 中提取 category (如 2_Interpreter → Interpreter) 聚合, 保持标准 schema。"""
        return (df_raw.with_columns(
                    pl.col("name").str.extract(r"^\d+_(.+?)/", 1).fill_null("other").alias("name")
                )
                .group_by("name")
                .agg([
                    pl.col("dur_ns").count().alias("cnt"),
                    (pl.col("dur_ns").sum() / 1_000_000.0).round(2).alias("total_ms"),
                    (pl.col("dur_ns").mean() / 1_000.0).round(1).alias("avg_us"),
                ])
                .sort("total_ms", descending=True))

    def category_render_fn(df_out: pl.DataFrame) -> str:
        import tabulate
        return tabulate.tabulate(df_out.to_numpy(), headers=df_out.columns, tablefmt="grid")

    tele.data_fn = category_data_fn
    tele.render_fn = category_render_fn

    with runtime.telemetry("custom_data_fn"):
        await run_xbt_bench(runtime, ctx, "custom_data_fn")

    tele.data_fn = orig_data
    tele.render_fn = orig_render


    # 特性 3: 自定义 render_fn — CSV 格式

    print("\n" + "="*60)
    print("  特性 3: 自定义 render_fn — CSV 格式")
    print("="*60)

    orig_render = tele.render_fn

    def csv_render(df: pl.DataFrame) -> str:
        return df.write_csv()

    tele.render_fn = csv_render

    with runtime.telemetry("csv_render"):
        await run_xbt_bench(runtime, ctx, "csv_render")

    tele.render_fn = orig_render


    # 特性 4: TelemetrySession 手动模式 — to_polars() / dump_json()

    print("\n" + "="*60)
    print("  特性 4: TelemetrySession 手动模式 — to_polars + dump_json")
    print("="*60)

    rt4 = MdioRuntime("/mem")
    ctx4 = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    sess = TelemetrySession(rt4.store, "manual_session", mode="bench")
    with sess:
        t0 = time.perf_counter()
        await rt4.exec('"hello world"' , ctx=ctx4)
        await rt4.exec('"foo" | +> /test_manual.md:: -f', ctx=ctx4)
        await rt4.exec('$/test_manual.md', ctx=ctx4)
        wall_ms = (time.perf_counter() - t0) * 1000

    # to_polars()
    df = sess.to_polars()
    assert df is not None, "to_polars() 返回 None!"
    print(f"  to_polars(): {df.shape[0]} rows, {df.shape[1]} cols")
    print(f"  columns: {df.columns}")
    print(f"  top 3 by dur_ns:\n{df.sort('dur_ns', descending=True).head(3)}")

    # dump_json()
    json_str = sess.dump_json()
    parsed = json.loads(json_str)
    print(f"  dump_json(): {len(parsed)} records")
    assert len(parsed) == df.shape[0], "dump_json 行数与 to_polars 不一致!"

    # dump_json to file
    os.makedirs("/tmp/mdio_tele_test", exist_ok=True)
    sess.dump_json("/tmp/mdio_tele_test/manual.json")
    print(f"  dump_json to file: {os.path.getsize('/tmp/mdio_tele_test/manual.json')} bytes")

    # layer_stats()
    stats = sess.layer_stats()
    print(f"  layer_stats(): {len(stats)} records")

    # report() standalone call with df_raw
    print(f"\n  report(df_raw, wall_ms) standalone:")
    print(report(df, wall_ms))


    # 特性 5: TelemetrySession.report(group_by=...)

    print("\n" + "="*60)
    print("  特性 5: sess.report(group_by='name')")
    print("="*60)
    # 复用上面 sess 的数据 — group_by 只依赖 layer_stats()
    sess.report(group_by="name")


    # 特性 6: module-level span() context manager

    print("\n" + "="*60)
    print("  特性 6: module-level span() context manager")
    print("="*60)

    async def demo_span():
        with span("custom_span/inline_op"):
            await asyncio.sleep(0.001)
        with span("custom_span/nested"):
            with span("custom_span/nested/inner"):
                await asyncio.sleep(0.001)

    with runtime.telemetry("span_demo"):
        await demo_span()
        await runtime.exec('"hello"', ctx=ctx)


    # 特性 7: @trace 带 **meta 元数据

    print("\n" + "="*60)
    print("  特性 7: @trace 带 **meta 元数据列")
    print("="*60)

    @trace("meta_test/with_category", category="custom", priority="high")
    def traced_with_meta(x):
        return x * 2

    @trace("meta_test/no_meta")
    def traced_no_meta(x):
        return x * 3

    rt7 = MdioRuntime("/mem")
    sess_meta = TelemetrySession(rt7.store, "meta_session", mode="bench")
    with sess_meta:
        for i in range(10):
            traced_with_meta(i)
            traced_no_meta(i)

    df_meta = sess_meta.to_polars()
    assert df_meta is not None, "meta session to_polars() 返回 None!"
    print(f"  meta session: {df_meta.shape[0]} rows")
    print(df_meta.head(10))


    # 特性 8: 组合 — 自定义 data_fn + render_fn 同时覆盖

    print("\n" + "="*60)
    print("  特性 8: 组合 — data_fn + render_fn 同时自定义")
    print("="*60)

    def p99_data_fn(df_raw: pl.DataFrame) -> pl.DataFrame:
        return (df_raw.group_by("name")
                .agg([
                    pl.col("dur_ns").count().alias("cnt"),
                    (pl.col("dur_ns").sum() / 1_000_000.0).round(2).alias("total_ms"),
                    (pl.col("dur_ns").mean() / 1_000.0).round(1).alias("avg_us"),
                    (pl.col("dur_ns").quantile(0.99) / 1_000.0).round(1).alias("p99_us"),
                ])
                .sort("p99_us", descending=True))

    def minimal_render(df: pl.DataFrame) -> str:
        lines = [f"  {r['name']}: avg={r['avg_us']}us p99={r['p99_us']}us"
                 for r in df.head(10).iter_rows(named=True)]
        return "\n".join(lines)

    tele.data_fn = p99_data_fn
    tele.render_fn = minimal_render

    with runtime.telemetry("combo_test"):
        await runtime.exec('"combo test payload"', ctx=ctx)
        for _ in range(5):
            await runtime.exec('"x" | +> /combo.md:: -f', ctx=ctx)

    tele.data_fn = tele._builtin_data
    tele.render_fn = tele._builtin_render


    # 总结

    print("\n" + "="*60)
    print("  全部 8 项特性验证通过")
    print("="*60)


if __name__ == "__main__":
    asyncio.run(main())
