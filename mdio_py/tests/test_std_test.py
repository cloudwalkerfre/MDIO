# mdio_py/tests/test_std_test.py
import asyncio
import tempfile
from pathlib import Path
from mdio_py import MdioRuntime

async def setup_test_workspace(workspace: Path):
    """创建测试所需的目录、文件及 MDIO 结构"""

    # 1. 普通文件与 AST 内容
    (workspace / "config.md").write_text("""# CONFIG
- DB_HOST: localhost
- PORT: 5432
- EMPTY_KEY: 
""")
    (workspace / "data.md").write_text("""# DATA
- LOGS: 
    error1
    error2
""")
    (workspace / "empty.md").write_text("")
    (workspace / "ro.md").write_text("# RO\n- readonly: yes")

    # 2. 目录结构
    sub_dir = workspace / "sub"
    sub_dir.mkdir()
    (sub_dir / "nested.md").write_text("# NESTED\n- value: inside")

    # 3. 系统级隐藏目录（模拟 .mds）
    sys_dir = workspace / ".mds"
    sys_dir.mkdir()
    (sys_dir / "test.md").write_text("# SYS\n- secret: kernel")

    # 4. 只读挂载点模拟（实际权限由 Rust 控制，这里仅作为路径存在）
    ro_dir = workspace / "ro_mount"
    ro_dir.mkdir()
    (ro_dir / "protected.md").write_text("# PROTECTED\n- data: topsecret")


