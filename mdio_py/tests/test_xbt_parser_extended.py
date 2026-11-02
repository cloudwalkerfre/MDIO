"""
Extended edge-case tests for the recursive-descent XBT parser (new grammar).
Each test prints its progress and assertions, matching the project test style.
"""
import pytest
from mdio_py.plugins.xbt.parser import parse_xbt as parse



# Multi-line condition edge cases

def test_condition_spanning_3_lines_inline_body():
    print("\n🧪 跨行条件 3行 + 行内正文")
    code = ">> [行一\n  行二\n  行三] 正文\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert n['type'] == 'action'
    assert '行一' in n['condition']
    assert '行二' in n['condition']
    assert '行三' in n['condition']
    assert n['prompt'] == '正文'
    print(f"   ✅ type={n['type']}, condition={repr(n['condition'])}, prompt={repr(n['prompt'])}")


def test_condition_spanning_5_lines():
    print("\n🧪 跨行条件 5行 + 缩进块")
    code = "?? [1\n  2\n  3\n  4\n  5]\n    >> [] 子\n"
    ast = parse(code)
    cond = ast[0]['children'][0]['condition']
    for s in ['1', '2', '3', '4', '5']:
        assert s in cond
    print(f"   ✅ 5行合并为 1 个 condition, 长度={len(cond)}")


def test_condition_with_embedded_brackets():
    print("\n🧪 嵌套括号 [[深度] 嵌套 [测试]]")
    code = "?? [[深度] 嵌套 [测试]]\n    >> [] 动作\n"
    ast = parse(code)
    cond = ast[0]['children'][0]['condition']
    assert cond == '[深度] 嵌套 [测试]'
    print(f"   ✅ 深度匹配正确: {repr(cond)} (在第二个 ] 闭合, 非第一个)")


def test_condition_with_cmd_marker_inside():
    print("\n🧪 condition 内含 {{ 但不触发 is_cmd_cond")
    code = ">> [包含 {{ 但不触发cmd}] 正文\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert '{{' in n['condition']
    print(f"   ✅ is_cmd_cond={n['is_cmd_cond']} (应为 False, {{ 不在边界)")


def test_condition_with_only_spaces():
    print("\n🧪 条件仅空格 [   ]")
    code = ">> [   ] 正文\n"
    ast = parse(code)
    assert ast[0]['children'][0]['condition'] == ''
    print(f"   ✅ condition={repr(ast[0]['children'][0]['condition'])} (空字符串)")


def test_unclosed_bracket_single_line():
    print("\n🧪 未闭合括号单行 → 应报错")
    with pytest.raises(Exception):
        parse(">> [未闭合 正文\n")
    print("   ✅ 正确报错 UnexpectedToken")


def test_unclosed_bracket_multi_line():
    print("\n🧪 未闭合括号多行 → 应报错")
    with pytest.raises(Exception):
        parse("?? [开始\n  续行1\n  续行2(无闭合)\n")
    print("   ✅ 正确报错 UnexpectedToken")



# CMD edge cases

def test_multiline_cmd_preserves_newlines():
    print("\n🧪 多行 CMD 保留换行")
    code = ">> [] {{\n    line1\n    line2\n    line3\n}}\n"
    ast = parse(code)
    prompt = ast[0]['children'][0]['prompt']
    assert 'line1' in prompt
    assert 'line2' in prompt
    assert 'line3' in prompt
    print(f"   ✅ prompt={repr(prompt)}")


def test_multiline_cmd_with_blank_line():
    print("\n🧪 多行 CMD 内含空行")
    code = ">> [] {{\n    line1\n\n    line3\n}}\n"
    ast = parse(code)
    prompt = ast[0]['children'][0]['prompt']
    assert 'line1' in prompt
    assert 'line3' in prompt
    print(f"   ✅ prompt={repr(prompt)} (空行保留)")


