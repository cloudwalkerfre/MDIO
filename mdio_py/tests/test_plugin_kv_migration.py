"""插件侧 KV 对拍：手工 md 解析（oracle） vs tojson + bullets_to_kv。

覆盖 bash / cron 插件内的四个迁移点：

  1. `bash/state.py::_parse_kv_chunks`（bash.md CONFIG / CONTEXTS 字段；`load_global_config`
     与 `load_context_config` 调用）——**带 `unquote_value=_unquote` 值后处理**；
  2. `bash/state.py::extract_strip_value`（PROC / EXEC 块里的 `kind`/`status`/`exit`；
     `service.py::list_state` 调用）——原先是一行正则；
  3. `cron/cron.py::_parse_cron_chunks`（cron.md 每个 `## job_id` 段 → cdef 字典）；
  4. `cron/cron.py::_read_proc_state`（proc.md 的 `PROCESS_TABLE.CRON.<job>` 段）。

三档口径（配方 §4 样板）：有 workspace 句柄走引擎 `workspace.to_json_text` → `bullets_to_kv`；
没有句柄（或引擎/JSON 异常）保留旧解析。测试里 `workspace=None` 那一档就是 oracle 的活体。

**新旧不等价**，本文件把实测差异逐类钉成断言（不假装等价），每个迁移点一条 canary
（差异集合必须恰好等于实测集合；helper 语义再变时它先红）。

一、全等类（迁移必须保持）
  平铺 `- k: v`（bash CONFIG 默认形状、cron cdef 真实形状）、JSON 值、值里带冒号、空值、
  无冒号 bullet、空行/标题/正文行、重复键（后者胜）、嵌套子项、tab/缩进、CRLF、冒号后多空格、
  **三引号/引号值**（新路径照样施加 `_unquote`，见 Part 1 的 post-processing 用例）。

二、差异类（逐条断言 + 机制）
  D1 `- m :  s`（冒号前空格）  → 旧 {'m': 's'}，新 {}（引擎给空 tag ⇒ 整键丢失）
  D3 懒续行                    → 旧只取本级行，新把续行折进值（软换行变空格）
  D5 围栏代码块内的 `- k: v`   → 旧**误读**成键值，新正确忽略（新实现的改进）
  D6 值里的行内代码            → 旧保留反引号原文 `x`，新去反引号留文本 x
  D7 值里的粗体/强调           → 旧保留标记原文 `**x**`，新去标记只留文本 x
  U1 顶层重复键 + `extract_strip_value` → 旧取**首个**命中，新取**末个**（与 kv 的后者胜一致；
     这是行正则与 kv 语义的唯一实质分岔，被 `list_state` 观察）

为什么调用方可接受：这些 md 都由插件自己拼装（`BashState.config_lines` / `exec_record_fields` /
`write_exec_fields` / cron cdef 写入器），都是**单行平铺 `- k: v`**（冒号后单空格、无行内标记、
LF 行尾、键唯一）⇒ 落在「全等类」。行内标记类的规范化（D5/D6/D7）方向统一为「去标记、留文本」。

三、未迁移的两处（判断结论：不在解析 md，只记录不动）
  - `bash/backends/local.py::_exec_capture_sync` 的 `re.compile(rf"{marker}:(\d+)")`：它是
    pexpect 对**子进程输出**的 `child.expect` 匹配（marker 是随机串 `__MDIO_DONE_<hex>__`），
    不是 md 解析，迁到 AST 无意义；
  - `cron/cron.py` 的 `_DURATION_RE` / `_COMPOUND_RE`：解析 `30s` / `1h30m` 这类时长字符串，
    与 md 无关。
  两处的现状由 `test_non_md_regexes_left_untouched` 钉住（谁把它们改成 md 解析，这里红）。
"""

import json
import re

import pytest

from mdio_py.core.ast_utils import bullets_to_kv, walk
from mdio_py.core.interpreter import RuntimeContext
from mdio_py.core.selector_builder import SelectorBuilder, drill
from mdio_py.core.text_utils import parse_kv_lines
from mdio_py.plugins.bash.state import BashState, _unquote
from mdio_py.plugins.cron.cron import CRONPlugin



