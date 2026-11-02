"""std 侧 kv 对拍：手写解析（oracle） vs tojson + bullets_to_kv。

迁移点：
  - `std/worktree_ops.py::_parse_worktree_entries`：worktrees.md 的每个 `## <name>` 块 →
    `{name, uid, rule, path, doc_subdir, wal_rel}`；被 `runtime/workspace.py` 与 `std/fs_ops.py`
    共 6 处调用（签名不变）。
  - `std/search_ops.py::_parse_find_cache`：find.md 的每个 `## <path>` 块 →
    `(doc_hash, [tokens])` 元组；tokenizer 仍走 `by_tag` 单读，未改。

两个 oracle（本文件内复刻旧实现语义）：
  1. worktrees：逐行 `- ` 开头 + 首个冒号切分（旧 `core/text_utils.py::parse_kv_lines`）；
  2. find：**逐行 strip 后**前缀匹配 `"- doc_hash: "` / `"- tokens: "`（旧 `search_ops.py:251-257`）。
     关键细节：strip 掉行尾空格后，`- tokens: ` 这类空值行匹配不到带空格的前缀 ⇒ 旧实现整条跳过。

实测差异（`test_divergence_whitelist_is_exactly_measured_set` 做 canary）：

| 样例 | 旧 | 新 | 方向 |
|---|---|---|---|
| 平铺 uid/rule/path/doc_subdir/wal_rel、空值 `- rule:`、值里带冒号、引号值、重复键、嵌套、CRLF、tab 缩进 | 一致 | 一致 | **必须全等** |
| worktrees `- uid : u`（冒号前有空格） | `{uid: 'u'}` | 丢该键 | 引擎判空 tag（与 gateway 迁移 D1 同类） |
| find `- doc_hash:h1` / `- tokens:a, b`（冒号后无空格） | 整条跳过 | 正常识别 | 以引擎 AST 为准：键确实存在，更稳 |
| find 空 `- tokens:` | 整条跳过 | 整条跳过（**刻意保留旧口径**） | 空 tokens 若入缓存，find 命中分支会以零 token 进索引 |

引擎边界（跨任务，实测）：末行是 `- k:`（冒号结尾且无尾换行）时 `parse_to_ast` 投影越界 panic
（`mdio_core/src/projection.rs:1879`；`PanicException` 不是 `Exception` 子类 ⇒ `except Exception`
兜不住）。VFS 落盘会把文本归一成以换行结尾，故 selector 读回的 chunk 不受影响；两处迁移仍加了
防御性补换行，本文件用 `test_engine_panic_*` 与 `test_vfs_chunk_*` 钉住这两条前提。
"""

import json

import pytest

from mdio_py.core import ast_utils
from mdio_py.core.ast_utils import bullets_to_kv
from mdio_py.core.selector_builder import SelectorBuilder
from mdio_py.std.search_ops import _format_find_cache, _parse_find_cache
from mdio_py.std.worktree_ops import _parse_worktree_entries

WORKTREES_MD = "/.mds/worktrees.md"
FIND_MD = "/.mds/cache/find.md"



# oracle 与新路径（与迁移实现同形）



def oracle_kv(body: str) -> dict:
    """旧 parse_kv_lines 语义复刻（core/text_utils.py:94-107）。"""
    out: dict = {}
    for line in str(body).split("\n"):
        line = line.strip()
        if not line.startswith("- ") or ":" not in line[2:]:
            continue
        k, v = line[2:].split(":", 1)
        out[k.strip()] = v.strip()
    return out


def oracle_find_entry(body: str):
    """旧 `_parse_find_cache` 的块内解析（search_ops.py 迁移前 :249-259）。"""
    doc_hash = None
    tokens = None
    for line in str(body).split("\n"):
        line = line.strip()
        if line.startswith("- doc_hash: "):
            doc_hash = line[len("- doc_hash: "):].strip()
        elif line.startswith("- tokens: "):
            raw = line[len("- tokens: "):].strip()
            tokens = [t for t in raw.split(", ") if t]
    if doc_hash and tokens is not None:
        return (doc_hash, tokens)
    return None


