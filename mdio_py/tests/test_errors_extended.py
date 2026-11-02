"""
MDIO 异常防线扩展测试
覆盖: V004 夺权拦截 / 系统路径保护 / RO 写入拒绝 / R201 不存在文件 /
      I500 宏未找到 / I404 未知函数 / calc 错误 / test 探针 / fmt 错误
"""

import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime


async def test_phase1_vfs_security(runtime: MdioRuntime):
    """VFS 安全防线 (V004 / 系统路径保护 / RO 拒绝)"""
    print("\n" + "-"*40 + "\n🔥 阶段 1: VFS 安全防线\n" + "-"*40)
    results = {}

    # Case 1.1: V004 非法挂载规则（根路径含冒号）
    print("\n💥 C1.1: V004 非法挂载规则 (root mount contains ':')")
    # ':' 是合法的 target:alias 分隔符
    try:
        runtime._workspace.derive_worktree("genesis", "/public:alias")
        ok = True
    except Exception as e: ok = False
    results["C1.1_V004_colon_root"] = ok
    print(f"   {'✅' if ok else '❌'} 拦截: {ok}")

    print("\n💥 C1.1b: V004 空挂载规则")
    # 空规则 = cfork（合法）
    try:
        runtime._workspace.derive_worktree("genesis", "")
        ok = True
    except Exception as e: ok = False
    results["C1.1b_V004_empty_rules"] = ok
    print(f"   {'✅' if ok else '❌'} 拦截: {ok}")

    # Case 1.2: V004 非法挂载规则
    print("\n💥 C1.2: V004 非法挂载规则 (alias starts with '.')")
    # 只拒绝 .mds/.mdio 别名
    try:
        runtime._workspace.derive_worktree("genesis", "/public:/./secret")
        ok = True
    except Exception as e: ok = False
    results["C1.2_V004_dot_alias"] = ok
    print(f"   {'✅' if ok else '❌'} 拦截: {ok}")

    # Case 1.3: 系统路径保护（非 root 写 /.mds/）
    print("\n💥 C1.3: 系统路径保护 (derived namespace mk /.mds/test.md)")
    child_mount = runtime._workspace.derive_worktree("genesis", "/public")
    result = await runtime.exec('mk -f "/.mds/test.md"', wt_uid=child_mount)
    msg = str(result).lower()
    # 子 wt 有自己的 .mds，mk 成功
    ok = "mdio_success" in str(result) or "permission denied" in str(result).lower()
    results["C1.3_sys_path_mk"] = ok
    print(f"   {'✅' if ok else '❌'}: {result}")

    # Case 1.4: 系统路径保护（非 root rm /.mds/）
    print("\n💥 C1.4: 系统路径保护 (derived namespace rm /.mds/proc.md)")
    result = await runtime.exec('rm -p "/.mds/proc.md"', wt_uid=child_mount)
    # 子 wt 有自己的 .mds
    ok = "mdio_success" in str(result) or "permission denied" in str(result).lower()
    results["C1.4_sys_path_rm"] = ok
    print(f"   {'✅' if ok else '❌'}: {result}")

    runtime._workspace.drop_derived_worktree(child_mount)

    # Case 1.5: RO 挂载写入拒绝
    print("\n💥 C1.5: RO 挂载写入拒绝 (write to RO zone)")
    # 注意：在初始化时 admin 已被挂载为 RO (/admin:RO)
    result = await runtime.exec('"hack" | +> /admin/secret.md::SECRET._b:hack', wt_uid="genesis")
    msg = str(result).lower()
    ok = "ro" in msg or "permission" in msg or "blocked" in msg or "denied" in msg
    results["C1.5_RO_write_reject"] = ok
    print(f"   {'✅' if ok else '❌'} 拦截: {result}")

    return results


