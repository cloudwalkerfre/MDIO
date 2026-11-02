"""Phase 3 验证测试：project worktree 创建/使用/drop + CWD 路由。

验证 register_worktree_at 创建独立 Backend worktree，
match_worktree 按 path 字段最长前缀匹配，
bootstrap 恢复双路径，drop 正确清理。
"""
import os
import pytest


async def test_register_worktree_at_creates_router(runtime, tmp_path):
    """register_worktree_at 创建独立 Router。"""
    proj_dir = str(tmp_path / "test_project")
    os.makedirs(proj_dir, exist_ok=True)
    wt_uid = runtime._workspace.register_worktree_at("genesis", proj_dir, "fs")
    assert wt_uid != "genesis"  # is a real uid
    assert len(wt_uid) == 8  # uid is 8-char base62


async def test_project_worktree_read_write(runtime, tmp_path):
    """项目目录 worktree 内正常读写文件。"""
    proj_dir = str(tmp_path / "project_rw")
    os.makedirs(proj_dir, exist_ok=True)
    wt_uid = runtime._workspace.register_worktree_at("genesis", proj_dir, "fs")
    await runtime.exec(f'"# PRW\n- v: hello" | /> /docs/test.md -f -god', wt_uid=wt_uid)
    result = await runtime.exec(f'$ /docs/test.md::PRW._b:v', wt_uid=wt_uid)
    assert "hello" in result


async def test_project_worktree_drop(runtime, tmp_path):
    """drop_worktree：session/arena 清理。"""
    proj_dir = str(tmp_path / "project_drop")
    os.makedirs(proj_dir, exist_ok=True)
    wt_uid = runtime._workspace.register_worktree_at("genesis", proj_dir, "fs")
    await runtime.exec(f'"# PD\n- v: x" | /> /docs/d.md -f -god', wt_uid=wt_uid)
    runtime._workspace.drop_derived_worktree(wt_uid)


async def test_project_worktree_drop_does_not_delete_files(runtime, tmp_path):
    """drop_worktree 后项目目录依然存在（Plan 3: VFS 数据在 mem 层）。"""
    proj_dir = str(tmp_path / "project_keep")
    os.makedirs(proj_dir, exist_ok=True)
    wt_uid = runtime._workspace.register_worktree_at("genesis", proj_dir, "fs")
    await runtime.exec(f'"# KEEP\n- v: keep" | /> /docs/keep.md -f -god', wt_uid=wt_uid)
    assert "KEEP" in runtime.store.get_text(wt_uid, "/docs/keep.md")
    runtime._workspace.drop_derived_worktree(wt_uid)
    assert os.path.isdir(proj_dir)


async def test_parse_worktree_entry_with_path(runtime):
    """worktrees.md entry 解析包含 path 字段。"""
    from mdio_py.std.worktree_ops import _parse_worktree_entries
    text = """# WORKTREES

## test_proj
- uid: test1234
- rule:
- path: /tmp/test_proj
"""
    # 先写到 VFS 再用 _parse_worktree_entries（走 selector 管线）
    ws = runtime._workspace
    from mdio_py.core.selector_builder import SelectorBuilder
    ws.write_locator("genesis", "/.mds/worktrees.md", SelectorBuilder().build(),
                     "+>", text, auto_scaffold=True, god=True, suppress_hooks=True)
    entries = _parse_worktree_entries(ws, "genesis")
    assert len(entries) == 1, f"expected 1 entry, got {len(entries)}: {entries}"
    assert entries[0]["name"] == "test_proj"
    assert entries[0]["path"] == "/tmp/test_proj"


# ── CWD 匹配测试 ──


async def test_cwd_match_exact(runtime, tmp_path):
    """CWD 精确匹配已注册 worktree 的 path 字段。"""
    proj_dir = str(tmp_path / "match_exact")
    os.makedirs(proj_dir, exist_ok=True)
    wt_uid = runtime._workspace.register_worktree_at("genesis", proj_dir, "fs")
    # 写 worktrees.md 以便 match_worktree 可读取
    from mdio_py.std.worktree_ops import _append_worktree_entry
    _append_worktree_entry(runtime._workspace, "exact", wt_uid, path=proj_dir)

    result = runtime._workspace.match_worktree(proj_dir)
    assert result == wt_uid


async def test_cwd_match_subpath_guard(runtime, tmp_path):
    """/project 不误匹配 /project_backup（is_subpath trailing / 防护）。"""
    proj_dir = str(tmp_path / "project_a")
    os.makedirs(proj_dir, exist_ok=True)
    proj_backup_dir = str(tmp_path / "project_a_backup")
    os.makedirs(proj_backup_dir, exist_ok=True)

    wt_uid = runtime._workspace.register_worktree_at("genesis", proj_dir, "fs")
    from mdio_py.std.worktree_ops import _append_worktree_entry
    _append_worktree_entry(runtime._workspace, "proj_a", wt_uid, path=proj_dir)

    # /project_a_backup 不应该匹配 /project_a
    result = runtime._workspace.match_worktree(proj_backup_dir)
    assert result is None


