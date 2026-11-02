"""cron cdef 键迁移测试：输入含 `- mount_id` 的 cron.md，bootstrap 后产出 `- wt_uid` 条目；已有 `- wt_uid` 时 no-op。"""
import pytest


@pytest.mark.asyncio
async def test_cdef_key_migration(tmp_path):
    from mdio_py import MdioRuntime
    from mdio_py.core.selector_builder import SelectorBuilder
    from mdio_py.plugins.cron.cron import CRONPlugin as CronPlugin

    rt = MdioRuntime(str(tmp_path))
    ws = rt._workspace

    # 写入旧格式 cron.md（mount_id 键）
    ws.write_locator(
        "genesis", "/.mds/cron.md",
        SelectorBuilder().build(), "/>",
        "# CRON_JOBS\n\n## job_x\n- enabled: true\n- mount_id: genesis\n- cwd: /\n- mode: at\n- schedule: +1d\n- pipeline: '\"hi\"'",
        auto_scaffold=True, god=True,
    )

    cp = CronPlugin()
    cp._migrate_cdef_key(ws, "genesis")

    text = ws.store.get_text("genesis", "/.mds/cron.md")
    assert "- mount_id:" not in text, f"old key remains: {text}"
    assert "- wt_uid: genesis" in text, f"new key missing: {text}"

    # 幂等：再跑一次不报错、不重复改写
    cp._migrate_cdef_key(ws, "genesis")
    text2 = ws.store.get_text("genesis", "/.mds/cron.md")
    assert text2.count("- wt_uid: genesis") == 1


@pytest.mark.asyncio
async def test_cdef_key_migration_noop_when_new_format(tmp_path):
    from mdio_py import MdioRuntime
    from mdio_py.core.selector_builder import SelectorBuilder
    from mdio_py.plugins.cron.cron import CRONPlugin as CronPlugin

    rt = MdioRuntime(str(tmp_path))
    ws = rt._workspace

    # 新格式 cron.md（wt_uid 键）→ 迁移应 no-op
    ws.write_locator(
        "genesis", "/.mds/cron.md",
        SelectorBuilder().build(), "/>",
        "# CRON_JOBS\n\n## job_y\n- enabled: true\n- wt_uid: genesis\n- cwd: /\n- mode: at\n- schedule: +1d\n- pipeline: '\"yo\"'",
        auto_scaffold=True, god=True,
    )

    cp = CronPlugin()
    cp._migrate_cdef_key(ws, "genesis")

    text = ws.store.get_text("genesis", "/.mds/cron.md")
    assert "- wt_uid: genesis" in text
    assert "- mount_id:" not in text
