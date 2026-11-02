"""CommonMark Spec 符合性测试 — 共享 fixtures、Selector 提取、AST 工具。"""

import pytest
from pathlib import Path
from mdio_py import MdioRuntime


# 工作区 fixture

@pytest.fixture(scope="module")
def cm_workspace(tmp_path_factory):
    """复制仓库根的 commonmark.md（本地未入库的规范数据文件）到临时工作区（文档根恒为 <ws>/wt），返回路径字符串。"""
    import shutil
    ws = tmp_path_factory.mktemp("cm")
    (ws / "wt").mkdir(parents=True, exist_ok=True)
    cm_src = Path(__file__).resolve().parent.parent.parent.parent / "commonmark.md"
    shutil.copy2(cm_src, ws / "wt" / "commonmark.md")
    return str(ws)


# Selector 提取工具

async def cm_desc(rt, h1, h2):
    """提取章节描述文字（第一个 paragraph）。"""
    r = await rt.exec(f'$/commonmark.md::"{h1}"."{h2}"._p[0] -pj content', wt_uid="genesis")
    return r.strip() if r and r.strip() and not r.startswith("- ") else "(无法提取章节描述)"


async def cm_example(rt, h1, h2, idx=0):
    """提取第 idx 个 example block，返回 (md_input, expected_html)。

    用 ._code[.tag == "example"][idx] 匹配（fence info string "example"），
    排除 spec 文本中的 ```markdown 等配图代码块。

    example body 格式: <markdown>\n.\n<html>\n  (期望 HTML 为空时,
    body 以 ". " 结尾、`.` 后直接关 fence, 如 Link reference definitions N=15)。
    """
    r = await rt.exec(
        f'$/commonmark.md::"{h1}"."{h2}"._code[.tag == "example"][{idx}] -pj content',
        wt_uid="genesis"
    )
    body = (r or "").strip("\n")
    parts = body.split("\n.\n", 1)
    if len(parts) != 2:
        # 期望 HTML 为空: body 形如 "<markdown>\n." (`.` 后无换行直接关 fence)
        if body.endswith("\n.") or body == ".":
            return body[:-2] if body.endswith("\n.") else "", ""
        raise ValueError(f"example #{idx} 格式错误（无 . 分隔符）: {str(r)[:100]}")
    return parts[0], parts[1]