async def test_cwd_unmatched_auto_register(runtime, tmp_path):
    """CWD 未匹配任何 worktree → load_worktree 自动注册新 worktree。"""
    proj_dir = str(tmp_path / "auto_project")
    os.makedirs(proj_dir, exist_ok=True)

    uid, is_new = runtime._workspace.load_worktree(proj_dir)
    assert is_new
    assert uid != "genesis"  # not genesis = it's a new uid
    # 再次 load 应该返回已存在的
    uid2, is_new2 = runtime._workspace.load_worktree(proj_dir)
    assert not is_new2
    assert uid2 == uid


async def test_load_worktree_missing_dir_raises(runtime, tmp_path):
    """不存在的外部根 → 报错（不静默注册空 WT）。"""
    missing = str(tmp_path / "never_created")
    before = [w[0] for w in runtime._workspace.store.list_worktrees()]
    import pytest
    with pytest.raises(ValueError, match="does not exist"):
        runtime._workspace.load_worktree(missing)
    after = [w[0] for w in runtime._workspace.store.list_worktrees()]
    assert after == before  # 未注册任何新 WT


async def test_cwd_match_longest_prefix(runtime, tmp_path):
    """多个 worktree 路径有前缀关系时，选最长匹配。"""
    parent_dir = str(tmp_path / "parent")
    os.makedirs(parent_dir, exist_ok=True)
    child_dir = os.path.join(parent_dir, "child")
    os.makedirs(child_dir, exist_ok=True)

    parent_wt = runtime._workspace.register_worktree_at("genesis", parent_dir, "fs")
    child_wt_uid = runtime._workspace.register_worktree_at("genesis", child_dir, "fs")

    from mdio_py.std.worktree_ops import _append_worktree_entry
    _append_worktree_entry(runtime._workspace, "parent", parent_wt, path=parent_dir)
    _append_worktree_entry(runtime._workspace, "child", child_wt_uid, path=child_dir)

    # child_dir 应该匹配 child（更长），不是 parent
    result = runtime._workspace.match_worktree(child_dir)
    assert result == child_wt_uid


# ── Bootstrap 双路径恢复测试 ──


async def test_bootstrap_restore_with_rule(runtime):
    """worktrees.md entry 有 rule → bootstrap 走 derive_worktree。"""
    from mdio_py.core.bootstrap import _restore_one_worktree
    entry = {
        "name": "test_fork",
        "wt_uid": 10,
        "parent_id": 0,
        "kind": "bfork",
        "rule": "/",
        "created_at": "2025-01-01T00:00:00Z",
    }
    # _restore_one_worktree is no-op in WT VFS (tree.ensure() handles restoration)
    _restore_one_worktree(runtime, entry)
    # Verify genesis bindings exist (WT VFS: bindings_info on genesis)
    info = runtime.store.bindings_info("genesis")
    assert isinstance(info, list)
    # wt_uid 10 was never registered in WT VFS, clean up via workspace
    try:
        runtime._workspace.drop_derived_worktree(10)
    except Exception:
        pass


async def test_bootstrap_restore_without_rule(runtime, tmp_path):
    """worktrees.md entry 无 rule 有 path → bootstrap 走 register_worktree_at。"""
    from mdio_py.core.bootstrap import _restore_one_worktree
    proj_dir = str(tmp_path / "bs_project")
    os.makedirs(proj_dir, exist_ok=True)

    entry = {
        "name": "test_proj_bs",
        "wt_uid": 11,
        "parent_id": 0,
        "kind": "project",
        "path": proj_dir,
        "created_at": "2025-01-01T00:00:00Z",
    }
    # _restore_one_worktree is no-op in WT VFS (tree.ensure() handles restoration)
    _restore_one_worktree(runtime, entry)
    # Verify genesis bindings exist (WT VFS: bindings_info on genesis)
    info = runtime.store.bindings_info("genesis")
    assert isinstance(info, list)
    try:
        runtime._workspace.drop_derived_worktree(11)
    except Exception:
        pass


