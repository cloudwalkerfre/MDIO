"""Lark-based parser implementation"""

from dataclasses import dataclass
from pathlib import Path
from lark import Lark
# import lark_rust
import lark_cython
from .mdio_transformer import MdioTransformer
# 导入 IR 定义，方便类型提示
# from .mdio_ir import Op, Pipeline
from .error import MdioError, ErrCode
from lark.exceptions import LarkError, UnexpectedToken, UnexpectedCharacters, VisitError

def get_grammar_path() -> Path:
    """Get the path to the grammar file."""
    return Path(__file__).parent / "mdio_grammar.lark"

def create_parser():
    """Create and return a Lark parser for MDIO language."""
    grammar_path = get_grammar_path()
    with open(grammar_path, "r", encoding="utf-8") as f:
        grammar_content = f.read()

    parser = Lark(
        grammar_content,
        parser='lalr',
        _plugins=lark_cython.plugins,
        lexer='contextual',
        cache=True,
        transformer=MdioTransformer(),  # 返回 IR 对象列表
        maybe_placeholders=False,
        propagate_positions=False,
        keep_all_tokens=False,
        # debug=False,
    )
    # print(type(parser.parser.lexer))
    # parser = Lark(
    #     grammar_content,
    #     parser='earley',
    #     lexer='dynamic',
    #     maybe_placeholders=False,
    #     propagate_positions=False,
    #     debug=False
    # )
    return parser

def _build_error_context(code: str, line: int, column: int, err_len: int = 1) -> str:
    """Rust 风格：源代码行 + ^ 光标，含 strip 续行缩进"""
    lines = code.split('\n')
    if line < 1 or line > len(lines):
        return ""

    target_line = lines[line - 1]
    col_idx = column - 1   # Lark column is 1-based
    indent = "    "        # strip 续行缩进

    # 超长行裁剪
    max_len = 72
    if len(target_line) > max_len:
        start = max(0, col_idx - 30)
        end = min(len(target_line), col_idx + 30)
        target_line = ("..." if start > 0 else "") + target_line[start:end] + ("..." if end < len(target_line) else "")
        col_idx -= start

    return f"{indent}{target_line}\n{indent}{' ' * col_idx}^"

_PARSER_CACHE = None
_PARSE_RESULT_CACHE: dict = {}
_PARSE_CACHE_MAX = 256

def parse_code(code: str):
    """Parse MDIO code and return the IR Op list."""
    if code in _PARSE_RESULT_CACHE:
        return _PARSE_RESULT_CACHE[code]

    global _PARSER_CACHE
    if _PARSER_CACHE is None:
        _PARSER_CACHE = create_parser()

    try:
        result = _PARSER_CACHE.parse(code)
        if len(_PARSE_RESULT_CACHE) >= _PARSE_CACHE_MAX:
            _PARSE_RESULT_CACHE.pop(next(iter(_PARSE_RESULT_CACHE)))
        _PARSE_RESULT_CACHE[code] = result
        return result
    except VisitError as e:
        orig = e.orig_exc
        if isinstance(orig, MdioError):
            raise orig
        raise MdioError("parser", ErrCode.PARSE_UNKNOWN, f"语法树编译失败: {e}", cause=e)
    except UnexpectedToken as e:
        ex_token = str(e.token.value)
        if ex_token == "":
            line = code.split('\n')[e.line - 1] if e.line <= len(code.split('\n')) else ""
            ctx = _build_error_context(code, e.line, len(line) + 1, 0)
            msg = f"语法错误：输入不完整\n{ctx}"
        else:
            ctx = _build_error_context(code, e.line, e.column, len(ex_token))
            msg = f"语法错误：意外 token '{ex_token}'\n{ctx}"

        raise MdioError("parser", ErrCode.PARSE_TOKEN, msg, cause=e)
    except UnexpectedCharacters as e:
        ctx = _build_error_context(code, e.line, e.column, 1)
        msg = f"语法错误：无法识别的字符\n{ctx}"

        raise MdioError("parser", ErrCode.PARSE_CHAR, msg, cause=e)
    except LarkError as e:
        raise MdioError(
            level="parser",
            code=ErrCode.PARSE_UNKNOWN,
            message=f"语法树编译失败: {str(e)}",
            cause=e
        )


