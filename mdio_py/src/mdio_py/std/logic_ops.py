# mdio_py/src/mdio_py/std/logic_ops.py
import re
from typing import Any, Tuple
from mdio_py.core import compile_selector_text, split_selector_addr
from mdio_py.core.text_utils import looks_like_file
from mdio_py.core.selector_builder import SelectorBuilder

async def test(workspace, pipe_data, args, kwargs, ctx) -> str:
    """
    MDIO test 指令 —— 探针集合

    返回 "true" 或 "false" 字符串，可直接用于 XBT 剧本条件分支。

    支持探针：
      -e/--exists     全域存在性 (mount/folder/file/element)
      -w/--writable   可写性 (存在且绑定 RW)
      -s/--size       非空 (目录有内容/文件>0/节点有值)
      -d/--dir        是否为目录
      -md/--md        是否为 .md 文件
      -wt/--wt        是否为存活 worktree uid
      -z/--empty      空值 (None/空串/空列表/空字典)
      -n/--notempty   非空
      -eq/-ne/-gt/-ge/-lt/-le  数值/字符串比较
      -in/--has       包含关系 (子串/列表元素/字典键)
      -re/--match     正则匹配

    无参数时默认执行 -n (非空检查)
    """
    # 1. 安全提取目标值：只取单目/VFS 探针携带的 Target，防止双目运算符把 Target 当操作数
    target = _extract_target(pipe_data, args, kwargs)

    # 无任何标志 -> 默认判断非空
    if not kwargs:
        return _bool_str(_is_nonempty(target))

    # 2. 一元逻辑标志探针
    if '-z' in kwargs or '--empty' in kwargs:
        return _bool_str(not _is_nonempty(target))
    if '-n' in kwargs or '--notempty' in kwargs:
        return _bool_str(_is_nonempty(target))

    # 3. VFS/AST 物理探针
    vfs_flags = {'-e', '--exists', '-w', '--writable', '-s', '--size',
                 '-d', '--dir', '-md', '--md', '-wt', '--wt'}
    if vfs_flags & kwargs.keys():
        return _bool_str(await _vfs_probe(workspace, target, kwargs, ctx))

    # 4. 二元比较探针（字典路由派发，优雅且高效）
    binary_ops = {
        '-eq': lambda a, b: a == b,
        '-ne': lambda a, b: a != b,
        '-gt': lambda a, b: a > b,
        '-ge': lambda a, b: a >= b,
        '-lt': lambda a, b: a < b,
        '-le': lambda a, b: a <= b,
    }
    for op, func in binary_ops.items():
        if op in kwargs:
            right = kwargs[op]
            return _bool_str(_compare(target, right, func))

    # 5. 包含与正则匹配探针
    if '-in' in kwargs or '-has' in kwargs or '--has' in kwargs:
        right = kwargs.get('-in', kwargs.get('-has', kwargs.get('--has')))
        return _bool_str(_contains(target, right))

    if '-re' in kwargs or '-match' in kwargs or '--match' in kwargs:
        pattern = kwargs.get('-re', kwargs.get('-match', kwargs.get('--match')))
        return _bool_str(await _regex_match(target, pattern))

    # 兜底未知标志
    return "false"



# 通用辅助函数

def _bool_str(b: bool) -> str:
    return "true" if b else "false"

def _is_nonempty(val: Any) -> bool:
    """判断实质非空 (None, 纯空格, 空列表/字典 -> False)"""
    if val is None:
        return False
    if isinstance(val, str):
        return bool(val.strip())
    if isinstance(val, (list, dict)):
        return len(val) > 0
    return True

