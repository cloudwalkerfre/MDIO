// patch match attr —— claim 计算判官
//
// 语义：核只报「该路径的**直接**认领者」——
// 与 op 节点**同一区间**（不回溯祖先；祖先回溯是消费端的事）。
// 结构性事实：CF 树 heading_candidates=[]（无节壳节点），节壳跨度只在 HS 树存在 ⇒
//   · 节内块级注册（Drill + RecursiveWildcard）两拓扑下都有对应节点 ⇒ 非真空对拍；
//   · 纯节壳注册（Drill only）在 CF 恒零命中、在 HS 可命中——两条都显式钉住。
//
// 判官：独立 oracle 枚举目标树全部节点（ast_path_of/path_str）建 path→区间表，
// 取「最长前缀」——不复用 walk_ast_path 的下行逻辑；再按「区间 ∈ 选中集」求期望。
use super::*;
use crate::r#match::{FieldMask, MatchRegistry};
use crate::selector::Step;
use parking_lot::Mutex;
use std::sync::Arc;

const TAG_BLOCK: &str = "blockMod";
const TAG_SHELL: &str = "shellMod";

const SHAPES: &[(&str, &[&str])] = &[
    ("flat", &["inline", "struct", "heading", "hlevel", "hins", "hdel", "combo"]),
    ("nested", &["inline", "heading", "hins", "hdel"]),
    ("setext", &["inline", "heading", "hdel"]),
    ("tight", &["inline", "heading", "combo"]),
    ("deep3", &["heading", "hins"]),
    ("fence_cross", &["inline", "heading"]),
    ("bq_inner_head", &["heading", "hdel"]),
];

/// 覆写区行 → 其所属节的标题文本（Drill 目标；不硬编码形态）。
fn section_drill(snap: &Snapshot, line: usize) -> Option<String> {
    let cell = snap.section_cell(&HEADINGSECTION_TOPOLOGY);
    let (s, _e, _lv, _bt) = cell
        .secs_iter()
        .rev()
        .find(|&(s, e, _, _)| s <= line && line < e)?;
    let raw = snap.line_text(s).to_string();
    let t = raw.trim_start_matches('#').trim().to_string();
    if t.is_empty() { None } else { Some(t) }
}

/// 该用例的 Drill 目标 = **第一个可解 op** 所属节的标题（对齐实际被编辑的节）。
fn case_drill(ctx: &ZoneContext, out: &[Patch]) -> Option<String> {
    let mut cache = DomainCache::default();
    for p in out {
        let is_old = matches!(p, Patch::Remove { .. });
        let segs = parse_path_like(patch_path(p));
        let snap = if is_old { &ctx.snap_old } else { &ctx.snap_new };
        let line = match resolve_patch_interval(ctx, ProjectionMode::Cf, p, &segs, &mut cache) {
            Some((s, _e)) => s as usize,
            None => {
                let iv = match ProjectionMode::Hs {
                    _ => resolve_patch_interval(ctx, ProjectionMode::Hs, p, &segs, &mut cache),
                };
                match iv {
                    Some((s, _e)) => s as usize,
                    None => continue,
                }
            }
        };
        if let Some(d) = section_drill(snap, line) {
            return Some(d);
        }
    }
    None
}

/// 枚举目标树全部节点路径（独立 oracle 建表步）。
fn enum_paths(
    seg: &crate::cache::Segment,
    snap: &Snapshot,
    tree: &crate::infer::AttrTreeNew,
) -> Vec<(Vec<usize>, (u32, u32))> {
    let mut v = Vec::with_capacity(tree.nodes.len());
    for i in 0..tree.nodes.len() {
        if let Some(c) = ast_path_of(tree, seg, snap, i) {
            let n = &tree.nodes[i];
            v.push((c, (n.start, n.end)));
        }
    }
    v
}

