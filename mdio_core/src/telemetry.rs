//! MDIO Telemetry — sub-microsecond instrumentation framework.
//!
//! ## Architecture
//! - `#[thread_local]` statics + `RefCell` for ACTIVE_SESSION/SESSION_STACK.
//! - `TelemetryInner` uses `parking_lot::Mutex` internally — required because
//!   `Arc<TelemetryInner>` must be `Send+Sync` to cross `py.detach()` boundaries.
//!   At RUNTIME all access is single-threaded, so mutexes are uncontended (~50ns).
//! - `ProjectStore` stores session behind `parking_lot::Mutex`.
//! - `SpanGuard` RAII — Drop writes complete event. Noop when no session active.
//! - `ensure_session()` — idempotent entry point, fast path ~20ns, slow path ~50ns.
//! - `telemetry_span!` macro — expands to `{ () }` when feature is OFF.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use ahash::AHashMap as HashMap;
use smallvec::SmallVec;
use parking_lot::Mutex as PlMutex;


// Clock — raw CNTVCT_EL0 ticks on Apple Silicon (~0.7ns), else Instant fallback


/// 直接读 CNTVCT_EL0,不带 `nomem`(cputicks 的 cntvct 带 nomem —— LLVM
/// 视其为纯读,可把夹在小函数两侧的两次读重排/合并 → 差值恒 0,桶测量
/// 失效实证;去掉 nomem 后读按"可能访存"处理,与夹在其间的代码保持
/// 程序序,差值真实)。非 aarch64 回退 cputicks(频率换算同源)。
#[inline(always)]
fn ticks_now() -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        let cnt: u64;
        unsafe {
            std::arch::asm!(
                "mrs {}, cntvct_el0",
                out(reg) cnt,
                options(nostack, preserves_flags)
            );
        }
        cnt
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        cputicks::Ticks::now().0
    }
}
fn ticks_to_ns(ticks: u64) -> u64 { (ticks as f64 * 1_000_000_000.0 / cputicks::Ticks::frequency() as f64) as u64 }

/// 探查专用:当前 cputicks 换算 ns(与 span 自计时同源时钟——§10.5 纪律:
/// 导出时序一律 cputicks 换算,不用 Instant)。成本桶累加(fdc_pass2 细分
/// 等)用;feature OFF 时本模块整体不编译,调用点必须 cfg 摘除。
#[inline]
pub fn ticks_ns() -> u64 {
    ticks_to_ns(ticks_now())
}

/// 计数器 span 开关（`MDIO_TELE_DEEP=1`）——**每行/每节点**一级的 span 只在这种
/// 模式下开：它们会把被观测段本身抬高一个量级（大盘实测），默认关保持采集轻量。
/// 分层下钻纪律（docs/TELEMETRY.md §10.7）：先粗（函数级）后细（计数器级）。
pub fn deep() -> bool {
    static D: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *D.get_or_init(|| std::env::var_os("MDIO_TELE_DEEP").is_some())
}

/// 深度级别 span：`deep()` 为假时退化为 noop guard（零记录、不读钟）。
/// 用法：`let _g = crate::telemetry::deep_span("name");`
#[inline]
pub fn deep_span(name: &'static str) -> SpanGuard {
    if deep() { span(name) } else { SpanGuard::noop() }
}


// CRC32


#[inline]
fn hash_name(name: &str) -> u32 {
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(name.as_bytes());
    hasher.finalize()
}


// TelemetryEvent — 32-byte compact event record


#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct TelemetryEvent {
    pub name_hash: u32,
    pub parent_hash: u32,
    pub start_ns: u64,     // since session epoch
    pub dur_ns: u64,
    pub meta_offset: u32,
    pub meta_len: u16,
    _pad: [u8; 2],
}
// 4+4+8+8+4+2+2 = 32 bytes

const DEFAULT_EVENT_CAPACITY: usize = 4096;
const DEFAULT_META_POOL_CAPACITY: usize = 16 * 1024;
const MAX_META_POOL: usize = 256 * 1024;  // 硬上限 — 防 TLV 泄漏
const MAX_BENCH_SAMPLES_PER_PHASE: usize = 100_000;  // p99 精度足够, 防无界内存
const MAX_LAYER_SAMPLES: usize = 100_000;  // layer 样本上限 (RS_proc 每包一条, 长跑累积)


// TelemetryConfig — runtime-tunable capacities (.env / Python override)


