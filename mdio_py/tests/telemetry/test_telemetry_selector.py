"""Telemetry: selector pattern precision benchmarks.

Tests each selector step type (drill, wildcard, recursive, filter, slice, compound)
independently to verify the framework captures accurate per-phase timing.

Run:
  uv run python tests/test_telemetry_selector.py
"""

import asyncio, tempfile
from pathlib import Path
from mdio_py import MdioRuntime
from mdio_py.core.selector_builder import SelectorBuilder

# Rich markdown document with varied structures
MD = """# Products
- name: Alpha
- price: 99
- tags: new

## Electronics
- name: Phone
- price: 599
- stock: 50
- tags: popular

### Mobile
- name: Phone Mini
- price: 399
- stock: 200
- tags: bestseller

### Laptop
- name: ProBook
- price: 1299
- stock: 15
- tags: premium

## Books
- name: Rust Guide
- price: 49
- stock: 500
- tags: educational

### Fiction
- name: The Story
- price: 19
- stock: 1000

## Misc
- note: free shipping over 100
- status: active
"""


def _populate(store, vp, content):
    store.mk("genesis", vp, parents=True, verbose=False)
    store.write_locator("genesis", vp, SelectorBuilder().build(), "/>",
                        content, auto_scaffold=True, god=True)


async def bench_selector_patterns(runtime: MdioRuntime):
    """Benchmark each selector step type independently."""
    vp = "/data/sel.md"
    _populate(runtime.store, vp, MD)

    patterns = {
        # ── Navigation ──
        "drill_2level": lambda: SelectorBuilder().drill("Products").drill("Electronics").build(),
        "drill_3level": lambda: SelectorBuilder().drill("Products").drill("Electronics")
            .drill("Mobile").build(),
        "wildcard_direct": lambda: SelectorBuilder().drill("Products").drill("Electronics")
            .any_child().build(),
        "recursive_wildcard": lambda: SelectorBuilder().drill("Products")
            .any_descendant().build(),
        "recursive_drill": lambda: SelectorBuilder().drill("Products")
            .recursive_drill("Mobile").build(),

        # ── Type filters ──
        "type_bullet": lambda: SelectorBuilder().drill("Products")
            .any_descendant().type_is("bullet").build(),
        "type_h2": lambda: SelectorBuilder().drill("Products")
            .any_descendant().type_is("h2").build(),
        "type_h3": lambda: SelectorBuilder().drill("Products")
            .any_descendant().type_is("h3").build(),

        # ── Property filters ──
        "tag_eq": lambda: SelectorBuilder().drill("Products")
            .any_descendant().tag_eq("name").build(),
        "tag_ne": lambda: SelectorBuilder().drill("Products")
            .any_descendant().tag_ne("name").build(),
        "content_has": lambda: SelectorBuilder().drill("Products")
            .any_descendant().content_has("price").build(),

        # ── Compound ──
        "and_filter": lambda: (SelectorBuilder().drill("Products")
            .any_descendant().type_is("bullet").tag_eq("name").build()),

        # ── Slice ──
        "slice_index0": lambda: SelectorBuilder().drill("Products").drill("Electronics")
            .any_child().slice(0).build(),
        "slice_range": lambda: SelectorBuilder().drill("Products").drill("Electronics")
            .any_child().slice(0, 2).build(),
        "slice_neg": lambda: SelectorBuilder().drill("Products").drill("Electronics")
            .any_child().slice(-1).build(),
    }

    for name, factory in patterns.items():
        sess = runtime.telemetry_session(f"sel_{name}", mode="bench")
        try:
            selector = factory()
            for _ in range(10):
                with sess.span(name):
                    runtime.store.read_locator("genesis", vp, selector)
            sess.report()
        except Exception as e:
            print(f"  {name}: SKIP ({e})")


async def bench_write_patterns(runtime: MdioRuntime):
    """Benchmark write patterns with different selector complexity."""
    vp = "/data/write_pat.md"
    _populate(runtime.store, vp, MD)

    writes = {
        "write_simple_drill": lambda: (
            SelectorBuilder().drill("Products").drill("Misc")
                .any_descendant().type_is("bullet").build(),
            "+>", "- simple: ok"),

        "write_overwrite": lambda: (
            SelectorBuilder().drill("Products").drill("Electronics")
                .any_descendant().tag_eq("name").build(),
            "/>", "- name: SuperPhone"),

        "write_deep_recursive": lambda: (
            SelectorBuilder().drill("Products").recursive_drill("Mobile")
                .any_descendant().type_is("bullet").build(),
            "+>", "- deep: insert"),

        "write_filtered": lambda: (
            SelectorBuilder().drill("Products").any_descendant()
                .type_is("bullet").content_has("99").build(),
            "/>", "- price: 109"),

        "write_compound": lambda: (
            SelectorBuilder().drill("Products").drill("Books")
                .any_descendant().filter_and(
                    ["TYPECHECK", "bullet"],
                    ["CMP", ".tag", "==", "name"]
                ).build(),
            "+>", "- name: New Book"),
    }

    for name, factory in writes.items():
        sess = runtime.telemetry_session(f"w_{name}", mode="bench")
        try:
            sel, op, text = factory()
            for _ in range(10):
                with sess.span(name):
                    runtime.store.write_locator("genesis", vp, sel, op, text)
            sess.report()
        except Exception as e:
            print(f"  {name}: SKIP ({e})")


async def main():
    print("═══ Selector Precision Benchmarks ═══\n")
    with tempfile.TemporaryDirectory() as d:
        ws = Path(d)
        r = MdioRuntime(str(ws))
        print("── Read patterns ──")
        await bench_selector_patterns(r)
        print("\n── Write patterns ──")
        await bench_write_patterns(r)
    print("\n═══ Done ═══")

if __name__ == "__main__":
    asyncio.run(main())
