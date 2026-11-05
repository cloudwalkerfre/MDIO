//! mdio-dircheck 库：方向闸的表、规则与扫描器。
//! 不 import mdio_core——事实全部来自源码文本；probe（tests/unit）经 dev-dep 引用本库同一份实现。

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;
use syn::visit::Visit;

// ── 层表 v1.2（执行面：L0-L5 共 24 模块；arrow 为 L4 纯 Rust 门面）──

pub const LAYERS: &[(&str, u8)] = &[
    ("block", 0), ("inline", 0), ("effect", 0), ("utils", 0), ("protocol", 0),
    ("vfs", 0), ("snap", 0), ("telemetry", 0),
    ("topology", 1),
    ("infer", 2),
    ("projection", 3), ("selector", 3), ("op", 3), ("cache", 3), ("incr", 3), ("match", 3),
    ("session", 4), ("store", 4), ("backend", 4), ("worktree", 4), ("hook", 4), ("engine", 4),
    ("arrow", 4),
    ("lib", 5),
];

/// open 声明层：任何层可引用（photocraft Standalone 的同款语义）。
pub const OPEN_DECL: &[&str] = &["domain", "dguard"];

/// closed 对照层：生产不得引用（dguard 禁名另守；方向闸报「生产不得引用对照层」）。
pub const CLOSED_VERIFY: &[&str] = &["sdl", "trilogy", "trinity"];

/// 特许接触面对（domain step2 S4 定案）：大框架声明的接触面之一——域声明表引用对照层的律名。
pub const SANCTIONED_CONTACT: &[(&str, &str, &str)] = &[
    ("domain", "trinity", "路线图二接触面：律名引用（LawId）——大框架声明唯一接触面之一"),
];

/// 文档化同层清单（复盘研究结论 B：降级为文档，非放行开关）——实测 26 个同层对
/// 全为架构本体核环（镜像表/增量核/视图三兄弟/运行时双核），侧向禁止属 S6 候选；
/// probe d2 断言「本清单与实测同层对逐对相等」，重构随删行。
pub const INTRA_LAYER_PAIRS: &[(&str, &str, &str)] = &[
    ("backend", "engine", "运行时层：后端任务调度与引擎互相喂数据"),
    ("backend", "session", "运行时层：后端驱动会话生命周期"),
    ("backend", "store", "运行时层：后端读写作坊"),
    ("backend", "worktree", "运行时层：后端读写工作区"),
    ("block", "effect", "L0 原子层：行扫描与效应表互为镜像"),
    ("block", "inline", "L0 原子层：块行内表示复用 inline helper"),
    ("block", "telemetry", "L0 原子层：块层打点"),
    ("cache", "incr", "L3 增量核：缓存与增量互喂"),
    ("cache", "op", "L3 视图层：缓存执行 op 族"),
    ("cache", "projection", "L3 视图层：缓存与投影对拍"),
    ("cache", "selector", "L3 视图层：缓存消费选择器"),
    ("effect", "inline", "L0 原子层：效应表内联表示"),
    ("effect", "telemetry", "L0 原子层：效应层打点"),
    ("engine", "hook", "运行时层：引擎触发钩子"),
    ("engine", "session", "运行时层：引擎与会话对拍"),
    ("engine", "store", "运行时层：引擎读写作坊"),
    ("hook", "store", "运行时层：钩子读写作坊"),
    ("incr", "match", "L3 视图层：增量与匹配对拍"),
    ("incr", "projection", "L3 视图层：增量喂投影"),
    ("incr", "selector", "L3 视图层：增量渲染 parity 测试引用选择器身份产品"),
    ("match", "projection", "L3 视图层：匹配与投影对拍"),
    ("match", "selector", "L3 视图层：匹配消费选择器"),
    ("op", "projection", "L3 视图层：op 族投影对拍"),
    ("op", "selector", "L3 视图层：op 族消费选择器"),
    ("projection", "selector", "L3 视图层：投影消费选择器"),
    ("session", "store", "运行时双核：会话读写作坊"),
    ("store", "worktree", "运行时层：作坊读写工作区"),
];

/// 全量模块名（29 = LAYERS 24 + OPEN 2 + CLOSED 3）：三表并集单源，probe 引用防漂移。
pub const ALL_MODULES: &[&str] = &[
    "arrow", "backend", "block", "cache", "domain", "dguard", "effect", "engine", "hook",
    "incr", "infer", "inline", "lib", "match", "op", "projection", "protocol", "sdl",
    "selector", "session", "snap", "store", "telemetry", "topology",
    "trilogy", "trinity", "utils", "vfs", "worktree",
];

