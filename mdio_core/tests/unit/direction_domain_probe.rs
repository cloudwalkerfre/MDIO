// 预实验 v1.1 + S2 单源化：
// 方向规则 × domain（outlet 豁免清单形态）。表与规则引用 mdio_dircheck（dev-dep，单源）；
// DOMAINS 侧（outlet 派生 / fixture / 断言语）留本文件。

use std::collections::HashSet;
use crate::domain::DOMAINS;
use mdio_dircheck::{layer_of, check_direction, check_v0, scan_dir};
use mdio_dircheck::{ALL_MODULES, CLOSED_VERIFY, INTRA_LAYER_PAIRS, OPEN_DECL, LAYERS};

/// 四对并行口径（step1 并行线）。
const CALIBER_PAIRS: &[(&str, &str)] = &[
    ("zone_auth", "zone_mat"),
    ("root_merge", "root_split"),
    ("node_end_nav", "node_end_rec"),
    ("cont_tail_view", "cont_tail_build"),
];

/// outlet 字符串 → (模块名, 符号名)。
fn outlet_module_symbol(outlet: &str) -> (&str, &str) {
    let segs: Vec<&str> = outlet.split("::").collect();
    assert!(segs.len() >= 2, "outlet {outlet} 不是 module::symbol 形状");
    (segs[0], segs[segs.len() - 1])
}

fn outlet_of(name: &str) -> &str {
    DOMAINS.iter().find(|d| d.name == name).and_then(|d| d.outlet).unwrap_or("")
}

/// 豁免清单从 DOMAINS 运行时派生——人类知识侧。
fn derived_outlet_modules() -> HashSet<&'static str> {
    DOMAINS.iter().filter_map(|d| d.outlet.map(|o| o.split("::").next().unwrap())).collect()
}

fn derived_set() -> HashSet<String> {
    derived_outlet_modules().into_iter().map(|s| s.to_string()).collect()
}

// ── A-E 断言（v1.0 保留 + E5 派生 + I 交叉）──

/// A：层表 + open/closed 覆盖全部 29 个模块。
#[test]
fn a_layer_table_covers_all_modules() {
    for m in ALL_MODULES {
        let hit = layer_of(m).is_some() || OPEN_DECL.contains(m) || CLOSED_VERIFY.contains(m);
        assert!(hit, "模块 {m} 不在层表 / open / closed 任何一档");
    }
}

/// a2（复盘缺口 A）：未登记 from 模块 → 「模块未登记」违规（先于方向判定）。
#[test]
fn a2_unregistered_from_is_violation() {
    let outlets: HashSet<String> = HashSet::new();
    let v = check_v0(&[("foo".into(), "session".into(), "use".into(), false)], &outlets);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("foo: 模块未登记"), "{v:?}");
    // 未登记 from × open 目标：登记判定仍优先
    let v = check_v0(&[("foo".into(), "domain".into(), "use".into(), false)], &outlets);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("foo: 模块未登记"), "{v:?}");
}

/// B：11 个 outlet 符号在声称的模块里真实存在（include_str 文本验证；正式版升级编译期符号引用）。
#[test]
fn b_outlet_symbols_exist_in_claimed_modules() {
    for d in DOMAINS.iter() {
        let Some(o) = d.outlet else { continue };
        let (modu, sym) = outlet_module_symbol(o);
        let src = match modu {
            "incr" => include_str!("../../src/incr.rs"),
            "cache" => include_str!("../../src/cache.rs"),
            "projection" => include_str!("../../src/projection.rs"),
            "infer" => include_str!("../../src/infer.rs"),
            "block" => include_str!("../../src/block.rs"),
            other => panic!("域 {} 的 outlet 模块 {other} 未列入文本验证面", d.name),
        };
        let needle = format!("fn {sym}");
        assert!(src.contains(&needle), "域 {} 的 outlet {o}: 符号 fn {sym} 不在 {modu}.rs", d.name);
    }
}

/// C：四对口径的 outlet 模块必须同层（两口径判官同处可对拍的结构前提）。
#[test]
fn c_caliber_pairs_live_in_same_layer() {
    for (a, b) in CALIBER_PAIRS {
        let oa = outlet_of(a);
        let ob = outlet_of(b);
        // A4-follow：node_end_rec 的 record_direct 已无独立出口符号（span_of 无独立导出）。
        // 该对只保留并行口径登记；无出口符号可做同层裁决，跳过而非 panic。
        if oa.is_empty() || ob.is_empty() {
            continue;
        }
        let (ma, _) = outlet_module_symbol(oa);
        let (mb, _) = outlet_module_symbol(ob);
        let la = layer_of(ma).unwrap_or_else(|| panic!("{ma} 不在层表"));
        let lb = layer_of(mb).unwrap_or_else(|| panic!("{mb} 不在层表"));
        assert_eq!(la, lb, "口径对 {a}/{b} 的 outlet 模块 {ma}(L{la})/{mb}(L{lb}) 不同层");
    }
}

/// D：文档化清单每对带理由、模块在层表、且同层。
#[test]
fn d_whitelist_pairs_have_reasons_and_same_layer() {
    for (a, b, reason) in INTRA_LAYER_PAIRS {
        assert!(!reason.is_empty(), "{a}/{b} 缺理由");
        assert!(layer_of(a).is_some() && layer_of(b).is_some(), "{a}/{b} 有模块不在层表");
        assert_eq!(layer_of(a), layer_of(b), "{a}/{b} 白名单必须同层");
    }
}

