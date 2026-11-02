use super::*;

// ── Block 效应表 ──

#[test]
fn container_effects_table_has_10_entries() {
    // FrameKind has 10 variants (0..=9), array must have exactly 10 rows
    assert_eq!(CONTAINER_EFFECTS.len(), 10);
}

#[test]
fn effect_for_all_frame_kinds() {
    let kinds = [
        FrameKind::BlockQuote,
        FrameKind::BulletList,
        FrameKind::OrderedList,
        FrameKind::CodeFence,
        FrameKind::HtmlBlock,
        FrameKind::Table,
        FrameKind::Comment,
        FrameKind::HtmlElement,
        FrameKind::RefDef,
        FrameKind::TableRow,
    ];
    for k in kinds {
        let eff = effect_for(k);
        assert!(!eff.container_type_str.is_empty(),
            "container_type_str empty for {:?}", k);
    }
}

#[test]
fn shell_kinds() {
    // 只有 BQ, BLT, OL 是 shell
    assert!(effect_for(FrameKind::BlockQuote).is_shell);
    assert!(effect_for(FrameKind::BulletList).is_shell);
    assert!(effect_for(FrameKind::OrderedList).is_shell);
    assert!(!effect_for(FrameKind::CodeFence).is_shell);
    assert!(!effect_for(FrameKind::HtmlBlock).is_shell);
    assert!(!effect_for(FrameKind::Table).is_shell);
    assert!(!effect_for(FrameKind::Comment).is_shell);
    assert!(!effect_for(FrameKind::HtmlElement).is_shell);
    assert!(!effect_for(FrameKind::RefDef).is_shell);
}

#[test]
fn transparent_none_by_default() {
    // All FrameKinds are currently non-transparent (matches old ast_is_transparent())
    for k in [FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::OrderedList,
              FrameKind::CodeFence, FrameKind::HtmlBlock, FrameKind::Table,
              FrameKind::Comment, FrameKind::HtmlElement, FrameKind::RefDef].iter() {
        assert!(!effect_for(*k).is_transparent, "{:?} should not be transparent", k);
    }
}

#[test]
fn item_leaf_only_blt_ol() {
    // auto_inject 含 ItemLeaf 的只有 BLT/OL
    assert!(effect_for(FrameKind::BulletList).auto_inject.iter()
        .any(|t| t.ty == NodeTemplateType::ItemLeaf));
    assert!(effect_for(FrameKind::OrderedList).auto_inject.iter()
        .any(|t| t.ty == NodeTemplateType::ItemLeaf));
    for k in [FrameKind::BlockQuote, FrameKind::CodeFence, FrameKind::HtmlBlock,
              FrameKind::Table, FrameKind::Comment, FrameKind::HtmlElement,
              FrameKind::RefDef].iter() {
        assert!(!effect_for(*k).auto_inject.iter()
            .any(|t| t.ty == NodeTemplateType::ItemLeaf),
            "{:?} should not have ItemLeaf", k);
    }
}

#[test]
fn bq_no_paragraph_inject() {
    // BQ 段落由 leaf 渲染（内容层 resolved=Paragraph）——auto_inject 不存在
    assert_eq!(effect_for(FrameKind::BlockQuote).auto_inject.len(), 0);
    assert_eq!(effect_for(FrameKind::BulletList).auto_inject.len(), 1);
    assert_eq!(effect_for(FrameKind::OrderedList).auto_inject.len(), 1);
}

#[test]
fn loose_wrapper_only_blt_ol() {
    assert_eq!(effect_for(FrameKind::BulletList).loose_wrapper, Some("p"));
    assert_eq!(effect_for(FrameKind::OrderedList).loose_wrapper, Some("p"));
    assert_eq!(effect_for(FrameKind::BlockQuote).loose_wrapper, None);
    assert_eq!(effect_for(FrameKind::CodeFence).loose_wrapper, None);
}

#[test]
fn item_leaf_type_str() {
    assert_eq!(effect_for(FrameKind::BulletList).item_leaf_type_str, "b");
    assert_eq!(effect_for(FrameKind::OrderedList).item_leaf_type_str, "ol");
}

