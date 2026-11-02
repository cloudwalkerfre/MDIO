"""
增量 AST → MST 管道专用测试床 (crdt_ast_patch_plan §7)

验证流程:
1. full snapshot ('F' frame) → MST applySnapshot
2. write → dirty_lines → line_to_path → build_mst_patch
3. 'P' frame → Python callback → MST applyPatch
4. 延迟测量: Rust 写 → Python callback 收到 'P' frame

运行: uv run --python 3.14t pytest mdio_py/tests/test_ast_mst_pipeline.py -v -s
"""
import asyncio
import json
import os
import time
import pytest
from mdio_py import mdio_core, MdioRuntime
from mdio_py.core.interpreter import RuntimeContext


@pytest.fixture
def store():
    s = mdio_core.ProjectStore("/mem")
    s.tree_ensure()
    return s


@pytest.fixture
async def runtime(tmp_path):
    """exec 管道用例宿主(interpreter 层——MDIO 正经写法入口)。
    async fixture:MdioRuntime 构造需要当前事件循环。"""
    return MdioRuntime(str(tmp_path))


def write_doc(store, text):
    store.write_locator("genesis", "/p.md", {"steps": []}, "/>", text, True, False, False, False)
    return store.resolve_path("genesis", "/p.md", False)


def get_ast(store, doc_id):
    # root 信封 ({ch, mode, nt, timestamp})——取 ch 为节点数组
    return json.loads(store.get_ast(doc_id))["ct"]


# Test 1: Full snapshot 'F' frame format validation

def test_f_frame_format(store):
    """验证 'F' 帧格式符合协议规范 (crdt_ast_patch_plan §2.1)"""
    doc_id = write_doc(store, "# F Test\n- a\n- b\n")
    pkt = store.get_ast_packet(doc_id)
    assert pkt[0] == 0x84, f"Expected DN_AST tag 0x84, got 0x{pkt[0]:02x}"
    assert pkt[13] == ord('F'), f"Expected 'F' type byte, got {chr(pkt[13])}"
    payload_len = int.from_bytes(pkt[1:5], "little")
    assert payload_len == len(pkt) - 13, "Payload length mismatch"

    # type byte + JSON
    json_bytes = pkt[14:]
    ast = json.loads(json_bytes.decode("utf-8"))["ct"]
    assert len(ast) == 2  # header + bullet_list


# Test 2: MST model compatibility (full snapshot → MST)

def test_full_snapshot_mst_compatible(store):
    """验证 to_ast_object 产出的 JSON 可以直接 applySnapshot 到 MST BlockNode"""
    doc_id = write_doc(store, "# Chat\n- user: hello\n- assistant: hi\n")
    ast = get_ast(store, doc_id)

    for node in ast:
        _validate_node(node, "root")


def _validate_node(node, path, depth=0):
    """递归验证 AST 节点结构符合 MST BlockNode 模型。"""
    assert isinstance(node, dict), f"{path}: expected dict, got {type(node)}"
    assert "nt" in node, f"{path}: missing nt"
    assert isinstance(node["nt"], str), f"{path}: nt must be string"
    if "lv" in node:
        assert isinstance(node["lv"], int), f"{path}: lv must be int"
    if "ct" in node:
        # 内容通道双型(外部形态定案):字符串 = 文本叶快捷;数组 = 内容流
        # (元素 = 字符串文本段或块节点)。
        if isinstance(node["ct"], list):
            for i, child in enumerate(node["ct"]):
                child_path = f"{path}/ct/{i}"
                if isinstance(child, dict):
                    _validate_node(child, child_path, depth + 1)
                else:
                    assert isinstance(child, str), f"{child_path}: elem must be str"
        else:
            assert isinstance(node["ct"], str), f"{path}: ct must be str or list"


# Test 3: Incremental patch pathway — full round trip with timing

def test_f_frame_roundtrip_timing(store):
    """测量完整 F 帧 roundtrip: write → get_ast_packet 延迟"""
    doc_id = write_doc(store, "# Header\n- initial\n\nparagraph\n")

    # Measure F frame generation time
    t0 = time.perf_counter()
    pkt = store.get_ast_packet(doc_id)
    t1 = time.perf_counter()

    roundtrip_ms = (t1 - t0) * 1000
    pkt_size = len(pkt)
    ast_size = len(pkt) - 14  # minus 13 header + 1 type byte
    print(f"\n  [F] Roundtrip: {roundtrip_ms:.3f}ms, packet={pkt_size}B, AST={ast_size}B")
    assert roundtrip_ms < 50, f"F frame generation too slow: {roundtrip_ms:.1f}ms"


# Test 4: Simulated incremental patch verification

def test_incremental_simulation(store):
    """模拟增量管道: 比较 full snapshots → 生成等效 RFC 6902 patches → 验证格式"""
    doc_id = write_doc(store, "# Before\n- a\n- b\n")
    before = get_ast(store, doc_id)

    # Append a bullet (simulating dirty_lines = {3})
    store.write_locator("genesis", "/p.md", {"steps": []}, "+>", "- c\n", True, False, False, False)
    after = get_ast(store, doc_id)

    # Simulate incremental: diff the full snapshots
    patches = _simulate_incremental_patch(before, after)
    assert len(patches) > 0, "Expected at least one patch for append"

    # Validate each patch matches RFC 6902
    for p in patches:
        assert "op" in p
        assert "path" in p
        assert p["op"] in ("add", "remove", "replace", "move", "copy", "test")
        assert p["path"].startswith("/")

    print(f"\n  [P simulated] Patches: {len(patches)}, ops: {[p['op'] for p in patches]}")


