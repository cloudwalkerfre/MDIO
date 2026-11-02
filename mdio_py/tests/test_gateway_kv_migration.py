"""gateway KV 取数对拍：oracle（`parse_kv_lines` 语义复刻） vs `tojson + bullets_to_kv`。

读点：`workspace.to_json_text(text)` → `mdio_py/core/ast_utils.bullets_to_kv(env)`：

  - `gateway/state.py::runtime_md_read`（runtime.md 字段；入参为 workspace）
  - `gateway/store.py::_parse_gateway_chunks`（gateway.md 条目；`_read_gateway_entries` 调用）

两种取数**不等价**，本文件把实测差异逐类钉成断言。差异集合来自实测，
并有一条 canary（`test_divergence_whitelist_is_exactly_measured_set`）：helper 语义变化时
它先失败，提示同步更新本文件。

一、全等类
  平铺 `- k: v`（含 runtime.md / gateway.md 的真实形状）、`- doc_map: {json}`、
  值里带冒号、引号值、`-k: v`（无空格，两边都忽略）、`- k:`（空值）、
  无冒号 bullet、空行/标题/正文行、重复键（后者胜）、**嵌套子项**（父值不被子文本污染，
  靠 `ast_utils.inline_text` 的容器屏障）、缩进 tab、引用块、表格之后的行、
  **冒号后多余空格**、**CRLF 行尾**（后两类由 helper 的值 strip 拉回全等）。

二、差异类（6 条，逐条断言 + 机制）
  D1 `- m :  s`（冒号前空格）→ oracle {'m': 's'}，当前实现 {}（引擎给空 tag ⇒ 整键丢失）
  D3 懒续行                → oracle 只取本级行，当前实现把续行折进值（软换行变空格）
  D4 `- k:` + 续行         → oracle ''，当前实现 'continued'（续行折进值后 strip）
  D5 围栏代码块内的 `- k: v` → oracle **误读**成键值，当前实现按 AST 正确忽略
  D6 值里的行内代码        → oracle 保留反引号原文 `x`，当前实现去反引号留文本 x
  D7 值里的粗体/斜体/链接  → oracle 保留标记原文，当前实现去标记只留文本

为什么调用方可接受：runtime.md 由 `state.py::runtime_md_write`、gateway.md 由 `store.py`
拼装，都是**单行平铺 `- k: v`（冒号后单空格、无行内标记、LF 行尾）** ⇒ 落在「全等类」。
D5/D6/D7 是行内标记类的规范化（标记被去掉、文本保留）。

store-only 档：`store.py::_parse` 用 `ProjectStore("/mem")`，调 `_parse_gateway_chunks`
时不传引擎句柄 ⇒ 走 `parse_kv_lines`；由 `test_chunk_kernel_legacy_branch_stays_old` 钉住。
"""

import json

import pytest

from mdio_py.core.ast_utils import bullets_to_kv
from mdio_py.gateway.state import RUNTIME_MD, runtime_md_read, runtime_md_write
from mdio_py.gateway.store import GatewayStore


# 两个实现：oracle 与当前路径


def oracle_kv(text: str) -> dict:
    """`parse_kv_lines` 的语义复刻。

    只认 `- ` 开头且 `line[2:]` 含冒号的行；首个冒号切分；键值都 strip；
    同名键后者覆盖前者。
    """
    out: dict = {}
    for line in str(text).split("\n"):
        line = line.strip()
        if not line.startswith("- ") or ":" not in line[2:]:
            continue
        k, v = line[2:].split(":", 1)
        out[k.strip()] = v.strip()
    return out


def migrated_kv(workspace, text: str) -> dict:
    """新路径：`workspace.to_json_text` → 信封 → `bullets_to_kv`。"""
    return bullets_to_kv(json.loads(workspace.to_json_text(text)))


# Part 1: 全等类 —— 新旧必须逐键一致

