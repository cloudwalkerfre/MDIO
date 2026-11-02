"""XBT driver 的 NLP 端到端测试：整棵树只有裸 NLP 节点（无 CMD）。

与 test_xbt_driver.py 的 fake 测试不同：
  - 蓝图里没有 {{...}}，selector / guard / action 全部是自然语言，走 _driver_pipeline 真路径；
  - 假 driver 读真实 prompt 再决定答案（等于校验 prompt 上下文够不够用），不是按识别串回固定值；
  - 断言最终产物 + prompt 契约（候选段只在 selector 子条件出现、已知累积、ask 位置）。
"""

import asyncio
import json
import re
import textwrap

from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext


NLP_TREE = textwrap.dedent("""
- PLAN:
    >> [] #analyst
        >> []
            ?? []
                ?? [变更涉及资金或退款吗?] 写高风险发布计划到 /out.md::PLAN
                ?? [] 写常规发布计划到 /out.md::PLAN
        >> [需要人工审批吗?]
            >> [] 写审批单到 /approval.md::REQ
        >> []
            == []
                >> [] 写回滚预案到 /out.md::ROLLBACK
                >> [] 写监控清单到 /out.md::MONITOR
        >> [] 根据黑板写执行摘要到 /out.md::SUMMARY
""").strip()

ACTION_PIPES = {
    "写高风险发布计划到 /out.md::PLAN":
        '"- risk: high" | +> /var/xbt.md::scope ; "## 高风险发布计划" | +> /out.md::PLAN',
    "写常规发布计划到 /out.md::PLAN":
        '"## 常规发布计划" | +> /out.md::PLAN',
    "写审批单到 /approval.md::REQ":
        '"- 审批: 高风险发布需双人审批" | +> /approval.md::REQ',
    "写回滚预案到 /out.md::ROLLBACK":
        '"- rollback: ready" | +> /var/xbt.md::scope ; "- 回滚: 关闭开关切回 v1" | +> /out.md::ROLLBACK',
    "写监控清单到 /out.md::MONITOR":
        '"- monitor: on" | +> /var/xbt.md::scope ; "- 监控: 支付成功率 / 错误率" | +> /out.md::MONITOR',
    "根据黑板写执行摘要到 /out.md::SUMMARY":
        '"- 摘要: 见黑板 key" | +> /out.md::SUMMARY',
}


def _cond_text(prompt: str) -> str:
    m = re.search(r"当前要判断的条件：\n> (.+?)\n", prompt)
    return m.group(1).strip() if m else ""


def _action_text(prompt: str) -> str:
    m = re.search(r"当前要做的动作：\n> (.+?)\n", prompt)
    return m.group(1).strip() if m else ""


def install_nlp_driver(runtime, decisions, calls):
    original = runtime.exec

    async def fake_exec(code, *args, **kwargs):
        text = str(code)
        if "@LLM.analyst" in text:
            if "-branch" in text:
                return json.dumps({"wl": "child", "ok": True})
            prompt = json.loads(text.split(" | @LLM.")[0])
            calls.append(prompt)
            if "当前要判断的条件：" in prompt:
                return json.dumps("true" if decisions.get(_cond_text(prompt)) else "false")
            pipe = ACTION_PIPES.get(_action_text(prompt))
            assert pipe, "unknown action: %r" % _action_text(prompt)
            # 新语义：XBT 只 exec 导演；导演（这里用假 driver 模拟）自己执行。
            return str(await original(pipe, *args, **kwargs))
        return await original(code, *args, **kwargs)

    runtime.exec = fake_exec


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


async def wait_dead(runtime, ctx, tid, timeout=15.0):
    end = asyncio.get_event_loop().time() + timeout
    while asyncio.get_event_loop().time() < end:
        if not await get_state(runtime, ctx, tid):
            return True
        await asyncio.sleep(0.05)
    return False


async def run_nlp_tree(decisions):
    runtime = MdioRuntime("/mem")
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    for f in ("/plan.md", "/out.md", "/approval.md", "/var/xbt.md"):
        await runtime.exec(f"mk -f {f}", ctx=ctx)
    await runtime.exec('"- task: 发布支付接口 v2" | +> /var/xbt.md::scope', ctx=ctx)
    await runtime.exec('"# PLAN" | +> /plan.md:: -f', ctx=ctx)
    await runtime.exec(f'"""```xbt\n{NLP_TREE}\n```""" | +> /plan.md::PLAN', ctx=ctx)

    calls = []
    install_nlp_driver(runtime, decisions, calls)
    await runtime.exec('@XBT -spawn "scope_test" -bp "/plan.md::PLAN"', ctx=ctx)
    assert await wait_dead(runtime, ctx, "scope_test"), "NLP tree did not finish"
    return runtime, ctx, calls


