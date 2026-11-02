"""LLM provider 只读能力：元数据、PROVIDERS 默认值合并、api_key env 解析、余额适配器。

边界：只读；不写配置；不打印 api_key；任何失败都降级为结构化 payload，不让调用方炸。
元数据遍历必须逐 provider try/except：any-llm 的 get_all_provider_metadata() 遇到未装 SDK 会整批抛。
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import time
from datetime import datetime, timezone
from typing import Any

# 缓存 TTL（秒）
PROVIDER_CACHE_TTL = 300.0
BALANCE_OK_TTL = 60.0
BALANCE_ERR_TTL = 10.0

# @LLM 只读命令的保留 flag（llm.py 的命令入口据此分流）
RESERVED_FLAGS = ("-provider", "-balance", "-check", "-models")

CONFIG_PATH = "/.mds/llm_config.md"
PROVIDERS_TITLE = "PROVIDERS"

# ProviderMetadata 的能力位（字段表见 any_llm/types/provider.py）
CAPABILITY_FIELDS = (
    "streaming", "reasoning", "completion", "embedding", "moderation", "responses",
    "image", "pdf", "list_models", "messages", "messages_native", "batch_completion",
    "image_generation", "audio_transcription", "audio_speech", "rerank", "files",
    "file_operations",
)

_PROVIDER_CACHE: dict[str, Any] = {"at": 0.0, "payload": None}
_BALANCE_CACHE: dict[tuple, tuple[float, dict]] = {}


def _now() -> float:
    return time.monotonic()


def _anyllm():
    """惰性导入 any-llm；单测可 monkeypatch 本函数注入替身。"""
    from any_llm import AnyLLM
    return AnyLLM


# ── provider 元数据 ─────────────────────────────────────────────────────

def normalize_provider_id(name: str) -> str:
    """业务惯用名 → any-llm provider id（kimi→moonshot / glm→zai / google→gemini）。"""
    from .backend import normalize_provider
    return normalize_provider(name)


def _metadata(provider: str):
    """取单个 provider 的 ProviderMetadata；失败返回 None（永不抛）。"""
    try:
        cls = _anyllm().get_provider_class(normalize_provider_id(provider))
        return cls.get_provider_metadata()
    except Exception:
        return None


def _api_base_default(provider: str) -> str:
    try:
        return str(getattr(_anyllm().get_provider_class(normalize_provider_id(provider)), "API_BASE", "") or "")
    except Exception:
        return ""


def _provider_entry(provider: str) -> dict:
    """固定字段集；缺元数据时 available=False + error。"""
    md = _metadata(provider)
    if md is None:
        return {"name": provider, "available": False, "error": "metadata_unavailable"}
    entry = {
        "name": str(getattr(md, "name", provider) or provider),
        "class": str(getattr(md, "class_name", "") or ""),
        "tier": str(getattr(md, "tier", "") or ""),
        "doc_url": str(getattr(md, "doc_url", "") or ""),
        "env_key": str(getattr(md, "env_key", "") or ""),
        "env_api_base": str(getattr(md, "env_api_base", "") or ""),
        "api_base": _api_base_default(provider),
        "available": True,
        "capabilities": {},
    }
    for field in CAPABILITY_FIELDS:
        value = getattr(md, field, None)
        entry["capabilities"][field] = "unknown" if value is None else bool(value)
    return entry


def providers_payload() -> list[dict]:
    """全部 provider 元数据；进程缓存，不联网。"""
    now = _now()
    if _PROVIDER_CACHE["payload"] is not None and now - float(_PROVIDER_CACHE["at"]) < PROVIDER_CACHE_TTL:
        return _PROVIDER_CACHE["payload"]
    try:
        names = sorted(set(str(p) for p in _anyllm().get_supported_providers()))
    except Exception:
        return []
    payload = [_provider_entry(name) for name in names]
    _PROVIDER_CACHE["at"] = now
    _PROVIDER_CACHE["payload"] = payload
    return payload


def provider_payload(provider: str) -> dict | None:
    provider = str(provider or "").strip().lower()
    if not provider:
        return None
    for entry in providers_payload():
        if entry.get("name") == provider:
            return entry
    return _provider_entry(provider)


# ── PROVIDERS 段读取 / 合并 / api_key 解析 ──────────────────────────────

def read_provider_section(workspace, provider: str) -> dict:
    """读 llm_config.md 的 PROVIDERS.<provider>；缺失/异常返回空 dict。"""
    provider = str(provider or "").strip()
    if not workspace or not provider:
        return {}
    try:
        chunks = workspace.read_section_chunks("genesis", CONFIG_PATH, f"{PROVIDERS_TITLE}.{provider}")
        text = "\n".join(str(c) for c in (chunks or []) if c)
        if not text.strip():
            return {}
        from mdio_py.core import ast_utils
        env = json.loads(workspace.to_json_text(text))
        cfg = ast_utils.bullets_to_kv(env)
    except Exception:
        return {}
    if not isinstance(cfg, dict):
        return {}
    if "extra_body" in cfg:
        cfg["extra_body"] = _parse_json_obj(cfg["extra_body"])
    return cfg


def _parse_json_obj(value: Any) -> dict:
    if isinstance(value, dict):
        return value
    try:
        parsed = json.loads(str(value))
        return parsed if isinstance(parsed, dict) else {}
    except Exception:
        return {}


def resolve_api_key(cfg: dict) -> tuple[dict, str]:
    """解析 api_key：env:NAME → 环境变量；空 → missing；其它按字面量。

    返回 (新配置, source)；source ∈ literal|env|missing。密钥值从不打印。
    """
    out = dict(cfg or {})
    raw = out.get("api_key")
    if raw is None or str(raw).strip() == "":
        return out, "missing"
    text = str(raw).strip()
    if text.startswith("env:"):
        name = text[4:].strip()
        value = os.environ.get(name, "") if name else ""
        if value:
            out["api_key"] = value
            return out, "env"
        out.pop("api_key", None)
        return out, "missing"
    return out, "literal"


def api_key_source(workspace, alias_cfg: dict | None) -> str:
    """凭据来源（literal|env|missing）；只做判定，不返回解析后的密钥值。"""
    from .backend import infer_provider

    cfg = dict(alias_cfg or {})
    provider = str(cfg.get("provider") or "").strip().lower()
    if not provider:
        provider = str(infer_provider(cfg.get("base_url") if isinstance(cfg.get("base_url"), str) else None) or "")
    if provider and not str(cfg.get("api_key") or "").strip():
        defaults = read_provider_section(workspace, provider)
        if defaults:
            merged = dict(defaults)
            merged.update(cfg)
            cfg = merged
    _resolved, source = resolve_api_key(cfg)
    return source


def merge_alias_config(workspace, alias_cfg: dict | None) -> dict:
    """alias 配置 ← PROVIDERS.<provider> 默认值（alias 覆盖）。

    合并顺序：PROVIDERS 默认值 → LLM_CONFIG.<alias>。
    provider 解析：alias.provider > infer_provider(alias.base_url)。
    """
    from .backend import infer_provider

    cfg = dict(alias_cfg or {})
    provider = str(cfg.get("provider") or "").strip().lower()
    if not provider:
        provider = str(infer_provider(cfg.get("base_url") if isinstance(cfg.get("base_url"), str) else None) or "")
    if provider:
        defaults = read_provider_section(workspace, provider)
        if defaults:
            merged = dict(defaults)
            merged.update(cfg)
            cfg = merged
        cfg.setdefault("provider", provider)
    if "extra_body" in cfg:
        cfg["extra_body"] = _parse_json_obj(cfg["extra_body"])
    cfg, _source = resolve_api_key(cfg)
    return cfg


# ── 余额适配器 ─────────────────────────────────────────────────────────

def _balance_cache_key(provider: str, api_base: str, api_key: str) -> tuple:
    digest = hashlib.sha1((api_key or "").encode("utf-8")).hexdigest()
    return (provider, api_base, digest)


async def _http_get(url: str, headers: dict, timeout: float, proxy: str | None):
    """单测可 monkeypatch 的 HTTP 入口；返回 (status_code, text)。"""
    import httpx
    kwargs: dict[str, Any] = {"headers": headers, "timeout": timeout}
    if proxy:
        kwargs["proxy"] = proxy
    async with httpx.AsyncClient(**kwargs) as client:
        resp = await client.get(url)
        return resp.status_code, resp.text


def _balance_root(api_base: str, provider: str) -> str:
    base = str(api_base or "").strip() or _api_base_default(provider)
    base = re.sub(r"/v1/?$", "", base.rstrip("/"))
    return base


async def _deepseek_balance(provider: str, api_key: str, api_base: str, proxy: str | None, timeout: float) -> dict:
    from .backend import make_backend

    url = _balance_root(api_base, provider) + "/user/balance"
    headers = {"Authorization": "Bearer " + api_key} if api_key else {}
    try:
        status, text = await _http_get(url, headers, timeout, proxy)
    except Exception as exc:                        # noqa: BLE001 - 任何网络失败都归一
        kind = make_backend().normalize(exc).kind
        return {"supported": True, "provider": provider, "error_kind": kind}
    if status != 200:
        kind = "auth" if status == 401 else "forbidden" if status == 403 else "rate_limit" if status == 429 else "server" if status >= 500 else "invalid_request"
        return {"supported": True, "provider": provider, "error_kind": kind, "http_status": status}
    try:
        data = json.loads(text)
    except Exception:
        return {"supported": True, "provider": provider, "error_kind": "invalid_request", "http_status": status}
    balances = []
    for item in data.get("balance_infos") or []:
        if not isinstance(item, dict):
            continue
        balances.append({
            "currency": str(item.get("currency", "") or ""),
            "total_balance": str(item.get("total_balance", "") or ""),
            "granted_balance": str(item.get("granted_balance", "") or ""),
            "topped_up_balance": str(item.get("topped_up_balance", "") or ""),
        })
    return {
        "supported": True,
        "provider": provider,
        "is_available": bool(data.get("is_available")),
        "balances": balances,
        "fetched_at": datetime.now(timezone.utc).astimezone().isoformat(timespec="seconds"),
    }


BALANCE_ADAPTERS = {"deepseek": _deepseek_balance}


async def fetch_balance(*, provider: str, api_key: str = "", api_base: str = "",
                        proxy: str | None = None, timeout: float = 20.0) -> dict:
    """余额（结构化 payload）；成功 60s / 失败 10s 缓存。不支持的 provider 给 doc_url。"""
    provider = normalize_provider_id(provider)
    if not provider:
        return {"supported": False, "error_kind": "config", "message": "provider required"}
    adapter = BALANCE_ADAPTERS.get(provider)
    if adapter is None:
        meta = provider_payload(provider) or {}
        return {"supported": False, "provider": provider, "doc_url": meta.get("doc_url", "")}
    key = _balance_cache_key(provider, api_base or "", api_key or "")
    now = _now()
    hit = _BALANCE_CACHE.get(key)
    if hit is not None and now < hit[0]:
        return hit[1]
    payload = await adapter(provider, api_key or "", api_base or "", proxy, float(timeout or 20.0))
    ttl = BALANCE_ERR_TTL if payload.get("error_kind") else BALANCE_OK_TTL
    _BALANCE_CACHE[key] = (now + ttl, payload)
    return payload


# ── 模型列表（P2）─────────────────────────────────────────────────────

async def list_models(*, provider: str, api_key: str = "", api_base: str = "",
                       proxy: str | None = None, timeout: float = 20.0) -> dict:
    """provider 的模型 id 列表；失败给 error_kind（不抛）。"""
    provider = normalize_provider_id(provider)
    if not provider:
        return {"supported": False, "error_kind": "config"}
    try:
        client_args: dict[str, Any] = {}
        if proxy:
            import httpx
            client_args["http_client"] = httpx.AsyncClient(proxy=proxy)
        client = _anyllm().create(provider, api_key=api_key or None,
                                  api_base=api_base or None,
                                  unified_exceptions=True, **client_args)
        items = await client.alist_models()
        return {"supported": True, "provider": provider,
                "models": [str(getattr(item, "id", item)) for item in (items or [])]}
    except Exception as exc:                        # noqa: BLE001
        from .backend import make_backend
        return {"supported": True, "provider": provider,
                "error_kind": make_backend().normalize(exc).kind}
