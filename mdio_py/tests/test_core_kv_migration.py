"""core/bootstrap 与 runtime/worktree_paths 的 kv 取数对拍。

两个 oracle（本文件中按目标语义冻结，不随源实现漂移）：

  · `oracle_kv_lines` = `core.text_utils.parse_kv_lines` 的语义复刻
  · `oracle_coords`   = `runtime.worktree_paths` 的 `## ` 分节 + `- uid:`/`- path:` 行解析

对拍口径：

① `core.bootstrap._hook_entry_kv`：结构走 `workspace.to_json_text` → `bullets_to_kv`；
   `cb`/`target` 取值保留 `parse_kv_lines` 的**原文**（守卫理由见该函数 docstring）。
   Part 1 把「全等 / 守卫 / 保留」三档逐条钉住。

② `runtime.worktree_paths._local_scope_roots`：纯函数，读盘上 `<根>/wt/.mds/worktrees.md`
   并用 `line.split(":", 1)` 解析；可用 `ast_utils.sections(env)` 按 `## ` 节取数
   （`bullets_to_kv` 整档扁平会把 N 节塌成最后一节；`sections()` 给出
   `{节标题: {键: 文本}}`，消费点要的 uid→path 能从中组出）。
   注意：标题的正文是**兄弟**节点，且 `## hook_*` 的标题 `t` 可能是行内节点数组；
   bootstrap 读的是 `read_locator(...any_child())` **已切好的条目体**（无标题），
   所以它必须用 `bullets_to_kv(body)`、不能用 `sections()`（无标题时恒空）。

分类口径（三档）：
  · 全等类 `EQUAL_BODIES`：当前实现与 oracle 逐键一致；
  · 守卫类 `GUARDED_BODIES`：裸 helper 会改写值，`_hook_entry_kv` 靠 `cb`/`target` 原文守卫拉回全等；
  · 保留类 `KEPT_DIVERGENT_BODIES`：裸 helper 与当前实现**都**与 oracle 有差异——这些形状
    本写入器（`interpreter` 恒为单行 `- k: v`）不会产出，仅记录 helper 语义方向。

canary 两条：裸 helper 的差异集合必须**恰好**等于实测集合；当前实现在「全等 + 守卫」两档上
必须与 oracle 全等。helper 语义或守卫再变时它们先红。
"""

import inspect
import json

import pytest

from mdio_py.core.ast_utils import bullets_to_kv, sections
from mdio_py.core.bootstrap import _HOOK_EXACT_TEXT_KEYS, _hook_entry_kv
from mdio_py.core.text_utils import parse_kv_lines
from mdio_py.runtime import worktree_paths


# 两个 oracle + 新路径取数器


def oracle_kv_lines(text: str) -> dict:
    """`parse_kv_lines` 的语义复刻：本文件内的冻结参考实现。"""
    out: dict = {}
    for line in str(text).split("\n"):
        line = line.strip()
        if not line.startswith("- ") or ":" not in line[2:]:
            continue
        k, v = line[2:].split(":", 1)
        out[k.strip()] = v.strip()
    return out


def oracle_coords(text: str) -> dict:
    """`_local_scope_roots` 文本解析的复刻。

    只复刻**文本解析**部分（`## ` 分节 + `- uid:`/`- path:` 成对成条）；递归下钻是盘上行为，
    由 `test_live_local_scope_roots_*` 用真实目录覆盖。
    """
    out: dict = {}
    uid = path = None
    for line in str(text).split("\n"):
        if line.startswith("## "):
            if uid and path:
                out[uid] = path
            uid = path = None
            continue
        if line.startswith("- uid:"):
            uid = line.split(":", 1)[1].strip()
        elif line.startswith("- path:"):
            path = line.split(":", 1)[1].strip()
    if uid and path:
        out[uid] = path
    return out


def new_kv(workspace, text: str) -> dict:
    """**裸** helper：`workspace.to_json_text` → `bullets_to_kv`（不含原文守卫）。"""
    return bullets_to_kv(json.loads(workspace.to_json_text(text)))


def _first_entry_body(hook_md: str) -> str:
    """hook.md 全文 → 第一个 `## hook_*` 条目的体（标题行以下、下一个 `## ` 之前）。"""
    sec = str(hook_md).split("## hook_", 1)[1]
    return "\n".join(sec.split("\n")[1:]).split("\n## ")[0]


# Part 1 · bootstrap hook.md 条目体

