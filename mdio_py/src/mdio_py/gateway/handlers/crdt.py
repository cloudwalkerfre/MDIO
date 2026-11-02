"""CRDT 双向同步处理器 — CrdtScope 统一 WS + iroh 广播。"""
import os
import time
import json
import asyncio
import struct
from litestar import WebSocket
from litestar.channels import ChannelsPlugin
from mdio_py import MdioRuntime
from mdio_py.gateway.peer import WsPeer

HEADER_SIZE = 13
DN_EXEC = 0x85
_AST_ENABLED_CACHE: dict[str, bool] = {}


def _schedule_ast_send(loop, coro_factory):
    """FJ 线程回调 → 事件循环投递（线程安全——FJ 异步化）。"""
    loop.call_soon_threadsafe(lambda: asyncio.ensure_future(coro_factory()))


def submit_ast_packet(store, doc_id, loop, send_coro_factory):
    """FULL JSON 异步投递（FJ 线程渲染——不阻塞调用线程）。
    回调（FJ 线程内）: frame 非 None → 投递到事件循环执行发送（调用方语义保持——
    拿帧自己发——只是渲染移线程）; None = 快照已过期（渲染期间编辑——丢弃——
    前端已有 P 增量流——如需重投调用方自行决定）。
    """
    def _on_frame(frame):
        if frame:
            _schedule_ast_send(loop, lambda: send_coro_factory(bytes(frame)))
    store.get_ast_packet_async(doc_id, _on_frame)


def _build_frame(tag: int, peer: int, doc: int, payload: bytes = b"") -> bytes:
    buf = bytearray(HEADER_SIZE + len(payload))
    buf[0] = tag
    struct.pack_into('<I', buf, 1, len(payload))
    struct.pack_into('<I', buf, 5, peer)
    struct.pack_into('<I', buf, 9, doc)
    if payload:
        buf[HEADER_SIZE:] = payload
    return bytes(buf)


def extract_doc_id(frame: bytes) -> int:
    return struct.unpack_from('<I', frame, 9)[0]


def _err_text(e: BaseException) -> str:
    """把异常变成给客户端的错误文本；Rust panic 单独标注（它是跨语言边界来的）。"""
    if type(e).__name__ == "PanicException":
        return f"- mdio_error: rust panic: {e}"   # 前缀对齐全仓协议
    return f"- mdio_error: {e}"


def _ast_enabled(store, wt_uid: str) -> bool:
    return True  # MVP: always enabled for testing


def _tag_bucket(tag: int) -> str:
    """Map wire tag to telemetry bucket, matching Rust SampleBucket::from_tag."""
    if tag in (0x30, 0xB0):
        return "probe"
    if tag in (0x01, 0x02, 0x03, 0x81, 0x82, 0xFF):
        return "ctrl"
    return "edit"


def _reachable_doc_ids(runtime: MdioRuntime, wt_uid: str) -> set[int] | None:
    """连接的 wt 下**可达**的 doc 集合（步骤 3 的准入判据）。

    用 get_vfs_tree(wt, True)：它本来就带 wt（mdio_core/src/lib.rs:833）、**只读**、
    不碰 resolve_path（那个未命中会 get_or_alloc 分配 arena 槽，不能当校验用）。
    返回 None = 判不了（调用方按「接受 + 空订阅」的安全侧退化）。
    """
    try:
        _st = getattr(runtime, "store", runtime)   # 兼容「传 runtime」与「传裸 store」（iroh 侧）
        tree = json.loads(_st.get_vfs_tree(wt_uid, True).decode())
        return {int(d["id"]) for d in tree if d.get("id") is not None}
    except Exception as e:
        print(f"[ADMIT] 树取不到 wt={wt_uid}: {e!r} ⇒ 退化为「接受 + 空订阅」", flush=True)
        return None