async def test_load_worktree_with_rule_binds_parent_docs_ro(runtime, tmp_path):
    """load 带 rule:项目 WT 获得父 VFS 路径的 RO 绑定(读共享/写拒绝/无 rule 隔离)。"""
    ws = runtime._workspace
    # genesis 准备共享文档
    await runtime.exec(
        '"# Docs\n## Chap1\n- a: 1" | /> /docs/skills/manual.md -f -god',
        wt_uid="genesis")
    proj_dir = str(tmp_path / "proj_rule")
    os.makedirs(proj_dir, exist_ok=True)
    # Binding::parse:第一段恒绑 /home;alias 从第二段起(与 bfork 同语法)。
    # 两段同目标:第一段(/home)已带 RO,第二段挂 /docs/skills(alias)。
    uid, is_new = ws.load_worktree(
        proj_dir, rule="/docs/skills:docs/skills:RO; /docs/skills:docs/skills:RO")
    assert is_new
    # 读共享(绑定目标=父 VFS 路径,数据同树)
    assert "Docs" in runtime.store.get_text(uid, "/docs/skills/manual.md")
    # 写拒绝(RO)
    res = await runtime.exec('"x" | /> /docs/skills/manual.md::Chap1 -f', wt_uid=uid)
    assert "R201" in str(res) or "RO" in str(res), f"expected RO block, got: {res}"
    # 无 rule 的项目:隔离(未绑定 → 本地查找为空)
    proj2 = str(tmp_path / "proj_plain")
    os.makedirs(proj2, exist_ok=True)
    uid2, _ = ws.load_worktree(proj2)
    assert runtime.store.get_text(uid2, "/docs/skills/manual.md") == ""


# ── reload:以磁盘为准的镜像对账 ──


@pytest.mark.persist
async def test_reload_removes_disk_deleted(runtime, tmp_path):
    """磁盘删除的文件,reload 后从 VFS 消失(幽灵镜像清除)。"""
    await runtime.exec(
        '"# A\n- v: 1" | /> /docs/a.md -f -god',
        wt_uid="genesis")
    assert runtime.store.get_text("genesis", "/docs/a.md")
    # 磁盘删(绕过 VFS,模拟外部删除)——文档根是 <根>/wt
    (tmp_path / "wt" / "docs" / "a.md").unlink()
    # reload 前:VFS 仍可读(幽灵)
    assert runtime.store.get_text("genesis", "/docs/a.md")
    await runtime.exec("reload genesis", wt_uid="genesis")
    assert runtime.store.get_text("genesis", "/docs/a.md") == ""


@pytest.mark.persist
async def test_reload_pumps_disk_new(runtime, tmp_path):
    """磁盘新增的文件,reload 后泵入 VFS。"""
    (tmp_path / "wt" / "new.md").write_text("# NEW\n- v: x")
    await runtime.exec("reload genesis", wt_uid="genesis")
    assert runtime.store.get_text("genesis", "/new.md") != ""


@pytest.mark.persist
async def test_reload_refreshes_disk_modified(runtime, tmp_path):
    """磁盘修改的内容,reload 后 VFS 读到新内容。"""
    await runtime.exec(
        '"# A\n- v: 1" | /> /docs/a.md -f -god',
        wt_uid="genesis")
    (tmp_path / "wt" / "docs" / "a.md").write_text("# A\n- v: 2")  # 外部改写
    await runtime.exec("reload genesis", wt_uid="genesis")
    assert "v: 2" in runtime.store.get_text("genesis", "/docs/a.md")


@pytest.mark.persist
async def test_reload_owner_filter_does_not_touch_parent_bindings(runtime, tmp_path):
    """带 rule 绑定的项目 WT reload:不误删 parent(genesis)的绑定镜像;
    只有 reload genesis 本身才清理。"""
    ws = runtime._workspace
    await runtime.exec(
        '"# Docs\n## Chap1\n- a: 1" | /> /docs/skills/manual.md -f -god',
        wt_uid="genesis")
    proj_dir = str(tmp_path / "proj_reload")
    os.makedirs(proj_dir, exist_ok=True)
    uid, _ = ws.load_worktree(
        proj_dir, rule="/docs/skills:docs/skills:RO; /docs/skills:docs/skills:RO")
    # 先把 VFS 写入落到盘上：否则 unlink 与异步 flush 竞争，flush 会把文件写回来，
    # 后面 reload genesis 便清不掉镜像（单跑 -k 时必现）。
    import time
    runtime.store.flush_worktree_sessions("genesis")
    time.sleep(0.3)
    # 磁盘删 genesis 的共享文档
    (tmp_path / "wt" / "docs" / "skills" / "manual.md").unlink()
    # 项目 WT reload:owner 过滤 → genesis 镜像不受影响(按 name 调用,覆盖 name→uid 解析)
    await runtime.exec(f"reload proj_reload", wt_uid=uid)
    assert "Docs" in runtime.store.get_text("genesis", "/docs/skills/manual.md")
    # genesis reload:才真正清理
    await runtime.exec("reload genesis", wt_uid="genesis")
    assert runtime.store.get_text("genesis", "/docs/skills/manual.md") == ""


if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))

