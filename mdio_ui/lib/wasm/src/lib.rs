use wasm_bindgen::JsCast;

mod lpp_protocol;
mod lppbuf;
mod net_protocol;
mod transport;

use wasm_bindgen::prelude::*;
use js_sys::{SharedArrayBuffer};

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use futures::{select, FutureExt};

use crate::lppbuf::*;
use crate::lpp_protocol::{MessageBuilder, MessageParser, LTag, AST_FULL, AST_PATCH};
use crate::net_protocol::{WireOp, NTag};
use transport::*;

use crop::Rope;
use cola::{Replica, ReplicaId, Anchor, AnchorBias};

extern crate console_error_panic_hook;
use std::panic;

// 宏定义: Debug toggle
#[cfg(feature = "debug")]
macro_rules! debug_log { ($($arg:tt)*) => { drill_log!(&format!($($arg)*).into()) }; }
#[cfg(not(feature = "debug"))]
macro_rules! debug_log { ($($arg:tt)*) => { () }; }

#[cfg(feature = "debug")]
macro_rules! debug_error { ($($arg:tt)*) => { web_sys::console::error_1(&format!($($arg)*).into()) }; }
#[cfg(not(feature = "debug"))]
macro_rules! debug_error { ($($arg:tt)*) => { () }; }

// DRILL 打点（无条件、但只在 wasm 真打；原生 cargo test 里 cfg 抹掉 —— 否则 wasm-bindgen 的 console 会在原生测试 panic/SIGABRT）。
// 收尾（方案 §7 步骤 6）✓：release wasm 里**完全静音**，只在带 debug feature 的构建里输出；
// 需要诊断时用 `--features debug` 重编即可，无需改任何调用点。
// ✓ 静音分支**不引用 $e**（引用会让 `&format!(..).into()` 失去目标类型 ⇒ E0283 ✗）：
// cfg 掉的语句在类型检查前就被移除，所以参数根本不会被求值。
#[macro_export]
macro_rules! drill_log {
    ($e:expr) => {{
        #[cfg(all(target_arch = "wasm32", feature = "debug"))]
        web_sys::console::log_1($e);
    }};
}

// ========== 工具函数 ==========

// 坐标模式位掩码（offset 模式用）
const MODE_MONACO: u8 = 0b01;
const MODE_OFFSET: u8 = 0b10;
const MODE_AST: u8 = 0b100;
const MAX_UNDO_DEPTH: usize = 1000;

// Cola 作者推荐的 Bincode 配置: Little Endian + VarInt
fn bincode_config() -> impl bincode::config::Config {
    bincode::config::standard()
        .with_little_endian()
        .with_variable_int_encoding()
}

#[cfg(not(feature = "nodejs"))]
#[allow(dead_code)]
pub(crate) fn get_performance() -> web_sys::Performance {
    if let Some(window) = web_sys::window() {
        if let Some(perf) = window.performance() {
            return perf;
        }
    }
    let global = js_sys::global();
    if let Ok(worker_scope) = global.dyn_into::<web_sys::WorkerGlobalScope>() {
        if let Some(perf) = worker_scope.performance() {
            return perf;
        }
    }
    panic!("Performance API not found in this environment");
}

// Node.js: globalThis.performance polyfilled by node_polyfill.mjs before WASM load
#[cfg(feature = "nodejs")]
#[allow(dead_code)]
pub(crate) fn get_performance() -> web_sys::Performance {
    let global = js_sys::global();
    js_sys::Reflect::get(&global, &"performance".into())
        .expect("Performance not polyfilled. Ensure node_polyfill.mjs runs before WASM init.")
        .unchecked_into()
}


#[wasm_bindgen]
pub struct LppUpdater {
    reader: LinearBufferReader,
}

#[wasm_bindgen]
impl LppUpdater {
    pub fn update(&self, idx: usize, sab: SharedArrayBuffer) {
        self.reader.update_buffer(idx, sab); // 这里不需要 &mut
    }
}

// Undo/Redo Record
struct UndoRecord {
    op_type: OpType,
    anchor: Anchor,
    content: String,                      // Saved text (empty only for insert-without-content edge cases)
    cursor_before: Option<(u32, u32)>,    // Monaco (line, col) before operation
    cursor_after: Option<(u32, u32)>,     // Monaco (line, col) after operation
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum OpType { Insert, Delete }

struct ImeSession {
    composing: bool,
}

// Session 结构体
struct DocumentSession {
    doc_id: u32,
    peer_id: u32,
    /// 本副本所属的血统世代号（服务端 checkpoint 携带）。服务端每个下行 CRDT 帧
    /// 都带它；不等 ⇒ 这份副本是另一条血统的后代，集成只会永久停车 ⇒ 先换代重取。
    lineage: u64,
    rope: Rope,
    replica: Replica,
    undo_stack: VecDeque<UndoRecord>,
    redo_stack: VecDeque<UndoRecord>,
    mode_mask: u8,
    ime: ImeSession,
    pending_inserts: HashMap<(u64, usize, usize), String>,
}

impl DocumentSession {
    fn push_undo(&mut self, record: UndoRecord) {
        self.undo_stack.push_back(record);
        while self.undo_stack.len() > MAX_UNDO_DEPTH {
            self.undo_stack.pop_front();
        }
    }

    fn from_snapshot(doc_id: u32, peer_id: u32, initial_text: String, crdt_state: cola::EncodedReplica, lineage: u64) -> Self {
        let text_len = initial_text.len();
        let rope = Rope::from(initial_text);

        let replica = Replica::decode(peer_id as ReplicaId, &crdt_state).expect("CRDT Decode");
        debug_log!("[from_snapshot] text_len={} replica_len={} match={} lineage={}", text_len, replica.len(), text_len == replica.len(), lineage);

        Self {
            doc_id,
            peer_id,
            lineage,
            rope,
            replica,
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            mode_mask: MODE_MONACO,
            ime: ImeSession {
                composing: false,
            },
            pending_inserts: HashMap::new(),
        }
    }

    // ----------------------------------------------------
    // 🧮 坐标转换核心逻辑 (Monaco Line/Col <-> UTF8 Byte)
    // ----------------------------------------------------

    fn monaco_to_byte(&mut self, line: usize, col: usize) -> usize {
        // 关键修复：允许 line 等于 line_len（即文件末尾的虚空行）
        // crop 认为 "text\n" 是 1 行。Monaco 认为是 2 行。
        // 当用户在第 2 行输入时，line=1，line_len=1。
        if line >= self.rope.line_len() {
            return self.rope.byte_len();
        }

        let line_start = self.rope.byte_of_line(line);
        let line_slice = self.rope.line(line);
        let utf16_len = line_slice.utf16_len();

        // 边界保护：如果 col 超过了行长，就认为是在行尾
        let safe_col = col.min(utf16_len);

        let byte_in_line = if safe_col == utf16_len {
            line_slice.byte_len()
        } else {
            line_slice.byte_of_utf16_code_unit(safe_col)
        };

        line_start + byte_in_line
    }

    fn byte_to_monaco(&mut self, offset: usize) -> (u32, u32) {
        let safe_offset = offset.min(self.rope.byte_len());

        let line_idx = self.rope.line_of_byte(safe_offset);
        let line_start = self.rope.byte_of_line(line_idx);
        let byte_in_line = safe_offset - line_start;

        // 算 UTF-16 列
        let col_utf16 = if line_idx < self.rope.line_len() {
            let line_slice = self.rope.line(line_idx);
            // line_slice.utf16_code_unit_of_byte(byte_in_line)
            // 优化：处理边界，防止 panic
            if byte_in_line >= line_slice.byte_len() {
                // 如果刚好在行尾/换行符之后，视为该行的结束
                line_slice.utf16_len()
            } else {
                line_slice.utf16_code_unit_of_byte(byte_in_line)
            }
        } else { 0 };

        // Return 1-based
        ((line_idx + 1) as u32, (col_utf16 + 1) as u32)
    }

    // -------------------------------------------------------------------------
    //    原则：先计算Monaco坐标 -> 定锚点 -> 改Rope -> 改CRDT
    // -------------------------------------------------------------------------
    fn handle_insert(&mut self, l: usize, c: usize, content: String, record_undo: bool) -> Vec<u8> {
        // IME: skip all local changes — the OS/browser composition UI handles display.
        // All intermediate text is invisible to rope/CRDT/undo/network.
        if self.ime.composing {
            return Vec::new();
        }

        let offset = self.monaco_to_byte(l, c);
        // 1. 先定锚点 (基于插入前的位置)
        let anchor = self.replica.create_anchor(offset, AnchorBias::Right);

        // 2. 物理写入
        self.rope.insert(offset, &content);

        // 3. CRDT 更新
        let op = self.replica.inserted(offset, content.len());

         // Debug: 确认插入位置
        debug_log!("[Insert] '{}' at {}, Len: {}. Anchor created.", content.escape_debug(), offset, content.len());

        // 4. 存 Undo (存 Anchor, 不存物理 offset)
        // 只有在需要时才推入 Undo 栈
        if record_undo {
            debug_log!("[UndoStack] Insert Recorded: '{}'", content.escape_debug());
            self.push_undo(UndoRecord {
                op_type: OpType::Insert,
                anchor,
                content: content.clone(),
                cursor_before: None,
                cursor_after: None,
            });
            self.redo_stack.clear();
        } else {
            debug_log!("[UndoStack] Insert Skipped (NoUndo): '{}'", content.escape_debug());
        }

        // 5. 产生 WireOp 字节 (不直接发送，由调用方 dispatch)
        let wire_ops = vec![WireOp::Insert { op, content }];
        bincode::serde::encode_to_vec(&wire_ops, bincode_config()).unwrap_or_default()
    }

    fn handle_delete(&mut self, l1: usize, c1: usize, l2: usize, c2: usize, record_undo: bool) -> Vec<u8> {
        let start = self.monaco_to_byte(l1, c1);
        let end = self.monaco_to_byte(l2, c2);
        // ── 打点（无条件，不受 debug feature 影响）：删除为什么会被静默吞掉 ──
        if start >= end {
            return Vec::new();
        }

        // IME: skip all local changes (same rationale as handle_insert)
        if self.ime.composing {
            return Vec::new();
        }

        // 1. 抢救文本
        let deleted_content = self.rope.byte_slice(start..end).to_string();

        // 2. 创建锚点 (Right Bias: 若在此处插入内容，锚点右移)
        let anchor = self.replica.create_anchor(start, AnchorBias::Right);

        // 3. 修改数据
        self.rope.delete(start..end);

        // 立即测试解析
        if let Some(_deleted_content) = self.replica.resolve_anchor(anchor) {
            debug_log!("[Delete] Anchor created at {}, resolves to {}", start, _deleted_content);
        }
        let op = self.replica.deleted(start..end);
        // 打点：真正执行了删除（与上面的静默 return 对照）
        debug_log!("[Delete] Range {}..{}, Text: '{}'", start, end, deleted_content.escape_debug());

        if record_undo {
            debug_log!("[UndoStack] Delete Recorded: '{}'", deleted_content.escape_debug());
            self.push_undo(UndoRecord {
                op_type: OpType::Delete,
                anchor,
                content: deleted_content,
                cursor_before: None,
                cursor_after: None,
            });
            self.redo_stack.clear();
        } else {
            debug_log!("[UndoStack] Delete Skipped (NoUndo): '{}'", deleted_content.escape_debug());
        }

        let wire_ops = vec![WireOp::Delete { op }];
        bincode::serde::encode_to_vec(&wire_ops, bincode_config()).unwrap_or_default()
    }

    // ── offset 模式 handler ──

    fn handle_insert_at(&mut self, offset: usize, content: String, record_undo: bool) -> Vec<u8> {
        let rope_len = self.rope.utf16_len();
        if offset > rope_len {
            web_sys::console::error_1(&format!(
                "[InsertAt] CLAMPED offset {} > rope.utf16_len {} — rope text head: {:?}",
                offset, rope_len,
                &self.rope.to_string()[..rope_len.min(200)],
            ).into());
        }
        let safe_offset = offset.min(rope_len);
        let byte_offset = self.rope.byte_of_utf16_code_unit(safe_offset);
        // 1. 先定锚点 (基于插入前的位置)
        let anchor = self.replica.create_anchor(byte_offset, AnchorBias::Right);
        // 2. 物理写入
        self.rope.insert(byte_offset, &content);
        // 3. CRDT 更新
        let op = self.replica.inserted(byte_offset, content.len());
        // 4. 存 Undo
        if record_undo {
            self.push_undo(UndoRecord {
                op_type: OpType::Insert,
                anchor,
                content: content.clone(),
                cursor_before: None,
                cursor_after: None,
            });
            self.redo_stack.clear();
        }
        // 5. 产生 WireOp 字节
        let wire_ops = vec![WireOp::Insert { op, content }];
        bincode::serde::encode_to_vec(&wire_ops, bincode_config()).unwrap_or_default()
    }

    fn handle_delete_at(&mut self, start_offset: usize, end_offset: usize, record_undo: bool) -> Vec<u8> {
        let rope_len = self.rope.utf16_len();
        if start_offset > rope_len || end_offset > rope_len {
            web_sys::console::error_1(&format!(
                "[DeleteAt] CLAMPED range ({}..{}) exceeds rope.utf16_len {} — rope text head: {:?}",
                start_offset, end_offset, rope_len,
                &self.rope.to_string()[..rope_len.min(200)],
            ).into());
        }
        let start = self.rope.byte_of_utf16_code_unit(start_offset.min(rope_len));
        let end   = self.rope.byte_of_utf16_code_unit(end_offset.min(rope_len));
        if start >= end { return Vec::new(); }
        // 1. 抢救文本
        let deleted_content = self.rope.byte_slice(start..end).to_string();
        // 2. 创建锚点
        let anchor = self.replica.create_anchor(start, AnchorBias::Right);
        // 3. 修改数据
        self.rope.delete(start..end);
        let op = self.replica.deleted(start..end);
        debug_log!("[DeleteAt] range {}..{} (byte {}..{}), Text: '{}'", start_offset, end_offset, start, end, deleted_content.escape_debug());
        if record_undo {
            self.push_undo(UndoRecord {
                op_type: OpType::Delete,
                anchor,
                content: deleted_content,
                cursor_before: None,
                cursor_after: None,
            });
            self.redo_stack.clear();
        }
        let wire_ops = vec![WireOp::Delete { op }];
        bincode::serde::encode_to_vec(&wire_ops, bincode_config()).unwrap_or_default()
    }


