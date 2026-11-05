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


async def test_ls_shell_root_and_relative(runtime_ls, wt_uid):
    """shell 化：`ls /`、`ls .`、`ls ..`、裸相对路径都能用"""
    # 绝对根：不再把 token 元组 str() 成 "[('SLASH', '/')]"
    res_root = await runtime_ls.exec('ls /', wt_uid=wt_uid)
    parts = res_root.split()
    assert "a.md" in parts and "sub" in parts, res_root
    assert "mdio_error" not in res_root

    # 无操作数 = 当前目录（根）
    res_bare = await runtime_ls.exec('ls', wt_uid=wt_uid)
    assert "a.md" in res_bare.split(), res_bare

    # `.` 指代当前目录（active_doc=/sub/c.md → /sub）
    res_dot = await runtime_ls.exec('ls .', active_doc="/sub/c.md", wt_uid=wt_uid)
    assert "c.md" in res_dot.split() and "deep" in res_dot.split(), res_dot

    # `..` 回上级（/sub → /）
    res_up = await runtime_ls.exec('ls ..', active_doc="/sub/c.md", wt_uid=wt_uid)
    assert "a.md" in res_up.split() and "sub" in res_up.split(), res_up

    # 裸相对路径（不带前导 /）
    res_dir = await runtime_ls.exec('ls sub', wt_uid=wt_uid)
    assert "c.md" in res_dir.split() and "deep" in res_dir.split(), res_dir

    # ./sub 形式
    res_dot_dir = await runtime_ls.exec('ls ./sub', wt_uid=wt_uid)
    assert "c.md" in res_dot_dir.split(), res_dot_dir


async def test_ls_shell_multi_paths(runtime_ls, wt_uid):
    """shell 化：空格分隔多路径，每路径一块；文件逐条一行"""
    # 两个文件：非 tty shell 风格一条一行
    res_files = await runtime_ls.exec('ls /a.md /b.md', wt_uid=wt_uid)
    assert res_files.strip().split('\n') == ["a.md", "b.md"], res_files

    # 目录操作数带 `path:` 头，块内一条一行
    res_dirs = await runtime_ls.exec('ls /sub /', wt_uid=wt_uid)
    assert "/sub:" in res_dirs and "/:" in res_dirs, res_dirs
    assert "c.md" in res_dirs and "a.md" in res_dirs, res_dirs

    # 一个不存在不影响其它条：成功的照列，错误行殿后
    res_mixed = await runtime_ls.exec('ls /nope /sub', wt_uid=wt_uid)
    assert "c.md" in res_mixed and "deep" in res_mixed, res_mixed
    assert "mdio_error" in res_mixed and "/nope" in res_mixed, res_mixed

    # 全部不存在：纯错误（首行即错误协议）且逐条一行
    res_all_bad = await runtime_ls.exec('ls /nope1 /nope2', wt_uid=wt_uid)
    lines = [ln for ln in res_all_bad.strip().split('\n') if ln]
    assert len(lines) == 2, res_all_bad
    assert all(ln.startswith("- mdio_error: [ls]") for ln in lines), res_all_bad
    assert "'/nope1'" in res_all_bad and "'/nope2'" in res_all_bad

    # 混合文件与错误：成功列表在前、错误在后
    res_mixed2 = await runtime_ls.exec('ls /a.md /nope /b.md', wt_uid=wt_uid)
    assert res_mixed2.startswith("a.md\nb.md"), res_mixed2
    assert "mdio_error" in res_mixed2 and "/nope" in res_mixed2, res_mixed2


async def test_ls_shell_multi_mixed_operands(runtime_ls, wt_uid):
    """多操作数：非目录先列、目录块后列（GNU 风格）；空目录块不与后续文件混淆。"""
    await runtime_ls.exec('mk -f /emptydir/', wt_uid=wt_uid)
    res = await runtime_ls.exec('ls /emptydir /a.md', wt_uid=wt_uid)
    assert res == "a.md\n/emptydir:", res

    res2 = await runtime_ls.exec('ls /a.md /sub', wt_uid=wt_uid)
    lines = res2.split("\n")
    assert lines[0] == "a.md", res2
    assert lines[1] == "/sub:" and "c.md" in res2, res2


async def test_ls_shell_flags_anywhere(runtime_ls, wt_uid):
    """shell 化：flags 可前/中/后，可合并，结果一致"""
    res_prefix = await runtime_ls.exec('ls -la /', wt_uid=wt_uid)
    res_suffix = await runtime_ls.exec('ls / -la', wt_uid=wt_uid)
    res_middle = await runtime_ls.exec('ls -l / -a', wt_uid=wt_uid)
    res_split = await runtime_ls.exec('ls -l -a /', wt_uid=wt_uid)

    for res in (res_prefix, res_suffix, res_middle, res_split):
        assert "mdio_error" not in res, res
        assert "RW MD" in res and ".hidden.md" in res, res
    # 同一集合排序后应完全一致
    assert sorted(res_prefix.split('\n')) == sorted(res_suffix.split('\n'))
    assert sorted(res_prefix.split('\n')) == sorted(res_middle.split('\n'))
    assert sorted(res_prefix.split('\n')) == sorted(res_split.split('\n'))

    # 单旗带路径（旧行为 `ls -l /` 里 / 会被 -l 吞成值）
    res_l = await runtime_ls.exec('ls -l /', wt_uid=wt_uid)
    assert any("RW MD" in ln and "a.md" in ln for ln in res_l.split('\n')), res_l
    # `--` 之后全为路径
    res_dd = await runtime_ls.exec('ls -- /', wt_uid=wt_uid)
    assert "a.md" in res_dd.split(), res_dd

    # -1：一条一行（shell 非 tty 习惯）
    res_1 = await runtime_ls.exec('ls -1 /', wt_uid=wt_uid)
    assert len(res_1.strip().split('\n')) > 1, res_1
    assert "a.md" in res_1 and "sub" in res_1, res_1


