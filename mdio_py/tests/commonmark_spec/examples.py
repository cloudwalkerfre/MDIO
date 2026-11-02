"""
从仓库根的 commonmark.md（本地未入库的规范数据文件）提取所有 example block 作为测试数据。

每个 example: (section_heading, index, markdown_input, expected_html)
"""

import re
from pathlib import Path
from collections import defaultdict

_cm_path = Path(__file__).resolve().parent.parent.parent.parent / "commonmark.md"

_examples_cache = None


def _parse():
    """解析 commonmark.md 返回 list[dict]"""
    global _examples_cache
    if _examples_cache is not None:
        return _examples_cache

    text = _cm_path.read_text(encoding="utf-8")
    lines = text.split("\n")
    current_h2 = None
    examples = []
    i = 0
    while i < len(lines):
        line = lines[i]
        if line.startswith("## ") and not line.startswith("### "):
            current_h2 = line[3:].strip()
            i += 1
            continue
        m = re.match(r"^(\x60{3,})\s*example\s*$", line)
        if m:
            fence = m.group(1)
            i += 1
            body = []
            while i < len(lines):
                if lines[i] == fence:
                    i += 1
                    break
                body.append(lines[i])
                i += 1
            body_str = "\n".join(body)
            parts = body_str.split("\n.\n", 1)
            if len(parts) == 2:
                examples.append({
                    "heading": current_h2 or "?",
                    "md": parts[0],
                    "html": parts[1],
                })
        i += 1

    # 编号
    counts = defaultdict(int)
    for ex in examples:
        idx = counts[ex["heading"]]
        ex["index"] = idx
        counts[ex["heading"]] = idx + 1

    _examples_cache = examples
    return examples


def for_heading(heading: str):
    """返回指定 heading 下所有 example 的 [(md, html), ...] 列表"""
    return [(e["md"], e["html"]) for e in _parse() if e["heading"] == heading]


def all_headings():
    """返回所有有 example 的 heading 名"""
    return sorted(set(e["heading"] for e in _parse()))


def count(heading: str) -> int:
    return len(for_heading(heading))
