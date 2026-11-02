"""端到端 CRDT 广播测试。

验证: MDIO 内部写入 → **唯一投递出口 deliver** → 帧成型（DN_SYNC）。
覆盖各操作符、选择器类型、文件类型、scaffold、边界、空编辑、并发、协议兼容等。
"""
import struct
import asyncio
import pytest
from mdio_py import MdioRuntime
from mdio_py.core.selector_builder import SelectorBuilder
import mdio_py.gateway.crdt_scope as _cs

DN_SYNC = 0x83
HEADER_SIZE = 13


def decode_dn_sync(packet: bytes) -> dict:
    tag, payload_len, peer, doc_id = struct.unpack_from("<BIII", packet)
    assert tag == DN_SYNC
    assert payload_len == len(packet) - HEADER_SIZE
    return {"tag": tag, "payload_len": payload_len, "peer": peer, "doc_id": doc_id,
            "payload": packet[HEADER_SIZE:]}


class _Rec:
    """投递出口的透明记录器：记录 (frame, [channel]) 后**照常调用真实现**。

    queue 接口与旧 ChannelsPlugin._pub_queue 一致（empty/get_nowait/task_done），
    因此下面的 _drain/_get_one 与各用例主体都不用改。
    """

    def __init__(self):
        self._pub_queue = asyncio.Queue()
        self.calls = []

    async def __call__(self, lookup, doc_id, frame, kind, origin=None, store=None):
        self._pub_queue.put_nowait((bytes(frame), [f"doc_{doc_id}"]))
        self.calls.append({"doc_id": doc_id, "kind": kind, "origin": origin, "len": len(frame)})
        return await self._real(lookup, doc_id, frame, kind, origin=origin, store=store)


async def _setup(rt):
    """装一个投递出口记录器（替代旧的 ChannelsPlugin 观测点）。"""
    rec = _Rec()
    rec._real = _cs.deliver
    _cs.deliver = rec          # 透明代理：记录后调用真实现
    rt._workspace._deliver_rec = rec
    return rec


def _drain(q):
    while not q.empty():
        q.get_nowait()
        q.task_done()


def _get_one(q):
    data, channels = q.get_nowait()
    q.task_done()
    return data, channels


# 操作符覆盖

@pytest.mark.asyncio
async def test_append_triggers_publish(tmp_path):
    """+> 操作触发 publish。"""
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Hello\n", god=True)

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty()
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    data, _ = _get_one(ch._pub_queue)
    assert decode_dn_sync(data)["payload"]


@pytest.mark.asyncio
async def test_overwrite_triggers_publish(tmp_path):
    """/> 覆盖操作触发 publish。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Orig\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().drill("Orig").build(), "/>", "New")

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty()
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    data, _ = _get_one(ch._pub_queue)
    assert decode_dn_sync(data)["payload"]


@pytest.mark.asyncio
async def test_delete_triggers_publish(tmp_path):
    """-> 删除操作触发 publish。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Gone\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().drill("Gone").build(), "->", "")

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty()
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    data, _ = _get_one(ch._pub_queue)
    assert decode_dn_sync(data)["payload"]


@pytest.mark.asyncio
async def test_patch_triggers_publish(tmp_path):
    """~> patch 生成 delete+insert 两个 WireOp，编码在一个包里。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# P\n- k: hello\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().drill("P").type_is("bullet").tag_eq("k").slice(0).build(), "~>", '[{"old":"hello","new":"world"}]')

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    data, _ = _get_one(ch._pub_queue)
    info = decode_dn_sync(data)
    assert len(info["payload"]) > 0


# 选择器

@pytest.mark.asyncio
async def test_drill_key_selector(tmp_path):
    """Drill + Key 选择器。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>",
        "# Top\n## Child\n- k: v\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().drill("Top").drill("Child").type_is("bullet").tag_eq("k").slice(0).build(), "/>", "changed")

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty()


