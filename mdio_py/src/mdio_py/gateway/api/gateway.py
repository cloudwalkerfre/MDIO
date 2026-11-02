"""iroh Gateway API — share / mount / unmount endpoints（folder 级；wt 级见 /control/project/*）。

share:   生成 ticket，授权远端加载本 WT。持久化到 gateway.md。
mount:   解析 ticket → 连接远端 → 下载文件 → 写入已有 WT 的 VFS → 持久化 gateway.md。
unmount: 断开该远程 folder（删 rule 段 + gateway.md 条目，停 handler），文件保留为 local。
"""

from __future__ import annotations
import asyncio, base64, json, os, random, struct, string, time
from pathlib import Path

from litestar import Controller, post, Response, Request
from litestar.di import Provide
from litestar.datastructures import State
from mdio_py.gateway.state import provide_runtime
from mdio_py import MdioRuntime

HEADER_SIZE = 13
UP_TREE, DN_TREE = 0x01, 0x81
UP_OPEN, DN_OPEN = 0x02, 0x82


def _build_frame(tag: int, peer: int, doc: int, payload: bytes = b"") -> bytes:
    buf = bytearray(HEADER_SIZE + len(payload))
    buf[0] = tag
    struct.pack_into('<I', buf, 1, len(payload))
    struct.pack_into('<I', buf, 5, peer)
    struct.pack_into('<I', buf, 9, doc)
    buf[HEADER_SIZE:] = payload
    return bytes(buf)


def _make_token() -> str:
    """CSPRNG token（random.choices 的 MT19937 非加密安全）。"""
    import secrets
    return secrets.token_urlsafe(12)


# share — 生成 ticket + 持久化 token

async def iroh_share(runtime: MdioRuntime, wt_uid: str, ep,
                     path: str = "/", name: str = "") -> dict:
    """为指定 WT 生成 iroh ticket。持久化到 gateway.md。"""
    ws = runtime._workspace

    if not name:
        name = path.rstrip("/").rsplit("/", 1)[-1] or "root"

    gateway_store = getattr(ws, '_gateway_store', None)
    if gateway_store is None:
        return {"error": "gateway not enabled"}
    ticket = gateway_store.share(wt_uid, path, name)
    return {"ticket": ticket, "wt_uid": wt_uid}


# load — 拉取远程 WT + 持久化恢复信息

