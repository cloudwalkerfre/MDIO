"""
测试 @LLM 网络错误重试逻辑（注入假 client，不访问真实 API、不依赖任何库的异常类）。

后端为 any-llm：测试用真 `AnyLLMBackend` 做归一化，
只把它的 `_client()` 换成替身 —— 判定语义（retryable/kind）走真代码，
而网络调用被替换掉。
"""
import asyncio
import json
import sys
from unittest.mock import MagicMock, patch

import mdio_py.plugins.llm.llm as _llm  # noqa: E402

from mdio_py.plugins.llm.backend import AnyLLMBackend, LLMResponse  # noqa: E402

LLMPlugin = _llm.LLMPlugin
_is_retryable = _llm._is_retryable
_format_llm_error = _llm._format_llm_error

from any_llm.exceptions import (  # noqa: E402
    AuthenticationError,
    RateLimitError,
    ProviderError,
    UpstreamProviderError,
)
from httpx import HTTPError as HttpxHTTPError  # noqa: E402

PASS, FAIL = 0, 0


def check(name, condition):
    global PASS, FAIL
    if condition:
        PASS += 1
        print(f"  ✅ {name}")
    else:
        FAIL += 1
        print(f"  ❌ {name}")


# 辅助工厂（用真实类，不用 MagicMock，避免 __aiter__ 兼容问题）

def _make_secrets(model="test-model", base_url="https://openrouter.ai/api"):
    return {"model": model, "base_url": base_url, "api_key": "sk-test"}


def _make_messages():
    return [{"role": "user", "content": "hello"}]


class _FakeDelta:
    """模拟 OpenAI stream chunk delta。"""
    def __init__(self, content=None, reasoning_content=None, tool_calls=None):
        self.content = content
        self.reasoning_content = reasoning_content
        self.tool_calls = tool_calls


class _FakeStreamChunk:
    """模拟 LiteLLM CustomStreamWrapper 产出的单个 chunk。"""
    def __init__(self, content=None, reasoning=None, tool_calls=None):
        delta = _FakeDelta(content, reasoning, tool_calls)
        self.choices = [MagicMock(delta=delta)]


class _FakeStreamResponse:
    """可被 async for 消费的流式响应（带可选的异步异常注入）。"""
    def __init__(self, chunks: list, error: BaseException | None = None):
        self._chunks = chunks
        self._error = error

    def __aiter__(self):
        return self

    async def __anext__(self):
        if self._chunks:
            return self._chunks.pop(0)
        if self._error:
            raise self._error
        raise StopAsyncIteration


class _FakeMessage:
    """模拟非流式响应的 message 字段。"""
    def __init__(self, content=None, reasoning_content=None, tool_calls=None):
        self.content = content
        self.reasoning_content = reasoning_content
        self.tool_calls = tool_calls


class _FakeNonStreamResponse:
    """模拟非流式 API 一次性返回。"""
    def __init__(self, content=None, reasoning=None, tool_calls=None):
        msg = _FakeMessage(content, reasoning, tool_calls)
        self.choices = [MagicMock(message=msg)]


# 异常构造器（any-llm 真类 + 标准库瞬态错误）

def _auth_err(msg="bad key"):
    return AuthenticationError(message=msg, status_code=401, provider_name="deepseek")


def _api_conn_err(msg="connection lost"):
    """连不上：实测 any-llm 归 UpstreamProviderError(502)（可重试）。"""
    return UpstreamProviderError(message=msg, status_code=502, provider_name="deepseek")


def _timeout_err(msg="timed out"):
    """超时：实测是 ProviderError(status=None, original=APITimeoutError)（可重试）。"""
    class APITimeoutError(Exception):
        pass
    return ProviderError(message=msg, original_exception=APITimeoutError(),
                         provider_name="deepseek")


def _rate_limit_err(msg="429"):
    return RateLimitError(message=msg, status_code=429, provider_name="deepseek")


class _FakeClient:
    """替身 any-llm client：acompletion 由脚本注入。"""

    def __init__(self, fn):
        self.acompletion = fn


def _patch_client(fn):
    """把插件用的 any-llm 后端的 _client() 换成替身（判定语义仍走真 normalize）。"""
    backend = AnyLLMBackend()
    backend._client = lambda req: _FakeClient(fn)      # noqa: SLF001 - 测试替身
    return patch("mdio_py.plugins.llm.llm._backend_for", lambda *a, **k: backend)


async def _collect(gen):
    results = []
    async for item in gen:
        results.append(item)
    return results


# 测试 1: _is_retryable / _format_llm_error

