"""测试 workspace.write_locator 的 CRDT 广播行为。

验证 **唯一投递出口 deliver**：
  1. 返回值仍是 bytes
  2. 未配置 channels 时写入正常完成
  3. 非空编辑抵达唯一出口，帧成型正确（DN_SYNC / len / peer=0）
  4. 不同文件走各自的 doc_id
"""
import struct
import asyncio
import pytest
from mdio_py import MdioRuntime
from mdio_py.core.selector_builder import SelectorBuilder, drill
import mdio_py.gateway.crdt_scope as _cs


class _Rec:
    """投递出口记录器：记录后照常调用真实现。"""

    def __init__(self):
        self.calls = []

    async def __call__(self, lookup, doc_id, frame, kind, origin=None, store=None):
        self.calls.append({"doc_id": doc_id, "kind": kind, "packet": bytes(frame)})
        return await self._real(lookup, doc_id, frame, kind, origin=origin, store=store)


def _install():
    rec = _Rec()
    rec._real = _cs.deliver
    _cs.deliver = rec
    return rec

DN_SYNC = 0x83
HEADER_SIZE = 13


def _setup(tmp_path):
    """创建最小环境并生成一个空文件。"""
    rt = MdioRuntime(str(tmp_path))
    rt._workspace.write_locator("genesis", "/test.md",
        SelectorBuilder().build(), "+>", "# OK\n", god=True)
    return rt


@pytest.mark.asyncio
async def test_write_locator_returns_bytes_unchanged(tmp_path):
    """返回值类型仍是 bytes，所有现有调用方不受影响。"""
    rt = _setup(tmp_path)
    result = rt._workspace.write_locator("genesis", "/test.md",
        drill("OK"), "/>", "X")
    assert isinstance(result, bytes)


@pytest.mark.asyncio
async def test_no_channels_no_error(tmp_path):
    """未配置 channels 时写入正常完成，无异常。"""
    rt = _setup(tmp_path)
    assert not hasattr(rt._workspace, '_channels')
    rt._workspace.write_locator("genesis", "/test.md",
        drill("OK"), "/>", "Y")


@pytest.mark.asyncio
async def test_with_channels_publishes_dn_sync(tmp_path):
    """配置 channels 后，非空编辑应 publish DN_SYNC 包。"""
    rt = _setup(tmp_path)
    rec = _install()
    await asyncio.sleep(0.05)   # 让 _setup 里那笔写的延后投递先跑完
    rec.calls.clear()

    rt._workspace.write_locator("genesis", "/test.md",
        drill("OK"), "/>", "Z")
    await asyncio.sleep(0.03)   # 投递走 create_task ⇒ 等一拍

    assert len(rec.calls) == 1, f"应恰好一笔投递，实际 {len(rec.calls)}"
    packet = rec.calls[0]["packet"]
    assert len(packet) > HEADER_SIZE, f"packet too short: {len(packet)} bytes"
    tag, payload_len, peer, doc = struct.unpack_from('<BIII', packet)
    assert tag == DN_SYNC
    assert payload_len == len(packet) - HEADER_SIZE
    assert peer == 0  # peer 是 server 固定值 0
    assert doc == rec.calls[0]["doc_id"]


@pytest.mark.asyncio
async def test_doc_id_per_document(tmp_path):
    """不同文件获得各自的 doc_id 和 channel。"""
    rt = _setup(tmp_path)
    # 创建第二个文件
    rt._workspace.write_locator("genesis", "/other.md",
        SelectorBuilder().build(), "+>", "# B\n", god=True)

    rec = _install()
    await asyncio.sleep(0.05)   # 让 _setup 里那笔写的延后投递先跑完
    rec.calls.clear()

    rt._workspace.write_locator("genesis", "/test.md",
        drill("OK"), "/>", "A")
    rt._workspace.write_locator("genesis", "/other.md",
        drill("B"), "/>", "C")
    await asyncio.sleep(0.05)

    assert len(rec.calls) == 2, f"应两笔投递，实际 {len(rec.calls)}"
    assert rec.calls[0]["doc_id"] != rec.calls[1]["doc_id"], "不同文件应有不同 doc_id"

if __name__ == "__main__":
    import sys, pytest
    sys.exit(pytest.main(["-v", "--tb=short", __file__] + sys.argv[1:]))