/// d2（复盘缺口 B）：文档化清单与实测同层对逐对相等——清单 = 现状快照，重构随删行。
#[test]
fn d2_sanctioned_list_equals_scanned_same_layer_pairs() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let (_mods, edges) = scan_dir(&dir);
    let mut actual: Vec<(&str, &str)> = Vec::new();
    for (f, t, kind, _test) in &edges {
        if kind == "root_item" { continue; }
        let (Some(_), Some(_)) = (layer_of(f), layer_of(t)) else { continue };
        if layer_of(f) == layer_of(t) && f != t {
            let pair = if f < t { (f.as_str(), t.as_str()) } else { (t.as_str(), f.as_str()) };
            if !actual.contains(&pair) { actual.push(pair); }
        }
    }
    actual.sort();
    let mut want: Vec<(&str, &str)> = INTRA_LAYER_PAIRS.iter().map(|(a, b, _)| {
        if a < b { (*a, *b) } else { (*b, *a) }
    }).collect();
    want.sort();
    want.dedup();
    assert_eq!(actual, want, "文档清单与实测同层对漂移（新增同层对 → 进清单；消失 → 删行）");
}

/// E1：向上引用未命中 outlet → 违规，文案给修复路径。
#[test]
fn e1_upward_unregistered_is_violation() {
    let v = check_direction(&[("cache", "engine")], LAYERS, &["incr::compute_zone"]);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("登记 domain outlet"), "文案缺修复路径: {}", v[0]);
}

/// E2：向上引用命中 outlet → 放行。
#[test]
fn e2_upward_hits_outlet_is_allowed() {
    let layers = &[("low", 0), ("high", 1)];
    let v = check_direction(&[("low", "high")], layers, &["high::registered_entry"]);
    assert!(v.is_empty(), "{v:?}");
}

/// E3：同层与向下引用恒合法。
#[test]
fn e3_same_layer_and_downward_allowed() {
    let layers = &[("a", 1), ("b", 1), ("c", 0)];
    let v = check_direction(&[("a", "b"), ("a", "c")], layers, &[]);
    assert!(v.is_empty(), "{v:?}");
}

/// E4：未登记模块即违规。
#[test]
fn e4_unregistered_module_is_violation() {
    let v = check_direction(&[("x", "y")], &[], &[]);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("未登记"));
}

/// E5：豁免清单确实由 DOMAINS 运行时派生（四个有出口的模块集合）。
#[test]
fn e5_outlet_modules_derived_from_domains() {
    let derived = derived_outlet_modules();
    let expect: HashSet<&str> = ["cache", "incr", "infer", "projection"].into_iter().collect();
    assert_eq!(derived, expect, "派生出口模块集与预期不符");
}

/// I（S2 新增，焊点②机器面）：扫描器从 domain.rs 源码解析的 outlet 模块集 == DOMAINS 运行时派生集。
#[test]
fn i_scanner_outlets_equal_runtime_derived() {
    let scanned = mdio_dircheck::outlet_modules_from(include_str!("../../src/domain.rs"));
    let derived = derived_set();
    assert_eq!(scanned, derived, "源码解析与运行时派生漂移——注册表被绕过");
}

// ── F/G/H：fixture 闭环（编译器事实 × 人类知识）──

/// v0 扫描产物（实测）：每行 from,to,disposition。
/// disposition ∈ violation | covered | open_decl | root_item（与门禁同构，缺口 I）
const FIXTURE: &str = include_str!("direction_fixture.txt");

fn fixture_rows() -> Vec<(&'static str, &'static str, &'static str)> {
    FIXTURE
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let p: Vec<&str> = l.split(',').collect();
            assert_eq!(p.len(), 3, "fixture 行格式错: {l}");
            (p[0], p[1], p[2])
        })
        .collect()
}

/// fixture 行 → Edge：root_item 档映射为 root_item kind（check_v0 豁免），其余按 path。
fn v0(rows: &[(&str, &str, &str)]) -> Vec<String> {
    let outlets = derived_set();
    let edges: Vec<mdio_dircheck::Edge> = rows
        .iter()
        .map(|(f, t, d)| {
            let kind = if *d == "root_item" { "root_item" } else { "path" };
            (f.to_string(), t.to_string(), kind.to_string(), false)
        })
        .collect();
    check_v0(&edges, &outlets)
}

/// F：fixture 全量过 check_v0，违规数 == 预期（棘轮测试版——每条都已裁决）。
#[test]
fn f_fixture_violation_count_matches() {
    let rows = fixture_rows();
    let v = v0(&rows);
    let expect = rows.iter().filter(|(_, _, d)| *d == "violation").count();
    assert_eq!(v.len(), expect, "违规数漂移: {v:?}");
}

/// G：covered 边（向上且命中 outlet）确实放行——接表的直接证据。
#[test]
fn g_covered_edges_are_allowed() {
    for (f, t, d) in fixture_rows() {
        if d == "covered" {
            let v = v0(&[(f, t, d)]);
            assert!(v.is_empty(), "{f}->{t} 应被 outlet 豁免却报违规: {v:?}");
        }
    }
}

/// H：fixture 处置列与规则输出逐条一致（每条违规都经过裁决，不是裸数字）。
#[test]
fn h_each_row_matches_disposition() {
    for (f, t, d) in fixture_rows() {
        let v = v0(&[(f, t, d)]);
        match d {
            "violation" => {
                assert_eq!(v.len(), 1, "{f}->{t} 预期 {d} 但输出 {v:?}");
            }
            "covered" | "open_decl" | "root_item" => {
                assert!(v.is_empty(), "{f}->{t} 预期 {d} 却报违规: {v:?}");
            }
            other => panic!("fixture 处置列未知值: {other}"),
        }
    }
}
