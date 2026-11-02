"""XBT tag -> LLM driver MVP：NLP 节点交给 driver 产出的 exec 管道执行。"""

import asyncio
import tempfile
import textwrap

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.xbt.parser import parse_xbt
from mdio_py.plugins.xbt.stepper import (
    build_driver_prompt,
    derive_position_path,
    render_candidates,
)


async def get_state(runtime, ctx, tid):
    raw = await runtime.exec(f"$/.mds/proc.md::PROCESS_TABLE.XBT.{tid}", ctx=ctx)
    if not raw or "- mdio_error:" in str(raw):
        return {}
    out = {}
    for line in str(raw).strip().split("\n"):
        if line.startswith("- ") and ":" in line:
            k, v = line[2:].split(":", 1)
            out[k.strip()] = v.strip()
    return out


def install_fake_driver(runtime, action_pipeline):
    original = runtime.exec

    async def fake_exec(code, *args, **kwargs):
        text = str(code)
        if "@LLM.fake_driver" in text:
            if "当前要判断的条件" in text:
                return '"true"'
            if "当前要做的动作" in text:
                # 新语义：XBT 只 exec 导演；导演自己执行它想做的事。
                return str(await original(action_pipeline, *args, **kwargs))
            return "ok"
        return await original(code, *args, **kwargs)

    runtime.exec = fake_exec


async def wait_dead(runtime, ctx, tid, timeout=4.0):
    end = asyncio.get_event_loop().time() + timeout
    while asyncio.get_event_loop().time() < end:
        state = await get_state(runtime, ctx, tid)
        if not state:
            return True
        await asyncio.sleep(0.1)
    return False


async def test_tag_driver_condition_then_cmd_action():
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    install_fake_driver(runtime, '"driven-action" | +> /log.md::ACTIONS')

    await runtime.exec("mk -f /plan.md", ctx=ctx)
    await runtime.exec("mk -f /log.md", ctx=ctx)
    blueprint = textwrap.dedent("""
    ?? [需要执行 A 吗] #fake_driver
        >> [] {{ "A" | +> /log.md::ACTIONS }}
    """).strip()
    await runtime.exec('"# PLAN" | +> /plan.md:: -f', ctx=ctx)
    await runtime.exec(f'"""```xbt\n{blueprint}\n```""" | +> /plan.md::PLAN', ctx=ctx)

    await runtime.exec('@XBT -spawn "tag_cond" -bp "/plan.md::PLAN"', ctx=ctx)
    assert await wait_dead(runtime, ctx, "tag_cond"), "condition driver did not finish"
    log = await runtime.exec("$ /log.md::ACTIONS", ctx=ctx)
    assert "A" in str(log)


async def test_tag_driver_nlp_action():
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    install_fake_driver(runtime, '"driven" | +> /log.md::ACTIONS')

    await runtime.exec("mk -f /plan.md", ctx=ctx)
    await runtime.exec("mk -f /log.md", ctx=ctx)
    blueprint = textwrap.dedent("""
    >> [] #fake_driver 写一句话
    """).strip()
    await runtime.exec('"# PLAN" | +> /plan.md:: -f', ctx=ctx)
    await runtime.exec(f'"""```xbt\n{blueprint}\n```""" | +> /plan.md::PLAN', ctx=ctx)

    await runtime.exec('@XBT -spawn "tag_act" -bp "/plan.md::PLAN"', ctx=ctx)
    assert await wait_dead(runtime, ctx, "tag_act"), "action driver did not finish"
    log = await runtime.exec("$ /log.md::ACTIONS", ctx=ctx)
    assert "driven" in str(log)


async def test_driver_arg_used_without_tag():
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    install_fake_driver(runtime, '"default-driven" | +> /log.md::ACTIONS')

    await runtime.exec("mk -f /plan.md", ctx=ctx)
    await runtime.exec("mk -f /log.md", ctx=ctx)
    blueprint = textwrap.dedent("""
    >> [] 用默认 driver 写一句话
    """).strip()
    await runtime.exec('"# PLAN" | +> /plan.md:: -f', ctx=ctx)
    await runtime.exec(f'"""```xbt\n{blueprint}\n```""" | +> /plan.md::PLAN', ctx=ctx)

    await runtime.exec('@XBT -spawn "arg_act" -bp "/plan.md::PLAN" -driver "fake_driver"', ctx=ctx)
    assert await wait_dead(runtime, ctx, "arg_act"), "arg driver did not finish"
    log = await runtime.exec("$ /log.md::ACTIONS", ctx=ctx)
    assert "default-driven" in str(log)

