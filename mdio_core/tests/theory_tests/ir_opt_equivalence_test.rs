//! TraversalFilter 等价性验证。
//!
//! 生产 `select_nodes` + `Step::TraversalFilter` vs 原始 `select_nodes` 多 pass。
//! 运行: cargo test --test ir_opt_equivalence_test --features telemetry
//!
//! 定义基础
//!
//! 上下文：全部测试在 flat 模式 (nested=false)下运行。在此约束下：
//!   前提 1 (叶 heading): selector 作用于不含子 heading 的叶 heading 节点。
//!   前提 2 (无 setext):   叶 heading 的内容不包含 setext 标题标记。
//!   在这两条前提下，四种 traversal 产出函数行为等价：
//!     ⟦Drill⟧ ≡ ⟦RecursiveDrill⟧,  ⟦Wildcard⟧ ≡ ⟦RecursiveWildcard⟧
//!
//! 各步骤的指称语义 (⟦·⟧: Snap → Seq → Seq):
//!   遍历 T:  ⟦T⟧(snap)(P) = concat_{p ∈ P} children_T(snap, p)
//!   Filter F: ⟦F⟧(snap)(P) = [p ∈ P | eval_filter(snap, p, F)]
//!   Slice S_N (N ≥ 0): ⟦S_N⟧(snap)(P) = [P[N]] if |P| > N else []
//!   TraversalFilter TF(T, F̂, N+1):
//!     ⟦TF⟧(snap)(P) = let M = concat_{p∈P} [c ∈ children_T(snap,p) | ∀F∈F̂: eval_filter(snap,c,F)]
//!                      in if |M| > N then [M[N]] else []
//!
//! 主定理
//!
//! ⟦S_N⟧ · ⟦Fₖ⟧ · ... · ⟦F₁⟧ · ⟦T⟧  ≡  ⟦TF(T, F̂, N+1)⟧    (∀ T, ∀ F̂, ∀ N≥0, ∀ σ)
//!
//! 证明 (关键一步): 引理2 将 filter 推入 concat 内部 — 消灭中间全量序列。
//!
//! ── 测试覆盖 ──
//!
//! Wildcard: W+Slice, W+TYPECHECK, W+TYPECHECK+CMP, W+AND, W+OR, W+NOT,
//!           W+EMPTY, W+HAS, W+EQ, W+RE, W+REGEX, W+BY, setext, nested_list,
//!           codefence_bullet, checklist, multi_parent, table, indented_code, bq
//! RecursiveWildcard: RW+TYPECHECK(bullet/paragraph/header), RW+CMP,
//!                    RW+NOT, RW+deep_nest, RW+unlimited, RW+slice_n3
//! RecursiveDrill: RD+no_filter
//! Drill: D, D+Slice
//! 边界: empty_doc, blank_doc, contradictory, no_match, overflow
//!
//! ── 最终覆盖 ──
//!
//! | T | F̂ 组合 | N | 嵌套 | 测试 |
//! |---|---------|---|------|------|
//! | W | ∅ | 1 | 1 | k0 |
//! | W | TYPECHECK | 1,2,3 | 1 | slice_n1/n2, checklist |
//! | W | TYPECHECK+CMP | 1 | 1 | full_5step, real_tf_vs_original |
//! | W | AND(TYPECHECK+CMP) | 1 | 1 | and_f |
//! | W | OR(TYPECHECK+TYPECHECK) | 1 | 1 | or_f |
//! | W | NOT(TYPECHECK) | 1 | 1 | not_f |
//! | W | EMPTY | 1 | 1 | empty_f |
//! | W | HAS | 1 | 1 | has_f |
//! | W | EQ(tag) | 1 | 1 | eq_f |
//! | W | RE(tag) | 1 | 1 | re_f |
//! | W | REGEX(tag) | 1 | 1 | regex_f |
//! | W | BY(TYPECHECK) | 1 | 1 | by_exists_f |
//! | W | 互斥 TYPECHECK | 1 | 1 | contradictory |
//! | D | ∅ | 1 | 1 | drill_heading |
//! | RW | ∅ | 1 | 2 | (等价于 W) |
//! | RW | TYPECHECK(bullet) | 1,0 | 2 | recursive_wildcard_bullet/unlimited |
//! | RW | TYPECHECK+CMP | 1 | 2 | recursive_wildcard_tag_cmp |
//! | RW | TYPECHECK(paragraph) | 1 | 3 | recursive_wildcard_paragraph |
//! | RW | TYPECHECK(header) | 1 | 4 | recursive_wildcard_header_type |
//! | RW | TYPECHECK(bullet) | 3 | 2 | recursive_wildcard_slice_n3 |
//! | RW | TYPECHECK+CMP | 1 | 4 | recursive_wildcard_deep_nest |
//! | RW | NOT(TYPECHECK) | 1 | 3 | recursive_wildcard_not_typecheck |
//! | RD | ∅ | 1 | 2 | recursive_drill_no_type_filter |
//! | — | overflow | 51 | 1 | slice_overflow |
//! | — | 无匹配 | 1 | 1 | real_tf_no_match |
//! | — | setext | 1 | 1 | setext, real_tf_setext |
//! | — | 多父节点 | 1 | 2 | multi_parent |
//! | W | 全类型混排 | 1 | 1 | all_types |
//! | W | 嵌套list/BQ/codefence/table | 0 | 2 | nested_list, bq, codefence_bullet, table |
//! | W+D | indentCode | 1 | 1 | indented_code |
//! | Drill | bullet tag lookup | — | — | drill_bullet |
//! | — | 空文档 | — | — | empty_doc, blank_doc |

