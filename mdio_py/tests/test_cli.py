"""MDIO CLI 测试 — CLI 作为 HTTP 客户端，对 test app 执行命令。"""

from pathlib import Path

import pytest
from click.testing import CliRunner
from litestar.testing import TestClient

from mdio_py.cli import main


@pytest.fixture
def runner():
    return CliRunner()


@pytest.fixture
def cli_server(tmp_path, monkeypatch):
    """启动一个 control-mode 测试 app，并把 CLI 的 HTTP helper 指向它。"""
    (tmp_path / "wt").mkdir(exist_ok=True)
    (tmp_path / "wt" / "config.md").write_text(
        "# PROJECT\n"
        "- name: MDIO_NEO\n"
        "- version: 0.8.1\n"
        "- port: 5432\n"
    )

    monkeypatch.setenv("MDIO_VFS_MOUNTS", str(tmp_path))
    monkeypatch.setenv("MDIO_WORKSPACE_ROOT", str(tmp_path))
    monkeypatch.setenv("MDIO_PROCESS_MODE", "control")

    from mdio_py.gateway import create_app
    from mdio_py.cli import commands

    app = create_app()
    with TestClient(app=app, base_url="http://test.local") as client:
        def request_json(method, path, payload=None, timeout=10.0):
            if method == "GET":
                resp = client.get(path)
            else:
                resp = client.post(path, json=payload or {})
            assert 200 <= resp.status_code < 300, f"{method} {path} -> {resp.status_code}: {resp.text}"
            return resp.json()

        monkeypatch.setattr(commands, "_request_json", request_json)
        yield client


class TestCliBasics:
    def test_help(self, runner):
        result = runner.invoke(main, ["--help"])
        assert result.exit_code == 0
        assert "exec" in result.output
        assert "repl" in result.output
        assert "serve" in result.output
        assert "web" in result.output
        assert "stop" in result.output

    def test_version(self, runner):
        result = runner.invoke(main, ["--version"])
        assert result.exit_code == 0


class TestCliWeb:
    def test_web_no_open(self, runner, monkeypatch):
        from mdio_py.cli import commands
        seen = {}

        def fake_ensure(host, port, root=None):
            seen.update(host=host, port=port, root=root)

        enabled = []
        monkeypatch.setattr(commands, "_ensure_gateway", fake_ensure)
        monkeypatch.setattr(
            commands, "_get_json",
            lambda path, timeout=2: {"gateway_enabled": False},
        )
        monkeypatch.setattr(
            commands, "_post_json",
            lambda path, payload=None, timeout=10: enabled.append(path) or {"status": "enabled"},
        )
        result = runner.invoke(main, ["web", "--no-open", "--port", "18099"])
        assert result.exit_code == 0, result.output
        assert "http://127.0.0.1:18099/?wt=genesis" in result.output
        assert seen == {"host": "127.0.0.1", "port": 18099, "root": None}
        assert enabled == ["/control/gateway/on"]

    def test_web_opens_genesis(self, runner, monkeypatch):
        from mdio_py.cli import commands
        import webbrowser

        monkeypatch.setattr(commands, "_ensure_gateway", lambda host, port, root=None: None)
        monkeypatch.setattr(
            commands, "_get_json",
            lambda path, timeout=2: {"gateway_enabled": True},
        )
        opened = []
        monkeypatch.setattr(webbrowser, "open", lambda url: opened.append(url) or True)
        result = runner.invoke(main, ["web", "--port", "18099"])
        assert result.exit_code == 0, result.output
        assert opened == ["http://127.0.0.1:18099/?wt=genesis"]


