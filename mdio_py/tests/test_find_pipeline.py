import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime


async def run_find_integration_test():
    print("🚀 Starting MDIO `find` Extreme Integration Test (VFS Edition)...\n" + "="*60)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        app_file = ws / "app.md"
        app_file.write_text(
            "# SYSTEM\n"
            "## CONFIG\n"
            "### DATABASE\n"
            "- host: 127.0.0.1\n"
            "- pass: \"super_secret_password_1234\"\n"
            "\n"
            "## LOGS\n"
            "- user_1: 正常登录！欢迎来到地球 🌍\n"
            "\n"
            "- err1: system crashed due to OOM killer trigger.\n"
            "\n"
            "- err2: FATAL exception in thread main 线程崩溃！\n"
            "- err3: 最后一个小错误\n"
            "\n"
            "# RESULTS\n\n"
            "# MULTI_RESULTS\n\n"
        )

        db_file = ws / "db.md"
        db_file.write_text(
            "# SYSTEM\n"
            "## LOGS\n"
            "- warn1: db connection TimeOut after 30s. 请重试！\n"
        )

        # 2. 挂载 VFS 宇宙
        storage_root = str(ws)
        print(f"📦 VFS Universe mounted with rules: {storage_root}")

        runtime = MdioRuntime(storage_root)
        await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

        # 3. 管道执行 (基于纯虚拟语义系)
        pipeline_code = r'''
        "SYSTEM..*" | find -q "password" -c [5, 10] | +>RESULTS ;
        "SYSTEM.LOGS" | find -q "地球" -c [2, 2] | +>RESULTS ;
        "*.md::SYSTEM.LOGS" | find -q ["timeout", "/崩溃/i", "/OO[A-Z]/i"] -c [12, 12] -i | +>MULTI_RESULTS ;
        '''

        print("⚡ Executing Pipeline...")

        await runtime.exec(
            pipeline_code,
            active_doc="/app.md",
            wt_uid="genesis"
        )

        # 4. 结果校验
        final_text = runtime.store.get_text("genesis", "/app.md")
        print("✅ Final File State [/app.md]:\n", final_text)

        assert "- path: app.md::SYSTEM.CONFIG.DATABASE:pass" in final_text, "深度提取失败"
        assert "cret_password_1234" in final_text, "上下文截断错误"
        assert "- path: app.md::SYSTEM.LOGS:user_1" in final_text, "中文定位失败"
        assert "到地球 🌍" in final_text, "UTF-8边界乱码"
        assert "- path: app.md::SYSTEM.LOGS:err1" in final_text, "OOM节点丢失"
        assert "- path: app.md::SYSTEM.LOGS:err2" in final_text, "崩溃节点丢失"
        assert "- path: db.md::SYSTEM.LOGS:warn1" in final_text, "多文件搜索失败"

        print("\n🎉 ALL EXTREME GREP TESTS PASSED (VFS Ready)!")

        # BM25 集成测试（复用本文件的所有测试数据）
        print("\n⚡ BM25 Pipeline Integration Tests...\n")

        print("  BM25-P1: Cross-file BM25 ... ", end="", flush=True)
        r = await runtime.exec(
            'find -bm25 "password timeout connection" -p / -n 5',
            wt_uid="genesis",
        )
        lines = r.strip().split("\n")
        assert len(lines) >= 1, f"BM25-P1: no results"
        assert any("app.md" in l or "db.md" in l for l in lines), f"BM25-P1: bad results"
        print("PASSED")

        print("  BM25-P2: Scoped to db.md ... ", end="", flush=True)
        r = await runtime.exec(
            'find -bm25 "timeout 重试" -p /db.md -n 3',
            wt_uid="genesis",
        )
        assert "db.md" in r and "app.md" not in r, f"BM25-P2: scope leak: {r[:200]}"
        print("PASSED")

        print("  BM25-P3: CJK query hits ... ", end="", flush=True)
        r = await runtime.exec(
            'find -bm25 "登录 地球 崩溃 线程" -p / -n 3',
            wt_uid="genesis",
        )
        assert "app.md" in r, f"BM25-P3: no CJK match"
        print("PASSED")

        print("  BM25-P4: Section ranking ... ", end="", flush=True)
        r = await runtime.exec(
            'find -bm25 "host 127 password" -p /app.md -n 3',
            wt_uid="genesis",
        )
        assert "DATABASE" in r or "app.md" in r, f"BM25-P4: no results: {r[:200]}"
        print("PASSED")

        print("✅ ALL BM25 PIPELINE TESTS PASSED!")


if __name__ == "__main__":
    asyncio.run(run_find_integration_test())
