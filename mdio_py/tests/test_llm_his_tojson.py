"""LLM HIS 记录解析 / state kv 的 tojson（引擎 AST）对拍。

oracle：
  - 记录解析：`llm._parse_his_records`（行解析）
  - state kv：`mdio_py.core.text_utils.parse_kv_lines`
当前路径：`workspace.to_json_text(text)`（引擎 AST 信封，复用 FFI `parse_to_ast`）
        + `mdio_py.core.ast_utils.{inline_text, walk, bullets_to_kv}`

一、`_read_state`
  做法：`bullets_to_kv(json.loads(workspace.to_json_text(chunks[0])))`
  全等类（实测逐键一致）：平铺 `- k: v`、空值删除、值里带冒号、嵌套子项、重复键（后者胜）
  差异类（2 条，逐条钉住）：
    V1 续行（软换行）→ oracle 只取本级行 `line1`，当前实现折进值 `line1   line2`
    V2 行内标记       → oracle 保留 `` `mk -f` ``，当前实现去标记留 `mk -f`
  真实 proc 行由 `_write_proc` 逐键写成**单行平铺、无行内标记**的值 ⇒ 落在全等类。

二、ic 约定下的 `_parse_his_records(his_text, workspace=None)`
  载荷是 **ic 字面区**，记录 / 字段 / 载荷三者都从引擎 JSON 取：
    记录 = 顶层 `bl` 的 `b`（`role = inline_text(t)`，带内联值的 bullet 不算记录）
    字段 = 记录下嵌套 `bl` 的 `b`（键 `t` / 值 `ct`）
    载荷 = 记录下 `bq` 里 **`ic` 的 `cd`**（引擎已剥掉容器前缀与 4 空格缩进 ⇒ 读侧零去排版）
  实测：真实写入器产出的样例**逐字节全等**（含空行、嵌套记录形状、软换行）。
  **只支持引擎 JSON**：没有句柄或引擎异常 ⇒ 空列表。

本文件的差异断言 = 实测集合，且有 canary（`*_canary_*`）：helper/引擎语义变化时先失败。
"""

import json

import pytest

from mdio_py.core.ast_utils import bullets_to_kv, inline_text
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.selector_builder import drill
from mdio_py.core.text_utils import parse_kv_lines
from mdio_py.plugins.llm.llm import (
    _append_record,
    _parse_his_records,
    _read_his,
    _read_state,
    _write_proc,
)

# 真实 HIS 样例（与 test_llm_his_bq.py 的写入集合同源，逐条覆盖格式类别）
NASTY = "第一段\n- 列表行\n> 引用行\n\n第二段"
NESTED = "- user:\n  > 嵌套形状也照原样读回"
HIS_CASES = [
    ("user", "你好", {}),
    ("assistant", NASTY, {}),
    ("thinking", "", {}),
    ("tool_call", '{"pipeline": "$ /a.md::X"}', {"name": "execute_mdio", "id": "call_8f3a"}),
    ("tool_result", "- mdio_success: +> /a.md\n\n- E_TIMEOUT: 17",
     {"name": "execute_mdio", "id": "call_8f3a", "status": "ok"}),
    ("assistant", NESTED, {}),
    ("assistant", "带 emoji 🎯 与全角：，。\n尾行", {}),
]


def _write_real_his(rt, agent: str = "t1") -> str:
    """用真实写入器产出真实 HIS 文本并整节读回。"""
    ws = rt._workspace
    ctx, cfg = RuntimeContext(wt_uid="genesis"), {"his": "HIS"}
    for role, payload, fields in HIS_CASES:
        _append_record(ws, agent, "HIS", role, payload, cfg, ctx, fields=fields)
    return _read_his(ws, agent, "HIS", cfg, ctx)


# 一、真实 HIS 文本：生产读法（ic + JSON）逐字节 / tool_call 重建

async def test_his_ic_roundtrip_is_byte_equal(runtime_llm, sandbox_id):
    """真实写入器写出的 ic 载荷，生产读法逐字节还原（角色 / 字段 / 载荷）。"""
    raw = _write_real_his(runtime_llm)
    recs = _parse_his_records(raw, workspace=runtime_llm._workspace)

    assert [r["role"] for r in recs] == [c[0] for c in HIS_CASES]
    for i, (role, payload, fields) in enumerate(HIS_CASES):
        assert recs[i]["fields"] == {k: v for k, v in fields.items() if v}, \
            f"记录#{i} 字段不等：{recs[i]['fields']}"
        assert recs[i]["content"] == payload, f"记录#{i} 载荷不等：{recs[i]['content']!r}"