EQUAL_CASES = {
    # runtime.md 的真实形状（state.py::runtime_md_write 拼装）
    "runtime_md": (
        "# RUNTIME\n- pid: 1234\n- mode: gateway\n- host: 127.0.0.1\n"
        "- port: 8011\n- status: running\n- gateway_enabled: true\n"
    ),
    # gateway.md 条目的真实形状（store.py 拼装）
    "gateway_entry": (
        "## team-a\n- remote_addr: addr1\n- remote_token: tok1\n"
        "- local_prefix: /share/team-a\n- remote_prefix: /docs\n"
        "- mounted: 2026-01-01T00:00:00Z\n"
    ),
    "headings_blank_text": "# H\n\n## SYS\n- a: 1\n\n正文行 without colon\n- b: 2\n",
    "flags_and_empty_value": "- gateway_enabled\n- empty: \n- ok: true\n",
    "quoted_values": "- k: \"v v\"\n- j: 'x'\n",
    "colon_at_end": "- k:\n- j: v\n",
    "colon_in_value": "- url: http://a:8080/x\n- t: a: b: c\n- nospace: v\n",
    "doc_map_json": "- doc_map: {\"a\": 1, \"b\": 2}\n",
    "dup_last_wins": "- k: 1\n- k: 2\n",
    # 嵌套：inline_text 的容器屏障挡住子文本（否则父值会被污染）
    "nested_child": "- a: 1\n  - b: 2\n  - c: 3\n",
    "nested_deep": "- a: 1\n  - b: 2\n    - c: 3\n",
    "nested_parent_no_value": "- a\n  - b: 2\n",
    "dup_nested": "- k: 1\n  - k: 2\n",
    # 行首无空格（两边都忽略）、tab 缩进、引用块、表格后
    "no_space_after_dash": "-k: v\n",
    "tab_indent": "- a: 1\n\t- b: 2\n",
    "blockquote": "> quote\n- k: v\n",
    "table_after": "- k: v\n\n| a | b |\n|---|---|\n",
    # 全等：冒号后多空格、CRLF 行尾（值 strip）
    "extra_spaces_after_colon": "- j:    w  x  \n",
    "crlf": "- a: 1\r\n- b: 2\r\n",
}

EXPECTED = {
    "runtime_md": {"pid": "1234", "mode": "gateway", "host": "127.0.0.1",
                   "port": "8011", "status": "running", "gateway_enabled": "true"},
    "gateway_entry": {"remote_addr": "addr1", "remote_token": "tok1",
                      "local_prefix": "/share/team-a", "remote_prefix": "/docs",
                      "mounted": "2026-01-01T00:00:00Z"},
    "headings_blank_text": {"a": "1", "b": "2"},
    "flags_and_empty_value": {"empty": "", "ok": "true"},
    "quoted_values": {"k": "\"v v\"", "j": "'x'"},
    "colon_at_end": {"k": "", "j": "v"},
    "colon_in_value": {"url": "http://a:8080/x", "t": "a: b: c", "nospace": "v"},
    "doc_map_json": {"doc_map": "{\"a\": 1, \"b\": 2}"},
    "dup_last_wins": {"k": "2"},
    "nested_child": {"a": "1", "b": "2", "c": "3"},
    "nested_deep": {"a": "1", "b": "2", "c": "3"},
    "nested_parent_no_value": {"b": "2"},
    "dup_nested": {"k": "2"},
    "no_space_after_dash": {},
    "tab_indent": {"a": "1", "b": "2"},
    "blockquote": {"k": "v"},
    "table_after": {"k": "v"},
    "extra_spaces_after_colon": {"j": "w  x"},
    "crlf": {"a": "1", "b": "2"},
}


async def test_equal_cases_new_equals_oracle(runtime):
    """全等类：当前路径与 oracle 逐键全等。"""
    ws = runtime._workspace
    for name, text in EQUAL_CASES.items():
        old = oracle_kv(text)
        new = migrated_kv(ws, text)
        assert old == EXPECTED[name], f"{name}: oracle 与预期不符 {old!r}"
        assert new == EXPECTED[name], f"{name}: 新路径 {new!r} != 预期 {EXPECTED[name]!r}"


# Part 2: 差异类 —— 逐条钉住（含 canary）

