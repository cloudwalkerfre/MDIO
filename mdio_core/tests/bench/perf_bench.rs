//! tests/perf_bench.rs
//!
//! MDIO Core 性能基准
//!
//! Engine Micro  — rope / ripple / edit+snapshot 原子操作
//! Selector      — drill / key / wildcard / filter 路径解析
//! Transaction   — 事务分层开销 / 最大吞吐
//! Rate Control  — 不同速率 / 位置 / 模式下吞吐
//! Sync          — 单向 / 双向同步延迟
//! Parallel      — 跨文档并行扩展

use std::time::{Instant, Duration};
use std::thread;
use std::sync::Arc;
use dashmap::DashSet;
use rand::Rng;
use mdio_core::MdioEngine;
use mdio_core::selector::{Step, FilterExpr, TypeName, CmpOp, Value, SingleSlice};
use mdio_core::BlockProj;
use mdio_core::session::MdioSession;

// Helpers

fn build_structured_doc(sections: usize) -> String {
    let mut doc = String::new();
    for s in 0..sections {
        doc.push_str(&format!("# Section {}\n\n", s));
        doc.push_str(&format!("- host_{}: value_{}\n", s, s));
        for i in 0..10 {
            doc.push_str(&format!("- key_{}: v{}_{}\n", i, i, s));
        }
        doc.push('\n');
    }
    doc
}

fn build_simple_doc(size_bytes: usize) -> String {
    "Hello ".repeat(size_bytes / 6)
}

fn make_test_doc(num_sections: usize, bullets_per_section: usize) -> String {
    let mut doc = String::new();
    for s in 0..num_sections {
        doc.push_str(&format!("# Section {}\n", s));
        doc.push_str(&format!("Section {} description paragraph.\n\n", s));
        for b in 0..bullets_per_section {
            doc.push_str(&format!("- key_{}: value value value value_{}\n", b, b));
        }
        doc.push_str(&format!("```rust\nfn func_{}() {{\n    // code block\n}}\n```\n\n", s));
    }
    doc
}

// Engine Micro — rope / ripple / edit+snapshot 原子操作
//
// bench_ripple_position_sensitivity: 1MB 文档编辑首/中/尾部，测量 rope 位移 + ripple 扫描代价
// bench_edit_and_snapshot:           edit + make_snapshot 往返延迟
// bench_snapshot_clone:              Snapshot Arc clone 成本

#[test]
fn bench_ripple_position_sensitivity() {
    let sections = 1000;
    let text = build_structured_doc(sections);

    println!("\n========== 编辑位置敏感性 ==========\n");
    println!("文档: {:.0}KB, {} 行, {} sections",
        text.len() as f64 / 1024.0, text.lines().count(), sections);

    let head_line = format!("host_0: value_0\n");
    let head_pos = text.find(&head_line).unwrap() + head_line.len();
    let mid_line = format!("host_{}: value_{}\n", sections / 2, sections / 2);
    let mid_pos = text.find(&mid_line).unwrap() + mid_line.len();
    let tail_line = format!("host_{}: value_{}\n", sections - 1, sections - 1);
    let tail_pos = text.find(&tail_line).unwrap() + tail_line.len();

    let points = [
        ("头部 (S0)",    head_pos),
        ("中部 (S500)",  mid_pos),
        ("尾部 (S999)",  tail_pos),
    ];

    // 轮次 A: 等长行内替换
    println!("\n── 轮次 A: 等长替换 (5B → 7B, 行数不变) ──");
    println!("{:>20} | {:>10} | {:>12} | {:>10}", "位置", "edit", "snapshot", "总耗时");
    println!("{}", "-".repeat(58));

    for (label, ins_pos) in &points {
        let mut engine = MdioEngine::new(&text);
        let val_start = ins_pos - 7;
        let t0 = Instant::now();
        engine.edit(val_start, val_start + 7, "CHANGED");
        let edit_us = t0.elapsed().as_nanos() as f64 / 1000.0;
        let t0 = Instant::now();
        engine.make_snapshot();
        let snap_us = t0.elapsed().as_nanos() as f64 / 1000.0;
        println!("{:>20} | {:>8.1?}μs | {:>10.1?}μs | {:>8.1?}μs",
            label, edit_us, snap_us, edit_us + snap_us);
    }
    println!("  → edit 列差异 = rope B-tree 字节位移成本");
    println!("  → snapshot 列持平 = 行结构未变, ripple 首行即收敛");

    // 轮次 B: 插入新行
    let insert_text = "- new_key: new_value\n";
    println!("\n── 轮次 B: 插入 1 行 (行数 +1) ──");
    println!("{:>20} | {:>10} | {:>12} | {:>10}", "位置", "edit", "snapshot", "总耗时");
    println!("{}", "-".repeat(58));

    for (label, ins_pos) in &points {
        let mut engine = MdioEngine::new(&text);
        let t0 = Instant::now();
        engine.edit(*ins_pos, *ins_pos, insert_text);
        let edit_us = t0.elapsed().as_nanos() as f64 / 1000.0;
        let t0 = Instant::now();
        engine.make_snapshot();
        let snap_us = t0.elapsed().as_nanos() as f64 / 1000.0;
        println!("{:>20} | {:>8.1?}μs | {:>10.1?}μs | {:>8.1?}μs",
            label, edit_us, snap_us, edit_us + snap_us);
    }
    println!("  → snapshot 列出现显著差异 = ripple 从插入点扫描至收敛");
}

