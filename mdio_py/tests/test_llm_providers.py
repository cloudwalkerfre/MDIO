"""LLM provider 只读命令 / 合并 / 余额适配器单测。全程不联网。"""

from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

from mdio_py import MdioRuntime
from mdio_py.core.ast_utils import inline_text, walk
from mdio_py.plugins.llm import providers as P
from mdio_py.plugins.llm.llm import LLMPlugin

CONFIG = """# LLM_CONFIG

## A

- provider: deepseek
- model: m1
- reasoning_effort: high

# PROVIDERS

## deepseek

- base_url: https://api.deepseek.com/v1
- api_key: env:MY_TEST_KEY
- timeout: 30

## unsupported

- base_url: https://example.invalid/v1
"""


@pytest.fixture
async def rt(tmp_path):
    (tmp_path / "wt" / ".mds").mkdir(parents=True, exist_ok=True)
    (tmp_path / "wt" / ".mds" / "llm_config.md").write_text(CONFIG, encoding="utf-8")
    runtime = MdioRuntime(str(tmp_path))
    yield runtime
    runtime.close()


# 元数据遍历 / 降级

def _fake_anyllm(monkeypatch):
    good = SimpleNamespace(
        name="alpha", class_name="AlphaProvider", tier="verified",
        doc_url="https://alpha.example/", env_key="ALPHA_KEY", env_api_base="ALPHA_BASE",
        streaming=True, reasoning=True, completion=True, embedding=False,
        list_models=True,
    )
    class _Cls:
        API_BASE = "https://alpha.example/v1"
        @staticmethod
        def get_provider_metadata():
            return good
    class _Bad:
        API_BASE = ""
        @staticmethod
        def get_provider_metadata():
            raise ImportError("sdk missing")
    class _Any:
        @staticmethod
        def get_supported_providers():
            return ["alpha", "beta"]
        @staticmethod
        def get_provider_class(name):
            if name == "alpha":
                return _Cls
            return _Bad
    monkeypatch.setattr(P, "_anyllm", lambda: _Any)
    P._PROVIDER_CACHE["payload"] = None
    P._PROVIDER_CACHE["at"] = 0.0


def test_providers_payload_degrades_missing_sdk(monkeypatch):
    _fake_anyllm(monkeypatch)
    payload = P.providers_payload()
    assert [e["name"] for e in payload] == ["alpha", "beta"]
    alpha, beta = payload
    assert alpha["available"] is True
    assert alpha["env_key"] == "ALPHA_KEY"
    assert alpha["api_base"] == "https://alpha.example/v1"
    assert alpha["capabilities"]["reasoning"] is True
    assert alpha["capabilities"]["embedding"] is False
    assert alpha["capabilities"]["messages"] == "unknown"
    assert beta["available"] is False
    assert beta["error"] == "metadata_unavailable"


def test_provider_payload_single_and_unknown(monkeypatch):
    _fake_anyllm(monkeypatch)
    assert P.provider_payload("alpha")["available"] is True
    unknown = P.provider_payload("gamma")
    assert unknown["name"] == "gamma"
    assert unknown["available"] is False


# 合并 / api_key env

async def test_merge_provider_defaults_and_alias_override(rt, monkeypatch):
    monkeypatch.setenv("MY_TEST_KEY", "sk-from-env")
    ws = rt._workspace
    cfg = await LLMPlugin()._get_god_secrets(ws, "A")
    assert cfg["provider"] == "deepseek"
    assert cfg["base_url"] == "https://api.deepseek.com/v1"     # 来自 PROVIDERS
    assert cfg["timeout"] == "30"                               # 来自 PROVIDERS
    assert cfg["model"] == "m1"                                 # alias
    assert cfg["reasoning_effort"] == "high"                    # alias
    assert cfg["api_key"] == "sk-from-env"                      # env 解析
    assert "sk-from-env" not in json.dumps(cfg.get("extra_body") or {})


async def test_merge_env_missing_drops_key_and_reports_missing(rt, monkeypatch):
    monkeypatch.delenv("MY_TEST_KEY", raising=False)
    ws = rt._workspace
    cfg = await LLMPlugin()._get_god_secrets(ws, "A")
    assert "api_key" not in cfg
    _resolved, source = P.resolve_api_key(cfg)
    assert source == "missing"


def test_resolve_api_key_literal_and_missing():
    assert P.resolve_api_key({"api_key": "sk-literal"})[1] == "literal"
    assert P.resolve_api_key({"api_key": ""})[1] == "missing"
    assert P.resolve_api_key({})[1] == "missing"


# 余额（mock HTTP）