async def iroh_mount(runtime: MdioRuntime, ticket_str: str, name: str,
                     target_uid: str, ep) -> dict:
    """从 iroh ticket 将 remote subtree 挂载到已有 WT 下。

    完整流程：
    1. 解析 ticket → ticket_data[path] 为 remote_prefix
    2. 连接远端 → token 验证
    3. UP_TREE 获取文件树
    4. 逐文件 UP_OPEN 下载 → 路径映射写入 target_uid 的 VFS
    5. 持久化 gateway.md 到 target_uid（MD 格式）
    6. reconcile 激活 handler
    7. 创建 CrdtScope + IrohPeer
    """
    store = runtime.store

    # 1. 解析 ticket
    import iroh as _iroh
    b64_part = ticket_str.replace("iroh-share://", "").replace("iroh://", "")
    padding = 4 - len(b64_part) % 4
    if padding != 4:
        b64_part += "=" * padding
    ticket_data = json.loads(base64.urlsafe_b64decode(b64_part))
    addr_str = ticket_data["addr"]
    token = ticket_data.get("token", "").encode()
    remote_prefix = ticket_data.get("path", "/")

    # 用 iroh 原生 ticket 解析 addr（compact ticket → EndpointAddr）
    try:
        iroh_ticket = _iroh.EndpointTicket.from_string(addr_str)
        addr = iroh_ticket.endpoint_addr()
    except Exception:
        return {"error": f"Failed to parse iroh ticket: {addr_str[:60]}"}

    # 2. 连接 + token 验证
    import iroh as _iroh2
    _ep_client = await _iroh2.Endpoint.bind(_iroh2.EndpointOptions(preset=_iroh2.preset_n0()))
    await _ep_client.online()
    conn = await _ep_client.connect(addr, b"mdio-net/1")
    runtime._iroh_client_eps = getattr(runtime, '_iroh_client_eps', [])
    runtime._iroh_client_eps.append(_ep_client)
    bi = await conn.open_bi()
    await bi.send().write_all(token)
    await bi.send().finish()
    resp = bytes(await asyncio.wait_for(bi.recv().read_to_end(1024), timeout=10))
    if resp != b"accepted":
        conn.close(1, b"bad token")
        return {"error": "token rejected"}

    # 3. UP_TREE → DN_TREE
    bi2 = await conn.open_bi()
    await bi2.send().write_all(_build_frame(UP_TREE, 0, 0))
    await bi2.send().finish()
    tree_resp = bytes(await asyncio.wait_for(
        bi2.recv().read_to_end(65536), timeout=10
    ))
    if tree_resp[0] != DN_TREE:
        return {"error": f"DN_TREE expected, got tag=0x{tree_resp[0]:02x}"}
    files = json.loads(tree_resp[HEADER_SIZE:])

    # 4. 路径映射 + 下载文件 → 写入 target_uid 的 VFS
    local_prefix = f"/share/{name}"
    doc_map: dict[str, dict] = {}
    for f in files:
        remote_doc_id = f["id"]
        remote_path = f["path"]
        server_hash = f.get("hash", "")

        # 路径映射：/docs/hello.md→hello.md→/share/{name}/hello.md
        relative = remote_path.removeprefix(remote_prefix).lstrip("/")
        local_path = f"{local_prefix}/{relative}" if relative else local_prefix

        bi3 = await conn.open_bi()
        await bi3.send().write_all(_build_frame(UP_OPEN, 0, remote_doc_id))
        await bi3.send().finish()
        ckpt_resp = bytes(await asyncio.wait_for(
            bi3.recv().read_to_end(65536), timeout=10
        ))

        if ckpt_resp[0] != DN_OPEN:
            continue

        ckpt_payload = ckpt_resp[HEADER_SIZE:]
        try:
            from mdio_py import mdio_core
            text = mdio_core.ProjectStore.decode_checkpoint_text(ckpt_payload)
        except Exception:
            text = ""

        store.write_locator(
            target_uid, local_path, {"steps": []}, "+>",
            text, True, True, False, False,
        )
        local_doc_id = store.resolve_path(target_uid, local_path, False)

        try:
            store.load_checkpoint(local_doc_id, ckpt_payload)
        except Exception:
            pass

        doc_map[local_path] = {
            "local": local_doc_id,
            "remote": remote_doc_id,
            "hash": server_hash,
        }

    # 5. 发送 doc_map 到对端
    try:
        doc_map_payload = json.dumps({
            "type": "doc_map",
            "mapping": doc_map,
        }).encode()
        bi4 = await conn.open_bi()
        await bi4.send().write_all(doc_map_payload)
        await bi4.send().finish()
        await bi4.recv().read_to_end(1024)
    except Exception:
        pass

    # 6-7. GatewayStore 接管持久化 + Handler 创建 + scope 管理
    ws = runtime._workspace
    gateway_store = getattr(ws, '_gateway_store', None)

    runtime._iroh_conns = getattr(runtime, '_iroh_conns', [])
    runtime._iroh_conns.append(conn)

    if gateway_store:
        gateway_store.mount(target_uid, ticket_str, name, _ep_client=_ep_client, _conn=conn, _doc_map=doc_map)

    return {
        "wt_uid": target_uid if not gateway_store else target_uid,
        "local_prefix": local_prefix,
        "files": len(files),
        "doc_map": {k: {"local": v["local"], "remote": v["remote"]}
                     for k, v in doc_map.items()},
    }


# Controller

