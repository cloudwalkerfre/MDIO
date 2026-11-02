// prev 承诺列探针。
//   1b 结案:承诺列 == 重建态逐行(增量 ripple 的承诺与全量重建一致);
//   conv-7 定向:改写路径(list_end_guard / 32 层截断)下承诺异必不 conv;
//   语义哨兵:BQ 裸行/真空行/单层 BQ Push 的承诺值(原 P1/P2 语义保留);
//   P4 HS 诊断保留(与回退无关)。
// 背景:prev 承诺不是事件可导的(投影改写与 32 层截断两条零事件路径);
// 承诺列物化后 prev_semantic_boundary/line_effect_shape 不再需要。

use super::*;
use crate::block::BlockType;
use crate::projection::State;

fn next_rand(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

/// 承诺列等价断言:ripple 后的 prev_col == 最终文本全量重建的 prev_col。
fn assert_promise_equiv(doc: &str, start: usize, end: usize, text: &str) {
    let mut st = State::new(doc);
    st.edit(start, end, text);
    let snap = st.snapshot();
    let final_text = st.rope.to_string();
    drop(st);
    let mut fresh = State::new(&final_text);
    let fresh_snap = fresh.snapshot();
    assert_eq!(
        snap.prev_col.as_ref(),
        fresh_snap.prev_col.as_ref(),
        "承诺列不等价: doc={doc:?} edit=({start},{end})→{text:?}"
    );
    assert_eq!(
        snap.lines.as_ref(),
        fresh_snap.lines.as_ref(),
        "PL 也应等价(前提校验)"
    );
}

/// 1b 结案:随机语料 × 随机编辑链(含结构/内容/删行)——ripple 承诺列
/// 与全量重建逐行一致。
#[test]
fn promise_column_equals_rebuild() {
    let tokens: &[&str] = &[
        "plain paragraph\n",
        "some **bold** text\n",
        "# Heading\n",
        "- item\n",
        "- [ ] task\n",
        "1. first\n",
        "  - nested\n",
        "> quote\n",
        "> > nested\n",
        "```rust\nlet x = 1;\n```\n",
        "\n",
        "    indented code\n",
        "[id]: https://example.com \"t\"\n",
        "| a | b |\n|---|---|\n| c | d |\n",
        "> foo\n>\nlazy\n",
        "1. a\n10000. - > q\n",
    ];
    let mut rng: u64 = 0x1B5E_E37D_4A9C_0001;
    let mut cases = 0usize;
    for _case in 0..400u64 {
        let mut doc = String::new();
        let line_count = 3 + (next_rand(&mut rng) % 12) as usize;
        for _ in 0..line_count {
            doc.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
        }
        let len = doc.len();
        if len == 0 {
            continue;
        }
        let roll = next_rand(&mut rng) % 3;
        let (start, end, text) = if roll == 0 {
            let pos = next_rand(&mut rng) as usize % (len + 1);
            let tok = tokens[next_rand(&mut rng) as usize % tokens.len()];
            (pos, pos, tok.to_string())
        } else if roll == 1 {
            let start = next_rand(&mut rng) as usize % len;
            let end = start + 1 + next_rand(&mut rng) as usize % (len - start);
            (start, end, String::new())
        } else {
            let start = next_rand(&mut rng) as usize % len;
            let end = start + 1 + next_rand(&mut rng) as usize % (len - start);
            let tok = tokens[next_rand(&mut rng) as usize % tokens.len()];
            (start, end, tok.to_string())
        };
        assert_promise_equiv(&doc, start, end, &text);
        cases += 1;
    }
    eprintln!("[PROMISE-1B] {cases} 例:ripple 承诺列 == 重建态逐行(全等)");
}

/// conv-7 定向①:list_end_guard 改写(`10000. - > q` 段——packed 改写
/// Paragraph、承诺异——零事件路径)。编辑 marker 处触发改写,承诺列
/// 必须与重建态一致(conv 不得在承诺异时提前收敛)。
#[test]
fn conv7_list_end_guard_rewrite() {
    // 三种变体:尾部编辑(触发段)、首 item 编辑、中间内容编辑。
    for (doc, find, rep) in [
        ("1. a\n10000. - > q\n\ntail\n", "10000. - > q", "10000. - > qq\n"),
        ("1. a\n10000. - > q\n\ntail\n", "1. a", "1. b"),
        ("1. a\n10000. - > q\n\ntail\n", "tail", "TAIL"),
    ] {
        let start = doc.find(find).unwrap();
        assert_promise_equiv(doc, start, start + find.len(), rep);
    }
    eprintln!("[CONV7-LG] list_end_guard 三变体承诺列等价 ✓");
}

/// conv-7 定向②:32 层截断(MAX_BLOCK_NESTING)——超过嵌套上限的行被
/// 截断(零事件路径)。编辑深层附近,承诺列必须与重建态一致。
#[test]
fn conv7_max_nesting_truncation() {
    let mut doc = String::new();
    for i in 0..40 {
        doc.push_str(&"> ".repeat(i + 1));
        doc.push_str(&format!("lvl {i}\n"));
    }
    doc.push_str("\ntail\n");
    // 深层第 35 层(已超 32 截断)内容编辑 + 浅层编辑。
    let pos = doc.find("lvl 35").unwrap();
    assert_promise_equiv(&doc, pos, pos + 6, "lvl 35x");
    let pos2 = doc.find("lvl 2").unwrap();
    assert_promise_equiv(&doc, pos2, pos2 + 5, "lvl 2x");
    eprintln!("[CONV7-NEST] 40 层 BQ 嵌套(32 截断)承诺列等价 ✓");
}

/// 语义哨兵:真空行/BQ 裸行/单层 BQ Push 的承诺值。
#[test]
fn promise_semantics_sentinels() {
    let doc = "> foo\n>\nlazy\n\n- a\n";
    let mut st = State::new(&doc);
    let snap = st.snapshot();
    let col = &snap.prev_col;
    // L0 "> foo"(单层 BQ Push 段落):承诺 = Paragraph。
    assert_eq!(
        col[0].node_type(),
        BlockType::Paragraph,
        "BQ 开行的承诺应为 Paragraph"
    );
    // L1 ">"(BQ 裸行):承诺 = 无(invalid——PrevHint::Blank 语义)。
    assert!(!col[1].is_valid(), "BQ 裸行的承诺应为无(Blank)");
    // L3 真空行:承诺 = 无。
    assert!(!col[3].is_valid(), "真空行的承诺应为无");
    eprintln!("[PROMISE-SENT] 承诺语义哨兵:开行=Paragraph/裸行=无/真空=无 ✓");
}

/// P4(遗留诊断——非断言): HS diff 的 zone/patch 打印——空行修复暴露的
/// "zone 切章节"潜在缺口的取证器械(与回退无关,保留)。
/// compute_zone 不分派 SectionClosure（zone 单套 CF）；
/// 补丁出口统一走适配层 hs_patches。
#[test]
fn p4_diag_hs_zone_paths() {
    use crate::incr::{compute_zone, hs_patches, IncrTask, LineMap};
    use std::sync::Arc;
    for (name, old, find, rep) in [
        ("roundtrip", "# A\n\ntext\n\n## B\n\n- a\n", "text", "TEXT"),
        ("gate", "# A\n\nBody\n# B\n", "o", "X"),
    ] {
        let mut state = State::new(old);
        let snap_old = Arc::new(state.snapshot());
        let sb = old.find(find).unwrap();
        state.edit(sb, sb + find.len(), rep);
        let snap = Arc::new(state.snapshot());
        let diff = state.ripple_diff.take().unwrap();
        eprintln!(
            "[{name}] rls={} rle={} delta={}",
            diff.ripple_line_start, diff.ripple_line_end, diff.delta
        );
        let task = IncrTask {
            snap: snap.clone(),
            doc_id: 0,
            snap_old: snap_old.clone(),
            delta: diff.delta,
            edit_ranges: crate::cache::edit_actions_arc(vec![(sb, sb + find.len())]),
            ripple_line_start: diff.ripple_line_start,
            ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        let lm = LineMap::new(&task);
        let z = compute_zone(&task, &lm);
        eprintln!(
            "[{name}] zone(CF 单套) cs_o={} ce_o={} zs_o={} | cs_n={} ce_n={} zs_n={}",
            z.cs_o, z.ce_o, z.zs_o, z.cs_n, z.ce_n, z.zs_n
        );
        let patches = hs_patches(&task);
        eprintln!("[{name}] patches(适配层)={patches:?}");
    }
}
