"""
File system operations: rm, mk, ls
"""

import json
import posixpath
from datetime import datetime
from typing import List, Dict, Any, Tuple, Optional

from mdio_py.core.selector_builder import SelectorBuilder
from mdio_py.core.error import is_error_text

# 全局调试标志（可通过环境变量控制）
DEBUG_LS = True
def debug_print(*args):
    if DEBUG_LS:
        print("[LS DEBUG]", *args)

async def rm(workspace, pipe_data, args, kwargs, ctx):
    """
    Remove file or directory.
    
    Syntax: rm [-r] [-f] [-v] <path>
    """
    # print(f"[DEBUG mk] pipe_data={pipe_data!r}, args={args!r}, kwargs={kwargs!r}")
    # 提取路径：优先 -p，其次 args[0]，再 pipe_data，最后从 kwargs 值中找第一个字符串
    path = kwargs.get("-p")
    if path is None and args:
        path = args[0]
    if path is None and pipe_data is not None:
        path = pipe_data
    if path is None:
        for k, v in kwargs.items():
            if isinstance(v, str) and v.strip():
                path = v
                break
    if not path:
        return "- mdio_error: [rm] empty path"
    
    # 提取标志
    recursive = "-r" in kwargs
    force = "-f" in kwargs
    verbose = kwargs.get("-v", True)
    if isinstance(verbose, str):
        verbose = verbose.lower() not in ("false", "0", "no")
    
    vfs_path = ctx.resolve_path(str(path))
    
    try:
        records = workspace.store.rm(ctx.wt_uid, vfs_path, recursive, force, verbose)
        if verbose and records:
            return "\n".join(f"- mdio_success: {r}" for r in records)
        return ""
    except Exception as e:
        if force and "not found" in str(e).lower():
            return ""
        return f"- mdio_error: [rm] {e}"


async def mk(workspace, pipe_data, args, kwargs, ctx):
    """
    Create directory or .md file.
    
    Syntax: mk [-f] [-v] <path>
    """
    # print(f"[DEBUG mk] pipe_data={pipe_data!r}, args={args!r}, kwargs={kwargs!r}")
    # 提取路径：优先 -p，其次 args[0]，再 pipe_data，最后从 kwargs 值中找第一个字符串
    path = kwargs.get("-p")
    if path is None and args:
         # 过滤掉以 '-' 开头的标志参数（防止 "-f" 被当作路径）
        for a in args:
            if isinstance(a, str) and a.strip() and not a.startswith('-'):
                path = a
                break
    if path is None and pipe_data is not None:
        path = pipe_data
    if path is None:
        for v in kwargs.values():
            if isinstance(v, str) and v.strip() and not v.startswith('-'):
                path = v
                break
    if not path:
        return "- mdio_error: [mk] empty path"
    
    # 提取标志：-f 存在即为 True（无论其值是字符串还是布尔）
    parents = "-f" in kwargs
    verbose = kwargs.get("-v", True)
    if isinstance(verbose, str):
        verbose = verbose.lower() not in ("false", "0", "no")
    
    vfs_path = ctx.resolve_path(str(path))
    
    try:
        records = workspace.store.mk(ctx.wt_uid, vfs_path, parents, verbose)
        if verbose and records:
            return "\n".join(f"- mdio_success: {r}" for r in records)
        return f"- mdio_info: [mk] '{vfs_path}' already exists"
    except Exception as e:
        return f"- mdio_error: [mk] {e}"

# ── shell 化 ls 的位置参数归一化 ──
# 新语法：LS_CMD → transformer 已按 shell 词法切成 str 路径列表；
# 兼容旧 IR：path_prefix 产物是 [(token_type, text), ...]，这里拼回字符串。
def _ls_flatten_paths(value, out):
    if value is None:
        return
    if isinstance(value, str):
        # 显式空操作数（`ls ""`）必须保留成空路径交给 _ls_one 报错，
        # 不能在这里 quiet-skip（否则会被当成「无操作数 = 当前目录」）。
        out.append(value)
        return
    if isinstance(value, (list, tuple)):
        if value and all(isinstance(i, (tuple, list)) and len(i) == 2
                         and isinstance(i[0], str) for i in value):
            text = "".join(str(i[1]) for i in value)
            if text.strip():
                out.append(text)
            return
        for item in value:
            _ls_flatten_paths(item, out)
        return
    out.append(str(value))


