"""§6.9 Textual content — 3 examples (CM spec §6.9).

CM spec rules:
  - Any sequence of characters is valid content
  - Special characters ($, ., ;, ') are literal text
  - Unicode characters are preserved
  - Internal spaces are preserved as-is
  - Inline parsing does not alter character data

AST structure:
  p node: {"nt": "p", "ct": "raw text", "ct": [{"nt": "txt", "ct": "..."}, ...]}
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse

H1, H2 = "Inlines", "Textual content"


def _para_text(ast):
    """Extract raw text from first paragraph(ct 双型:字符串直取 / 数组拼 txt)。"""
    para = ast[0]
    ct = para.get("ct", "")
    if isinstance(ct, str):
        return ct.rstrip("\n")
    return "".join(n.get("ct","") for n in ct if isinstance(n, dict) and n.get("nt") == "txt")


# Tests — §6.9 Textual content (3 examples)

@pytest.mark.asyncio
async def test_tx_00_special_characters_as_literal(cm_workspace):
    """N=0: `hello $.;'there` — special chars preserved as literal text.
    CM spec §2.2: any sequence of characters is valid content."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"
    assert _para_text(ast) == "hello $.;'there"


@pytest.mark.asyncio
async def test_tx_01_unicode_text(cm_workspace):
    """N=1: `Foo χρῆν` — Unicode characters preserved.
    CM spec §2.1: any Unicode code point is a valid character."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"
    assert _para_text(ast) == "Foo χρῆν"


@pytest.mark.asyncio
async def test_tx_02_internal_spaces(cm_workspace):
    """N=2: `Multiple·····spaces` — internal spaces preserved.
    CM spec §2.2: spaces within a line are part of the textual content."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"
    assert _para_text(ast) == "Multiple     spaces"