# oracle：独立语义复刻（不依赖被测代码）



def oracle_kv(text: str, unquote=None) -> dict:
    """旧 `parse_kv_lines` 语义复刻（core/text_utils.py:94-107）。

    只认 `- ` 开头且 `line[2:]` 含冒号的行；首个冒号切分；键值都 strip；
    值后处理在 strip 之后施加；同名键后者覆盖前者。
    """
    out: dict = {}
    for line in str(text).split("\n"):
        line = line.strip()
        if not line.startswith("- ") or ":" not in line[2:]:
            continue
        k, v = line[2:].split(":", 1)
        v = v.strip()
        out[k.strip()] = unquote(v) if unquote else v
    return out


def oracle_extract(text: str, key: str) -> str:
    """旧 `extract_strip_value` 语义复刻：`(?m)^- key:\\s*(.*)$` 的**首个**命中。"""
    match = re.search(rf"(?m)^- {re.escape(key)}:\s*(.*)$", text)
    return match.group(1).strip() if match else ""


# ── 迁移后的入口（同一函数的两档：给句柄 = 新路径，不给 = 旧路径）──


def bash_kv(ws, text: str) -> dict:
    """`_parse_kv_chunks` 新路径（workspace 句柄）。"""
    return BashState(plugin=None)._parse_kv_chunks([text], workspace=ws)


def bash_kv_no_handle(text: str) -> dict:
    """`_parse_kv_chunks` 旧档（无句柄）——应当逐键等于 oracle_kv(text, _unquote)。"""
    return BashState(plugin=None)._parse_kv_chunks([text])


def engine_kv(ws, text: str) -> dict:
    """裸引擎（不经 `_unquote`），用来证明值后处理是新路径额外施加的。"""
    return bullets_to_kv(json.loads(ws.to_json_text(text)))


def cron_defs(ws, chunks: list) -> dict:
    """`_parse_cron_chunks` 新路径。"""
    return CRONPlugin._parse_cron_chunks(chunks, workspace=ws)


def cron_defs_no_handle(chunks: list) -> dict:
    """`_parse_cron_chunks` 旧档（无句柄）。"""
    return CRONPlugin._parse_cron_chunks(chunks)



# Part 1: bash CONFIG / CONTEXTS（_parse_kv_chunks + _unquote）


BASH_EQUAL_CASES = {
    # BashState.ensure_docs 写出的真实 CONFIG 形状
    "config_default": (
        "# CONFIG\n- backend: local\n- shell: bash\n- timeout: 30\n"
        "- session_engine: pexpect\n- stream_chunk: 4096\n- output_limit: 65536\n\n"
    ),
    # §3 的关键：值后处理必须照旧施加（三引号/引号/单引号）
    "quoted_values": "- shell: \"\"\"bash\"\"\"\n- cwd: \"/tmp/x y\"\n- j: 'z'\n",
    "env_json": '- env: {"BASH_TEST_TOKEN": "OK", "P": "/tmp/a"}\n',
    "colon_in_value": "- cwd: /tmp/a:b\n- note: a: b: c\n",
    "empty_value": "- cwd: \n- shell: bash\n",
    "dup_last_wins": "- shell: sh\n- shell: bash\n",
    "nested_child": "- shell: bash\n  - child: 2\n",
    "heading_blank_body": "# CONFIG\n\n- shell: bash\n\n正文 without colon\n",
    "no_colon_bullet": "- flag_line\n- shell: bash\n",
    "crlf": "- backend: local\r\n- shell: bash\r\n",
    "extra_spaces_after_colon": "- shell:   bash  \n",
}

BASH_EXPECTED = {
    "config_default": {"backend": "local", "shell": "bash", "timeout": "30",
                       "session_engine": "pexpect", "stream_chunk": "4096",
                       "output_limit": "65536"},
    "quoted_values": {"shell": "bash", "cwd": "/tmp/x y", "j": "z"},
    "env_json": {"env": '{"BASH_TEST_TOKEN": "OK", "P": "/tmp/a"}'},
    "colon_in_value": {"cwd": "/tmp/a:b", "note": "a: b: c"},
    "empty_value": {"cwd": "", "shell": "bash"},
    "dup_last_wins": {"shell": "bash"},
    "nested_child": {"shell": "bash", "child": "2"},
    "heading_blank_body": {"shell": "bash"},
    "no_colon_bullet": {"shell": "bash"},
    "crlf": {"backend": "local", "shell": "bash"},
    "extra_spaces_after_colon": {"shell": "bash"},
}

