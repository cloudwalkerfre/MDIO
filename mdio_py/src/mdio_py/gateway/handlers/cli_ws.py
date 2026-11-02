import json
from dataclasses import dataclass
from typing import Any
from uuid import uuid4

from litestar import Controller, WebSocket, websocket
from litestar.di import Provide

from mdio_py import MdioRuntime
from mdio_py.gateway.state import provide_runtime
from mdio_py.core.selector_builder import SelectorBuilder
from mdio_py.core.error import is_error_text


@dataclass
class CliSessionState:
    session_id: str
    wt_uid: str = "genesis"
    cwd: str = "/"
    active_doc: str | None = None


def _normalize_path(value: Any) -> str | None:
    if value is None:
        return None

    text = str(value).strip()
    if not text:
        return None
    if not text.startswith("/"):
        text = f"/{text}"
    return text


def _session_payload(session: CliSessionState) -> dict[str, Any]:
    return {
        "session_id": session.session_id,
        "wt_uid": session.wt_uid,
        "cwd": session.cwd,
        "active_doc": session.active_doc,
    }


def _root_path(value: Any) -> str:
    path = _normalize_path(value) or "/"
    return path


def _dir_path(value: Any) -> str:
    path = _root_path(value)
    return path if path == "/" or path.endswith("/") else f"{path}/"


def _bool_param(value: Any, default: bool) -> bool:
    if value is None:
        return default
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on"}
    return bool(value)


def _param(params: dict[str, Any], snake_key: str, camel_key: str) -> Any:
    if snake_key in params:
        return params[snake_key]
    return params.get(camel_key)


def _response(req_id: Any, ok: bool, result: Any = None, error: dict[str, Any] | None = None) -> str:
    payload: dict[str, Any] = {
        "kind": "res",
        "id": req_id,
        "ok": ok,
    }
    if ok:
        payload["result"] = result
    else:
        payload["error"] = error or {"type": "unknown_error", "message": "unknown error"}
    return json.dumps(payload, ensure_ascii=False)


def _event(event: str, data: Any) -> str:
    return json.dumps({
        "kind": "evt",
        "event": event,
        "data": data,
    }, ensure_ascii=False)


def _tree_entries(snapshot: list[dict[str, Any]], root_path: str, depth: int, include_hidden: bool) -> list[dict[str, Any]]:
    root_dir = _dir_path(root_path)
    root_is_root = root_dir == "/"
    seen: dict[str, dict[str, Any]] = {}

    for node in snapshot:
        file_path = _root_path(node.get("path"))
        if file_path == root_path and root_path != "/":
            name = file_path.rsplit("/", 1)[-1] or "/"
            seen[file_path] = {
                "path": file_path,
                "name": name,
                "kind": "file",
                "depth": 0,
            }
            continue

        if not root_is_root and not file_path.startswith(root_dir):
            continue

        rel_path = file_path[1:] if root_is_root else file_path[len(root_dir):]
        parts = [part for part in rel_path.split("/") if part]
        if not parts:
            continue

        hidden = any(part.startswith(".") for part in parts)
        if hidden and not include_hidden:
            continue

        for idx, part in enumerate(parts[:-1]):
            level = idx + 1
            if depth >= 0 and level > depth:
                break
            dir_path = f"{root_dir}{'/'.join(parts[:level])}/" if root_is_root else f"{root_dir}{'/'.join(parts[:level])}/"
            seen.setdefault(dir_path, {
                "path": dir_path,
                "name": part,
                "kind": "dir",
                "depth": level,
            })

        file_depth = len(parts)
        if depth >= 0 and file_depth > depth:
            continue
        seen[file_path] = {
            "path": file_path,
            "name": parts[-1],
            "kind": "file",
            "depth": file_depth,
        }

    return sorted(seen.values(), key=lambda entry: (entry["depth"], entry["path"]))


