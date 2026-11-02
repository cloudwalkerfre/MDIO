use futures::channel::mpsc::{channel, Receiver, Sender};
use futures::{select, SinkExt};
use futures::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use bytes::{BytesMut, Buf};
use ws_stream_wasm::WsMeta;
use futures::StreamExt;
use crate::net_protocol::{NTag, NET_HEADER_SIZE};


// 宏定义: Debug toggle
#[cfg(feature = "debug")]
macro_rules! debug_log { ($($arg:tt)*) => { crate::drill_log!(&format!($($arg)*).into()) }; }
#[cfg(not(feature = "debug"))]
macro_rules! debug_log { ($($arg:tt)*) => { () }; }

// ============================================================================

// Cola 作者推荐的 Bincode 配置: Little Endian + VarInt
fn bincode_config() -> impl bincode::config::Config {
    bincode::config::standard()
        .with_little_endian()
        .with_variable_int_encoding()
}

#[derive(Clone, Copy)]
pub enum Priority {
    High,
    Low,
}

struct FrameReader {
    buffer: BytesMut,
}

impl FrameReader {
    fn new() -> Self {
        Self { buffer: BytesMut::with_capacity(8192) }
    }

    /// 返回: (SenderPeerID, DocID, Tag, Payload)
    /// Echo filtering moved up to PeerSession level.
    async fn next_frame<R: futures::io::AsyncRead + Unpin>(&mut self, reader: &mut R) -> anyhow::Result<(u32, u32, u8, Vec<u8>)> {
        loop {
            if self.buffer.len() >= NET_HEADER_SIZE {
                let chunk = &self.buffer[..NET_HEADER_SIZE];

                let tag = chunk[0];
                let payload_len = u32::from_le_bytes([chunk[1],chunk[2],chunk[3],chunk[4]]) as usize;
                let total_len = NET_HEADER_SIZE + payload_len;

                if self.buffer.len() >= total_len {
                    let sender_id = u32::from_le_bytes([chunk[5],chunk[6],chunk[7],chunk[8]]);
                    let doc_id = u32::from_le_bytes([chunk[9],chunk[10],chunk[11],chunk[12]]);

                    debug_log!("RX Frame: Sender={} Doc={} Tag={} Len={}", sender_id, doc_id, tag, payload_len);

                    self.buffer.advance(NET_HEADER_SIZE);
                    let payload = self.buffer.split_to(payload_len).to_vec();
                    return Ok((sender_id, doc_id, tag, payload));
                }
            }

            let mut temp = [0u8; 4096];
            let n = reader.read(&mut temp).await?;
            if n == 0 { return Err(anyhow::anyhow!("EOF")); }
            self.buffer.extend_from_slice(&temp[..n]);
        }
    }
}

/// Channel type: (SenderPeerID, DocID, Tag, Payload)
pub type NetFrame = (u32, u32, u8, Vec<u8>);

#[derive(Clone)]
pub struct Transport {
    inbound_rx: Arc<futures::lock::Mutex<Receiver<NetFrame>>>,
    high_tx: Sender<Vec<u8>>,
    low_tx: Sender<Vec<u8>>,
    connected: Arc<AtomicBool>,
}