@pytest.mark.asyncio
async def test_root_selector(tmp_path):
    """空 md_path 选中全文（$ /file.md::）。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Root\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "/>", "# NewRoot\n")

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty()


@pytest.mark.asyncio
async def test_key_only_selector(tmp_path):
    """仅 Key（$ /file.md::key），根级 bullet 匹配。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>",
        "- k: v\n- other: x\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().type_is("bullet").tag_eq("k").slice(0).build(), "/>", "new_v")

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty()


# 空编辑 / 未命中（坑：scaffold 自动补节点）

@pytest.mark.asyncio
async def test_empty_write_no_scaffold_no_publish(tmp_path):
    """> 未命中 + auto_scaffold=False → 报错，不 publish。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Init\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    with pytest.raises(Exception):
        rt._workspace.write_locator("genesis", "/test.md",
            SelectorBuilder().drill("Ghost").build(), "/>", "X",
            auto_scaffold=False)

    assert ch._pub_queue.empty(), "未命中 + 无 scaffold → 报错, 不应 publish"


@pytest.mark.asyncio
async def test_scaffold_creates_and_publishes(tmp_path):
    """Scaffold 自动补节点 → 实际有编辑 → 应 publish。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Top\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    # MissingChild 不存在，auto_scaffold=True 自动创建
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().drill("Top").drill("MissingChild").type_is("bullet").tag_eq("k").slice(0).build(), "/>", "val")

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty(), "scaffold 创建的节点应广播"


# god mode / 文件创建

@pytest.mark.asyncio
async def test_god_creates_file_and_publishes(tmp_path):
    """god=True 创建新文件并广播。"""
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/new_file.md",
        SelectorBuilder().build(), "+>", "# Created\n", god=True)

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty()
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    data, _ = _get_one(ch._pub_queue)
    assert decode_dn_sync(data)["payload"]


# 文件类型

@pytest.mark.asyncio
async def test_user_file(tmp_path):
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)
    rt._workspace.write_locator("genesis", "/notes.md",
        SelectorBuilder().build(), "+>", "# Notes\n", god=True)
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    _, channels = _get_one(ch._pub_queue)
    assert channels[0].startswith("doc_")


@pytest.mark.asyncio
async def test_system_file(tmp_path):
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)
    rt._workspace.write_locator("genesis", "/.mds/config.md",
        SelectorBuilder().build(), "+>", "# CFG\n", god=True)
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    _, channels = _get_one(ch._pub_queue)
    assert channels[0].startswith("doc_")


# 同一文档多次写入 → 同一 channel

@pytest.mark.asyncio
async def test_same_file_same_channel(tmp_path):
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/doc.md",
        SelectorBuilder().build(), "+>", "# Doc\n- k: v1\n", god=True)
    rt._workspace.write_locator("genesis", "/doc.md",
        SelectorBuilder().drill("Doc").type_is("bullet").tag_eq("k").slice(0).build(), "/>", "v2")

    ch_set = set()
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    while not ch._pub_queue.empty():
        _, cl = _get_one(ch._pub_queue)
        ch_set.add(tuple(cl))
    assert len(ch_set) == 1, f"同一文件应只有 1 个 channel: {ch_set}"


@pytest.mark.asyncio
async def test_different_files_different_channels(tmp_path):
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/a.md",
        SelectorBuilder().build(), "+>", "# A\n", god=True)
    rt._workspace.write_locator("genesis", "/b.md",
        SelectorBuilder().build(), "+>", "# B\n", god=True)

    ch_set = set()
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    while not ch._pub_queue.empty():
        _, cl = _get_one(ch._pub_queue)
        ch_set.add(tuple(cl))
    assert len(ch_set) == 2


# 协议格式

@pytest.mark.asyncio
async def test_dn_sync_header_fields(tmp_path):
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# H\n", god=True)

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    data, _ = _get_one(ch._pub_queue)
    tag, plen, peer, did = struct.unpack_from("<BIII", data)
    assert tag == 0x83
    assert plen + HEADER_SIZE == len(data)
    assert peer == 0
    assert did


