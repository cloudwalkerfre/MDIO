//! 复盘修复方案 testbed 验证——不动生产：
//! 在这里用「拟议新规则」的本地副本跑状态空间网格 + 合成靶子，全绿后才把逻辑搬进 lib。

use std::collections::{HashMap, HashSet};
use mdio_dircheck::{Edge, LAYERS, OPEN_DECL, CLOSED_VERIFY, use_item_modules, scan_dir};

// ── 拟议 check_v0_v2（三态分派 + test 档位 + 对照层文案）──

fn check_v2(edges: &[Edge], outlets: &HashSet<String>) -> Vec<String> {
    let lm: HashMap<&str, u8> = LAYERS.iter().cloned().collect();
    let mut out = Vec::new();
    for (from, to, kind, test) in edges {
        if kind == "root_item" { continue; }
        // 登记判定 from：三态分派（未登记 → 违规，先于 to 侧任何判定）
        let in_layers = LAYERS.iter().any(|(n, _)| n == from);
        let in_open = OPEN_DECL.contains(&from.as_str());
        let in_closed = CLOSED_VERIFY.contains(&from.as_str());
        let lf: u8 = match (in_layers, in_open, in_closed) {
            (true, _, _) => *lm.get(from.as_str()).unwrap(),
            (false, true, _) | (false, _, true) => u8::MAX, // 声明/对照层 = 最高层特权
            (false, false, false) => {
                out.push(format!("{from}: 模块未登记，请登记进 LAYERS/OPEN_DECL/CLOSED_VERIFY"));
                continue;
            }
        };
        // to 侧：open 放行、特许接触面放行、closed 对照层违规、未登记、方向判定
        if OPEN_DECL.contains(&to.as_str()) { continue; }
        if mdio_dircheck::SANCTIONED_CONTACT.iter().any(|(f, t, _)| f == from && t == to) { continue; }
        if CLOSED_VERIFY.contains(&to.as_str()) {
            out.push(format!("{from}->{to}: 生产不得引用对照层"));
            continue;
        }
        let Some(tl) = lm.get(to.as_str()).copied() else {
            out.push(format!("{from}->{to}: 模块未登记"));
            continue;
        };
        if tl > lf && !outlets.contains(to) {
            let tier = if *test { "[test] " } else { "" };
            out.push(format!("{from}->{to}: {tier}向上引用未登记（L{lf}→L{tl}），请登记 domain outlet 或下移实现"));
        }
    }
    out
}

fn e(f: &str, t: &str, kind: &str, test: bool) -> Edge {
    (f.to_string(), t.to_string(), kind.to_string(), test)
}

// ── 1. 状态空间网格（from × to × 登记态 × test）──

#[test]
fn grid_from_unregistered_is_violation() {
    let out: HashSet<String> = ["incr".into()].into_iter().collect();
    // 未登记 from × 任意 to → 「模块未登记」，且优先于方向判定
    for (t, kind) in [("session", "use"), ("block", "path"), ("domain", "use"), ("sdl", "path")] {
        let v = check_v2(&[e("foo", t, kind, false)], &out);
        assert_eq!(v.len(), 1, "foo->{t} {v:?}");
        assert!(v[0].contains("foo: 模块未登记"), "{v:?}");
    }
}