def new_kv(workspace, body: str) -> dict:
    """新路径：tojson 信封 → bullets_to_kv（含迁移里的防御性补换行）。"""
    if not body.endswith("\n"):
        body += "\n"
    return bullets_to_kv(json.loads(workspace.to_json_text(body)))


def new_find_entry(workspace, body: str):
    """新路径 + 调用方值后处理（`", "` 切 tokens；空 tokens 保留旧跳过口径）。"""
    kv = new_kv(workspace, body)
    doc_hash = kv.get("doc_hash") or None
    tokens = [t for t in kv["tokens"].split(", ") if t] if kv.get("tokens") else None
    if doc_hash and tokens is not None:
        return (doc_hash, tokens)
    return None


def _write(ws, path: str, text: str) -> None:
    ws.write_locator("genesis", path, SelectorBuilder().build(), "+>", text,
                     auto_scaffold=True, god=True, suppress_hooks=True)


def _chunks(ws, path: str, root: str):
    ir = SelectorBuilder().drill(root).any_child().build()
    return [str(c) for c in ws.read_locator("genesis", path, ir)]



# Part 1: worktrees.md 块级对拍（平铺/边界）


WT_FLAT = {
    "canonical": (
        "- uid: test1234\n- rule:\n- path: /tmp/test_proj\n",
        {"uid": "test1234", "rule": "", "path": "/tmp/test_proj"},
    ),
    "full": (
        "- uid: abc999\n- rule: rw:/sandbox/*\n- path: /home/x/proj\n"
        "- doc_subdir: wt\n- wal_rel: .wal\n",
        {"uid": "abc999", "rule": "rw:/sandbox/*", "path": "/home/x/proj",
         "doc_subdir": "wt", "wal_rel": ".wal"},
    ),
    "colon_in_value": ("- uid: u\n- path: /tmp/a:b\n", {"uid": "u", "path": "/tmp/a:b"}),
    "quoted": ("- uid: u\n- rule: \"quoted rule\"\n", {"uid": "u", "rule": "\"quoted rule\""}),
    "dup_uid_last_wins": ("- uid: first\n- uid: second\n", {"uid": "second"}),
    "nested_child": ("- uid: n1\n- rule: rw:/a\n  - extra: deeper\n",
                     {"uid": "n1", "rule": "rw:/a", "extra": "deeper"}),
    "crlf": ("- uid: u\r\n- path: /tmp/p\r\n", {"uid": "u", "path": "/tmp/p"}),
    "tab_indent": ("- uid: u\n\t- extra: x\n", {"uid": "u", "extra": "x"}),
    "empty_value_and_flag": ("- uid: u\n- path:\n- flag_only\n", {"uid": "u", "path": ""}),
}


async def test_worktree_flat_chunks_new_equals_oracle(runtime):
    """worktrees.md 平铺/边界块：新路径与 oracle、与预期三方全等。"""
    ws = runtime._workspace
    for name, (body, expected) in WT_FLAT.items():
        old = oracle_kv(body)
        new = new_kv(ws, body)
        assert old == expected, f"{name}: oracle {old!r} != 预期 {expected!r}"
        assert new == expected, f"{name}: 新路径 {new!r} != 预期 {expected!r}"


