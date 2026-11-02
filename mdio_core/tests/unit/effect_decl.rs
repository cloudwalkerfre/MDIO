// 广义效应表验证 — body 双分区谓词
//

use super::*;

/// fc1_equiv (ripple) 与"栈全等"的关系: fc1 只比 adm 位, 忽略 out 位。
/// 逐位验证: fc1(a,b) = true ⟺ 差异位不在 adm_mask 中。
#[test]
fn fc1_ignores_out_bits() {
    use crate::effect::{fc1_equiv, CONTAINER_DECLS};
    for kind in [FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::OrderedList,
                 FrameKind::CodeFence, FrameKind::HtmlBlock, FrameKind::Table,
                 FrameKind::Comment, FrameKind::HtmlElement, FrameKind::RefDef] {
        let decl = &CONTAINER_DECLS[kind as usize];
        let base = BlockFrame::new(kind, 3, 0);
        for bit in 0..6u16 {
            let other = BlockFrame::new(kind, 3, (1u8 << bit) as u8);
            let in_adm = (1u16 << bit) & decl.body.adm_mask != 0;
            assert_eq!(fc1_equiv(base, other), !in_adm,
                "{:?} bit{}: fc1 应 = {}", kind, bit, !in_adm);
            // 全等永远 false (body 不同) — fc1 更弱, 在 out 位上仍能判等
            assert_ne!(base.0, other.0);
        }
        // indent 不同 → fc1 false
        assert!(!fc1_equiv(base, BlockFrame::new(kind, 4, 0)));
    }
}

/// same_container (incr) 与 diff 分区 (body_adm_eq) 一致。
#[test]
fn same_container_matches_diff_partition() {
    use crate::effect::same_container;
    // BQ: body 恒 0——kind 相同 → 同容器
    let bq0 = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    let bq1 = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    assert!(same_container(bq0, bq1));
    assert!(same_container(bq0, bq0));
    // BLT: bullet_code 不同 → 不同; LOOSE 不同 → 同 (diff adm 不含 LOOSE)
    let blt_dash = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    let blt_star = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'*'));
    let blt_loose = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-') | 0x08);
    assert!(!same_container(blt_dash, blt_star));
    assert!(same_container(blt_dash, blt_loose));
    // OL: delim 不同 → 不同
    let ol_dot = BlockFrame::new(FrameKind::OrderedList, 0, BlockFrame::encode_ol_body(b'.'));
    let ol_paren = BlockFrame::new(FrameKind::OrderedList, 0, BlockFrame::encode_ol_body(b')'));
    assert!(!same_container(ol_dot, ol_paren));
    // CF: fence_char/len 不同 → 同 (diff adm=∅)
    let cf3 = BlockFrame::new(FrameKind::CodeFence, 0, BlockFrame::encode_cf_body(b'`', 3));
    let cf5 = BlockFrame::new(FrameKind::CodeFence, 0, BlockFrame::encode_cf_body(b'~', 5));
    assert!(same_container(cf3, cf5));
    // HtmlBlock: block_type 不同 → 不同
    let hb1 = BlockFrame::new(FrameKind::HtmlBlock, 0, BlockFrame::encode_html_body(1));
    let hb2 = BlockFrame::new(FrameKind::HtmlBlock, 0, BlockFrame::encode_html_body(2));
    assert!(!same_container(hb1, hb2));
    // RefDef: has_content 不同 → 不同
    let rd0 = BlockFrame::new(FrameKind::RefDef, 0, 0);
    let rd1 = BlockFrame::new(FrameKind::RefDef, 0, 1);
    assert!(!same_container(rd0, rd1));
    // kind 不同 → false
    assert!(!same_container(bq0, blt_dash));
}