# 全等类：三边（oracle / 裸 helper / 迁移点）必须逐键一致。
EQUAL_BODIES = {
    # 写入器真实形状（interpreter 恒为单行 - k: v）
    "flat_real": (
        "- target: /config.md::CTX.SYS._b:host\n"
        "- cb: '\"Host 变了!\" | +> /log.md::LOGS'\n"
        "- is_always: true\n"
        "- wt_uid: genesis\n"
        "- ctx_active_doc: null\n"
        "- ctx_cwd: /\n"
    ),
    "heading_blank_text": "# HOOKS\n\n## hook_ab12\n- a: 1\n\n正文行 without colon\n- b: 2\n",
    "empty_and_flag": "- is_always: true\n- ctx_active_doc: \n- note\n",
    "colon_in_value": "- target: /a.md::S._b:k\n- cb: '\"x\" | +> /b.md::L'\n",
    "crlf": "- a: 1\r\n- b: 2\r\n",
    "extra_spaces_after_colon": "- ctx_cwd:    /x  \n",
    "dup_last_wins": "- wt_uid: a\n- wt_uid: b\n",
    "nested_child": "- target: /a.md::S\n  - extra: x\n",
    "no_space_after_dash": "-k: v\n- ok: 1\n",
    "tab_indent": "- a: 1\n\t- b: 2\n",
}

EXPECTED_EQUAL = {
    "flat_real": {"target": "/config.md::CTX.SYS._b:host",
                  "cb": "'\"Host 变了!\" | +> /log.md::LOGS'",
                  "is_always": "true", "wt_uid": "genesis",
                  "ctx_active_doc": "null", "ctx_cwd": "/"},
    "heading_blank_text": {"a": "1", "b": "2"},
    "empty_and_flag": {"is_always": "true", "ctx_active_doc": ""},
    "colon_in_value": {"target": "/a.md::S._b:k", "cb": "'\"x\" | +> /b.md::L'"},
    "crlf": {"a": "1", "b": "2"},
    "extra_spaces_after_colon": {"ctx_cwd": "/x"},
    "dup_last_wins": {"wt_uid": "b"},
    "nested_child": {"target": "/a.md::S", "extra": "x"},
    "no_space_after_dash": {"ok": "1"},
    "tab_indent": {"a": "1", "b": "2"},
}

# 守卫类：裸 helper 会改写 `cb`/`target` 的**值**——行内标记被规范化掉，或续行被折进值；
# 当前实现用原文守卫把这两个键拉回 oracle。方向：守卫档 = 原文，裸 helper = 规范化/折叠后的值。
GUARDED_BODIES = {
    "cb_inline_code": "- cb: 'ls `date`.md'\n- is_always: true\n",
    "cb_link": "- cb: 'cat [a](b).md'\n",
    "cb_emphasis": "- cb: 'run **x** now'\n",
    "target_inline_code": "- target: /a`b`.md::S\n",
    "target_link": "- target: /a[x](y).md::S\n",
    "target_lazy_continuation": "- target: /a.md::S\n  continuation line\n",
}

EXPECTED_GUARDED = {
    "cb_inline_code": ({"cb": "'ls `date`.md'", "is_always": "true"},
                       {"cb": "'ls date.md'", "is_always": "true"}),
    "cb_link": ({"cb": "'cat [a](b).md'"}, {"cb": "'cat a.md'"}),
    "cb_emphasis": ({"cb": "'run **x** now'"}, {"cb": "'run x now'"}),
    "target_inline_code": ({"target": "/a`b`.md::S"}, {"target": "/ab.md::S"}),
    "target_link": ({"target": "/a[x](y).md::S"}, {"target": "/ax.md::S"}),
    "target_lazy_continuation": ({"target": "/a.md::S"}, {"target": "/a.md::S continuation line"}),
}

# 保留类：**结构**差异且落在守卫之外（键不是 cb/target）。写入器（单行 `- k: v`，冒号前无空格）
# 不会产出这些形状，故不改写它们的行为，只把方向钉死；hook.md 允许手工续行/空格时这里先红。
KEPT_DIVERGENT_BODIES = {
    "space_before_colon": "- m :  s\n",
    "empty_value_continuation": "- is_always:\n  true\n",
}

EXPECTED_KEPT_DIVERGENT = {
    # (oracle, 裸 helper == 迁移点)
    "space_before_colon": ({"m": "s"}, {}),
    "empty_value_continuation": ({"is_always": ""}, {"is_always": "true"}),
}

ALL_BODIES = {**EQUAL_BODIES, **GUARDED_BODIES, **KEPT_DIVERGENT_BODIES}


async def test_equal_bodies_three_ways_agree(runtime):
    """全等类：oracle == 裸 helper == 当前实现。"""
    for name, body in EQUAL_BODIES.items():
        old = oracle_kv_lines(body)
        assert old == EXPECTED_EQUAL[name], f"{name}: oracle 与预期不符 {old!r}"
        assert new_kv(runtime._workspace, body) == old, f"{name}: 裸新路径 {new_kv(runtime._workspace, body)!r}"
        assert _hook_entry_kv(runtime._workspace, body) == old, f"{name}: 迁移点与 oracle 不一致"