    // JS IME 结束时，请作为一个原子操作记下来"
    fn handle_explicit_undo(&mut self, l: usize, c: usize, content: String) -> Result<(), JsValue> {
        let end_offset = self.monaco_to_byte(l, c);
        // Right Bias: Anchor 在插入文本的【后面】，undo 时从锚点往前删
        let anchor = self.replica.create_anchor(end_offset, AnchorBias::Right);

        debug_log!("[ExplicitUndo] Recorded '{}' at end={}, len={}", content.escape_debug(), end_offset, content.len());

        self.push_undo(UndoRecord {
            op_type: OpType::Insert,
            anchor,
            content,
            cursor_before: None,
            cursor_after: None,
        });
        self.redo_stack.clear();

        Ok(())
    }

    fn handle_ime_begin(&mut self, _l: usize, _c: usize) {
        self.ime.composing = true;
        debug_log!("[IME] Composition started");
    }

    /// Cancel IME composition. Called when the user cancels composition (e.g., Esc)
    /// or on any unexpected composition end without commit. Clears the composing
    /// flag to prevent permanent blocking of all insert/delete/undo/redo operations.
    fn handle_ime_cancel(&mut self) {
        self.ime.composing = false;
        debug_log!("[IME] Composition cancelled");
    }

    /// Commit IME composition as a single atomic insert.
    /// Payload format: [line:u32 LE][col:u32 LE][textLen:u32 LE][text:N] (same as parse_insert)
    fn handle_ime_commit(&mut self, payload: &[u8]) -> Vec<u8> {
        self.ime.composing = false;

        if payload.len() < 12 { return Vec::new(); }
        let l = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
        let c = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
        let text_len = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
        if payload.len() < 12 + text_len { return Vec::new(); }
        let content = match String::from_utf8(payload[12..12 + text_len].to_vec()) {
            Ok(s) => s,
            Err(_) => {
                web_sys::console::error_1(&"IME commit: invalid UTF-8".into());
                return Vec::new();
            }
        };

        if content.is_empty() { return Vec::new(); }

        let offset = self.monaco_to_byte(l, c);

        // Single atomic operation: anchor → rope → CRDT → undo → network
        let anchor = self.replica.create_anchor(offset, AnchorBias::Right);
        self.rope.insert(offset, &content);
        let op = self.replica.inserted(offset, content.len());

        debug_log!("[IME] Committed '{}' at offset {}, len={}", content.escape_debug(), offset, content.len());

        self.push_undo(UndoRecord {
            op_type: OpType::Insert,
            anchor,
            content: content.clone(),
            cursor_before: None,
            cursor_after: None,
        });
        self.redo_stack.clear();

        let wire_ops = vec![WireOp::Insert { op, content }];
        bincode::serde::encode_to_vec(&wire_ops, bincode_config()).unwrap_or_default()
    }

    // -------------------------------------------------------------------------
    // Undo: 必须严格遵循 先计算坐标给前端 -> 再修改本地 Rope
    // -------------------------------------------------------------------------
    fn handle_undo(&mut self, writer: &LinearBufferWriter) -> Vec<u8> {
        debug_log!("[Undo] undo_stack size before: {}", self.undo_stack.len());
        debug_log!("[Undo] redo_stack size before: {}", self.redo_stack.len());
        let mut wire_ops: Vec<WireOp> = Vec::new();
        if let Some(record) = self.undo_stack.pop_back() {
            debug_log!("[Undo] Popped record: type={:?}, text={:?}", record.op_type, record.content);
            // 🎯 解析 Anchor 获取当前真实位置
            let pos = match self.replica.resolve_anchor(record.anchor) {
                Some(p) => p,
                None => {
                    debug_log!("[Undo] Anchor not yet integrated, deferring");
                    self.undo_stack.push_back(record);
                    return Vec::new();
                }
            };

            match record.op_type {
                OpType::Insert => {
                    // ⏪ 撤销插入
                    // Right Bias: Anchor 在插入后位于文本【后面】
                    // 所以删除范围是 (pos - len) .. pos
                    let start = pos.saturating_sub(record.content.len());
                    let end = pos;
                    debug_log!("[Undo-Insert] Pos: {}, Range: {}..{}", pos, start, end);

                    if end <= self.rope.byte_len() {
                        if self.mode_mask & MODE_MONACO != 0 {
                            // 1. 算 Line/Col 给 JS
                            let (l1, c1) = self.byte_to_monaco(start);
                            let (l2, c2) = self.byte_to_monaco(end);

                            // 2. To JS
                            let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                                .payload_remote_delete(l1, c1, l2, c2).build();
                            let _ = writer.write_chunk(&p);
                        }
                        if self.mode_mask & MODE_OFFSET != 0 {
                            let from = self.rope.utf16_code_unit_of_byte(start);
                            let to   = self.rope.utf16_code_unit_of_byte(end);
                            let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                                .mode_offset()
                                .payload_remote_delete_at(from as u32, to as u32).build();
                            let _ = writer.write_chunk(&p);
                        }

                        // 3. 本地执行
                        self.rope.delete(start..end);
                        let op = self.replica.deleted(start..end);

                        // 4. 收集 WireOp (由调用方 dispatch)
                        wire_ops.push(WireOp::Delete { op });

                        // 5. 把原始Insert记录放入redo_stack
                        self.redo_stack.push_back(record);
                        debug_log!("[Undo] Insert Pushed to redo_stack");
                    } else {
                        // Range out of bounds — push record back to avoid silent loss
                        self.undo_stack.push_back(record);
                        debug_log!("[Undo] Insert range OOB, record returned to undo_stack");
                    }
                },
                OpType::Delete => {
                    // 撤销删除 = 插入
                    if !record.content.is_empty() {
                        let content = record.content.clone();
                        // 🔥 Right Bias: Anchor 指向原删除点的 Start (因为删除后 start 和 end 重合在一点)
                        let start = pos;

                        if self.mode_mask & MODE_MONACO != 0 {
                            // 1. 算坐标 (插入点的坐标)
                            let (l, c) = self.byte_to_monaco(start);

                            // 2. To JS
                            let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                                .payload_remote_insert(l, c, &content).build();
                            let _ = writer.write_chunk(&p);
                        }
                        if self.mode_mask & MODE_OFFSET != 0 {
                            let offset = self.rope.utf16_code_unit_of_byte(start);
                            let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                                .mode_offset()
                                .payload_remote_insert_at(offset as u32, &content).build();
                            let _ = writer.write_chunk(&p);
                        }

                        // 3. 本地执行
                        self.rope.insert(start, &content);
                        let op = self.replica.inserted(start, content.len());

                        // 4. 收集 WireOp
                        wire_ops.push(WireOp::Insert { op, content });

                        // 5. 把原始Delete记录放入redo_stack
                        self.redo_stack.push_back(record);
                        debug_log!("[Undo] Delete Pushed to undo_stack");
                    }
                }
            }
        }
        debug_log!("[Undo] undo_stack size after: {}", self.undo_stack.len());
        debug_log!("[Undo] redo_stack size after: {}", self.redo_stack.len());
        bincode::serde::encode_to_vec(&wire_ops, bincode_config()).unwrap_or_default()
    }

    // -------------------------------------------------------------------------
    // Redo: 同理，先计算 -> 再执行
    // -------------------------------------------------------------------------
    fn handle_redo(&mut self, writer: &LinearBufferWriter) -> Vec<u8> {
        debug_log!("[Redo] undo_stack size before: {}", self.undo_stack.len());
        debug_log!("[Redo] redo_stack size before: {}", self.redo_stack.len());

        let mut wire_ops: Vec<WireOp> = Vec::new();
        if let Some(record) = self.redo_stack.pop_back() {
            debug_log!("[Redo] Found record: type={:?}, len={}, has_text={}", record.op_type, record.content.len(), !record.content.is_empty());
            let pos = match self.replica.resolve_anchor(record.anchor) {
                Some(p) => p,
                None => {
                    debug_log!("[Redo] Anchor not yet integrated, deferring");
                    self.redo_stack.push_back(record);
                    return Vec::new();
                }
            };

            // 🔍 关键：检查文本内容
            debug_log!("[Redo] Text content (raw): '{:?}'", record.content);
            debug_log!("[Redo] Text length in bytes: {}", record.content.len());
            debug_log!("[Redo] Text as hex: {:02x?}", record.content.as_bytes());

            // Anchor -> 位置
            debug_log!("[Redo] Resolved position: {}", pos);

            // Redo 的操作类型和原始操作一致
            // 当初是 Insert，Redo 就是 Insert。
            match record.op_type {
                OpType::Insert => {
                    if !record.content.is_empty() {
                        let content = record.content.clone();
                        // Right Bias: after undo, anchor resolves to the original insertion point.
                        // Re-insert at pos — anchor shifts right to pos+len again.
                        let start = pos;
                        debug_log!("[Redo-Insert] Pos: {}, Text: '{}'", start, content.escape_debug());

                        if self.mode_mask & MODE_MONACO != 0 {
                            let (l, c) = self.byte_to_monaco(start);

                            // 1. To JS
                            debug_log!("[Redo] Insert monaco: line={}, col={}", l, c);
                            let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                                .payload_remote_insert(l, c, &content).build();
                            let _ = writer.write_chunk(&p);
                        }
                        if self.mode_mask & MODE_OFFSET != 0 {
                            let offset = self.rope.utf16_code_unit_of_byte(start);
                            let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                                .mode_offset()
                                .payload_remote_insert_at(offset as u32, &content).build();
                            let _ = writer.write_chunk(&p);
                        }

                        // 2. 本地 Do
                        self.rope.insert(start, &content);
                        // 注意：这里 op_type 依然是 Insert，因为这是 Redo
                        let op = self.replica.inserted(start, content.len());

                        // 3. 收集 WireOp
                        wire_ops.push(WireOp::Insert { op, content });

                        // 放回undo_stack
                        self.undo_stack.push_back(record);
                        debug_log!("[Redo] Insert Pushed to undo_stack");
                    } else {
                        debug_error!("[Redo] ERROR: Record has no text!");
                    }
                },
                OpType::Delete => {
                    // Right Bias Anchor 在 Undo (恢复文本) 后，被推到了恢复文本的右侧 (pos)
                    // 所以删除范围是 (pos - len) .. pos
                    let start = pos.saturating_sub(record.content.len());
                    let end = pos;
                    debug_log!("[Redo-Delete] Pos: {}, Range: {}..{}", pos, start, end);
                    // Protect
                    if end <= self.rope.byte_len() {
                        if self.mode_mask & MODE_MONACO != 0 {
                            let (l1, c1) = self.byte_to_monaco(start);
                            let (l2, c2) = self.byte_to_monaco(end);

                            let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                                .payload_remote_delete(l1, c1, l2, c2).build();
                            let _ = writer.write_chunk(&p);
                            debug_log!("[Redo] Delete monaco: line1={}, col1={}, line2={}, col2={}", l1, c1, l2, c2);
                        }
                        if self.mode_mask & MODE_OFFSET != 0 {
                            let from = self.rope.utf16_code_unit_of_byte(start);
                            let to   = self.rope.utf16_code_unit_of_byte(end);
                            let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                                .mode_offset()
                                .payload_remote_delete_at(from as u32, to as u32).build();
                            let _ = writer.write_chunk(&p);
                        }

                        self.rope.delete(start..end);
                        let op = self.replica.deleted(start..end);

                        wire_ops.push(WireOp::Delete { op });

                        // 放回undo_stack
                        self.undo_stack.push_back(record);
                        debug_log!("[Redo] Delete Pushed to undo_stack");
                    } else {
                        // Range out of bounds — push record back
                        self.redo_stack.push_back(record);
                        debug_log!("[Redo] Delete range OOB, record returned to redo_stack");
                    }
                }
            }
        }
        debug_log!("[Redo] undo_stack size after: {}", self.undo_stack.len());
        debug_log!("[Redo] redo_stack size after: {}", self.redo_stack.len());
        bincode::serde::encode_to_vec(&wire_ops, bincode_config()).unwrap_or_default()
    }