def _ls_flags(kwargs):
    """短旗合并：{'-l': True} / {'-la': True} 都摊成 {'l','a'}（长旗忽略）。"""
    flags = set()
    for key in kwargs:
        if not isinstance(key, str) or not key.startswith("-") or key.startswith("--"):
            continue
        flags.update(key[1:])
    return flags


def _ls_extract_paths(args, kwargs, pipe_data):
    """位置参数 → 路径操作数列表（shell：无操作数 = 当前目录）。"""
    paths = []
    _ls_flatten_paths(args, paths)
    if not paths:
        pflag = kwargs.get("-p")     # 旧式 `-p <path>` 兼容
        if isinstance(pflag, str) and pflag.strip():
            paths.append(pflag)
    if not paths and pipe_data is not None:
        text = str(pipe_data).strip()
        if text:
            paths.append(text)
    return paths or ["."]


async def ls(workspace, pipe_data, args, kwargs, ctx):
    """
    List directory contents — shell 化体验。

    Syntax: ls [-alRrtd1] [--] [path...]
      - flags 可前/中/后、可合并（-la）、可空格分隔；`--` 之后全为路径
      - 路径：绝对 / 相对（`.`、`..`、`dir`）/ 跨 wt（`uid::/path`、`-p uid::/path`）
      - 多路径逐条输出；某条不存在只报该条错误，不影响其它条
      - 不支持长选项（`--help` 等）：显式报错，不静默当 flag 吞掉
    """
    # 长选项（--help / --all …）不在 ls 的能力面内：显式报错，避免
    # 「敲了 --help 却得到一份目录列表」的静默错义。`--`（end of options）
    # 已在 transformer 里消费，不会进 kwargs。
    long_opts = sorted(k for k in kwargs
                       if isinstance(k, str) and k.startswith("--"))
    if long_opts:
        return (f"- mdio_error: [ls] 不支持的长选项 "
                f"{', '.join(repr(x) for x in long_opts)}"
                f"（用法: ls [-laRrtd1] [--] [path...]）")

    flags = _ls_flags(kwargs)

    show_all    = "a" in flags
    long_format = "l" in flags
    sort_time   = "t" in flags
    reverse     = "r" in flags
    recursive   = "R" in flags
    dir_self    = "d" in flags

    paths = _ls_extract_paths(args, kwargs, pipe_data)
    multi = len(paths) > 1
    # 多操作数或显式 -1 时，非长格式一条一行（shell 非 tty 行为）
    one_per_line = multi or "1" in flags

    success_entries = []   # [(is_dir_block, text)]
    error_lines = []
    for raw_path in paths:
        text, is_dir_block = await _ls_one(
            workspace, raw_path,
            show_all=show_all, long_format=long_format, sort_time=sort_time,
            reverse=reverse, recursive=recursive, dir_self=dir_self,
            one_per_line=one_per_line, ctx=ctx)
        if is_error_text(text):
            error_lines.append(text)
            continue
        if multi and is_dir_block:
            text = f"{raw_path}:" + (f"\n{text}" if text else "")
        if text:
            success_entries.append((is_dir_block, text))
    if multi:
        # GNU 风格：非目录操作数先列、目录块后列（同 `ls file dir`）。
        # 这同时消掉一个歧义：空目录块只有 `path:` 头，若紧跟着下一个
        # 操作数的文件名，会看起来像该文件的父目录。
        ordered = ([t for d, t in success_entries if not d]
                   + [t for d, t in success_entries if d])
    else:
        ordered = [t for _d, t in success_entries]
    # 逐路径错误：成功的先列，错误行统一殿后（全部失败时首行即错误→错误协议成立）
    return "\n".join(ordered + error_lines)


