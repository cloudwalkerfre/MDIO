"""Tests for DN_AST channel: to_ast_object + get_ast + get_ast_packet."""
import json
import pytest
from mdio_py import mdio_core


@pytest.fixture
def store():
    s = mdio_core.ProjectStore("/mem")
    s.tree_ensure()
    return s


def write_and_get_doc(store, text: str) -> int:
    """Write text to /t.md (replace mode) and return doc_id."""
    store.write_locator(
        "genesis", "/t.md", {"steps": []}, "/>", text,
        True, False, False, False,
    )
    return store.resolve_path("genesis", "/t.md", False)


# Basic block types

def test_ast_header(store):
    doc_id = write_and_get_doc(store, "# Hello\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) == 1
    n = ast[0]
    assert n["nt"] == "h"
    assert n["lv"] == 1
    assert n.get("t") == "Hello"


def test_ast_bullet_list(store):
    doc_id = write_and_get_doc(store, "- world\n- foo\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) == 1
    container = ast[0]
    assert container["nt"] == "bl"
    assert "ct" in container
    children = container["ct"]
    assert len(children) == 2
    assert all(c["nt"] == "b" for c in children)


def test_ast_paragraph(store):
    doc_id = write_and_get_doc(store, "hello world\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) == 1
    assert ast[0]["nt"] == "p"


def test_ast_empty(store):
    doc_id = write_and_get_doc(store, "\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert ast == []


def test_ast_multi_header(store):
    doc_id = write_and_get_doc(store, "# H1\n## H2\n### H3\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) == 3
    assert ast[0]["lv"] == 1
    assert ast[1]["lv"] == 2
    assert ast[2]["lv"] == 3


def test_ast_code_fence(store):
    doc_id = write_and_get_doc(store, "```python\nprint('hi')\n```\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) >= 1
    container = ast[0]
    assert container["nt"] == "c"
    # code_fence 是一个整体节点: 语言 tag 和内容 code 都挂在 fence 节点自己身上
    assert container.get("t") == "python"
    assert container.get("cd") == "print('hi')\n"


def test_ast_nested(store):
    text = "# Top\n- item1\n  - sub1\n- item2\n"
    doc_id = write_and_get_doc(store, text)
    ast = json.loads(store.get_ast(doc_id))["ct"]
    # header + bullet_list containing bullets
    assert len(ast) >= 1
    assert ast[0]["nt"] == "h"


def _unwrap_p(ast):
    """递归收集所有 nt=p 节点的 ct 文本(值段; 富文本拆段时拼接 txt 节点)。"""
    out = []
    if isinstance(ast, dict):
        if ast.get("nt") == "p":
            ct = ast.get("ct", "")
            if isinstance(ct, str):
                out.append(ct)
            elif isinstance(ct, list):
                out.append("".join(
                    c.get("ct", "") if isinstance(c, dict) else str(c)
                    for c in ct))
        for v in ast.values():
            if isinstance(v, (list, dict)):
                out.extend(_unwrap_p(v))
    elif isinstance(ast, list):
        for c in ast:
            out.extend(_unwrap_p(c))
    return out


def test_ast_bq_nested_item_value(store):
    """BQ 夹在列表层间时, 内层 item 值段剥全部容器层
    (- > - b 的值 = b, 无 '> -' 前缀残留)。"""
    for md, exp in [
        ("- > - b\n", "b"),
        ("- > - b\n  > - c\n", "b"),
        ("- > 1. x\n", "x"),
        ("- > > - b\n", "b"),          # 双 BQ 夹层
        ("- > 1. x\n  > 2. y\n", "x"),  # BQ 夹 OL + 续行
        ("> - x\n", "x"),
        ("- - x\n", "x"),
        ("- > - x\n", "x"),
    ]:
        doc_id = write_and_get_doc(store, md)
        ast = json.loads(store.get_ast(doc_id))["ct"]
        vals = _unwrap_p(ast)
        assert any(exp in v for v in vals), f"{md!r}: 值段应含 {exp!r}, got {vals}"


def test_ast_item_value_leading_space(store):
    """值段前导空格是内容的一部分(selector 语义固化: '- k:  123' 值 = ' 123')。"""
    doc_id = write_and_get_doc(store, "- k:  123\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    vals = _unwrap_p(ast)
    assert " 123" in vals, f"前导空格应保留, got {vals}"


def test_ast_list_hb_interplay(store):
    """与 hb 联动: 列表内 hb 块的内容通道整块原文(剥容器前缀),
    内层 item 值段不残留 BQ 前缀。"""
    doc_id = write_and_get_doc(store, "- > <div>\n> foo\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    hbs = []
    def _walk(n):
        if isinstance(n, dict):
            if n.get("nt") == "hb":
                hbs.append(n.get("ct", ""))
            for v in n.values():
                if isinstance(v, (list, dict)):
                    _walk(v)
        elif isinstance(n, list):
            for c in n:
                _walk(c)
    _walk(ast)
    assert hbs, "应存在 hb 节点"
    assert any("<div>" in h and "foo" in h for h in hbs), f"hb ct 应含整块原文, got {hbs}"


def test_ast_blockquote(store):
    # blockquote is a container — need actual content
    doc_id = write_and_get_doc(store, "> quoted text\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) >= 1
    assert any(n["nt"] == "bq" for n in ast)


def test_ast_table(store):
    text = "| a | b |\n| - | - |\n| 1 | 2 |\n"
    doc_id = write_and_get_doc(store, text)
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) >= 1
    assert any(n["nt"] == "tb" for n in ast)


def test_ast_thematic_break(store):
    doc_id = write_and_get_doc(store, "---\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) == 1
    assert ast[0]["nt"] == "hr"


def test_ast_html_block(store):
    doc_id = write_and_get_doc(store, "<div>\nhello\n</div>\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) >= 1
    assert any(n["nt"] == "hb" for n in ast)


def test_ast_checklist(store):
    doc_id = write_and_get_doc(store, "- [x] done\n- [ ] todo\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) >= 1
    children = ast[0].get("ct", ast)
    # C3 面语义：无键 ck 的 tag = 状态字符 x / X / 空格（框文本不进 tag）
    assert any(c["nt"] == "b" and c.get("t") == "x" for c in children)
    assert any(c["nt"] == "b" and c.get("t") == " " for c in children)


def test_ast_ordered_list(store):
    doc_id = write_and_get_doc(store, "1. first\n2. second\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) >= 1
    container = ast[0]
    assert container["nt"] == "od"


def test_ast_indented_code(store):
    doc_id = write_and_get_doc(store, "    code line\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) >= 1
    assert ast[0]["nt"] == "ic"


def test_ast_comment(store):
    doc_id = write_and_get_doc(store, "<!-- comment -->\n")
    ast = json.loads(store.get_ast(doc_id))["ct"]
    assert len(ast) >= 1


# Packet format

def test_ast_packet_format(store):
    doc_id = write_and_get_doc(store, "# X\n")
    pkt = store.get_ast_packet(doc_id)
    assert pkt is not None
    assert len(pkt) >= 14  # 13 header + at least 1 byte payload
    # DN_AST = 0x84
    assert pkt[0] == 0x84
    # payload length (LE)
    payload_len = int.from_bytes(pkt[1:5], "little")
    assert payload_len == len(pkt) - 13
    # doc_id in header
    hdr_doc_id = int.from_bytes(pkt[9:13], "little")
    assert hdr_doc_id == doc_id
    # type byte = 'F' (full)
    assert pkt[13] == ord("F")
    # remaining is valid JSON
    json_str = pkt[14:].decode("utf-8")
    parsed = json.loads(json_str)["ct"]
    assert len(parsed) == 1
    assert parsed[0]["nt"] == "h"


def test_ast_packet_type_byte(store):
    """Verify type byte is 'F' (full)."""
    doc_id = write_and_get_doc(store, "- a\n- b\n")
    pkt = store.get_ast_packet(doc_id)
    assert pkt[13] == ord("F")


# get_ast null handling

def test_get_ast_none_for_invalid_doc(store):
    assert store.get_ast(999) is None
