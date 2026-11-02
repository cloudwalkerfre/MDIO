import json
import re
import posixpath
from mdio_py.core import compile_selector_text, split_selector_addr
from mdio_py.core.text_utils import looks_like_file
from .utils import format_tree_node
from mdio_py.core.selector_builder import SelectorBuilder, by_tag

import jieba
import bm25s

# 通用 VFS 目标解析算子 (Parser)
def resolve_vfs_target(target_locator: str, ctx) -> tuple[str, str, dict]:
    """
    统一解析管道目标，返回 (绝对虚拟路径通配符, md_path部分, 编译出的 IR_DICT)
    """
    target_locator = str(target_locator).strip()
    addr = split_selector_addr(target_locator)   # 统一切分入口（1/3 段 :: 走语法权威）
    if addr.file_path is not None:
        raw_file, raw_md = addr.file_path, addr.md_path
    else:
        # 无 :: 时按指纹判定
        if looks_like_file(target_locator) or target_locator.startswith("~"):   # 路径指纹判定
            raw_file, raw_md = target_locator, ""
        elif ctx.active_doc:
            # 存在文件作用域，缺省指代在原文件里搜 DOM
            raw_file, raw_md = ctx.active_doc, target_locator
        else:
            raw_file, raw_md = target_locator, ""
    abs_file_pat = ctx.resolve_path(raw_file)
    # 目录探测：自动追加通配符，防止 tree -p / 查无此物
    if abs_file_pat.endswith("/"):
        abs_file_pat += "*"
    elif abs_file_pat == "" or abs_file_pat == ".":
        abs_file_pat = "/*"
    elif "." not in abs_file_pat.split("/")[-1] and "*" not in abs_file_pat and "?" not in abs_file_pat:
        # 没有扩展名、没有星号，被视为扫描目录：比如 /logs
        abs_file_pat += "/*"
    # 主动构造 AST 的 Dict
    if raw_md.strip():
        sel = compile_selector_text(raw_md, on_error="drill_path")   # drill_path 回落 = 按 . 拆 Drill
    else:
        sel = SelectorBuilder().build()
    return abs_file_pat, raw_md, sel

# def resolve_vfs_target(target_locator: str, ctx) -> tuple[str, str, str]:
#     """
#     统一解析管道目标，返回 (绝对虚拟路径通配符, md_path部分, 编译出的 IR_JSON)
#     """
#     target_locator = str(target_locator).strip()

#     if "::" in target_locator:
#         raw_file, raw_md = target_locator.split("::", 1)
#     else:
#         if ctx.active_doc:
#             # 存在文件作用域，缺省指代在原文件里搜 DOM
#             raw_file, raw_md = ctx.active_doc, target_locator
#         else:
#             # 没有文件作用域（如纯 "/" | tree），则缺省指代文件通配模式
#             raw_file, raw_md = target_locator, ""

#     # 使用新的上下文明气绝对洗消 (算出如 /docs/*.md)
#     abs_file_pat = ctx.resolve_path(raw_file)

#     # 主动构造 AST 的 JSON
#     if raw_md.strip():
#         ir = parse_code(f"${raw_md}")
#         sel = ir[0].atoms[0].selector
#         # ir_json = json.dumps(sel.__dict__, ensure_ascii=False)
#         # ir_json = json.dumps(asdict(sel), ensure_ascii=False)
#         ir_dict = asdict(sel)
#     else:
#         # 查全文件不需要过滤器
#         # ir_json = json.dumps({"md_path": "", "filters": [], "key": None}, ensure_ascii=False)
#         ir_dict = {"md_path": "", "filters": [], "key": None}

#     return abs_file_pat, raw_md, ir_dict

