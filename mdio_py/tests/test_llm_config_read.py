"""LLM 配置读取：整节一次读出（tojson 投影）与逐键读取 oracle 的对拍。

判据：同一份配置下两条路径键值相同；白名单外的键保留（交给能力预检），
无冒号 bullet 必须被忽略（行解析的忽略规则）。
"""

import json

import pytest

from mdio_py import MdioRuntime
from mdio_py.core.selector_builder import by_tag
from mdio_py.plugins.llm.llm import LLMPlugin

CONFIG = """# LLM_CONFIG

## ALPHA

- model: alpha-model
- base_url: https://api.example.com/v1
- api_key: sk-test-123
- extra_body: {"thinking": {"type": "enabled"}}
- provider: deepseek
- backend: anyllm
- timeout: 30
- custom_capability: on
- plain text no colon

## BETA

- model: beta-model
- reasoning_effort: high
"""

# 逐键读取的键表
OLD_KEYS = ("model", "base_url", "api_key", "proxy", "extra_body", "reasoning_effort",
            "provider", "backend", "timeout")


async def _old_read(workspace, alias: str) -> dict:
    """oracle：逐键 by_tag 读取。"""
    config = {}
    for key in OLD_KEYS:
        try:
            chunks = workspace.read_locator(
                "genesis", "/.mds/llm_config.md", by_tag(f"LLM_CONFIG.{alias}", key))
        except Exception:
            continue
        if not chunks:
            continue
        val = str(chunks[0]).strip()
        if key == "extra_body":
            try:
                val = json.loads(val)
            except Exception:
                val = {}
        config[key] = val
    return config


@pytest.fixture
async def rt_cfg(tmp_path):
    (tmp_path / "wt" / ".mds").mkdir(parents=True, exist_ok=True)
    (tmp_path / "wt" / ".mds" / "llm_config.md").write_text(CONFIG, encoding="utf-8")
    rt = MdioRuntime(str(tmp_path))
    yield rt
    rt.close()


async def test_whole_section_read_matches_per_key_read(rt_cfg):
    """对拍：整节读取 ⊇ 旧逐键读取，且公共键取值逐一致。"""
    ws = rt_cfg._workspace
    new = await LLMPlugin()._get_god_secrets(ws, "ALPHA")
    old = await _old_read(ws, "ALPHA")
    assert old, "oracle 至少应读到一部分键（夹具/选择器失效）"
    for k, v in old.items():
        assert k in new, f"整节读取丢了键 {k}"
        assert new[k] == v, f"{k}: 新={new[k]!r} 旧={v!r}"


async def test_extra_keys_kept_and_plain_bullet_ignored(rt_cfg):
    """白名单外的键保留；无冒号 bullet（t 为空）被忽略。"""
    ws = rt_cfg._workspace
    new = await LLMPlugin()._get_god_secrets(ws, "ALPHA")
    assert new.get("custom_capability") == "on"
    assert "plain text no colon" not in new
    assert not any(k.strip() == "" for k in new)
    assert isinstance(new.get("extra_body"), dict)


async def test_second_alias_reads_independently(rt_cfg):
    """每个别名各自成一个字典，不串味。"""
    ws = rt_cfg._workspace
    beta = await LLMPlugin()._get_god_secrets(ws, "BETA")
    assert beta.get("model") == "beta-model"
    assert beta.get("reasoning_effort") == "high"
    assert "provider" not in beta


async def test_missing_alias_returns_empty(rt_cfg):
    ws = rt_cfg._workspace
    assert await LLMPlugin()._get_god_secrets(ws, "NOPE") == {}


# 能力预检接线（advisory，不阻断）

def _fresh_precheck():
    from mdio_py.plugins.llm import llm as llm_mod
    llm_mod._PRECHECK_DONE.clear()
    return llm_mod._precheck_config


def test_precheck_flags_unknown_key_and_memoizes():
    precheck = _fresh_precheck()
    secrets = {"model": "m1", "provider": "deepseek", "custom_capability": "on"}
    msgs = precheck(secrets)
    assert any("custom_capability" in m for m in msgs), msgs
    # 同一份配置第二次直接返回空（不重复提示）
    assert precheck(secrets) == []


def test_precheck_never_raises_on_garbage():
    precheck = _fresh_precheck()
    assert isinstance(precheck({}), list)
    assert isinstance(precheck({"model": None, "provider": 123}), list)


def test_precheck_registered_keys_pass():
    """已登记的核心键不应产生「未知键」类消息。"""
    precheck = _fresh_precheck()
    msgs = precheck({"model": "m1", "provider": "deepseek", "backend": "anyllm",
                     "reasoning_effort": "high"})
    assert not any("未知配置键" in m for m in msgs), msgs