async def test_tool_call_arguments_rebuilt_from_tool_field(runtime_llm, sandbox_id):
    """`- tool:` 字段 + ic 载荷 ⇒ `arguments` 用 json.dumps 重建，逐字节等于原 args。"""
    from mdio_py.plugins.llm.llm import _rebuild_messages, _tc_arguments
    from mdio_py.core.selector_builder import SelectorBuilder as SB
    from mdio_py.core.interpreter import RuntimeContext

    ws = runtime_llm._workspace
    ctx = RuntimeContext(wt_uid="genesis")
    raw_args = '{"pipeline": "用 `mdio` 读 *重点* 与 [doc](http://x/y)"}'
    _append_record(ws, "tc1", "HIS", "assistant", "我来跑", {}, ctx)
    _append_record(ws, "tc1", "HIS", "tool_call", "用 `mdio` 读 *重点* 与 [doc](http://x/y)",
                   {}, ctx, fields={"name": "execute_mdio", "id": "call_1", "tool": "pipeline"})
    _append_record(ws, "tc1", "HIS", "tool_result", "- mdio_success: X", {}, ctx,
                   fields={"name": "execute_mdio", "id": "call_1"})

    assert _tc_arguments({"tool": "pipeline"}, "用 `mdio` 读 *重点* 与 [doc](http://x/y)") == raw_args
    msgs = _rebuild_messages(_read_his(ws, "tc1", "HIS", {}, ctx), workspace=ws)
    tc = msgs[0]["tool_calls"][0]                      # tool_call 挂到前一条 assistant
    assert tc["id"] == "call_1" and tc["function"]["name"] == "execute_mdio"
    assert tc["function"]["arguments"] == raw_args
    assert msgs[1] == {"role": "tool", "tool_call_id": "call_1",
                       "name": "execute_mdio", "content": "- mdio_success: X"}


async def test_tool_call_without_tool_field_uses_payload_as_json(runtime_llm, sandbox_id):
    """没有 `- tool:` 字段（多参数 / 旧记录）⇒ 载荷本身就是整串 JSON，原样使用。"""
    from mdio_py.plugins.llm.llm import _tc_arguments
    raw_args = '{"path": "/a.md", "mode": "ro"}'
    assert _tc_arguments({}, raw_args) == raw_args
    assert _tc_arguments({}, "") == "{}"


async def test_record_parser_line_parse_plain_bq(runtime_llm, sandbox_id):
    """架构钉：只 bq 新形走**行解析**（md 源逐字节保真）；句柄参数保留但不参与解析。"""
    raw = _write_real_his(runtime_llm)
    ws = runtime_llm._workspace

    recs = _parse_his_records(raw, workspace=ws)
    assert recs and recs[1]["content"] == NASTY       # md 源逐字节
    assert _parse_his_records(raw) == recs             # 有无句柄结果一致(行解析)


RICH = "\n".join([
    "排序结果：", "", "> 今天的错误分布", "", "1. E_CONN_REFUSED —— 42 次", "2. E_TIMEOUT —— 17 次",
    "", "复现：", "", "```bash", "grep -c E_CONN_REFUSED /logs/today.md", "```", "",
    "    缩进行也原样（这四空格是载荷自己的）", "", "- user:", "  这条形似记录，其实是载荷内容",
])
ALL_CASES = [
    ("user", {}, "看一下 /logs/today.md 里的错误，按频率排一下\n\n- 只要今天的\n- 排完说最该先修哪个"),
    ("thinking", {}, "先取 ERRORS 节；用 sel 统计频率；排序后给建议。"),
    ("assistant", {}, "我先读一遍日志。"),
    ("tool_call", {"name": "execute_mdio", "id": "call_8f3a", "tool": "pipeline"},
     "$ /logs/today.md::ERRORS"),
    ("tool_result", {"name": "execute_mdio", "id": "call_8f3a"},
     "- mdio_success: $ /logs/today.md::ERRORS\n\n- E_CONN_REFUSED: 42\n- E_TIMEOUT: 17"),
    ("thinking", {}, "连接层问题会连带放大超时，先修连接。"),
    ("assistant", {}, RICH),
    ("tool_call", {"name": "execute_mdio", "id": "call_11be", "tool": "pipeline"},
     "@LLM.actor_libai.run"),
    ("tool_result", {"name": "execute_mdio", "id": "call_11be"},
     "李白说：行路难，行路难。\n\n- 论点一：世路艰难\n- 论点二：仍要举杯"),
    ("assistant", {}, "综合两边，先修 E_CONN_REFUSED。"),
    ("user", {}, ""),
]


async def test_all_elements_roundtrip(runtime_llm, sandbox_id):
    """全要素样例逐字节：四类角色 + thinking、空行、缩进行、自带围栏、形似记录、空载荷。"""
    ws, ctx = runtime_llm._workspace, RuntimeContext(wt_uid="genesis")
    for role, fields, payload in ALL_CASES:
        _append_record(ws, "all1", "HIS", role, payload, {}, ctx, fields=fields)

    recs = _parse_his_records(_read_his(ws, "all1", "HIS", {}, ctx), workspace=ws)
    assert [(r["role"], r["content"]) for r in recs] == [(c[0], c[2]) for c in ALL_CASES]
    for i, (_, fields, _) in enumerate(ALL_CASES):
        assert recs[i]["fields"] == {k: v for k, v in fields.items() if v}, f"记录#{i} 字段不等"