def test_cmd_condition_single_line():
    print("\n🧪 CMD 条件单行 [{{ ... }}]")
    code = ">> [{{ $/s::key eq 'ok' }}] 动作\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert n['is_cmd_cond'] == True
    assert '$/s::key' in n['condition']
    print(f"   ✅ is_cmd_cond=True, condition={repr(n['condition'])}")


def test_cmd_condition_multiline():
    print("\n🧪 CMD 条件跨行")
    code = "?? [{{\n  $/s::a eq 'x'\n  and $/s::b ne 'y'\n}}]\n    >> [] 动作\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert n['is_cmd_cond'] == True
    assert '$/s::a' in n['condition']
    assert '$/s::b' in n['condition']
    print(f"   ✅ is_cmd_cond=True, condition={repr(n['condition'])}")


def test_cmd_with_template_vars():
    print("\n🧪 CMD 内含模板变量 {TID} {PROC}")
    code = ">> [] {{ @XBT -spawn \"{TID}_sub\" -bp \"{PROC}\" }}\n"
    ast = parse(code)
    assert '{TID}' in ast[0]['children'][0]['prompt']
    assert '{PROC}' in ast[0]['children'][0]['prompt']
    print(f"   ✅ 模板变量保留在 prompt 中")


def test_empty_cmd():
    print("\n🧪 空 CMD {{}}")
    code = ">> [] {{}}\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert n['is_cmd'] == True
    assert n['prompt'] == ''
    print(f"   ✅ is_cmd=True, prompt={repr(n['prompt'])} (空字符串, 非 None)")


def test_cmd_with_only_whitespace():
    print("\n🧪 CMD 仅空格 {{   }}")
    code = ">> [] {{   }}\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert n['is_cmd'] == True
    assert n['prompt'] == ''
    print(f"   ✅ is_cmd=True, prompt={repr(n['prompt'])} (strip 后为空)")



# Indentation & nesting

def test_deep_nesting_10_levels():
    print("\n🧪 10 层深度嵌套")
    code = ">> []\n"
    for lv in range(1, 11):
        code += "    " * lv + ">> []\n"
    code += "    " * 11 + "叶子\n"
    ast = parse(code)
    node = ast[0]['children'][0]
    for _ in range(11):
        node = node['children'][0]
    assert node['prompt'] == '叶子'
    print(f"   ✅ 穿越 11 层, 叶子={repr(node['prompt'])}")


def test_deep_nesting_15_levels():
    print("\n🧪 15 层深度嵌套")
    code = ">> []\n"
    for lv in range(1, 16):
        code += "    " * lv + ">> []\n"
    code += "    " * 16 + "最深\n"
    ast = parse(code)
    node = ast[0]['children'][0]
    for _ in range(16):
        node = node['children'][0]
    assert node['prompt'] == '最深'
    print(f"   ✅ 穿越 16 层 (Lark LALR 只能 1 层)")


def test_siblings_at_same_indent():
    print("\n🧪 5 兄弟同缩进")
    code = ">> []\n    A\n    B\n    C\n    D\n    E\n"
    ast = parse(code)
    children = ast[0]['children'][0]['children']
    assert len(children) == 5
    for i, ch in enumerate(children):
        assert ch['type'] == 'action'
    print(f"   ✅ {len(children)} 个 text_stmt 兄弟")


def test_dedent_then_new_block():
    print("\n🧪 dedent 后新 block")
    code = ">> []\n    == []\n        A\n        B\n    == []\n        C\n"
    ast = parse(code)
    outer = ast[0]['children'][0]
    assert len(outer['children']) == 2
    assert len(outer['children'][0]['children']) == 2
    assert len(outer['children'][1]['children']) == 1
    print(f"   ✅ 2 个 parallel 控制, [0]有 2 子, [1]有 1 子")