async def test_tag_scope_judge_then_writer():
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    calls = []
    original = runtime.exec

    async def fake_exec(code, *args, **kwargs):
        text = str(code)
        if "@LLM.judge" in text:
            calls.append(("judge", text))
            return '"true"'
        if "@LLM.writer" in text:
            calls.append(("writer", text))
            return str(await original('"plan-ok" | +> /out.md::PLAN', *args, **kwargs))
        return await original(code, *args, **kwargs)

    runtime.exec = fake_exec
    await runtime.exec("mk -f /plan.md", ctx=ctx)
    await runtime.exec("mk -f /out.md", ctx=ctx)
    blueprint = textwrap.dedent("""
    >> [] #judge
        >> []
            ?? []
                ?? [判断1：高风险吗?] {{ "- risk: high" | +> {VAR} }}
                ?? [] {{ "- risk: low" | +> {VAR} }}
        >> []
            ?? []
                ?? [判断2：可回滚吗?] {{ "- rollback: yes" | +> {VAR} }}
                ?? [] {{ "- rollback: no" | +> {VAR} }}
        >> [] #writer
            >> [] 根据 blackboard 生成执行计划到 /out.md::PLAN
    """).strip()
    await runtime.exec('"# PLAN" | +> /plan.md:: -f', ctx=ctx)
    await runtime.exec(f'"""```xbt\n{blueprint}\n```""" | +> /plan.md::PLAN', ctx=ctx)

    await runtime.exec('@XBT -spawn "scope_test" -bp "/plan.md::PLAN"', ctx=ctx)
    assert await wait_dead(runtime, ctx, "scope_test"), "complex driver test did not finish"
    judge_prompts = [text for (kind, text) in calls if kind == "judge"]
    writer_prompts = [text for (kind, text) in calls if kind == "writer"]
    assert len(judge_prompts) == 2, calls
    assert "判断1" in judge_prompts[0], judge_prompts[0]
    assert "判断2" in judge_prompts[1], judge_prompts[1]
    assert "risk: high" in judge_prompts[1], "judge 第二次调用没看到第一次 CMD 写的黑板"
    assert len(writer_prompts) == 1, calls
    out = await runtime.exec("$ /out.md::PLAN", ctx=ctx)
    assert "plan-ok" in str(out), out

async def test_parallel_reviewer_driver():
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    calls = []
    original = runtime.exec

    async def fake_exec(code, *args, **kwargs):
        text = str(code)
        if "@LLM.reviewer" in text:
            calls.append(text)
            return str(await original('"review-ok" | +> /out.md::REVIEW', *args, **kwargs))
        return await original(code, *args, **kwargs)

    runtime.exec = fake_exec
    await runtime.exec("mk -f /plan.md", ctx=ctx)
    await runtime.exec("mk -f /out.md", ctx=ctx)
    blueprint = textwrap.dedent("""
    >> [] #reviewer
        == []
            >> [] 审查风险
            >> [] 审查成本
        == []
            >> [] 审查合规
    """).strip()
    await runtime.exec('"# PLAN" | +> /plan.md:: -f', ctx=ctx)
    await runtime.exec(f'"""```xbt\n{blueprint}\n```""" | +> /plan.md::PLAN', ctx=ctx)

    await runtime.exec('@XBT -spawn "parallel_reviewer" -bp "/plan.md::PLAN"', ctx=ctx)
    assert await wait_dead(runtime, ctx, "parallel_reviewer"), "parallel reviewer did not finish"
    assert len(calls) >= 2, calls
    out = await runtime.exec("$ /out.md::REVIEW", ctx=ctx)
    assert "review-ok" in str(out), out



# prompt 纯函数（不跑 runtime）


SEL_BP = textwrap.dedent("""
?? [] #drv
    ?? [判断1：高风险吗?] {{ "- risk: high" | +> {VAR} }}
    ?? [] {{ "- risk: low" | +> {VAR} }}
""").strip()

SELF_BP = textwrap.dedent("""
>> [自检通过?] {{ "ok" | +> /t.md::A }}
""").strip()

ACTION_BP = textwrap.dedent("""
>> [] #drv
    >> [] 写一句话
""").strip()


def test_prompt_condition_self_guard():
    ast = parse_xbt(SELF_BP)
    node = ast[0]["children"][0]
    out = build_driver_prompt("t_0", "0.0", node, "condition", "", ast, SELF_BP)
    assert "全部候选" not in out
    assert "当前要判断的条件：\n> 自检通过?" in out
    assert "← 当前" in out
    assert out.rstrip().splitlines()[-1] == "请结合上下文，只返回 true 或 false。"


