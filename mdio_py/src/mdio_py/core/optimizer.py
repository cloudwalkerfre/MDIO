from .mdio_ir import Pipeline, PluginCall, ScopeBlock, ExecBlock, Op, SelectorIR, fuse_steps
from typing import List

class ExecMode:
    NATIVE = "NATIVE"
    HYBRID = "HYBRID"

RUST_NATIVE_FUNCS = {"_len", "_fmt", "_token", "_split"}


def optimize_ir(ops: List[Op]) -> List[Op]:
    """
    遍历 IR 树，应用优化:
    1. 为 Pipeline 打上执行模式标签 (NATIVE/HYBRID)
    2. 对 SelectorIR 应用 fuse_steps（仅限 DSL 路径中未在 build() 时融合的）
    """
    if not ops:
        return []

    for op in ops:
        # SelectorIR.steps 融合: duck-typing 覆盖 ReadOp/WriteOp/MemReadOp
        # (ExecBlock 无 .steps → 自然跳过; ScopeBlock 走 elif)
        if hasattr(op, 'selector') and hasattr(op.selector, 'steps'):
            sel = op.selector
            if sel.steps:
                sel.steps = fuse_steps(sel.steps)
        elif isinstance(op, ScopeBlock):
            if hasattr(op, 'steps') and op.steps:
                op.steps = fuse_steps(op.steps)
            if isinstance(op.pipeline, list):
                optimize_ir(op.pipeline)
            elif isinstance(op.pipeline, Pipeline):
                _tag_pipeline(op.pipeline)
        elif isinstance(op, ExecBlock):
            if hasattr(op.pipeline, 'atoms'):
                optimize_ir(op.pipeline.atoms)
                _tag_pipeline(op.pipeline)
        elif isinstance(op, Pipeline):
            if hasattr(op, 'atoms'):
                optimize_ir(op.atoms)   # 递归 atoms 找 ReadOp/WriteOp 等
            _tag_pipeline(op)

    return ops

def _tag_pipeline(pipeline: Pipeline):
    mode = ExecMode.NATIVE
    if hasattr(pipeline, 'atoms'):
        for atom in pipeline.atoms:
            if isinstance(atom, PluginCall):
                mode = ExecMode.HYBRID
                break
            if isinstance(atom, ExecBlock):
                _tag_pipeline(atom.pipeline)
                if getattr(atom.pipeline, 'mode', ExecMode.NATIVE) == ExecMode.HYBRID:
                    mode = ExecMode.HYBRID
                    break
    pipeline.mode = mode
