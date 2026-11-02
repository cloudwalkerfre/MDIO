"""sel_ops —— 选择器编译出口（`sel compile "<选择器原文>"`）。

目标：声明装载时把 `match` 的值按 `::` 切成「文档 + 选择器」两段，选择器那一段送网关换回一份 IR。
为什么必须有这条出口：客户端本地只做标题链（ui/region.js 组件刻意不做本地状态），完整选择器语义必须由核心唯一权威。

形态：不新增端点、不新增 DTO —— 挂 /exec 的动词（与 `worktree use` 同构，FuncCall 嵌套），响应体就是 `CommandResponse.result` 字符串。
两条纪律（都不许踩）：
  ① 不调用 core/interpreter.py 的 _split_selector_path：它对裸选择器返回 steps=[]（把整串当 file_path），
     且编译失败会静默回填 `[["Drill", sel]]`；本出口自己加 `$ ` 前缀、失败**抛**。
  ② 只吃右段：左段（作用域 / 文档）是客户端的事，引擎不认 `npm:` / `doc:` / `.ui` 这些写法。
"""
import json
from typing import Any, List, Optional

from mdio_py.core import compile_selector_text
from mdio_py.core.error import MdioError, ErrCode


def _first_str(args: List[Any]) -> str:
    """取第一个字符串参数（FuncCall 嵌套形态下，参数可能被包一层）。"""
    for a in args or []:
        if hasattr(a, 'name') and not getattr(a, 'args', None):
            return str(a.name)
        s = str(a).strip()
        if len(s) >= 2 and ((s[0] == '"' and s[-1] == '"') or (s[0] == "'" and s[-1] == "'")):
            return s[1:-1]
        if s:
            return s
    return ""


async def _compile(workspace, pipe_data, args, kwargs, ctx) -> str:
    """`sel compile "<选择器>"` → {"steps": Step[]}（融合后的 steps，客户端直接缓存）。"""
    sel = _first_str(args)
    if not sel:
        return "- mdio_error: [%s] sel compile requires a selector" % ErrCode.PARSE_TOKEN
    steps = compile_selector_text(sel).steps   # 唯一编译入口（空 IR 内部抛 P001，失败照旧上抛）
    return json.dumps({"steps": steps}, ensure_ascii=False)


_SUB = {"compile": _compile}


async def sel(workspace, pipe_data, args, kwargs, ctx):
    """SEL —— 选择器编译（`sel compile "<选择器原文>"`）。子命令与 worktree 同构。"""
    sub: Optional[str] = None
    sub_args = list(args)
    if sub_args and hasattr(sub_args[0], 'name'):
        nested = sub_args.pop(0)
        sub = str(nested.name).strip().lower()
        for na in getattr(nested, 'args', []):
            sub_args.append(str(na.name) if hasattr(na, 'name') else str(na).strip())
    elif sub_args and not str(sub_args[0]).startswith("-"):
        sub = str(sub_args.pop(0)).strip().lower()
    if not sub:
        return "- mdio_error: [%s] sel requires a sub-command: %s" % (ErrCode.PARSE_TOKEN, ", ".join(sorted(_SUB.keys())))
    handler = _SUB.get(sub)
    if not handler:
        return "- mdio_error: [%s] unknown sel sub-command '%s'. Available: %s" % (ErrCode.PARSE_TOKEN, sub, ", ".join(sorted(_SUB.keys())))
    return await handler(workspace, pipe_data, sub_args, kwargs, ctx)