async def test_parse_worktree_entries_end_to_end(runtime):
    """真实函数对拍：写真实形状的 worktrees.md → `_parse_worktree_entries` == 逐块 oracle。"""
    ws = runtime._workspace
    text = (
        "# WORKTREES\n\n"
        "## test_proj\n- uid: test1234\n- rule:\n- path: /tmp/test_proj\n\n"
        "## layered\n- name: layered\n- uid: abc999\n- rule: rw:/sandbox/*\n"
        "- path: /home/x/proj\n- doc_subdir: wt\n- wal_rel: .wal\n\n"
        "## nested\n- uid: n1\n- rule: rw:/a\n  - extra: deeper\n- path: /tmp/nested\n"
    )
    _write(ws, WORKTREES_MD, text)

    entries = _parse_worktree_entries(ws, "genesis")
    assert [e["name"] for e in entries] == ["test_proj", "layered", "nested"]

    expected = [
        {"name": "test_proj", "uid": "test1234", "rule": "", "path": "/tmp/test_proj"},
        {"name": "layered", "uid": "abc999", "rule": "rw:/sandbox/*",
         "path": "/home/x/proj", "doc_subdir": "wt", "wal_rel": ".wal"},
        {"name": "nested", "uid": "n1", "rule": "rw:/a", "extra": "deeper",
         "path": "/tmp/nested"},
    ]
    assert entries == expected

    # 逐块对拍：oracle（旧手写行解析）与新路径必须一致
    for chunk, entry in zip(_chunks(ws, WORKTREES_MD, "WORKTREES"), entries):
        lines = chunk.split("\n")
        name = lines[0].strip().lstrip("#").strip()
        body = "\n".join(lines[1:])
        assert entry == {"name": name, **oracle_kv(body)}, f"chunk={chunk!r}"
        assert entry == {"name": name, **new_kv(ws, body)}, f"chunk={chunk!r}"


async def test_worktree_space_before_colon_divergence(runtime):
    """差异①：`- uid : u`（冒号前空格）在引擎里 tag 为空 ⇒ 新实现丢键，旧实现保留。"""
    body = "- uid : u\n- path: /tmp/p\n"
    assert oracle_kv(body) == {"uid": "u", "path": "/tmp/p"}
    assert new_kv(runtime._workspace, body) == {"path": "/tmp/p"}
    assert "uid" not in new_kv(runtime._workspace, body)


async def test_chunk_framing_avoids_heading_sibling_trap(runtime):
    """核对 Lead 广播 #1（CF 拓扑：标题正文是**兄弟**节点）：

    - 反例：只看标题节点子树 ⇒ `bullets_to_kv(标题节点)` 是空表（这正是坑）；
    - 本迁移走的是引擎块框架给的 chunk（标题行 + 其后正文行），故不受影响；
    - 与 `ast_utils.sections(env)` 的窗口切分交叉验证：两边结果一致。
    """
    ws = runtime._workspace
    text = "# WORKTREES\n\n## test_proj\n- uid: test1234\n- rule:\n- path: /tmp/test_proj\n"
    _write(ws, WORKTREES_MD, text)

    chunks = _chunks(ws, WORKTREES_MD, "WORKTREES")
    assert len(chunks) == 1 and chunks[0].split("\n")[0].startswith("## ")
    body = "\n".join(chunks[0].split("\n")[1:])
    assert new_kv(ws, body) == {"uid": "test1234", "rule": "", "path": "/tmp/test_proj"}

    env = json.loads(ws.to_json_text(text))
    heading = next(n for n in ast_utils.walk(env)
                   if n.get("nt") == "h" and ast_utils.inline_text(n.get("t")) == "test_proj")
    assert ast_utils.bullets_to_kv(heading) == {}, "标题子树里没有 bullet —— 正是广播里的坑"

    assert ast_utils.sections(env)["test_proj"] == new_kv(ws, body)



# Part 2: find 缓存块级对拍 + 真实函数闭环


FIND_FLAT = {
    "normal": ("- doc_hash: h1\n- tokens: alpha, beta\n", ("h1", ["alpha", "beta"])),
    "single_token": ("- doc_hash: h1\n- tokens: alpha\n", ("h1", ["alpha"])),
    "comma_variants": ("- doc_hash: h1\n- tokens: a,b, c\n", ("h1", ["a,b", "c"])),
    "comma_only": ("- doc_hash: h1\n- tokens: ,\n", ("h1", [","])),
    "dup_hash_last_wins": ("- doc_hash: h1\n- doc_hash: h2\n- tokens: a\n", ("h2", ["a"])),
    "nested_lines": ("- doc_hash: h1\n  - tokens: a, b\n", ("h1", ["a", "b"])),
    "crlf": ("- doc_hash: h1\r\n- tokens: a, b\r\n", ("h1", ["a", "b"])),
}

