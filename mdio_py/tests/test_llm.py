import os
import re
import asyncio
import pytest
from pathlib import Path
from mdio_py import MdioRuntime


async def test_stats_no_args(runtime_llm, sandbox_id):
    print("\n=== 📊 test_stats_no_args ===")
    await runtime_llm.exec('-> /.mds/llm.md::AGENTS.director', wt_uid=sandbox_id)
    await runtime_llm.exec('@LLM.director.init -m "DEFAULT"', wt_uid=sandbox_id)
    result = await runtime_llm.exec('@LLM', wt_uid=sandbox_id)
    print(result)
    assert "# CURRENT_AGENTS" in result
    print("✅ 统计信息格式验证通过")


async def test_stats_single_agent(runtime_llm, sandbox_id):
    print("\n=== 📋 test_stats_single_agent ===")
    await runtime_llm.exec('-> /.mds/llm.md::AGENTS.director', wt_uid=sandbox_id)
    await runtime_llm.exec('@LLM.director.init -m "DEFAULT"', wt_uid=sandbox_id)
    result = await runtime_llm.exec('@LLM.director', wt_uid=sandbox_id)
    print(result)
    assert result.startswith("## director")
    assert "- model: " in result
    print("✅ 单个 agent 统计格式正确")


@pytest.mark.live
async def test_char_accumulation(runtime_llm, sandbox_id, live_llm):
    print("\n=== 🔢 test_char_accumulation ===")
    await runtime_llm.exec('-> /.mds/llm.md::AGENTS.director', wt_uid=sandbox_id)
    await runtime_llm.exec('@LLM.director.init -m "DEFAULT"', wt_uid=sandbox_id)

    stats_before = await runtime_llm.exec('@LLM.director', wt_uid=sandbox_id)
    match = re.search(r'- chars: (\d+)', stats_before)
    chars_before = int(match.group(1)) if match else 0
    print(f"对话前 chars: {chars_before}")

    await runtime_llm.exec('"第一条消息" | @LLM.director.run', wt_uid=sandbox_id)
    await runtime_llm.exec('"第二条消息" | @LLM.director.run', wt_uid=sandbox_id)

    stats_after = await runtime_llm.exec('@LLM.director', wt_uid=sandbox_id)
    match = re.search(r'- chars: (\d+)', stats_after)
    chars_after = int(match.group(1)) if match else 0
    print(f"对话后 chars: {chars_after}")
    assert chars_after > chars_before, f"字符数未累加: {chars_before} -> {chars_after}"
    print("✅ chars 正确累加")


@pytest.mark.live
async def test_msg_rounds_increment(runtime_llm, sandbox_id, live_llm):
    print("\n=== 🔄 test_msg_rounds_increment ===")
    await runtime_llm.exec('-> /.mds/llm.md::AGENTS.director', wt_uid=sandbox_id)
    await runtime_llm.exec('@LLM.director.init -m "DEFAULT"', wt_uid=sandbox_id)

    stats_before = await runtime_llm.exec('@LLM.director', wt_uid=sandbox_id)
    match = re.search(r'- msg_rounds: (\d+)', stats_before)
    rounds_before = int(match.group(1)) if match else 0
    print(f"对话前 msg_rounds: {rounds_before}")

    await runtime_llm.exec('"测试轮数增加" | @LLM.director.run', wt_uid=sandbox_id)

    stats_after = await runtime_llm.exec('@LLM.director', wt_uid=sandbox_id)
    match = re.search(r'- msg_rounds: (\d+)', stats_after)
    rounds_after = int(match.group(1)) if match else 0
    print(f"对话后 msg_rounds: {rounds_after}")
    assert rounds_after == rounds_before + 1, f"轮数未增加: {rounds_before} -> {rounds_after}"
    print("✅ msg_rounds 正确递增")


async def test_config_update(runtime_llm, sandbox_id):
    print("\n=== ⚙️ test_config_update ===")
    await runtime_llm.exec('-> /.mds/llm.md::AGENTS.director', wt_uid=sandbox_id)
    await runtime_llm.exec('@LLM.director.init -m "DEFAULT"', wt_uid=sandbox_id)

    new_sysp = "你是一个测试助手，请简洁回答。"
    update_res = await runtime_llm.exec(f'@LLM.director -sysp "{new_sysp}"', wt_uid=sandbox_id)
    print("更新回执:", update_res)
    config_output = await runtime_llm.exec('@LLM.director', wt_uid=sandbox_id)
    match = re.search(r'- sysp: (.*?)(?:\n|$)', config_output)
    if match:
        sysp_value = match.group(1).rstrip('...')
        print(f"更新后 sysp: {sysp_value}")
        assert new_sysp.startswith(sysp_value) or sysp_value.startswith(new_sysp), "sysp 未更新"
    else:
        pytest.fail("未找到 sysp 字段")
    print("✅ 配置更新成功")


async def test_provider_backend_config(runtime_llm, sandbox_id):
    """provider/backend 可按 agent 配置并在 stats 里可见（any-llm 的 provider 缺省由 base_url 推断）。"""
    print("\n=== 🔌 test_provider_backend_config ===")
    await runtime_llm.exec('-> /.mds/llm.md::AGENTS.director', wt_uid=sandbox_id)
    await runtime_llm.exec('@LLM.director.init -m "DEFAULT" -provider deepseek',
                           wt_uid=sandbox_id)

    stats = await runtime_llm.exec('@LLM.director', wt_uid=sandbox_id)
    assert re.search(r'- backend: anyllm', stats), stats
    assert re.search(r'- provider: deepseek', stats), stats

    res = await runtime_llm.exec('@LLM.director -provider openrouter', wt_uid=sandbox_id)
    assert "configuration updated" in res, res
    stats = await runtime_llm.exec('@LLM.director', wt_uid=sandbox_id)
    assert re.search(r'- provider: openrouter', stats), stats
    print("✅ provider/backend 可 init、可更新、可见")


