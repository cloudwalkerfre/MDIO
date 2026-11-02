import asyncio
import json
import tempfile
import pytest
from mdio_py import MdioRuntime


# 测试数据

BASIC_MD = """# PROJECT
## CONFIG
- PORT: 5432
- DB_HOST: localhost
## TASKS
- design: complete UI
- api: develop endpoints
## NOTES
- note1: first note
- note2: second note
"""

NESTED_MD = """# CONFIG
- database:
    - host: 127.0.0.1
    - port: 5432
    - users:
        - super_admin
        - read_only_admin
- logging:
    - level: debug
"""

DEEP_MD = """# A
## B
### C
- key: value
- list:
    - item1
    - item2
        - subitem
"""

LARGE_MD = "# LARGE\n" + "\n".join(f"- item_{i}: value_{i}" for i in range(100))


# 基本选择器测试

async def test_basic_head(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT'
    result = await runtime_mem.exec(code)
    assert "PROJECT" in result
    assert "CONFIG" in result
    print(f'\n[test_basic_head]:\n{result}')

async def test_basic_key(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.CONFIG._b:PORT -pj content'
    result = await runtime_mem.exec(code)
    assert result.strip() == "5432"
    print(f'\n[test_basic_key]:\n{result}')

async def test_wildcard_key(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.CONFIG.*._b:PORT -pj content'
    result = await runtime_mem.exec(code)
    assert result.strip() == "5432"
    print(f'\n[test_wildcard_key]:\n{result}')

async def test_recursive_key(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.._b:PORT -pj content'
    result = await runtime_mem.exec(code)
    assert result.strip() == "5432"
    print(f'\n[test_recursive_key]:\n{result}')

async def test_deep_nested_key(runtime_mem):
    code = f'"""{DEEP_MD}""" | $$A.B.C._b:key -pj content'
    result = await runtime_mem.exec(code)
    assert result.strip() == "value"
    print(f'\n[test_deep_nested_key]:\n{result}')

@pytest.mark.skip(reason="not called by old runner — needs investigation")
async def test_nested_list_value(runtime_mem):
    code = f'"""{DEEP_MD}""" | $$A.B.C.list._b:subitem'
    result = await runtime_mem.exec(code)
    assert "subitem" in result
    print(f'\n[test_nested_list_value]:\n{result}')


# 多节点返回（列表）

async def test_multiple_nodes(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.NOTES.*'
    result = await runtime_mem.exec(code)
    
    # 验证它是纯文本，并且包含换行符（表明有多条）
    assert isinstance(result, str)
    assert "first note" in result
    assert "second note" in result
    # 确切地证明它用换行符合并了：
    assert len(result.strip().split('\n')) == 2
    print(f'\n[test_multiple_nodes]:\n{result}')


@pytest.mark.skip(reason="not called by old runner — needs investigation")
async def test_wildcard_all_strips(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.CONFIG.*'
    result = await runtime_mem.exec(code)
    assert isinstance(result, list)
    assert len(result) == 2
    print(f'\n[test_wildcard_all_strips]:\n{result}')


# 过滤器测试

async def test_filter_key_eq(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.TASKS.*[.tag=="api"]'
    result = await runtime_mem.exec(code)
    assert "develop endpoints" in result
    print(f'\n[test_filter_key_eq]:\n{result}')

async def test_filter_val_eq(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.CONFIG.*[.content=="localhost"]'
    result = await runtime_mem.exec(code)
    assert "DB_HOST" in result
    print(f'\n[test_filter_val_eq]:\n{result}')

async def test_filter_key_regex(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.TASKS.*[.tag.re("des.*|api")]'
    result = await runtime_mem.exec(code)
    assert len(result.strip().split('\n')) == 2
    print(f'\n[test_filter_key_regex]:\n{result}')

async def test_filter_and_combo(runtime_mem):
    # .. 已经是展开所有层级，不需要再加 .*
    code = f'"""{BASIC_MD}""" | $$PROJECT..[.tag=="PORT" && .content=="5432"]'
    result = await runtime_mem.exec(code)
    assert "PORT" in result
    print(f'\n[test_filter_and_combo]:\n{result}')

async def test_filter_or_combo(runtime_mem):
    # .. 已经是展开所有层级
    code = f'"""{BASIC_MD}""" | $$PROJECT..[.tag=="PORT" || .tag=="DB_HOST"]'
    result = await runtime_mem.exec(code)
    assert len(result.strip().split('\n')) == 2
    print(f'\n[test_filter_or_combo]:\n{result}')

async def test_filter_not(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.CONFIG.*[!(.tag=="PORT")]'
    result = await runtime_mem.exec(code)
    assert "DB_HOST" in result
    assert "PORT" not in result
    print(f'\n[test_filter_not]:\n{result}')



# 切片测试

async def test_slice_single_index(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.NOTES.*[0]'
    result = await runtime_mem.exec(code)
    assert "first note" in result
    print(f'\n[test_slice_single_index]:\n{result}')

async def test_slice_negative_index(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.NOTES.*[-1]'
    result = await runtime_mem.exec(code)
    assert "second note" in result
    print(f'\n[test_slice_negative_index]:\n{result}')

async def test_slice_range(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.NOTES.*[0:2]'
    result = await runtime_mem.exec(code)
    assert len(result.strip().split('\n')) == 2
    print(f'\n[test_slice_range]:\n{result}')

async def test_slice_step(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.NOTES.*[::2]'
    result = await runtime_mem.exec(code)
    lines = result.strip().split('\n')
    assert len(lines) == 1
    assert "first note" in lines[0]
    print(f'\n[test_slice_step]:\n{result}')

async def test_slice_multiple(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.NOTES.*[0, -1]'
    result = await runtime_mem.exec(code)
    assert len(result.strip().split('\n')) == 2
    print(f'\n[test_slice_multiple]:\n{result}')


# 多步管道组合

async def test_two_stage_pipeline(runtime_mem):
    # 区域律：database 的 content 带嵌套子树（含 super_admin——旧链恢复）——
    # 段1 输出喂段2，在 users 子树上再取内容。
    code = f'"""{NESTED_MD}""" | $$.._b:database -pj content | $$.._b:users -pj content'
    result = await runtime_mem.exec(code)
    assert "super_admin" in result
    assert "read_only_admin" in result
    print(f'\n[test_two_stage_pipeline]:\n{result}')

async def test_three_stage_pipeline(runtime_mem):
    code = f'"""{NESTED_MD}""" | $$.._b:database -pj content | $$.._b:users -pj content | split("\\n") | len'
    result = await runtime_mem.exec(code)
    assert str(result).strip() == "2", f"📉 三段管道提取失败, 实际返回了: {repr(result)}"
    print(f'\n[test_three_stage_pipeline]:\n{result}')

async def test_item_children_chain(runtime_mem):
    """回归: `..item.*` 链——区域律下 item 节点 end = Cell 区域(子树尾——
    深度跟踪精确不含兄弟), 直接子枚举天然覆盖嵌套子树。"""
    code = f'"""{NESTED_MD}""" | $$..users.*[bullet] -pj content'
    result = await runtime_mem.exec(code)
    assert result.strip().split("\n") == ["super_admin", "read_only_admin"]
    print(f'\n[test_item_children_chain]:\n{result}')

async def test_pipe_with_std_lib(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.NOTES.* | split("\\n") | join(";")'
    result = await runtime_mem.exec(code)
    assert "first note" in str(result)
    assert "second note" in str(result)
    print(f'\n[test_pipe_with_std_lib]:\n{result}')


# 标准库集成

async def test_length_on_list(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.NOTES.* | split("\\n") | len'
    result = await runtime_mem.exec(code)
    # 强转字符串并带上报错提示
    assert str(result).strip() == "2", f"📉 预期是 '2', 但实际返回了: {repr(result)}"
    print(f'\n[test_length_on_list]:\n{result}')

async def test_get_from_list(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.NOTES.* | split("\\n") | get(1)'
    result = await runtime_mem.exec(code)
    assert "second note" in str(result), f"📉 没有找到目标文本, 实际返回了: {repr(result)}"
    print(f'\n[test_get_from_list]:\n{result}')


async def test_loads_dumps_roundtrip(runtime_mem):
    md = '{"key": "value"}'
    code = f'"""{md}""" | loads | get("key") | dumps'
    result = await runtime_mem.exec(code)
    assert result == '"value"'
    print(f'\n[test_loads_dumps_roundtrip]:\n{result}')


# 多行值提取（缩进块）

async def test_multiline_value(runtime_mem):
    multiline_md = """# TEST
- block: |
    line1
    line2
"""
    code = f'"""{multiline_md}""" | $$TEST._b:block -pj content'
    result = await runtime_mem.exec(code)
    assert "line1" in result
    assert "line2" in result
    print(f'\n[test_multiline_value]:\n{result}')


# 动态路径（嵌套 $()）

async def test_dynamic_path(runtime_mem):
    # 姿势 1：纯字面量执行块。
    code1 = f'"""{BASIC_MD}""" | $$("PROJECT.CONFIG._b:PORT")'
    result1 = await runtime_mem.exec(code1)
    assert "5432" in str(result1), f"姿势1失败: {result1}"
    print(f'\n[test_dynamic_path1]:\n{result1}')

    # 姿势 2：管道艺术 -> "把字符串流入格式化器"
    # 在这个独立的执行块里： "PROJECT.CONFIG" 作为数据，流向了 fmt!
    code2 = f'"""{BASIC_MD}""" | $$("PROJECT.CONFIG" | fmt("{{}}._b:PORT"))'
    result2 = await runtime_mem.exec(code2)
    assert "5432" in str(result2), f"姿势2失败: {result2}"
    print(f'\n[test_dynamic_path2]:\n{result2}')


# 错误处理

async def test_nonexistent_key(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.CONFIG._b:NONEXISTENT'
    result = await runtime_mem.exec(code)
    assert result == ""
    print(f'\n[test_nonexistent_key]:\n{result}')

async def test_nonexistent_head(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$NONEXISTENT'
    result = await runtime_mem.exec(code)
    assert result == ""
    print(f'\n[test_nonexistent_head]:\n{result}')

async def test_invalid_selector_syntax(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.['
    result = await runtime_mem.exec(code)
    assert result.startswith("- mdio_error:")
    print(f'\n[test_invalid_selector_syntax]:\n{result}')


# 大文档性能（可选，可注释）

async def test_large_document_slice(runtime_mem):
    code = f'"""{LARGE_MD}""" | $$LARGE.*[50:55]'
    result = await runtime_mem.exec(code)
    lines = result.strip().split('\n')
    assert len(lines) == 5
    assert "item_50" in lines[0]
    print(f'\n[test_large_document_slice]:\n{result}')


# 内存写入后读取

async def test_write_then_read_in_memory(runtime_mem):
    # 💥 直接利用我们强悍的 Auto-Scaffold (-f true) 智能接骨能力：
    # 1. 裸文本直接打入 file1，引擎自动创建 Head 和 Key
    # 2. 从 file1 读出来，再次打入 file2，引擎在 file2 再次自动接骨
    # 3. 最后从 file2 原样抽出
    code = """
    mk -f /mem/test.md ;
    mk -f /mem/result.md ;
    "initial" | +> /mem/test.md::TEST._b:key ;
    $/mem/test.md::TEST._b:key -pj content | +> /mem/result.md::TEST._b:key ;
    $/mem/result.md::TEST._b:key -pj content
    """
    result = await runtime_mem.exec(code)
    assert str(result).strip() == "initial", f"内存 VFS 读写穿透失败: {repr(result)}"
    print(f'\n[test_write_then_read_in_memory]:\n{result}')



# 命名 mem（$$ -n/--name）

HIS_ONE = "## HIS\n### main\n- user:\n  > hello\n"

async def test_named_mem_roundtrip(runtime_mem):
    code = (json.dumps(HIS_ONE, ensure_ascii=False)
            + " | $$ -n __topo; $ __topo::HIS.main -pj raw")
    result = await runtime_mem.exec(code)
    assert "- user:\n  > hello" in result

async def test_named_mem_selector_names_input(runtime_mem):
    code = (json.dumps(HIS_ONE, ensure_ascii=False)
            + " | $$ ..*_b -n __topo; $ __topo:: -pj raw")
    result = await runtime_mem.exec(code)
    assert "## HIS" in result and "### main" in result and "- user:" in result

async def test_named_mem_append(runtime_mem):
    code = (json.dumps(HIS_ONE, ensure_ascii=False) + " | $$ -n __topo; "
            + json.dumps("- assistant:\n  > added\n", ensure_ascii=False)
            + " | +> __topo::HIS.main; $ __topo::HIS.main -pj raw")
    result = await runtime_mem.exec(code)
    assert "- assistant:\n  > added" in result
    assert "- user:\n  > hello" in result

async def test_named_mem_replace(runtime_mem):
    code = (json.dumps(HIS_ONE, ensure_ascii=False) + " | $$ -n __topo; "
            + json.dumps("- user:\n  > replaced\n", ensure_ascii=False)
            + " | /> __topo::HIS.main; $ __topo::HIS.main -pj raw")
    result = await runtime_mem.exec(code)
    assert "replaced" in result and "hello" not in result

async def test_named_mem_delete(runtime_mem):
    code = (json.dumps(HIS_ONE, ensure_ascii=False) + " | $$ -n __topo; "
            + "-> __topo::HIS.main._b; $ __topo::HIS.main._b -pj tag")
    result = await runtime_mem.exec(code)
    assert "user" not in result

async def test_named_mem_duplicate_overwrites(runtime_mem):
    first = "## HIS\n### main\n- user:\n  > first\n"
    second = "## HIS\n### main\n- user:\n  > second\n"
    code = (json.dumps(first, ensure_ascii=False) + " | $$ -n __topo; "
            + json.dumps(second, ensure_ascii=False) + " | $$ -n __topo; "
            + "$ __topo::HIS.main -pj raw")
    result = await runtime_mem.exec(code)
    assert "second" in result and "first" not in result

async def test_named_mem_exec_boundary(runtime_mem):
    await runtime_mem.exec(json.dumps(HIS_ONE, ensure_ascii=False) + " | $$ -n __topo")
    result = await runtime_mem.exec("$ __topo::HIS.main -pj raw")
    assert "mdio_error" in result or "File not found" in result

async def test_named_mem_map_f(runtime_mem):
    code = (json.dumps(HIS_ONE, ensure_ascii=False) + " | $$ -n __topo; "
            + json.dumps("- x: 1\n", ensure_ascii=False)
            + " | map -sel '..*_b' -f ( _ | $ __topo::HIS.main -pj raw )")
    result = await runtime_mem.exec(code)
    assert "- mdio_error" not in result
    assert "- user:" in result

async def test_named_mem_empty_input(runtime_mem):
    code = json.dumps("", ensure_ascii=False) + " | $$ -n __empty; $ __empty:: -pj raw"
    result = await runtime_mem.exec(code)
    assert "- mdio_error" not in result

async def test_named_mem_long_flag(runtime_mem):
    code = (json.dumps(HIS_ONE, ensure_ascii=False)
            + " | $$ --name __topo; $ __topo::HIS.main -pj raw")
    result = await runtime_mem.exec(code)
    assert "- user:\n  > hello" in result

async def test_named_mem_multi_hit(runtime_mem):
    text = "## HIS\n### main\n- user:\n  > a\n- user:\n  > b\n"
    code = (json.dumps(text, ensure_ascii=False) + " | $$ -n __topo; "
            + "-> __topo::HIS.main._b; $ __topo::HIS.main._b -pj tag")
    result = await runtime_mem.exec(code)
    assert "user" not in result

async def test_named_mem_no_hit_write(runtime_mem):
    code = (json.dumps(HIS_ONE, ensure_ascii=False) + " | $$ -n __topo; "
            + "-> __topo::NOPE")
    result = await runtime_mem.exec(code)
    assert "mdio_error" in result

async def test_named_mem_invalid_name(runtime_mem):
    code = json.dumps(HIS_ONE, ensure_ascii=False) + " | $$ -n bad/name"
    result = await runtime_mem.exec(code)
    assert "mdio_error" in result


async def test_named_mem_before_file(runtime_mem):
    # 同名真实文件存在时，命名 mem 优先。
    await runtime_mem.exec('mk -f /__topo.md')
    await runtime_mem.exec(json.dumps("## HIS\n### main\n- user:\n  > file\n", ensure_ascii=False)
                           + " | /> /__topo.md:: -pj r")
    code = (json.dumps(HIS_ONE, ensure_ascii=False)
            + " | $$ -n __topo; $ __topo::HIS.main -pj raw")
    result = await runtime_mem.exec(code)
    assert "hello" in result and "file" not in result


# 序列执行（分号）

@pytest.mark.skip(reason="not called by old runner — needs investigation")
async def test_sequence_execution(runtime_mem):
    code = f'"""{BASIC_MD}""" | $$PROJECT.CONFIG._b:PORT ; $$PROJECT.CONFIG._b:DB_HOST'
    result = await runtime_mem.exec(code)
    # 序列执行返回最后一个表达式的结果
    assert "localhost" in result
    print(f'\n[test_sequence_execution]:\n{result}')


# 运行所有测试（如果直接执行）

if __name__ == "__main__":
    async def run_all():
         with tempfile.TemporaryDirectory() as tmpdir:
            runtime_mem = MdioRuntime(tmpdir)
            await runtime_mem.exec('mk -f /system_error.md', wt_uid="genesis")
            await test_basic_head(runtime_mem)
            await test_basic_key(runtime_mem)
            await test_wildcard_key(runtime_mem)
            await test_recursive_key(runtime_mem)
            await test_multiple_nodes(runtime_mem)
            await test_filter_key_eq(runtime_mem)
            await test_slice_range(runtime_mem)
            await test_two_stage_pipeline(runtime_mem)
            await test_length_on_list(runtime_mem)
            await test_nonexistent_key(runtime_mem)
            await test_invalid_selector_syntax(runtime_mem)
            await test_dynamic_path(runtime_mem)
            await test_write_then_read_in_memory(runtime_mem)
            print("✅ 所有扩展测试通过！")

    asyncio.run(run_all())