async def test_guarded_bodies_pin_divergence_and_guard(runtime):
    """守卫类：裸 helper 会改写值（实测方向），当前实现把 `cb`/`target` 拉回原文。"""
    for name, body in GUARDED_BODIES.items():
        old_expect, raw_expect = EXPECTED_GUARDED[name]
        assert oracle_kv_lines(body) == old_expect, f"{name}: oracle 变了"
        assert new_kv(runtime._workspace, body) == raw_expect, f"{name}: 裸 helper 的改写方向变了"
        assert _hook_entry_kv(runtime._workspace, body) == old_expect, (
            f"{name}: 守卫失效——重启重放会注册出被改写的回调/选择器"
        )


async def test_guard_key_list_is_exactly_measured(runtime):
    """canary：守卫键集合必须恰好是实测需要原文的 `cb`/`target`（多/少都先红）。"""
    assert tuple(_HOOK_EXACT_TEXT_KEYS) == ("cb", "target"), _HOOK_EXACT_TEXT_KEYS


async def test_kept_divergences_are_pinned(runtime):
    """保留类：裸 helper 与当前实现同向偏（值折叠/键丢失），逐条钉住方向。"""
    for name, body in KEPT_DIVERGENT_BODIES.items():
        old_expect, new_expect = EXPECTED_KEPT_DIVERGENT[name]
        assert oracle_kv_lines(body) == old_expect, f"{name}: oracle 变了"
        raw = new_kv(runtime._workspace, body)
        assert raw == new_expect, f"{name}: 裸新路径 {raw!r} != {new_expect!r}"
        assert _hook_entry_kv(runtime._workspace, body) == new_expect, (
            f"{name}: 迁移点应与裸新路径同向（守卫不管结构差异）"
        )


async def test_canary_divergence_set_is_exactly_measured(runtime):
    """canary：裸 helper 相对 oracle 的差异集合必须恰好是实测的 8 类。"""
    divergent = {name for name, body in ALL_BODIES.items()
                 if oracle_kv_lines(body) != new_kv(runtime._workspace, body)}
    measured = set(GUARDED_BODIES) | set(KEPT_DIVERGENT_BODIES)
    assert divergent == measured, (
        f"差异集合变了：多出 {divergent - measured}，消失 {measured - divergent}"
        " —— 请同步更新本文件与迁移报告"
    )


async def test_canary_migrated_point_equals_oracle_where_required(runtime):
    """canary：当前实现必须在「全等 + 守卫」两档上与 oracle 全等（保留档除外）。"""
    for name in {**EQUAL_BODIES, **GUARDED_BODIES}:
        body = ALL_BODIES[name]
        assert _hook_entry_kv(runtime._workspace, body) == oracle_kv_lines(body), name


async def test_live_hook_md_body_migrated_equals_oracle(runtime):
    """活路径：写入器产出的**真实**条目体上，当前实现与 oracle 全等，且重放确实登记成功。"""
    ws = runtime._workspace
    await runtime.exec("mk -f /a.md", wt_uid="genesis")
    await runtime.exec('"# A\n\n## S\n\n- k: 1" | /> /a.md -f -god', wt_uid="genesis")
    r = await runtime.exec(
        "hook -target /a.md::S._b:k -cb '\"CB\" | /> /a.md::S._b:k' -is_always",
        wt_uid="genesis")
    assert "mdio_error" not in str(r), f"注册报错: {r}"

    body = _first_entry_body(runtime.store.get_text("genesis", "/.mds/hook.md"))
    old = oracle_kv_lines(body)
    assert old["is_always"] == "true" and old["wt_uid"] == "genesis", old
    assert old["cb"].startswith("'") and old["cb"].endswith("'"), old
    assert _hook_entry_kv(ws, body) == old, f"真实条目体新旧不一致：{body!r}"

    # 重放（与 test_hook_wt_context 的对应用例同法）：重放后仍必须登记 1 条
    from mdio_py.core.bootstrap import _replay_hooks_from_md
    runtime.store.clear_worktree_hooks("genesis")
    _replay_hooks_from_md(ws, "genesis", "/.mds/hook.md")
    assert runtime.store.hook_count_for_worktree("genesis") == 1, "重放未登记（迁移打断活路径）"