@pytest.mark.live
async def test_callback(runtime_llm, sandbox_id, live_llm):
    print("\n=== 🪝 test_callback ===")
    await runtime_llm.exec('-> /.mds/llm.md::AGENTS.director', wt_uid=sandbox_id)
    await runtime_llm.exec('@LLM.director.init -m "DEFAULT"', wt_uid=sandbox_id)

    cb_pipeline = "'true' | +>/test_cb_flag::R._b:callback_executed"
    cmd = f'mk "/test_cb_flag.md" -f ; "Hello, callback test" | @LLM.director.run -cb """{cb_pipeline}"""'

    result = await runtime_llm.exec(cmd, wt_uid=sandbox_id)
    print(f"对话结果摘要: {result[:100]}...")

    flag_content = await runtime_llm.exec("$/test_cb_flag::R._b:callback_executed", wt_uid=sandbox_id)
    print(f"回调res: {flag_content}")
    assert "true" in flag_content, "回调未执行或内容不正确"
    print("✅ 回调成功执行")

    await runtime_llm.exec("->/test_cb_flag::R._b:callback_executed", wt_uid=sandbox_id)


async def test_callback_runs_once_at_turn_end(runtime_llm, sandbox_id):
    """离线验证 -cb 的执行点：状态带着 cb 跑到 DONE 时执行一次；下一轮（cb 已随瞬态清掉）不再执行。"""
    from mdio_py.core.interpreter import RuntimeContext
    from mdio_py.plugins.llm.llm import LLMPlugin, _append_record, _read_state

    ws, ctx = runtime_llm._workspace, RuntimeContext(wt_uid=sandbox_id)
    cfg = {"his": "HIS", "m": "FAKE", "sysp": "", "tool": "false", "retry": "1"}

    async def fake_infer(messages, secrets, is_stream, tools=None, num_retries=1):
        yield ("content", "好")

    async def fake_secrets(ws_, alias):
        return {"model": "fake", "api_key": "x"}

    plugin = LLMPlugin()
    plugin._infer_and_stream = fake_infer
    plugin._get_god_secrets = fake_secrets

    await runtime_llm.exec("mk -f /cb_flag.md", wt_uid=sandbox_id)
    _append_record(ws, "cba", "HIS", "user", "你好", cfg, ctx)

    await plugin._handle_busy(ws, "cba", "main", {
        "status": "BUSY", "turn": "1", "step": "0",
        "cb": "'true' | +>/cb_flag.md::R._b:fired"}, cfg)

    assert _read_state(ws, "cba", "main", ctx).get("status") == "DONE"
    assert "true" in await runtime_llm.exec("$/cb_flag.md::R._b:fired", wt_uid=sandbox_id)

    # 第二轮：cb 已随瞬态字段清掉 ⇒ 清空标志后再跑一轮，不应被重新执行
    await runtime_llm.exec("-> /cb_flag.md::R._b:fired", wt_uid=sandbox_id)
    await plugin._handle_busy(ws, "cba", "main", _read_state(ws, "cba", "main", ctx), cfg)
    assert "true" not in await runtime_llm.exec("$/cb_flag.md::R._b:fired", wt_uid=sandbox_id)


async def main():
    """Run all LLM tests sequentially with a real temp runtime."""
    import tempfile, shutil
    tmp = Path(tempfile.mkdtemp())
    print(f"Temp dir: {tmp}")

    # 唯一配置来源：tests/fixtures/genesis（无 secret）
    fixture_config = Path(__file__).resolve().parent / "fixtures" / "genesis" / "wt" / ".mds" / "llm_config.md"
    (tmp / "wt" / ".mds").mkdir(parents=True, exist_ok=True)
    shutil.copy(fixture_config, tmp / "wt" / ".mds" / "llm_config.md")

    rt = MdioRuntime(str(tmp))
    await rt.exec("mk -f /.mds/llm.md", wt_uid="genesis")
    await rt.exec("mk -f /.mds/proc.md", wt_uid="genesis")
    sid = "genesis"

    tests = [
        ("test_stats_no_args", test_stats_no_args, False),
        ("test_stats_single_agent", test_stats_single_agent, False),
        ("test_char_accumulation", test_char_accumulation, True),
        ("test_msg_rounds_increment", test_msg_rounds_increment, True),
        ("test_config_update", test_config_update, False),
        ("test_provider_backend_config", test_provider_backend_config, False),
        ("test_callback", test_callback, True),
    ]
    live_ok = bool(os.environ.get("MDIO_LLM_SECRET"))
    for name, fn, needs_live in tests:
        print(f"\n{'='*60}")
        print(f"Running {name}...")
        print(f"{'='*60}")
        if needs_live and not live_ok:
            print(f"⏭️  {name} SKIPPED: MDIO_LLM_SECRET 未设置（live 测试）")
            continue
        try:
            if needs_live:
                await fn(rt, sid, True)
            else:
                await fn(rt, sid)
            print(f"✅ {name} PASSED")
        except Exception as e:
            print(f"❌ {name} FAILED: {e}")

    shutil.rmtree(tmp)
    print(f"\n✅ All tests complete, temp dir cleaned.")


if __name__ == "__main__":
    asyncio.run(main())