async def _exec_pipeline(runtime: MdioRuntime, payload: dict,
                         conn_wt: str | None = None) -> str:
    """两条 exec 路径（0x05 主线程直连 / 0x0E WASM 通路）共享的执行核心。

    返回 raw 文本（mdio_success: / mdio_error: 前缀）——tag 层负责帧头与响应包装。
    __reset__ 为测试专用：强制关 session 重建（SOP Step 1 依赖）。
    """
    code = payload.get("code", "")
    # **显式 wt_uid 优先**（业主拍板：ws 是正路，应用跨树 exec——chat 的工地树、宿主
    # readDoc 等——必须经载荷显式指定；「连接上下文优先」会吞掉显式 wt_uid，把写落到连接树）。
    # 不带载荷时退回连接上下文；两者皆无才 genesis。0x05 / 0x0E 两条 exec 通路共用。
    wt_uid = payload.get("wt_uid") or conn_wt or "genesis"

    # 测试用: __reset__ <path>\n<content> → 强制关 session 重建
    if code.startswith("__reset__"):
        first_nl = code.find("\n")
        vpath = code[9:first_nl].strip() if first_nl > 9 else code[9:].strip()
        content = code[first_nl + 1:] if first_nl > 9 else "# reset\n"
        if not vpath.startswith("/") or vpath.startswith("/.mds"):
            return f"- mdio_error: invalid reset path: {vpath}"
        try:
            doc_id = runtime.store.reset_doc(wt_uid, vpath, content)
        except Exception:
            from mdio_py.core.selector_builder import SelectorBuilder
            ir = SelectorBuilder().build()
            runtime.store.write_locator(wt_uid, vpath, ir, "+>", content,
                auto_scaffold=True, god=True, auto_level=False, suppress_hooks=True, proj=None)
            try:
                doc_id = runtime.store.reset_doc(wt_uid, vpath, content)
            except Exception as e2:
                return f"- mdio_error: reset retry failed: {e2}"
        # reset_doc = sessions.remove + open_session ⇒ **血统已换**（新的 base run 身份）。
        # 订阅者手上那份副本从此是"另一条血统"，它们的下一个 op 会永久静默——所以
        # 在这里主动推一帧 DN_OPEN（新 checkpoint），让它们立刻整篇重取。
        try:
            from mdio_py.gateway.crdt_scope import push_relineage
            gws = getattr(getattr(runtime, "_workspace", None), "_gateway_store", None)
            scope = gws.get_scope(wt_uid) if gws is not None else None
            if scope is not None:
                # 步骤 2：lookup 优先用 GatewayStore（走投递新索引），没有才退回 scope
                _gws = getattr(getattr(runtime, "_workspace", None), "_gateway_store", None)
                await push_relineage(runtime.store, _gws or scope, [doc_id], reason="__reset__")
        except Exception as e3:
            print(f"⚠️ [__reset__] 换代推送失败: {e3!r}", flush=True)
        return f"mdio_success: reset {vpath} doc_id={doc_id}"

    result = await runtime.exec(
        code,
        active_doc=payload.get("active_doc", ""),
        wt_uid=wt_uid,
    )
    return str(result)


def _ws_peer_id(socket) -> str:
    """WS 连接的 scope peer_id：注册与清理必须用同一个值（唯一来源）。"""
    return f"ws_{id(socket):x}"[:16]


async def handle_crdt_sync(
    socket: WebSocket,
    channels: ChannelsPlugin,
    runtime: MdioRuntime,
    wt_runtimes: dict,
    conn_wt: str | None = None,
):
    """双向 CRDT 同步入口：保证连接生命周期结束时清理 scope peer。

    清理放 finally —— TaskGroup 被取消（客户端断开 ⇒ handler 取消）时，
    reader_task 尾部的清理不会执行，死 peer 会永远留在各 scope 的订阅集里。
    见 BUGS 复盘 A4。
    """
    # E1：连接 id 在 accept 处**铸一次**，下行全部用它（注册/清理同一来源，杜绝二次派生分叉）。
    peer_id = _ws_peer_id(socket)
    try:
        await _handle_crdt_sync_impl(socket, channels, runtime, wt_runtimes, conn_wt, peer_id)
    finally:
        gws = getattr(getattr(runtime, "_workspace", None), "_gateway_store", None)
        if gws is not None:
            await gws.channel_unsubscribe_all(peer_id)   # R6：通道订阅随连接退干净（停写任务）
            gws.detach_peer(peer_id)          # 投递表 + 全部文档订阅 + 各 wt 分域一次摘干净
        else:
            for scope in wt_runtimes.values():
                scope.remove_peer(peer_id)