pub fn layer_of(m: &str) -> Option<u8> {
    LAYERS.iter().find(|(n, _)| *n == m).map(|(_, l)| *l)
}

// ── 规则纯函数 ──

/// 通用规则（合成层表 smoke 用）。
pub fn check_direction(imports: &[(&str, &str)], layers: &[(&str, u8)], outlets: &[&str]) -> Vec<String> {
    let lm: std::collections::HashMap<&str, u8> = layers.iter().cloned().collect();
    let mut out = Vec::new();
    for (from, to) in imports {
        let (lf, lt) = match (lm.get(from), lm.get(to)) {
            (Some(f), Some(t)) => (*f, *t),
            _ => {
                out.push(format!("{from}->{to}: 模块未登记"));
                continue;
            }
        };
        if lt > lf && !outlets.iter().any(|o| o.starts_with(&format!("{to}::"))) {
            out.push(format!("{from}->{to}: 向上引用未登记（L{lf}→L{lt}），请登记 domain outlet 或下移实现"));
        }
    }
    out
}

/// 边四元组：from, to, kind（use/path/type/root_item）, is_test。
pub type Edge = (String, String, String, bool);

/// 方向规则 v2（复盘研究结论）：两阶段判定——登记判定（from 三态分派先于一切）+ 方向判定。
/// 三态：LAYERS 取层号；OPEN/CLOSED 取最高层特权（声明/对照层可向下看一切）；其余 → 未登记违规。
/// test 边同判不豁免，违规文案带 [test] 档位。
pub fn check_v0(edges: &[Edge], outlets: &HashSet<String>) -> Vec<String> {
    let lm: std::collections::HashMap<&str, u8> = LAYERS.iter().cloned().collect();
    let mut out = Vec::new();
    for (from, to, kind, test) in edges {
        if kind == "root_item" {
            continue; // crate 根符号（PERF_LOCK/telemetry_*）：不进方向判定
        }
        // 登记判定 from：未登记 → 违规（先于 to 侧任何判定）
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
        // to 侧：open 放行；特许接触面放行；closed 对照层专属违规；未登记；方向判定
        if OPEN_DECL.contains(&to.as_str()) {
            continue;
        }
        if SANCTIONED_CONTACT.iter().any(|(f, t, _)| f == from && t == to) {
            continue; // 大框架声明的接触面（domain→trinity 律名引用等）
        }
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

// ── 扫描器 ──

pub fn normalize(s: &str) -> String {
    s.strip_prefix("r#").unwrap_or(s).to_string()
}

fn attr_is_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.meta.require_list().map(|l| l.tokens.to_string().contains("test")).unwrap_or(false)
    })
}

/// use 语句的目标模块集（token 流形态）。段内切 ` as `（重命名取原模块）；raw ident 归一。
pub fn use_item_modules(i: &syn::ItemUse) -> Vec<String> {
    use quote::ToTokens;
    let s = i.to_token_stream().to_string().replace(" :: ", "::");
    let body = s.trim().trim_start_matches("use ").trim_end_matches(';').trim();
    let Some(rest) = body.strip_prefix("crate::") else { return vec![] };
    let seg = |x: &str| x.split(" as ").next().unwrap_or(x).to_string();
    if rest.starts_with('{') {
        let inner = rest.trim_start_matches('{').trim_end_matches('}');
        inner.split(',').filter_map(|p| {
            let p = p.trim().trim_start_matches("self::").trim_start_matches("super::");
            p.split("::").next().filter(|x| !x.is_empty()).map(|x| normalize(&seg(x)))
        }).collect()
    } else {
        rest.split("::").next().filter(|x| !x.is_empty()).map(|x| normalize(&seg(x))).into_iter().collect()
    }
}

fn crate_target(path: &syn::Path) -> Option<&syn::Ident> {
    if path.leading_colon.is_none() && path.segments.len() >= 2 && path.segments[0].ident == "crate" {
        Some(&path.segments[1].ident)
    } else { None }
}

struct Collector<'a> {
    edges: &'a mut Vec<Edge>,
    modules: &'a BTreeSet<String>,
    from: String,
    in_test: bool,
}

