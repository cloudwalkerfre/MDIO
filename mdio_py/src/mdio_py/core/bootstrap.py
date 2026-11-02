"""
MDIO Bootstrap: 启动清理（proc.md）和钩子重放
"""

import json

from mdio_py.core import compile_selector_text, split_selector_addr
from mdio_py.core.text_utils import parse_kv_lines
from mdio_py.core.selector_builder import SelectorBuilder

DEBUG = False

# hook.md 里**取值必须是原文**的键：`cb` 是回调代码、`target` 是选择器——interpreter.py 的 HookOp 注册分支以
# `- cb: {cb_code!r}` 写盘，两者都可能带行内标记（`x` / **x** / [x](y)）。
_HOOK_EXACT_TEXT_KEYS = ("cb", "target")


def _hook_entry_kv(workspace, body: str) -> dict:
    """hook.md 条目体（`## hook_*` 标题行以下）→ {键: 文本}。

    结构走引擎：`workspace.to_json_text` → `bullets_to_kv`。两处刻意保留旧行解析，都不是图省事：
      · `_HOOK_EXACT_TEXT_KEYS`：AST 的 `inline_text` 会把行内标记规范化掉（实测
        ``ls `date`.md`` → `ls date.md`、`/a[x](y).md` → `/ax.md`），照搬就会在重启重放时
        注册出**被改写的回调/选择器**；故这两个键从行解析取原文并施加值后处理。
      · 引擎解析抛异常时退化为行解析：一条坏条目不该让整轮重放静默少钩。
    """
    try:
        from mdio_py.core.ast_utils import bullets_to_kv
        kv = bullets_to_kv(json.loads(workspace.to_json_text(body)))
    except Exception:
        return parse_kv_lines(body)
    exact = parse_kv_lines(body)
    for key in _HOOK_EXACT_TEXT_KEYS:
        if key in exact:
            kv[key] = exact[key]
    return kv


def _restore_one_worktree(runtime, entry: dict) -> None:
    """占位：树的恢复由 tree.ensure() 承担，本函数不做恢复。"""
    pass


def _hook_ctx_from_entry(data: dict, default_uid: str):
    """hook.md 条目 → (entry_uid, ctx_dict)。

    两条纪律（都是实测踩出来的）：
    · **条目自述优先**：`- wt_uid` 是注册时的 wt。重放时按自述归位，
      否则项目钩子会被注册到 genesis。
    · **`ctx_active_doc` 归一**：解释器把 None 写成字符串 "null"，回填成路径会让
      回调在缺省寻址时解析到 /null；统一归一成 None。
    """
    ad = data.get("ctx_active_doc")
    entry_uid = data.get("wt_uid") or default_uid
    return entry_uid, {
        "wt_uid": entry_uid,
        "active_doc": None if ad in (None, "", "null") else ad,
        "vfs_path": data.get("ctx_cwd") or "/",
    }


