// g7_entry_matrix —— G7 崩溃门（按入口）：四个入口都不 panic，且输出合法。
// 输入：`- k:`（末字节是空值条目的冒号、无尾换行，4 字节）。
// 判据（两个变量写清）：四个入口 —— ① 直调内容面 ② 生产信封（cf / hs / bf 三种投影）
// ③ 写面 —— 都不 panic，且输出合法（信封可解析且 nt == "root"）。
// 用 catch_unwind 把 panic 变成可断言的结果，不用 #[should_panic]。
use mdio_core::block::{BlockProj, BlockType};
use mdio_core::projection::State;
use mdio_core::topology::ProjectionMode;
use serde_json::Value;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// 文档末字节是空值条目的冒号、无尾换行（4 字节）
const DOC: &str = "- k:";

fn quiet<F: FnOnce() -> R + std::panic::UnwindSafe, R>(f: F) -> Result<R, String> {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = catch_unwind(f);
    std::panic::set_hook(prev);
    r.map_err(|e| {
        if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else if let Some(s) = e.downcast_ref::<&str>() {
            s.to_string()
        } else {
            "(non-string panic)".to_string()
        }
    })
}

#[test]
fn g7_entry_matrix_no_panic() {
    println!("== G7 输入 doc = {:?}（{} 字节，无尾换行）==", DOC, DOC.len());
    let mut fails: Vec<String> = Vec::new();

    // 入口 1：直调内容面（NodeProj::list_content 的键型臂）
    {
        let mut st = State::new(DOC);
        let snap = st.snapshot();
        let node = snap.build_node(0, 1, BlockType::Bullet, false);
        match quiet(AssertUnwindSafe(|| {
            snap.read_block_proj(&node, BlockProj::Content)
        })) {
            Ok(v) => println!("[1 direct content      ] Ok({:?})", v),
            Err(p) => {
                println!("[1 direct content      ] PANIC: {}", p);
                fails.push(format!("direct content: {}", p));
            }
        }
    }

    // 入口 2/3/4：生产信封 cf / hs / bf
    for (name, mode) in [
        ("cf", ProjectionMode::Cf),
        ("hs", ProjectionMode::Hs),
        ("bf", ProjectionMode::Bf),
    ] {
        let mut st = State::new(DOC);
        let snap = st.snapshot();
        match quiet(AssertUnwindSafe(|| snap.to_ast_object_envelope(mode, None))) {
            Ok(s) => match serde_json::from_str::<Value>(&s) {
                Ok(v) if v.get("nt").and_then(|x| x.as_str()) == Some("root") => {
                    println!("[2 envelope {}          ] Ok({} bytes, nt=root)", name, s.len())
                }
                Ok(_) => {
                    println!("[2 envelope {}          ] 信封非法：nt != root", name);
                    fails.push(format!("envelope {}: nt != root", name));
                }
                Err(e) => {
                    println!("[2 envelope {}          ] 信封非法 JSON: {}", name, e);
                    fails.push(format!("envelope {}: bad json {}", name, e));
                }
            },
            Err(p) => {
                println!("[2 envelope {}          ] PANIC: {}", name, p);
                fails.push(format!("envelope {}: {}", name, p));
            }
        }
    }

    // 入口 5：写面（compute_write_from_node 的几何快路径 tag 分支）
    {
        let mut st = State::new(DOC);
        let snap = st.snapshot();
        let node = snap.build_node(0, 1, BlockType::Bullet, false);
        match quiet(AssertUnwindSafe(|| {
            snap.compute_write_from_node(&node, "content", "X")
        })) {
            Ok(Ok((r, t))) => println!("[3 write content       ] Ok(range {}..{} text {:?})", r.start, r.end, t),
            Ok(Err(e)) => println!("[3 write content       ] Err({})", e),
            Err(p) => {
                println!("[3 write content       ] PANIC: {}", p);
                fails.push(format!("write content: {}", p));
            }
        }
    }

    assert!(
        fails.is_empty(),
        "G7 红：{} 个入口 panic —— {}",
        fails.len(),
        fails.join(" | ")
    );
}