async def _ls_one(workspace, raw_path, *, show_all, long_format, sort_time,
                  reverse, recursive, dir_self, one_per_line, ctx):
    """单个操作数的 ls（原实现主体）。返回 (输出文本, 是否目录块)。"""
    raw_path_str = str(raw_path).strip()
    if raw_path_str == "":
        # shell 语义：空路径是非法操作数，报错而不是静默列出当前目录。
        return "- mdio_error: [ls] '' No such file or directory", False

    target_wt = ctx.wt_uid
    from mdio_py.core import split_selector_addr
    addr = split_selector_addr(raw_path_str, wt_first=True)   # 统一切分入口（ls 文档化约定「uid::/path」）
    if addr.wt is not None:
        target_wt = addr.wt.lstrip("%")   # 剥离可选的宏标识
        raw_path_str = addr.file_path or "/"
        # 跨越时空后，如果路径非绝对路径，强制补齐根节点，防止带入原时空的孤儿 cwd
        if not raw_path_str.startswith("/"):
            raw_path_str = "/" + raw_path_str

    vfs_path = ctx.resolve_path(raw_path_str)

    # 3. 拉取全部平层节点（加入异常守卫，防备越权死锁及死链）
    try:
        tree_bytes = workspace.store.get_vfs_tree(target_wt, show_all)
        if not tree_bytes:
            return "", True
        all_files = json.loads(tree_bytes.decode('utf-8'))
    except Exception as e:
        return f"- mdio_error: [ls] target worktree access failed: {e}", False

    # 4. 树形收束器：将扁平文件归纳为目标视角的层级
    target_dir = vfs_path if vfs_path.endswith('/') else vfs_path + '/'
    entries = {} # path -> {"name": ..., "path": ..., "is_dir": ...}
    is_exact_file = False

    for node in all_files:
        f_path = node['path']
        
        # 命中确切的单体文件
        if f_path == vfs_path and not vfs_path.endswith('/'):
            entries[vfs_path] = {"name": posixpath.basename(vfs_path), "path": vfs_path, "is_dir": False}
            is_exact_file = True
            continue

        if not f_path.startswith(target_dir):
            continue

        # 截取相对部分
        rel_path = f_path[len(target_dir):]
        parts = rel_path.split('/')

        if recursive:
            # 递归模式：沿途文件夹也补充进显示字典
            curr = target_dir
            for part in parts[:-1]:
                curr += part + '/'
                if curr not in entries:
                    entries[curr] = {"name": part, "path": curr, "is_dir": True}
            entries[f_path] = {"name": parts[-1], "path": f_path, "is_dir": False}
        else:
            # 非递归模式：截断为下级第一层
            top_level = parts[0]
            is_dir_entry = len(parts) > 1
            entry_path = target_dir + top_level + ('/' if is_dir_entry else '')
            if entry_path not in entries:
                entries[entry_path] = {"name": top_level, "path": entry_path, "is_dir": is_dir_entry}

    # 异常情况：既不是文件，并且底层也确认不存在该目录名
    if not entries and not workspace.store.path_exists(target_wt, target_dir):
        return f"- mdio_error: [ls] '{raw_path_str}' No such file or directory", False
        
    # 如果指定了 -d 且查的是目录，则只展示首级节点自己
    if dir_self and not is_exact_file:
        name = posixpath.basename(target_dir.rstrip('/')) or "/"
        entries = {target_dir: {"name": name, "path": target_dir, "is_dir": True}}

    # 5. 过滤隐藏文件与提纯
    final_entries = []
    for info in entries.values():
        if not show_all and info["name"].startswith('.'):
            continue
        final_entries.append(info)

    # 6. 元数据挂载 (仅在使用了长格式/时间排序时请求到底层)
    if long_format or sort_time:
        for e in final_entries:
            if e["is_dir"]:
                e.update({"type": "DR", "chars": 0, "mtime": 0, "tokens": 0, "perm": "RW"})
                if long_format:
                    try:
                        _, _, is_ro = workspace.store.get_file_metadata(target_wt, e["path"])
                        e["perm"] = "RO" if is_ro else "RW"
                    except Exception: pass
            else:
                e["type"] = "MD"
                try:
                    size, mtime, is_ro = workspace.store.get_file_metadata(target_wt, e["path"])
                    e.update({"chars": size, "mtime": mtime, "perm": "RO" if is_ro else "RW"})
                except Exception:
                    e.update({"chars": 0, "mtime": 0, "perm": "RW"})
                
                if long_format:
                    try:
                        tree_json = workspace.run_tree(target_wt, e["path"], SelectorBuilder().build(), 1, 1)
                        resp = json.loads(tree_json)
                        e["tokens"] = resp.get("file_tokens", 0)
                        e["chars"] = resp.get("file_chars", e.get("chars", 0))
                    except Exception:
                        e["tokens"] = 0

    # 7. 排序输出
    # 多操作数时 shell 风格：目录操作数块带上 `path:` 头（错误行由上层统一处理）
    is_dir_block = not is_exact_file and not dir_self
    if sort_time:
        final_entries.sort(key=lambda x: x.get("mtime", 0), reverse=not reverse)
    elif recursive:
        # 递归输出是扁平路径列表：按完整 path 排序（旧实现按 basename 排，
        # 会出现 /share/notes.md 排在 /readme.md 前后之类的反直觉顺序）。
        final_entries.sort(key=lambda x: x["path"].lower(), reverse=reverse)
    else:
        final_entries.sort(key=lambda x: x["name"].lower(), reverse=reverse)

    if long_format:
        lines = []
        for e in final_entries:
            dt = datetime.fromtimestamp(e["mtime"]).strftime("%Y-%m-%d %H:%M") if e.get("mtime") else "----/--/-- --:--"
            display_name = e["path"] if recursive else e["name"]
            
            lines.append(
                f"- {e.get('perm', 'RW')} {e['type']} "
                f"{e.get('tokens', 0):<5} {e.get('chars', 0):<7} {dt} {display_name}"
            )
        return "\n".join(lines), is_dir_block
    if recursive:
        return "\n".join(f"- {e['path']}" for e in final_entries), is_dir_block
    names = [e["name"] for e in final_entries]
    ambiguous = any((" " in n) or ("\t" in n) for n in names)
    if one_per_line or ambiguous:
        # 多操作数（类似 shell 非 tty 的 `ls -1`）：一条一行，避免块间混淆。
        # 文件名含空白时也必须一条一行：否则空格分隔的输出无法无歧义切分
        # （例如 "a b.md" 会被读成两个条目）。
        return "\n".join(names), is_dir_block
    return " ".join(names), is_dir_block


