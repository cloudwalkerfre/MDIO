"""tojson：md 文本 → 整档 JSON（AST 信封）。

判据对应方案 §7.2：
  A 引擎两路同源：canon(to_ast_json(D)) == canon(parse_to_ast(get_text(D)))
  C 算子与引擎同源：canon(tojson(管道)) == canon(parse_to_ast(同一文本))
  D 可用性：tojson | loads 往返；nt == "root" 且 ct 是 list
  E 边界：空输入 → mdio_error；文档路径取不到内容 → 抛错

另有登记面断言（路由表由流水线隐式验证；std 导出直接断言）。
"""

import json

import pytest

from mdio_py import MdioRuntime
import mdio_py.std as std_module

NL = chr(10)
CFG = "# CFG" + NL + NL + "- model: m1" + NL + "- sysp: hi" + NL


def canon(env: str) -> str:
    """信封归一：去掉随调用变化的字段。"""
    d = json.loads(env)
    d.pop("md", None)
    d.pop("timestamp", None)
    return json.dumps(d, sort_keys=True, ensure_ascii=False)


@pytest.fixture
async def rt_cfg(tmp_path):
    """带一份配置文档的临时 runtime（文档根恒为 <root>/wt）。"""
    (tmp_path / "wt").mkdir(exist_ok=True)
    (tmp_path / "wt" / "cfg.md").write_text(CFG)
    (tmp_path / "wt" / "empty.md").write_text("")
    rt = MdioRuntime(str(tmp_path))
    yield rt
    rt.close()


# ── 登记面 ────────────────────────────────────────────────────────

def test_std_exports_tojson():
    assert "tojson" in std_module.__all__
    assert callable(std_module.tojson)


async def test_std_surface_is_fully_registered(runtime):
    """std 面一致性：每个导出都必须在解释器路由表里可解析（防“算子存在但没注册”）。

    导出名 ≠ 算子名的地方要显式列出（实测两处）：`length`→`len`、`hash_state`→`hash`。
    """
    from mdio_py.std import __all__ as STD_EXPORTS
    router = runtime._interpreter.std_functions
    alias = {"length": "len", "hash_state": "hash"}
    missing = [n for n in STD_EXPORTS if alias.get(n, n) not in router]
    assert missing == [], f"未注册的 std 导出: {missing}"
    assert router["tojson"] is std_module.tojson


# ── A 引擎两路同源（不经过算子） ───────────────────────────────────

async def test_engine_doc_and_text_paths_agree(rt_cfg):
    st = rt_cfg._workspace._store
    doc_path = st.to_ast_json("genesis", "/cfg.md")
    text_path = st.parse_to_ast(st.get_text("genesis", "/cfg.md"))
    assert canon(doc_path) == canon(text_path)


# ── C 算子与引擎同源 ─────────────────────────────────────────────

async def test_op_matches_engine_text_path(rt_cfg):
    st = rt_cfg._workspace._store
    piped = await rt_cfg.exec("$ /cfg.md | tojson", wt_uid="genesis")
    engine = st.parse_to_ast(st.get_text("genesis", "/cfg.md"))
    assert canon(piped) == canon(engine)


async def test_literal_text_pipe(rt_cfg):
    res = await rt_cfg.exec('"- k: v" | tojson', wt_uid="genesis")
    d = json.loads(res)
    assert d["nt"] == "root"
    assert isinstance(d["ct"], list)
    assert d["ct"], "字面量文本应产出至少一个块节点"


# ── D 可用性 ─────────────────────────────────────────────────────

async def test_pipe_into_loads(rt_cfg):
    # 管道形态：tojson → loads 能跑通（exec 返回最终值的字符串表示）
    res = await rt_cfg.exec("$ /cfg.md | tojson | loads", wt_uid="genesis")
    assert "'nt': 'root'" in res

    # 往返：tojson 的产物能被 loads 解析成对象
    raw = await rt_cfg.exec("$ /cfg.md | tojson", wt_uid="genesis")
    d = await std_module.loads(rt_cfg._workspace, raw, [], {}, None)
    assert isinstance(d, dict)
    assert d["nt"] == "root"
    assert isinstance(d["ct"], list)


async def test_doc_binding_returns_envelope(rt_cfg):
    raw = rt_cfg._workspace.to_ast_json_doc("genesis", "/cfg.md")
    d = json.loads(raw)
    assert d["nt"] == "root"
    assert d["md"] == "cfg.md"


# ── E 边界 ───────────────────────────────────────────────────────

async def test_empty_input_is_error(rt_cfg):
    res = await std_module.tojson(rt_cfg._workspace, "", [], {}, None)
    assert res.startswith("- mdio_error: [tojson]")