#[test]
fn depth_rules_semantics() {
    // fn 指针 → 枚举——规则语义断言
    use crate::effect::DepthOnlyRule as DR;
    assert_eq!(effect_for(FrameKind::BlockQuote).depth_only_pred, DR::OnBlockStart);
    assert_eq!(effect_for(FrameKind::Comment).depth_only_pred,
        DR::WhenFrameInStack(FrameKind::Comment));
    assert_eq!(effect_for(FrameKind::Table).depth_only_pred, DR::WhenTableConfirmed);
    assert_eq!(effect_for(FrameKind::BulletList).depth_only_pred, DR::Always);
    // eval 语义（Comment 帧在栈 → depth_only——同行配对无帧 → 非）
    let mut st = crate::projection::State::new("<!--\ncomment\n-->\n");
    let snap = st.snapshot();
    let mut fold = crate::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
    crate::infer::walk_zone(&snap.frame_log.entries, (0, snap.line_len() as u32), &mut fold);
    let (recs, arena) = fold.finish();
    let blocks = crate::infer::build_block_records(&snap.frame_log.entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, snap.line_len() as u32));
    let spans = crate::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries, &snap.lines, &snap.rope, (0, snap.line_len() as u32), &arena);
    assert!(DR::WhenFrameInStack(FrameKind::Comment).eval(&spans, 1, 1, FrameKind::Comment),
        "Comment 帧在栈应 depth_only");
    // BQ 开行 → OnBlockStart true（is_block_start）
    let mut st2 = crate::projection::State::new("> foo\n> bar\n");
    let snap2 = st2.snapshot();
    let mut fold2 = crate::infer::RecordFold::new(&snap2.frame_log.entries, &snap2.lines);
    crate::infer::walk_zone(&snap2.frame_log.entries, (0, snap2.line_len() as u32), &mut fold2);
    let (recs2, arena2) = fold2.finish();
    let blocks2 = crate::infer::build_block_records(&snap2.frame_log.entries, &snap2.lines, &snap2.open_depth, &snap2.rope, &recs2, &arena2, (0, snap2.line_len() as u32));
    let spans2 = crate::infer::LineView::new(&recs2, &blocks2, &snap2.frame_log.entries, &snap2.lines, &snap2.rope, (0, snap2.line_len() as u32), &arena2);
    assert!(DR::OnBlockStart.eval(&spans2, 0, 0, FrameKind::BlockQuote),
        "BQ 开行应 depth_only（is_block_start）");
}

#[test]
fn content_mode_values() {
    use crate::block::BlockType;
    // 单源化：容器表 content_mode 唯一入口
    // content_mode_for——直读 BLOCK_TYPE_EFFECTS（27 行全量）
    assert_eq!(crate::effect::content_mode_for(BlockType::Blockquote), ContentMode::Stripped);
    assert_eq!(crate::effect::content_mode_for(BlockType::CodeFenceOpen), ContentMode::Code);
    assert_eq!(crate::effect::content_mode_for(BlockType::HtmlBlock), ContentMode::Raw);
    assert_eq!(crate::effect::content_mode_for(BlockType::TableRow), ContentMode::Table);
    // TableSeparator 裁决: 容器优先时代 Table（Table 帧）——单源后
    // 块类型表真值 None_（ast_is_skip——内容从不提取）
    assert_eq!(crate::effect::content_mode_for(BlockType::TableSeparator), ContentMode::None_);
    // 修正（表化回归——旧硬编码 AngleBracket 分支返回注释内容
    // ——表化时误定 None_——恢复 Stripped——TestPjComment 暴露）
    assert_eq!(crate::effect::content_mode_for(BlockType::Comment), ContentMode::Stripped);
}