async def reload(workspace, pipe_data, args, kwargs, ctx):
    """以磁盘为准重载 worktree:清理磁盘已消失的镜像 + 重泵磁盘现存文件。
    Syntax: reload [wt]   缺省当前 WT;wt 支持 uid 或 worktrees.md 中的 name。
    语义:以磁盘为准,内存中未落盘的编辑会被丢弃(返回中列出)。"""
    wt = kwargs.get("-p")
    if wt is None and args:
        for a in args:
            # 裸位置参数被解析为嵌套 FuncCall(与 worktree bfork 同款),按 name 解包
            if hasattr(a, "name"):
                s = str(a.name).strip()
            else:
                s = str(a).strip()
            if s and not s.startswith("-"):
                wt = s
                break
    if wt is None:
        wt = ctx.wt_uid
    wt = str(wt).strip()
    # name 优先匹配 worktrees.md 条目(8 字符 name 如 MDIO_NEO 不能当 uid 直用);
    # 无同名条目时原样透传(视为 uid,store 侧报错更准确)
    from mdio_py.std.worktree_ops import _parse_worktree_entries
    for e in _parse_worktree_entries(workspace):
        if e.get("name") == wt:
            wt = e.get("uid", wt)
            break
    try:
        records = workspace.store.reload(wt)
    except Exception as e:
        return f"- mdio_error: [reload] {e}"
    # reload 摘掉该 WT 全部文档的 session（store.rs reload：sessions.remove）⇒ **血统全换**。
    # 对仍有订阅者的文档主动推换代；否则订阅端会在旧血统上永久静默。
    try:
        from mdio_py.gateway.crdt_scope import push_relineage
        gws = getattr(workspace, "_gateway_store", None)
        scope = gws.get_scope(wt) if gws is not None else None
        if scope is not None:
            # 步骤 2：lookup 优先用 GatewayStore；doc 列表仍按该 wt 的分域索引取（reload 是 wt 级操作）
            await push_relineage(workspace.store, gws or scope,
                                 list(scope.subscriptions.keys()), reason="reload")
    except Exception as e2:
        print(f"⚠️ [reload] 换代推送失败: {e2!r}", flush=True)
    if not records:
        return "- mdio_success: reload ok (no changes)"
    return "\n".join(f"- mdio_success: {r}" for r in records)
