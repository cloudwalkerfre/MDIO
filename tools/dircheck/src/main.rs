//! mdio-dircheck CLI。
//! 用法:
//!   mdio-dircheck <src_dir>                    打印边集与 outlets（扫描模式）
//!   mdio-dircheck check <src_dir> [--baseline <file>]   门禁模式（基线棘轮，红 = 退出码 1）

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("check") {
        let dir = args.get(2).cloned().unwrap_or_else(|| "mdio_core/src".to_string());
        let mut baseline_path = "tools/dircheck/baseline.txt".to_string();
        if let Some(i) = args.iter().position(|a| a == "--baseline") {
            if let Some(p) = args.get(i + 1) { baseline_path = p.clone(); }
        }
        let v = mdio_dircheck::gate_violations(Path::new(&dir));
        let baseline = std::fs::read_to_string(&baseline_path).unwrap_or_else(|_| String::new());
        match mdio_dircheck::compare_violations(&v, &baseline) {
            Ok(()) => println!("check-core-layers OK: {} 条违规，全部在基线", v.len()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }
    let dir = args.get(1).cloned().unwrap_or_else(|| "mdio_core/src".to_string());
    let (modules, edges) = mdio_dircheck::scan_dir(Path::new(&dir));
    let domain_src = std::fs::read_to_string(Path::new(&dir).join("domain.rs")).expect("domain.rs");
    let outlets = mdio_dircheck::parse_outlets(&domain_src);

    println!("# modules {}", modules.len());
    println!("# edges");
    for (from, to, kind, test) in &edges {
        println!("edge\t{from}->{to}\t{kind}\t{}", if *test { "test" } else { "" });
    }
    println!("# outlets");
    for (name, outlet) in &outlets {
        println!("outlet\t{name}\t{outlet}");
    }
}