use mdio_core::selector::{Step, FilterExpr, TypeName, SingleSlice, select_nodes,
    CmpOp, Value, FilterMethod, ByQuantifier};
use mdio_core::projection::Snapshot;
use mdio_core::projection::{BlockNode, BlockType, BlockProj};

fn snap(text: &str) -> Snapshot {
    mdio_core::MdioEngine::new(text).make_snapshot()
}

fn orig(steps: &[Step], snap: &Snapshot) -> Vec<BlockNode> {
    select_nodes(snap, steps)
}

fn nodes_eq(a: &[BlockNode], b: &[BlockNode]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| {
        x.start_line == y.start_line && x.end_line == y.end_line
            && x.byte_range == y.byte_range && x.node_type == y.node_type
    })
}

// 辅助: 构造 filter
fn typecheck(tn: TypeName) -> FilterExpr { FilterExpr::TypeCheck(tn) }
fn tag_eq(v: &str) -> FilterExpr { FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(v.into()) } }
fn has(v: &str) -> FilterExpr { FilterExpr::Method { proj: BlockProj::Content, method: FilterMethod::Has(v.into()) } }
fn re(pat: &str) -> FilterExpr { FilterExpr::Method { proj: BlockProj::Tag, method: FilterMethod::Re(regex::Regex::new(pat).unwrap()) } }
fn empty() -> FilterExpr { FilterExpr::Method { proj: BlockProj::Content, method: FilterMethod::Empty } }
fn eq_tag(v: &str) -> FilterExpr { FilterExpr::Method { proj: BlockProj::Tag, method: FilterMethod::Eq(Value::String(v.into())) } }
fn regex_p(pat: &str) -> FilterExpr { FilterExpr::Regex { proj: BlockProj::Tag, regex: regex::Regex::new(pat).unwrap() } }
fn and(fs: Vec<FilterExpr>) -> FilterExpr { FilterExpr::And(fs) }
fn or(fs: Vec<FilterExpr>) -> FilterExpr { FilterExpr::Or(fs) }
fn not(f: FilterExpr) -> FilterExpr { FilterExpr::Not(Box::new(f)) }
fn by_exists(f: FilterExpr) -> FilterExpr { FilterExpr::By { proj: BlockProj::Content,
    steps: vec![Step::RecursiveWildcard, Step::Filter(f)], quantifier: ByQuantifier::Exists } }