# TREE 指令
async def tree(workspace, pipe_data, args, kwargs, ctx):
    depth = kwargs.get("-d", args[0] if args else 3)
    detail = kwargs.get("-detail", args[1] if (len(args)>1) else 1)
    
    show_hidden = "-a" in kwargs
    show_head = kwargs.get("-head", True)
    if isinstance(show_head, str):
        show_head = show_head.lower() == "true"
    show_key = kwargs.get("-key", False)
    if isinstance(show_key, str):
        show_key = show_key.lower() == "true"

    # 冲突检查
    if not show_head and show_key:
        return "- mdio_error: [tree] Conflict: -head false conflicts with -key true"

    target_locator = kwargs.get("-p")
    if not target_locator:
        if pipe_data is not None:
            target_locator = pipe_data
        elif len(args) > 2:
            target_locator = args[2]
        else:
            target_locator = ctx.active_doc if ctx.active_doc else "/"

    if not isinstance(target_locator, str) or not target_locator:
        target_locator = "/"

    try:
        abs_file_pat, raw_md, sel = resolve_vfs_target(target_locator, ctx)
    except Exception as e:
        return f"- mdio_error: [tree] parse failed, err: {e}"

    try:
        file_list = workspace.expand_path_pattern(
            ctx.wt_uid, abs_file_pat, include_hidden=show_hidden
        )
    except Exception as e:
        return f"- mdio_error: {str(e)}"

    if not file_list:
        return "- tree: no items found"

    output_lines = ["- tree:"]
    for vfs_path in file_list:
        try:
            ret_json = workspace.run_tree(ctx.wt_uid, vfs_path, sel, int(depth), int(detail))
            resp = json.loads(ret_json)
            if not resp: continue
            
            nodes = resp.get("nodes", [])
            if not nodes: continue
            
            file_node_repr = {
                "node_type": "file", 
                "name": posixpath.basename(vfs_path),
                "chars": resp.get("file_chars", 0),
                "tokens": resp.get("file_tokens", 0),
                "lines": resp.get("file_lines", 0),
                "children": nodes,
            }
            output_lines.extend(format_tree_node(file_node_repr, prefix="    ", is_last=True, is_root=True))
            
        except Exception as e:
            output_lines.append(f"    📄 {posixpath.basename(vfs_path)} (Tree Build Error: {e})")

    return "\n".join(output_lines) if len(output_lines) > 1 else "- tree: no items found"

# async def tree(workspace, pipe_data, args, kwargs, ctx):
#     depth = kwargs.get("-d", args[0] if args else 3)
#     detail = kwargs.get("-detail", args[1] if (len(args)>1) else 1)
    
#     target_locator = kwargs.get("-p")
#     if not target_locator:
#         if pipe_data is not None:
#              target_locator = pipe_data
#         elif len(args) > 2:
#              target_locator = args[2]
#         else:
#              # 🌟 修复：如果它发疯地裸调 `tree`，默认帮它查全宇宙 "/"
#              target_locator = ctx.active_doc if ctx.active_doc else "/"

#     # if not isinstance(target_locator, str) or not target_locator:
#     #     return "- error: [tree] needs a valid path locator"
#     if not isinstance(target_locator, str) or not target_locator:
#         target_locator = "/"

#     try:
#         # 🌟 统一调用通用解析器
#         abs_file_pat, raw_md, ir_dict = resolve_vfs_target(target_locator, ctx)
#     except Exception as e:
#         return f"- error: [tree] parse failed, err: {e}"

#     try:
#         file_list = workspace.expand_path_pattern(ctx.wt_uid, abs_file_pat)
#     except Exception as e:
#         return f"- error: {str(e)}"
        
#     if not file_list:
#         return "- tree: no items found"

#     output_lines = ["- tree:"]
#     for vfs_path in file_list:
#         try:
#             ret_json = workspace.run_tree(ctx.wt_uid, vfs_path, ir_dict, int(depth), int(detail))
#             resp = json.loads(ret_json)
#             if not resp: continue
            
#             nodes = resp.get("nodes", [])
#             if not nodes: continue
            
#             file_node_repr = {
#                 "node_type": "file", 
#                 "name": posixpath.basename(vfs_path), # 摒弃 Path.name
#                 "chars": resp.get("file_chars", 0), "tokens": resp.get("file_tokens", 0),
#                 "lines": resp.get("file_lines", 0), "children": nodes,
#             }
#             output_lines.extend(format_tree_node(file_node_repr, prefix="    ", is_last=True, is_root=True))
            
#         except Exception as e:
#             output_lines.append(f"    📄 {posixpath.basename(vfs_path)} (Tree Build Error: {e})")

#     return "\n".join(output_lines) if len(output_lines) > 1 else "- tree: no items found"


# BM25 全文检索（find -bm25 分支）

_CURRENT_TOKENIZER = "v1"
# per-WT cache, keyed by wt_uid.
_cache_states: dict[str, dict[str, tuple[str, list[str]]]] = {}
_cache_dirty_flags: dict[str, bool] = {}
_cache_tokenizers: dict[str, str | None] = {}


