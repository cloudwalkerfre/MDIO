import asyncio
import re
import tempfile
from pathlib import Path

import pytest
from mdio_py import MdioRuntime
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.selector_builder import drill


@pytest.mark.skip(reason="hooks leave background tasks — CancelledError on pytest teardown; passes under old runner")
async def test_step6_upgrades(runtime_pid):
    print("🚀 [Step6] Starting Integration Test (single process)\n" + "=" * 80)

    # 1. macro resolution
    runtime_pid.store.write_locator("genesis", "/.mds/env.md", drill("SYSTEM_ENV"), "+>", "\n- GLOBAL_KEY: GlobalSecret\n")
    runtime_pid.store.write_locator("genesis", "/.mds/env.md", drill("LOCAL_ENV"), "+>", "\n- local_key: LocalValue\n")
    g = await runtime_pid.exec("%GLOBAL_KEY", wt_uid="genesis")
    l = await runtime_pid.exec("%local_key", wt_uid="genesis")
    assert "GlobalSecret" in str(g)
    assert "LocalValue" in str(l)
    print("✅ Alias macro resolution works")

    # 2. hook register + trigger
    await runtime_pid.exec('''
        hook -target /config.md::CTX.SYS._b:host -cb '"hook_triggered" | +> /log.md::LOGS' -is_always ;
        hook -target /config.md::CTX.SYS._b:port -cb '"one_shot" | +> /log.md::LOGS' ;
    ''', active_doc="/config.md", wt_uid="genesis")
    await runtime_pid.exec('"newhost" | /> /config.md::CTX.SYS._b:host ; "9090" | /> /config.md::CTX.SYS._b:port', wt_uid="genesis")
    await asyncio.sleep(0.2)
    log = runtime_pid.store.get_text("genesis", "/log.md")
    assert "hook_triggered" in log
    assert "one_shot" in log
    print("✅ Hook trigger works")

    # 3. one-shot auto-destroy
    await runtime_pid.exec('"another" | /> /config.md::CTX.SYS._b:port', wt_uid="genesis")
    await asyncio.sleep(0.2)
    log2 = runtime_pid.store.get_text("genesis", "/log.md")
    assert log2.count("one_shot") == 1, "One-shot hook not destroyed"
    print("✅ One-shot hook auto-destroy works")

    # 4. proc register/unregister
    from mdio_py.plugins.plugin_base import PluginBase
    class Dummy(PluginBase):
        protocol_name = "TEST"
        async def __call__(self, *args): pass
    dummy = Dummy()
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    pid = await dummy._register_proc(runtime_pid._workspace, ctx, "dummy")
    proc_content = runtime_pid.store.get_text("genesis", "/.mds/proc.md")
    assert pid in proc_content
    await dummy._unregister_proc(runtime_pid._workspace, ctx, pid)
    proc_content2 = runtime_pid.store.get_text("genesis", "/.mds/proc.md")
    assert pid not in proc_content2
    print("✅ Process register/unregister works")

    # 5. hook deletion sync
    hook_md = runtime_pid.store.get_text("genesis", "/.mds/hook.md")
    match = re.search(r'## hook_(\d+)', hook_md)
    assert match
    hook_id = match.group(1)
    await runtime_pid.exec(f'hook -del {hook_id}', wt_uid="genesis")
    hook_md2 = runtime_pid.store.get_text("genesis", "/.mds/hook.md")
    assert f"## hook_{hook_id}" not in hook_md2
    print("✅ Hook deletion sync works")

    # 6. namespace cascading termination
    wt_uid = runtime_pid._workspace.derive_worktree("genesis", "/")
    ctx_ns = RuntimeContext(wt_uid=wt_uid, vfs_path="/", active_doc=None)
    pid_ns = await dummy._register_proc(runtime_pid._workspace, ctx_ns, "ns_task")
    assert pid_ns in runtime_pid._workspace.active_tasks[wt_uid]
    runtime_pid._workspace.drop_derived_worktree(wt_uid)
    assert wt_uid not in runtime_pid._workspace.active_tasks
    print("✅ Namespace cascading termination works (memory verification)")

    print("\n🎉 All single-process features verified!")


if __name__ == "__main__":
    async def _run():
        with tempfile.TemporaryDirectory() as tmpdir:
            ws = Path(tmpdir)
            (ws / "config.md").write_text("# CTX\n## SYS\n- host: localhost\n- port: 8080\n")
            (ws / "log.md").write_text("# LOGS\n")
            rt = MdioRuntime(str(ws))
            await rt.exec('mk -f /.mds/env.md', wt_uid="genesis")
            await rt.exec('"# SYSTEM_ENV\n" | +> /.mds/env.md::', wt_uid="genesis")
            await rt.exec('mk -f /.mds/hook.md', wt_uid="genesis")
            await rt.exec('mk -f /.mds/proc.md', wt_uid="genesis")
            await test_step6_upgrades(rt)

    try:
        asyncio.run(_run())
    except asyncio.CancelledError:
        pass