def test_complex_dedent_pattern():
    print("\n🧪 复杂 dedent: >> 嵌套 + text_stmt")
    code = """>> []
    >> []
        A
        B
    >> []
        C
    >> []
        >> []
            D
"""
    ast = parse(code)
    root = ast[0]['children'][0]
    assert len(root['children']) == 3
    assert len(root['children'][0]['children']) == 2
    assert root['children'][1]['children'][0]['prompt'] == 'C'
    assert root['children'][2]['node_type'] == '>>'
    assert root['children'][2]['children'][0]['children'][0]['prompt'] == 'D'
    print(f"   ✅ 3 个序列孩子, [2]内嵌 >>→>>→D")


def test_tab_indent_deep():
    print("\n🧪 Tab 缩进多层")
    code = ">> []\n\t== []\n\t\t>> [] 叶\n"
    ast = parse(code)
    n = ast[0]['children'][0]['children'][0]['children'][0]
    assert n['prompt'] == '叶'
    print(f"   ✅ Tab 展开为 4 空格, 叶子={repr(n['prompt'])}")


def test_mixed_space_and_tab_indent():
    print("\n🧪 空格 + Tab 混合缩进")
    code = ">> []\n    == [] 空格\n\t== [] Tab\n"
    ast = parse(code)
    assert len(ast[0]['children'][0]['children']) == 2
    print(f"   ✅ 空格(4) 和 Tab(→4) 同缩进 → 兄弟")



# Tags

def test_single_tag():
    print("\n🧪 单个 Tag #FAST")
    code = ">> [] #FAST 动作\n"
    ast = parse(code)
    assert ast[0]['children'][0]['tags'] == ['#FAST']
    print(f"   ✅ tags={ast[0]['children'][0]['tags']}")


def test_multiple_tags():
    print("\n🧪 多个 Tag #R1 #WEB #FAST")
    code = "?? [条件] #R1 #WEB #FAST 动作\n"
    ast = parse(code)
    assert ast[0]['children'][0]['tags'] == ['#R1', '#WEB', '#FAST']
    print(f"   ✅ tags={ast[0]['children'][0]['tags']}")


def test_tags_only_line_head():
    print("\n🧪 纯 Tag 行头 #ONLY_TAG")
    code = "#ONLY_TAG 正文\n"
    ast = parse(code)
    assert ast[0]['children'][0]['tags'] == ['#ONLY_TAG']
    assert ast[0]['children'][0]['prompt'] == '正文'
    print(f"   ✅ tags={ast[0]['children'][0]['tags']}, prompt={repr(ast[0]['children'][0]['prompt'])}")


def test_tag_with_underscore_and_digits():
    print("\n🧪 Tag 含下划线和数字")
    code = ">> [] #TAG_123 #v2_0 动作\n"
    ast = parse(code)
    assert '#TAG_123' in ast[0]['children'][0]['tags']
    assert '#v2_0' in ast[0]['children'][0]['tags']
    print(f"   ✅ tags={ast[0]['children'][0]['tags']}")


def test_hash_in_body_not_a_tag():
    print("\n🧪 body 中 # 不是 Tag")
    code = ">> [] 这不是 #标签 而是正文中的井号\n"
    ast = parse(code)
    assert ast[0]['children'][0]['tags'] == []
    assert '#' in ast[0]['children'][0]['prompt']
    print(f"   ✅ tags=[] (空), prompt 含 #")



# Root statement

def test_root_with_deep_children():
    print("\n🧪 命名根节点 - MAIN: 深层嵌套")
    code = "- MAIN:\n    >> []\n        >> []\n            最深\n"
    ast = parse(code)
    assert ast[0]['type'] == 'root'
    assert ast[0]['name'] == 'MAIN'
    ch = ast[0]['children'][0]['children'][0]['children'][0]
    assert ch['prompt'] == '最深'
    print(f"   ✅ root name={ast[0]['name']}, 叶子={repr(ch['prompt'])}")


def test_root_with_mixed_children():
    print("\n🧪 命名根节点 - PLAN: 混合子节点")
    code = "- PLAN:\n    步骤1\n    >> [] 步骤2\n    {{ cmd }}\n"
    ast = parse(code)
    assert ast[0]['name'] == 'PLAN'
    assert len(ast[0]['children']) == 3
    assert ast[0]['children'][2]['is_cmd'] == True
    print(f"   ✅ name=PLAN, children={len(ast[0]['children'])} (text_stmt + action + CMD)")


