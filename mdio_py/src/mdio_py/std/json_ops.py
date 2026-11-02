import json

async def loads(workspace, pipe_data, args, kwargs, ctx):
    """将 Markdown Strip 纯文本 或 JSON 字符串解析为 Python 对象"""
    target = kwargs.get("-v", pipe_data)
    if isinstance(target, (dict, list)):
        return target
        
    if not isinstance(target, str):
        return target
        
    clean_str = target.strip()
    # 剥除可能携带的 MDIO 缩进和 strip 头
    if clean_str.startswith('- '):
        clean_str = clean_str[2:]
        
    try:
        return json.loads(clean_str)
    except json.JSONDecodeError:
        return "- mdio_error: [loads] JSON parse failed"

async def dumps(workspace, pipe_data, args, kwargs, ctx):
    """将 Python 对象转为安全的 JSON 字符串"""
    target = kwargs.get("-v", pipe_data)
    indent = kwargs.get("-indent", None)
    if indent is not None:
        return json.dumps(target, ensure_ascii=False, indent=int(indent))
    return json.dumps(target, ensure_ascii=False)

_TOJSON_FACES = ("raw", "content", "tag", "mark", "container", "ast")


async def tojson(workspace, pipe_data, args, kwargs, ctx):
    """md（管道文本 或 `-p` 选择器）→ JSON；`-pj` 决定取哪个**面**。

    两个模式（`-pj` 在两侧的含义不同，故意如此）：

    1) **选择器模式**（给了 `-p`，或 `args[0]`）：走引擎读路径取面，把每个命中节点序列化成 JSON。
       - `-pj raw`（**默认**）= 每个命中节点的**原文，逐字节**（与 `$` 的默认面一致）；
       - `-pj content` = 语义文本（正是喂模型的形态，剥掉标记）；
       - `-pj tag` / `mark` / `container` = 该面；
       - `-pj ast` = 每个命中节点子树 → AST JSON（等价于 `$ <sel> | tojson`）；
       - 多命中 → JSON 数组；单命中 → 该值本身。
    2) **文本模式**（管道给 md 文本、无 `-p`）：没有“节点”可谈，`-pj` 只认 `ast`（**默认**）与 `raw`
       （原样返回，等价于恒等）。

    依据：设计硬约束——「selector → JSON」必须能区分**语义文本**
    （content）与**原文**（raw），否则需要字节一致的消费者会各自重实现渲染规则。
    """
    loc = str(kwargs.get("-p", args[0] if args else "") or "").strip()
    face = str(kwargs.get("-pj", kwargs.get("-j", "")) or "").strip()

    if face and face not in _TOJSON_FACES:
        return (f"- mdio_error: [tojson] 未知的面 '{face}'；"
                f"可用：{', '.join(_TOJSON_FACES)}")

    if loc:
        face = face or "raw"                       # 选择器读：默认原文（与 $ 一致）
        try:
            from .search_ops import resolve_vfs_target
            abs_pat, _raw_md, ir = resolve_vfs_target(loc, ctx)
            paths = [abs_pat]
            if "*" in abs_pat or "?" in abs_pat:
                paths = workspace.expand_path_pattern(ctx.wt_uid, abs_pat) or []
            if not paths:
                return f"- mdio_error: [tojson] 未命中任何文档: {loc}"
            chunks = []
            for p in paths:
                got = workspace.read_locator(ctx.wt_uid, p, ir,
                                             proj=("raw" if face == "ast" else face))
                chunks.extend(got or [])
        except (KeyboardInterrupt, SystemExit, GeneratorExit):
            raise
        except BaseException as e:
            return f"- mdio_error: [tojson] {type(e).__name__}: {e}"
        if not chunks:
            return f"- mdio_error: [tojson] 未命中任何节点: {loc}"
        if face == "ast":
            try:
                chunks = [workspace.to_json_text(c) for c in chunks]
            except (KeyboardInterrupt, SystemExit, GeneratorExit):
                raise
            except BaseException as e:
                return f"- mdio_error: [tojson] {type(e).__name__}: {e}"
        return chunks[0] if len(chunks) == 1 else json.dumps(chunks, ensure_ascii=False)

    # 文本模式
    text = pipe_data if isinstance(pipe_data, str) else ""
    if not text.strip():
        return "- mdio_error: [tojson] 需要 md 文本（管道）或 -p 选择器"
    face = face or "ast"
    if face == "raw":
        return text                               # 恒等：文本没有“面”，raw 即原样
    if face != "ast":
        return (f"- mdio_error: [tojson] 文本模式只支持 -pj ast|raw"
                f"（'{face}' 需要 -p 选择器）")
    try:
        return workspace.to_json_text(text)
    except (KeyboardInterrupt, SystemExit, GeneratorExit):
        raise
    except BaseException as e:
        # 引擎 panic 抛的是 PanicException（不是 Exception 子类）——这里一并兜住，
        # 让"读一段 md"永远不会炸穿调用方的管道。
        return f"- mdio_error: [tojson] {type(e).__name__}: {e}"

async def length(workspace, pipe_data, args, kwargs, ctx):
    """多态长度统计 (List的条目数 / Dict的键数 / String的字符数)"""
    target = kwargs.get("-v", pipe_data)
    try:
        return len(target) if target is not None else 0
    except TypeError:
        return 0

async def get(workspace, pipe_data, args, kwargs, ctx):
    """
    轻量提取器。
    用法: $DATA | get "users" | get 0
    """
    key = kwargs.get("-k", args[0] if args else None)
    target = kwargs.get("-v", pipe_data)
    
    if key is None or target is None:
        return "- mdio_error: [get] requires -k and -v"
        
    try:
        if isinstance(target, dict):
            return target.get(str(key))
        if isinstance(target, list) and isinstance(key, int):
            if 0 <= key < len(target):
                return target[key]
            return f"- mdio_error: [get] index {key} out of range (len={len(target)})"
        # 如果 key 被识别为 str 但实质代表数字
        if isinstance(target, list) and str(key).isdigit():
            idx = int(key)
            if 0 <= idx < len(target):
                return target[idx]
            return f"- mdio_error: [get] index {idx} out of range (len={len(target)})"
    except Exception as e:
        return f"- mdio_error: [get] {e}"

    return "- mdio_error: [get] key extraction failed"

# async def jq(workspace, pipe_data, args, kwargs, ctx):
#     """
#     高级封装：调用操作系统的 jq 引擎或 Python jsonpath-ng 进行纯文本 JSON 切割。
#     用法: $RAW_JSON | jq ".data.users[].name"
#     （此处给出一个轻量级降级实现，若存在复杂需求可直接 import pyjq）
#     """
#     expression = kwargs.get("-e", args[0] if args else None)
#     target = kwargs.get("-v", pipe_data)
    
#     if expression is None:
#         return "- error: [jq] expression required"
        
#     # 如果为了极致性能和不依赖三方库，这里建议先保留简单的降级报错，
#     # 待安装例如 `jmespath` 或 `jq` 库后真正放开。这里模拟占位：
#     try:
#         import jq
#         # target 可能是 str，也可能是 dict
#         if isinstance(target, str):
#             target = json.loads(target)
#         return jq.all(expression, target)
#     except ImportError:
#         return "- error: [jq] 'pyjq' library is not installed in the environment."
#     except Exception as e:
#         return f"- error: [jq] failed: {e}"
