"""能力预检单测：`provider_metadata` / `precheck_capabilities`。

纪律：全程不联网。除文件末尾两条对真实 any-llm 的弱断言（只到「能取到 deepseek 元数据」
这一层，不钉死易变字段）外，所有元数据都用 SimpleNamespace 替身注入，因此断言不随
any-llm 版本漂移，也不受缺失 SDK 影响。
"""

from __future__ import annotations

import sys
from types import SimpleNamespace

import pytest

from mdio_py.plugins.llm import backend as B

# ProviderMetadata 的能力位字段名（与 any_llm/types/provider.py 一致）；替身按此给默认值
_METADATA_FIELDS = (
    "streaming", "reasoning", "completion", "embedding", "moderation", "responses",
    "image", "pdf", "list_models", "messages", "messages_native", "batch_completion",
    "image_generation", "audio_transcription", "audio_speech", "rerank", "files",
    "file_operations",
)

# llm.py::_get_god_secrets 真实会读到的配置键 + 能力键：整套配置一个都不该被点名
_REAL_CONFIG = {
    "model": "deepseek-chat",
    "base_url": "https://api.deepseek.com/v1",
    "api_key": "sk-x",
    "proxy": "http://127.0.0.1:7898",
    "timeout": 180.0,
    "provider": "deepseek",
    "backend": "anyllm",
    "extra_body": {"a": 1},
    "reasoning_effort": "low",
    "stream": True,
}


def fake_metadata(**overrides) -> SimpleNamespace:
    """能力位默认全 False 的元数据替身；env_key 默认 None，避免误触「缺 api_key」提示。"""
    fields: dict = {name: False for name in _METADATA_FIELDS}
    fields.update({"name": "fake", "tier": "community", "env_key": None, "env_api_base": None,
                   "doc_url": "", "class_name": "FakeProvider"})
    fields.update(overrides)
    return SimpleNamespace(**fields)


@pytest.fixture
def seen(monkeypatch):
    """把 provider_metadata 换成替身，并记录它收到的 provider 入参（用于验解析优先级）。"""
    box: dict = {"md": fake_metadata(), "providers": []}

    def _fake(provider):
        box["providers"].append(provider)
        return box["md"]

    monkeypatch.setattr(B, "provider_metadata", _fake)
    return box


def _one_containing(messages: list[str], needle: str) -> str:
    hits = [m for m in messages if needle in m]
    assert len(hits) == 1, f"期望恰好一条含 {needle!r} 的消息，实得：{messages}"
    return hits[0]


# 键表形状：白名单本身就是契约

def test_key_tables_shape_and_disjoint():
    assert {"model", "base_url", "api_key", "proxy", "timeout", "provider", "backend"} <= B.CORE_KEYS
    assert not (B.CORE_KEYS & set(B.CAPABILITY_KEYS)), "同一个键不能既是直通键又是能力键"
    assert B.CAPABILITY_KEYS["reasoning_effort"] == "reasoning"
    assert B.CAPABILITY_KEYS["thinking"] == "reasoning"
    assert B.CAPABILITY_KEYS["stream"] == "streaming"
    assert "extra_body" not in B.CORE_KEYS, "extra_body 归能力键侧（登记但不判能力位）"
    assert B.CAPABILITY_KEYS["extra_body"] == ""


# 第 1 类：能力不匹配

def test_capability_false_is_flagged_with_key_provider_capability(seen):
    seen["md"] = fake_metadata(streaming=False)
    msgs = B.precheck_capabilities({"provider": "fake", "api_key": "sk-x", "stream": True})
    msg = _one_containing(msgs, "能力不匹配")
    assert "'stream'" in msg          # 键名
    assert "'fake'" in msg            # 提供者
    assert "streaming" in msg         # 能力名（= 元数据字段名）
    assert "False" in msg
    assert len(msgs) == 1


def test_capability_true_passes(seen):
    seen["md"] = fake_metadata(streaming=True, reasoning=True)
    assert B.precheck_capabilities({"provider": "fake", "api_key": "sk-x",
                                    "stream": True, "reasoning_effort": "low"}) == []


def test_two_keys_mapped_to_same_capability_each_get_a_message(seen):
    seen["md"] = fake_metadata(reasoning=False)
    msgs = B.precheck_capabilities({"provider": "fake", "api_key": "sk-x",
                                    "reasoning_effort": "low", "thinking": True})
    assert len(msgs) == 2, msgs
    assert _one_containing(msgs, "'reasoning_effort'") and _one_containing(msgs, "'thinking'")


def test_falsy_capability_value_is_treated_as_explicitly_off(seen):
    """`stream: false` 是显式关闭，不是「配置了不支持的能力」。"""
    seen["md"] = fake_metadata(streaming=False, reasoning=False)
    assert B.precheck_capabilities({"provider": "fake", "api_key": "sk-x",
                                    "stream": False, "reasoning_effort": ""}) == []


