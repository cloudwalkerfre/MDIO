"""坐标纪律守卫（三根分层 · 阶段 0：定义钉死）。

这些断言测的不是"功能"，而是**约束**——它们随分层推进收紧：
  阶段 0/1（今天）：地盘/运行时根各只有一份实现；project_root 只是别名；换算函数恒等 / None。
  阶段 3（切布局）：把 test_conversion_is_identity_today 的期望改成 <project>/wt 与 <project>。
"""
import os
import re
from pathlib import Path

from mdio_py import _env
from mdio_py.runtime import worktree_paths as wp

SRC = Path(_env.__file__).resolve().parent          # mdio_py/
PATHS_FILE = Path(wp.__file__).resolve()           # 坐标文件（唯一允许出现层级名的地方）
ENV_FILE = Path(_env.__file__).resolve()


def _py_files():
    return [p for p in SRC.rglob("*.py")]


def _install_mode() -> bool:
    return "site-packages" in str(ENV_FILE)


def test_home_single_implementation():
    """「mdio 的地盘在哪」只准有一份实现：除 _env.py 外不得再出现 parents[3]/parents[4]。"""
    offenders = []
    for p in _py_files():
        if p == ENV_FILE:
            continue
        if re.search(r"parents\[[34]\]", p.read_text(encoding="utf-8", errors="ignore")):
            offenders.append(str(p.relative_to(SRC)))
    assert offenders == [], f"重复的地盘定位逻辑：{offenders}"


def test_project_root_is_alias_of_home():
    """老名字 project_root 只是**地盘**的兼容别名（指向同一个函数对象）。"""
    assert _env.project_root is _env.mdio_home
    assert _env.ensure_project_root is _env.ensure_mdio_home


def test_runtime_root_is_root_wt_dir():
    """runtime_root = **根 wt（genesis）的根**：装态 <home>/genesis、源码态 <home>/workspace。"""
    expect = _env.mdio_home() / ("genesis" if _install_mode() else "workspace")
    assert _env.runtime_root() == expect
    # 调用方不得再自己拼 /genesis（历史上 __init__.py 与 config.py 各推了一份，源码态还不一致）
    init_text = (SRC / "__init__.py").read_text(encoding="utf-8")
    assert 'runtime_root() / "genesis"' not in init_text
    assert "project_root()" not in init_text


def test_relative_workspace_root_is_anchored(monkeypatch, tmp_path):
    """MDIO_WORKSPACE_ROOT 使用相对路径时，必须锚定到实例地盘，不能随进程 cwd 漂移。"""
    monkeypatch.setenv("MDIO_HOME", str(tmp_path))
    monkeypatch.setenv("MDIO_WORKSPACE_ROOT", "./workspace")
    assert _env.runtime_root() == (tmp_path / "workspace").resolve()


def test_absolute_workspace_root_is_preserved(monkeypatch, tmp_path):
    absolute = tmp_path / "abs-workspace"
    monkeypatch.setenv("MDIO_HOME", str(tmp_path / "home"))
    monkeypatch.setenv("MDIO_WORKSPACE_ROOT", str(absolute))
    assert _env.runtime_root() == absolute


def test_config_reuses_env_roots():
    """gateway/config.py 不再自带定位副本；地盘与运行时根各自转发。"""
    text = (SRC / "gateway" / "config.py").read_text(encoding="utf-8")
    assert "from mdio_py._env import mdio_home, runtime_root" in text
    assert not re.search(r"parents\[[34]\]", text), "config.py 里还有定位副本"
    assert "site-packages" not in text, "config.py 里还有安装态判定副本"
    from mdio_py.gateway.config import HOME_ROOT, PROJECT_ROOT, RUNTIME_ROOT
    assert HOME_ROOT == _env.mdio_home()
    assert PROJECT_ROOT == HOME_ROOT          # 兼容别名 = 地盘
    assert RUNTIME_ROOT == _env.runtime_root()


