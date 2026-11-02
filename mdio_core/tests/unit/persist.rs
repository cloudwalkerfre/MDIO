// 持久化端到端测试（仅 feature=persistence 时编译）

use super::super::*;
use std::path::PathBuf;

// ── TempDir guard ──────────────────────────────────────────────

struct TDir { path: PathBuf }
impl TDir {
    fn new(prefix: &str) -> Self {
        let dir = PathBuf::from(format!("/tmp/__mdio_ptest_{}_{}", prefix, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TDir { path: dir }
    }
    fn s(&self) -> String { self.path.to_string_lossy().to_string() }
}
impl Drop for TDir {
    fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.path); }
}

fn mk_store(root: &str) -> ProjectStore {
    // AP §5:persist 用例需要持久化开启(MDIO_PERSIST=1)——进程级共享同值"1",
    // 并行线程无竞态(无用例需要"0";需要假值的用例放独立 integration 文件=进程隔离)。
    std::env::set_var("MDIO_PERSIST", "1");
    // 三根分层：布局名由 host 给（core 无默认、无扁平兜底）。与生产同形：
    // 文档根 = `<根>/wt`，运行态根 = `<根>`，WAL 目录 = `<根>/.wal`。
    ProjectStore::new_with_layout(root, Some("wt".to_string()), Some(".wal".to_string())).unwrap()
}

/// 分层布局下的**文档根**：`<storage_root>/wt`。
fn doc_root(tdir: &TDir) -> PathBuf { tdir.path.join("wt") }

/// 分层布局下的文档磁盘路径：`<storage_root>/wt/<rel>`。
fn doc(tdir: &TDir, rel: &str) -> PathBuf { doc_root(tdir).join(rel) }

fn mk_file(store: &ProjectStore, vpath: &str) -> (u32, Arc<MdioSession>) {
    let vpath = normalize_vpath(vpath);
    let doc_id = store.resolve_path("genesis", &vpath, true).unwrap_or_else(|_| {
        // 文件不存在——create_path 建空文件(get_router 不存在——WT WT VFS 后
        // 建文件走 ProjectStore::create_path)
        store.create_path("genesis", &vpath, false, false).ok();
        store.resolve_path("genesis", &vpath, false).unwrap()
    });
    (doc_id, store.open_session(doc_id).unwrap())
}

fn read_file(tdir: &TDir, name: &str) -> String {
    String::from_utf8(std::fs::read(doc(tdir, name)).unwrap()).unwrap()
}

#[test]
fn flush_one_writes_and_clears_dirty() {
    let tmp = TDir::new("f1");
    let store = mk_store(&tmp.s());
    let (doc_id, session) = mk_file(&store, "/a.md");

    session.execute_transaction(None, true, |ctx| {
        ctx.state.engine.edit(0, 0, "# OK\n");
    });

    flush_one(doc_id, &session, &store.arena, &store.vfs_op, &store.tree, &store.backends);

    assert_eq!(read_file(&tmp, "a.md"), "# OK\n");
    assert!(!session.dirty.load(Ordering::Relaxed));
    drop(store);
}

#[test]
fn multiple_flush_cycles() {
    let tmp = TDir::new("f2");
    let store = mk_store(&tmp.s());
    let (doc_id, session) = mk_file(&store, "/c.md");

    session.execute_transaction(None, true, |ctx| ctx.state.engine.edit(0, 0, "# R1\n"));
    flush_one(doc_id, &session, &store.arena, &store.vfs_op, &store.tree, &store.backends);
    assert_eq!(read_file(&tmp, "c.md"), "# R1\n");

    session.execute_transaction(None, true, |ctx| ctx.state.engine.edit(5, 5, "- k: r2\n"));
    flush_one(doc_id, &session, &store.arena, &store.vfs_op, &store.tree, &store.backends);
    assert_eq!(read_file(&tmp, "c.md"), "# R1\n- k: r2\n");

    drop(store);
}

