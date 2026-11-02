//! tests/stress_test_session.rs
//!
//! 压力测试与并发一致性验证 (Cola 0.5.1 + Rand 0.9 修复版)

use std::time::Instant;
use std::sync::Arc;
use dashmap::DashSet;
use rand::Rng;
use mdio_core::session::MdioSession;
use mdio_core::op::WriteEdit;

/// 测试：高频小更新
#[test]
fn test_high_frequency_small_updates() {
    let num_updates = 10000;
    let update_size = 10;





    let peer_id = rand::rng().random::<u64>();
    let session = MdioSession::new(0, "", peer_id, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
    let mut rng = rand::rng();
    let start = Instant::now();

    for i in 0..num_updates {
        let text: String = std::iter::repeat_with(|| rng.random_range(b'a'..=b'z') as char)
            .take(update_size - 1)
            .collect();
        let pos = i * (update_size - 1);
        let _ = session.execute_transaction(None, false, |ctx| {
            ctx.apply_mdio_op(vec![pos..pos], "+>", &text);
        });
    }

    let elapsed = start.elapsed();




}

/// 测试：验证数据一致性
#[test]
fn test_data_consistency() {


    let peer_id = rand::rng().random::<u64>();
    let session = MdioSession::new(0, "initial", peer_id, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
    let mut rng = rand::rng();

    // 多次写入 - 每次都在末尾追加
    let mut current_len = 7; // "initial" 长度
    for i in 0..100 {
        let text = format!("- item {}\n", i);
        let pos = current_len;
        let _ = session.execute_transaction(None, false, |ctx| {
            ctx.apply_mdio_op(vec![pos..pos], "+>", &text);
        });
        current_len += text.len();
    }

    // 验证
    let engine_text = session.get_engine_text();

    assert!(engine_text.starts_with("initial"), "Engine 开头不正确");
    assert_eq!(engine_text.len(), current_len, "长度不一致");


}

/// 测试：edit_batch 批量写入
#[test]
fn test_edit_batch() {
    let batch_size = 1000;




    let peer_id = 1;
    let session = MdioSession::new(0, "Start", peer_id, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()));
    let start = Instant::now();

    // 构建批量操作 — 预先计算位置
    let mut ops = Vec::new();
    let mut pos = session.get_engine_text().len();
    for i in 0..batch_size {
        ops.push(mdio_core::op::WriteEdit { range: pos..pos, text: format!("batch_{}_", i) });
        pos += format!("batch_{}_", i).len();
    }

    let _ = session.execute_transaction(None, false, |ctx| {
        ctx.apply_mdio_op_multi(ops, "+>");
    });

    let elapsed = start.elapsed();


}

/// 测试：协作模拟 - 双端同步
/// 验证 Cola CRDT 的收敛性：两个并发插入最终应该收敛到相同状态
///
/// Cola 的工作原理：
/// 1. 第一个 peer 创建文档 (Genesis)
/// 2. 第二个 peer 使用 from_checkpoint Fork 加入（继承历史）
/// 3. 每个 peer 做本地操作
/// 4. 交换操作，收敛到相同状态
#[test]
fn test_collaborative_sync() {


    let text = "Hello ";
    let alive = Arc::new(DashSet::new());

    // 1. Peer 1 创建文档 (Genesis)
    let s1 = MdioSession::new(0, text, 1, "".into(), None, None, None, alive.clone(), Arc::new(mdio_core::session::DetectClock::new()));

    // 2. Peer 2 使用 from_checkpoint Fork 加入（正确方式）
    let checkpoint = s1.get_checkpoint();
    let s2 = MdioSession::from_checkpoint(0, &checkpoint, 2, text, "".into(), None, None, None, alive, Arc::new(mdio_core::session::DetectClock::new())).expect("from_checkpoint");



    // 3. 并发操作
    let (_, wire1) = s1.execute_transaction(None, false, |ctx| {
        ctx.apply_mdio_op(vec![6..6], "+>", "World");
    });


    let (_, wire2) = s2.execute_transaction(None, false, |ctx| {
        ctx.apply_mdio_op(vec![6..6], "+>", "Rust");
    });


    // 4. 交换操作

    s1.apply_update(&wire2);



    s2.apply_update(&wire1);


    // 5. 最终一致性验证
    let t1 = s1.get_engine_text();
    let t2 = s2.get_engine_text();

    if t1 != t2 {



        panic!("CRDT Divergence!");
    }


    assert!(t1.contains("World") && t1.contains("Rust"),
            "结果应包含两个插入: '{}'", t1);
}

/// 测试：单向操作同步（确认 checkpoint 机制工作）
#[test]
fn test_one_way_sync() {


    let text = "Hello ";
    let alive = Arc::new(DashSet::new());

    // s1 是初始状态 (Genesis)
    let s1 = MdioSession::new(0, text, 1, "".into(), None, None, None, alive.clone(), Arc::new(mdio_core::session::DetectClock::new()));

    // s2 使用 from_checkpoint Fork 加入（正确方式）
    let checkpoint = s1.get_checkpoint();
    let s2 = MdioSession::from_checkpoint(0, &checkpoint, 2, text, "".into(), None, None, None, alive, Arc::new(mdio_core::session::DetectClock::new())).expect("from_checkpoint");

    // s1 做操作
    let (_, wire1) = s1.execute_transaction(None, false, |ctx| {
        ctx.apply_mdio_op(vec![6..6], "+>", "World");
    });


    // s2 应用 s1 的操作（不需要再次加载 checkpoint，因为 Fork 已继承历史）
    s2.apply_update(&wire1);


    assert_eq!(s1.get_engine_text(), s2.get_engine_text());

}
