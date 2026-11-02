"""ControlController 集成测试。"""

import os

import pytest
from litestar.testing import TestClient


def _tp(label: str) -> None:
    """Trace control request steps; enabled only with MDIO_PYTEST_TRACE=1."""
    if os.environ.get("MDIO_PYTEST_TRACE") != "1":
        return
    fd = os.open(
        "/tmp/mdio_control_trace.log",
        os.O_WRONLY | os.O_CREAT | os.O_APPEND,
        0o644,
    )
    try:
        os.write(fd, f"{os.getpid()} {label}\n".encode())
    finally:
        os.close(fd)


@pytest.fixture
def control_app(tmp_path, monkeypatch):
    monkeypatch.setenv("MDIO_VFS_MOUNTS", str(tmp_path))
    monkeypatch.setenv("MDIO_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("MDIO_PROCESS_MODE", "control")
    from mdio_py.gateway import create_app
    return create_app()


def test_status_reports_runtime_md(tmp_path, control_app):
    with TestClient(app=control_app, base_url="http://test.local") as client:
        resp = client.get("/control/status")
        assert resp.status_code == 200
        data = resp.json()
        assert data["running"] is True
        assert data["mode"] == "control"
        assert data["pid"]
        assert data["root"] == str(tmp_path)
        assert data["gateway_enabled"] is False
        assert "genesis" in data["wts"]


def test_control_exec(tmp_path, control_app):
    (tmp_path / "wt").mkdir(exist_ok=True)
    (tmp_path / "wt" / "a.md").write_text("# A\n- v: 7\n")
    with TestClient(app=control_app, base_url="http://test.local") as client:
        resp = client.post("/control/exec", json={
            "code": "$ /a.md::A.v",
            "active_doc": "",
            "wt_uid": "genesis",
        })
        assert resp.status_code == 200
        data = resp.json()
        assert data["status"] == "ok"
        assert "7" in data["result"]





def test_worktree_rpc_load_offload(tmp_path, control_app):
    proj = tmp_path / "rpc_proj"
    proj.mkdir()
    (proj / "a.md").write_text("# A\n- k: v\n", encoding="utf-8")

    with TestClient(app=control_app, base_url="http://test.local") as client:
        loaded = client.post("/worktree", json={
            "id": 1,
            "method": "worktree.load",
            "params": {"path": str(proj), "parent": "genesis"},
        }).json()
        assert loaded["ok"] is True, loaded
        uid = loaded["result"]["uid"]
        assert loaded["result"]["is_new"] is True

        listed = client.post("/worktree", json={
            "id": 2,
            "method": "worktree.list",
            "params": {},
        }).json()
        assert listed["ok"] is True
        assert any(row["uid"] == uid for row in listed["result"]["worktrees"])

        offloaded = client.post("/worktree", json={
            "id": 3,
            "method": "worktree.offload",
            "params": {"wt": "rpc_proj"},
        }).json()
        assert offloaded["ok"] is True, offloaded
        assert offloaded["result"]["uid"] == uid


def test_worktree_ws_list(control_app):
    with TestClient(app=control_app, base_url="http://test.local") as client:
        with client.websocket_connect("/ws/cli") as ws:
            ws.send_json({"id": 1, "method": "worktree.list", "params": {}})
            response = ws.receive_json()
            assert response["ok"] is True
            assert response["result"]["worktrees"]


def test_gateway_on_off_roundtrip(control_app, monkeypatch):
    import mdio_py.gateway.state as state
    monkeypatch.setattr(state, "IROH_AVAILABLE", False)

    with TestClient(app=control_app, base_url="http://test.local") as client:
        _tp("client-entered")
        assert client.get("/control/gateway/status").json()["enabled"] is False
        _tp("status-before-on")

        resp = client.post("/control/gateway/on", json={})
        _tp("gateway-on-returned")
        assert resp.status_code == 200
        assert resp.json()["status"] == "enabled"
        assert client.get("/control/gateway/status").json()["enabled"] is True
        _tp("status-on-ok")

        # gateway off 后声明文件不受影响，runtime.md 标记关闭。
        resp = client.post("/control/gateway/off", json={})
        _tp("gateway-off-returned")
        assert resp.status_code == 200
        assert resp.json()["status"] == "disabled"
        assert client.get("/control/gateway/status").json()["enabled"] is False
        _tp("status-off-ok")


def test_gateway_off_idempotent(control_app):
    with TestClient(app=control_app, base_url="http://test.local") as client:
        assert client.get("/control/gateway/status").json()["enabled"] is False
        resp = client.post("/control/gateway/off", json={})
        assert resp.status_code == 200
        assert resp.json()["status"] == "disabled"
        assert client.get("/control/gateway/status").json()["enabled"] is False


def test_ws_cli_hello_and_exec(control_app):
    with TestClient(app=control_app, base_url="http://test.local") as client:
        with client.websocket_connect("/ws/cli") as ws:
            ws.send_json({"id": 1, "method": "hello"})
            hello = ws.receive_json()
            assert hello["kind"] == "res"
            assert hello["ok"] is True

            ws.send_json({
                "id": 2,
                "method": "exec.run",
                "params": {
                    "code": "$ /.mds/runtime.md::RUNTIME.status",
                    "wt_uid": "genesis",
                },
            })
            resp = ws.receive_json()
            assert resp["ok"] is True
            assert "running" in resp["result"]["raw"]


def test_stop_marks_stopping(control_app):
    with TestClient(app=control_app, base_url="http://test.local") as client:
        resp = client.post("/control/stop", json={})
        assert resp.status_code == 200
        assert resp.json()["status"] == "stopping"