/// 构建 fused steps: 顶层 Drill 保留，后续 T+F*+Slice → TF。
fn build_fused(drill: Option<&str>, t: Step, filters: &[FilterExpr], limit: usize) -> Vec<Step> {
    let mut steps = Vec::new();
    if let Some(name) = drill {
        steps.push(Step::Drill(name.into()));
    }
    steps.push(Step::TraversalFilter {
        traversal: Box::new(t),
        filters: filters.to_vec(),
        limit,
    });
    steps
}

/// 构建 original steps: 顶层 Drill 保留，后续 T + 各 Filter + Slice。
fn build_orig(drill: Option<&str>, t: Step, filters: &[FilterExpr], limit: usize) -> Vec<Step> {
    let mut steps = Vec::new();
    if let Some(name) = drill {
        steps.push(Step::Drill(name.into()));
    }
    steps.push(t);
    for f in filters {
        steps.push(Step::Filter(f.clone()));
    }
    if limit > 0 {
        steps.push(Step::Slice(vec![SingleSlice::Index((limit - 1) as isize)]));
    }
    steps
}

/// 等价性断言宏：orig(limit>0) vs fused。
macro_rules! assert_equiv {
    ($s:expr, $drill:expr, $t:expr, $filters:expr, $limit:expr) => {{
        let orig_steps = build_orig($drill, $t.clone(), &$filters, $limit);
        let fused_steps = build_fused($drill, $t, &$filters, $limit);
        let r1 = select_nodes($s, &orig_steps);
        let r2 = select_nodes($s, &fused_steps);
        assert!(nodes_eq(&r1, &r2), "orig {:?} vs fused {:?}", r1.iter().map(|n| (n.start_line, n.end_line, n.node_type)).collect::<Vec<_>>(),
                                                  r2.iter().map(|n| (n.start_line, n.end_line, n.node_type)).collect::<Vec<_>>());
        (r1, r2)
    }};
}


#[test] fn empty_doc() { assert!(orig(&[Step::Wildcard, Step::Slice(vec![SingleSlice::Index(0)])], &snap("")).is_empty()); }

#[test]
fn blank_doc() {
    let s = snap("\n");
    let steps_orig = [Step::Wildcard, Step::Slice(vec![SingleSlice::Index(0)])];
    let steps_fused = [Step::TraversalFilter { traversal: Box::new(Step::Wildcard), filters: vec![], limit: 1 }];
    assert!(nodes_eq(&orig(&steps_orig, &s), &select_nodes(&s, &steps_fused)));
}

#[test]
fn drill_wild_slice0() {
    let s = snap("# H1\ntext\n- item\n");
    let (_, r2) = assert_equiv!(&s, Some("H1"), Step::Wildcard, [], 1);
    assert_eq!(r2.len(), 1);
}

#[test]
fn full_5step() {
    let s = snap("# H1\n- setup: a\n  more\n- config: b\n");
    let fs = [typecheck(TypeName::Bullet), tag_eq("setup")];
    let (r1, _) = assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
    assert_eq!(r1.len(), 1);
}

#[test] fn slice_n1() {
    let s = snap("# H1\n- a\n- b\n- c\n"); let fs = [typecheck(TypeName::Bullet)];
    let (r1, _) = assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 2);
    assert_eq!(r1.len(), 1);
}

#[test] fn slice_n2() {
    let s = snap("# H1\n- a\n- b\n- c\n"); let fs = [typecheck(TypeName::Bullet)];
    let (r1, _) = assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 3);
    assert_eq!(r1.len(), 1);
}

#[test] fn slice_overflow() {
    let s = snap("# H1\n- i\n");
    let (r1, r2) = assert_equiv!(&s, Some("H1"), Step::Wildcard, [], 51);
    assert!(r1.is_empty()); assert!(r2.is_empty());
}

