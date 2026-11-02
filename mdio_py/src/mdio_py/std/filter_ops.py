"""filter: keep items whose predicate is true, delete items whose predicate is false."""

from __future__ import annotations

from .map_common import error, get_frame, parse_sel, require_exec_block, run_item


def _parse_bool(result):
    if isinstance(result, str) and result.startswith("- mdio_error:"):
        return None, result
    val = str(result or "").strip().lower()
    if val == "true":
        return True, None
    if val == "false":
        return False, None
    return None, f"predicate 必须返回 true/false，实际为 {result!r}"


async def filter(workspace, pipe_data, args, kwargs, ctx):
    try:
        selector_text, ir, face = parse_sel(kwargs)
        exec_block = require_exec_block(kwargs)
        frame = get_frame()
        if face not in ("content", "raw", "tag"):
            raise ValueError(f"face {face!r} 暂不支持（仅 content/raw/tag）")
    except Exception as e:
        return error("filter", f"{type(e).__name__}: {e}")

    source_text = str(pipe_data if pipe_data is not None else "")
    try:
        frame.create_doc("filter_src", source_text)
        out_path = frame.create_doc("filter_out", source_text)
    except Exception as e:
        return error("filter", f"临时文档创建失败: {type(e).__name__}: {e}")

    try:
        hits = workspace.query_memory_ranges(source_text, ir, proj=face)
    except Exception as e:
        return error("filter", f"-sel 查询失败: {type(e).__name__}: {e}")

    if not hits:
        return frame.read_doc(out_path)

    decisions = []
    for idx, (_start, _end, payload) in enumerate(hits):
        try:
            result = await run_item(
                workspace, exec_block, ctx, selector_text, idx, payload, out_path
            )
        except Exception as e:
            return error("filter", f"predicate 执行失败: {type(e).__name__}: {e}")

        keep, err = _parse_bool(result)
        if err is not None:
            return err if str(err).startswith("- mdio_error:") else error("filter", err)
        decisions.append(keep)

    if all(decisions):
        return frame.read_doc(out_path)

    out_text = frame.read_doc(out_path)
    try:
        h = frame.store.mem_iter_begin(out_text, ir, proj=None)
        rev = list(reversed(decisions))
        seen = 0
        while h.next() is not None:
            if seen >= len(rev):
                return error("filter", "输出 item 数与选中 item 数不一致")
            if not rev[seen]:
                h.commit("", "->", None)
            seen += 1
        out_text = h.end()
        if seen != len(rev):
            return error("filter", "输出 item 数与选中 item 数不一致")
        frame.write_full_doc(out_path, out_text)
    except Exception as e:
        return error("filter", f"删除失败: {type(e).__name__}: {e}")

    return out_text
