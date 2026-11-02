// BackendConfig 测试

use crate::backend::{BackendConfig, Scheme};

fn with_runtime<F: FnOnce()>(f: F) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all().build().unwrap();
    let _guard = rt.enter();
    f();
}

// ── storage_key ──

#[test]
fn storage_key_fs_join() {
    let cfg = BackendConfig::resolve(Scheme::Fs);
    assert_eq!(cfg.storage_key("/tmp/root", "docs/a.md"), "/tmp/root/docs/a.md");
}

#[test]
fn storage_key_fs_root_trailing_slash() {
    let cfg = BackendConfig::resolve(Scheme::Fs);
    assert_eq!(cfg.storage_key("/tmp/root/", "/docs/a.md"), "/tmp/root/docs/a.md");
}

#[test]
fn storage_key_memory_join() {
    let cfg = BackendConfig::resolve(Scheme::Memory);
    assert_eq!(cfg.storage_key("/mem", "docs/a.md"), "/mem/docs/a.md");
}

// ── contains ──

#[test]
fn contains_fs_subpath() {
    let cfg = BackendConfig::resolve(Scheme::Fs);
    assert!(cfg.contains("/tmp/root", "/tmp/root/docs"));
}

#[test]
fn contains_fs_exact_match() {
    let cfg = BackendConfig::resolve(Scheme::Fs);
    assert!(cfg.contains("/tmp/root", "/tmp/root"));
}

#[test]
fn contains_fs_trailing_slash_guard() {
    let cfg = BackendConfig::resolve(Scheme::Fs);
    assert!(!cfg.contains("/tmp/root", "/tmp/root_backup"));
}

#[test]
fn contains_fs_not_child() {
    let cfg = BackendConfig::resolve(Scheme::Fs);
    assert!(!cfg.contains("/tmp/root", "/tmp/other"));
}

#[test]
fn contains_memory_always_true() {
    let cfg = BackendConfig::resolve(Scheme::Memory);
    assert!(cfg.contains("/mem", "/anything/else"));
}

// ── create_operator ──

#[test]
fn create_operator_fs() {
    with_runtime(|| {
        let cfg = BackendConfig::resolve(Scheme::Fs);
        let op = cfg.create_operator("/tmp/test_fs_op").unwrap();
        op.write("hello.md", "# Hello".as_bytes()).unwrap();
        let data = op.read("hello.md").unwrap().to_vec();
        assert_eq!(data, b"# Hello");
    });
}

#[test]
fn create_operator_memory() {
    with_runtime(|| {
        let cfg = BackendConfig::resolve(Scheme::Memory);
        let op = cfg.create_operator("/mem").unwrap();
        op.write("test.md", "# Mem".as_bytes()).unwrap();
        let data = op.read("test.md").unwrap().to_vec();
        assert_eq!(data, b"# Mem");
    });
}

// ── defaults / resolve ──

#[test]
fn defaults_table() {
    let defaults = BackendConfig::defaults();
    assert_eq!(defaults.len(), 2);
    assert_eq!(defaults[0].scheme, Scheme::Fs);
    assert_eq!(defaults[1].scheme, Scheme::Memory);
    assert_eq!(defaults[0].checkpoint_interval_secs, 300);
    assert_eq!(defaults[1].checkpoint_interval_secs, 0);
    assert!(!defaults[1].persistent);
}

#[test]
fn defaults_persistent_matches_feature() {
    let defaults = BackendConfig::defaults();
    let expected = cfg!(feature = "persistence");
    assert_eq!(defaults[0].persistent, expected);
}

// ── /mem：L7 的 Memory scheme 后端（无 L3 外部根）──

/// `/mem`＝纯内存 store：后端配置**来自既有 `BackendConfig` 表**（scheme=Memory、persistent=false）、
/// `ProjectStore("/mem")` 必须能构造（「指向 `/` 的 FsBackend」在布局名必填后不成立）、
/// 文档走正常路径、且 `external_root` 为空（没有任何 L3 磁盘对应物）。
#[test]
fn mem_store_uses_memory_backend() {
    with_runtime(|| {
        // 后端＝既有轮子的一个配置：`BackendConfig::resolve(Scheme::Memory)` + memlayer 算子
        let be = crate::backend::MemoryBackend::new("/").unwrap();
        assert_eq!(be.cfg().scheme, Scheme::Memory);
        assert!(!be.cfg().persistent, "Memory scheme 不持久化（表内定，不各处硬写）");

        let store = crate::store::ProjectStore::new_with_layout(
            "/mem", Some("wt".to_string()), Some(".wal".to_string()),
        ).expect("/mem 应能构造（走 MemoryBackend）");
        // 文档走正常路径：建 → 解 → 开会话 → 编辑 → 读回
        store.create_path("genesis", "/m.md", true, false).ok();
        let id = store.resolve_path("genesis", "/m.md", false).expect("resolve");
        let sess = store.open_session(id).expect("open");
        sess.execute_transaction(None, true, |ctx| { ctx.state.engine.edit(0, 0, "# MEM\n"); });
        assert_eq!(sess.get_engine_text(), "# MEM\n");
        // 无 L3 外部根：external_root 必须是 None（指向 "/" 会去碰真实根）
        let t = store.tree.read();
        assert_eq!(t.nodes.get("genesis").unwrap().external_root, None,
            "/mem 不应有 external_root（无 L3 根 ⇒ 纯内存）");
    });
}
