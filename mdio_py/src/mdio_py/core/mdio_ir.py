from dataclasses import dataclass, field
from typing import Any, List, Dict, Optional, Union

# --- 基础接口 ---
@dataclass
class SelectorIR:
    """新 SelectorIR — 步骤序列格式。必须用 @dataclass 而非 slots=True（pythonize 读 __dict__）。"""
    file_path: Optional[str] = None
    context_id: Optional[Any] = None
    steps: List[List] = field(default_factory=list)

    def is_tag_target(self) -> bool:
        """检测 steps 是否以 tag 选择结尾（CMP(.tag,==,key) + Slice）。
        表示对 keyed content 的读写意图，默认使用 content 投影。
        兼容融合前（[CMP, Slice]）和融合后（TraversalFilter 内含 CMP(.tag)）两种形式。"""
        if not self.steps:
            return False
        last = self.steps[-1]
        if not last:
            return False
        # 融合后: TraversalFilter(_, [..., CMP(.tag,==,key), ...], _)
        if last[0] == "TraversalFilter":
            filters = last[2] if len(last) > 2 else []
            for f in filters:
                if f and len(f) >= 4 and f[0] == "CMP" and f[1] == ".tag" and f[2] == "==":
                    return True
            return False
        # 未融合: ... + ["CMP", ".tag", "==", key] + ["Slice", N]
        if len(self.steps) < 2:
            return False
        if not (last[0] == "Slice" and len(last) >= 2):
            return False
        second_last = self.steps[-2]
        return (second_last and second_last[0] == "CMP"
                and second_last[1] == ".tag" and second_last[2] == "==")

@dataclass(slots=True)
class Op:
    """
    原子操作基类
    警告：切勿在此基类中添加任何带有默认值的字段（如 =None）。
    这会打破子类 dataclass 的 __init__ 函数参数生成排序，导致致命报错！
    """
    pass

@dataclass(slots=True)
class PipeDataToken(Op):
    """占位符单例：代表当前层级管道流经的数据"""
    pass

# 全局单例
PIPE_DATA = PipeDataToken()

# --- 具体指令 ---
@dataclass(slots=True)
class ReadOp(Op):
    selector: SelectorIR
    kwargs: Dict[str, Any] = field(default_factory=dict)

@dataclass(slots=True)
class MemReadOp(Op):
    selector: SelectorIR
    kwargs: Dict[str, Any] = field(default_factory=dict)

@dataclass(slots=True)
class WriteOp(Op):
    operator: str
    selector: SelectorIR
    kwargs: Dict[str, Any] = field(default_factory=dict)

@dataclass(slots=True)
class HookOp(Op):
    kwargs: Dict[str, Any] = field(default_factory=dict)

@dataclass(slots=True)
class MacroToken(Op):
    name: str  

# @dataclass(slots=True)
# class EmitOp(Op):
#     event_name: Any

@dataclass(slots=True)
class FuncCall(Op):
    name: str 
    args: List[Any]
    kwargs: Dict[str, Any] = field(default_factory=dict)

@dataclass(slots=True)
class PluginCall(Op):
    protocol: str 
    chain: List[Any] 
    kwargs: Dict[str, Any] = field(default_factory=dict)

@dataclass(slots=True)
class ScopeBlock(Op):
    file_path: str
    pipeline: List['Pipeline'] # 块内是一个语句序列
    context_id: Optional[Any] = None  # 时空前缀（wt_uid / MacroToken）
    steps: List[List] = field(default_factory=list)  # 文档内部作用范围，由步骤序列表达

@dataclass(slots=True)
class ExecBlock(Op):
    """$($...) 嵌套执行块"""
    pipeline: 'Pipeline'

@dataclass(slots=True)
class Literal(Op):
    value: Any

@dataclass(slots=True)
class FString(Op):
    raw_value: str

# @dataclass(slots=True)
# class TeeOp(Op):
#     selector: Selector

@dataclass(slots=True)
class Pipeline:
    atoms: List[Op] # 管道链
    mode: str = "NATIVE"


