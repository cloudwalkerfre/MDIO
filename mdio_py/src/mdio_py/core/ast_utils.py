"""消费侧工具：从 `tojson` 产出的 AST 信封里提取常见形状。

边界：`tojson` 算子只做「md 文本 → 整档 JSON」，不认识业务形状；
把 JSON 变成 `{键: 文本}` 这类映射属于**调用方**。这里放的是多个调用方共用的最小提取器，
不参与引擎投影，也不改变 `tojson` 的契约（它仍然原样透传信封）。

实测语义（`# H` / `- model: m1` / `- plain text` 的 AST）：
    - k: v   ⇒ {"nt":"b","t":"k","ct":[{"nt":"p","ct":"v"}]}
    - 无冒号 ⇒ {"nt":"b","t":"","ct":[{"nt":"p","ct":"plain text"}]}
因此「只认 `- k: v` 行」等价于「取 `t` 非空的 bullet」——空键即过滤。
"""

from typing import Any, Dict, Iterator


# 容器节点是**文本屏障**：键值只取节点自己的行内文本，绝不吞掉嵌套子项的文本。
# （实测：`- k:` 下挂子 bullet 时，`ct` = [p(自己的行内), bl(子项)]，若不设屏障，
#  键值会变成「自己的行内 + 子项文本」的拼接。）
_CONTAINER_NT = frozenset({"root", "h", "bl", "b"})


def inline_text(v: Any) -> str:
    """把节点 `ct` 的三种形态（字符串 / 字典 / 节点数组）统一成文本。

    - 容器节点（root/h/bl/b）返回空串——行内文本写在各层 `p` / `txt` 里；
    - 行内代码（实测 `{"nt":"cd","cd":"xyz"}`，`ct` 为 None）取 `cd` 字段；
    - 粗体/斜体/链接（`st` / `lk`）递归取内层 `txt`——标记被去掉，文字保留。
    """
    if isinstance(v, str):
        return v
    if isinstance(v, list):
        return "".join(inline_text(x) for x in v)
    if isinstance(v, dict):
        if v.get("nt") in _CONTAINER_NT:
            return ""
        if v.get("ct") is None:
            return str(v.get("cd") or "")
        return inline_text(v.get("ct", ""))
    return ""


def walk(node: Any) -> Iterator[Dict[str, Any]]:
    """深度优先遍历 `ct`（信封 → 所有后代节点）。"""
    if not isinstance(node, dict):
        return
    yield node
    kids = node.get("ct")
    if isinstance(kids, list):
        for ch in kids:
            if isinstance(ch, dict):
                yield from walk(ch)


def bullets_to_kv(env: Any) -> Dict[str, str]:
    """信封里所有 `- k: v` 形式的 bullet → `{键: 文本}`。

    - 递归遍历：bullet 位于 `bl` 容器内，也可能嵌在别的 bullet 之下；
    - 只取 `t` 非空的节点（无冒号的 bullet 其 `t` 为空串）；
    - **键用 `inline_text` 归一**：标题/带标记的键在 AST 里可能是行内节点数组（实测 `## hook_2`
      的 `t` = `[{'nt':'txt','ct':'hook'},…]`），`str(t)` 会得到一串 Python repr；
    - 值去首尾空白（CRLF/多余空格不会污染比较，如 `true\\r`）；
    - 同名键后者覆盖前者（与逐行解析的 dict 赋值一致）。
    """
    out: Dict[str, str] = {}
    for n in walk(env):
        if n.get("nt") != "b":
            continue
        key = inline_text(n.get("t")).strip()
        if not key:
            continue
        out[key] = inline_text(n.get("ct", "")).strip()
    return out


def sections(env: Any) -> Dict[str, Dict[str, str]]:
    """节表：`{节标题: {键: 文本}}`，按文档顺序，同名节后者覆盖前者。

    **节正文是标题的兄弟而不是子节点**（实测 CF 拓扑）：所以“读某一节”不能只走标题子树，
    要取标题之后、下一个标题之前的兄弟块。本函数按文档顺序做这个窗口切分，供
    「读某个配置节 / 清单条目」这类场景使用（`$` 读取路径下的 `read_section_chunks`
    是另一条等价通路）。
    """
    flat = list(walk(env))
    out: Dict[str, Dict[str, str]] = {}
    for i, node in enumerate(flat):
        if not isinstance(node, dict) or node.get("nt") != "h":
            continue
        title = inline_text(node.get("t")).strip()
        if not title:
            continue
        window: list = []
        for nxt in flat[i + 1:]:
            if isinstance(nxt, dict) and nxt.get("nt") == "h":
                break
            window.append(nxt)
        out[title] = bullets_to_kv({"nt": "root", "ct": window})
    return out


# ── AST → markdown（消费侧反向映射）─────────────────────────────────
# 用途：把「引用容器（bq）」的子树按层级映射回**载荷原文**（HIS 的载荷读法之一）。
# 实测（9 类载荷）：纯文本/空行/软换行、列表、载荷自带引用、引用内含列表、`*` 强调、
# 链接、转义 —— **7/9 逐字节一致**；两类不可逆（见 _LOSSLESS_NOTE）。

_LOSSLESS_NOTE = (
    "AST 不含这两类字节信息：① 强调分隔符形式（`_x_`/`__x__` 会映射成 `*x*`/`**x**`）；"
    "② 行尾空白（硬换行的两个空格）——需要逐字节时请改走原文（raw + 一层 peel）。"
)