async def test_empty_doc_pipe_is_error(rt_cfg):
    res = await rt_cfg.exec("$ /empty.md | tojson", wt_uid="genesis")
    assert res.startswith("- mdio_error: [tojson]")


async def test_doc_binding_raises_on_missing(rt_cfg):
    with pytest.raises(ValueError):
        rt_cfg._workspace.to_ast_json_doc("genesis", "/definitely-not-here.md")


# ── 引擎边界：无尾换行的空值行（绑定层归一 + 算子兜底）─────────────

async def test_no_trailing_newline_empty_value_is_normalized(rt_cfg):
    """末行 `- k:`（空值）且无尾换行的文本，引擎会越界 panic（projection.rs:1879）。

    绑定层补尾换行后必须正常产出信封；这段测试同时钉住"VFS 文本恒以换行结尾"这个前提。
    """
    env = rt_cfg._workspace.to_json_text("- k:")
    d = json.loads(env)
    assert d["nt"] == "root"
    assert isinstance(d["ct"], list)


# ── 选择器模式 + `-pj` 面（规范形 §9.9 硬约束：语义文本 vs 原文）──────

PAY_MD = "# H\n\n- assistant:\n  > 第一段\n  >\n  > 第二段第一行\n  > 第二段第二行\n"


@pytest.fixture
async def rt_faces(tmp_path):
    (tmp_path / "wt").mkdir(exist_ok=True)
    (tmp_path / "wt" / "cfg.md").write_text(CFG)
    (tmp_path / "wt" / "pay.md").write_text(PAY_MD)
    rt = MdioRuntime(str(tmp_path))
    yield rt
    rt.close()


async def test_selector_default_face_is_raw_and_byte_exact(rt_faces):
    """选择器模式默认 `raw`：逐字节（空载荷行、软换行都在）——HIS 字节面走这条。"""
    res = await rt_faces.exec("tojson -p /pay.md::H._b", wt_uid="genesis")
    assert "> 第一段" in res, res
    assert "\n  >\n" in res, f"空载荷行必须原样保留: {res!r}"


async def test_selector_content_face_is_semantic_text(rt_faces):
    """`-pj content`：语义文本（剥掉引用标记）——喂模型的形态。"""
    res = await rt_faces.exec('tojson -p /pay.md::H._b -pj content', wt_uid="genesis")
    assert "第一段" in res and "> " not in res, res


async def test_selector_ast_face_matches_pipe_form(rt_faces):
    """`-pj ast`：命中节点子树 → AST JSON，与 `$ sel | tojson` 同源。"""
    via_sel = await rt_faces.exec('tojson -p /pay.md::H._b -pj ast', wt_uid="genesis")
    via_pipe = await rt_faces.exec("$ /pay.md::H._b | tojson", wt_uid="genesis")
    assert canon(via_sel) == canon(via_pipe)


async def test_selector_tag_face(rt_faces):
    """`-pj tag`：键面。"""
    res = await rt_faces.exec('tojson -p /cfg.md::CFG._b -pj tag', wt_uid="genesis")
    assert "model" in res, res


async def test_unknown_face_is_error(rt_faces):
    res = await rt_faces.exec('tojson -p /cfg.md::CFG._b -pj nope', wt_uid="genesis")
    assert res.startswith("- mdio_error: [tojson] 未知的面"), res


async def test_text_mode_raw_is_identity_and_other_faces_rejected(rt_faces):
    res = await rt_faces.exec('"- k: v" | tojson -pj raw', wt_uid="genesis")
    assert "- k: v" in res and not res.startswith("- mdio_error"), res
    res2 = await rt_faces.exec('"- k: v" | tojson -pj content', wt_uid="genesis")
    assert res2.startswith("- mdio_error: [tojson] 文本模式只支持"), res2


# ── bq 面的适用矩阵（实测）：content = 剥所有层；raw + 一层 peel = 恰好一层 ──

HIS_PAYLOADS = {
    "plain": "第一段\n\n第二段第一行\n第二段第二行",
    "list_like": "- mdio_success: X\n\n- E_CONN_REFUSED: 42\n- E_TIMEOUT: 17",
    "own_quote": "说明\n\n> 这是载荷自己的引用\n\n尾行",
}


def _his_text(payload: str) -> str:
    """按写入规则编码（`  > ` 前缀 / 空行裸 `  >`）。"""
    body = "\n".join("  > " + l if l else "  >" for l in payload.split("\n"))
    return f"# H\n\n- assistant:\n{body}\n"


def _peel_one(ln: str) -> str:
    """剥恰好一层：去 item 缩进（2）→ 去一个 `>` → 若紧跟一空格再去掉一个。"""
    s = ln[2:] if ln.startswith("  ") else ln
    if s.startswith(">"):
        s = s[1:]
        if s.startswith(" "):
            s = s[1:]
    return s