# ── 类型标注别名表 ──
# NOTE: this dict resolves user-facing aliases ("b", "x", "codeblock", …) to
# canonical type names ("bullet", "code", …).  The canonical names are consumed
# downstream by Rust's parse_type_name() in mdio_core/src/selector.rs.  When a
# new alias or canonical name is added, both files must be updated in sync.

TYPE_ANNOT: Dict[str, str] = {
    "b":   "bullet",
    "blt": "bullet",
    "bullet": "bullet",
    "x":        "bullet",
    "checklist": "bullet",
    "o":   "ol",
    "ol":  "ol",
    "orderlist": "ol",
    "ordered_list": "ol",
    "c":   "code",
    "code": "code",
    "codeblock": "code",
    "indtcode": "indtcode",
    "p":   "paragraph",
    "paragraph": "paragraph",
    "h":   "h",
    "head": "h",
    "h1":  "h1", "head1": "h1",
    "h2":  "h2", "head2": "h2",
    "h3":  "h3", "head3": "h3",
    "h4":  "h4", "head4": "h4",
    "h5":  "h5", "head5": "h5",
    "h6":  "h6", "head6": "h6",
    "bq":  "bq",
    "blockquote": "bq",
    "tb":  "tb",
    "table": "tb",
    "comment": "comment",
    "html": "html",
}


def _prepend_wildcard(steps: list[list]) -> None:
    """在 step 列表末尾插入 Wildcard，如果末尾尚不是 Wildcard/RecursiveWildcard 的话。"""
    if not steps or steps[-1] not in (["Wildcard"], ["RecursiveWildcard"]):
        steps.append(["Wildcard"])

def _prepend_recursive_wildcard(steps: list[list]) -> None:
    """在 step 列表末尾插入 RecursiveWildcard。"""
    if not steps or steps[-1] not in (["Wildcard"], ["RecursiveWildcard"]):
        steps.append(["RecursiveWildcard"])

# ── Step fusion ──

_FILTER_TAGS = {"TYPECHECK", "CMP", "REGEX", "HAS", "EQ", "RE",
                "EMPTY", "AND", "OR", "NOT",
                "BY", "BY_EMPTY", "BY_ANY", "BY_ALL"}
_TRAVERSAL_TAGS = {"Drill", "Wildcard", "RecursiveDrill", "RecursiveWildcard"}

def fuse_steps(steps: List[List]) -> List[List]:
    """反向扫描, 融合 [T, F*, Slice(Index(N>=0))] → TraversalFilter.
    纯函数，对已融合 steps 幂等。

    等价性由 mdio_core/tests/theory_tests/ir_opt_equivalence_test.rs 覆盖。"""
    if not steps:
        return steps
    result = []
    i = len(steps) - 1
    while i >= 0:
        step = steps[i]
        tag = step[0] if step else ""
        if tag == "Slice" and len(step) > 1:
            n = step[1]
            if isinstance(n, int) and n >= 0:
                j = i - 1
                filters = []
                while j >= 0:
                    ft = steps[j][0] if steps[j] else ""
                    if ft in _FILTER_TAGS:
                        filters.insert(0, steps[j])
                        j -= 1
                    else:
                        break
                if j >= 0:
                    tt = steps[j][0] if steps[j] else ""
                    if tt in _TRAVERSAL_TAGS:
                        traversal = steps[j]
                        fused = ["TraversalFilter", traversal, filters, n + 1]
                        result = fuse_steps(steps[:j]) + [fused] + result
                        return result
        # 末段 [T, F*] 无 Slice → TF(T, F*, limit=0), 全量收集
        elif tag in _FILTER_TAGS:
            j = i - 1
            filters = [step]
            while j >= 0:
                ft = steps[j][0] if steps[j] else ""
                if ft in _FILTER_TAGS:
                    filters.insert(0, steps[j])
                    j -= 1
                else:
                    break
            if j >= 0:
                tt = steps[j][0] if steps[j] else ""
                if tt in _TRAVERSAL_TAGS:
                    traversal = steps[j]
                    fused = ["TraversalFilter", traversal, filters, 0]  # limit=0 全量
                    result = fuse_steps(steps[:j]) + [fused] + result
                    return result
        result.insert(0, step)
        i -= 1
    return result
