"""CLI 进程生命周期命令测试（mock 层，不真起 granian）。"""

from click.testing import CliRunner

from mdio_py.cli import main


def test_serve_refuses_when_already_running(monkeypatch):
    import mdio_py.cli.commands as c
    monkeypatch.setattr(c, "_probe_status", lambda: {"pid": 123, "mode": "gateway"})
    launched = []
    monkeypatch.setattr(c, "_launch_granian", lambda **kw: launched.append(kw))

    result = CliRunner().invoke(main, ["serve"])
    assert result.exit_code == 0
    assert "already running" in result.output
    assert launched == []


def test_serve_launches_gateway(monkeypatch):
    import mdio_py.cli.commands as c
    monkeypatch.setattr(c, "_probe_status", lambda: None)
    launched = []
    monkeypatch.setattr(c, "_launch_granian", lambda **kw: launched.append(kw))

    result = CliRunner().invoke(main, ["serve", "--port", "8011"])
    assert result.exit_code == 0
    assert launched and launched[0]["mode"] == "gateway"


def test_status_not_running(monkeypatch):
    import mdio_py.cli.commands as c
    import urllib.error

    def fake_get(path, timeout=10.0):
        raise urllib.error.URLError(ConnectionRefusedError(61, "refused"))
    monkeypatch.setattr(c, "_get_json", fake_get)

    result = CliRunner().invoke(main, ["status"])
    assert result.exit_code == 0
    assert "not running" in result.output


def test_stop_not_running(monkeypatch):
    import mdio_py.cli.commands as c
    import urllib.error

    def fake_get(path, timeout=10.0):
        raise urllib.error.URLError(ConnectionRefusedError(61, "refused"))
    monkeypatch.setattr(c, "_get_json", fake_get)

    result = CliRunner().invoke(main, ["stop"])
    assert result.exit_code == 0
    assert "not running" in result.output