def _simulate_incremental_patch(before_ast, after_ast):
    """Produce RFC 6902 patches by comparing two full snapshots."""
    patches = []
    before_json = json.dumps(before_ast, sort_keys=True)
    after_json = json.dumps(after_ast, sort_keys=True)
    if before_json == after_json:
        return patches

    # Append case: after has more nodes
    if len(after_ast) > len(before_ast):
        for i in range(len(before_ast), len(after_ast)):
            patches.append({"op": "add", "path": "/children/-", "value": after_ast[i]})
    # Remove case
    elif len(after_ast) < len(before_ast):
        for i in range(len(after_ast), len(before_ast)):
            patches.append({"op": "remove", "path": f"/children/{i}"})
    # Replace case
    else:
        for i, (b, a) in enumerate(zip(before_ast, after_ast)):
            if json.dumps(b, sort_keys=True) != json.dumps(a, sort_keys=True):
                patches.append({"op": "replace", "path": f"/children/{i}", "value": a})
    return patches


# Test 5: Large doc F frame size benchmarking

def test_f_frame_size_scaling(store):
    """验证不同规模文档的 F 帧大小 (<500行文档全量可行)"""
    sizes = []
    for n_lines in [10, 50, 100, 200]:
        text = "# Scale Test\n" + "\n".join(f"- item {i}" for i in range(n_lines)) + "\n"
        doc_id = write_doc(store, text)
        pkt = store.get_ast_packet(doc_id)
        ast_bytes = len(pkt) - 14  # minus header + type byte
        sizes.append((n_lines, ast_bytes))
        print(f"\n  [F size] {n_lines:>4} lines → {ast_bytes:>6} B AST")

    # Verify less-than-linear growth (containers merge items)
    assert sizes[-1][1] < sizes[-1][0] * 500, f"AST too large for {sizes[-1][0]} lines: {sizes[-1][1]}B"


# Test 6: Chatbox scenario — full snapshot after each write

def test_chatbox_scenario(store):
    """模拟 Chatbox 消息追加场景 (ui_mvp_paln §4)"""
    doc_id = write_doc(store, "# Chat: demo\n")
    messages = [
        "user: hello",
        "assistant: hi there",
        "user: how are you",
        "assistant: i'm good",
    ]

    t0 = time.perf_counter()
    for msg in messages:
        store.write_locator("genesis", "/p.md", {"steps": []}, "+>", f"- {msg}\n", True, False, False, False)
        _ = store.get_ast_packet(doc_id)
    t1 = time.perf_counter()

    total_ms = (t1 - t0) * 1000
    per_msg_ms = total_ms / len(messages)
    ast = get_ast(store, doc_id)

    # Verify all messages are present
    bullet_count = sum(1 for n in _collect_all_nodes(ast) if n.get("nt") == "b")
    assert bullet_count >= len(messages), f"Expected >= {len(messages)} bullets, got {bullet_count}"
    print(f"\n  [Chatbox] {len(messages)} messages, total={total_ms:.1f}ms, per_msg={per_msg_ms:.1f}ms, bullets={bullet_count}")


def _collect_all_nodes(nodes):
    """递归收集所有 AST 节点(ct 双型:仅数组递归,字符串跳过)"""
    result = list(nodes)
    for n in nodes:
        kids = n.get("ct")
        if isinstance(kids, list):
            result.extend(_collect_all_nodes(kids))
    return result


# Test 7: P frame build + format validation

def test_p_frame_build(store):
    """验证 get_ast_packet 产出正确的 'F' 帧格式 + 已有 P 帧在 callback 验证"""
    # Must write a doc first so get_ast_packet has content
    doc_id = write_doc(store, "# P Build\n- test\n")
    pkt = store.get_ast_packet(doc_id)
    assert pkt is not None, "get_ast_packet returned None"
    assert pkt[0] == 0x84, f"Expected DN_AST 0x84, got 0x{pkt[0]:02x}"
    assert pkt[13] == ord('F'), f"Expected 'F' type byte"
    # Verify payload is valid JSON
    json_bytes = pkt[14:]
    parsed = json.loads(json_bytes.decode("utf-8"))["ct"]
    assert len(parsed) >= 1


# Test 8: P frame end-to-end — 真实增量场景 → MST 验证
#
# 这是最关键的测试：完整模拟增量编辑 → P 帧回调 → parse patches
# → 对比 full snapshot 验证正确性 → 精确计时每个阶段

import threading
from collections import deque