#[test]
fn grid_registered_cells_match_expectation() {
    let out: HashSet<String> = ["incr".into()].into_iter().collect();
    // from=OPEN（domain）→ 最高层特权：向下看一切合法
    assert!(check_v2(&[e("domain", "topology", "use", false)], &out).is_empty());
    assert!(check_v2(&[e("domain", "session", "use", false)], &out).is_empty());
    // from=CLOSED（trinity）→ 同理（对照层消费者）
    assert!(check_v2(&[e("trinity", "infer", "path", false)], &out).is_empty());
    // to=CLOSED → 生产不得引用对照层（新文案）
    let v = check_v2(&[e("cache", "sdl", "use", false)], &out);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("生产不得引用对照层"), "{v:?}");
    // 特许接触面（S4）：domain→trinity 放行；其余 closed 目标仍红
    assert!(check_v2(&[e("domain", "trinity", "type", false)], &out).is_empty());
    let v = check_v2(&[e("engine", "trinity", "use", false)], &out);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("生产不得引用对照层"), "{v:?}");
    // to=OPEN → 放行
    assert!(check_v2(&[e("projection", "domain", "use", false)], &out).is_empty());
    // to 未登记 → 模块未登记
    let v = check_v2(&[e("cache", "bar", "path", false)], &out);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("模块未登记"), "{v:?}");
    // 向上未豁免 → 违规；命中 outlet → 放行
    let v = check_v2(&[e("cache", "session", "path", false)], &out);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("向上引用未登记"), "{v:?}");
    assert!(check_v2(&[e("cache", "incr", "path", false)], &out).is_empty());
    // 向下/同层恒合法
    assert!(check_v2(&[e("session", "cache", "path", false)], &out).is_empty());
    assert!(check_v2(&[e("cache", "incr", "path", false)], &out).is_empty());
    assert!(check_v2(&[e("cache", "projection", "path", false)], &out).is_empty());
    // root_item 豁免
    assert!(check_v2(&[e("op", "PERF_LOCK", "root_item", true)], &out).is_empty());
}

#[test]
fn grid_test_tier_annotated() {
    let out: HashSet<String> = HashSet::new();
    let v = check_v2(&[e("cache", "session", "path", true)], &out);
    assert_eq!(v.len(), 1);
    assert!(v[0].contains("[test]"), "test 档位缺失: {v:?}");
    let v2 = check_v2(&[e("cache", "session", "path", false)], &out);
    assert!(!v2[0].contains("[test]"), "生产边误标 test 档: {v2:?}");
}

// ── 2. use 形状（含 as 修复的本地副本）──

fn use_mods_v2(i: &syn::ItemUse) -> Vec<String> {
    use quote::ToTokens;
    let s = i.to_token_stream().to_string().replace(" :: ", "::");
    let body = s.trim().trim_start_matches("use ").trim_end_matches(';').trim();
    let Some(rest) = body.strip_prefix("crate::") else { return vec![] };
    if rest.starts_with('{') {
        let inner = rest.trim_start_matches('{').trim_end_matches('}');
        inner.split(',').filter_map(|p| {
            let p = p.trim().trim_start_matches("self::").trim_start_matches("super::");
            p.split("::").next().filter(|x| !x.is_empty()).map(|x| x.split(" as ").next().unwrap_or(x).to_string()).map(|x| x.strip_prefix("r#").unwrap_or(&x).to_string())
        }).collect()
    } else {
        rest.split("::").next().filter(|x| !x.is_empty()).map(|x| x.split(" as ").next().unwrap_or(x).to_string()).map(|x| x.strip_prefix("r#").unwrap_or(&x).to_string()).into_iter().collect()
    }
}