#[test]
fn body_layout_masks_semantics() {
    use crate::block::FrameKind as FK;
    use crate::effect::{decl_for, fc1_equiv, same_container};
    // mask 由 block.rs 位常量派生——受检不变量——
    // adm/out 分区语义: out 位不参与 admission（fc1_equiv/same_container/
    // classify 只读 adm 位）
    let kinds: [FK; 10] = [FK::BlockQuote, FK::BulletList, FK::OrderedList, FK::CodeFence,
        FK::HtmlBlock, FK::Table, FK::Comment, FK::HtmlElement, FK::RefDef, FK::TableRow];
    for kind in kinds {
        let body = decl_for(kind).body;
        assert_eq!(body.adm_mask & body.out_mask, 0, "{kind:?} adm/out 位域重叠");
        if body.out_mask == 0 { continue; }
        let base = crate::block::BlockFrame::new(kind, 2, 0);
        let flipped = crate::block::BlockFrame::new(kind, 2, body.out_mask as u8);
        assert!(fc1_equiv(base, flipped), "{kind:?} out 位翻转应保持 fc1 等价");
        assert!(same_container(base, flipped), "{kind:?} out 位翻转应保持同容器");
        // LOOSE(out) 翻转不改变列表分类（admission 判定）
        if matches!(kind, FK::BulletList | FK::OrderedList) {
            let mut facts = crate::block::LineFacts::from(b"  - x");
            facts.set_prev_type(Some(crate::block::BlockType::Paragraph));
            let s0 = [base];
            let s1 = [flipped];
            let rt0 = crate::block::ScanRuntime { stack: &s0, open_lines: &[], rope: None };
            let rt1 = crate::block::ScanRuntime { stack: &s1, open_lines: &[], rope: None };
            let sc0 = crate::block::classify_list_scene(&facts, &base, &rt0);
            let sc1 = crate::block::classify_list_scene(&facts, &flipped, &rt1);
            // ScanAction 无 PartialEq——Debug 形态比（单级化后 classify 直产结论）
            assert_eq!(format!("{:?}", sc0), format!("{:?}", sc1), "{kind:?} LOOSE(out) 翻转应不改变 classify_list_scene");
        }
    }
}

#[test]
fn ctx_guard_lazy_override() {
    // GuardLazy 是 scan 侧分类（不经 classify_ctx——
    // projection.rs PopPush guard 分支——行按 Paragraph 归属）——表条目
    // 锁: GuardLazy → RedirectTo(LazyPara)——apply 的 LazyPara 映射
    // Normal（行归属 scan 已处理——shape 无修改）——条目一致性防漂移
    let found = crate::effect::CTX_OVERRIDES.iter()
        .find(|(c, _)| *c == crate::effect::CtxClass::GuardLazy)
        .expect("GuardLazy 应在 CTX_OVERRIDES");
    assert!(matches!(found.1,
        Some(crate::effect::Override::RedirectTo(crate::effect::Render::LazyPara))),
        "GuardLazy 应映射 RedirectTo(LazyPara)");
    // apply 对 LazyPara 的 shape 映射（静态——apply 的 match 分支）
    assert!(matches!(crate::effect::override_for(crate::effect::CtxClass::GuardLazy),
        Some(crate::effect::Override::RedirectTo(crate::effect::Render::LazyPara))));
}

#[test]
fn container_type_str_matches_current() {
    // 与当前 container_type_str 值一致
    assert_eq!(effect_for(FrameKind::BlockQuote).container_type_str, "bq");
    assert_eq!(effect_for(FrameKind::BulletList).container_type_str, "bl");
    assert_eq!(effect_for(FrameKind::OrderedList).container_type_str, "od");
    assert_eq!(effect_for(FrameKind::CodeFence).container_type_str, "c");
    assert_eq!(effect_for(FrameKind::HtmlBlock).container_type_str, "hb");
    assert_eq!(effect_for(FrameKind::Table).container_type_str, "tb");
    assert_eq!(effect_for(FrameKind::Comment).container_type_str, "cm");
    assert_eq!(effect_for(FrameKind::HtmlElement).container_type_str, "he");
    assert_eq!(effect_for(FrameKind::RefDef).container_type_str, "rd");
}

// ── Inline 效应表 ──

#[test]
fn inline_effects_table_has_28_entries() {
    // 7 GeoKind × 4 sub = 28
    assert_eq!(INLINE_EFFECTS.len(), 28);
}