/// oracle：路径分量是 elems 前缀的**最长**节点 ⇒ 其区间。
fn oracle_iv(paths: &[(Vec<usize>, (u32, u32))], elems: &[usize], off: usize) -> Option<(u32, u32)> {
    let mut best: Option<(usize, (u32, u32))> = None;
    for (c, iv) in paths {
        if c.is_empty() || c.len() > elems.len() || c[0] + off != elems[0] {
            continue;
        }
        let mut ok = true;
        for k in 1..c.len() {
            if c[k] != elems[k] {
                ok = false;
                break;
            }
        }
        if ok && best.map_or(true, |(l, _)| c.len() > l) {
            best = Some((c.len(), *iv));
        }
    }
    best.map(|(_, iv)| iv)
}

fn selected_ivs(snap: &Snapshot, steps: &[Step]) -> Vec<(u32, u32)> {
    crate::selector::select_nodes(snap, steps)
        .iter()
        .map(|n| (n.start_line as u32, n.end_line as u32))
        .collect()
}

/// HS：定域（root_starts → sealed → domain_parts）；oracle 侧只共享这①②步。
fn hs_domain(
    snap: &Snapshot,
    elems: &[usize],
    cache: &mut DomainCache,
) -> Option<(Arc<(crate::cache::Segment, crate::infer::AttrTreeNew)>, usize)> {
    let cell = snap.section_cell(&HEADINGSECTION_TOPOLOGY);
    let line = *cell.root_starts.get(*elems.first()?)? as usize;
    let intervals: Vec<(usize, usize)> = cell.secs_iter().map(|(s, e, _, _)| (s, e)).collect();
    let sealed = crate::infer::sealed_interval_of(
        &snap.frame_log.entries,
        &snap.lines,
        &snap.open_depth,
        snap.line_len(),
        &intervals,
        line,
    );
    let part = domain_parts(snap, sealed, &HEADINGSECTION_TOPOLOGY, cache)?;
    let off = hs_off_before(&cell, part.0.start_line);
    Some((part, off))
}

fn two_regs(doc_id: u32, drill: &str, snap_old: &Snapshot) -> Arc<Mutex<MatchRegistry>> {
    let mut r = MatchRegistry::new();
    let steps_b = vec![Step::Drill(drill.into()), Step::RecursiveWildcard];
    let rb = r.register_reg(
        "genesis",
        "**",
        steps_b.clone(),
        TAG_BLOCK,
        false,
    );
    // 状态初值按生产同律——注册在「写前快照」（= 下一写 snap_old）上求值；
    // scope = 真实节区间（compute_scope_range，穿测跳过路径；非测试捷径全文档）。
    let (scope_b, wd_b) = crate::hook::compute_scope_range(&steps_b, snap_old)
        .map(|s| (s, false))
        .unwrap_or(((0, snap_old.line_len()), true));
    let rb_def = r.add_def(rb, doc_id, scope_b, snap_old.line_len() as u32).unwrap();
    r.init_state(doc_id, rb_def, scope_b, snap_old.line_len() as u32, &steps_b, snap_old, wd_b);
    let steps_s = vec![Step::Drill(drill.into())];
    let (scope_s, wd_s) = crate::hook::compute_scope_range(&steps_s, snap_old)
        .map(|s| (s, false))
        .unwrap_or(((0, snap_old.line_len()), true));
    let rs = r.register_reg("genesis", "**", steps_s.clone(), TAG_SHELL, false);
    let rs_def = r.add_def(rs, doc_id, scope_s, snap_old.line_len() as u32).unwrap();
    r.init_state(doc_id, rs_def, scope_s, snap_old.line_len() as u32, &steps_s, snap_old, wd_s);
    Arc::new(Mutex::new(r))
}

