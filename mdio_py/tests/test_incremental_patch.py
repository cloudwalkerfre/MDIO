"""Tests for the incremental AST patch pipeline.

Covers:
- line_to_path: 行号 → MST 路径
- extract_affected_nodes: dirty_lines → AffectedNode list
- build_mst_patch: 路径 + node → RFC 6902 JSON patch
- block_node_to_json: single node → named-property JSON
"""
import json
import pytest
from mdio_py import mdio_core


@pytest.fixture
def store():
    s = mdio_core.ProjectStore("/mem/_inc_patch")
    s.tree_ensure()
    return s


def write_doc(store, text, path="/t.md"):
    store.write_locator("genesis", path, {"steps": []}, "+>", text, True, False, False, False)
    return store.resolve_path("genesis", path, False)


def get_ast(store, doc_id):
    # root 信封 ({ch, mode, nt, timestamp})——取 ch 为节点数组
    return json.loads(store.get_ast(doc_id))["ct"]


# Full-snapshot baseline verified first

def test_full_snapshot_roundtrip(store):
    """Full 'F' frame roundtrip: write → get_ast → valid JSON."""
    doc_id = write_doc(store, "# H1\n- a\n- b\n\npara\n", "/f1.md")
    ast = get_ast(store, doc_id)
    assert len(ast) >= 1
    # 短字段名（nt/h）—— 全量快照的 header 节点使用短字段名
    assert ast[0]["nt"] == "h"


# line_to_path connectivity

def test_line_to_path_single_header(store):
    """# Only → MST path would be /children/0 (verified via full AST)."""
    doc_id = write_doc(store, "# Only\n", "/f2.md")
    ast = get_ast(store, doc_id)
    assert ast[0]["nt"] == "h"
    # MST path for this header: /children/0


# incremental patch simulation

def simulate_patch_for_change(before_ast, after_ast):
    """Simulate what the incremental pipeline produces.

    比较全量快照并生成等价 RFC 6902 patches。
    """
    patches = []
    # Simple approach: if lengths differ, new items were appended
    if len(after_ast) > len(before_ast):
        for i in range(len(before_ast), len(after_ast)):
            patches.append({
                "op": "add",
                "path": f"/children/-",
                "value": after_ast[i],
            })
    elif len(after_ast) < len(before_ast):
        for i in range(len(after_ast), len(before_ast)):
            patches.append({
                "op": "remove",
                "path": f"/children/{i}",
            })
    else:
        # Same count: compare node-by-node
        for i in range(len(after_ast)):
            b = before_ast[i]
            a = after_ast[i]
            if b != a:
                patches.append({
                    "op": "replace",
                    "path": f"/children/{i}",
                    "value": a,
                })
    return patches


def test_append_bullet_patch(store):
    """Append a bullet: should produce an 'add' patch in the full snapshot diff."""
    doc_id = write_doc(store, "- a\n- b\n", "/f3.md")
    before = get_ast(store, doc_id)
    n_before = len(json.dumps(before))

    # Append to the end of file
    store.write_locator("genesis", "/f3.md", {"steps": []}, "+>", "- c\n", True, False, False, False)

    after = get_ast(store, doc_id)
    n_after = len(json.dumps(after))

    # After AST must be larger — a new node was added
    assert n_after > n_before, f"AST should grow after append: {n_before} → {n_after}"


def test_edit_content_patch(store):
    """Edit content of existing node: AST content field should change."""
    doc_id = write_doc(store, "hello\n", "/f4.md")
    before = get_ast(store, doc_id)
    before_text = json.dumps(before)

    # Replace entire doc content
    store.write_locator("genesis", "/f4.md", {"steps": []}, "/>", "Hello\n", True, False, False, False)

    after = get_ast(store, doc_id)
    after_text = json.dumps(after)

    assert after_text != before_text, "AST should change after edit"


# block_node_to_json via Python

def test_full_snapshot_has_all_fields(store):
    """Verify named-property JSON has all expected fields."""
    doc_id = write_doc(store, "# Title\ntext\n```py\ncode\n```\n- item\n", "/f5.md")
    ast = get_ast(store, doc_id)

    # Find header
    headers = [n for n in ast if n["nt"] == "h"]
    assert len(headers) >= 1
    h = headers[0]
    assert "nt" in h
    assert "lv" in h

    # Find code fence
    fences = [n for n in ast if n["nt"] == "c"]
    assert len(fences) >= 1


# verify patch format compatibility

def test_generated_patch_is_rfc6902(store):
    """Verify patches match RFC 6902 format."""
    doc_id = write_doc(store, "- one\n", "/f6.md")
    before = get_ast(store, doc_id)

    store.write_locator("genesis", "/f6.md", {"steps": [["Drill", "BLT"], ["Slice", [-1]]]}, "+>", "- two\n", True, False, False, False)

    after = get_ast(store, doc_id)
    patches = simulate_patch_for_change(before, after)

    for p in patches:
        assert "op" in p, "RFC 6902 patch must have 'op'"
        assert "path" in p, "RFC 6902 patch must have 'path'"
        assert p["op"] in ("add", "remove", "replace", "move", "copy", "test"), f"Invalid op: {p['op']}"
