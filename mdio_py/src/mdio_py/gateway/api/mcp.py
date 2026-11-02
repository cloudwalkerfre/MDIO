"""MCP endpoint — 与 /exec 平级的普通一员, 单文件 Controller。

双时代兼容(按 method 自然分流, 不看来源):
  legacy 客户端(2025-11-25)  → initialize 无会话握手 → tools/list → tools/call
  modern 客户端(2026-07-28)  → server/discover        → tools/list → tools/call
JSON-RPC 语义照官方 spec《Versioning and Compatibility》兼容矩阵。
"""
import os

from litestar import Controller, post
from litestar.di import Provide
from mdio_py.gateway.state import provide_runtime
from mdio_py import MdioRuntime
from mdio_py.runtime.worktree_paths import is_anchor_dir

EXEC_SCHEMA = {
    "type": "object",
    "properties": {
        "code": {"type": "string", "description": "MDIO pipeline expression"},
        "cwd": {"type": "string", "description": "harness 项目根目录（如 /path/to/myproj）；mdio 根 = <cwd>/.mdio（其下 wt/ 为文档根），首用自动建 <cwd>/.mdio/wt。匹配已有 worktree，未注册时注册新 worktree；省略则作用于 genesis"},
        "active_doc": {"type": "string", "description": "Focus document path, default '/'"},
    },
    "additionalProperties": False,
    "required": ["code"],
}

EXEC_DESC = (
    "Execute an MDIO pipeline over the document workspace. "
    "Examples: $ /docs/a.md::HEAD | /> /docs/b.md::TARGET | $ /docs/a.md::..*_h -pj tag. "
    "Writes persist only when the server runs with MDIO_PERSIST=1."
)

# 首次注册项目 wt 时自动带的子侧挂载：把 genesis 的 /docs/skills 只读映射进项目。
# 两段同目标：第 0 段恒绑 /home（Binding::parse 的硬规则），第 2 段才给出 /docs/skills 别名。
DEFAULT_RO_RULE = "/docs/skills:docs/skills:RO; /docs/skills:docs/skills:RO"


def _ensure_app_readme(root: str, project_name: str) -> None:
    """Create the wt-root README consumed by the UI's `# app` / tag filter.

    The new README rule is the source of a wt's display name and `tag: app`;
    without it the switcher falls back to the registered uid. Never overwrite
    a user-authored README.
    """
    path = os.path.join(root, "wt", "README.md")
    if os.path.exists(path):
        return
    name = " ".join(str(project_name or "").split()).strip() or "project"
    content = "# app\n\n- name: " + name + "\n- tag: app\n"
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(content)


def mdio_root_of(cwd: str) -> str:
    """harness 项目根 → mdio 项目根（= <项目根>/.mdio，缺则初始化）。

    幂等：已经是 .mdio 则原样返回（此时文档根 = <cwd>/wt，同样补建）。
    **不上溯**——上溯会命中 mdio HOME 自身。
    前置条件：cwd 必须是已存在的目录；只初始化 <cwd>/.mdio 与 <cwd>/.mdio/wt，
    绝不创建 cwd 本身（避免 cwd 写错时静默造出幽灵项目）。
    同时按新 README 规则补 wt/README.md（`# app` + name + `tag: app`）。
    """
    cwd = os.path.realpath(cwd)
    if not os.path.isdir(cwd):
        raise ValueError(f"cwd does not exist or is not a directory: {cwd}")
    if os.path.basename(cwd) == ".mdio":
        root = cwd
        project_name = os.path.basename(os.path.dirname(cwd))
    else:
        root = os.path.join(cwd, ".mdio")
        project_name = os.path.basename(cwd)
    # 两分支都保证文档根 <root>/wt 存在：_ensure_app_readme 要写 <root>/wt/README.md，
    # 而 cwd 已是 .mdio 时原代码不建 wt，会 FileNotFoundError 并让项目注册失败。
    os.makedirs(os.path.join(root, "wt"), exist_ok=True)
    _ensure_app_readme(root, project_name)
    return root


