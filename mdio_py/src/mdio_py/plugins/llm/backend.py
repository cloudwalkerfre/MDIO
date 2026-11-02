"""@LLM 后端适配层：库相关的一切都关在这个文件里。

两条硬约定：
1. `llm.py` 只从本模块 import，且文件内不出现任何库名——可 grep 守：
       grep -n "litellm\\|any_llm" mdio_py/src/mdio_py/plugins/llm/llm.py   # 应为 0
2. 错误一律先 `normalize()` 成 `LLMErrorInfo` 再用；**不要依赖 `str(exc)`**——
   库的 `__str__` 会读自己一堆属性，属性缺失时会自己抛异常（实测踩到）。

本模块只保留 any-llm 后端。
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import Any, AsyncIterator, Callable, Literal, Protocol

_log = logging.getLogger("mdio.llm.backend")

# ── 归一化错误 kind（12 个）─────────────────────────────────────────────
KIND_AUTH = "auth"
KIND_FORBIDDEN = "forbidden"
KIND_NOT_FOUND = "not_found"
KIND_RATE_LIMIT = "rate_limit"
KIND_INVALID_REQUEST = "invalid_request"
KIND_CONTEXT_LENGTH = "context_length"
KIND_CONTENT_FILTER = "content_filter"
KIND_INSUFFICIENT_FUNDS = "insufficient_funds"
KIND_TIMEOUT = "timeout"
KIND_CONNECTION = "connection"
KIND_SERVER = "server"
KIND_CONFIG = "config"
KIND_UNKNOWN = "unknown"

# 只有这四类可重试（其余一律立即失败，避免把编程错误/权限错误重试放大）
RETRYABLE_KINDS = frozenset({KIND_RATE_LIMIT, KIND_SERVER, KIND_TIMEOUT, KIND_CONNECTION})

# original_exception 的类名 → 超时 / 连接（跨库统一，来自实测）
_TIMEOUT_ORIGINALS = frozenset({
    "APITimeoutError", "TimeoutException", "ReadTimeout", "ConnectTimeout", "Timeout", "TimeoutError",
})
_CONN_ORIGINALS = frozenset({
    "APIConnectionError", "ConnectError", "ConnectionError", "RemoteProtocolError", "InternalServerError",
})

# base_url 主机名 → any-llm 的 provider key（不在表里的走 create_openai_compatible）
_HOST_PROVIDERS = (
    ("deepseek.com", "deepseek"),
    ("openrouter.ai", "openrouter"),
)

# 业务侧惯用名 → any-llm provider id（配置/界面可写 kimi/glm/google）。
PROVIDER_ALIASES = {
    "kimi": "moonshot",
    "glm": "zai",
    "google": "gemini",
}


def normalize_provider(name: str | None) -> str:
    key = str(name or "").strip().lower()
    return PROVIDER_ALIASES.get(key, key)


class LLMBackendError(Exception):
    """适配层自身的错误（缺依赖、未知后端名、缺 provider 等）。走 normalize 时归 config。"""


def safe_msg(e: Any) -> str:
    """从异常取消息，绝不调用 `str(e)`（理由见模块 docstring）。"""
    if isinstance(e, str):
        return e
    for attr in ("message", "detail"):
        v = getattr(e, attr, None)
        if isinstance(v, str) and v:
            return v
    args = getattr(e, "args", None)
    if args:
        try:
            return str(args[0])
        except Exception:
            pass
    return type(e).__name__


# 思考文本的拼写：any-llm 自己的常量就是这四个（any_llm/constants.py: REASONING_FIELD_NAMES），
# 另外它还有一个 Reasoning 模型，暴露成 `reasoning`（序列化为字符串）与 `reasoning.content`（类型化访问）。
# 只读 `reasoning_content` 会在别的 provider 上把思考**静默丢掉**。
_REASONING_FIELDS = ("reasoning_content", "thinking", "think", "chain_of_thought")


def reasoning_text(obj: Any) -> str:
    """从 delta / message 取思考文本，兼容四种拼写 + `reasoning` 字符串/对象两种形态。"""
    if obj is None:
        return ""
    for name in _REASONING_FIELDS:
        v = getattr(obj, name, None)
        if isinstance(v, str) and v:
            return v
    r = getattr(obj, "reasoning", None)
    if isinstance(r, str) and r:
        return r
    content = getattr(r, "content", None)
    if isinstance(content, str) and content:
        return content
    if isinstance(r, dict) and isinstance(r.get("content"), str) and r["content"]:
        return r["content"]
    # pydantic 的额外字段（部分 provider 的透传字段落在这里）
    extra = getattr(obj, "model_extra", None)
    if isinstance(extra, dict):
        for name in _REASONING_FIELDS + ("reasoning",):
            v = extra.get(name)
            if isinstance(v, str) and v:
                return v
            if isinstance(v, dict) and isinstance(v.get("content"), str) and v["content"]:
                return v["content"]
            inner = getattr(v, "content", None)
            if isinstance(inner, str) and inner:
                return inner
    return ""


# ── 数据结构 ───────────────────────────────────────────────────────────

@dataclass(frozen=True)
class LLMErrorInfo:
    kind: str
    message: str
    status: int | None = None
    param: str | None = None
    code: str | None = None
    provider: str | None = None
    original: BaseException | None = None
    retryable: bool = False


@dataclass(frozen=True)
class ToolCallData:
    index: int | None = None
    id: str | None = None
    name: str | None = None
    arguments: str = ""


@dataclass(frozen=True)
class LLMEvent:
    kind: Literal["thinking", "content", "tool", "error"]
    text: str = ""
    tool: ToolCallData | None = None
    error: LLMErrorInfo | None = None


@dataclass(frozen=True)
class LLMResponse:
    content: str = ""
    reasoning: str = ""
    tool_calls: list[ToolCallData] = field(default_factory=list)


@dataclass(frozen=True)
class LLMRequest:
    model: str
    messages: list[dict]
    api_key: str = ""
    api_base: str | None = None
    provider: str | None = None
    extra_body: dict = field(default_factory=dict)
    reasoning_effort: str | None = None
    tools: list[dict] | None = None
    stream: bool = False
    timeout: float | None = 180.0
    proxy: str | None = None


class LLMBackend(Protocol):
    name: str

    async def stream(self, req: LLMRequest) -> AsyncIterator[LLMEvent]: ...
    async def complete(self, req: LLMRequest) -> LLMResponse: ...
    def normalize(self, exc: BaseException) -> LLMErrorInfo: ...


def _mk_error(kind: str, exc: Any, *, status: int | None = None, param: str | None = None,
              code: str | None = None, provider: str | None = None) -> LLMErrorInfo:
    return LLMErrorInfo(
        kind=kind, message=safe_msg(exc), status=status, param=param, code=code,
        provider=provider, original=exc if isinstance(exc, BaseException) else None,
        retryable=kind in RETRYABLE_KINDS,
    )


def _classify_by_status(status: int | None, exc: Any, msg: str, *, param=None, code=None,
                        provider=None) -> LLMErrorInfo:
    """状态码优先的分类。status 缺失时由调用方先用 original 类型补齐。"""
    if status == 401:
        kind = KIND_AUTH
    elif status == 403:
        kind = KIND_FORBIDDEN
    elif status == 404:
        kind = KIND_NOT_FOUND
    elif status == 429:
        kind = KIND_RATE_LIMIT
    elif status == 402:
        kind = KIND_INSUFFICIENT_FUNDS
    elif status in (400, 422):
        low = msg.lower()
        if "context" in low and ("length" in low or "token" in low):
            kind = KIND_CONTEXT_LENGTH
        elif "content" in low and ("filter" in low or "policy" in low or "blocked" in low):
            kind = KIND_CONTENT_FILTER
        else:
            kind = KIND_INVALID_REQUEST
    elif status is not None and 500 <= status < 600:
        kind = KIND_SERVER
    else:
        kind = KIND_UNKNOWN
    return _mk_error(kind, exc, status=status, param=param, code=code, provider=provider)


def infer_provider(api_base: str | None, configured: str | None = None) -> str | None:
    """provider 解析：配置里写了就用；否则按 base_url 主机名推断；都不是 → None（走自定义端点）。"""
    if configured:
        return normalize_provider(configured)
    base = (api_base or "").lower()
    for host, provider in _HOST_PROVIDERS:
        if host in base:
            return provider
    return None


# ── 能力预检：配置键 = 可预检的能力键 ─────────────────────────────────
#
# 政策：当前后端没有 drop_params 的静默丢弃——非推理模型收到 reasoning_effort
# 会直接本地报错。
# 因此配置里出现的每个键都必须落进一个已知抽屉，落不进去就当场点名，而不是等第一次真
# 调用才炸：
#   ① CORE_KEYS       直接进请求，不需要能力位
#   ② CAPABILITY_KEYS 有对应 ProviderMetadata 字段，可按元数据判定支持与否
#   ③ 其余            「未知键」——白名单外的键一律点名
# 预检本身只在配置变更路径上跑；元数据取不到（provider 名不受支持 / SDK 缺失）一律退化
# 为 None，绝不让预检变成新的失败点。

# ① 直接进请求的键：有值就发，不查元数据
CORE_KEYS: frozenset[str] = frozenset({
    "model", "base_url", "api_key", "proxy", "timeout", "provider", "backend",
})

# ② 配置键 → ProviderMetadata 字段名（字段表见 any_llm/types/provider.py）。
#    值为 "" 表示已登记但没有能力位可判：原样透传，预检放行。
CAPABILITY_KEYS: dict[str, str] = {
    "reasoning_effort": "reasoning",
    "thinking": "reasoning",
    "stream": "streaming",
    "extra_body": "",
    "completion": "completion",
    "embedding": "embedding",
    "moderation": "moderation",
    "responses": "responses",
    "image": "image",
    "pdf": "pdf",
    "files": "files",
    "file_operations": "file_operations",
    "rerank": "rerank",
    "image_generation": "image_generation",
    "audio_transcription": "audio_transcription",
    "audio_speech": "audio_speech",
    "batch_completion": "batch_completion",
    "messages": "messages",
    "messages_native": "messages_native",
    "list_models": "list_models",
}

# getattr 的「字段不存在」哨兵：与「字段存在但为假」区分开，后者才报能力不匹配
_MISSING = object()


def _metadata_field(md: Any, field: str) -> Any:
    """读一个元数据字段；字段缺失或读取抛异常 → `_MISSING`（无从判定，宁可不报也不误报）。"""
    try:
        return getattr(md, field, _MISSING)
    except Exception:                               # noqa: BLE001 - 元数据替身/代理属性可能抛
        return _MISSING


def provider_metadata(provider: str | None) -> Any | None:
    """取单个提供者的 any-llm `ProviderMetadata`；**任何失败一律 None，永不抛异常**。

    - 惰性导入：本函数被调用前不 import any_llm，不给普通 import 路径加负担
    - provider 为空 / 名字不受支持 / 对应 SDK 未装（`get_provider_class` 抛 ImportError）→ None
    - 不做缓存：预检只在配置变更路径上跑一次，缓存会带来跨用例的陈旧值
    """
    try:
        key = normalize_provider(provider)
        if not key:
            return None
        from any_llm import AnyLLM
        return AnyLLM.get_provider_class(key).get_provider_metadata()
    except Exception as e:                          # noqa: BLE001 - 预检不得成为失败点
        _log.debug("provider_metadata(%r) 不可得：%s", provider, e)
        return None


def precheck_capabilities(secrets: dict, provider: str | None = None) -> list[str]:
    """配置键能力预检：返回人类可读消息列表，**空列表 = 通过**；本函数永不抛异常。

    逐键判定（键名升序，消息顺序稳定）：
    1. 能力不匹配——配了能力键（真值）而元数据对应字段为假：点名 键名/提供者/能力名；
    2. 未知键——既不在 `CORE_KEYS` 也不在 `CAPABILITY_KEYS`：一律点名；
    3. 缺 api_key——元数据带 `env_key`：提示可用哪个环境变量（从不打印任何密钥值）。

    元数据取不到（None）时：只做第 2 类判定，另给一条「能力元数据不可得」提示，
    第 1、3 类跳过。provider 解析优先级：函数入参 `provider` > 配置里的 `provider` 键 >
    按 `base_url` 主机名推断（后两步即 `infer_provider`）。
    值为假的能力键（`stream: false`）算显式关闭，不判为「配置了能力」。
    """
    messages: list[str] = []
    try:
        if secrets is None:
            cfg: dict = {}
        elif isinstance(secrets, dict):
            cfg = secrets
        else:
            messages.append(f"能力预检提示：配置不是字典（{type(secrets).__name__}），已按空配置处理")
            cfg = {}

        base_url = cfg.get("base_url")
        cfg_provider = cfg.get("provider")
        prov = str(provider or "").strip().lower() or infer_provider(
            base_url if isinstance(base_url, str) else None,
            cfg_provider if isinstance(cfg_provider, str) else None)
        md = provider_metadata(prov)

        if md is None:
            messages.append(
                f"能力元数据不可得：provider={prov or '未指定'} 的 ProviderMetadata 取不到"
                f"（provider 名不受 any-llm 支持，或对应 SDK 未安装）；"
                f"本次只做未知键判定，能力判定已跳过")

        for key in sorted(cfg, key=str):
            field = CAPABILITY_KEYS.get(key)
            if field is None:
                if key not in CORE_KEYS:
                    messages.append(
                        f"未知配置键 {key!r}：既不是请求直通键（CORE_KEYS），也未登记为能力键"
                        f"（CAPABILITY_KEYS）；请改用已登记键，或在 CAPABILITY_KEYS 里登记")
                continue
            if md is None or not field or not cfg.get(key):
                continue                            # 元数据不可得 / 无能力位 / 显式关闭
            value = _metadata_field(md, field)
            if value is _MISSING:
                continue                            # 该元数据对象没有此字段 → 无从判定
            if not value:
                messages.append(
                    f"能力不匹配：配置键 {key!r} 需要提供者 {prov!r} 的 {field} 能力，"
                    f"但元数据 {field}={value!r}（该提供者不支持）")

        if md is not None and not cfg.get("api_key"):
            env_key = _metadata_field(md, "env_key")
            if env_key and env_key is not _MISSING:
                messages.append(
                    f"未配置 api_key：提供者 {prov!r} 可用环境变量 {env_key} 提供密钥"
                    f"（消息从不包含密钥值）")
    except Exception as e:                          # noqa: BLE001 - 预检永不抛
        _log.warning("precheck_capabilities 内部异常，已降级：%s", e)
    return messages


# ── any-llm 后端 ───────────────────────────────────────────────────────

class AnyLLMBackend:
    """any-llm 后端。要点（全部实测）：

    - provider 必填（配置或按 base_url 推断）；未知主机走 `create_openai_compatible`
    - 实例级 `unified_exceptions=True`（对流式迭代中的错误同样生效）
    - `client_args={"max_retries": 0}` 关掉 openai SDK 的隐藏重试（否则 429/5xx 每次实发 3 个请求）
    - 代理只能经 `client_args={"http_client": httpx.AsyncClient(proxy=...)}`
    - 实例按 (provider, api_base, api_key, proxy) 缓存复用（每次新建会丢连接复用）
    """

    name = "anyllm"
    _clients: dict[tuple, Any] = {}

    @classmethod
    def _client(cls, req: LLMRequest):
        try:
            from any_llm import AnyLLM
        except Exception as e:                     # pragma: no cover
            raise LLMBackendError(f"any_llm 未安装: {e}") from e

        provider = infer_provider(req.api_base, req.provider)
        key = (provider, req.api_base, req.api_key, req.proxy)
        client = cls._clients.get(key)
        if client is not None:
            return client

        client_args: dict[str, Any] = {"max_retries": 0}
        if req.proxy:
            import httpx
            client_args["http_client"] = httpx.AsyncClient(proxy=req.proxy)
        if provider:
            client = AnyLLM.create(provider, api_key=req.api_key or None,
                                   api_base=req.api_base or None,
                                   unified_exceptions=True, **client_args)
        else:
            # 不在支持列表里的 OpenAI 兼容端点（ZenMux 之类）
            client = AnyLLM.create_openai_compatible(
                name="mdio-custom", api_base=req.api_base, api_key=req.api_key or None,
                unified_exceptions=True, **client_args)
        cls._clients[key] = client
        return client

    def _kwargs(self, req: LLMRequest, stream: bool) -> dict:
        kw: dict[str, Any] = {"model": req.model, "messages": req.messages, "stream": stream}
        if req.tools:
            kw["tools"] = req.tools
        if req.extra_body:
            kw["extra_body"] = dict(req.extra_body)
        if req.reasoning_effort:
            kw["reasoning_effort"] = req.reasoning_effort
        if req.timeout:
            kw["timeout"] = req.timeout
        return kw

    async def stream(self, req: LLMRequest) -> AsyncIterator[LLMEvent]:
        try:
            client = self._client(req)
            stream = await client.acompletion(**self._kwargs(req, True))
        except Exception as e:                     # noqa: BLE001 - 统一走 normalize
            yield LLMEvent(kind="error", error=self.normalize(e))
            return
        tool_slots: dict[int, dict] = {}
        try:
            async for chunk in stream:
                delta = chunk.choices[0].delta
                rc = reasoning_text(delta)
                if rc:
                    yield LLMEvent(kind="thinking", text=rc)
                if getattr(delta, "content", None):
                    yield LLMEvent(kind="content", text=delta.content)
                for tc in (getattr(delta, "tool_calls", None) or []):
                    idx = getattr(tc, "index", 0) or 0
                    slot = tool_slots.setdefault(idx, {"id": None, "name": None, "args": ""})
                    if getattr(tc, "id", None):
                        slot["id"] = tc.id
                    fn = getattr(tc, "function", None)
                    if fn is not None:
                        if getattr(fn, "name", None):
                            slot["name"] = fn.name
                        if getattr(fn, "arguments", None):
                            slot["args"] += fn.arguments
        except Exception as e:                     # noqa: BLE001 - 流中断也归一化
            yield LLMEvent(kind="error", error=self.normalize(e))
            return
        if tool_slots:
            idx = min(tool_slots)                  # 只取第一个 tool call
            s = tool_slots[idx]
            yield LLMEvent(kind="tool", tool=ToolCallData(index=idx, id=s["id"], name=s["name"],
                                                          arguments=s["args"]))

    async def complete(self, req: LLMRequest) -> LLMResponse:
        client = self._client(req)
        r = await client.acompletion(**self._kwargs(req, False))
        msg = r.choices[0].message
        tools: list[ToolCallData] = []
        for tc in (getattr(msg, "tool_calls", None) or []):
            fn = getattr(tc, "function", None)
            tools.append(ToolCallData(index=getattr(tc, "index", None), id=getattr(tc, "id", None),
                                      name=getattr(fn, "name", None) if fn else None,
                                      arguments=(getattr(fn, "arguments", "") or "") if fn else ""))
        return LLMResponse(content=(getattr(msg, "content", "") or ""),
                           reasoning=reasoning_text(msg),
                           tool_calls=tools)

    def normalize(self, exc: BaseException) -> LLMErrorInfo:
        if isinstance(exc, LLMBackendError):
            return _mk_error(KIND_CONFIG, exc)
        # 标准库 / httpx 的瞬态错误：任何 provider 侧都一样，先按类型判
        try:
            import httpx
            if isinstance(exc, (httpx.TimeoutException, TimeoutError)):
                return _mk_error(KIND_TIMEOUT, exc)
            if isinstance(exc, (httpx.HTTPError, ConnectionError)):
                return _mk_error(KIND_CONNECTION, exc)
        except Exception:                          # pragma: no cover
            pass
        status = getattr(exc, "status_code", None)
        code = getattr(exc, "code", None)
        param = getattr(exc, "param", None)
        err_type = getattr(exc, "error_type", None)
        provider = getattr(exc, "provider_name", None)
        orig = getattr(exc, "original_exception", None)
        orig_name = type(orig).__name__ if orig is not None else ""

        try:
            from any_llm import exceptions as X
        except Exception:                          # pragma: no cover - 依赖缺失
            X = None

        if X is not None and isinstance(exc, getattr(X, "AnyLLMError", ())):
            # 配置类：fail-fast，永不重试
            for cls_name in ("MissingApiKeyError", "UnsupportedProviderError", "UnsupportedParameterError"):
                cls = getattr(X, cls_name, None)
                if cls is not None and isinstance(exc, cls):
                    return _mk_error(KIND_CONFIG, exc, status=status, code=code, param=param,
                                     provider=provider)
            if isinstance(exc, getattr(X, "AuthenticationError", ())):
                kind = KIND_FORBIDDEN if status == 403 or code == "permission_denied" \
                    or err_type == "permission_error" else KIND_AUTH
                return _mk_error(kind, exc, status=status, code=code, param=param, provider=provider)
            if isinstance(exc, getattr(X, "ModelNotFoundError", ())):
                return _mk_error(KIND_NOT_FOUND, exc, status=status or 404, code=code, param=param,
                                 provider=provider)
            if isinstance(exc, getattr(X, "RateLimitError", ())):
                return _mk_error(KIND_RATE_LIMIT, exc, status=status or 429, code=code, param=param,
                                 provider=provider)
            if isinstance(exc, getattr(X, "ContextLengthExceededError", ())):
                return _mk_error(KIND_CONTEXT_LENGTH, exc, status=status, code=code, param=param,
                                 provider=provider)
            if isinstance(exc, getattr(X, "ContentFilterError", ())):
                return _mk_error(KIND_CONTENT_FILTER, exc, status=status, code=code, param=param,
                                 provider=provider)
            if isinstance(exc, getattr(X, "InsufficientFundsError", ())):
                return _mk_error(KIND_INSUFFICIENT_FUNDS, exc, status=status or 402, code=code,
                                 param=param, provider=provider)
            # 无状态码时先看 original 类型（超时/连接），再看 5xx
            if status is None and orig_name in _TIMEOUT_ORIGINALS:
                return _mk_error(KIND_TIMEOUT, exc, status=status, code=code, param=param,
                                 provider=provider)
            if status is None and orig_name in _CONN_ORIGINALS:
                return _mk_error(KIND_CONNECTION, exc, status=status, code=code, param=param,
                                 provider=provider)
        # 非 AnyLLMError：pydantic ValidationError / TypeError 等本地错误 → fail-fast
        if orig_name in _TIMEOUT_ORIGINALS:
            return _mk_error(KIND_TIMEOUT, exc, status=status, code=code, param=param, provider=provider)
        if isinstance(exc, (ValueError, TypeError)) or type(exc).__name__ == "ValidationError":
            return _mk_error(KIND_CONFIG, exc, status=status, code=code, param=param, provider=provider)
        return _classify_by_status(status, exc, safe_msg(exc), param=param, code=code, provider=provider)


# ── 工厂 ───────────────────────────────────────────────────────────────

_BACKENDS: dict[str, Callable[[], LLMBackend]] = {
    "anyllm": AnyLLMBackend,
}

DEFAULT_BACKEND = "anyllm"


def make_backend(name: str | None = None) -> LLMBackend:
    """后端只有 any-llm。"""
    key = (name or DEFAULT_BACKEND).strip().lower() or DEFAULT_BACKEND
    if key in ("any-llm", "any_llm"):
        key = "anyllm"
    factory = _BACKENDS.get(key)
    if factory is None:
        raise LLMBackendError(f"未知后端 {name!r}；可选：{sorted(_BACKENDS)}（litellm 已退役）")
    return factory()