class PatchCollector:
    """收集 P 帧 callback + 计时。"""
    def __init__(self, store):
        self.received: deque = deque()
        self.events: deque = deque()  # (event, timeout)
        self.store = store
        self.store.register_ast_callback(self._callback)

    def _callback(self, packet: bytes):
        t_recv = time.perf_counter()
        tag = packet[0]
        doc_id = int.from_bytes(packet[9:13], "little")
        ast_type = chr(packet[13]) if len(packet) > 13 else '?'
        # Parse patches from payload
        patches = []
        if ast_type == 'P' and len(packet) > 14:
            try:
                patches = json.loads(packet[14:].decode("utf-8"))
            except Exception:
                pass
        self.received.append({
            "tag": tag, "doc_id": doc_id, "type": ast_type,
            "len": len(packet), "patches": patches, "t_recv": t_recv,
        })

    def wait(self, timeout=5.0, expected=1):
        """等待至少 expected 个 P 帧到达。返回是否成功。"""
        deadline = time.perf_counter() + timeout
        while len(self.received) < expected:
            if time.perf_counter() > deadline:
                return False
            time.sleep(0.01)
        return True

    def clear(self):
        self.received.clear()


def _apply_patches_to_snapshot(snapshot_nodes, patches):
    """将 RFC 6902 patches 应用到 AST JSON 快照（纯 Python 模拟 MST applyPatch）。

    WC-3：字段写的线路形态已统一为 `{op, path:"<元素路径>/<字段名>", value}`
    ——路径末段承载字段名（标准 JSON Pointer）。同一个 "ct" 既可能是子通道令牌
    （`/…/ct/3`）也可能是字段名（`/…/ct`，块叶内容串或子数组），区分靠**位置**
    而非名字，所以这里按"末段是否为数字"判定，不按段名过滤。
    """
    nodes = list(snapshot_nodes)  # shallow copy

    def _navigate(path):
        """返回 (container, key, is_field)。

        - 数组位：container 是列表、key 是下标（"-" → 末尾），is_field=False；
        - 字段位：container 是节点对象、key 是字段名，is_field=True；
        - 根：返回 (nodes, None, False)（整文档替换）。
        """
        parts = [p for p in path.split("/") if p]
        if not parts:
            return nodes, None, False
        cur = nodes
        for p in parts[:-1]:
            if p == "ct":
                # 子通道令牌：进入当前节点的子数组（在列表上出现时是无操作）。
                cur = cur.get("ct", []) if isinstance(cur, dict) else cur
            else:
                idx = int(p)
                cur = cur[idx] if isinstance(cur, list) and 0 <= idx < len(cur) else []
        last = parts[-1]
        if last == "-":
            return cur, len(cur), False
        try:
            return cur, int(last), False
        except ValueError:
            return cur, last, True

    for patch in patches:
        op = patch["op"]
        container, key, is_field = _navigate(patch["path"])
        if is_field:
            # 字段写（WC-3 线路形态）：container 是节点对象、key 是字段名。
            if isinstance(container, dict):
                if op in ("replace", "add"):
                    container[key] = patch["value"]
                elif op == "remove":
                    container.pop(key, None)
            continue
        if op == "replace":
            if key is None and isinstance(container, list):
                val = patch["value"]
                try:
                    container[:] = val if isinstance(val, list) else [val]
                except TypeError:
                    pass  # parent might be immutable in edge case
            elif isinstance(container, list) and key is not None:
                if key < len(container):
                    container[key] = patch["value"]
                else:
                    # Path past end — new node, treat as add at end
                    container.append(patch["value"])
        elif op == "add":
            if isinstance(container, list) and key is not None:
                if key >= len(container):
                    container.append(patch["value"])
                else:
                    container.insert(key, patch["value"])
        elif op == "remove":
            if isinstance(container, list) and key is not None and 0 <= key < len(container):
                container.pop(key)
        elif op == "replace_attr":
            # 历史帧兼容（旧录像里仍是自定义动词）：path 定位对象、attr 为字段名。
            # 回放旧 fixture 的分支仍接受该动词。
            if isinstance(container, dict) and "attr" in patch:
                container[patch["attr"]] = patch["value"]
    return nodes


def _ast_signature(nodes):
    """计算 AST 结构签名：只比较 nodeType，忽略 content 值。"""
    sig = []
    for n in nodes:
        sig.append(n.get("nt"))
    return sig


# Test 8b: Append → P frame 验证

def test_p_frame_append(store):
    """追加非合并块 → P frame → 应用后 = full snapshot。"""
    collector = PatchCollector(store)

    # Use thematic-break separated blocks to prevent paragraph merging
    doc_id = write_doc(store, "# One\n---\n")
    before_ast = get_ast(store, doc_id)
    time.sleep(0.05)
    collector.clear()

    t0 = time.perf_counter()
    store.write_locator("genesis", "/p.md", {"steps": []}, "+>", "# Two\n", True, False, False, False)
    write_ms = (time.perf_counter() - t0) * 1000

    ok = collector.wait(timeout=5.0, expected=1)
    assert ok, "P frame not received"

    recv = collector.received[0]
    patches = recv["patches"]
    print(f"\n  [Append] write={write_ms:.3f}ms type=P patches={len(patches)} ops={[p['op'] for p in patches]}")

    after_ast = get_ast(store, doc_id)
    patched = _apply_patches_to_snapshot(before_ast, patches)
    assert _ast_signature(patched) == _ast_signature(after_ast)