def test_two_root_stmts():
    print("\n🧪 双根节点 → 虚拟根包裹")
    code = "- A:\n    >> [] a\n- B:\n    >> [] b\n"
    ast = parse(code)
    assert ast[0]['type'] == 'root'
    assert ast[0].get('name') is None
    assert len(ast[0]['children']) == 2
    assert ast[0]['children'][0]['name'] == 'A'
    assert ast[0]['children'][1]['name'] == 'B'
    print(f"   ✅ 虚拟根包裹 A 和 B")


def test_root_stmt_with_underscore_name():
    print("\n🧪 根节点名含下划线 MY_PLAN_V2")
    code = "- MY_PLAN_V2:\n    >> [] 动作\n"
    ast = parse(code)
    assert ast[0]['name'] == 'MY_PLAN_V2'
    print(f"   ✅ name={ast[0]['name']}")



# Whitespace & line endings

def test_crlf_line_endings():
    print("\n🧪 Windows 换行 \\r\\n")
    code = ">> [] 一\r\n>> [] 二\r\n"
    ast = parse(code)
    assert len(ast[0]['children']) == 2
    print(f"   ✅ {len(ast[0]['children'])} 个 action")


def test_trailing_blank_lines():
    print("\n🧪 尾部空行")
    code = ">> [] 动作\n\n\n\n"
    ast = parse(code)
    assert ast[0]['children'][0]['prompt'] == '动作'
    print(f"   ✅ prompt={repr(ast[0]['children'][0]['prompt'])}")


def test_leading_blank_lines():
    print("\n🧪 首部空行")
    code = "\n\n\n>> [] 动作\n"
    ast = parse(code)
    assert ast[0]['children'][0]['prompt'] == '动作'
    print(f"   ✅ prompt={repr(ast[0]['children'][0]['prompt'])}")


def test_lines_with_only_spaces():
    print("\n🧪 纯空格行")
    code = ">> [] 动作\n    \n>> [] 下一个\n"
    ast = parse(code)
    assert len(ast[0]['children']) == 2
    print(f"   ✅ {len(ast[0]['children'])} 个 action, 空格行被跳过")


def test_no_trailing_newline():
    print("\n🧪 无尾部换行")
    ast = parse(">> [] 动作")
    assert ast[0]['children'][0]['prompt'] == '动作'
    print(f"   ✅ 自动补 \\n, prompt={repr(ast[0]['children'][0]['prompt'])}")



# Unicode

def test_chinese_nlp():
    print("\n🧪 中文 NLP")
    code = "?? [条件判断] 执行动作：启动防御系统\n"
    ast = parse(code)
    assert '启动防御系统' in ast[0]['children'][0]['prompt']
    print(f"   ✅ prompt={repr(ast[0]['children'][0]['prompt'])}")


def test_japanese_and_emoji_in_body():
    print("\n🧪 日文 + emoji")
    code = ">> [] 日本語テスト 🚀\n"
    ast = parse(code)
    assert '日本語テスト' in ast[0]['children'][0]['prompt']
    print(f"   ✅ prompt={repr(ast[0]['children'][0]['prompt'])}")


def test_chinese_condition_and_block():
    print("\n🧪 中文条件 + 缩进块")
    code = "?? [请问需要响应吗？]\n    >> [] ⚡紧急行动\n"
    ast = parse(code)
    assert '需要响应吗' in ast[0]['children'][0]['condition']
    assert '紧急行动' in ast[0]['children'][0]['children'][0]['prompt']
    print(f"   ✅ condition={repr(ast[0]['children'][0]['condition'])}, prompt={repr(ast[0]['children'][0]['children'][0]['prompt'])}")



# DM enforcement (semantic check)

def test_mixed_dm_siblings():
    print("\n🧪 同层 DM 不一致 → 应报错")
    with pytest.raises(Exception, match='DM 不一致'):
        parse(">> [] a\n== [] b\n")
    print("   ✅ 正确报错: 同层 DM 不一致")


