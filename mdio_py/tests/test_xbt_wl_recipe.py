"""driver 类型配方 + 世界线接线回归（XBT 侧）。

覆盖：== 每孩子一条世界线且真并发、HIS 段隔离、>> 继承 token、跨类型叶子降级、
未登记类型不建线、open 失败不挂死、裸 tag 默认 LLM 兼容、跨类型 fork 直接 FAILED。
"""

import asyncio
import re
import time

import pytest

from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.llm.llm import LLMPlugin
from mdio_py.plugins.llm import arrow_handoff


TASK_RE = re.compile("当前要做的动作：" + chr(10) + "> (.+?)" + chr(10))
TOKEN_RE = re.compile('-wl "([^"]+)" -branch')
TOKEN_FORMAT = re.compile(r"b[0-9a-f]{8}_[01]")


def _ctx():
    return RuntimeContext(wt_uid="genesis")


def install_exec(rt, commands):
    """拦截 @FAKE.* 假插件与 @LLM.fake；其余走真实 runtime，并记录 driver 命令。"""
    original = rt.exec

    async def fake(code, *args, **kwargs):
        text = str(code)
        if "@LLM." in text or "@FAKE." in text:
            commands.append(text)
        if "@FAKE." in text:
            if "当前要判断的条件" in text:
                return "true"
            match = TASK_RE.search(text)
            task = match.group(1).strip() if match else "?"
            return '"- fake:' + task + '" | +> /log.md::ACTIONS'
        if "| @LLM.fake" in text:
            return '"- fake:bare" | +> /log.md::ACTIONS'
        return await original(code, *args, **kwargs)

    rt.exec = fake
    return original


def install_fake_model(monkeypatch, calls):
    async def fake_secrets(self, ws_, alias):
        return {"model": "fake", "api_key": "x", "backend": "fake"}

    async def fake_infer(self, messages, secrets, is_stream, tools=None, num_retries=1):
        text = str(messages[-1].get("content") or "")
        match = TASK_RE.search(text)
        task = match.group(1).strip() if match else "?"
        calls.append(("start", task, time.time()))
        await asyncio.sleep(0.05)
        calls.append(("end", task, time.time()))
        yield ("content", '"- ans:' + task + '" | +> /log.md::ACTIONS')

    monkeypatch.setattr(LLMPlugin, "_get_god_secrets", fake_secrets)
    monkeypatch.setattr(LLMPlugin, "_infer_and_stream", fake_infer)


async def init_agent(rt):
    plugin = LLMPlugin()
    result = await plugin(rt._workspace, "", ["analyst", "init"], _ctx(),
                          {"-m": "FAKE", "-tool": "false"})
    assert "initialized" in str(result), result


async def run_tree(rt, name, bp, timeout=12.0):
    """写蓝图、spawn、等到线程死亡；返回 (tid, dead, wall)。"""
    tid = "wl_" + name
    fence = chr(96) * 3
    await rt.exec("mk -f /log.md", wt_uid="genesis")
    await rt.exec("mk -f /plan_" + name + ".md", wt_uid="genesis")
    await rt.exec('"# PLAN" | +> /plan_' + name + '.md:: -f', wt_uid="genesis")
    await rt.exec('"""' + fence + "xbt" + chr(10) + bp.strip() + chr(10) + fence
                  + '""" | +> /plan_' + name + '.md::PLAN', wt_uid="genesis")
    started = time.time()
    await rt.exec('@XBT -spawn "' + tid + '" -bp "/plan_' + name + '.md::PLAN"', ctx=_ctx())
    while time.time() - started < timeout:
        raw = await rt.exec("$/.mds/proc.md::PROCESS_TABLE.XBT." + tid, wt_uid="genesis")
        if not raw or "- mdio_error:" in str(raw):
            return tid, True, time.time() - started
        await asyncio.sleep(0.02)
    return tid, False, time.time() - started