    // 处理网络下行消息
    fn apply_net_msg(&mut self, msg: &WireOp, writer: &LinearBufferWriter) -> Result<(), JsValue> {
        match msg {
            WireOp::Insert { op, content } => {
                debug_log!("[WireOp::Insert] content={}", content);
                if let Some(offset) = self.replica.integrate_insertion(op) {
                    if self.mode_mask & MODE_MONACO != 0 {
                        let (l, c) = self.byte_to_monaco(offset);
                        debug_log!("[ApplyNet] Insert at ({},{}) inserted_by={} text='{}' len={}", l, c, op.inserted_by(), content, content.len());

                        let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                            .payload_remote_insert(l, c, content).build();
                        // 打点：写给 Monaco 的那条下行帧（tag 首字节含 mode 位 —— 前端 (tag & 0x80) 会跳过 offset 模式帧）
                        writer.write_chunk(&p)?;
                    }
                    if self.mode_mask & MODE_OFFSET != 0 {
                        let pos = self.rope.utf16_code_unit_of_byte(offset);
                        let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                            .mode_offset()
                            .payload_remote_insert_at(pos as u32, content).build();
                        writer.write_chunk(&p)?;
                    }

                    self.rope.insert(offset, content);
                } else {
                    debug_log!("[ApplyNet] integrate None! replica_len={} inserted_by={}", self.replica.len(), op.inserted_by());
                    let range = op.text().temporal_range();
                    let key = (op.inserted_by(), range.start, range.end);
                    self.pending_inserts.insert(key, content.clone());
                }
            },
            WireOp::Delete { op } => {
                debug_log!("[WireOp::Delete]");
                let ranges = self.replica.integrate_deletion(op);

                for range in ranges {
                    if self.mode_mask & MODE_MONACO != 0 {
                        let (l1, c1) = self.byte_to_monaco(range.start);
                        let (l2, c2) = self.byte_to_monaco(range.end);

                        let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                            .payload_remote_delete(l1, c1, l2, c2).build();
                        writer.write_chunk(&p)?;
                    }
                    if self.mode_mask & MODE_OFFSET != 0 {
                        let from = self.rope.utf16_code_unit_of_byte(range.start);
                        let to   = self.rope.utf16_code_unit_of_byte(range.end);
                        let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                            .mode_offset()
                            .payload_remote_delete_at(from as u32, to as u32).build();
                        writer.write_chunk(&p)?;
                    }

                    self.rope.delete(range.clone());
                }
            },
            _ => {}
        }

        // CRDT 积压释放（和 server 端 apply_update 一致）
        loop {
            let mut processed = 0;

            let back_ins: Vec<_> = self.replica.backlogged_insertions()
                .map(|(txt, off)| (txt.clone(), off))
                .collect();
            for (txt, offset) in back_ins {
                let r = txt.temporal_range();
                let key = (txt.inserted_by(), r.start, r.end);
                if let Some(content) = self.pending_inserts.remove(&key) {
                    if self.mode_mask & MODE_MONACO != 0 {
                        let (l, c) = self.byte_to_monaco(offset);
                        let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                            .payload_remote_insert(l, c, &content).build();
                        writer.write_chunk(&p)?;
                    }
                    if self.mode_mask & MODE_OFFSET != 0 {
                        let pos = self.rope.utf16_code_unit_of_byte(offset);
                        let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                            .mode_offset()
                            .payload_remote_insert_at(pos as u32, &content).build();
                        writer.write_chunk(&p)?;
                    }
                    self.rope.insert(offset, &content);
                    processed += 1;
                }
            }

            let back_del: Vec<_> = self.replica.backlogged_deletions().collect();
            for ranges in back_del {
                for range in ranges.into_iter().rev() {
                    if self.mode_mask & MODE_MONACO != 0 {
                        let (l1, c1) = self.byte_to_monaco(range.start);
                        let (l2, c2) = self.byte_to_monaco(range.end);
                        let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                            .payload_remote_delete(l1, c1, l2, c2).build();
                        writer.write_chunk(&p)?;
                    }
                    if self.mode_mask & MODE_OFFSET != 0 {
                        let from = self.rope.utf16_code_unit_of_byte(range.start);
                        let to   = self.rope.utf16_code_unit_of_byte(range.end);
                        let p = MessageBuilder::new().header(LTag::RemoteOp, self.peer_id, self.doc_id)
                            .mode_offset()
                            .payload_remote_delete_at(from as u32, to as u32).build();
                        writer.write_chunk(&p)?;
                    }
                    self.rope.delete(range);
                    processed += 1;
                }
            }

            if processed == 0 { break; }
        }

        Ok(())
    }
}


// ========== EditorCore (主入口) ==========

// ========== PeerSession ==========

struct PeerSession {
    peer_id: u32,
    sessions: HashMap<u32, DocumentSession>,
    pending_mode_mask: HashMap<u32, u8>,
    active_mode_mask:  HashMap<u32, u8>,
}

impl PeerSession {
    fn new(peer_id: u32) -> Self {
        Self {
            peer_id,
            sessions: HashMap::new(),
            pending_mode_mask: HashMap::new(),
            active_mode_mask:  HashMap::new(),
        }
    }
}

// ========== Reconnection State Machine ==========

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum State {
    Connected = 0,
    Disconnected = 1,
    Reconnecting = 2,
    Syncing = 3,
    Idle = 4,
}

/// DnTree payload 中的文件条目（refresh_tree_json inline 产出的 JSON）。
#[derive(serde::Deserialize)]
struct FileNodeWithHash {
    path: String,
    id: u32,
    hash: String,
}

struct ReconnectionState {
    state: State,
    retry_count: u32,
    url: String,
    pending_ops: HashMap<u32, Vec<Vec<u8>>>,
    sync_ops: Vec<(u32, Vec<u8>)>,
    local_hashes: HashMap<u32, String>,
    server_hashes: HashMap<u32, String>,
    /// **跨重启的名字账本**：doc_id → path（每次 DnTree 都记；服务端重启后旧 id 的 path 靠它翻回，
    /// 再在新树里按 path 找到新 id —— 恢复只用名字，不用句柄）。
    known_paths: HashMap<u32, String>,
    /// virtual ↔ real：JS 侧的 doc id（虚拟，跨重启稳定）↔ 服务端的 real id（每次重启换代）。
    /// 上行翻 v→r、下行翻 r→v；exec（按 exec_id 路由）不翻。
    virtual_to_real: HashMap<u32, u32>,
    real_to_virtual: HashMap<u32, u32>,
    sync_waiting: HashSet<u32>,
    /// 已就"血统不等"请求过换代（重发 UpOpen）的文档。一个血统只请求一次：
    /// 重取完成后本地 lineage 就与帧一致，不会再触发；血统再变才再请求——不是轮询。
    relineage_pending: HashSet<u32>,
    on_state_change: Option<Box<dyn Fn(u8)>>,
}

impl ReconnectionState {
    fn new() -> Self {
        Self {
            state: State::Connected,
            retry_count: 0,
            url: String::new(),
            pending_ops: HashMap::new(),
            sync_ops: Vec::new(),
            local_hashes: HashMap::new(),
            server_hashes: HashMap::new(),
            known_paths: HashMap::new(),
            virtual_to_real: HashMap::new(),
            real_to_virtual: HashMap::new(),
            sync_waiting: HashSet::new(),
            relineage_pending: HashSet::new(),
            on_state_change: None,
        }
    }

    fn set_on_state_change(&mut self, cb: Box<dyn Fn(u8)>) {
        self.on_state_change = Some(cb);
    }

    fn set_state(&mut self, new_state: State) {
        if self.state != new_state {
            self.state = new_state;
            if let Some(cb) = &self.on_state_change {
                cb(new_state as u8);
            }
        }
    }

    fn dispatch(&mut self, doc_id: u32, peer_id: u32, op: Vec<u8>, transport: &Transport) {
        match self.state {
            State::Connected => {
                if transport.is_connection_valid() {
                    transport.send_bytes(peer_id, doc_id, NTag::UpSync, &op, Priority::High);
                } else {
                    self.set_state(State::Disconnected);
                    self.retry_count = 0;
                    self.pending_ops.entry(doc_id).or_default().push(op);
                }
            }
            State::Disconnected | State::Reconnecting | State::Idle => {
                self.pending_ops.entry(doc_id).or_default().push(op);
            }
            State::Syncing => {
                self.sync_ops.push((doc_id, op));
            }
        }
    }

    fn on_disconnect(&mut self) {
        if self.state != State::Idle {
            self.set_state(State::Disconnected);
            self.retry_count = 0;
        }
    }

    fn backoff_ms(&self) -> u32 {
        (1u32 << self.retry_count.min(4)) * 1000
    }
}

// ========== EditorCore (主入口) ==========

#[wasm_bindgen]
pub struct EditorCore {
    peers: HashMap<u32, PeerSession>,
    upstream: LinearBufferReader,
    downstream: LinearBufferWriter,
    transport: Transport,
    reconnect: ReconnectionState,
    running: AtomicBool,
}

pub(crate) struct WasmTelemetrySample {
    layer: String,
    dur_ns: u64,
    ts_ms: f64,       // performance.now 时刻（与主线程同时间轴）
    tag: u8,
    doc_id: u32,
    terminated: bool,
}

// thread_local 全局采集：run_loop 是 &mut self 的 async（wasm-bindgen 借用检查），
// 从 JS 侧调 get_telemetry_data 会与运行中的 run_loop 冲突（recursive use of an object）。
// 采集写全局、读取全局 —— 不与 self 借用纠缠。
thread_local! {
    pub(crate) static WASM_TELEMETRY: std::cell::RefCell<Vec<WasmTelemetrySample>> =
        std::cell::RefCell::new(Vec::new());
}

/// 按名字对账的纯决策：旧账本里有 path 且新树里同 path 换了 id ⇒ 迁移（pid, old_id, new_id）。
/// 只迁不丢：path 不在新树的不产出任何决策（丢弃由服务端 B1 门回 DnErr 兜底）。
fn plan_remaps(prev_paths: &HashMap<u32, String>, new_by_path: &HashMap<String, u32>,
               docs: &[(u32, u32)]) -> Vec<(u32, u32, u32)> {
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for &(pid, doc_id) in docs {
        if !seen.insert(doc_id) { continue; }   // 同一 doc 挂多个 peer 只决策一次
        if let Some(path) = prev_paths.get(&doc_id) {
            if let Some(&new_id) = new_by_path.get(path) {
                if new_id != doc_id { out.push((pid, doc_id, new_id)); }
            }
        }
    }
    out
}

#[wasm_bindgen]
impl EditorCore {
    #[wasm_bindgen(constructor)]
    pub fn new(
        _reserved: u32, // was peer_id, now unused; kept for JS API compat
        up0: SharedArrayBuffer, up1: SharedArrayBuffer,
        down0: SharedArrayBuffer, down1: SharedArrayBuffer,
        on_upgrade: js_sys::Function,
    ) -> Self {
        panic::set_hook(Box::new(console_error_panic_hook::hook));
        // 开机自报：这条一出现 ⇒ 跑的就是**带打点的这一版 wasm**（用来排除"浏览器吃旧缓存"）
        // （启动打点已随收尾摘除；DRILL-* 系列走 debug_log!，只在调试构建里出现）

        let downstream = LinearBufferWriter::new(down0, down1, on_upgrade);

        let mut reconnect = ReconnectionState::new();
        let ds = downstream.clone();
        reconnect.set_on_state_change(Box::new(move |state_u8| {
            let p = MessageBuilder::new()
                .header(LTag::ConnectionState, 0, 0)
                .payload(&[state_u8])
                .build();
            let _ = ds.write_chunk(&p);
        }));

        Self {
            peers: HashMap::new(),
            upstream: LinearBufferReader::new(up0, up1),
            downstream,
            transport: Transport::new(),
            reconnect,
            running: AtomicBool::new(true),
        }
    }

    pub fn get_updater(&self) -> LppUpdater {
        LppUpdater {
            reader: self.upstream.clone()
        }
    }

    #[wasm_bindgen]
    pub async fn connect(&mut self, url: String) -> Result<(), JsValue> {
        self.reconnect.url = url.clone();
        self.transport.connect(&url).await?;
        // System message: peer_id=0
        self.transport.send_bytes(0, 0, NTag::UpTree, &[], Priority::Low);
        self.reconnect.set_state(State::Connected);
        debug_log!("🌐 Connected. Sent UP_TREE request.");
        Ok(())
    }

    #[wasm_bindgen]
    pub async fn run_loop(&mut self) -> Result<(), JsValue> {
        while self.running.load(Ordering::Relaxed) {
            // ① 断线检测：Connected 状态下检查 transport 连接有效性
            if self.reconnect.state == State::Connected && !self.transport.is_connection_valid() {
                self.reconnect.on_disconnect();
                debug_log!("🔌 Connection lost → DISCONNECTED");
                continue;
            }

            match self.reconnect.state {
                State::Connected | State::Syncing => {
                    self.step_select().await?;
                }
                State::Disconnected => {
                    let delay = self.reconnect.backoff_ms();
                    debug_log!("⏳ Backoff {}ms before reconnect attempt", delay);
                    gloo_timers::future::TimeoutFuture::new(delay).await;
                    self.reconnect.set_state(State::Reconnecting);
                }
                State::Reconnecting => {
                    let url = self.reconnect.url.clone();
                    match self.transport.connect(&url).await {
                        Ok(_) => {
                            self.transport.send_bytes(0, 0, NTag::UpTree, &[], Priority::Low);
                            self.reconnect.set_state(State::Syncing);
                            self.reconnect.retry_count = 0;
                            debug_log!("🔄 Reconnected → SYNCING");
                        }
                        Err(e) => {
                            debug_error!("Reconnect failed: {:?}", e);
                            self.reconnect.retry_count += 1;
                            if self.reconnect.retry_count >= 5 {
                                self.reconnect.set_state(State::Idle);
                                debug_log!("😴 Max retries → IDLE");
                            } else {
                                self.reconnect.set_state(State::Disconnected);
                            }
                        }
                    }
                }
                State::Idle => {
                    debug_log!("😴 IDLE — waiting 30s before next attempt");
                    gloo_timers::future::TimeoutFuture::new(30_000).await;
                    self.reconnect.set_state(State::Reconnecting);
                    self.reconnect.retry_count = 0;
                }
            }
        }
        Ok(())
    }

    /// 单次 select! 迭代：处理一条 upstream 或 network 消息。
    /// Connected 和 Syncing 共享，差异在 process_network_msg 内部按 state 分支。
    async fn step_select(&mut self) -> Result<(), JsValue> {
        // ⚠️ clone 在循环内（每次 step_select 调用时重新 clone）
        // 重连后 Transport 内部 Arc 被替换，必须重新拿最新引用
        let up_handle = self.upstream.clone();
        let net_handle = self.transport.clone();

        // up_fut: 本地编辑，零延迟
        let mut up_fut = async {
            up_handle.recv().await
        }.boxed_local().fuse();

        // net_fut: channel EOF（onclose drop Sender）= 真断线，返回 None
        let st_dbg = self.reconnect.state;
        let mut net_fut = async {
            let valid = net_handle.is_connection_valid();
            if valid {
                net_handle.accept_stream().await
            } else if matches!(st_dbg, State::Connected | State::Syncing) {
                // 实测根因：valid=false 且状态是 Connected ⇒ 之前走 pending() 永久挂起 ⇒
                // step_select 永不返回 ⇒ 外层体检（run_loop :1021）永不到 ⇒ 引擎静默死锁、
                // 帧堆在 channel 里没人吃。改成返回 None ⇒ 走协议自己的断线路径（on_disconnect
                // ⇒ 退避 ⇒ 重连自愈）。不加任何新状态。
                None
            } else {
                futures::future::pending::<Option<NetFrame>>().await
            }
        }.boxed_local().fuse();

        select! {
            // 分支 A: SAB upstream（零延迟）
            up_res = up_fut => {
                match up_res {
                    Ok(raw_data) => {
                        if let Err(_e) = self.process_local_msg(raw_data) {
                            debug_error!("Local Msg Err: {:?}", _e);
                        }
                    },
                    Err(_e) => debug_error!("LPPB Recv Error: {:?}", _e),
                }
            },

            // 分支 B: 网络消息
            net_res = net_fut => {
                match net_res {
                    Some((sender_peer_id, doc_id, tag_u8, payload)) => {
                        if let Ok(tag) = NTag::try_from(tag_u8) {
                            self.process_network_msg(sender_peer_id, doc_id, tag, payload)?;
                        } else {
                            debug_error!("Unknown Net Tag: {}", tag_u8);
                        }
                    },
                    None => {
                        // channel EOF = 连接断开
                        if self.reconnect.state == State::Syncing {
                            self.reconnect.set_state(State::Reconnecting);
                            debug_log!("🔌 Stream ended during SYNCING → RECONNECTING");
                        } else {
                            self.reconnect.on_disconnect();
                            debug_log!("🔌 Network stream ended → DISCONNECTED");
                        }
                    }
                }
            },
        }
        Ok(())
    }