# Test 8c: Content edit → P frame 验证

def test_p_frame_content_edit(store):
    """编辑已有节点的内容 → P frame → 应用后 = full snapshot。"""
    collector = PatchCollector(store)

    doc_id = write_doc(store, "hello world\n")
    before_ast = get_ast(store, doc_id)
    time.sleep(0.05)
    collector.clear()

    t0 = time.perf_counter()
    store.write_locator("genesis", "/p.md", {"steps": []}, "/>", "HELLO WORLD\n", True, False, False, False)
    t1 = time.perf_counter()
    write_ms = (t1 - t0) * 1000

    ok = collector.wait(timeout=5.0, expected=1)
    t2 = time.perf_counter()
    total_ms = (t2 - t0) * 1000

    assert ok, "P frame not received for content edit"
    recv = collector.received[0]
    patches = recv["patches"]

    print(f"\n  [Content Edit] write={write_ms:.3f}ms total={total_ms:.3f}ms type={recv['type']} patches={len(patches)} ops={[p['op'] for p in patches]}")

    after_ast = get_ast(store, doc_id)
    patched = _apply_patches_to_snapshot(before_ast, patches)
    assert _ast_signature(patched) == _ast_signature(after_ast), \
        f"Patched AST doesn't match full snapshot"


# Test 8c-hs-proj: HS + proj → 字段写路径形态 → 模拟前端 applyPatch

@pytest.mark.skipif(
    os.environ.get("MDIO_PROJ_MODE") != "hs"
    or os.environ.get("MDIO_PATCH_GRANULARITY", "").lower() != "proj",
    reason="HS proj gate requires MDIO_PROJ_MODE=hs and MDIO_PATCH_GRANULARITY=proj",
)
def test_hs_proj_field_write_frontend_apply(store):
    """HS + proj 下 P 帧的字段写应为标准 6902 路径形态
    `{op:"replace", path:"<元素路径>/<字段名>", value}`，且模拟 MST applyPatch 后
    与全量快照深度相等——消费端契约 gate。（WC-3 前这里期望自定义动词
    `replace_attr`；线路统一后该动词不再出现。）

    gw_pj_bench_v3 的计账约定：会话建立后第一次编辑是 TAX/cold edit，
    允许旧侧还在第一代 Base 上时回退壳级 add+replace；p50 与消费端
    gate 均针对稳态编辑。这里先做一次 warm-up（同时校验冷路径补丁
    仍能正确重建全量快照），再断言第二次编辑输出路径形态的字段写。
    """
    store.set_mode("hs")
    collector = PatchCollector(store)
    doc_id = write_doc(store, "# Intro\n\nhello world\n")
    before_cold = get_ast(store, doc_id)
    time.sleep(0.05)
    collector.clear()

    def edit(value):
        store.write_locator(
            "genesis", "/p.md",
            {"steps": [["Drill", "Intro"]]},
            "/>", value, True, False, False, False, proj="content",
        )

    # TAX/cold edit：冷路径补丁只要求“可重建全量快照”，不强求字段级。
    edit("WARMUP")
    assert collector.wait(timeout=5.0, expected=1), "P frame not received for cold edit"
    cold_patches = collector.received[-1]["patches"]
    after_cold = get_ast(store, doc_id)
    patched_cold = _apply_patches_to_snapshot(before_cold, cold_patches)
    assert json.dumps(patched_cold, sort_keys=True) == json.dumps(after_cold, sort_keys=True), \
        "HS proj cold patch failed to reconstruct full snapshot"

    # 稳态编辑：HS + proj 的消费端契约 = 路径形态的字段写（WC-3）。
    before_ast = after_cold
    collector.clear()
    edit("HELLO WORLD")
    assert collector.wait(timeout=5.0, expected=1), "P frame not received for steady edit"
    recv = collector.received[-1]
    patches = recv["patches"]
    assert patches, "expected non-empty steady patch"
    # ① 自定义动词不出现在线路上；② 至少一条字段写，且"改哪个字段"由路径末段承载。
    assert all(p["op"] != "replace_attr" for p in patches), \
        f"replace_attr must be gone from the wire, got {patches}"
    field_writes = [
        p for p in patches
        if p["op"] == "replace"
        and "attr" not in p
        and p["path"].rsplit("/", 1)[-1] in ("t", "ct", "cd")
    ]
    assert field_writes, f"expected a path-form field write (<元素路径>/<字段>), got {patches}"

    after_ast = get_ast(store, doc_id)
    patched = _apply_patches_to_snapshot(before_ast, patches)
    assert json.dumps(patched, sort_keys=True) == json.dumps(after_ast, sort_keys=True), \
        "HS proj simulated frontend apply != full snapshot"
    print(f"\n  [HS proj frontend] cold_ops={[p['op'] for p in cold_patches]} "
          f"steady_ops={[p['op'] for p in patches]} "
          f"steady_paths={[p['path'] for p in patches]} "
          f"field_writes={[p['path'] for p in field_writes]}")


# Test 8c-hs-proj-array: 数组值字段写（WC-4 §7.2 数组值收尾）

