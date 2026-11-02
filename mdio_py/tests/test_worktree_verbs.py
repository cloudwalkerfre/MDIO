"""wt 级（load/offload）与 folder 级（mount/unmount）动词的钉子测试。

**wt 级 = load/offload**（本地项目），**folder 级 = mount/unmount**
（iroh 远程共享）。本文件钉住：
  ① wt 级 load/offload 真的能建/能拆（且可重复调用有明确回执）；
  ② load 默认挂到当前 wt，parent 只能是 self 或 self 后代；
  ③ offload 只能卸载当前 wt 的后代，genesis 自身不可 offload；
  ④ 8 层深度是 Python 创建入口的统一上限；
  ⑤ folder 级动词仍要求 /share/<name> 形态。
"""
from pathlib import Path

from mdio_py import MdioRuntime


async def test_wt_load_offload_roundtrip(tmp_path: Path):
    proj = tmp_path / "proj"
    proj.mkdir()
    (proj / "a.md").write_text("# A\n- k: v\n", encoding="utf-8")

    rt = MdioRuntime(str(tmp_path))

    r1 = str(await rt.exec(f'worktree load "{proj}"', wt_uid="genesis"))
    assert "loaded" in r1 and "mdio_error" not in r1, r1
    uid = rt._workspace.resolve_worktree_name("proj")
    assert uid, "load 之后应能按名解析出 uid"

    r2 = str(await rt.exec(f'worktree load "{proj}"', wt_uid="genesis"))
    assert "已存在" in r2 and "mdio_error" not in r2, r2

    r3 = str(await rt.exec('worktree offload "proj"', wt_uid="genesis"))
    assert "offloaded" in r3 and "mdio_error" not in r3, r3
    assert rt._workspace.resolve_worktree_name("proj") is None, "offload 之后不应再解析到"


async def test_wt_load_scope_and_offload_descendant(tmp_path: Path):
    proj = tmp_path / "p1"
    proj.mkdir()
    (proj / "a.md").write_text("# A\n- k: v\n", encoding="utf-8")
    other = tmp_path / "p2"
    other.mkdir()
    rt = MdioRuntime(str(tmp_path))

    b = str(await rt.exec('worktree bfork "child" --rule "/x"', wt_uid="genesis"))
    child = rt._workspace.resolve_worktree_name("child")
    assert child, b

    r1 = str(await rt.exec(f'worktree load "{proj}"', wt_uid=child))
    assert "loaded" in r1 and "mdio_error" not in r1, r1
    grandchild = rt._workspace.resolve_worktree_name("p1")
    assert grandchild, r1
    assert rt._workspace._store.parent_uid(grandchild) == child

    r2 = str(await rt.exec(f'worktree load "{other}" --pwt genesis', wt_uid=child))
    assert "must be self or a descendant" in r2, r2

    r3 = str(await rt.exec('worktree offload "child"', wt_uid=child))
    assert "must be a descendant" in r3, r3

    r4 = str(await rt.exec('worktree offload "p1"', wt_uid=child))
    assert "offloaded" in r4 and "mdio_error" not in r4, r4


async def test_wt_depth_limit(tmp_path: Path):
    rt = MdioRuntime(str(tmp_path))
    parent = "genesis"
    for index in range(7):
        name = f"d{index}"
        result = str(await rt.exec(
            f'worktree bfork "{name}" --rule "/"', wt_uid=parent
        ))
        assert "created" in result, result
        parent = rt._workspace.resolve_worktree_name(name)
        assert parent
    assert rt._workspace.worktree_level(parent) == 8
    result = str(await rt.exec('worktree bfork "too_deep" --rule "/"', wt_uid=parent))
    assert "level limit 8" in result, result




async def test_offload_parent_resets_active_descendant(tmp_path: Path):
    rt = MdioRuntime(str(tmp_path))
    first = str(await rt.exec('worktree bfork "parent" --rule "/x"', wt_uid="genesis"))
    parent_uid = rt._workspace.resolve_worktree_name("parent")
    assert parent_uid, first
    second = str(await rt.exec('worktree bfork "child" --rule "/y"', wt_uid=parent_uid))
    child_uid = rt._workspace.resolve_worktree_name("child")
    assert child_uid, second
    assert rt._workspace.active_worktree_uid == child_uid

    result = str(await rt.exec('worktree offload "parent"', wt_uid="genesis"))
    assert "offloaded" in result and "mdio_error" not in result, result
    assert rt._workspace.active_worktree_uid == "genesis"


