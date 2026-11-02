//! 故障注入与 panic 围堵的记账（诊断/实验用，生产默认零行为）。
//!
//! 两件事，都很窄：
//! 1. `MDIO_TEST_PANIC=<kind>[:<n>]`（kind = `exec` | `path`）—— 在对应路径上
//!    第 n 次调用时**炸一次**（默认 n=1）。用来验证「一次坏任务 / 一次坏请求
//!    会把什么一起带下去」。不设这个 env 时零开销、零行为。
//! 2. 被 `catch_unwind` 拦下的任务必须**留下痕迹**：光「不崩」不够 —— 被跳过的
//!    任务意味着那篇文档的补丁可能缺失。不记账就等于把「静默死亡」换成
//!    「静默陈旧」，后者更糟。
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

static ARMED: OnceLock<(String, u32)> = OnceLock::new();
static SEEN: AtomicU64 = AtomicU64::new(0);

/// 该路径是否被注入要求炸一次（`MDIO_TEST_PANIC=exec|path[:n]`）。
pub(crate) fn panic_armed(kind: &str) -> bool {
    let (want, nth) = ARMED.get_or_init(|| {
        let raw = std::env::var("MDIO_TEST_PANIC").unwrap_or_default();
        match raw.split_once(':') {
            Some((k, n)) => (k.trim().to_string(), n.trim().parse().unwrap_or(1)),
            None => (raw.trim().to_string(), 1),
        }
    });
    if want.is_empty() || want != kind { return false; }
    // 可选武装文件：启动期本身会调用 edit（实测 ≥2 次），计数法只能靠猜。
    // 给了 `MDIO_TEST_PANIC_ARM=<path>` 就等这个文件出现才开始计数 ——
    // 于是「武装时刻」可以被精确地放到实验开始之后。
    if let Ok(f) = std::env::var("MDIO_TEST_PANIC_ARM") {
        if !std::path::Path::new(&f).exists() { return false; }
    }
    SEEN.fetch_add(1, Ordering::SeqCst) + 1 == u64::from(*nth)
}

static CONTAINED: AtomicU64 = AtomicU64::new(0);
static DIRTY: OnceLock<parking_lot::Mutex<HashSet<u32>>> = OnceLock::new();

fn dirty() -> &'static parking_lot::Mutex<HashSet<u32>> {
    DIRTY.get_or_init(|| parking_lot::Mutex::new(HashSet::new()))
}

/// 拦下一次任务 panic：计数 + 记下该文档「补丁可能缺失」。
pub(crate) fn note_contained_panic(doc_id: u32) {
    CONTAINED.fetch_add(1, Ordering::Relaxed);
    dirty().lock().insert(doc_id);
}

/// 报表：(`拦下次数`, `待重同步的文档`)。幂等可重复读（不清空）。
pub fn contained_panic_report() -> (u64, Vec<u32>) {
    let n = CONTAINED.load(Ordering::Relaxed);
    let mut docs: Vec<u32> = dirty().lock().iter().copied().collect();
    docs.sort_unstable();
    (n, docs)
}