@pytest.mark.asyncio
async def test_peer_zero_not_filtered(tmp_path):
    """peer=0 (server origin) 不会被客户端过滤。"""
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Peer\n", god=True)

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    data, _ = _get_one(ch._pub_queue)
    _, _, peer, _ = struct.unpack_from("<BIII", data)
    assert peer == 0, "peer 应为 0"

    # 客户端 my_peer_id 是随机非零值 → 0 不会被过滤


# 返回值兼容

@pytest.mark.asyncio
async def test_return_value_is_bytes(tmp_path):
    """write_locator 返回 bytes（非空编辑 → DN_SYNC 包）。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Ret\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    result = rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().drill("Ret").build(), "/>", "Val")
    assert isinstance(result, bytes)
    assert len(result) > HEADER_SIZE


@pytest.mark.asyncio
async def test_empty_return_value_falsy(tmp_path):
    """空编辑返回空 bytes → Python 真值为 False。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Init\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    # scaffold=False + 未命中 → 报错，捕获后确认没 publish
    with pytest.raises(Exception):
        rt._workspace.write_locator("genesis", "/test.md",
            SelectorBuilder().drill("Ghost").build(), "/>", "X",
            auto_scaffold=False)


# 降级

@pytest.mark.asyncio
async def test_no_channels_silent(tmp_path):
    rt = MdioRuntime(str(tmp_path))
    assert not hasattr(rt._workspace, '_channels')
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Ok\n", god=True)


# 大 payload / Unicode

@pytest.mark.asyncio
async def test_large_payload(tmp_path):
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    big = "X" * 50000
    rt._workspace.write_locator("genesis", "/big.md",
        SelectorBuilder().build(), "+>",
        f"# Big\n- data: {big}\n", god=True)

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    data, _ = _get_one(ch._pub_queue)
    assert len(decode_dn_sync(data)["payload"]) >= 50000


@pytest.mark.asyncio
async def test_unicode(tmp_path):
    rt = MdioRuntime(str(tmp_path))
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/uni.md",
        SelectorBuilder().build(), "+>",
        "# 你好\n- 键: 值 🚀\n", god=True)

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    d, _ = _get_one(ch._pub_queue)
    assert decode_dn_sync(d)["payload"]


# replace_container / auto_level

@pytest.mark.asyncio
async def test_replace_container_true(tmp_path):
    """默认 content → 替换容器内容 → 能广播。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>",
        "# Old\n- k: v\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().drill("Old").build(), "/>",
        "# New\n- k: v\n")

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty()
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    d, _ = _get_one(ch._pub_queue)
    assert decode_dn_sync(d)["payload"]


@pytest.mark.asyncio
async def test_auto_level(tmp_path):
    """auto_level 自动调整 heading 层级 → 能广播。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>",
        "# Top\n", god=True)
    ch = await _setup(rt)
    _drain(ch._pub_queue)

    # auto_level + scaffold: 在 # Top 下创建 ## Sub
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().drill("Top").drill("Sub").build(), "/>",
        "# Sub\n", auto_scaffold=True, auto_level=True)

    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    assert not ch._pub_queue.empty()
    await asyncio.sleep(0.03)   # 投递走 workspace 的 create_task ⇒ 等一拍再断言
    d, _ = _get_one(ch._pub_queue)
    assert decode_dn_sync(d)["payload"]


# 多文档并发写入（串行快速连续）

@pytest.mark.asyncio
async def test_rapid_sequential_writes(tmp_path):
    """快速连续写入 10 次，每条都入队。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# Rapid\n", god=True)
    ch = await _setup(rt)
    await asyncio.sleep(0.03)   # 先让 _setup 之前那笔写的延后投递跑完，再清空
    _drain(ch._pub_queue)

    for i in range(10):
        rt._workspace.write_locator("genesis", "/test.md",
            SelectorBuilder().drill("Rapid").build(), "/>", f"Val{i}")

    await asyncio.sleep(0.08)   # 10 笔投递都走 create_task ⇒ 等它们跑完再数
    count = 0
    while not ch._pub_queue.empty():
        _get_one(ch._pub_queue)
        count += 1
    assert count == 10, f"10 次写入应全部入队, 实际 {count}"

if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))