/// 形态×编辑类型 × mode（Block 粒度 ⇒ 无 ReplaceAttr，不涉 sens 短路）。
/// 返回 (核对 op 数, 块级命中数, 节壳命中数, 无法定 Drill 而跳过的用例数)。
fn matrix(mode: ProjectionMode) -> (usize, usize, usize, usize) {
    let mut checked = 0usize;
    let mut hit_block = 0usize;
    let mut hit_shell = 0usize;
    let mut skipped = 0usize;
    for &(shape, kinds) in SHAPES {
        for &kind in kinds {
            let Some((_o, _n, task)) = make_case(kind, shape) else { continue };
            let (pf, ctx) = diff_patches_mode_with_ctx(&task, &CONTAINER_TOPOLOGY);
            let topo = match mode {
                ProjectionMode::Hs => &HEADINGSECTION_TOPOLOGY,
                _ => &CONTAINER_TOPOLOGY,
            };
            let out = adapt(&task, &pf, &ctx, topo);
            if out.is_empty() {
                continue;
            }
            let Some(drill) = case_drill(&ctx, &out) else {
                skipped += 1;
                continue;
            };
            let steps_block = vec![Step::Drill(drill.clone()), Step::RecursiveWildcard];
            let steps_shell = vec![Step::Drill(drill.clone())];
            let reg = two_regs(task.doc_id, &drill, &ctx.snap_old);
            let claims = claim_ids(&task, &out, mode, &reg, &ctx);
            assert_eq!(claims.len(), out.len(), "{shape}/{kind}/{mode:?}: len contract");
            let silo = selected_ivs(&ctx.snap_old, &steps_shell);
            let siln = selected_ivs(&ctx.snap_new, &steps_shell);
            let bilo = selected_ivs(&ctx.snap_old, &steps_block);
            let biln = selected_ivs(&ctx.snap_new, &steps_block);
            let paths_old = enum_paths(&ctx.o_seg, &ctx.snap_old, &ctx.o_tree);
            let paths_new = enum_paths(&ctx.n_seg, &ctx.snap_new, &ctx.n_tree);
            let mut past = DomainCache::default();
            for (i, p) in out.iter().enumerate() {
                let elems = parse_path_like(patch_path(p));
                let is_old = matches!(p, Patch::Remove { .. });
                checked += 1;
                let iv = match mode {
                    ProjectionMode::Hs => {
                        let snap = if is_old { &ctx.snap_old } else { &ctx.snap_new };
                        match hs_domain(snap, &elems, &mut past) {
                            Some((part, off)) => {
                                let paths = enum_paths(&part.0, snap, &part.1);
                                oracle_iv(&paths, &elems, off)
                            }
                            None => None,
                        }
                    }
                    _ => {
                        let paths = if is_old { &paths_old } else { &paths_new };
                        let off = if is_old { ctx.off_old } else { ctx.off_new };
                        oracle_iv(paths, &elems, off)
                    }
                };
                let sel_block = if is_old { &bilo } else { &biln };
                let sel_shell = if is_old { &silo } else { &siln };
                let mut expect: Vec<&str> = Vec::new();
                if let Some(iv) = iv {
                    if sel_block.contains(&iv) {
                        expect.push(TAG_BLOCK);
                    }
                    if sel_shell.contains(&iv) {
                        expect.push(TAG_SHELL);
                    }
                }
                let got: Vec<&str> = claims[i].iter().map(|s| &**s).collect();
                assert_eq!(
                    got, expect,
                    "{shape}/{kind}/{mode:?} op#{i} path={} claims={:?}",
                    patch_path(p), claims
                );
                if got.contains(&TAG_BLOCK) {
                    hit_block += 1;
                }
                if got.contains(&TAG_SHELL) {
                    hit_shell += 1;
                }
            }
        }
    }
    (checked, hit_block, hit_shell, skipped)
}

#[test]
fn claims_match_oracle_cf() {
    let (n, hit_block, hit_shell, skipped) = matrix(ProjectionMode::Cf);
    assert!(n > 0, "CF 矩阵为空（夹具失效）");
    assert_eq!(skipped, 0, "有用例定不出 Drill 目标——夹具失效");
    assert!(hit_block > 0, "CF：节内块级注册零命中——判官退化");
    assert_eq!(
        hit_shell, 0,
        "CF 树 heading_candidates=[]（无节壳节点）⇒ 纯节壳注册恒零命中；\
         此断言变红 = 拓扑已变，需重审「直接认领 = 同一区间」语义"
    );
}