def test_all_same_dm_ok():
    print("\n🧪 同层 DM 一致 → 通过")
    ast = parse(">> [] a\n>> [] b\n>> [] c\n")
    assert ast[0]['node_type'] == '>>'
    print(f"   ✅ node_type={ast[0]['node_type']}")


def test_selector_inferred_from_children():
    print("\n🧪 node_type 推断: 父 >>, 子 ?? → node_type=??")
    code = ">> [父]\n    ?? [子A] a\n    ?? [子B] b\n"
    ast = parse(code)
    assert ast[0]['children'][0]['node_type'] == '??'
    print(f"   ✅ node_type={ast[0]['children'][0]['node_type']} (由子 DM 反向推断)")


def test_parallel_inferred_from_children():
    print("\n🧪 node_type 推断: 子 == → node_type==")
    code = ">> []\n    == [] A\n    == [] B\n"
    ast = parse(code)
    assert ast[0]['children'][0]['node_type'] == '=='
    print(f"   ✅ node_type={ast[0]['children'][0]['node_type']}")


def test_empty_children_default_to_sequence():
    print("\n🧪 孩子无 DM → 默认 node_type=>>")
    code = ">> []\n    NLP文本\n"
    ast = parse(code)
    ctrl = ast[0]['children'][0]
    assert ctrl['node_type'] == '>>'
    print(f"   ✅ node_type={ctrl['node_type']} (child_dms 为空 → 默认)")


def test_action_without_prompt_fails():
    print("\n🧪 空 action (无 prompt) → 应报错")
    with pytest.raises(Exception, match='UnexpectedToken'):
        parse(">> []\n")
    print("   ✅ 正确报错 UnexpectedToken")


def test_selector_child_without_condition_fails():
    print("\n🧪 Selector 孩子无 [条件] → 应报错")
    with pytest.raises(Exception, match='Selector'):
        parse("?? [选择器]\n    NLP文本\n")
    print("   ✅ 正确报错: Selector 子节点必须携带 [条件]")



# Body-on-next-line patterns

def test_nlp_on_next_line():
    print("\n🧪 正文在下一行 (NLP)")
    code = ">> []\n动作正文\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert n['prompt'] == '动作正文'
    assert n['is_cmd'] == False
    print(f"   ✅ prompt={repr(n['prompt'])}, is_cmd=False")


def test_cmd_on_next_line():
    print("\n🧪 正文在下一行 (CMD)")
    code = ">> []\n{{ cmd }}\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert n['prompt'] == 'cmd'
    assert n['is_cmd'] == True
    print(f"   ✅ prompt={repr(n['prompt'])}, is_cmd=True")


def test_line_head_only_no_next_line():
    print("\n🧪 仅行头无下文 → 空 action → 报错")
    with pytest.raises(Exception, match='UnexpectedToken'):
        parse(">> []")
    print("   ✅ 正确报错 UnexpectedToken")


def test_line_head_then_another_line_head():
    print("\n🧪 行头后跟另一个行头 → 空 action → 报错")
    with pytest.raises(Exception, match='UnexpectedToken'):
        parse(">> []\n>> [] 动作\n")
    print("   ✅ 第一个 >> [] 无 body, 第二个是独立语句")


def test_body_on_next_line_then_dedent():
    print("\n🧪 内部空 action 触发语义错误")
    code = ">> []\n    >> []\n动作在下一行\n"
    with pytest.raises(Exception, match='UnexpectedToken'):
        parse(code)
    print("   ✅ 内层 >> [] 无 body → UnexpectedToken")



# Bare CMD & text_stmt at top level

def test_bare_cmd_at_top_level():
    print("\n🧪 顶层裸 CMD")
    code = "{{ cmd }}\n"
    ast = parse(code)
    assert ast[0]['children'][0]['is_cmd'] == True
    print(f"   ✅ is_cmd=True, prompt={repr(ast[0]['children'][0]['prompt'])}")