BASH_DIVERGENT_CASES = {
    "D1_space_before_colon": "- m :  s\n",
    "D3_lazy_continuation": "- cwd: /tmp\n  lazy line\n",
    "D5_code_block_line": "```\n- shell: sh\n```\n",
    "D6_inline_code_value": "- shell: `bash`\n",
    "D7_emphasis_value": "- shell: **bash**\n",
}

BASH_EXPECTED_DIVERGENT = {
    # (oracle, 新)
    "D1_space_before_colon": ({"m": "s"}, {}),
    "D3_lazy_continuation": ({"cwd": "/tmp"}, {"cwd": "/tmp lazy line"}),
    "D5_code_block_line": ({"shell": "sh"}, {}),
    "D6_inline_code_value": ({"shell": "`bash`"}, {"shell": "bash"}),
    "D7_emphasis_value": ({"shell": "**bash**"}, {"shell": "bash"}),
}


async def test_bash_kv_equal_cases(runtime):
    """全等类：oracle、无句柄旧档、新路径三者逐键一致。"""
    ws = runtime._workspace
    for name, text in BASH_EQUAL_CASES.items():
        old = oracle_kv(text, unquote=_unquote)
        assert old == BASH_EXPECTED[name], f"{name}: oracle 与预期不符 {old!r}"
        no_handle = bash_kv_no_handle(text)
        assert no_handle == BASH_EXPECTED[name], f"{name}: 旧档 {no_handle!r} != 预期"
        new = bash_kv(ws, text)
        assert new == BASH_EXPECTED[name], f"{name}: 新路径 {new!r} != 预期 {BASH_EXPECTED[name]!r}"


async def test_bash_kv_divergence_whitelist_is_exactly_measured_set(runtime):
    """canary：bash CONFIG 的差异集合必须恰好是实测的 5 类。"""
    ws = runtime._workspace
    cases = {**BASH_EQUAL_CASES, **BASH_DIVERGENT_CASES}
    divergent = {name for name, text in cases.items()
                 if oracle_kv(text, unquote=_unquote) != bash_kv(ws, text)}
    assert divergent == set(BASH_DIVERGENT_CASES), (
        f"差异集合变了：多出 {divergent - set(BASH_DIVERGENT_CASES)}，"
        f"消失 {set(BASH_DIVERGENT_CASES) - divergent} —— 请同步更新本文件与迁移报告"
    )


@pytest.mark.parametrize("name", sorted(BASH_DIVERGENT_CASES))
async def test_bash_kv_each_divergence_pinned(runtime, name):
    """逐条差异：旧/新各自钉死（方向与理由见模块 docstring）。"""
    text = BASH_DIVERGENT_CASES[name]
    old_expect, new_expect = BASH_EXPECTED_DIVERGENT[name]
    assert oracle_kv(text, unquote=_unquote) == old_expect, f"{name}: oracle 变了"
    assert bash_kv(runtime._workspace, text) == new_expect, f"{name}: 新路径变了"


async def test_value_post_processing_unquote_still_applied(runtime):
    """§3 硬要求：新路径取到 kv 后**照旧**施加 `_unquote`。

    证据链：裸引擎给带引号原文 → 迁移入口给剥离后的值 → 与旧档/oracle 逐键一致。
    """
    ws = runtime._workspace
    text = "- shell: \"\"\"bash\"\"\"\n- cwd: \"/tmp/x y\"\n"
    raw = engine_kv(ws, text)
    assert raw == {"shell": '"""bash"""', "cwd": '"/tmp/x y"'}, raw   # 引擎不做 unquote
    new = bash_kv(ws, text)
    assert new == {"shell": "bash", "cwd": "/tmp/x y"}, new           # 迁移入口照旧施加
    assert new == bash_kv_no_handle(text) == oracle_kv(text, unquote=_unquote)