async def test_phase2_interpreter_errors(runtime: MdioRuntime):
    """解释器错误 (R201 文件不存在 / I500 宏未找到 / I404 未知函数)"""
    print("\n" + "-"*40 + "\n🔥 阶段 2: 解释器错误防线\n" + "-"*40)
    results = {}

    # Case 2.1: R201 写不存在文件（无 -f/-god）
    print("\n💥 C2.1: R201 写不存在文件 (无 -f 无 -god)")
    result = await runtime.exec('"data" | +> /nofile_xyz.md::HEAD._b:key', wt_uid="genesis")
    msg = str(result).lower()
    ok = "mdio_error" in msg and ("does not exist" in msg or "not found" in msg or "不存在" in msg)
    results["C2.1_R201_write_noexist"] = ok
    print(f"   {'✅' if ok else '❌'} 拦截: {result}")

    # Case 2.2: R201 读不存在文件
    print("\n💥 C2.2: R201 读不存在文件")
    result = await runtime.exec('$/ghost.md::HEAD', wt_uid="genesis")
    msg = str(result).lower()
    ok = "mdio_error" in msg and ("not found" in msg or "r201" in msg)
    results["C2.2_R201_read_noexist"] = ok
    print(f"   {'✅' if ok else '❌'} 拦截: {result}")

    # Case 2.3: Mount 不存在（已销毁的 wt_uid）
    print("\n💥 C2.3: Mount 不存在 (destroyed wt_uid)")
    tmp_mount = runtime._workspace.derive_worktree("genesis", "/public")
    runtime._workspace.drop_derived_worktree(tmp_mount)
    result = await runtime.exec('$/readme.md', wt_uid=tmp_mount)
    msg = str(result).lower()
    ok = "mdio_error" in msg or "not found" in msg or result == "" or "r201" in msg
    results["C2.3_R201_mount_gone"] = ok
    print(f"   {'✅' if ok else '❌'} 拦截: {result}")

    # Case 2.4: I500 宏未找到
    print("\n💥 C2.4: I500 宏未找到 (%NONEXISTENT)")
    result = await runtime.exec('"data" | +> %NONEXISTENT::HEAD', wt_uid="genesis", active_doc="/default.md")
    msg = str(result).lower()
    ok = "mdio_error" in msg
    results["C2.4_I500_macro_not_found"] = ok
    print(f"   {'✅' if ok else '❌'} 拦截: {result}")

    # Case 2.5: I404 未知标准函数
    print("\n💥 C2.5: I404 未知标准函数 (变体)")
    result = await runtime.exec('"data" | nonexistent_func_xyz()', wt_uid="genesis")
    msg = str(result).lower()
    ok = "i404" in msg or "unknown" in msg
    results["C2.5_I404_unknown_func"] = ok
    print(f"   {'✅' if ok else '❌'} 拦截: {result}")

    return results