DIVERGENT_CASES = {
    "D1_space_before_colon": "- m :  s\n",
    "D3_lazy_continuation": "- k: v\n  continuation line\n",
    "D4_empty_value_continuation": "- k:\n  continued\n",
    "D5_code_block_line": "```\n- k: v\n```\n",
    "D6_inline_code_value": "- k: `x`\n",
    "D7_emphasis_value": "- k: **x**\n",
}

EXPECTED_DIVERGENT = {
    # (oracle, 新)
    "D1_space_before_colon": ({"m": "s"}, {}),
    "D3_lazy_continuation": ({"k": "v"}, {"k": "v continuation line"}),
    "D4_empty_value_continuation": ({"k": ""}, {"k": "continued"}),
    "D5_code_block_line": ({"k": "v"}, {}),
    "D6_inline_code_value": ({"k": "`x`"}, {"k": "x"}),
    "D7_emphasis_value": ({"k": "**x**"}, {"k": "x"}),
}


async def test_divergence_whitelist_is_exactly_measured_set(runtime):
    """canary：差异集合必须恰好是实测的 8 类。

    helper（core/ast_utils.py）语义变化 ⇒ 这里先失败，提示同步更新本文件。
    """
    ws = runtime._workspace
    cases = {**EQUAL_CASES, **DIVERGENT_CASES}
    divergent = {name for name, text in cases.items()
                 if oracle_kv(text) != migrated_kv(ws, text)}
    assert divergent == set(DIVERGENT_CASES), (
        f"差异集合变了：多出 {divergent - set(DIVERGENT_CASES)}，"
        f"消失 {set(DIVERGENT_CASES) - divergent} —— 请同步更新本文件与迁移报告"
    )


@pytest.mark.parametrize("name", sorted(DIVERGENT_CASES))
async def test_each_divergence_pinned(runtime, name):
    """逐条差异：旧/新各自钉死（新行为为什么可接受见模块 docstring）。"""
    text = DIVERGENT_CASES[name]
    old_expect, new_expect = EXPECTED_DIVERGENT[name]
    assert oracle_kv(text) == old_expect, f"{name}: oracle 变了"
    new = migrated_kv(runtime._workspace, text)
    assert new == new_expect, f"{name}: 新路径 {new!r} != {new_expect!r}"


async def test_d5_code_block_is_an_improvement(runtime):
    """D5 单独写清方向：代码块里的 `- k: v` oracle 会**误读**，当前实现按 AST 正确忽略。"""
    text = "```\n- k: v\n```\n"
    assert oracle_kv(text) == {"k": "v"}, "旧实现在代码块内误读（这是 bug）"
    assert migrated_kv(runtime._workspace, text) == {}, "新实现按 AST 只认真 bullet"


# Part 3: gateway.md 内核（_parse_gateway_chunks）三档语义


async def test_chunk_kernel_new_path_equals_oracle_on_flat(runtime):
    """内核（传 workspace）：平铺 chunk 与 oracle 全等，flag 行仍为 True。"""
    chunk = ("## team-a\n- remote_addr: addr1\n- remote_token: tok1\n"
             "- local_prefix: /share/team-a\n- keep_alive\n")
    entries = GatewayStore._parse_gateway_chunks([chunk], workspace=runtime._workspace)
    assert len(entries) == 1
    e = entries[0]
    assert e["_title"] == "team-a"
    body = {k: v for k, v in e.items() if k != "_title"}
    assert body == {"remote_addr": "addr1", "remote_token": "tok1",
                    "local_prefix": "/share/team-a", "keep_alive": True}
    assert {k: v for k, v in body.items() if v is not True} == \
        oracle_kv("\n".join(chunk.split("\n")[1:]))


async def test_chunk_kernel_nested_is_equal(runtime):
    """嵌套 chunk：父字段值不被子项污染（inline_text 容器屏障），新旧一致。"""
    chunk = "## team-a\n- remote_addr: addr1\n  - extra: x\n"
    new_entries = GatewayStore._parse_gateway_chunks([chunk], workspace=runtime._workspace)
    old_entries = GatewayStore._parse_gateway_chunks([chunk])   # 无句柄 → 旧 parse_kv_lines
    assert old_entries[0] == new_entries[0], (old_entries[0], new_entries[0])
    assert new_entries[0]["remote_addr"] == "addr1"
    assert new_entries[0]["extra"] == "x"