async def test_body_is_headingless_so_sections_is_empty_here(runtime):
    """canary：bootstrap 读的是**已切好的条目体**（无标题）。

    标题的正文是它的**兄弟**节点，所以「读某一节」要用 `sections()`；但本迁移点的输入是
    `read_locator(...any_child())` 切出来的条目体（标题行已被 `lines[0]` 吃掉）——
    该文本上没有标题 ⇒ `sections()` 恒空，这里必须用 `bullets_to_kv(body)`。
    """
    body = EQUAL_BODIES["flat_real"]
    env = json.loads(runtime._workspace.to_json_text(body))
    assert sections(env) == {}, "无标题文本上 sections() 应为空——语义变了就要重评本迁移点"
    assert bullets_to_kv(env) == oracle_kv_lines(body)
    assert _hook_entry_kv(runtime._workspace, body) == oracle_kv_lines(body)


def test_oracle_replica_still_matches_shipped_parse_kv_lines():
    """oracle 忠实性：复刻的 oracle 与**在用的** `parse_kv_lines` 在同一批样例上全等。"""
    for name, body in ALL_BODIES.items():
        assert oracle_kv_lines(body) == parse_kv_lines(body), name


# Part 2 · runtime/worktree_paths 文本解析

# 真实 worktrees.md 形状（worktrees/genesis/wt/.mds/worktrees.md 实测）：节标题 = wt **name**，
# 节体里才是 `- uid:` / `- path:`（外加 external_root / doc_subdir / wal_rel）。
WORKTREES_MD = (
    "# WORKTREES\n\n"
    "## exp_proj\n- uid: c5BJBF3A\n- path: /p/a\n- external_root: /p/a\n- doc_subdir: wt\n- wal_rel: .wal\n\n"
    "## exp_proj2\n- uid: c5HmAQHh\n- path: /p/b\n- external_root: /p/b\n- doc_subdir: wt\n- wal_rel: .wal\n"
)
COORDS_EXPECT = {"c5BJBF3A": "/p/a", "c5HmAQHh": "/p/b"}
FLAT_EXPECT = {"uid": "c5HmAQHh", "path": "/p/b", "external_root": "/p/b",
               "doc_subdir": "wt", "wal_rel": ".wal"}


def test_coords_shape_is_bullet_kv_lineform():
    """形状实测：`- uid:` / `- path:` 就是 `- k: v` 行形态（所以形状**不是**阻塞点）。"""
    assert oracle_coords("- uid: c5BJBF3A\n- path: /p/a\n") == {"c5BJBF3A": "/p/a"}


async def test_flat_helper_collapses_sections_but_sections_helper_works(runtime):
    """形状两条实测：扁平 `bullets_to_kv` 塌成最后一节；`sections()` 按节取数可用。"""
    ws = runtime._workspace
    assert oracle_coords(WORKTREES_MD) == COORDS_EXPECT
    assert new_kv(ws, WORKTREES_MD) == FLAT_EXPECT, (
        f"扁平 helper 应只留最后一节（丢 exp_proj）：{new_kv(ws, WORKTREES_MD)!r}"
    )
    secs = sections(json.loads(ws.to_json_text(WORKTREES_MD)))
    assert secs["exp_proj"]["uid"] == "c5BJBF3A" and secs["exp_proj2"]["uid"] == "c5HmAQHh"
    assert secs["WORKTREES"] == {}, "一级标题节下无 bullet（正文是兄弟块）"
    # 消费点要的 uid→path：sections() 组得出来，纯函数路径没有引擎句柄
    assert {v["uid"]: v["path"] for v in secs.values() if "uid" in v} == COORDS_EXPECT


def test_signature_canary_local_scope_roots_has_no_engine_handle():
    """canary：`_local_scope_roots` 是纯函数（无 store/workspace）——加了句柄这里先红，提示可迁。"""
    params = list(inspect.signature(worktree_paths._local_scope_roots).parameters)
    assert params == ["start_project_root"], (
        f"签名变了（{params}）：若已拿到引擎句柄，可把 :132-134 的行解析迁到 "
        "tojson + 按 `## ` 分节的 AST 取数；若仍无句柄，保持旧路径"
    )


def test_live_local_scope_roots_reads_multi_section_from_disk(tmp_path):
    """活路径：盘上两节 worktrees.md → 两条 uid→path（扁平 helper 做不到的那半）。"""
    docs = tmp_path / "proj" / "wt" / ".mds"
    docs.mkdir(parents=True)
    (docs / "worktrees.md").write_text(WORKTREES_MD)
    got = worktree_paths._local_scope_roots(str(tmp_path / "proj"))
    assert got == COORDS_EXPECT, got


@pytest.mark.parametrize("name", sorted(KEPT_DIVERGENT_BODIES))
def test_kept_divergence_cases_have_explicit_direction(name):
    """保留类也做一次纯文本方向断言（不依赖引擎），方便无 runtime 时定位。"""
    body = KEPT_DIVERGENT_BODIES[name]
    old_expect, _ = EXPECTED_KEPT_DIVERGENT[name]
    assert oracle_kv_lines(body) == old_expect