def compile_selector_text(sel, *, on_error="raise"):
    """选择器右段文本 → SelectorIR（全仓唯一编译入口）。

    等价于 optimize_ir(parse_code("$ " + sel)) 后取首原子选择器。
    回落策略必须显式声明：
      raise       解析失败向上抛（sel compile / match / MemReadOp 动态路径）
      drill       回落 steps=[["Drill", sel]]（整串单 Drill，不拆点）
      drill_path  回落 SelectorBuilder().drill(sel)（按 . 拆成多个 Drill）
      empty       回落 steps=[]（空选择器 = 全文档语义）
    """
    from .optimizer import optimize_ir
    from .mdio_ir import SelectorIR

    sel = str(sel)

    def _fallback():
        if on_error == "drill":
            return SelectorIR(steps=[["Drill", sel]])
        if on_error == "drill_path":
            from .selector_builder import SelectorBuilder
            return SelectorBuilder().drill(sel).build()
        if on_error == "empty":
            from .selector_builder import SelectorBuilder
            return SelectorBuilder().build()
        raise ValueError("compile_selector_text: unknown on_error: " + on_error)

    try:
        ops = optimize_ir(parse_code("$ " + sel))
        if not ops or not getattr(ops[0], "atoms", None):
            if on_error == "raise":
                raise MdioError("parser", ErrCode.PARSE_TOKEN,
                                "compile_selector_text: 空 IR: " + sel)
            return _fallback()
        return ops[0].atoms[0].selector
    except Exception:
        if on_error == "raise":
            raise
        return _fallback()


@dataclass
class SelectorAddr:
    """选择器地址三段。方言 = 语法权威 mdio_transformer.locator_core。

    地址本质 = wt :: file_path :: md_path；本结构就是这三段的落点。"""
    wt: "str | None" = None        # 两个 :: 时的左段（上下文/wt 名）；wt_first 约定下 = 一个 :: 的左段
    file_path: "str | None" = None  # 文档路径段（0 个 :: 且以 / 开头、无冒号时也落这里）
    md_path: str = ""               # 选择器右段文本


def split_selector_addr(raw, *, wt_first: bool = False) -> SelectorAddr:
    """全仓唯一 `::` 切分入口。与语法权威一致（地址 = wt :: file_path :: md_path）：
      0 个 :: → md_path=整串；若以 / 开头且不含 ':' 则 file_path=整串、md_path=""（同 locator_core）
      1 个 :: → file_path=左, md_path=右（wt_first=True：wt=左, file_path=右, md_path=""——ls/test 的文档化局部约定）
      2 个 :: → wt=左, file_path=中, md_path=右（多余 :: 留在 md_path 里，语法自身忽略）
    """
    s = str(raw).strip()
    parts = s.split("::")
    if len(parts) >= 3:
        return SelectorAddr(wt=parts[0].strip(), file_path=parts[1].strip(),
                            md_path="::".join(parts[2:]).strip())
    if len(parts) == 2:
        if wt_first:
            return SelectorAddr(wt=parts[0].strip(), file_path=parts[1].strip(), md_path="")
        return SelectorAddr(wt=None, file_path=parts[0].strip(), md_path=parts[1].strip())
    if s.startswith("/") and ":" not in s:
        return SelectorAddr(file_path=s, md_path="")
    return SelectorAddr(md_path=s)


def compile_selector_addr(raw, *, on_error="raise", wt_first: bool = False):
    """split_selector_addr + compile_selector_text 的一步入口。

    返回 (SelectorAddr, SelectorIR)。空 md_path 按 on_error 回落（empty / drill_path ⇒ steps=[]；
    raise ⇒ 抛 P001；drill 语义只给 md_path 本身，空 md_path 勿用）。
    """
    addr = split_selector_addr(raw, wt_first=wt_first)
    return addr, compile_selector_text(addr.md_path, on_error=on_error)

# def parse_code(code: str):
#     """Parse MDIO code and return the IR Op list."""
#     parser = create_parser()
#     return parser.parse(code)


if __name__ == "__main__":
    import sys
    import json

    # 简单的 pprint 辅助函数
    def print_ir(obj):
        if isinstance(obj, list):
            print(f"IR List ({len(obj)} items):")
            for item in obj:
                print(item)
        else:
            print(obj)

    if len(sys.argv) > 1:
        file_path = sys.argv[1]
        try:
            with open(file_path, "r", encoding="utf-8") as f:
                code = f.read()
            result = parse_code(code)
            print_ir(result)
        except Exception as e:
            print(f"Error parsing file: {e}")
    else:
        print("Usage: python -m src <file_path>")