async def read(rt, code):
    return str(await rt.exec(code, wt_uid="genesis"))


async def proc_table(rt):
    return (await read(rt, "$/.mds/proc.md::PROCESS_TABLE.XBT")).strip()


def open_tokens(commands):
    return [TOKEN_RE.search(command).group(1) for command in commands if "-branch" in command]


@pytest.mark.slow
async def test_llm_parallel_children_get_own_worldline(runtime_llm, monkeypatch):
    rt = runtime_llm
    await init_agent(rt)
    calls, commands = [], []
    install_fake_model(monkeypatch, calls)
    install_exec(rt, commands)

    bp = """- PLAN:
    >> [] #LLM.analyst 主线程动作
    >> [] #LLM.analyst
        == [] #LLM.analyst 并发A
        == [] #LLM.analyst 并发B
"""
    _, dead, wall = await run_tree(rt, "llmpar", bp)
    assert dead, f"并行树未结束（wall={wall:.2f}s），可能 busy 或挂死"

    opens = [c for c in commands if "-branch" in c]
    assert len(opens) == 2, opens
    tokens = open_tokens(commands)
    assert len(set(tokens)) == 2 and all(TOKEN_FORMAT.fullmatch(t) for t in tokens), tokens

    invokes = [c for c in commands if "@LLM." in c and "-branch" not in c]
    assert any('-wl "main"' in c for c in invokes), invokes
    for token in tokens:
        assert any('-wl "' + token + '"' in c for c in invokes), (token, invokes)

    assert (await proc_table(rt)) == "## XBT"

    starts = {task: t for kind, task, t in calls if kind == "start"}
    ends = {task: t for kind, task, t in calls if kind == "end"}
    assert set(starts) >= {"并发A", "并发B"} and set(ends) >= {"并发A", "并发B"}, calls
    assert max(starts["并发A"], starts["并发B"]) < min(ends["并发A"], ends["并发B"]), calls

    his = await read(rt, "$ /.mds/llm.md::AGENTS.analyst.HIS -pj raw")
    assert his.count("当前要做的动作：") == 3, his[:500]
    assert his.count("+ b") == 2, his[:400]
    for task in ("并发A", "并发B"):
        msgs = await arrow_handoff.handoff(his, tokens[0 if task == "并发A" else 1], rt._workspace, _ctx())
        contents = [str(m.get("content")) for m in msgs]
        assert any("当前要做的动作：" + chr(10) + "> " + task in c for c in contents), (task, contents)
        assert contents[-1].endswith('ans:' + task + '" | +> /log.md::ACTIONS'), (task, contents[-1])


async def test_branch_thread_inherits_token_and_cross_type_leaf(runtime_llm, monkeypatch, capsys):
    rt = runtime_llm
    await init_agent(rt)
    calls, commands = [], []
    install_fake_model(monkeypatch, calls)
    install_exec(rt, commands)

    bp = """- PLAN:
    >> [] #LLM.analyst
        == [] #LLM.analyst 并发A
        == []
            >> [] #LLM.analyst 内层
            >> [] #FAKE.ex 换类型叶子
"""
    _, dead, wall = await run_tree(rt, "inherit", bp)
    assert dead, f"树未结束（wall={wall:.2f}s）"

    tokens = open_tokens(commands)
    assert len(tokens) == 2, commands
    nested_token = tokens[1]
    inner = [c for c in commands if ("当前要做的动作：\\n> 内层") in c and "-branch" not in c]
    assert inner and '-wl "' + nested_token + '"' in inner[0], (nested_token, inner[:1])

    fake_leaf = [c for c in commands if "换类型叶子" in c and "@FAKE." in c]
    assert fake_leaf and all("-wl" not in c for c in fake_leaf), fake_leaf
    captured = capsys.readouterr()
    assert "不属于世界线介质" in captured.out, captured.out[-500:]

    his = await read(rt, "$ /.mds/llm.md::AGENTS.analyst.HIS -pj raw")
    assert "ans:内层" in his, his[:500]
    assert (await proc_table(rt)) == "## XBT"