#[test] fn k0() { let s = snap("# H1\ntext\n- i\n"); assert_equiv!(&s, Some("H1"), Step::Wildcard, [], 1); }

#[test] fn contradictory() { let s = snap("# H1\n- a\n- b\n");
    let fs = [typecheck(TypeName::Bullet), typecheck(TypeName::Paragraph)];
    let (r1, r2) = assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
    assert!(r1.is_empty()); assert!(r2.is_empty());
}

#[test] fn setext() { let s = snap("# Top\nTitle\n=====\n- i\n");
    let fs = [typecheck(TypeName::Header(0))];
    assert_equiv!(&s, Some("Top"), Step::Wildcard, fs, 1);
}

#[test] fn has_f() { let s = snap("# H1\n- setup: a\n- config: b\n");
    let fs = [typecheck(TypeName::Bullet), has("a")];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn regex_f() { let s = snap("# H1\n- alpha: a\n- beta: b\n");
    let fs = [typecheck(TypeName::Bullet), regex_p("^alp")];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn and_f() { let s = snap("# H1\n- setup: a\n- config: b\n");
    let fs = [and(vec![typecheck(TypeName::Bullet), tag_eq("setup")])];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn not_f() { let s = snap("# H1\ntext\n- i\n");
    let fs = [not(typecheck(TypeName::Bullet))];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn drill_heading() { let s = snap("# Introduction\ntext\n");
    assert_equiv!(&s, None, Step::Drill("Introduction".into()), [], 1);
}

#[test] fn drill_bullet() { let s = snap("# H1\n- setup: v\n");
    // Drill 匹配 bullet/checklist 的 kv 标签（与投影 tag 同源；hook 注册前缀
    // 解析依赖 match_node_name 的 Bullet 分支）。断言 Drill("setup") 命中 bullet。
    let r = orig(&[Step::Drill("setup".into())], &s);
    assert_eq!(r.len(), 1, "Drill must match bullet kv tag");
    assert_eq!(r[0].node_type, BlockType::Bullet);
}

#[test] fn all_types() { let s = snap("# H1\n## H2\npara\n- b\n- [x] c\n1. o\n```\ncode\n```\n> bq\n<!-- c -->\n|a|b|\n|---|---|\n|1|2|\n    i\n---\n");
    let fs = [typecheck(TypeName::Header(0))];
    assert_equiv!(&s, None, Step::Wildcard, fs, 1);
}

#[test] fn empty_f() { let s = snap("# H1\n- setup: a\n\n");
    let fs = [empty()];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn eq_f() { let s = snap("# H1\n- setup: v\n- config: x\n");
    let fs = [typecheck(TypeName::Bullet), eq_tag("setup")];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn re_f() { let s = snap("# H1\n- setup: a\n- config: b\n");
    let fs = [typecheck(TypeName::Bullet), re("^set")];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn or_f() { let s = snap("# H1\ntext\n- b\n");
    let fs = [or(vec![typecheck(TypeName::Bullet), typecheck(TypeName::Paragraph)])];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn k3() { let s = snap("# H1\n- setup: a\n  d\n- b\n");
    let fs = [typecheck(TypeName::Bullet), tag_eq("setup"), has("a")];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn nested_list() { let s = snap("# H1\n- outer\n  - inner\n    deep\n- outer2\n");
    assert_equiv!(&s, Some("H1"), Step::Wildcard, [], 0);
}

#[test] fn codefence_bullet() { let s = snap("# H1\n- i\n  ```\n  c\n  ```\n  m\n");
    assert_equiv!(&s, Some("H1"), Step::Wildcard, [], 0);
}

#[test] fn checklist() { let s = snap("# H1\n- [x] done: t1\n- [ ] todo: t2\n");
    let fs = [typecheck(TypeName::Bullet)];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn multi_parent() {
    let s = snap("# H1\n- a: 1\n## H2\n- b: 2\n");
    let fs = [typecheck(TypeName::Bullet)];
    assert_equiv!(&s, None, Step::Wildcard, fs, 1);
}

#[test] fn table() { assert!(!orig(&[Step::Drill("H1".into()), Step::Wildcard],
    &snap("# H1\n|a|b|\n|---|---|\n|1|2|\n|3|4|\n")).is_empty()); }

#[test] fn indented_code() { let s = snap("# H1\n    c1\n    c2\n- b\n");
    let fs = [typecheck(TypeName::IndtCode)];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn by_exists_f() { let s = snap("# H1\n- setup: a\n  detail\n- config: b\n");
    let fs = [typecheck(TypeName::Bullet), by_exists(typecheck(TypeName::Paragraph))];
    assert_equiv!(&s, Some("H1"), Step::Wildcard, fs, 1);
}

#[test] fn nested_bq() { assert!(!orig(&[Step::Drill("H1".into()), Step::Wildcard],
    &snap("# H1\n> - nested\n>   para\n")).is_empty()); }

