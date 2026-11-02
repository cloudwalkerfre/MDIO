"""install.md manifest parser.

WTS manifest is Markdown, parsed by the MDIO engine (not TOML/YAML):

    # INSTALL

    - schema: 1
    - name: MDIO_WTS

    ## WORKTREES

    ### genesis
    - path: genesis
    - required: true

    ### chat
    - path: chat
    - parent: genesis

The parser accepts both AST shapes produced by the MDIO engine:
- nested (HS topology): headings carry their children in ct;
- flat (CF/proj topology): headings are siblings and their bodies follow.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path, PurePosixPath


@dataclass(frozen=True)
class WtSpec:
    name: str
    path: str
    parent: str = "genesis"
    required: bool = False
    root: bool = False


def _text(node) -> str:
    if isinstance(node, str):
        return node
    if isinstance(node, list):
        return "".join(_text(item) for item in node)
    if not isinstance(node, dict):
        return ""
    if node.get("nt") in ("txt", "sb", "code"):
        return _text(node.get("ct"))
    return _text(node.get("ct"))


def _is_heading(node) -> bool:
    return isinstance(node, dict) and node.get("nt") == "h"


def _find_heading(nodes, title: str):
    """Return (heading, sibling_tail) for a heading anywhere in the tree."""
    for idx, node in enumerate(nodes or []):
        if not isinstance(node, dict):
            continue
        if _is_heading(node) and node.get("t") == title:
            return node, list((nodes or [])[idx + 1:])
        child = node.get("ct")
        if isinstance(child, list):
            found = _find_heading(child, title)
            if found:
                return found
    return None


def _heading_sequence(node, tail: list[dict]) -> list[dict]:
    """Return the sequence carrying this section's body.

    HS nests members under the heading (ct); CF/proj leaves the heading empty
    and emits the body as following siblings.
    """
    body = node.get("ct")
    if isinstance(body, list):
        if any(_is_heading(child) for child in body):
            return body
        if body:
            return tail
    return tail


def _members(sequence: list[dict]):
    current = None
    buf: list[dict] = []
    out = []
    for child in sequence:
        if _is_heading(child):
            level = child.get("lv")
            if isinstance(level, int) and level <= 2:
                break
            if level == 3:
                if current is not None:
                    out.append((current, buf))
                current = child
                buf = []
                continue
        if current is not None:
            buf.append(child)
    if current is not None:
        out.append((current, buf))
    return out


def _bullets(nodes: list[dict]) -> dict[str, str]:
    out: dict[str, str] = {}
    for child in nodes or []:
        if not isinstance(child, dict) or child.get("nt") != "bl":
            continue
        for item in child.get("ct") or []:
            if not isinstance(item, dict) or item.get("nt") != "b":
                continue
            key = str(item.get("t") or "").strip()
            if key:
                out[key] = _text(item.get("ct")).strip()
    return out


def parse_install_text(text: str) -> list[WtSpec]:
    from mdio_py import mdio_core as mrc

    raw = mrc.ProjectStore("/mem").parse_to_ast(text)
    envelope = json.loads(raw)
    roots = envelope.get("ct") or []
    found = _find_heading(roots, "WORKTREES")
    if not found:
        raise ValueError("install.md: missing ## WORKTREES")
    section, tail = found
    members = _members(_heading_sequence(section, tail))
    if not members:
        raise ValueError("install.md: WORKTREES has no ### members")

    specs: list[WtSpec] = []
    seen: set[str] = set()
    for member, member_tail in members:
        name = str(member.get("t") or "").strip()
        if not name:
            raise ValueError("install.md: empty worktree name")
        if name in seen:
            raise ValueError(f"install.md: duplicate worktree {name!r}")
        seen.add(name)
        body = member.get("ct")
        fields = _bullets(body if isinstance(body, list) and body else member_tail)
        path = fields.get("path", "")
        if not path:
            raise ValueError(f"install.md: worktree {name!r} missing path")
        pure = PurePosixPath(path)
        if pure.is_absolute() or ".." in pure.parts:
            raise ValueError(f"install.md: unsafe path for {name!r}: {path}")
        parent = fields.get("parent", "genesis") or "genesis"
        required = fields.get("required", "false").strip().lower() in ("1", "true", "yes")
        is_root = fields.get("root", "false").strip().lower() in ("1", "true", "yes")
        specs.append(WtSpec(name=name, path=path, parent=parent, required=required, root=is_root))

    roots = [spec for spec in specs if spec.root]
    if len(roots) > 1:
        raise ValueError("install.md: multiple worktrees marked root")
    names = {spec.name for spec in specs}
    for spec in specs:
        if not spec.root and spec.parent != "genesis" and spec.parent not in names:
            raise ValueError(f"install.md: parent {spec.parent!r} not found for {spec.name!r}")
    return specs


def load_install_md(path: Path) -> list[WtSpec] | None:
    if not path.is_file():
        return None
    return parse_install_text(path.read_text(encoding="utf-8"))