def _rpc(rid, result, modern=False):
    out = {"jsonrpc": "2.0", "id": rid}
    if result is not None:
        if modern and "resultType" not in result:
            # 2026-07-28 世代：结果必须自带 resultType（"缺省=complete" 只对更早世代成立）
            result = {**result, "resultType": "complete"}
        out["result"] = result
    return out

def _is_modern(params) -> bool:
    """请求是否声明 2026-07-28 世代（看 params._meta 的协议版本）；旧客户端 ⇒ False。"""
    meta = (params or {}).get("_meta") or {}
    ver = meta.get("io.modelcontextprotocol/protocolVersion") or ""
    return str(ver).startswith("2026-")

def _rpc_err(rid, code, message):
    return {"jsonrpc": "2.0", "id": rid, "error": {"code": code, "message": message}}

class MCPController(Controller):
    dependencies = {"runtime": Provide(provide_runtime, sync_to_thread=False)}

    @post("/mcp")
    async def mcp(self, data: dict, runtime: MdioRuntime) -> dict:
        method = data.get("method")
        rid = data.get("id")
        params = data.get("params") or {}
        modern = _is_modern(params)          # 2026-07-28 世代：结果需自带 resultType

        # 通知(无 id)一律 204, 不回应
        if rid is None:
            return None

        if method == "initialize":  # legacy 握手; 不带 Session-Id = 无会话模式
            return _rpc(rid, {
                "protocolVersion": "2025-11-25",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "mdio", "version": "0.8.5"},
            }, modern)

        if method == "server/discover":  # modern 自描述
            return _rpc(rid, {
                "supportedVersions": ["2026-07-28", "2025-11-25"],
                "capabilities": {"tools": {"listChanged": False}},
            }, modern)

        if method == "tools/list":
            return _rpc(rid, {
                "tools": [{"name": "exec", "description": EXEC_DESC, "inputSchema": EXEC_SCHEMA}],
                "ttlMs": 0, "cacheScope": "private",
            }, modern)

        if method == "tools/call":
            name = (params.get("name") or "")
            if name != "exec":
                return _rpc_err(rid, -32602, f"Unknown tool: {name}")
            args = params.get("arguments") or {}
            wt_uid = "genesis"
            is_new = False
            asked_cwd = bool(args.get("cwd"))
            if asked_cwd:
                try:
                    _cwd = os.path.realpath(args["cwd"])
                    if not os.path.isdir(_cwd):
                        return _rpc_err(rid, -32602, f"invalid cwd: not a directory: {_cwd}")
                    if not is_anchor_dir(_cwd):
                        # mdio 根 = <harness 项目根>/.mdio；首次注册自动带 genesis RO 子挂载
                        from mdio_py.gateway.worktree_api import dispatch_worktree
                        _proj = mdio_root_of(_cwd)
                        _rule = os.environ.get("MDIO_LOAD_RULE", DEFAULT_RO_RULE)
                        loaded = await dispatch_worktree(runtime, "worktree.load", {
                            "path": _proj,
                            "rule": _rule,
                            "name": os.path.basename(os.path.dirname(_proj)) or "default",
                            "parent": "genesis",
                        }, self_uid="genesis")
                        wt_uid, is_new = loaded["uid"], loaded["is_new"]
                except Exception as exc:
                    return _rpc_err(rid, -32602, f"invalid cwd: {exc}")
            try:
                text = await runtime.exec(
                    args["code"],
                    active_doc=args.get("active_doc") or "/",
                    wt_uid=wt_uid,
                )
            except Exception as exc:  # 业务错误照旧以文本回给模型(可自纠)
                text = f"- mdio_error: {exc}"
            content = [{"type": "text", "text": text}]
            if asked_cwd:
                # 回执：让调用方知道这次落在哪个 wt、是不是刚注册的（不影响首块文本）
                content.append({"type": "text",
                                "text": f"[worktree] uid={wt_uid} new={str(is_new).lower()}"})
            return _rpc(rid, {
                "content": content,
                "resultType": "complete",
            })

        return _rpc_err(rid, -32601, f"Method not found: {method}")
