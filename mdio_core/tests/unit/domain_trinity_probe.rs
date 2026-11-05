// 预实验：domain 登记表 × trinity 律表的接线审计。
//
// 目的：验证路线图二第②步「律名引用」的机器形态——五条审计能否写成会红的断言。
// 边界：不接生产 outlet、不新增 LawId 变体、不改 LAW_TABLE 本体；
//       判官「真入闸」（cargo test -- --list 全量比对）留正式版，此处以白名单占位。

use crate::domain::DOMAINS;
use crate::trinity::{LAW_TABLE, LawId, LawStatus};

/// 判官库存三源（S5：--list 权威源，fixture 由 just regen-judge-inventory 再生成）。
const INV_DEFAULT: &str = include_str!("judge_inventory_default.txt");
const INV_OBS: &str = include_str!("judge_inventory_obs.txt");
const INV_BINS: &str = include_str!("judge_binaries.txt");

fn judge_in_inventory(name: &str) -> bool {
    INV_DEFAULT.lines().any(|l| l.trim_end().split("::").last() == Some(name))
        || INV_OBS.lines().any(|l| l.trim_end().split("::").last() == Some(name))
        || INV_BINS.lines().any(|l| l.trim_end() == name)
}

/// S4 强链接映射：域名 → 根律（读 DOMAINS 真实 law 列，LawId 直接返回）。
fn laws_for(name: &str) -> &'static [LawId] {
    let d = DOMAINS.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("未知域 {name}"));
    match d.law {
        LawId::RZone => &[LawId::RZone],
        LawId::RRootOrd => &[LawId::RRootOrd],
        LawId::RSecs => &[LawId::RSecs],
        LawId::RTreeFold => &[LawId::RTreeFold],
        LawId::RSplice => &[LawId::RSplice],
        other => panic!("域 {name} 的律 {other:?} 不是根律——映射断"),
    }
}

/// 四对并行口径（同一事实的两种实现；等价律的共享断言面）。
const CALIBER_PAIRS: &[(&str, &str)] = &[
    ("zone_auth", "zone_mat"),
    ("root_merge", "root_split"),
    ("node_end_nav", "node_end_rec"),
    ("cont_tail_view", "cont_tail_build"),
];

fn law_entry(id: LawId) -> &'static crate::trinity::Law {
    LAW_TABLE
        .iter()
        .find(|l| l.id == id)
        .unwrap_or_else(|| panic!("LawId {id:?} 不在 LAW_TABLE"))
}

/// 审计①：每个域行至少一条律（新域未映射即红——「地图漂移」的机器拦截）。
#[test]
fn every_domain_has_at_least_one_law() {
    for d in DOMAINS.iter() {
        assert!(
            !laws_for(d.name).is_empty(),
            "域 {} 无律映射：新域必须先补 laws_for + plan",
            d.name
        );
    }
}

/// 审计②：映射到的每条律真实存在于 trinity::LAW_TABLE。
#[test]
fn every_cited_law_exists_in_table() {
    for d in DOMAINS.iter() {
        for law in laws_for(d.name) {
            assert!(
                LAW_TABLE.iter().any(|l| l.id == *law),
                "域 {} 引用的律 {law:?} 不在 LAW_TABLE",
                d.name
            );
        }
    }
}

/// 审计③（S5 升级）：自带 judge 的行，judge 名必须 ∈ 判官库存（--list 三源，真实入闸）。
#[test]
fn self_judged_rows_cite_known_tests() {
    for d in DOMAINS.iter() {
        if let Some(j) = d.judge {
            assert!(
                judge_in_inventory(j),
                "域 {} 的判官 {j} 不在判官库存（--list 三源）",
                d.name
            );
        }
    }
}

/// 审计④：judge=None 的行（走根律继承），其映射律必须「已证 / 断言 / 被守且判官指针非空」——
/// 继承链不允许落到一条自身也无判官的律上（计划的失败判据：律引用只剩名字没有判官）。
#[test]
fn inherited_rows_map_to_non_empty_laws() {
    for d in DOMAINS.iter() {
        if d.judge.is_none() {
            for law in laws_for(d.name) {
                let e = law_entry(*law);
                let ok = matches!(e.status, LawStatus::Proven | LawStatus::Asserted)
                    || (e.status == LawStatus::Guarded && e.judge != "-");
                assert!(
                    ok,
                    "域 {} 无判官，映射律 {:?}（status={:?} judge={}）继承链为空",
                    d.name, law, e.status, e.judge
                );
            }
        }
    }
}

/// 审计⑤（S3 改写）：每对并行口径两成员的律都必须有入闸判官——
/// 共享面已下沉到根律层（root_merge=RRootOrd / root_split=RSecs 各自带真实判官）。
#[test]
fn caliber_pairs_have_judge_backed_laws() {
    for (a, b) in CALIBER_PAIRS {
        for m in [a, b] {
            for law in laws_for(m) {
                let e = law_entry(*law);
                let ok = (e.status == LawStatus::Guarded && e.judge != "-")
                    || matches!(e.status, LawStatus::Proven | LawStatus::Asserted);
                assert!(
                    ok,
                    "口径成员 {m} 的律 {law:?}（status={:?} judge={}）无入闸判官",
                    e.status, e.judge
                );
            }
        }
    }
}

