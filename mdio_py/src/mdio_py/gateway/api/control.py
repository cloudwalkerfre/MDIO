"""ControlController — 运行时控制面。

所有状态都以 genesis 的 /.mds/runtime.md 为准；本 controller 只做薄封装。
"""
from __future__ import annotations

from litestar import Controller, Request, get, post
from litestar.di import Provide

from mdio_py import MdioRuntime
from mdio_py.gateway.dtos import (
    CommandRequest,
    CommandResponse,
    GatewayShareRequest,
)
from mdio_py.gateway.state import (
    attach_gateway,
    detach_gateway,
    provide_runtime,
    runtime_md_read,
    runtime_md_set,
    start_shutdown,
)


class ControlController(Controller):
    path = "/control"
    dependencies = {"runtime": Provide(provide_runtime, sync_to_thread=False)}

    @get("/status")
    async def status(self, request: Request, runtime: MdioRuntime) -> dict:
        fields = runtime_md_read(runtime._workspace)
        stopping = fields.get("status") == "stopping"
        wts = [w[0] for w in runtime.store.list_worktrees()]
        gateway_enabled = fields.get("gateway_enabled") == "true"
        # ── 长驻任务与补丁桥的读数 ──────────────────────────────────
        # 判"服务在不在干活"不能只看进程活着：**桥停摆时 exec / WS 全好，只是没有补丁**
        #（"文档全白而 exec 全好"是这种停摆的现场）。上游停发的证据在 `ast_bridge.enqueued`
        # 不再增长，下游卡住的证据在 `backlog/stalled`；`tasks.*` 是两条监护壳的重起次数与最后一条错。
        st = request.app.state
        _bridge = getattr(st, "ast_bridge", None)
        _sups = getattr(st, "task_supervisors", None) or {}
        _tasks = {k: v.snapshot() for k, v in _sups.items()}
        return {
            "running": not stopping,
            "stopping": stopping,
            "tasks": _tasks,
            "ast_bridge": _bridge.health() if _bridge is not None else None,
            "pid": int(fields["pid"]) if fields.get("pid", "").isdigit() else None,
            "mode": fields.get("mode", ""),
            "root": fields.get("root", ""),
            "host": fields.get("host", ""),
            "port": int(fields["port"]) if fields.get("port", "").isdigit() else None,
            "gateway_enabled": gateway_enabled,
            "wts": wts,
        }

    @get("/wt/chain")
    async def wt_chain(self, runtime: MdioRuntime, uid: str = "genesis") -> dict:
        """只读：wt uid → ui 作用域链 `[自己, 父, …]`（子覆父）+ 各根的盘上 ui 目录。

        与 `ui_dirs_for_wt` 同源（同一条 parent_uid 遍历）：客户端 ui: 解析按本条依次试
        `wtui_<uid>/<path>`，不另造拓扑；取件的 404/200 语义仍由 /ui-assets 那条链决定。
        """
        from mdio_py.gateway.config import RUNTIME_ROOT
        from mdio_py.runtime.worktree_paths import _local_scope_roots, ui_dirs_for_wt

        chain = [uid]
        seen = {uid}
        cur = runtime.store.parent_uid(uid)
        while cur and cur not in seen and len(chain) < 8:
            chain.append(cur)
            seen.add(cur)
            cur = runtime.store.parent_uid(cur)
        projects = {u: p for u, _n, _a, p in runtime.store.list_worktrees() if p}
        for k, v in _local_scope_roots(RUNTIME_ROOT).items():
            projects.setdefault(k, v)
        root = projects.get(uid)
        roots = ui_dirs_for_wt(uid, root, runtime.store.parent_uid) if root else []
        return {"wt_uid": uid, "chain": chain, "roots": roots}

    @post("/stop", status_code=200)
    async def stop(self, request: Request, runtime: MdioRuntime) -> dict:
        try:
            runtime_md_set(runtime._workspace, "status", "stopping")
        except Exception:
            pass
        start_shutdown(runtime, request.app.state)
        return {"status": "stopping"}

    @post("/gateway/on", status_code=200)
    async def gateway_on(self, request: Request, runtime: MdioRuntime) -> dict:
        fields = runtime_md_read(runtime._workspace)
        if fields.get("gateway_enabled") == "true":
            return {"status": "enabled"}
        await attach_gateway(runtime, request.app.state)
        try:
            runtime_md_set(runtime._workspace, "gateway_enabled", "true")
        except Exception:
            pass
        return {"status": "enabled"}

    @post("/gateway/off", status_code=200)
    async def gateway_off(self, request: Request, runtime: MdioRuntime) -> dict:
        try:
            runtime_md_set(runtime._workspace, "gateway_enabled", "false")
        except Exception:
            pass
        await detach_gateway(runtime, request.app.state)
        return {"status": "disabled"}

    @get("/gateway/status")
    async def gateway_status(self, request: Request, runtime: MdioRuntime) -> dict:
        fields = runtime_md_read(runtime._workspace)
        gateway_store = getattr(request.app.state, "gateway_store", None)
        return {
            "enabled": fields.get("gateway_enabled") == "true",
            "actives": list(gateway_store._actives.keys()) if gateway_store else [],
            "scopes": list(gateway_store._scopes.keys()) if gateway_store else [],
        }

    @post("/exec", status_code=200)
    async def exec_pipeline(self, data: CommandRequest, runtime: MdioRuntime) -> CommandResponse:
        try:
            result = await runtime.exec(
                data.code,
                active_doc=data.active_doc,
                wt_uid=data.wt_uid,
            )
            return CommandResponse(status="ok", result=str(result))
        except Exception as e:
            return CommandResponse(status="error", result=str(e))

    @post("/share", status_code=200)
    async def share(self, request: Request, data: GatewayShareRequest) -> dict:
        ep = getattr(request.app.state, "iroh_endpoint", None)
        if ep is None:
            return {"error": "iroh not available"}
        from mdio_py.gateway.api.gateway import iroh_share
        try:
            result = await iroh_share(
                runtime=request.app.state.runtime,
                wt_uid=data.wt_uid,
                ep=ep,
                path=data.path,
                name=data.name,
            )
            return result
        except Exception as e:
            return {"error": str(e)}