async def test_bash_kv_falls_back_when_engine_unavailable():
    """句柄不可用（对象没有 to_json_text）⇒ 静默降级旧解析，绝不抛。"""
    st = BashState(plugin=None)
    text = "- shell: sh\n- timeout: 30\n"
    assert st._parse_kv_chunks([text], workspace=object()) == oracle_kv(text, unquote=_unquote)
    assert st._parse_kv_chunks([text], workspace=ws_raises()) == oracle_kv(text, unquote=_unquote)


def ws_raises():
    """假句柄：to_json_text 直接抛（引擎异常路径）。"""

    class _Boom:
        def to_json_text(self, text):
            raise RuntimeError("engine boom")

    return _Boom()


async def test_heading_topology_guard_never_pass_a_section_node(runtime):
    """守卫（Lead 实测的 CF 拓扑）：heading 的正文是它的**兄弟**节点。

    `bullets_to_kv(节节点)` 会静默得 `{}`；本迁移从不这么用——输入永远是「一个节的原始
    文本」（含标题行），引擎投影后对**整档** walk，bullet 一定能取到（非空断言在此）。
    谁把这里改成「先取节节点再喂 bullets_to_kv」，这条先红。
    """
    ws = runtime._workspace
    text = "# CONFIG\n- shell: bash\n- timeout: 30\n"
    env = json.loads(ws.to_json_text(text))
    heading = next(n for n in walk(env) if n.get("nt") == "h")
    assert bullets_to_kv(heading) == {}, "如果这里不再为 {}，说明拓扑变了，请复核本文件"
    assert engine_kv(ws, text) == {"shell": "bash", "timeout": "30"}
    assert bash_kv(ws, text) == {"shell": "bash", "timeout": "30"}
    # 标题文本可能是行内节点数组（`## hook_2` 下划线被当强调）——名称来自原始文本行，
    # 不经 AST：cron 的 job 名提取用 lines[0]，故不受影响（末尾用例覆盖真实 job 名）。
    assert cron_defs(ws, ["## job_2\n- enabled: true\n"]) == {"job_2": {"enabled": "true"}}


async def test_load_global_config_uses_engine_path(runtime, wt_uid, monkeypatch):
    """活路径：`load_global_config`（`service.py` 真实调用面）走引擎，不再碰旧行解析。"""
    from mdio_py.plugins.bash import state as state_mod

    ws = runtime._workspace
    st = BashState(plugin=None)
    await st.ensure_docs(ws, wt_uid)
    ctx = RuntimeContext(wt_uid=wt_uid)

    default = await st.load_global_config(ws, ctx)
    assert (default.backend, default.shell, default.timeout) == ("local", "bash", 30)

    # 把旧解析打死：新路径还在跑就说明迁移真的落地（不是死代码）
    def _boom(*a, **k):
        raise AssertionError("新路径不应再调用旧 parse_kv_lines")

    monkeypatch.setattr(state_mod, "parse_kv_lines", _boom)
    # 整档重写（保留 `# CONFIG` 标题）：`drill("CONFIG")` + "/>" 会连标题一起换掉，
    # 之后 read_section_chunks(..., "CONFIG") 读不到节（实测），与本用例目的无关。
    ws.write_locator(
        wt_uid, "/.mds/bash.md", SelectorBuilder().build(), "/>",
        "# CONFIG\n- shell: \"\"\"zsh\"\"\"\n- timeout: 45\n- stream_chunk: 8192\n",
        auto_scaffold=True, god=True, suppress_hooks=True,
    )
    config = await st.load_global_config(ws, ctx)
    assert (config.shell, config.timeout, config.stream_chunk) == ("zsh", 45, 8192), config



# Part 2: bash extract_strip_value（PROC / EXEC 块）


# service.py::update_run_proc / write_exec_fields 写出的真实形状
EXEC_BLOCK = ("## 601b98f9\n\n- mode: command\n\n- cmd: printf hello\n\n- stdout: hello\n\n"
              "- stderr: \n\n- exit: 0\n\n- cwd: /tmp\n\n- shell: bash\n\n"
              "- timeout: 30\n\n- status: DONE")