/// 规范矩阵（包 A——Spec 域显式化——Π₁ 完备性断言——新增类型/改语义时红）。
/// 每列的非默认类型集合显式声明——表条目与矩阵互证（`t ∈ Spec(a) ⟺ δ_T(t,a) ≠ default`）。
#[test]
fn block_type_decl_matrix_complete() {
    use crate::effect::{block_decl, InterruptRule};
    use BlockType::*;
    const CLOSES_PARAGRAPH: &[BlockType] = &[
        IndentedCode, CodeContent, CodeFenceClose, Comment,
        HtmlContent, HtmlBlock, HtmlElement, RefDef, TableSeparator, TableRow,
        // 扩集（SDL prev_closes 组工具驱动——CM §5.1 非段落全集——
        // 实测确认 `> # h\nlazy` 等 lazy 误吸——列缺口修复）：
        Header1, Header2, Header3, Header4, Header5, Header6,
        ThematicBreak, HsHead1, HsHead2,
        // 符合性裁决 #238（`> ```\nfoo` → foo 顶层——cmark 佐证）：
        // CF 开行后无 lazy（弹栈后 prev=CodeFenceOpen——BQ lazy 判定——
        // 逐层豁免暴露原"合理缺"为真 bug——补列）
        CodeFenceOpen,
    ];
    // A1（打断单源化——CM §4.1 规则全集）：
    const INTERRUPT_YES: &[BlockType] = &[ThematicBreak, CodeFenceOpen, Blockquote, TableRow,
        Header1, Header2, Header3, Header4, Header5, Header6];
    const INTERRUPT_LIST: &[BlockType] = &[Bullet, Checklist, OrderedListItem];
    const INTERRUPT_HTML: &[BlockType] = &[HtmlBlock];
    // 显式 27 类型数组（数组长度编译期检查——少一个编译错——BlockType 无 repr 转换）
    const ALL: [BlockType; 27] = [
        Header1, Header2, Header3, Header4, Header5, Header6,
        Bullet, CodeFenceOpen, Blockquote, OrderedListItem, Paragraph, Comment,
        TableRow, Checklist, HsHead1, HsHead2, HsSep, HtmlBlock, ThematicBreak,
        CodeFenceClose, CodeContent, HtmlContent, IndentedCode, TableSeparator,
        Blank, RefDef, HtmlElement,
    ];
    for bt in ALL {
        let d = block_decl(bt);
        assert_eq!(d.closes_paragraph, CLOSES_PARAGRAPH.contains(&bt),
            "closes_paragraph 矩阵不一致: {bt:?}");
        let rule_ok = match d.interrupt_rule {
            InterruptRule::Yes => INTERRUPT_YES.contains(&bt),
            InterruptRule::List => INTERRUPT_LIST.contains(&bt),
            InterruptRule::Html => INTERRUPT_HTML.contains(&bt),
            InterruptRule::No => {
                !INTERRUPT_YES.contains(&bt)
                    && !INTERRUPT_LIST.contains(&bt)
                    && !INTERRUPT_HTML.contains(&bt)
            }
        };
        assert!(rule_ok, "interrupt_rule 矩阵不一致: {bt:?}");
    }
}

/// closing_pl 不变式：Specific ⟺ kind ∈ {CF, Comment, HtmlBlock}（原
/// end_includes_close=true 语义——单列化后由枚举保证——消费点无解释权）。
#[test]
fn container_effect_closing_pl_matrix() {
    use crate::effect::{ClosingPl, effect_for};
    for kind in [FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::OrderedList,
                 FrameKind::CodeFence, FrameKind::HtmlBlock, FrameKind::Table,
                 FrameKind::Comment, FrameKind::HtmlElement, FrameKind::RefDef,
                 FrameKind::TableRow] {
        let specific = matches!(effect_for(kind).closing_pl, ClosingPl::Specific(_));
        let expect = matches!(kind,
            FrameKind::CodeFence | FrameKind::Comment | FrameKind::HtmlBlock);
        assert_eq!(specific, expect, "closing_pl 不变式不一致: {kind:?}");
    }
}
