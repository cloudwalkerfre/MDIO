"""patch match attr —— 声明层 + 注册绑定 + P 帧端到端。

判官面：
  ① 纯函数：地址切分（首个 ::、无 :: 抛）、文件段归一（.* → **）、选择器编译（与 sel compile 同源）；
  ② 绑定：register_match / remove_match / clear_* / match_count_for_worktree 生命周期；
  ③ 端到端：注册 ⇒ P 帧对应 op 带 match:["mod"]；无注册 ⇒ 帧内无 match 键。
"""
from __future__ import annotations

import json
import time
from collections import deque

import pytest

from mdio_py import MdioRuntime
from mdio_py.core.error import MdioError
from mdio_py.core.mdio_ir import SelectorIR
from mdio_py.std import match_ops as mo


# ① 纯函数

def test_normalize_file_pattern():
    assert mo.normalize_file_pattern("") == "**"
    assert mo.normalize_file_pattern(".*") == "**"
    assert mo.normalize_file_pattern("/docs/.*") == "/docs/**"
    assert mo.normalize_file_pattern("/a.md") == "/a.md"
    assert mo.normalize_file_pattern("**/*.md") == "**/*.md"


def test_split_address_first_double_colon_only():
    assert mo.split_address("::A.B") == ("", "A.B")
    assert mo.split_address("/doc.md::A.B") == ("/doc.md", "A.B")
    assert mo.split_address("/doc.md::A::B") == ("/doc.md", "A::B")
    with pytest.raises(MdioError):
        mo.split_address("A.B")          # 无 :: ⇒ 抛（禁静默回填）
    with pytest.raises(MdioError):
        mo.split_address("/doc.md::")    # 空右段 ⇒ 抛


def test_compile_steps_is_sel_compile_same_source():
    from mdio_py.core import parse_code

    steps = mo.compile_steps("H1")
    assert isinstance(steps, list) and steps
    assert steps[0] == ["Drill", "H1"], steps
    # 与 sel compile 同源：同一 parse_code + optimize_ir 产出
    assert steps == parse_code("$ H1")[0].atoms[0].selector.steps
    with pytest.raises(MdioError):
        mo.compile_steps("")


# ② 绑定生命周期

async def test_register_and_remove_match_binding(runtime: MdioRuntime):
    from mdio_py.core.selector_builder import SelectorBuilder

    (runtime.store).get_text  # 面存在性（store 即 PyProjectStore）
    await runtime.exec("mk -f /m.md", wt_uid="genesis")
    await runtime.exec('"# H1\n\npara\n" | /> /m.md', wt_uid="genesis")
    ir = SelectorBuilder().drill("H1").any_child().build()

    reg = runtime.store.register_match("genesis", "/m.md", ir, "modA")
    assert isinstance(reg, int) and reg != 0
    assert runtime.store.match_count_for_worktree("genesis") == 1

    # 无 Drill 前缀（D5）⇒ 抛 IOError（不静默成功）
    with pytest.raises(Exception):
        runtime.store.register_match("genesis", "/m.md", SelectorIR(steps=[["Wildcard"]]), "bad")

    assert runtime.store.remove_match(reg) == 1
    assert runtime.store.match_count_for_worktree("genesis") == 0


# ③ P 帧端到端

class PatchCollector:
    def __init__(self, store):
        self.received = deque()
        self.store = store
        self.store.register_ast_callback(self._cb)

    def _cb(self, packet: bytes):
        if len(packet) > 14 and chr(packet[13]) == "P":
            try:
                self.received.append(json.loads(packet[14:].decode("utf-8")))
            except Exception:
                pass

    def wait(self, timeout=5.0, expected=1):
        deadline = time.perf_counter() + timeout
        while len(self.received) < expected:
            if time.perf_counter() > deadline:
                return False
            time.sleep(0.01)
        return True




@pytest.mark.parametrize("registered", [True, False])
async def test_pframe_carries_match_only_when_registered(runtime: MdioRuntime, registered: bool):
    from mdio_py.core.selector_builder import SelectorBuilder

    coll = PatchCollector(runtime.store)
    await runtime.exec("mk -f /m.md", wt_uid="genesis")
    await runtime.exec('"# H1\n\npara text\n" | /> /m.md', wt_uid="genesis")
    assert coll.wait(5.0), "首帧超时"
    coll.received.clear()

    if registered:
        ir = SelectorBuilder().drill("H1").any_child().build()
        assert runtime.store.register_match("genesis", "/m.md", ir, "modA") != 0

    await runtime.exec("mk -f /m.md", wt_uid="genesis")
    await runtime.exec('"# H1\n\npara text CHANGED\n" | /> /m.md', wt_uid="genesis")
    assert coll.wait(5.0), "编辑帧超时"
    patches = [p for frame in coll.received for p in frame]
    assert patches, "编辑必须产补丁"
    hits = [p for p in patches if p.get("match")]
    if registered:
        assert hits, "注册命中 ⇒ 该 op 必须带 match：%r" % patches
        assert all(p["match"] == ["modA"] for p in hits), hits
    else:
        assert not hits, "无注册 ⇒ 帧内不得出现 match 键：%r" % hits

async def test_get_ast_carries_per_node_match(runtime: MdioRuntime):
    from mdio_py.core.selector_builder import SelectorBuilder

    await runtime.exec("mk -f /m.md", wt_uid="genesis")
    await runtime.exec('"# H1\\n\\npara\\n" | /> /m.md', wt_uid="genesis")
    doc_id = runtime.store.resolve_path("genesis", "/m.md", False)
    plain = runtime.store.get_ast(doc_id)
    assert plain and '"match"' not in plain, "无注册 ⇒ F 帧不得有 match 键"

    ir = SelectorBuilder().drill("H1").any_child().build()
    reg = runtime.store.register_match("genesis", "/m.md", ir, "modA")
    assert reg != 0
    with_match = runtime.store.get_ast(doc_id)
    assert '"match":["modA"]' in with_match, with_match[:400]

    assert runtime.store.remove_match(reg) == 1
    after = runtime.store.get_ast(doc_id)
    assert '"match"' not in after, "注销后 F 帧不得残留 match"
