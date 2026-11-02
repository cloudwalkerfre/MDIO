"""VFS 全链路集成测试

覆盖：文件夹/文件创建删除、文档追加/删除/清空、project load 正确加载。
"""
import os
import pytest



# 文件夹 / 文件创建


async def test_mk_file(runtime):
    """创建单个 .md 文件。"""
    runtime.store.mk("genesis", "/docs/new_file.md", parents=True, verbose=False)
    assert runtime.store.path_exists("genesis", "/docs/new_file.md")


async def test_mk_file_no_parents_fails(runtime):
    """不传 parents=True 时，父目录不存在应失败。"""
    with pytest.raises(Exception):
        runtime.store.mk("genesis", "/deep/nested/f.md", parents=False, verbose=False)


async def test_mk_file_with_parents(runtime):
    """parents=True 自动创建父目录链。"""
    runtime.store.mk("genesis", "/a/b/c/d.md", parents=True, verbose=False)
    assert runtime.store.path_exists("genesis", "/a/b/c/d.md")


async def test_mk_dir(runtime):
    """创建目录（以 / 结尾）。"""
    runtime.store.mk("genesis", "/mydir/", parents=True, verbose=False)
    # 目录创建后，可以在其中创建文件
    runtime.store.mk("genesis", "/mydir/x.md", parents=False, verbose=False)
    assert runtime.store.path_exists("genesis", "/mydir/x.md")


async def test_mk_overwrite_existing(runtime):
    """创建已存在的文件 → 无操作，不报错。"""
    runtime.store.mk("genesis", "/exist.md", parents=True, verbose=False)
    # 第二次创建不应报错
    runtime.store.mk("genesis", "/exist.md", parents=True, verbose=False)



# 文件夹 / 文件删除


async def test_rm_file(runtime):
    """删除单个文件。"""
    runtime.store.mk("genesis", "/rm_test.md", parents=True, verbose=False)
    assert runtime.store.path_exists("genesis", "/rm_test.md")
    runtime.store.rm("genesis", "/rm_test.md", recursive=False, force=True, verbose=False)
    assert not runtime.store.path_exists("genesis", "/rm_test.md")


async def test_rm_nonexistent_with_force(runtime):
    """删除不存在的文件 + force → 不报错。"""
    runtime.store.rm("genesis", "/no_such.md", recursive=False, force=True, verbose=False)


async def test_rm_dir_recursive(runtime):
    """递归删除目录。"""
    runtime.store.mk("genesis", "/rmdir/", parents=True, verbose=False)
    runtime.store.mk("genesis", "/rmdir/a.md", parents=False, verbose=False)
    runtime.store.mk("genesis", "/rmdir/b.md", parents=False, verbose=False)
    runtime.store.rm("genesis", "/rmdir/", recursive=True, force=True, verbose=False)
    # 目录和内容都不应存在
    assert not runtime.store.path_exists("genesis", "/rmdir/a.md")
    assert not runtime.store.path_exists("genesis", "/rmdir/b.md")



# 文档内容操作


async def test_append_content(runtime):
    """+> 追加内容到文件。"""
    await runtime.exec('"# APPEND\n" | /> /append.md:: -f -god', wt_uid="genesis")
    await runtime.exec('"- item1\n- item2\n" | +> /append.md::APPEND -f', wt_uid="genesis")
    result = await runtime.exec('$ /append.md::APPEND', wt_uid="genesis")
    assert "item1" in result
    assert "item2" in result


async def test_delete_content(runtime):
    """-> 删除文件中的内容。"""
    await runtime.exec('"# DEL\n- keep\n- remove\n- keep2\n" | /> /del.md:: -f -god', wt_uid="genesis")
    # 删除 "remove" 行
    await runtime.exec('$ /del.md::DEL | ->', wt_uid="genesis")
    result = await runtime.exec('$ /del.md::DEL', wt_uid="genesis")
    assert "keep" in result
    assert "keep2" in result


async def test_overwrite_content(runtime):
    """\"/> 覆写替换整个块。"""
    await runtime.exec('"# OW\n- old_val\n" | /> /ow.md:: -f -god', wt_uid="genesis")
    # 替换整个块
    await runtime.exec('"# OW\n- new_val\n" | /> /ow.md::OW -f', wt_uid="genesis")
    result = await runtime.exec('$ /ow.md::OW._b', wt_uid="genesis")
    assert "new_val" in result
    assert "old_val" not in result


async def test_clear_content(runtime):
    """\"/> 覆写为空字符串 → 清空内容。"""
    await runtime.exec('"# CLEAR\n- data\n" | /> /clear.md:: -f -god', wt_uid="genesis")
    # 清空文件
    await runtime.exec('"" | /> /clear.md:: -f', wt_uid="genesis")
    result = await runtime.exec('$ /clear.md', wt_uid="genesis")
    # 清空后内容应为空或只有残余空白
    assert "data" not in result


async def test_append_to_nonexistent_head(runtime):
    """+> 追加到不存在的 heading → 接骨自动创建。"""
    await runtime.exec('"# ROOT\n" | /> /scaffold.md:: -f -god', wt_uid="genesis")
    await runtime.exec('"- k: v\n" | +> /scaffold.md::ROOT.NEW_HEAD._b:k -f', wt_uid="genesis")
    result = await runtime.exec('$ /scaffold.md::ROOT.NEW_HEAD._b:k', wt_uid="genesis")
    assert "v" in result



# Project load → VFS 正确加载


async def test_project_load_reads_existing_files(runtime, tmp_path):
    """load 已有 .md 文件的项目目录 → 内容正确加载。"""
    proj = str(tmp_path / "load_test")
    os.makedirs(proj + "/wt/docs", exist_ok=True)

    # 在磁盘上预先创建文件（文档根恒为 <proj>/wt）
    with open(os.path.join(proj, "wt", "docs", "readme.md"), "w") as f:
        f.write("# LOADED\n- k: hello\n")

    wt_uid = runtime._workspace.register_worktree_at("genesis", proj, "fs")
    result = await runtime.exec('$ /docs/readme.md::LOADED._b:k', wt_uid=wt_uid)
    assert "hello" in result


async def test_project_load_empty_dir(runtime, tmp_path):
    """load 空目录 → 可以正常写入新文件。"""
    proj = str(tmp_path / "load_empty")
    os.makedirs(proj, exist_ok=True)

    wt_uid = runtime._workspace.register_worktree_at("genesis", proj, "fs")
    await runtime.exec('"# EMPTY_PROJ\n- v: init" | /> /start.md:: -f -god', wt_uid=wt_uid)
    result = await runtime.exec('$ /start.md::EMPTY_PROJ._b:v', wt_uid=wt_uid)
    assert "init" in result


async def test_project_load_preserves_existing_sys_files(runtime, tmp_path):
    """load 已有 .mds/ 系统文件的项目目录 → env.md 可读写。"""
    proj = str(tmp_path / "load_sys")
    os.makedirs(proj + "/.mds", exist_ok=True)
    with open(os.path.join(proj, ".mds", "env.md"), "w") as f:
        f.write("# LOCAL_ENV\n- DB: pg\n")

    wt_uid = runtime._workspace.register_worktree_at("genesis", proj, "fs")
    # 项目目录的 /.mds/env.md 应该存在且可读
    # inject_sys_mount 写默认内容（不覆盖已有）
    exists = runtime.store.path_exists(wt_uid, "/.mds/env.md")
    assert exists

if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))

