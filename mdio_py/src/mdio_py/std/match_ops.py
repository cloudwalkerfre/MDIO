"""match_ops —— 归属注册出口（patch match attr 的声明层）。

职责（只做三件事，其余交核）：
  ① 地址切分：**首个** `::` 切「文件段 + 选择器右段」（无 `::` ⇒ 抛，禁静默回填）；
  ② 文件段归一：声明层约定 → 核认 glob（核永不看到 `.*`；globset 的 `*` 不跨 `/`）；
  ③ 选择器右段经 Lark 编成 steps（与 `sel compile` 同一条路：parse_code + optimize_ir）。

纪律（沿用 std/sel_ops.py）：
  - 不调用 core/interpreter.py 的 _split_selector_path（对裸选择器会返回 steps=[]）；
  - 只吃右段：左段（文档/作用域）由调用方按地址约定给出；
  - 编译失败**抛**，不回填。
"""
from __future__ import annotations

from typing import Any, List, Optional, Tuple

from mdio_py.core import compile_selector_text, split_selector_addr
from mdio_py.core.error import ErrCode, MdioError

#: 声明层「任意文档」写法 → 核认 glob（globset：`**` 跨 `/`，`*` 不跨）。
ANY_FILE_DECL = ".*"


def normalize_file_pattern(file_seg: str) -> str:
    """声明层文件段 → 核认 glob 形态。

    - 空段 / `.*` → `**`（任意文档，含子目录）
    - `<dir>/.*` → `<dir>/**`
    - 已是核认 glob（含 `*`）或其他路径 → 原样透传
    """
    seg = (file_seg or "").strip()
    if seg in ("", ANY_FILE_DECL):
        return "**"
    if seg.endswith("/" + ANY_FILE_DECL):
        return seg[: -len(ANY_FILE_DECL)] + "**"
    if seg == ANY_FILE_DECL:
        return "**"
    return seg


def split_address(address: str) -> Tuple[str, str]:
    """地址 → (文件段, 选择器右段)。**首个** `::` 切分（契约：右段保留其后的 ::）；
    无 `::` 或空右段 ⇒ 抛。"""
    s = (address or "").strip()
    if "::" not in s:
        raise MdioError(
            "match",
            ErrCode.PARSE_TOKEN,
            "match address requires '::' (file::selector): " + repr(address),
        )
    file_seg, _, sel = s.partition("::")
    sel = sel.strip()
    if not sel:
        raise MdioError(
            "match",
            ErrCode.PARSE_TOKEN,
            "match address has empty selector: " + repr(address),
        )
    return file_seg.strip(), sel


def compile_steps(sel: str) -> List[Any]:
    """选择器右段 → steps（与 `sel compile` 同源：parse_code + optimize_ir）。"""
    steps = compile_selector_text(sel).steps   # 唯一编译入口
    if not steps:
        raise MdioError("match", ErrCode.PARSE_TOKEN, "match: 空 steps: " + sel)
    return steps


def register_match(store, wt_uid: str, address: str, tag: str, *, ir_cls=None) -> int:
    """地址 + 标签 → 核注册；返回 reg_id（核侧失败会抛 IOError）。"""
    file_seg, sel = split_address(address)
    pattern = normalize_file_pattern(file_seg)
    steps = compile_steps(sel)
    if ir_cls is None:  # 延迟导入，避免与 mdio_ir 循环
        from mdio_py.core.mdio_ir import SelectorIR as ir_cls  # type: ignore
    return int(store.register_match(wt_uid, pattern, ir_cls(steps=steps), tag))


def remove_match(store, reg_id: int) -> int:
    return int(store.remove_match(int(reg_id)))


def clear_doc_matches(store, wt_uid: str, vfs_path: str) -> None:
    store.clear_doc_matches(wt_uid, vfs_path)


def clear_worktree_matches(store, wt_uid: str) -> None:
    store.clear_worktree_matches(wt_uid)


def match_count_for_worktree(store, wt_uid: str) -> int:
    return int(store.match_count_for_worktree(wt_uid))