def _inline_md(v: Any) -> str:
    """行内节点 → markdown 文本（尽量还原标记）。"""
    if isinstance(v, str):
        return v
    if isinstance(v, list):
        return "".join(_inline_md(x) for x in v)
    if isinstance(v, dict):
        nt = v.get("nt")
        if nt == "txt":
            return v.get("ct") or ""
        if nt in ("sb", "lb"):          # 软换行 / 硬换行
            return "\n"
        if nt == "cd":                  # 行内代码
            return "`" + (v.get("cd") or "") + "`"
        if nt == "tx":                  # 转义字符
            return "\\" + (v.get("ct") or "")
        if nt == "em":
            return "*" + _inline_md(v.get("ct")) + "*"
        if nt == "st":
            return "**" + _inline_md(v.get("ct")) + "**"
        if nt == "lk":
            return "[" + _inline_md(v.get("ct")) + "](" + (v.get("h") or "") + ")"
        if nt == "im":
            return "![" + (v.get("alt") or "") + "](" + (v.get("s") or "") + ")"
        if nt == "p":
            return _inline_md(v.get("ct"))
        return ""
    return ""


def _block_lines(n: Any) -> list:
    """块节点 → markdown 行。"""
    if not isinstance(n, dict):
        return []
    nt = n.get("nt")
    if nt in ("p", "txt"):
        return _inline_md(n.get("ct") if nt == "p" else [n]).split("\n")
    if nt == "bl":                                   # 列表（载荷里的 `- k: v` 行）
        out = []
        for it in (n.get("ct") or []):
            if not isinstance(it, dict) or it.get("nt") != "b":
                continue
            body = _block_lines({"nt": "p", "ct": it.get("ct")})
            key = it.get("t") or ""
            out += [(f"- {key}: {body[0]}" if key else f"- {body[0]}")] + body[1:]
        return out
    if nt in ("od", "ul"):                           # 有序列表容器（实测 ol 项带编号于 `t`）
        out = []
        for it in (n.get("ct") or []):
            if not isinstance(it, dict) or it.get("nt") != "ol":
                continue
            body = _block_lines({"nt": "p", "ct": it.get("ct")})
            out += [(f"{it.get('t') or '1'}. {body[0]}")] + body[1:]
        return out
    if nt == "ol":                                   # 单个有序项（无容器时的兜底）
        body = _block_lines({"nt": "p", "ct": n.get("ct")})
        return [(f"{n.get('t') or '1'}. {body[0]}")] + body[1:]
    if nt == "bq":                                   # 嵌套引用
        return bq_payload(n)
    if nt == "h":
        return ["#" * (n.get("lv") or 1) + " " + _inline_md(n.get("t"))]
    if nt == "c":                                    # 围栏代码块
        # 引用容器内的 `c` 节点：`cd` 自带 `> ` 前缀，且**已含闭合围栏**（实测）
        body = (n.get("cd") or "").split("\n")
        body = [(l[2:] if l.startswith("> ") else ("" if l == ">" else l)) for l in body]
        while body and body[-1] == "":
            body.pop()
        closing = "" if (body and body[-1].strip() == "```") else "```"
        lines = ["```" + (n.get("t") or "")] + body
        return lines + ([closing] if closing else [])
    return []


def bq_payload(bq: Any) -> list:
    """引用容器（bq）子树 → 载荷行列表（**已剥掉容器自身那层前缀**）。

    规则（实测得出）：
    - 子块默认不加前缀；**嵌套 bq 子块**才加 `> `（它是载荷自己写的引用行）；
    - 兄弟块之间补空行；**两个相邻的嵌套 bq 之间**补的是空引用行 `>`（写侧空载荷行
      在引用里写成 `  > >`，解析后就是两个兄弟 bq）。
    """
    out: list = []
    prev = None
    for ch in (bq.get("ct") or []) if isinstance(bq, dict) else []:
        if out:
            both_bq = (isinstance(prev, dict) and prev.get("nt") == "bq"
                       and isinstance(ch, dict) and ch.get("nt") == "bq")
            out.append(">" if both_bq else "")
        lines = _block_lines(ch)
        if isinstance(ch, dict) and ch.get("nt") == "bq":
            lines = [("> " + l if l else ">") for l in lines]
        out += lines
        prev = ch
    return out


def bq_payload_text(bq: Any) -> str:
    """`bq_payload` 的字符串形态（尾部换行剥掉）。"""
    return "\n".join(bq_payload(bq)).rstrip("\n")


def is_lossless(bq: Any) -> bool:
    """该 bq 子树能否**保证**被 `bq_payload` 逐字节还原。

    返回 False 的构造（实测）：
    - `em` / `st`：AST 不存强调分隔符形式（`_x_` 会映射成 `*x*`）；
    - `lb`：AST 只存"硬换行"，两个尾随空格丢失；
    - `bl` / `od`：**列表项之间的空行（loose list）在 JSON 里没有标记**（引擎只在扫描态
      里用它），所以列表型载荷无法保证逐字节 —— 保守判 False。

    返回 False 时，若调用方要求逐字节，应改走原文（`-pj raw` + 一层 peel）。
    """
    for n in walk(bq):
        nt = n.get("nt")
        if nt in ("em", "st", "lb", "bl", "od"):
            return False
    return True