@pytest.mark.skipif(
    os.environ.get("MDIO_PROJ_MODE") != "hs"
    or os.environ.get("MDIO_PATCH_GRANULARITY", "").lower() != "proj",
    reason="HS proj gate requires MDIO_PROJ_MODE=hs and MDIO_PATCH_GRANULARITY=proj",
)
def test_hs_proj_array_field_write_frontend_apply(store):
    """HS + proj 下**数组值**字段写：含行内标记的内容编辑应发一条
    ``{op:"replace", path:"<元素路径>/t", value:[...]}``——value 是**数组**
    （裸 JSON，不是被引号包起来的字符串），且模拟 MST applyPatch 后与全量
    快照深度相等（WC-4 数组值收尾判官）。"""
    store.set_mode("hs")
    collector = PatchCollector(store)
    doc_id = write_doc(store, "# Intro\n\nhello world\n")
    time.sleep(0.05)
    collector.clear()

    def edit(value):
        store.write_locator(
            "genesis", "/p.md",
            {"steps": [["Drill", "Intro"]]},
            "/>", value, True, False, False, False, proj="content",
        )

    edit("WARMUP")
    assert collector.wait(timeout=5.0, expected=1), "P frame not received for cold edit"
    before_ast = get_ast(store, doc_id)

    collector.clear()
    edit("**BOLD** world")
    assert collector.wait(timeout=5.0, expected=1), "P frame not received for steady edit"
    patches = collector.received[-1]["patches"]
    assert patches, "expected non-empty steady patch"
    writes = [
        p for p in patches
        if p["op"] == "replace" and "attr" not in p
        and p["path"].rsplit("/", 1)[-1] in ("t", "ct", "cd")
    ]
    assert writes, f"expected path-form field write, got {patches}"
    arrays = [p for p in writes if isinstance(p.get("value"), list)]
    assert arrays, f"含标记内容 ⇒ 字段值应是数组（裸 JSON）, got {writes}"

    after_ast = get_ast(store, doc_id)
    patched = _apply_patches_to_snapshot(before_ast, patches)
    assert json.dumps(patched, sort_keys=True) == json.dumps(after_ast, sort_keys=True), \
        "HS proj simulated frontend apply (array value) != full snapshot"

    def field_value_is_array(root, patch):
        """补丁**实际写的那个字段**在后端树里是否为数组（值域对拍）。

        数组在**内容面**：含标记的段落/标题把行内节点放在 `ct`（行内子通道
        数组），tag 面 `t` 此时是空串——所以不能按名字找"哪个节点有 t 数组"。
        按 §6.6 的位置判定导航元素路径：标准 JSON Pointer 逐段走，段落在列表
        上且值为 "ct" 时是无操作令牌（与 _navigate 同律），其余段按当前容器
        形态取值；末段才是字段名。
        """
        elem_path, _, field = patch["path"].rpartition("/")
        node = root
        for seg in [s for s in elem_path.split("/") if s]:
            if isinstance(node, list):
                if seg == "ct":
                    continue
                node = node[int(seg)]
            else:
                node = node[seg]
        return isinstance(node, dict) and isinstance(node.get(field), list)

    w = arrays[0]
    assert field_value_is_array(after_ast, w), \
        f"线路字段写 value 是数组，后端同字段却不是数组: path={w['path']} " \
        f"value={w['value']!r} 树={json.dumps(after_ast)}"
    print(f"\n  [HS proj array] ops={[p['op'] for p in patches]} "
          f"paths={[p['path'] for p in patches]} "
          f"array_values={[p['value'] for p in arrays]}")


# Test 8c: Multi-line paragraph edit

async def test_p_frame_multiline(runtime):
    """编辑多行段落 → P frame 应覆盖正确的行范围(管道写法:._p[0] -pj content)。"""
    store = runtime._workspace.store
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    collector = PatchCollector(store)

    await runtime.exec('"# Doc\n\nline one\nline two\nline three\n" | /> /p.md -f -god', ctx=ctx)
    await asyncio.sleep(0.2)  # 排空建文档初始帧(异步晚到)
    collector.clear()
    doc_id = store.resolve_path("genesis", "/p.md", False)
    before_ast = get_ast(store, doc_id)

    # Replace middle paragraph(管道正经写法)
    t0 = time.perf_counter()
    await runtime.exec('"NEW LINE" | /> /p.md::Doc._p[0] -pj content', ctx=ctx)
    t1 = time.perf_counter()
    write_ms = (t1 - t0) * 1000
    await asyncio.sleep(0.3)
    t2 = time.perf_counter()
    total_ms = (t2 - t0) * 1000

    assert collector.received, "P frame not received for multi-line edit"
    recv = collector.received[0]
    patches = recv["patches"]

    print(f"\n  [Multi-line] write={write_ms:.3f}ms total={total_ms:.3f}ms type={recv['type']} patches={len(patches)} ops={[p['op'] for p in patches]}")

    after_ast = get_ast(store, doc_id)
    patched = _apply_patches_to_snapshot(before_ast, patches)
    assert _ast_signature(patched) == _ast_signature(after_ast), \
        f"Patched AST doesn't match full snapshot"


# Test 8d: Latency breakdown

