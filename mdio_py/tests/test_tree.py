import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime
from mdio_py.core.error import MdioError


async def run_vfs_tree_integration_test():
    print("🚀 Starting MDIO VFS Namespace & `tree` Ultimate Edge Case Test...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)
        dummy_root = ws / ".scratchpad"
        dummy_root.mkdir(parents=True)

        (ws / "src").mkdir()
        (ws / "logs").mkdir()

        app_file = ws / "app.md"
        app_file.write_text(
            "# SYSTEM\n"
            "## CONFIG\n"
            "- host: 127.0.0.1\n"
            "- port: 8080\n"
            "- 密码不能外泄\n"
            "这是一段游离的 Paragraph 段落。\n"
            "## DEEP_NEST\n"
            "### LEVEL3\n"
            "#### LEVEL4\n"
            "- target: reached\n"
            "\n"
            "# EMOJI_TEST\n"
            "- cn: 这是一个含 Emoji 的测试 🚀\n"
            "- jp: こんにちは世界\n"
            "\n"
            "# MIXED_BLOCKS\n"
            "- status: unknown\n"
            "```python\n"
            "def foo():\n"
            "    pass\n"
            "```\n"
            "一段孤立的话。\n"
            "又一段孤立的话。\n"
            "- 匿名的 Strip\n"
            "\n"
            "# TESTS\n\n",
            encoding="utf-8"
        )

        utils_file = ws / "src" / "utils.md"
        utils_file.write_text("# UTILS\n## MATH\n- add: func\n- sub: func\n", encoding="utf-8")

        logs_file = ws / "logs" / "sys.md"
        logs_file.write_text("# LOGS\n- error: fatal\n", encoding="utf-8")


        # 1. 初始化 genesis 命名空间

        storage_root = str(ws)
        print(f"📦 [Genesis] Base Universe mounted with: {storage_root}")
        runtime = MdioRuntime(storage_root)
        ws_mount = runtime._workspace

        await runtime.exec('mk -f /data/app.md', wt_uid="genesis")


        # 2. 派生子命名空间

        print("\n🧬 [Derivation] 正在基于本源宇宙派生各司其职的子集...")

        guest_mount_str = "/.scratchpad; /data::RO"
        wt_uid_guest = ws_mount.derive_worktree("genesis", guest_mount_str)
        print(f"   ↳ [guest_ro] 诞生! wt_uid={wt_uid_guest} 挂载策略: {guest_mount_str}")

        logger_mount_str = "/logs"
        wt_uid_logger = ws_mount.derive_worktree("genesis", logger_mount_str)
        print(f"   ↳ [agent_logger] 诞生! wt_uid={wt_uid_logger} 挂载策略: {logger_mount_str}")

        hacker_mount_str = "/.scratchpad; /data::RW"  # 试图把父级 RO 的 /data 升权为 RW
        # Plan-3 不在创建期拒绝升权，而是依赖运行时 resolve 的模式合并（祖先 RO 向下传播）兜底拦截
        hacker_id = ws_mount.derive_worktree(wt_uid_guest, hacker_mount_str)
        escalation = await runtime.exec('"SHOULD_FAIL" | +> /data/app.md::SYSTEM._b:hack', wt_uid=hacker_id, active_doc="/default.md")
        print(f"   🛡️ [Security] 升权写入运行时拦截结果: {escalation}")
        assert "R201" in str(escalation) or "RO" in str(escalation), f"❌ RO 父级升权 RW 后写入未被拦截: {escalation}"


        # 3. genesis 命名空间：tree 查询测试

        print("\n⚡ [Test 1] 正在本源宇宙 (root) 下执行 AST 深度解析...")
        pipeline_normal = r'''
        "NONEXISTENT" | tree | fmt -t """\nC1_EMPTY:\n{}""" | +>TESTS ;
        "SYSTEM.CONFIG" | tree -d 3 -detail 1 | fmt -t """\nC2_DETAIL1:\n{}""" | +>TESTS ;
        "SYSTEM.CONFIG" | tree -d 3 -detail 2 | fmt -t """\nC3_DETAIL2:\n{}""" | +>TESTS ;
        "SYSTEM.DEEP_NEST" | tree -d 0 -detail 1 | fmt -t """\nC4_DEPTH0:\n{}""" | +>TESTS ;
        "SYSTEM.DEEP_NEST" | tree -d 1 -detail 1 | fmt -t """\nC5_DEPTH1:\n{}""" | +>TESTS ;
        "EMOJI_TEST" | tree -d 2 -detail 2 | fmt -t """\nC6_EMOJI:\n{}""" | +>TESTS ;
        "MIXED_BLOCKS" | tree -d 3 -detail 2 | fmt -t """\nC7_MIXED:\n{}""" | +>TESTS ;
        "/src/utils.md::*" | tree -d 3 -detail 1 | fmt -t """\nC8_CROSS_FILE:\n{}""" | +>TESTS ;
        "/src/*.md::*" | tree -d 3 -detail 1 | fmt -t """\nC9_WILDCARD_DIR:\n{}""" | +>TESTS ;
        '''

        await runtime.exec(pipeline_normal, active_doc="/app.md", wt_uid="genesis")

        final_text = runtime.store.get_text("genesis", "/app.md")
        assert "C1_EMPTY" in final_text and "tree: no items found" in final_text, "Failed C1"

        c2_section = final_text.split("C2_DETAIL1:")[1].split("C3_DETAIL2:")[0]
        assert ":host" in c2_section and "strips" not in c2_section, "Failed C2"

        c7_section = final_text.split("C7_MIXED:")[1].split("C8_CROSS_FILE:")[0]
        assert "1 code blocks" in c7_section and "1 paragraphs" in c7_section, "Failed C7"

        c9_section = final_text.split("C9_WILDCARD_DIR:")[1]
        assert "📄 utils.md" in c9_section, "Failed C9"

        print("   ✅ [Test 1] AST节点树推导计算通过")


        # 4. 新参数测试：-a, -head, -key

        print("\n⚡ [Test 4] 测试 tree 新参数 -a, -head, -key ...")
        # 创建隐藏文件/目录（Plan-3 VFS 为内存态：磁盘直写不可见，须经 VFS 写入才能被 tree 扫到）
        await runtime.exec('"""# SECRET\n- data: top_secret\n""" | +> /.secret/hidden.md:: -god', wt_uid="genesis")
        await runtime.exec('"""# ROOT_HIDDEN\n- flag: invisible\n""" | +> /.hidden_root.md:: -god', wt_uid="genesis")
        await runtime.exec('"""# INTERNAL\n- status: hidden\n""" | +> /src/.internal.md:: -god', wt_uid="genesis")

        # 强制刷新 VFS 缓存（调用一次 expand_glob 触发扫描）
        ws_mount.expand_path_pattern("genesis", "/*", include_hidden=True)

        async def run_tree_and_print(cmd, description):
            print(f"\n📌 {description}")
            result = await runtime.exec(cmd, active_doc="/app.md", wt_uid="genesis")
            print(result)
            return result

        # 4.1 默认（不显示隐藏）
        res_default = await run_tree_and_print(
            'tree -p / -d 2 -detail 1',
            "默认 tree（应不显示隐藏文件/目录）"
        )
        assert ".secret" not in res_default and ".hidden_root.md" not in res_default

        # 4.2 -a 显示隐藏
        res_a = await run_tree_and_print(
            'tree -a -p / -d 2 -detail 1',
            "tree -a（应显示隐藏文件/目录）"
        )
        # 只检查根目录下的隐藏文件，不检查隐藏目录本身（tree 不显示目录节点）
        assert ".hidden_root.md" in res_a, "-a 未显示隐藏文件"

        # 4.3 -head false
        res_head_false = await run_tree_and_print(
            'tree -head false -p /app.md -d 3',
            "tree -head false（应只显示文件统计，无 Head 节点）"
        )
        assert "SYSTEM" not in res_head_false and "CONFIG" not in res_head_false

        # 4.4 -key true
        res_key_true = await run_tree_and_print(
            'tree -key true -p SYSTEM.CONFIG -d 2',
            "tree -key true（应显示 Strip 键名）"
        )
        assert ":host" in res_key_true and ":port" in res_key_true

        # 4.5 冲突测试
        res_conflict = await run_tree_and_print(
            'tree -head false -key true -p SYSTEM.CONFIG',
            "tree -head false -key true（应报错）"
        )
        assert "mdio_error" in res_conflict and "conflict" in res_conflict.lower()

        # 4.6 组合
        res_a_head_false = await run_tree_and_print(
            'tree -a -head false -p / -d 2',
            "tree -a -head false（显示隐藏文件且不展开内部）"
        )
        assert ".hidden_root.md" in res_a_head_false and "SYSTEM" not in res_a_head_false

        print("   ✅ [Test 4] 所有新参数验证通过")


        # 5. 子命名空间 A：只读权限测试 (guest_ro)

        print("\n⚡ [Test 2] 切换至 'guest_ro'，尝试修改被挂载至 /data 的核心文件...")

        # guest_ro 试图写入 RO 挂载点
        evil_code = '"*" | tree | +> /data/app.md::TESTS'
        ro_result = await runtime.exec(evil_code, active_doc="/default.md", wt_uid=wt_uid_guest)

        assert "mdio_error" in ro_result, "RO 拦截失效，未返回错误标志"
        assert "RO" in ro_result or "Read-Only" in ro_result or "R201" in ro_result, "拦截原因错误"
        print("   ✅ [Test 2] RO 写入被拒绝")


        # 6. 子命名空间 B：信息隔离测试 (agent_logger)

        print("\n⚡ [Test 3] 切换至 'agent_logger' 宇宙，开展视界嗅探...")

        # 🚨 加上 :: ，强制遍历它世界里的所有纯文件
        log_sight = await runtime.exec('"/*::" | tree', active_doc="/default.md", wt_uid=wt_uid_logger)
        print(f"👀 agent_logger 眼中的世界树:\n{log_sight}\n")

        assert "app.md" not in log_sight, "agent_logger 不应看到 app.md"
        assert "sys.md" in log_sight, "自身根目录映射丢失！"
        print("   ✅ [Test 3] 命名空间隔离正常")

        print("\n🏆========================================================🏆")
        print("🎉 VFS 挂载法则、派生溯源、读写屏障 全线校验通过！ 🎉")
        print("🏆========================================================🏆\n")


if __name__ == "__main__":
    asyncio.run(run_vfs_tree_integration_test())
