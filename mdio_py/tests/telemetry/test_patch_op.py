"""Analysis: ~> patch operator cost breakdown."""
import asyncio
import pytest
from mdio_py.core.selector_builder import SelectorBuilder

DOC = """# Config
- host: localhost
- port: 8080
- debug: false
- log_level: info
- max_conn: 100
- timeout: 30
"""


async def test_append_baseline(runtime, tel_session):
    """+> (append) — baseline for edit op comparison. 100 rounds."""
    if tel_session is None: return
    vp = "/patch/append.md"
    runtime.store.mk("genesis", vp, True, False)
    runtime.store.write_locator("genesis", vp, SelectorBuilder().build(), "/>", DOC, True, True, False, False)

    for _ in range(100):
        sb = SelectorBuilder()
        sb.drill("Config").any_descendant().tag_eq("host")
        runtime.store.write_locator("genesis", vp, sb.build(), "+>", "- host: 127.0.0.1\n")

async def test_patch_small(runtime, tel_session):
    """~> (patch) — single JSON patch op. 100 rounds."""
    if tel_session is None: return
    vp = "/patch/small.md"
    runtime.store.mk("genesis", vp, True, False)
    runtime.store.write_locator("genesis", vp, SelectorBuilder().build(), "/>", DOC, True, True, False, False)

    for _ in range(100):
        sb = SelectorBuilder()
        sb.drill("Config").any_descendant().tag_eq("host")
        runtime.store.write_locator("genesis", vp, sb.build(), "~>",
                                    '[{"op":"replace","old":"localhost","new":"127.0.0.1"}]')

async def test_overwrite_baseline(runtime, tel_session):
    """/> (overwrite) — baseline. 100 rounds."""
    if tel_session is None: return
    vp = "/patch/overwrite.md"
    runtime.store.mk("genesis", vp, True, False)
    runtime.store.write_locator("genesis", vp, SelectorBuilder().build(), "/>", DOC, True, True, False, False)

    for _ in range(100):
        sb = SelectorBuilder()
        sb.drill("Config").any_descendant().tag_eq("host")
        runtime.store.write_locator("genesis", vp, sb.build(), "/>", "- host: 127.0.0.1\n")