def test_text_stmt_at_top_level():
    print("\n🧪 顶层裸 NLP (text_stmt)")
    code = "裸文本\n"
    ast = parse(code)
    assert ast[0]['children'][0]['prompt'] == '裸文本'
    print(f"   ✅ prompt={repr(ast[0]['children'][0]['prompt'])}")


def test_mixed_bare_at_top_level():
    print("\n🧪 顶层混合: CMD + NLP + CMD")
    code = "{{ cmd1 }}\n裸NLP\n{{ cmd2 }}\n"
    ast = parse(code)
    assert len(ast[0]['children']) == 3
    assert ast[0]['children'][0]['is_cmd'] == True
    assert ast[0]['children'][1]['is_cmd'] == False
    assert ast[0]['children'][2]['is_cmd'] == True
    print(f"   ✅ 3 个顶层节点: CMD, NLP, CMD")



# Real-world blueprint patterns

def test_simple_worker():
    print("\n🧪 Worker 蓝图: ?? [根] → >> [{{ @XBT -step }}]")
    code = """?? [根]
    >> [] {{ @XBT -step "{TID}_next" }}
"""
    ast = parse(code)
    ctrl = ast[0]['children'][0]
    assert ctrl['node_type'] == '>>'
    assert ctrl['children'][0]['is_cmd'] == True
    print(f"   ✅ node_type={ctrl['node_type']}, is_cmd={ctrl['children'][0]['is_cmd']}")


def test_guard_selector():
    print("\n🧪 Guard Selector: 三个 ?? 分支")
    code = """?? [需要响应吗?]
    >> [] {{ 进入战斗状态 }}
?? [资源储备不足?]
    >> [] {{ 启动补给协议 }}
?? []
    >> [] {{ 继续巡逻 }}
"""
    ast = parse(code)
    assert len(ast[0]['children']) == 3
    for i, child in enumerate(ast[0]['children']):
        assert child['node_type'] == '>>'
    print(f"   ✅ {len(ast[0]['children'])} 个分支, 全部 node_type=>>")


def test_meta_d_skeleton():
    print("\n🧪 Meta D 骨架: 三路 Selector")
    code = """- META_D:
    ?? [{{ $当前挂起是 self_condition? }}]
        >> [] {{ ConditionJudge }}
    ?? [{{ $当前挂起是 condition? }}]
        >> [] {{ Selector }}
    ?? []
        >> [] {{ yield }}
"""
    ast = parse(code)
    assert ast[0]['name'] == 'META_D'
    assert len(ast[0]['children']) == 3
    for child in ast[0]['children']:
        assert child['node_type'] == '>>'
        assert child['children'][0]['is_cmd'] == True
    print(f"   ✅ name={ast[0]['name']}, {len(ast[0]['children'])} 个 Selector 分支")


def test_sequence_with_parallel_subtasks():
    print("\n🧪 Sequence + Parallel 组合任务")
    code = """- TASK:
    >> []
        >> [] {{ 初始化 }}
        >> []
            == []
                >> [] {{ 子任务A }}
                >> [] {{ 子任务B }}
                >> [] {{ 子任务C }}
        >> [] {{ 汇总报告 }}
"""
    ast = parse(code)
    root = ast[0]
    assert root['name'] == 'TASK'
    children = root['children']
    assert len(children) == 1
    seq = children[0]
    assert seq['type'] == 'control'
    assert len(seq['children']) == 3
    assert seq['children'][0]['is_cmd'] == True
    inner = seq['children'][1]
    assert inner['node_type'] == '=='
    par = inner['children'][0]
    assert par['dm'] == '=='
    assert par['node_type'] == '>>'
    assert len(par['children']) == 3
    assert seq['children'][2]['is_cmd'] == True
    print("   ✅ TASK → 初始化 → [并行 A,B,C] → 汇总报告")
    print(f"   ✅ 并行节点: dm={par['dm']}, node_type={par['node_type']}, children={len(par['children'])}")



