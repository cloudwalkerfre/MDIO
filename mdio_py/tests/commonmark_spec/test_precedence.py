"""§3.1 Precedence — 1 example (CM spec §3.1).

CM spec rules:
  - Container block structure takes precedence over inline parsing
  - A backtick (`` ` ``) that starts a code span cannot cross block-level
    boundaries (list items, block quotes, etc.)
  - In `- \\`one / - two\\``, the backtick before "one" and after "two"
    do NOT form a code span because they are in separate list items

AST structure:
  bullet list: [["bullet"], [["bullet", "-", [inline content]], ...]]
  Each bullet item: ["bullet", "-", [inline children]]
  Note: the example in commonmark.md has no "." separator (format issue),
  so cm_example cannot parse it; we use rt.exec() directly.
"""

import pytest
from mdio_py import MdioRuntime
from .conftest import cm_workspace  # noqa: F401
from .ast_helpers import ast_parse


# Tests — §3.1 Precedence (1 example)

@pytest.mark.asyncio
async def test_pr_00_no_code_span_across_list_items(cm_workspace):
    """N=0: `- \\`one / - two\\`` — backtick is literal, NOT a code span.
    CM spec §4.4: container block structure (list items) takes precedence.
    The `` ` `` before "one" and after "two" are separate, each in its own
    list item → they cannot form a code span that crosses block boundaries."""
    rt = MdioRuntime(cm_workspace)
    r = await rt.exec('$/commonmark.md::.."Precedence"._code[0] -pj content', wt_uid="genesis")
    md, _ = r.split("\n.\n", 1)
    import sys; print(f"\n[DBG] pr_00 r={repr(r[:300])}", file=sys.stderr);
    print(f"[DBG] pr_00 md={repr(md[:200])}", file=sys.stderr);
    ast = ast_parse(md)
    assert len(ast) == 1
    container = ast[0]
    assert container.get("nt") == "bl"
    children = container.get("ct", [])
    assert len(children) == 2
    # item 文本(值 p 化:内容流递归取 txt;ct 双型)。
    def item_text(it):
        out = []
        def _w(x):
            if isinstance(x, dict):
                if x.get("nt") == "txt":
                    out.append(x.get("ct", ""))
                else:
                    c = x.get("ct")
                    if isinstance(c, str):
                        out.append(c)
                    elif isinstance(c, list):
                        for it2 in c: _w(it2)
            elif isinstance(x, list):
                for it2 in x: _w(it2)
        _w(it)
        return "".join(out)
    # item 1: `one（literal backtick, NOT code span）
    item1 = children[0]
    assert item1.get("nt") == "b"
    text1 = item_text(item1)
    assert text1 == "`one"
    # item 2: two`（literal backtick, NOT code span）
    item2 = children[1]
    assert item2.get("nt") == "b"
    text2 = item_text(item2)
    assert text2 == "two`"
    # Verify no code span was created
    for item in children:
        inlines = item.get("ct", [])
        assert not any(isinstance(n, dict) and n.get("nt") == "cd" for n in inlines)
