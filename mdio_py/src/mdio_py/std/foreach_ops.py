"""foreach: execute exec for every -sel item and concatenate results."""

from __future__ import annotations

from .map_common import (error, get_frame, parse_sel, require_exec_block,
                         run_item)


async def foreach(workspace, pipe_data, args, kwargs, ctx):
    try:
        selector_text, ir, face = parse_sel(kwargs)
        exec_block = require_exec_block(kwargs)
        frame = get_frame()
    except Exception as e:
        return error("foreach", f"{type(e).__name__}: {e}")

    source_text = str(pipe_data if pipe_data is not None else "")
    try:
        src = frame.create_doc("foreach_src", source_text)
        scratch = frame.create_doc("foreach_scratch", source_text)
    except Exception as e:
        return error("foreach", f"临时文档创建失败: {type(e).__name__}: {e}")

    try:
        hits = workspace.query_memory_ranges(source_text, ir, proj=face)
    except Exception as e:
        return error("foreach", f"-sel 查询失败: {type(e).__name__}: {e}")

    out = []
    for idx, (_start, _end, payload) in enumerate(hits):
        try:
            result = await run_item(
                workspace, exec_block, ctx, selector_text, idx, payload, scratch
            )
        except Exception as e:
            return error("foreach", f"exec 执行失败: {type(e).__name__}: {e}")
        if isinstance(result, str) and result.startswith("- mdio_error:"):
            return result
        out.append(str(result or ""))

    return "".join(out)
