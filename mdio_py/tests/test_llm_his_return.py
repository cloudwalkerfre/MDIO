"""主线 → 分支 b1 → 嵌套 b2 → 回主线 be 的 HIS 回归。"""

from pathlib import Path

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.mdio_ir import SelectorIR
from mdio_py.plugins.llm import arrow_handoff

FIXTURE = Path(__file__).parent / "fixtures" / "his_main_branch_return.txt"


def _ctx():
    return RuntimeContext(wt_uid="genesis")


async def _his(rt):
    text = FIXTURE.read_text()
    rt._workspace.write_locator("genesis", "/wl.md", SelectorIR(file_path=None, steps=[]), "/>", text, auto_scaffold=True)
    return str(await rt.exec("$ /wl.md::HIS -pj raw", wt_uid="genesis"))


async def _contents(rt, wl):
    his = await _his(rt)
    msgs = await arrow_handoff.handoff(his, wl, rt._workspace, _ctx())
    return [m.get("content") for m in msgs]


BASELINE = [
    "主线：开题",
    "主线：开题回复",
    "主线：分叉点 b1",
    "主线：分支后继续",
    "主线：回到主线 be",
    "主线：be 之后",
    "主线：be 后的普通消息",
]

BE = [
    "主线：开题",
    "主线：开题回复",
    "主线：分叉点 b1",
    "主线：分支后继续",
    "分支 b1：入口",
    "分支 b1：内容",
    "分支 b1：分叉点 b2",
    "分支 b1：b2 之后",
    "主线：be 之后",
    "主线：be 后的普通消息",
]

B1 = [
    "主线：开题",
    "主线：开题回复",
    "分支 b1：入口",
    "分支 b1：内容",
    "分支 b1：分叉点 b2",
    "分支 b1：b2 之后",
]


async def test_main_baseline(runtime_llm):
    assert await _contents(runtime_llm, "") == BASELINE


async def test_branch_returns_at_be(runtime_llm):
    assert await _contents(runtime_llm, "be") == BE


async def test_branch_b1_stays_diverged(runtime_llm):
    assert await _contents(runtime_llm, "b1") == B1