impl Collector<'_> {
    fn add(&mut self, to: &str, kind: &str) {
        if to == self.from {
            return; // 自引用（crate::telemetry 于 telemetry.rs 内等）不进方向判定
        }
        let kind = if self.modules.contains(to) { kind } else { "root_item" };
        self.edges.push((self.from.clone(), to.to_string(), kind.to_string(), self.in_test));
    }
    fn with_test(&mut self, attrs: &[syn::Attribute], f: impl FnOnce(&mut Self)) {
        let saved = self.in_test;
        self.in_test = saved || attr_is_test(attrs);
        f(self);
        self.in_test = saved;
    }
}

impl<'ast> Visit<'ast> for Collector<'_> {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        self.with_test(&m.attrs, |s| {
            if m.ident == "tests" { s.in_test = true; }
            syn::visit::visit_item_mod(s, m);
        });
    }
    fn visit_item_use(&mut self, i: &'ast syn::ItemUse) {
        self.with_test(&i.attrs, |s| {
            for m in use_item_modules(i) { s.add(&m, "use"); }
            syn::visit::visit_item_use(s, i);
        });
    }
    // H 修复：五类含代码 item 的 cfg(test) 项级跟踪（fn/const/static/impl/trait）
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
        if let Some(to) = crate_target(&p.path) { self.add(&normalize(&to.to_string()), "path"); }
        syn::visit::visit_expr_path(self, p);
    }
    fn visit_type_path(&mut self, p: &'ast syn::TypePath) {
        if let Some(to) = crate_target(&p.path) { self.add(&normalize(&to.to_string()), "type"); }
        syn::visit::visit_type_path(self, p);
    }
}

/// 扫描 src 目录：返回（模块名列表，边集）。
pub fn scan_dir(dir: &Path) -> (Vec<String>, Vec<Edge>) {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).expect("read_dir") {
        let p = entry.expect("entry").path();
        if p.extension().and_then(|e| e.to_str()) != Some("rs") { continue; }
        files.push(p);
    }
    files.sort();
    let mut modules = Vec::new();
    let mut module_set = BTreeSet::new();
    for p in &files {
        modules.push(p.file_stem().unwrap().to_str().unwrap().to_string());
    }
    module_set.extend(modules.iter().cloned());
    let mut edges = Vec::new();
    for p in &files {
        let name = p.file_stem().unwrap().to_str().unwrap().to_string();
        let src = fs::read_to_string(p).expect("read");
        let file = syn::parse_file(&src).unwrap_or_else(|e| panic!("{name}.rs: {e}"));
        let mut c = Collector { edges: &mut edges, modules: &module_set, from: name, in_test: false };
        c.visit_file(&file);
    }
    edges.sort();
    edges.dedup();
    (modules, edges)
}

/// 从 domain.rs 文本解析 (name, outlet) 对。
pub fn parse_outlets(src: &str) -> Vec<(String, String)> {
    let f = syn::parse_file(src).expect("domain.rs 解析失败");
    let mut out = Vec::new();
    for item in &f.items {
        let syn::Item::Static(s) = item else { continue };
        if s.ident != "DOMAINS" { continue; }
        let syn::Expr::Array(arr) = &*s.expr else { continue };
        for el in &arr.elems {
            let syn::Expr::Struct(st) = el else { continue };
            let mut name = None;
            let mut outlet = None;
            for fld in &st.fields {
                let syn::Member::Named(ident) = &fld.member else { continue };
                if ident == "name" {
                    if let syn::Expr::Lit(l) = &fld.expr {
                        if let syn::Lit::Str(s) = &l.lit { name = Some(s.value()); }
                    }
                } else if ident == "outlet" {
                    if let syn::Expr::Call(c) = &fld.expr {
                        if let Some(arg) = c.args.first() {
                            if let syn::Expr::Lit(l) = arg {
                                if let syn::Lit::Str(s) = &l.lit { outlet = Some(s.value()); }
                            }
                        }
                    }
                }
            }
            if let (Some(n), Some(o)) = (name, outlet) { out.push((n, o)); }
        }
    }
    out
}

/// outlet 的模块集（首段）。
pub fn outlet_modules_from(src: &str) -> HashSet<String> {
    parse_outlets(src).into_iter().map(|(_, o)| o.split("::").next().unwrap().to_string()).collect()
}

/// 门禁：扫描 + check_v0 + 规范化（排序去重）→ 违规清单。
pub fn gate_violations(dir: &Path) -> Vec<String> {
    let (_modules, edges) = scan_dir(dir);
    let domain_src = fs::read_to_string(dir.join("domain.rs")).expect("domain.rs");
    let outlets = outlet_modules_from(&domain_src);
    let mut v = check_v0(&edges, &outlets);
    v.sort();
    v.dedup();
    v
}