async def _handle_crdt_sync_impl(
    socket: WebSocket,
    channels: ChannelsPlugin,
    runtime: MdioRuntime,
    wt_runtimes: dict,
    conn_wt: str | None = None,
    peer_id: str = "",
):
    """双向 CRDT 同步——CrdtScope 广播，不区分 WS/iroh。peer_id 由 handle_crdt_sync 铸一次传入。"""
    store = runtime.store
    if not peer_id:
        peer_id = _ws_peer_id(socket)   # 兼容直连调用面
    if os.environ.get("MDIO_WS_TRACE") == "1":
        print(f"[WS] 新连接 raw_id={id(socket):x} → peer_id={peer_id}（截断到 16 字符）", flush=True)

    async def ws_send(data: bytes) -> None:
        await socket.send_bytes(data)

    # 【实验计时 T2】连接级账本（env 门控 MDIO_FRAME_TIMING=1，默认关）。
    # 三点：rx=收到该帧 / eng=交给引擎 / tx=引擎结果写回 socket；id 把一帧的三行串起来。
    # 与既有 _ft 同一门控；只跳过 UpTree(0x01) 与两条 exec 分支（0x05/0x0E，它们不经过引擎）。
    # （持有器用可变容器：嵌套函数里对它做 += 会被当成 reader_task 的局部名 ⇒ UnboundLocalError）
    _ft_st = {"on": os.environ.get("MDIO_FRAME_TIMING") == "1", "seq": 0, "rx": 0.0, "doc": 0}

    # tag 速查：0x81 DnTree 0x82 DnOpen 0x83 DnSync 0x84 DnAst 0x85 DnExec 0xFF Err

    async with asyncio.TaskGroup() as tg:
        async def reader_task():
            while True:
                try:
                    data = await socket.receive_bytes()
                    tag = data[0] if data else 0

                    # 【实验计时 T2-a】收到该帧的时刻（receive_bytes 返回的那一瞬）
                    _ft_id = None
                    if _ft_st["on"] and data and data[0] not in (0x01, 0x05, 0x0E):
                        _ft_st["seq"] += 1
                        _ft_id = _ft_st["seq"]
                        _ft_st["rx"] = time.perf_counter()
                        _ft_st["doc"] = struct.unpack_from('<I', data, 9)[0] if len(data) >= HEADER_SIZE else 0
                        print(f"[FT] rx id={_ft_id} tag=0x{data[0]:02x} doc={_ft_st['doc']} t={time.time():.6f}", flush=True)

                    # ── UP_EXEC (0x05): 主线程直连 exec 通道（SOP/测试用）──
                    if len(data) > 0 and data[0] == 0x05:
                        try:
                            payload = json.loads(data[HEADER_SIZE:].decode('utf-8'))
                            result = await _exec_pipeline(runtime, payload, conn_wt)
                            ack = _build_frame(DN_EXEC, 0, 0, result.encode('utf-8'))
                            await socket.send_bytes(ack)
                        except BaseException as e:
                            # Rust panic 变 PanicException（BaseException 子类）——**就地**拦：
                            # 这样能把它变成这一条请求的错误结果，而不是让整个连接失能。
                            ack = _build_frame(DN_EXEC, 0, 0, _err_text(e).encode('utf-8'))
                            await socket.send_bytes(ack)
                        continue

                    # ── UP_EXEC (0x0E): WASM 通路 exec（前端统一走这条）──
                    # 响应回填请求帧的 peer/doc（WASM 引擎按此路由回主线程），
                    # payload 带 exec_id 关联并发请求——与 0x05 纯文本响应分道。
                    if len(data) > 0 and data[0] == 0x0E:
                        try:
                            peer = struct.unpack_from('<I', data, 5)[0]
                            doc = struct.unpack_from('<I', data, 9)[0]
                            payload = json.loads(data[HEADER_SIZE:].decode('utf-8'))
                            exec_id = payload.get("exec_id", 0)
                            result = await _exec_pipeline(runtime, payload, conn_wt)
                            resp = json.dumps({"exec_id": exec_id, "raw": result}, ensure_ascii=False)
                            ack = _build_frame(DN_EXEC, peer, doc, resp.encode('utf-8'))
                            # 注意：**exec 分支到此 continue**，不经过 store.handle_packet
                            await socket.send_bytes(ack)
                        except BaseException as e:
                            # 就地拦 PanicException：**exec_id 在这里还活着**，能回一个带
                            # exec_id 的错误结果 —— 客户端拿到错误文本，而不是永远 pending
                            # （外层兜底拦得住连接，但那时 exec_id 已随栈解开丢失）。
                            resp = json.dumps({"exec_id": exec_id, "raw": _err_text(e)}, ensure_ascii=False)
                            ack = _build_frame(DN_EXEC, peer, doc, resp.encode('utf-8'))
                            await socket.send_bytes(ack)
                        continue

                    # 准入已搬进 **Rust 协议层**（R2 删除旧 Python 门）：store.rs:1596-1613 handle_packet(…, conn_wt)
                    # + store.rs:1575-1594 doc_reachable_from（归属 ∨ 挂载可达）；开关 MDIO_ADMIT_GATE（默认 "1"）。
                    # Python 侧不再判门，也不再打 [ADMIT] / [FT] admit —— 门在 handle_packet 里，零额外 FFI。

                    t1 = time.perf_counter()
                    # 【实验计时 T2-b】交给引擎的时刻
                    if _ft_id:
                        print(f"[FT] eng id={_ft_id} rx_to_engine={(t1 - _ft_st['rx']) * 1000:.2f}ms", flush=True)
                    # 【实验计时】帧到达网关的时刻（供 store.deliver 算「收到帧 → PJ 转发出去」）。
                    # 注意：deliver 在 **gateway store**（gws）上，与 runtime.store 不是同一个对象 ⇒
                    # 锚点必须记在 gws 上，否则 deliver 读不到。
                    try:
                        _gws_ft = getattr(getattr(runtime, "_workspace", None), "_gateway_store", None)
                        if _gws_ft is not None:
                            _gws_ft._ft_in_at = t1
                            _gws_ft._ft_id = _ft_id
                            _gws_ft._ft_rx = _ft_st["rx"]
                    except Exception:
                        pass
                    action = store.handle_packet(data, conn_wt)
                    # 步骤 3（树通道，零 Rust）：Rust 的 0x01 分支回的是 refresh_tree_json("genesis")
                    # （store.rs:1584 写死），与连接的 wt 无关 ⇒ 这里补发一份**本连接 wt** 的树，
                    # 覆盖客户端刚收到的那份。放在 handle_packet **之后**（Rust 的树已同步发出）
                    # 且不 return ⇒ 订阅逻辑照常执行，不丢订阅。
                    if conn_wt and len(data) > 0 and data[0] == 0x01:
                        try:
                            _tj = store.get_vfs_tree(conn_wt, True)
                            _tb = bytes(_tj) if isinstance(_tj, (bytes, bytearray)) else str(_tj).encode()
                            await socket.send_bytes(struct.pack("<BIII", 0x81, len(_tb), 0, 0) + _tb)
                        except Exception as _e:
                            print(f"[TREE] wt={conn_wt} 取树失败：{_e!r}", flush=True)
                    t2 = time.perf_counter()
                    doc_id = extract_doc_id(data)
                    # 【实验计时】帧进 core → core 处理完（env 门控 MDIO_FRAME_TIMING=1，默认关；
                    # 只跳过 UpTree(0x01) 的刷屏 ⇒ 其余帧每帧一行。实验完可整段删。）
                    _ft = (os.environ.get("MDIO_FRAME_TIMING") == "1") and bool(data) and data[0] != 0x01
                    if _ft:
                        print(f"[FT] in id={_ft_id} tag=0x{data[0]:02x} doc={doc_id} core={(t2 - t1) * 1000:.2f}ms t={time.time():.6f}", flush=True)
                    payload = action.data
                    # 热路径 print 已注释：每帧同步 I/O 阻塞事件循环（帧延迟来源）
                    # if (t2 - t1) * 1000 > 1:
                    #     print(f" [CRDT] handle_packet doc={doc_id} took {((t2-t1)*1000):.1f}ms")

                    # 帧分类观测已摘除（补丁投递排错改看 [BCAST-MISS] / [ADMIT]）

                    # Direct — 回复当前 peer
                    if action.is_direct and payload:
                        _ft_s0 = time.perf_counter()
                        await socket.send_bytes(bytes(payload))
                        if _ft:
                            _ft_s1 = time.perf_counter()
                            # 【实验计时 T2-c】引擎结果写回 socket 的时刻：send=await 本身的耗时（granian/ASGI 的写/flush）
                            print(f"[FT] tx id={_ft_id} via=direct doc={doc_id} send={(_ft_s1 - _ft_s0) * 1000:.2f}ms"
                                  f" engine_to_tx={(_ft_s1 - t1) * 1000:.2f}ms rx_to_tx={(_ft_s1 - _ft_st['rx']) * 1000:.2f}ms"
                                  f" t={time.time():.6f}", flush=True)

                    # Subscribe (DirectAndSub)
                    _admit = True   # 准入结论：未走到校验（无 wt / 非订阅）时视为放行
                    if action.is_subscribe:
                        if payload:
                            _ft_s0 = time.perf_counter()
                            await socket.send_bytes(bytes(payload))
                            if _ft:
                                _ft_s1 = time.perf_counter()
                                print(f"[FT] tx id={_ft_id} via=sub doc={doc_id} send={(_ft_s1 - _ft_s0) * 1000:.2f}ms"
                                      f" engine_to_tx={(_ft_s1 - t1) * 1000:.2f}ms rx_to_tx={(_ft_s1 - _ft_st['rx']) * 1000:.2f}ms"
                                      f" t={time.time():.6f}", flush=True)
                        wt_uid = store.doc_wt_uid(doc_id)
                        if wt_uid:
                            # wt_runtimes 是**每连接一份的 state 副本**（Litestar 语义），
                            # 只写副本的 scope 永远进不了 AstBridge 的广播路径——后者按
                            # gateway_store._scopes（权威表）找 scope，找不到就静默丢补丁。
                            # 实测：项目 wt 的第一个连接自建 scope 只落在副本里 ⇒
                            # [AST-P] uid=c5BJBF3A scope=N ⇒ 项目文档只有 F 帧、没有 P 帧。
                            # 故 scope 一律以权威表为准，副本指向同一对象。
                            from mdio_py.gateway.crdt_scope import CrdtScope
                            gws = getattr(getattr(runtime, "_workspace", None), "_gateway_store", None)
                            if gws is not None and hasattr(gws, "_get_or_create_scope"):
                                scope = gws._get_or_create_scope(wt_uid)
                            else:
                                scope = wt_runtimes.get(wt_uid) or CrdtScope(wt_uid)
                            wt_runtimes[wt_uid] = scope
                            ws_peer = WsPeer(peer_id, ws_send)
                            # 步骤 3：**准入校验** —— 该 doc 必须从连接的 wt 可达。
                            # 只在连接**带 wt**（/ws/gateway/{wt}）时校验；无 wt 的老端点不校验（弃用中）。
                            # 注意：这里必须用**连接的 wt**（参数 conn_wt），
                            # 不能用局部 wt_uid —— 上面那行已经把它覆盖成「文档的归属 wt」了。
                            _admit, _why = True, ""
                            if conn_wt:
                                _ids = _reachable_doc_ids(runtime, conn_wt)
                                if _ids is None:
                                    _admit, _why = False, "可达性判不了（取树失败）⇒ 放行打开，但**不登记订阅**"
                                elif int(doc_id) not in _ids:
                                    _admit, _why = False, f"doc={doc_id} 不在该连接 wt 的可达集"
                            if not _admit:
                                print(f"[ADMIT] **不登记订阅** peer={peer_id} doc={doc_id} conn_wt={conn_wt} ⇒ {_why}", flush=True)
                            # 步骤 2：走 4 个写口（投递索引 + wt 分域双写）
                            elif gws is not None:
                                gws.attach_peer(ws_peer, scope)
                                gws.subscribe(peer_id, doc_id, scope)
                                # R6 步骤 1：接进文档通道（框架订阅者 + 本连接写任务）；
                                # 在 AST F 帧之前 await 完成，避免"先发后订"窗口。
                                await gws.channel_subscribe(peer_id, doc_id)
                            else:
                                if peer_id not in scope.peers:
                                    scope.add_peer(ws_peer)
                                scope.subscribe(peer_id, doc_id)
                        # DN_AST 通道：UP_OPEN 后独立发送 AST 全量 (crdt_ast_patch_plan §2.4)
                        # FJ 异步化——投递 FJ 线程渲染——不阻塞本线程——
                        # 完成回调经事件循环发送（语义保持：发给新订阅者）
                        # 步骤 3 补（复核 F1）：AST 全帧原先在准入门**之外**（与 if wt_uid: 同级）⇒ 一并纳入
                        if wt_uid and _admit and _ast_enabled(store, wt_uid):
                            loop = asyncio.get_running_loop()
                            submit_ast_packet(
                                store, doc_id, loop,
                                lambda pkg: socket.send_bytes(pkg))

                    # Broadcast — scope 广播给所有 peer (WS + iroh)
                    if action.is_broadcast and payload:
                        wt_uid = store.doc_wt_uid(doc_id)
                        # 唯一出口（kind=TextOp，origin=发起端 peer ⇒ 跳过它，避免双应用）
                        from mdio_py.gateway.crdt_scope import deliver, KIND_TEXT
                        gws = getattr(getattr(runtime, "_workspace", None), "_gateway_store", None)
                        await deliver(gws if gws is not None else (lambda u: wt_runtimes.get(u)),
                                      doc_id, bytes(payload), KIND_TEXT,
                                      origin=peer_id, store=store)
                        # DN_AST: 增量 P-patch 由 Path Worker → AstBridge 单独广播。
                        # 此处不发全量 F-snapshot，避免与 P-patch 并发导致 MST dead-node。
                        # [Perf] Sync 每帧 print 已注释：热路径同步 I/O

                    # Unsubscribe
                    if action.is_unsubscribe:
                        wt_uid = store.doc_wt_uid(doc_id)
                        if os.environ.get("MDIO_WS_TRACE") == "1":
                            print(f"[UNSUB] peer={peer_id} doc={doc_id} uid={wt_uid}", flush=True)
                        if wt_uid and wt_uid in wt_runtimes:
                            _gws = getattr(getattr(runtime, "_workspace", None), "_gateway_store", None)
                            if _gws is not None:
                                _gws.unsubscribe(peer_id, doc_id, wt_runtimes[wt_uid])
                                await _gws.channel_unsubscribe(peer_id, doc_id)
                            else:
                                wt_runtimes[wt_uid].unsubscribe(peer_id, doc_id)

                except BaseException as e:   # 必须覆盖 BaseException —— 见下
                    # Rust panic 跨 pyo3 会变成 PanicException，而它**派生自 BaseException**
                    # （pyo3 的设计：像 SystemExit 一样穿透整栈）。这里不拦，它就会掀掉整条
                    # 连接的 TaskGroup。实测（注入一次 panic）：write 超时、pending 1→2、
                    # isReady 仍 true、HTTP 仍 200、日志出现
                    #   BaseExceptionGroup('unhandled errors in a TaskGroup', [PanicException])
                    # —— 客户端看到的是「连接还在、请求永远不回」。
                    if type(e).__name__ == "PanicException":
                        print(f"⚠️ [WS] Rust panic 已拦下（连接保持）: {e}", flush=True)
                        try:
                            await socket.send_bytes(_build_frame(
                                DN_EXEC, 0, 0,
                                ("mdio_error: rust panic: " + str(e)).encode("utf-8")))
                        except Exception:
                            pass
                        continue
                    # 其余异常：静默 break 会让连接「看着还在、实际已死」，所以必须打出来。
                    import traceback
                    print(f"⚠️ [WS] reader 异常退出 peer={peer_id}: {e!r}", flush=True)
                    traceback.print_exc()
                    break

        tg.create_task(reader_task())