PROC_BLOCK = "## 601b98f9\n\n- status: RUNNING\n\n- kind: run\n\n- pid: 4242"

EXTRACT_EQUAL_CASES = {
    # (text, key) → 期望值
    "exec_exit": (EXEC_BLOCK, "exit"),
    "exec_status": (EXEC_BLOCK, "status"),
    "proc_kind": (PROC_BLOCK, "kind"),
    "proc_status": (PROC_BLOCK, "status"),
    "missing_key": (PROC_BLOCK, "nope"),
    "empty_text": ("", "status"),
    "flag_only": ("- status\n", "status"),
    "indented_pseudo_line_first": ("- stdout: x\n    - status: BOGUS\n- status: RUNNING", "status"),
}

EXTRACT_EXPECTED_EQUAL = {
    "exec_exit": "0",
    "exec_status": "DONE",
    "proc_kind": "run",
    "proc_status": "RUNNING",
    "missing_key": "",
    "empty_text": "",
    "flag_only": "",
    "indented_pseudo_line_first": "RUNNING",
}

EXTRACT_DIVERGENT_CASES = {
    # U1：顶层重复键 → 旧取首个，新取末个（`bullets_to_kv` 后者胜）
    "U1_duplicate_top_level": ("- status: FIRST\n- status: SECOND\n", "status"),
    "U2_inline_code_value": ("- status: `RUNNING`\n", "status"),
    "U3_lazy_continuation": ("- status: v\n  cont\n", "status"),
}

EXTRACT_EXPECTED_DIVERGENT = {
    "U1_duplicate_top_level": ("FIRST", "SECOND"),
    "U2_inline_code_value": ("`RUNNING`", "RUNNING"),
    "U3_lazy_continuation": ("v", "v cont"),
}


async def test_extract_strip_value_equal_cases(runtime):
    """全等类：oracle、无句柄旧档、新路径三者一致。"""
    ws = runtime._workspace
    for name, (text, key) in EXTRACT_EQUAL_CASES.items():
        expect = EXTRACT_EXPECTED_EQUAL[name]
        assert oracle_extract(text, key) == expect, f"{name}: oracle 与预期不符"
        no_handle = BashState.extract_strip_value(text, key)
        assert no_handle == expect, f"{name}: 旧档 {no_handle!r} != {expect!r}"
        new = BashState.extract_strip_value(text, key, ws)
        assert new == expect, f"{name}: 新路径 {new!r} != {expect!r}"


async def test_extract_strip_value_divergence_whitelist_is_exactly_measured_set(runtime):
    """canary：extract_strip_value 的差异集合必须恰好是实测的 3 类。"""
    ws = runtime._workspace
    cases = {**EXTRACT_EQUAL_CASES, **EXTRACT_DIVERGENT_CASES}
    divergent = {
        name for name, (text, key) in cases.items()
        if oracle_extract(text, key) != BashState.extract_strip_value(text, key, ws)
    }
    assert divergent == set(EXTRACT_DIVERGENT_CASES), (
        f"差异集合变了：多出 {divergent - set(EXTRACT_DIVERGENT_CASES)}，"
        f"消失 {set(EXTRACT_DIVERGENT_CASES) - divergent} —— 请同步更新本文件与迁移报告"
    )


@pytest.mark.parametrize("name", sorted(EXTRACT_DIVERGENT_CASES))
async def test_extract_strip_value_each_divergence_pinned(runtime, name):
    text, key = EXTRACT_DIVERGENT_CASES[name]
    old_expect, new_expect = EXTRACT_EXPECTED_DIVERGENT[name]
    assert oracle_extract(text, key) == old_expect, f"{name}: oracle 变了"
    assert BashState.extract_strip_value(text, key, runtime._workspace) == new_expect, \
        f"{name}: 新路径变了"