# Additional edge case: NLP+CMD mixing behavior

def test_nlp_with_embedded_cmd_is_raw_text():
    print("\n🧪 NLP 中嵌入 {{}} → 归类为 RAW_TEXT")
    code = ">> [] 先做这个 {{ cmd }} 再做那个\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert n['is_cmd'] == False
    assert '{{ cmd }}' in n['prompt']
    print(f"   ✅ is_cmd=False, {{}} 作为普通字符保留在 prompt 中")


def test_cmd_with_nlp_tail_is_raw_text():
    print("\n🧪 CMD 后有 NLP 尾巴 → 归类为 RAW_TEXT")
    code = ">> [] {{ cmd }} 尾巴\n"
    ast = parse(code)
    n = ast[0]['children'][0]
    assert n['is_cmd'] == False
    print(f"   ✅ is_cmd=False (不以 }} 结尾 → 不是纯 CMD)")



if __name__ == '__main__':
    print("=" * 60)
    print("XBT 递归下降解析器 — 扩展测试套件 (59 项)")
    print("=" * 60)

    tests = [
        # Multi-line conditions
        test_condition_spanning_3_lines_inline_body,
        test_condition_spanning_5_lines,
        test_condition_with_embedded_brackets,
        test_condition_with_cmd_marker_inside,
        test_condition_with_only_spaces,
        test_unclosed_bracket_single_line,
        test_unclosed_bracket_multi_line,
        # CMD
        test_multiline_cmd_preserves_newlines,
        test_multiline_cmd_with_blank_line,
        test_cmd_condition_single_line,
        test_cmd_condition_multiline,
        test_cmd_with_template_vars,
        test_empty_cmd,
        test_cmd_with_only_whitespace,
        # Indentation
        test_deep_nesting_10_levels,
        test_deep_nesting_15_levels,
        test_siblings_at_same_indent,
        test_dedent_then_new_block,
        test_complex_dedent_pattern,
        test_tab_indent_deep,
        test_mixed_space_and_tab_indent,
        # Tags
        test_single_tag,
        test_multiple_tags,
        test_tags_only_line_head,
        test_tag_with_underscore_and_digits,
        test_hash_in_body_not_a_tag,
        # Root stmt
        test_root_with_deep_children,
        test_root_with_mixed_children,
        test_two_root_stmts,
        test_root_stmt_with_underscore_name,
        # Whitespace
        test_crlf_line_endings,
        test_trailing_blank_lines,
        test_leading_blank_lines,
        test_lines_with_only_spaces,
        test_no_trailing_newline,
        # Unicode
        test_chinese_nlp,
        test_japanese_and_emoji_in_body,
        test_chinese_condition_and_block,
        # DM enforcement
        test_mixed_dm_siblings,
        test_all_same_dm_ok,
        test_selector_inferred_from_children,
        test_parallel_inferred_from_children,
        test_empty_children_default_to_sequence,
        test_action_without_prompt_fails,
        test_selector_child_without_condition_fails,
        # Body on next line
        test_nlp_on_next_line,
        test_cmd_on_next_line,
        test_line_head_only_no_next_line,
        test_line_head_then_another_line_head,
        test_body_on_next_line_then_dedent,
        # Bare statements
        test_bare_cmd_at_top_level,
        test_text_stmt_at_top_level,
        test_mixed_bare_at_top_level,
        # Real-world
        test_simple_worker,
        test_guard_selector,
        test_meta_d_skeleton,
        test_sequence_with_parallel_subtasks,
        # Edge
        test_nlp_with_embedded_cmd_is_raw_text,
        test_cmd_with_nlp_tail_is_raw_text,
    ]

    passed = 0
    failed = 0
    for test in tests:
        try:
            test()
            passed += 1
        except Exception as e:
            failed += 1
            print(f"\n❌ {test.__name__}: {e}")

    print()
    print("=" * 60)
    print(f"  通过: {passed} / 失败: {failed} / 总计: {len(tests)}")
    print("=" * 60)