async def test_unknown_type_creates_no_worldline(runtime_llm):
    rt = runtime_llm
    commands = []
    install_exec(rt, commands)

    bp = """- PLAN:
    >> [] #FAKE.ex
        == [] #FAKE.ex 并发X
        == [] #FAKE.ex 并发Y
"""
    _, dead, wall = await run_tree(rt, "unknown", bp)
    assert dead, f"树未结束（wall={wall:.2f}s）"
    assert [c for c in commands if "-branch" in c] == []
    fake_invokes = [c for c in commands if "@FAKE." in c]
    assert len(fake_invokes) == 2, fake_invokes
    assert all("-wl" not in c for c in fake_invokes), fake_invokes
    assert (await proc_table(rt)) == "## XBT"


async def test_open_failure_fails_tree_without_hang(runtime_llm, capsys):
    rt = runtime_llm
    commands = []
    install_exec(rt, commands)

    bp = """- PLAN:
    >> [] #LLM.nobody
        == [] #LLM.nobody 并发A
        == [] #LLM.nobody 并发B
"""
    _, dead, wall = await run_tree(rt, "openfail", bp, timeout=8.0)
    assert dead, f"open 失败后树未在 8s 内结束（wall={wall:.2f}s）"
    captured = capsys.readouterr()
    assert "open 世界线失败" in captured.out, captured.out[-500:]
    assert (await proc_table(rt)) == "## XBT"


async def test_bare_tag_defaults_to_llm(runtime_llm):
    rt = runtime_llm
    commands = []
    install_exec(rt, commands)

    bp = """- PLAN:
    >> [] #fake 动作X
"""
    _, dead, wall = await run_tree(rt, "bare", bp)
    assert dead, f"树未结束（wall={wall:.2f}s）"
    assert any('| @LLM.fake -wl "main"' in c for c in commands), commands


async def test_cross_type_fork_child_fails(runtime_llm, monkeypatch, capsys):
    rt = runtime_llm
    await init_agent(rt)
    calls, commands = [], []
    install_fake_model(monkeypatch, calls)
    install_exec(rt, commands)

    bp = """- PLAN:
    >> [] #LLM.analyst
        == [] #LLM.analyst 并发A
        == []
            == [] #FAKE.ex 跨类型fork
"""
    _, dead, wall = await run_tree(rt, "xfork", bp, timeout=8.0)
    assert dead, f"跨类型 fork 后树未结束（wall={wall:.2f}s）"
    captured = capsys.readouterr()
    assert "跨世界线介质" in captured.out, captured.out[-500:]
    assert (await proc_table(rt)) == "## XBT"



# 单元：地址解析与配方命令逐字匹配


def test_driver_address_and_recipe_formatting():
    from mdio_py.plugins.xbt.driver_types import (
        parse_address, format_invoke, format_open, recipe_for,
    )

    assert parse_address("analyst") == ("LLM", "analyst", "LLM")
    assert parse_address("LLM.analyst") == ("LLM", "analyst", "LLM")
    assert parse_address("FAKE.ex") == ("FAKE", "ex", "FAKE")
    assert parse_address("a.b.c") == ("a", "b.c", "a")

    assert format_invoke("LLM.analyst", "main") == '@LLM.analyst -wl "main"'
    assert format_invoke("LLM.analyst", None) == "@LLM.analyst"
    assert format_open("LLM.analyst", "b12345678_0") == '@LLM.analyst -wl "b12345678_0" -branch'

    assert recipe_for("FAKE.ex")[0] == "FAKE"
    assert recipe_for("FAKE.ex")[1]["worldline"] is False
    assert format_invoke("FAKE.ex", "x") == "@FAKE.ex"
    assert format_open("FAKE.ex", "b1") is None