async def test_extract_strip_value_falls_back_without_handle(runtime):
    """无句柄 / 引擎异常 ⇒ 旧正则语义（三档），行为不打折。"""
    text = "- status: DONE\n- exit: 0\n"
    assert BashState.extract_strip_value(text, "status") == "DONE"
    assert BashState.extract_strip_value(text, "status", None) == oracle_extract(text, "status")
    assert BashState.extract_strip_value(text, "status", ws_raises()) == oracle_extract(text, "status")
    # 同一文本走新路径也一致（这条文本落在全等类）
    assert BashState.extract_strip_value(text, "status", runtime._workspace) == "DONE"


async def test_list_state_live_path_uses_engine(runtime, wt_uid, monkeypatch):
    """活路径：`@BASH` 列出状态（service.py::list_state）走引擎后仍报对 kind/status/exit。"""
    from mdio_py.plugins.bash import state as state_mod

    await runtime.exec('"printf hello" | @BASH', wt_uid=wt_uid)
    out = await runtime.exec("@BASH", wt_uid=wt_uid)
    assert "# BASH" in out and "## RECENT" in out, out
    assert "exit=0" in out, out   # 当前 list_state 的 RECENT 行格式：`- <run_id>: exit=<n>`

    def _boom(*a, **k):
        raise AssertionError("新路径不应再调用旧 parse_kv_lines")

    monkeypatch.setattr(state_mod, "parse_kv_lines", _boom)
    out_new = await runtime.exec("@BASH", wt_uid=wt_uid)
    assert out_new == out, (out_new, out)



# Part 3: cron cdef（_parse_cron_chunks）


# drill('CRON_JOBS').any_child() 的真实 chunk 形状（含 `## job_id` 标题行）
CRON_EQUAL_CHUNKS = {
    "flat_job": (
        "## job_a\n- enabled: true\n- wt_uid: genesis\n- cwd: /\n- mode: at\n"
        "- schedule: +1d\n- pipeline: '\"hi\"'\n\n"
    ),
    "nested_note": (
        "## job_b\n- enabled: true\n- wt_uid: genesis\n- mode: every\n- schedule: 30s\n"
        "- pipeline: '\"x\"'\n  - note: nested\n"
    ),
    "empty_body": "## job_c\n",
    "quoted_pipeline": "## job_d\n- pipeline: \"'a:b'\"\n- cwd: '/x y'\n",
    "colon_in_value": "## job_e\n- schedule: a:b:c\n- mode: cron\n",
}

CRON_EXPECTED = {
    "flat_job": {"job_a": {"enabled": "true", "wt_uid": "genesis", "cwd": "/",
                           "mode": "at", "schedule": "+1d", "pipeline": "'\"hi\"'"}},
    "nested_note": {"job_b": {"enabled": "true", "wt_uid": "genesis", "mode": "every",
                              "schedule": "30s", "pipeline": "'\"x\"'", "note": "nested"}},
    "empty_body": {"job_c": {}},
    "quoted_pipeline": {"job_d": {"pipeline": "\"'a:b'\"", "cwd": "'/x y'"}},
    "colon_in_value": {"job_e": {"schedule": "a:b:c", "mode": "cron"}},
}

CRON_DIVERGENT_CHUNKS = {
    "D1_space_before_colon": "## job_x\n- m :  s\n- enabled: true\n",
    "D3_lazy_continuation": "## job_x\n- pipeline: v\n  lazy\n",
    "D5_code_block_line": "## job_x\n```\n- pipeline: v\n```\n",
    "D6_inline_code_value": "## job_x\n- pipeline: `v`\n",
    "D7_emphasis_value": "## job_x\n- pipeline: **v**\n",
}

CRON_EXPECTED_DIVERGENT = {
    # (oracle, 新)
    "D1_space_before_colon": ({"job_x": {"m": "s", "enabled": "true"}},
                              {"job_x": {"enabled": "true"}}),
    "D3_lazy_continuation": ({"job_x": {"pipeline": "v"}},
                             {"job_x": {"pipeline": "v lazy"}}),
    "D5_code_block_line": ({"job_x": {"pipeline": "v"}}, {"job_x": {}}),
    "D6_inline_code_value": ({"job_x": {"pipeline": "`v`"}}, {"job_x": {"pipeline": "v"}}),
    "D7_emphasis_value": ({"job_x": {"pipeline": "**v**"}}, {"job_x": {"pipeline": "v"}}),
}


