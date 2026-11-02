//! Dump every exhaustive_v2 document up to a line bound.
//!
//! cargo run -p mdio_core --features trinity-obs --example exhaustive_v2_dump -- [max_lines] [out_dir]

#[cfg(feature = "trinity-obs")]
fn main() {
    use mdio_core::trinity::exhaustive_v2::enumerate_docs;
    use std::path::PathBuf;

    let mut args = std::env::args().skip(1);
    let max_lines = args
        .next()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(4);
    let out = args.next().map(PathBuf::from);
    let e = enumerate_docs(max_lines, None);
    if let Some(dir) = out {
        std::fs::create_dir_all(&dir).expect("create out dir");
        let mut n = 0usize;
        for d in &e.docs {
            if d.text.trim().is_empty() {
                continue;
            }
            std::fs::write(dir.join(format!("doc_{n:06}.md")), &d.text).expect("write doc");
            n += 1;
        }
        std::fs::write(
            dir.join("MANIFEST"),
            format!(
                "docs={n}\npaths={}\nmax_lines={}\ncapped={}\n",
                e.docs.len(),
                e.max_lines,
                e.capped
            ),
        )
        .expect("write manifest");
    }
    println!(
        "paths={} max_lines={} capped={}",
        e.docs.len(),
        e.max_lines,
        e.capped
    );
}

#[cfg(not(feature = "trinity-obs"))]
fn main() {
    eprintln!("build with --features trinity-obs");
}
