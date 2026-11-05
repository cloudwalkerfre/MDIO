"""Gateway 三层架构 — 端到端测试。

使用 MdioRuntime 真实实例，覆盖：
  - types: RemoteDecl.from_ticket/from_files, Active.compute_state
  - store: mount → reconcile → Handler start → tick 投影
  - store: share → unmount/unshare → deactivate_wt → 清理
  - handler: configure, stop, start, _auth_token self-inject
  - 回调: _on_handler_event, on_document_written 触发 reconcile
"""

import asyncio
import base64
import json
import os
import time

import pytest
from mdio_py import MdioRuntime
from mdio_py.gateway.types import RemotePhase


# Part 1: 纯类型 — 无 I/O


def test_remote_decl_from_ticket():
    from mdio_py.gateway.types import RemoteDecl

    ticket_data = json.dumps({"addr": "127.0.0.1:1234", "token": "my_token", "path": "/docs"})
    b64 = base64.urlsafe_b64encode(ticket_data.encode()).decode().rstrip("=")

    d = RemoteDecl.from_ticket(f"iroh-share://{b64}", "uid_abc", "team-a")

    assert d.wt_uid == "uid_abc"
    assert d.path == "/share/team-a"
    assert d.entry_id == "team-a"
    assert d.token == "my_token"
    assert d.addr == "127.0.0.1:1234"
    assert d.local_prefix == "/share/team-a"
    assert d.remote_prefix == "/docs"


def test_remote_decl_from_files():
    from mdio_py.gateway.types import RemoteDecl

    rule_entries = [("/share/team-a", "team-a"), ("/share/team-b", "team-b")]
    gateway_entries = [
        {"_title": "team-a", "shared_token": "tok_a", "remote_addr": "addr_a",
         "local_prefix": "/share/team-a", "remote_prefix": "/docs"},
        {"_title": "team-c", "shared_token": "tok_c"},
    ]

    result = RemoteDecl.from_files("w1", rule_entries, gateway_entries)

    assert len(result) == 1
    assert result[0].wt_uid == "w1"
    assert result[0].entry_id == "team-a"
    assert result[0].token == "tok_a"
    assert result[0].addr == "addr_a"


def test_remote_phase():
    from mdio_py.gateway.types import RemotePhase

    assert RemotePhase.ACTIVE.value == "active"
    assert RemotePhase.PENDING.value == "pending"
    assert RemotePhase.ACTIVE != RemotePhase.PENDING


def test_remote_decl_frozen():
    from mdio_py.gateway.types import RemoteDecl

    d = RemoteDecl(wt_uid="w1", path="/share/x", entry_id="x",
                   token="t", addr=None, local_prefix="/x", remote_prefix=None)
    try:
        d.wt_uid = "other"
        assert False, "should not reach here"
    except Exception:
        pass


# Part 2: Handler — 真实实例


def test_handler_configure_stop_start():
    from mdio_py.gateway.iroh_transport import ConnectionHandler
    from mdio_py.gateway.types import RemoteDecl

    async def _test():
        h = ConnectionHandler()
        d = RemoteDecl(wt_uid="w1", path="/share/x", entry_id="x",
                       token="tok", addr="addr1", local_prefix="/x",
                       remote_prefix="/remote")
        h.configure(d)
        assert h.addr == "addr1"
        assert h._saved_token == b"tok"
        assert h.wt_uid == "w1"
        assert h.entry_id == "x"
        assert h.local_prefix == "/x"
        assert h.remote_prefix == "/remote"
        assert h.doc_map == {}

        h.stop()
        assert h.state == "idle"

        h.start()
        assert h._task is not None
        h.stop()

    asyncio.run(_test())


def test_handler_notify_state_change():
    from mdio_py.gateway.iroh_transport import ConnectionHandler

    h = ConnectionHandler()
    h.wt_uid = "w1"
    h.entry_id = "e1"
    events = []

    def cb(key, state):
        events.append((key, state))

    h.on_state_change = cb
    h._notify_state_change("connected")
    assert events == [("w1:e1", "connected")]

    h._notify_state_change("disconnected")
    assert events == [("w1:e1", "connected"), ("w1:e1", "disconnected")]

    # None callback should not crash
    h2 = ConnectionHandler()
    h2.wt_uid = "w2"
    h2.entry_id = "e2"
    h2._notify_state_change("connected")


