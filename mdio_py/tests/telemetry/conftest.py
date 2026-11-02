"""Telemetry test scaffolding — bench mode for tests in this dir.

Provides a `tel_session` fixture that creates a bench-mode TelemetrySession
and auto-reports at teardown.

Usage:
    async def test_my_thing(runtime, tel_session):
        with tel_session.span("my_span"):
            ...

Set MDIO_TELEMETRY=0 to skip (useful for quick dev runs).
Set MDIO_TELEMETRY=trace for trace mode instead of bench.
"""
import os
import pytest
from mdio_py.telemetry import TelemetrySession


@pytest.fixture
def tel_session(request):
    """Bench-mode TelemetrySession, auto-reports at teardown."""
    mode = os.environ.get("MDIO_TELEMETRY", "bench")
    if mode == "0":
        yield None; return

    rt = None
    for name in ["runtime", "runtime_std", "runtime_ls", "runtime_mem",
                 "runtime_pid", "runtime_llm"]:
        if name in request.fixturenames:
            rt = request.getfixturevalue(name)
            break
    if rt is None:
        yield None; return

    sess = TelemetrySession(
        rt.store,
        request.node.name[:40].replace("/", "_"),
        mode="bench" if mode == "bench" else "trace",
    )
    yield sess
    sess.report()
    sess.close()
