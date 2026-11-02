//! mdio-roundtrip —— MD ⟷ 产物 往返验证工具
//!
//! 用法: mdio-roundtrip <file.md> [file2.md ...]   （构建需 --features trinity-obs）
//! 输出: 每文件 E1/(CL)/(CC) 三判官 + 规范形分叉报告（trinity text_shape_report）。

use mdio_core::projection::State;
use mdio_core::trinity::entries::edge1_report;
use mdio_core::trinity::obs::RealBundle;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("用法: mdio-roundtrip <file.md> [...]");
        std::process::exit(2);
    }
    let mut total = 0usize;
    for path in &args {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => { eprintln!("{path}: 读取失败 {e}"); continue; }
        };
        let mut st = State::new(&text);
        let snap = st.snapshot();
        let bundle = RealBundle::from_snapshot(&snap);
        let (report, divs) = edge1_report(&bundle);
        eprintln!("[trinity] {path}: {} 行——E1={} CL={} CC={}",
            snap.line_len(), report.bits.e1, report.cl, report.cc);
        for d in divs {
            total += 1;
            eprintln!("  行{} [{:?}] {:?}: {:?}", d.line, d.kind, d.ft, d.evidence);
        }
    }
    if total > 0 {
        std::process::exit(1);
    }
}