def test_handler_auth_token_self_inject():
    from mdio_py.gateway.iroh_transport import ConnectionHandler
    from mdio_py.gateway.types import RemoteDecl, Active
    from mdio_py.gateway.crdt_scope import CrdtScope

    scope = CrdtScope("w1")
    decl = RemoteDecl(wt_uid="w1", path="/share/x", entry_id="x",
                      token="tok", addr=None, local_prefix="/docs",
                      remote_prefix=None)
    active = Active(key="w1:x", decl=decl, scope=scope, handler=None)
    lookup = {b"tok": active}

    async def _test():
        h = ConnectionHandler()
        h._lookup = lambda t: lookup.get(t)
        result = []

        async def _reply(d):
            result.append(d)

        await h._auth_token(b"tok", "peer_1", _reply)
        assert active.handler is h
        assert h.wt_uid == "w1"
        assert h.entry_id == "x"
        assert h.local_prefix == "/docs"
        assert "peer_1" in scope.peers

        # invalid token
        result.clear()
        h2 = ConnectionHandler()
        h2._lookup = lambda t: None
        await h2._auth_token(b"bad", "p1", _reply)
        assert result == [b"ERR:invalid_token"]

    asyncio.run(_test())


def test_classify_frame():
    from mdio_py.gateway.iroh_transport import classify_frame

    is_tok = lambda t: t == b"tok"
    assert classify_frame(b"tok", is_token_fn=is_tok) == "token"
    assert classify_frame(b"tok|payload", is_token_fn=is_tok) == "token"
    assert classify_frame(b"other", is_token_fn=is_tok) == "unknown"
    assert classify_frame(b"\x01" + b"\x00" * 12, is_token_fn=is_tok) == "crdt"
    assert classify_frame(b'{"type":"doc_map"}', is_token_fn=is_tok) == "doc_map"


# Part 3: GatewayStore — 真实 MdioRuntime


from mdio_py.gateway.store import GatewayStore


def _gs(tmp_path):
    rt = MdioRuntime(str(tmp_path))
    gs = GatewayStore(store=rt.store, workspace=rt._workspace, iroh_endpoint=None)
    return gs, rt


def _mk_wt(gateway_store, rt, name, rule=""):
    # 与 bfork 一致：derive 只建内存树节点，须写 worktrees.md 条目，
    # 否则任何 tree_ensure（mount/write_locator auto_scaffold）会清掉该 WT。
    from mdio_py.std.worktree_ops import _append_worktree_entry
    uid = rt.store.derive_worktree("genesis", name, rule)
    _append_worktree_entry(rt._workspace, name, uid, parent_wt="genesis", rule=rule)
    return uid


@pytest.fixture
def tmp_path_fixture(tmp_path):
    return tmp_path


@pytest.mark.asyncio
async def test_store_mount(tmp_path):
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_mount")

    ticket_data = json.dumps({"addr": "addr1", "token": "tok1", "path": "/docs"})
    b64 = base64.urlsafe_b64encode(ticket_data.encode()).decode().rstrip("=")
    ticket = f"iroh-share://{b64}"

    active = gs.mount(uid, ticket, "team-a")

    assert active.phase == RemotePhase.PENDING
    key = f"{uid}:team-a"
    assert key in gs._actives

    # gateway.md 应该已写入，Active 已注册；
    # mount 写文件后 reconcile 补建 Handler（rule 已持久化时 desired 非空）
    assert active.handler is not None, "mount 后 reconcile 应补建 Handler"
    assert active.handler.wt_uid == uid
    active.handler.stop()
    gs.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_store_share(tmp_path):
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_share")

    ticket = gs.share(uid, "/data", "data_share")
    assert ticket.startswith("iroh-share://")

    key = f"{uid}:data_share"
    assert key in gs._actives
    active = gs._actives[key]
    assert active.phase == RemotePhase.PENDING
    assert active.handler is None
    assert active.decl.entry_id == "data_share"

    # gateway.md 应该已写入
    text = rt.store.get_text(uid, "/.mds/gateway.md") or ""
    assert "data_share" in text
    assert "shared_token" in text

    gs.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_store_unmount_unshare(tmp_path):
    gs, rt = _gs(tmp_path)
    # Share then unshare（不依赖 WT tree）
    uid = _mk_wt(gs, rt, "wt_us")
    gs.share(uid, "/local", "shr")
    key = f"{uid}:shr"
    assert key in gs._actives
    gs.unshare(uid, "/local")
    assert key not in gs._actives


@pytest.mark.asyncio
async def test_store_deactivate_wt(tmp_path):
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_da")

    gs.share(uid, "/a", "entry_a")
    gs.share(uid, "/b", "entry_b")

    assert len([k for k in gs._actives if k.startswith(f"{uid}:")]) == 2

    gs.deactivate_wt(uid)
    assert len([k for k in gs._actives if k.startswith(f"{uid}:")]) == 0


# Part 4: reconcile — 真实路径