def _extract_target(pipe_data, args, kwargs) -> Any:
    """基于身份隔离提取目标数据，严防双目运算符劫持 Target"""
    unary_flags = {
        '-z', '--empty', '-n', '--notempty', 
        '-e', '--exists', '-w', '--writable', '-s', '--size',
        '-d', '--dir', '-md', '--md', '-wt', '--wt'
    }
    # 只有单目/VFS探针才有资格从 kwargs 提取随身携带的 Target
    for flag in unary_flags:
        if flag in kwargs:
            v = kwargs[flag]
            if v is not None and not isinstance(v, bool) and str(v).strip() not in ("", "_"):
                return v
                
    # 否则（如双目运算 -eq、裸转等），目标必须来自管道流或匿名参数
    if pipe_data is not None:
        return pipe_data
    if args:
        return args[0]
    return ""



# 物理维度嗅探引擎 (VFS & AST)

async def _vfs_probe(workspace, target, kwargs, ctx) -> bool:
    """处理 VFS/AST 四维降维探针"""
    if '-wt' in kwargs or '--wt' in kwargs:
        return _is_live_wt(workspace, target)

    wt_uid, vfs_path, md_sel, sel = _parse_mdio_path(target, ctx)

    if '-e' in kwargs or '--exists' in kwargs:
        return await _exists(workspace, wt_uid, vfs_path, md_sel, sel)
    if '-w' in kwargs or '--writable' in kwargs:
        return await _writable(workspace, wt_uid, vfs_path, md_sel, sel)
    if '-s' in kwargs or '--size' in kwargs:
        return await _nonempty(workspace, wt_uid, vfs_path, md_sel, sel)
    if '-d' in kwargs or '--dir' in kwargs:
        return await _is_dir(workspace, wt_uid, vfs_path)
    if '-md' in kwargs or '--md' in kwargs:
        return await _is_md_file(workspace, wt_uid, vfs_path)
        
    return False

def _parse_mdio_path(target: Any, ctx) -> Tuple:
    """解析 MDIO 万能坐标，返回 (wt_uid, vfs_path, md_selector_str, IR_Dict)"""
    target_str = str(target).strip()
    wt_uid = ctx.wt_uid
    vfs_path = ""
    md_sel = ""

    # 1. 拆解跨维度前缀（wt_uid 是 uid 字符串，非 u32）—— 统一切分入口
    addr = split_selector_addr(target_str)
    if addr.wt is not None:
        # 三段式 = 语法权威：左段恒为 wt
        wt_uid = addr.wt.lstrip('%')
        vfs_path = addr.file_path or ""
        md_sel = addr.md_path
    elif addr.file_path is not None:
        left = addr.file_path
        prefix = left.lstrip('%')
        # 两段式：左段是 wt 名（genesis / 8 位 uid）才是跨域；否则是普通 file_path::md_path
        if not left.startswith('/') and (prefix == "genesis" or (len(prefix) == 8 and prefix.isalnum())):
            wt_uid = prefix
            vfs_path = addr.md_path
            md_sel = ""
        else:
            vfs_path, md_sel = left, addr.md_path
    else:
        # 指纹判定：带斜杠或点号偏向文件，否则纯 AST 节点
        if looks_like_file(target_str):   # 路径指纹判定
            vfs_path = target_str
        else:
            md_sel = target_str

    if not vfs_path:
        vfs_path = ctx.active_doc if ctx.active_doc else "/"
        
    vfs_path = ctx.resolve_path(vfs_path)

    # 2. 构建 SelectorIR（:key 语法由 Transformer 处理）
    if md_sel:
        sel = compile_selector_text(md_sel, on_error="empty")   # empty 回落 = 空选择器
    else:
        sel = SelectorBuilder().build()
    return wt_uid, vfs_path, md_sel, sel

def _is_live_wt(workspace, target) -> bool:
    """探测孤室宇宙的存在性（wt_uid 是 uid 字符串，非 u32）"""
    s = str(target).lstrip('%').strip()
    if not s:
        return False
    try:
        return workspace.store.has_worktree(s)
    except Exception:
        return False

