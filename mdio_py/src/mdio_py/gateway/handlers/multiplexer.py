from typing import Any
from litestar import Controller, WebSocket, websocket
from litestar.channels import ChannelsPlugin
from litestar.di import Provide
from litestar.datastructures import State
from mdio_py.gateway.state import provide_runtime, MdioRuntime
from mdio_py.gateway.handlers.crdt import handle_crdt_sync


async def provide_wt_runtimes(state: State) -> dict:
    return getattr(state, "wt_runtimes", {})


async def provide_gateway_store(state: State):
    return getattr(state, "gateway_store", None)


class GatewayMultiplexer(Controller):
    dependencies = {
        "runtime": Provide(provide_runtime, sync_to_thread=False),
        "wt_runtimes": Provide(provide_wt_runtimes),
        "gateway_store": Provide(provide_gateway_store),
    }

    @websocket("/ws/gateway")
    async def gateway(
        self,
        socket: WebSocket,
        channels: ChannelsPlugin,
        runtime: MdioRuntime,
        wt_runtimes: dict,
    ) -> None:
        await socket.accept()
        # 确保 genesis 有 scope
        if "genesis" not in wt_runtimes:
            from mdio_py.gateway.crdt_scope import CrdtScope
            wt_runtimes["genesis"] = CrdtScope("genesis")
        # 无 wt 的端点：**不传连接 wt**——订阅不校验可达性、exec 仍用载荷里的显式 wt；
        # 带 wt 的端点走 /ws/gateway/{wt}。
        await handle_crdt_sync(socket, channels, runtime, wt_runtimes, None)

    @websocket("/ws/gateway/{wt_uid:str}")
    async def gateway_wt(
        self,
        socket: WebSocket,
        channels: ChannelsPlugin,
        runtime: MdioRuntime,
        wt_runtimes: dict,
        gateway_store: Any,
        wt_uid: str,
    ) -> None:
        """按 WT 路由的 WS 端点：/ws/gateway/{wt_uid}。

        **按需建 scope**：不用 has_scope 当门槛（它问的是「_scopes 里有没有它」，
        不是「wt 是否存在」，且看不到未挂载的 wt）。只在**格式非法**时拒（1008）。
        连接的 wt 由此确定，并一路传给 exec 与树。"""
        if not wt_uid or "/" in wt_uid or len(wt_uid) > 64:
            await socket.accept()
            await socket.close(1008, f"bad wt: {wt_uid}")
            return
        await socket.accept()
        if gateway_store is not None:
            gateway_store._get_or_create_scope(wt_uid)   # 按需建（幂等）
        await handle_crdt_sync(socket, channels, runtime, wt_runtimes, wt_uid)