class TestCliLoad:
    @staticmethod
    def _wts(tmp_path):
        root = tmp_path / "MDIO_WTS"
        for name in ("genesis", "chat"):
            (root / name / "wt").mkdir(parents=True)
        (root / "install.md").write_text(
            """# INSTALL

## WORKTREES

### genesis
- path: genesis
- required: true

### chat
- path: chat
- parent: genesis
""",
            encoding="utf-8",
        )
        return root

    def test_list_manifest(self, runner, tmp_path):
        root = self._wts(tmp_path)
        result = runner.invoke(main, ["load", str(root), "--list"])
        assert result.exit_code == 0, result.output
        assert "genesis" in result.output
        assert "chat" in result.output

    def test_default_loads_all(self, runner, tmp_path, monkeypatch):
        from mdio_py.cli import commands

        root = self._wts(tmp_path)
        calls = []
        monkeypatch.setattr(commands, "_ensure_gateway", lambda *a, **k: None)
        monkeypatch.setattr(
            commands, "_post_worktree",
            lambda method, params, timeout=30: calls.append(params) or {"uid": "x", "is_new": True},
        )
        result = runner.invoke(main, ["load", str(root)])
        assert result.exit_code == 0, result.output
        assert [c["parent"] for c in calls] == ["genesis", "genesis"]
        assert [Path(c["path"]).name for c in calls] == ["genesis", "chat"]

    def test_root_requires_matching_gateway(self, runner, tmp_path, monkeypatch):
        from mdio_py.cli import commands

        root = tmp_path / "ROOT_WTS"
        (root / "genesis" / "wt").mkdir(parents=True)
        (root / "install.md").write_text(
            """# INSTALL

## WORKTREES

### genesis
- path: genesis
- root: true
""",
            encoding="utf-8",
        )
        monkeypatch.setattr(commands, "_ensure_gateway", lambda *a, **k: None)
        monkeypatch.setattr(commands, "_get_json", lambda *a, **k: {"root": str(tmp_path / "other")})
        result = runner.invoke(main, ["load", str(root), "--wt", "genesis"])
        assert result.exit_code != 0
        assert "already running with root" in result.output
        assert "mdio stop" in result.output
        assert "single-worktree loads stay hot" in result.output

    def test_pwt_requires_single_wt(self, runner, tmp_path):
        root = self._wts(tmp_path)
        result = runner.invoke(main, ["load", str(root), "--pwt", "other"])
        assert result.exit_code != 0
        assert "--pwt" in result.output

    def test_rejects_loose_markdown_dir(self, runner, tmp_path):
        """D33: 既无 install.md 又无 wt/ 的裸目录必须明确拒绝，不能静默成功产空树。"""
        loose = tmp_path / "loose"
        loose.mkdir()
        (loose / "a.md").write_text("# A\n", encoding="utf-8")
        result = runner.invoke(main, ["load", str(loose)])
        assert result.exit_code != 0, result.output
        assert "neither a WTS" in result.output
        assert "nothing was loaded" in result.output

    def test_select_one(self, runner, tmp_path, monkeypatch):
        from mdio_py.cli import commands

        root = self._wts(tmp_path)
        calls = []
        monkeypatch.setattr(commands, "_ensure_gateway", lambda *a, **k: None)
        monkeypatch.setattr(
            commands, "_post_worktree",
            lambda method, params, timeout=30: calls.append(params) or {"uid": "x", "is_new": True},
        )
        result = runner.invoke(main, ["load", str(root), "--wt", "chat"])
        assert result.exit_code == 0, result.output
        assert len(calls) == 1
        assert calls[0]["parent"] == "genesis"


class TestCliExec:
    def test_exec_read(self, runner, cli_server):
        result = runner.invoke(
            main, ["exec", "$ /config.md::PROJECT.name"]
        )
        assert result.exit_code == 0
        assert "MDIO_NEO" in result.output

    def test_exec_write(self, runner, cli_server):
        result = runner.invoke(
            main, ["exec",
                   '$ /config.md::PROJECT._b:port | /> /config.md::PROJECT.version --text "v2" -f']
        )
        assert result.exit_code == 0

    def test_exec_stdin_pipe(self, runner, cli_server):
        result = runner.invoke(
            main, ["exec", '$ /config.md::PROJECT._b:port'],
            input="override"
        )
        assert "5432" in result.output

    def test_exec_tree(self, runner, cli_server):
        result = runner.invoke(main, ["exec", "tree /config.md -d 2"])
        assert result.exit_code == 0
        assert "PROJECT" in result.output


class TestCliStatus:
    def test_status(self, runner, cli_server):
        result = runner.invoke(main, ["status"])
        assert result.exit_code == 0
        assert "running: True" in result.output
        assert "genesis" in result.output