def _parse_find_cache(workspace, wt_uid: str) -> tuple[dict, str | None]:
    """Parse find.md cache via selector（条目 kv 走 tojson 信封 + bullets_to_kv）。"""
    from mdio_py.core.selector_builder import SelectorBuilder, by_tag
    from mdio_py.core.ast_utils import bullets_to_kv
    # read tokenizer
    tokenizer = None
    try:
        chunks = workspace.read_locator(
            wt_uid, "/.mds/cache/find.md",
            by_tag("FIND_CACHE", "tokenizer"),
            proj="content",
        )
        if chunks:
            tokenizer = str(chunks[0]).strip()
    except Exception:
        pass

    # read all file entries
    state = {}
    try:
        ir = SelectorBuilder().drill("FIND_CACHE").any_child().build()
        children = workspace.read_locator(wt_uid, "/.mds/cache/find.md", ir)
    except Exception:
        return state, tokenizer

    for chunk in children:
        lines = str(chunk).split("\n")
        if not lines:
            continue
        path = lines[0].strip().lstrip("#").strip()
        body = "\n".join(lines[1:])
        if not body.endswith("\n"):
            # 防御性归一：裸 to_json_text 在末行是 `- k:`（冒号结尾且无换行）时会投影越界
            # panic（实测 byte index out of bounds；PanicException 不是 Exception 子类，兜不住）。
            # VFS 读回的 chunk 目前恒以换行结尾（store 落盘即归一），对 kv 提取无影响。
            body += "\n"
        kv = bullets_to_kv(json.loads(workspace.to_json_text(body)))
        doc_hash = kv.get("doc_hash") or None
        # 空 `- tokens:` 行：解析后匹配不到 "- tokens: " 前缀 ⇒ 整条跳过；
        # 空 tokens 一旦入缓存，find 的命中分支会以零 token 进索引。
        tokens = [t for t in kv["tokens"].split(", ") if t] if kv.get("tokens") else None
        if path and doc_hash and tokens is not None:
            state[path] = (doc_hash, tokens)
    return state, tokenizer


def _format_find_cache(state: dict) -> str:
    out = ["# FIND_CACHE", f"- tokenizer: {_CURRENT_TOKENIZER}\n"]
    for path in sorted(state):
        doc_hash, tokens = state[path]
        out.append(f"## {path}")
        out.append(f"- doc_hash: {doc_hash}")
        out.append(f"- tokens: {', '.join(tokens)}")
        out.append("")
    return '\n'.join(out)


def _locate_hits(text: str, query_tokens: list[str]) -> list[tuple[int, int, str]]:
    hits = []
    lower = text.lower()
    for tok in query_tokens:
        if len(tok) < 2:
            continue
        pos = 0
        while True:
            pos = lower.find(tok.lower(), pos)
            if pos == -1:
                break
            hits.append((lower[:pos].count('\n'), pos, tok))
            pos += 1
    return hits


def _snippet_at_pos(text: str, char_pos: int, term_len: int, ctx: int = 40) -> str:
    start = max(0, char_pos - ctx)
    end = min(len(text), char_pos + term_len + ctx)
    snip = text[start:end].replace('\n', ' ').replace('\r', ' ')
    if start > 0:
        snip = "..." + snip
    if end < len(text):
        snip = snip + "..."
    return snip.strip()


def _strip_md(text: str) -> str:
    text = re.sub(r'```[\s\S]*?```', ' ', text)
    text = re.sub(r'^[#\-*>|]+', '', text, flags=re.MULTILINE)
    text = re.sub(r'\[([^\]]+)\]\([^)]+\)', r'\1', text)
    return text


def _split_cjk(text: str) -> list[tuple[str, bool]]:
    parts = []
    pos = 0
    for m in re.finditer(r'[一-鿿㐀-䶿豈-﫿]+', text):
        if m.start() > pos:
            parts.append((text[pos:m.start()], False))
        parts.append((m.group(), True))
        pos = m.end()
    if pos < len(text):
        parts.append((text[pos:], False))
    return parts


def _tokenize(text: str) -> list[str]:
    text = _strip_md(text)
    tokens = []
    for chunk, is_cjk in _split_cjk(text):
        if is_cjk:
            tokens.extend(jieba.lcut(chunk))
        else:
            tokens.extend(re.findall(r'[a-zA-Z0-9_]+', chunk.lower()))
    return [t.strip() for t in tokens if t.strip()]