@pytest.mark.parametrize("name", sorted(HIS_PAYLOADS))
async def test_bq_face_matrix(tmp_path, name):
    pay = HIS_PAYLOADS[name]
    (tmp_path / "wt").mkdir(exist_ok=True)
    (tmp_path / "wt" / "pay.md").write_text(_his_text(pay))
    rt = MdioRuntime(str(tmp_path))
    try:
        ct = await rt.exec("tojson -p /pay.md::H._b._bq -pj content", wt_uid="genesis")
        raw = await rt.exec("tojson -p /pay.md::H._b._bq -pj raw", wt_uid="genesis")
        peeled = "\n".join(_peel_one(ln) for ln in raw.rstrip("\n").split("\n"))
        assert peeled == pay, f"{name}: raw+peel 必须逐字节\n got={peeled!r}\nwant={pay!r}"
        if name == "own_quote":
            assert ct != pay, "载荷自带 `>` 行时 content 会多剥一层（记录该边界）"
        else:
            assert ct == pay, f"{name}: content 面就是剥好的载荷\n got={ct!r}\nwant={pay!r}"
    finally:
        rt.close()


async def test_literal_pipe_empty_value_does_not_blast_pipeline(rt_cfg):
    """字面量管道同样走这条边界：算子必须返回结果或 mdio_error，绝不抛穿。"""
    res = await rt_cfg.exec('"- k:" | tojson', wt_uid="genesis")
    assert isinstance(res, str)
    assert res.startswith("{"), res[:120]


# ── 消费侧提取器（core/ast_utils.py）──────────────────────────────

def test_bullets_to_kv_flat_and_nested():
    from mdio_py.core.ast_utils import bullets_to_kv
    env = {
        "nt": "root",
        "ct": [
            {"nt": "h", "lv": 1, "t": "H", "ct": ""},
            {"nt": "bl", "ct": [
                {"nt": "b", "t": "model", "ct": [{"nt": "p", "t": "", "ct": "m1"}]},
                {"nt": "b", "t": "plain", "ct": [{"nt": "p", "t": "", "ct": "no colon"}]},
                {"nt": "b", "t": "nested", "ct": [
                    {"nt": "p", "t": "", "ct": "outer"},
                    {"nt": "bl", "ct": [
                        {"nt": "b", "t": "inner", "ct": "deep"},
                    ]},
                ]},
            ]},
        ],
    }
    assert bullets_to_kv(env) == {
        "model": "m1",
        "plain": "no colon",
        "nested": "outer",
        "inner": "deep",
    }


def test_bullets_to_kv_ignores_empty_key():
    """无冒号的 bullet 其 `t` 为空串 ⇒ 必须被忽略（等价于旧行解析的忽略规则）。"""
    from mdio_py.core.ast_utils import bullets_to_kv
    env = {"nt": "root", "ct": [{"nt": "bl", "ct": [
        {"nt": "b", "t": "", "ct": [{"nt": "p", "t": "", "ct": "无键"}]},
        {"nt": "b", "t": "k", "ct": "v"},
    ]}]}
    assert bullets_to_kv(env) == {"k": "v"}


def test_inline_text_forms():
    from mdio_py.core.ast_utils import inline_text
    assert inline_text("s") == "s"
    assert inline_text([{"nt": "txt", "ct": "a"}, {"nt": "txt", "ct": "b"}]) == "ab"
    assert inline_text({"nt": "p", "ct": [{"nt": "txt", "ct": "x"}]}) == "x"
    assert inline_text(None) == ""
    assert inline_text(123) == ""


def test_bullets_to_kv_tolerates_garbage():
    from mdio_py.core.ast_utils import bullets_to_kv
    assert bullets_to_kv({}) == {}
    assert bullets_to_kv(None) == {}
    assert bullets_to_kv({"ct": "not-a-list"}) == {}


def test_bullets_to_kv_reads_code_span_text():
    """行内代码：文本在 `cd` 字段（实测 `{"nt":"cd","cd":"xyz"}`，`ct` 为 None）。"""
    from mdio_py.core.ast_utils import bullets_to_kv
    env = {"nt": "root", "ct": [{"nt": "bl", "ct": [
        {"nt": "b", "t": "k", "ct": [{"nt": "p", "t": "", "ct": [
            {"nt": "txt", "ct": "用 "}, {"nt": "cd", "cd": "cd"}, {"nt": "txt", "ct": " 切换"}]}]},
    ]}]}
    assert bullets_to_kv(env) == {"k": "用 cd 切换"}