/// Telemetry 容量配置。默认值与旧编译期常量一致；`from_env` 读取
/// `MDIO_TELEMETRY_*` 环境变量；Python 显式参数优先级最高。
#[derive(Debug, Clone, Copy)]
pub struct TelemetryConfig {
    pub max_events: usize,
    pub max_meta_pool: usize,
    pub max_bench_samples_per_phase: usize,
    pub max_layer_samples: usize,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            max_events: DEFAULT_EVENT_CAPACITY,
            max_meta_pool: MAX_META_POOL,
            max_bench_samples_per_phase: MAX_BENCH_SAMPLES_PER_PHASE,
            max_layer_samples: MAX_LAYER_SAMPLES,
        }
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}

impl TelemetryConfig {
    /// 从标准环境变量读取；缺省/非法值时回退编译期默认值。
    pub fn from_env() -> Self {
        Self {
            max_events: env_usize("MDIO_TELEMETRY_MAX_EVENTS", DEFAULT_EVENT_CAPACITY),
            max_meta_pool: env_usize("MDIO_TELEMETRY_MAX_META_POOL", MAX_META_POOL),
            max_bench_samples_per_phase: env_usize(
                "MDIO_TELEMETRY_BENCH_SAMPLES_PER_PHASE",
                MAX_BENCH_SAMPLES_PER_PHASE,
            ),
            max_layer_samples: env_usize("MDIO_TELEMETRY_MAX_LAYER_SAMPLES", MAX_LAYER_SAMPLES),
        }
    }
}


// TelemetryMode


#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TelemetryMode { Trace, Bench }


// PhaseAccumulator


#[derive(Debug, Clone)]
pub struct PhaseAccumulator {
    pub durations: HashMap<u32, Vec<u64>>,
    max_samples: usize,
}

impl PhaseAccumulator {
    pub fn new() -> Self {
        Self::new_with_limit(MAX_BENCH_SAMPLES_PER_PHASE)
    }

    pub fn new_with_limit(max_samples: usize) -> Self {
        Self {
            durations: HashMap::new(),
            max_samples: max_samples.max(1),
        }
    }

    pub fn record(&mut self, name_hash: u32, dur_ns: u64) {
        let entry = self.durations.entry(name_hash).or_default();
        if entry.len() < self.max_samples {
            entry.push(dur_ns);
        }
    }
}


// TelemetryInner — Mutex inside (Send+Sync), shared via Arc


pub struct TelemetryInner {
    pub name: String,
    pub mode: TelemetryMode,
    pub epoch: Instant,

    pub(crate) phase_names: PlMutex<HashMap<u32, String>>,
    pub(crate) meta_pool: PlMutex<Vec<u8>>,
    pub(crate) meta_offset: PlMutex<usize>,

    pub(crate) events: PlMutex<Vec<TelemetryEvent>>,
    pub(crate) events_dropped: AtomicU64,
    pub(crate) max_events: usize,
    pub(crate) max_meta_pool: usize,
    pub(crate) max_layer_samples: usize,

    pub(crate) accumulator: PlMutex<Option<PhaseAccumulator>>,
    pub(crate) layer_stats: PlMutex<Vec<LayerSample>>,
}

/// Per-layer sample recorded each time a frame passes through a node.
#[derive(Debug, Clone)]
pub struct LayerSample {
    pub layer_name: String,
    pub dur_ns: u64,
    pub bucket: SampleBucket,
    pub tag: u8,
    pub doc_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleBucket { Edit, Probe, Ctrl }

impl SampleBucket {
    pub(crate) fn from_tag(tag: u8) -> Self {
        match tag {
            0x30 | 0xB0 => Self::Probe,
            // Ctrl: OpenDoc, CloseDoc, UpTree, DnTree, DnOpen, DnErr
            0x01 | 0x02 | 0x03 | 0x81 | 0x82 | 0xFF => Self::Ctrl,
            _ => Self::Edit,
        }
    }
}

impl TelemetryInner {
    pub fn new(name: String, mode: TelemetryMode) -> Self {
        Self::new_with_config(name, mode, TelemetryConfig::default())
    }

    /// 从环境变量读取容量配置（`.env` / shell export 均可生效）。
    pub fn from_env(name: String, mode: TelemetryMode) -> Self {
        Self::new_with_config(name, mode, TelemetryConfig::from_env())
    }