#[test]
fn crash_recovery_replays_wal() {
    let tmp = TDir::new("f4");
    let store = mk_store(&tmp.s());
    let (doc_id, session) = mk_file(&store, "/r.md");

    session.execute_transaction(None, true, |ctx| ctx.state.engine.edit(0, 0, "# OLD\n"));
    flush_one(doc_id, &session, &store.arena, &store.vfs_op, &store.tree, &store.backends);
    let sk = session.storage_key.clone();
    drop(session);
    drop(store);

    // Append a WAL entry (simulating crash before next flush)
    let wal_path = tmp.path.join(FsBackend::wal_path(&sk));
    let existing = std::fs::read(&wal_path).unwrap_or_default();
    let mut wire = existing;
    append_edit_to_wal(&mut wire, 6, 6, "- v: recovered\n");
    std::fs::write(&wal_path, &wire).unwrap();

    // Reopen → recovery replays WAL
    let store2 = mk_store(&tmp.s());
    let doc_id2 = store2.resolve_path("genesis", "/r.md", false).unwrap();
    let session2 = store2.open_session(doc_id2).unwrap();
    assert_eq!(session2.get_engine_text(), "# OLD\n- v: recovered\n");
    drop(store2);
}

#[test]
fn new_file_no_base_replays_all() {
    let tmp = TDir::new("f5");
    // 磁盘先建空文件——store new 时 backend.load 吸进 memory(vfs_op 是
    // Memory——load 只在 new 执行一次——顺序必须：先磁盘、后 store)
    std::fs::create_dir_all(doc_root(&tmp)).unwrap();
    std::fs::write(doc(&tmp, "n.md"), "").unwrap();
    let store = mk_store(&tmp.s());
    let doc_id = store.resolve_path("genesis", "/n.md", false).unwrap();
    let session = store.open_session(doc_id).unwrap();
    let sk = session.storage_key.clone();
    drop(session);
    drop(store);

    let wal_path = tmp.path.join(FsBackend::wal_path(&sk));
    std::fs::create_dir_all(wal_path.parent().unwrap()).unwrap();
    let mut wire = Vec::new();
    append_edit_to_wal(&mut wire, 0, 0, "# NEW\n");
    std::fs::write(&wal_path, &wire).unwrap();

    let store2 = mk_store(&tmp.s());
    let doc_id2 = store2.resolve_path("genesis", "/n.md", false).unwrap();
    let session2 = store2.open_session(doc_id2).unwrap();
    assert_eq!(session2.get_engine_text(), "# NEW\n");
    drop(store2);
}

#[test]
fn missing_file_graceful() {
    let tmp = TDir::new("f6");
    let store = mk_store(&tmp.s());
    assert!(store.resolve_path("genesis", "/nope.md", false).is_err());
    drop(store);
}

// ── helpers ────────────────────────────────────────────────────

fn append_edit_to_wal(wire: &mut Vec<u8>, start: usize, end: usize, text: &str) {
    let rec = EditRecord { start, end, text: text.into() };
    let entry = WalEntry::Edit(rec);
    let payload = bincode::serde::encode_to_vec(&entry, bincode_cfg()).unwrap();
    let len = payload.len() as u32;
    let mut crc = crc32fast::Hasher::new();
    crc.update(&payload);
    wire.extend_from_slice(&len.to_le_bytes());
    wire.extend_from_slice(&WAL_MAGIC.to_le_bytes());
    wire.extend_from_slice(&crc.finalize().to_le_bytes());
    wire.extend_from_slice(&payload);
}

// AP §5 新增:文件系统操作写透(镜像/删除/重置)——仅 feature=persistence 编译

#[test]
fn create_file_mirrors_to_disk() {
    let tmp = TDir::new("ap_cf");
    let store = mk_store(&tmp.s());
    store.create_path("genesis", "/doc_m.md", true, false).ok();
    // 写透:磁盘空文件即现(同步)
    assert!(doc(&tmp, "doc_m.md").exists(), "mdio create 后磁盘应有文件");
    assert_eq!(std::fs::read(doc(&tmp, "doc_m.md")).unwrap().len(), 0);
    drop(store);
}

#[test]
fn create_dir_mirrors() {
    let tmp = TDir::new("ap_cd");
    let store = mk_store(&tmp.s());
    store.create_path("genesis", "/dir_m", true, false).ok();
    assert!(doc(&tmp, "dir_m").is_dir(), "mdio mkdir 后磁盘应有目录");
    drop(store);
}

#[test]
fn remove_file_mirrors_and_wal() {
    let tmp = TDir::new("ap_rf");
    let store = mk_store(&tmp.s());
    store.create_path("genesis", "/del_m.md", true, false).ok();
    assert!(doc(&tmp, "del_m.md").exists());
    store.remove_path("genesis", "/del_m.md", false, false, false).unwrap();
    assert!(!doc(&tmp, "del_m.md").exists(), "rm 后磁盘文件应消失");
    // WAL 删除(delete_wal 幂等):存在则必删,不存在不报错
    let sk = {
        let t = store.tree.read();
        let n = t.nodes.get("genesis").unwrap();
        let (sk, _, _) = n.resolve("/del_m.md", &t);
        sk
    };
    drop(store);
    let wal = tmp.path.join(FsBackend::wal_path(&sk));
    // 方案 d:drop 排水时队列残留批可能复活 WAL——只允许无害 marker(文档已删,无人 replay)
    if wal.exists() {
        let b = std::fs::read(&wal).unwrap_or_default();
        let frames = decode_frames(&b);
        assert!(frames.iter().all(|f| f.0), "rm 后 WAL 若复活,只应是无害 marker:frames={frames:?}");
    }
}

