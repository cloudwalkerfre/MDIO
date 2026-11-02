//! 回归测试:merged/mid 局部段被跨区间查询复用导致漏块。
//!
//! 场景(链深 1——生产常态:每次编辑后 path worker 首次消费建 Base,
//! 下次编辑链上 Edit{base: Base}):
//!   1. 编辑1(尾部插 "- x\n")→ 快照1 上查询一次 → ensure_node 建 Base
//!   2. 编辑2(para c 后插 "- y\n")→ 链深 1,mid ≈ [6,15)
//!   3. A:find_direct_children(H2) —— 跨 mid → merged 局部段构建
//!   4. B:find_direct_children(H3) —— 跨 mid 且超出 A 段
//!
//! 共享 structure 缓存只接纳全量段([0,n) 构建);mid/merged 局部段
//! 只供本次查询(同区间复用由 intervals 五元组缓存承担)。
//! B==REF=[(13,14),(15,16),(16,17)]。

use std::sync::Arc;

use dashmap::DashSet;
use mdio_core::block::BlockType;
use mdio_core::engine::MdioSnapshot;
use mdio_core::selector::find_direct_children;
use mdio_core::session::MdioSession;

/// 原始行:0 #H1,1 b,2 para a,3 b,4 #H2,5 b,6 para b,7 b,8 para c,9 b,
///         10 #H3,11 b,12 para d,13 b,14 tail
/// 编辑1:tail 后插 "- x\n"(尾部)
/// 编辑2:para c 后插 "- y\n"(H2/H3 之间——mid 覆盖 [6,15))
/// 快照2 行:0 #H1,1 b,2 para a,3 b,4 #H2,5 b,6 para b,7 b,8 para c,
///         9 - y,10 b,11 #H3,12 b,13 para d,14 b,15 tail,16 - x (n=17)
const DOC: &str =
    "# H1\n\npara a\n\n# H2\n\npara b\n\npara c\n\n# H3\n\npara d\n\ntail\n";

fn session(doc: &str) -> MdioSession {
    MdioSession::new(0, doc, 42, "".into(), None, None, None, Arc::new(DashSet::new()), Arc::new(mdio_core::session::DetectClock::new()))
}

fn insert_after(s: &MdioSession, find: &str, ins: &str) {
    // find 必须含行尾 \n,插入点 = 该行之后(否则插在行内,变成段落文本)
    let pos = s.get_engine_text().find(find).expect("pattern in doc") + find.len();
    let _ = s.execute_transaction(None, false, |ctx| {
        ctx.apply_mdio_op(vec![pos..pos], "+>", ins);
    });
}

fn dump(label: &str, v: &[mdio_core::block::BlockNode]) {
    let spans: Vec<(usize, usize)> = v.iter().map(|c| (c.start_line, c.end_line)).collect();
    eprintln!("[{label}] children={} spans={spans:?}", v.len());
}

#[test]
fn merged_zone_partial_segment_cross_range_reuse_probe() {
    // ── 主实验:编辑1 → 快照1 查询建 Base → 编辑2(链深 1)──
    let s = session(DOC);
    insert_after(&s, "tail\n", "- x\n"); // 编辑1(尾部)
    let snap1: MdioSnapshot = s.publisher.load().as_ref().clone();
    // 快照 1 上查询一次:ensure_node 建 Base(生产:path worker 首次消费等效)
    let root1 = snap1.build_node(0, snap1.line_len(), BlockType::Blank, false);
    let _ = find_direct_children(&snap1, &root1, None);
    let ins2_pos = s.get_engine_text().find("para c\n").expect("para c") + "para c\n".len();
    insert_after(&s, "para c\n", "- y\n"); // 编辑2:链深 1
    let snap2: MdioSnapshot = s.publisher.load().as_ref().clone();
    let n = snap2.line_len();
    eprintln!("[ENV] n={n}");

    // 打印编辑 2 的权威 zone(compute_zone——与缓存物化同语义)
    let task2 = mdio_core::incr::IncrTask {
        snap: Arc::new(snap2.clone()),
        doc_id: 0,
        snap_old: Arc::new(snap1.clone()),
        delta: 1,
        edit_ranges: mdio_core::edit_actions_arc(vec![(ins2_pos, ins2_pos)]),
        ripple_line_start: 9,
        ripple_line_end: 9,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let lm2 = mdio_core::incr::LineMap::new(&task2);
    let z2 = mdio_core::incr::compute_zone(&task2, &lm2);
    eprintln!("[ZONE2] zs_o={} ce_o={} zs_n={} ce_n={}", z2.zs_o, z2.ce_o, z2.zs_n, z2.ce_n);
    for (i, l) in snap2.rope.to_string().lines().enumerate() {
        eprintln!("[DOC] {i}: {l:?}");
    }

    // A:先查 H2 子(跨 mid → merged 局部段写共享缓存)
    let h2 = snap2.build_node(4, 11, BlockType::Header2, false);
    let a = find_direct_children(&snap2, &h2, None);
    dump("A(H2 子,跨 mid 首查)", &a);

    // B:再查 H3 子(跨 mid,超出 A 段 → 复用共享缓存局部段 → 漏块?)
    let h3 = snap2.build_node(11, n, BlockType::Header3, false);
    let b = find_direct_children(&snap2, &h3, None);
    dump("B(H3 子,复用共享缓存)", &b);

    // ── 对照:先查 root(全量段)再查 H2/H3 ──
    let s2 = session(DOC);
    insert_after(&s2, "tail\n", "- x\n");
    let snap1b: MdioSnapshot = s2.publisher.load().as_ref().clone();
    let root1b = snap1b.build_node(0, snap1b.line_len(), BlockType::Blank, false);
    let _ = find_direct_children(&snap1b, &root1b, None);
    insert_after(&s2, "para c\n", "- y\n");
    let snap2b: MdioSnapshot = s2.publisher.load().as_ref().clone();
    let root2 = snap2b.build_node(0, snap2b.line_len(), BlockType::Blank, false);
    let _ = find_direct_children(&snap2b, &root2, None); // 全量段写共享缓存
    let h2b = snap2b.build_node(4, 11, BlockType::Header2, false);
    let a_ref = find_direct_children(&snap2b, &h2b, None);
    dump("REF(H2 子,全量段)", &a_ref);
    let h3b = snap2b.build_node(11, n, BlockType::Header3, false);
    let b_ref = find_direct_children(&snap2b, &h3b, None);
    dump("REF2(H3 子,全量段)", &b_ref);

    let b_spans: Vec<(usize, usize)> = b.iter().map(|c| (c.start_line, c.end_line)).collect();
    let ref_spans: Vec<(usize, usize)> = b_ref.iter().map(|c| (c.start_line, c.end_line)).collect();
    assert_eq!(
        b_spans, ref_spans,
        "漏块!B(H3 子)={b_spans:?} REF={ref_spans:?} —— merged 局部段被跨区间复用"
    );
    eprintln!("[PASS] B == REF: {b_spans:?}");
}
