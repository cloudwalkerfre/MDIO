"""M4-1：VFS 树 hidden 标志语义——WS 树（UP_TREE 0x01 应答）现在传 True，
声明本体（/.mds/*）与普通文档同是树上的节点（P1：树本身承载一切）。"""
import json


async def test_vfs_tree_hidden_flag(runtime):
    raw = runtime.store.get_vfs_tree("genesis", True)
    data = json.loads(raw.decode()) if isinstance(raw, bytes) else json.loads(raw)
    paths = [f.get("path", "") for f in data]
    assert "/.mds/env.md" in paths, "hidden=True 必须包含 .mds 节点（fixture 里 env.md 是种子）"

    raw2 = runtime.store.get_vfs_tree("genesis", False)
    data2 = json.loads(raw2.decode()) if isinstance(raw2, bytes) else json.loads(raw2)
    paths2 = [f.get("path", "") for f in data2]
    assert "/.mds/env.md" not in paths2, "hidden=False 保持旧语义（供对照）"