async def test_wt_use_switches_and_resets_to_genesis(tmp_path: Path):
    """`worktree use`：子 wt 能切、**genesis 能回根**（否则服务端 active 卡在子 wt）。"""
    proj = tmp_path / "p2"
    proj.mkdir()
    (proj / "a.md").write_text("# A\n", encoding="utf-8")
    rt = MdioRuntime(str(tmp_path))
    r1 = str(await rt.exec(f'worktree load "{proj}"', wt_uid="genesis"))
    assert "loaded" in r1, r1

    r2 = str(await rt.exec('worktree use "p2"', wt_uid="genesis"))
    assert "switched to 'p2'" in r2, r2
    assert rt._workspace.active_worktree_uid == rt._workspace.resolve_worktree_name("p2")

    r3 = str(await rt.exec('worktree use "genesis"', wt_uid="genesis"))
    assert "switched to 'genesis'" in r3, r3
    assert rt._workspace.active_worktree_uid == "genesis"
    assert rt._workspace.active_worktree_name is None

    # 负例：不存在的名字仍要报错（别把 use 变成"什么都能切"）
    r4 = str(await rt.exec('worktree use "nope"', wt_uid="genesis"))
    assert "not found" in r4, r4


async def test_genesis_cannot_be_offloaded(tmp_path: Path):
    rt = MdioRuntime(str(tmp_path))
    r = str(await rt.exec('worktree offload "genesis"', wt_uid="genesis"))
    assert "genesis 不可 offload" in r, r


async def test_folder_verbs_still_require_share_path(tmp_path: Path):
    rt = MdioRuntime(str(tmp_path))
    m = str(await rt.exec("worktree mount", wt_uid="genesis"))
    assert "mount requires /share/" in m, m
    u = str(await rt.exec("worktree unmount", wt_uid="genesis"))
    assert "unmount requires /share/" in u, u
    # 改名后旧名不存在（不是并存）
    old = str(await rt.exec("worktree unload /share/x", wt_uid="genesis"))
    assert "unknown worktree sub-command" in old, old


async def test_cli_mount_persists_rule_and_genesis_fails_clean(tmp_path: Path):
    """B2 回归：CLI mount 必须写回 worktrees.md（rebuild 后仍在）；genesis 无条目 ⇒ 明确失败。

    旧实现只调 RuleManager(store)：两个 if 都不成立 ⇒ 只改内存，下次 rebuild 即丢。
    见 BUGS 复盘 B2。
    """
    rt = MdioRuntime(str(tmp_path))
    # genesis 没有 worktrees.md 条目可写 ⇒ 明确报错 + 回滚内存，不留假成功
    m0 = str(await rt.exec('worktree mount --path /share/x --from "iroh://abc" --as x', wt_uid="genesis"))
    assert "mdio_error" in m0, m0
    assert rt.store.rule_remote_entries("genesis") == []
    # cfork 目标有条目 ⇒ 写回文件，强制全量 rebuild 后仍在
    await rt.exec('worktree cfork "kid"', wt_uid="genesis")
    kid = rt._workspace.active_worktree_uid
    m = str(await rt.exec('worktree mount --path /share/foo --from "iroh://abc" --as foo', wt_uid="genesis"))
    assert "added remote" in m, m
    assert rt.store.rule_remote_entries(kid) == [("/share/foo", "foo")]
    rt.store.flush_worktree_sessions("genesis")
    rt.store.rule_add_remote("genesis", "/probe", "probe")   # 强制 dirty 触发全量重建
    rt.store.tree_ensure()
    assert rt.store.rule_remote_entries(kid) == [("/share/foo", "foo")]


async def test_load_project_named_genesis_gets_suffixed_name(tmp_path: Path):
    """根 genesis 必须参与子 wt 名称占用：basename=genesis 的项目应命名为 genesis_1。"""
    proj = tmp_path / "genesis"
    proj.mkdir()
    (proj / "a.md").write_text("# A\n", encoding="utf-8")
    rt = MdioRuntime(str(tmp_path))

    uid, is_new = rt._workspace.load_worktree(str(proj), name="genesis")
    assert is_new
    from mdio_py.std.worktree_ops import _parse_worktree_entries
    names = [e["name"] for e in _parse_worktree_entries(rt._workspace)]
    assert "genesis_1" in names, names
    assert "genesis" not in names, names
    assert rt._workspace.resolve_worktree_name("genesis_1") == uid
