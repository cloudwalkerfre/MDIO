// domain step2 预实验 P0（不动生产）：S1/S5 机制原型 + 发现 A 处置 + 强链接前置。
// 判官库存三份 fixture 由 cargo test --list 生成（再生命令见 DEV/domain_step2_plan P0 节）。

use crate::domain::DOMAINS;
use crate::trinity::{LawId, LawStatus, LAW_TABLE};

struct RootLawRow {
    id: LawId,
    proposition: &'static str,
    anchor: &'static str,
    status: &'static str,
    judges: &'static [&'static str],
}

static ROOT_LAWS: &[RootLawRow] = &[
    RootLawRow { id: LawId::RIncrEq, proposition: "编辑加行级涟漪的结果等于全量重建", anchor: "domain_mechanism_plan 8.1", status: "Guarded", judges: &["ripple_equivalence_test"] },
    RootLawRow { id: LawId::RTreeFold, proposition: "折叠与树构建互逆", anchor: "domain_mechanism_plan 8.1", status: "Guarded", judges: &["structure_equivalence_test", "view_arrays_fold_equivalence", "tree_fold_block_nesting_oracle", "container_end_for_dual_caliber"] },
    RootLawRow { id: LawId::RZone, proposition: "窗口闭包与独立参考实现一致", anchor: "domain_mechanism_plan 8.1", status: "Guarded", judges: &["zone_closure_equivalence", "zone_oracle_equivalence", "merged_zone_chain_probe"] },
    RootLawRow { id: LawId::RSplice, proposition: "前缀恒等加区间重算加后缀平移等于全量", anchor: "domain_mechanism_plan 8.1", status: "Guarded", judges: &["assembled_full_equals_full_build", "splice_directed_equivalence"] },
    RootLawRow { id: LawId::RSecs, proposition: "结构编辑后区间等于全量单源", anchor: "domain_mechanism_plan 8.1", status: "Guarded", judges: &["secs_cell_update_structural_matches_full"] },
    RootLawRow { id: LawId::RRootOrd, proposition: "根枚举起点集合等于归属树根集合", anchor: "domain_mechanism_plan 8.1", status: "Guarded", judges: &["root_blocks_before_identity_matches_cf_tree_roots"] },
];

static ROW_LAW_MAP: &[(&str, LawId, bool)] = &[
    ("zone_auth", LawId::RZone, false),
    ("zone_mat", LawId::RZone, false),
    ("root_merge", LawId::RRootOrd, false),
    ("root_split", LawId::RSecs, true),
    ("node_end_nav", LawId::RTreeFold, true),
    ("node_end_rec", LawId::RTreeFold, true),
    ("cont_tail_view", LawId::RTreeFold, true),
    ("cont_tail_build", LawId::RTreeFold, true),
    ("tree", LawId::RTreeFold, false),
    ("sec", LawId::RSecs, false),
    ("span", LawId::RSplice, false),
];

const INV_DEFAULT: &str = include_str!("judge_inventory_default.txt");
const INV_OBS: &str = include_str!("judge_inventory_obs.txt");
const INV_BINS: &str = include_str!("judge_binaries.txt");

fn judge_in_inventory(name: &str) -> bool {
    INV_DEFAULT.lines().any(|l| l.trim_end().split("::").last() == Some(name))
        || INV_OBS.lines().any(|l| l.trim_end().split("::").last() == Some(name))
        || INV_BINS.lines().any(|l| l.trim_end() == name)
}

fn root_law(id: LawId) -> &'static RootLawRow {
    ROOT_LAWS.iter().find(|l| l.id == id).unwrap_or_else(|| panic!("unknown root law {id:?}"))
}

#[test]
fn a1_root_law_rows_integrity() {
    let mut seen: Vec<LawId> = Vec::new();
    for law in ROOT_LAWS {
        assert!(!seen.contains(&law.id), "根律 id 重复：{:?}", law.id);
        seen.push(law.id);
        assert!(!law.proposition.is_empty(), "{:?} 命题空", law.id);
        assert!(!law.anchor.is_empty(), "{:?} 节锚空", law.id);
        assert!(matches!(law.status, "Guarded" | "Asserted" | "Proven"), "{:?} 状态非法", law.id);
        if law.status == "Guarded" {
            assert!(!law.judges.is_empty(), "{:?} Guarded 但判官空", law.id);
        }
    }
}