#[test]
fn inline_effect_key_coverage() {
    // 验证每种 GeoKind 都能查到有效的 effect
    use GeoKind::*;
    let geos = [Plain, PairDelim, Fenced, Bracket, AngleWrap, Escape, Entity];
    for geo in geos {
        for sub in 0u8..=3 {
            // 构造 InlineFrame — 使用对应 geo 的构造方法
            let frame = match geo {
                Plain => InlineFrame::plain(sub),
                PairDelim => {
                    if sub == 3 {
                        // strikethrough: delim_byte=2, closer_use=2
                        InlineFrame::pair_delim(2, 2)
                    } else {
                        InlineFrame::pair_delim(0, sub)
                    }
                }
                Fenced => InlineFrame::fenced(sub, 1),
                Bracket => match sub {
                    0 => InlineFrame::bracket(false, false),
                    1 => InlineFrame::bracket(true, false),
                    2 => InlineFrame::bracket(false, true),
                    _ => InlineFrame::bracket(false, false),
                },
                AngleWrap => InlineFrame::angle_wrap(sub),
                Escape => InlineFrame::escape(),
                Entity => InlineFrame::entity(),
            };
            let key = inline_effect_key(geo, frame);
            assert!(key < INLINE_EFFECTS.len(),
                "key out of bounds: geo={:?} sub={} key={}", geo, sub, key);
            let eff = &INLINE_EFFECTS[key];
            assert!(!eff.node_type.is_empty(),
                "empty node_type for geo={:?} sub={} key={}", geo, sub, key);
        }
    }
}

#[test]
fn inline_emphasis_node_types() {
    // 验证 emphasis / strong / strikethrough 的 nodeType
    let emph_1 = inline_effect_for(GeoKind::PairDelim, InlineFrame::pair_delim(0, 1));
    assert_eq!(emph_1.node_type, "em");
    assert!(emph_1.is_container);

    let strong = inline_effect_for(GeoKind::PairDelim, InlineFrame::pair_delim(0, 2));
    assert_eq!(strong.node_type, "st");
    assert!(strong.is_container);

    let strike = inline_effect_for(GeoKind::PairDelim, InlineFrame::pair_delim(2, 2));
    assert_eq!(strike.node_type, "dl");
    assert!(strike.is_container);
}

#[test]
fn inline_link_attrs() {
    let link = inline_effect_for(GeoKind::Bracket,
        InlineFrame::bracket(false, false));
    assert_eq!(link.node_type, "lk");
    assert_eq!(link.attr_fields.len(), 3); // href, content, title

    let image = inline_effect_for(GeoKind::Bracket,
        InlineFrame::bracket(true, false));
    assert_eq!(image.node_type, "im");
    assert_eq!(image.attr_fields.len(), 3); // src, alt, title

    let ref_link = inline_effect_for(GeoKind::Bracket,
        InlineFrame::bracket(false, true));
    assert_eq!(ref_link.node_type, "rf");
    assert_eq!(ref_link.attr_fields.len(), 1); // href only
}

#[test]
fn inline_title_is_optional() {
    let link = inline_effect_for(GeoKind::Bracket,
        InlineFrame::bracket(false, false));
    let title_attr = link.attr_fields.iter().find(|a| a.name == "ti").unwrap();
    assert!(title_attr.optional);
}

#[test]
fn inline_fenced_node_types() {
    let code = inline_effect_for(GeoKind::Fenced, InlineFrame::fenced(0, 1));
    assert_eq!(code.node_type, "cd");
    assert_eq!(code.content_field, ContentField::Code);

    let math = inline_effect_for(GeoKind::Fenced, InlineFrame::fenced(1, 1));
    assert_eq!(math.node_type, "mt");

    let dm = inline_effect_for(GeoKind::Fenced, InlineFrame::fenced(2, 1));
    assert_eq!(dm.node_type, "dm");
}

#[test]
fn inline_plain_node_types() {
    // 表值与 fast path 对齐（原 "tx"/"hb" 是死声明——Plain
    // 走 write_inline_node fast path——输出 "txt"/"sb"/"lb"——SDL 诊断抓漂移）
    let text = inline_effect_for(GeoKind::Plain, InlineFrame::plain(0));
    assert_eq!(text.node_type, "txt");
    assert_eq!(text.content_field, ContentField::Text);

    let sb = inline_effect_for(GeoKind::Plain, InlineFrame::plain(1));
    assert_eq!(sb.node_type, "sb");
    assert_eq!(sb.content_field, ContentField::StaticStr(" "));

    let hb = inline_effect_for(GeoKind::Plain, InlineFrame::plain(2));
    assert_eq!(hb.node_type, "lb");
    assert_eq!(hb.content_field, ContentField::StaticStr("\n"));
}

