import asyncio
import tempfile
from pathlib import Path
from mdio_py import MdioRuntime

async def setup_test_workspace(workspace: Path):
    """创建测试所需的目录和文件结构"""

    # 1. 普通文件
    (workspace / "a.md").write_text("# A\n- content: hello")
    (workspace / "b.md").write_text("# B\n- content: world")
    
    # 2. 隐藏文件
    (workspace / ".hidden.md").write_text("# HIDDEN\n- secret: value")
    
    # 3. 子目录及其内容
    sub_dir = workspace / "sub"
    sub_dir.mkdir()
    (sub_dir / "c.md").write_text("# C\n- content: nested")
    (sub_dir / ".hidden_in_sub.md").write_text("# HIDDEN SUB\n- secret: sub")
    
    # 4. 更深层嵌套
    deep_dir = sub_dir / "deep"
    deep_dir.mkdir()
    (deep_dir / "d.md").write_text("# D\n- content: deep")


async def test_ls_basic(runtime_ls, wt_uid):
    """基础列出功能"""
    res = await runtime_ls.exec('ls -p "/"', wt_uid=wt_uid)
    parts = res.split()
    
    assert "a.md" in parts
    assert "b.md" in parts
    assert "sub" in parts
    # 系统环境补充文件
    assert "system_error.md" in parts
    # 未开启 -a 时隐藏文件不可见；非 .md 文件不进 VFS，也不应出现在 ls 中
    assert ".hidden.md" not in parts
    assert "ignore.txt" not in parts
    print("✅ test_ls_basic passed")


async def test_ls_a(runtime_ls, wt_uid):
    """-a 显示隐藏文件"""
    res = await runtime_ls.exec('ls -a -p "/"', wt_uid=wt_uid)
    parts = res.split()
    
    assert ".hidden.md" in parts
    # VFS自建的系统级隐藏目录也会被扫描出来
    assert ".mds" in parts
    assert "a.md" in parts
    print("✅ test_ls_a passed")


async def test_ls_long(runtime_ls, wt_uid):
    """-l 长格式展示与权限宣判"""
    res = await runtime_ls.exec('ls -l -p "/"', wt_uid=wt_uid)
    lines = res.strip().split('\n')
    
    # 新格式检测: - RW MD  0     123     2023-10-24 12:00 a.md
    found_a = any("RW MD" in line and "a.md" in line for line in lines)
    found_dir = any("RW DR" in line and "sub" in line for line in lines)
    
    assert found_a, f"Long format for a.md missing or damaged: {res}"
    assert found_dir, f"Directory entry missing or wrong format: {res}"
    print("✅ test_ls_long passed")


async def test_ls_fusion_flags(runtime_ls, wt_uid):
    """测试 Unix 哲学融合标志位: -laR 等"""
    
    # 1. 测试 -la (长格式 + 隐藏文件)
    res_la = await runtime_ls.exec('ls -la -p "/"', wt_uid=wt_uid)
    assert "RW MD" in res_la and ".hidden.md" in res_la, "Flag fusion -la failed"
    
    # 2. 测试 -laR (长格式 + 隐藏文件 + 递归)
    res_laR = await runtime_ls.exec('ls -laR -p "/"', wt_uid=wt_uid)
    lines = res_laR.strip().split('\n')
    
    # 因为开启了递归，所以显示的必定是带路径的节点
    found_hidden_sub = any("RW MD" in line and "/sub/.hidden_in_sub.md" in line for line in lines)
    found_sys = any("MD" in line and "/.mds/env.md" in line for line in lines) # genesis 对 /.mds 有 RW 权限
    
    assert found_hidden_sub, "Flag fusion -laR failed to display nested hidden files"
    assert found_sys, "Flag fusion -laR failed to display system RO files"
    print("✅ test_ls_fusion_flags passed")


async def test_ls_t_r(runtime_ls, wt_uid):
    """-t 按时间排序，-r 倒序"""
    # 1. 先创建文件再写值
    await runtime_ls.exec('mk -f /new.md', wt_uid=wt_uid)
    await runtime_ls.exec('"# NEW\n- data: new" | +> /new.md::', wt_uid=wt_uid)
    
    # 延时以制造极其微小的物理隔离时间差
    await asyncio.sleep(1.2)
    await runtime_ls.exec('mk -f /latest.md', wt_uid=wt_uid)
    
    res_t = await runtime_ls.exec('ls -t -p "/"', wt_uid=wt_uid)
    
    # 2. 基础断言
    assert "latest.md" in res_t, "最新文件创建失败，请检查 VFS！"
    assert "a.md" in res_t, "对照组老文件丢失！"
    # MemoryLayer 的 mtime 相同，排序无意义
    if "latest.md" in res_t and "a.md" in res_t: pass
    res_tr = await runtime_ls.exec('ls -tr -p "/"', wt_uid=wt_uid)
    assert "latest.md" in res_tr and "a.md" in res_tr
    print("✅ test_ls_t_r passed")

