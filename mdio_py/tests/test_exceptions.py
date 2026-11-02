import asyncio
import tempfile
from pathlib import Path

from mdio_py import MdioRuntime


async def test_exceptions_coverage(tmp_path):
    """pytest entry — uses tmp_path fixture."""
    await _run(runtime=None, ws=tmp_path)


async def run_all_exceptions_test():
    """Legacy entry for subprocess runner."""
    with tempfile.TemporaryDirectory() as tmpdir:
        await _run(runtime=None, ws=Path(tmpdir))


async def _run(runtime, ws):
    print("🚀 Starting MDIO All-Exceptions Coverage & Defense Test...\n" + "="*80)

    # 准备沙箱物理文件（文档根恒为 <ws>/wt；挂载声明只定权限）
    (ws / "wt" / "readonly").mkdir(parents=True)
    (ws / "wt" / "readonly/sys.md").write_text("# SYS\n- info: Read_Only_Data")

    # 启动 Root 并配置一段只读区域
    vfs_config = f"{ws}; {ws}/readonly:/ro_zone:RO"
    runtime = MdioRuntime(vfs_config)
    await runtime.exec('mk -f /system_error.md', wt_uid="genesis")

    results = {}

    print("\n" + "-"*40 + "\n🔥 阶段 1: 编译器语法/词法防线 (Pxxx)\n" + "-"*40)

    # Case 1: P002 无法识别的非法字符
    # 加入正则引擎无法识别的非法符号
    p002_code = "合法的中文路径 :: ^¥§¿"
    print(f"💥 执行 C1 (P002): {p002_code}")
    res_c1 = await runtime.exec(p002_code, wt_uid="genesis", active_doc="/default.md")
    # 断言修改，确保它真的是被 P002 挡住的
    results["C1_P002"] = "[P002]" in res_c1 and "mdio_error" in res_c1
    print(f"🛡️ 拦截确认: {'✅' if results['C1_P002'] else '❌'}")

    # Case 2: P001 意外的 Token (语法结构错误)
    p001_code = '"data" 12356'
    print(f"\n💥 执行 C2 (P001): {p001_code}")
    res_c2 = await runtime.exec(p001_code, wt_uid="genesis", active_doc="/default.md")
    results["C2_P001"] = "[P001]" in res_c2 and "mdio_error" in res_c2
    print(f"🛡️ 拦截确认: {'✅' if results['C2_P001'] else '❌'}")

    print("\n" + "-"*40 + "\n🔥 阶段 2: Rust 底层 VFS / IO 防线 (Rxxx)\n" + "-"*40)

    # Case 3: R201 文件不存在
    r201_code = "$/ghost_folder/ghost_file.md::GHOST"
    print(f"💥 执行 C3-1 (R201 Not Found): {r201_code}")
    res_c3_1 = await runtime.exec(r201_code, wt_uid="genesis", active_doc="/default.md")
    results["C3_1_R201"] = "[R201]" in res_c3_1 and "File not found" in res_c3_1
    print(f"🛡️ 拦截确认: {'✅' if results['C3_1_R201'] else '❌'}")

    # Case 4: R201 只读挂载保护
    r201_ro_code = '"Hack" | +> /ro_zone/sys.md::SYS._b:hack'
    print(f"\n💥 执行 C3-2 (R201 Protect): {r201_ro_code}")
    res_c3_2 = await runtime.exec(r201_ro_code, wt_uid="genesis", active_doc="/default.md")
    results["C3_2_R201"] = "[R201]" in res_c3_2 and "Target is RO" in res_c3_2
    print(f"🛡️ 拦截确认: {'✅' if results['C3_2_R201'] else '❌'}")

    print("\n" + "-"*40 + "\n🔥 阶段 3: 解释器逻辑崩溃与拦截防线 (Ixxx)\n" + "-"*40)

    # Case 5: I404 调用不存在的标准库函数
    i404_code = '"data" | magic_function()'
    print(f"💥 执行 C5 (I404 Func Missing): {i404_code}")
    res_c5 = await runtime.exec(i404_code, wt_uid="genesis", active_doc="/default.md")
    results["C5_I404"] = "I404" in res_c5 and "Unknown" in res_c5
    print(f"🛡️ 拦截确认: {'✅' if results['C5_I404'] else '❌'}")

    # Case 6: I500 毒化拦截 (ExecBlock 异常蔓延阻断)
    # 构造一个合法的 AST，但是在执行时调用不存在的函数，引发运行时断流
    # 这样错误串将向上气泡，并被 _eval_dynamic_path 拦截转化成 I500
    i500_code = '"绝不能落盘的违禁物品" | +> $($("这会引发运行时异常" | un_exist_crash_func()))'
    print(f"\n💥 执行 C6 (I500 ExecBlock Poisoning): {i500_code}")
    res_c6 = await runtime.exec(i500_code, wt_uid="genesis", active_doc="/default.md")
    bad_files = list(ws.glob("*- mdio_error*"))
    # 这时，就会完美触发 I500 的毒化拦截
    results["C6_I500"] = "[I500]" in res_c6 and (len(bad_files) == 0)
    print(f"🛡️ 拦截确认: {'✅' if results['C6_I500'] else '❌'}")

    # Case 7: V003 空间上下文缺失
    v003_code = '/sub/::{ $DOC }'
    print(f"\n💥 执行 C7 (VFS_NO_FOC): {v003_code} (active_doc='')")
    res_c7 = await runtime.exec(v003_code, wt_uid="genesis", active_doc="")
    results["C7_V003"] = "[V003]" in res_c7 or "缺省" in res_c7
    print(f"🛡️ 拦截确认: {'✅' if results['C7_V003'] else '❌'}")

    print("\n" + "-"*40 + "\n🔥 阶段 4: 插件沙箱防线 (Cxxx)\n" + "-"*40)

    # Case 8: C500 插件内部执行异常 (BASH子系统；@BASH.run 旧语法已移除)
    c500_code = '"""python3 -c "print(undefined_var)" """ | @BASH'
    print(f"\n💥 执行 C8 (C500 Plugin Crash): {c500_code}")
    res_c8 = await runtime.exec(c500_code, wt_uid="genesis", active_doc="/default.md")
    results["C8_C500"] = "[C500]" in res_c8 or ("mdio_error" in res_c8 and "NameError" in res_c8)
    print(f"🛡️ 拦截确认: {'✅' if results['C8_C500'] else '❌'}")
    print(f"   -> 详细栈: {res_c8}")

    print("\n" + "-"*40 + "\n🔥 阶段 5: Root 灾备落盘检查\n" + "-"*40)
    try:
        sys_text = runtime.store.get_text("genesis", "/system_error.md")
        results["SYS_LOG"] = "P001" in sys_text or "P002" in sys_text
        print(f"✅ 系统中找到 /system_error.md 灾备日志内容！\n | 片段撷取: {sys_text[:100].strip()}...")
    except Exception as e:
        results["SYS_LOG"] = False
        print(f"❌ 查无日志投影: {e}")

    # 综合断言
    print("\n✅ 异常体系防御网最终测评结果:\n" + "="*40)
    all_passed = True
    for c, passed in results.items():
        print(f"[{c}] -> {'🟢 PASSED (成功拦截/防御)' if passed else '🔴 FAILED (被穿透)'}")
        if not passed: all_passed = False
    print("=" * 40)

    assert results["C1_P002"], "未能拦截 P002 (非法字符)"
    assert results["C2_P001"], "未能拦截 P001 (非法语法结构)"
    assert results["C3_1_R201"], "未能正确抛出 R201 (文件不存在)"
    assert results["C3_2_R201"], "未能拦截向只读挂载点发起的写入 (R201)"
    assert results["C5_I404"], "未正确识别/拦截不存在的 std_functions"
    assert results["C6_I500"], "执行内联崩溃后，产生了含有异常代码的垃圾文件落盘！"
    assert results["C7_V003"], "指代空指针缺失异常未发生"
    assert results["C8_C500"], "未捕获 BASH 子进程的异常并封装成错误"
    assert results["SYS_LOG"], "解释器外层包裹的灾备内存文件写入引擎彻底失效！"

    if all_passed:
        print("\n🏆================================================🏆")
        print("🎉 EXCEPTION & VULNERABILITY COVERAGE: 100% SECURE 🎉")
        print("🏆================================================🏆\n")


if __name__ == "__main__":
    asyncio.run(run_all_exceptions_test())