def test_bullets_to_kv_strips_values():
    """值去首尾空白：CRLF/多余空格不得污染比较（旧行解析就是 strip）。"""
    from mdio_py.core.ast_utils import bullets_to_kv
    env = {"nt": "root", "ct": [{"nt": "bl", "ct": [
        {"nt": "b", "t": "gateway_enabled", "ct": [{"nt": "p", "ct": "true "}]},
        {"nt": "b", "t": "  spaced  ", "ct": [{"nt": "p", "ct": "  v  "}]},
    ]}]}
    assert bullets_to_kv(env) == {"gateway_enabled": "true", "spaced": "v"}


def test_sections_windows_cover_sibling_bullets():
    """节表：标题正文是标题的**兄弟**而非子节点（实测 CF 拓扑）——窗口切分必须覆盖它。

    这条测试同时是"把 `bullets_to_kv(节节点)` 当读节手段"那个坑的回归守卫：
    标题的 `t` 还可能是行内节点数组（`hook_7` 里的下划线被当强调标记）。
    """
    from mdio_py.core.ast_utils import sections
    env = {"nt": "root", "ct": [
        {"nt": "h", "lv": 1, "t": "HOOKS", "ct": ""},
        {"nt": "h", "lv": 2, "t": [{"nt": "txt", "ct": "hook"}, {"nt": "txt", "ct": "_"},
                                   {"nt": "txt", "ct": "7"}], "ct": ""},
        {"nt": "bl", "ct": [
            {"nt": "b", "t": "target", "ct": [{"nt": "p", "ct": "/a.md::S._b:k"}]},
            {"nt": "b", "t": "is_always", "ct": "true"},
        ]},
        {"nt": "h", "lv": 2, "t": "hook_8", "ct": ""},
        {"nt": "bl", "ct": [{"nt": "b", "t": "target", "ct": "/b.md"}]},
    ]}
    secs = sections(env)
    assert secs["HOOKS"] == {}
    assert secs["hook_7"] == {"target": "/a.md::S._b:k", "is_always": "true"}
    assert secs["hook_8"] == {"target": "/b.md"}


# ── bq 子树 → 载荷原文（JSON 层级映射，实测 7/9 逐字节）──────────────

MAP_PAYLOADS = {
    "plain": "第一段\n\n第二段第一行\n第二段第二行",
    "list": "- a: 1\n- b: 2",
    "own_quote": "说明\n\n> 自己的引用\n\n尾",
    "quote_list": "> 引用里还有列表\n>\n> - x\n> - y",
    "emphasis_star": "斜体 *em* 与 **st** 与 `cd`",
    "link": "看 [文档](http://x/y) 尾",
    "escape": "转义 \\*星\\*",
    # 两类**不可逆**（AST 不含该字节信息）：
    "emphasis_under": "斜体 _em_ 与 __st__",
    "trailing": "硬换行尾  \n末行",
}
_UNRECOVERABLE = {"emphasis_under", "trailing"}


@pytest.mark.parametrize("name", sorted(MAP_PAYLOADS))
async def test_bq_payload_mapping_from_json(tmp_path, name):
    """`bq` 子树 → 载荷原文：**JSON 层级映射**（`ast_utils.bq_payload_text`）。

    实测 9 类载荷 7 类逐字节一致；`emphasis_under` / `trailing` 两类不可逆
    （AST 不存强调分隔符形式与行尾空格）——需要逐字节时改走 raw + 一层 peel。

    `is_lossless` 是**保守**检测器：它按"是否可能出现不可逆构造"判定，
    所以 `emphasis_star`（`*em*` 恰好还原）也会被判 False —— 这正是守卫该有的语义
    （不保证 ≠ 一定不等）。
    """
    pay = MAP_PAYLOADS[name]
    (tmp_path / "wt").mkdir(exist_ok=True)
    (tmp_path / "wt" / "h.md").write_text(_his_text(pay))
    rt = MdioRuntime(str(tmp_path))
    try:
        from mdio_py.core import ast_utils
        ast_json = await rt.exec("tojson -p /h.md::H._b -pj ast", wt_uid="genesis")
        env = json.loads(ast_json)
        bq = next(n for n in ast_utils.walk(env) if n.get("nt") == "bq")
        mapped = ast_utils.bq_payload_text(bq)
        lossless = ast_utils.is_lossless(bq)
        if name in _UNRECOVERABLE:
            assert mapped != pay                      # 记录在案的不可逆类
            assert lossless is False, "检测器必须报出不可逆"
        else:
            assert mapped == pay, f"{name}: mapped={mapped!r} want={pay!r}"
            # 保守语义：`emphasis_star`（分隔符形式不可知）与含列表的载荷（loose 空行不可判）都报 False
            assert lossless is (name in ("plain", "own_quote", "link", "escape"))
    finally:
        rt.close()