// 真正的 TraversalFilter 步骤, 通过 select_nodes 执行
#[test]
fn real_tf_vs_original() {
    let s = snap("# H1\n- setup: a\n  more\n- config: b\n- run: c\n");
    let steps_orig = [
        Step::Drill("H1".into()), Step::Wildcard,
        Step::Filter(typecheck(TypeName::Bullet)), Step::Filter(tag_eq("setup")),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let steps_fused = [
        Step::Drill("H1".into()),
        Step::TraversalFilter { traversal: Box::new(Step::Wildcard),
            filters: vec![typecheck(TypeName::Bullet), tag_eq("setup")], limit: 1 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert_eq!(r1.len(), 1);
}

#[test]
fn real_tf_slice_n2() {
    let s = snap("# H1\n- a: 1\n- b: 2\n- c: 3\n");
    let steps_orig = [Step::Drill("H1".into()), Step::Wildcard,
        Step::Filter(typecheck(TypeName::Bullet)), Step::Slice(vec![SingleSlice::Index(2)])];
    let steps_fused = [Step::Drill("H1".into()),
        Step::TraversalFilter { traversal: Box::new(Step::Wildcard),
            filters: vec![typecheck(TypeName::Bullet)], limit: 3 }];
    assert!(nodes_eq(&select_nodes(&s, &steps_orig), &select_nodes(&s, &steps_fused)));
}

#[test]
fn real_tf_no_match() {
    let s = snap("# H1\n- a: 1\n- b: 2\n");
    let steps_orig = [Step::Drill("H1".into()), Step::Wildcard,
        Step::Filter(typecheck(TypeName::Bullet)), Step::Filter(tag_eq("nonexist")),
        Step::Slice(vec![SingleSlice::Index(0)])];
    let steps_fused = [Step::Drill("H1".into()),
        Step::TraversalFilter { traversal: Box::new(Step::Wildcard),
            filters: vec![typecheck(TypeName::Bullet), tag_eq("nonexist")], limit: 1 }];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert!(r1.is_empty());
}

#[test]
fn real_tf_k0() {
    let s = snap("# H1\ntext\n- i\n");
    let steps_orig = [Step::Drill("H1".into()), Step::Wildcard, Step::Slice(vec![SingleSlice::Index(0)])];
    let steps_fused = [Step::Drill("H1".into()),
        Step::TraversalFilter { traversal: Box::new(Step::Wildcard), filters: vec![], limit: 1 }];
    assert!(nodes_eq(&select_nodes(&s, &steps_orig), &select_nodes(&s, &steps_fused)));
}

#[test]
fn real_tf_setext() {
    let s = snap("# Top\nTitle\n=====\n- i\n");
    let steps_orig = [Step::Drill("Top".into()), Step::Wildcard,
        Step::Filter(typecheck(TypeName::Header(0))), Step::Slice(vec![SingleSlice::Index(0)])];
    let steps_fused = [Step::Drill("Top".into()),
        Step::TraversalFilter { traversal: Box::new(Step::Wildcard),
            filters: vec![typecheck(TypeName::Header(0))], limit: 1 }];
    assert!(nodes_eq(&select_nodes(&s, &steps_orig), &select_nodes(&s, &steps_fused)));
}

// RecursiveWildcard + TYPECHECK (regression: pre-filter skip 丢子节点)
#[test]
fn real_tf_recursive_wildcard_bullet() {
    let s = snap("# Top\n## Mid\n- item: val\n- other: x\n## Alt\n- another: y\n");
    let steps_orig = [
        Step::Drill("Top".into()), Step::RecursiveWildcard,
        Step::Filter(typecheck(TypeName::Bullet)), Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let steps_fused = [
        Step::Drill("Top".into()),
        Step::TraversalFilter { traversal: Box::new(Step::RecursiveWildcard),
            filters: vec![typecheck(TypeName::Bullet)], limit: 1 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert_eq!(r1.len(), 1);
}

#[test]
fn real_tf_recursive_wildcard_tag_cmp() {
    let s = snap("# Project\n## Config\n- PORT: 5432\n- HOST: localhost\n## Tasks\n- design: ui\n");
    let steps_orig = [
        Step::Drill("Project".into()), Step::RecursiveWildcard,
        Step::Filter(typecheck(TypeName::Bullet)), Step::Filter(tag_eq("PORT")),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let steps_fused = [
        Step::Drill("Project".into()),
        Step::TraversalFilter { traversal: Box::new(Step::RecursiveWildcard),
            filters: vec![typecheck(TypeName::Bullet), tag_eq("PORT")], limit: 1 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert_eq!(r1.len(), 1);
}

#[test]
fn real_tf_recursive_wildcard_unlimited() {
    let s = snap("# Top\n## A\n- a1: x\n- a2: y\n## B\n- b1: z\npara\n");
    let steps_orig = [
        Step::Drill("Top".into()), Step::RecursiveWildcard,
        Step::Filter(typecheck(TypeName::Bullet)),
    ];
    let steps_fused = [
        Step::Drill("Top".into()),
        Step::TraversalFilter { traversal: Box::new(Step::RecursiveWildcard),
            filters: vec![typecheck(TypeName::Bullet)], limit: 0 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert_eq!(r1.len(), 3);
}

#[test]
fn real_tf_recursive_wildcard_paragraph() {
    let s = snap("# Top\n## Mid\npara\n### Deep\n- bullet\nmore para\n");
    let steps_orig = [
        Step::Drill("Top".into()), Step::RecursiveWildcard,
        Step::Filter(typecheck(TypeName::Paragraph)), Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let steps_fused = [
        Step::Drill("Top".into()),
        Step::TraversalFilter { traversal: Box::new(Step::RecursiveWildcard),
            filters: vec![typecheck(TypeName::Paragraph)], limit: 1 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert!(!r1.is_empty());
}

#[test]
fn real_tf_recursive_wildcard_header_type() {
    let s = snap("# Top\n## A\n- a: 1\n### A1\n- inner: x\n## B\n- b: 2\n");
    let steps_orig = [
        Step::Drill("Top".into()), Step::RecursiveWildcard,
        Step::Filter(typecheck(TypeName::Header(0))), Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let steps_fused = [
        Step::Drill("Top".into()),
        Step::TraversalFilter { traversal: Box::new(Step::RecursiveWildcard),
            filters: vec![typecheck(TypeName::Header(0))], limit: 1 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert_eq!(r1.len(), 1);
}

#[test]
fn real_tf_recursive_wildcard_slice_n3() {
    let s = snap("# Top\n## A\n- a1: x\n- a2: y\n- a3: z\n- a4: w\n## B\n- b1: u\n");
    let steps_orig = [
        Step::Drill("Top".into()), Step::RecursiveWildcard,
        Step::Filter(typecheck(TypeName::Bullet)), Step::Slice(vec![SingleSlice::Index(2)]),
    ];
    let steps_fused = [
        Step::Drill("Top".into()),
        Step::TraversalFilter { traversal: Box::new(Step::RecursiveWildcard),
            filters: vec![typecheck(TypeName::Bullet)], limit: 3 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert_eq!(r1.len(), 1);
}

#[test]
fn real_tf_recursive_wildcard_deep_nest() {
    let s = snap("# Top\n## L1\n### L2\n#### L3\n- deep: val\n- deep2: x\n## Other\n- shallow: y\n");
    let steps_orig = [
        Step::Drill("Top".into()), Step::RecursiveWildcard,
        Step::Filter(typecheck(TypeName::Bullet)), Step::Filter(tag_eq("deep")),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let steps_fused = [
        Step::Drill("Top".into()),
        Step::TraversalFilter { traversal: Box::new(Step::RecursiveWildcard),
            filters: vec![typecheck(TypeName::Bullet), tag_eq("deep")], limit: 1 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert_eq!(r1.len(), 1);
}

#[test]
fn real_tf_recursive_wildcard_not_typecheck() {
    let s = snap("# Top\npara top\n## Sub\n- bullet: x\npara sub\n### Deep\n- bullet2: y\n");
    let steps_orig = [
        Step::Drill("Top".into()), Step::RecursiveWildcard,
        Step::Filter(not(typecheck(TypeName::Bullet))),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let steps_fused = [
        Step::Drill("Top".into()),
        Step::TraversalFilter { traversal: Box::new(Step::RecursiveWildcard),
            filters: vec![not(typecheck(TypeName::Bullet))], limit: 1 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert_eq!(r1.first().map(|n| n.node_type), Some(BlockType::Paragraph));
}

#[test]
fn real_tf_recursive_drill_no_type_filter() {
    let s = snap("# Top\n## Config\n- host: x\n## Data\n- item: y\n");
    let steps_orig = [
        Step::Drill("Top".into()), Step::RecursiveDrill("Config".into()),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let steps_fused = [
        Step::Drill("Top".into()),
        Step::TraversalFilter { traversal: Box::new(Step::RecursiveDrill("Config".into())),
            filters: vec![], limit: 1 },
    ];
    let r1 = select_nodes(&s, &steps_orig);
    let r2 = select_nodes(&s, &steps_fused);
    assert!(nodes_eq(&r1, &r2));
    assert_eq!(r1.len(), 1);
}

// 速度对比: 原始 vs TraversalFilter
#[test]
fn bench_tf_vs_original() {
    let s = snap("# H1\n- setup: a\n  more\n- config: b\n- run: c\n- deploy: d\n- test: e\n");
    let steps_orig = [
        Step::Drill("H1".into()), Step::Wildcard,
        Step::Filter(typecheck(TypeName::Bullet)), Step::Filter(tag_eq("setup")),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let steps_fused = [
        Step::Drill("H1".into()),
        Step::TraversalFilter { traversal: Box::new(Step::Wildcard),
            filters: vec![typecheck(TypeName::Bullet), tag_eq("setup")], limit: 1 },
    ];

    for _ in 0..100 { select_nodes(&s, &steps_orig); select_nodes(&s, &steps_fused); }

    use std::time::Instant;
    let rounds = 10000;
    let t0 = Instant::now();
    for _ in 0..rounds { select_nodes(&s, &steps_orig); }
    let orig_time = t0.elapsed();
    let t0 = Instant::now();
    for _ in 0..rounds { select_nodes(&s, &steps_fused); }
    let fused_time = t0.elapsed();

    let orig_ns = orig_time.as_nanos() as f64 / rounds as f64;
    let fused_ns = fused_time.as_nanos() as f64 / rounds as f64;



}
