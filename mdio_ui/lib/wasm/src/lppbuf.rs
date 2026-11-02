use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use wasm_bindgen::prelude::*;
use js_sys::{SharedArrayBuffer, Uint8Array, Int32Array, Atomics, Function};
use wasm_bindgen_futures::JsFuture;
use crate::lpp_protocol::HEADER_SIZE;

// 协议常量
const HEADER_SIZE_BYTES: u32 = 128; // 128字节头部
const WRITE_PTR_IDX: u32 = 0;       // Bytes 0-3
const READ_PTR_IDX: u32 = 16;       // Bytes 64-67 (Padding后)
const TAG_SWITCH: u8 = 0xFE;
const TAG_NOOP: u8 = 0xFF;

// 1. JS 胶水层
// 避免在 Rust 里用 Reflect 解析 {async: true, value: Promise}
// 直接让 JS 返回 Promise 或者 null，零 GC 压力。
#[wasm_bindgen(inline_js = "
export function smart_wait(stateview, index, expected) {
    // 1. Double Check: 在进入昂贵的 Promise 构造前，先看一眼是不是已经变了
    // stateview 是 Int32Array
    const current = Atomics.load(stateview, index);
    if (current !== expected) {
        return null; // 既然已经变了，直接返回 null 让 Rust 继续读取
    }

    // 2. 只有确实没变，才挂起等待
    const res = Atomics.waitAsync(stateview, index, expected);

    if (res.async) {
        return res.value; // 返回 Promise
    }

    // res.async 为 false 说明 wait 期间值变了(value='not-equal') 或 超时(value='timed-out')
    // 无论哪种，都意味着我们可以回 Rust 再试一次了
    return null;
}
")]
extern "C" {
    fn smart_wait(state: &Int32Array, index: u32, expected: i32) -> JsValue;
}

#[derive(Clone)]
struct BufferSlot {
    sab: SharedArrayBuffer, // 保持引用防止被回收
    state: Int32Array,
    data: Uint8Array,
    capacity: usize,
}

impl BufferSlot {
    fn new(sab: SharedArrayBuffer) -> Self {
        let capacity = sab.byte_length() as usize;
        Self {
            sab: sab.clone(),
            state: Int32Array::new(&sab).subarray(0, 32),
            data: Uint8Array::new(&sab),
            capacity,
        }
    }

    /// Reader 的核心职责：读完后重置 Buffer，让 Writer 可以再次使用
    fn reset(&self) {
        let _ = Atomics::store(&self.state, WRITE_PTR_IDX, HEADER_SIZE_BYTES as i32);
        let _ = Atomics::store(&self.state, READ_PTR_IDX, HEADER_SIZE_BYTES as i32);
    }

    fn write_ptr(&self) -> u32 {
        Atomics::load(&self.state, WRITE_PTR_IDX).unwrap_or(HEADER_SIZE_BYTES as i32) as u32
    }

    fn read_ptr(&self) -> u32 {
        Atomics::load(&self.state, READ_PTR_IDX).unwrap_or(HEADER_SIZE_BYTES as i32) as u32
    }
}

#[derive(Clone)]
pub struct LinearBufferReader {
    slots: Arc<[RwLock<BufferSlot>; 2]>,
    active_idx: Arc<AtomicUsize>,
}

