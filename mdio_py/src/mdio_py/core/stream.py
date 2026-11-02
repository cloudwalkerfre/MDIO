"""管道流协议工具 —— 方言识别 + 流表记录。

设计归属（谁决定什么）：
- 流 = asyncgen（值域之外的一等物；写目标遇流自动流式，零 kwarg）
- 粒度 = 源（yield 边界即 chunk 边界；管道不聚合不拆切）
- 管道 = 纯搬运 + 类型适配（值消费点累积成值）
- 写目标 = 只分流中与流末（流中每 chunk suppress 写；流末一笔非 suppress → hook 恰好一次）

本模块只提供无状态工具，不含任何驱动编排——编排在消费方
（interpreter 的 WriteOp 分支）。classify 识别两种方言：
- 元组方言（llm）: ("content", x) / ("error", x) / ("thinking"|"tool", x) ...
- 文本方言（bash）: 以 "- mdio_error:" 开头的 str = error（全仓既有错误约定）
"""

import asyncio
import time
import uuid

from .error import ERROR_PREFIX, is_error_text
_CHUNK_KINDS = {"content", "chunk", "text"}
_EVENT_KINDS = {"thinking", "tool", "event", "progress", "status", "info", "warning"}

# 流表（/.mds/streams.md）路径与容器
STREAMS_PATH = "/.mds/streams.md"
STREAMS_CONTAINER = "STREAM_TABLE"


def classify(item):
    """把流项归一为 (kind, payload)；kind ∈ {"chunk", "event", "error"}。

    规则：
    - 元组方言 (ctype, payload)：content/chunk/text → chunk；
      error → error；thinking/tool/event/... → event（旁路，不入写目标）；
      未知 ctype → error（防脏写：宁可报错不可 str() 脏化进文档）
    - 文本方言：以 "- mdio_error:" 开头的 str → error（bash 收尾尾巴自动分流）；
      其余 str → chunk
    - None / 其他形态 → event（旁路）
    """
    if isinstance(item, tuple) and len(item) == 2 and isinstance(item[0], str):
        ctype, payload = item
        if ctype in _CHUNK_KINDS:
            return ("chunk", payload)
        if ctype == "error":
            return ("error", payload)
        if ctype in _EVENT_KINDS:
            return ("event", (ctype, payload))
        return ("error", "unknown stream ctype: %r" % (ctype,))
    if isinstance(item, str):
        if is_error_text(item):
            # 前导空白容忍（bash 收尾会在错误串前带换行，见 plugins/bash/service.py 的 mdio_error 返回）
            return ("error", item)
        return ("chunk", item)
    return ("event", item)


def new_run_id() -> str:
    """流表段 id：短 uuid。"""
    return uuid.uuid4().hex[:8]



async def stream_write(data, op, write, *, classify_fn=None):
    """公共流式写驱动（延迟一拍实现——局限内保证 hook 恰好一次）。

    批序契约：前 N-1 笔真实内容以 suppress_hooks=True 落笔（延迟一拍——
    下一 chunk 到达时才落上一拍），最后一笔真实内容以 suppress_hooks=False
    落笔。真实内容写必产生真实编辑区间 → 剪枝通过 → 目标 hash 变 → hook
    触发恰好一次——不依赖空写（空写事务的 edit 区间为空，会触发剪枝 skip，
    不可靠）也不产生任何附加污染（无空行尾巴）。

    事件方言：chunk → 写目标（空 chunk 跳过、不占 pending）；event → 旁路；
    error → 终态 failed（已到达内容照落——保留策略）。全空流（无任何非空
    chunk）→ 零写（文档没变——hook 自判不触发；错误感知走 exec 返回 + 流表）。

    异常语义：普通异常 → failed（不传播——与 error 项同语义）；CancelledError
    → 置 cancelled 后 re-raise（finally 落末笔照常——调用方负责记录与传播）。

    返回 (status, reason, batches, bytes)；write(op, text, suppress_hooks)
    由调用方注入（真实写或测试 recorder）。
    """
    classify_fn = classify_fn or classify
    status = "completed"
    reason = None
    pending = None
    wrote = False
    batches = 0
    total = 0
    try:
        async for item in data:
            kind, payload = classify_fn(item)
            if kind == "chunk":
                text = str(payload)
                if not text:
                    continue
                if pending is not None:
                    eff = op if not wrote else "+>"
                    write(eff, pending, True)
                    wrote = True
                    batches += 1
                    total += len(pending)
                pending = text
            elif kind == "error":
                if status == "completed":
                    status = "failed"
                reason = str(payload)
            # event → 旁路（不入写目标）
    except asyncio.CancelledError:
        status = "cancelled"
        reason = "cancelled"
        raise
    except Exception as e:
        if status == "completed":
            status = "failed"
        reason = str(e)
    finally:
        if pending is not None:
            eff = op if not wrote else "+>"
            write(eff, pending, False)  # 末笔真实内容非 suppress → hook 恰好一次
            batches += 1
            total += len(pending)
    return status, reason, batches, total


def write_stream_record(workspace, wt_uid, run_id, status, reason,
                        meta=None, *, batches=0, total_bytes=0):
    """流表记录：/.mds/streams.md 的 STREAM_TABLE 段下追加 run_id 小节。

    终态信息走"文档即载体"（hook 触发载荷只有 cb_code + 冻结于注册时刻的
    ctx_dict，见 core/events.py）——cb 代码读本表得 status/reason。
    记录自身 suppress_hooks=True（系统区防涟漪）；单次事务写整个小节。
    reason 单行化（换行转字面 \\n）防破坏 bullet 结构。
    """
    from .selector_builder import drill  # 延迟导入避免模块级环
    meta = meta or {}
    safe_reason = ""
    if reason:
        # 单行化：换行/回车转空格，防破坏流表的 bullet 结构
        safe_reason = str(reason).replace("\r", " ").replace("\n", " ")
    lines = [
        "",
        "### " + run_id,
        "- status: " + str(status),
    ]
    if safe_reason:
        lines.append("- reason: " + safe_reason)
    for k, v in meta.items():
        val = str(v).replace("\n", " ").replace("\r", " ")
        lines.append("- " + str(k) + ": " + val)
    lines.append("- batches: " + str(batches))
    lines.append("- bytes: " + str(total_bytes))
    lines.append("- ts: " + str(int(time.time())))
    body = "\n".join(lines)
    try:
        workspace.write_locator(
            wt_uid, STREAMS_PATH, drill(STREAMS_CONTAINER), "+>", body,
            auto_scaffold=True, god=True, auto_level=False,
            suppress_hooks=True, proj="content",
        )
    except Exception:
        pass  # 系统区记录失败不阻断主流程（记录是尽力而为）