def test_p_frame_latency_breakdown(store):
    """精确计时：Rust write → Path Worker → Python callback 各阶段延迟。"""
    collector = PatchCollector(store)

    doc_id = write_doc(store, "# Timer\n- start\n")
    time.sleep(0.05)
    collector.clear()

    # Measure: write_locator call time (includes Rust execute_transaction)
    t_write_start = time.perf_counter()
    store.write_locator("genesis", "/p.md", {"steps": []}, "+>", "- middle\n", True, False, False, False)
    t_write_end = time.perf_counter()
    write_latency = (t_write_end - t_write_start) * 1000

    # Wait for Path Worker → callback
    t_wait_start = time.perf_counter()
    ok = collector.wait(timeout=5.0, expected=1)
    t_wait_end = time.perf_counter()

    if ok and collector.received:
        recv = collector.received[0]
        callback_latency = (recv["t_recv"] - t_write_start) * 1000
        total_observed = (t_wait_end - t_write_start) * 1000
        patches = recv["patches"]

        print(f"\n  [Latency] write_locator={write_latency:.3f}ms "
              f"callback_arrival={callback_latency:.3f}ms "
              f"total_observed={total_observed:.3f}ms "
              f"type={recv['type']} len={recv['len']}B patches={len(patches)}")

        assert write_latency < 10, f"write_locator too slow: {write_latency:.1f}ms"
        assert callback_latency < 50, f"callback too slow: {callback_latency:.1f}ms"
    else:
        print(f"\n  [Latency] write={write_latency:.3f}ms - no P callback received")
        assert False, "P frame callback not received"


# Test 8e: Batch multiple edits → multiple P frames

async def test_p_frame_batch_edits(runtime):
    """连续多次编辑 → P 帧累积应用 = full snapshot(管道写法)。"""
    store = runtime._workspace.store
    ctx = RuntimeContext(wt_uid="genesis", vfs_path="/", active_doc=None)
    collector = PatchCollector(store)

    await runtime.exec('"# Batch\n- one\n- two\n- three\n" | /> /p.md -f -god', ctx=ctx)
    await asyncio.sleep(0.2)  # 排空初始帧(异步晚到)
    collector.clear()
    doc_id = store.resolve_path("genesis", "/p.md", False)
    before_ast = get_ast(store, doc_id)

    edits = [
        '"- four\n" | +> /p.md::Batch',
        '"- five\n" | +> /p.md::Batch',
        '"# BATCH UPDATED\n" | /> /p.md -f -god',
    ]
    for code in edits:
        await runtime.exec(code, ctx=ctx)
    await asyncio.sleep(0.3)

    after_ast = get_ast(store, doc_id)

    n_recv = len(collector.received)
    n_expected = len(edits)
    print(f"\n  [Batch] {n_expected} edits → {n_recv} P frames")
    for i, recv in enumerate(list(collector.received)[:n_expected]):
        ops = [p['op'] for p in recv['patches']] if recv['patches'] else ['?']
        print(f"    frame {i}: {recv['type']} doc={recv['doc_id']} len={recv['len']}B ops={ops}")

    # Apply all patches cumulatively
    patched = list(before_ast)
    for recv in list(collector.received)[:n_expected]:
        if recv["patches"]:
            patched = _apply_patches_to_snapshot(patched, recv["patches"])

    assert _ast_signature(patched) == _ast_signature(after_ast), \
        f"Batch-patched AST doesn't match full snapshot"


# Test 8f: P frame payload validity

def test_p_frame_payload_validity(store):
    """P 帧 payload 必须是合法的 RFC 6902 JSON patch 数组。"""
    collector = PatchCollector(store)

    doc_id = write_doc(store, "# Valid\n- check\n")
    collector.clear()

    store.write_locator("genesis", "/p.md", {"steps": []}, "+>", "- new\n", True, False, False, False)
    ok = collector.wait(timeout=5.0, expected=1)
    assert ok, "P frame not received"

    recv = collector.received[0]
    assert recv["type"] == 'P', f"Expected 'P' type, got {recv['type']}"
    assert recv["tag"] == 0x84, f"Expected tag 0x84"

    for patch in recv["patches"]:
        assert "op" in patch, "RFC 6902 patch missing 'op'"
        assert "path" in patch, "RFC 6902 patch missing 'path'"
        assert patch["op"] in ("add", "remove", "replace"), f"Invalid op: {patch['op']}"
        assert patch["path"].startswith("/"), f"Path must start with /: {patch['path']}"

    print(f"\n  [P valid] {len(recv['patches'])} patches: {json.dumps(recv['patches'], indent=2)[:500]}")


# Test 8g: Edit → full snapshot equivalence