def test_absent_metadata_field_is_not_judged(seen):
    """元数据对象没有该字段（替身缺项）→ 无从判定，静默放行而不是误报。"""
    md = fake_metadata(streaming=True)
    object.__delattr__(md, "rerank")
    seen["md"] = md
    assert B.precheck_capabilities({"provider": "fake", "api_key": "sk-x", "rerank": True}) == []


# 第 2 类：未知键

def test_unknown_key_is_flagged_even_with_falsy_value(seen):
    """白名单外的键一律点名：键写在配置里就算数，值是什么不影响。"""
    seen["md"] = fake_metadata(streaming=True)
    msgs = B.precheck_capabilities({"provider": "fake", "api_key": "sk-x", "temperature": 0.7})
    msg = _one_containing(msgs, "未知配置键")
    assert "'temperature'" in msg
    assert len(msgs) == 1

    msgs_none = B.precheck_capabilities({"provider": "fake", "api_key": "sk-x", "temperature": None})
    assert len(msgs_none) == 1 and "'temperature'" in msgs_none[0]


def test_unknown_keys_are_reported_in_stable_sorted_order(seen):
    seen["md"] = fake_metadata(streaming=True)
    msgs = B.precheck_capabilities({"provider": "fake", "api_key": "sk-x",
                                    "zeta": 1, "alpha": 2})
    assert len(msgs) == 2, msgs
    assert "'alpha'" in msgs[0] and "'zeta'" in msgs[1]   # 键名升序，消息顺序稳定


def test_real_config_keys_all_pass(seen):
    """真实配置键集合（llm.py 的 _get_god_secrets + 能力键）一条消息都不该产生。"""
    seen["md"] = fake_metadata(streaming=True, reasoning=True)
    assert B.precheck_capabilities(_REAL_CONFIG) == []


# 第 3 类：缺 api_key 的 env 提示

def test_missing_api_key_suggests_env_key(seen):
    seen["md"] = fake_metadata(env_key="FAKE_API_KEY", streaming=True)
    msgs = B.precheck_capabilities({"provider": "fake", "model": "m"})
    msg = _one_containing(msgs, "api_key")
    assert "FAKE_API_KEY" in msg
    assert "'fake'" in msg
    assert len(msgs) == 1


def test_no_env_hint_when_metadata_has_no_env_key(seen):
    seen["md"] = fake_metadata(env_key=None, env_api_base="FAKE_API_BASE", streaming=True)
    assert B.precheck_capabilities({"provider": "fake", "model": "m"}) == []


def test_messages_never_echo_config_values(seen):
    """消息里只出现键名与提供者，绝不出现配置里的值（api_key 尤其）。"""
    seen["md"] = fake_metadata(streaming=False, env_key="FAKE_API_KEY")
    msgs = B.precheck_capabilities({"provider": "fake", "api_key": "sk-super-secret",
                                    "stream": True, "temperature": "sk-in-unknown-key"})
    joined = "".join(msgs)
    assert "sk-super-secret" not in joined
    assert "sk-in-unknown-key" not in joined
    assert len(msgs) == 2


# 元数据不可得分支

def test_metadata_unavailable_only_judges_unknown_keys(monkeypatch):
    monkeypatch.setattr(B, "provider_metadata", lambda provider: None)
    msgs = B.precheck_capabilities({"provider": "fake", "api_key": "sk-x",
                                    "stream": True, "embedding": True, "temperature": 0.7})
    assert len(msgs) == 2, msgs
    unavailable = _one_containing(msgs, "能力元数据不可得")
    unknown = _one_containing(msgs, "未知配置键")
    assert "'temperature'" in unknown
    assert "stream" not in unavailable and "embedding" not in "".join(m for m in msgs if m is unknown)
    assert unavailable.startswith("能力元数据不可得")   # 提示在前，未知键在后


def test_metadata_unavailable_names_unresolved_provider(monkeypatch):
    monkeypatch.setattr(B, "provider_metadata", lambda provider: None)
    msgs = B.precheck_capabilities({})
    assert len(msgs) == 1 and "未指定" in msgs[0]


# provider 解析优先级：入参 > 配置键 > base_url 推断

def test_provider_inferred_from_base_url(seen):
    seen["md"] = fake_metadata(streaming=True)
    assert B.precheck_capabilities({"base_url": "https://api.deepseek.com/v1"}) == []
    assert seen["providers"] == ["deepseek"]


def test_provider_config_key_beats_base_url(seen):
    seen["md"] = fake_metadata(streaming=True)
    B.precheck_capabilities({"base_url": "https://api.deepseek.com/v1", "provider": "OpenRouter"})
    assert seen["providers"] == ["openrouter"]      # 配置键优先且归一化小写


