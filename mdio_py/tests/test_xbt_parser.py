import pytest
from mdio_py.plugins.xbt.parser import parse_xbt


def check_semantic_rules(ast):
    errors = []
    root_child_dms = [node.get('dm') for node in ast if node.get('dm') is not None]
    if len(set(root_child_dms)) > 1:
        errors.append(f"根节点下子节点 DM 不一致: {set(root_child_dms)}")
    if not ast:
        errors.append("空树")

    def _check(nodes):
        if not nodes:
            return
        dms = [n['dm'] for n in nodes if n.get('dm')]
        if len(set(dms)) > 1:
            errors.append(f"同层 DM 不一致: {set(dms)}")
        for node in nodes:
            if node['type'] == 'control':
                if node.get('prompt'):
                    errors.append("控制节点不应有 prompt")
                if not node.get('children'):
                    errors.append("控制节点没有子节点")
                if node.get('children'):
                    _check(node['children'])
            elif node['type'] == 'action':
                if node.get('children'):
                    errors.append("动作节点不应有子节点")
                if not node.get('prompt'):
                    errors.append("动作节点没有 prompt")
            elif node['type'] == 'root':
                if node.get('children'):
                    _check(node['children'])
    _check(ast)
    return errors


VALID_CASES = [
    {
        "name": "单个动作节点",
        "code": "?? [根] 动作\n",
        "validate": lambda r: len(r) == 1 and r[0]['type'] == 'root' and r[0]['node_type'] == '??' and r[0]['children'][0]['type'] == 'action'
    },
    {
        "name": "Selector + 两个动作",
        "code": "?? [A] a\n?? [B] b\n",
        "validate": lambda r: len(r) == 1 and len(r[0]['children']) == 2
    },
    {
        "name": "Sequence 根",
        "code": ">> [] 一\n>> [] 二\n",
        "validate": lambda r: r[0]['node_type'] == '>>'
    },
    {
        "name": "Parallel 根",
        "code": "== [并发]\n    == [] A\n    == [] B\n",
        "validate": lambda r: r[0]['type'] == 'root' and r[0]['children'][0]['dm'] == '==' and len(r[0]['children'][0]['children']) == 2
    },
    {
        "name": "嵌套控制",
        "code": "?? [主]\n    == [分支]\n        >> [] 子1\n        >> [] 子2\n",
        "validate": lambda r: (r[0]['children'][0]['dm'] == '??' and
                               r[0]['children'][0]['node_type'] == '==' and
                               r[0]['children'][0]['children'][0]['dm'] == '==' and
                               r[0]['children'][0]['children'][0]['node_type'] == '>>')
    },
    {
        "name": "条件跨行",
        "code": "?? [这是一个跨行\n的条件] 动作\n",
        "validate": lambda r: '\n' in r[0]['children'][0]['condition']
    },
    {
        "name": "动作跨行",
        "code": ">> []\n    第一行\n    第二行\n",
        "validate": lambda r: r[0]['children'][0]['type'] == 'control' and len(r[0]['children'][0]['children']) == 2
    },
    {
        "name": "模型/工具标记",
        "code": "?? [测试] #R1 #WEB 联网\n",
        "validate": lambda r: r[0]['children'][0]['tags'] == ['#R1', '#WEB']
    },
    {
        "name": "根节点显式条件",
        "code": "[全局]\n    ?? [子] 动作\n",
        "validate": lambda r: (r[0]['children'][0]['condition'] == '全局' and
                               r[0]['children'][0]['dm'] is None and
                               r[0]['children'][0]['node_type'] == '??')
    },
    {
        "name": "深层嵌套",
        "code": "?? [L1]\n    ?? [L2]\n        ?? [L3]\n            ?? [] 深度\n",
        "validate": lambda r: r[0]['children'][0]['type'] == 'control'
    },
]

PARSE_ERROR_CASES = [
    {
        "name": "空树",
        "code": "",
        "error_contains": "UnexpectedEOF"
    },
    {
        "name": "控制节点带 prompt",
        "code": "?? [非法] {{ 不应该 }}\n    >> [] 子\n",
        "error_contains": "UnexpectedToken"
    },
    {
        "name": "动作节点带子节点",
        "code": "动作\n    >> [] 子\n",
        "error_contains": "UnexpectedToken"
    },
    {
        "name": "条件未闭合",
        "code": "?? [未闭合\n动作\n",
        "error_contains": "UnexpectedToken"
    },
    {
        "name": "Selector 子节点无 [条件]",
        "code": "?? [选择器]\n    NLP文本\n",
        "error_contains": "Selector"
    },
    {
        "name": "根节点后无缩进子节点",
        "code": "[根]\n?? [子] 动作\n",
        "error_contains": "UnexpectedToken"
    },
    {
        "name": "同层 DM 不一致",
        "code": "?? [A] a\n== [B] b\n",
        "error_contains": "DM 不一致"
    },
]


@pytest.mark.parametrize("case", VALID_CASES, ids=lambda c: c["name"])
def test_xbt_valid_parse(case):
    result = parse_xbt(case["code"])
    semantic_errors = check_semantic_rules(result)
    assert not semantic_errors, f"语义错误: {semantic_errors}"
    if case.get("validate"):
        assert case["validate"](result), f"自定义验证失败: {result}"


@pytest.mark.parametrize("case", PARSE_ERROR_CASES, ids=lambda c: c["name"])
def test_xbt_parse_errors(case):
    with pytest.raises(Exception) as exc_info:
        parse_xbt(case["code"])
    if case.get("error_contains"):
        assert case["error_contains"] in str(exc_info.value), \
            f"预期错误包含 '{case['error_contains']}'，实际: {exc_info.value}"

def test_tag_allows_dotted_driver_address():
    """#tag 允许点：LLM.analyst 作为一个 tag 返回；裸 tag 不受影响。"""
    ast = parse_xbt("- PLAN:" + chr(10) + "    >> [] #LLM.analyst 动作")
    assert ast[0]["children"][0]["tags"] == ["#LLM.analyst"]
    ast2 = parse_xbt("- PLAN:" + chr(10) + "    >> [] #analyst 动作")
    assert ast2[0]["children"][0]["tags"] == ["#analyst"]


if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))