def test_prompt_condition_selector_child():
    ast = parse_xbt(SEL_BP)
    selector = ast[0]["children"][0]
    child = selector["children"][0]

    node_path, sel, idx = derive_position_path(ast, "0.0", child)
    assert sel is selector and idx == 0 and node_path == "0.0.0"

    out = build_driver_prompt("t_0", "0.0", child, "condition", "", ast, SEL_BP)
    assert "这个 selector 的全部候选（当前问第 0 个）：" in out
    assert "- 0:" in out and "- 1:" in out
    assert '- 0:\n    > [判断1：高风险吗?] → 成立时执行：{{ "- risk: high" | +> {VAR} }}' in out
    assert '- 1:\n    > [] → 成立时执行：{{ "- risk: low" | +> {VAR} }}' in out
    assert "请只针对第 0 个返回 true/false，不要回答整组。" in out

    pos = out.split("```text", 1)[1].split("```", 1)[0]
    assert "0.0" not in pos, pos


def test_prompt_action():
    ast = parse_xbt(ACTION_BP)
    action = ast[0]["children"][0]["children"][0]
    out = build_driver_prompt("t_0", "0.0.0", action, "action", "", ast, ACTION_BP)
    assert "全部候选" not in out
    assert "当前要做的动作：" in out and "> 写一句话" in out
    assert "写一句话   ← 当前" in out
    assert out.rstrip().endswith('请按 action prompt 执行。')


def test_prompt_has_no_known_section():
    ast = parse_xbt(ACTION_BP)
    action = ast[0]["children"][0]["children"][0]
    known = "- task: 发布支付接口 v2\n- risk: high"
    out = build_driver_prompt("scope_0", "0.0.0", action, "action", known, ast, ACTION_BP)
    assert "已知：" not in out
    assert known not in out
    assert "/var/xbt.md" not in out


def test_prompt_has_blackboard_addr():
    ast = parse_xbt(ACTION_BP)
    action = ast[0]["children"][0]["children"][0]
    out = build_driver_prompt("t_0", "0.0.0", action, "action", "", ast, ACTION_BP,
                              "", "/XBT_BlackBoard/t_0.md::BlackBoard")
    assert "黑板：/XBT_BlackBoard/t_0.md::BlackBoard" in out
    assert "/var/xbt.md" not in out


def test_prompt_without_blackboard_addr():
    ast = parse_xbt(ACTION_BP)
    action = ast[0]["children"][0]["children"][0]
    out = build_driver_prompt("t_0", "0.0.0", action, "action", "", ast, ACTION_BP)
    assert "黑板：" not in out
    assert "/var/xbt.md" not in out


def test_prompt_containers():
    ast = parse_xbt(SEL_BP)
    selector = ast[0]["children"][0]
    child = selector["children"][0]
    out = build_driver_prompt("t_0", "0.0", child, "condition", "", ast, SEL_BP)
    assert "```xbt\n" + SEL_BP + "\n```" in out
    assert "```text" in out
    assert render_candidates(selector, 0).startswith("这个 selector 的全部候选")
    assert "node_kind" not in out
    # 无蓝图时不伪造代码块
    no_bp = build_driver_prompt("t_0", "0.0", child, "condition", "", ast, "")
    assert "```xbt" not in no_bp

    # 多行候选按行拆成多条 bq
    multiline_bp = "?? [] #drv\n    ?? [第一行\n第二行] {{ \"- r: h\" | +> {VAR} }}\n    ?? [] {{ \"- r: l\" | +> {VAR} }}"
    ast2 = parse_xbt(multiline_bp)
    cand = render_candidates(ast2[0]["children"][0], 0)
    assert "    > [第一行" in cand
    assert "    > 第二行]" in cand


async def test_blackboard_addr_only_first_driver_prompt():
    runtime = MdioRuntime(tempfile.mkdtemp(prefix="xbt_brief_"))
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    prompts = []
    original = runtime.exec

    async def fake_exec(code, *args, **kwargs):
        text = str(code)
        if "@LLM.fake_driver" in text:
            prompts.append(text)
            if "当前要判断的条件" in text:
                return '"true"'
            return "ok"
        return await original(code, *args, **kwargs)

    runtime.exec = fake_exec
    await runtime.exec("mk -f /plan.md", ctx=ctx)
    await runtime.exec('"# PLAN" | +> /plan.md:: -f', ctx=ctx)
    blueprint = textwrap.dedent("""
    - PLAN:
        >> [] #fake_driver
            >> [] 动作一
            >> [] 动作二
    """).strip()
    fence = chr(96) * 3
    await runtime.exec('"""' + fence + "xbt\n" + blueprint + "\n" + fence + '""" | /> /plan.md::PLAN', ctx=ctx)

    await runtime.exec('@XBT -spawn "brief_test" -bp "/plan.md::PLAN"', ctx=ctx)
    assert await wait_dead(runtime, ctx, "brief_test"), "brief test tree did not finish"
    assert len(prompts) == 2, prompts
    assert "黑板：/XBT_BlackBoard/brief_test.md::BlackBoard" in prompts[0], prompts[0]
    assert "黑板：" not in prompts[1], prompts[1]


