"""Analysis: read vs write path comparison."""
import asyncio
import pytest
from mdio_py.core.selector_builder import SelectorBuilder

DOC = """# Project
- name: MDIO
- version: 0.8.3

## Design
- language: Rust
- license: MIT

### Deep
- nested: value
"""


async def test_read_baseline(runtime, tel_session):
    """Read path: drill + wildcard + filter, 200 rounds."""
    if tel_session is None: return
    vp = "/rw/read.md"
    runtime.store.mk("genesis", vp, True, False)
    runtime.store.write_locator("genesis", vp, SelectorBuilder().build(), "/>", DOC, True, True, False, False)

    for _ in range(200):
        sb = SelectorBuilder()
        sb.drill("Project").drill("Design").any_descendant().type_is("bullet")
        runtime.store.read_locator("genesis", vp, sb.build())

async def test_write_baseline(runtime, tel_session):
    """Write path: same selector, append, 200 rounds."""
    if tel_session is None: return
    vp = "/rw/write.md"
    runtime.store.mk("genesis", vp, True, False)
    runtime.store.write_locator("genesis", vp, SelectorBuilder().build(), "/>", DOC, True, True, False, False)

    for _ in range(200):
        sb = SelectorBuilder()
        sb.drill("Project").drill("Design").any_descendant().type_is("bullet")
        runtime.store.write_locator("genesis", vp, sb.build(), "+>", "- bench: ok")


async def test_read_container(runtime, tel_session):
    """Read path: Container projection cost."""
    if tel_session is None: return
    vp = "/rw/container.md"
    runtime.store.mk("genesis", vp, True, False)
    runtime.store.write_locator("genesis", vp, SelectorBuilder().build(), "/>", DOC, True, True, False, False)

    for _ in range(200):
        sb = SelectorBuilder()
        sb.drill("Project").drill("Design").drill("Deep")
        runtime.store.read_locator("genesis", vp, sb.build(), proj="container")

async def test_read_content(runtime, tel_session):
    """Read path: Content projection cost (baseline for Container)."""
    if tel_session is None: return
    vp = "/rw/content.md"
    runtime.store.mk("genesis", vp, True, False)
    runtime.store.write_locator("genesis", vp, SelectorBuilder().build(), "/>", DOC, True, True, False, False)

    for _ in range(200):
        sb = SelectorBuilder()
        sb.drill("Project").drill("Design").drill("Deep")
        runtime.store.read_locator("genesis", vp, sb.build(), proj="content")