#[test]
fn bench_edit_and_snapshot() {
    let doc = make_test_doc(200, 10);
    let mut engine = MdioEngine::new(&doc);
    let mid = engine.byte_len() / 2;

    let n = 5_000;
    let start = Instant::now();
    for _ in 0..n {
        engine.edit(mid, mid, "x");
        let _snap = engine.make_snapshot();
        engine.edit(mid, mid + 1, "");
        engine.make_snapshot();
    }
    let avg_us = start.elapsed().as_micros() as f64 / (n as f64 * 2.0);
    println!("[edit_and_snapshot]        n={}  avg={:.2} μs per (edit+snapshot)", n, avg_us);
}

#[test]
fn bench_snapshot_clone() {
    let doc = make_test_doc(200, 10);
    let mut engine = MdioEngine::new(&doc);
    let snapshot = engine.make_snapshot();

    let n = 10_000;
    let start = Instant::now();
    for _ in 0..n {
        let _c = snapshot.clone();
    }
    let avg_us = start.elapsed().as_micros() as f64 / n as f64;
    println!("[snapshot_clone]           n={}  avg={:.3} μs", n, avg_us);
}

// Selector — drill / key / wildcard / filter 路径解析

#[test]
fn bench_select_drill_key() {
    let doc = make_test_doc(100, 20);
    let mut engine = MdioEngine::new(&doc);
    let snapshot = engine.make_snapshot();

    let steps = vec![
        Step::Drill("Section 50".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String("key_10".into()) }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let n = 10_000;
    let start = Instant::now();
    for _ in 0..n {
        let _ = snapshot.select_nodes(&steps);
    }
    let avg_us = start.elapsed().as_micros() as f64 / n as f64;
    println!("[select_drill_key]        n={}  avg={:.3} μs", n, avg_us);
}

#[test]
fn bench_select_drill_header() {
    let doc = make_test_doc(500, 5);
    let mut engine = MdioEngine::new(&doc);
    let snapshot = engine.make_snapshot();

    let steps = vec![Step::Drill("Section 250".to_string())];

    let n = 10_000;
    let start = Instant::now();
    for _ in 0..n {
        let _ = snapshot.select_nodes(&steps);
    }
    let avg_us = start.elapsed().as_micros() as f64 / n as f64;
    println!("[select_drill_header]     n={}  avg={:.3} μs", n, avg_us);
}

#[test]
fn bench_select_wildcard() {
    let doc = make_test_doc(100, 100);
    let mut engine = MdioEngine::new(&doc);
    let snapshot = engine.make_snapshot();

    let steps = vec![
        Step::Drill("Section 50".to_string()),
        Step::Wildcard,
    ];

    let n = 5_000;
    let start = Instant::now();
    for _ in 0..n {
        let _ = snapshot.select_nodes(&steps);
    }
    let avg_us = start.elapsed().as_micros() as f64 / n as f64;
    println!("[select_wildcard]         n={}  avg={:.3} μs", n, avg_us);
}

#[test]
fn bench_select_recursive_code_filter() {
    let doc = make_test_doc(200, 10);
    let doc_size = doc.len();
    let mut engine = MdioEngine::new(&doc);
    let snapshot = engine.make_snapshot();

    let steps = vec![
        Step::RecursiveWildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Code)),
    ];

    let n = 100;
    let start = Instant::now();
    for _ in 0..n {
        let _ = snapshot.select_nodes(&steps);
    }
    let avg_us = start.elapsed().as_micros() as f64 / n as f64;
    println!("[select_recursive_code]   n={}  avg={:.1} μs  doc={} bytes", n, avg_us, doc_size);
}