#[test]
fn rm_open_doc_no_resurrect() {
    let tmp = TDir::new("ap_ro");
    let store = mk_store(&tmp.s());
    store.create_path("genesis", "/open_m.md", true, false).ok();
    let (doc_id, sess) = mk_file(&store, "/open_m.md");
    sess.execute_transaction(None, true, |ctx| ctx.state.engine.edit(0, 0, "# EDIT\n"));
    store.remove_path("genesis", "/open_m.md", false, false, false).unwrap();
    // 删除已开会话文档:session 必须先关(dirty 不再被 flush 线程复活)
    assert!(!store.sessions.contains_key(&doc_id), "rm 后 session 应已关闭");
    assert!(!doc(&tmp, "open_m.md").exists());
    drop(store);
}

#[test]
fn rm_recursive_clears_sessions() {
    let tmp = TDir::new("ap_rr");
    let store = mk_store(&tmp.s());
    store.create_path("genesis", "/tree/a.md", true, false).ok();
    store.create_path("genesis", "/tree/b.md", true, false).ok();
    let (id_a, _s1) = mk_file(&store, "/tree/a.md");
    let (id_b, _s2) = mk_file(&store, "/tree/b.md");
    store.remove_path("genesis", "/tree", true, false, false).unwrap();
    assert!(!store.sessions.contains_key(&id_a) && !store.sessions.contains_key(&id_b),
        "rm -r 后目录内 session 应全部关闭");
    assert!(!doc(&tmp, "tree").exists(), "递归删除后磁盘目录树应消失");
    drop(store);
}

#[test]
fn reset_doc_overwrites_disk_and_wal() {
    let tmp = TDir::new("ap_rs");
    let store = mk_store(&tmp.s());
    store.create_path("genesis", "/rst_m.md", true, false).ok();
    let (doc_id, _sess) = mk_file(&store, "/rst_m.md");
    store.reset_doc("genesis", "/rst_m.md", "# RESET\n- v: new\n").unwrap();
    assert_eq!(std::fs::read_to_string(doc(&tmp, "rst_m.md")).unwrap(),
        "# RESET\n- v: new\n", "reset 后磁盘内容应为新内容");
    // WAL 清净:reset 后重开不重放旧 edits
    let sk = {
        let t = store.tree.read();
        let n = t.nodes.get("genesis").unwrap();
        let (sk, _, _) = n.resolve("/rst_m.md", &t);
        sk
    };
    let wal = tmp.path.join(FsBackend::wal_path(&sk));
    assert!(!wal.exists(), "reset 后 WAL 应被删除");
    // 重开断言:文本=新内容
    let sess2 = store.open_session(doc_id).unwrap();
    assert_eq!(sess2.get_engine_text(), "# RESET\n- v: new\n");
    drop(store);
}

// WAL 结算修复回归(方案 d:FlushMarker 与 edits 同队列;append 合并写;worker trim)

/// 等待 worker 落盘,聚合 .wal/ 下所有 .log(系统文件亦有 WAL;帧流拼接仍可逐帧解码)。
fn wait_wal_all(tdir: &TDir) -> Vec<u8> {
    let wal_dir = tdir.path.join(".wal");
    let mut buf = Vec::new();
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        buf.clear();
        if let Ok(rd) = std::fs::read_dir(&wal_dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "log").unwrap_or(false) {
                    if let Ok(b) = std::fs::read(&p) { buf.extend_from_slice(&b); }
                }
            }
        }
        if !buf.is_empty() { break; }
    }
    buf
}

