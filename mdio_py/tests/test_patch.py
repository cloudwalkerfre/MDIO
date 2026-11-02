import asyncio
import tempfile
import textwrap
from pathlib import Path

from mdio_py import MdioRuntime
from mdio_py.core.selector_builder import SelectorBuilder




async def run_test_step(runtime, step_name, cmd, expected_content, filename, ws, init_content=None):
    print(f"\n=== {step_name} ===")
    file_path = ws / filename
    if init_content is not None:
        # WT VFS: VFS write (not disk)
        ir = SelectorBuilder().build()
        runtime._workspace.write_locator("genesis", f"/{filename}", ir, "/>", init_content, auto_scaffold=True, god=True)

    print("🚀 指令:", cmd.strip())

    virtual_path = f"/{filename}"
    await runtime.exec(cmd.strip(), active_doc=virtual_path, wt_uid="genesis")

    actual = runtime.store.get_text("genesis", virtual_path)

    print("📦 内存中结果:\n" + "-"*40)
    print(actual, end="")
    print("-"*40)

    # assert actual == expected_content, f"内容不符:\n期望:\n{expected_content}\n实际:\n{actual}"
    assert actual.rstrip() == expected_content.rstrip(), f"内容不符:\n期望:\n{expected_content}\n实际:\n{actual}"
    print(f"✅ 【{step_name}】通过")