@pytest.mark.asyncio
async def test_reconcile_creates_handler(tmp_path):
    """mount → 写文件 → reconcile → Handler 创建 → start 运行。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_rec", "")

    # 模拟 mount 的完整流程：先写 gateway.md + rule，再 reconcile
    rt.store.write_locator(uid, "/.mds/gateway.md", {"steps": []},
        "+>",
        "## rec\n- remote_addr: addr_rec\n- remote_token: tok_rec\n"
        "- local_prefix: /share/rec\n- remote_prefix: /remote\n",
        True, True, False, False)
    rt.store.rule_add_remote(uid, "/share/rec", "rec")

    gs.reconcile(uid)

    key = f"{uid}:rec"
    assert key in gs._actives
    active = gs._actives[key]
    assert active.handler is not None
    assert active.handler.wt_uid == uid
    assert active.handler.entry_id == "rec"

    # Handler 已 start，运行在后台。stop 它。
    active.handler.stop()
    gs.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_reconcile_fill_handler(tmp_path):
    """mount 先建空 Active → 写文件 → reconcile 补建 Handler。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_fill", "")

    from mdio_py.gateway.types import RemoteDecl, Active
    from mdio_py.gateway.crdt_scope import CrdtScope
    scope = CrdtScope(uid)
    gs._scopes[uid] = scope
    decl = RemoteDecl(wt_uid=uid, path="/share/fill", entry_id="fill",
                      token="tok_fill", addr="addr_fill",
                      local_prefix="/share/fill", remote_prefix="/remote")
    gs._actives[f"{uid}:fill"] = Active(key=f"{uid}:fill", decl=decl,
                                         scope=scope, handler=None)

    rt.store.write_locator(uid, "/.mds/gateway.md", {"steps": []},
        "+>",
        "## fill\n- remote_addr: addr_fill\n- remote_token: tok_fill\n"
        "- local_prefix: /share/fill\n",
        True, True, False, False)
    rt.store.rule_add_remote(uid, "/share/fill", "fill")

    gs.reconcile(uid)

    active = gs._actives[f"{uid}:fill"]
    assert active.handler is not None, "Handler should be filled by reconcile"
    active.handler.stop()
    gs.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_reconcile_cleanup_stale(tmp_path):
    """desired 中已删除 → reconcile 清理 Active。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_stale", "")

    from mdio_py.gateway.types import RemoteDecl, Active
    from mdio_py.gateway.crdt_scope import CrdtScope
    scope = CrdtScope(uid)
    gs._scopes[uid] = scope
    decl = RemoteDecl(wt_uid=uid, path="/share/stale", entry_id="stale",
                      token="tok", addr=None, local_prefix="/stale",
                      remote_prefix=None)
    gs._actives[f"{uid}:stale"] = Active(key=f"{uid}:stale", decl=decl,
                                          scope=scope, handler=None)

    assert f"{uid}:stale" in gs._actives
    gs.reconcile(uid)  # no matching rule/gateway → should clean up
    assert f"{uid}:stale" not in gs._actives


@pytest.mark.asyncio
async def test_reconcile_no_disconnected_reconnect(tmp_path):
    """MD §六：reconcile 不应在 Disconnected 时创建新 Handler。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_norec")

    rt.store.write_locator(uid, "/.mds/gateway.md", {"steps": []},
        "+>",
        "## nr\n- remote_addr: a\n- remote_token: t\n- local_prefix: /share/nr\n",
        True, True, False, False)
    rt.store.rule_add_remote(uid, "/share/nr", "nr")

    gs.reconcile(uid)

    active = gs._actives[f"{uid}:nr"]
    assert active.handler is not None
    handler_before = active.handler

    # 模拟断线
    handler_before.state = "disconnected"
    handler_before._notify_state_change("disconnected")

    gs.reconcile(uid)
    assert active.handler is handler_before, "same handler, no reconnect"

    handler_before.stop()
    gs.deactivate_wt(uid)


# Part 5: tick + 回调