#[test]
fn claims_match_oracle_hs() {
    let (n, hit_block, hit_shell, skipped) = matrix(ProjectionMode::Hs);
    assert!(n > 0, "HS 矩阵为空（夹具失效）");
    assert_eq!(skipped, 0, "有用例定不出 Drill 目标——夹具失效");
    assert!(hit_block > 0, "HS：节内块级注册零命中——判官退化");
    assert!(hit_shell > 0, "HS：节壳注册应能命中壳级 op（非真空证据）");
}

#[test]
fn claims_len_contract_bf_and_empty_registry() {
    for mode in [ProjectionMode::Cf, ProjectionMode::Hs, ProjectionMode::Bf] {
        let reg = Arc::new(Mutex::new(MatchRegistry::new()));
        for (shape, kind) in [("flat", "inline"), ("nested", "heading")] {
            let Some((_o, _n, task)) = make_case(kind, shape) else { continue };
            let (patches, claims) = patches_and_claims(&task, mode, &reg);
            assert_eq!(claims.len(), patches.len(), "{shape}/{kind}/{mode:?}");
            assert!(claims.iter().all(|c| c.is_empty()), "无注册 ⇒ 全空");
        }
    }
}

#[test]
fn claims_and_patches_agree_with_direct_claim_ids() {
    let Some((_o, _n, task)) = make_case("inline", "flat") else { return };
    let reg = two_regs(0, "Section 0", &task.snap_old);
    let (patches, claims) = patches_and_claims(&task, ProjectionMode::Cf, &reg);
    let (pf, ctx) = diff_patches_mode_with_ctx(&task, &CONTAINER_TOPOLOGY);
    let out = adapt(&task, &pf, &ctx, &CONTAINER_TOPOLOGY);
    assert_eq!(patches, out, "公共入口的补丁必须等于旧入口");
    assert_eq!(claims, claim_ids(&task, &out, ProjectionMode::Cf, &reg, &ctx));
}

#[test]
fn sens_short_circuit_returns_empty_on_ct_only_frames() {
    // 谓词级（matcher::tests）已单测；此处断言**集成级**：ct-only 帧上
    // 纯 Drill 注册（sens={t}，非结构敏感）整条短路 ⇒ claims 全空。
    let mut seen = 0usize;
    for &(shape, kinds) in SHAPES {
        for &kind in kinds {
            let Some((_o, _n, task)) = make_case(kind, shape) else { continue };
            let (pf, ctx) = diff_patches_mode_with_ctx_g(
                &task,
                &CONTAINER_TOPOLOGY,
                crate::topology::Granularity::Proj,
            );
            let out = adapt(&task, &pf, &ctx, &CONTAINER_TOPOLOGY);
            if out.is_empty() {
                continue;
            }
            let frame = match crate::r#match::frame_field_mask(&out) {
                Some(m) => m,
                None => continue,
            };
            if frame.intersects(FieldMask::from_bits(FieldMask::T)) {
                continue; // 只挑 ct-only 帧
            }
            seen += 1;
            let Some(drill) = case_drill(&ctx, &out) else { continue };
            let mut r = MatchRegistry::new();
            let reg_id = r.register_reg(
                "genesis",
                "**",
                vec![Step::Drill(drill)],
                TAG_SHELL,
                false,
            );
            r.add_def(reg_id, task.doc_id, (0, usize::MAX), 0);
            let defs = r.defs_for_doc(task.doc_id);
            assert!(
                crate::r#match::sens_allows_skip(&defs[0], Some(frame)),
                "{shape}/{kind}: 纯 Drill 注册在 ct-only 帧上必须可跳过"
            );
            let reg = Arc::new(Mutex::new(r));
            let claims = claim_ids(&task, &out, ProjectionMode::Cf, &reg, &ctx);
            assert!(
                claims.iter().all(|c| c.is_empty()),
                "{shape}/{kind}: ct-only 帧整条短路 ⇒ 全空"
            );
        }
    }
    assert!(seen > 0, "夹具未出现 ct-only 帧——短路判官退化");
}