#[test]
fn inline_escape_and_entity() {
    let esc = inline_effect_for(GeoKind::Escape, InlineFrame::escape());
    assert_eq!(esc.node_type, "tx"); // MDIO 边界约定

    let ent = inline_effect_for(GeoKind::Entity, InlineFrame::entity());
    assert_eq!(ent.node_type, "entity");
    assert_eq!(ent.content_field, ContentField::DecodedEntity);
}

#[test]
fn inline_autolink_and_html_inline() {
    let autolink = inline_effect_for(GeoKind::AngleWrap, InlineFrame::angle_wrap(0));
    assert_eq!(autolink.node_type, "al");
    assert_eq!(autolink.content_field, ContentField::Href);
    // 排 bug：原 2 条（h + ct）——ct 由 content_field 已写——
    // 重复输出（`<http://a.b>` 两个 ct）——删 ct 条目（h 保留）
    assert_eq!(autolink.attr_fields.len(), 1); // href only

    // html_inline (sub=1): 当前不可达, 但表条目存在
    let hi = inline_effect_for(GeoKind::AngleWrap, InlineFrame::angle_wrap(1));
    assert_eq!(hi.node_type, "hi");
    assert_eq!(hi.content_field, ContentField::AngleWrapText);
}

// ── classify_delta — η_w⁻¹ 统一事件反推 ──

use crate::effect::{classify_delta, EventClass, GDelta};
use crate::block::FrameKind;

fn g_none() -> GDelta { GDelta::default() }
fn g_item() -> GDelta { GDelta { item_boundary: true, ..Default::default() } }
fn g_cc() -> GDelta { GDelta { container_change: true, ..Default::default() } }
fn g_loose() -> GDelta { GDelta { is_loose_flip: true, ..Default::default() } }

#[test]
fn cd_all_push_pop_are_c_info() {
    // All 9 FrameKind: Push/Pop → C_info. is_nesting distinction
    // handled by is_recursive_container at call sites, not here.
    for k in 0..9u8 {
        let kind: FrameKind = unsafe { std::mem::transmute(k) };
        assert_eq!(classify_delta(kind, 1, &g_none()), EventClass::C_info_Push, "{kind:?} Push");
        assert_eq!(classify_delta(kind, -1, &g_none()), EventClass::C_info_Pop, "{kind:?} Pop");
    }
}

#[test]
fn cd_same_depth_signals() {
    // Any kind: depth 0 + item_boundary → I_info (G already restricts to list)
    assert_eq!(classify_delta(FrameKind::BulletList, 0, &g_item()), EventClass::I_info_PopPush);
    // depth 0 + container_change → C_info cross
    assert_eq!(classify_delta(FrameKind::BulletList, 0, &g_cc()), EventClass::C_info_PopPush_cross);
    // depth 0 + loose_flip → T_info BodyMut
    assert_eq!(classify_delta(FrameKind::BulletList, 0, &g_loose()), EventClass::T_info_BodyMut_out);
}

#[test]
fn cd_continue_default() {
    // depth 0 + no signals → T_info_Continue, regardless of kind
    for k in [FrameKind::BlockQuote, FrameKind::CodeFence, FrameKind::Table] {
        assert_eq!(classify_delta(k, 0, &g_none()), EventClass::T_info_Continue, "{k:?}");
    }
}

#[test]
fn cd_item_boundary_takes_priority() {
    // item_boundary beats container_change — G never sets both, but if it did, item wins
    let both = GDelta { item_boundary: true, container_change: true, ..Default::default() };
    assert_eq!(classify_delta(FrameKind::BulletList, 0, &both), EventClass::I_info_PopPush);
}

#[test]
fn cd_loose_beats_continue() {
    let g = GDelta { is_loose_flip: true, ..Default::default() };
    assert_eq!(classify_delta(FrameKind::BulletList, 0, &g), EventClass::T_info_BodyMut_out);
}

// ── ContainerEffect.block_type / BlockTypeDecl.in_non_shell ──

