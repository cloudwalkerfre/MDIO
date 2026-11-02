"""§4.9 Blank lines — 1 example (CM spec §4.9).

CM spec rules:
  - Blank lines (empty or containing only spaces/tabs) have no effect
    on the parsed document except to separate block-level elements
  - Blank lines before/after/between elements are ignored
  - They do not become empty paragraphs or other visible content

AST structure:
  Blank lines themselves do not appear in the AST. They only serve as
  separators between block-level elements (paragraphs, headings, etc.).
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse

H1, H2 = "Leaf blocks", "Blank lines"


# Tests — §4.9 Blank lines (1 example)

@pytest.mark.asyncio
async def test_bl_00_blank_lines_ignored(cm_workspace):
    """N=0: leading/trailing blank lines ignored → p(aaa) + h1(aaa).
    CM spec §4.9: blank lines between elements have no effect other than
    separating blocks. Leading and trailing blank lines are stripped."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert len(ast) == 2
    # block 1: paragraph "aaa"
    assert ast[0].get("nt") == "p"
    txt = ast[0].get("ct","")
    assert txt.strip() == "aaa"
    # block 2: h1 "aaa"
    assert ast[1].get("nt") == "h"
    assert ast[1].get("lv") == 1
    assert "aaa" in ast[1].get("t","") or "aaa" in ast[1].get("ct","")