#[test]
fn bench_select_for_write() {
    let doc = make_test_doc(100, 20);
    let mut engine = MdioEngine::new(&doc);
    let snapshot = engine.make_snapshot();

    let steps = vec![
        Step::Drill("Section 50".to_string()),
        Step::Wildcard,
    ];

    let n = 5_000;
    let start = Instant::now();
    for _ in 0..n {
        let _ = mdio_core::selector::select_for_write(&snapshot, &steps);
    }
    let avg_us = start.elapsed().as_micros() as f64 / n as f64;
    println!("[select_for_write]        n={}  avg={:.3} μs", n, avg_us);
}

// Transaction — 事务分层开销 / 最大吞吐
//
// bench_transaction_overhead: 逐层剥离 Engine → Session空 → 全栈
// test_max_throughput:        10K ops 极限吞吐 + 延迟分布 + 一致性验证

#[test]
fn bench_transaction_overhead() {
    let text = build_simple_doc(50_000);
    let iterations = 1000;
    let edit_text = "X".repeat(10);

    println!("\n========== 事务固定开销拆解 ==========\n");
    println!("文档: {:.0}KB, 迭代: {}", text.len() as f64 / 1024.0, iterations);

    // 层1: 纯 Engine (edit + make_snapshot)
    {
        let mut engine = MdioEngine::new(&text);
        let mut times = Vec::with_capacity(iterations);
        for i in 0..iterations {
            let pos = i * 10;
            let t0 = Instant::now();
            engine.edit(pos, pos, &edit_text);
            engine.make_snapshot();
            times.push(t0.elapsed().as_nanos() as f64 / 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = times.len();
        println!("[Engine] edit + make_snapshot:");
        println!("  P50={:>8.2}μs  P99={:>8.2}μs  avg={:>8.2}μs  ← rope + ripple + clone",
            times[n/2], times[n*99/100], times.iter().sum::<f64>() / n as f64);
    }

    // 层2: 空事务框架
    {
        let session = MdioSession::new(0, &text, 1, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
        let mut times = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let t0 = Instant::now();
            let (_, _): ((), Vec<u8>) = session.execute_transaction(None, false, |_ctx| {});
            times.push(t0.elapsed().as_nanos() as f64 / 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = times.len();
        println!("\n[Session] execute_transaction (空闭包):");
        println!("  P50={:>8.2}μs  P99={:>8.2}μs  avg={:>8.2}μs  ← lock + 空快照 + swap",
            times[n/2], times[n*99/100], times.iter().sum::<f64>() / n as f64);
    }

    // 层3: 全栈编辑
    {
        let session = MdioSession::new(0, &text, 1, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
        let mut times = Vec::with_capacity(iterations);
        for i in 0..iterations {
            let pos = i * 10;
            let txt = edit_text.clone();
            let t0 = Instant::now();
            let (_, _) = session.execute_transaction(None, false, |ctx| {
                ctx.apply_mdio_op(vec![pos..pos], "+>", &txt);
            });
            times.push(t0.elapsed().as_nanos() as f64 / 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = times.len();
        println!("\n[Session] execute_transaction (append 10B):");
        println!("  P50={:>8.2}μs  P99={:>8.2}μs  avg={:>8.2}μs  ← 全路径编辑",
            times[n/2], times[n*99/100], times.iter().sum::<f64>() / n as f64);
    }

    println!("\n解读:");
    println!("  层1 (Engine)      = rope + parser + snapshot clone 的物理极限");
    println!("  层2 (空事务)      = Mutex lock + empty TransactionContext + ArcSwap store");
    println!("  层3 (全栈)        = 层1 + 层2 + apply_mdio_op 分发 + WireOp 序列化");
}

#[test]
fn test_max_throughput() {
    let num_ops = 10_000;
    let warmup_ops = 1000;

    println!("\n========== 极限吞吐量 ==========\n");

    let peer_id = rand::rng().random::<u64>();
    let session = MdioSession::new(0, "", peer_id, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
    let mut rng = rand::rng();

    // 预热
    println!("预热阶段 ({} 次操作)...", warmup_ops);
    let mut warmup_len = 0;
    for _ in 0..warmup_ops {
        let pos = if warmup_len == 0 { 0 } else { rng.random_range(0..warmup_len) };
        let size = rng.random_range(1..=300);
        let text: String = std::iter::repeat_with(|| rng.random_range(b'a'..=b'z') as char)
            .take(size).collect();
        let (_, _) = session.execute_transaction(None, false, |ctx| {
            ctx.apply_mdio_op(vec![pos..pos], "+>", &text);
        });
        warmup_len += size;
    }
    println!("预热完成: {} bytes", warmup_len);

    // 预生成测试操作
    let mut ops = Vec::with_capacity(num_ops);
    let mut current_len = warmup_len;
    for i in 0..num_ops {
        let pos = if current_len == 0 { 0 } else { rng.random_range(0..current_len) };
        let size = rng.random_range(1..=300);
        let text: String = std::iter::repeat_with(|| rng.random_range(b'a'..=b'z') as char)
            .take(size).collect();
        ops.push((pos, text.clone(), size));
        current_len += size;
        if i % 2000 == 0 && i > 0 { println!("  已生成 {}/{} 个操作...", i, num_ops); }
    }

    // 纯执行测量
    let mut latencies = Vec::with_capacity(num_ops);
    let start = Instant::now();
    for (i, (pos, text, _)) in ops.iter().enumerate() {
        let op_start = Instant::now();
        let (_, _) = session.execute_transaction(None, false, |ctx| {
            ctx.apply_mdio_op(vec![*pos..*pos], "+>", text);
        });
        latencies.push(op_start.elapsed().as_nanos());
        if i % 2000 == 0 && i > 0 { println!("  已执行 {}/{} 个操作...", i, num_ops); }
    }
    let elapsed = start.elapsed();

    latencies.sort();
    let n = latencies.len();
    let throughput = num_ops as f64 / elapsed.as_secs_f64();

    println!("\n📊 性能统计:");
    println!("总操作: {} 次 (含预热 {} 次)", num_ops, warmup_ops);
    println!("总时间: {:.3}s", elapsed.as_secs_f64());
    println!("系统吞吐量: {:.0} ops/s", throughput);
    println!("\n⏱️ 延迟分布 (微秒):");
    println!("  平均: {:.1} μs", latencies.iter().sum::<u128>() as f64 / n as f64 / 1000.0);
    println!("  P50:  {:.1} μs", latencies[n/2] as f64 / 1000.0);
    println!("  P90:  {:.1} μs", latencies[(n*90)/100] as f64 / 1000.0);
    println!("  P99:  {:.1} μs", latencies[(n*99)/100] as f64 / 1000.0);
    println!("  P99.9: {:.1} μs", latencies[(n*999)/1000] as f64 / 1000.0);

    // 数据一致性验证
    let expected_len = warmup_len + ops.iter().map(|(_, _, size)| size).sum::<usize>();
    let actual_len = {
        let snap = session.publisher.load();
        snap.byte_len()
    };
    if actual_len == expected_len {
        println!("\n✅ 数据一致性验证通过: {} bytes", actual_len);
    } else {
        println!("\n❌ 数据不一致: 预期 {} bytes, 实际 {} bytes", expected_len, actual_len);
        panic!("数据一致性验证失败");
    }
    println!("\n🎯 测试完成");
}

// Rate Control — 不同速率 / 位置 / 模式下的吞吐
//
// test_rate_controlled:        随机位置随机大小, 6 档目标速率
// test_rate_controlled_fixed:  末尾追加固定大小, 6 档目标速率
// test_rate_controlled_middle: 1MB 位置固定大小, 6 档目标速率
// test_engine_rate_controlled: 纯 Engine (无 Cola) 末尾追加
// test_engine_bulk_throughput: 纯 Engine 50K 随机位置

const RATE_TARGETS: [u32; 6] = [1000, 5000, 10000, 30000, 50000, 80000];
const RATE_DURATION: Duration = Duration::from_secs(3);

#[test]
fn test_rate_controlled() {
    println!("\n========== 速率控制 — 随机位置随机大小 ==========\n");
    println!("{:>10} | {:>12} | {:>10} | {:>10}", "目标速率", "实际速率", "平均 μs", "P99 μs");
    println!("{}", "-".repeat(55));

    for &target_rate in &RATE_TARGETS {
        let peer_id = rand::rng().random::<u64>();
        let session = MdioSession::new(0, "", peer_id, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
        let mut latencies = Vec::new();
        let interval_ns = 1_000_000_000u64 / target_rate as u64;
        let mut rng = rand::rng();
        let mut current_len = 0;
        let start = Instant::now();
        let mut count = 0;

        while start.elapsed() < RATE_DURATION {
            let loop_start = Instant::now();
            let pos = if current_len == 0 { 0 } else { rng.random_range(0..current_len) };
            let size = rng.random_range(1..=100);
            let text: String = std::iter::repeat_with(|| rng.random_range(b'a'..=b'z') as char)
                .take(size).collect();
            let op_start = Instant::now();
            let (_, _) = session.execute_transaction(None, false, |ctx| {
                ctx.apply_mdio_op(vec![pos..pos], "+>", &text);
            });
            latencies.push(op_start.elapsed().as_nanos());
            current_len += size;
            count += 1;
            let elapsed_ns = loop_start.elapsed().as_nanos() as u64;
            if elapsed_ns < interval_ns {
                let sleep_us = (interval_ns - elapsed_ns) / 1000;
                if sleep_us >= 1 { thread::sleep(Duration::from_micros(sleep_us)); }
            }
        }

        let elapsed = start.elapsed();
        let actual_rate = count as f64 / elapsed.as_secs_f64();
        latencies.sort();
        let n = latencies.len();
        let avg = latencies.iter().sum::<u128>() as f64 / n as f64 / 1000.0;
        let p99 = latencies[(n*99)/100] as f64 / 1000.0;
        println!("{:>10}/s | {:>12.0}/s | {:>10.1} | {:>10.1}", target_rate, actual_rate, avg, p99);
    }
    println!("\n✅ 完成");
}

#[test]
fn test_rate_controlled_fixed() {
    let fixed_size = 50;
    let fixed_text = "a".repeat(fixed_size);

    println!("\n========== 速率控制 — 末尾追加 {} bytes ==========\n", fixed_size);
    println!("{:>10} | {:>12} | {:>10} | {:>10}", "目标速率", "实际速率", "平均 μs", "P99 μs");
    println!("{}", "-".repeat(55));

    for &target_rate in &RATE_TARGETS {
        let peer_id = rand::rng().random::<u64>();
        let session = MdioSession::new(0, "", peer_id, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
        let mut latencies = Vec::new();
        let interval_ns = 1_000_000_000u64 / target_rate as u64;
        let mut current_len = 0;
        let start = Instant::now();
        let mut count = 0;

        while start.elapsed() < RATE_DURATION {
            let loop_start = Instant::now();
            let op_start = Instant::now();
            let (_, _) = session.execute_transaction(None, false, |ctx| {
                ctx.apply_mdio_op(vec![current_len..current_len], "+>", &fixed_text);
            });
            latencies.push(op_start.elapsed().as_nanos());
            current_len += fixed_size;
            count += 1;
            let elapsed_ns = loop_start.elapsed().as_nanos() as u64;
            if elapsed_ns < interval_ns {
                let sleep_us = (interval_ns - elapsed_ns) / 1000;
                if sleep_us >= 1 { thread::sleep(Duration::from_micros(sleep_us)); }
            }
        }

        let elapsed = start.elapsed();
        let actual_rate = count as f64 / elapsed.as_secs_f64();
        latencies.sort();
        let n = latencies.len();
        let avg = latencies.iter().sum::<u128>() as f64 / n as f64 / 1000.0;
        let p99 = latencies[(n*99)/100] as f64 / 1000.0;
        println!("{:>10}/s | {:>12.0}/s | {:>10.1} | {:>10.1}", target_rate, actual_rate, avg, p99);
    }
    println!("\n✅ 完成");
}

#[test]
fn test_rate_controlled_middle() {
    let fixed_size = 50;
    let fixed_text = "a".repeat(fixed_size);
    let middle_pos = 1_000_000;

    println!("\n========== 速率控制 — 中间插入 (pos={}) ==========\n", middle_pos);
    println!("{:>10} | {:>12} | {:>10} | {:>10}", "目标速率", "实际速率", "平均 μs", "P99 μs");
    println!("{}", "-".repeat(55));

    for &target_rate in &RATE_TARGETS {
        let peer_id = rand::rng().random::<u64>();
        let session = MdioSession::new(0, "", peer_id, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
        let prefill = "x".repeat(middle_pos);
        let (_, _) = session.execute_transaction(None, false, |ctx| {
            ctx.apply_mdio_op(vec![0..0], "+>", &prefill);
        });
        let mut latencies = Vec::new();
        let interval_ns = 1_000_000_000u64 / target_rate as u64;
        let start = Instant::now();
        let mut count = 0;

        while start.elapsed() < RATE_DURATION {
            let loop_start = Instant::now();
            let op_start = Instant::now();
            let (_, _) = session.execute_transaction(None, false, |ctx| {
                ctx.apply_mdio_op(vec![middle_pos..middle_pos], "+>", &fixed_text);
            });
            latencies.push(op_start.elapsed().as_nanos());
            count += 1;
            let elapsed_ns = loop_start.elapsed().as_nanos() as u64;
            if elapsed_ns < interval_ns {
                let sleep_us = (interval_ns - elapsed_ns) / 1000;
                if sleep_us >= 1 { thread::sleep(Duration::from_micros(sleep_us)); }
            }
        }

        let elapsed = start.elapsed();
        let actual_rate = count as f64 / elapsed.as_secs_f64();
        latencies.sort();
        let n = latencies.len();
        let avg = latencies.iter().sum::<u128>() as f64 / n as f64 / 1000.0;
        let p99 = latencies[(n*99)/100] as f64 / 1000.0;
        println!("{:>10}/s | {:>12.0}/s | {:>10.1} | {:>10.1}", target_rate, actual_rate, avg, p99);
    }
    println!("\n✅ 完成");
}

#[test]
fn test_engine_rate_controlled() {
    let fixed_size = 50;
    let fixed_text = "a".repeat(fixed_size);
    let engine_rates: [u32; 7] = [1000, 5000, 10000, 30000, 50000, 80000, 100000];

    println!("\n========== 纯 Engine 速率控制 (无 Cola) ==========\n");
    println!(" 模式: 末尾追加, 固定大小 {} bytes", fixed_size);
    println!("{:>10} | {:>12} | {:>10} | {:>10}", "目标速率", "实际速率", "平均 μs", "P99 μs");
    println!("{}", "-".repeat(55));

    for &target_rate in &engine_rates {
        let mut engine = MdioEngine::new("");
        let mut latencies = Vec::new();
        let batch_size = 100;
        let batch_interval_ns = 1_000_000_000u64 * batch_size as u64 / target_rate as u64;
        let mut current_len = 0;
        let start = Instant::now();
        let mut count = 0;

        while start.elapsed() < RATE_DURATION {
            let batch_start = Instant::now();
            for _ in 0..batch_size {
                let op_start = Instant::now();
                engine.edit(current_len, current_len, &fixed_text);
                latencies.push(op_start.elapsed().as_nanos());
                current_len += fixed_size;
                count += 1;
            }
            let elapsed_ns = batch_start.elapsed().as_nanos() as u64;
            if elapsed_ns < batch_interval_ns {
                thread::sleep(Duration::from_nanos(batch_interval_ns - elapsed_ns));
            }
        }

        let elapsed = start.elapsed();
        let actual_rate = count as f64 / elapsed.as_secs_f64();
        latencies.sort();
        let n = latencies.len();
        let avg = latencies.iter().sum::<u128>() as f64 / n as f64 / 1000.0;
        let p99 = latencies[(n*99)/100] as f64 / 1000.0;
        println!("{:>10}/s | {:>12.0}/s | {:>10.1} | {:>10.1}", target_rate, actual_rate, avg, p99);
    }
    println!("\n✅ 完成");
}

#[test]
fn test_engine_bulk_throughput() {
    let num_ops = 50_000;
    let fixed_size = 50;
    let fixed_text = "a".repeat(fixed_size);

    println!("\n========== 纯 Engine 批量吞吐量 ==========\n");
    println!("模式: 随机位置插入, 固定大小 {} bytes, 连续 {} 次操作", fixed_size, num_ops);

    let mut engine = MdioEngine::new("");
    let mut current_len = 0;
    let mut rng = rand::rng();
    let pos_table: Vec<f64> = (0..100).map(|_| rng.random::<f64>()).collect();

    // 预热
    println!("预热 1000 次...");
    for i in 0..1000 {
        let pos = (pos_table[i % 100] * current_len as f64) as usize;
        engine.edit(pos, pos, &fixed_text);
        current_len += fixed_size;
    }

    // 批量测试
    let start = Instant::now();
    for i in 0..num_ops {
        let pos = (pos_table[i % 100] * current_len as f64) as usize;
        engine.edit(pos, pos, &fixed_text);
        current_len += fixed_size;
    }
    let elapsed = start.elapsed();

    let throughput = num_ops as f64 / elapsed.as_secs_f64();
    let avg_latency_us = elapsed.as_micros() as f64 / num_ops as f64;
    let final_bytes = current_len;
    let data_rate_mbs = final_bytes as f64 / elapsed.as_secs_f64() / (1024.0 * 1024.0);

    println!("\n📊 性能统计:");
    println!("  总操作数: {} 次", num_ops);
    println!("  总时间: {:.3}s", elapsed.as_secs_f64());
    println!("  平均延迟: {:.2} μs/op", avg_latency_us);
    println!("  系统吞吐量: {:.0} ops/s", throughput);
    println!("  文档大小: {:.2} MB", final_bytes as f64 / (1024.0 * 1024.0));
    println!("  数据增长速率: {:.2} MB/s", data_rate_mbs);
    println!("\n✅ 完成");
}

// Sync — 单向 / 双向同步延迟

#[test]
fn test_one_way_sync_throughput() {
    let text = "Hello ";
    let iterations = 1000;

    println!("\n========== 单向同步速率 ==========\n");
    println!("迭代次数: {}", iterations);

    let mut latencies = Vec::with_capacity(iterations);
    let mut sizes = Vec::with_capacity(iterations);

    for i in 0..iterations {
        let s1 = MdioSession::new(1, text, 1, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
        let s2 = MdioSession::new(2, text, 2, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));

        let mut rng = rand::rng();
        let size = rng.random_range(10..=500);
        let text_content: String = std::iter::repeat_with(|| rng.random_range(b'a'..=b'z') as char)
            .take(size).collect();

        let start = Instant::now();
        let (_, wire) = s1.execute_transaction(None, false, |ctx| {
            ctx.apply_mdio_op(vec![6..6], "+>", &text_content);
        });
        s2.apply_update(&wire);
        let elapsed = start.elapsed();
        latencies.push(elapsed.as_nanos());
        sizes.push(size);

        if i % 100 == 0 && i > 0 { println!("  已完成 {}/{}", i, iterations); }
    }

    latencies.sort();
    let n = latencies.len();
    let avg_latency_us: f64 = latencies.iter().sum::<u128>() as f64 / n as f64 / 1000.0;
    let p50_us = latencies[n/2] as f64 / 1000.0;
    let p99_us = latencies[(n*99)/100] as f64 / 1000.0;
    let avg_size: usize = sizes.iter().sum::<usize>() / n;
    let throughput = 1_000_000_000.0 / (avg_latency_us * 1000.0);

    println!("\n📊 同步延迟统计:");
    println!("  平均: {:.1} μs", avg_latency_us);
    println!("  P50:  {:.1} μs", p50_us);
    println!("  P99:  {:.1} μs", p99_us);
    println!("\n📊 吞吐量:");
    println!("  平均文本大小: {} bytes", avg_size);
    println!("  同步速率: {:.0} ops/s", throughput);
    println!("\n✅ 完成");
}

#[test]
fn test_bidirectional_sync_throughput() {
    let text = "Hello ";
    let iterations = 500;

    println!("\n========== 双向同步速率 ==========\n");
    println!("迭代次数: {}", iterations);

    let mut latencies = Vec::with_capacity(iterations);
    let mut rng = rand::rng();

    for i in 0..iterations {
        let s1 = MdioSession::new(1, text, 1, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
        let s2 = MdioSession::new(2, text, 2, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));

        let size1 = rng.random_range(10..=100);
        let size2 = rng.random_range(10..=100);
        let text1: String = std::iter::repeat_with(|| rng.random_range(b'a'..=b'z') as char)
            .take(size1).collect();
        let text2: String = std::iter::repeat_with(|| rng.random_range(b'a'..=b'z') as char)
            .take(size2).collect();

        let start = Instant::now();
        let (_, wire1) = s1.execute_transaction(None, false, |ctx| {
            ctx.apply_mdio_op(vec![6..6], "+>", &text1);
        });
        let (_, wire2) = s2.execute_transaction(None, false, |ctx| {
            ctx.apply_mdio_op(vec![6..6], "+>", &text2);
        });
        s1.apply_update(&wire2);
        s2.apply_update(&wire1);
        let elapsed = start.elapsed();
        latencies.push(elapsed.as_nanos());

        if i % 100 == 0 && i > 0 { println!("  已完成 {}/{}", i, iterations); }
    }

    latencies.sort();
    let n = latencies.len();
    let avg_latency_us: f64 = latencies.iter().sum::<u128>() as f64 / n as f64 / 1000.0;
    let p50_us = latencies[n/2] as f64 / 1000.0;
    let p99_us = latencies[(n*99)/100] as f64 / 1000.0;
    let throughput = 1_000_000_000.0 / (avg_latency_us * 1000.0);

    println!("\n📊 双向同步延迟统计:");
    println!("  平均: {:.1} μs", avg_latency_us);
    println!("  P50:  {:.1} μs", p50_us);
    println!("  P99:  {:.1} μs", p99_us);
    println!("\n📊 吞吐量:");
    println!("  同步速率: {:.0} ops/s", throughput);
    println!("\n✅ 完成");
}

// Parallel — 跨文档并行扩展

#[test]
fn bench_parallel_scaling() {
    let doc_size = 50_000;
    let ops_per_session = 500;
    let edit_text = "X".repeat(10);

    println!("\n========== 跨文档并行度 ==========\n");
    println!("文档: {:.0}KB x N, 每 session {} 次 append",
        doc_size as f64 / 1024.0, ops_per_session);

    let thread_counts: [usize; 5] = [1, 2, 4, 8, 16];

    // 串行基线
    let baseline = {
        let session = MdioSession::new(0, &build_simple_doc(doc_size), 1, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
        let t0 = Instant::now();
        for i in 0..ops_per_session {
            let txt = edit_text.clone();
            let (_, _) = session.execute_transaction(None, false, |ctx| {
                ctx.apply_mdio_op(vec![(i * 10)..(i * 10)], "+>", &txt);
            });
        }
        t0.elapsed()
    };

    println!("\n{:>6} | {:>10} | {:>12} | {:>8}",
        "线程", "wall time", "吞吐 (ops/s)", "加速比");
    println!("{}", "-".repeat(45));
    println!("{:>6} | {:>8.2?} | {:>10.0} | {:>6.1?}x  (基线)",
        "1", baseline, ops_per_session as f64 / baseline.as_secs_f64(), 1.0);

    for &n in &thread_counts {
        if n == 1 { continue; }
        let et = edit_text.clone();
        let t0 = Instant::now();
        std::thread::scope(|s| {
            for tid in 0..n {
                let text = build_simple_doc(doc_size);
                let txt = et.clone();
                s.spawn(move || {
                    let session = MdioSession::new(tid as u32, &text, (tid + 1) as u64, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
                    for i in 0..ops_per_session {
                        let t = txt.clone();
                        let (_, _) = session.execute_transaction(None, false, |ctx| {
                            ctx.apply_mdio_op(vec![(i * 10)..(i * 10)], "+>", &t);
                        });
                    }
                });
            }
        });
        let wall = t0.elapsed();
        let total_ops = n * ops_per_session;
        let throughput = total_ops as f64 / wall.as_secs_f64();
        let speedup = baseline.as_secs_f64() / wall.as_secs_f64() * n as f64;
        println!("{:>6} | {:>8.2?} | {:>10.0} | {:>6.1?}x", n, wall, throughput, speedup);
    }

    println!("\n解读:");
    println!("  加速比接近线程数 → DashMap + per-session Mutex 近线性扩展");
    println!("  加速比停滞     → 撞到 CPU 物理核心数");
}