#[test]
fn use_shapes_v2() {
    let a: syn::ItemUse = syn::parse_quote!(use crate::a::b;);
    assert_eq!(use_mods_v2(&a), vec!["a"]);
    let b: syn::ItemUse = syn::parse_quote!(use crate::{a::b, c};);
    assert_eq!(use_mods_v2(&b), vec!["a", "c"]);
    let c: syn::ItemUse = syn::parse_quote!(use crate::a as aa;);
    assert_eq!(use_mods_v2(&c), vec!["a"], "as 重命名应取原模块");
    let d: syn::ItemUse = syn::parse_quote!(use crate::r#match::{Patch, Claim};);
    assert_eq!(use_mods_v2(&d), vec!["match"]);
    let f: syn::ItemUse = syn::parse_quote!(use crate::a::{self, b};);
    assert_eq!(use_mods_v2(&f), vec!["a"], "self 组走首段路径");
    let g: syn::ItemUse = syn::parse_quote!(use crate::a::{b::{c, d}};);
    assert_eq!(use_mods_v2(&g), vec!["a"], "深组走首段路径");
}

// ── 3. 项级 cfg(test) 跟踪（五类 item）──

use syn::visit::Visit;
struct ProbeC {
    edges: Vec<Edge>,
    in_test: bool,
}
impl ProbeC {
    fn hit(&mut self, to: &str) {
        self.edges.push(("probe".into(), to.to_string(), "path".into(), self.in_test));
    }
    fn with_test(&mut self, attrs: &[syn::Attribute], f: impl FnOnce(&mut Self)) {
        let saved = self.in_test;
        let is = attrs.iter().any(|a| {
            a.path().is_ident("cfg") && a.meta.require_list().map(|l| l.tokens.to_string().contains("test")).unwrap_or(false)
        });
        self.in_test = saved || is;
        f(self);
        self.in_test = saved;
    }
}
impl<'ast> Visit<'ast> for ProbeC {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        let saved = self.in_test;
        let is = m.attrs.iter().any(|a| {
            a.path().is_ident("cfg") && a.meta.require_list().map(|l| l.tokens.to_string().contains("test")).unwrap_or(false)
        });
        self.in_test = saved || m.ident == "tests" || is;
        syn::visit::visit_item_mod(self, m);
        self.in_test = saved;
    }
    fn visit_item_fn(&mut self, i: &'ast syn::ItemFn) {
        self.with_test(&i.attrs, |s| syn::visit::visit_item_fn(s, i));
    }
    fn visit_item_const(&mut self, i: &'ast syn::ItemConst) {
        self.with_test(&i.attrs, |s| syn::visit::visit_item_const(s, i));
    }
    fn visit_item_static(&mut self, i: &'ast syn::ItemStatic) {
        self.with_test(&i.attrs, |s| syn::visit::visit_item_static(s, i));
    }
    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        self.with_test(&i.attrs, |s| syn::visit::visit_item_impl(s, i));
    }
    fn visit_item_trait(&mut self, i: &'ast syn::ItemTrait) {
        self.with_test(&i.attrs, |s| syn::visit::visit_item_trait(s, i));
    }
    fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
        if p.path.segments.len() >= 2 && p.path.segments[0].ident == "crate" {
            self.hit(&p.path.segments[1].ident.to_string());
        }
        syn::visit::visit_expr_path(self, p);
    }
}

#[test]
fn item_level_test_tracking() {
    let src = r#"
        #[cfg(test)] fn t1() { let _ = crate::a::x(); }
        fn p1() { let _ = crate::b::x(); }
        #[cfg(test)] const T2: () = { let _ = crate::c::x(); };
        static P2: () = { let _ = crate::d::x(); };
        #[cfg(test)] mod m { fn t3() { let _ = crate::e::x(); } }
    "#;
    let f = syn::parse_file(src).unwrap();
    let mut c = ProbeC { edges: vec![], in_test: false };
    c.visit_file(&f);
    for (_, to, _, test) in &c.edges {
        let want = matches!(to.as_str(), "a" | "c" | "e");
        assert_eq!(*test, want, "to={to}: 期望 test={want}");
    }
    assert!(!c.edges.is_empty());
}

// ── 4. B 清单完整性：lib 的文档化清单（INTRA_LAYER_PAIRS）== 实测同层对 ──

#[test]
fn sanctioned_list_equals_scanned_same_layer_pairs() {
    let dir = format!("{}/../../mdio_core/src", env!("CARGO_MANIFEST_DIR"));
    let (_mods, edges) = scan_dir(std::path::Path::new(&dir));
    let lm: HashMap<&str, u8> = LAYERS.iter().cloned().collect();
    let mut actual: Vec<(&str, &str)> = Vec::new();
    for (f, t, kind, _test) in &edges {
        if kind == "root_item" { continue; }
        let (Some(lf), Some(lt)) = (lm.get(f.as_str()), lm.get(t.as_str())) else { continue };
        if lf == lt && f != t {
            let pair = if f < t { (f.as_str(), t.as_str()) } else { (t.as_str(), f.as_str()) };
            if !actual.contains(&pair) { actual.push(pair); }
        }
    }
    actual.sort();
    let mut want: Vec<(&str, &str)> = mdio_dircheck::INTRA_LAYER_PAIRS
        .iter()
        .map(|(a, b, _)| if a < b { (*a, *b) } else { (*b, *a) })
        .collect();
    want.sort();
    want.dedup();
    assert_eq!(actual, want, "文档清单与实测同层对漂移");
}
