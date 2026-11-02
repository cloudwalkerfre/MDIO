"""CLI worktree source parsing and load/offload command tests."""

import io
import tarfile
from pathlib import Path

from click.testing import CliRunner

import mdio_py.cli.commands as commands
from mdio_py.cli import main


def test_github_source_and_archive_url():
    kind, name, location = commands._source_kind("https://github.com/acme/wts")
    assert kind == "github"
    assert name == "wts"
    assert commands._archive_url(location, None).endswith("/archive/HEAD.tar.gz")
    assert commands._archive_url(location, "v1").endswith("/archive/v1.tar.gz")


def test_local_source_requires_directory(tmp_path):
    kind, name, location = commands._source_kind(str(tmp_path))
    assert kind == "local"
    assert Path(location) == tmp_path


def test_load_dry_run_github():
    result = CliRunner().invoke(main, [
        "load", "https://github.com/acme/wts", "--dry-run", "--pwt", "genesis"
    ])
    assert result.exit_code == 0, result.output
    assert "archive/HEAD.tar.gz" in result.output
    assert "parent:  genesis" in result.output


def test_old_path_option_rejected(tmp_path):
    result = CliRunner().invoke(main, ["load", "--path", str(tmp_path)])
    assert result.exit_code != 0


def test_local_bare_dir_fallback(tmp_path):
    proj = tmp_path / "bare"
    proj.mkdir()
    result = CliRunner().invoke(main, ["load", str(proj), "--dry-run"])
    assert result.exit_code == 0, result.output
    assert str(proj) in result.output


def test_local_load_uses_worktree_rpc(tmp_path, monkeypatch):
    proj = tmp_path / "proj"
    (proj / "wt").mkdir(parents=True)
    calls = []
    monkeypatch.setattr(commands, "_ensure_gateway", lambda *args, **kwargs: None)
    monkeypatch.setattr(
        commands, "_post_worktree",
        lambda method, params, timeout=30: calls.append((method, params)) or             {"uid": "uid-1", "is_new": True},
    )
    result = CliRunner().invoke(main, ["load", str(proj), "--pwt", "genesis"])
    assert result.exit_code == 0, result.output
    assert calls == [("load", {
        "path": str(proj),
        "rule": "",
        "parent": "genesis",
    })]


def test_offload_selector_requires_exactly_one(monkeypatch):
    calls = []
    monkeypatch.setattr(
        commands, "_post_worktree",
        lambda method, params, timeout=30: calls.append((method, params)) or             {"uid": "uid-1"},
    )
    runner = CliRunner()
    assert runner.invoke(main, ["offload"]).exit_code != 0
    assert runner.invoke(main, ["offload", "a", "--path", "/tmp/x"]).exit_code != 0
    result = runner.invoke(main, ["offload", "a"])
    assert result.exit_code == 0, result.output
    assert calls == [("offload", {"wt": "a"})]


def test_extract_archive_strips_top_directory(tmp_path):
    archive = tmp_path / "wts.tar.gz"
    with tarfile.open(archive, "w:gz") as tf:
        data = b"# A\n- k: v\n"
        info = tarfile.TarInfo("repo/genesis/wt/a.md")
        info.size = len(data)
        tf.addfile(info, io.BytesIO(data))
    target = tmp_path / "dest"
    commands._extract_archive(archive, target)
    assert (target / "genesis" / "wt" / "a.md").read_text() == "# A\n- k: v\n"