#[test]
fn a2_root_law_judges_in_inventory() {
    for law in ROOT_LAWS {
        for j in law.judges {
            assert!(judge_in_inventory(j), "根律 {:?} 判官 {} 不在库存", law.id, j);
        }
    }
}

/// A3（S4 升级）：ROW_LAW_MAP == DOMAINS 实际 law 列（强链接，LawId 直接比对）。
#[test]
fn a3_map_covers_all_rows() {
    let names: Vec<&str> = DOMAINS.iter().map(|d| d.name).collect();
    assert_eq!(names.len(), 11, "域行数不是 11，映射表需同步");
    for &n in &names {
        let (_, law_id, _) = *ROW_LAW_MAP.iter().find(|(dn, _, _)| *dn == n).unwrap_or_else(|| panic!("域行 {n} 缺映射"));
        let d = DOMAINS.iter().find(|d| d.name == n).unwrap();
        assert_eq!(
            d.law, law_id,
            "域行 {n} 的 law 列 {:?} 与映射表 {law_id:?} 漂移",
            d.law
        );
    }
}

#[test]
fn a4_a5_judge_discipline() {
    let own: Vec<(&str, Option<&str>)> = DOMAINS.iter().map(|d| (d.name, d.judge)).collect();
    for (name, j) in &own {
        let (_, law_id, _inherit) = ROW_LAW_MAP.iter().find(|(dn, _, _)| dn == name).unwrap();
        let law = root_law(*law_id);
        if let Some(j) = j {
            assert!(law.judges.contains(j), "域行 {} 的 judge {} 不属于律 {:?} 判官集 {:?}", name, j, law_id, law.judges);
        }
    }
    let mut seen: Vec<(&str, &str, bool)> = Vec::new();
    for (name, j) in &own {
        if let Some(j) = j {
            let (_, _, inherit) = *ROW_LAW_MAP.iter().find(|(dn, _, _)| dn == name).unwrap();
            if let Some((prev_name, _, prev_inh)) = seen.iter().find(|(_, pj, _)| pj == j).copied() {
                assert!(inherit || prev_inh, "判官 {} 同时直接写在 {} 与 {} 两行", j, prev_name, name);
                assert!(inherit != prev_inh, "判官 {} 在 {} 与 {} 上同为继承或同为直接", j, prev_name, name);
            }
            seen.push((name, j, inherit));
        }
    }
}

#[test]
fn a6_strong_link_prereq() {
    let _: Option<crate::trinity::LawId> = None;
    assert_eq!(std::mem::size_of::<crate::trinity::LawId>(), 1);
    // 21 条（15 + 六根律）+ 结构不变量 I1/I2/I3 = 24。
    // 24 条 + 旧 ATOM_DECLS 19 行 = 43。
    assert_eq!(LAW_TABLE.len(), 43, "LAW_TABLE 现状 43 条（15 + 六根律 + 三不变量 + ATOM_DECLS 19）");
}

/// a8（S1 新增）：六条根律全覆盖——LawId 变体存在、Guarded、判官 ∈ 库存。
#[test]
fn a8_root_laws_registered_and_guarded() {
    use crate::trinity::LawId;
    let expect: [(LawId, &str); 6] = [
        (LawId::RIncrEq, "ripple_equivalence_test"),
        (LawId::RTreeFold, "structure_equivalence_test"),
        (LawId::RZone, "zone_closure_equivalence"),
        (LawId::RSplice, "assembled_full_equals_full_build"),
        (LawId::RSecs, "secs_cell_update_structural_matches_full"),
        (LawId::RRootOrd, "root_blocks_before_identity_matches_cf_tree_roots"),
    ];
    for (id, judge) in expect {
        let law = crate::trinity::status_for(id).unwrap_or_else(|| panic!("{id:?} 未登记"));
        assert_eq!(law.status, LawStatus::Guarded, "{id:?} 应为 Guarded");
        assert_eq!(law.judge, judge, "{id:?} 主判官不符");
        assert!(judge_in_inventory(judge), "{id:?} 判官 {judge} 不在库存");
    }
}

/// a7（S2 改写）：全表 Guarded ⇒ judge ∈ 库存——prose 假指针零容忍。
/// S2 改写：断言「Guarded 的判官必须真实」。
#[test]
fn a7_guarded_judges_all_in_inventory() {
    for law in LAW_TABLE {
        if law.status == LawStatus::Guarded {
            assert!(
                judge_in_inventory(law.judge),
                "{:?} 为 Guarded 但判官 {} 不在库存（prose 假指针）",
                law.id,
                law.judge
            );
        }
    }
}