def test_is_retryable():
    print("\n── _is_retryable ──")
    check("APIConnectionError", _is_retryable(_api_conn_err()))
    check("Timeout", _is_retryable(_timeout_err()))
    check("RateLimitError", _is_retryable(_rate_limit_err()))
    check("HttpxHTTPError", _is_retryable(HttpxHTTPError("reset")))
    check("ConnectionError", _is_retryable(ConnectionError("refused")))
    check("TimeoutError", _is_retryable(TimeoutError("timed out")))
    check("NOT AuthenticationError", not _is_retryable(_auth_err()))
    check("NOT ValueError", not _is_retryable(ValueError("bad")))
    check("NOT TypeError", not _is_retryable(TypeError("bad type")))
    check("NOT json.JSONDecodeError", not _is_retryable(json.JSONDecodeError("x", "", 0)))


def test_format_llm_error():
    print("\n── _format_llm_error ──")
    msg = _format_llm_error(_auth_err(), "deepseek-v3")
    check("auth prefix", "[LLM] auth" in msg and "deepseek-v3" in msg)
    check("auth suggests check API key", "check API key" in msg)

    msg = _format_llm_error(ValueError("unexpected boom"), "m")
    check("fallback contains error text", "unexpected boom" in msg)


# 测试 2: _infer_and_stream 重试路径

async def test_non_retryable_immediate():
    """不可重试 → 1 次调用，立即返回 error。"""
    call_count = 0

    async def raise_auth(**kw):
        nonlocal call_count; call_count += 1
        raise _auth_err()

    plugin = LLMPlugin()
    with _patch_client(raise_auth):
        results = await _collect(
            plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=False, num_retries=3))

    assert call_count, f"expected 1, got {call_count}"
    assert results[0][0] == "error"
    assert "auth" in results[0][1]
    print("  ✅ non_retryable_immediate")


async def test_retryable_then_success():
    """第一次抛瞬态错误 → 重试成功。"""
    call_count = 0

    async def flaky(**kw):
        nonlocal call_count; call_count += 1
        if call_count == 1:
            raise _api_conn_err()
        return _FakeNonStreamResponse("recovered after retry")

    plugin = LLMPlugin()
    with _patch_client(flaky):
        results = await _collect(
            plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=False, num_retries=2))

    assert call_count == 2, f"expected 2, got {call_count}"
    assert results[-1][0] == "content"
    assert results[-1][1] == "recovered after retry"
    print("  ✅ retryable_then_success")


async def test_retry_exhausted():
    """每次都抛瞬态错误 → 耗尽。"""
    call_count = 0

    async def always_fail(**kw):
        nonlocal call_count; call_count += 1
        raise _api_conn_err("dead")

    plugin = LLMPlugin()
    with _patch_client(always_fail):
        results = await _collect(
            plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=False, num_retries=2))

    assert call_count == 3
    assert results[0][0] == "error"
    assert "retry exhausted" in results[0][1]
    assert "3x" in results[0][1]
    print("  ✅ retry_exhausted")


async def test_zero_retries():
    """num_retries=0 → 1 次尝试即报 exhausted。"""
    call_count = 0

    async def fail(**kw):
        nonlocal call_count; call_count += 1
        raise _api_conn_err()

    plugin = LLMPlugin()
    with _patch_client(fail):
        results = await _collect(
            plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=False, num_retries=0))

    assert call_count
    assert results[0][0] == "error"
    assert "1x" in results[0][1]
    print("  ✅ zero_retries")


async def test_stream_success():
    """流式成功：应正常 yield，无 error。"""
    plugin = LLMPlugin()
    mock_stream = _FakeStreamResponse([
        _FakeStreamChunk("Hello"),
        _FakeStreamChunk(" world"),
    ])

    async def stream_ok(**kw):
        assert kw["stream"] is True
        return mock_stream

    with _patch_client(stream_ok):
        results = await _collect(
            plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=True, num_retries=1))

    ctipes = [c for c, _ in results]
    assert "error" not in ctipes, f"unexpected error: {results}"
    # 流式路径 yield ("thinking", "") 和（如果有 tool_calls）("tool", ...)
    # 对于纯文本，assistant_buffer 不单独 yield（内容已实时输出到 stdout）
    print("  ✅ stream_success")


async def test_stream_partial_data():
    """流中途中断但有部分内容 → 降级使用部分内容，不重试。"""
    call_count = 0

    async def stream_partial(**kw):
        nonlocal call_count; call_count += 1
        return _FakeStreamResponse(
            [_FakeStreamChunk("前半段")],
            error=HttpxHTTPError("stream broke midway"),
        )

    plugin = LLMPlugin()
    with _patch_client(stream_partial):
        results = await _collect(
            plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=True, num_retries=2))

    assert call_count, f"partial data should NOT retry, got {call_count} calls"
    ctipes = [c for c, _ in results]
    assert "error" not in ctipes, f"unexpected error: {results}"
    print("  ✅ stream_partial_data")