    /// Get or create a PeerSession by local peer_id.
    fn get_or_create_peer(&mut self, peer_id: u32) -> &mut PeerSession {
        self.peers.entry(peer_id).or_insert_with(|| PeerSession::new(peer_id))
    }

    /// 视图关闭：只清**该 adapter** 对该文档的项；只有本连接**再无**任何 adapter 持有
    /// 该文档时，才向服务端发 UpClose。
    ///
    /// 为什么必须这样分：服务端的退订粒度是**连接 × 文档**（gateway
    /// crdt_scope.subscribe(ws_peer, doc)——一连接一个 peer），而关闭的粒度是
    /// **视图**（Monaco / AST / offset）。旧实现无条件发 UpClose，于是"关掉 AST
    /// 视图"就把整条连接对这篇文档的订阅退掉了；而客户端侧又只清被点名的那一个
    /// adapter，其余的 session 原样留下 = **幽灵会话**——它持有上一代血统，会在
    /// 重开时被 fork 成新副本的血统（见 research/2026-09-21-lineage-single-source-model.md §10 ③④）。
    fn close_view(&mut self, peer_id: u32, doc_id: u32, _transport: &Transport) -> Result<(), JsValue> {
        // 切档 ≠ 退订（方案 §5.3 ✓）：**只摘视图** —— 不删会话、不发 UpClose ✗。
        // 会话（Rope + cola 副本 + lineage）的寿命跟「引擎实例 × 文档」走；
        // 退订只由域销毁发生（连接结束 ⇒ 网关 detach_peer + channel_unsubscribe_all）。
        // 原先的三件事已一并移除：sessions.remove ✗、still_wanted 判定、UpClose 发送 ✗
        //   （它们把"关一个视图"翻译成"整篇作废 + 退订"，是本次事故的设计错源）。
        if let Some(p) = self.peers.get_mut(&peer_id) {
            p.pending_mode_mask.remove(&doc_id);
            p.active_mode_mask.remove(&doc_id);
        }
        Ok(())
    }

    /// 内部：真退订（保留给域级/驱逐路径；当前无调用方 —— 退订由连接结束发生 ✓）。
    #[allow(dead_code)]
    fn release_doc(&mut self, peer_id: u32, doc_id: u32, transport: &Transport) -> Result<(), JsValue> {
        if let Some(p) = self.peers.get_mut(&peer_id) {
            p.pending_mode_mask.remove(&doc_id);
            p.active_mode_mask.remove(&doc_id);
            p.sessions.remove(&doc_id);
        }
        let still_wanted = self.peers.values().any(|p| {   // 仅供 release_doc 使用
            p.pending_mode_mask.contains_key(&doc_id)
                || p.active_mode_mask.contains_key(&doc_id)
                || p.sessions.contains_key(&doc_id)
        });
        let _ = still_wanted;
        // 没有任何视图再要它 ⇒ 同步等待表里也必须摘掉。否则"同步进行中把这个文档关了"
        // 会让 sync_waiting 里留一个永远等不到 DnOpen 的条目，客户端就永久卡在 Syncing、
        // 所有下行 CRDT 帧只进 sync_ops 不应用（Monaco 冻住而 render 照常）。
        if !still_wanted {
            self.reconnect.sync_waiting.remove(&doc_id);
            self.reconnect.relineage_pending.remove(&doc_id);
        }
        debug_log!("🚪 [CloseView] peer={} doc={} 仍有 adapter 持有={} ⇒ 退订={}",
            peer_id, doc_id, still_wanted, !still_wanted);
        if !still_wanted {
            transport.send_bytes(peer_id, doc_id, NTag::UpClose, &[], Priority::High);
        }
        Ok(())
    }

    /// 上行载荷约定（与服务端下行同一形状）：[lineage: u64 LE][bincode(Vec<WireOp>)]。
    fn stamp_lineage(lineage: u64, ops: &[u8]) -> Vec<u8> {
        let mut p = Vec::with_capacity(8 + ops.len());
        p.extend_from_slice(&lineage.to_le_bytes());
        p.extend_from_slice(ops);
        p
    }

    /// 解上行载荷前缀。长度不足（无前缀）⇒ 号取 0、原样返回（服务端同样容忍）。
    fn split_lineage(body: &[u8]) -> (u64, &[u8]) {
        if body.len() >= 8 {
            (u64::from_le_bytes(body[0..8].try_into().unwrap()), &body[8..])
        } else {
            (0, body)
        }
    }

    /// 血统不等 ⇒ **整篇重取**：对本连接持有该文档的视图重发 UpOpen，服务端据此回
    /// DN_OPEN（新 checkpoint 带新血统号），DnOpen 分支会把会话整体换掉。
    ///
    /// 每个文档只请求一次（relineage_pending）：重取后本地 lineage 就与帧一致，不会
    /// 再触发；血统再变才再请求。**不是轮询**——触发点是"收到了一条属于别的血统的
    /// 帧"这个事实本身，没有定时器、没有重试循环；万一请求丢了，恢复入口仍是宿主上
    /// 那个可见的"重开"按钮（reopenDoc），与"零帧"同一条处置路径。
    fn request_relineage(&mut self, doc_id: u32, views: &[(u32, u8)]) -> Result<(), JsValue> {
        if self.reconnect.relineage_pending.contains(&doc_id) {
            debug_log!("↩️ [Relineage] doc={} 已请求过换代，等 DnOpen 回来（不重复请求）", doc_id);
            return Ok(());
        }
        self.reconnect.relineage_pending.insert(doc_id);
        for (pid, mask) in views {
            // 服务端按帧头 peer 回 DnOpen（store.rs UP_OPEN 回填），所以 pending 必须
            // 挂在**同一个** peer 上——否则回复会被判成"无人认领的推送"。
            if let Some(p) = self.peers.get_mut(pid) {
                *p.pending_mode_mask.entry(doc_id).or_insert(0) |= *mask;
            }
            self.transport.send_bytes(*pid, doc_id, NTag::UpOpen, &[], Priority::High);
            debug_log!("↩️ [Relineage] doc={} peer={} mask=0b{:03b} 重发 UpOpen（换代重取）", doc_id, pid, mask);
        }
        Ok(())
    }

    /// 步骤 4：把某会话的**当前全文**作为一帧幂等覆盖下发（Type=5 replace_all）。
    ///
    /// 为什么必须有它：新 sink 加入**已经打开**的文档时，服务端按「(连接,文档) 已打开」不再回一份
    /// DnOpen ⇒ 引擎这边也不会走 from_snapshot 那条路 ⇒ 新 sink 永远空着（实测：重挂 Monaco 后编辑器无内容）。
    fn push_full_text(peer_id: u32, doc_id: u32, sess: &DocumentSession, mask: u8,
                      downstream: &LinearBufferWriter) -> Result<(), JsValue> {
        let text = sess.rope.to_string();
        let vdoc = sess.doc_id;   // A1：会话的 doc_id 即虚拟 id（JS 侧键）
        if mask & MODE_MONACO != 0 {
            let p = MessageBuilder::new()
                .header(LTag::RemoteOp, peer_id, vdoc)
                .payload_remote_replace_all(&text)
                .build();
            downstream.write_chunk(&p)?;
        }
        if mask & MODE_OFFSET != 0 {
            let p = MessageBuilder::new()
                .header(LTag::RemoteOp, peer_id, vdoc)
                .mode_offset()
                .payload_remote_replace_all_at(&text)
                .build();
            downstream.write_chunk(&p)?;
        }
        debug_log!("📤 [OpenDoc] peer={} doc={} 补发全文 {} 字符（新 sink 加入已开文档）", peer_id, doc_id, text.chars().count());
        Ok(())
    }

    fn process_local_msg(&mut self, raw: Vec<u8>) -> Result<(), JsValue> {
        let tag_byte = raw[0];
        let is_offset = (tag_byte & 0x80) != 0;
        let t_in = get_performance().now();
        let is_offset = (tag_byte & 0x80) != 0;

        debug_log!("Raw LPPB Packet: {:?}", raw);
        debug_log!("🔥 Worker Recv LPPB Msg. Len: {}", raw.len());

        let parser = match MessageParser::new(&raw) {
            Ok(p) => p,
            Err(_e) => {
                debug_error!("Msg Parse Error: {:?}", _e);
                return Ok(());
            }
        };

        let peer_id = parser.peer_id();
        let doc_id = parser.doc_id();
        // A1：JS 上行的是**虚拟 id**（跨重启稳定）；引擎内部按 real id 键。
        // exec（0x0E）不翻 —— 它的应答按 exec_id 路由、且 doc 只作回声；其余帧 v→r。
        let doc_id = if tag_byte == LTag::ExecReq as u8 { doc_id }
            else { self.reconnect.virtual_to_real.get(&doc_id).copied().unwrap_or(doc_id) };
        debug_log!("LPPB Route: peer={} doc={} tag={}", peer_id, doc_id, tag_byte);

        let downstream = self.downstream.clone();
        let transport = self.transport.clone();

        // 编辑操作产生的 upstream bytes（空 = 无需 dispatch）
        let mut upstream_bytes: Vec<u8> = Vec::new();

        {
            let peer = self.get_or_create_peer(peer_id);

            // ── offset 模式路径 ──
            if is_offset {
                let op = tag_byte & 0x7F;
                let mode_bit = MODE_OFFSET;

                if op == 0x01 {
                    if let Ok(_text) = parser.parse_open_doc() {
                        let mode_byte = match parser.payload() {
                            Ok(p) if !p.is_empty() => p[0],
                            _ => mode_bit,
                        };
                        *peer.pending_mode_mask.entry(doc_id).or_insert(0) |= mode_byte;
                        transport.send_bytes(peer_id, doc_id, NTag::UpOpen, &[], Priority::High);
                        debug_log!("📂 Peer[{}] Doc[{}] Opened (offset mode, mode_byte=0b{:03b})", peer_id, doc_id, mode_byte);
                    }
                    return Ok(());
                }

                if op == 0x02 {
                    self.close_view(peer_id, doc_id, &transport)?;
                    return Ok(());
                }

                if let Some(sess) = peer.sessions.get_mut(&doc_id) {
                    match op {
                        0x03 | 0x08 => {
                            if let Ok((pos, content)) = parser.parse_insert_at() {
                                upstream_bytes = sess.handle_insert_at(pos, content, op == 0x03);
                            }
                        }
                        0x04 | 0x09 => {
                            if let Ok((from, to)) = parser.parse_delete_at() {
                                upstream_bytes = sess.handle_delete_at(from, to, op == 0x04);
                            }
                        }
                        0x05 => upstream_bytes = sess.handle_undo(&downstream),
                        0x06 => upstream_bytes = sess.handle_redo(&downstream),
                        _ => {}
                    }
                }
            } else {
                // ── Monaco 路径 (bit7=0) ──
                let tag = match parser.tag() {
                    Ok(t) => {
                        debug_log!("🔥 Worker Recv LTag: {:?}", t);
                        t
                    },
                    Err(_e) => {
                        debug_error!("Unknown LTag: {:?}", _e);
                        return Ok(());
                    }
                };

                // 3. 处理生命周期 (Open/Close)
                if tag == LTag::OpenDoc {
                    if let Ok(_text) = parser.parse_open_doc() {
                        debug_log!("📂 [JS->WASM] Peer[{}] Request Open Doc [{}]", peer_id, doc_id);
                        let mode_mask = match parser.payload() {
                            Ok(p) if !p.is_empty() => p[0],
                            _ => MODE_MONACO,
                        };
                        let already = peer.sessions.contains_key(&doc_id);
                        *peer.pending_mode_mask.entry(doc_id).or_insert(0) |= mode_mask;
                        transport.send_bytes(peer_id, doc_id, NTag::UpOpen, &[], Priority::High);
                        if already {
                            if let Some(sess) = peer.sessions.get(&doc_id) {
                                let m = sess.mode_mask | mode_mask;
                                Self::push_full_text(peer_id, doc_id, sess, m, &downstream)?;
                            }
                        }
                        debug_log!("📂 Peer[{}] Doc[{}] Opened (mode_mask=0b{:03b})", peer_id, doc_id, mode_mask);
                    }
                    return Ok(());
                }

                if tag == LTag::CloseDoc {
                    // 视图关闭（不是"整条连接不看了"）——退订判据收敛在 close_view 里。
                    self.close_view(peer_id, doc_id, &transport)?;
                    return Ok(());
                }

                // 4. 处理业务逻辑 (分发给 Session)

                // PROBE: forward to server without session requirement
                if tag == LTag::TelemetryProbe {
                    transport.send_bytes(peer_id, 0, NTag::TelemetryProbe, &[], Priority::Low);
                    // Don't return — let telemetry stamp at function bottom record WASM_local_proc
                }

                // EXEC: exec 请求透传 gateway（peer=0/doc=0 系统帧，payload 原样转发——
                // 引擎不解析 exec 语义，语义层在 gateway Python 侧）
                if tag == LTag::ExecReq {
                    if let Ok(p) = parser.payload() {
                        let p_vec = p.to_vec();
                        transport.send_bytes(peer_id, doc_id, NTag::UpExec, &p_vec, Priority::Low);
                    }
                    // Don't return — let telemetry stamp at function bottom record WASM_local_proc
                }

                if let Some(sess) = peer.sessions.get_mut(&doc_id) {
                    match tag {
                        LTag::LocalInsert => {
                            if let Ok((l, c, content)) = parser.parse_insert() {
                                upstream_bytes = sess.handle_insert(l, c, content, true);
                            } else {
                                debug_error!("[LocalInsert] parse_insert FAILED");
                            }
                        },
                        LTag::LocalInsertNoUndo => {
                            if let Ok((l, c, content)) = parser.parse_insert() {
                                upstream_bytes = sess.handle_insert(l, c, content, false);
                            }
                        },
                        LTag::LocalDelete => {
                            // 打点：这行能打出来 ⇒ 删除 op **确实到了引擎**（否则说明丢在读取/分派之前）
                            if let Ok((l1, c1, l2, c2)) = parser.parse_delete() {
                                upstream_bytes = sess.handle_delete(l1, c1, l2, c2, true);
                            } else {
                            }
                        },
                        LTag::LocalDeleteNoUndo => {
                            if let Ok((l1, c1, l2, c2)) = parser.parse_delete() {
                                upstream_bytes = sess.handle_delete(l1, c1, l2, c2, false);
                            }
                        },
                        LTag::ExplicitUndoRecord => {
                            if let Ok((l, c, content)) = parser.parse_explicit_undo() {
                                let _ = sess.handle_explicit_undo(l, c, content);
                            }
                        },
                        LTag::ImeBegin => {
                            sess.handle_ime_begin(0, 0);
                        },
                        LTag::ImeCommit => {
                            if let Ok(payload) = parser.payload() {
                                upstream_bytes = sess.handle_ime_commit(payload);
                            }
                        },
                        LTag::ImeCancel => {
                            sess.handle_ime_cancel();
                        },
                        LTag::UndoReq => {
                            if sess.ime.composing {
                                debug_log!("[Undo] Blocked during IME composition");
                            } else {
                                debug_log!("📥 [Rust] Received UndoReq for peer={} doc={}", peer_id, doc_id);
                                upstream_bytes = sess.handle_undo(&downstream);
                            }
                        },
                        LTag::RedoReq => {
                            if sess.ime.composing {
                                debug_log!("[Redo] Blocked during IME composition");
                            } else {
                                debug_log!("📥 [Rust] Received RedoReq for peer={} doc={}", peer_id, doc_id);
                                upstream_bytes = sess.handle_redo(&downstream);
                            }
                        },
                        LTag::CursorMove => {
                            if let Ok((_line, _col)) = parser.parse_cursor() {
                                debug_log!("📝 [Peer {} Doc {}] Cursor moved to {}:{}", peer_id, doc_id, _line, _col);
                            }
                        },
                        _ => {}
                    }
                } else {
                    debug_error!("Skipped op for non-existent Peer[{}] Doc[{}]", peer_id, doc_id);
                }
            }
        } // peer 借用释放

        // 分发 upstream bytes（按重连状态决定发送或缓冲）
        if !upstream_bytes.is_empty() {
            // 先在**纯 op 字节**上 decode（本地 fan-out 用；dispatch 会 move）
            let ops_result = bincode::serde::decode_from_slice::<Vec<WireOp>, _>(&upstream_bytes, bincode_config());

            // 上行也带血统号（与服务端下行帧同一约定 [lineage: u64 LE][bincode]）：
            // 服务端凭它认出「这笔编辑锚在一条已死血统上」，从而**不集成、改回推 checkpoint**
            // （store.rs UP_SYNC）——否则那笔编辑会在服务端 pending 里永久停车且毫无痕迹。
            // 缓冲进 pending_ops / sync_ops 的也是带号字节，重连重放原样发出：号是**出题时**
            // 的血统，正是这笔 op 该有的归属。
            let lineage = self.peers.get(&peer_id)
                .and_then(|p| p.sessions.get(&doc_id))
                .map(|s| s.lineage)
                .unwrap_or(0);
            let _has_sess = self.peers.get(&peer_id)
                .map(|p| p.sessions.contains_key(&doc_id))
                .unwrap_or(false);
            let stamped = Self::stamp_lineage(lineage, &upstream_bytes);

            // 发给 server
            self.reconnect.dispatch(doc_id, peer_id, stamped, &self.transport);

            // 本地 fan-out：绕过 server 回声，零延迟同步同 doc 的其他 peer session
            if let Ok((ops, _)) = ops_result {
                let downstream = self.downstream.clone();
                for (&pid, peer) in &mut self.peers {
                    if pid == peer_id { continue; }
                    if let Some(sess) = peer.sessions.get_mut(&doc_id) {
                        for op in &ops {
                            let _ = sess.apply_net_msg(op, &downstream);
                        }
                    }
                }
            }
        }

        // ── telemetry stamp ──
        let dur_ns = ((get_performance().now() - t_in) * 1_000_000.0) as u64;
        let tag_clean = tag_byte & 0x7F;
        let terminated = matches!(tag_clean, 0x07 | 0x0B | 0x0D);
        WASM_TELEMETRY.with(|t| t.borrow_mut().push(WasmTelemetrySample {
            layer: "WASM_local_proc".into(), dur_ns,
            ts_ms: get_performance().now(),
            tag: tag_clean, doc_id: 0, terminated,
        }));

        Ok(())
    }