@pytest.mark.asyncio
async def test_tick_projection(tmp_path):
    """tick 将 Handler 状态投影到 Active.state。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_tick")

    rt.store.write_locator(uid, "/.mds/gateway.md", {"steps": []},
        "+>",
        "## tk\n- remote_addr: a\n- remote_token: t\n- local_prefix: /share/tk\n",
        True, True, False, False)
    rt.store.rule_add_remote(uid, "/share/tk", "tk")

    gs.reconcile(uid)

    active = gs._actives[f"{uid}:tk"]
    assert active.handler is not None

    # Handler 从 reconnecting 开始，tick 看到 disconnected → Active.state = Disconnected
    gs.tick()
    # 在测试下，Handler 没有真实连接，状态可能是 disconnected 或 reconnecting
    assert active.phase in (RemotePhase.PENDING, RemotePhase.ACTIVE)

    # 手动通知 → 回调更新投影
    active.handler.state = "disconnected"
    active.handler._notify_state_change("disconnected")
    assert active.phase == RemotePhase.PENDING

    active.handler.stop()
    gs.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_tick_expired(tmp_path):
    """Failed + 7d → Expired。"""
    gs, rt = _gs(tmp_path)
    from mdio_py.gateway.types import RemoteDecl, Active
    from mdio_py.gateway.crdt_scope import CrdtScope
    from mdio_py.gateway.iroh_transport import ConnectionHandler

    scope = CrdtScope("w1")
    gs._scopes["w1"] = scope
    decl = RemoteDecl(wt_uid="w1", path="/share/x", entry_id="x",
                      token="tok", addr="addr1", local_prefix="/share/x",
                      remote_prefix=None)
    h = ConnectionHandler()
    h.configure(decl)
    h.state = "idle"
    active = Active(key="w1:x", decl=decl, scope=scope, handler=h, phase=RemotePhase.PENDING)
    active._created_at = time.time() - 8 * 86400  # 8 days
    gs._actives["w1:x"] = active

    gs.tick()
    # PENDING 超过 7 天 → reconcile 清理（当前无 gateway.md → 应被移除）
    assert "w1:x" not in gs._actives
    h.stop()


@pytest.mark.asyncio
async def test_on_handler_event(tmp_path):
    """_on_handler_event 只更新投影，不触发行为。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_evt")

    rt.store.write_locator(uid, "/.mds/gateway.md", {"steps": []},
        "+>",
        "## ev\n- remote_addr: a\n- remote_token: t\n- local_prefix: /share/ev\n",
        True, True, False, False)
    rt.store.rule_add_remote(uid, "/share/ev", "ev")
    gs.reconcile(uid)

    key = f"{uid}:ev"
    active = gs._actives[key]
    assert active.handler is not None

    # before reconciliation count
    actives_before = len(gs._actives)

    # phase 由 Handler._notify_state_change 直接更新
    active.handler._active = active
    active.handler.state = "connected"
    active.handler.doc_map = {"dummy": "map"}  # doc_map 非空 → ACTIVE
    active.handler._notify_state_change("connected")
    assert active.phase == RemotePhase.ACTIVE

    active.handler.state = "disconnected"
    active.handler._notify_state_change("disconnected")
    assert active.phase == RemotePhase.PENDING

    # unknown key → no crash (on_state_change callback is still called)
    active.handler.on_state_change("w1:unknown", "connected")

    # reconcile should NOT have been triggered by _on_handler_event
    assert len(gs._actives) == actives_before

    active.handler.stop()
    gs.deactivate_wt(uid)


# Part 6: _parse 迁移


def test_parse():
    from mdio_py.gateway.store import GatewayStore

    assert GatewayStore._parse("") == []
    assert GatewayStore._parse("# GATEWAY\n") == []

    text = "## e1\n- key1: val1\n- key2: val2\n## e2\n- flag\n- key3: val3\n"
    entries = GatewayStore._parse(text)
    assert len(entries) == 2
    assert entries[0]["_title"] == "e1"
    assert entries[0]["key1"] == "val1"
    assert entries[1]["_title"] == "e2"
    assert entries[1]["flag"] is True
    assert entries[1]["key3"] == "val3"


# Part 7: 查询


@pytest.mark.asyncio
async def test_store_queries(tmp_path):
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_q")

    from mdio_py.gateway.types import RemoteDecl, Active
    from mdio_py.gateway.crdt_scope import CrdtScope
    scope = CrdtScope(uid)
    gs._scopes[uid] = scope
    decl = RemoteDecl(wt_uid=uid, path="/share/q", entry_id="q",
                      token="qtok", addr=None, local_prefix="/q",
                      remote_prefix=None)
    gs._actives[f"{uid}:q"] = Active(key=f"{uid}:q", decl=decl, scope=scope,
                                      handler=None)

    assert gs.has_scope(uid) is True
    assert gs.has_scope("nonexistent") is False
    assert gs.get_scope(uid) is scope
    assert len(list(gs.iter_scopes())) == 1
    assert gs.get_handler(uid, "q") is None  # handler=None

    assert gs.lookup_token(b"qtok") is not None
    assert gs.is_token(b"qtok") is True
    assert gs.lookup_token(b"bad") is None
    assert gs.is_token(b"bad") is False

    gs.deactivate_wt(uid)


# Part 8: 统一注册表 + 重启恢复