/// 棘轮比对：新增违规 → Err（列出）；基线修复（当前无此条目）→ Err（提示删基线行）；一致 → Ok。
pub fn compare_violations(current: &[String], baseline: &str) -> Result<(), String> {
    let base: std::collections::BTreeSet<&str> = baseline.lines().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
    let cur: std::collections::BTreeSet<&str> = current.iter().map(|s| s.as_str()).collect();
    let mut errs = Vec::new();
    for c in &cur {
        if !base.contains(c) {
            errs.push(format!("新增违规: {c}"));
        }
    }
    for b in &base {
        if !cur.contains(b) {
            errs.push(format!("基线条目已消失（已修复）——请从 baseline 删除该行: {b}"));
        }
    }
    if errs.is_empty() { Ok(()) } else { Err(errs.join("\n")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_two_shapes() {
        let f: syn::ItemUse = syn::parse_quote!(use crate::a::b;);
        assert_eq!(use_item_modules(&f), vec!["a".to_string()]);
        let g: syn::ItemUse = syn::parse_quote!(use crate::{a::b, c};);
        assert_eq!(use_item_modules(&g), vec!["a".to_string(), "c".to_string()]);
    }

    #[test]
    fn raw_ident_normalized() {
        let g: syn::ItemUse = syn::parse_quote!(use crate::r#match::Patch;);
        assert_eq!(use_item_modules(&g), vec!["match".to_string()]);
    }

    #[test]
    fn as_rename_takes_original_module() {
        let c: syn::ItemUse = syn::parse_quote!(use crate::a as aa;);
        assert_eq!(use_item_modules(&c), vec!["a".to_string()]);
    }

    #[test]
    fn unregistered_from_is_violation() {
        let outlets: HashSet<String> = HashSet::new();
        let v = check_v0(&[("foo".into(), "session".into(), "use".into(), false)], &outlets);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("foo: 模块未登记"), "{v:?}");
        // 未登记 from × open 目标：登记判定仍优先
        let v = check_v0(&[("foo".into(), "domain".into(), "use".into(), false)], &outlets);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("foo: 模块未登记"), "{v:?}");
    }

    #[test]
    fn closed_verify_target_message() {
        let outlets: HashSet<String> = HashSet::new();
        let v = check_v0(&[("cache".into(), "sdl".into(), "use".into(), false)], &outlets);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("生产不得引用对照层"), "{v:?}");
        // 特许接触面：domain→trinity 放行；其余 closed 目标仍红
        let v = check_v0(&[("domain".into(), "trinity".into(), "use".into(), false)], &outlets);
        assert!(v.is_empty(), "{v:?}");
        let v = check_v0(&[("engine".into(), "trinity".into(), "use".into(), false)], &outlets);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("生产不得引用对照层"), "{v:?}");
    }

    #[test]
    fn check_v0_rules() {
        let outlets: HashSet<String> = ["incr".to_string()].into_iter().collect();
        // 向上未登记 → 违规
        let v = check_v0(&[("cache".into(), "session".into(), "path".into(), false)], &outlets);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("登记 domain outlet"));
        // 向上命中 outlet → 放行
        let v = check_v0(&[("cache".into(), "incr".into(), "path".into(), false)], &outlets);
        assert!(v.is_empty(), "{v:?}");
        // open_decl 目标 → 放行
        let v = check_v0(&[("projection".into(), "domain".into(), "use".into(), false)], &outlets);
        assert!(v.is_empty(), "{v:?}");
        // root_item → 豁免
        let v = check_v0(&[("op".into(), "PERF_LOCK".into(), "root_item".into(), true)], &outlets);
        assert!(v.is_empty(), "{v:?}");
        // 未登记目标 → 违规
        let v = check_v0(&[("a".into(), "b".into(), "path".into(), false)], &outlets);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("未登记"));
    }

    #[test]
    fn ratchet_three_states() {
        let cur = ["x->y: 向上引用未登记".to_string()];
        // 一致 → Ok
        assert!(compare_violations(&cur, "x->y: 向上引用未登记\n").is_ok());
        // 新增违规 → Err
        let e = compare_violations(&cur, "").unwrap_err();
        assert!(e.contains("新增违规"), "{e}");
        // 基线修复（条目消失）→ Err
        let e = compare_violations(&[], "x->y: 向上引用未登记\n").unwrap_err();
        assert!(e.contains("基线条目已消失"), "{e}");
        // 注释与空行不参与比对
        assert!(compare_violations(&cur, "# 真债挂账\n\nx->y: 向上引用未登记\n").is_ok());
    }
}