def cron_oracle(chunks: list) -> dict:
    """旧 `_parse_cron_chunks` 语义复刻：首行当标题，其余行 `parse_kv_lines`（无 unquote）。"""
    result: dict = {}
    for chunk in chunks:
        lines = chunk.strip().split("\n")
        if not lines:
            continue
        name = lines[0].strip().lstrip("#").strip()
        if name:
            result[name] = oracle_kv("\n".join(lines[1:]))
    return result


async def test_cron_defs_equal_cases(runtime):
    ws = runtime._workspace
    for name, chunk in CRON_EQUAL_CHUNKS.items():
        assert cron_oracle([chunk]) == CRON_EXPECTED[name], f"{name}: oracle 与预期不符"
        assert cron_defs_no_handle([chunk]) == CRON_EXPECTED[name], f"{name}: 旧档不符"
        assert cron_defs(ws, [chunk]) == CRON_EXPECTED[name], f"{name}: 新路径不符"


async def test_cron_defs_divergence_whitelist_is_exactly_measured_set(runtime):
    """canary：cron cdef 的差异集合必须恰好是实测的 5 类。"""
    ws = runtime._workspace
    cases = {**CRON_EQUAL_CHUNKS, **CRON_DIVERGENT_CHUNKS}
    divergent = {name for name, chunk in cases.items()
                 if cron_oracle([chunk]) != cron_defs(ws, [chunk])}
    assert divergent == set(CRON_DIVERGENT_CHUNKS), (
        f"差异集合变了：多出 {divergent - set(CRON_DIVERGENT_CHUNKS)}，"
        f"消失 {set(CRON_DIVERGENT_CHUNKS) - divergent} —— 请同步更新本文件与迁移报告"
    )


@pytest.mark.parametrize("name", sorted(CRON_DIVERGENT_CHUNKS))
async def test_cron_defs_each_divergence_pinned(runtime, name):
    chunk = CRON_DIVERGENT_CHUNKS[name]
    old_expect, new_expect = CRON_EXPECTED_DIVERGENT[name]
    assert cron_oracle([chunk]) == old_expect, f"{name}: oracle 变了"
    assert cron_defs(runtime._workspace, [chunk]) == new_expect, f"{name}: 新路径变了"


async def test_cron_defs_falls_back_without_handle():
    chunk = CRON_DIVERGENT_CHUNKS["D1_space_before_colon"]
    assert cron_defs_no_handle([chunk]) == cron_oracle([chunk])
    assert CRONPlugin._parse_cron_chunks([chunk], workspace=ws_raises()) == cron_oracle([chunk])


async def test_read_cron_defs_live_path_uses_engine(runtime, wt_uid, monkeypatch):
    """活路径：`_read_cron_defs` 走引擎读 cron.md（不再调旧 parse_kv_lines）。"""
    from mdio_py.plugins.cron import cron as cron_mod

    ws = runtime._workspace
    body = ("# CRON_JOBS\n\n## job_a\n- enabled: true\n- wt_uid: genesis\n- mode: at\n"
            "- schedule: +1d\n- pipeline: '\"hi\"'\n")
    ws.write_locator(wt_uid, "/.mds/cron.md", SelectorBuilder().build(), "/>", body,
                     auto_scaffold=True, god=True, suppress_hooks=True)
    plugin = CRONPlugin()
    ctx = RuntimeContext(wt_uid=wt_uid)

    defs = await plugin._read_cron_defs(ws, ctx)
    assert set(defs) == {"job_a"}, defs
    assert defs["job_a"]["pipeline"] == "'\"hi\"'", defs
    assert defs["job_a"]["wt_uid"] == "genesis"

    def _boom(*a, **k):
        raise AssertionError("新路径不应再调用旧 parse_kv_lines")

    monkeypatch.setattr(cron_mod, "parse_kv_lines", _boom)
    assert await plugin._read_cron_defs(ws, ctx) == defs



# Part 4: cron proc state（_read_proc_state）


