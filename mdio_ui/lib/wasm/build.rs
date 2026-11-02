use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // 如果目标平台是wasm32，则确保wasm-bindgen-cli已安装
    let target = env::var("TARGET").unwrap_or_default();
    if target.contains("wasm32") {
        println!("cargo:rerun-if-changed=src/lib.rs");
        println!("cargo:rerun-if-changed=src/protocol.rs");
        println!("cargo:rerun-if-changed=src/ringbuf.rs");

        // 确保输出目录存在
        let out_dir = env::var("OUT_DIR").unwrap();
        let wasm_dir = PathBuf::from("../web/wasm");

        if !wasm_dir.exists() {
            fs::create_dir_all(&wasm_dir).expect("无法创建wasm目录");
        }

        println!("cargo:warning=构建WASM目标: {}", target);
        println!("cargo:warning=输出目录: {}", out_dir);

        // 为WebTransport启用unstable APIs（关键修复）
        println!("cargo:rustc-cfg=web_sys_unstable_apis");

        // 添加wasm-bindgen构建标志（wasm32使用rust-lld，不需要-Wl,前缀）
        println!("cargo:rustc-cdylib-link-arg=--no-entry");
    }

    // 重新运行构建脚本的条件
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
}