def test_explicit_provider_argument_wins(seen):
    seen["md"] = fake_metadata(streaming=True)
    B.precheck_capabilities({"provider": "deepseek"}, provider="OpenRouter")
    assert seen["providers"] == ["openrouter"]


# 永不抛异常

@pytest.mark.parametrize("secrets", [
    None, {}, "", "oops", [], 42,
    {"extra_body": "not-a-dict"}, {"stream": object()}, {"api_key": None},
    {"provider": 123}, {"base_url": 123}, {"timeout": None},
])
def test_precheck_never_raises_on_odd_secrets(secrets, seen):
    seen["md"] = fake_metadata(streaming=False, env_key="FAKE_API_KEY")
    out = B.precheck_capabilities(secrets)
    assert isinstance(out, list)
    assert all(isinstance(m, str) for m in out)


@pytest.mark.parametrize("secrets", [None, "", "oops", [], 42, {"stream": True}])
def test_precheck_never_raises_when_metadata_lookup_explodes(secrets, monkeypatch):
    def boom(provider):
        raise RuntimeError("元数据查询炸了")

    monkeypatch.setattr(B, "provider_metadata", boom)
    out = B.precheck_capabilities(secrets, provider="deepseek")
    assert isinstance(out, list) and all(isinstance(m, str) for m in out)


@pytest.mark.parametrize("provider", [None, "", "   ", "nope-xyz", 123, object(), [], {"a": 1}])
def test_provider_metadata_never_raises(provider):
    assert B.provider_metadata(provider) is None


def test_odd_config_keys_do_not_break_unknown_key_reporting(seen):
    """键类型混杂也不许中断遍历：未知键照样点名（内层排序按 str 兜底）。"""
    seen["md"] = fake_metadata(streaming=True)
    msgs = B.precheck_capabilities({1: "a", "zzz": 2, "api_key": "sk-x", "provider": "fake"})
    assert len(msgs) == 2, msgs
    assert "未知配置键" in msgs[0] and "未知配置键" in msgs[1]


def test_hostile_metadata_object_never_raises_and_still_reports_unknown_key(monkeypatch):
    class HostileMetadata:
        @property
        def streaming(self):
            raise RuntimeError("元数据替身炸了")

        env_key = None

    monkeypatch.setattr(B, "provider_metadata", lambda provider: HostileMetadata())
    msgs = B.precheck_capabilities({"provider": "fake", "stream": True, "unknown_key": 1})
    assert len(msgs) == 1 and "未知配置键" in msgs[0]


def test_provider_metadata_none_when_sdk_missing(monkeypatch):
    monkeypatch.setitem(sys.modules, "any_llm", None)   # `from any_llm import AnyLLM` 直接 ImportError
    assert B.provider_metadata("deepseek") is None


def test_provider_metadata_none_when_lookup_fails(monkeypatch):
    import any_llm

    def boom(provider_key):
        raise RuntimeError("provider 模块导入失败")

    monkeypatch.setattr(any_llm.AnyLLM, "get_provider_class", staticmethod(boom))
    assert B.provider_metadata("deepseek") is None


# 对真实 any-llm 的弱断言（不联网；不钉死易变字段）

def test_real_deepseek_metadata_is_reachable():
    pytest.importorskip("any_llm")
    md = B.provider_metadata("deepseek")
    assert md is not None, "真实 any-llm 应能取到 deepseek 的 ProviderMetadata"
    assert md.name == "deepseek"
    assert bool(md.streaming), "deepseek 支持流式（稳定事实，不随版本变）"
    assert bool(md.tier), "tier 应有值（verified/community，不钉死具体取值）"
    # 真元数据下，一个正常的 deepseek 配置必须放行（全程只读元数据，不发请求）
    assert B.precheck_capabilities({"provider": "deepseek", "api_key": "x", "stream": True}) == []


def test_precheck_never_touches_network(monkeypatch):
    """元数据是类属性，预检不得引出任何 socket：把 socket 打死再跑一遍真元数据路径。"""
    pytest.importorskip("any_llm")
    import socket

    def boom(*args, **kwargs):
        raise AssertionError("能力预检不得发起网络请求")

    monkeypatch.setattr(socket, "socket", boom)
    monkeypatch.setattr(socket, "create_connection", boom)
    assert B.provider_metadata("deepseek") is not None
    assert B.precheck_capabilities({"provider": "deepseek", "api_key": "x", "stream": True}) == []


def test_registered_capability_fields_exist_in_real_metadata():
    """CAPABILITY_KEYS 里登记的非空字段名必须真的存在于 ProviderMetadata（防拼写漂移）。"""
    pytest.importorskip("any_llm")
    from any_llm.types.provider import ProviderMetadata

    known = set(ProviderMetadata.model_fields)
    bad = {key: field for key, field in B.CAPABILITY_KEYS.items() if field and field not in known}
    assert not bad, f"CAPABILITY_KEYS 登记了 ProviderMetadata 不存在的字段：{bad}"