    fn process_network_msg(&mut self, sender_peer_id: u32, doc_id: u32, tag: NTag, payload: Vec<u8>) -> Result<(), JsValue> {
        let t_in = get_performance().now();
        match tag {
            NTag::DnTree => {
                let p = MessageBuilder::new()
                    .header(LTag::RemoteOp, 0, 0) // PeerID=0, DocID=0 for System
                    .payload(&payload).build();
                self.downstream.write_chunk(&p)?;

                if self.reconnect.state == State::Syncing {
                    self.start_doc_sync(&payload)?;
                }
            },
            NTag::DnOpen => {
                // checkpoint 载荷 = (text, EncodedReplica, lineage)。lineage 决定这份副本
                // 属于哪条血统；服务端每个下行 CRDT 帧都带同一个号，不等就先换代（见 DnSync）。
                type SnapData<'a> = (String, cola::EncodedReplica<'a>, u64);

                let decode_res: Result<(SnapData, usize), _> = bincode::serde::decode_from_slice(&payload, bincode_config());
                match decode_res {
                    Ok(((text, encoded_state, lineage), _bytes_read)) => {
                        // 步骤 1: 绑定请求者。【精确】优先——服务端已在帧头回填 UpOpen 的
                        // adapter peer（store.rs UP_OPEN），所以"这条 DnOpen 是谁要的"由帧
                        // 自己说明，不再依赖 HashMap 遍历序。遍历只作为旧网关/未回填路径
                        // （如 iroh 中继）的兜底，命中时打点，便于发现还在走兜底的调用面。
                        let (target_peer_id, pending) = {
                            let mut tid = None;
                            let mut p = 0u8;
                            if let Some(peer) = self.peers.get_mut(&sender_peer_id) {
                                if let Some(m) = peer.pending_mode_mask.remove(&doc_id) {
                                    tid = Some(sender_peer_id);
                                    p = m;
                                }
                            }
                            let mut fell_back = false;
                            if tid.is_none() {
                                for (&pid, peer) in self.peers.iter_mut() {
                                    if let Some(m) = peer.pending_mode_mask.remove(&doc_id) {
                                        tid = Some(pid);
                                        p = m;
                                        fell_back = true;
                                        break;
                                    }
                                }
                            }
                            if fell_back {
                                debug_log!("⚠️ [DnOpen] 帧头 peer={} 无该 doc 的 pending ⇒ 退回遍历命中 peer={:?}（应为旧网关）", sender_peer_id, tid);
                            }
                            (tid, p)
                        };

                        // 步骤 1b: 目标集合。
                        //   · 请求式（UpOpen → DnOpen）：只有一个目标，就是帧头那个 peer；
                        //   · **服务端主动推送**的换代通知没有对应的 UpOpen ⇒ 没有 pending，
                        //     此时要对"本连接**所有**持有该文档的视图"逐个重取——Monaco 与
                        //     AST 是两个 adapter，只挑第一个会漏掉真正持有副本的那个。
                        let targets: Vec<(u32, u8)> = match target_peer_id {
                            Some(t) => vec![(t, pending)],
                            None => {
                                let holders: Vec<(u32, u8)> = self.peers.iter()
                                    .filter(|(_, p)| p.sessions.contains_key(&doc_id) || p.active_mode_mask.contains_key(&doc_id))
                                    .map(|(&pid, p)| (pid, p.active_mode_mask.get(&doc_id).copied().unwrap_or(MODE_MONACO)))
                                    .collect();
                                if holders.is_empty() {
                                    debug_log!("[DnOpen] 无 pending 且本连接未持有 doc={} ⇒ 忽略（不是给我们的）", doc_id);
                                    return Ok(());
                                }
                                debug_log!("🔄 [DnOpen] 服务端推送换代 doc={} ⇒ {} 个本地视图整篇重取", doc_id, holders.len());
                                holders
                            }
                        };
                        // 换代重取已到达：解除"已请求换代"标记（下次血统再变才再请求）。
                        self.reconnect.relineage_pending.remove(&doc_id);
                        let downstream = self.downstream.clone();

                        // 步骤 2【血统单源】：副本**一律**从服务端当前 checkpoint 派生。
                        //
                        // 旧实现此处会「借用任一本地会话的 replica.fork()」。那不是继承，
                        // 是**复活一段服务端已经作废的旧血统**：本地会话可能是上一代
                        // （服务端 __reset__ / reload 之前）留下的幽灵，fork 出来的新副本
                        // 与服务端当前血统的 base run 身份不同 ⇒ 之后服务端每个 op 都
                        // integrate None 并永久静默（判据 ⑦：fork幽灵=None / 派生=Some，
                        // 见 mdio_core/tests/cola_probe.rs；活体证据见
                        // research/2026-09-21-lineage-single-source-model.md §10）。
                        //
                        // 本地未上行的编辑不会因此丢失：它们要么已随 UP_SYNC 到达服务端
                        // （checkpoint 因此已含它们），要么还在 ReconnectState.pending_ops
                        // 里、并在 UpOpen **之前**重发（lib.rs start_doc_sync）。
                        let vdoc = self.reconnect.real_to_virtual.get(&doc_id).copied().unwrap_or(doc_id);
                        for (target_peer_id, pending) in targets {
                            let peer = self.get_or_create_peer(target_peer_id);

                            let prev = peer.active_mode_mask.get(&doc_id).copied().unwrap_or(0);
                            let mode_mask = if pending == 0 { prev | MODE_MONACO } else { prev | pending };

                            // §5.3：mask 登记必须**在"跳过重取"的早退之前** ✗ ——
                            // 否则重开视图（会话仍在 ⇒ 走跳过分支）后 AST 面收不到帧。
                            peer.active_mode_mask.insert(doc_id, mode_mask);
                            // 换代幂等：血统号没变 ⇒ 这条通知对此视图无事可做（同一代重推是常态）。
                            if let Some(existing) = peer.sessions.get(&doc_id) {
                                if existing.lineage == lineage {
                                    debug_log!("[DnOpen] peer={} doc={} lineage={} 未变 ⇒ 跳过重取", target_peer_id, doc_id, lineage);
                                    continue;
                                }
                                debug_log!("🔄 [DnOpen] peer={} doc={} 血统 {} ⇒ {}，整篇重取",
                                    target_peer_id, doc_id, existing.lineage, lineage);
                            }


                            if mode_mask & (MODE_MONACO | MODE_OFFSET) != 0 {
                                // A1：会话 doc_id 用**虚拟 id**（下行回填用），map 键仍是 real id（服务端对账用）
                                let mut sess = DocumentSession::from_snapshot(vdoc, target_peer_id, text.clone(), encoded_state.clone(), lineage);
                                sess.mode_mask = mode_mask;
                                peer.sessions.insert(doc_id, sess);

                                // 重推动全文 = **权威替换**（Type=5），不是"在 1:1 插一段"（Type=1）：
                                // 换代重取是幂等覆盖语义，用插入表达会让客户端靠长度启发式去猜，
                                // 短文档直接翻倍（见 payload_remote_replace_all 的注释）。
                                if mode_mask & MODE_MONACO != 0 {
                                    let p = MessageBuilder::new()
                                        .header(LTag::RemoteOp, target_peer_id, vdoc)
                                        .payload_remote_replace_all(&text)
                                        .build();
                                    downstream.write_chunk(&p)?;
                                }
                                if mode_mask & MODE_OFFSET != 0 {
                                    let p = MessageBuilder::new()
                                        .header(LTag::RemoteOp, target_peer_id, vdoc)
                                        .mode_offset()
                                        .payload_remote_replace_all_at(&text)
                                        .build();
                                    downstream.write_chunk(&p)?;
                                }
                            }

                            debug_log!("✅ Peer[{}] Doc[{}] loaded from snapshot (mode_mask=0b{:03b})", target_peer_id, doc_id, mode_mask);
                        }

                        // SYNCING: 更新 local_hash + 检查 sync 完成
                        if self.reconnect.state == State::Syncing {
                            if let Some(hash) = self.reconnect.server_hashes.get(&doc_id) {
                                self.reconnect.local_hashes.insert(doc_id, hash.clone());
                            }
                            self.reconnect.sync_waiting.remove(&doc_id);
                            if self.reconnect.sync_waiting.is_empty() {
                                self.finish_syncing()?;
                            }
                        }
                    },
                    Err(e) => {
                        debug_error!("❌ Snapshot Decode Error: {:?}", e);
                    }
                }
            },
            NTag::DnSync => {
                // SYNCING: 缓冲远端编辑，不立即应用
                if self.reconnect.state == State::Syncing {
                    self.reconnect.sync_ops.push((doc_id, payload));
                    return Ok(());
                }
                // 载荷 = [lineage: u64 LE][bincode(Vec<WireOp>)]（store.rs stamp_lineage /
                // lib.rs write_locator 都按这个形状出帧）。血统号是**先决条件检查**：
                // 本地副本若不是这条血统的后代，这些 op 集成进去只会进 pending 永久停车
                // （判据 ③⑤b⑦），所以先换代重取，而不是"试一下再说"。
                if payload.len() < 8 {
                    debug_error!("❌ [DnSync] 载荷过短、缺血统号 len={} doc={}", payload.len(), doc_id);
                    return Ok(());
                }
                let frame_lineage = u64::from_le_bytes(payload[0..8].try_into().unwrap());
                let body = &payload[8..];
                // **绝不静默**：一条下行 CRDT 帧要么被应用、要么有一条说明它为什么没被应用的日志。
                // （模型禁止"永久静默"——修复前的实现里，这里解码失败会一声不响地丢掉整帧，
                //   表现恰好是「Monaco 冻住、render 照常更新」，与血统断链同一张脸。）
                let decoded = match bincode::serde::decode_from_slice::<Vec<WireOp>, _>(body, bincode_config()) {
                    Ok((ops, used)) => {
                        if used != body.len() {
                            debug_error!("⚠️ [DnSync] op 流有 {} 字节尾余（len={} used={}）doc={}",
                                body.len() - used, body.len(), used, doc_id);
                        }
                        Some(ops)
                    }
                    Err(e) => {
                        let head: String = payload.iter().take(16).map(|b| format!("{:02x} ", b)).collect();
                        debug_error!("❌ [DnSync] op 解码失败 doc={} len={} lineage={} head={} err={:?}",
                            doc_id, payload.len(), frame_lineage, head, e);
                        None
                    }
                };
                if let Some(ops) = decoded {
                    // 无条件计数（不是 debug-only）：它是"这条 op 有没有人接住"的判据，
                    // 生产构建也要能报出来。
                    let mut _applied = 0u32;
                    let downstream = self.downstream.clone();
                    let mut stale: Vec<(u32, u8)> = Vec::new();
                    // ── 打点（无条件）：T1「Monaco 不更新」的三个静默出口都在这一段 ──
                    for (_, peer) in self.peers.iter_mut() {
                        if sender_peer_id == peer.peer_id {
                            debug_log!("👻 Peer[{}] dropped own echo for doc[{}]", peer.peer_id, doc_id);
                            continue;
                        }
                        let has_sess = peer.sessions.contains_key(&doc_id);
                        let (lin, mode) = peer.sessions.get(&doc_id).map(|s| (s.lineage, s.mode_mask)).unwrap_or((0, 0));
                        if let Some(sess) = peer.sessions.get_mut(&doc_id) {
                            if sess.lineage != frame_lineage {
                                debug_error!("❌ [DnSync] 血统不等：帧={} 本地={} peer={} doc={} ⇒ 请求换代（不集成）",
                                    frame_lineage, sess.lineage, peer.peer_id, doc_id);
                                stale.push((peer.peer_id, sess.mode_mask));
                                continue;
                            }
                            for op in &ops {
                                sess.apply_net_msg(op, &downstream)?;
                            }
                            _applied += 1;
                        }
                    }
                    if !stale.is_empty() {
                        self.request_relineage(doc_id, &stale)?;
                    }
                    // 「有帧但无人接收」必须响：订阅用了过期 docId / 会话已关而订阅还在，
                    // 都会表现成「文档从此不再更新」且毫无日志。见 CrdtClient 的同款计数。
                    if _applied == 0 && stale.is_empty() {
                        debug_error!("❌ [DnSync] doc={} 收到 {} 个 op，但本连接没有任何会话接住它（sender={}）",
                            doc_id, ops.len(), sender_peer_id);
                    }
                    debug_log!("🚀 [Client Perf] Sync Batch: sender={} doc={}, peers={} lineage={}", sender_peer_id, doc_id, _applied, frame_lineage);
                }
            },
            NTag::DnAst => {
                if payload.is_empty() { return Ok(()); }
                let ast_type = payload[0]; // 'F' = 全量, 'P' = 增量 patch
                let ast_json = &payload[1..];
                let sub_tag = if ast_type == b'F' { AST_FULL } else { AST_PATCH };
                let downstream = self.downstream.clone();
                // 步骤 4（第一段）：帧**不再按视图各发一份** —— 每篇文档只发一帧，
                // 帧头 peer 取该文档第一个要 AST 的视图。客户端按**子标签家族**路由
                // （AST 帧投给所有要 AST 的 sink），一份足够且不会双应用
                // （Monaco 的 sink 不消费 AST 子标签，AST 视图不消费 CRDT 子标签）。
                let mut sent_doc = false;
                {
                    let masks: Vec<(u32, String)> = self.peers.iter().map(|(pid, p)| (*pid, format!("{:?}", p.active_mode_mask.get(&doc_id)))).collect();
                }
                for (_, peer) in self.peers.iter() {
                    if let Some(mask) = peer.active_mode_mask.get(&doc_id) {
                        if mask & MODE_AST == 0 { continue; }
                        if sent_doc { continue; }
                        sent_doc = true;
                        let vdoc = self.reconnect.real_to_virtual.get(&doc_id).copied().unwrap_or(doc_id);
                        let p = MessageBuilder::new()
                            .header(LTag::RemoteOp, peer.peer_id, vdoc)
                            .payload_ast_bytes(sub_tag, ast_json)
                            .build();
                        downstream.write_chunk(&p)?;
                    }
                }
            },
            NTag::TelemetryReturn => {
                let p = MessageBuilder::new()
                    .header(LTag::TelemetryReturn, sender_peer_id, 0)
                    .payload(&payload).build();
                self.downstream.write_chunk(&p)?;
            },
            // exec 响应：gateway DnExec → LPPB ExecRes 送回主线程（peer=0/doc=0 系统帧，
            // payload 为 gateway 构造的 JSON {exec_id, raw}——主线程按 exec_id 关联请求）
            NTag::DnExec => {
                let p = MessageBuilder::new()
                    .header(LTag::ExecRes, sender_peer_id, doc_id)
                    .payload(&payload).build();
                self.downstream.write_chunk(&p)?;
            },
            // 服务端说"这篇文档不存在"（DN_ERR，载荷是原因：doc_gone / open_failed）。
            // 它是 rm / reload / 陈旧 doc_id 的**唯一**回话；没有它，客户端只会一直等那帧
            // 永远不来的 DnOpen（零帧），或者把编辑发进一个已经不存在的文档里（本地已应用、
            // 服务端默默丢掉 = 用户以为存上了）。
            NTag::DnErr => {
                let reason = String::from_utf8_lossy(&payload).to_string();
                debug_error!("❌ [DnErr] doc={} reason={} ⇒ 服务端没有这篇文档，收掉本地会话", doc_id, reason);
                let mut gone: Vec<u32> = Vec::new();
                for (&pid, p) in self.peers.iter_mut() {
                    let had_session = p.sessions.remove(&doc_id).is_some();
                    let had_mask = p.active_mode_mask.remove(&doc_id).is_some();
                    // pending 也要算：**"刚发出 UpOpen 就被回绝"正是最常见的那种**
                    // （打开一篇已删除的文档），此时还没有会话、只有 pending——漏掉它
                    // 宿主就只能干等 4s 的"零帧"看门狗，而手里其实已经有确切理由了。
                    let had_pending = p.pending_mode_mask.remove(&doc_id).is_some();
                    if had_session || had_mask || had_pending { gone.push(pid); }
                }
                // 一个都没引用它：也要报。用帧头那个 peer（服务端回填的就是请求者）。
                if gone.is_empty() { gone.push(sender_peer_id); }
                self.reconnect.sync_waiting.remove(&doc_id);
                self.reconnect.relineage_pending.remove(&doc_id);
                // B2（防御性）：已判死刑的 doc 的缓冲 op 一并清掉 —— 否则 finish_syncing 会把它再发一遍（再被拒、刷日志）。
                // 注：DnErr×Syncing 的竞态**未被实测触发过**，此行为逻辑推演的正确收尾，代价极小，保留。
                self.reconnect.sync_ops.retain(|(d, _)| *d != doc_id);
                // 报给 JS 侧（LTag::Error = 0xFF，帧头 peer 保留原视图 id）：宿主负责可见。
                let downstream = self.downstream.clone();
                let vdoc = self.reconnect.real_to_virtual.get(&doc_id).copied().unwrap_or(doc_id);
                for pid in gone {
                    let p = MessageBuilder::new()
                        .header(LTag::Error, pid, vdoc)
                        .payload(&payload).build();
                    downstream.write_chunk(&p)?;
                }
            },
            _ => {}
        }
        // ── telemetry stamp ──
        let dur_ns = ((get_performance().now() - t_in) * 1_000_000.0) as u64;
        WASM_TELEMETRY.with(|t| t.borrow_mut().push(WasmTelemetrySample {
            layer: "WASM_net_proc".into(), dur_ns,
            ts_ms: get_performance().now(),
            tag: 0, doc_id, terminated: false,
        }));
        Ok(())
    }

