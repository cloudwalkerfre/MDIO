//! MDIO_PERSIST 缺省(未设置)= false 时,文件系统操作零磁盘动作。
//! 独立 integration 文件 = 独立测试进程(env 隔离——与 persist.rs 的 set_var 互不干扰)。

use mdio_core::store::ProjectStore;
use std::path::PathBuf;

struct TDir { path: PathBuf }
impl TDir {
    fn new(prefix: &str) -> Self {
        let dir = PathBuf::from(format!("/tmp/__mdio_apdis_{}_{}", prefix, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TDir { path: dir }
    }
    fn s(&self) -> String { self.path.to_string_lossy().to_string() }
}
impl Drop for TDir {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); }
}

#[test]
fn persist_disabled_no_disk_side_effects() {
    std::env::remove_var("MDIO_PERSIST"); // 显式隔离(与宿主环境无关)
    let tmp = TDir::new("ap_dis");
    let store = ProjectStore::new_with_layout(
        &tmp.s(),
        Some("wt".to_string()),
        Some(".wal".to_string()),
    )
    .unwrap();
    store.create_path("genesis", "/off_m.md", true, false).ok();
    assert!(!tmp.path.join("wt/off_m.md").exists() && !tmp.path.join("off_m.md").exists(),
        "缺省(无 MDIO_PERSIST)时 create 不应写盘");
    store.create_path("genesis", "/off_dir", true, false).ok();
    assert!(!tmp.path.join("wt/off_dir").exists() && !tmp.path.join("off_dir").exists(),
        "缺省时 mkdir 不应写盘");
    store.remove_path("genesis", "/off_m.md", false, true, false).ok();
    drop(store);
}
