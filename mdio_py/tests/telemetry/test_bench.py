"""Telemetry framework validation — benchmarks common MDIO operations.

Covers: bench write, trace read, combined R/W, auto-collect.
Run: uv run python tests/test_telemetry_bench.py
"""

import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime
from mdio_py.core.selector_builder import SelectorBuilder, drill

MD_CONTENT = """# Project
- name: MDIO
- version: 0.8.3

## Design
- language: Rust+Python
- license: MIT

### Deep
- nested: value
- another: thing

## Performance
- target: <50ns probe overhead
- clock: Instant::now()
"""


def _populate(store, vpath: str, content: str):
    store.mk("genesis", vpath, parents=True, verbose=False)
    store.write_locator("genesis", vpath, SelectorBuilder().build(), "/>",
                        content, auto_scaffold=True, god=True)


# ── Bench: write path ───────────────────────────────────────────────────────


async def test_write_bench(runtime: MdioRuntime):
    vp = "/data/bench.md"
    _populate(runtime.store, vp, MD_CONTENT)
    sess = runtime.telemetry_session("bench_write", mode="bench")

    # warmup
    for _ in range(10):
        sb = SelectorBuilder()
        sb.drill("Project").drill("Deep").any_descendant().type_is("bullet")
        runtime.store.write_locator("genesis", vp, sb.build(), "+>", "- warmup")

    # Pattern 1: append under heading
    for _ in range(50):
        with sess.span("append"):
            sb = SelectorBuilder()
            sb.drill("Project").drill("Performance").any_descendant().type_is("bullet")
            runtime.store.write_locator("genesis", vp, sb.build(), "+>", "- bench: ok")

    # Pattern 2: overwrite by matched key
    for _ in range(50):
        with sess.span("overwrite"):
            sb = SelectorBuilder()
            sb.drill("Project").drill("Design").any_descendant().tag_eq("language")
            runtime.store.write_locator("genesis", vp, sb.build(), "/>",
                                        "- language: Rust+Python+WASM")

    # Pattern 3: keyed insert
    for _ in range(50):
        with sess.span("keyed"):
            sb = SelectorBuilder()
            sb.drill("Project").any_descendant().type_is("paragraph")
            runtime.store.write_locator("genesis", vp, sb.build(), "+>", "- note: bench")

    sess.report()


# ── Trace: read path ────────────────────────────────────────────────────────


async def test_read_trace(runtime: MdioRuntime):
    vp = "/data/read.md"
    _populate(runtime.store, vp, MD_CONTENT)
    sess = runtime.telemetry_session("trace_read")

    with sess.span("all"):
        # Direct drill
        with sess.span("drill"):
            sb = SelectorBuilder()
            runtime.store.read_locator("genesis", vp, sb.drill("Project").drill("Design").build())

        # Recursive wildcard
        with sess.span("recursive"):
            runtime.store.read_locator("genesis", vp, drill("Project", "Design"))

        # Recursive + filter
        with sess.span("filter"):
            sb = SelectorBuilder()
            sb.drill("Project").any_descendant().type_is("bullet")
            runtime.store.read_locator("genesis", vp, sb.build())

        # Filter by content value
        with sess.span("content_match"):
            sb = SelectorBuilder()
            sb.drill("Project").any_descendant().content_has("value")
            runtime.store.read_locator("genesis", vp, sb.build(), proj="content")

        # Slice
        with sess.span("slice"):
            sb = SelectorBuilder()
            sb.drill("Project").drill("Design").any_child().slice(0)
            runtime.store.read_locator("genesis", vp, sb.build())

        # Deep 3-level drill
        with sess.span("deep"):
            sb = SelectorBuilder()
            sb.drill("Project").drill("Design").drill("Deep").any_descendant().type_is("bullet")
            runtime.store.read_locator("genesis", vp, sb.build())

    sess.report()


# ── Combined write + read ───────────────────────────────────────────────────


async def test_combined(runtime: MdioRuntime):
    vp = "/data/combined.md"
    _populate(runtime.store, vp, MD_CONTENT)
    sess = runtime.telemetry_session("combined")

    with sess.span("write_then_read"):
        with sess.span("write"):
            sb = SelectorBuilder()
            sb.drill("Project").any_descendant().type_is("bullet")
            runtime.store.write_locator("genesis", vp, sb.build(), "+>", "- combined: test")

        with sess.span("read"):
            sb = SelectorBuilder()
            sb.drill("Project").any_descendant().type_is("bullet")
            runtime.store.read_locator("genesis", vp, sb.build())

    sess.report()


# ── Auto-collect (Level 0) ──────────────────────────────────────────────────


async def test_auto(runtime: MdioRuntime):
    vp = "/data/auto.md"
    _populate(runtime.store, vp, MD_CONTENT)
    runtime.store.write_locator("genesis", vp, SelectorBuilder().build(), "/>",
                                "- language: auto-collected",
                                auto_scaffold=True, god=True)
    runtime.store.read_locator("genesis", vp, SelectorBuilder().build())
    runtime.telemetry_report()


# ── Main ────────────────────────────────────────────────────────────────────


async def main():
    print("═══ MDIO Telemetry Benchmark ═══\n")
    with tempfile.TemporaryDirectory() as d:
        ws = Path(d)
        print("[1/4] Bench: write_locator × 600...")
        await test_write_bench(MdioRuntime(str(ws)))

        print("\n[2/4] Trace: read_locator patterns...")
        await test_read_trace(MdioRuntime(str(ws)))

        print("\n[3/4] Combined: write + read nested...")
        await test_combined(MdioRuntime(str(ws)))

        print("\n[4/4] Auto-collect (telemetry='auto')...")
        await test_auto(MdioRuntime(str(ws), telemetry="auto"))
    print("\n═══ Done ═══")

if __name__ == "__main__":
    asyncio.run(main())