FIND_SKIPPED = {
    "empty_tokens": "- doc_hash: h1\n- tokens: \n",          # 旧：strip 后前缀不匹配 ⇒ 跳过
    "empty_tokens_nospace": "- doc_hash: h1\n- tokens:\n",
    "no_hash": "- tokens: alpha\n",
    "empty_hash": "- doc_hash: \n- tokens: alpha\n",
}


async def test_find_flat_chunks_new_equals_oracle(runtime):
    """find 缓存块：平铺样例新旧一致。"""
    ws = runtime._workspace
    for name, (body, expected) in FIND_FLAT.items():
        old = oracle_find_entry(body)
        new = new_find_entry(ws, body)
        assert old == expected, f"{name}: oracle {old!r} != 预期 {expected!r}"
        assert new == expected, f"{name}: 新路径 {new!r} != 预期 {expected!r}"


async def test_find_skipped_cases_stay_skipped(runtime):
    """缺失/空值的块：新旧都跳过（含刻意保留的「空 tokens 跳过」口径）。"""
    ws = runtime._workspace
    for name, body in FIND_SKIPPED.items():
        assert oracle_find_entry(body) is None, f"{name}: oracle 不该产出"
        assert new_find_entry(ws, body) is None, f"{name}: 新路径不该产出"


async def test_parse_find_cache_end_to_end(runtime):
    """真实函数闭环：`_format_find_cache` 写入 → `_parse_find_cache` == 逐块 oracle。"""
    ws = runtime._workspace
    written = {"/docs/a.md": ("h1", ["alpha", "beta", "gamma"]),
               "/docs/b.md": ("h2", []),          # 空 tokens → 新旧都跳过
               "/docs/c.md": ("h3", ["x"])}
    _write(ws, FIND_MD, _format_find_cache(written))

    state, tokenizer = _parse_find_cache(ws, "genesis")
    assert tokenizer == "v1", "tokenizer 仍走 by_tag 单读，未迁移"
    assert state == {"/docs/a.md": ("h1", ["alpha", "beta", "gamma"]),
                     "/docs/c.md": ("h3", ["x"])}

    # 逐块 oracle 对拍（跳过非条目块：chunk 首行不是 `## <path>` 的 tokenizer 块）
    expected = {}
    for chunk in _chunks(ws, FIND_MD, "FIND_CACHE"):
        lines = chunk.split("\n")
        path = lines[0].strip().lstrip("#").strip()
        if not path.startswith("/"):
            continue
        got = oracle_find_entry("\n".join(lines[1:]))
        if got is not None:
            expected[path] = got
    assert state == expected


async def test_find_no_space_after_colon_divergence(runtime):
    """差异②：`- doc_hash:h1` / `- tokens:a, b`（冒号后无空格）旧整条跳过、新正常识别。"""
    ws = runtime._workspace
    hash_body = "- doc_hash:h1\n- tokens: alpha, beta\n"
    tok_body = "- doc_hash: h1\n- tokens:alpha, beta\n"
    assert oracle_find_entry(hash_body) is None
    assert oracle_find_entry(tok_body) is None
    assert new_find_entry(ws, hash_body) == ("h1", ["alpha", "beta"])
    assert new_find_entry(ws, tok_body) == ("h1", ["alpha", "beta"])