    pub fn new_with_config(name: String, mode: TelemetryMode, cfg: TelemetryConfig) -> Self {
        Self {
            name,
            mode,
            epoch: Instant::now(),
            phase_names: PlMutex::new(HashMap::new()),
            meta_pool: PlMutex::new(Vec::with_capacity(DEFAULT_META_POOL_CAPACITY)),
            meta_offset: PlMutex::new(0),
            events: PlMutex::new(Vec::with_capacity(cfg.max_events.min(DEFAULT_EVENT_CAPACITY))),
            events_dropped: AtomicU64::new(0),
            max_events: cfg.max_events,
            max_meta_pool: cfg.max_meta_pool,
            max_layer_samples: cfg.max_layer_samples,
            accumulator: PlMutex::new(match mode {
                TelemetryMode::Bench => Some(PhaseAccumulator::new_with_limit(cfg.max_bench_samples_per_phase)),
                TelemetryMode::Trace => None,
            }),
            layer_stats: PlMutex::new(Vec::new()),
        }
    }

    pub fn register_phase(&self, name: &str) -> u32 {
        let hash = hash_name(name);
        self.phase_names.lock().entry(hash).or_insert_with(|| name.to_string());
        hash
    }

    /// 会话指针 id(线程本地栈归属判定用——同线程换会话时栈要重置)。
    #[inline]
    fn tid_mark(&self) -> usize {
        self as *const TelemetryInner as usize
    }

    pub fn push_span(&self, name_hash: u32) -> u32 {
        SPAN_STACK.with(|cell| {
            let mut stack = cell.borrow_mut();
            if stack.last().map(|(_, _, m)| *m) != Some(self.tid_mark()) {
                stack.clear();
            }
            let parent = stack.last().map(|(h, _, _)| *h).unwrap_or(0);
            stack.push((name_hash, ticks_now(), self.tid_mark()));
            parent
        })
    }

    pub fn pop_span(&self, name_hash: u32) {
        SPAN_STACK.with(|cell| {
            let mut stack = cell.borrow_mut();
            if stack.last().map(|(h, _, m)| (*h, *m)) == Some((name_hash, self.tid_mark())) {
                stack.pop();
            }
        })
    }

    /// Python 侧 span 入栈(与 Rust SpanGuard 同栈——线程本地,因果父链
    /// 不受其他线程并发 span 污染)。
    pub fn py_push_span(&self, name_hash: u32) -> u32 {
        SPAN_STACK.with(|cell| {
            let mut stack = cell.borrow_mut();
            if stack.last().map(|(_, _, m)| *m) != Some(self.tid_mark()) {
                stack.clear();
            }
            let parent = stack.last().map(|(h, _, _)| *h).unwrap_or(0);
            stack.push((name_hash, ticks_now(), self.tid_mark()));
            parent
        })
    }

    /// Python 侧 span 出栈:从栈尾找本会话同 hash 条目(协程交错形态的
    /// 兜底——移除到该位置,父 = 移除后栈顶)。返回 (start_ticks, parent)
    /// 或 None(未找到)。
    pub fn py_pop_span(&self, name_hash: u32) -> Option<(u64, u32)> {
        SPAN_STACK.with(|cell| {
            let mut stack = cell.borrow_mut();
            let pos = stack.iter().rposition(|(h, _, m)| *h == name_hash && *m == self.tid_mark())?;
            let (_, start, _) = stack.remove(pos);
            let parent = if stack.last().map(|(_, _, m)| *m) == Some(self.tid_mark()) {
                stack.last().map(|(h, _, _)| *h).unwrap_or(0)
            } else {
                0
            };
            Some((start, parent))
        })
    }

    /// TLV: u8:n_pairs, per-pair: u8:k_len, [k], u16_LE:v_len, [v]
    pub fn write_metadata(&self, meta: &[(String, String)]) -> (u32, u16) {
        if meta.is_empty() { return (0, 0); }
        let mut pool = self.meta_pool.lock();
        let mut off = self.meta_offset.lock();
        let start = *off;
        // 超预算时静默丢弃元数据 (事件本身仍记录) — 防无限增长
        let est: usize = 1 + meta.iter().map(|(k, v)| 1 + k.len() + 2 + v.len()).sum::<usize>();
        if pool.len().saturating_add(est) > self.max_meta_pool { return (0, 0); }
        pool.resize(start + 1, 0);
        let mut n: u8 = 0;
        for (k, v) in meta {
            let kb = k.as_bytes(); let vb = v.as_bytes();
            pool.push(kb.len() as u8);
            pool.extend_from_slice(kb);
            pool.extend_from_slice(&(vb.len() as u16).to_le_bytes());
            pool.extend_from_slice(vb);
            n += 1;
        }
        pool[start] = n;
        let len = (pool.len() - start) as u16;
        *off = pool.len();
        (start as u32, len)
    }

