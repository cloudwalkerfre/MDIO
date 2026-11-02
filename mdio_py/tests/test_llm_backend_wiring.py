"""接线上断言：后端不依赖 litellm（环境与依赖中均不得出现该库名）。

这组用例防回归：把某个 LLM 库 import 回插件、或把后端接错，这里会直接失败。
"""

import importlib.util
import sys
from pathlib import Path

from mdio_py.plugins.llm import backend as B
from mdio_py.plugins.llm.llm import _backend_for, _is_retryable

LLM_PY = Path(__file__).resolve().parent.parent / "src" / "mdio_py" / "plugins" / "llm" / "llm.py"


def test_default_backend_is_anyllm():
    """默认后端就是 any-llm；工厂产出的实例类型正确。"""
    b = _backend_for(None)
    assert b.name == "anyllm", b.name
    assert isinstance(b, B.AnyLLMBackend)
    assert B.DEFAULT_BACKEND == "anyllm"


def test_litellm_is_fully_retired():
    """litellm 不得可导入、不得出现在依赖声明中。"""
    assert importlib.util.find_spec("litellm") is None, "litellm 仍可导入，退役不彻底"
    root = Path(__file__).resolve().parents[2]        # tests -> mdio_py -> repo root
    for rel in ("pyproject.toml", "mdio_py/pyproject.toml"):
        for line in (root / rel).read_text(encoding="utf-8").splitlines():
            s = line.strip()
            if "litellm" not in s:
                continue
            code_part = s.split("#", 1)[0]
            assert "litellm" not in code_part, f"{rel} 里 litellm 仍是依赖/代码: {s}"


def test_plugin_has_no_library_names():
    """llm.py 里不得出现任何库名（适配层是唯一边界）。"""
    src = LLM_PY.read_text(encoding="utf-8")
    for name in ("litellm", "any_llm", "import aiohttp"):
        assert name not in src, f"llm.py 出现库名: {name}"


def test_unknown_backend_is_config_error():
    """未注册的后端名 → config 类错误（fail-fast，不重试）。"""
    try:
        B.make_backend("nope")
    except B.LLMBackendError as e:
        info = B.AnyLLMBackend().normalize(e)
        assert info.kind == B.KIND_CONFIG and not info.retryable
    else:
        raise AssertionError("未知后端竟然没报错")


def test_normalize_survives_missing_attrs():
    """归一化不能依赖 str(exc)：属性缺失的异常也要能分类（库的真实 __str__ 会自己炸）。"""
    class Weird(Exception):
        pass

    e = Weird()
    info = B.AnyLLMBackend().normalize(e)
    assert info.kind in (B.KIND_UNKNOWN, B.KIND_SERVER, B.KIND_CONFIG), info.kind
    assert isinstance(info.message, str)

    # 内置瞬态错误仍按类型判（与旧行为一致）
    assert _is_retryable(TimeoutError("t"))
    assert _is_retryable(ConnectionError("c"))
    assert not _is_retryable(ValueError("v"))


def test_async_subagent_call_detection():
    """父 agent 卡 WAIT_TOOL 的防护：异步子 agent 判定必须是结构判据。

    按子串判断（如 `".run" not in pipeline`）会把 `$ /logs/..run_x` 这类管道误判成异步调用，
    于是不写 tool_result，父 agent 停在 WAIT_TOOL。
    """
    from mdio_py.plugins.llm.llm import _is_async_llm_call

    assert _is_async_llm_call("@LLM.sub.run")
    assert _is_async_llm_call('"hi" | @LLM.sub.run')
    assert _is_async_llm_call("@LLM.sub.stream")
    assert _is_async_llm_call('@LLM.sub.run -cb "x"')

    assert not _is_async_llm_call("$ /logs/..run_x")
    assert not _is_async_llm_call('"please run it" | @MDIO.echo')
    assert not _is_async_llm_call("@LLM.sandbox.init")
    assert not _is_async_llm_call("")


def test_reasoning_field_portability():
    """思考文本的拼写不止一种：any-llm 自己的常量列了 4 个字段名（reasoning_content/
    thinking/think/chain_of_thought），另有 Reasoning 模型暴露成 reasoning 与 reasoning.content。

    只读 reasoning_content 会在别的 provider 上把思考**静默丢掉**（插件会写出空 thinking 记录）。
    """
    from mdio_py.plugins.llm.backend import reasoning_text

    class A:
        reasoning_content = "A"

    class B:
        reasoning = "B"

    class C:
        class R:
            content = "C"
        reasoning = R()

    class D:
        thinking = "D"

    class E:
        think = "E"

    class F:
        chain_of_thought = "F"

    class G:
        model_extra = {"reasoning_content": "G"}

    class H:
        model_extra = {"reasoning": {"content": "H"}}

    class I:
        pass

    assert [reasoning_text(x()) for x in (A, B, C, D, E, F, G, H)] == list("ABCDEFGH")
    assert reasoning_text(I()) == ""
    assert reasoning_text(None) == ""


def test_agent_level_overrides_reach_the_request():
    """agent 级 provider/backend/timeout/proxy 必须真的并进请求（否则配置是空转）。"""
    from mdio_py.plugins.llm.llm import _merge_request_overrides

    merged = _merge_request_overrides(
        {"model": "m", "api_key": "k", "provider": "deepseek", "backend": "anyllm"},
        {"provider": "openrouter", "timeout": "30", "sysp": "x"})
    assert merged["provider"] == "openrouter"          # agent 级优先
    assert merged["timeout"] == "30"
    assert merged["backend"] == "anyllm"               # 未覆盖的保持别名值
    assert "sysp" not in merged                        # 非请求项不得混进 secrets

    # 空值不得覆盖别名（配置里写了 key 但没填值）
    keep = _merge_request_overrides({"provider": "deepseek"},
                                    {"provider": "", "backend": None, "proxy": ""})
    assert keep["provider"] == "deepseek"
    assert "proxy" not in keep