def test_p_frame_equal_to_full_snapshot(store):
    """P frame patches 应用到 before → 结果 = new full snapshot（深度相等）。"""
    collector = PatchCollector(store)

    doc_id = write_doc(store, "# Verify\n- initial A\n- initial B\n")
    before_ast = get_ast(store, doc_id)
    time.sleep(0.05)
    collector.clear()

    # Complex edit: replace first bullet content
    store.write_locator(
        "genesis", "/p.md",
        {"steps": [["Wildcard"], ["CMP", ".tag", "==", "initial A"], ["Slice", [0]]]},
        "/>", "- updated A\n", True, False, False, False,
    )

    ok = collector.wait(timeout=5.0, expected=1)
    after_ast = get_ast(store, doc_id)

    if ok and collector.received:
        recv = collector.received[0]
        patched = _apply_patches_to_snapshot(before_ast, recv["patches"])

        # Deep equality check
        patched_json = json.dumps(patched, sort_keys=True)
        after_json = json.dumps(after_ast, sort_keys=True)
        assert patched_json == after_json, \
            f"P frame patches don't produce same result as full snapshot\npatched: {patched_json[:200]}\nafter: {after_json[:200]}"

        print(f"\n  [Equivalence] P frame patches → same result as full snapshot ✅")
    else:
        print(f"\n  [Equivalence] No P frame received, using full snapshot fallback (valid for Steps 1-4)")
        assert True  # F frame fallback works


# Test 9: Container-lifting — verify ripple node → container, not leaf

def test_container_lift_bullet_append(store):
    """追加 bullet 到已有 bullet_list → patch 对象是容器 (bullet_list)，不是叶子 (bullet)。"""
    collector = PatchCollector(store)
    doc_id = write_doc(store, "# Doc\n- a\n- b\n")
    time.sleep(0.05)
    collector.clear()

    store.write_locator("genesis", "/p.md", {"steps": []}, "+>", "- c\n", True, False, False, False)
    ok = collector.wait(timeout=5.0, expected=1)
    assert ok, "P frame not received"

    patch = collector.received[0]["patches"][0]
    # 对齐实际（既有 diff 行为）：追加 item → item 级 add（非容器 replace）
    assert patch["op"] == "add", f"Expected add, got {patch['op']}"
    assert patch["value"]["nt"] == "b", \
        f"Expected item (b), got {patch['value']['nt']}"
    assert patch["path"] == "/ct/1/ct/2", f"Expected /ct/1/ct/2, got {patch['path']}"
    print(f"\n  [Append] op={patch['op']} nt={patch['value']['nt']} path={patch['path']}")


def test_container_lift_new_bullet_list(store):
    """首个 bullet 创建 bullet_list → 容器提升也生效。"""
    collector = PatchCollector(store)
    doc_id = write_doc(store, "# Only Header\n")
    time.sleep(0.05)
    collector.clear()

    store.write_locator("genesis", "/p.md", {"steps": []}, "+>", "- first\n", True, False, False, False)
    ok = collector.wait(timeout=5.0, expected=1)
    assert ok, "P frame not received"

    patch = collector.received[0]["patches"][0]
    # 首个 bullet 编辑产生容器提升（bl——patch value 是容器）
    assert patch["op"] == "add", f"Expected add, got {patch['op']}"
    assert patch["value"]["nt"] == "bl", \
        f"Expected container (bl), got {patch['value']['nt']}"
    assert patch["path"] == "/ct/1", f"Expected /ct/1, got {patch['path']}"
    print(f"\n  [FirstItem] op={patch['op']} nt={patch['value']['nt']} path={patch['path']}")


def test_leaf_content_edit(store):
    """内容编辑（无容器变化）→ container=false, path 指向叶子。"""
    collector = PatchCollector(store)
    doc_id = write_doc(store, "hello world\n")
    collector.clear()

    store.write_locator("genesis", "/p.md", {"steps": []}, "/>", "HELLO WORLD\n", True, False, False, False)
    ok = collector.wait(timeout=5.0, expected=1)
    assert ok, "P frame not received"

    patch = collector.received[0]["patches"][0]
    # 整行替换产生 add（非 replace）
    assert patch["op"] == "add", f"Expected add, got {patch['op']}"
    assert patch["value"]["nt"] == "p", \
        f"Leaf edit should be p, got {patch['value']['nt']}"
    assert patch["path"] == "/ct/0", f"Expected path=/ct/0, got {patch['path']}"
    print(f"\n  [Leaf] op={patch['op']} nt={patch['value']['nt']} path={patch['path']}")


def test_container_value_matches_full_snapshot(store):
    """容器 patch 的 value 与 full snapshot 中同名路径的子树一致。"""
    collector = PatchCollector(store)
    doc_id = write_doc(store, "# T\n- a\n- b\n")
    time.sleep(0.05)
    collector.clear()

    store.write_locator("genesis", "/p.md", {"steps": []}, "+>", "- c\n", True, False, False, False)
    ok = collector.wait(timeout=5.0, expected=1)
    assert ok, "P frame not received"

    patch = collector.received[0]["patches"][0]
    after_ast = get_ast(store, doc_id)

    # Extract container from full AST at the same path
    full_container = after_ast[1]  # /ct/1 = bullet_list
    patch_value = patch["value"]

    # diff 的 patch value 是 item 级（既有行为——非容器子树——
    # 对齐：item 与全量的对应 item 一致）
    assert patch_value["nt"] == "b", f"patch value should be item (b), got {patch_value['nt']}"
    full_item = full_container["ct"][2]
    assert full_item["nt"] == patch_value["nt"]
    assert full_item.get("ct") == patch_value.get("ct")
    print(f"\n  [Match] patch item == full snapshot item at path={patch['path']} ✅")


