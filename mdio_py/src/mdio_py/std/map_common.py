"""Shared map/foreach helpers: strict -sel parsing and item execution."""

from __future__ import annotations

import posixpath
import re
from dataclasses import replace
from typing import Any

from ..core import parse_code
from ..core.mdio_ir import ExecBlock, Literal, Pipeline
from ..core.mem_slot import current_temp_frame


def error(cmd: str, msg: str) -> str:
    return f"- mdio_error: [{cmd}] {msg}"


def parse_sel(kwargs: dict) -> tuple[str, Any, str]:
    """Return (selector_text, SelectorIR, face) for a strict -sel locator."""
    from ..core.interpreter import get_projection

    sel_text = str(kwargs.get("-sel", "") or "").strip()
    if not sel_text:
        raise ValueError("需要 -sel <selector> -pj <face>")
    if "-pj" in kwargs:
        raise ValueError("-pj 必须写在 -sel 里，不能再作为独立参数")
    if "-n" in kwargs:
        raise ValueError("map/foreach 不接受 -n，内部 source/output 由命令管理")
    if "::" in sel_text:
        raise ValueError("-sel 不允许带 <path>::，只能是节点 selector")

    parsed = parse_code("$$ " + sel_text)[0].atoms[0]
    selector = parsed.selector
    if getattr(selector, "file_path", None) is not None:
        raise ValueError("-sel 不允许带 <path>::，只能是节点 selector")

    m = re.search(r"\s+-pj\s+\S+\s*$", sel_text)
    selector_text = sel_text[:m.start()].strip() if m else sel_text
    if not selector_text or "::" in selector_text:
        raise ValueError("-sel 非法")
    face = get_projection(parsed.kwargs) or "content"
    return selector_text, selector, face


def item_md(selector_text: str, idx: int, payload: str) -> str:
    return (
        "# item\n"
        f"- idx: {idx}\n"
        f"- loc: {selector_text}[{idx}]\n"
        "## md\n"
        f"{payload}\n"
    )


def exec_ctx_for(ctx, doc_path: str):
    return replace(
        ctx,
        active_doc=doc_path,
        vfs_path=posixpath.dirname(doc_path),
    )


async def run_item(workspace, exec_block: ExecBlock, ctx, selector_text: str,
                   idx: int, payload: str, active_doc: str):
    item = item_md(selector_text, idx, payload)
    frame = get_frame()
    frame.aliases["__input_mem"] = frame.create_doc("input", item)
    atoms = [Literal(item)] + list(exec_block.pipeline.atoms)
    return await workspace.runtime._interpreter.run(
        [Pipeline(atoms=atoms)], exec_ctx_for(ctx, active_doc)
    )


def require_exec_block(kwargs: dict) -> ExecBlock:
    exec_block = kwargs.get("-f")
    if not isinstance(exec_block, ExecBlock):
        raise ValueError("-f 必须是 exec block")
    return exec_block


def get_frame():
    frame = current_temp_frame()
    if frame is None:
        raise RuntimeError("当前没有 temp doc frame")
    return frame
