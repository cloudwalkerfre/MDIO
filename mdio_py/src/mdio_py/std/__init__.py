"""
MDIO Standard Library (std)
严格遵循“控制参数”与“数据流”隔离的 UNIX 哲学。
"""

from .string_ops import fmt, split, join, trim, upper, lower
from .json_ops import loads, dumps, length, get, tojson
from .map_ops import map
from .foreach_ops import foreach
from .filter_ops import filter
from .search_ops import find, tree
from .fs_ops import rm, mk, ls, reload
from .hash_ops import hash_state
from .logic_ops import test
from .math_ops import calc
from .worktree_ops import snap, worktree
from .sel_ops import sel
from .arrow_ops import arrow

# 暴露给 Interpreter 注册使用的路由表
__all__ = [
    # 字符串处理
    "fmt", "split", "join", "trim", "upper", "lower",
    # 结构化处理
    "loads", "dumps", "length", "get", "tojson",
    # 树变换
    "map", "foreach", "filter",
    # 检索与分析 (双模态: 内存/物理引擎)
    "find", "tree",
    # 文件夹/MD FS
     "rm", "mk", "ls", "reload",
     "hash_state",
     "test",
     "calc",
    "snap",
    "worktree",
    "sel",
    # 数据面(arrow HS facade)
    "arrow",
]