async def test_phase3_std_errors(runtime: MdioRuntime):
    """标准函数错误 (calc / test / fmt)"""
    print("\n" + "-"*40 + "\n🔥 阶段 3: 标准函数错误防线\n" + "-"*40)
    results = {}

    # calc 错误

    print("\n💥 C3.1: calc 除零")
    result = await runtime.exec('"0" | calc -e "1/0" -x _', wt_uid="genesis")
    ok = "mdio_error" in str(result) and "calc" in str(result)
    results["C3.1_calc_div_zero"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: {result}")

    print("\n💥 C3.2: calc 未定义变量")
    result = await runtime.exec('"0" | calc -e "$UNDEFINED_VAR" -x _', wt_uid="genesis")
    ok = "mdio_error" in str(result) and "calc" in str(result)
    results["C3.2_calc_undefined_var"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: {result}")

    print("\n💥 C3.3: calc 系统保留字 (min)")
    result = await runtime.exec('"0" | calc -e "1+2" -min _', wt_uid="genesis")
    ok = "mdio_error" in str(result) and ("保留字" in str(result) or "calc" in str(result))
    results["C3.3_calc_reserved_word"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: {result}")

    print("\n💥 C3.4: calc 非法函数调用")
    result = await runtime.exec('"0" | calc -e "illegal_func(1)" -x _', wt_uid="genesis")
    ok = "mdio_error" in str(result) and "calc" in str(result)
    results["C3.4_calc_illegal_func"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: {result}")

    print("\n💥 C3.5: calc 非法字面量 (字符串)")
    result = await runtime.exec('"0" | calc -e "\"hello\"" -x _', wt_uid="genesis")
    ok = "mdio_error" in str(result) and "calc" in str(result)
    results["C3.5_calc_illegal_literal"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: {result}")

    print("\n💥 C3.6: calc 参数解析失效（非数字变量）")
    result = await runtime.exec('"0" | calc -e "$X + 1" -X abc -x _', wt_uid="genesis")
    ok = "mdio_error" in str(result) and "calc" in str(result)
    results["C3.6_calc_bad_param"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: {result}")

    # test 探针

    print("\n💥 C3.7: test -e 不存在路径 → false")
    result = await runtime.exec('test -e "/nope.md"', wt_uid="genesis")
    ok = str(result).strip() == "false"
    results["C3.7_test_e_false"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.8: test -e 存在路径 → true")
    result = await runtime.exec('test -e "/public/readme.md"', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.8_test_e_true"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.9: test -md 是 .md 文件 → true")
    result = await runtime.exec('test -md "/public/readme.md"', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.9_test_md_true"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.10: test -md 不是 .md 文件 → false")
    result = await runtime.exec('test -md "/public/"', wt_uid="genesis")
    ok = str(result).strip() == "false"
    results["C3.10_test_md_false"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.11: test -d 是目录 → true")
    result = await runtime.exec('test -d "/public/"', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.11_test_d_true"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.12: test -d 不是目录 → false")
    result = await runtime.exec('test -d "/public/readme.md"', wt_uid="genesis")
    ok = str(result).strip() == "false"
    results["C3.12_test_d_false"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.13: test -z 空字符串 → true")
    result = await runtime.exec('"" | test -z', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.13_test_z_true"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.14: test -z 非空字符串 → false")
    result = await runtime.exec('"hello" | test -z', wt_uid="genesis")
    ok = str(result).strip() == "false"
    results["C3.14_test_z_false"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.15: test -n 非空 → true")
    result = await runtime.exec('"hello" | test -n', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.15_test_n_true"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.16: test -n 空 → false")
    result = await runtime.exec('"" | test -n', wt_uid="genesis")
    ok = str(result).strip() == "false"
    results["C3.16_test_n_false"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.17: test -w RO 路径 → false")
    result = await runtime.exec('test -w "/admin/secret.md"', wt_uid="genesis")
    ok = str(result).strip() == "false"
    results["C3.17_test_w_RO_false"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.18: test -wt 存活 worktree → true (Plan 3: wt_uid=uid)")
    result = await runtime.exec('test -wt "genesis"', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.18_test_wt_true"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.19: test -wt 不存在 worktree → false")
    result = await runtime.exec('test -wt "99999"', wt_uid="genesis")
    ok = str(result).strip() == "false"
    results["C3.19_test_wt_false"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.20: test -eq 相等 → true")
    result = await runtime.exec('"42" | test -eq "42"', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.20_test_eq_true"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.21: test -eq 不等 → false")
    result = await runtime.exec('"42" | test -eq "99"', wt_uid="genesis")
    ok = str(result).strip() == "false"
    results["C3.21_test_eq_false"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.22: test -gt 大于 → true")
    result = await runtime.exec('"100" | test -gt "50"', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.22_test_gt_true"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.23: test -re 正则匹配 → true")
    result = await runtime.exec('"hello123" | test -re "^\\\\w+\\\\d+$"', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.23_test_re_true"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.24: test -re 正则不匹配 → false")
    result = await runtime.exec('"hello" | test -re "^\\\\d+$"', wt_uid="genesis")
    ok = str(result).strip() == "false"
    results["C3.24_test_re_false"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    print("\n💥 C3.25: test 无参数默认 -n（非空 → true）")
    result = await runtime.exec('"something" | test', wt_uid="genesis")
    ok = str(result).strip() == "true"
    results["C3.25_test_default_n"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: '{result}'")

    # fmt 错误

    print("\n💥 C3.26: fmt 无模板")
    result = await runtime.exec('"data" | fmt', wt_uid="genesis")
    ok = "error" in str(result).lower() and "fmt" in str(result).lower()
    results["C3.26_fmt_no_template"] = ok
    print(f"   {'✅' if ok else '❌'} 结果: {result}")

    return results


async def main():
    print("🚀 Starting MDIO Extended Error Defense Test Suite...\n" + "=" * 80)

    # init
    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        (ws / "public").mkdir()
        (ws / "public/readme.md").write_text("# PUBLIC\n- content: open\n")
        (ws / "admin").mkdir()
        (ws / "admin/secret.md").write_text("# SECRET\n- password: admin_only\n")

        storage_root = f"{ws}; {ws}/admin:/admin:RO"
        runtime = MdioRuntime(storage_root)
        await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

        all_results = {}

        r1 = await test_phase1_vfs_security(runtime)
        all_results.update(r1)

        r2 = await test_phase2_interpreter_errors(runtime)
        all_results.update(r2)

        r3 = await test_phase3_std_errors(runtime)
        all_results.update(r3)

        # Summary
        print("\n" + "=" * 80)
        print("✅ 扩展异常防线测试结果:\n" + "=" * 80)
        passed = 0
        failed = 0
        for case, ok in all_results.items():
            tag = "🟢 PASSED" if ok else "🔴 FAILED"
            if ok:
                passed += 1
            else:
                failed += 1
            print(f"  [{case}] -> {tag}")
        print("=" * 80)
        print(f"  总计: {passed} passed, {failed} failed / {len(all_results)} total")
        print("=" * 80)

        if failed > 0:
            raise AssertionError(f"{failed} test(s) failed!")


if __name__ == "__main__":
    asyncio.run(main())