def test_conversion_follows_layout():
    """阶段 3 已切布局：文档根 = `<p>/wt`、实现根 = `<p>/plugin`；运行态根仍 None（WAL 由 wal_rel 决定）。

    布局名（wt / .wal）由坐标文件给：**core 不认识这些名字**，只按 host 传来的名字拼路径。
    """
    p = "/tmp/example-project"
    assert wp.doc_root_for(p) == os.path.join(os.path.realpath(p), "wt")
    assert wp.plugin_root_for(p) == os.path.join(os.path.realpath(p), "plugin")
    assert wp.state_root_for(p) is None
    assert wp.doc_subdir_for(p) == "wt"
    assert wp.wal_rel_for(p) == ".wal"


def test_resolvers_are_the_only_wiring():
    """换算函数的**唯一消费点**：boot 用布局名（doc_subdir_for/wal_rel_for）、资产出口用 ui_dirs_for_wt。

    注意：挂载配置里的 path 段是**项目根**（不是文档根）——文档子目录由 core 按 doc_subdir 自己套，
    所以 __init__.py 里**不该**出现 doc_root_for（这条断言防的是"两处各套一层 /wt"）。
    """
    init_text = (SRC / "__init__.py").read_text(encoding="utf-8")
    assert "doc_subdir_for(" in init_text and "wal_rel_for(" in init_text
    assert "doc_root_for(" not in init_text, "挂载配置的 path 是项目根，不能再套文档根"
    ua_text = (SRC / "gateway" / "api" / "ui_assets.py").read_text(encoding="utf-8")
    assert "ui_dirs_for_wt(" in ua_text      # 单根换算已收进链并集器（ui_scope_root_for 的消费点）
    assert "for uid, _name, _vfs_alias, project_root in" in ua_text, "list_worktrees() 应具名解包"


def test_worktrees_md_has_no_external_root_field():
    """`- external_root:` 字段已退役（= path 的同值副本；core 用 path 兜底）⇒ 不准再写。"""
    ops = (SRC / "std" / "worktree_ops.py").read_text(encoding="utf-8")
    assert "- external_root:" not in ops, "worktree_ops.py 仍在写 external_root 字段"
    ws = (SRC / "runtime" / "workspace.py").read_text(encoding="utf-8")
    assert "external_root: {normalized_root}" not in ws, "workspace.py 仍有回填逻辑"


def test_is_anchor_dir_narrowed():
    """锚点收窄（分层后）：地盘与**默认 genesis** 是自家；地盘下其它目录（如 demo）是可加载项目。"""
    home = os.path.realpath(str(_env.mdio_home()))
    gen = os.path.realpath(str(_env.runtime_root()))
    assert wp.is_anchor_dir(home) is True
    assert wp.is_anchor_dir(os.path.dirname(home)) is True
    assert wp.is_anchor_dir(gen) is True
    assert wp.is_anchor_dir(os.path.join(gen, "wt")) is True
    assert wp.is_anchor_dir(os.path.join(home, "demo")) is False, "~/.mdio/demo 是可加载的项目"
    assert wp.is_anchor_dir("/tmp/some-project") is False


def test_layout_literals_only_in_coordinate_file():
    """层级名（wt / plugin / .wal）不得作为**路径字面量**散落在 src 别处。"""
    pat = re.compile(r'"(?:/wt|wt/|/plugin|plugin/|\.wal|\.mds/\.wal)"')
    offenders = []
    for p in _py_files():
        if p == PATHS_FILE:
            continue
        for i, line in enumerate(p.read_text(encoding="utf-8", errors="ignore").splitlines(), 1):
            if line.lstrip().startswith("#"):
                continue
            if pat.search(line):
                offenders.append(f"{p.relative_to(SRC)}:{i}")
    assert offenders == [], f"层级名散落在坐标文件之外：{offenders}"
