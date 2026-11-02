// patch match attr —— F 帧归属判官
//
// 判官：① 无注册（ClaimMap 空）⇒ 与默认渲染 **逐字节相同**；
//       ② 有注册 ⇒ 被选中区间对应的块级节点对象带 "match":["tag"]，其余不带。
use crate::r#match::{ClaimMap, MatchRegistry};
use crate::projection::State;
use crate::selector::Step;
use crate::topology::{ProjectionMode, CONTAINER_TOPOLOGY, HEADINGSECTION_TOPOLOGY};

fn snapshot() -> crate::projection::Snapshot {
    State::new("# H1\n\npara one\n\n# H2\n\npara two\n").snapshot()
}

#[test]
fn f_frame_byte_identical_without_claims() {
    let snap = snapshot();
    for mode in [ProjectionMode::Cf, ProjectionMode::Hs, ProjectionMode::Bf] {
        let plain = snap.to_ast_object_mode(mode);
        let empty = ClaimMap::empty();
        let with = snap.to_ast_object_mode_with_claims(mode, &empty);
        assert_eq!(plain, with, "{mode:?}: 空归属必须逐字节不变");
    }
}

#[test]
fn f_frame_carries_match_only_for_claimed_nodes() {
    let snap = snapshot();
    // 注册：H1 的直接子块（段落）/ H2 的节壳——两个方向都验。
    let mut r = MatchRegistry::new();
    let reg_child = r.register_reg(
        "genesis",
        "**",
        vec![Step::Drill("H1".into()), Step::Wildcard],
        "childMod",
        false,
    );
    r.add_def(reg_child, 0, (0, usize::MAX), 0);
    let reg_shell = r.register_reg(
        "genesis",
        "**",
        vec![Step::Drill("H2".into())],
        "shellMod",
        false,
    );
    r.add_def(reg_shell, 0, (0, usize::MAX), 0);
    let defs = r.defs_for_doc(0);
    let claims = ClaimMap::build(&defs, &snap);
    assert!(!claims.is_empty(), "H1 段落与 H2 节壳都应入选");

    for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
        let json = snap.to_ast_object_mode_with_claims(mode, &claims);
        let v: serde_json::Value = serde_json::from_str(&json).expect("F 帧必须是合法 JSON");
        let mut found_child = false;
        let mut tags_total = 0usize;
        fn walk(n: &serde_json::Value, found_child: &mut bool, tags_total: &mut usize) {
            if let Some(o) = n.as_object() {
                if let Some(m) = o.get("match").and_then(|x| x.as_array()) {
                    *tags_total += m.len();
                    if m.iter().any(|t| t.as_str() == Some("childMod")) {
                        *found_child = true;
                    }
                }
                if let Some(ct) = o.get("ct").and_then(|x| x.as_array()) {
                    for c in ct {
                        walk(c, found_child, tags_total);
                    }
                }
            } else if let Some(arr) = n.as_array() {
                for c in arr {
                    walk(c, found_child, tags_total);
                }
            }
        }
        // 信封根：{"nt":"root",...,"ct":[...]}
        if let Some(ct) = v.get("ct").and_then(|x| x.as_array()) {
            for c in ct {
                walk(c, &mut found_child, &mut tags_total);
            }
        }
        assert!(tags_total > 0, "{mode:?}: 必须有节点带 match");
        assert!(found_child, "{mode:?}: H1 的直接子块应带 childMod");
        // 无认领的节点不得出现空数组键
        assert!(
            !json.contains("\"match\":[]"),
            "{mode:?}: 空认领不得写空数组键"
        );
    }
}

#[test]
fn f_render_entries_agree_on_empty_claims() {
    // 两个入口（无归属 / 空归属）在 P 值路径上同源：render_tree == render_tree_with(&())
    let snap = snapshot();
    let seg = crate::cache::structure_for_range(&snap, 0, snap.line_len());
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), seg.view.clone(), &seg.arena);
    let tree = crate::infer::build_attribution_tree(
        &seg.records, &seg.blocks, &view, &seg.arena,
        (0u32, snap.line_len() as u32), &CONTAINER_TOPOLOGY);
    let _ = HEADINGSECTION_TOPOLOGY;
    let mut a = Vec::new();
    crate::incr::render_tree(&mut a, &snap, &view, &tree, tree.roots[0] as usize, &CONTAINER_TOPOLOGY);
    let mut b = Vec::new();
    crate::incr::render_tree_with(&mut b, &snap, &view, &tree, tree.roots[0] as usize, &CONTAINER_TOPOLOGY, &());
    assert_eq!(a, b, "render_tree == render_tree_with(&())");
    // P 路径等价：F 渲染
    let mut c = Vec::new();
    crate::incr::render_tree_roots(&mut c, &snap, &view, &tree, &CONTAINER_TOPOLOGY);
    let mut d = Vec::new();
    crate::incr::render_tree_roots_with(&mut d, &snap, &view, &tree, &CONTAINER_TOPOLOGY, &());
    assert_eq!(c, d, "render_tree_roots == render_tree_roots_with(&())");
}
