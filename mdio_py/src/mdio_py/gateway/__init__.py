import os
from pathlib import Path
from litestar import Litestar
from litestar.plugins.pydantic import PydanticPlugin
from litestar.channels import ChannelsPlugin
from litestar.channels.backends.memory import MemoryChannelsBackend
from litestar.config.cors import CORSConfig
from litestar.static_files import create_static_files_router
from litestar.middleware.base import MiddlewareProtocol
from litestar.types import ASGIApp, Scope, Receive, Send

# 模块级引用，供 lifespan 注入到 workspace
_channels_plugin = None

from mdio_py.gateway.api.exec import ExecController
from mdio_py.gateway.api.control import ControlController
from mdio_py.gateway.api.mcp import MCPController
from mdio_py.gateway.api.fs import FileSystemController
from mdio_py.gateway.api.gateway import IrohGatewayController
from mdio_py.gateway.api.telemetry import TelemetryController
from mdio_py.gateway.api.ui_assets import UiAssetsController
from mdio_py.gateway.handlers.cli_ws import CliWsController
from mdio_py.gateway.handlers.multiplexer import GatewayMultiplexer

from mdio_py.gateway.state import lifespan
from mdio_py.gateway.config import settings

# --- COOP/COEP Middleware ---
class SecurityHeadersMiddleware(MiddlewareProtocol):
    def __init__(self, app: ASGIApp):
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        async def send_wrapper(message):
            if message["type"] == "http.response.start":
                # 添加 WASM 必需的安全头
                headers = message.setdefault("headers", [])
                headers.append((b"Cross-Origin-Opener-Policy", b"same-origin"))
                headers.append((b"Cross-Origin-Embedder-Policy", b"require-corp"))
            await send(message)

        await self.app(scope, receive, send_wrapper)


def create_app() -> Litestar:
    cors_config = CORSConfig(allow_origins=["*"]) # 开发模式
    from importlib.resources import files as _resource_files
    try:
        packaged_ui = Path(str(_resource_files("mdio_py"))) / "web"
    except Exception:
        packaged_ui = Path(settings.MDIO_WORKSPACE_ROOT.parent / "mdio_ui" / "web")
    ui_base = Path(os.environ.get("MDIO_UI_PATH", str(packaged_ui)))

    route_handlers = [ExecController, ControlController, MCPController, FileSystemController, IrohGatewayController, TelemetryController, UiAssetsController, CliWsController, GatewayMultiplexer]

    # 静态文件路由 — 仅当 UI 目录存在时挂载
    ui_dirs = [d for d in [ui_base / "client", ui_base / "src", ui_base / "wasm"] if d.exists()]
    if ui_dirs:
        route_handlers.append(create_static_files_router(
            path="/",
            directories=ui_dirs,
            html_mode=True,
            opt={"exclude_from_auth": True},
        ))

    global _channels_plugin
    _channels_plugin = ChannelsPlugin(backend=MemoryChannelsBackend(), arbitrary_channels_allowed=True)

    return Litestar(
        plugins=[
            _channels_plugin,
        ],
        route_handlers=route_handlers,
        lifespan=[lifespan],
        middleware=[SecurityHeadersMiddleware],
        cors_config=cors_config,
        debug=settings.MDIO_DEBUG
    )

def get_channels_plugin():
    return _channels_plugin

app = create_app()