class IrohGatewayController(Controller):
    path = "/gateway"
    dependencies = {"runtime": Provide(provide_runtime, sync_to_thread=False)}

    @post("/test/close-peers")
    async def close_peers(self, runtime: MdioRuntime) -> Response:
        """测试用：关闭所有 iroh 连接。用于模拟网络中断。"""
        count = 0
        for c in getattr(runtime, '_iroh_conns', []):
            try: c.close(0, b"test_close")
            except: pass
            count += 1
        runtime._iroh_conns = []
        return Response({"closed": count})

    @post("/test/mk-bfork")
    async def mk_bfork(self, request: Request, runtime: MdioRuntime) -> Response:
        """测试用：创建 bfork WT（带 rule）。"""
        try:
            data = await request.json()
        except Exception:
            data = {}
        name = data.get("name", "test")
        rule_str = data.get("rule", "")
        uid = runtime.store.derive_worktree("genesis", name, rule_str)
        return Response({"uid": uid, "name": name})

    @post("/test/reset")
    async def test_reset(self, runtime: MdioRuntime) -> Response:
        """测试用：重置 GatewayStore 状态（清理所有 share/mount）。"""
        gs = getattr(runtime._workspace, '_gateway_store', None)
        if gs is not None:
            for key in list(gs._actives.keys()):
                active = gs._actives.pop(key, None)
                if active and active.handler:
                    active.handler.stop()
            for scope in gs._scopes.values():
                scope.peers.clear()
                scope.subscriptions.clear()
            gs._scopes.clear()
            # 投递索引必须一起清 —— 两张表与 wt 分域索引由同一批写口维护，
            # 只清一边 ⇒ reset 后 deliver 仍会 queued 到已 reset 的连接。
            gs._peers.clear()
            gs._doc_subs.clear()
            # 通道订阅也要退 —— 只清 _doc_subs 时，reset 之后
            # deliver 仍会 publish 进这些写任务（publish 不读 _doc_subs）。逐条退并留痕。
            for _key in list(gs._chan_ctx.keys()):
                await gs.channel_unsubscribe(_key[0], _key[1])
            print(f"[RESET] 通道退订 {len(gs._chan_ctx)} 残余 / 跳过计数 {gs._chan_skip}", flush=True)
        return Response({"status": "reset"})

    @post("/test/handler-status")
    async def handler_status(self, runtime: MdioRuntime) -> Response:
        """测试用：返回 GatewayManager._handlers 的 key 列表。"""
        gs = getattr(runtime._workspace, '_gateway_store', None)
        if gs is None:
            return Response({"handlers": []})
        return Response({"handlers": list(gs._actives.keys())})



    @post("/mount")
    async def mount_iroh(
        self,
        request: Request,
        runtime: MdioRuntime,
    ) -> Response:
        try:
            data = await request.json()
        except Exception:
            return Response({"error": "invalid JSON"}, status_code=400)

        ticket = data.get("ticket", "")
        name = data.get("name", "remote")
        target_uid = data.get("target", "genesis")
        if not ticket:
            return Response({"error": "ticket required"}, status_code=400)

        ep = getattr(request.app.state, "iroh_endpoint", None)
        if ep is None:
            return Response(
                {"error": "iroh not available. Gateway not running?"},
                status_code=503,
            )

        try:
            result = await iroh_mount(runtime, ticket, name, target_uid, ep)
            return Response(result)
        except Exception as e:
            import traceback
            detail = f"{type(e).__name__}: {e}"
            traceback.print_exc()
            return Response({"error": detail}, status_code=500)

    @post("/unmount")
    async def unmount_iroh(
        self,
        request: Request,
        runtime: MdioRuntime,
    ) -> Response:
        """folder 级：断开一个远程 folder（文件留本地）。body: {path: "/share/<name>", name?/wt_uid?}"""
        try:
            data = await request.json()
        except Exception:
            return Response({"error": "invalid JSON"}, status_code=400)
        path = data.get("path", "")
        if not path:
            return Response({"error": "path required (/share/<name>)"}, status_code=400)
        gs = getattr(runtime._workspace, "_gateway_store", None)
        if gs is None:
            return Response({"error": "gateway store not available"}, status_code=503)
        try:
            wt_uid = data.get("wt_uid") or getattr(runtime._workspace, "active_worktree_uid", "genesis")
            if data.get("name"):
                resolved = runtime._workspace.resolve_worktree_name(data["name"])
                if resolved:
                    wt_uid = resolved
            gs.unmount(wt_uid, path)
            return Response({"status": "ok", "wt_uid": wt_uid, "path": path})
        except Exception as e:
            return Response({"error": f"{type(e).__name__}: {e}"}, status_code=500)

    @post("/{wt_uid:str}/share")
    async def share_iroh(
        self,
        wt_uid: str,
        request: Request,
        runtime: MdioRuntime,
    ) -> Response:
        ep = getattr(request.app.state, "iroh_endpoint", None)
        if ep is None:
            return Response(
                {"error": "iroh not available. Start Gateway first."},
                status_code=503,
            )

        # 读取共享路径和命名
        try:
            body = await request.body()
            data = json.loads(body) if body else {}
        except Exception:
            data = {}
        path = data.get("path", "/")
        name = data.get("name", "")

        try:
            result = await iroh_share(runtime, wt_uid, ep, path, name)
            return Response(result)
        except Exception as e:
            return Response({"error": str(e)}, status_code=500)