/// 解码 WAL 帧序列 → Vec<(is_marker, edit_text)>。
fn decode_frames(wire: &[u8]) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut p = 0usize;
    while p + 12 <= wire.len() {
        let l = u32::from_le_bytes([wire[p], wire[p+1], wire[p+2], wire[p+3]]) as usize;
        if u32::from_le_bytes([wire[p+4], wire[p+5], wire[p+6], wire[p+7]]) != WAL_MAGIC { break; }
        let ee = p + 12 + l;
        if ee > wire.len() { break; }
        if let Ok((entry, _)) = bincode::serde::decode_from_slice::<WalEntry, _>(&wire[p+12..ee], bincode_cfg()) {
            match entry {
                WalEntry::FlushMarker => out.push((true, String::new())),
                WalEntry::Edit(rec) => out.push((false, rec.text)),
            }
        }
        p = ee;
    }
    out
}

/// 重建 store 并打开 doc,返回 replay 后的 engine 文本。
fn reopen_text(tdir: &TDir, vpath: &str) -> String {
    let store = mk_store(&tdir.s());
    let vpath = normalize_vpath(vpath);
    let doc_id = store.resolve_path("genesis", &vpath, false).unwrap();
    let session = store.open_session(doc_id).unwrap();
    let text = session.get_engine_text();
    drop(session);
    drop(store);
    text
}

#[test]
fn wal_no_double_apply_after_flush() {
    let tmp = TDir::new("nodup");
    let store = mk_store(&tmp.s());
    let (doc_id, session) = mk_file(&store, "/a.md");
    session.execute_transaction(None, true, |ctx| {
        ctx.apply_mdio_op_multi(vec![crate::op::WriteEdit { range: 0..0, text: "# T\n".into() }], "+>");
    });
    flush_one(doc_id, &session, &store.arena, &store.vfs_op, &store.tree, &store.backends);
    session.execute_transaction(None, true, |ctx| {
        ctx.apply_mdio_op_multi(vec![crate::op::WriteEdit { range: 4..4, text: "- k: v\n".into() }], "+>");
    });
    flush_one(doc_id, &session, &store.arena, &store.vfs_op, &store.tree, &store.backends);
    drop(session);
    drop(store);
    // 重启:两代均已结算,WAL 重放 0 → 无重复(多代端到端)
    let text = reopen_text(&tmp, "/a.md");
    assert_eq!(text, "# T\n- k: v\n", "双应用回归:got {text:?}");
}

#[test]
fn wal_merge_append_keeps_all_entries() {
    std::env::set_var("MDIO_WAL_INTERVAL_MS", "10");
    // 禁用自动 flush(定期 flush 会把未结算 edits 冲进 base + trim,合并写
    // 保帧的前提不成立)——本测试的结算由显式 flush_one 控制(本测试不调)。
    std::env::set_var("MDIO_FLUSH_INTERVAL_MS", "3600000");
    let tmp = TDir::new("merge");
    let store = mk_store(&tmp.s());
    let (doc_id, session) = mk_file(&store, "/a.md");
    session.execute_transaction(None, true, |ctx| { ctx.apply_mdio_op_multi(vec![crate::op::WriteEdit { range: 0..0, text: "A".into() }], "+>"); });
    session.execute_transaction(None, true, |ctx| { ctx.apply_mdio_op_multi(vec![crate::op::WriteEdit { range: 0..0, text: "B".into() }], "+>"); });
    // 合并窗口竞态:wait_wal_all 聚合全部 .log——系统文件的 marker 帧可能先
    // 落盘导致提前 break(此时 a.md 的 edits 还在 worker 的合并窗口内)。
    // 自旋等"至少一个 Edit 帧"出现再断言(worker 周期 10ms,1s 余量充足)。
    let wire = {
        let mut w = wait_wal_all(&tmp);
        let mut tries = 0;
        while tries < 20 && decode_frames(&w).iter().all(|f| f.0) {
            std::thread::sleep(std::time::Duration::from_millis(50));
            w = wait_wal_all(&tmp);
            tries += 1;
        }
        w
    };
    drop(session);
    drop(store);
    // 覆盖写回归:两批 edits 都应留在 WAL(合并写;系统文件帧混入不干扰)
    let frames = decode_frames(&wire);
    let edits: Vec<&str> = frames.iter().filter(|f| !f.0).map(|f| f.1.as_str()).collect();
    assert_eq!(edits, vec!["A", "B"], "合并写回归:frames={frames:?}");
}