async def _bm25_search(workspace, query: str, scope: str, kwargs: dict, ctx) -> str:
    from mdio_py.mdio_core import snap_state_hash

    n = int(kwargs.get("-n", 10))

    if not scope:
        return "- mdio_error: [bm25] needs -p scope"

    try:
        abs_file_pat, raw_md, sel = resolve_vfs_target(scope, ctx)
    except Exception as e:
        return f"- mdio_error: [bm25] parse selector failed: {e}"

    try:
        file_list = workspace.expand_path_pattern(ctx.wt_uid, abs_file_pat)
    except Exception as e:
        return f"- mdio_error: {e}"

    file_list = [f for f in file_list if not f.endswith('/')]
    if not file_list:
        return "- bm25: no documents found"

    # Step 2 — load cache from current wt_uid
    wt_uid = ctx.wt_uid
    _cache_state = _cache_states.get(wt_uid, {})
    _cache_tokenizer = _cache_tokenizers.get(wt_uid)
    if _cache_tokenizer is None:
        try:
            _cache_state, _cache_tokenizer = _parse_find_cache(workspace, wt_uid)
        except Exception:
            _cache_state = {}
            _cache_tokenizer = None
        _cache_states[wt_uid] = _cache_state
        _cache_tokenizers[wt_uid] = _cache_tokenizer

    force_rebuild = _cache_tokenizer != _CURRENT_TOKENIZER
    _cache_dirty = _cache_dirty_flags.get(wt_uid, False) or force_rebuild

    # Step 3 — validate & rebuild per-file
    all_tokens = []
    all_paths = []
    all_texts = {}
    drill_path = '.'.join(s[1] for s in sel.steps if s[0] == "Drill")

    for vfs_path in file_list:
        try:
            text = workspace.store.get_text(ctx.wt_uid, vfs_path)
        except Exception:
            continue
        if not text or not text.strip():
            continue

        current_hash = snap_state_hash(text)

        if not force_rebuild:
            cached = _cache_state.get(vfs_path)
            if cached and cached[0] == current_hash:
                all_tokens.append(cached[1])
                all_paths.append(vfs_path)
                all_texts[vfs_path] = text
                continue

        tokens = _tokenize(text)
        if not tokens:
            continue
        all_tokens.append(tokens)
        all_paths.append(vfs_path)
        all_texts[vfs_path] = text
        _cache_state[vfs_path] = (current_hash, tokens)
        _cache_dirty = True

    if not all_tokens:
        return "- bm25: all documents are empty"

    # Step 4 — build index & query
    retriever = bm25s.BM25(method="lucene")
    retriever.index(all_tokens)

    query_tokens = _tokenize(query)
    if not query_tokens:
        return "- bm25: no relevant documents found"

    results, scores = retriever.retrieve([query_tokens], k=min(n, len(all_paths)), corpus=all_paths)

    # Step 5 — persist to current wt_uid if dirty
    if _cache_dirty:
        try:
            md = _format_find_cache(_cache_state)
            workspace.write_locator(wt_uid, "/.mds/cache/find.md",
                                   SelectorBuilder().build(), "/>", md, god=True)
            _cache_states[wt_uid] = _cache_state
            _cache_tokenizers[wt_uid] = _CURRENT_TOKENIZER
            _cache_dirty_flags[wt_uid] = False
        except Exception:
            pass

    # Step 6 — display
    doc_list = results[0]
    score_list = scores[0]
    if len(score_list) == 0 or score_list.max() <= 0:
        return "- bm25: no relevant documents found"

    lines = []
    for i in range(len(doc_list)):
        vfs_path = doc_list[i]
        doc_str = vfs_path.get("text", vfs_path) if isinstance(vfs_path, dict) else str(vfs_path)
        score = float(score_list[i])
        if score <= 0:
            break

        text = all_texts.get(doc_str, "")
        if not text:
            continue

        hits = _locate_hits(text, query_tokens)
        seen_headings = set()
        line_headings = {}
        for line_no, char_pos, matched_term in hits:
            if line_no not in line_headings:
                try:
                    line_headings[line_no] = workspace.heading_at_line(ctx.wt_uid, doc_str, line_no)
                except Exception:
                    line_headings[line_no] = ""
            heading = line_headings[line_no]

            if drill_path and not heading.startswith(drill_path):
                continue

            if heading in seen_headings:
                continue
            seen_headings.add(heading)

            snippet = _snippet_at_pos(text, char_pos, len(matched_term))
            file_name = posixpath.basename(doc_str)
            sel_display = f"{file_name}::{heading}" if heading else file_name
            lines.append(f"- path: {sel_display}, {json.dumps(snippet, ensure_ascii=False)}")
            if len(lines) >= n:
                break
        if len(lines) >= n:
            break

    return '\n'.join(lines) if lines else "- bm25: no relevant documents found"


