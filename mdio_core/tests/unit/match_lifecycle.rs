// patch match attr —— 注册/注销生命周期判官（spec S1）
//
// 面：register_match（D5 门 / 展开 / 逐文档 scope 门 / 通配模板）、
//     remove_match、clear_doc_matches、clear_worktree_matches、drop_worktree 接线。
// 零残留 = 注销后 count_for_worktree == 0 且 defs/regs/wildcard/applied 全空。
use crate::selector::{SelectorIR, Step};
use crate::store::ProjectStore;

fn drill(t: &str) -> SelectorIR {
    SelectorIR { steps: vec![Step::Drill(t.into())] }
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let d = std::path::PathBuf::from(format!("/tmp/__mdio_match_{}_{}", tag, std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 三根分层存储：布局名由 host 给（core 无默认、无扁平兜底）——文档 `<根>/wt`、WAL `<根>/.wal`。
fn mk_store(dir: &std::path::Path) -> ProjectStore {
    ProjectStore::new_with_layout(
        &dir.to_string_lossy(),
        Some("wt".to_string()),
        Some(".wal".to_string()),
    )
    .unwrap()
}

/// 建文档并写入内容（VFS mem）——内容必须在 open_session 之前落盘。
fn seed(store: &ProjectStore, wt_uid: &str, path: &str, text: &str) -> u32 {
    store.create_path(wt_uid, path, true, false).ok();
    let doc_id = store.resolve_path(wt_uid, path, false).expect("resolve");
    let sk = store.arena.get_meta(doc_id).expect("meta").storage_key;
    let _ = store.vfs_op.write(&sk, text.as_bytes().to_vec());
    doc_id
}

#[test]
fn match_register_remove_zero_residue() {
    let dir = tmp_dir("lc1");
    let store = mk_store(&dir);
    let doc = seed(&store, "genesis", "/m.md", "# T\n\nbody\n");

    let reg = store.register_match("genesis", "/m.md", drill("T"), "modA");
    assert!(reg != 0, "可解析的 Drill 注册必须成功");
    assert_eq!(store.match_count_for_worktree("genesis"), 1);
    assert!(store.match_registry.lock().has_doc(doc));

    assert_eq!(store.remove_match(reg), 1, "注销应清掉 1 条 def");
    assert_eq!(store.match_count_for_worktree("genesis"), 0);
    assert!(!store.match_registry.lock().has_doc(doc));
    assert_eq!(store.match_registry.lock().defs.len(), 0);
    assert_eq!(store.match_registry.lock().regs.len(), 0);
    assert!(store.match_registry.lock().wildcard.is_empty());
    assert!(store.match_registry.lock().applied.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn match_register_gates() {
    let dir = tmp_dir("lc2");
    let store = mk_store(&dir);
    let doc = seed(&store, "genesis", "/m.md", "# T\n\nbody\n");
    let _ = doc;

    // ① 无 Drill 前缀（D5）⇒ 0 哨兵，不注册。
    let r = store.register_match(
        "genesis",
        "/m.md",
        SelectorIR { steps: vec![Step::Wildcard] },
        "bad",
    );
    assert_eq!(r, 0, "无 Drill 前缀必须被拒");
    assert_eq!(store.match_count_for_worktree("genesis"), 0);

    // ② 具体地址 + 前缀在当前文档不解析 ⇒ 0（失败可观测，模板撤销）。
    let r = store.register_match("genesis", "/m.md", drill("NOPE"), "bad2");
    assert_eq!(r, 0, "具体地址但 scope 不解析必须失败");
    assert_eq!(store.match_count_for_worktree("genesis"), 0);
    assert!(store.match_registry.lock().regs.is_empty(), "失败不得留模板");

    // ③ 通配 + 当前无任何文档解析出 scope ⇒ 保留模板（等新文档补登记）。
    let r = store.register_match("genesis", "/**", drill("Absent"), "later");
    assert!(r != 0, "通配注册即使当前零命中也要保留模板");
    assert_eq!(store.match_count_for_worktree("genesis"), 0);
    assert_eq!(store.match_registry.lock().wildcard.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn match_wildcard_fanout_and_clear() {
    let dir = tmp_dir("lc3");
    let store = mk_store(&dir);
    let doc_a = seed(&store, "genesis", "/a.md", "# T\n\na\n");

    // 通配注册：注册期只展开**已存在**文档（a）；b 之后才出现 ⇒ 走 C5 补登记。
    let reg = store.register_match("genesis", "/**", drill("T"), "modA");
    assert!(reg != 0);
    assert_eq!(store.match_count_for_worktree("genesis"), 1, "注册期只登记已存在文档");
    let doc_b = seed(&store, "genesis", "/b.md", "# T\n\nb\n");
    assert!(store.match_registry.lock().has_doc(doc_a));
    assert!(!store.match_registry.lock().has_doc(doc_b));

    let snap_b = crate::projection::project_text("# T\n\nb\n");
    assert!(store.match_registry.lock().needs_fanout(doc_b, "genesis"));
    assert_eq!(store.match_registry.lock().ensure_doc(doc_b, "genesis", &snap_b), 1);
    assert_eq!(store.match_count_for_worktree("genesis"), 2);
    assert!(store.match_registry.lock().has_doc(doc_b));
    assert!(!store.match_registry.lock().needs_fanout(doc_b, "genesis"));

    // 文档级清理：只清该文档、该 wt 的 def。
    store.clear_doc_matches(doc_b, "genesis");
    assert_eq!(store.match_count_for_worktree("genesis"), 1);
    assert!(!store.match_registry.lock().has_doc(doc_b));
    assert!(store.match_registry.lock().applied.get(&(reg, doc_b)).is_none());

    // worktree 级清理：defs/regs/wildcard/applied 全清。
    store.clear_worktree_matches("genesis");
    assert_eq!(store.match_count_for_worktree("genesis"), 0);
    assert!(store.match_registry.lock().regs.is_empty());
    assert!(store.match_registry.lock().wildcard.is_empty());
    assert!(store.match_registry.lock().applied.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn drop_worktree_wires_match_clear() {
    let dir = tmp_dir("lc4");
    let store = mk_store(&dir);
    // ① genesis 有既有守卫（拒卸）⇒ 不得动注册表。
    let _ = seed(&store, "genesis", "/m.md", "# T\n\nbody\n");
    let reg_g = store.register_match("genesis", "/m.md", drill("T"), "modA");
    assert!(reg_g != 0);
    assert!(
        store.drop_worktree(&["genesis".to_string()]).is_err(),
        "genesis 拒卸（既有守卫）"
    );
    assert_eq!(store.match_count_for_worktree("genesis"), 1, "守卫路径不得误清");

    // ② 派生 wt 的真实卸载路径 ⇒ 归属零残留（C1 接线）。
    let sub_root = dir.join("sub");
    std::fs::create_dir_all(&sub_root).unwrap();
    let sub_uid = store
        .register_worktree_at_with_layout(
            "genesis",
            "sub",
            &sub_root.to_string_lossy(),
            "fs",
            "",
            Some("wt".to_string()),
            Some(".wal".to_string()),
        )
        .expect("register sub wt");
    let _ = seed(&store, &sub_uid, "/s.md", "# T\n\ns\n");
    let reg_s = store.register_match(&sub_uid, "/s.md", drill("T"), "modSub");
    assert!(reg_s != 0, "派生 wt 上注册");
    assert_eq!(store.match_count_for_worktree(&sub_uid), 1);
    assert_eq!(store.match_count_for_worktree("genesis"), 1, "跨 wt 不串");

    store.drop_worktree(&[sub_uid.clone()]).expect("drop sub wt");
    assert_eq!(
        store.match_count_for_worktree(&sub_uid),
        0,
        "drop_worktree 必须同步清归属（C1 接线）"
    );
    assert_eq!(store.match_count_for_worktree("genesis"), 1, "genesis 注册不受影响");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── R1 准入门（协议层）──────────────────────────────────────────────
//
// 门住在 `ProjectStore::handle_packet(packet, conn_wt)`：`conn_wt` = 连接视图，
// `UP_OPEN`/`UP_SYNC` 时先判该 doc 在连接 wt 的可达集里；不可达一律回 `DnErr(0xFF)`
// （「拒绝必须协议可见」）。本测覆盖 C0 补的那条接线（pyo3 把 conn_wt 传进 Rust）。
#[test]
fn admit_gate_rejects_unreachable_doc() {
    use crate::protocol::{DN_ERR, HEADER_SIZE, UP_OPEN, UP_SYNC};
    use crate::store::ServerAction;
    std::env::set_var("MDIO_ADMIT_GATE", "1"); // 门默认开；OnceLock 进程级，显式钉成开

    let dir = tmp_dir("admit");
    let store = mk_store(&dir);
    let doc_id = seed(&store, "genesis", "/g.md", "# T\n\nbody\n");

    // 最小帧：tag[0] ＋ peer[5..9] ＋ doc_id[9..13]（HEADER_SIZE = 13）。
    let frame = |tag: u8, doc: u32| -> Vec<u8> {
        let mut p = vec![0u8; HEADER_SIZE];
        p[0] = tag;
        p[5..9].copy_from_slice(&7u32.to_le_bytes());
        p[9..13].copy_from_slice(&doc.to_le_bytes());
        p
    };

    // ① conn_wt = None ⇒ 无连接上下文、不设门（UP_OPEN 正常建会话）
    match store.handle_packet(&frame(UP_OPEN, doc_id), None) {
        ServerAction::DirectAndSub { .. } => {}
        other => panic!("conn_wt=None 应放行，实得 {other:?}"),
    }
    // ② conn_wt = 归属 wt（genesis）⇒ 可达 ⇒ 放行
    match store.handle_packet(&frame(UP_OPEN, doc_id), Some("genesis")) {
        ServerAction::DirectAndSub { .. } => {}
        other => panic!("可达连接应放行，实得 {other:?}"),
    }
    // ③ conn_wt = 未知 wt ⇒ 不可达 ⇒ DnErr(0xFF)（安全拒绝，不是静默丢、更不是 panic）
    match store.handle_packet(&frame(UP_SYNC, doc_id), Some("__no_such_wt__")) {
        ServerAction::Direct { data } => {
            assert_eq!(data[0], DN_ERR, "不可达必须回 DnErr；实得首字节 {:#04X}", data[0]);
        }
        other => panic!("不可达应被拒（DnErr），实得 {other:?}"),
    }
    // ④ 短帧（< HEADER_SIZE）⇒ None，不 panic
    assert!(matches!(store.handle_packet(&[0u8; 4], Some("genesis")), ServerAction::None));
    let _ = std::fs::remove_dir_all(&dir);
}