async def test_fetch_balance_deepseek_ok(monkeypatch):
    P._BALANCE_CACHE.clear()
    async def fake_get(url, headers, timeout, proxy):
        assert url == "https://api.deepseek.com/user/balance"
        assert headers["Authorization"].startswith("Bearer sk-")
        return 200, json.dumps({"is_available": True, "balance_infos": [
            {"currency": "CNY", "total_balance": "509.95", "granted_balance": "0.00", "topped_up_balance": "509.95"},
        ]})
    monkeypatch.setattr(P, "_http_get", fake_get)
    payload = await P.fetch_balance(provider="deepseek", api_key="sk-x",
                                    api_base="https://api.deepseek.com/v1")
    assert payload["supported"] is True
    assert payload["is_available"] is True
    assert payload["balances"][0]["currency"] == "CNY"
    assert payload["balances"][0]["total_balance"] == "509.95"
    assert payload["fetched_at"]


async def test_fetch_balance_error_status_and_unsupported(monkeypatch):
    P._BALANCE_CACHE.clear()
    async def fake_get(url, headers, timeout, proxy):
        return 401, "{}"
    monkeypatch.setattr(P, "_http_get", fake_get)
    payload = await P.fetch_balance(provider="deepseek", api_key="sk-x",
                                    api_base="https://api.deepseek.com")
    assert payload["error_kind"] == "auth"

    unsupported = await P.fetch_balance(provider="mystery", api_key="", api_base="")
    assert unsupported["supported"] is False


# 命令输出 + tojson 契约

async def test_command_providers_output_contract(rt, monkeypatch):
    _fake_anyllm(monkeypatch)
    out = await LLMPlugin()(rt._workspace, "", [], None, {"-provider": True})
    assert out.startswith("# PROVIDERS")
    assert "## alpha" in out
    assert "## beta" in out
    assert "### capabilities" in out
    env = json.loads(rt._workspace.to_json_text(out))
    h1 = [n for n in walk(env) if n.get("nt") == "h" and n.get("lv") == 1][0]
    assert inline_text(h1.get("t")) == "PROVIDERS"
    names = [inline_text(n.get("t")) for n in walk(env) if n.get("nt") == "h" and n.get("lv") == 2]
    assert names == ["alpha", "beta"]
    assert "- available: true" in out
    assert "- available: false" in out


async def test_command_balance_and_check(rt, monkeypatch):
    async def fake_fetch_balance(**kwargs):
        return {"supported": True, "provider": "deepseek", "is_available": True,
                "balances": [{"currency": "CNY", "total_balance": "1.00",
                              "granted_balance": "0.00", "topped_up_balance": "1.00"}],
                "fetched_at": "2026-10-26T00:00:00+08:00"}
    monkeypatch.setattr(P, "fetch_balance", fake_fetch_balance)
    monkeypatch.setenv("MY_TEST_KEY", "sk-from-env")
    plugin = LLMPlugin()
    bal = await plugin(rt._workspace, "", [], None, {"-balance": True, "-provider": "deepseek"})
    assert bal.startswith("# BALANCE")
    assert "## deepseek" in bal
    assert "- is_available: true" in bal
    assert "#### CNY" in bal

    chk = await plugin(rt._workspace, "", [], None, {"-check": "A"})
    assert chk.startswith("# CHECK")
    assert "api_key_source: env" in chk
    assert "## messages" in chk


def test_provider_alias_normalization():
    from mdio_py.plugins.llm import backend as B
    assert B.normalize_provider("kimi") == "moonshot"
    assert B.normalize_provider("glm") == "zai"
    assert B.normalize_provider("google") == "gemini"
    assert B.normalize_provider("deepseek") == "deepseek"
    assert P.normalize_provider_id("kimi") == "moonshot"


async def test_no_alias_provider_slash_model(rt, monkeypatch):
    monkeypatch.setenv("MY_TEST_KEY", "sk-env")
    cfg = await LLMPlugin()._get_god_secrets(rt._workspace, "deepseek/deepseek-v4-flash")
    assert cfg.get("provider") == "deepseek"
    assert cfg.get("model") == "deepseek-v4-flash"
    assert cfg.get("base_url") == "https://api.deepseek.com/v1"
    assert cfg.get("api_key") == "sk-env"


async def test_command_models_uses_provider_credentials(rt, monkeypatch):
    captured = {}
    async def fake_list(**kwargs):
        captured.update(kwargs)
        return {"supported": True, "provider": "deepseek", "models": ["m1", "m2"]}
    monkeypatch.setattr(P, "list_models", fake_list)
    monkeypatch.setenv("MY_TEST_KEY", "sk-from-env")
    out = await LLMPlugin()(rt._workspace, "", [], None, {"-models": True, "-provider": "deepseek"})
    assert out.startswith("# MODELS")
    assert "m1" in out and "m2" in out
    assert captured.get("provider") == "deepseek"
    assert captured.get("api_key") == "sk-from-env"


async def test_command_does_not_steal_agent_list(rt):
    out = await LLMPlugin()(rt._workspace, "", [], None, {})
    assert "No agents initialized yet" in out