def test_tool_record_helpers_roundtrip_arguments():
    """写侧字段/载荷选择 + 读侧 arguments 重建：单参数逐字节，多参数回退整串 JSON。"""
    from mdio_py.plugins.llm.llm import (_tc_arguments, _tool_record_fields,
                                         _tool_record_payload)
    single = {"name": "execute_mdio", "id": "c1", "pipeline": "$ /a.md::X",
              "raw_args": '{"pipeline": "$ /a.md::X"}', "tool": "pipeline"}
    assert _tool_record_payload(single) == "$ /a.md::X"
    assert _tool_record_fields(single) == {"name": "execute_mdio", "id": "c1", "tool": "pipeline"}
    assert _tc_arguments(_tool_record_fields(single), _tool_record_payload(single)) \
        == single["raw_args"]

    multi = {"name": "execute_mdio", "id": "c2", "pipeline": "",
             "raw_args": '{"path": "/a.md", "mode": "ro"}', "tool": ""}
    assert _tool_record_payload(multi) == '{"path": "/a.md", "mode": "ro"}'
    assert _tc_arguments(_tool_record_fields(multi), _tool_record_payload(multi)) \
        == multi["raw_args"]


# 二、state kv 对拍（_read_state → tojson）

KV_EQUAL = {
    "flat": "- turn: 3\n- status: WAIT_TOOL\n- step: 1\n- sys_hook_id: 123\n",
    "empty_deleted": "- turn: 2\n- status: BUSY\n",
    "colon_in_value": "- turn: 1\n- error_detail: [LLM] auth: deepseek — check API key\n",
    "nested_child": "- turn: 1\n- status: DONE\n  - extra: inner\n",
    "dup_key": "- turn: 1\n- turn: 2\n- status: DONE\n",
}
KV_DIVERGENT = {
    # V1：续行（软换行）→ 旧只取本级行，新折进值
    "continuation": ("- turn: 1\n- error_detail: line1\n    line2\n- status: ERROR\n",
                     {"turn": "1", "error_detail": "line1", "status": "ERROR"},
                     {"turn": "1", "error_detail": "line1   line2", "status": "ERROR"}),
    # V2：行内标记 → 旧保留反引号，新去标记留文本
    "inline_mark": ("- turn: 1\n- error_detail: use `mk -f` first\n",
                    {"turn": "1", "error_detail": "use `mk -f` first"},
                    {"turn": "1", "error_detail": "use mk -f first"}),
}


def _new_kv(workspace, text: str) -> dict:
    return bullets_to_kv(json.loads(workspace.to_json_text(text)))


async def test_state_kv_equal_classes(runtime_llm, sandbox_id):
    """全等类：平铺 / 空值删除 / 值里带冒号 / 嵌套子项 / 重复键。"""
    ws = runtime_llm._workspace
    for name, text in KV_EQUAL.items():
        assert parse_kv_lines(text) == _new_kv(ws, text), f"{name} 新旧不等"


async def test_state_kv_divergence_V1_continuation_line(runtime_llm, sandbox_id):
    """V1：续行折进值（旧只取本级行）。"""
    ws = runtime_llm._workspace
    text, old_exp, new_exp = KV_DIVERGENT["continuation"]
    assert parse_kv_lines(text) == old_exp
    assert _new_kv(ws, text) == new_exp


async def test_state_kv_divergence_V2_inline_mark(runtime_llm, sandbox_id):
    """V2：行内标记被去标记（值文本保留）。"""
    ws = runtime_llm._workspace
    text, old_exp, new_exp = KV_DIVERGENT["inline_mark"]
    assert parse_kv_lines(text) == old_exp
    assert _new_kv(ws, text) == new_exp


async def test_state_kv_canary_exactly_measured_set(runtime_llm, sandbox_id):
    """canary：state kv 的差异形状集合必须恰好等于实测集合。"""
    ws = runtime_llm._workspace
    diverged = {n for n, t in {**KV_EQUAL, **{k: v[0] for k, v in KV_DIVERGENT.items()}}.items()
                if parse_kv_lines(t) != _new_kv(ws, t)}
    assert diverged == set(KV_DIVERGENT), f"差异集合变了：{diverged}"


async def test_read_state_matches_oracle_on_real_proc_row(runtime_llm, sandbox_id):
    """`_read_state` 与 oracle 在同一条真实 proc 行上逐键一致。"""
    ws, ctx = runtime_llm._workspace, RuntimeContext(wt_uid=sandbox_id)
    _write_proc(ws, "t1", "main", ctx, turn="2", status="WAIT_TOOL", step="1")

    chunks = ws.read_locator("genesis", "/.mds/proc.md", drill("PROCESS_TABLE.LLM.t1.main"))
    raw_row = chunks[0] if chunks else ""
    oracle = parse_kv_lines(raw_row)

    state = _read_state(ws, "t1", "main", ctx)
    for key in ("turn", "status", "step"):
        assert state[key] == oracle[key], f"{key}: {state[key]!r} vs {oracle[key]!r}"
    assert (state["turn"], state["status"], state["step"]) == ("2", "WAIT_TOOL", "1")
