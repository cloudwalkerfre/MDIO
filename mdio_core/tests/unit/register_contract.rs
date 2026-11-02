// 注册入口的家族契约校验（S1）——票据/服务家族/未知取值一律拒绝；`fs` 放行且行为不变。
//
// 位置契约：校验必须早于 `new_name()`/`tree.write()`（拒绝零副作用）。本文件只测判定本体；
// 零副作用的端到端证据在外层受控复现：7 参票据 ⇒ 报错且 CWD 下无 `iroh:` 目录。
use crate::store::ProjectStore;

fn err_of(root: &str, scheme: &str) -> String {
    ProjectStore::contract_check_external_root(root, scheme)
        .expect_err(&format!("({root:?}, {scheme:?}) 应被拒绝"))
        .to_string()
}

#[test]
fn fs_family_accepts_local_paths() {
    for root in ["/tmp/x", "/home/a/proj", "relative/path", "/mem"] {
        for scheme in ["fs", "FS", ""] {
            assert!(
                ProjectStore::contract_check_external_root(root, scheme).is_ok(),
                "{root} + {scheme} 应放行（fs 家族行为逐字节不变）"
            );
        }
    }
}

#[test]
fn empty_root_is_rejected() {
    assert!(err_of("", "fs").contains("不能为空"));
    assert!(err_of("   ", "fs").contains("不能为空"));
}

#[test]
fn ticket_shaped_root_is_rejected_with_pointer() {
    for root in ["iroh://x", "iroh-share://abc", "iroh:x"] {
        let e = err_of(root, "fs");
        assert!(e.contains("rule_add_remote"), "票据拒绝需指路 rule_add_remote：{e}");
    }
}

#[test]
fn uri_shaped_root_is_rejected_for_fs_family() {
    // 服务型根在 fs 家族下也不是本地路径：消息要同时点明两条出路（票据 → rule；服务 → 未接入）。
    let e = err_of("s3://bucket/prefix", "fs");
    assert!(e.contains("本地路径"), "{e}");
    assert!(e.contains("rule_add_remote") && e.contains("服务家族"), "{e}");
}

#[test]
fn memory_family_is_rejected_with_alternative() {
    let e = err_of("/tmp/x", "memory");
    assert!(e.contains("memory"), "{e}");
    assert!(e.contains("derive_worktree"), "memory 拒绝需给替代路径：{e}");
}

#[test]
fn service_family_message_wins_over_root_shape() {
    // 先判家族、再判根形态：服务家族不论根写成什么，都必须是“服务未接入”，不得被误判成票据。
    for (root, scheme) in [("/tmp/x", "remote"), ("s3://bucket/prefix", "remote"), ("/tmp/x", "s3")] {
        let e = err_of(root, scheme);
        assert!(e.contains("服务家族"), "{root} + {scheme} 应为服务家族消息：{e}");
        assert!(!e.contains("rule_add_remote"), "{root} + {scheme} 不应被误判为票据：{e}");
    }
}

// ── S1b：注册原子化（失败不留幽灵节点）────────────────────────────────

/// `/mem` 库：genesis 无外部根、走 Memory 家族（与 Python `ProjectStore('/mem')` 同形）。
fn mem_store() -> ProjectStore {
    ProjectStore::new_with_layout("/mem", None, None).unwrap()
}

#[test]
fn failed_backend_construction_leaves_no_worktree_node() {
    let store = mem_store();
    let before = store.list_worktrees().len();
    // 布局名缺失 ⇒ `new_with_docs` 必失败——正是今天 4 参调用的真实失败路径。
    let err = store
        .register_worktree_at_with_layout("genesis", "p", "/tmp/mdio_s1b_probe", "fs", "", None, None)
        .expect_err("布局名缺失应失败");
    assert!(err.to_string().contains("layout"), "失败原因应是布局：{err}");
    let after = store.list_worktrees();
    assert_eq!(after.len(), before, "失败后树上不得多出节点（S1b：树与后端一致）");
    assert!(!after.iter().any(|w| w.name == "p"), "失败的 wt 不得留在树上");
}

#[test]
fn successful_registration_still_inserts_node() {
    let store = mem_store();
    let dir = format!("/tmp/__mdio_s1b_ok_{}", std::process::id());
    std::fs::create_dir_all(format!("{dir}/wt")).unwrap();
    let uid = store
        .register_worktree_at_with_layout("genesis", "ok", &dir, "fs", "",
                                          Some("wt".to_string()), Some(".wal".to_string()))
        .expect("fs ＋ 真目录 ＋ 布局名应成功");
    assert!(
        store.list_worktrees().iter().any(|w| w.uid == uid && w.name == "ok"),
        "成功路径仍要插节点（重排不得改变正常语义）"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn child_named_genesis_is_not_root() {
    // 子条目名可以等于 "genesis"（项目目录 basename）；根身份必须由 parent 判定，
    // 否则 build 会把它当根、重复读同一份 worktrees.md，无限递归爆栈。
    let store = mem_store();
    store.vfs_op.write(
        "/.mds/worktrees.md",
        "# WORKTREES\n\n## genesis\n- uid: cP48TPyi\n- path: /tmp/mdio-child-genesis\n- doc_subdir: wt\n- wal_rel: .wal\n",
    ).unwrap();
    {
        let mut tree = store.tree.write();
        tree.dirty = true;
        tree.ensure(&store);
    }
    let tree = store.tree.read();
    let root = tree.nodes.get("genesis").expect("synthetic root must exist");
    assert!(root.parent.is_none(), "genesis root must have no parent");
    let child = tree.nodes.get("cP48TPyi").expect("child named genesis must be a normal child");
    assert_eq!(child.name, "genesis");
    assert_eq!(child.parent.as_deref(), Some("genesis"));
    assert_eq!(child.storage_root, "/.mds/mounts/mnt_cP48TPyi/");
}
