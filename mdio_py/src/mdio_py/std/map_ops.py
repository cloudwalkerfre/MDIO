"""map: in-place replacement of each selected node face in an output temp doc."""

from __future__ import annotations

import json

from .map_common import (error, get_frame, parse_sel, require_exec_block,
                         run_item)


def _overlaps(hits) -> bool:
    ranges = [(int(s), int(e)) for s, e, _p in hits]
    ranges.sort()
    for i in range(1, len(ranges)):
        if ranges[i][0] < ranges[i - 1][1]:
            return True
    return False


async def map(workspace, pipe_data, args, kwargs, ctx):
    try:
        selector_text, ir, face = parse_sel(kwargs)
        exec_block = require_exec_block(kwargs)
        frame = get_frame()
        if face not in ("content", "raw", "tag"):
            raise ValueError(f"face {face!r} 暂不支持（仅 content/raw/tag）")
    except Exception as e:
        return error("map", f"{type(e).__name__}: {e}")

    source_text = str(pipe_data if pipe_data is not None else "")
    try:
        frame.create_doc("map_src", source_text)
        out_path = frame.create_doc("map_out", source_text)
    except Exception as e:
        return error("map", f"临时文档创建失败: {type(e).__name__}: {e}")

    try:
        hits = workspace.query_memory_ranges(source_text, ir, proj=face)
    except Exception as e:
        return error("map", f"-sel 查询失败: {type(e).__name__}: {e}")

    if not hits:
        return frame.read_doc(out_path)
    if _overlaps(hits):
        return error("map", "item 选择区间重叠，第一版拒绝")

    results = []
    for idx, (_start, _end, payload) in enumerate(hits):
        try:
            result = await run_item(
                workspace, exec_block, ctx, selector_text, idx, payload, out_path
            )
        except Exception as e:
            return error("map", f"exec 执行失败: {type(e).__name__}: {e}")
        if isinstance(result, str) and result.startswith("- mdio_error:"):
            return result
        new = str(result or "")
        if face == "raw" and not new.endswith("\n"):
            # raw 是整块行；没有尾换行会和下一条记录粘住。
            new += "\n"
        results.append((idx, payload, new))

    # Apply through the same selector over the output copy; do not synthesize
    # item locators with [idx].  MemIter yields hits in reverse document order.
    out_text = frame.read_doc(out_path)
    try:
        h = frame.store.mem_iter_begin(out_text, ir, proj=None)
        rev = list(reversed(results))
        seen = 0
        while h.next() is not None:
            if seen >= len(rev):
                return error("map", "输出 item 数与选中 item 数不一致")
            idx, payload, result = rev[seen]
            patch = json.dumps(
                [{"old": payload, "new": result}], ensure_ascii=False
            )
            h.commit(patch, "~>", None)
            seen += 1
        out_text = h.end()
        if seen != len(rev):
            return error("map", "输出 item 数与选中 item 数不一致")
        frame.write_full_doc(out_path, out_text)
    except Exception as e:
        return error("map", f"apply 失败: {type(e).__name__}: {e}")

    return out_text