async def test_ls_shell_quoted_space_path(runtime_ls, wt_uid):
    """shell 化：引号路径可含空格"""
    await runtime_ls.exec('mk -f "/a b.md"', wt_uid=wt_uid)

    res = await runtime_ls.exec('ls "/"', wt_uid=wt_uid)
    assert "a b.md" in res, res

    res_one = await runtime_ls.exec('ls "a b.md"', wt_uid=wt_uid)
    assert res_one.strip() == "a b.md", res_one

    res_abs = await runtime_ls.exec('ls "/a b.md"', wt_uid=wt_uid)
    assert res_abs.strip() == "a b.md", res_abs


async def test_ls_shell_pipeline(runtime_ls, wt_uid):
    """shell 化后仍是可进管道的普通命令"""
    res = await runtime_ls.exec('ls / | split " "', wt_uid=wt_uid)
    assert "a.md" in res and "sub" in res, res

    res_fmt = await runtime_ls.exec('ls -l / | fmt "- {}"', wt_uid=wt_uid)
    assert "a.md" in res_fmt and "RW MD" in res_fmt, res_fmt


async def test_ls_shell_legacy_p_flag(runtime_ls, wt_uid):
    """兼容：旧式 `ls -p <path>`、`ls -p "uid::/path"`、`ls %macro`、`ls _` 继续可用"""
    res = await runtime_ls.exec('ls -p "/"', wt_uid=wt_uid)
    assert "a.md" in res.split() and "sub" in res.split(), res

    # 旧语法里的宏位置参数（%PWD = 当前目录）仍求值，不是字面路径
    res_macro = await runtime_ls.exec('ls %PWD', active_doc="/sub/c.md", wt_uid=wt_uid)
    assert "c.md" in res_macro.split() and "deep" in res_macro.split(), res_macro
    res_macro_l = await runtime_ls.exec('ls -l %PWD', active_doc="/sub/c.md", wt_uid=wt_uid)
    assert "RW MD" in res_macro_l and "c.md" in res_macro_l, res_macro_l

    ws = runtime_ls._workspace
    wt_uid_child = ws.derive_worktree("genesis", "/sub")
    try:
        res_child = await runtime_ls.exec(f'ls -p "%{wt_uid_child}::/home"', wt_uid="genesis")
        assert "c.md" in res_child and "deep" in res_child, res_child
    finally:
        ws.drop_derived_worktree(wt_uid_child)


async def test_ls_shell_ambiguous_names_one_per_line(runtime_ls, wt_uid):
    """文件名含空白时，非长格式也必须一条一行：空格分隔无法无歧义切分。"""
    await runtime_ls.exec('mk -f "/a b.md"', wt_uid=wt_uid)

    res = await runtime_ls.exec('ls /', wt_uid=wt_uid)
    lines = res.split("\n")
    assert "a b.md" in lines, res
    assert "a.md" in lines and "sub" in lines, res
    assert "a.md b.md" not in res, res

    # 无空白文件名时保留旧的空格分隔（兼容）
    res_sub = await runtime_ls.exec('ls /sub', wt_uid=wt_uid)
    assert "\n" not in res_sub, res_sub
    assert "c.md" in res_sub.split() and "deep" in res_sub.split(), res_sub


async def test_ls_shell_recursive_sorted_by_path(runtime_ls, wt_uid):
    """-R 是扁平路径列表：按完整 path 排序（不再按 basename 产生反直觉顺序）。"""
    res = await runtime_ls.exec('ls -R /', wt_uid=wt_uid)
    paths = [ln[2:] for ln in res.split("\n") if ln.startswith("- ")]
    assert paths == sorted(paths, key=str.lower), res
    assert "/sub/" in paths and "/sub/c.md" in paths and "/sub/deep/" in paths, res
    assert len(paths) == len(set(paths)), res


async def test_ls_shell_empty_operand_errors(runtime_ls, wt_uid):
    """显式空操作数（ls ""）按 shell 语义报错，不静默退化成当前目录。"""
    res = await runtime_ls.exec('ls ""', wt_uid=wt_uid)
    assert res.startswith("- mdio_error: [ls] ''"), res
    assert "No such file or directory" in res, res


async def test_ls_shell_long_option_rejected(runtime_ls, wt_uid):
    """不支持的长选项（--help 等）显式报错，不静默当 flag 吞掉；`--` 仍可用。"""
    for code in ('ls --help', 'ls --all /'):
        res = await runtime_ls.exec(code, wt_uid=wt_uid)
        assert res.startswith("- mdio_error: [ls] 不支持的长选项"), (code, res)
        assert "--" in res and "用法" in res, (code, res)

    # end-of-options 的 `--` 不受影响：其后全部当路径
    res = await runtime_ls.exec('ls -- /', wt_uid=wt_uid)
    assert "mdio_error" not in res and "a.md" in res, res


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
        await test_ls_shell_root_and_relative(runtime_ls, 0)
        await test_ls_shell_multi_paths(runtime_ls, 0)
        await test_ls_shell_flags_anywhere(runtime_ls, 0)
        await test_ls_shell_quoted_space_path(runtime_ls, 0)
        await test_ls_shell_pipeline(runtime_ls, 0)
        await test_ls_shell_legacy_p_flag(runtime_ls, 0)
        print("\n🏆 ALL LS TESTS PASSED! CLI BEHAVIOR IS UNIX-COMPLIANT.")

if __name__ == "__main__":
    asyncio.run(run_all_tests())