def _replay_hooks_from_md(workspace, wt_uid: str, hook_path: str):
    """Read hook.md via selector and re-register persistent hooks."""
    from mdio_py.core.selector_builder import SelectorBuilder
    ir = SelectorBuilder().drill("HOOKS").any_child().build()
    try:
        children = workspace.read_locator(wt_uid, hook_path, ir)
    except Exception:
        return

    count = 0
    for chunk in children:
        lines = str(chunk).split("\n")
        if not lines:
            continue
        name = lines[0].strip().lstrip("#").strip()
        if not name.startswith("hook_"):
            continue
        data = _hook_entry_kv(workspace, "\n".join(lines[1:]))

        if data.get("is_always") != "true":
            continue

        target = data.get("target", "")
        addr = split_selector_addr(target)   # 统一切分入口（file_path + md_path）
        if addr.file_path is not None:
            vfs_path, md_path = addr.file_path, addr.md_path
        else:
            vfs_path, md_path = target, ""
        if ":" in md_path and "._" not in md_path.split(":")[0]:
            parts = md_path.rsplit(":", 1)
            md_path = f"{parts[0]}._b:{parts[1]}"

        entry_uid, ctx_dict = _hook_ctx_from_entry(data, wt_uid)
        if not md_path.strip():
            # 无选择器 = 整篇文档钩子（与 interpreter HookOp 的 steps=[] 同语义，保留）
            hook_sel = SelectorBuilder().build()
        else:
            try:
                hook_sel = compile_selector_text(md_path, on_error="raise")   # 唯一编译入口
            except Exception as e:
                # 坏选择器不许回落成全文档钩子：跳过该条并告警
                # （与 register_hook 失败的逐条容错同律）。
                print(f"   ⚠️  hook 重放跳过（{entry_uid} {vfs_path}）：target 选择器编译失败 {md_path!r} → {e}")
                continue
        try:
            workspace.store.register_hook_by_uid(
                uid=entry_uid, vfs_path=vfs_path, ir_dict=hook_sel,
                cb_code=data["cb"], ctx_dict=ctx_dict, is_always=True,
            )
        except Exception as e:
            # 逐条容错：一条重放不了（wt 已消失 / 路径不在该 wt）不能拖垮其余条目
            print(f"   ⚠️  hook 重放跳过（{entry_uid} {vfs_path}）: {e}")
            continue
        if entry_uid != wt_uid:
            print(f"   ↪️  hook 归位：{vfs_path} → wt {entry_uid}（原在 {wt_uid} 的 hook.md）")
        count += 1

    if count:
        print(f"   🪝  Restored {count} hook(s) from {hook_path}")


async def bootstrap_runtime(runtime):
    print("🔧 [bootstrap] WT Plan 3 post-init bootstrap...")
    ws = runtime._workspace
    store = runtime.store

    # 列目录前先从 worktrees.md 重建树
    store.tree_ensure()

    wt_list = store.list_worktrees()
    print(f"🌳 [bootstrap] Found {len(wt_list)} WT(s)")

    for uid, name, storage_root, ext_path in wt_list:
        wt_uid = uid
        mdio = f"{storage_root}.mds"
        label = f"{name} ({uid[:8]}...)" if len(uid) > 8 else f"{name} ({uid})"
        print(f"\n📦 [bootstrap] WT: {label}")

        # 1. 清空 proc.md
        proc_path = f"{mdio}/proc.md"
        try:
            content = store.get_text("genesis", proc_path)
            if content.strip() and content.strip() != "# PROCESS_TABLE":
                await runtime.exec(f'"# PROCESS_TABLE\n" | /> {proc_path}:: -god', wt_uid="genesis")
                print(f"   🧹 Cleared {proc_path}")
        except Exception as e:
            print(f"   ⚠️  Failed to clear {proc_path}: {e}")

        # 2. 重放 hook.md
        hook_path = "/.mds/hook.md"  # L2 虚拟路径（wt_uid 参数已定位 worktree）
        try:
            _replay_hooks_from_md(ws, wt_uid, hook_path)
        except Exception as e:
            print(f"   ⚠️  Failed to replay hooks from {hook_path}: {e}")

        # 3. gateway 激活由 GatewayStore.reconcile_all 承担（state.py lifespan 内完成）

    # 3. LLM agent 恢复（遍历全部 WT，不只 genesis）
    print("\n🤖 [bootstrap] Rebuilding LLM agents...")
    llm_plugin = ws.get_plugin("LLM")
    if llm_plugin is not None:
        for uid, _name, _sr, _ep in wt_list:
            try:
                await llm_plugin._bootstrap_agents(ws, uid)
            except Exception as e:
                print(f"   ⚠️  Failed to bootstrap LLM agents for {uid[:8]}: {e}")
        print("   ✅ LLM agents restored")

    # 4. CRON 恢复
    print("\n⏰ [bootstrap] Rebuilding CRON jobs...")
    cron_plugin = ws.get_plugin("CRON")
    if cron_plugin is not None:
        try:
            await cron_plugin.bootstrap(ws)
            print("   ✅ CRON jobs restored")
        except Exception as e:
            print(f"   ⚠️  Failed to bootstrap CRON: {e}")

    print("✅ [bootstrap] Bootstrap completed.")
