import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime


async def run_advanced_pipeline():
    print("🚁 [Advanced Pipeline Benchmark] Commencing (VFS NEO)...\n" + "="*80)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        file_env = ws / "env.md"
        file_env.write_text("""# PROD
## NET
- db_host: aws-rds.internal
- db_pool: 20
## FEATURE
- use_crdt: true
- use_cache: false

# DEV
## NET
- db_host: localhost
- db_pool: 2
""")

        file_sys = ws / "sys.md"
        file_sys.write_text("""# LOGS
## RECENT
- init: sys_start
""")

        storage_root = str(ws)
        print(f"📦 Virtual Workspace Mounted at: {storage_root}")

        runtime = MdioRuntime(storage_root)
        await runtime.exec('mk -f /system_error.md', wt_uid="genesis")
        await runtime.exec('mk -f /env.md', wt_uid="genesis")
        await runtime.exec('mk -f /sys.md', wt_uid="genesis")

        # VFS 下的绝佳管道
        pipeline_code = r'''
        "60" | /> *..NET._b:db_pool ;
        $PROD.NET._b:db_host | fmt -t "- target: {}" | +> /sys.md::LOGS.RECENT ;
        -> PROD.FEATURE.use_cache ;
        '''

        print(f"⚡ Launching Composite Payload:\n{pipeline_code}\n" + "-"*40)

        err_msg = await runtime.exec(pipeline_code, active_doc="/env.md", wt_uid="genesis")

        if str(err_msg) != "OK" and "mdio_error" in str(err_msg):
            print(f"⚠️ Pipeline returned warning/error:\n{err_msg}")

        env_result = runtime.store.get_text("genesis", "/env.md")
        sys_result = runtime.store.get_text("genesis", "/sys.md")

        print("\n=== 📄 Final [env.md] State ===")
        print(env_result.strip())
        print("\n=== 📄 Final [sys.md] State ===")
        print(sys_result.strip())

        assert "db_pool: 60" in env_result, "Broad overwriting failed!"
        assert env_result.count("db_pool: 60") == 1, "First target overwritten"
        assert "db_pool: 2" in env_result, "Second target unchanged (Slice(0) selects first)"

        assert "use_cache: false" not in env_result, "Deletion target survived!"
        assert "use_cache:" not in env_result, "Deletion left empty traces."
        assert "use_crdt: true" in env_result, "Collateral damage applied during deletion!"

        assert "- target: aws-rds.internal" in sys_result, "Cross-file Data-driven append failed."

        print("\n🏆========================================================🏆")
        print("🎉 ALL ADVANCED MUTATION & BROAD-CAST PIPELINE TESTS PASSED! 🎉")
        print("🏆========================================================🏆\n")


if __name__ == "__main__":
    asyncio.run(run_advanced_pipeline())