@pytest.mark.asyncio
async def test_store_share_writes_rule(tmp_path):
    """share 写 rule——重启恢复的前提（reconcile 从 rule+gateway.md join 推导）。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_share_rule")

    gs.share(uid, "/data", "data_share_rule")

    remote = rt.store.rule_remote_entries(uid)
    assert ("/share/data_share_rule", "data_share_rule") in remote,         f"share 应写 rule: {remote}"

    # worktrees.md rule 字段应含 /share/ 段（_write_rule_to_file 写回）
    text = rt.store.get_text("genesis", "/.mds/worktrees.md") or ""
    assert "/share/data_share_rule" in text

    gs.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_store_share_restore_via_reconcile(tmp_path):
    """share 侧重启恢复：新 GatewayStore（模拟重启）→ reconcile → Active/scope 恢复。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_share_restore")

    gs.share(uid, "/data", "data_restore")
    assert f"{uid}:data_restore" in gs._actives

    # 模拟重启：同 store/workspace 新建 GatewayStore（内存态全丢）
    gs2 = GatewayStore(store=rt.store, workspace=rt._workspace, iroh_endpoint=None)
    gs2.reconcile(uid)

    key = f"{uid}:data_restore"
    assert key in gs2._actives, f"重启后 share Active 应恢复: {list(gs2._actives.keys())}"
    active = gs2._actives[key]
    assert active.handler is None  # share 侧：Listener，等对端
    assert active.decl.token  # token 从 gateway.md 恢复
    assert gs2.has_scope(uid)

    gs2.deactivate_wt(uid)
    gs.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_reconcile_restores_doc_map(tmp_path):
    """mount 条目带 doc_map → 重启后 reconcile 恢复 doc_map 基线 + stale 标记。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_dm_restore")

    import json as _json
    doc_map = {"/share/team-dm/hello.md": {"local": 42, "remote": 99, "hash": "h1"}}
    dm_json = _json.dumps(doc_map, ensure_ascii=False)

    rt.store.write_locator(uid, "/.mds/gateway.md", {"steps": []},
        "+>",
        ("## team-dm\n"
         "- remote_addr: addr_dm\n"
         "- remote_token: tok_dm\n"
         "- local_prefix: /share/team-dm\n"
         "- remote_prefix: /docs\n"
         f"- doc_map: {dm_json}\n"),
        True, True, False, False)
    rt.store.rule_add_remote(uid, "/share/team-dm", "team-dm")

    gs2 = GatewayStore(store=rt.store, workspace=rt._workspace, iroh_endpoint=None)
    gs2.reconcile(uid)

    key = f"{uid}:team-dm"
    active = gs2._actives[key]
    assert active.handler is not None
    assert active.handler.doc_map == doc_map,         f"doc_map 应从文件恢复: {active.handler.doc_map}"
    assert getattr(active.handler, "_doc_map_stale", False) is True,         "恢复的 doc_map 应标记 stale（重连后由 _rebuild_doc_map 校正）"

    active.handler.stop()
    gs2.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_update_mount_doc_map_helper(tmp_path):
    """_update_mount_doc_map：doc_map 入文件 + 幂等更新。"""
    import json as _json
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_dm_helper")

    rt.store.write_locator(uid, "/.mds/gateway.md", {"steps": []},
        "+>",
        ("## team-h\n"
         "- remote_addr: addr_h\n"
         "- remote_token: tok_h\n"
         "- local_prefix: /share/team-h\n"
         "- remote_prefix: /docs\n"),
        True, True, False, False)

    dm1 = {"/share/team-h/a.md": {"local": 1, "remote": 10, "hash": "x"}}
    gs._update_mount_doc_map(uid, "team-h", dm1)
    text = rt.store.get_text(uid, "/.mds/gateway.md") or ""
    assert "- doc_map:" in text
    # 经 selector 解析还原
    entries = GatewayStore._read_gateway_entries(gs, uid)
    e = entries[0]
    assert _json.loads(e["doc_map"]) == dm1

    # 幂等更新：换新映射，旧行替换不重复
    dm2 = {"/share/team-h/a.md": {"local": 2, "remote": 20, "hash": "y"}}
    gs._update_mount_doc_map(uid, "team-h", dm2)
    text2 = rt.store.get_text(uid, "/.mds/gateway.md") or ""
    assert text2.count("- doc_map:") == 1, f"doc_map 行应唯一: {text2}"
    entries2 = GatewayStore._read_gateway_entries(gs, uid)
    assert _json.loads(entries2[0]["doc_map"]) == dm2

    # 其他条目不受影响
    assert "## team-h" in text2 and "remote_addr: addr_h" in text2

    gs.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_release_scope_keeps_ws_peer(tmp_path):
    """统一注册表后 _release_scope：有 ws peer 时保留 scope（unmount 不误清本地会话）。"""
    from mdio_py.gateway.crdt_scope import CrdtScope
    from mdio_py.gateway.peer import WsPeer
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_rel_ws")

    scope = CrdtScope(uid)
    gs._scopes[uid] = scope
    ws_peer = WsPeer("ws_abc", lambda d: None)
    scope.add_peer(ws_peer)

    # 无 Active + 有 ws peer → 不删
    gs._release_scope(uid)
    assert gs.has_scope(uid), "有 ws peer 时 scope 应保留"

    # 移除 ws peer → 可回收
    scope.remove_peer("ws_abc")
    gs._release_scope(uid)
    assert not gs.has_scope(uid), "无 peer 后 scope 应回收"

    gs.deactivate_wt(uid)


# Part 9: D35 — unmount 必须按 path 解析 owner，不能回落到 active WT
#
# 旧链路：CLI unmount --wt 缺省 None → /gateway/unmount 缺省 active_worktree_uid →
# store.unmount(active, path)。mount 目标与 active 不是同一棵树时，删除打到错树：
# 目标树 gateway.md/rule 残留（重启 reconcile 从残留记录复活挂载）；错树若恰有同名
# 条目则被静默删掉。修复后：未显式给 WT 时按 path 在三档挂载点里唯一解析。


def _mount_ticket():
    ticket_data = json.dumps({"addr": "addr1", "token": "tok1", "path": "/docs"})
    b64 = base64.urlsafe_b64encode(ticket_data.encode()).decode().rstrip("=")
    return f"iroh-share://{b64}"


def _gateway_text(rt, wt_uid):
    return rt.store.get_text(wt_uid, "/.mds/gateway.md") or ""


def _stop_all(*stores, wt_uids=()):
    for store in stores:
        for active in list(store._actives.values()):
            if active.handler:
                active.handler.stop()
        for uid in wt_uids:
            store.deactivate_wt(uid)


@pytest.mark.asyncio
async def test_unmount_by_path_resolves_owner_not_active_wt(tmp_path):
    """D35 主回归：mount 到非 active 树 A，只给 path unmount ⇒ 清 A、不动 active 树 B、重启不复活。"""
    gs, rt = _gs(tmp_path)
    uid_a = _mk_wt(gs, rt, "wt_owner_a")
    uid_b = _mk_wt(gs, rt, "wt_owner_b")
    rt._workspace.active_worktree_uid = uid_b   # active ≠ mount 目标（D35 触发条件）

    gs.mount(uid_a, _mount_ticket(), "team-a")
    gs.mount(uid_b, _mount_ticket(), "keep-b")   # B 上的哨兵：不许被误改
    handler_a = gs._actives[f"{uid_a}:team-a"].handler
    assert handler_a is not None

    result = gs.unmount_by_path("/share/team-a/")   # 不给 wt_uid/wt_name；尾斜杠应归一化

    assert result["wt_uid"] == uid_a, f"应按 path 解析到 A: {result}"
    assert result["name"] == "team-a"
    assert result["resolved_by"] == "path"
    assert result["removed"] is True
    assert handler_a.state == "idle", "运行时 handler 应已停"

    # A：gateway.md / rule / Active 三处清理干净（读回校验已由 unmount 内部执行）
    assert "## team-a" not in _gateway_text(rt, uid_a)
    assert ("/share/team-a", "team-a") not in rt.store.rule_remote_entries(uid_a)
    assert f"{uid_a}:team-a" not in gs._actives

    # B：未被误改
    assert "## keep-b" in _gateway_text(rt, uid_b)
    assert ("/share/keep-b", "keep-b") in rt.store.rule_remote_entries(uid_b)
    assert f"{uid_b}:keep-b" in gs._actives
    assert rt._workspace.active_worktree_uid == uid_b, "active WT 不应被副作用改写"

    # 重启：新 GatewayStore reconcile 后 A 的 team-a 不得从残留记录复活。
    _stop_all(gs, wt_uids=(uid_a, uid_b))
    gs2 = GatewayStore(store=rt.store, workspace=rt._workspace, iroh_endpoint=None)
    gs2.reconcile_all()
    try:
        assert f"{uid_a}:team-a" not in gs2._actives, \
            f"重启后 A 的挂载不应复活: {list(gs2._actives)}"
        assert f"{uid_b}:keep-b" in gs2._actives, "B 的挂载应正常恢复"
    finally:
        _stop_all(gs2, wt_uids=(uid_a, uid_b))


@pytest.mark.asyncio
async def test_unmount_by_path_ambiguous_lists_candidates(tmp_path):
    """两棵树挂同名 path：多命中必须报 400 并列出候选，且不删任何一棵。"""
    from mdio_py.gateway.store import GatewayTargetAmbiguous

    gs, rt = _gs(tmp_path)
    uid_a = _mk_wt(gs, rt, "wt_dup_a")
    uid_b = _mk_wt(gs, rt, "wt_dup_b")
    gs.mount(uid_a, _mount_ticket(), "dup")
    gs.mount(uid_b, _mount_ticket(), "dup")

    with pytest.raises(GatewayTargetAmbiguous) as ei:
        gs.unmount_by_path("/share/dup")

    msg = str(ei.value)
    assert ei.value.status_code == 400
    assert uid_a in msg and uid_b in msg and "dup" in msg, f"候选需可执行: {msg}"
    # 未消歧 ⇒ 一棵都不动
    assert f"{uid_a}:dup" in gs._actives
    assert f"{uid_b}:dup" in gs._actives
    assert "## dup" in _gateway_text(rt, uid_a)
    assert "## dup" in _gateway_text(rt, uid_b)

    # 显式 wt_uid / wt_name 都能消歧（保持既有显式语义）
    r1 = gs.unmount_by_path("/share/dup", wt_uid=uid_a)
    assert r1["wt_uid"] == uid_a and r1["resolved_by"] == "wt_uid"
    assert f"{uid_b}:dup" in gs._actives, "A 的卸载不许碰 B"
    r2 = gs.unmount_by_path("/share/dup", wt_name="wt_dup_b")
    assert r2["wt_uid"] == uid_b and r2["resolved_by"] == "wt_name"
    assert f"{uid_a}:dup" not in gs._actives and f"{uid_b}:dup" not in gs._actives

    _stop_all(gs, wt_uids=(uid_a, uid_b))


@pytest.mark.asyncio
async def test_unmount_by_path_not_found_and_explicit_mismatch(tmp_path):
    """无命中/显式 WT 与 path 不匹配都必须明确报错，绝不能回落到 active WT。"""
    from mdio_py.gateway.store import GatewayTargetNotFound

    gs, rt = _gs(tmp_path)
    uid_a = _mk_wt(gs, rt, "wt_nf_a")
    uid_b = _mk_wt(gs, rt, "wt_nf_b")
    rt._workspace.active_worktree_uid = uid_b
    gs.mount(uid_a, _mount_ticket(), "only-a")

    # ① 无命中
    with pytest.raises(GatewayTargetNotFound) as e1:
        gs.unmount_by_path("/share/nope")
    assert e1.value.status_code == 404
    assert "没有对应挂载点" in str(e1.value)

    # ② 显式 WT 与 path 不匹配（旧实现会静默拿 active 兜底/什么都不删）
    with pytest.raises(GatewayTargetNotFound) as e2:
        gs.unmount_by_path("/share/only-a", wt_uid=uid_b)
    assert uid_b in str(e2.value)
    assert f"{uid_a}:only-a" in gs._actives, "错树请求不许删任何东西"
    assert "## only-a" in _gateway_text(rt, uid_a)

    # ③ 未知 WT 名同样报错（旧实现 resolve 失败就静默改用 active）
    with pytest.raises(GatewayTargetNotFound):
        gs.unmount_by_path("/share/only-a", wt_name="ghost-wt")

    # ④ 显式指向正确树 ⇒ 正常卸载
    r = gs.unmount_by_path("/share/only-a", wt_uid=uid_a)
    assert r["wt_uid"] == uid_a and r["removed"] is True
    assert "## only-a" not in _gateway_text(rt, uid_a)

    _stop_all(gs, wt_uids=(uid_a, uid_b))


@pytest.mark.asyncio
async def test_unmount_readback_verification_fails_loud(tmp_path):
    """gateway.md 条目没删掉就必须按失败返回（读回校验），且不动运行时。"""
    gs, rt = _gs(tmp_path)
    uid = _mk_wt(gs, rt, "wt_verify")
    gs.mount(uid, _mount_ticket(), "verify")

    # 模拟「调用过但没写掉」：删除实现被架空
    gs._rewrite_gateway_md = lambda wt_uid, entry_id: None

    with pytest.raises(RuntimeError, match="读回校验失败"):
        gs.unmount_by_path("/share/verify")

    assert "## verify" in _gateway_text(rt, uid), "演示用：文件确实没删"
    assert f"{uid}:verify" in gs._actives, "文件清理失败 ⇒ 运行时保持原样"

    _stop_all(gs, wt_uids=(uid,))


@pytest.mark.asyncio
async def test_unmount_http_endpoint_no_active_fallback(tmp_path):
    """HTTP 层：/gateway/unmount 不带 wt_uid/name 时也不得回落到 active WT（D35）。

    直接调 handler.fn（litestar 装饰器保留的原函数），绕过启动 HTTP 服务的成本。
    """
    from mdio_py.gateway.api.gateway import IrohGatewayController

    class _FakeRequest:
        def __init__(self, data):
            self._data = data

        async def json(self):
            return self._data

    gs, rt = _gs(tmp_path)
    uid_a = _mk_wt(gs, rt, "wt_http_a")
    uid_b = _mk_wt(gs, rt, "wt_http_b")
    rt._workspace.active_worktree_uid = uid_b   # active ≠ mount 目标
    previous = getattr(rt._workspace, "_gateway_store", None)
    rt._workspace._gateway_store = gs
    handler = IrohGatewayController.unmount_iroh

    def call(data):
        return handler.fn(object.__new__(IrohGatewayController), _FakeRequest(data), rt)

    def status(resp):
        # litestar Response 不显式给 status_code 时留 None（运行时按 200 序列化）
        return resp.status_code or 200

    try:
        gs.mount(uid_a, _mount_ticket(), "http-a")

        # ① 只给 path：按 owner 解析到 A（旧实现会拿 active=B）
        resp = await call({"path": "/share/http-a"})
        assert status(resp) == 200, resp.content
        assert resp.content["wt_uid"] == uid_a
        assert resp.content["resolved_by"] == "path"
        assert f"{uid_a}:http-a" not in gs._actives
        assert "## http-a" not in _gateway_text(rt, uid_a)

        # ② 无命中：404，绝不静默 200
        resp = await call({"path": "/share/ghost"})
        assert status(resp) == 404, resp.content
        assert "没有对应挂载点" in resp.content["error"]

        # ③ 多命中：400 且候选可执行
        gs.mount(uid_a, _mount_ticket(), "dup-x")
        gs.mount(uid_b, _mount_ticket(), "dup-x")
        resp = await call({"path": "/share/dup-x"})
        assert status(resp) == 400, resp.content
        assert uid_a in resp.content["error"] and uid_b in resp.content["error"]

        # ④ 显式 wt_uid / name 消歧（name 仍走 resolve_worktree_name）
        resp = await call({"path": "/share/dup-x", "wt_uid": uid_b})
        assert status(resp) == 200 and resp.content["wt_uid"] == uid_b
        resp = await call({"path": "/share/dup-x", "name": "wt_http_a"})
        assert status(resp) == 200 and resp.content["wt_uid"] == uid_a
    finally:
        _stop_all(gs, wt_uids=(uid_a, uid_b))
        rt._workspace._gateway_store = previous


def test_cli_unmount_does_not_default_to_active_wt(monkeypatch):
    """CLI 不指定 --wt 时不向服务端发送任何 WT 缺省值（否则服务端会打错树）。"""
    from click.testing import CliRunner
    import mdio_py.cli.commands as commands
    from mdio_py.cli import main

    calls = []

    def _fake_post(path, payload=None, timeout=10.0):
        calls.append((path, payload))
        return {"status": "ok", "wt_uid": "uid-A", "path": "/share/x", "removed": True}

    monkeypatch.setattr(commands, "_post_json", _fake_post)
    runner = CliRunner()

    r = runner.invoke(main, ["unmount", "--path", "/share/x"])
    assert r.exit_code == 0, r.output
    assert calls == [("/gateway/unmount", {"path": "/share/x"})], calls
    assert "unmounted /share/x from uid-A" in r.output

    # 显式 --wt 语义保持：作为 name 传给服务端（服务端再 resolve_worktree_name）
    calls.clear()
    r = runner.invoke(main, ["unmount", "-p", "/share/x", "--wt", "wt-A"])
    assert r.exit_code == 0, r.output
    assert calls == [("/gateway/unmount", {"path": "/share/x", "name": "wt-A"})], calls

    # 服务端解析失败（400 带候选详情）⇒ CLI 原样报出可执行信息
    def _fail(path, payload=None, timeout=10.0):
        raise RuntimeError(
            "server returned HTTP 400: path '/share/x' 命中多个挂载点，"
            "请用 --wt 指定 worktree：uid-A:dup(/share/dup), uid-B:dup(/share/dup)")

    monkeypatch.setattr(commands, "_post_json", _fail)
    r = runner.invoke(main, ["unmount", "-p", "/share/x"])
    assert r.exit_code == 0
    assert "- mdio_error:" in r.output
    assert "uid-A:dup" in r.output and "uid-B:dup" in r.output


def test_cli_http_error_carries_server_detail(monkeypatch):
    """_request_json 的 HTTPError 分支要把服务端 {"error": ...} 详情带进异常消息。"""
    import io
    import urllib.error
    import mdio_py.cli.commands as commands

    def _raise_http(*_args, **_kwargs):
        body = json.dumps({"error": "path '/share/x' 命中多个挂载点"}).encode()
        raise urllib.error.HTTPError(
            "http://127.0.0.1:8011/gateway/unmount", 400, "Bad Request", {}, io.BytesIO(body))

    monkeypatch.setattr(commands, "_opener_for", lambda host: _raise_http)

    with pytest.raises(RuntimeError) as ei:
        commands._request_json("POST", "/gateway/unmount", {"path": "/share/x"})

    message = str(ei.value)
    assert "HTTP 400" in message
    assert "命中多个挂载点" in message, f"服务端详情必须带出: {message}"
