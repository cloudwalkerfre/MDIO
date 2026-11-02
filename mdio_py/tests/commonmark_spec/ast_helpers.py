"""
CommonMark spec 测试 — AST 查询工具。

parse_to_ast(md) → 短 key JSON AST (to_ast_object 格式):

  keys:  nt (nodeType), ct (content), ch (children), lv (level),
         t (text/tag/marker), cd (code), h (href), ti (title),
         s (src), alt, u (url)

  nt 值: p=paragraph, h=header, c=code_fence, ic=indented_code,
         hr=thematic_break, bq=blockquote, bl=bullet_list, b=bullet,
         od=ordered_list, ol=ordered_list_item, em=emphasis, st=strong,
         lk=link, im=image, al=autolink, cd=inline_code, txt=text,
         tx=escaped, sb=softbreak, lb=line_break, entity=entity,
         cm=comment, rd=refdef, dl=strikethrough, dm=display_math,
         mt=inline_math, hb=html_block, he=html_element
"""
import json, tempfile
from mdio_py.mdio_core import ProjectStore

_dummy_dir = tempfile.mkdtemp(prefix="mdio_cm_ast_")
# 布局无兜底：文档根 = <根>/wt、WAL = <根>/.wal，两个名字必须显式给（缺名 core 直接报错）。
_store = ProjectStore(_dummy_dir, "wt", ".wal")


def _unwrap_root(ast):
    """解包 root 信封（{nt:root, mode, ch:[...]}），取 .ch 数组返回。"""
    if isinstance(ast, dict) and ast.get("nt") == "root":
        return ast.get("ct", [])
    return ast


def ast_parse(md: str):
    """解析 markdown 文本为 Python 对象（list[dict]——root 信封解包）。"""
    return _unwrap_root(json.loads(_store.parse_to_ast(md)))


def ast_parse_nested(md: str):
    """解析 markdown 文本。现在和 ast_parse 相同（统一 nested 格式）。"""
    return _unwrap_root(json.loads(_store.parse_to_ast_nested(md)))


# 通用查询

def ast_find_all(node, node_type: str):
    """递归搜索 AST 中所有 nt 匹配的节点。返回 list[node]."""
    results = []
    if isinstance(node, dict) and node.get("nt") == node_type:
        results.append(node)
    if isinstance(node, dict):
        for v in node.values():
            if isinstance(v, (list, dict)):
                results.extend(ast_find_all(v, node_type))
    elif isinstance(node, list):
        for child in node:
            results.extend(ast_find_all(child, node_type))
    return results


def ast_find_first(node, node_type: str):
    results = ast_find_all(node, node_type)
    return results[0] if results else None


def ast_has_type(node, node_type: str) -> bool:
    return len(ast_find_all(node, node_type)) > 0


def ast_inline_text(node) -> str:
    """递归提取节点的纯文本内容。"""
    if isinstance(node, str):
        return node
    if isinstance(node, dict):
        if node.get("nt") == "txt":
            return node.get("ct", "")
        return "".join(ast_inline_text(v) for v in node.values()
                       if isinstance(v, (str, list, dict)))
    if isinstance(node, list):
        return "".join(ast_inline_text(c) for c in node)
    return ""


def ast_block_types(ast):
    """返回 AST 中所有顶层 block 的 nt 列表。"""
    if not isinstance(ast, list):
        return []
    return [b["nt"] for b in ast if isinstance(b, dict) and "nt" in b]


# AST → HTML 渲染（CM spec 对比用）

# nt 值 → HTML tag 渲染
BLOCK_HTML = {
    # 规则 R：标题 Tag 面两编码——t 是数组 = 行内节点（改后主形态）；
    # t 是字符串 = 源文本（无触发）。兼容旧形态（行内挂在 ct）作回退。
    "h": lambda b: f"<h{b.get('lv',1)}>{_render_inlines(b.get('t')) if isinstance(b.get('t'), list) else (_render_inlines(b.get('ct',[])) or b.get('t',''))}</h{b.get('lv',1)}>",
    "p": lambda b: f"<p>{_render_inlines(b.get('ct',[]))}</p>",
    "c": lambda b: _render_code(b),
    "ic": lambda b: _render_code(b),
    "hr": lambda _: "<hr />",
    "bq": lambda b: _render_bq(b),
    "bl": lambda b: f"<ul>\n{_render_list_items(b)}\n</ul>",
    "od": lambda b: f"<ol>\n{_render_list_items(b)}\n</ol>",
    "b": lambda b: _render_item(b),
    "ol": lambda b: _render_item(b),
    "hb": lambda b: _render_hb(b),
    "he": lambda b: _render_he(b),
    "cm": lambda _: "",
    "rd": lambda _: "",
}

INLINE_HTML = {
    "txt": lambda n: n.get("ct", ""),
    "sb": lambda _: "\n",
    "lb": lambda _: "<br />\n",
    "em": lambda n: f"<em>{_render_inlines(n.get('ct',[]))}</em>",
    "st": lambda n: f"<strong>{_render_inlines(n.get('ct',[]))}</strong>",
    "dl": lambda n: f"<del>{_render_inlines(n.get('ct',[]))}</del>",
    "cd": lambda n: f"<code>{n.get('cd','')}</code>",
    "lk": lambda n: f"<a href=\"{n.get('h','')}\">{_render_inlines(n.get('ct',[]))}</a>",
    "im": lambda n: f"<img src=\"{n.get('s','')}\" alt=\"{n.get('alt','')}\" />",
    "al": lambda n: f"<a href=\"{n.get('h','')}\">{n.get('ct','')}</a>",
    "dm": lambda n: f"<code>{n.get('cd','')}</code>",
    "mt": lambda n: f"<code>{n.get('cd','')}</code>",
    "tx": lambda n: n.get("ct", ""),
    "entity": lambda n: n.get("ct", ""),
}