    pub fn record_event(
        &self, name_hash: u32, parent_hash: u32,
        dur_ns: u64, meta: &[(String, String)],
    ) {
        match self.mode {
            TelemetryMode::Trace => {
                let start_ns = self.epoch.elapsed().as_nanos() as u64 - dur_ns;
                let (meta_offset, meta_len) = self.write_metadata(meta);
                let event = TelemetryEvent {
                    name_hash, parent_hash, start_ns, dur_ns,
                    meta_offset, meta_len, _pad: [0; 2],
                };
                let mut events = self.events.lock();
                if events.len() < self.max_events {
                    events.push(event);
                } else {
                    self.events_dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            TelemetryMode::Bench => {
                if let Some(ref mut acc) = *self.accumulator.lock() {
                    acc.record(name_hash, dur_ns);
                }
                // 自定义报告：带 kv 的 span 额外保留 events 样本（计数/属性
                // 维度——普通 span 只进 acc，避免全量双写开销）。
                if !meta.is_empty() {
                    let start_ns = self.epoch.elapsed().as_nanos() as u64 - dur_ns;
                    let (meta_offset, meta_len) = self.write_metadata(meta);
                    let event = TelemetryEvent {
                        name_hash, parent_hash, start_ns, dur_ns,
                        meta_offset, meta_len, _pad: [0; 2],
                    };
                    let mut events = self.events.lock();
                    if events.len() < self.max_events {
                        events.push(event);
                    } else {
                        self.events_dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }

    pub fn add_layer_sample(&self, layer_name: &str, dur_ns: u64, bucket: SampleBucket, tag: u8, doc_id: u32) {
        let mut stats = self.layer_stats.lock();
        if stats.len() >= self.max_layer_samples { return; }  // 静默丢弃 (对齐 events cap 语义)
        stats.push(LayerSample {
            layer_name: layer_name.to_string(), dur_ns, bucket, tag, doc_id,
        });
    }

    pub fn clear(&self) {
        self.events.lock().clear();
        self.meta_pool.lock().clear();
        *self.meta_offset.lock() = 0;
        SPAN_STACK.with(|cell| cell.borrow_mut().clear());
        self.events_dropped.store(0, Ordering::Relaxed);
        if let Some(ref mut acc) = *self.accumulator.lock() { acc.durations.clear(); }
        self.layer_stats.lock().clear();
    }
}


// SpanGuard — RAII span


#[must_use]
pub struct SpanGuard {
    session: Option<Arc<TelemetryInner>>,
    name_hash: u32,
    parent_hash: u32,
    start_ticks: u64,
    meta: SmallVec<[(String, String); 4]>,
}

impl SpanGuard {
    pub fn start(session: Arc<TelemetryInner>, name: &str) -> Self {
        let name_hash = session.register_phase(name);
        let parent_hash = session.push_span(name_hash);
        Self {
            session: Some(session), name_hash, parent_hash,
            start_ticks: ticks_now(), meta: SmallVec::new(),
        }
    }

    pub fn noop() -> Self {
        Self {
            session: None, name_hash: 0, parent_hash: 0,
            start_ticks: 0, meta: SmallVec::new(),
        }
    }

    pub fn kv(mut self, key: impl Into<String>, value: impl ToString) -> Self {
        if self.session.is_some() { self.meta.push((key.into(), value.to_string())); }
        self
    }
}

impl Drop for SpanGuard {
    fn drop(&mut self) {
        let Some(ref sess) = self.session else { return };
        let dur_ns = ticks_to_ns(ticks_now() - self.start_ticks);
        sess.pop_span(self.name_hash);
        sess.record_event(self.name_hash, self.parent_hash, dur_ns, &self.meta);
    }
}


// SessionGuard


pub struct SessionGuard { prev: Option<Arc<TelemetryInner>>, active: bool }

impl SessionGuard {
    fn noop() -> Self { Self { prev: None, active: false } }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if self.active {
            ACTIVE_SESSION.with(|c| *c.borrow_mut() = self.prev.take());
        }
    }
}


// Thread-Local State (RefCell here is safe — true thread-local access)


thread_local! {
    static ACTIVE_SESSION: RefCell<Option<Arc<TelemetryInner>>> = RefCell::new(None);
    static SESSION_STACK: RefCell<Vec<Option<Arc<TelemetryInner>>>> = RefCell::new(Vec::new());
/// 因果 span 栈(线程本地——Rust SpanGuard 与 Python span 共用):
    /// 条目 = (name_hash, start_ticks, session_mark)。线程本地是硬约束:
    /// 共享栈会被并发 worker(FJ/Path)的 span 交叉污染(worker span 拿到
    /// 主线程栈顶当父;主线程后续父链漂移——实测证据 = 读窗口里出现
    /// 写侧 span 的因果错挂)。换会话(session_mark 不同)时栈重置。
    static SPAN_STACK: RefCell<Vec<(u32, u64, usize)>> = RefCell::new(Vec::new());
}


// Public API


pub fn current_session() -> Option<Arc<TelemetryInner>> {
    ACTIVE_SESSION.with(|cell| cell.borrow().clone())
}

pub fn set_session(sess: &Arc<TelemetryInner>) -> SessionGuard {
    let prev = ACTIVE_SESSION.with(|c| c.replace(Some(sess.clone())));
    SessionGuard { prev, active: true }
}

/// Idempotent: fast path (~20ns) if already set, slow (~50ns) if needs Store lock.
/// Returns noop if telemetry_active is false (runtime off-switch).
pub fn ensure_session(
    active: &std::sync::atomic::AtomicBool,
    store_session: &PlMutex<Option<Arc<TelemetryInner>>>,
) -> SessionGuard {
    if !active.load(std::sync::atomic::Ordering::Relaxed) {
        return SessionGuard::noop();
    }
    if ACTIVE_SESSION.with(|c| c.borrow().is_some()) {
        return SessionGuard::noop();
    }
    if let Some(ref sess) = *store_session.lock() {
        let prev = ACTIVE_SESSION.with(|c| c.replace(Some(sess.clone())));
        return SessionGuard { prev, active: true };
    }
    SessionGuard::noop()
}

/// Python `__enter__`: push prev session, set new, record span start.
pub fn py_span_enter(sess: &Arc<TelemetryInner>, name: &str) {
    let prev = ACTIVE_SESSION.with(|c| c.borrow().clone());
    SESSION_STACK.with(|s| s.borrow_mut().push(prev));
    ACTIVE_SESSION.with(|c| *c.borrow_mut() = Some(sess.clone()));
    let name_hash = sess.register_phase(name);
    sess.py_push_span(name_hash);
}

/// Python `__exit__`: pop span, dispatch to events (Trace) or accumulator (Bench), restore session.
/// meta 来自 Python span 的 **kwargs — 写入 TLV 池 (trace 模式, 对齐 SpanGuard::kv)。
pub fn py_span_exit(sess: &Arc<TelemetryInner>, name: &str, meta: Vec<(String, String)>) {
    let name_hash = hash_name(name);
    // 单 worker async 事件循环: 协程 A 在 span 内 await 时协程 B 可能 enter/exit span,
    // 栈顶不匹配 → 从栈中找该 span 移除到该位置兜底记录(时长 = enter→exit
    // 全跨度,语义正确)——不静默丢。栈为线程本地(并发 worker 的 span 不再
    // 交叉污染父链)。
    if let Some((start, parent_hash)) = sess.py_pop_span(name_hash) {
        let dur_ns = ticks_to_ns(ticks_now() - start);
        match sess.mode {
            TelemetryMode::Trace => {
                let start_ns = sess.epoch.elapsed().as_nanos() as u64 - dur_ns;
                let (meta_offset, meta_len) = sess.write_metadata(&meta);
                let mut events = sess.events.lock();
                if events.len() < sess.max_events {
                    events.push(TelemetryEvent {
                        name_hash, parent_hash, start_ns, dur_ns,
                        meta_offset, meta_len, _pad: [0; 2],
                    });
                } else {
                    sess.events_dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            TelemetryMode::Bench => {
                if let Some(ref mut acc) = *sess.accumulator.lock() {
                    acc.record(name_hash, dur_ns);
                }
            }
        }
    }
    let prev = SESSION_STACK.with(|s| s.borrow_mut().pop().flatten());
    ACTIVE_SESSION.with(|c| *c.borrow_mut() = prev);
}

pub fn span(name: &str) -> SpanGuard {
    ACTIVE_SESSION.with(|cell| match &*cell.borrow() {
        Some(ref sess) => SpanGuard::start(sess.clone(), name),
        None => SpanGuard::noop(),
    })
}


// Tests


#[cfg(test)]
mod tests {
    include!("../tests/unit/telemetry.rs");
}


// Stats & Utilities


// decode_meta retained for report_json_trace
fn decode_meta(bytes: &[u8]) -> Vec<(String, String)> {
    if bytes.is_empty() { return Vec::new(); }
    let n = bytes[0] as usize; let mut pairs = Vec::with_capacity(n); let mut pos = 1;
    for _ in 0..n {
        if pos >= bytes.len() { break; }
        let k_len = bytes[pos] as usize; pos += 1;
        let k = String::from_utf8_lossy(&bytes[pos..pos+k_len]).to_string(); pos += k_len;
        if pos + 2 > bytes.len() { break; }
        let v_len = u16::from_le_bytes([bytes[pos], bytes[pos+1]]) as usize; pos += 2;
        let v = String::from_utf8_lossy(&bytes[pos..pos+v_len]).to_string(); pos += v_len;
        pairs.push((k, v));
    }
    pairs
}


// ---- 内置文本报告：从 events 因果父链自动推导名字树 ----

/// 内置文本报告（树状阶段账）：**层级不从代码声明，而从 events 的因果
/// `parent_hash` 自动推导**——新加任何 span 都会自动出现在正确位置，
/// 不需要维护 HIER 常量。inclusive = 该名 Σdur；`(自身)` = inclusive − Σ(子名)。
/// 多父名（同一名字挂在多个父下）按**主导父**（Σdur 最大者）归属，会在
/// 脚注列出拆分——此时其父的 `(自身)` 是近似（可能高估）。
///
/// 注意：Bench 模式的 events 只保留带 kv 的 span（树只剩 stage 一层）；
/// 要完整树请用 **Trace 模式**并把 `MDIO_TELEMETRY_MAX_EVENTS` 抬到足够大
/// （事件截断时本函数会打印警告）。
pub fn report_text(sess: &TelemetryInner) -> String {
    use std::fmt::Write;
    let events = sess.events.lock();
    let phase_names = sess.phase_names.lock();
    if events.is_empty() {
        return String::new();
    }
    let name_of = |h: u32| -> String {
        phase_names.get(&h).cloned().unwrap_or_else(|| format!("{:08x}", h))
    };
    // 聚合：name → (Σms, cnt)；边 (父名, 子名) → (Σms, cnt)；根 = parent_hash==0
    let mut sums: HashMap<String, (f64, u64)> = HashMap::new();
    let mut edges: HashMap<(String, String), (f64, u64)> = HashMap::new();
    let mut roots: Vec<String> = Vec::new();
    for ev in events.iter() {
        let nm = name_of(ev.name_hash);
        let e = sums.entry(nm.clone()).or_insert((0.0, 0));
        e.0 += ev.dur_ns as f64 / 1e6;
        e.1 += 1;
        if ev.parent_hash == 0 {
            if !roots.contains(&nm) {
                roots.push(nm);
            }
        } else {
            let ee = edges
                .entry((name_of(ev.parent_hash), nm))
                .or_insert((0.0, 0));
            ee.0 += ev.dur_ns as f64 / 1e6;
            ee.1 += 1;
        }
    }
    // 每个子名的主导父 ＋ 多父名记录
    let mut bychild: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    for ((p, c), (ms, _)) in &edges {
        bychild.entry(c.clone()).or_default().push((p.clone(), *ms));
    }
    let mut dom: HashMap<String, String> = HashMap::new();
    let mut multi: Vec<(String, Vec<(String, f64)>)> = Vec::new();
    for (c, mut parents) in bychild {
        parents.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let main = parents[0].0.clone();
        if parents.len() > 1 {
            multi.push((c.clone(), parents));
        }
        dom.insert(c, main);
    }
    // 父 → 子（仅归属主导父）
    let mut byparent: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    for ((p, c), (ms, _)) in &edges {
        if dom.get(c) == Some(p) {
            byparent.entry(p.clone()).or_default().push((c.clone(), *ms));
        }
    }
    for v in byparent.values_mut() {
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }

    let mut out = String::new();
    let dropped = sess.events_dropped.load(Ordering::Relaxed);
    if dropped > 0 {
        let _ = writeln!(out, "⚠ 事件截断 {dropped} 条——树不完整；请调大 MDIO_TELEMETRY_MAX_EVENTS");
    }
    let _ = writeln!(out, "（inclusive＝含子项；子按主导父归属；多父名见脚注）");

    fn rec(
        out: &mut String,
        name: &str,
        sums: &HashMap<String, (f64, u64)>,
        byparent: &HashMap<String, Vec<(String, f64)>>,
        denom: f64,
        depth: usize,
        depth_cap: usize,
    ) {
        if depth > depth_cap {
            return;
        }
        let Some(&(ms, cnt)) = sums.get(name) else { return };
        let pct = if denom > 0.0 { ms / denom * 100.0 } else { 0.0 };
        let avg = if cnt > 0 { ms * 1000.0 / cnt as f64 } else { 0.0 };
        let _ = writeln!(
            out,
            "{:indent$}{:<22}{:>9.2}ms {:>5.1}% cnt={:>8} avg={:.2}µs",
            "", name, ms, pct, cnt, avg,
            indent = depth * 2,
        );
        let mut child_sum = 0.0;
        if let Some(kids) = byparent.get(name) {
            for (c, _cms) in kids {
                if let Some(&(cms, _)) = sums.get(c.as_str()) {
                    rec(out, c, sums, byparent, denom, depth + 1, depth_cap);
                    child_sum += cms;
                }
            }
        }
        if child_sum > 0.0 && ms > child_sum {
            let own = ms - child_sum;
            let _ = writeln!(
                out,
                "{:indent$}{:<22}{:>9.2}ms {:>5.1}%  (自身)",
                "", "(自身)", own, if denom > 0.0 { own / denom * 100.0 } else { 0.0 },
                indent = (depth + 1) * 2,
            );
        }
    }

    for r in &roots {
        let denom = sums.get(r).map(|v| v.0).unwrap_or(0.0);
        rec(&mut out, r, &sums, &byparent, denom, 0, 24);
    }
    // 覆盖集 = 从根沿 byparent 可达的所有名字；孤儿 = 不在任何子树上的名字
    // （常见原因：根的事件被 cap 截掉，或该名只出现在其它场景）。
    fn mark_covered(
        name: &str,
        byparent: &HashMap<String, Vec<(String, f64)>>,
        covered: &mut HashMap<String, bool>,
        depth: usize,
    ) {
        if depth > 24 || covered.contains_key(name) {
            return;
        }
        covered.insert(name.to_string(), true);
        if let Some(kids) = byparent.get(name) {
            for (c, _) in kids {
                mark_covered(c, byparent, covered, depth + 1);
            }
        }
    }
    let mut covered: HashMap<String, bool> = HashMap::new();
    for r in &roots {
        mark_covered(r, &byparent, &mut covered, 0);
    }
    let mut orphan: Vec<String> = sums
        .keys()
        .filter(|k| !covered.contains_key(*k))
        .cloned()
        .collect();
    orphan.sort_by(|a, b| sums[b].0.partial_cmp(&sums[a].0).unwrap_or(std::cmp::Ordering::Equal));
    if !orphan.is_empty() {
        let _ = writeln!(out, "（未挂在根下的名字）");
        for o in orphan {
            let (ms, cnt) = sums[&o];
            let _ = writeln!(out, "  {o:<22}{:>9.2}ms cnt={cnt}", ms);
        }
    }
    if !multi.is_empty() {
        let _ = writeln!(out, "（多父名——按主导父归属，其父的(自身)为近似）");
        for (c, parents) in multi {
            let tot: f64 = parents.iter().map(|p| p.1).sum();
            let parts: Vec<String> = parents
                .iter()
                .map(|(p, ms)| format!("{} {:.0}%", p, ms / tot * 100.0))
                .collect();
            let _ = writeln!(out, "  {c:<22}{}", parts.join(" | "));
        }
    }
    out
}

// ---- Merge JS data ----

pub fn merge_js_data(sess: &TelemetryInner, json_str: &str) {
    let Ok(data) = serde_json::from_str::<serde_json::Value>(json_str) else { return };
    let Some(obj) = data.as_object() else { return };
    for (source, layers) in obj {
        let Some(layer_obj) = layers.as_object() else { continue };
        for (layer, durs) in layer_obj {
            let Some(arr) = durs.as_array() else { continue };
            let full_name = format!("{}_{}", source, layer);
            for v in arr {
                // 接受整数纳秒 (JS snapshotNs) 与浮点毫秒 (手写/测试) — 统一转纳秒
                let d = if let Some(i) = v.as_u64() {
                    i
                } else if let Some(f) = v.as_f64() {
                    (f * 1_000_000.0) as u64
                } else {
                    continue
                };
                sess.add_layer_sample(&full_name, d, SampleBucket::Edit, 0, 0);
            }
        }
    }
}

// ---- Export layer stats as JSON ----

pub fn export_layer_stats_json(sess: &TelemetryInner) -> String {
    let stats = sess.layer_stats.lock();
    let items: Vec<serde_json::Value> = stats.iter().map(|s| {
        let bucket_str = match s.bucket {
            SampleBucket::Edit => "edit",
            SampleBucket::Probe => "probe",
            SampleBucket::Ctrl => "ctrl",
        };
        serde_json::json!({
            "layer_name": s.layer_name,
            "dur_ns": s.dur_ns,
            "bucket": bucket_str,
            "tag": s.tag,
            "doc_id": s.doc_id,
        })
    }).collect();
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".into())
}

/// Export phase durations as JSON array of {name_hash, name, dur_ns}.
/// Bench mode: from accumulator; Trace mode: aggregated from events buffer.
pub fn export_phase_json(sess: &TelemetryInner) -> String {
    let phase_names = sess.phase_names.lock();
    let mut rows: Vec<serde_json::Value> = Vec::new();
    // ① kv 样本事件（带 span meta——计数/属性维度；src=events）
    {
        let events = sess.events.lock();
        let pool = sess.meta_pool.lock();
        for ev in events.iter() {
            let name = phase_names.get(&ev.name_hash).cloned().unwrap_or_else(|| format!("{:08x}", ev.name_hash));
            let mut row = serde_json::json!({
                "name_hash": ev.name_hash,
                // 导出 parent_hash：平铺流没有树，跨线程归属无法做
                //（实测 path_span_sum > path_render_total 即为证据）。
                "parent_hash": ev.parent_hash,
                "name": name,
                "dur_ns": ev.dur_ns,
                "src": "events",
            });
            if ev.meta_len > 0 {
                let bytes = &pool[ev.meta_offset as usize..][..ev.meta_len as usize];
                for (k, v) in decode_meta(bytes) {
                    row[k] = serde_json::Value::String(v);
                }
            }
            rows.push(row);
        }
    }
    // ② acc 全量时间（无 kv——时间/计数维度；src=acc）
    {
        let acc = sess.accumulator.lock();
        if let Some(ref acc) = *acc {
            for (name_hash, durs) in &acc.durations {
                let name = phase_names.get(name_hash).cloned().unwrap_or_else(|| format!("{:08x}", name_hash));
                for dur_ns in durs {
                    rows.push(serde_json::json!({
                        "name_hash": name_hash,
                        "name": name,
                        "dur_ns": dur_ns,
                        "src": "acc",
                    }));
                }
            }
        }
    }
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
}

// ---- Chrome Trace JSON ----

pub fn report_json_trace(sess: &TelemetryInner) -> String {
    let events = sess.events.lock(); let phase_names = sess.phase_names.lock(); let pool = sess.meta_pool.lock();
    let mut je = Vec::new();
    for ev in events.iter() {
        let name = phase_names.get(&ev.name_hash).cloned().unwrap_or_else(|| format!("{:08x}", ev.name_hash));
        let mut map: HashMap<String, String> = if ev.meta_len > 0 {
            let bytes = &pool[ev.meta_offset as usize..][..ev.meta_len as usize];
            decode_meta(bytes).into_iter().collect()
        } else { HashMap::new() };
        // Chrome trace 格式无 parent 字段 — 父 span 名显式放入 args 保住树结构
        if ev.parent_hash != 0 {
            let parent = phase_names.get(&ev.parent_hash).cloned().unwrap_or_else(|| format!("{:08x}", ev.parent_hash));
            map.insert("parent".to_string(), parent);
        }
        let args = serde_json::to_string(&map).unwrap_or_else(|_| "{}".into());
        je.push(format!(r#"{{"name":"{}","cat":"mdio","ph":"X","ts":{},"dur":{},"pid":1,"tid":1,"args":{}}}"#,
            name, ev.start_ns as f64 / 1000.0, ev.dur_ns as f64 / 1000.0, args));
    }
    format!("{{\"traceEvents\":[{}]}}", je.join(","))
}


// ---- Public: report_all (Rust 出口 — 写 trace JSON + 消费性 clear; 终端报告由 Python report_session 完成) ----

pub fn report_all(sess: &TelemetryInner, base_dir: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // JSON trace (for speedscope.app — keep)
    let json = report_json_trace(sess);
    // 空事件时 JSON 为 {"traceEvents":[]} — 不写空壳文件
    if !json.contains("\"traceEvents\":[]") && !json.contains("\"traceEvents\":[ ]") {
        let _ = write_file(base_dir, &sess.name, ts, "json", &json);
    }

    sess.clear();
    String::new()
}

fn write_file(base_dir: &str, session_name: &str, ts: u64, ext: &str, content: &str) -> Option<String> {
    let dir = std::path::Path::new(base_dir).join(sanitize_dirname(session_name));
    if std::fs::create_dir_all(&dir).is_err() { return None; }
    let fname = format!("report_{:016x}.{}", ts, ext);
    let p = dir.join(&fname);
    if std::fs::write(&p, content).is_err() { return None; }
    Some(format!("{}/{}", dir.display(), fname))
}

fn sanitize_dirname(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}