#[test]
fn wal_settled_log_is_dropped_after_settlement() {
    std::env::set_var("MDIO_WAL_INTERVAL_MS", "10");
    // 关掉定期 flush：结算时机只由下面三次显式 flush 决定（否则定期 flush 会掺进来）。
    std::env::set_var("MDIO_FLUSH_INTERVAL_MS", "3600000");
    let tmp = TDir::new("settle_drop");
    let store = mk_store(&tmp.s());
    let (doc_id, session) = mk_file(&store, "/a.md");
    let sk = session.storage_key.clone();
    for text in ["gen1", "gen2", "gen3"] {
        session.execute_transaction(None, true, |ctx| {
            ctx.apply_mdio_op_multi(vec![crate::op::WriteEdit { range: 0..0, text: text.into() }], "+>");
        });
        flush_one(doc_id, &session, &store.arena, &store.vfs_op, &store.tree, &store.backends);
    }
    drop(session);
    drop(store); // Drop 排水：worker drain 余批并执行「结算即清」
    // 三代全部结算 ⇒ 该 doc 的 .log 应被**整个删除**（不再留一个只为占位的结算点）。
    let wal = tmp.path.join(FsBackend::wal_path(&sk));
    assert!(!wal.exists(), "已结算且其后无 edit ⇒ 该 doc 的 WAL 应被整个删除：{wal:?}");
    // 端到端：三代都已进 base，重启 replay 应为 0（无重复应用）。
    let text = reopen_text(&tmp, "/a.md");
    assert_eq!(text, "gen3gen2gen1", "三代已结算 ⇒ 重启后不得重复：got {text:?}");
}

#[test]
fn wal_unsettled_tail_survives_after_settlement() {
    std::env::set_var("MDIO_WAL_INTERVAL_MS", "10");
    std::env::set_var("MDIO_FLUSH_INTERVAL_MS", "3600000"); // 第二代必须保持未结算
    let tmp = TDir::new("settle_tail");
    let store = mk_store(&tmp.s());
    let (doc_id, session) = mk_file(&store, "/a.md");
    let sk = session.storage_key.clone();
    // ① 第一代：edit + 结算 ⇒ 结算之后立刻清，该 doc 的 .log 应消失。
    session.execute_transaction(None, true, |ctx| {
        ctx.apply_mdio_op_multi(vec![crate::op::WriteEdit { range: 0..0, text: "gen1".into() }], "+>");
    });
    flush_one(doc_id, &session, &store.arena, &store.vfs_op, &store.tree, &store.backends);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let wal = tmp.path.join(FsBackend::wal_path(&sk));
    assert!(!wal.exists(), "第一代结算后该 doc 的 WAL 应已清掉：{wal:?}");
    // ② 第二代：只编辑、不结算 ⇒ 盘上只留这一段，且**不带结算点**（它的 base 已更新，
    //    replay 要能从新 base 起全量应用）。
    session.execute_transaction(None, true, |ctx| {
        ctx.apply_mdio_op_multi(vec![crate::op::WriteEdit { range: 0..0, text: "gen2".into() }], "+>");
    });
    std::thread::sleep(std::time::Duration::from_millis(300));
    let frames = decode_frames(&std::fs::read(&wal).expect("未结算段应留在盘上"));
    let edits: Vec<&str> = frames.iter().filter(|f| !f.0).map(|f| f.1.as_str()).collect();
    assert_eq!(edits, vec!["gen2"], "只留未结算段：frames={frames:?}");
    assert!(frames.iter().all(|f| !f.0), "保留段不应再带结算点：frames={frames:?}");
    // ③ 端到端：重启 = 新 base(gen1) + 未结算段(gen2)
    drop(session);
    drop(store);
    let text = reopen_text(&tmp, "/a.md");
    assert_eq!(text, "gen2gen1", "replay 应在新 base 上重放未结算段：got {text:?}");
}

#[test]
fn wal_replay_respects_fresh_base() {
    std::env::set_var("MDIO_WAL_INTERVAL_MS", "10");
    let tmp = TDir::new("freshbase");
    let store = mk_store(&tmp.s());
    let (doc_id, session) = mk_file(&store, "/a.md");
    session.execute_transaction(None, true, |ctx| { ctx.apply_mdio_op_multi(vec![crate::op::WriteEdit { range: 0..0, text: "# V1\n".into() }], "+>"); });
    flush_one(doc_id, &session, &store.arena, &store.vfs_op, &store.tree, &store.backends);
    let _ = wait_wal_all(&tmp); // 等 marker 落盘
    drop(session);
    drop(store);
    // 外部把 base 直接改新(V2):WAL 只剩旧代,replay 应重放 0,base 原样生效
    std::fs::write(doc(&tmp, "a.md"), "# V2\n").unwrap();
    let text = reopen_text(&tmp, "/a.md");
    assert_eq!(text, "# V2\n", "replay 应尊重新 base:got {text:?}");
}
