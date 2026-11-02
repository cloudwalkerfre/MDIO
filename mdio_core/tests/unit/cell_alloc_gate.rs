// cell_alloc_gate — 性能门（分配数轨）
//
// 自写计数 #[global_allocator]：零依赖、跨平台、可本机跑；默认 feature 集下
// 采集（in-process 库内测试，不受 feature 门控）。指令数轨（gungraun /
// Valgrind）在 Apple Silicon macOS 上不可运行（只能放 Linux CI）。
//
// 计数口径：只统计**当前线程**在**窗口内**的堆分配（thread_local 计数，
// 并行测试的其他线程互不污染）；窗口内不得有测试自身的格式化/打印。
// 判据形态（等价改写的性能面）：
//   * 命中 / Peek / UseIfReady 未命中路径：**零堆流量**（换元不得加分配）；
//   * 冷构建路径：有堆流量但受上界约束（阈值 = 实测 + 余量，防病态膨胀）。

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::projection::{Snapshot, State};

struct CountingAlloc;

/// 计数开关：未开启时每次分配只多一条 relaxed load。
static ALLOC_ARMED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static T_ALLOCS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static T_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if ALLOC_ARMED.load(Ordering::Relaxed) {
            T_ALLOCS.with(|c| c.set(c.get() + 1));
            T_BYTES.with(|c| c.set(c.get() + layout.size() as u64));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if ALLOC_ARMED.load(Ordering::Relaxed) {
            T_ALLOCS.with(|c| c.set(c.get() + 1));
            T_BYTES.with(|c| c.set(c.get() + layout.size() as u64));
        }
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if ALLOC_ARMED.load(Ordering::Relaxed) {
            T_ALLOCS.with(|c| c.set(c.get() + 1));
            T_BYTES.with(|c| c.set(c.get() + new_size.saturating_sub(layout.size()) as u64));
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC_GATE: CountingAlloc = CountingAlloc;

/// 窗口测量：返回 (结果, 分配次数, 分配字节)。结果在本线程窗口外析构。
fn measure<R>(f: impl FnOnce() -> R) -> (R, u64, u64) {
    T_ALLOCS.with(|c| c.set(0));
    T_BYTES.with(|c| c.set(0));
    ALLOC_ARMED.store(true, Ordering::SeqCst);
    let r = f();
    ALLOC_ARMED.store(false, Ordering::SeqCst);
    (r, T_ALLOCS.with(|c| c.get()), T_BYTES.with(|c| c.get()))
}

fn cold(text: &str) -> Snapshot {
    State::new(text).snapshot()
}

const DOC: &str = "# H\n\npara one\n\npara two\n";

#[test]
fn alloc_gate_hit_and_peek_paths_are_allocation_free() {
    // 冷构建在窗口内（有流量），随后同一格的命中路径必须零流量。
    let snap = cold(DOC);

    let (rt, a_cold, b_cold) = measure(|| snap.root_table_cell_merge());
    println!("[alloc_gate] {{\"op\":\"root_table_cell_merge_cold\",\"allocs\":{a_cold},\"bytes\":{b_cold}}}");
    assert!(a_cold > 0 && b_cold > 0, "冷构建必须有堆流量（否则窗口没生效）");

    let (rt2, a_hit, b_hit) = measure(|| snap.root_table_cell_merge());
    assert_eq!((a_hit, b_hit), (0, 0), "命中路径不得有堆流量（S2 换元加了分配）");
    assert!(Arc::ptr_eq(&rt, &rt2), "命中必须同一 Arc");

    let (peek_hit, a_peek, b_peek) = measure(|| snap.peek_root_table_cell(false));
    assert!(peek_hit, "Build 之后 Peek 必须 true");
    assert_eq!((a_peek, b_peek), (0, 0), "Peek 命中路径不得有堆流量");

    // 冷 Peek / 冷 UseIfReady 未命中：零流量（不建、不装）。
    let snap2 = cold(DOC);
    let (peek_cold, a_p1, b_p1) = measure(|| snap2.peek_root_table_cell(false));
    assert!(!peek_cold);
    assert_eq!((a_p1, b_p1), (0, 0), "冷 Peek 不得有堆流量");
    let (ifr, a_p2, b_p2) = measure(|| snap2.block_spans_cell_if_ready());
    assert!(ifr.is_none());
    assert_eq!((a_p2, b_p2), (0, 0), "冷 UseIfReady 未命中不得有堆流量");
    let (pks, a_p3, b_p3) = measure(|| snap2.peek_block_spans_cell());
    assert!(pks.is_none());
    assert_eq!((a_p3, b_p3), (0, 0), "冷 Peek（跨度）不得有堆流量");

    // 树格 / 节格：冷构建有流量，二查零流量。
    let (t, a_t, b_t) = measure(|| snap2.tree_cell(&crate::topology::CONTAINER_TOPOLOGY));
    println!("[alloc_gate] {{\"op\":\"tree_cell_cold\",\"allocs\":{a_t},\"bytes\":{b_t}}}");
    assert!(a_t > 0 && b_t > 0);
    let (t2, a_t2, b_t2) = measure(|| snap2.tree_cell(&crate::topology::CONTAINER_TOPOLOGY));
    assert_eq!((a_t2, b_t2), (0, 0), "树格二查不得有堆流量");
    assert!(Arc::ptr_eq(&t.seg, &t2.seg));
}

#[test]
fn alloc_gate_warm_inherit_after_install_is_allocation_free() {
    let prev = Arc::new(cold(DOC));
    let _ = prev.root_table_cell_merge();
    let _ = prev.block_spans_cell();
    let mut cur = cold(DOC);
    cur.prev_snapshot = Arc::downgrade(&prev);

    // 首次继承命中会安装本代格槽（建槽允许分配），第二次起必须零流量。
    let (rt1, a1, b1) = measure(|| cur.root_table_cell_merge());
    println!("[alloc_gate] {{\"op\":\"warm_inherit_first\",\"allocs\":{a1},\"bytes\":{b1}}}");
    let (rt2, a2, b2) = measure(|| cur.root_table_cell_merge());
    assert_eq!((a2, b2), (0, 0), "已装槽后的继承命中不得有堆流量");
    assert!(Arc::ptr_eq(&rt1, &rt2));

    // UseIfReady 命中装载后，Peek 与 Build 都零流量。
    let (sp, a3, b3) = measure(|| cur.block_spans_cell_if_ready());
    println!("[alloc_gate] {{\"op\":\"warm_if_ready_first\",\"allocs\":{a3},\"bytes\":{b3}}}");
    assert!(sp.is_some());
    let (sp2, a4, b4) = measure(|| cur.peek_block_spans_cell());
    assert_eq!((a4, b4), (0, 0), "Peek 不得有堆流量");
    assert!(sp2.is_some());
    let (sp3, a5, b5) = measure(|| cur.block_spans_cell());
    assert_eq!((a5, b5), (0, 0), "Build 命中已装槽不得有堆流量");
    assert!(Arc::ptr_eq(sp.as_ref().unwrap(), &sp3));
}

/// 冷构建上界（阈值比对）：实测值写进消息；上界只防病态膨胀，不锁常数因子。
#[test]
fn alloc_gate_cold_build_cost_within_bound() {
    let snap = cold(DOC);
    let (rt, a, b) = measure(|| snap.root_table_cell_merge());
    println!("[alloc_gate] {{\"op\":\"cold_build_bound\",\"allocs\":{a},\"bytes\":{b}}}");
    assert!(!rt.is_empty());
    assert!(a <= 64, "冷建根表分配次数上界 64，实测 {a}");
    assert!(b <= 256 * 1024, "冷建根表分配字节上界 256KiB，实测 {b}");
}
