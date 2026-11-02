// VFS 路径处理单元测试
//
// 覆盖：normalize_vpath、resolve_vpath、sub_mount 派发、
// 多 worktree 嵌套路径、from_root、scheme 查询等。

use super::*;
use crate::backend::{BackendConfig, Scheme};
use crate::utils::normalize_vpath;

// ── normalize_vpath ───────────────────────────────────────────────

#[test]
fn normalize_vpath_leading_slash() {
    assert_eq!(normalize_vpath("/docs/a.md"), "/docs/a.md");
}

#[test]
fn normalize_vpath_no_leading_slash() {
    assert_eq!(normalize_vpath("docs/a.md"), "/docs/a.md");
}

#[test]
fn normalize_vpath_double_dots() {
    // /docs/../other.md → /other.md
    assert_eq!(normalize_vpath("/docs/../other.md"), "/other.md");
}

#[test]
fn normalize_vpath_multiple_double_dots() {
    assert_eq!(normalize_vpath("/a/b/../../c.md"), "/c.md");
}

#[test]
fn normalize_vpath_single_dot() {
    assert_eq!(normalize_vpath("/./docs/./a.md"), "/docs/a.md");
}

#[test]
fn normalize_vpath_traversal_attempt() {
    // /../../../etc/passwd → /etc/passwd (no escape above root)
    assert_eq!(normalize_vpath("/../../../etc/passwd"), "/etc/passwd");
}

#[test]
fn normalize_vpath_root_only() {
    assert_eq!(normalize_vpath("/"), "/");
    assert_eq!(normalize_vpath(""), "/");
}

#[test]
fn normalize_vpath_consecutive_slashes() {
    assert_eq!(normalize_vpath("/a//b///c.md"), "/a/b/c.md");
}

// ── VfsRouter::from_root ──────────────────────────────────────────

#[test]
fn from_root_creates_valid_router() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Memory);
    let router = VfsRouter::from_root("/mem/test", cfg).unwrap();
    assert_eq!(router.root_point().real_path, "/mem/test");
    assert_eq!(router.scheme(), Scheme::Memory);
    assert!(router.parent_id.is_none());
}

#[test]
fn from_root_fs_scheme() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Fs);
    let router = VfsRouter::from_root("/tmp/test_vfs", cfg).unwrap();
    assert_eq!(router.scheme(), Scheme::Fs);
}

// ── resolve_vpath / route with sub_mounts ─────────────────────────

#[test]
fn route_root_path() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Memory);
    let mut router = VfsRouter::from_root("/mem/root", cfg.clone()).unwrap();
    router.inject_sys_mount("/mem/root/.mds").unwrap();

    // Root path → root_mount
    let (_, _, internal) = router.route("/docs/a.md");
    assert_eq!(internal, "docs/a.md");
}

#[test]
fn route_sub_mount_path() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Memory);
    let mut router = VfsRouter::from_root("/mem/root", cfg.clone()).unwrap();
    router.inject_sys_mount("/mem/root/.mds").unwrap();

    // .mds path → .mds sub_mount
    let (_, mode, internal) = router.route("/.mds/env.md");
    assert_eq!(internal, "env.md");
    assert_eq!(*mode, MountMode::RW);
}

#[test]
fn route_sub_mount_preserves_worktree_path() {
    // Worktree path must survive route() for correct ArenaMeta storage.
    // route() strips mount prefix, but the stored worktree_path is the ORIGINAL.
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Memory);
    let mut router = VfsRouter::from_root("/mem/root", cfg.clone()).unwrap();
    router.inject_sys_mount("/mem/root/.mds").unwrap();

    // Store: worktree_path = "/.mds/env.md"
    // route("/.mds/env.md") → internal = "env.md" (correct for sub_mount I/O)
    // But ArenaMeta stores "/.mds/env.md" — re-routing from ArenaMeta works
    let (_op, _mode, internal) = router.route("/.mds/env.md");
    assert_eq!(internal, "env.md");

    // Re-route stored worktree_path — gives same result
    let (_op2, _mode2, internal2) = router.route("/.mds/env.md");
    assert_eq!(internal2, "env.md");
}

// ── get_real_absolute_path with backend ────────────────────────────

#[test]
fn get_real_absolute_path_fs_backend() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Fs);
    let router = VfsRouter::from_root("/tmp/myproject", cfg).unwrap();
    let abs = router.get_real_absolute_path("/docs/readme.md");
    assert_eq!(abs, "/tmp/myproject/docs/readme.md");
}

#[test]
fn get_real_absolute_path_memory_backend() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Memory);
    let router = VfsRouter::from_root("/mem", cfg).unwrap();
    let abs = router.get_real_absolute_path("/data/file.md");
    assert_eq!(abs, "/mem/data/file.md");
}

// ── Multi-worktree nested paths ────────────────────────────────────

#[test]
fn nested_worktree_paths_independent() {
    // Verify that different wt_uids have independent path resolution
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Memory);

    let router0 = VfsRouter::from_root("/mem/genesis", cfg.clone()).unwrap();
    let router1 = VfsRouter::from_root("/mem/project1", cfg.clone()).unwrap();

    let abs0 = router0.get_real_absolute_path("/docs/a.md");
    let abs1 = router1.get_real_absolute_path("/docs/a.md");

    assert_eq!(abs0, "/mem/genesis/docs/a.md");
    assert_eq!(abs1, "/mem/project1/docs/a.md");
    // Same virtual path → different physical paths on different wt_uids
}

#[test]
fn sub_mount_info_system_vs_user() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Memory);
    let mut router = VfsRouter::from_root("/mem/root", cfg.clone()).unwrap();
    router.inject_sys_mount("/mem/root/.mds").unwrap();

    let info = router.sub_mount_info();
    let mdio = info.iter().find(|(n, _, _)| n == ".mds").unwrap();
    assert_eq!(mdio.2, MountKind::System);
}

// ── scheme_from_src ────────────────────────────────────────────────

#[test]
fn from_root_memory_is_infallible() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _guard = rt.enter();
    let cfg = BackendConfig::resolve(Scheme::Memory);
    let router = VfsRouter::from_root("/mem/test", cfg);
    assert!(router.is_ok());
    assert_eq!(router.unwrap().scheme(), Scheme::Memory);
}