def _esc_code(s: str) -> str:
    """代码体按 CM 输出转义（& < > 三个字符）。"""
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _render_code(block) -> str:
    """<pre><code>…</code></pre>。

    代码体在短键 cd（c 与 ic 都没有 ct——读 ct 会渲染成空 code，
    于是 HTML 比对看不见代码内容）；HTML 转义；尾换行按 CM 约定补齐
    （mdio 的 ic 内容不带尾换行、c 带）。
    """
    txt = block.get("cd", "")
    if txt and not txt.endswith("\n"):
        txt += "\n"
    return f"<pre><code>{_esc_code(txt)}</code></pre>"


def ast_to_html(ast) -> str:
    if not isinstance(ast, list):
        return ""
    parts = []
    for block in ast:
        if not isinstance(block, dict):
            continue
        r = _render_block(block)
        if r:
            parts.append(r)
    return "\n".join(parts)


def _render_block(block) -> str:
    if not isinstance(block, dict):
        return ""
    nt = block.get("nt", "")
    if nt in BLOCK_HTML:
        return BLOCK_HTML[nt](block)
    return _render_leaf(block)


def _render_leaf(block) -> str:
    ch = block.get("ct", [])
    if ch:
        return _render_inlines(ch)
    c = block.get("ct", "")
    if c:
        return str(c)
    return ""



def _render_item(item) -> str:
    """列表项按 CM 形态渲染。

    tight（内容只有一个段落）→ `<li>文本</li>`；
    松散/多块（段落后面还有块）→ 每块独占一行：`<li>\\n<p>…</p>\\n</li>`。
    多块项若把块节点的 ct 当行内列表拼，会抛
    `TypeError: sequence item 1: expected str instance, list found`。
    """
    blocks = [c for c in item.get("ct", []) if isinstance(c, dict)]
    if len(blocks) == 1 and blocks[0].get("nt") == "p":
        return f"<li>{_render_inlines(blocks[0].get('ct', []))}</li>"
    inner = "\n".join(_render_block(c) for c in blocks)
    return f"<li>\n{inner}\n</li>"


def _render_hb(block) -> str:
    """HtmlBlock content: string raw passthrough, or element tree (dual-typed)."""
    c = block.get("ct", "")
    if isinstance(c, str):
        # hb 的 ct 保留尾换行(内容规范)——ast_to_html 用 "\n".join 连接
        # 块——尾换行 + 连接换行 = 空行(hb_24 实测多一个空行)——trim 对齐
        # CM 期望(块间单换行)。
        return c.rstrip("\n")
    if isinstance(c, list):
        return "\n".join(_render_block(ch) for ch in c if isinstance(ch, dict) and _render_block(ch))
    return ""


def _render_he(block) -> str:
    """HtmlElement: <tag>children</tag> round-trip."""
    t = block.get("t", "")
    c = block.get("ct", "")
    if isinstance(c, str):
        inner = c
    elif isinstance(c, list):
        inner = "".join(_render_inlines(c) if isinstance(ch, dict) and ch.get("nt") not in ("he", "hb") else (_render_block(ch) if isinstance(ch, dict) and ch.get("nt") == "he" else _render_inlines([ch])) for ch in c)
    else:
        inner = ""
    return f"<{t}>{inner}</{t}>"


def _render_children(block) -> str:
    """渲染容器内子 block。"""
    parts = []
    for child in block.get("ct", []):
        if isinstance(child, dict):
            r = _render_block(child)
            if r:
                parts.append(r)
    return "\n".join(parts)


def _render_bq(block) -> str:
    """<blockquote>——空引用只占一行（多余空行会让 CM 比对多出一个换行）。"""
    inner = _render_children(block)
    return f"<blockquote>\n{inner}\n</blockquote>" if inner else "<blockquote>\n</blockquote>"


def _render_list_items(container) -> str:
    """渲染 bullet_list / ordered_list 内的 item（形态见 _render_item）。"""
    items = []
    for child in container.get("ct", []):
        if isinstance(child, dict) and child.get("nt") in ("b", "ol"):
            items.append(_render_item(child))
    return "\n".join(items)


def _render_inlines(inlines) -> str:
    """渲染 inline 节点列表。

    遇到嵌套 list（块内容误挂在行内位）递归拼接，避免 list 直接进 join 抛 TypeError。
    """
    result = []
    for node in (inlines or []):
        if isinstance(node, str):
            result.append(node)
        elif isinstance(node, list):
            result.append(_render_inlines(node))
        elif isinstance(node, dict):
            nt = node.get("nt", "")
            if nt in INLINE_HTML:
                result.append(INLINE_HTML[nt](node))
            elif nt in BLOCK_HTML:
                result.append(_render_block(node))
            elif isinstance(node.get("ct", ""), str):
                result.append(node.get("ct", ""))
    return "".join(result)


# 嵌套 AST → HTML（ast_parse_nested 用）

def ast_nested_to_html(ast) -> str:
    """Convert nested AST to HTML。现在和 flat 相同（统一 nested 格式）。"""
    return ast_to_html(ast)