    // ================================================================
    // Reconnection helpers
    // ================================================================

    /// SYNCING Step 1-2: DnTree 到达后，对已打开文档发 pending_ops + UpOpen。
    /// SYNCING Step 1-2: DnTree 到达后，对已打开文档发 pending_ops + UpOpen。
    fn start_doc_sync(&mut self, tree_payload: &[u8]) -> Result<(), JsValue> {
        // 解析 DnTree payload → server_hashes
        let nodes: Vec<FileNodeWithHash> =
            serde_json::from_slice(tree_payload).unwrap_or_default();

        self.reconnect.server_hashes.clear();
        self.reconnect.sync_waiting.clear();
        // A1：名字账本（id → path）每棵 DnTree 都记；旧账本先快照，再用它把上一代的
        // 旧 id 翻译成 path、在新树里按 path 找新 id。只迁不丢（丢弃由 DnErr 兜底）⇒
        // 后到的全量树不含本 wt 文档时自然无迁移可做，无需「权威树窗口」判定。
        let prev_paths = self.reconnect.known_paths.clone();
        self.reconnect.known_paths.clear();
        let mut new_by_path: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for node in &nodes {
            self.reconnect.server_hashes.insert(node.id, node.hash.clone());
            self.reconnect.known_paths.insert(node.id, node.path.clone());
            new_by_path.insert(node.path.clone(), node.id);
        }

        // ── 先按服务端树对账：本地会话的 doc_id 必须**在树里**才值得同步 ──
        //
        // 服务端重启后 arena 会重新分配 doc_id；本地还挂着上一代的 id。给这些 id 发
        // UpOpen 永远等不到 DnOpen，于是 sync_waiting 永远清不空 ⇒ 客户端卡在 Syncing，
        // 而 Syncing 期间**所有**下行 CRDT 帧都进 sync_ops 缓冲、一个都不应用——
        // 表现恰好是「Monaco 冻住、render 照常更新」（2026-09-21 实测可确定性复现：
        // 开着内存态文档 → 重启网关 → 重开另一篇 → 点按钮 ⇒ doc 变、Monaco 不动）。
        // 判据：DnTree 已经给出了权威的 (path, doc_id, hash) 表，用它筛。
        // A1：按「名字」对账 —— 只判 sessions 会漏掉 AST-only 的僵尸 mask，且旧实现把
        // 「服务端重启换代」当丢弃处理 ⇒ 永久静默。现在：path 没变 ⇒ 迁到新 id；path 没了 ⇒ 才丢。
        // 只迁不丢：path 还在且 id 变了 ⇒ 迁；其余（含 path 不在本树）都不动。
        // 「文档真没了」由服务端 B1 门拒绝 UpOpen 回 DnErr 兜底（引擎收会话 + 宿主可见），
        // 不做本地静默丢弃（实测：重连时第二棵全量树不含本 wt 文档 ⇒ 静默丢弃误杀活会话）。
        let mut doc_pairs: Vec<(u32, u32)> = Vec::new();
        for (&pid, peer) in &self.peers {
            for &doc_id in peer.sessions.keys().chain(peer.active_mode_mask.keys()) {
                doc_pairs.push((pid, doc_id));
            }
        }
        let to_migrate: Vec<(u32, u32, u32)> = if self.reconnect.server_hashes.is_empty() {
            Vec::new()
        } else {
            plan_remaps(&prev_paths, &new_by_path, &doc_pairs)
        };
            for (pid, old_id, new_id) in &to_migrate {
                debug_log!("🔁 [Sync] 服务端换代：peer={} doc={}→{}（按 path 迁）", pid, old_id, new_id);
                if let Some(p) = self.peers.get_mut(pid) {
                    if let Some(mut sess) = p.sessions.remove(old_id) {
                        // sess.doc_id 保持**旧值**（= JS 的虚拟 id）—— 下行帧用它回填，JS 无需感知换代 ✓
                        p.sessions.insert(*new_id, sess);
                    }
                    if let Some(m) = p.active_mode_mask.remove(old_id) { p.active_mode_mask.insert(*new_id, m); }
                    if let Some(m) = p.pending_mode_mask.remove(old_id) { p.pending_mode_mask.insert(*new_id, m); }
                }
                if let Some(ops) = self.reconnect.pending_ops.remove(old_id) {
                    self.reconnect.pending_ops.insert(*new_id, ops);
                }
                {
                    let v = self.reconnect.real_to_virtual.get(old_id).copied().unwrap_or(*old_id);
                    self.reconnect.virtual_to_real.insert(v, *new_id);
                    self.reconnect.real_to_virtual.insert(*new_id, v);
                    debug_log!("🔁 [Sync] 映射登记 v={} → r={}", v, new_id);
                }
                self.reconnect.local_hashes.remove(old_id);
                for (d, _) in self.reconnect.sync_ops.iter_mut() { if *d == *old_id { *d = *new_id; } }
            }

        // A2：收集已打开文档 = **sessions ∪ active_mode_mask**（订阅状态才是重放载体）。
        // AST-only 文档没有 session，但登记在 active_mode_mask —— 旧实现漏掉它 ⇒ P 补丁永久停。
        let docs_to_sync: Vec<(u32, u32, u8)> = {
            let mut result = Vec::new();
            for (&pid, peer) in &self.peers {
                let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
                for (&doc_id, sess) in &peer.sessions {
                    seen.insert(doc_id);
                    result.push((pid, doc_id, sess.mode_mask));
                }
                for (&doc_id, &mask) in &peer.active_mode_mask {
                    if !seen.contains(&doc_id) {
                        result.push((pid, doc_id, mask));
                    }
                }
            }
            result
        };

        debug_log!("🔄 start_doc_sync: {} docs to sync", docs_to_sync.len());

        for (peer_id, doc_id, mode_mask) in &docs_to_sync {
            let server_hash = self.reconnect.server_hashes.get(doc_id).cloned().unwrap_or_default();
            let local_hash = self.reconnect.local_hashes.get(doc_id).cloned();
            let has_pending = self.reconnect.pending_ops.get(doc_id)
                .map_or(false, |v| !v.is_empty());
            let need_checkpoint = has_pending || local_hash.as_deref() != Some(&server_hash);

            // Step 2: 先发 pending_ops（WS 有序，server 先 apply 再响应 UpOpen）
            if need_checkpoint {
                if let Some(ops) = self.reconnect.pending_ops.get(doc_id) {
                    for op_bytes in ops.iter() {
                        self.transport.send_bytes(*peer_id, *doc_id, NTag::UpSync, op_bytes, Priority::High);
                    }
                }
            }

            // 始终发 UpOpen（建立 channel 订阅 + 触发 DnOpen checkpoint）
            self.transport.send_bytes(*peer_id, *doc_id, NTag::UpOpen, &[], Priority::High);

            // 修复 Bug 3: 设 pending_mode_mask，让 DnOpen 能找到 peer
            if let Some(peer) = self.peers.get_mut(peer_id) {
                *peer.pending_mode_mask.entry(*doc_id).or_insert(0) |= mode_mask;
            }

            self.reconnect.sync_waiting.insert(*doc_id);
        }

        // 没有需要同步的文档 → 直接完成
        if self.reconnect.sync_waiting.is_empty() {
            self.finish_syncing()?;
        }

        Ok(())
    }