class CliWsController(Controller):
    dependencies = {"runtime": Provide(provide_runtime, sync_to_thread=False)}

    @websocket("/ws/cli")
    async def cli(
        self,
        socket: WebSocket,
        runtime: MdioRuntime,
    ) -> None:
        await socket.accept()
        session = CliSessionState(session_id=f"sess_{uuid4().hex[:10]}")

        while True:
            try:
                raw = await socket.receive_text()
            except Exception:
                break
            # WS 消息到达 litestar（拆 exec 旅程: JS 发送 → litestar 收到 → 处理 → 响应发出）
            from mdio_py.telemetry import _TelemetrySpan as _ws_span
            with _ws_span(runtime.store, "ws_recv"):
                try:
                    message = json.loads(raw)
                except json.JSONDecodeError:
                    await socket.send_text(_response(None, False, error={
                        "type": "bad_json",
                        "message": "payload must be valid JSON text",
                    }))
                    continue

                req_id = message.get("id")
                method = str(message.get("method", "")).strip()
                params = message.get("params") or {}

                if not isinstance(params, dict):
                    await socket.send_text(_response(req_id, False, error={
                        "type": "bad_request",
                        "message": "params must be an object",
                    }))
                    continue

                try:
                    if method == "hello":
                        await socket.send_text(_response(req_id, True, {
                            "protocol": "mdio.cli.v0",
                            "session": _session_payload(session),
                            "capabilities": [
                                "hello",
                                "ping",
                                "session.get",
                                "session.update",
                                "fs.tree",
                                "fs.read",
                                "exec.run",
                                "worktree.load",
                                "worktree.offload",
                                "worktree.list",
                                "worktree.current",
                            ],
                        }))

                    elif method == "ping":
                        await socket.send_text(_response(req_id, True, {"pong": True}))

                    elif method == "session.get":
                        await socket.send_text(_response(req_id, True, {
                            "session": _session_payload(session),
                        }))

                    elif method == "session.update":
                        wt_uid = _param(params, "wt_uid", "wtUid")
                        cwd = _param(params, "cwd", "cwd")
                        active_doc = _param(params, "active_doc", "activeDoc")

                        if wt_uid is not None:
                            session.wt_uid = str(wt_uid)
                        if cwd is not None:
                            session.cwd = _normalize_path(cwd) or "/"
                        if "active_doc" in params or "activeDoc" in params:
                            session.active_doc = _normalize_path(active_doc)

                        payload = {"session": _session_payload(session)}
                        await socket.send_text(_response(req_id, True, payload))
                        await socket.send_text(_event("session.changed", payload["session"]))

                    elif method.startswith("worktree."):
                        from mdio_py.gateway.worktree_api import dispatch_worktree
                        try:
                            result = await dispatch_worktree(
                                runtime, method, params, self_uid=session.wt_uid
                            )
                        except Exception as e:
                            await socket.send_text(_response(req_id, False, error={
                                "type": "worktree_error",
                                "message": str(e),
                            }))
                            continue
                        await socket.send_text(_response(req_id, True, result=result))

                    elif method == "exec.run":
                        from mdio_py.telemetry import _TelemetrySpan as _tel_span
                        with _tel_span(runtime.store, "cli_exec_run"):
                            code = str(params.get("code", "")).strip()
                            if not code:
                                await socket.send_text(_response(req_id, False, error={
                                    "type": "bad_request",
                                    "message": "exec.run requires non-empty params.code",
                                }))
                                continue

                            raw_result = await runtime.exec(
                                code,
                                active_doc=session.active_doc or "",
                                wt_uid=session.wt_uid,
                            )
                        result_text = str(raw_result)
                        ok = not is_error_text(result_text)
                        await socket.send_text(_response(req_id, ok, result={
                            "raw": result_text,
                            "status": "ok" if ok else "error",
                            "session": _session_payload(session),
                        }, error=None if ok else {
                            "type": "mdio_error",
                            "message": result_text,
                        }))

                    elif method == "fs.tree":
                        wt_uid = _param(params, "wt_uid", "wtUid")
                        path = _param(params, "path", "path")
                        depth = _param(params, "depth", "depth")
                        include_hidden = _param(params, "include_hidden", "includeHidden")

                        target_wt = str(wt_uid) if wt_uid is not None else session.wt_uid
                        target_path = _root_path(path)
                        target_depth = int(depth) if depth is not None else 3
                        show_hidden = _bool_param(include_hidden, True)

                        tree_bytes = runtime.store.get_vfs_tree(target_wt, show_hidden)
                        snapshot = json.loads(tree_bytes.decode("utf-8")) if tree_bytes else []
                        entries = _tree_entries(snapshot, target_path, target_depth, show_hidden)

                        await socket.send_text(_response(req_id, True, {
                            "wt_uid": target_wt,
                            "path": target_path,
                            "depth": target_depth,
                            "include_hidden": show_hidden,
                            "entries": entries,
                        }))

                    elif method == "fs.read":
                        wt_uid = _param(params, "wt_uid", "wtUid")
                        path = _param(params, "path", "path")
                        if not path:
                            await socket.send_text(_response(req_id, False, error={
                                "type": "bad_request",
                                "message": "fs.read requires params.path",
                            }))
                            continue

                        target_wt = str(wt_uid) if wt_uid is not None else session.wt_uid
                        target_path = _root_path(path)
                        exists = bool(runtime.store.path_exists(target_wt, target_path))
                        chunks = runtime._workspace.read_locator(
                            target_wt,
                            target_path,
                            SelectorBuilder().build(),
                        ) if exists else []
                        text = "\n".join(str(chunk) for chunk in chunks) if chunks else ""

                        await socket.send_text(_response(req_id, True, {
                            "wt_uid": target_wt,
                            "path": target_path,
                            "exists": exists,
                            "text": text,
                        }))

                    else:
                        await socket.send_text(_response(req_id, False, error={
                            "type": "unknown_method",
                            "message": f"unknown method: {method}",
                        }))
                except Exception as exc:
                    await socket.send_text(_response(req_id, False, error={
                        "type": "server_error",
                        "message": str(exc),
                    }))
