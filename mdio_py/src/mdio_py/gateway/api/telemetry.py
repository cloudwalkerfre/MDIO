"""Telemetry HTTP endpoints — JS/WASM data flush, layer stats query."""

from litestar import Controller, post, get
from litestar.di import Provide
from mdio_py.gateway.state import provide_runtime
from mdio_py import MdioRuntime
from mdio_py.telemetry import TELEMETRY_COMPILED, TelemetrySession


class TelemetryController(Controller):
    path = "/telemetry"
    dependencies = {"runtime": Provide(provide_runtime)}

    @post("/session-create")
    async def session_create(self, data: dict, runtime: MdioRuntime) -> dict:
        """Create a named telemetry session on the gateway."""
        if not TELEMETRY_COMPILED:
            return {"status": "error", "message": "telemetry not compiled"}
        try:
            name = data.get("name", f"gw_{id(runtime):x}")
            mode = data.get("mode", "trace")
            sess = TelemetrySession(
                runtime.store, name, mode,
                max_events=data.get("max_events"),
                max_meta_pool=data.get("max_meta_pool"),
                bench_samples_per_phase=data.get("bench_samples_per_phase"),
                max_layer_samples=data.get("max_layer_samples"),
            )
            return {"status": "ok", "session": name, "mode": mode}
        except Exception as e:
            return {"status": "error", "message": str(e)}

    @post("/js-flush")
    async def js_flush(self, data: dict, runtime: MdioRuntime) -> dict:
        """Receive JS/WASM telemetry data and merge into the active session."""
        if not TELEMETRY_COMPILED:
            return {"status": "error", "message": "telemetry not compiled"}
        import json, time
        try:
            # Auto-create session if none active (first flush creates it)
            if not runtime.store.telemetry_session_active():
                runtime.store.telemetry_session_create(f"auto_js_{int(time.time())}", "trace")
            runtime.store.telemetry_merge_js_data(json.dumps(data))
            return {"status": "ok", "merged": True}
        except Exception as e:
            return {"status": "error", "message": str(e)}

    @get("/layer-stats")
    async def layer_stats(self, runtime: MdioRuntime) -> dict:
        """Return raw layer stats as JSON."""
        if not TELEMETRY_COMPILED:
            return {"status": "error", "message": "telemetry not compiled"}
        import json
        try:
            raw = runtime.store.telemetry_get_layer_stats()
            return {"status": "ok", "data": json.loads(raw)}
        except Exception as e:
            return {"status": "error", "message": str(e)}

    @get("/export")
    async def export(self, runtime: MdioRuntime) -> dict:
        """非消费导出：phase（span 明细）+ layer_stats（跨栈层样本）+ session 元信息。

        供 polars 自由分析（分析脚本/外部工具）——不触发消费性 clear，
        /telemetry/report 才是消费出口。
        """
        if not TELEMETRY_COMPILED:
            return {"status": "error", "message": "telemetry not compiled"}
        import json
        try:
            store = runtime.store
            active = bool(store.telemetry_session_active()) if hasattr(store, "telemetry_session_active") else False
            phases = json.loads(store.telemetry_export_phase_json())
            layers = json.loads(store.telemetry_get_layer_stats())
            mode = None
            try:
                mode = store.telemetry_session_mode() or None
            except Exception:
                pass
            return {
                "status": "ok",
                "session_active": active,
                "mode": mode,
                "phase_rows": len(phases),
                "layer_rows": len(layers),
                "phases": phases,
                "layers": layers,
            }
        except Exception as e:
            return {"status": "error", "message": str(e)}

    @get("/report")
    async def report(self, runtime: MdioRuntime) -> dict:
        """Generate full Python report (polars pipeline: terminal + layer + files)."""
        if not TELEMETRY_COMPILED:
            return {"status": "error", "message": "telemetry not compiled"}
        try:
            from mdio_py.telemetry import report_session
            text = report_session(runtime.store)
            return {"status": "ok", "report": text}
        except Exception as e:
            return {"status": "error", "message": str(e)}
