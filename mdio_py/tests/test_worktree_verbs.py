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
    assert "unmount requires --path /share/" in u, u
    # 位置参数形态不可用（语法会把裸路径当读表达式）——与 mount 同款，必须 --path
    u_pos = str(await rt.exec("worktree unmount /share/foo", wt_uid="genesis"))
    assert "unmount requires --path /share/" in u_pos, u_pos
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


# ── folder 级 unmount：owner 解析 + 持久化 + gateway.md 清理 ──

async def _std_mount_foo(rt):
    res = str(await rt.exec(
        'worktree mount --path /share/foo --from "iroh://abc" --as foo',
        wt_uid="genesis"))
    assert "added remote" in res and "mdio_error" not in res, res


async def test_std_unmount_resolves_owner_by_path_and_persists(tmp_path: Path):
    """D35 同类：unmount 不能再缺省 active 树；删完必须写回 worktrees.md，rebuild 不复活。"""
    rt = MdioRuntime(str(tmp_path))
    await rt.exec('worktree cfork "kid"', wt_uid="genesis")
    kid = rt._workspace.active_worktree_uid
    await _std_mount_foo(rt)
    assert rt.store.rule_remote_entries(kid) == [("/share/foo", "foo")]

    # 切回 genesis（旧实现拿 active=genesis 当目标：静默成功、kid 的 rule 还在）
    await rt.exec('worktree use "genesis"', wt_uid="genesis")
    res = str(await rt.exec('worktree unmount --path /share/foo', wt_uid="genesis"))
    assert "unmounted" in res and kid in res, res
    assert rt.store.rule_remote_entries(kid) == [], res

    # 强制全量 rebuild：若 rule 没写回文件，这里会从 worktrees.md 复活
    rt.store.flush_worktree_sessions("genesis")
    rt.store.rule_add_remote("genesis", "/probe", "probe")
    rt.store.tree_ensure()
    assert rt.store.rule_remote_entries(kid) == []


async def test_std_unmount_removes_gateway_md_entry(tmp_path: Path):
    """gateway.md 条目也必须清：只删 rule 时重启 reconcile 仍会看到挂载声明。"""
    from mdio_py.core.selector_builder import SelectorBuilder

    rt = MdioRuntime(str(tmp_path))
    await rt.exec('worktree cfork "kid"', wt_uid="genesis")
    kid = rt._workspace.active_worktree_uid
    await _std_mount_foo(rt)
    rt._workspace.write_locator(
        kid, "/.mds/gateway.md", SelectorBuilder().build(), "+>",
        "# GATEWAY\n\n## foo\n- local_prefix: /share/foo\n", auto_scaffold=True)
    await rt.exec('worktree use "genesis"', wt_uid="genesis")

    res = str(await rt.exec('worktree unmount --path /share/foo', wt_uid="genesis"))
    assert "unmounted" in res and "mdio_error" not in res, res
    gw = rt.store.get_text(kid, "/.mds/gateway.md") or ""
    assert "## foo" not in gw, gw
    assert rt.store.rule_remote_entries(kid) == []


async def test_std_unmount_explicit_wrong_wt_does_not_delete(tmp_path: Path):
    """显式给出别的 WT 时，路径不属于它 ⇒ 报错，不能删到别的树。"""
    rt = MdioRuntime(str(tmp_path))
    await rt.exec('worktree cfork "kid"', wt_uid="genesis")
    kid = rt._workspace.active_worktree_uid
    await _std_mount_foo(rt)
    await rt.exec('worktree cfork "kid2"', wt_uid="genesis")
    kid2 = rt._workspace.active_worktree_uid
    assert kid2 != kid

    res = str(await rt.exec(
        'worktree unmount --path /share/foo --name kid2', wt_uid="genesis"))
    assert "mdio_error" in res and "unmounted" not in res, res
    assert rt.store.rule_remote_entries(kid) == [("/share/foo", "foo")]

    # 未知 wt 名：必须返回可读错误（旧代码用了不存在的 ErrCode.NOT_FOUND ⇒ I500）
    res_ghost = str(await rt.exec(
        'worktree unmount --path /share/foo --name ghost', wt_uid="genesis"))
    assert "mdio_error" in res_ghost and "not found" in res_ghost, res_ghost
    assert rt.store.rule_remote_entries(kid) == [("/share/foo", "foo")]

    # 无任何命中：也必须报错，不能静默成功
    res2 = str(await rt.exec('worktree unmount --path /share/nope', wt_uid="genesis"))
    assert "mdio_error" in res2 and "unmounted" not in res2, res2
    assert rt.store.rule_remote_entries(kid) == [("/share/foo", "foo")]


async def test_std_unmount_ambiguous_requires_explicit_wt(tmp_path: Path):
    """同 path 在多个 WT 都有挂载点时列候选、拒绝猜；显式 --name 才执行。"""
    rt = MdioRuntime(str(tmp_path))
    await rt.exec('worktree cfork "kid"', wt_uid="genesis")
    kid = rt._workspace.active_worktree_uid
    await _std_mount_foo(rt)
    await rt.exec('worktree cfork "kid2"', wt_uid="genesis")
    kid2 = rt._workspace.active_worktree_uid
    await _std_mount_foo(rt)
    await rt.exec('worktree use "genesis"', wt_uid="genesis")

    res = str(await rt.exec('worktree unmount --path /share/foo', wt_uid="genesis"))
    assert "mdio_error" in res and "命中多个挂载点" in res, res
    assert rt.store.rule_remote_entries(kid) == [("/share/foo", "foo")]
    assert rt.store.rule_remote_entries(kid2) == [("/share/foo", "foo")]

    ok = str(await rt.exec('worktree unmount --path /share/foo --name kid', wt_uid="genesis"))
    assert "unmounted" in ok and "mdio_error" not in ok, ok
    assert rt.store.rule_remote_entries(kid) == []
    assert rt.store.rule_remote_entries(kid2) == [("/share/foo", "foo")]


async def test_std_unmount_prefers_live_gateway_store(tmp_path: Path):
    """serve/HTTP 进程里有 _gateway_store 时必须复用它（才能停对应 handler），不能另起临时实例。"""
    rt = MdioRuntime(str(tmp_path))
    await rt.exec('worktree cfork "kid"', wt_uid="genesis")
    kid = rt._workspace.active_worktree_uid
    calls = []

    class _FakeGatewayStore:
        def unmount_by_path(self, path, *, wt_uid=None, wt_name=None):
            calls.append((path, wt_uid, wt_name))
            return {"path": path, "wt_uid": wt_uid or "fake"}

    rt._workspace._gateway_store = _FakeGatewayStore()
    res = str(await rt.exec(
        'worktree unmount --path /share/x --name kid', wt_uid="genesis"))
    assert "unmounted" in res and "mdio_error" not in res, res
    assert calls == [("/share/x", None, "kid")], calls


def test_worktree_checkout_bare_numeric_snap_hash_parses():
    """D15：数字开头的 snap hash 不开引号也必须进实参，不能变成 P001。"""
    from mdio_py.core import parse_code

    for h in ("47b35b1f29ac", "47b35b1f29ac50e2c541c37355bec178384902a750746989c15de4d5e66bb9a2"):
        ir = parse_code(f"worktree checkout {h}")
        atom = ir[0].atoms[0]
        assert atom.name == "worktree"
        checkout = atom.args[0]
        assert checkout.name == "checkout"
        assert checkout.args == [h]
    # 纯数字（无 a-f）不能被 HEX_IDENT 抢成字符串实参。
    num = parse_code("x 12345678")[0].atoms[0]
    assert num.args == [12345678]
