import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime


async def run_glob_and_scope_vfs_test():
    print("🚀 Starting MDIO `tree` Glob & Namespace Isolation Test (VFS)...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        docs_dir = ws / "docs"
        docs_dir.mkdir()
        nested_dir = docs_dir / "nested"
        nested_dir.mkdir()

        (ws / "root_app.md").write_text("# ROOT APP\n")
        (docs_dir / "a.md").write_text("# DOC A\n## INFO\n- txt: A")
        (docs_dir / "b.md").write_text("# DOC B\n## INFO\n- txt: B")
        (nested_dir / "c.md").write_text("# DOC C\n## INFO\n- txt: C")

        # 隐藏文件 (VFS 依然可见，隔离靠 Namespace 路径)
        (nested_dir / ".sys_secret.md").write_text("# TOP SECRET\n- key: 123")

        # 1. 初始化 genesis 命名空间
        storage_root = str(ws)
        print(f"📦 [Universe] Root mounted: {storage_root}")
        runtime = MdioRuntime(storage_root)
        await runtime.exec('mk -f /REPORT.md', wt_uid="genesis")
        await runtime.exec('mk -f /REPORT_JAIL.md', wt_uid="genesis")
        await runtime.exec('mk -f /default.md', wt_uid="genesis")

        # 派生受限命名空间，只映射 /docs/nested
        # 将物理 nested_dir 挂载为虚拟根
        jail_id = runtime._workspace.derive_worktree("genesis", "/docs/nested")
        print(f"📦 [Universe] nested_jail mounted (wt_uid={jail_id}): {str(nested_dir)}")


        # 阶段 1：全局视角扫描 (root 身份)

        print("\n⚡ Phase 1: Global Scanning (root Universe)")

        pipeline_p1 = r'''
        "docs/*.md::" | tree -d 0 | fmt -t """\nT1_SINGLE_LEVEL:\n{}""" | +> /REPORT.md:: ;
        "docs/**/*.md::" | tree -d 0 | fmt -t """\nT2_RECURSIVE:\n{}""" | +> /REPORT.md:: ;
        "/docs/nested/.sys_secret.md::" | tree | fmt -t """\nT3_HIDDEN_FILE:\n{}""" | +> /REPORT.md:: ;
        '''

        res_p1 = await runtime.exec(pipeline_p1, active_doc="/default.md", wt_uid="genesis")
        if "error" in str(res_p1).lower(): print(f"⚠️ Phase 1 执行预警: {res_p1}")


        # 阶段 2：受限视野扫描 (nested_jail 身份)

        print(f"🔍 Debug: Checking files in jail namespace (wt_uid={jail_id})")
        files = runtime._workspace.expand_path_pattern(jail_id, "/*.md")
        print(f"   Matched files: {files}")
        for f in files:
            content = runtime.store.get_text(jail_id, f)
            print(f"   Content of {f}:\n{content}\n---")

        print("\n⚡ Phase 2: Restricted Perspective (nested_jail Universe)")

        # 在子命名空间中创建目标文件
        await runtime.exec('mk -f "/REPORT_JAIL.md"', wt_uid=jail_id)

        # 直接在它的世界根目录下扫
        pipeline_p2 = r'''
        "/home/*.md::" | tree -d 0 | fmt -t """\nT4_JAIL_CONTENT:\n{}""" | +> /REPORT_JAIL.md:: ;
        '''
        res_p2 = await runtime.exec(pipeline_p2, active_doc="/default.md", wt_uid=jail_id)
        if "error" in str(res_p2).lower(): print(f"⚠️ Phase 2 执行预警: {res_p2}")


        # 结果校验

        final_text_p1 = runtime.store.get_text("genesis", "/REPORT.md")
        final_text_p2 = runtime.store.get_text(jail_id, "/REPORT_JAIL.md")

        print("\n✅ Final Report P1 [/REPORT.md]:\n" + "-"*40)
        print(final_text_p1.strip())
        print("\n✅ Final Report P2 [/REPORT_JAIL.md]:\n" + "-"*40)
        print(final_text_p2.strip())


        # 断言剖析

        assert final_text_p1.strip(), "REPORT.md 应生成"

        # T1：单层匹配
        t1_section = final_text_p1.split("T1_SINGLE_LEVEL:\n")[1].split("T2_RECURSIVE:\n")[0]
        assert "📄 a.md" in t1_section and "📄 b.md" in t1_section
        assert "c.md" not in t1_section, "T1: 单层结构发生了越级递归泄露"

        # T2：递归匹配
        t2_section = final_text_p1.split("T2_RECURSIVE:\n")[1].split("T3_HIDDEN_FILE:\n")[0]
        assert "📄 a.md" in t2_section and "📄 c.md" in t2_section, "T2: 递归扫描 `**/*.md` 丢失深层文件"

        # T3：dotfile 默认被忽略 —— tree（走 glob/expand_path_pattern）不会暴露 . 开头文件（root 亦然）
        t3_section = final_text_p1.split("T3_HIDDEN_FILE:\n")[1]
        assert "TOP SECRET" not in t3_section, "T3: dotfile 不应被默认 tree 暴露（MDIO 默认忽略 dot file/folder）"

        # T4：沙盒隔离验证（Plan-3 下 jail 内容挂在 /home，故扫描 /home/*.md）
        # nested_jail 只能看到自己的 c.md，看不到父宇宙的 root_app/a/b
        assert "📄 c.md" in final_text_p2, "T4: 无法在沙盒根目录发现文件"
        assert "root_app.md" not in final_text_p2, "T4: jail 不应看到父命名空间根目录的文件"
        assert "a.md" not in final_text_p2, "T4: jail 不应看到同级命名空间的文件"


        # 路径穿越逃逸探测

        # 验证 get_text 能否通过 `..` 横跳脱逃沙盒
        escape_res = runtime.store.get_text(jail_id, "/../../../REPORT.md")
        # get_text 在找不到时会平稳返回空字符串 ""
        assert "T1_SINGLE_LEVEL" not in escape_res, "路径穿越不应逃出 jail 命名空间"
        assert escape_res == "", "通过路径拼接访问越域文件没有被正确封锁"
        print("✅ [Security] 路径清洗与 `..` 上跳逃逸拦截成功！")

        print("\n🏆========================================================🏆")
        print("🎉 MULTI-LEVEL VFS GLOB ALGORITHM & NAMESPACE SHIELD PASSED! 🎉")
        print("🏆========================================================🏆\n")


if __name__ == "__main__":
    asyncio.run(run_glob_and_scope_vfs_test())
