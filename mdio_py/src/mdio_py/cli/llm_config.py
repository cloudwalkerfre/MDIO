"""mdio llm config：CLI 侧生成 mdio 写入管道，走现有 /exec。

配置就是 md 文档；这里不新增网关路由，也不新增 @LLM 配置命令。
"""

from __future__ import annotations

import asyncio
import json
import os
import sys

import click

from mdio_py.plugins.llm import providers as provider_ops
from mdio_py.plugins.llm.llm import (
    DEFAULT_ALIAS,
    DEFAULT_BACKEND,
    DEFAULT_XBT_SYSP_FILE,
    XBT_DRIVER,
)

from .commands import _not_started, _post_json


def _q(value) -> str:
    """MDIO selector 名/值的双引号字面量。"""
    return json.dumps(str(value), ensure_ascii=False)


def _field(path: str, selector: str, value) -> str:
    return _q(value) + " | /> " + path + "::" + selector


def _err(text: str) -> str:
    return text if text.startswith("- mdio_error:") else "- mdio_error: " + text


def _validate_provider(provider: str):
    name = provider_ops.normalize_provider_id(provider)
    known = {str(p.get("name") or "") for p in provider_ops.providers_payload()}
    if not name or name not in known:
        return "", None, _err("unknown or unavailable provider %r (not recognized by any-llm)" % provider)
    info = provider_ops.provider_payload(name) or {}
    return name, info, ""


def _resolve_secret(secret: str) -> str:
    try:
        cfg, _ = provider_ops.resolve_api_key({"api_key": secret})
        return str(cfg.get("api_key") or "")
    except Exception:
        return str(secret or "")


def _default_model(provider: str, info: dict, secret: str, explicit: str | None):
    if explicit:
        return explicit, ""
    if provider == "deepseek":
        return "deepseek-flash", ""
    base_url = str((info or {}).get("api_base") or "")
    payload = asyncio.run(provider_ops.list_models(
        provider=provider,
        api_key=_resolve_secret(secret),
        api_base=base_url,
        timeout=20.0,
    ))
    models = [str(m) for m in (payload.get("models") or [])]
    if payload.get("error_kind"):
        msg = str(payload.get("error_kind"))
    else:
        msg = "no model list"
    if models:
        return "", ("- mdio_error: provider %r requires --default_model because any-llm "
                    "has no default/recommended model.\n  Available models:\n%s"
                    % (provider, "\n".join("    - " + m for m in models[:30])))
    return "", ("- mdio_error: provider %r requires --default_model (%s)."
                % (provider, msg))


def build_commands(provider: str, secret: str, default_model: str | None = None):
    """返回 (name, info, model, commands, error)。"""
    name, info, err = _validate_provider(provider)
    if err:
        return "", {}, "", [], err
    model, err = _default_model(name, info, secret, default_model)
    if err:
        return name, info, "", [], err
    base_url = str((info or {}).get("api_base") or "")
    commands = [
        "mk -f /.mds/llm_config.md",
        "mk -f /.mds/llm.md",
    ]
    # PROVIDERS
    if base_url:
        commands.append(_field("/.mds/llm_config.md",
                               _q("PROVIDERS") + "." + _q(name) + "._b:base_url",
                               base_url))
    commands.append(_field("/.mds/llm_config.md",
                           _q("PROVIDERS") + "." + _q(name) + "._b:api_key", secret))
    commands.append(_field("/.mds/llm_config.md",
                           _q("PROVIDERS") + "." + _q(name) + "._b:timeout", "180"))
    # LLM_CONFIG.DEFAULT = 系统默认模型
    commands.append(_field("/.mds/llm_config.md",
                           _q("LLM_CONFIG") + "." + _q(DEFAULT_ALIAS) + "._b:provider", name))
    commands.append(_field("/.mds/llm_config.md",
                           _q("LLM_CONFIG") + "." + _q(DEFAULT_ALIAS) + "._b:model", model))
    if name == "deepseek":
        commands.append(_field("/.mds/llm_config.md",
                               _q("LLM_CONFIG") + "." + _q(DEFAULT_ALIAS) + "._b:reasoning_effort", "max"))
        commands.append(_field("/.mds/llm_config.md",
                               _q("LLM_CONFIG") + "." + _q(DEFAULT_ALIAS) + "._b:extra_body",
                               '{"thinking": {"type": "enabled"}}'))
    # AGENTS.XBT_DIRECTOR
    agent_sel = _q("AGENTS") + "." + _q(XBT_DRIVER)
    commands.extend([
        _field("/.mds/llm.md", agent_sel + "._b:m", DEFAULT_ALIAS),
        _field("/.mds/llm.md", agent_sel + "._b:sysp_file", DEFAULT_XBT_SYSP_FILE),
        _field("/.mds/llm.md", agent_sel + "._b:tool", "true"),
        _field("/.mds/llm.md", agent_sel + "._b:his", "HIS"),
        _field("/.mds/llm.md", agent_sel + "._b:retry", "1"),
        _field("/.mds/llm.md", agent_sel + "._b:backend", DEFAULT_BACKEND),
    ])
    return name, info, model, commands, ""


def _run(code: str, wt_uid: str) -> str:
    result = _post_json("/exec", {
        "code": code,
        "active_doc": "",
        "wt_uid": wt_uid,
    })
    out = str(result.get("result") or "")
    if result.get("status") == "error" or out.startswith("- mdio_error:"):
        raise RuntimeError(out if out.startswith("- mdio_error:") else _err(out))
    return out


@click.group("llm")
def llm_group():
    """LLM provider/model configuration."""


@llm_group.command("config")
@click.argument("provider")
@click.argument("secret", required=False)
@click.option("--default_model", "default_model", default=None,
              help="Default model id for this provider (required for non-deepseek).")
@click.pass_context
def llm_config_cmd(ctx, provider, secret, default_model):
    """Configure provider + secret; preset the system default model and XBT_DIRECTOR.

    
    Examples:
      mdio llm config deepseek
      mdio llm config deepseek env:DEEPSEEK_API_KEY
      mdio llm config openai sk-xxx --default_model gpt-5.1
    """
    if not secret:
        secret = os.environ.get("MDIO_LLM_SECRET", "").strip()
    if not secret and sys.stdin.isatty():
        secret = click.prompt("Secret", hide_input=True)
    if not secret:
        click.echo("- mdio_error: secret is required (pass it, or set MDIO_LLM_SECRET)")
        ctx.exit(1)
        return
    name, info, model, commands, err = build_commands(provider, secret, default_model)
    if err:
        click.echo(err)
        ctx.exit(1)
        return
    try:
        wt = ctx.obj.get("wt", "genesis") if ctx.obj else "genesis"
        for code in commands:
            _run(code, wt)
        _run("$ /.mds/llm_config.md", wt)
        _run("$ /.mds/llm.md", wt)
    except (RuntimeError, OSError, Exception) as e:
        text = str(e)
        if "Connection refused" in text or "Urlopen" in type(e).__name__:
            _not_started()
        else:
            click.echo(text if text.startswith("- mdio_error:") else _err(text))
        ctx.exit(1)
        return
    click.echo("\n".join([
        "- mdio_success:",
        "    - provider " + name + " configured",
        "    - default model: " + model + (" (reasoning_effort=max, thinking=enabled)"
                                          if name == "deepseek" else ""),
        "    - XBT_DIRECTOR: ready",
    ]))