async def test_chunk_kernel_legacy_branch_stays_old(runtime):
    """不传引擎句柄 ⇒ 走 `parse_kv_lines`（store-only 装配），语义与之前一致。

    `_parse` 用 `ProjectStore("/mem")`，其现有测试（tests/test_gateway_store.py、
    tests/test_iroh.py 的 `GatewayStore._parse`）依赖该结果。store-only 档（离线装配）
    与 workspace 档同源。
    """
    chunk = "## mount\n- name: proj\n- rule: rw:/a/*\n- flag_line\n"
    legacy = GatewayStore._parse_gateway_chunks([chunk])
    new = GatewayStore._parse_gateway_chunks([chunk], workspace=runtime._workspace)
    only_store = GatewayStore._parse_gateway_chunks([chunk], store=runtime.store)
    assert legacy == new == only_store
    # 差异样例上三档的分岔：无句柄 = parse_kv_lines
    nested = "## mount\n- name: proj\n- m :  s\n"
    assert GatewayStore._parse_gateway_chunks([nested])[0].get("m") == "s"
    assert "m" not in GatewayStore._parse_gateway_chunks([nested], workspace=runtime._workspace)[0]


# Part 4: runtime.md 读写闭环（state.py 的真实调用面）


async def test_runtime_md_roundtrip_matches_oracle(runtime, wt_uid):
    """写 → 读闭环：runtime_md_read 与 oracle 在平铺字段上全等。"""
    fields = {"pid": "4242", "mode": "gateway", "host": "127.0.0.1", "port": "8011",
              "root": "/tmp/ws", "status": "running", "gateway_enabled": "true"}
    runtime_md_write(runtime._workspace, fields)
    raw = runtime.store.get_text(wt_uid, RUNTIME_MD)
    assert raw.startswith("# RUNTIME")
    assert oracle_kv(raw) == fields, "写入器与 oracle 口径应一致（平铺）"
    assert runtime_md_read(runtime._workspace) == fields
    assert runtime_md_read(runtime._workspace) == oracle_kv(raw)


async def test_runtime_md_missing_file_returns_empty(runtime):
    """文件不存在 ⇒ 空表（旧契约保持）。"""
    assert runtime_md_read(runtime._workspace) == {}


async def test_runtime_md_read_rejects_bare_store(runtime):
    """入参契约：裸 store 立刻 AttributeError，不静默返回空表。

    为什么钉这条：若把「传错对象」吞成 {}，`_shutdown_once` 里
    `fields.get('gateway_enabled') == 'true'` 会静默判否、跳过 detach_gateway。
    """
    with pytest.raises(AttributeError):
        runtime_md_read(runtime.store)


# Part 5: 活路径闭环 —— _read_gateway_entries 确实走新路径


async def test_read_gateway_entries_flat_matches_oracle(runtime, wt_uid):
    """活路径 `_read_gateway_entries`（传 workspace+store）在平铺 gateway.md 上等于 oracle。"""
    from mdio_py.core.selector_builder import SelectorBuilder
    body = "- remote_addr: addr1\n- remote_token: tok1\n- local_prefix: /share/team-a\n"
    runtime._workspace.write_locator(
        wt_uid, "/.mds/gateway.md", SelectorBuilder().build(), "+>",
        f"## team-a\n{body}", auto_scaffold=True, god=True, suppress_hooks=True,
    )
    gs = GatewayStore(store=runtime.store, workspace=runtime._workspace, iroh_endpoint=None)
    entries = gs._read_gateway_entries(wt_uid)
    assert entries, "gateway.md 应有条目"
    titles = [e["_title"] for e in entries]
    assert "team-a" in titles, f"实际标题：{titles}"
    e = next(e for e in entries if e["_title"] == "team-a")
    assert {k: v for k, v in e.items() if k != "_title"} == oracle_kv(body)