async def test_patch():
    with tempfile.TemporaryDirectory() as tmpdir:
        ws = Path(tmpdir)

        runtime = MdioRuntime(str(ws))
        print("🚀 Starting MDIO `~>` Patch Extractor & Engine Test (VFS NEO)...\n" + "="*80)

        base_content = textwrap.dedent("""\
        # DOCS
        ## TODO
        - item1: 修复所有的 Bug
        - item2: 进行性能测似
        - item3: 准备发布版本
        """)
        await run_test_step(runtime, "测试 1：单块补丁",
            textwrap.dedent('''"\"\"\"<<<<\n- item2: 进行性能测似\n====\n- item2: 进行性能测试 (✅己修复)\n>>>>\"\"\" | ~>/target_1.md::DOCS.TODO'''),
            textwrap.dedent("""# DOCS\n## TODO\n- item1: 修复所有的 Bug\n- item2: 进行性能测试 (✅己修复)\n- item3: 准备发布版本\n"""),
            "target_1.md", ws, base_content)

        await run_test_step(runtime, "测试 2：多块补丁",
            textwrap.dedent('''"\"\"\"<<<<\n- item2: 进行性能测似\n====\n- item2: 进行性能测试 (✅己修复)\n>>>>\n<<<<\n- item3: 准备发布版本\n====\n- item3: 准备发布 v1.0.0\n>>>>\"\"\" | ~>/target_2.md::DOCS.TODO'''),
            textwrap.dedent("""# DOCS\n## TODO\n- item1: 修复所有的 Bug\n- item2: 进行性能测试 (✅己修复)\n- item3: 准备发布 v1.0.0\n"""),
            "target_2.md", ws, base_content)

        multi_line_content = textwrap.dedent("""\
        # DOCS
        ## TODO
        - item1: 修复所有的 Bug
        - item2: 这是一个
            跨多行的描述
            需要替换
        - item3: 正常任务
        """)
        await run_test_step(runtime, "测试 3：多行补丁",
            textwrap.dedent('''"\"\"\"<<<<\n- item2: 这是一个\n    跨多行的描述\n    需要替换\n====\n- item2: 这是已经\n    替换后的多行\n    内容\n>>>>\"\"\" | ~>/target_3.md::DOCS.TODO'''),
            textwrap.dedent("""# DOCS\n## TODO\n- item1: 修复所有的 Bug\n- item2: 这是已经\n    替换后的多行\n    内容\n- item3: 正常任务\n"""),
            "target_3.md", ws, multi_line_content)

        multi_match_content = textwrap.dedent("""\
        # TASKS
        - fix: 修复 bug
        - fix: 修复另一个 bug
        - update: 更新文档
        """)
        await run_test_step(runtime, "测试 4：多个相同旧文本，只替换第一个",
            textwrap.dedent('''"\"\"\"<<<<\n- fix: 修复 bug\n====\n- fix: 已修复 (DONE)\n>>>>\"\"\" | ~>/target_4.md::TASKS'''),
            textwrap.dedent("""# TASKS\n- fix: 已修复 (DONE)\n- fix: 修复另一个 bug\n- update: 更新文档\n"""),
            "target_4.md", ws, multi_match_content)

        boundary_content = "# ROOT\n中间行\n- LAST_LINE"
        await run_test_step(runtime, "测试 5：文件结尾 EOF 匹配",
            textwrap.dedent('''"""<<<<\n- LAST_LINE\n====\n- END_LINE 🚀\n>>>>""" | ~>/boundary.md::ROOT'''),
            "# ROOT\n中间行\n- END_LINE 🚀",
            "boundary.md", ws, boundary_content)

        special_regex_content = "# CONFIG\n- regex_chars: a*b.c[d]e\f\n"
        cmd_6 = '"""<<<<\n- regex_chars: a*b.c[d]e\f\n====\n- regex_chars: star-dot-bracket-backslash\n>>>>""" | ~>/target_6.md::CONFIG'
        await run_test_step(runtime, "测试 6：包含控制字符 \\f 的文本替换", cmd_6,
            "# CONFIG\n- regex_chars: star-dot-bracket-backslash\n", "target_6.md", ws, special_regex_content)

        json_single_content = textwrap.dedent("""# SETTINGS\n- config: {"name": "test", "version": 1, "enabled": true}\n""")
        await run_test_step(runtime, "测试 7：JSON 字段修改（单行）",
            textwrap.dedent('''"\"\"\"<<<<\n- config: {"name": "test", "version": 1, "enabled": true}\n====\n- config: {"name": "test", "version": 2, "enabled": true}\n>>>>\"\"\" | ~>/target_7.md::SETTINGS'''),
            textwrap.dedent("""# SETTINGS\n- config: {"name": "test", "version": 2, "enabled": true}\n"""),
            "target_7.md", ws, json_single_content)

        json_multi_content = textwrap.dedent("""\
        # SETTINGS
        - config: |
            {
              "name": "test",
              "version": 1,
              "enabled": true
            }
        """)
        await run_test_step(runtime, "测试 8：JSON 字段修改（多行）",
            textwrap.dedent('''"\"\"\"<<<<\n- config: |\n    {\n      "name": "test",\n      "version": 1,\n      "enabled": true\n    }\n====\n- config: |\n    {\n      "name": "test",\n      "version": 2,\n      "enabled": true\n    }\n>>>>\"\"\" | ~>/target_8.md::SETTINGS'''),
            textwrap.dedent("""# SETTINGS\n- config: |\n    {\n      "name": "test",\n      "version": 2,\n      "enabled": true\n    }\n"""),
            "target_8.md", ws, json_multi_content)

        json_multi_block_content = textwrap.dedent("""\
        # SETTINGS
        - config: {"name": "test", "version": 1, "enabled": false}
        """)
        await run_test_step(runtime, "测试 9：多块修改 JSON 字段",
            textwrap.dedent('''"\"\"\"<<<<\n- config: {"name": "test", "version": 1, "enabled": false}\n====\n- config: {"name": "test", "version": 2, "enabled": false}\n>>>>\n<<<<\n"enabled": false\n====\n"enabled": true\n>>>>\"\"\" | ~>/target_9.md::SETTINGS'''),
            textwrap.dedent("""# SETTINGS\n- config: {"name": "test", "version": 2, "enabled": true}\n"""),
            "target_9.md", ws, json_multi_block_content)

        json_replace_content = textwrap.dedent("""\
        # DATA
        - info: |
            {
              "user": "alice",
              "score": 100
            }
        """)
        await run_test_step(runtime, "测试 10：整体替换 JSON 块",
            textwrap.dedent('''"\"\"\"<<<<\n- info: |\n    {\n      "user": "alice",\n      "score": 100\n    }\n====\n- info: |\n    {\n      "user": "bob",\n      "score": 200,\n      "role": "admin"\n    }\n>>>>\"\"\" | ~>/target_10.md::DATA'''),
            textwrap.dedent("""# DATA\n- info: |\n    {\n      "user": "bob",\n      "score": 200,\n      "role": "admin"\n    }\n"""),
            "target_10.md", ws, json_replace_content)

        regex_json_content = textwrap.dedent("""\
        # DATA
        - json: {"pattern": "a*b.c"}
        """)
        await run_test_step(runtime, "测试 11：JSON 中包含正则特殊字符",
            textwrap.dedent('''"\"\"\"<<<<\n- json: {"pattern": "a*b.c"}\n====\n- json: {"pattern": "new_pattern"}\n>>>>\"\"\" | ~>/target_11.md::DATA'''),
            textwrap.dedent("""# DATA\n- json: {"pattern": "new_pattern"}\n"""),
            "target_11.md", ws, regex_json_content)

        no_match_content = textwrap.dedent("""\
        # DATA
        - value: original
        """)
        await run_test_step(runtime, "测试 12：替换失败（旧文本不存在引发的安全回退）",
            textwrap.dedent('''"\"\"\"<<<<\n- value: does_not_exist\n====\n- value: new\n>>>>\"\"\" | ~>/target_12.md::DATA'''),
            no_match_content,
            "target_12.md", ws, no_match_content)

        multi_special_content = textwrap.dedent("""\
        # CONFIG
        - pattern1: a*b
        - pattern2: c.d
        """)
        await run_test_step(runtime, "测试 13：多个块混合特殊字符匹配",
            textwrap.dedent('''"\"\"\"<<<<\n- pattern1: a*b\n====\n- pattern1: star-star\n>>>>\n<<<<\n- pattern2: c.d\n====\n- pattern2: dot-dot\n>>>>\"\"\" | ~>/target_13.md::CONFIG'''),
            textwrap.dedent("""# CONFIG\n- pattern1: star-star\n- pattern2: dot-dot\n"""),
            "target_13.md", ws, multi_special_content)

        whitespace_content = textwrap.dedent("""\
        # CONFIG
        - key: value  with  multiple  spaces
        """)
        await run_test_step(runtime, "测试 14：空白差异（模拟大模型生成文本带来的不一致）",
            textwrap.dedent('''"""<<<<\n- key: value with multiple spaces\n====\n- key: replaced\n>>>>""" | ~>/target_14.md::CONFIG'''),
            whitespace_content,
            "target_14.md", ws, whitespace_content)

        crlf_content = "# CONFIG\r\n- key: value\r\n"
        await run_test_step(runtime, "测试 15：换行符差异（CRLF vs LF 抹平）",
            textwrap.dedent('''"""<<<<\n- key: value\n====\n- key: replaced\n>>>>""" | ~>/target_15.md::CONFIG'''),
            "# CONFIG\r\n- key: replaced\r\n",
            "target_15.md", ws, crlf_content)

        nested_content = textwrap.dedent("""\
        # DATA
        - text: This contains special markers (<<< and ===)
        """)
        await run_test_step(runtime, "测试 16：补丁块嵌套标记防注入",
            textwrap.dedent('''"""<<<<\n- text: This contains special markers (<<< and ===)\n====\n- text: Replaced with nested markers inside\n>>>>""" | ~>/target_16.md::DATA'''),
            textwrap.dedent("""# DATA\n- text: Replaced with nested markers inside\n"""),
            "target_16.md", ws, nested_content)

        empty_old_content = textwrap.dedent("""\
        # DATA
        - key: value
        """)
        await run_test_step(runtime, "测试 17：空旧文本 (退化为忽略)",
            textwrap.dedent('''"""<<<<\n\n====\n- new: inserted\n>>>>""" | ~>/target_17.md::DATA'''),
            empty_old_content,
            "target_17.md", ws, empty_old_content)

        delete_content = textwrap.dedent("""\
        # DATA
        - key: to be deleted
        - keep: this
        """)
        await run_test_step(runtime, "测试 18：删除操作（新文本为空）",
            textwrap.dedent('''"""<<<<\n- key: to be deleted\n====\n\n>>>>""" | ~>/target_18.md::DATA'''),
            textwrap.dedent("""# DATA\n\n- keep: this\n"""),
            "target_18.md", ws, delete_content)

        substring_content = textwrap.dedent("""\
        # DATA
        - line: abcdefgh
        """)
        await run_test_step(runtime, "测试 19：子串非贪婪匹配修改",
            textwrap.dedent('''"""<<<<\ncdef\n====\nXYZ\n>>>>""" | ~>/target_19.md::DATA'''),
            textwrap.dedent("""# DATA\n- line: abXYZgh\n"""),
            "target_19.md", ws, substring_content)


        # 测试 20：极多连续补丁块（压力测试 & CRC计算验证）

        print("\n=== 测试 20：极多连续补丁块（压力测试） ===")
        content_lines = [f"- item{i}: value{i}" for i in range(100)]
        many_blocks_content = "# DATA\n" + "\n".join(content_lines) + "\n"
        filename = "target_20.md"
        # WT VFS: VFS write
        runtime._workspace.write_locator("genesis", f"/{filename}", SelectorBuilder().build(), "/>", many_blocks_content, auto_scaffold=True, god=True)

        patch_blocks = []
        for i in range(50):
            patch_blocks.append(f"<<<<\n- item{i}: value{i}\n====\n- item{i}: replaced{i}\n>>>>")

        cmd_20 = '"""' + '\n'.join(patch_blocks) + '""" | ~>/target_20.md::DATA'
        print("🚀 指令: [50个补丁块省略, 测试连环定锚与偏移计算]")
        await runtime.exec(cmd_20, active_doc="/target_20.md", wt_uid="genesis")

        final_20 = runtime.store.get_text("genesis", "/target_20.md")

        lines = final_20.splitlines()
        assert lines[0] == "# DATA", f"标题错误: {lines[0]}"
        for i in range(50):
            assert f"- item{i}: replaced{i}" in final_20, f"Item {i} 未被替换"
        for i in range(50, 100):
            assert f"- item{i}: value{i}" in final_20, f"Item {i} 丢失, 并发坐标偏移发生碰撞！"
        print("✅ 【测试 20：极多连续补丁块（压力测试）】通过")


        # 测试 21：多字节字符边界 (Rust UTF-16/8 Metric 对齐验证)

        unicode_content = textwrap.dedent("""\
        # DATA
        - emoji: 🚀🔥
        - chinese: 测试文本
        """)
        await run_test_step(runtime, "测试 21：多字节字符与 Emoji 物理边界修改",
            textwrap.dedent('''"""<<<<\n- emoji: 🚀🔥\n====\n- emoji: ✅✨\n>>>>\n<<<<\n- chinese: 测试文本\n====\n- chinese: 替换文本\n>>>>""" | ~>/target_21.md::DATA'''),
            textwrap.dedent("""# DATA\n- emoji: ✅✨\n- chinese: 替换文本\n"""),
            "target_21.md", ws, unicode_content)

        print("\n🏆========================================================🏆")
        print("🎉 VFS NEO: ALL 21 AIDER-PATCH SURGICAL OPERATIONS PASSED! 🎉")
        print("🏆========================================================🏆\n")


if __name__ == "__main__":
    asyncio.run(test_patch())
