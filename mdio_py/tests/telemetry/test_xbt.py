"""XBT Stepper benchmark — compare manual time.perf_counter() vs telemetry framework.

同一负载分别用 Python time.perf_counter() 与 telemetry session 计时，对比两种口径：

- 手写计时：每阶段 count + 总墙钟（Python 时钟，无嵌套）
- telemetry：每阶段 min/avg/p50/p99/σ + 树形嵌套（Rust 时钟，自动采集）

Run:
  uv run python mdio_py/tests/telemetry/test_xbt.py
"""

import asyncio
import time
import uvloop
from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext


async def test_xbt_with_telemetry():
    print("\n═══ XBT Stepper Benchmark (Telemetry) ═══\n")

    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)

    await runtime.exec("mk -f /fake_meta.md", ctx=ctx)
    await runtime.exec("mk -f /log.md", ctx=ctx)
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

    my = "/fake_meta.md::TEST"
    await runtime.exec(f'"# TEST" | +> /fake_meta.md:: -f', ctx=ctx)
    fenced = f"```xbt\n{blueprint_script}\n```"
    await runtime.exec(f'"""{fenced}""" | +> {my}', ctx=ctx)

    stop_event = asyncio.Event()
    tid = "tid_telemetry"

    # ── Telemetry: bench mode for the full stepper run ──
    sess = runtime.telemetry_session("xbt_stepper", mode="bench")
    N = 5

    for i in range(N):
        with sess.span("xbt_full_run", round=i):
            # Reset proc state
            await runtime.exec(f'"LAUNCHED" | /> /.mds/proc.md::PROCESS_TABLE.XBT.{tid}.status',
                               ctx=ctx, wt_uid="genesis")
            await runtime.exec(f'"""{my}""" | /> /.mds/proc.md::PROCESS_TABLE.XBT.{tid}.blueprint',
                               ctx=ctx, wt_uid="genesis")

            # Step through XBT states
            max_steps = 15
            for step_idx in range(max_steps):
                with sess.span("xbt_step", step=step_idx):
                    try:
                        raw = await runtime.exec(
                            f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid} | xbt step",
                            ctx=ctx, wt_uid="genesis")
                        if "mdio_error" in str(raw):
                            break
                        status = await runtime.exec(
                            f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}.status",
                            ctx=ctx, wt_uid="genesis")
                        status_str = status[0].strip() if status else ""
                        if status_str in ("DONE", "FAILED"):
                            break
                    except Exception:
                        break

    sess.report()
    stop_event.set()
    print("\n═══ Done — compare with test_xbt_stepper_analysis.py ═══")
    print("Old: time.perf_counter() per function, flat counts")
    print("New: telemetry bench mode, nested per-phase stats with p50/p99/σ")


if __name__ == "__main__":
    asyncio.run(test_xbt_with_telemetry())