PROC_MD = ("# PROCESS_TABLE\n\n## CRON\n\n### job_a\n- status: running\n- pid: 123\n"
           "- last_run: 2026-01-01T00:00:00Z\n")
PROC_EXPECT = {"status": "running", "pid": "123", "last_run": "2026-01-01T00:00:00Z"}


async def test_read_proc_state_equal_to_oracle(runtime, wt_uid):
    ws = runtime._workspace
    ws.write_locator(wt_uid, "/.mds/proc.md", SelectorBuilder().build(), "/>", PROC_MD,
                     auto_scaffold=True, god=True, suppress_hooks=True)
    plugin = CRONPlugin()
    ctx = RuntimeContext(wt_uid=wt_uid)
    state = await plugin._read_proc_state(ws, ctx, "job_a")
    assert state == PROC_EXPECT, state
    assert state == oracle_kv(PROC_MD.split("### job_a\n", 1)[1])


async def test_read_proc_state_live_path_uses_engine(runtime, wt_uid, monkeypatch):
    from mdio_py.plugins.cron import cron as cron_mod

    ws = runtime._workspace
    ws.write_locator(wt_uid, "/.mds/proc.md", SelectorBuilder().build(), "/>", PROC_MD,
                     auto_scaffold=True, god=True, suppress_hooks=True)
    plugin = CRONPlugin()
    ctx = RuntimeContext(wt_uid=wt_uid)

    def _boom(*a, **k):
        raise AssertionError("新路径不应再调用旧 parse_kv_lines")

    monkeypatch.setattr(cron_mod, "parse_kv_lines", _boom)
    assert await plugin._read_proc_state(ws, ctx, "job_a") == PROC_EXPECT
    assert await plugin._read_proc_state(ws, ctx, "no_such_job") is None



# Part 5: 未迁移的正则（只记录不动）的守卫



def test_non_md_regexes_left_untouched():
    """`local.py` 的 marker 正则是 pexpect 匹配子进程输出；cron 两个正则是时长解析。

    两者都不是 md 解析（判定结论见模块 docstring 第三节），因此保持原样：
    本用例只钉住「它们仍在原处、语义没被误改」。
    """
    from pathlib import Path

    root = Path(__file__).resolve().parents[1] / "src" / "mdio_py" / "plugins"
    local_src = (root / "bash" / "backends" / "local.py").read_text(encoding="utf-8")
    assert 'child.expect(re.compile(rf"{re.escape(marker)}:(\\d+)"), timeout=timeout)' in local_src
    cron_src = (root / "cron" / "cron.py").read_text(encoding="utf-8")
    assert '_DURATION_RE = re.compile(r"^(\\d+)\\s*(s|m|h|d)$")' in cron_src
    assert '_COMPOUND_RE = re.compile(r"(\\d+\\.?\\d*)\\s*([smhd])")' in cron_src


def _legacy_call_lines(src: str) -> list:
    """源码里真正的旧解析**调用行**（docstring 里用反引号引用的说明不算）。"""
    return [ln.strip() for ln in src.splitlines() if "parse_kv_lines(" in ln and "`" not in ln]


def test_legacy_parse_left_only_as_degradation_branch():
    """旧 `parse_kv_lines` 只允许剩在降级档（`kv is None` 分支里），不得回到主路径。

    数量是契约：state.py 1 处（`_parse_kv_chunks` 的降级）、cron.py 2 处
    （`_parse_cron_chunks` + `_read_proc_state` 各一）。谁把主路径改回旧解析，这里先红。
    """
    from pathlib import Path

    root = Path(__file__).resolve().parents[1] / "src" / "mdio_py" / "plugins"
    state_src = (root / "bash" / "state.py").read_text(encoding="utf-8")
    assert len(_legacy_call_lines(state_src)) == 1, _legacy_call_lines(state_src)
    assert "if kv is None:" in state_src, "旧解析不在降级分支里"

    cron_src = (root / "cron" / "cron.py").read_text(encoding="utf-8")
    assert len(_legacy_call_lines(cron_src)) == 2, _legacy_call_lines(cron_src)
    assert cron_src.count("if kv is None:") == 2, "cron 旧解析不在降级分支里"
