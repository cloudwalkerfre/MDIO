"""SelectorBuilder — programmatic selector construction API.

Produces SelectorIR instances with Vec<Step>-compatible step sequences.
DSL-parsed and Builder-built selectors converge at the step-sequence format.
"""

from typing import Any, List, Optional
from .mdio_ir import SelectorIR, TYPE_ANNOT, _prepend_wildcard, _prepend_recursive_wildcard, fuse_steps


class SelectorBuilder:
    """Fluent builder for MDIO selector step sequences."""

    def __init__(self):
        self._steps: List[List] = []

    # ── Path steps ──

    def drill(self, name: str) -> "SelectorBuilder":
        # split dotted paths like "A.B.C" into separate Drill steps
        # (same semantics as the DSL's md_path.split("."))
        for part in name.split("."):
            if part:
                self._steps.append(["Drill", part])
        return self

    def any_child(self) -> "SelectorBuilder":
        self._steps.append(["Wildcard"])
        return self

    def any_descendant(self) -> "SelectorBuilder":
        self._steps.append(["RecursiveWildcard"])
        return self

    def recursive_drill(self, name: str) -> "SelectorBuilder":
        self._steps.append(["RecursiveDrill", name])
        return self

    # ── Type check (auto-prepends Wildcard like DSL TYPE_ANNOT) ──

    def type_is(self, annot: str) -> "SelectorBuilder":
        # D23 收口：编程构造面也必须走严格类型名解析，未知名 fail-fast。
        # 旧实现 `TYPE_ANNOT.get(annot, annot)` 会把 'zzz' 原样透传给 Rust，
        # 而 Rust 对解不出的 TYPECHECK 返回 None ⇒ filter 被静默丢弃（全量输出）。
        from .mdio_transformer import resolve_type_name
        type_name = resolve_type_name(annot, where="SelectorBuilder.type_is")
        _prepend_wildcard(self._steps)
        self._steps.append(["TYPECHECK", type_name])
        return self

    # ── Filter steps ──

    def tag_eq(self, value: str) -> "SelectorBuilder":
        self._steps.append(["CMP", ".tag", "==", value])
        return self

    def tag_ne(self, value: str) -> "SelectorBuilder":
        self._steps.append(["CMP", ".tag", "!=", value])
        return self

    def mark_eq(self, value: str) -> "SelectorBuilder":
        self._steps.append(["CMP", ".mark", "==", value])
        return self

    def mark_ne(self, value: str) -> "SelectorBuilder":
        self._steps.append(["CMP", ".mark", "!=", value])
        return self

    def content_has(self, text: str) -> "SelectorBuilder":
        self._steps.append(["HAS", ".content", text])
        return self

    def content_empty(self) -> "SelectorBuilder":
        self._steps.append(["EMPTY", ".content"])
        return self

    def filter_and(self, *exprs: List) -> "SelectorBuilder":
        self._steps.append(["AND", list(exprs)])
        return self

    def filter_or(self, *exprs: List) -> "SelectorBuilder":
        self._steps.append(["OR", list(exprs)])
        return self

    def filter_not(self, expr: List) -> "SelectorBuilder":
        self._steps.append(["NOT", expr])
        return self

    # ── Slice ──

    def slice(self, *specs) -> "SelectorBuilder":
        """int index or [\"RANGE\", start, end, step] list.
        Single int → [\"Slice\", n]; multiple/lists → [\"Slice\", [items...]]."""
        items = []
        for s in specs:
            if isinstance(s, int):
                items.append(s)
            elif isinstance(s, list) and s[0] == "RANGE":
                items.append(s)
        if len(items) == 1 and isinstance(items[0], int):
            self._steps.append(["Slice", items[0]])
        else:
            self._steps.append(["Slice", items])
        return self

    # ── Build ──

    def build(self, file_path: Optional[str] = None,
              context_id: Optional[Any] = None) -> SelectorIR:
        return SelectorIR(file_path=file_path, context_id=context_id,
                          steps=fuse_steps(list(self._steps)))


# ── Convenience functions ──

def by_tag(heading: str, tag: str, *,
           type_name: str = "bullet", index: int = 0) -> SelectorIR:
    """$HEAD._b:tag equivalent."""
    return (SelectorBuilder()
            .drill(heading)
            .type_is(type_name)
            .tag_eq(tag)
            .slice(index)
            .build())


def by_type(heading: str, type_name: str) -> SelectorIR:
    """$HEAD._nt equivalent (no tag filter)."""
    return (SelectorBuilder()
            .drill(heading)
            .type_is(type_name)
            .build())


def drill(path: str) -> SelectorIR:
    """$X.Y.Z → Drill("X"), Drill("Y"), Drill("Z")."""
    b = SelectorBuilder()
    for part in path.split("."):
        b.drill(part)
    return b.build()