    /// SYNCING Step 4: 重放 sync_ops（先发 server，再 apply 本地）。
    fn finish_syncing(&mut self) -> Result<(), JsValue> {
        debug_log!("🔄 finish_syncing — replaying sync_ops");

        for _round in 0..3 {
            let ops = std::mem::take(&mut self.reconnect.sync_ops);
            if ops.is_empty() { break; }

            for (doc_id, wire_bytes) in ops {
                // B2（防御性）：会话已被 DnErr 收掉的 doc 不再重发 —— 再发只会再被拒、刷日志。
                // 注：与 sync_ops.retain 同一竞态、同样未被实测触发，保留为收尾防线。
                let still_wanted = self.peers.values().any(|p| {
                    p.sessions.contains_key(&doc_id) || p.active_mode_mask.contains_key(&doc_id)
                });
                if !still_wanted { continue; }
                // 1. 先发给 server（修复 Bug 2: 不发则 op 永久丢失）
                self.transport.send_bytes(0, doc_id, NTag::UpSync, &wire_bytes, Priority::High);

                // 2. 应用到本地新 session（cola integrate 幂等，server 广播回声不会重复）
                //    wire_bytes 带血统前缀（见 process_local_msg），先解前缀再 decode。
                let (_, body) = Self::split_lineage(&wire_bytes);
                if let Ok((wire_ops, _)) = bincode::serde::decode_from_slice::<Vec<WireOp>, _>(
                    body, bincode_config(),
                ) {
                    let downstream = self.downstream.clone();
                    for peer in self.peers.values_mut() {
                        if let Some(sess) = peer.sessions.get_mut(&doc_id) {
                            for op in &wire_ops {
                                sess.apply_net_msg(op, &downstream)?;
                            }
                        }
                    }
                }
            }
        }

        // pending_ops 已随 UpOpen 发出，server checkpoint 已包含
        self.reconnect.pending_ops.clear();

        self.reconnect.set_state(State::Connected);
        debug_log!("✅ SYNC complete → CONNECTED");
        Ok(())
    }

    #[wasm_bindgen]
    pub fn is_connected(&self) -> bool {
        self.transport.is_connection_valid()
    }

    #[wasm_bindgen]
    pub fn get_transport_type(&self) -> String {
        if self.is_connected() {
            "WebSocket".to_string()
        } else {
            "Disconnected".to_string()
        }
    }
}

/// 独立导出（不接收 &self）：run_loop 是 &mut self 的 async，进行中调用任何
/// EditorCore 方法都会被 wasm-bindgen 判为 recursive use。读全局采集，JS 侧直接调。
#[wasm_bindgen]
pub fn editor_telemetry_data() -> String {
    let mut json = String::from("{\"WASM\":{");
    let mut layers: std::collections::HashMap<String, Vec<f64>> = std::collections::HashMap::new();
    // 读全局采集（take 清空 —— 消费性读取）
    let samples = WASM_TELEMETRY.with(|t| std::mem::take(&mut *t.borrow_mut()));
    for s in &samples {
        if s.terminated { continue; }
        layers.entry(s.layer.clone()).or_default().push(s.dur_ns as f64);
        layers.entry(format!("{}_ts", s.layer)).or_default().push(s.ts_ms);
    }
    for (i, (layer, vals)) in layers.iter().enumerate() {
        if i > 0 { json.push(','); }
        json.push('"');
        json.push_str(layer);
        json.push_str("\":[");
        for (j, v) in vals.iter().enumerate() {
            if j > 0 { json.push(','); }
            json.push_str(&format!("{}", v));
        }
        json.push(']');
    }
    json.push_str("}}");
    json
}

