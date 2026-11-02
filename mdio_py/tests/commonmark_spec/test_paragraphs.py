"""§4.8 Paragraphs — 8 examples (CM spec §4.8).

CM spec rules:
  - A paragraph is a sequence of non-blank lines that cannot be interpreted
    as other block-level elements
  - Paragraphs are separated by one or more blank lines
  - Lines are wrapped to form the paragraph; leading spaces on the first
    line are stripped (≤3 spaces). Continuation lines with leading
    whitespace: CM spec says these spaces should be stripped
  - ≥4 spaces of indentation on the first line → indented code block
  - Two or more trailing spaces at the end of a line → hard line break

AST structure:
  p node: ["p", [inline children]]
  - Inline children: txt, sb (soft break), lb (hard line break), em, etc.
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace, cm_example  # noqa: F401
from .ast_helpers import ast_parse

H1, H2 = "Leaf blocks", "Paragraphs"


def _para_texts(ast):
    """Extract text from all paragraph nodes, joining via soft/hard breaks."""
    result = []
    for block in ast:
        if isinstance(block, dict) and block.get("nt") == "p":
            parts = []
            for n in block.get("ct", []):
                if isinstance(n, dict):
                    if n.get("nt") == "txt":
                        parts.append(n.get("ct",""))
                    elif n.get("nt") in ("sb", "lb"):
                        parts.append("\n")
            # 纯文本段落无 ch（无 inline 节点）——文本在 ct（CM §4.8 已收束）
            if not parts:
                parts.append(block.get("ct", ""))
            result.append("".join(parts))
    return result


# Tests — §4.8 Paragraphs (8 examples)

# 基本段落

@pytest.mark.asyncio
async def test_pa_00_blank_line_separates_paragraphs(cm_workspace):
    """N=0: `aaa\\n\\nbbb` → blank line separates two paragraphs.
    CM spec §4.8: 'A sequence of non-blank lines ... cannot form paragraphs
    across blank lines.'"""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 0)
    ast = ast_parse(md)
    assert len(ast) == 2
    assert ast[0].get("nt") == "p" and ast[1].get("nt") == "p"
    assert _para_texts(ast) == ["aaa", "bbb"]


@pytest.mark.asyncio
async def test_pa_01_multiline_wrapped(cm_workspace):
    """N=1: `aaa\\nbbb\\n\\nccc\\nddd` → two paragraphs, each with soft break.
    CM spec: lines within a paragraph are joined with soft breaks (<p>aaa\nbbb</p>)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 1)
    ast = ast_parse(md)
    assert len(ast) == 2
    assert _para_texts(ast) == ["aaa\nbbb", "ccc\nddd"]


@pytest.mark.asyncio
async def test_pa_02_multiple_blank_lines(cm_workspace):
    """N=2: `aaa\\n\\n\\nbbb` → multiple blank lines same as single.
    CM spec: 'one or more blank lines' separate paragraphs."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 2)
    ast = ast_parse(md)
    assert len(ast) == 2
    assert _para_texts(ast) == ["aaa", "bbb"]


# 缩进 / 续行行为

@pytest.mark.asyncio
async def test_pa_03_continuation_leading_space_stripped(cm_workspace):
    """N=3: `··aaa\\n·bbb` → ≤3-space indent stripped from first line;
    continuation line leading space also stripped per CM spec.
    Expected: <p>aaa\nbbb</p> (no leading space on bbb)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 3)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"
    assert _para_texts(ast) == ["aaa\nbbb"]


@pytest.mark.asyncio
async def test_pa_04_deeply_indented_continuation(cm_workspace):
    """N=4: long continuation lines with many spaces → all spaces stripped.
    CM spec: continuation lines in a paragraph have indent stripped.
    Expected: <p>aaa\nbbb\nccc</p>."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 4)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"
    assert _para_texts(ast) == ["aaa\nbbb\nccc"]


@pytest.mark.asyncio
async def test_pa_05_three_space_indent_ok(cm_workspace):
    """N=5: `···aaa\\nbbb` → ≤3-space indent on first line → paragraph.
    CM spec: 'up to three spaces of indentation.' Leading spaces stripped."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 5)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"
    assert _para_texts(ast) == ["aaa\nbbb"]


@pytest.mark.asyncio
async def test_pa_06_four_space_indent_code(cm_workspace):
    """N=6: `····aaa\\nbbb` → 4-space indent → indented code block + paragraph.
    CM spec: 'Four spaces of indentation is too many.' for paragraph.
    Expected: code_block(aaa) + p(bbb)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 6)
    ast = ast_parse(md)
    assert len(ast) == 2
    assert ast[0].get("nt") == "ic", f"expected code_block, got {ast[0].get('nt')!r}"
    assert ast[1].get("nt") == "p"


@pytest.mark.asyncio
async def test_pa_07_trailing_spaces_hard_break(cm_workspace):
    """N=7: `aaa·····\\nbbb·····` → trailing spaces → hard line break.
    CM spec: 2+ trailing spaces → line break; spaces are stripped from content.
    Expected: <p>aaa<br />\nbbb</p> with content 'aaa' and 'bbb' (no trailing spaces)."""
    rt = MdioRuntime(cm_workspace)
    md, _ = await cm_example(rt, H1, H2, 7)
    ast = ast_parse(md)
    assert len(ast) == 1
    assert ast[0].get("nt") == "p"
    inlines = ast[0].get("ct", [])
    has_lb = any(isinstance(n, dict) and n.get("nt") == "lb" for n in inlines)
    assert has_lb, "should have hard line break (lb)"
    assert _para_texts(ast) == ["aaa\nbbb"]