#[test]
fn container_block_type_matches_legacy_mapping() {
    let cases = [
        (FrameKind::BlockQuote, BlockType::Blockquote),
        (FrameKind::BulletList, BlockType::Bullet),
        (FrameKind::OrderedList, BlockType::OrderedListItem),
        (FrameKind::CodeFence, BlockType::CodeFenceOpen),
        (FrameKind::HtmlBlock, BlockType::HtmlBlock),
        (FrameKind::Table, BlockType::TableRow),
        (FrameKind::Comment, BlockType::Comment),
        (FrameKind::HtmlElement, BlockType::HtmlElement),
        (FrameKind::RefDef, BlockType::RefDef),
        (FrameKind::TableRow, BlockType::TableRow),
    ];
    for (k, expected) in cases {
        assert_eq!(effect_for(k).block_type, expected, "{k:?}");
        assert_eq!(container_block_type(k), expected, "{k:?}");
    }
}

#[test]
fn is_list_container_matches_legacy_list_enum() {
    let cases = [
        (FrameKind::BlockQuote, false),
        (FrameKind::BulletList, true),
        (FrameKind::OrderedList, true),
        (FrameKind::CodeFence, false),
        (FrameKind::HtmlBlock, false),
        (FrameKind::Table, false),
        (FrameKind::Comment, false),
        (FrameKind::HtmlElement, false),
        (FrameKind::RefDef, false),
        (FrameKind::TableRow, false),
    ];
    for (k, expected) in cases {
        assert_eq!(is_list_container(k), expected, "{k:?}");
    }
}

#[test]
fn is_nesting_container_matches_decl() {
    let cases = [
        (FrameKind::BlockQuote, true),
        (FrameKind::BulletList, true),
        (FrameKind::OrderedList, true),
        (FrameKind::CodeFence, false),
        (FrameKind::HtmlBlock, false),
        (FrameKind::Table, false),
        (FrameKind::Comment, false),
        (FrameKind::HtmlElement, false),
        (FrameKind::RefDef, false),
        (FrameKind::TableRow, false),
    ];
    for (k, expected) in cases {
        assert_eq!(is_nesting_container(k), expected, "{k:?}");
        assert_eq!(decl_for(k).is_nesting, expected, "{k:?}");
    }
}

#[test]
fn needs_full_shell_matches_legacy_set() {
    let true_types = [
        BlockType::CodeFenceOpen,
        BlockType::Comment,
        BlockType::TableRow,
        BlockType::HtmlBlock,
        BlockType::CodeContent,
        BlockType::HtmlContent,
        BlockType::TableSeparator,
        BlockType::RefDef,
        BlockType::HtmlElement,
    ];
    for t in true_types {
        assert!(needs_full_shell(t), "{t:?} should need full shell");
    }
    for i in 0..27u8 {
        let t: BlockType = unsafe { std::mem::transmute(i) };
        if !true_types.contains(&t) {
            assert!(!needs_full_shell(t), "{t:?} should not need full shell");
        }
    }
}

#[test]
fn all_block_type_tables_cover_27_variants() {
    for i in 0..27u8 {
        let t: BlockType = unsafe { std::mem::transmute(i) };
        let _ = BLOCK_TYPE_META[t as usize];
        let _ = BLOCK_TYPE_DECLS[t as usize];
        let _ = BLOCK_TYPE_EFFECTS[t as usize];
        let _ = LINE_SPECS[t as usize];
        let _ = BLOCK_ATTRS[t as usize];
    }
}

/// IC 谓词关系钉：IC 几何谓词（mark_width==Fixed("    ")）蕴含提取谓词
/// （content_mode==Code），反之不然（CodeFenceOpen/CodeContent 也是 Code
/// 但非 Fixed("    ")）——两个谓词是**不同等价类**，各已单源（几何=LINE_SPECS
/// 列、提取=BLOCK_TYPE_EFFECTS 列）；「IC 双认」经此钉判定为假缺口。
#[test]
fn ic_predicates_subset_all_27() {
    let mut ic_count = 0;
    for i in 0..27u8 {
        let t: BlockType = unsafe { std::mem::transmute(i) };
        let by_mode = content_mode_for(t) == ContentMode::Code;
        let by_geom = matches!(line_spec(t).mark_width, MarkWidth::Fixed("    "));
        assert!(
            !by_geom || by_mode,
            "Fixed(\"    \") 类型必须是 Code: BlockType {i}"
        );
        if by_geom {
            ic_count += 1;
        }
    }
    assert_eq!(ic_count, 1, "Fixed(\"    \") 几何谓词必须唯一命中 IndentedCode");
}
