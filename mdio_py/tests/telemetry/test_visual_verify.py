"""逐个特性验证报告产出 — 肉眼检查三段式 + 字段对齐。"""
import sys, os, asyncio, time, json, polars as pl

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))

from mdio_py.telemetry import trace, span, TelemetrySession, report
import mdio_py.telemetry as tele

# 模块级装饰器
import mdio_py.core as core_module
import mdio_py.core.optimizer as opt_module
from mdio_py.core.mdio_transformer import MdioTransformer
from lark import Lark
import mdio_py

Lark.parse = trace("1_Compiler/1.Lark (Raw Regex & AST)")(Lark.parse)
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

from mdio_py import MdioRuntime

from mdio_py.core.interpreter import RuntimeContext

async def main():
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)


    # 1. 默认三段式 — 肉眼检查 Section 1/2/3 边界

    print("\n" + "="*60)
    print("  [1] 默认三段式报告 — 检查 Section 1 → 2 → 3 边界")
    print("="*60)

    with runtime.telemetry("verify_default"):
        for _ in range(3):
            await runtime.exec('"hello"')


    # 2. 默认 data_fn + render_fn — 字段检查

    print("\n" + "="*60)
    print("  [2] 默认 data_fn/render_fn — 字段检查")
    print("="*60)
    print(f"  data_fn = {tele.data_fn.__name__}")
    print(f"  render_fn = {tele.render_fn.__name__}")

    sess = TelemetrySession(runtime.store, "verify_fields", mode="bench")
    with sess:
        await runtime.exec('"x" | +> /f.md:: -f')
        await runtime.exec('$/f.md')

    df_raw = sess.to_polars()
    print(f"\n  df_raw columns: {df_raw.columns}")
    print(f"  df_raw shape: {df_raw.shape}")
    print(f"  df_raw sample:\n{df_raw.head(5)}")

    df_out = tele.data_fn(df_raw)
    print(f"\n  df_out (data_fn output) columns: {df_out.columns}")
    print(f"  df_out:\n{df_out}")

    rendered = tele.render_fn(df_out)
    print(f"\n  render_fn output:\n{rendered}")


    # 3. 自定义 data_fn — 不同列名时 report() 的 Section 1 降级

    print("\n" + "="*60)
    print("  [3] 自定义 data_fn + render_fn 配对替换")
    print("="*60)

    orig_data = tele.data_fn
    orig_render = tele.render_fn

    def custom_data(df_raw):
        return (df_raw.with_columns(
                    pl.col("name").str.extract(r"^\d+_(.+?)/", 1).fill_null("other").alias("name")
                )
                .group_by("name")
                .agg([
                    pl.col("dur_ns").count().alias("cnt"),
                    (pl.col("dur_ns").sum() / 1_000_000.0).round(2).alias("total_ms"),
                ])
                .sort("total_ms", descending=True))

    def custom_render(df_out):
        import tabulate
        return tabulate.tabulate(df_out.to_numpy(), headers=df_out.columns, tablefmt="grid")

    tele.data_fn = custom_data
    tele.render_fn = custom_render

    with runtime.telemetry("custom_pair"):
        await runtime.exec('"hello custom pair"')

    tele.data_fn = orig_data
    tele.render_fn = orig_render


    # 4. 自定义 render_fn — 对得上 data_fn 输出列

    print("\n" + "="*60)
    print("  [4] 自定义 render_fn — data_fn 列 → render_fn 消费一致性")
    print("="*60)

    def p99_data(df_raw):
        return (df_raw.group_by("name")
                .agg([
                    pl.col("dur_ns").count().alias("cnt"),
                    (pl.col("dur_ns").sum() / 1_000_000.0).round(2).alias("total_ms"),
                    (pl.col("dur_ns").mean() / 1_000.0).round(1).alias("avg_us"),
                    (pl.col("dur_ns").quantile(0.99) / 1_000.0).round(1).alias("p99_us"),
                ])
                .sort("p99_us", descending=True))

    def p99_render(df):
        lines = []
        for r in df.head(5).iter_rows(named=True):
            lines.append(f"  {r['name']:<50} cnt={r['cnt']:>5}  avg={r['avg_us']:>8}us  p99={r['p99_us']:>8}us")
        return "\n".join(lines)

    tele.data_fn = p99_data
    tele.render_fn = p99_render

    with runtime.telemetry("p99_verify"):
        for _ in range(3):
            await runtime.exec('"p99 test"')

    tele.data_fn = tele._builtin_data
    tele.render_fn = tele._builtin_render


    # 5. to_polars / dump_json / layer_stats 数据源一致性

    print("\n" + "="*60)
    print("  [5] to_polars / dump_json / layer_stats 数据一致性")
    print("="*60)

    sess2 = TelemetrySession(runtime.store, "consistency", mode="bench")
    with sess2:
        await runtime.exec('"data check"')

    df_polars = sess2.to_polars()
    json_str = sess2.dump_json()
    json_parsed = json.loads(json_str)
    layer = sess2.layer_stats()

    print(f"  to_polars rows:  {df_polars.shape[0] if df_polars is not None else 0}")
    print(f"  dump_json items: {len(json_parsed)}")
    print(f"  layer_stats:     {len(layer)} (Rust-internal, 轻量 exec 无 XBT 时为 0)")

    if df_polars is not None and len(json_parsed) > 0:
        assert df_polars.shape[0] == len(json_parsed), \
            f"MISMATCH: to_polars({df_polars.shape[0]}) != dump_json({len(json_parsed)})"
        print(f"  ✓ to_polars 与 dump_json 行数一致: {df_polars.shape[0]}")
    else:
        print(f"  ⚠ 无数据 (空 exec 不会产生很多 span)")


    # 6. TelemetrySession.report() vs TelemetrySession.report(group_by=...)

    print("\n" + "="*60)
    print("  [6] sess.report() vs sess.report(group_by=...)")
    print("="*60)

    sess3 = TelemetrySession(runtime.store, "report_modes", mode="bench")
    with sess3:
        await runtime.exec('"a" | +> /rpt.md:: -f')
        await runtime.exec('"b" | +> /rpt.md:: -f')

    print("\n  --- sess3.report() [默认] ---")
    sess3.report()

    print("\n  --- sess3.report(group_by='name') [按 name 聚合] ---")
    sess3.report(group_by="name")


    # 7. span() context manager — 嵌套 span 是否在报告中出现

    print("\n" + "="*60)
    print("  [7] span() context manager — 嵌套 span 检查")
    print("="*60)

    with runtime.telemetry("span_nested"):
        with span("outer_layer"):
            with span("inner_layer"):
                await asyncio.sleep(0.001)
        # 触发编译器 span
        await runtime.exec('"span check"')


    # 总结

    print("\n" + "="*60)
    print("  验证完毕 — 以上全部报告内容已肉眼检查")
    print("="*60)

if __name__ == "__main__":
    asyncio.run(main())