impl Transport {
    pub fn new() -> Self {
        let (tx, _) = channel(1);
        let (_, rx) = channel(1);
        Self {
            inbound_rx: Arc::new(futures::lock::Mutex::new(rx)),
            high_tx: tx.clone(),
            low_tx: tx,
            connected: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(not(feature = "nodejs"))]
    pub async fn connect(&mut self, url: &str) -> Result<(), JsValue> {
        let (_meta, stream) = WsMeta::connect(url, None).await
            .map_err(|e| JsValue::from_str(&format!("WS Err: {:?}", e)))?;
        let io = stream.into_io();
        let (mut raw_reader, mut raw_writer) = io.split();

        let (high_tx, mut high_rx) = channel::<Vec<u8>>(1024);
        let (low_tx, mut low_rx) = channel::<Vec<u8>>(128);
        let (mut in_tx, in_rx) = channel::<NetFrame>(1024);

        self.connected.store(true, Ordering::SeqCst);
        self.inbound_rx = Arc::new(futures::lock::Mutex::new(in_rx));
        self.high_tx = high_tx;
        self.low_tx = low_tx;

        // ── 活性标记：写/读循环一旦退出（EOF、写失败、channel 关闭）就必须落下 ──
        //
        // run_loop 靠 is_connection_valid() 决定要不要走 Disconnected → 退避重连；
        // step_select 也靠它决定要不要继续 await 网络分支。**不落标记的后果**：
        // 服务端没了，客户端还"以为自己连着"——不重连、不拉新 DnTree、exec 永远 pending，
        // 除了刷新页面没有任何恢复路径（2026-09-21 实测：杀掉网关 12s 后客户端零日志、
        // 端口已释放而状态仍是 Connected）。Node.js 分支早就用 onclose 做了这件事
        // （见下方 connect 的 nodejs 版），浏览器分支漏了。
        let connected_w = self.connected.clone();
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                let packet = select! {
                    msg = high_rx.next() => msg,
                    msg = low_rx.next() => msg,
                    complete => break,
                };
                if let Some(data) = packet {
                    if raw_writer.write_all(&data).await.is_err() {
                        // 写失败 = 连接真死 ⇒ 才落 false。
                        connected_w.store(false, Ordering::SeqCst);
                        break;
                    }
                } else { break; }   // 通道关闭 = 被新 connect 替换（正常退出）⇒ **不落 false**
            }
            debug_log!("🔌 Writer loop ended");
        });

        let connected_r = self.connected.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut framer = FrameReader::new();
            loop {
                let _t_recv = crate::get_performance().now();
                match framer.next_frame(&mut raw_reader).await {
                    Ok(item) => {
                        // 接收耗时打点（WS 帧到达 → FrameReader 解析完）
                        let dur_ns = ((crate::get_performance().now() - _t_recv) * 1_000_000.0) as u64;
                        crate::WASM_TELEMETRY.with(|t| t.borrow_mut().push(crate::WasmTelemetrySample {
                            layer: "WASM_recv_proc".into(), dur_ns,
                            ts_ms: _t_recv,
                            tag: item.2, doc_id: item.1, terminated: false,
                        }));
                        if in_tx.send(item).await.is_err() { break; }
                    },
                    Err(e) => {
                        debug_log!("🛑 Transport Reader Error: {:?}", e);
                        break;
                    },
                }
            }
            connected_r.store(false, Ordering::SeqCst);
            debug_log!("🔌 Reader loop ended → connected=false");
        });

        Ok(())
    }

    // Node.js: raw globalThis.WebSocket, no ws_stream_wasm needed.
    // Each onmessage = one complete Gateway binary packet.
    // Echo filtering moved up to PeerSession level.
    #[cfg(feature = "nodejs")]
    pub async fn connect(&mut self, url: &str) -> Result<(), JsValue> {
        use wasm_bindgen::prelude::*;
        use js_sys::{ArrayBuffer, Uint8Array};
        use std::cell::RefCell;
        use std::rc::Rc;

        let (high_tx, mut high_rx) = channel::<Vec<u8>>(1024);
        let (low_tx, mut low_rx) = channel::<Vec<u8>>(128);
        let (in_tx, in_rx) = channel::<NetFrame>(1024);

        // Rc<RefCell<Option<Sender>>> — onclose 时置 None → Sender drop → channel EOF
        let in_tx = Rc::new(RefCell::new(Some(in_tx)));

        let global = js_sys::global();
        let ws_ctor = js_sys::Reflect::get(&global, &"WebSocket".into())?;
        let args = js_sys::Array::new();
        args.push(&JsValue::from_str(url));
        let ws = js_sys::Reflect::construct(
            &wasm_bindgen::JsCast::unchecked_ref::<js_sys::Function>(&ws_ctor),
            &args,
        )?;
        let _ = js_sys::Reflect::set(&ws, &"binaryType".into(), &"arraybuffer".into());

        let connected = self.connected.clone();

        // Writer: channel → ws.send
        let ws_send = ws.clone();
        wasm_bindgen_futures::spawn_local(async move {
            loop {
                let packet = select! {
                    msg = high_rx.next() => msg,
                    msg = low_rx.next() => msg,
                    complete => break,
                };
                if let Some(data) = packet {
                    let arr = Uint8Array::from(data.as_slice());
                    let send_fn = js_sys::Reflect::get(
                        &ws_send, &"send".into(),
                    ).unwrap();
                    let send_fn: &js_sys::Function = JsCast::unchecked_ref(&send_fn);
                    let send_args = js_sys::Array::new();
                    send_args.push(&arr);
                    let _ = js_sys::Reflect::apply(
                        send_fn, &ws_send, &send_args,
                    );
                } else { break; }
            }
        });

        // Reader: ws.onmessage → channel
        let ws_recv = ws.clone();
        let in_tx_msg = in_tx.clone();
        let on_msg = Closure::wrap(Box::new(move |event: JsValue| {
            // 帧到达 worker 打点（performance.now 与主线程同时间轴,可对比 exec_done → frame_arrive）
            let _t_ws = crate::get_performance().now();
            let data = js_sys::Reflect::get(&event, &"data".into()).unwrap();
            let bytes = Uint8Array::new(&JsCast::unchecked_ref::<ArrayBuffer>(&data)).to_vec();
            if bytes.len() < NET_HEADER_SIZE { return; }
            let sender = u32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]);
            let doc_id = u32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]);
            let payload = bytes[NET_HEADER_SIZE..].to_vec();
            let frame = (sender, doc_id, bytes[0], payload);
            crate::WASM_TELEMETRY.with(|t| t.borrow_mut().push(crate::WasmTelemetrySample {
                layer: "WASM_recv_proc".into(),
                dur_ns: (((crate::get_performance().now() - _t_ws) * 1_000_000.0) as u64),
                ts_ms: _t_ws,
                tag: bytes[0], doc_id, terminated: false,
            }));
            if let Some(tx) = in_tx_msg.borrow_mut().as_mut() {
                if let Err(e) = tx.try_send(frame) {
                    if e.is_full() {
                        #[cfg(feature = "debug")]
                        web_sys::console::warn_1(&"[Transport] inbound channel full, dropping message".into());
                    }
                }
            }
        }) as Box<dyn FnMut(JsValue)>);
        let _ = js_sys::Reflect::set(
            &ws_recv, &"onmessage".into(), on_msg.as_ref().unchecked_ref(),
        );
        on_msg.forget();

        // onclose: drop Sender → channel EOF → accept_stream returns None
        let ws_close = ws.clone();
        let conn_close = connected.clone();
        let in_tx_close = in_tx.clone();
        let on_close = Closure::wrap(Box::new(move || {
            *in_tx_close.borrow_mut() = None;
            conn_close.store(false, Ordering::SeqCst);
        }) as Box<dyn FnMut()>);
        let _ = js_sys::Reflect::set(
            &ws_close, &"onclose".into(), on_close.as_ref().unchecked_ref(),
        );
        on_close.forget();

        let ws_error = ws.clone();
        let conn_error = connected.clone();
        let in_tx_error = in_tx.clone();
        let on_error = Closure::wrap(Box::new(move || {
            *in_tx_error.borrow_mut() = None;
            conn_error.store(false, Ordering::SeqCst);
            #[cfg(feature = "debug")]
            web_sys::console::error_1(&"[Transport] WebSocket error".into());
        }) as Box<dyn FnMut()>);
        let _ = js_sys::Reflect::set(
            &ws_error, &"onerror".into(), on_error.as_ref().unchecked_ref(),
        );
        on_error.forget();

        // Wait for open
        let (open_tx, open_rx) = futures::channel::oneshot::channel();
        let open_tx = std::cell::RefCell::new(Some(open_tx));
        let on_open = Closure::wrap(Box::new(move || {
            if let Some(tx) = open_tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        }) as Box<dyn FnMut()>);
        let _ = js_sys::Reflect::set(
            &ws, &"onopen".into(), on_open.as_ref().unchecked_ref(),
        );
        on_open.forget();
        let _ = open_rx.await;

        self.connected.store(true, Ordering::SeqCst);
        self.inbound_rx = Arc::new(futures::lock::Mutex::new(in_rx));
        self.high_tx = high_tx;
        self.low_tx = low_tx;
        Ok(())
    }

    /// 发送原始字节 (NetHeader: [Tag][Len][Peer][Doc])
    pub fn send_bytes(&self, peer_id: u32, doc_id: u32, tag: NTag, payload: &[u8], priority: Priority) {
        let total_len = NET_HEADER_SIZE + payload.len();
        let mut buf = Vec::with_capacity(total_len);

        buf.push(tag as u8);
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes()); // Len
        buf.extend_from_slice(&peer_id.to_le_bytes());                 // Peer
        buf.extend_from_slice(&doc_id.to_le_bytes());                  // Doc
        buf.extend_from_slice(payload);

        let mut tx = match priority {
            Priority::High => self.high_tx.clone(),
            Priority::Low => self.low_tx.clone(),
        };
        wasm_bindgen_futures::spawn_local(async move {
            let _ = tx.send(buf).await;
        });
    }

    /// Helper: 序列化并发送
    pub fn send_msg<T: serde::Serialize>(&self, peer_id: u32, doc_id: u32, tag: NTag, msg: &T, priority: Priority) {
        if let Ok(data) = bincode::serde::encode_to_vec(msg, bincode_config()) {
            self.send_bytes(peer_id, doc_id, tag, &data, priority);
        } else {
            #[cfg(feature="debug")]
            web_sys::console::error_1(&"Failed to serialize msg".into());
        }
    }

    pub async fn accept_stream(&self) -> Option<NetFrame> {
        self.inbound_rx.lock().await.next().await
    }

    pub fn is_connection_valid(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}
