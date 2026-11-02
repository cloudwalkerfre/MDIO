import asyncio
import tempfile
from pathlib import Path
from mdio_py import MdioRuntime


async def run_vfs_sandbox_test():
    print("🚀 Starting MDIO VFS NEO (Opendal) Sandbox & Permissions Test...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        # 1. 创建测试目录
        (ws / "public").mkdir()
        (ws / "sub").mkdir()
        (ws / ".admin").mkdir()
        (ws / ".cls").mkdir()

        act1_dir = ws / ".cls/.actor1"
        act2_dir = ws / ".cls/.actor2"
        act1_dir.mkdir()
        act2_dir.mkdir()

        # 2. 写入物理测试数据
        (ws / "public/notice.md").write_text("# MSG\n- welcome: public_space\n", encoding="utf-8")
        (ws / ".admin/keys.md").write_text("# SECRETS\n- root_pwd: god_mode_only\n", encoding="utf-8")

        (act1_dir / "mem.md").write_text("# MEMORY\n- core: actor1_memory_data\n", encoding="utf-8")
        (act2_dir / "mem.md").write_text("# MEMORY\n- core: actor2_memory_data\n", encoding="utf-8")

        (ws / "sub/readme.md").write_text("# DOC\n- info: sub_dir_doc\n", encoding="utf-8")


        # 🌟 核心 VFS 路由表配置 (Docker -v 风格)

        # Root 令牌：挂载整个硬盘，但强制保护 .admin 目录为只读 (RO)
        storage_root = f"{ws}; {ws}/.admin:/admin:RO"
        print(f"📦 Root Universe mounted: {storage_root}")
        runtime = MdioRuntime(storage_root)

        # 利用系统派生独立 Agent 空间 (降维挂载)
        # guest 只能看到 public 文件夹
        guest_id = runtime._workspace.derive_worktree("genesis", "/public")
        # actor1 只能看到自己的内存文件
        actor1_id = runtime._workspace.derive_worktree("genesis", "/.cls/.actor1")
        # actor2 只能看到自己的内存文件
        actor2_id = runtime._workspace.derive_worktree("genesis", "/.cls/.actor2")

        results = {}

        # ========================= Case 1: Root 合法读取与只读(RO)挂载阻断 =========================
        print("⚡ [C1] Testing Root Read & RO Mount Protection...")
        # Root 读取合法文件（.admin 已被挂载到 /admin）
        res_root = await runtime.exec('$/admin/keys.md::SECRETS._b:root_pwd', wt_uid="genesis", active_doc="/default.md")

        # 🚨 试图用 Root 强行写入被声明为 :RO 的挂载点
        res_ro_write = await runtime.exec('"hacked" | +> /admin/keys.md::SECRETS._b:hack', wt_uid="genesis", active_doc="/default.md")

        results["C1"] = "god_mode_only" in str(res_root) and "R201" in str(res_ro_write)

        # ========================= Case 2: 降维沙盒 (Guest) 访问拦截 =========================
        print("⚡ [C2] Testing Sandbox Confinement (Guest)...")
        # Plan-3：Guest 由 "/public" 派生，内容经 /home binding 映射；Guest 的虚拟根 `/` 是它自己的隔离 storage（非物理 /public）。
        # Guest 试图访问 `/admin/keys.md`——它的 binding 里没有 /admin，落在自己的隔离 storage 下，必然找不到。
        res_guest = await runtime.exec('$/admin/keys.md::SECRETS._b:root_pwd', wt_uid=guest_id, active_doc="/default.md")
        results["C2"] = "no matches found" in str(res_guest) or "R201" in str(res_guest)

        # ========================= Case 3: 恶意的物理逃逸尝试 (Path Traversal) =========================
        print("⚡ [C3] Testing Physical Escape via `../../`...")
        # Actor1 试图通过 ../../ 跳出自己的空间去偷看 Actor2
        # 经过 posixpath.normpath 后，../../ 被限定在命名空间根 / 下
        # Plan-3 下 actor 自有数据位于 /home binding，故 /mem.md 不存在；核心安全断言是：绝不泄露 actor2。
        res_escape = await runtime.exec('$../../mem.md::MEMORY._b:core', wt_uid=actor1_id, active_doc="/default.md")
        results["C3"] = "actor2_memory_data" not in str(res_escape)

        # ========================= Case 4: 废弃指令提权尝试 (-u 免疫) =========================
        print("⚡ [C4] Testing Legacy Usurpation (-u Flag Immunity)...")
        # -u 参数不能覆盖 exec 的 wt_uid
        res_usurp = await runtime.exec('$/admin/keys.md::SECRETS._b:root_pwd -u root', wt_uid=guest_id, active_doc="/default.md")
        results["C4"] = "no matches found" in str(res_usurp) or "R201" in str(res_usurp)

        # ========================= Case 5: 作用域环境漂移 (`::{}` CWD) =========================
        print("⚡ [C5] Testing Scope Block CWD Drift...")
        # 验证相对路径在块作用域内的计算
        pipeline_scope = r'''
        /sub/::{
            $readme.md::DOC._b:info | +> /public/notice.md::MSG ;
        }
        '''
        await runtime.exec(pipeline_scope, wt_uid="genesis", active_doc="/default.md")
        res_scope_check = runtime.store.get_text("genesis", "/public/notice.md")
        results["C5"] = "sub_dir_doc" in res_scope_check

        # ========================= Case 6: 宿主目录宏替换 (`~/` 推导) =========================
        print("⚡ [C6] Testing Home Directory Deduction (`~/`)...")
        # `~/` 在任何沙箱内部，永远指向它自己的虚拟根 `/`
        res_home = await runtime.exec('$~/mem.md::MEMORY._b:core', wt_uid=actor2_id, active_doc="/docs/a/b.md")
        results["C6"] = "actor2_memory_data" in str(res_home)

        # ========================= Case 7: 通配符视野隔离 (Glob View Isolation) =========================
        print("⚡ [C7] Testing Glob Filtering Security...")
        # Root 执行 /*/*.md，应该能漫游全局
        res_glob_root = str(await runtime.exec('$/*/*.md:: | len', wt_uid="genesis", active_doc="/default.md"))

        # Actor1 执行虽然写着 /*/*.md，但它的天地只有自己的那一个小文件夹
        res_glob_act1 = str(await runtime.exec('$/*/*.md:: | len', wt_uid=actor1_id, active_doc="/default.md"))

        # 毫无疑问，Root 扫出的文件数一定大于被囚禁的 Actor1
        # （Actor1 内根本没有深层目录，所以 /*/*.md 返回应该是0或者只找到一层）
        results["C7"] = res_glob_act1 != res_glob_root

        # ========================= Case 8: 空指针寻址拦截 (V003 Loss of Context) =========================
        print("⚡ [C8] Testing Missing Active Document Rejection...")
        # 目录作用域下如果使用残缺指标 `$`，将因失去 active_doc 抛出 V003
        pipeline_nodoc = r'''
        /sub/::{
            $DOC._b:info | +> /public/notice.md::MSG ;
        }
        '''
        res_nodoc = await runtime.exec(pipeline_nodoc, wt_uid="genesis", active_doc="/default.md")
        results["C8"] = "V003" in str(res_nodoc)

        # ========================= Case 9: 未注册 Token 拦截 (Identity Spoofing) =========================
        print("⚡ [C9] Testing Unregistered Token Rejection...")
        # 如果有人试图通过 API 传入未经 declare 的 Token，Rust DashMap 找不到对应的 Router
        # 使用一个不可能存在的 wt_uid（例如 9999）模拟伪造身份
        res_spoof = await runtime.exec('$/public/notice.md::MSG', wt_uid=9999, active_doc="/default.md")
        results["C9"] = "not found" in str(res_spoof).lower() or "R201" in str(res_spoof)

        # ========================= Case 10: 懒加载跨界攻击防护 (Creation Sandbox Containment) =========================
        print("⚡ [C10] Testing Lazy Creation Sandbox Escape...")
        # Guest 试图通过路径归一化后的 /admin/keys.md 创建文件
        # Plan-3：Guest 没有 /admin binding，该路径落在它自己的隔离 storage 下（与父级物理路径、Root 的 /admin 互不相干）
        # 先使用 mk -f 创建文件（-f 表示自动创建父目录）
        await runtime.exec('mk -f /admin/keys.md', wt_uid=guest_id, active_doc="/default.md")
        # 再写入内容
        await runtime.exec('"hack" | +> /admin/keys.md::SECRETS._b:hack', wt_uid=guest_id, active_doc="/default.md")

        # 检查 Guest 视角下的文件内容（落在 Guest 自己的隔离 storage，非物理 /public）
        guest_fake_keys = runtime.store.get_text(guest_id, "/admin/keys.md")
        # 检查 Root 视角下的 /admin/keys.md（.admin:RO 挂载）应未被修改
        root_keys_text = runtime.store.get_text("genesis", "/admin/keys.md")

        results["C10"] = "hack" in guest_fake_keys and "hack" not in root_keys_text


        # 断言阶段

        print("\n✅ VFS NEO Evaluation:\n" + "-"*40)
        for c, passed in results.items():
            print(f"[{c}] -> {'🟢 PASSED' if passed else '🔴 FAILED'}")
        print("-" * 40)

        assert results["C1"], "C1 失败：RO 挂载点应拒绝写入"
        assert results["C2"], "Failed C2: Guest 沙箱被穿透，非授权目录被读取！"
        assert results["C3"], "C3 失败：../../ 不应泄露 actor2 数据"
        assert results["C4"], "C4 失败：-u 参数不应绕过 wt_uid 限制"
        assert results["C5"], "Failed C5: 作用域块 `::{}` 未能正确改变内部相对求值环境！"
        assert results["C6"], "Failed C6: `~/` 宿主宏未能正确推导回沙盒虚拟根！"
        assert results["C7"], "Failed C7: Root 与 Sandbox 的通配符扫描视图没有发生隔离！"
        assert results["C8"], "Failed C8: 缺省 active_doc 的异常情况未抛出 V003 拦截！"
        assert results["C9"], "C9 失败：不存在的 wt_uid 应被拦截"
        assert results["C10"], "Failed C10: 创建文件的越界防护失效，沙箱子域污染了全局环境！"

        print("🏆=============================================================🏆")
        print("🎉 ALL VFS NEO (OPENDAL) PERMISSION & SANDBOX EDGE CASES PASSED! 🎉")
        print("🏆=============================================================🏆")


if __name__ == "__main__":
    asyncio.run(run_vfs_sandbox_test())
