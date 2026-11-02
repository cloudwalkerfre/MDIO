"""AP 写透测试:MDIO_PERSIST=1 时 mk/rm/reset 同步镜像到磁盘。

conftest 的 autouse `_persist_marker_env` 按 pytest.mark.persist 分面设置 env:
- 带 marker 的用例:MDIO_PERSIST=1(写透生效)
- 不带(缺省系):pop(与隔离语义/测试不污染一致)
"""

import pytest


@pytest.mark.persist
async def test_mk_mirrors_to_disk(runtime, workdir):
    runtime._workspace.mk("genesis", "/ap_doc.md", True, True)
    assert (workdir / "wt" / "ap_doc.md").exists(), "MDIO_PERSIST=1 时 mk 应同步写盘"


@pytest.mark.persist
async def test_mkdir_mirrors_to_disk(runtime, workdir):
    runtime._workspace.mk("genesis", "/ap_dir", True, True)
    assert (workdir / "wt" / "ap_dir").is_dir(), "MDIO_PERSIST=1 时 mkdir 应同步落盘"


@pytest.mark.persist
async def test_rm_mirrors_to_disk(runtime, workdir):
    runtime._workspace.mk("genesis", "/ap_rm.md", True, True)
    assert (workdir / "wt" / "ap_rm.md").exists()
    runtime._workspace.rm("genesis", "/ap_rm.md", False, True, True)
    assert not (workdir / "wt" / "ap_rm.md").exists(), "rm 后磁盘应同步删除"


@pytest.mark.persist
async def test_rm_open_doc_no_resurrect(runtime, workdir):
    runtime._workspace.mk("genesis", "/ap_open.md", True, True)
    await runtime.exec('"# EDIT\n- v: 1" | /> /ap_open.md -god')
    runtime._workspace.rm("genesis", "/ap_open.md", False, True, True)
    assert not (workdir / "wt" / "ap_open.md").exists()


@pytest.mark.persist
async def test_reset_overwrites_disk(runtime, workdir):
    runtime._workspace.mk("genesis", "/ap_rst.md", True, True)
    runtime.store.reset_doc("genesis", "/ap_rst.md", "# AP\n- v: 1\n")
    assert (workdir / "wt" / "ap_rst.md").read_text() == "# AP\n- v: 1\n", "reset 后磁盘应为新内容"


@pytest.mark.persist
async def test_cross_store_reload_visible(runtime, workdir):
    """写透后跨 store（新建 MdioRuntime，同一目录）重载，磁盘内容应可见。"""
    runtime._workspace.mk("genesis", "/ap_reload.md", True, True)
    await runtime.exec('"# AP\n- v: reload" | /> /ap_reload.md -god')
    # 等待 flush 线程落盘(100ms 拍)后,以新 runtime(磁盘 load)验证可见
    import time
    time.sleep(0.4)
    from mdio_py import MdioRuntime
    rt2 = MdioRuntime(str(workdir))
    text = rt2.store.get_text("genesis", "/ap_reload.md")
    assert "reload" in text, f"跨 store 重载应看到写透内容: {text!r}"


async def test_default_off_no_disk(runtime, workdir):
    """缺省(无 MDIO_PERSIST):create 不应写盘(autouse fixture 保证 env 空)。"""
    runtime._workspace.mk("genesis", "/ap_off.md", True, True)
    assert not (workdir / "wt" / "ap_off.md").exists(), "缺省(无 MDIO_PERSIST)时不应写盘"
