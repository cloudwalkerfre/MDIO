from litestar import Controller, Request, post
from litestar.di import Provide
from mdio_py.gateway.dtos import CommandRequest, CommandResponse
from mdio_py.gateway.state import provide_runtime
from mdio_py import MdioRuntime
from mdio_py.gateway.worktree_api import dispatch_worktree

class ExecController(Controller):
    dependencies = {"runtime": Provide(provide_runtime, sync_to_thread=False)}

    @post("/worktree")
    async def worktree_rpc(self, request: Request, runtime: MdioRuntime) -> dict:
        try:
            data = await request.json()
        except Exception as e:
            return {"ok": False, "error": {"message": f"invalid request: {e}"}}
        rid = data.get("id")
        method = str(data.get("method") or "")
        params = data.get("params") or {}
        if not method.startswith("worktree."):
            return {"id": rid, "ok": False, "error": {"message": "worktree method required"}}
        try:
            result = await dispatch_worktree(runtime, method, params, self_uid="genesis")
        except Exception as e:
            return {"id": rid, "ok": False, "error": {"message": str(e)}}
        return {"id": rid, "ok": True, "result": result}

    @post("/exec")
    async def execute_command(
        self,
        data: CommandRequest,
        runtime: MdioRuntime
    ) -> CommandResponse:
        try:
            result = await runtime.exec(
                data.code, 
                active_doc=data.active_doc, 
                wt_uid=data.wt_uid          # 使用 uid 字符串句柄
            )
            return CommandResponse(status="ok", result=str(result))
        except Exception as e:
            return CommandResponse(status="error", result=str(e))