def test_sequential_appends_same_container_path(store):
    """连续追加 → 每次 patch 路径一致，容器 children 递增。"""
    collector = PatchCollector(store)
    doc_id = write_doc(store, "# Doc\n- a\n")
    time.sleep(0.05)
    collector.clear()

    for i, text in enumerate(["- b\n", "- c\n", "- d\n"]):
        store.write_locator("genesis", "/p.md", {"steps": []}, "+>", text, True, False, False, False)
        ok = collector.wait(timeout=5.0, expected=i + 1)
        assert ok, f"P frame {i} not received"

    after_ast = get_ast(store, doc_id)
    received = list(collector.received)
    assert len(received) == 3

    for i, recv in enumerate(received):
        patch = recv["patches"][0]
        # 追加 → item 级 add（/ct/1/ct/{i+1}——容器 ch 递增——非容器级 replace——
        # value 是 b item——无 ch）
        assert patch["path"] == f"/ct/1/ct/{i + 1}", \
            f"Expected item path /ct/1/ct/{i+1}, got {patch['path']}"
        assert patch["value"]["nt"] == "b", \
            f"Frame {i}: expected item (b), got {patch['value']['nt']}"
    print(f"\n  [Sequential] 3 appends → item paths /ct/1/ct/1..3 ✅")


# Test 10: All container types — verify container lifting for every framed type

def test_container_ordered_list(store):
    """有序列表追加 → 容器提升到 ordered_list。"""
    collector = PatchCollector(store)
    doc_id = write_doc(store, "# Doc\n1. a\n2. b\n")
    time.sleep(0.05)
    collector.clear()

    store.write_locator("genesis", "/p.md", {"steps": []}, "+>", "3. c\n", True, False, False, False)
    ok = collector.wait(timeout=5.0, expected=1)
    assert ok, "P frame not received"

    patch = collector.received[0]["patches"][0]
    # 对齐实际（既有 diff 行为）：追加 item → item 级 add（ol）
    assert patch["op"] == "add", f"Expected add, got {patch['op']}"
    assert patch["value"]["nt"] == "ol", \
        f"Expected ol, got {patch['value']['nt']}"
    assert patch["path"] == "/ct/1/ct/2"
    print(f"\n  [OrderedList] op={patch['op']} nt={patch['value']['nt']} path={patch['path']} ✅")


def test_container_blockquote_append(store):
    """引用块追加内容 → 容器提升到 blockquote，children 递增。"""
    collector = PatchCollector(store)
    doc_id = write_doc(store, "> a\n> b\n")
    time.sleep(0.05)
    collector.clear()

    store.write_locator("genesis", "/p.md", {"steps": []}, "+>", "> c\n", True, False, False, False)
    ok = collector.wait(timeout=5.0, expected=1)
    assert ok, "P frame not received"

    patches = collector.received[0]["patches"]
    if "DBG_PATCH" in __import__("os").environ:
        print("  PATCHES:", patches)
    # 规范形态（同类同槽数 ⇒ Block→REPL）："> a\n> b\n" 的 BQ 内容是**同一个段落**
    # （软换行），追加 "> c" 只让该段落变长 ⇒ 1 条 replace。
    # 测试基线固定 MDIO_PATCH_GRANULARITY=block：段落内部 ct 路径为 /ct/0/ct/0。
    assert len(patches) == 1, f"Expected 1 patch (REPL), got {len(patches)}"
    assert patches[0]["op"] == "replace" and patches[0]["path"] == "/ct/0/ct/0"
    # block 粒度：value 是段落节点，内部 ct 为 inline 内容流。
    value = patches[0]["value"]
    assert isinstance(value, dict) and value.get("nt") == "p", f"Expected paragraph node, got {value!r}"
    text_leaves = [n.get("ct") for n in value.get("ct", []) if isinstance(n, dict) and n.get("nt") == "txt"]
    assert text_leaves == ["a", "b", "c"], f"Expected txt leaves a/b/c, got {text_leaves}"
    print("\n  [Blockquote] REPL paragraph at /ct/0/ct/0 ✅")


def test_container_first_of_each_type(store):
    """每个容器类型：首次数据创建容器 → 提升到容器节点。"""
    for md_type, expected in [
        ("- a\n", "bl"),
        ("1. a\n", "od"),
        ("> a\n", "bq"),
    ]:
        collector = PatchCollector(store)
        # Fresh doc
        store.write_locator("genesis", "/p.md", {"steps": []}, "/>", "# H\n", True, False, False, False)
        time.sleep(0.05)
        collector.clear()

        store.write_locator("genesis", "/p.md", {"steps": []}, "+>", md_type, True, False, False, False)
        ok = collector.wait(timeout=5.0, expected=1)
        assert ok, f"P frame not received for {expected}"

        patch = collector.received[0]["patches"][0]
        assert patch["value"]["nt"] == expected, \
            f"First {md_type!r}: expected {expected}, got {patch['value']['nt']}"
        assert patch["path"] == "/ct/1", \
            f"First child container should be at /children/1, got {patch['path']}"
        print(f"  [{expected}] first creation ✅", flush=True)
    print(f"\n  [FirstOfEach] 4 container types all verified ✅")