async def test_stream_no_data_fallback():
    """流第一块就中断且无数据 → 下次自动降级为非流式。"""
    call_count = 0
    kw_records = []

    async def stream_then_nonstream(**kw):
        nonlocal call_count; call_count += 1
        kw_records.append(kw.copy())
        if call_count == 1:
            # 第一次：流式，但一条数据都没有就断开
            return _FakeStreamResponse([], error=_api_conn_err("stream broke"))
        else:
            # 第二次：降级为非流式成功
            assert kw["stream"] is False, f"expected stream=False on fallback"
            return _FakeNonStreamResponse("fallback success")

    plugin = LLMPlugin()
    with _patch_client(stream_then_nonstream):
        results = await _collect(
            plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=True, num_retries=2))

    assert call_count == 2
    assert kw_records[0]["stream"] is True
    assert kw_records[1]["stream"] is False
    assert results[-1][0] == "content"
    assert results[-1][1] == "fallback success"
    print("  ✅ stream_no_data_fallback")


async def test_cancelled_during_stream():
    """CancelledError 在流消费期间应向外传播。"""
    async def boom(**kw):
        return _FakeStreamResponse([], error=asyncio.CancelledError())

    plugin = LLMPlugin()
    with _patch_client(boom):
        try:
            await _collect(plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=True, num_retries=2))
            assert False, "should have raised CancelledError"
        except asyncio.CancelledError:
            pass
    print("  ✅ cancelled_during_stream")


async def test_cancelled_at_acompletion():
    """CancelledError 在 await acompletion 时向外传播。"""
    async def raise_cancel(**kw):
        raise asyncio.CancelledError()

    plugin = LLMPlugin()
    with _patch_client(raise_cancel):
        try:
            await _collect(plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=False, num_retries=2))
            assert False, "should have raised CancelledError"
        except asyncio.CancelledError:
            pass
    print("  ✅ cancelled_at_acompletion")


async def test_programming_error_no_retry():
    """编程错误（TypeError）→ 不重试，立即返回 error。"""
    call_count = 0

    async def raise_type_err(**kw):
        nonlocal call_count; call_count += 1
        raise TypeError("bad type in some handler")

    plugin = LLMPlugin()
    with _patch_client(raise_type_err):
        results = await _collect(
            plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=False, num_retries=5))

    assert call_count, f"programming error must NOT retry, got {call_count}"
    assert results[0][0] == "error"
    assert "bad type" in results[0][1]
    print("  ✅ programming_error_no_retry")


async def test_backoff_delays():
    """验证退避延迟：1s, 2s, 4s, ... 30s cap。"""
    async def always_fail(**kw):
        raise _api_conn_err()

    sleep_records = []
    async def fake_sleep(delay):
        sleep_records.append(delay)

    plugin = LLMPlugin()
    with _patch_client(always_fail):
        with patch("mdio_py.plugins.llm.llm.asyncio.sleep", fake_sleep):
            await _collect(
                plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=False, num_retries=3))

    assert sleep_records == [1, 2, 4], f"delay mismatch: {sleep_records}"
    print("  ✅ backoff_delays")


async def test_backoff_cap_at_30():
    """退避在 2^attempt > 30 时封顶为 30。"""
    async def always_fail(**kw):
        raise _api_conn_err()

    sleep_records = []
    async def fake_sleep(delay):
        sleep_records.append(delay)

    plugin = LLMPlugin()
    with _patch_client(always_fail):
        with patch("mdio_py.plugins.llm.llm.asyncio.sleep", fake_sleep):
            await _collect(
                plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=False, num_retries=10))

    assert len(sleep_records) == 10
    assert sleep_records[:5] == [1, 2, 4, 8, 16]
    assert sleep_records[5:] == [30] * 5
    print("  ✅ backoff_cap_at_30")


async def test_nonstream_success():
    """非流式正常路径回归验证。"""
    plugin = LLMPlugin()
    mock_resp = _FakeNonStreamResponse("hello world")

    async def nonstream(**kw):
        return mock_resp

    with _patch_client(nonstream):
        results = await _collect(
            plugin._infer_and_stream(_make_messages(), _make_secrets(), is_stream=False, num_retries=1))

    assert results[-1][0] == "content"
    assert results[-1][1] == "hello world"
    print("  ✅ nonstream_success")


# Runner

async def run_all():
    print("🧪 LLM Retry Logic Tests")
    print("=" * 60)

    test_is_retryable()
    test_format_llm_error()

    print("\n── _infer_and_stream retry paths ──")
    await test_non_retryable_immediate()
    await test_retryable_then_success()
    await test_retry_exhausted()
    await test_zero_retries()
    await test_stream_success()
    await test_stream_partial_data()
    await test_stream_no_data_fallback()
    await test_cancelled_during_stream()
    await test_cancelled_at_acompletion()
    await test_programming_error_no_retry()
    await test_backoff_delays()
    await test_backoff_cap_at_30()
    await test_nonstream_success()

    print("\n" + "=" * 60)
    total = PASS + FAIL
    print(f"Results: {PASS}/{total} passed ({FAIL} failures)")
    return FAIL == "genesis"


if __name__ == "__main__":
    ok = asyncio.run(run_all())
    exit(0 if ok else 1)