def assert_prompt_contract(calls):
    cond_prompts = [p for p in calls if "当前要判断的条件：" in p]
    act_prompts = [p for p in calls if "当前要做的动作：" in p]
    candidate_prompts = [p for p in calls if "这个 selector 的全部候选" in p]

    # 所有 prompt 都带蓝图、位置、ask；黑板地址只在第一条 user msg 给一次
    for i, p in enumerate(calls):
        assert "```xbt\n" in p, p[:200]
        assert "你在这棵树里的位置：" in p
        assert "/var/xbt.md" not in p
        assert ("黑板：/XBT_BlackBoard/scope_test.md::BlackBoard" in p) if i == 0 else ("黑板：" not in p)
        assert "node_kind" not in p
        pos = p.split("```text", 1)[1].split("```", 1)[0]
        assert "← 当前" in pos
        assert not re.search(r"\d+\.\d+", pos), pos

    # 条件 prompt 里，被测条件用「当前要判断的条件」段取，不能拿整段 prompt 搜
    # （蓝图里也含这些自然语言，整段搜会误命中）。
    cand_conds = [_cond_text(p) for p in cond_prompts]
    assert cand_conds.count("变更涉及资金或退款吗?") == 1, cand_conds
    assert cand_conds.count("需要人工审批吗?") == 1, cand_conds

    # 候选段：只在 selector 子条件出现，且只有当前 child 被问
    assert len(candidate_prompts) == 1, candidate_prompts
    cand = candidate_prompts[0]
    assert _cond_text(cand) == "变更涉及资金或退款吗?"
    assert "当前问第 0 个" in cand
    assert "- 0:" in cand and "- 1:" in cand
    assert "> [] → 成立时执行：" in cand

    # 自守卫：无候选段
    guard = [p for p in cond_prompts if _cond_text(p) == "需要人工审批吗?"][0]
    assert "这个 selector 的全部候选" not in guard

    # action：无候选段，位置保留动作体，ask 是 action prompt
    for p in act_prompts:
        assert "这个 selector 的全部候选" not in p
        last = [ln for ln in p.splitlines() if ln.strip()][-1]
        assert last == '请按 action prompt 执行。'

    # 每个 condition 的 ask 是布尔
    for p in cond_prompts:
        last = [ln for ln in p.splitlines() if ln.strip()][-1]
        assert last == "请结合上下文，只返回 true 或 false。"

    # selector 命中：fallback 不作为 condition 被问
    assert all(_cond_text(p) != "写常规发布计划到 /out.md::PLAN" for p in cond_prompts)
    return cond_prompts, act_prompts


async def test_nlp_tree_high_risk_path():
    runtime, ctx, calls = await run_nlp_tree({
        "变更涉及资金或退款吗?": True,
        "需要人工审批吗?": True,
    })
    cond_prompts, act_prompts = assert_prompt_contract(calls)

    # 7 次 driver 调用：selector 子条件 + 选中 action + 自守卫 + 审批 action + 2 并行 + 摘要 action
    assert len(cond_prompts) == 2, calls
    assert len(act_prompts) == 5, calls
    assert len(calls) == 7

    out = str(await runtime.exec("$ /out.md::PLAN", ctx=ctx))
    assert "高风险发布计划" in out and "常规发布计划" not in out

    approval = str(await runtime.exec("$ /approval.md::REQ", ctx=ctx))
    assert "高风险发布需双人审批" in approval

    assert "关闭开关切回 v1" in str(await runtime.exec("$ /out.md::ROLLBACK", ctx=ctx))
    assert "支付成功率 / 错误率" in str(await runtime.exec("$ /out.md::MONITOR", ctx=ctx))
    assert "摘要" in str(await runtime.exec("$ /out.md::SUMMARY", ctx=ctx))

    # 执行记录不再进 prompt；黑板地址只在第一条 user msg 给一次
    summary_prompt = [p for p in act_prompts if _action_text(p) == "根据黑板写执行摘要到 /out.md::SUMMARY"][0]
    assert "/var/xbt.md" not in summary_prompt
    assert "黑板：" not in summary_prompt

    bb = str(await runtime.exec("$ /var/xbt.md::scope", ctx=ctx))
    assert "- risk: high" in bb


async def test_nlp_tree_fallback_and_guard_skip():
    runtime, ctx, calls = await run_nlp_tree({
        "变更涉及资金或退款吗?": False,
        "需要人工审批吗?": False,
    })
    cond_prompts, act_prompts = assert_prompt_contract(calls)

    # 6 次：selector 子条件(1) + fallback action(1) + 自守卫(1) + 并行 action(2) + 摘要(1)
    assert len(cond_prompts) == 2, calls
    assert len(act_prompts) == 4, calls
    assert len(calls) == 6

    out = str(await runtime.exec("$ /out.md::PLAN", ctx=ctx))
    assert "常规发布计划" in out and "高风险发布计划" not in out

    # 自守卫 false → 整棵审批子树跳过
    assert "审批" not in str(await runtime.exec("$ /approval.md::REQ", ctx=ctx))

    # 执行记录不再进 prompt；黑板位置来自蓝图原文（- BB:）
    summary_prompt = [p for p in act_prompts if _action_text(p) == "根据黑板写执行摘要到 /out.md::SUMMARY"][0]
    assert "/var/xbt.md" not in summary_prompt
