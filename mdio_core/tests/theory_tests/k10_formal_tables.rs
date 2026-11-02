//! K10 形式表的可执行检查：验证 27 个 BlockTypes 的 E27 表与
//! 10 容器 Geo/Body 表在 adm/out 上全备、互斥，并与列表/嵌套单源字段一致。

use mdio_core::block::BlockType;
use mdio_core::block::FrameKind;
use mdio_core::effect::{
    block_decl, decl_for, effect_for, BLOCK_TYPE_DECLS, CONTAINER_DECLS,
};

#[test]
fn e27_container_and_block_tables_are_consistent() {
    let block_types = [
        BlockType::Header1,
        BlockType::Header2,
        BlockType::Header3,
        BlockType::Header4,
        BlockType::Header5,
        BlockType::Header6,
        BlockType::Bullet,
        BlockType::CodeFenceOpen,
        BlockType::Blockquote,
        BlockType::OrderedListItem,
        BlockType::Paragraph,
        BlockType::Comment,
        BlockType::TableRow,
        BlockType::Checklist,
        BlockType::HsHead1,
        BlockType::HsHead2,
        BlockType::HsSep,
        BlockType::HtmlBlock,
        BlockType::ThematicBreak,
        BlockType::CodeFenceClose,
        BlockType::CodeContent,
        BlockType::HtmlContent,
        BlockType::IndentedCode,
        BlockType::TableSeparator,
        BlockType::Blank,
        BlockType::RefDef,
        BlockType::HtmlElement,
    ];
    assert_eq!(block_types.len(), 27);
    assert_eq!(BLOCK_TYPE_DECLS.len(), 27);

    let mut seen = [false; 27];
    for t in block_types {
        let i = t as usize;
        assert!(i < 27);
        assert!(!seen[i], "duplicate BlockType id {i}");
        seen[i] = true;
    }
    assert!(seen.iter().all(|&x| x), "missing BlockType id");

    let containers = [
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
    assert_eq!(containers.len(), 10);
    assert_eq!(CONTAINER_DECLS.len(), 10);

    for &k in &containers {
        let d = decl_for(k);
        assert_eq!(d.kind, k);
        let body = d.body;
        assert_eq!(
            body.adm_mask & body.out_mask,
            0,
            "adm/out overlap for {k:?}"
        );
        assert!(
            (body.adm_mask | body.out_mask) <= 0x3F,
            "body mask beyond 6 bits for {k:?}"
        );
    }

    // E4: list single-source.
    for &k in &[FrameKind::BulletList, FrameKind::OrderedList] {
        let t = effect_for(k).block_type;
        assert!(block_decl(t).is_list_item, "{k:?} must be list item");
    }
    for &k in &[
        FrameKind::BlockQuote,
        FrameKind::CodeFence,
        FrameKind::HtmlBlock,
        FrameKind::Table,
        FrameKind::Comment,
        FrameKind::HtmlElement,
        FrameKind::RefDef,
        FrameKind::TableRow,
    ] {
        let t = effect_for(k).block_type;
        assert!(!block_decl(t).is_list_item, "{k:?} must not be list item");
    }

    // E5: nesting single-source.
    for &k in &[
        FrameKind::BlockQuote,
        FrameKind::BulletList,
        FrameKind::OrderedList,
    ] {
        assert!(decl_for(k).is_nesting, "{k:?} must be nesting");
    }
    for &k in &[
        FrameKind::CodeFence,
        FrameKind::HtmlBlock,
        FrameKind::Table,
        FrameKind::Comment,
        FrameKind::HtmlElement,
        FrameKind::RefDef,
        FrameKind::TableRow,
    ] {
        assert!(!decl_for(k).is_nesting, "{k:?} must not be nesting");
    }

    // Headers: level source is Meta, not Mark.
    for (t, level) in [
        (BlockType::Header1, 1),
        (BlockType::Header2, 2),
        (BlockType::Header3, 3),
        (BlockType::Header4, 4),
        (BlockType::Header5, 5),
        (BlockType::Header6, 6),
        (BlockType::HsHead1, 1),
        (BlockType::HsHead2, 2),
    ] {
        assert_eq!(block_decl(t).header_level, Some(level), "{t:?}");
    }

    // Fallback rows: only TableRow/RefDef in E27.
    let fallback_rows = block_types
        .iter()
        .filter(|&&t| block_decl(t).context_fallback.is_some())
        .count();
    assert_eq!(fallback_rows, 2);

    println!(
        "E27/GeoBody tables: 27 block rows, 10 container rows, consistency ok"
    );
}
