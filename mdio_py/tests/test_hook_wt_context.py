"""hook 回调在**注册它的那个 wt** 里执行。

配置：`events.py` 调 `runtime.exec(cb_code, ctx=ctx)`；若 `MdioRuntime.exec` 不读 `ctx`
（`initial_ctx` 由显式参数构造、`wt_uid` 默认 genesis），非 genesis wt 的钩子回调会在
genesis 里执行：回调里的地址在 genesis 不存在 → `[I500] File does not exist`，且该错误串
被 `asyncio.create_task` 丢弃，外部看到的就是「钩子没触发」。
"""
import asyncio


async def test_hook_callback_runs_in_registering_wt(runtime, tmp_path):
    ws = runtime._workspace
    await runtime.exec("mk -f /system_error.md", wt_uid="genesis")

    proj = tmp_path / "proj"
    (proj / "wt").mkdir(parents=True)
    (proj / "wt" / "ui.md").write_text(
        "# UI\n\n## STATE\n\n- status: idle\n\n## LOG\n\n- marker: (空)\n",
        encoding="utf-8",
    )

    proj_uid, is_new = ws.load_worktree(str(proj))
    assert is_new, "项目 wt 应被注册为新 wt"

    await runtime.exec(
        r'''hook -target /ui.md::STATE._b:status -cb '"CB_OK" | /> /ui.md::LOG._b:marker' -is_always''',
        wt_uid=proj_uid,
    )
    await asyncio.sleep(0.1)

    await runtime.exec('"go" | /> /ui.md::STATE._b:status', wt_uid=proj_uid)
    await asyncio.sleep(0.5)

    # 索引级判据（防"静默跳过"）：钩子必须真的登记在项目 wt 上，且没落到 genesis
    assert runtime.store.hook_count_for_worktree(proj_uid) >= 1, "钩子未登记进项目 wt 的索引"
    assert runtime.store.hook_count_for_worktree("genesis") == 0, "钩子误落到 genesis 的索引"

    proj_text = runtime.store.get_text(proj_uid, "/ui.md")
    assert "CB_OK" in proj_text, (
        "回调没有在注册它的 wt 里落笔（ctx 被丢弃 ⇒ 回调跑在 genesis）: " + repr(proj_text)
    )

    gen_text = ""
    try:
        gen_text = runtime.store.get_text("genesis", "/ui.md")
    except Exception:
        gen_text = ""
    assert "CB_OK" not in gen_text, "回调落到了 genesis（说明 ctx 未被尊重）"

async def test_hook_is_always_persists_in_derived_wt(runtime):
    """派生 wt：-is_always 条目应落在该 wt 自己的 /.mds/hook.md，且回调在该 wt 执行。

    这是本修的下游风险点：持久化写（interpreter 里 '+> …/.mds/hook.md::HOOKS -f'）现在带 ctx，
    落点从 genesis 变成注册 wt 自己。派生 wt 若不自动建 .mds/，这一步会失败。
    """
    ws = runtime._workspace
    await runtime.exec("mk -f /system_error.md", wt_uid="genesis")

    uid = ws.derive_worktree("genesis", "")
    await runtime.exec('"# D\n\n## S\n\n- a: 1\n\n## L\n\n- m: (空)" | /> /d.md -f -god', wt_uid=uid)

    r = await runtime.exec(
        "hook -target /d.md::S._b:a -cb '" + '\"DW\" | /> /d.md::L._b:m' + "' -is_always",
        wt_uid=uid,
    )
    assert "mdio_error" not in str(r), f"注册报错: {r}"

    own = runtime.store.get_text(uid, "/.mds/hook.md")
    assert "d.md::S._b:a" in own, f"派生 wt 的 hook.md 未收到条目: {own[:300]!r}"

    await runtime.exec('\"2\" | /> /d.md::S._b:a', wt_uid=uid)
    await asyncio.sleep(0.5)
    assert "DW" in runtime.store.get_text(uid, "/d.md"), "回调未在派生 wt 落笔"


def test_hook_ctx_from_entry_normalizes():
    """重放条目 → ctx 的两条纪律：wt 自述优先；字符串 null 归一成 None。"""
    from mdio_py.core.bootstrap import _hook_ctx_from_entry

    uid, ctx = _hook_ctx_from_entry({"wt_uid": "projX", "ctx_active_doc": "null", "ctx_cwd": "/notes"}, "genesis")
    assert uid == "projX" and ctx["wt_uid"] == "projX", ctx
    assert ctx["active_doc"] is None, "ctx_active_doc 的字符串 null 应归一成 None: %r" % (ctx,)
    assert ctx["vfs_path"] == "/notes", ctx

    uid2, ctx2 = _hook_ctx_from_entry({}, "genesis")
    assert uid2 == "genesis" and ctx2["vfs_path"] == "/" and ctx2["active_doc"] is None

    _, ctx3 = _hook_ctx_from_entry({"ctx_active_doc": "/a.md"}, "genesis")
    assert ctx3["active_doc"] == "/a.md", "真实 active_doc 不能被吞"


async def test_deldoc_purges_persisted_entries(runtime):
    """-deldoc 必须连 hook.md 里的条目一起删，否则重启重放会把撤掉的钩子复活。"""
    ws = runtime._workspace
    await runtime.exec("mk -f /system_error.md", wt_uid="genesis")
    for name in ("a", "b"):
        await runtime.exec("mk -f /%s.md" % name, wt_uid="genesis")
        await runtime.exec('"# %s\\n\\n## S\\n\\n- k: 1" | /> /%s.md' % (name.upper(), name), wt_uid="genesis")
    await runtime.exec("hook -target /a.md::S._b:k -cb '\"CB\" | /> /a.md::S._b:k' -is_always", wt_uid="genesis")
    await runtime.exec("hook -target /a.md::S._b:k -cb '\"CB\" | /> /a.md::S._b:k' -is_always", wt_uid="genesis")
    await runtime.exec("hook -target /b.md::S._b:k -cb '\"CB\" | /> /b.md::S._b:k' -is_always", wt_uid="genesis")

    md0 = runtime.store.get_text("genesis", "/.mds/hook.md")
    assert md0.count("## hook_") == 3, md0
    r = await runtime.exec("hook -deldoc /a.md", wt_uid="genesis")
    assert "持久条目 -2" in str(r), r
    md1 = runtime.store.get_text("genesis", "/.mds/hook.md")
    assert md1.count("## hook_") == 1 and "/a.md::" not in md1, md1

    # 模拟重启：清 Rust 侧（不动 hook.md）→ 重放 → 被判死的 a.md 不得复活
    runtime.store.clear_worktree_hooks("genesis")
    from mdio_py.core.bootstrap import _replay_hooks_from_md
    _replay_hooks_from_md(ws, "genesis", "/.mds/hook.md")
    assert runtime.store.hook_count_for_worktree("genesis") == 1, "撤掉的钩子在重启重放后复活了"
