"""install.md WTS manifest parser tests."""
from pathlib import Path

import pytest

from mdio_py.cli.wts_manifest import load_install_md, parse_install_text


INSTALL = """# INSTALL

- schema: 1
- name: MDIO_WTS

## WORKTREES

### genesis
- path: genesis
- required: true

### chat
- path: chat
- parent: genesis
"""


def test_parse_install_md():
    specs = parse_install_text(INSTALL)
    assert [s.name for s in specs] == ["genesis", "chat"]
    assert specs[0].path == "genesis"
    assert specs[0].required is True
    assert specs[1].parent == "genesis"


def test_parse_root_marker():
    specs = parse_install_text("""# INSTALL

## WORKTREES

### genesis
- path: genesis
- root: true

### chat
- path: chat
""")
    assert specs[0].root is True
    assert specs[1].root is False


def test_load_install_md(tmp_path: Path):
    md = tmp_path / "install.md"
    md.write_text(INSTALL, encoding="utf-8")
    specs = load_install_md(md)
    assert specs is not None
    assert specs[1].name == "chat"


def test_missing_manifest(tmp_path: Path):
    assert load_install_md(tmp_path / "install.md") is None


def test_rejects_missing_worktrees():
    with pytest.raises(ValueError, match="WORKTREES"):
        parse_install_text("# INSTALL\n")


def test_rejects_duplicate_name():
    with pytest.raises(ValueError, match="duplicate"):
        parse_install_text("""# INSTALL

## WORKTREES

### a
- path: a

### a
- path: b
""")


def test_rejects_unsafe_path():
    with pytest.raises(ValueError, match="unsafe"):
        parse_install_text("""# INSTALL

## WORKTREES

### a
- path: ../escape
""")


def test_rejects_missing_parent():
    with pytest.raises(ValueError, match="parent"):
        parse_install_text("""# INSTALL

## WORKTREES

### a
- path: a
- parent: nope
""")
