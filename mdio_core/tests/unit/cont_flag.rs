use crate::engine::MdioEngine;
use crate::block::{BlockType, PackedLineInfo};

#[test]
fn cont_scenario_1_first_line() {
    let engine = MdioEngine::new("paragraph\n");
    let lines = &engine.state.lines;
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].node_type(), BlockType::Paragraph);
    assert!(!lines[0].cont(), "first line cont must be false (prev_line_type=None)");
}

#[test]
fn cont_scenario_2_consecutive_paragraph() {
    let engine = MdioEngine::new("a\nb\nc\n");
    let lines = &engine.state.lines;
    assert_eq!(lines.len(), 3);
    assert!(!lines[0].cont(), "line 0: first Paragraph, cont=false");
    assert!(lines[1].cont(), "line 1: prev=Paragraph, not block_start, cont=true");
    assert!(lines[2].cont(), "line 2: prev=Paragraph, not block_start, cont=true");
}

#[test]
fn cont_scenario_3_blank_breaks_paragraph() {
    let engine = MdioEngine::new("a\n\nb\n");
    let lines = &engine.state.lines;
    assert_eq!(lines.len(), 3);
    assert!(!lines[0].cont(), "line 0: first line");
    assert_eq!(lines[1].node_type(), BlockType::Blank);
    assert!(!lines[1].cont(), "line 1: Blank, cont must be false (ft != Paragraph)");
    assert!(!lines[2].cont(), "line 2: prev_line_type=None after Blank, cont=false");
}

#[test]
fn cont_scenario_4_push_breaks_paragraph() {
    let engine = MdioEngine::new("- a\n  continued\n- b\n");
    let lines = &engine.state.lines;
    assert_eq!(lines.len(), 3);
    assert!(!lines[0].cont(), "line 0: Bullet, not Paragraph");
    // 符合性裁决 #255：item 打开行 prev_hint=Paragraph——
    // continued 是 item 内容段落延续——官方 1 p（<li>a\ncontinued</li>）——
    // 旧断言（prev=Bullet 不合并——2 p）是 hack——已修正
    assert!(lines[1].cont(), "line 1: Paragraph after item-open prev_hint, cont=true");
    assert!(!lines[2].cont(), "line 2: Bullet, not Paragraph");
}

#[test]
fn cont_scenario_5_ic_consecutive() {
    let engine = MdioEngine::new("    code1\n    code2\n");
    let lines = &engine.state.lines;
    assert_eq!(lines.len(), 2);
    assert!(!lines[0].cont(), "line 0: first IC, cont=false");
    assert!(lines[1].cont(), "line 1: prev=IC same type, cont=true");
}

#[test]
fn cont_scenario_6_ic_blank() {
    let engine = MdioEngine::new("    code1\n\n    code2\n");
    let lines = &engine.state.lines;
    assert_eq!(lines.len(), 3);
    assert!(!lines[0].cont(), "line 0: first IC");
    assert_eq!(lines[1].node_type(), BlockType::Blank);
    assert!(lines[2].cont(), "line 2: IC after Blank, lookback finds line 0 IC, cont=true");
}

#[test]
fn cont_scenario_7_bq_not_paragraph() {
    // u8 词法语义：> 开头 = Blockquote，cont 由 u8 序列驱动（内容层在 resolved 列）
    let engine = MdioEngine::new("> a\n> b\n> c\n");
    let lines = &engine.state.lines;
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].node_type(), BlockType::Blockquote);
    assert!(!lines[0].cont());
    assert_eq!(lines[1].node_type(), BlockType::Paragraph);
    // BQ 开行后内容行：prev_hint = Content(Paragraph)（修点 5——H2 cont 修复）
    assert!(lines[1].cont(), "line1: BQ 开行后段落续行, cont=true");
    assert_eq!(lines[2].node_type(), BlockType::Paragraph);
    assert!(lines[2].cont(), "line2: prev=Paragraph == Paragraph, cont=true");
}

#[test]
fn cont_scenario_8_pop_loop_final_type() {
    let engine = MdioEngine::new("- item\n  text\n");
    let lines = &engine.state.lines;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].node_type(), BlockType::Bullet);
    assert!(!lines[0].cont(), "Bullet, not Paragraph");
    // 符合性裁决 #255：item 内容段落延续——官方 1 p（<li>item\ntext</li>）——
    // 旧断言（prev=Bullet 不合并）是 hack——已修正
    assert_eq!(lines[1].node_type(), BlockType::Paragraph);
    assert!(lines[1].cont(), "Paragraph after item-open prev_hint, cont=true");
}