async def test_find_bm25_live_path_end_to_end(runtime):
    """bm25 活路径（`_parse_find_cache` 的唯一消费者）：写缓存 → 解析 → 命中缓存结果不变。

    为什么单独跑活路径：脚本式 `test_find_comprehensive.py` 在 C1 处就 abort（既有红），
    它后面的 `-bm25` 段根本执行不到，故 bm25 段不能作为迁移的回归依据。
    """
    ws = runtime._workspace
    await runtime.exec('"# DOC A\n- alpha beta gamma" | +> /docs/a.md -f -god', wt_uid="genesis")
    await runtime.exec('"# DOC B\n- delta epsilon zeta" | +> /docs/b.md -f -god', wt_uid="genesis")

    r1 = await runtime.exec('find -bm25 "alpha" -p "/docs/*" -n 5', wt_uid="genesis")
    assert "a.md" in r1 and "alpha" in r1, f"bm25 首跑应命中 a.md：{r1!r}"

    state, tokenizer = _parse_find_cache(ws, "genesis")
    assert tokenizer == "v1"
    assert "/docs/a.md" in state and "alpha" in state["/docs/a.md"][1], state
    cached = ws.store.get_text("genesis", FIND_MD)

    r2 = await runtime.exec('find -bm25 "alpha" -p "/docs/*" -n 5', wt_uid="genesis")
    assert r2 == r1, "缓存命中分支结果应与首次一致"
    assert ws.store.get_text("genesis", FIND_MD) == cached, "缓存不应被重复改写"



# Part 3: 引擎边界与 VFS 前提



async def test_engine_handles_colon_bullet_at_eof(runtime):
    """引擎已修复末行 `- k:` 无尾换行的边界：裸 FFI `parse_to_ast` 不再 panic，
    返回正常 root 信封；保留回归防止旧 panic 复现。
    """
    env = json.loads(runtime._workspace._store.parse_to_ast("- k:"))
    assert env["nt"] == "root"


async def test_binding_normalizes_missing_trailing_newline(runtime):
    """绑定层防护：缺尾换行由 `to_json_text` 归一，调用方拿不到 panic。"""
    env = runtime._workspace.to_json_text("- k:")
    assert json.loads(env)["nt"] == "root"


async def test_vfs_chunk_always_ends_with_newline(runtime):
    """VFS 前提：写入不带尾换行，store 落盘仍归一成以换行结尾 ⇒ selector chunk 恒带 `\\n`。

    这正是两处迁移的防御性补换行在当前调用路径上不会触发的理由。
    """
    ws = runtime._workspace
    _write(ws, WORKTREES_MD, "# WORKTREES\n\n## t\n- uid: u\n- rule:")   # 故意无尾换行
    raw = ws.store.get_text("genesis", WORKTREES_MD)
    assert raw.endswith("- rule:\n"), f"落盘未归一：{raw!r}"
    chunks = _chunks(ws, WORKTREES_MD, "WORKTREES")
    assert chunks and all(c.endswith("\n") for c in chunks), chunks



# Part 4: canary —— 差异集合必须恰好是实测集合


_DIVERGENT_WT = {"- uid : u\n- path: /tmp/p\n"}
_DIVERGENT_FIND = {"- doc_hash:h1\n- tokens: a\n", "- doc_hash: h1\n- tokens:a\n"}


async def test_divergence_whitelist_is_exactly_measured_set(runtime):
    """canary：整份样例集上的差异必须恰好落在实测清单里（helper/引擎语义再变时先红）。"""
    ws = runtime._workspace

    wt_bodies = {body for body, _ in WT_FLAT.values()} | _DIVERGENT_WT
    diff_wt = {body for body in wt_bodies if oracle_kv(body) != new_kv(ws, body)}
    assert diff_wt == _DIVERGENT_WT, f"worktrees 差异集变了：{diff_wt}"

    find_bodies = ({body for body, _ in FIND_FLAT.values()}
                   | set(FIND_SKIPPED.values()) | _DIVERGENT_FIND)
    diff_find = {body for body in find_bodies
                 if oracle_find_entry(body) != new_find_entry(ws, body)}
    assert diff_find == _DIVERGENT_FIND, f"find 差异集变了：{diff_find}"