# GREP 指令
async def find(workspace, pipe_data, args, kwargs, ctx):
    # 1. 编译正则查询参数等保持不变
    raw_queries = kwargs.get("-q", args[0] if args else [])
    if not raw_queries and "-bm25" not in kwargs:
        return "- mdio_error: [find] missing queries"
    if isinstance(raw_queries, str): raw_queries = [raw_queries]

    ignore_case = kwargs.get("-i", False)
    ctx_b_val, ctx_a_val = 0, 0
    ctx_arg = kwargs.get("-c", [20, 20])
    if isinstance(ctx_arg, list) and len(ctx_arg) >= 2:
        ctx_b_val, ctx_a_val = int(ctx_arg[0]), int(ctx_arg[1])
    elif isinstance(ctx_arg, int):
        ctx_b_val = ctx_a_val = ctx_arg

    query_configs = []
    for q in raw_queries:
        q_str = str(q).strip()
        is_re = False
        match = re.match(r'^/(.*?)/([a-z]*)$', q_str)
        if match:
            q_str, flags = match.group(1), match.group(2)
            is_re = True
            if 'i' in flags: ignore_case = True
        query_configs.append((q_str, is_re, ignore_case))

    if "-bm25" in kwargs:
        bm25_val = kwargs.get("-bm25")
        query = raw_queries[0] if raw_queries else ""
        if not query and bm25_val and bm25_val is not True:
            query = str(bm25_val)
        return await _bm25_search(workspace, query=query,
                                  scope=kwargs.get("-p"), kwargs=kwargs, ctx=ctx)

    # 2. 定位锚点
    target_locator = kwargs.get("-p")
    if not target_locator:
        if pipe_data is not None:
             target_locator = pipe_data
        elif len(args) > 1:
             target_locator = args[1]
             
    if not target_locator:
        return "- mdio_error: [find] needs a target (-p) or pipeline data"

    # 如果是处理内存中的数据，不涉及 VFS
    if isinstance(target_locator, (list, dict)):
        results = []
        items = target_locator if isinstance(target_locator, list) else [target_locator]
        for item in items:
            item_str = str(item)
            for q_str, is_re, q_ic in query_configs:
                if is_re:
                    flags = re.IGNORECASE if q_ic else 0
                    if re.search(q_str, item_str, flags):
                        results.append(item_str)
                        break
                else:
                    if q_ic and q_str.lower() in item_str.lower():
                        results.append(item_str)
                        break
                    elif not q_ic and q_str in item_str:
                        results.append(item_str)
                        break
        return results

    # 统一调用通用解析器
    try:
        abs_file_pat, raw_md, sel = resolve_vfs_target(target_locator, ctx)
    except Exception as e:
        return f"- mdio_error: [find] parse selector failed: {e}"

    try:
        file_list = workspace.expand_path_pattern(ctx.wt_uid, abs_file_pat)
    except Exception as e:
        return f"- mdio_error: {str(e)}"

    if not file_list:
        return "- mdio_error: no matches found"

    # 3. 交由 Rust 下放查询
    all_raw_results = []
    for vfs_path in file_list:
        try:
            results = workspace.run_find(ctx.wt_uid, vfs_path, sel, query_configs, ctx_b_val, ctx_a_val)
            if results:
                file_name = posixpath.basename(vfs_path)
                results = [f"- path: {file_name}::{r.split('path: ')[1]}" if "path: " in r else r for r in results]
            all_raw_results.extend(results)
        except Exception as e:
            all_raw_results.append(f"- error: find failed in {vfs_path}: {e}")

    # 4. 去重合并输出
    unique_results, seen_paths = [], set()
    for r in all_raw_results:
        if r.startswith("- path: "):
            path_key = r.split(",")[0].replace("- path: ", "").strip()
            if path_key not in seen_paths:
                seen_paths.add(path_key)
                unique_results.append(r)
        else:
            unique_results.append(r) 

    return "\n".join(unique_results) if unique_results else "- find: no matches found"