async def _exists(workspace, wt_uid: str, vfs_path: str, md_sel: str, sel) -> bool:
    if md_sel:
        try: return bool(workspace.read_locator(wt_uid, vfs_path, sel))
        except Exception: return False
    return workspace.path_exists(wt_uid, vfs_path)

async def _writable(workspace, wt_uid: str, vfs_path: str, md_sel: str, sel: dict) -> bool:
    if not await _exists(workspace, wt_uid, vfs_path, md_sel, sel): return False
    try:
        _, _, is_ro = workspace.store.get_file_metadata(wt_uid, vfs_path)
        return not is_ro
    except Exception: return False

async def _nonempty(workspace, wt_uid: str, vfs_path: str, md_sel: str, sel: dict) -> bool:
    """非空探针：文件大小>0 / 目录有内容 / 节点值非空"""
    if md_sel:
        try:
            results = workspace.read_locator(wt_uid, vfs_path, sel)
            return any(r and str(r).strip() for r in results)
        except Exception:
            return False

    if not workspace.path_exists(wt_uid, vfs_path):
        return False

    # 1. 文件夹判定：优先使用 is_dir FFI，避免 normalize_vpath 吞掉尾部 /
    is_directory = False
    if vfs_path.endswith('/'):
        is_directory = True
    else:
        try:
            is_directory = workspace.store.is_dir(wt_uid, vfs_path)
        except Exception:
            pass

    # print(f"🔍 [DEBUG test -s] Path: '{vfs_path}' | Is Dir: {is_directory}")

    if is_directory:
        # 目录的非空判定：不看 size，看目录内文件数
        check_path = vfs_path if vfs_path.endswith('/') else vfs_path + '/'
        
        # 使用不包括隐藏文件（或包括隐藏文件）的探测
        # 一般 test -s 只要里面有任何文件（包含 .开头的）就算非空
        raw_files = workspace.expand_path_pattern(wt_uid, check_path + '*', include_hidden=True)
        
        # 剔除目录自身的匹配（防止 globset 把 /emptydir/ 自己算进去）
        real_files = [f for f in raw_files if f.rstrip('/') != check_path.rstrip('/')]
        
        # print(f"🔍 [DEBUG test -s] Folder pure kids count: {len(real_files)} | raw was: {raw_files}")
        return len(real_files) > 0

    else:
        # 纯文件的非空判定：看底层字节是否大于 0
        try:
            size, _, _ = workspace.store.get_file_metadata(wt_uid, vfs_path)
            # print(f"🔍 [DEBUG test -s] File size: {size}")
            return size > 0
        except Exception:
            return False


async def _is_dir(workspace, wt_uid: str, vfs_path: str) -> bool:
    try:
        return workspace.store.is_dir(wt_uid, vfs_path)
    except Exception:
        return False

async def _is_md_file(workspace, wt_uid: str, vfs_path: str) -> bool:
    return vfs_path.endswith('.md') and workspace.path_exists(wt_uid, vfs_path)



# 逻辑演算器 (Binary Ops & Math)

def _compare(left: Any, right: Any, op) -> bool:
    try:
        return op(float(left) if left is not None else 0.0, float(right))
    except (ValueError, TypeError):
        return op(str(left) if left is not None else "", str(right))

def _contains(left: Any, right: Any) -> bool:
    if left is None or right is None: return False
    
    if isinstance(left, str):
        return str(right) in left
        
    if isinstance(left, dict):
        # 兼容字典键的弱类型对比，例如 "2" in {2: "a"}
        return (right in left) or (str(right) in [str(k) for k in left.keys()])
        
    if isinstance(left, list):
        # 解决 Python 中 2 == "2" 直接返回 False 而不报错的问题
        return any((item == right) or (str(item) == str(right)) for item in left)
        
    return False


async def _regex_match(text: Any, pattern: Any) -> bool:
    if text is None: return False
    try:
        return re.search(str(pattern), str(text)) is not None
    except re.error:
        return False
