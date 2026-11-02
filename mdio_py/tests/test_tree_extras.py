import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime


async def run_tree_extra_fuzz_test():
    print("🚀 Starting MDIO `tree` Extra Fuzzing & Boundaries Test (VFS NEO)...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)


        # 0. 文件准备床

        app_file = ws / "app.md"
        app_file.write_text("# SYSTEM\n## CONFIG\n- host: 127\n- port: 80\n", encoding="utf-8")

        # 空文件
        (ws / "empty.md").write_text("", encoding="utf-8")

        # 只有 Strip 的文件 (没有 Header)
        (ws / "strips_only.md").write_text("- item1\n- item2: val\n- just text\n", encoding="utf-8")

        # 混合权限目录
        mixed_dir = ws / "mixed_perms"
        mixed_dir.mkdir()
        (mixed_dir / "ok.md").write_text("# OK", encoding="utf-8")
        (mixed_dir / ".hidden.md").write_text("# HIDDEN", encoding="utf-8")

        # 极深嵌套目录 (12层深)
        deep_dir = ws
        for i in range(12):
            deep_dir = deep_dir / f"L{i}"
            deep_dir.mkdir()
        (deep_dir / "deep.md").write_text("# MARIANA_TRENCH\n", encoding="utf-8")

        # 获取基于 ws 的相对路径并转为 Posix (例如 "L0/L1/.../deep.md")
        deep_rel_path = (deep_dir / "deep.md").relative_to(ws).as_posix()

        # 混合缩进 (空格和Tab)
        mixed_indent_file = ws / "indent.md"
        mixed_indent_file.write_text(
            "# INDENT\n"
            "- list:\n"
            "    - space_sub\n"
            "\t- tab_sub\n"
            "```python\n"
            "\tdef func():\n"
            "        pass\n"
            "```\n",
            encoding="utf-8"
        )

        # 1. 初始化 genesis 命名空间
        storage_root = str(ws)
        runtime = MdioRuntime(storage_root)
        await runtime.exec('mk -f /REPORT.md', wt_uid="genesis")


        # 2. 探测管线 (完全遵循 VFS NEO 规范)

        pipeline_code = f'''
        "SYSTEM.CONFIG" | tree -d 3 -detail 0 | fmt -t """\n[[C1_DETAIL_0]]\n{{}}""" | +> /REPORT.md:: ;
        "SYSTEM" | tree -d 999 -detail 1 | fmt -t """\n[[C2_DEPTH_999]]\n{{}}""" | +> /REPORT.md:: ;
        "/ghost_file.md::*" | tree -d 3 -detail 1 | fmt -t """\n[[C3_GHOST]]\n{{}}""" | +> /REPORT.md:: ;
        "/empty.md::*" | tree -d 3 -detail 1 | fmt -t """\n[[C4_EMPTY]]\n{{}}""" | +> /REPORT.md:: ;
        "/strips_only.md::*" | tree -d 3 -detail 2 | fmt -t """\n[[C5_STRIPS]]\n{{}}""" | +> /REPORT.md:: ;
        "/mixed_perms/*.md::*" | tree -d 3 -detail 1 | fmt -t """\n[[C6_MIXED_PERMS]]\n{{}}""" | +> /REPORT.md:: ;
        "/{deep_rel_path}::*" | tree -d 3 -detail 1 | fmt -t """\n[[C7_DEEP_DIR]]\n{{}}""" | +> /REPORT.md:: ;
        "/indent.md::*" | tree -d 3 -detail 2 | fmt -t """\n[[C8_MIXED_INDENT]]\n{{}}""" | +> /REPORT.md:: ;
        '''

        print("⚡ Processing Fuzzing Pipeline...")
        res = await runtime.exec(pipeline_code, active_doc="/app.md", wt_uid="genesis")
        if "error" in str(res).lower(): print(f"⚠️ 执行预警 (可能被系统安全拦截): {res}")

        # 读取独立落盘的报告文件
        final_text = runtime.store.get_text("genesis", "/REPORT.md")

        print("\n✅ Extra Tests Report [/REPORT.md]:\n" + "-"*40)
        print(final_text.strip())
        print("-" * 40)


        # 断言区 (使用精确切块，绝不依赖隐性换行)

        assert final_text.strip(), "⚠️ 严重异常：REPORT.md 未能成功物理落盘！"

        # 1. Detail 0 必须屏蔽一切 Key
        c1 = final_text.split("[[C1_DETAIL_0]]")[1].split("[[C2_DEPTH_999]]")[0]
        assert "## CONFIG" in c1, "C1: 缺少 Head！"
        assert ":host" not in c1 and "strips" not in c1, "C1: Detail=0 泄漏了内容细节！"

        # 2. 深度 999 防爆
        c2 = final_text.split("[[C2_DEPTH_999]]")[1].split("[[C3_GHOST]]")[0]
        assert "## CONFIG" in c2 and ":host" in c2, "C2: Depth 999 导致解析失败！"

        # 3. 幽灵文件
        c3 = final_text.split("[[C3_GHOST]]")[1].split("[[C4_EMPTY]]")[0]
        assert "no items found" in c3, "C3: 幽灵文件没有按预期返回空提示！"

        # 4. 空文件
        c4 = final_text.split("[[C4_EMPTY]]\n")[1].split("[[C5_STRIPS]]\n")[0]
        assert "no items found" in c4, "C4: 已存在的空文件并未返回 no items found，处理异常！"

        # 5. 只有 Strip 的文件
        c5 = final_text.split("[[C5_STRIPS]]\n")[1].split("[[C6_MIXED_PERMS]]\n")[0]
        assert ":item2" in c5, "C5: 文件没有 Header 时 key 提取丢失"
        assert "├── - item1" in c5 or "├── -" in c5, "C5: 无名 Strip 未能被正确展示在顶层"

        # 6. 通配符混合隐藏文件 (*.md)
        c6 = final_text.split("[[C6_MIXED_PERMS]]")[1].split("[[C7_DEEP_DIR]]")[0]
        assert "ok.md" in c6, "C6: 合法文件 ok.md 扫描丢失！"
        assert ".hidden" not in c6, "C6: `*.md` 通配符展开没有静默剔除由于安全策略被隐藏的 dotfile！"

        # 7. 极深嵌套 (不报 OS Error，正常解析)
        c7 = final_text.split("[[C7_DEEP_DIR]]")[1].split("[[C8_MIXED_INDENT]]")[0]
        assert "MARIANA_TRENCH" in c7, "C7: 极深目录路径超过限制导致树解析崩溃！"

        # 8. 混合排版 (空格与Tab 造成的不规则结构缩进)
        c8 = final_text.split("[[C8_MIXED_INDENT]]")[1]
        assert "1 code blocks" in c8, "C8: 代码块中的 Tab 混用导致底层状态机 Parser 检测 CodeFence 失效！"

        print("🏆========================================================🏆")
        print("🎉 SUPER FUZZING: ALL 8 PROPOSED VFS EDGE CASES PASSED! 🎉")
        print("🏆========================================================🏆\n")


if __name__ == "__main__":
    asyncio.run(run_tree_extra_fuzz_test())
