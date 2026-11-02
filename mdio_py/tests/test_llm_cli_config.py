"""CLI llm config + system default model + XBT_DIRECTOR."""

import os
import tempfile

import pytest
from click.testing import CliRunner

from mdio_py import MdioRuntime
from mdio_py.cli import main as cli_main
from mdio_py.cli import llm_config as cli_llm
from mdio_py.cli.llm_config import build_commands
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.plugins.llm import providers as provider_ops
from mdio_py.plugins.llm.llm import (
    DEFAULT_ALIAS,
    DEFAULT_XBT_SYSP_FILE,
    XBT_DRIVER,
    _read_config,
    _read_state,
    _resolve_sysp_path,
)


def _rt():
    return MdioRuntime(tempfile.mkdtemp(prefix="mdio_llm_cli_"))


def _ctx():
    return RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)


async def _exec(rt, code, ctx=None):
    r = await rt.exec(code, ctx=ctx or _ctx())
    assert not str(r).startswith("- mdio_error:"), (code, r)
    return str(r)


async def test_build_and_apply_deepseek_commands():
    rt = _rt()
    ctx = _ctx()
    name, info, model, commands, err = build_commands("deepseek", "sk-test", None)
    assert not err
    assert name == "deepseek"
    assert model == "deepseek-flash"
    assert commands[0] == "mk -f /.mds/llm_config.md"
    for code in commands:
        await _exec(rt, code, ctx)
    # second round must be idempotent
    for code in commands:
        await _exec(rt, code, ctx)
    cfg = str(await rt.exec("$ /.mds/llm_config.md", ctx=ctx))
    llm = str(await rt.exec("$ /.mds/llm.md", ctx=ctx))
    assert cfg.count("## deepseek") == 1, cfg
    assert cfg.count("## " + DEFAULT_ALIAS) == 1, cfg
    assert llm.count("## " + XBT_DRIVER) == 1, llm
    prov = provider_ops.read_provider_section(rt._workspace, "deepseek")
    assert prov["base_url"] == "https://api.deepseek.com"
    assert prov["api_key"] == "sk-test"
    agent = _read_config(rt._workspace, XBT_DRIVER, ctx)
    assert agent["m"] == DEFAULT_ALIAS
    assert agent["sysp_file"] == DEFAULT_XBT_SYSP_FILE
    assert agent["sysp"] == _resolve_sysp_path(DEFAULT_XBT_SYSP_FILE).read_text(encoding="utf-8")


async def test_unknown_provider_is_rejected():
    _name, _info, _model, commands, err = build_commands("definitely-not-a-provider", "sk")
    assert err.startswith("- mdio_error:")
    assert commands == []


def test_non_deepseek_requires_default_model(monkeypatch):
    async def _models(**kwargs):
        return {"supported": True, "provider": "openai", "models": ["m-a", "m-b"]}

    monkeypatch.setattr(provider_ops, "list_models", _models)
    _name, _info, _model, commands, err = build_commands("openai", "sk")
    assert err.startswith("- mdio_error:")
    assert "--default_model" in err
    assert "m-a" in err and "m-b" in err
    assert commands == []


async def test_non_deepseek_with_explicit_model():
    name, _info, model, commands, err = build_commands("openai", "sk", "gpt-5.1")
    assert not err
    assert name == "openai" and model == "gpt-5.1"
    assert any("_b:model" in c and "gpt-5.1" in c for c in commands)


async def test_init_without_m_uses_default_alias():
    rt = _rt()
    ctx = _ctx()
    for code in [
        "mk -f /.mds/llm_config.md",
        "mk -f /.mds/llm.md",
        '"deepseek" | /> /.mds/llm_config.md::"LLM_CONFIG"."DEFAULT"._b:provider',
        '"deepseek-flash" | /> /.mds/llm_config.md::"LLM_CONFIG"."DEFAULT"._b:model',
    ]:
        await _exec(rt, code, ctx)
    r = str(await rt.exec("@LLM.dev.init -tool false", ctx=ctx))
    assert not r.startswith("- mdio_error:")
    assert _read_config(rt._workspace, "dev", ctx)["m"] == DEFAULT_ALIAS


async def test_lazy_xbt_director_reuses_init_and_creates_hook():
    rt = _rt()
    ctx = _ctx()
    for code in [
        "mk -f /.mds/llm_config.md",
        "mk -f /.mds/llm.md",
        '"deepseek" | /> /.mds/llm_config.md::"LLM_CONFIG"."DEFAULT"._b:provider',
        '"deepseek-flash" | /> /.mds/llm_config.md::"LLM_CONFIG"."DEFAULT"._b:model',
    ]:
        await _exec(rt, code, ctx)
    plugin = rt._workspace.get_plugin("LLM")
    err = await plugin._ensure_xbt_director(rt._workspace, ctx)
    assert not err
    agent = _read_config(rt._workspace, XBT_DRIVER, ctx)
    assert agent["m"] == DEFAULT_ALIAS
    assert len(agent["sysp"]) > 100
    state = _read_state(rt._workspace, XBT_DRIVER, "main", ctx)
    assert state.get("sys_hook_id") and state.get("sys_hook_id") != "0"


async def test_lazy_xbt_director_without_default_reports_english_error():
    rt = _rt()
    ctx = _ctx()
    plugin = rt._workspace.get_plugin("LLM")
    err = await plugin._ensure_xbt_director(rt._workspace, ctx)
    assert err.startswith("- mdio_error: XBT_DIRECTOR is missing provider/secret configuration.")


def test_xbt_default_driver_falls_back_to_xbt_director(monkeypatch):
    from mdio_py.plugins.xbt import stepper as xbt_stepper
    from mdio_py.plugins.xbt.parser import parse_xbt

    monkeypatch.setattr(xbt_stepper, "DEFAULT_DRIVER", "XBT_DIRECTOR")
    ast = parse_xbt("- PLAN:" + chr(10) + "    >> [] 动作")
    ptr = "0.0 " + xbt_stepper.compute_node_hash(ast[0]["children"][0])
    assert xbt_stepper.find_driver(ast, ptr, {}) == "XBT_DIRECTOR"


def test_cli_command_posts_exec_pipeline(monkeypatch):
    seen = []

    def _post(path, payload, timeout=10.0):
        seen.append((path, payload))
        return {"status": "ok", "result": "ok"}

    monkeypatch.setattr(cli_llm, "_post_json", _post)
    out = CliRunner().invoke(cli_main, ["llm", "config", "deepseek", "sk-test"])
    assert out.exit_code == 0, out.output
    assert seen[0][0] == "/exec"
    assert seen[0][1]["code"] == "mk -f /.mds/llm_config.md"
    assert any("PROVIDERS" in p["code"] for _, p in seen)
    assert "- mdio_success:" in out.output
    assert "provider deepseek configured" in out.output


def test_cli_command_unknown_provider_fails(monkeypatch):
    seen = []

    def _post(path, payload, timeout=10.0):
        seen.append((path, payload))
        return {"status": "ok", "result": "ok"}

    monkeypatch.setattr(cli_llm, "_post_json", _post)
    out = CliRunner().invoke(cli_main, ["llm", "config", "definitely-not-a-provider", "sk"])
    assert out.exit_code != 0
    assert "- mdio_error:" in out.output
    assert not seen