// ============================================================================
// 测试
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// Create a DocumentSession for native testing.
    /// Uses mode_mask=0 to skip downstream SAB writes (which need WASM).
    fn make_test_session() -> DocumentSession {
        let text = "hello world\n";
        let rope = Rope::from(text);
        let replica = Replica::new(1, rope.byte_len());
        let encoded = replica.encode();
        let mut sess = DocumentSession::from_snapshot(1, 1, text.to_string(), encoded, 1u64);
        sess.mode_mask = 0; // no downstream writes in native tests
        sess
    }

    // ── UndoRecord + push_undo + stack limit ──

    #[test]
    fn push_undo_adds_record() {
        let mut sess = make_test_session();
        let anchor = sess.replica.create_anchor(0, AnchorBias::Left);
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor,
            content: "hi".into(), cursor_before: None, cursor_after: None,
        });
        assert_eq!(sess.undo_stack.len(), 1);
        assert_eq!(sess.undo_stack[0].content, "hi");
    }

    #[test]
    fn stack_limit_evicts_oldest() {
        let mut sess = make_test_session();
        for i in 0..(MAX_UNDO_DEPTH + 10) {
            let anchor = sess.replica.create_anchor(0, AnchorBias::Left);
            sess.push_undo(UndoRecord {
                op_type: OpType::Insert, anchor,
                content: format!("op{}", i), cursor_before: None, cursor_after: None,
            });
        }
        assert_eq!(sess.undo_stack.len(), MAX_UNDO_DEPTH);
        assert_eq!(sess.undo_stack[0].content, "op10");
    }

    #[test]
    fn new_insert_clears_redo_stack() {
        let mut sess = make_test_session();
        let a = sess.replica.create_anchor(0, AnchorBias::Left);
        sess.redo_stack.push_back(UndoRecord {
            op_type: OpType::Delete, anchor: a,
            content: "stale".into(), cursor_before: None, cursor_after: None,
        });
        sess.redo_stack.clear(); // simulates handle_insert's redo_stack.clear()
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor: a,
            content: "fresh".into(), cursor_before: None, cursor_after: None,
        });
        assert_eq!(sess.redo_stack.len(), 0);
        assert_eq!(sess.undo_stack.len(), 1);
    }

    // ── Anchor lifecycle ──

    #[test]
    fn anchor_at_zero_resolves() {
        let sess = make_test_session();
        let anchor = sess.replica.create_anchor(0, AnchorBias::Left);
        assert_eq!(sess.replica.resolve_anchor(anchor), Some(0));
    }

    #[test]
    fn anchor_at_end_resolves() {
        let sess = make_test_session();
        let len = sess.replica.len();
        let anchor = sess.replica.create_anchor(len, AnchorBias::Right);
        assert_eq!(sess.replica.resolve_anchor(anchor), Some(len));
    }

    #[test]
    fn foreign_anchor_from_unintegrated_remote_returns_none() {
        // Replica A has text. Replica B forks from A.
        // A inserts new text and creates anchor in it.
        // B hasn't integrated A's insertion yet → anchor returns None.
        let mut replica_a = Replica::new(1, 12);
        let mut replica_b = replica_a.fork(2);
        let insertion = replica_a.inserted(3, 5);

        // Anchor in the NEW text (temporal position > 12)
        let anchor_in_new_text = replica_a.create_anchor(5, AnchorBias::Left);

        // Replica B hasn't integrated the insertion yet → None
        assert_eq!(replica_b.resolve_anchor(anchor_in_new_text), None);

        // After B integrates the insertion, anchor resolves
        replica_b.integrate_insertion(&insertion);
        assert!(replica_b.resolve_anchor(anchor_in_new_text).is_some());
    }

    // ── Undo/Redo: stack manipulation ──

    #[test]
    fn undo_pop_and_push_redo_cycle() {
        let mut sess = make_test_session();
        // Simulate an insert with undo tracking
        let anchor = sess.replica.create_anchor(0, AnchorBias::Left);
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor,
            content: "test".into(), cursor_before: None, cursor_after: None,
        });
        assert_eq!(sess.undo_stack.len(), 1);

        // Pop from undo, push to redo (simulating undo execution)
        let record = sess.undo_stack.pop_back().unwrap();
        sess.redo_stack.push_back(record);
        assert_eq!(sess.undo_stack.len(), 0);
        assert_eq!(sess.redo_stack.len(), 1);

        // Pop from redo, push to undo (simulating redo execution)
        let record = sess.redo_stack.pop_back().unwrap();
        sess.push_undo(record);
        assert_eq!(sess.undo_stack.len(), 1);
        assert_eq!(sess.redo_stack.len(), 0);
    }

    #[test]
    fn undo_record_preserves_content_and_type() {
        let mut sess = make_test_session();
        let anchor = sess.replica.create_anchor(0, AnchorBias::Left);
        sess.push_undo(UndoRecord {
            op_type: OpType::Delete, anchor,
            content: "deleted text".into(), cursor_before: None, cursor_after: None,
        });
        let record = sess.undo_stack.pop_back().unwrap();
        assert_eq!(record.op_type, OpType::Delete);
        assert_eq!(record.content, "deleted text");
    }

    #[test]
    fn new_edit_clears_redo_stack_via_push_undo() {
        let mut sess = make_test_session();
        let anchor = sess.replica.create_anchor(0, AnchorBias::Left);
        // Push to undo then move to redo (simulates undo)
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor,
            content: "first".into(), cursor_before: None, cursor_after: None,
        });
        let record = sess.undo_stack.pop_back().unwrap();
        sess.redo_stack.push_back(record);
        assert_eq!(sess.redo_stack.len(), 1);

        // New edit: clear redo, push undo
        sess.redo_stack.clear();
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor,
            content: "second".into(), cursor_before: None, cursor_after: None,
        });
        assert_eq!(sess.redo_stack.len(), 0);
        assert_eq!(sess.undo_stack.len(), 1);
    }

    // ── IME state machine ──

    #[test]
    fn ime_begin_sets_composing_flag() {
        let mut sess = make_test_session();
        assert!(!sess.ime.composing);
        sess.handle_ime_begin(0, 0);
        assert!(sess.ime.composing);
    }

    #[test]
    fn ime_insert_is_skipped_while_composing() {
        let mut sess = make_test_session();
        let rope_before = sess.rope.to_string();
        let undo_before = sess.undo_stack.len();

        sess.handle_ime_begin(0, 0);
        // This insert should be silently dropped
        let _ = sess.handle_insert(0, 1, "字".into(), false);

        assert_eq!(sess.rope.to_string(), rope_before, "rope unchanged during IME");
        assert_eq!(sess.undo_stack.len(), undo_before, "no undo record during IME");
    }

    #[test]
    fn ime_delete_is_skipped_while_composing() {
        let mut sess = make_test_session();
        let rope_before = sess.rope.to_string();

        sess.handle_ime_begin(0, 0);
        let _ = sess.handle_delete(0, 1, 0, 3, false);

        assert_eq!(sess.rope.to_string(), rope_before, "rope unchanged during IME");
    }

    /// Parse an ImeCommit payload and apply it atomically (without Transport, for testing).
    fn simulate_ime_commit(sess: &mut DocumentSession, line: u32, col: u32, text: &str) {
        assert!(sess.ime.composing, "must be composing before commit");
        let offset = sess.monaco_to_byte(line as usize, col as usize);
        let anchor = sess.replica.create_anchor(offset, AnchorBias::Right);
        sess.rope.insert(offset, text);
        let _op = sess.replica.inserted(offset, text.len());
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor,
            content: text.into(), cursor_before: None, cursor_after: None,
        });
        sess.redo_stack.clear();
        sess.ime.composing = false;
    }

    #[test]
    fn ime_commit_inserts_final_text_atomically() {
        let mut sess = make_test_session();
        sess.ime.composing = true;
        simulate_ime_commit(&mut sess, 0, 1, "你好世界");

        assert!(!sess.ime.composing, "composing flag cleared");
        assert!(sess.rope.to_string().contains("你好世界"), "final text in rope");
        assert_eq!(sess.undo_stack.len(), 1, "one undo record");
        assert_eq!(sess.undo_stack[0].content, "你好世界");
    }

    #[test]
    fn consecutive_ime_sessions_each_produce_undo_record() {
        let mut sess = make_test_session();
        sess.ime.composing = true;
        simulate_ime_commit(&mut sess, 0, 1, "你好");
        assert_eq!(sess.undo_stack.len(), 1);

        // Second IME: commit after first text (col=7 = after "你好" + 1)
        sess.handle_ime_begin(0, 0);
        sess.ime.composing = true;
        simulate_ime_commit(&mut sess, 0, 7, "世界");
        assert_eq!(sess.undo_stack.len(), 2);
    }

    #[test]
    fn empty_ime_commit_is_no_op() {
        let mut sess = make_test_session();
        sess.ime.composing = true;
        // Empty text should not produce undo record
        sess.ime.composing = false; // manual, since no-op skips rope
        assert_eq!(sess.undo_stack.len(), 0);
    }

    #[test]
    fn composing_blocks_undo() {
        let mut sess = make_test_session();
        sess.ime.composing = true;
        let can_undo = !sess.ime.composing;
        assert!(!can_undo, "undo blocked during composition");
    }

    #[test]
    fn composed_text_can_be_undone() {
        let mut sess = make_test_session();
        let initial = sess.rope.to_string();
        sess.ime.composing = true;
        simulate_ime_commit(&mut sess, 0, 1, "XYZ");

        assert!(sess.rope.to_string().contains("XYZ"));
        assert_eq!(sess.undo_stack.len(), 1);

        // Undo: pop and reverse the insert
        let record = sess.undo_stack.pop_back().unwrap();
        let pos = sess.replica.resolve_anchor(record.anchor).unwrap();
        let start = pos.saturating_sub(record.content.len());
        sess.rope.delete(start..pos);
        let _op = sess.replica.deleted(start..pos);
        sess.redo_stack.push_back(record);

        assert_eq!(sess.rope.to_string(), initial, "undo restored original text");
    }

    #[test]
    fn cancelled_ime_does_not_change_rope() {
        let mut sess = make_test_session();
        let initial = sess.rope.to_string();
        sess.handle_ime_begin(0, 0);
        assert!(sess.ime.composing);
        // User cancels composition (Esc) — just clear the flag
        sess.ime.composing = false;
        assert_eq!(sess.rope.to_string(), initial, "cancelled IME = no change");
        assert_eq!(sess.undo_stack.len(), 0);
    }

    /// Sequential undo works in production (anchor resolution after CRDT ops
    /// is complex — tested via integration). Here we verify stack discipline.
    #[test]
    fn undo_stack_lifo_order_is_maintained() {
        let mut sess = make_test_session();
        let a = sess.replica.create_anchor(0, AnchorBias::Right);
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor: a,
            content: "first".into(), cursor_before: None, cursor_after: None,
        });
        let b = sess.replica.create_anchor(1, AnchorBias::Right);
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor: b,
            content: "second".into(), cursor_before: None, cursor_after: None,
        });
        let c = sess.replica.create_anchor(2, AnchorBias::Right);
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor: c,
            content: "third".into(), cursor_before: None, cursor_after: None,
        });

        // LIFO: last pushed = first popped
        assert_eq!(sess.undo_stack.pop_back().unwrap().content, "third");
        assert_eq!(sess.undo_stack.pop_back().unwrap().content, "second");
        assert_eq!(sess.undo_stack.pop_back().unwrap().content, "first");
        assert!(sess.undo_stack.is_empty());
    }

    #[test]
    fn redo_after_multiple_undos_restores_all() {
        let mut sess = make_test_session();
        let anchor = sess.replica.create_anchor(0, AnchorBias::Right);
        sess.rope.insert(0, "X");
        let _op = sess.replica.inserted(0, 1);
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor,
            content: "X".into(), cursor_before: None, cursor_after: None,
        });

        // Undo
        let r = sess.undo_stack.pop_back().unwrap();
        let p = sess.replica.resolve_anchor(r.anchor).unwrap();
        sess.rope.delete(p.saturating_sub(r.content.len())..p);
        sess.redo_stack.push_back(r);

        // Redo
        let r = sess.redo_stack.pop_back().unwrap();
        let p = sess.replica.resolve_anchor(r.anchor).unwrap();
        let redo_pos = p.saturating_sub(r.content.len());
        sess.rope.insert(redo_pos, &r.content);
        sess.push_undo(r);

        assert!(sess.rope.to_string().contains("X"));
        assert_eq!(sess.redo_stack.len(), 0);
        assert_eq!(sess.undo_stack.len(), 1);
    }

    #[test]
    fn undo_blocked_when_ime_composing() {
        let mut sess = make_test_session();
        sess.ime.composing = true;
        // The process_local_msg guard: if sess.ime.composing { skip undo }
        let can_undo = !sess.ime.composing;
        assert!(!can_undo);
    }

    // ── Coordinate conversion (ASCII: byte == UTF-16 code unit) ──

    #[test]
    fn monaco_to_byte_first_line() {
        let mut sess = make_test_session();
        // Monaco adapter sends 0-based lines: line=0 is first line
        // col=3 means the 4th char ('l' in "hello")
        let byte = sess.monaco_to_byte(0, 3);
        assert_eq!(byte, 3);
    }

    #[test]
    fn monaco_to_byte_second_line_returns_end() {
        let mut sess = make_test_session();
        // line=1 means second line (past "hello world\n" which is 1 line in crop)
        // Should return rope.byte_len() (= end of document)
        let byte = sess.monaco_to_byte(1, 0);
        assert_eq!(byte, sess.rope.byte_len()); // = 12 (end of "hello world\n")
    }

    #[test]
    fn byte_to_monaco_returns_1_based() {
        let mut sess = make_test_session();
        let (line, col) = sess.byte_to_monaco(3);
        assert_eq!(line, 1); // 1-based line
        assert_eq!(col, 4);  // 1-based col (4th character)
    }

    // ── UndoRecord no redundant len field ──

    #[test]
    fn undo_record_derives_len_from_content() {
        let mut sess = make_test_session();
        sess.rope.insert(0, "abc");
        let _op = sess.replica.inserted(0, 3);
        let anchor = sess.replica.create_anchor(0, AnchorBias::Right);
        sess.push_undo(UndoRecord {
            op_type: OpType::Insert, anchor,
            content: "abc".into(), cursor_before: None, cursor_after: None,
        });

        let record = sess.undo_stack.pop_back().unwrap();
        // No .len field — use .content.len()
        assert_eq!(record.content.len(), 3);
    }

    // ── ReconnectionState ──

    use std::cell::RefCell;
    use std::rc::Rc;

    fn make_reconnect() -> (ReconnectionState, Rc<RefCell<Vec<u8>>>) {
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut state = ReconnectionState::new();
        let r = received.clone();
        state.set_on_state_change(Box::new(move |s| r.borrow_mut().push(s)));
        (state, received)
    }

    #[test]
    fn set_state_triggers_callback() {
        let (mut state, received) = make_reconnect();
        state.set_state(State::Disconnected);
        assert_eq!(*received.borrow(), vec![State::Disconnected as u8]);
    }

    #[test]
    fn set_state_same_value_no_callback() {
        let (mut state, received) = make_reconnect();
        state.set_state(State::Connected); // same as initial
        assert!(received.borrow().is_empty());
    }

    #[test]
    fn dispatch_connected_dead_transitions_to_disconnected() {
        let mut state = ReconnectionState::new();
        let transport = Transport::new(); // not connected
        state.dispatch(42, 1, vec![0xAB], &transport);
        assert_eq!(state.state, State::Disconnected);
        assert_eq!(state.pending_ops.get(&42).unwrap().len(), 1);
    }

    #[test]
    fn dispatch_disconnected_buffers_pending() {
        let mut state = ReconnectionState::new();
        state.set_state(State::Disconnected);
        let transport = Transport::new();
        state.dispatch(42, 1, vec![0xAB], &transport);
        assert_eq!(state.pending_ops.get(&42).unwrap().len(), 1);
        assert!(state.sync_ops.is_empty());
    }

    #[test]
    fn dispatch_syncing_buffers_sync_ops() {
        let mut state = ReconnectionState::new();
        state.set_state(State::Syncing);
        let transport = Transport::new();
        state.dispatch(42, 1, vec![0xAB], &transport);
        assert_eq!(state.sync_ops.len(), 1);
        assert_eq!(state.sync_ops[0], (42, vec![0xAB]));
        assert!(!state.pending_ops.contains_key(&42));
    }

    #[test]
    fn on_disconnect_from_connected() {
        let mut state = ReconnectionState::new();
        state.on_disconnect();
        assert_eq!(state.state, State::Disconnected);
        assert_eq!(state.retry_count, 0);
    }

    #[test]
    fn on_disconnect_from_idle_is_noop() {
        let mut state = ReconnectionState::new();
        state.set_state(State::Idle);
        state.on_disconnect();
        assert_eq!(state.state, State::Idle);
    }

    #[test]
    fn plan_remaps_moves_by_path_and_never_drops() {
        let mut prev: HashMap<u32, String> = HashMap::new();
        prev.insert(109u32, "/stats.md".to_string());
        let mut nbp: HashMap<String, u32> = HashMap::new();
        nbp.insert("/stats.md".to_string(), 110u32);
        let docs = vec![(7u32, 109u32)];
        // 换代：旧 109 → 新 110
        assert_eq!(plan_remaps(&prev, &nbp, &docs), vec![(7, 109, 110)]);
        // path 不在新树 ⇒ 不迁不丢（DnErr 兜底）
        let empty: HashMap<String, u32> = HashMap::new();
        assert!(plan_remaps(&prev, &empty, &docs).is_empty());
        // id 稳定 ⇒ 不动
        let mut stable: HashMap<String, u32> = HashMap::new();
        stable.insert("/stats.md".to_string(), 109u32);
        assert!(plan_remaps(&prev, &stable, &docs).is_empty());
        // 同一 doc 挂两个 peer ⇒ 只决策一次（去重，避免双迁）
        let docs2 = vec![(7u32, 109u32), (8u32, 109u32)];
        assert_eq!(plan_remaps(&prev, &nbp, &docs2), vec![(7, 109, 110)]);
        // 无 path 记录（首次同步）⇒ 不动
        let docs3 = vec![(7u32, 999u32)];
        assert!(plan_remaps(&prev, &nbp, &docs3).is_empty());
    }

    #[test]
    fn backoff_sequence() {
        let mut state = ReconnectionState::new();
        assert_eq!(state.backoff_ms(), 1000);
        state.retry_count = 1;
        assert_eq!(state.backoff_ms(), 2000);
        state.retry_count = 2;
        assert_eq!(state.backoff_ms(), 4000);
        state.retry_count = 3;
        assert_eq!(state.backoff_ms(), 8000);
        state.retry_count = 4;
        assert_eq!(state.backoff_ms(), 16000);
        state.retry_count = 10;
        assert_eq!(state.backoff_ms(), 16000); // capped
    }

    #[test]
    fn full_reconnect_cycle_callbacks() {
        let (mut state, received) = make_reconnect();
        state.set_state(State::Disconnected);
        state.set_state(State::Reconnecting);
        state.set_state(State::Syncing);
        state.set_state(State::Connected);
        assert_eq!(
            *received.borrow(),
            vec![
                State::Disconnected as u8,
                State::Reconnecting as u8,
                State::Syncing as u8,
                State::Connected as u8,
            ]
        );
    }

    // ── cola decode vs fork: can decode'd Replica integrate foreign op? ──

    #[test]
    fn decode_replica_can_integrate_foreign_op() {
        let text_len = 13;
        let server = Replica::new(9999 as ReplicaId, text_len);
        let encoded = server.encode();

        let mut client_a = Replica::decode(1111 as ReplicaId, &encoded).unwrap();
        let mut client_b = Replica::decode(2222 as ReplicaId, &encoded).unwrap();

        let insertion = client_a.inserted(13, 1);
        let result = client_b.integrate_insertion(&insertion);

        assert!(result.is_some(), "decode'd Replica must integrate foreign ReplicaId's op");
    }

    #[test]
    fn fork_replica_can_integrate_foreign_op() {
        let mut client_a = Replica::new(1111 as ReplicaId, 13);
        let mut client_b = client_a.fork(2222 as ReplicaId);

        let insertion = client_a.inserted(13, 1);
        let result = client_b.integrate_insertion(&insertion);

        assert!(result.is_some(), "fork'd Replica must integrate foreign ReplicaId's op");
    }

    /// Full roundtrip: server encode → bincode checkpoint → client decode → integrate
    #[test]
    fn checkpoint_roundtrip_preserves_integrate() {
        // Simulate server checkpoint
        let text = "1\n11\n11\n11\n11";
        let server_rep = Replica::new(9999 as ReplicaId, text.len());
        let dump = (text.to_string(), server_rep.encode(), 42u64);
        let checkpoint = bincode::serde::encode_to_vec(&dump, bincode_config()).unwrap();

        // Simulate client A decode from checkpoint
        let (decoded, _): ((String, cola::EncodedReplica, u64), usize) =
            bincode::serde::decode_from_slice(&checkpoint, bincode_config()).unwrap();
        assert_eq!(decoded.2, 42, "checkpoint 必须携带血统号（客户端据此判断是否要换代）");
        let mut client_a = Replica::decode(1111 as ReplicaId, &decoded.1).unwrap();

        // Simulate client B decode from same checkpoint
        let (decoded2, _): ((String, cola::EncodedReplica, u64), usize) =
            bincode::serde::decode_from_slice(&checkpoint, bincode_config()).unwrap();
        let mut client_b = Replica::decode(2222 as ReplicaId, &decoded2.1).unwrap();

        // A inserts, B integrates
        let insertion = client_a.inserted(text.len(), 1);
        let result = client_b.integrate_insertion(&insertion);
        assert!(result.is_some(), "checkpoint roundtrip must preserve integrate capability");
    }

    /// 上行载荷前缀往返：客户端 stamp 出去的号，必须能被同一套解前缀读回来。
    /// 这条判据把"上行也带血统号"钉在类型层——它是服务端辨认"这笔编辑属于已死血统"
    /// 的唯一依据（store.rs UP_SYNC），漏了就会退回到"服务端静默停车"。
    #[test]
    fn upstream_lineage_prefix_roundtrip() {
        let ops = bincode::serde::encode_to_vec(&Vec::<WireOp>::new(), bincode_config()).unwrap();
        let stamped = EditorCore::stamp_lineage(0xABCD_1234, &ops);
        let (l, body) = EditorCore::split_lineage(&stamped);
        assert_eq!(l, 0xABCD_1234, "带号载荷必须解出同一个号");
        assert_eq!(body, &ops[..], "解出的 op 字节必须与原样一致");

        let (l2, body2) = EditorCore::split_lineage(&ops);
        assert_eq!(l2, 0, "无前缀 ⇒ 号取 0（服务端同样容忍）");
        assert_eq!(body2, &ops[..]);
    }

    /// 血统号不是装饰：换了血统就是**另一条 lineage**，旧副本再也集成不进来。
    /// 这条判据钉住 DnSync 的"血统不等 ⇒ 先换代，不集成"前置检查的必要性
    /// （对应 mdio_core/tests/cola_probe.rs ③⑤b；活体见 research §10）。
    #[test]
    fn lineage_change_makes_old_replica_unable_to_integrate() {
        let text = "abcdef";
        // 老血统 L1：服务端首次建档，客户端从它的 checkpoint 派生
        let mut server_l1 = Replica::new(9999 as ReplicaId, text.len());
        let ckpt_l1 = bincode::serde::encode_to_vec(
            (text.to_string(), server_l1.encode(), 1u64), bincode_config()).unwrap();
        let (d1, _): ((String, cola::EncodedReplica, u64), usize) =
            bincode::serde::decode_from_slice(&ckpt_l1, bincode_config()).unwrap();
        assert_eq!(d1.2, 1);
        let mut client = Replica::decode(1111 as ReplicaId, &d1.1).unwrap();

        // 同血统内：服务端的 op 客户端能集成（对照）
        let same = server_l1.inserted(3, 1);
        assert!(client.integrate_insertion(&same).is_some(), "同血统必须能集成");

        // 换代：服务端以新 base run 重建 ⇒ 新血统 L2，血统号必须不同
        let mut server_l2 = Replica::new(8888 as ReplicaId, text.len());
        let l2_lineage = 2u64;
        assert_ne!(l2_lineage, d1.2, "换代后血统号必须变化，否则客户端误判『没变』");
        let foreign = server_l2.inserted(3, 1);
        assert!(client.integrate_insertion(&foreign).is_none(),
            "换了血统的 op 必须集成不了——所以 DnSync 必须先查血统号再决定集成，而不是试了才知道");
    }
}