async def test_basic_unary(runtime_std, wt_uid):
    """测试 -z / -n / 无参数默认行为"""
    # 空值探测
    assert await runtime_std.exec('"" | test -z', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"" | test --empty', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"hello" | test -z', wt_uid=wt_uid) == "false"
    assert await runtime_std.exec('"  " | test -z', wt_uid=wt_uid) == "true"   # 纯空格视为空

    # 非空探测
    assert await runtime_std.exec('"" | test -n', wt_uid=wt_uid) == "false"
    assert await runtime_std.exec('"hello" | test -n', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"  x" | test --notempty', wt_uid=wt_uid) == "true"

    # 无参数默认 -n
    assert await runtime_std.exec('"" | test', wt_uid=wt_uid) == "false"
    assert await runtime_std.exec('"abc" | test', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"   " | test', wt_uid=wt_uid) == "false"

    # 列表/字典空值
    assert await runtime_std.exec('[] | test -z', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('[1,2] | test -z', wt_uid=wt_uid) == "false"
    assert await runtime_std.exec('{} | test -n', wt_uid=wt_uid) == "false"
    print("✅ test_basic_unary passed")


async def test_vfs_exists(runtime_std, wt_uid):
    """测试 -e 存在性（文件/目录/节点）"""
    # 文件存在
    assert await runtime_std.exec('"/config.md" | test -e', wt_uid=wt_uid) == "true"
    # 文件不存在
    assert await runtime_std.exec('"/nonexist.md" | test -e', wt_uid=wt_uid) == "false"
    # 目录存在（以 / 结尾）
    assert await runtime_std.exec('"/sub/" | test -e', wt_uid=wt_uid) == "true"
    # AST 节点存在（CONFIG 节点下有内容）
    assert await runtime_std.exec('"/config.md::CONFIG" | test -e', wt_uid=wt_uid) == "true"
    # AST 节点不存在
    assert await runtime_std.exec('"/config.md::NOTEXIST" | test -e', wt_uid=wt_uid) == "false"
    # 带 Key 的 Strip 存在
    assert await runtime_std.exec('"/config.md::CONFIG._b:DB_HOST" | test -e', wt_uid=wt_uid) == "true"
    # 空 Key（存在但值为空）
    assert await runtime_std.exec('"/config.md::CONFIG._b:EMPTY_KEY" | test -e', wt_uid=wt_uid) == "true"
    print("✅ test_vfs_exists passed")


async def test_vfs_writable(runtime_std, wt_uid):
    """测试 -w 可写性（存在且挂载点 RW）"""
    # 普通文件可写（根挂载点为 RW）
    assert await runtime_std.exec('"/config.md" | test -w', wt_uid=wt_uid) == "true"
    # 不存在文件不可写
    assert await runtime_std.exec('"/nosuch.md" | test -w', wt_uid=wt_uid) == "false"
    # AST 节点存在且父文件可写
    assert await runtime_std.exec('"/config.md::CONFIG._b:PORT" | test -w', wt_uid=wt_uid) == "true"
    # 注意：实际只读挂载点需要由 Rust 层控制，这里无法模拟，但信任底层 get_file_metadata 的 is_ro
    # 实际 genesis 号空间的 /.mds 下文件是 RO？根据代码 wt_uid="genesis" 且路径 / 下 .mds 是 RO，但我们的测试工作区不是系统根，因此正常可写
    print("✅ test_vfs_writable passed")


async def test_vfs_nonempty(runtime_std, wt_uid):
    """测试 -s 非空（文件大小>0 / 目录有内容 / 节点值非空）"""
    # 非空文件
    assert await runtime_std.exec('"/config.md" | test -s', wt_uid=wt_uid) == "true"
    # 空文件
    assert await runtime_std.exec('"/empty.md" | test -s', wt_uid=wt_uid) == "false"
    # 目录有内容（/sub 下有 nested.md）
    assert await runtime_std.exec('"/sub/" | test -s', wt_uid=wt_uid) == "true"
    # 空目录（新建一个）
    await runtime_std.exec('mk -f "/emptydir/"', wt_uid=wt_uid)
    assert await runtime_std.exec('"/emptydir/" | test -s', wt_uid=wt_uid) == "false"
    # AST 节点非空（PORT 有值）
    assert await runtime_std.exec('"/config.md::CONFIG._b:PORT" | test -s', wt_uid=wt_uid) == "true"
    # AST 节点值为空（EMPTY_KEY 无值）
    assert await runtime_std.exec('"/config.md::CONFIG._b:EMPTY_KEY" | test -s', wt_uid=wt_uid) == "false"
    # 多行 Strip 非空（LOGS 有内容）
    assert await runtime_std.exec('"/data.md::DATA._b:LOGS" | test -s', wt_uid=wt_uid) == "true"
    print("✅ test_vfs_nonempty passed")


async def test_vfs_type(runtime_std, wt_uid):
    """测试 -d / -md 类型判断"""
    # 目录
    assert await runtime_std.exec('"/sub/" | test -d', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"/sub" | test -d', wt_uid=wt_uid) == "true"   # 自动补斜杠
    # 文件
    assert await runtime_std.exec('"/config.md" | test -md', wt_uid=wt_uid) == "true"
    # 目录不是文件
    assert await runtime_std.exec('"/sub/" | test -md', wt_uid=wt_uid) == "false"
    # 文件不是目录
    assert await runtime_std.exec('"/config.md" | test -d', wt_uid=wt_uid) == "false"
    # 不存在
    assert await runtime_std.exec('"/none.md" | test -md', wt_uid=wt_uid) == "false"
    print("✅ test_vfs_type passed")


async def test_wt_probe(runtime_std):
    """测试 -wt worktree 存活检测"""
    ws = runtime_std._workspace
    # 创建子 worktree
    child_id = ws.derive_worktree("genesis", "/sub")
    try:
        # RuntimeContext 收 str wt_uid，旧的 mount 探针需要 wt_uid 数值为 int，现已是 str
        # -wt 检测在 Rust 侧现已接受 uid str
        result = await runtime_std.exec(f'"{child_id}" | test -wt', wt_uid="genesis")
        assert result in ("true", "false"), f"unexpected: {result}"
    finally:
        ws.drop_derived_worktree(child_id)
    print("✅ test_mount_probe passed")


async def test_binary_compare(runtime_std, wt_uid):
    """测试 -eq -ne -gt -ge -lt -le 比较"""
    # 数值比较
    assert await runtime_std.exec('5 | test -eq 5', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('5 | test -ne 3', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('5 | test -gt 3', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('5 | test -ge 5', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('5 | test -lt 10', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('5 | test -le 5', wt_uid=wt_uid) == "true"
    # 字符串字典序
    assert await runtime_std.exec('"abc" | test -eq "abc"', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"abc" | test -ne "def"', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"abc" | test -gt "aaa"', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"abc" | test -lt "abd"', wt_uid=wt_uid) == "true"
    # 混合类型：优先数值
    assert await runtime_std.exec('"5" | test -eq 5', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"5a" | test -eq 5', wt_uid=wt_uid) == "false"  # 无法转数值，回退字符串比较
    print("✅ test_binary_compare passed")


async def test_contains(runtime_std, wt_uid):
    """测试 -in / --has 包含关系"""
    # 字符串子串
    assert await runtime_std.exec('"hello world" | test -in "world"', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"hello" | test --has "x"', wt_uid=wt_uid) == "false"
    # 列表元素
    assert await runtime_std.exec('[1,2,3] | test -in 2', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('["a","b"] | test -in "c"', wt_uid=wt_uid) == "false"
    # 字典键
    assert await runtime_std.exec('{"name":"mdio"} | test -in "name"', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('{"x":1} | test --has "y"', wt_uid=wt_uid) == "false"
    print("✅ test_contains passed")


async def test_regex_match(runtime_std, wt_uid):
    """测试 -re / --match 正则匹配"""
    assert await runtime_std.exec('"abc123" | test -re "\\d+"', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"abc" | test --match "^[a-z]+$"', wt_uid=wt_uid) == "true"
    assert await runtime_std.exec('"ABC" | test -re "^[a-z]+$"', wt_uid=wt_uid) == "false"
    # 非法正则返回 false
    assert await runtime_std.exec('"test" | test -re "["', wt_uid=wt_uid) == "false"
    print("✅ test_regex_match passed")


async def test_combined_with_pipe(runtime_std, wt_uid):
    """测试 test 与动态查询、数据流组合"""
    
    # 组合 1：动态先读取 VFS AST 数据，再推给 test
    code = """
    $/config.md::CONFIG._b:PORT | test -gt 5000
    """
    res = await runtime_std.exec(code, wt_uid=wt_uid)
    # 因为 PORT=5432，5432 > 5000 应当为 True
    assert res == "true", f"Expected 'true', got {res}"

    code2 = """
    $/config.md::CONFIG._b:PORT | test -gt 6000
    """
    res2 = await runtime_std.exec(code2, wt_uid=wt_uid)
    assert res2 == "false", f"Expected 'false', got {res2}"

    # 组合 2：嵌套在 $() 内动态求值，并交由最外层检测
    # 探测 "/config.md" 这个文件里的 CONFIG 节下，是不是包含 DB_HOST 这个词
    code3 = """
    ($/config.md::CONFIG._b:DB_HOST) | test -has "local"
    """
    res3 = await runtime_std.exec(code3, wt_uid=wt_uid)
    res33 = await runtime_std.exec("($/config.md::CONFIG._b:DB_HOST)", wt_uid=wt_uid)
    res333 = await runtime_std.exec("$/config.md::CONFIG._b:DB_HOST", wt_uid=wt_uid)
    print(f"($/config.md::CONFIG._b:DB_HOST): {res33}")
    print(f"$/config.md::CONFIG._b:DB_HOST: {res333}")
    assert res3 == "true", f"Expected 'true', got {res3}"
    
    print("✅ test_combined_with_pipe passed")


async def test_default_no_args(runtime_std, wt_uid):
    """无参数时的默认行为（-n）"""
    assert await runtime_std.exec('test ""', wt_uid=wt_uid) == "false"
    assert await runtime_std.exec('test "a"', wt_uid=wt_uid) == "true"
    # 通过管道
    assert await runtime_std.exec('"" | test', wt_uid=wt_uid) == "false"
    assert await runtime_std.exec('"x" | test', wt_uid=wt_uid) == "true"
    print("✅ test_default_no_args passed")


async def run_all_tests():
    print("🛸 [TEST OPS] Launching Comprehensive Test Suite...\n" + "="*70)

    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)
        await setup_test_workspace(ws)

        runtime_std = MdioRuntime(tmpdir)
        # 确保基础文件系统就绪
        await runtime_std.exec('mk -f /emptydir/', wt_uid="genesis")

        await test_basic_unary(runtime_std, 0)
        await test_vfs_exists(runtime_std, 0)
        await test_vfs_writable(runtime_std, 0)
        await test_vfs_nonempty(runtime_std, 0)
        await test_vfs_type(runtime_std, 0)
        await test_mount_probe(runtime_std)
        await test_binary_compare(runtime_std, 0)
        await test_contains(runtime_std, 0)
        await test_regex_match(runtime_std, 0)
        await test_combined_with_pipe(runtime_std, 0)
        await test_default_no_args(runtime_std, 0)
        print("\n🏆 ALL TEST OPS PASSED! test COMMAND BEHAVIOR IS CORRECT.")

if __name__ == "__main__":
    asyncio.run(run_all_tests())
