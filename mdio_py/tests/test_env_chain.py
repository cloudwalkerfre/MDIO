"""env 注入链：命中顺序、优先级、安装态落地；回退链三档 + shell 优先 + 安装态首跑落地样板。"""

import os

from mdio_py import _env

PROBE = "MDIO_ENV_CHAIN_PROBE"


def _isolate(monkeypatch, tmp_path, home_text=None, rt_text=None):
    """把三档路径全部指向 tmp（避免读到机器上的真实 .env）。"""
    home = tmp_path / "home"
    home.mkdir(exist_ok=True)
    rt = tmp_path / "rt"
    rt.mkdir(exist_ok=True)
    if home_text is not None:
        (home / ".env").write_text(home_text, encoding="utf-8")
    if rt_text is not None:
        (rt / ".env").write_text(rt_text, encoding="utf-8")
    monkeypatch.setenv("MDIO_HOME", str(home))
    monkeypatch.setenv("MDIO_WORKSPACE_ROOT", str(rt))
    monkeypatch.delenv(PROBE, raising=False)
    monkeypatch.delenv("MDIO_ENV_DEBUG", raising=False)
    monkeypatch.delenv("MDIO_ENV_NO_SEED", raising=False)
    return home, rt


def test_env_source_paths_order():
    """回退链三档顺序：地盘 → 运行时根 → 打包样板（顺序即优先级）。"""
    ps = _env.env_source_paths()
    assert len(ps) == 3, ps
    assert ps[0] == _env.mdio_home() / ".env"
    assert ps[1] == _env.runtime_root() / ".env"
    assert ps[2].name == "env.example"


def test_home_env_is_injected(tmp_path, monkeypatch):
    """第一档命中即注入，并在解析账里留痕。"""
    home, _rt = _isolate(monkeypatch, tmp_path, home_text=f"{PROBE}=from-home\n")
    _env.load_project_env()
    assert os.environ.get(PROBE) == "from-home"
    rep = _env.env_sources_report()
    hit = [x for x in rep["loaded"] if x["path"] == str(home / ".env")]
    assert hit and hit[0]["injected"] == 1, rep


def test_runtime_root_is_fallback(tmp_path, monkeypatch):
    """第二档：地盘没有时由运行时根补上（docstring 一直声称的位置）。"""
    _home, rt = _isolate(monkeypatch, tmp_path, rt_text=f"{PROBE}=from-runtime-root\n")
    _env.load_project_env()
    assert os.environ.get(PROBE) == "from-runtime-root"


def test_home_wins_over_runtime_root(tmp_path, monkeypatch):
    """两档都有时，地盘优先（先到先得）。"""
    _isolate(
        monkeypatch,
        tmp_path,
        home_text=f"{PROBE}=from-home\n",
        rt_text=f"{PROBE}=from-runtime-root\n",
    )
    _env.load_project_env()
    assert os.environ.get(PROBE) == "from-home"


def test_shell_export_wins(tmp_path, monkeypatch):
    """shell 显式导出优先级最高（setdefault 不覆盖）。"""
    _isolate(monkeypatch, tmp_path, home_text=f"{PROBE}=from-home\n")
    monkeypatch.setenv(PROBE, "from-shell")
    _env.load_project_env()
    assert os.environ.get(PROBE) == "from-shell"


def test_installed_first_run_seeds_home_env(tmp_path, monkeypatch):
    """安装态首跑：样板落地为 $MDIO_HOME/.env（仅当不存在），且注入零变化。"""
    home, _rt = _isolate(monkeypatch, tmp_path)
    example = tmp_path / "env.example"
    example.write_text(f"# 全注释样板\n# {PROBE}=from-example\n", encoding="utf-8")
    monkeypatch.setattr(_env, "_installed", lambda: True)
    monkeypatch.setattr(_env, "package_env_example", lambda: example)

    _env.load_project_env()

    seeded = home / ".env"
    assert seeded.is_file(), "安装态首跑必须落地 $MDIO_HOME/.env"
    assert _env.env_sources_report()["seeded"] == str(seeded)
    assert os.environ.get(PROBE) is None, "全注释样板不得注入任何键"


def test_seed_disabled_by_env(tmp_path, monkeypatch):
    """MDIO_ENV_NO_SEED=1 ⇒ 不落地（保持地盘干净）。"""
    home, _rt = _isolate(monkeypatch, tmp_path)
    example = tmp_path / "env.example"
    example.write_text("# 样板\n", encoding="utf-8")
    monkeypatch.setattr(_env, "_installed", lambda: True)
    monkeypatch.setattr(_env, "package_env_example", lambda: example)
    monkeypatch.setenv("MDIO_ENV_NO_SEED", "1")

    _env.load_project_env()

    assert not (home / ".env").exists()
    assert _env.env_sources_report()["seeded"] is None