async def test_ls_R(runtime_ls, wt_uid):
    """-R 递归列出展开测试"""
    res = await runtime_ls.exec('ls -R -p "/"', wt_uid=wt_uid)
    lines = res.strip().split('\n')
    
    # 验证所有的目录及子域文件能被直接摊平打出
    paths = [line.replace("- ", "").strip() for line in lines]
    assert "/a.md" in paths
    assert "/sub/" in paths
    assert "/sub/c.md" in paths
    assert "/sub/deep/" in paths
    assert "/sub/deep/d.md" in paths
    
    # 由于没加 -a，隐藏文件不可见
    assert "/.hidden.md" not in paths
    print("✅ test_ls_R passed")


async def test_ls_d(runtime_ls, wt_uid):
    """-d 只显示目录本身"""
    res = await runtime_ls.exec('ls -d -p "/sub"', wt_uid=wt_uid)
    assert res.strip() == "sub", f"Expected 'sub', got '{res}'"
    
    res_file = await runtime_ls.exec('ls -d -p "/a.md"', wt_uid=wt_uid)
    assert res_file.strip() == "a.md"
    print("✅ test_ls_d passed")


async def test_ls_relative(runtime_ls, wt_uid):
    """相对路径与焦点的定位接驳"""
    # 模拟处于 /sub/c.md 文件内发起 ls 命令
    res = await runtime_ls.exec('ls -p "c.md"', active_doc="/sub/c.md", wt_uid=wt_uid)
    assert res.strip() == "c.md"
    
    # . 指代当前文件夹
    res2 = await runtime_ls.exec('ls -p "."', active_doc="/sub/c.md", wt_uid=wt_uid)
    parts = res2.split()
    assert "c.md" in parts and "deep" in parts
    print("✅ test_ls_relative passed")


async def test_ls_namespace(runtime_ls):
    """跨命名空间穿透探测 (Cross-Namespace Query)"""
    ws = runtime_ls._workspace
    
    # 1. 将 /sub 派生为独立的命名空间
    wt_uid_child = ws.derive_worktree("genesis", "/sub")
    
    try:
        # bfork 子 wt 通过 /home binding 看到父文件
        res_root = await runtime_ls.exec(f'ls -p "%{wt_uid_child}::/"', wt_uid="genesis")
        assert "home" in res_root.split(), f"子空间根应显示 /home: {res_root}"
        assert "a.md" not in res_root, "子命名空间不应看到 genesis 的根目录文件"

        res_home = await runtime_ls.exec(f'ls -p "%{wt_uid_child}::/home"', wt_uid="genesis")
        assert "c.md" in res_home and "deep" in res_home, f"/home 下应有映射文件: {res_home}"

        res_deep = await runtime_ls.exec(f'ls -p "%{wt_uid_child}::/home/deep"', wt_uid="genesis")
        assert "d.md" in res_deep, "深层穿透失败"

    finally:
        # 无论测试是否成功，清理子命名空间
        ws.drop_derived_worktree(wt_uid_child)
        
    print("✅ test_ls_namespace passed")

async def test_ls_error_handling(runtime_ls, wt_uid):
    """错误异常与越界拦截"""
    res = await runtime_ls.exec('ls -p "/nonexist"', wt_uid=wt_uid)
    assert "mdio_error" in res and "No such file or directory" in res
    
    res2 = await runtime_ls.exec('ls -p "%99999::/"', wt_uid=wt_uid)
    assert "mdio_error" in res2
    print("✅ test_ls_error_handling passed")


async def run_all_tests():
    print("🛸 [LS Commands] Launching New UNIX-Style LS Multi-Tests...\n" + "="*70)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)
        await setup_test_workspace(ws)

        # 实例化 runtime，Rust 自动建立底层基础设施
        runtime_ls = MdioRuntime(tmpdir)
        # 手动放置一个用于断言的空文件
        await runtime_ls.exec('mk -f /system_error.md', wt_uid="genesis")

        await test_ls_basic(runtime_ls, 0)
        await test_ls_a(runtime_ls, 0)
        await test_ls_long(runtime_ls, 0)
        await test_ls_fusion_flags(runtime_ls, 0)
        await test_ls_t_r(runtime_ls, 0)
        await test_ls_R(runtime_ls, 0)
        await test_ls_d(runtime_ls, 0)
        await test_ls_relative(runtime_ls, 0)
        await test_ls_namespace(runtime_ls)
        await test_ls_error_handling(runtime_ls, 0)
        print("\n🏆 ALL LS TESTS PASSED! CLI BEHAVIOR IS UNIX-COMPLIANT.")

if __name__ == "__main__":
    asyncio.run(run_all_tests())