impl LinearBufferReader {
    pub fn new(sab0: SharedArrayBuffer, sab1: SharedArrayBuffer) -> Self {
        let s0 = BufferSlot::new(sab0);
        let s1 = BufferSlot::new(sab1);
        s0.reset();
        s1.reset();
        {
             let w = s0.write_ptr();
             #[cfg(feature = "debug")]
             web_sys::console::log_1(&format!("🦀 [Rust Init] Slot0 WPtr: {}", w).into());
        }

        Self {
            // 初始化固定长度数组的 RwLock
            slots: Arc::new([RwLock::new(s0), RwLock::new(s1)]),
            active_idx: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// 动态扩容支持 + "踢醒旧 Buffer" 防止死锁
    pub fn update_buffer(&self, idx: usize, new_sab: SharedArrayBuffer) {
        if idx < 2 {
            // 只获取当前需要修改的 Slot 的写锁，不影响另一个 Slot 工作
            if let Ok(mut slot_guard) = self.slots[idx].write() {
                // 1. 保存旧 State
                let old_state = slot_guard.state.clone();
                // 2. 替换
                *slot_guard = BufferSlot::new(new_sab);
                // 3. 踢醒
                let _ = Atomics::store(&old_state, WRITE_PTR_IDX, i32::MAX);
                // 唤醒全部等待者（count=∞），允许多个 reader 并行等待
                let _ = Atomics::notify_with_count(&old_state, WRITE_PTR_IDX, u32::MAX);
            }
        }
    }

    // 2: 统一的 recv() 方法
    // 将 wait 和 read 合并，对外提供唯一的异步获取数据接口
    // 返回: Ok(Payload), Err(JS Error)
    pub async fn recv(&self) -> Result<Vec<u8>, JsValue> {
        loop {
            let idx = self.active_idx.load(Ordering::Acquire);

            // 状态快照
            let state_view;
            let current_w_ptr;

            // 1. 同步检查 (Short Lock)
            {
                let slot = self.slots[idx].read().unwrap();
                let w_ptr = slot.write_ptr();
                let r_ptr = slot.read_ptr();
                #[cfg(feature = "debug")]
                web_sys::console::log_1(&format!("🦀 [Rust LPPB] Loop. Idx: {} W: {} R: {}", idx, w_ptr, r_ptr).into());

                let available = w_ptr.saturating_sub(r_ptr);

                if available > 0 {
                    let tag = slot.data.get_index(r_ptr);

                    // Case A: 切换 Buffer
                    if tag == TAG_SWITCH {
                        slot.reset(); // Reset当前Buffer
                        let new_idx = (idx + 1) % 2;
                        self.active_idx.store(new_idx, Ordering::Release);

                        // 释放锁，直接进入下一次循环去读新 Buffer (Continue loop)
                        continue;
                    }

                    // Case B: 正常数据
                    // 确保数据完整才读，避免半包
                    if available >= HEADER_SIZE as u32 {
                        // 解析长度
                        let l0 = slot.data.get_index(r_ptr + 1) as u32;
                        let l1 = slot.data.get_index(r_ptr + 2) as u32;
                        let l2 = slot.data.get_index(r_ptr + 3) as u32;
                        let l3 = slot.data.get_index(r_ptr + 4) as u32;
                        let body_len = l0 | (l1 << 8) | (l2 << 16) | (l3 << 24);

                        let total_len = HEADER_SIZE as u32 + body_len;

                        // 双重检查: Body 是否已完全到达?
                        if available >= total_len {
                            let mut data = vec![0u8; total_len as usize];
                            slot.data.subarray(r_ptr, r_ptr + total_len).copy_to(&mut data);

                            // 更新读指针 (Commit)
                            let _ = Atomics::store(&slot.state, READ_PTR_IDX, (r_ptr + total_len) as i32);

                            return Ok(data);
                        }
                    }

                    // Case C: NoOp
                    if tag == TAG_NOOP {
                        let _ = Atomics::store(&slot.state, READ_PTR_IDX, (r_ptr + 1) as i32);
                        #[cfg(feature = "debug")]
                        web_sys::console::log_1(&"🦀 [Rust] NOOP Detected. Lock Dropped. Sleeping...".into());
                        continue;
                    }
                }

                // 准备休眠
                state_view = slot.state.clone();
                current_w_ptr = w_ptr;
            } // 锁在这里释放。休眠时不持锁。

            // 2. 智能等待 (Smart Wait)
            let wait_result = smart_wait(&state_view, WRITE_PTR_IDX, current_w_ptr as i32);

            if !wait_result.is_null() {
                let _ = JsFuture::from(js_sys::Promise::from(wait_result)).await;
            }
        }
    }
}

// Writer 部分 (对应 Downstream: Rust -> JS)
#[derive(Clone)]
pub struct LinearBufferWriter {
    slots: Arc<[RwLock<BufferSlot>; 2]>,
    active_idx: Arc<AtomicUsize>,
    on_upgrade: Function,
}

impl LinearBufferWriter {
    pub fn new(sab0: SharedArrayBuffer, sab1: SharedArrayBuffer, on_upgrade: Function) -> Self {
        let s0 = BufferSlot::new(sab0);
        let s1 = BufferSlot::new(sab1);
        s0.reset();
        s1.reset();

        Self {
            slots: Arc::new([RwLock::new(s0), RwLock::new(s1)]),
            active_idx: Arc::new(AtomicUsize::new(0)),
            on_upgrade,
        }
    }

    /// 核心写入逻辑：支持动态扩容 + 锁降级优化
    pub fn write(&self, data: &[u8]) -> Result<(), JsValue> {
        let idx = self.active_idx.load(Ordering::Acquire);

        // 普通写入只需要 Read 锁，因为 BufferSlot 内部是 SharedArrayBuffer (Int32Array)
        // 它们本身是 Interior Mutability 的，通过 Atomics 支持并发读写
        {
            let slot_guard = self.slots[idx].read().unwrap();
            let cur = &*slot_guard; // 这里的 cur 是 BufferSlot

            let len = data.len();
            let needed = len + 1;
            let w_ptr = cur.write_ptr();

            // 1. 直写模式 (Happy Path) - 只需要 Read 锁！
            if w_ptr as usize + needed <= cur.capacity {
                cur.data.subarray(w_ptr, w_ptr + len as u32).copy_from(data);
                let _ = Atomics::store(&cur.state, WRITE_PTR_IDX, (w_ptr + len as u32) as i32);
                let _ = Atomics::notify_with_count(&cur.state, WRITE_PTR_IDX, u32::MAX);
                return Ok(());
            }

            // 2. 空间不足 (Need Switch)
            // 在 Read 锁下写入 SWITCH 是安全的 (Atomic writes to SAB)
            cur.data.set_index(w_ptr, TAG_SWITCH);
            let _ = Atomics::store(&cur.state, WRITE_PTR_IDX, (w_ptr + 1) as i32);
            // 唤醒全部等待者
            let _ = Atomics::notify_with_count(&cur.state, WRITE_PTR_IDX, u32::MAX);
        } // Read 锁在此释放

        // 开始切换逻辑
        let next_idx = (idx + 1) % 2;

        // 需要检查备用 Buffer，这里需要 Write 锁（可能要修改 next slot）
        let mut next_slot_guard = self.slots[next_idx].write().unwrap();

        let next_busy = next_slot_guard.write_ptr() != HEADER_SIZE_BYTES
            || next_slot_guard.read_ptr() != HEADER_SIZE_BYTES;

        if next_busy {
            return Err(JsValue::from_str("Backpressure: Buffer busy"));
        }

        let required_cap = (HEADER_SIZE_BYTES as usize) + data.len() + 2048;
        let current_cap = next_slot_guard.capacity;
        let default_cap = 8 * 1024 * 1024;

        if required_cap > current_cap {
            // 扩容
            let raw_size = required_cap * 2;
            let aligned_size = (raw_size + 3) & !3;
            let new_sab = SharedArrayBuffer::new(aligned_size as u32);
            *next_slot_guard = BufferSlot::new(new_sab.clone());
            next_slot_guard.reset();

            // 捕获 JS 调用可能的 panic/error
            if let Err(_e) = self.on_upgrade.call2(&JsValue::NULL, &JsValue::from(next_idx as u32), &new_sab) {
                #[cfg(feature = "debug")]
                web_sys::console::error_1(&"❌ JS Callback Failed: on_upgrade".into());
            }

        } else if current_cap > default_cap && required_cap < default_cap {
            // 缩容
            let new_sab = SharedArrayBuffer::new(default_cap as u32);
            *next_slot_guard = BufferSlot::new(new_sab.clone());
            next_slot_guard.reset();

            // 捕获 JS 调用可能的 panic/error
            if let Err(_e) = self.on_upgrade.call2(&JsValue::NULL, &JsValue::from(next_idx as u32), &new_sab) {
                #[cfg(feature = "debug")]
                web_sys::console::error_1(&"❌ JS Callback Failed: on_upgrade".into());
            }
        }

        // 写入新 Buffer
        self.active_idx.store(next_idx, Ordering::Release);

        // 此时我们持有 next_slot_guard (Write Lock), 写入是独占的，安全
        next_slot_guard.data.subarray(HEADER_SIZE_BYTES, HEADER_SIZE_BYTES + data.len() as u32).copy_from(data);
        let _ = Atomics::store(&next_slot_guard.state, WRITE_PTR_IDX, (HEADER_SIZE_BYTES + data.len() as u32) as i32);
        let _ = Atomics::notify_with_count(&next_slot_guard.state, WRITE_PTR_IDX, u32::MAX);

        Ok(())
    }

    /// 写入数据块（同步，用于兼容现有代码）
    pub fn write_chunk(&self, data: &[u8]) -> Result<(), JsValue> {
        self.write(data)
    }
}
