//! MDIO 效应表 — 静态表驱动（概念格 → JSON 发射配置）。
//!
//! 表驱动架构（数据化重构的产物——生产查表、SDL 独立推导比对对象）:
//!   容器效应: CONTAINER_EFFECTS（壳/注入/closing 语义）+ CONTAINER_DECLS
//!     （scan 判定列——admit/classify/rules/约束链）
//!   块类型:   BLOCK_TYPE_DECLS（merges/打断/后向确认改写）+
//!     BLOCK_TYPE_EFFECTS（content_mode/trailing/corrections）+
//!     BLOCK_TYPE_META（命名/跳过）+ BLOCK_ATTRS（写面属性）
//!   行几何:   LINE_SPECS（mark_width/tag_rule/content_rule…）
//!   内联:     INLINE_EFFECTS（(GeoKind×sub) 索引）
//!   呈现修改: CTX_OVERRIDES（CtxClass × Override——apply 查表）
//!
//! 核心公式:
//!   effect(type) = TABLE[type.kind]——O(1) 直接索引（FrameKind/BlockType 为 repr(u8)）

use crate::block::{BlockFrame, BlockType, FrameKind, LineFacts, ScanAction};
use crate::inline::{GeoKind, InlineField, InlineFrame};

/// 注入子节点模板。
///
/// 为什么是切片而不是枚举?
///   枚举 (None | Paragraph | ItemLeaf) 把当前碰巧的三种模式固化为类型系统的一部分。
///   将来新容器需要注入 [Badge, Paragraph] 两个异构子节点时, 枚举要加第四个变体,
///   所有 match 处都要改。切片不需要 — 遍历 1-2 个元素的静态切片, 纳秒级, 零成本。
///   保留切片同时保留了通用性和心智模型: "容器打开时可以自动注入任意子节点序列"。
#[derive(Debug, Clone, Copy)]
pub struct NodeTemplate {
    /// 注入节点的类型。ItemLeaf 是哨兵值, 表示使用效应表的 item_leaf_type_str。
    pub ty: NodeTemplateType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeTemplateType {
    /// 注入 item leaf — 壳内嵌的虚拟帧。实际 nodeType 从 item_leaf_type_str 读取。
    ItemLeaf,
}

/// 内容提取模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentMode {
    /// HtmlBlock: raw passthrough, 不解析 inline
    Raw,
    /// BQ, BLT, OL, Paragraph, HtmlElement: 剥离容器前缀
    Stripped,
    /// CF, IC: 代码内容; JSON key 用 "cd" 而非 "ct"
    Code,
    /// TableRow: 制表符分隔
    Table,
    /// HsSep, ThematicBreak, CodeFenceClose, Blank, RefDef: 无内容
    None_,
}

/// Output routing for containers that don't go to the block tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffect { PopulateRefMap, Invisible }

/// 容器效应表条目——容器渲染语义的数据化声明。
#[derive(Debug, Clone)]
pub struct ContainerEffect {
    /// attrs 在容器壳上（is_shell）还是 item leaf 上？BQ 是特例：
    /// is_shell=true 但无 item leaf——attrs 在 "bq" 壳自身。
    pub is_shell: bool,

    /// FrameKind → 语义 BlockType 的唯一映射（container_block_type /
    /// content_type_of 的单一来源）。
    pub block_type: BlockType,

    /// 完全透明, 不产容器壳 (仅 HtmlBlock)。
    pub is_transparent: bool,

    /// 壳的 nodeType 字符串。
    pub container_type_str: &'static str,

    /// item leaf 的 nodeType 字符串。
    /// 仅当 auto_inject 包含 ItemLeaf 时有效; 否则为 ""。
    pub item_leaf_type_str: &'static str,

    /// 容器壳打开时自动注入的子节点序列。
    ///
    /// BQ/CF/Table/Comment/HtmlBlock/HtmlElement/RefDef/TableRow: []
    /// BLT/OL:    [ItemLeaf]
    pub auto_inject: &'static [NodeTemplate],

    /// ItemLeaf 注入后, 若 is_loose 为 true, 用此 nodeType 包裹内容。
    /// BLT/OL → Some("p"); 其他 → None。
    pub loose_wrapper: Option<&'static str>,

    /// 深度通道判定（fn 指针 → 枚举——4 个谓词可枚举可验证（SDL 可量化）。
    /// 原 fn 指针的"将来新容器任意谓词"通用性由枚举变体保留（可扩展），
    /// 换取可验证性（枚举穷举——受检不变量）。eval 保留 fk 参数（当前变体
    /// 不需要）。
    pub depth_only_pred: DepthOnlyRule,

    /// Output routing. None → block tree (default).
    pub side_effect: Option<SideEffect>,
    /// [f] 帧谓词：列④ closing Pop 行的包含语义（单源——NoClose/Specific——
    /// 原 end_includes_close bool + Option 双列合一）。F/P 一致由单表保证
    /// （span_end 消费——projection/incr 共用）。
    pub closing_pl: ClosingPl,
}

// ── depth_only 判定规则（枚举数据化）──

/// 深度通道判定规则（"该行是否创建容器节点还是并入父"——容器开壳语义）。
/// 枚举可穷举可验证——SDL 可量化（与 sdl Atom/eval 同构）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthOnlyRule {
    /// 总是 depth_only (BLT, OL, CF, HtmlBlock, HtmlElement, RefDef)
    Always,
    /// 仅容器开行（BQ——首个内容行由 leaf 渲染（EMPTY_INJECT）——同深度
    /// 后续行是常规块）
    OnBlockStart,
    /// 栈上存在指定帧类型（Comment——同行配对 <!-- --> 无帧 → 非
    /// depth_only → 走 Path B）
    WhenFrameInStack(FrameKind),
    /// 表格已确认（separator 验证后——伪表内容行按 Paragraph 渲染）
    WhenTableConfirmed,
}

impl DepthOnlyRule {
    /// 求值（保留 fk 参数——当前变体不需要）。
    /// 泛型化（impl ResolveView——FrameSpans/RowView 双实现）。
    pub fn eval(self, view: &dyn ResolveView,
                line: usize, depth: usize, _fk: FrameKind) -> bool {
        match self {
            DepthOnlyRule::Always => true,
            DepthOnlyRule::OnBlockStart => view.is_block_start(line),
            DepthOnlyRule::WhenFrameInStack(k) => (0..depth).any(|d| {
                crate::block::frame_kind(view.node_type_at_depth(line, d)) == Some(k)
            }),
            DepthOnlyRule::WhenTableConfirmed => view.table_confirmed_at(line),
        }
    }
}

// ── 静态表 — 按 FrameKind 判别式直接索引 ──

/// BLT/OL: [ItemLeaf]
static LIST_INJECT: &[NodeTemplate] = &[NodeTemplate { ty: NodeTemplateType::ItemLeaf }];

/// 空注入 (BQ, CF, Table, Comment, HtmlBlock, HtmlElement, RefDef, TableRow)
static EMPTY_INJECT: &[NodeTemplate] = &[];

/// 效应表 — 10 行, FrameKind 已是 #[repr(u8)], 直接索引。
pub static CONTAINER_EFFECTS: [ContainerEffect; 10] = [
    // index 0: FrameKind::BlockQuote
    ContainerEffect {
        is_shell: true,
        block_type: BlockType::Blockquote,
        is_transparent: false,
        container_type_str: "bq",
        item_leaf_type_str: "",
        auto_inject: EMPTY_INJECT,  // BQ 段落由 leaf 渲染（内容层 resolved=Paragraph）
        loose_wrapper: None,
        depth_only_pred: DepthOnlyRule::OnBlockStart,
        side_effect: None,
        closing_pl: ClosingPl::NoClose,
    },
    // index 1: FrameKind::BulletList
    ContainerEffect {
        is_shell: true,
        block_type: BlockType::Bullet,
        is_transparent: false,
        container_type_str: "bl",
        item_leaf_type_str: "b",
        auto_inject: LIST_INJECT,
        loose_wrapper: Some("p"),
        depth_only_pred: DepthOnlyRule::Always,
        side_effect: None,
        closing_pl: ClosingPl::NoClose,
    },
    // index 2: FrameKind::OrderedList
    ContainerEffect {
        is_shell: true,
        block_type: BlockType::OrderedListItem,
        is_transparent: false,
        container_type_str: "od",
        item_leaf_type_str: "ol",
        auto_inject: LIST_INJECT,
        loose_wrapper: Some("p"),
        depth_only_pred: DepthOnlyRule::Always,
        side_effect: None,
        closing_pl: ClosingPl::NoClose,
    },
    // index 3: FrameKind::CodeFence
    ContainerEffect {
        is_shell: false,
        block_type: BlockType::CodeFenceOpen,
        is_transparent: false,
        container_type_str: "c",
        item_leaf_type_str: "",
        auto_inject: EMPTY_INJECT,
        loose_wrapper: None,
        depth_only_pred: DepthOnlyRule::Always,
        side_effect: None,
        closing_pl: ClosingPl::Specific(BlockType::CodeFenceClose),
    },
    // index 4: FrameKind::HtmlBlock
    ContainerEffect {
        is_shell: false,
        block_type: BlockType::HtmlBlock,
        is_transparent: false,
        container_type_str: "hb",
        item_leaf_type_str: "",
        auto_inject: EMPTY_INJECT,
        loose_wrapper: None,
        depth_only_pred: DepthOnlyRule::Always,
        side_effect: None,
        // 列④：closing 行 PL=HtmlContent（HTML_SCENE_ENDS/空行结束 type 6-7——
        // 含）；约束 Pop（Paragraph——bar 行弹 hb）→ 不含（原 None 的
        // map_or(true) 兜底把约束 Pop 也含入块——双输出）。
        closing_pl: ClosingPl::Specific(BlockType::HtmlContent),
    },
    // index 5: FrameKind::Table
    ContainerEffect {
        is_shell: false,
        block_type: BlockType::TableRow,
        is_transparent: false,
        container_type_str: "tb",
        item_leaf_type_str: "",
        auto_inject: EMPTY_INJECT,
        loose_wrapper: None,
        depth_only_pred: DepthOnlyRule::WhenTableConfirmed,
        side_effect: None,
        closing_pl: ClosingPl::NoClose,
    },
    // index 6: FrameKind::Comment
    ContainerEffect {
        is_shell: false,
        block_type: BlockType::Comment,
        is_transparent: false,
        container_type_str: "cm",
        item_leaf_type_str: "",
        auto_inject: EMPTY_INJECT,
        loose_wrapper: None,
        depth_only_pred: DepthOnlyRule::WhenFrameInStack(FrameKind::Comment),
        // content_mode 已移出容器表（per-BlockType 属性——直读
        // BLOCK_TYPE_EFFECTS）。回归修正值保留在块类型表（Comment=Stripped）。
        side_effect: Some(SideEffect::Invisible),
        closing_pl: ClosingPl::Specific(BlockType::Comment),
    },
    // index 7: FrameKind::HtmlElement
    ContainerEffect {
        is_shell: false,
        block_type: BlockType::HtmlElement,
        is_transparent: false,
        container_type_str: "he",
        item_leaf_type_str: "",
        auto_inject: EMPTY_INJECT,
        loose_wrapper: None,
        depth_only_pred: DepthOnlyRule::Always,
        side_effect: None,
        closing_pl: ClosingPl::NoClose,
    },
    // index 8: FrameKind::RefDef
    ContainerEffect {
        is_shell: false,
        block_type: BlockType::RefDef,
        is_transparent: false,
        container_type_str: "rd",
        item_leaf_type_str: "",
        auto_inject: EMPTY_INJECT,
        loose_wrapper: None,
        depth_only_pred: DepthOnlyRule::Always,
        side_effect: Some(SideEffect::PopulateRefMap),
        closing_pl: ClosingPl::NoClose,
    },
    // index 9: FrameKind::TableRow
    ContainerEffect {
        is_shell: false,
        block_type: BlockType::TableRow,
        is_transparent: false,
        container_type_str: "tr",
        item_leaf_type_str: "",
        auto_inject: EMPTY_INJECT,
        loose_wrapper: None,
        depth_only_pred: DepthOnlyRule::Always,
        side_effect: None,
        closing_pl: ClosingPl::NoClose,
    },
];

pub fn effect_for(kind: FrameKind) -> &'static ContainerEffect {
    &CONTAINER_EFFECTS[kind as usize]
}

pub fn decl_for(kind: FrameKind) -> &'static ContainerDecl {
    &CONTAINER_DECLS[kind as usize]
}

/// 节点类型的内容提取模式——表驱动入口（content_mode 本质是 per-BlockType——
/// 容器表直读 BLOCK_TYPE_EFFECTS 27 行全量单源）。
/// TableSeparator 裁决: None_（容器优先时代 Table——但 ast_is_skip——
/// 内容从不提取——None_ 才是真值）。
pub fn content_mode_for(t: crate::block::BlockType) -> ContentMode {
    BLOCK_TYPE_EFFECTS[t as usize].content_mode
}


// BLOCK_TYPE_META — 统一命名/跳过表（4 个散落命名函数合一：node_type_to_str
// （debug_name）/ast_block_type_bytes + node_type_named_str（compact_name——
// 容器 fallback effect 表）/ast_is_skip（is_skip）——SDL 可穷举比对）


/// 每个 BlockType 的命名 + 元数据。
#[derive(Debug, Clone, Copy)]
pub struct BlockTypeMeta {
    /// 调试/日志名（原 node_type_to_str——block_type_name 输出）
    pub debug_name: &'static str,
    /// 紧凑 JSON type（原 node_type_named_str——"h"/"b"/"ol"/"c"…
    /// 空串 = 运行时查 effect 表（容器类型）或 "??"（无节点类型）
    pub compact_name: &'static str,
    /// 不产 AST 节点（原 ast_is_skip）
    pub is_skip: bool,
}

pub static BLOCK_TYPE_META: [BlockTypeMeta; 27] = [
    // 0-5 Header1-6
    BlockTypeMeta { debug_name: "head", compact_name: "h", is_skip: false },
    BlockTypeMeta { debug_name: "head", compact_name: "h", is_skip: false },
    BlockTypeMeta { debug_name: "head", compact_name: "h", is_skip: false },
    BlockTypeMeta { debug_name: "head", compact_name: "h", is_skip: false },
    BlockTypeMeta { debug_name: "head", compact_name: "h", is_skip: false },
    BlockTypeMeta { debug_name: "head", compact_name: "h", is_skip: false },
    // 6 Bullet — compact 查 effect（item_leaf "b"）
    BlockTypeMeta { debug_name: "bullet", compact_name: "", is_skip: false },
    // 7 CodeFenceOpen — compact 查 effect（"c"）
    BlockTypeMeta { debug_name: "code", compact_name: "", is_skip: false },
    // 8 Blockquote — compact 查 effect（"bq"）
    BlockTypeMeta { debug_name: "blockquote", compact_name: "", is_skip: false },
    // 9 OrderedListItem — compact 查 effect（item_leaf "ol"）
    BlockTypeMeta { debug_name: "ordered_list", compact_name: "", is_skip: false },
    // 10 Paragraph
    BlockTypeMeta { debug_name: "paragraph", compact_name: "p", is_skip: false },
    // 11 Comment — compact 查 effect（"cm"）
    BlockTypeMeta { debug_name: "comment", compact_name: "", is_skip: false },
    // 12 TableRow — compact 查 effect（"tr"）
    BlockTypeMeta { debug_name: "table", compact_name: "", is_skip: false },
    // 13 Checklist
    BlockTypeMeta { debug_name: "checklist", compact_name: "x", is_skip: false },
    // 14-15 HsHead1/2
    BlockTypeMeta { debug_name: "head", compact_name: "h", is_skip: false },
    BlockTypeMeta { debug_name: "head", compact_name: "h", is_skip: false },
    // 16 HsSep
    BlockTypeMeta { debug_name: "hs_sep", compact_name: "", is_skip: false },
    // 17 HtmlBlock — compact 查 effect（"hb"）
    BlockTypeMeta { debug_name: "html", compact_name: "", is_skip: false },
    // 18 ThematicBreak
    BlockTypeMeta { debug_name: "thematic_break", compact_name: "hr", is_skip: false },
    // 19 CodeFenceClose — compact "??"（无节点）
    BlockTypeMeta { debug_name: "code", compact_name: "", is_skip: true },
    // 20 CodeContent
    BlockTypeMeta { debug_name: "code", compact_name: "", is_skip: true },
    // 21 HtmlContent
    BlockTypeMeta { debug_name: "html", compact_name: "", is_skip: true },
    // 22 IndentedCode
    BlockTypeMeta { debug_name: "indtcode", compact_name: "ic", is_skip: false },
    // 23 TableSeparator — compact "tr"（skip——原 node_type_named_str 的
    // TableRow|TableSeparator 分支）
    BlockTypeMeta { debug_name: "table", compact_name: "tr", is_skip: true },
    // 24 Blank
    BlockTypeMeta { debug_name: "blank", compact_name: "", is_skip: true },
    // 25 RefDef — compact 查 effect（"rd"）
    BlockTypeMeta { debug_name: "refdef", compact_name: "", is_skip: false },
    // 26 HtmlElement — compact 查 effect（"he"）
    BlockTypeMeta { debug_name: "html_element", compact_name: "", is_skip: false },
];

/// O(1) 查紧凑名（原 node_type_named_str——行为逐值一致——表值对照已验）。
/// 容器类型（compact_name 空）fallback 到 effect 表（item_leaf/container——
/// item 判定用 is_list_item 列）；无节点类型（fallback 失败）→ "??"。
pub fn compact_type_name(t: BlockType) -> &'static str {
    let meta = &BLOCK_TYPE_META[t as usize];
    if !meta.compact_name.is_empty() { return meta.compact_name; }
    if let Some(fk) = crate::block::frame_kind(t) {
        let eff = effect_for(fk);
        if block_decl(t).is_list_item {
            return eff.item_leaf_type_str;
        }
        return eff.container_type_str;
    }
    "??"
}

/// O(1) 跳过判定（原 ast_is_skip）。
#[inline]
pub fn meta_is_skip(t: BlockType) -> bool {
    BLOCK_TYPE_META[t as usize].is_skip
}


// Block 属性表 — write_attrs 数据化 (对标 INLINE_EFFECTS.attr_fields)

//
// 设计原则: 属性列表是数据, 提取逻辑 (read_block_proj / node_tag / node_content)
// 一行不动 — 所以 `-`/`*`/`+` bullet 标记、refdef title、fence lang 等宝贵
// 细节全部原样保留。write_attrs 只是从"硬编码一串 read_block_proj 调用"变成
// "查表驱动同一串调用"。
//
// 每种类型的 attr 顺序 = 旧 write_attrs 的发射顺序: level, text, tag, content, code
// (RefDef 特例: label, url, title)。

/// 属性来源 — 复用现有投影/提取函数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockAttrSrc {
    /// read_block_proj(node, Tag)
    Tag,
    /// read_block_proj(node, Content)
    Content,
    /// read_block_proj(node, Raw)
    Raw,
    /// header_level(t) as u32 (仅 header 类型; 非 optional, 恒发射)
    Level,
    /// parse_refdef_multi(read_block_proj(node, Raw)).2 (RefDef title)
    RefDefTitle,
    /// parse_refdef_multi(read_block_proj(node, Raw)).0 (RefDef label——raw
    /// 提取——Tag/Content 投影对 RefDef 为空，label/href 需额外解析)
    RefDefLabel,
    /// parse_refdef_multi(read_block_proj(node, Raw)).1 (RefDef href)
    RefDefHref,
}

/// 单个 block JSON 属性的描述。
#[derive(Debug, Clone, Copy)]
pub struct BlockAttrSpec {
    /// JSON key 名称
    pub name: &'static str,
    /// 数据来源
    pub src: BlockAttrSrc,
    /// true = 值为空时跳过 (旧行为: 所有字符串 attr 都 if !empty)
    pub optional: bool,
    /// true = loose list item 时抑制 (内容改走 paragraph 包裹)
    pub suppress_when_loose: bool,
}

const fn attr(name: &'static str, src: BlockAttrSrc, optional: bool, suppress_when_loose: bool) -> BlockAttrSpec {
    BlockAttrSpec { name, src, optional, suppress_when_loose }
}

use BlockAttrSrc::{Content, Level, Raw, RefDefHref, RefDefLabel, RefDefTitle, Tag};

static HEADER_ATTRS: &[BlockAttrSpec] = &[
    attr("lv", Level, false, false),
    attr("t", Tag, true, false),
    attr("ct", Content, true, false),
];
static LIST_ITEM_ATTRS: &[BlockAttrSpec] = &[
    attr("t", Tag, true, false),
    attr("ct", Content, true, true),
];
static CODE_FENCE_ATTRS: &[BlockAttrSpec] = &[
    attr("t", Tag, true, false),
    attr("cd", Content, true, false),
];
static GENERIC_ATTRS: &[BlockAttrSpec] = &[
    attr("t", Tag, true, false),
    attr("ct", Content, true, false),
];
static HTML_BLOCK_ATTRS: &[BlockAttrSpec] = &[
    attr("t", Tag, true, false),
    attr("ct", Content, true, false), // 经统一读面入口(node_content)——容器前缀剥层
];
static INDENTED_CODE_ATTRS: &[BlockAttrSpec] = &[
    attr("t", Tag, true, false),
    attr("cd", Content, true, false),
];
pub static TAG_ONLY_ATTRS: &[BlockAttrSpec] = &[
    attr("t", Tag, true, false),
];
static REFDEF_ATTRS: &[BlockAttrSpec] = &[
    attr("t", RefDefLabel, true, false),
    attr("u", RefDefHref, true, false),
    attr("ti", RefDefTitle, true, false),
];

/// 27 行, 按 BlockType 判别式直接索引 (Header1=0 .. HtmlElement=26)。
///
/// 对应旧 write_attrs 的全部条件:
///   - content 排除: CodeFenceOpen/Close/CodeContent 不在列表放 content (CodeFenceOpen 用 code)
///   - code: 仅 CodeFenceOpen/IndentedCode
///   - loose 抑制: Bullet/Checklist/OrderedListItem 的 content
///   - HtmlBlock content 用 Raw
///   - RefDef 特例: label/url/title, 无通用 tag
pub static BLOCK_ATTRS: [&[BlockAttrSpec]; 27] = [
    HEADER_ATTRS,        // 0  Header1
    HEADER_ATTRS,        // 1  Header2
    HEADER_ATTRS,        // 2  Header3
    HEADER_ATTRS,        // 3  Header4
    HEADER_ATTRS,        // 4  Header5
    HEADER_ATTRS,        // 5  Header6
    LIST_ITEM_ATTRS,     // 6  Bullet
    CODE_FENCE_ATTRS,    // 7  CodeFenceOpen
    GENERIC_ATTRS,       // 8  Blockquote
    LIST_ITEM_ATTRS,     // 9  OrderedListItem
    GENERIC_ATTRS,       // 10 Paragraph
    GENERIC_ATTRS,       // 11 Comment
    GENERIC_ATTRS,       // 12 TableRow
    LIST_ITEM_ATTRS,     // 13 Checklist
    HEADER_ATTRS,        // 14 HsHead1
    HEADER_ATTRS,        // 15 HsHead2
    GENERIC_ATTRS,       // 16 HsSep
    HTML_BLOCK_ATTRS,    // 17 HtmlBlock
    GENERIC_ATTRS,       // 18 ThematicBreak
    TAG_ONLY_ATTRS,      // 19 CodeFenceClose
    TAG_ONLY_ATTRS,      // 20 CodeContent
    GENERIC_ATTRS,       // 21 HtmlContent
    INDENTED_CODE_ATTRS, // 22 IndentedCode
    GENERIC_ATTRS,       // 23 TableSeparator
    GENERIC_ATTRS,       // 24 Blank
    REFDEF_ATTRS,        // 25 RefDef
    GENERIC_ATTRS,       // 26 HtmlElement
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlankPolicy { Break, Allow }

/// 列④：closing Pop 行的包含语义（end_includes_close 的细化——单源）。
/// Option 歧义（None = "无 closing 概念" vs "任意行含"——HtmlBlock 双输出）
/// 从类型层消除——语义函数全定义——消费点无解释权。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosingPl {
    /// 容器无 closing 行概念——Pop 行不属于块（原 end_includes_close=false）
    NoClose,
    /// closing 行的 PL——pop 行 PL == Specific → 行属于块（closing 行含）；
    /// 否则（约束 Pop——Paragraph/Blank——fence 由 containing block 结束闭合）不含
    Specific(BlockType),
}

/// 结算原子——容器 Pop 行的块尾语义（closing_pl 列求值器——与表同住，
/// 正逆同列律：boundary_for/RecordFold/BlockEndCursor 共消费）。
/// 契约：结果 ∈ {pop_line, pop_line+1}；NoClose 恒 pop_line；
/// Specific(pl) 当且仅当 pop_pl == pl 时 pop_line + 1。
/// 机械零 kind 分支：只匹配注入后的表值，不枚举任何容器类型。
#[inline]
pub fn end_for_rule(kind: FrameKind, pop_line: usize, pop_pl: BlockType) -> usize {
    match effect_for(kind).closing_pl {
        ClosingPl::NoClose => pop_line,
        ClosingPl::Specific(pl) => {
            if pop_pl == pl { pop_line + 1 } else { pop_line }
        }
    }
}

/// 段落打断规则——CM §4.1 的 per-type can/cannot interrupt paragraph——
/// 单列两消费点（顶层 classify_at_top 拒绝集 + lazy classify_bq_scene 打断
/// 判定）——消除双表达。几何（开行缩进 ≤3——CM §4.1/4.5/4.6 全限）与修饰
/// （number-1/空 item/type7）由消费点统一应用（interrupts_para——感知原子
/// 词法谓词）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptRule {
    /// 不能打断段落（IC/RefDef/段落内容/HTML type 7 等）
    No,
    /// 可打断（ATX/thematic/fence/BQ/表格——开行条件满足时）
    Yes,
    /// 列表 marker——需修饰：非空 item + OL 的 number==1（数值——CM §5.3）
    List,
    /// HtmlBlock——需修饰：type ≠ 7（CM §4.6——type 7 不能打断）
    Html,
}

#[derive(Debug, Clone, Copy)]
pub struct BlockTypeDecl {
    /// [L] 纯行谓词：merges 块类型（Part A 连续段）
    pub merges: bool,
    /// [L] 纯行谓词：空行策略（IC 段内空行吸收）
    pub blank_policy: BlankPolicy,
    /// [L] 纯行谓词：打断规则（CM §4.1）——单源——顶层/lazy 两消费点查
    /// 同一列——几何（开行缩进 ≤3）与修饰（number-1/空 item/type7）由消费点
    /// 统一应用（interrupts_para——感知原子词法谓词）
    pub interrupt_rule: InterruptRule,
    /// [prev] 相邻谓词：prev 是此类型 → item/BQ 段落关闭（lazy 只覆盖段落——
    /// 块内容后无 lazy continuation）。增量完整性由扫描器私有的承诺列
    /// (prev_col——projection.rs 内 scan/ripple 自读,无跨模块读者)保证；
    /// PrevSlot 是 fold 的占位类型。
    pub closes_paragraph: bool,
    /// [L] 纯行谓词（列表 item 类型：Bullet/Checklist/OrderedListItem——walk
    /// 的 10 处 matches! 表驱动化——与 depth_only 枚举化同模式）
    pub is_list_item: bool,
    /// 非 shell 内容/容器块：编辑时需要完整容器进 zone（needs_full_shell 单源）。
    pub in_non_shell: bool,
    /// 后向确认改写:块类型的有效解析依赖后续行/块闭
    /// 确认（GFM 表格 = 表头+分隔两行前瞻——单行词法 PL 无法自证）——谓词
    /// 失败退化为 fallback。消费:两个写入点（compute_derived_spans 的
    /// resolved 列 / build_block_records 的 rt）查表改写——resolve/树层读改写
    /// 后值——零补丁。
    pub context_fallback: Option<ContextFallback>,
    /// [L] 标题级别——全仓"是否标题 + 级别"单一来源：
    /// Header1..6 = Some(1..6)；HsHead1 = Some(1)、HsHead2 = Some(2)
    /// （setext 与 ATX 的等价 = 两行同值）；HsSep 及其余 = None。
    pub header_level: Option<u8>,
}

/// 后向确认改写条目（见 BlockTypeDecl.context_fallback）。
#[derive(Debug, Clone, Copy)]
pub struct ContextFallback {
    pub fallback: BlockType,
    /// true = 保持原类型;false = 改写为 fallback。
    pub predicate: fn(&BlockRewriteCtx, usize) -> bool,
}

/// 谓词上下文——两个写入点都可构造（view + rope——行内容判定）。
pub struct BlockRewriteCtx<'a> {
    /// 行相位视图：改写谓词只问行相位原语（表行确认 / 容器尾 / 窗口尾），
    /// 因此构建期视图（BuildView）可直接入位，无需结构就绪。
    pub view: &'a dyn LineResolveView,
    pub rope: &'a crop::Rope,
}

// ── 相位层（S3 从 domain.rs 迁入）──
//
// 相位标记类型是**生产语义层的类型**，与查询契约同住本模块；声明模块（domain）不持有它们
// ——否则规范侧（sdl.rs 的 &dyn 签名）会被迫 import 声明表，撞 §8.4 的 I5。

/// 相位标记（类型事实，不承载数据）。
pub struct LinePhase;
pub struct PostPhase;
pub struct ZonePhase;

/// 相位可知性标记：某类视图在相位 P 可知。
pub trait Knowable<P> {}

/// 行相位可知的实现标记（建期可用）。
pub struct LineOnly;
/// 结构就绪相位可知的实现标记。
pub struct PostOnly;
/// 窗口闭包相位可知的实现标记。
pub struct ZoneOnly;

impl Knowable<LinePhase> for LineOnly {}
impl Knowable<PostPhase> for PostOnly {}
impl Knowable<ZonePhase> for ZoneOnly {}

/// 行相位查询契约（建期即可回答的原语）：只依赖行值与帧事件日志。
///
/// 与 [`ResolveView`] 的分工：需要已结算结构的原语（块起点/块尾/深度帧/呈现通道分类）
/// 只在 `ResolveView` 上；构建期视图只实现本 trait ⇒「在错误的相位提问」是编译错误，
/// 而不是拿到一个看起来合理的常量。
///
/// 合法用法（行相位原语）：
/// ```
/// use mdio_core::effect::LineResolveView;
/// fn ask(v: &dyn LineResolveView) -> bool { v.table_confirmed_at(0) }
/// ```
/// 错相位提问（行相位视图上没有 Post 原语，必须编译失败）：
/// ```compile_fail
/// use mdio_core::effect::LineResolveView;
/// fn ask(v: &dyn LineResolveView) -> bool { v.block_start_at(0) }
/// ```
pub trait LineResolveView {
    /// 表行是否已被分隔行确认（伪表行回落 Paragraph 的依据）。
    fn table_confirmed_at(&self, line: usize) -> bool;
    /// 当前消费窗口的尾（开区间）。
    fn zone_end(&self) -> usize;
    /// 该行所属块的结束行。
    fn end_for(&self, line: usize) -> Option<usize>;
    /// 该行的容器栈深（关闭深度）。
    fn close_depth_at(&self, line: usize) -> usize;
    /// 容器尾（构建期语义：栈顶记录 start == line 的充要式——行相位即知，
    /// 是「Build ⇒ Post」派生映射的唯一已知例外）。
    fn container_end_for(&self, _line: usize) -> Option<usize> { None }
}

/// 结构就绪相位查询契约（行相位原语 + 依赖已结算结构的原语）。
/// 消费方 = LineView（生产唯一实现）；构建期视图只实现 [`LineResolveView`]。
pub trait ResolveView: LineResolveView {
    fn resolved_type_at(&self, line: usize) -> BlockType;
    fn block_start_at(&self, line: usize) -> bool;
    fn block_end_at(&self, line: usize) -> bool;
    // 结构就绪原语——默认实现 = 基础值（给不产块的实现兜底）
    fn node_type_at_depth(&self, _line: usize, _depth_idx: usize) -> BlockType { BlockType::Paragraph }
    fn frame_kind_at_depth(&self, _line: usize, _depth_idx: usize) -> Option<crate::block::FrameKind> { None }
    /// 行 i 深度 d 的完整帧（含 indent——缩进变化时 depth-only 弹栈
    /// 看不见容器身份——RowView 返回 records frame——FrameSpans 无 indent）。
    fn frame_at_depth(&self, _line: usize, _depth_idx: usize) -> Option<crate::block::BlockFrame> { None }
    /// 行 i 的行前链帧（零重放供给——LineView 走 chain_row 物化池 O(1)；
    /// 无池实现返回空，消费方回退重放路径）。
    fn chain_frames_at(&self, _line: usize) -> smallvec::SmallVec<[crate::block::BlockFrame; 4]> {
        smallvec::SmallVec::new()
    }
    fn is_container_boundary(&self, _line: usize) -> bool { false }
    fn is_container_change(&self, _line: usize) -> bool { false }
    fn is_loose_at(&self, _line: usize) -> bool { false }
    fn content_kind_at(&self, _line: usize, _depth_idx: usize) -> Option<BlockType> { None }
    fn is_block_start(&self, _line: usize) -> bool { false }
    fn is_leaf(&self, _line: usize) -> bool { false }
    fn advance(&self, _t: BlockType, _line: usize) -> Option<usize> { None }
    /// 呈现通道分类器（组合派生列）——无默认实现（trait 对象可调——
    /// where Self: Sized 会排除 dyn）。
    fn classify_ctx(&self, line: usize, is_item_open: bool,
                    item_depth: usize) -> CtxClass;
    /// resolve 的消费端直读——默认实现委托 infer 泛型版（单一实现）。
    fn resolve_fast(&self, line: usize) -> (BlockType, usize) where Self: Sized {
        crate::infer::resolve_from_cursor(self, line)
    }
}

/// TableRow 保持谓词（挂载点——表行声明）：行在**确认**的 Table 帧区间内。
pub(crate) fn table_row_keep(ctx: &BlockRewriteCtx, line: usize) -> bool {
    ctx.view.table_confirmed_at(line)
}

/// RefDef 保持谓词（#200/#198——挂载点）：块只有开行（容器尾 <= line+1）
/// 且冒号后空/未闭合 title → false（撤销 Paragraph）。多行 rd（title
/// 跨行）保持。非 start 行 → 容器尾 None → 保持。
pub(crate) fn a05_keep(ctx: &BlockRewriteCtx, line: usize) -> bool {
    let end = ctx.view.container_end_for(line).unwrap_or(ctx.view.zone_end());
    if end > line + 1 { return true; }
    let line_slice = ctx.rope.line(line);
    let mut chunks = line_slice.chunks();
    let first = chunks.next().unwrap_or("");
    let s: &str = if first.len() == line_slice.byte_len() { first } else { &line_slice.to_string() };
    let line_b = s.as_bytes();
    let close = crate::block::find_label_end(line_b);
    match close {
        Some(c) if c + 1 < line_b.len() && line_b[c + 1] == b':' => {
            let trimmed = crate::block::skip_leading_whitespace(&line_b[c + 2..]);
            if trimmed.is_empty() {
                false
            } else {
                let bb = trimmed;
                let mut uend = 0;
                while uend < bb.len() && bb[uend] != b' ' && bb[uend] != b'\t' { uend += 1; }
                !crate::block::unclosed_title(
                    crate::block::str_view(&bb[uend..]))
            }
        }
        // 非开行形态的 RefDef 行（label 跨行——未识别为 rd——保持）
        _ => true,
    }
}

/// 查表改写:t 声明了 context_fallback 且谓词失败 → fallback;否则 t。
pub fn context_resolve(t: BlockType, ctx: &BlockRewriteCtx, line: usize) -> BlockType {
    let Some(cf) = block_decl(t).context_fallback else { return t };
    if (cf.predicate)(ctx, line) { t } else { cf.fallback }
}

/// 表条目查询（写入点通用 pass 用）。
pub fn context_fallback_for(t: BlockType) -> Option<&'static ContextFallback> {
    block_decl(t).context_fallback.as_ref()
}

/// 逆还原（正逆同规则）:该行 PL（词法——真相）是某条目的类型且解析层已
/// 回退（块/列 = fallback 且回退成立——正谓词失败）→ Some(条目类型)——
/// restore_pl 消费——伪表行块记录回退 Paragraph——还原应还原 PL 词法
/// （TableRow）——rd 撤销行同理。
/// pl 守卫（原生 Paragraph 块非回退行——不还原）。
pub fn context_restore(t: BlockType, pl: BlockType, ctx: &BlockRewriteCtx, line: usize) -> Option<BlockType> {
    let Some(cf) = block_decl(pl).context_fallback else { return None };
    if cf.fallback == t && !(cf.predicate)(ctx, line) {
        Some(pl)
    } else {
        None
    }
}

const BLT_DECL_DEFAULT: BlockTypeDecl = BlockTypeDecl {
    merges: false, blank_policy: BlankPolicy::Break,
    interrupt_rule: InterruptRule::No, closes_paragraph: false,
    is_list_item: false, in_non_shell: false, context_fallback: None,
    header_level: None,
};

pub static BLOCK_TYPE_DECLS: [BlockTypeDecl; 27] = {
    let mut arr = [BLT_DECL_DEFAULT; 27];
    arr[BlockType::Paragraph as usize]    = BlockTypeDecl { merges: true, ..BLT_DECL_DEFAULT };
    arr[BlockType::HsHead1 as usize]      = BlockTypeDecl { merges: true, closes_paragraph: true, header_level: Some(1), ..BLT_DECL_DEFAULT };
    arr[BlockType::HsHead2 as usize]      = BlockTypeDecl { merges: true, closes_paragraph: true, header_level: Some(2), ..BLT_DECL_DEFAULT };
    arr[BlockType::IndentedCode as usize] = BlockTypeDecl { merges: true, blank_policy: BlankPolicy::Allow, closes_paragraph: true, ..BLT_DECL_DEFAULT };
    // 列⑤ 表条目（3 类型等价；扩集 7 成员）：
    arr[BlockType::CodeContent as usize]    = BlockTypeDecl { closes_paragraph: true, in_non_shell: true, ..BLT_DECL_DEFAULT };
    arr[BlockType::CodeFenceClose as usize] = BlockTypeDecl { closes_paragraph: true, ..BLT_DECL_DEFAULT };
    // 打断单源化——几何 ≤3 与修饰由消费点统一应用：
    //   Yes: Header1-6（ATX 可中断段落——CM §4.1）、ThematicBreak、CodeFenceOpen、
    //        Blockquote（CM §4.1）、TableRow（GFM——顶层实测打断——uniformity——
    //        lazy 侧对称）
    //   List: Bullet/Checklist/OrderedListItem（number-1 数值——CM §5.3）
    //   Html: HtmlBlock（type 7 例外——CM §4.6）
    arr[BlockType::Bullet as usize]  = BlockTypeDecl { interrupt_rule: InterruptRule::List, is_list_item: true, ..BLT_DECL_DEFAULT };
    arr[BlockType::Checklist as usize] = BlockTypeDecl { interrupt_rule: InterruptRule::List, is_list_item: true, ..BLT_DECL_DEFAULT };
    arr[BlockType::OrderedListItem as usize] = BlockTypeDecl { interrupt_rule: InterruptRule::List, is_list_item: true, ..BLT_DECL_DEFAULT };
    arr[BlockType::Header1 as usize] = BlockTypeDecl { interrupt_rule: InterruptRule::Yes, closes_paragraph: true, header_level: Some(1), ..BLT_DECL_DEFAULT };
    arr[BlockType::Header2 as usize] = BlockTypeDecl { interrupt_rule: InterruptRule::Yes, closes_paragraph: true, header_level: Some(2), ..BLT_DECL_DEFAULT };
    arr[BlockType::Header3 as usize] = BlockTypeDecl { interrupt_rule: InterruptRule::Yes, closes_paragraph: true, header_level: Some(3), ..BLT_DECL_DEFAULT };
    arr[BlockType::Header4 as usize] = BlockTypeDecl { interrupt_rule: InterruptRule::Yes, closes_paragraph: true, header_level: Some(4), ..BLT_DECL_DEFAULT };
    arr[BlockType::Header5 as usize] = BlockTypeDecl { interrupt_rule: InterruptRule::Yes, closes_paragraph: true, header_level: Some(5), ..BLT_DECL_DEFAULT };
    arr[BlockType::Header6 as usize] = BlockTypeDecl { interrupt_rule: InterruptRule::Yes, closes_paragraph: true, header_level: Some(6), ..BLT_DECL_DEFAULT };
    arr[BlockType::ThematicBreak as usize]   = BlockTypeDecl { interrupt_rule: InterruptRule::Yes, closes_paragraph: true, ..BLT_DECL_DEFAULT };
    // #238（符合性裁决——cmark 佐证 `> ```\nfoo` → foo 顶层）：CF 开行后
    // 无 lazy（CM §5.1 lazy 只覆盖段落——fence 内容行被约束链拒后弹栈——
    // prev=CodeFenceOpen——BQ lazy 判定需要 closes——逐层豁免暴露原
    // "合理缺"（sdl_prev_closes_diff 注释）为真 bug——表列补 true——
    // prev_closes_spec 已含 CodeFenceOpen——SDL 自动一致）
    arr[BlockType::CodeFenceOpen as usize] = BlockTypeDecl { interrupt_rule: InterruptRule::Yes, closes_paragraph: true, in_non_shell: true, ..BLT_DECL_DEFAULT };
    arr[BlockType::Blockquote as usize]    = BlockTypeDecl { interrupt_rule: InterruptRule::Yes, ..BLT_DECL_DEFAULT };
    arr[BlockType::TableRow as usize]      = BlockTypeDecl {
        interrupt_rule: InterruptRule::Yes, closes_paragraph: true,
        in_non_shell: true,
        // 伪表改写:链上无确认 Table 帧（分隔行确认失败/未发生——
        // `|a| = b` 数学文本——GFM 官方段落）→ Paragraph。
        // 谓词 = 行在确认 Table 帧区间（table_confirmed_at——spans 列预扫 /
        // BlockView 位图——**含表头行**——帧开行的确认在后续行——顺序
        // 栈查对表头行 false——必须全局预扫——table_confirmed_flags）。
        context_fallback: Some(ContextFallback {
            fallback: BlockType::Paragraph,
            predicate: table_row_keep,
        }),
        ..BLT_DECL_DEFAULT
    };
    arr[BlockType::HtmlBlock as usize]     = BlockTypeDecl { interrupt_rule: InterruptRule::Html, closes_paragraph: true, in_non_shell: true, ..BLT_DECL_DEFAULT };
    arr[BlockType::Comment as usize]       = BlockTypeDecl { closes_paragraph: true, in_non_shell: true, ..BLT_DECL_DEFAULT };
    // 列⑤ 扩集 7 成员（列表词法盲区：RefDef/表格/Comment 等块内容后无
    // lazy continuation——prev 词法唯一机制——与约束链同批）：
    arr[BlockType::HtmlContent as usize]    = BlockTypeDecl { closes_paragraph: true, in_non_shell: true, ..BLT_DECL_DEFAULT };
    arr[BlockType::HtmlElement as usize]    = BlockTypeDecl { closes_paragraph: true, in_non_shell: true, ..BLT_DECL_DEFAULT };
    // rd 撤销（#200/#198）:块只有开行（容器尾 <= line+1）且冒号后空/未闭合
    // title → 撤销 Paragraph（`[foo]:\n\nbar`——官方 p——refmap 无条目）。
    // 多行 rd（title 跨行——#197）保持。原实现内联于 compute_derived_spans
    // 与 build_block_records——迁入表。
    arr[BlockType::RefDef as usize]         = BlockTypeDecl {
        closes_paragraph: true,
        in_non_shell: true,
        context_fallback: Some(ContextFallback {
            fallback: BlockType::Paragraph,
            predicate: a05_keep,
        }),
        ..BLT_DECL_DEFAULT
    };
    arr[BlockType::TableSeparator as usize] = BlockTypeDecl { closes_paragraph: true, in_non_shell: true, ..BLT_DECL_DEFAULT };
    arr
};

pub fn block_decl(t: BlockType) -> &'static BlockTypeDecl {
    &BLOCK_TYPE_DECLS[t as usize]
}

/// FrameKind → 语义 BlockType 的唯一映射（container_block_type /
/// content_type_of 的单一来源）。
#[inline]
pub fn container_block_type(kind: FrameKind) -> BlockType {
    effect_for(kind).block_type
}

/// 列表容器判定：由 ContainerEffect.block_type 派生 BlockTypeDecl.is_list_item。
/// 不新增独立 is_list 列，避免两个“列表”概念。
#[inline]
pub fn is_list_container(kind: FrameKind) -> bool {
    block_decl(effect_for(kind).block_type).is_list_item
}

/// 嵌套容器判定：ContainerDecl.is_nesting 的薄封装。
#[inline]
pub fn is_nesting_container(kind: FrameKind) -> bool {
    decl_for(kind).is_nesting
}

/// 非 shell 内容/容器块：编辑时需要完整容器进 zone。
#[inline]
pub fn needs_full_shell(t: BlockType) -> bool {
    block_decl(t).in_non_shell
}

/// BLOCK_TYPE_EFFECTS 访问器（列的读口）。
#[inline]
pub fn block_type_effects(t: BlockType) -> &'static BlockTypeEffect {
    &BLOCK_TYPE_EFFECTS[t as usize]
}

/// 推导列：can_interrupt(t, m) = interrupt_rule(t) × is_structural(m)
/// （+ List/Html 词法修饰）。打断判定单源；block::interrupts_para 仅委托。
pub(crate) fn can_interrupt(t: BlockType, content: &[u8], indent_col: usize) -> bool {
    let rule = block_decl(t).interrupt_rule;
    if matches!(rule, InterruptRule::No) { return false; }
    if !geo::is_structural(indent_col) { return false; }
    match rule {
        InterruptRule::List => {
            if crate::block::is_empty_list_item(content) { return false; }
            t != BlockType::OrderedListItem || crate::block::number_is_one(content)
        }
        InterruptRule::Html => crate::block::detect_html_block_type(content) != 7,
        _ => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailingPolicy { Include, Backtrack }

#[derive(Debug, Clone)]
pub enum Correction {
    Setext { underline_eq: BlockType, underline_dash: BlockType },
}

#[derive(Debug, Clone)]
pub struct BlockTypeEffect {
    pub trailing: TrailingPolicy,
    pub corrections: &'static [Correction],
    pub content_mode: ContentMode,
    /// 事实列：BQ 空行带前缀行的归属——prev==IC → 同一缩进码块；
    /// 其余行基础默认=Blockquote。
    pub blank_prefix_type: BlockType,
    /// 规则名列（C′ 收编）：段落修饰——OL 行=OlStartMustBe1、HtmlBlock 行=HtmlNot7、
    /// 其余 None。
    pub para_modifier: ParaModifier,
}

/// BQ 续行 IC 惰性调整（公式列：BqIcLazyRule 由 CONTAINER_DECLS[BlockQuote] 行持有）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BqIcLazyRule {
    /// 原值不动。
    Keep,
    /// prev 不闭段落 ∧ ct==IC → lazy paragraph；否则原值。
    ParaIfNoClose,
}

/// 段落修饰（C′ 收编——规则名列：apply_para_modifiers 读本列选识别器组合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParaModifier {
    /// 无修饰。
    None,
    /// OL：number≠1 → Paragraph（CM §5.3）。
    OlStartMustBe1,
    /// HtmlBlock：type==7 → Paragraph（CM §4.6）。
    HtmlNot7,
}

/// 渲染子结构的基础形态（序列化结构层声明——"scan 逆"的对称——
/// **不建独立表**（effect 表无方向——现存表反向推导即可——单源）——
/// 由三张现存表推导：
///   is_list_item（BLOCK_TYPE_DECLS）→ Item
///   frame_kind（容器类型）→ Container
///   inline_mode（block_proj_spec——TagOnly/ContentOnly/TagThenContent）
///   → Inline（RawText/None 排除——不产 inline 子）
///   其余 → None_
/// 消费：SDL 渲染分派审计（sdl_children_shape_audit）对照锚——防渲染漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildrenShape {
    /// 无子——输出不含 ch（ch:[] 移除语义）
    None_,
    /// inline 子（叶子——tag/content inline 输入——ch 由内容决定）
    Inline,
    /// item 子结构（loose wrapper p + inline + 块子）
    Item,
    /// 容器子（树子节点——ch 由树结构决定）
    Container,
}

/// 反向推导（非表——现存表查询——单源）。
pub fn children_shape(t: BlockType) -> ChildrenShape {
    use crate::block::InlineFeed;
    if block_decl(t).is_list_item {
        ChildrenShape::Item
    } else if crate::block::frame_kind(t).is_some() {
        ChildrenShape::Container
    } else {
        match crate::block::block_proj_spec(t).inline_mode {
            InlineFeed::TagOnly | InlineFeed::ContentOnly | InlineFeed::TagThenContent => {
                ChildrenShape::Inline
            }
            _ => ChildrenShape::None_,
        }
    }
}

/// 区域角色——拓扑维单一事实（节点 span 的空间来源——推导函数零新表：
/// children_shape 同款——消费: selector block_end_resolve / 树
/// resolve_from_cursor——header 与 item 同律：都是区域（Section/Cell），
/// 差异只在投影维的 LINE_SPECS 表面编码）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionRole {
    /// 章节区（header——章节壳；span = 章节区间 ∩ 外层容器边界）
    Section,
    /// 容器区（BQ/BLT/OL/CF/Table… 帧；span = 帧边界）
    Container,
    /// 单元格（Bullet/Checklist/OrderedListItem；span = marker 行 + 自身内容
    /// + 嵌套子树——item_subtree_end 深度跟踪）
    Cell,
    /// 叶子块（段落/代码等；span = 自身块）
    Leaf,
}

/// 反向推导（非表——现存表查询——单源）。
pub fn region_role(t: BlockType) -> RegionRole {
    if crate::block::is_header(t) {
        RegionRole::Section
    } else if block_decl(t).is_list_item {
        RegionRole::Cell
    } else if crate::block::frame_kind(t).is_some() {
        RegionRole::Container
    } else {
        RegionRole::Leaf
    }
}

/// item 内容剥层声明（读/写/parse/trilogy/sdl 共同消费——正逆同规则内容
/// 层单源）。剥层 = 行级 Push 帧数 - 已剥外层数（item 自身帧除外——复合
/// 开行/嵌套=多 Push——BQ 帧计入但 peel_markers 的 marker 语义天然对齐——
/// break 兜底）。outer_stripped = 外层 marker 是否已被 strip_list_marker/
/// 几何起点剥除——复合开行行类型取最内层（`1. - z` 行
/// 类型 Bullet——内容首层是 OL marker——strip_list_marker(Bullet) 失败——
/// outer_stripped=false——剥全层——与树嵌套一致）。动态量（帧结构）——
/// 推导函数（children_shape 先例）——非静态表（`- [x] - y` 与 `- - foo`
/// 同类型 Bullet——inner 0 vs 1——帧差异类型表表达不了）。
pub fn item_inner_layers(line_pushes: usize, outer_stripped: bool) -> usize {
    line_pushes.saturating_sub(1) + usize::from(!outer_stripped)
}

const BLT_EFFECT_DEFAULT: BlockTypeEffect = BlockTypeEffect {
    trailing: TrailingPolicy::Include, corrections: &[],
    content_mode: ContentMode::Stripped,
    blank_prefix_type: BlockType::Blockquote,
    para_modifier: ParaModifier::None,
};

pub static BLOCK_TYPE_EFFECTS: [BlockTypeEffect; 27] = {
    use BlockType::*;
    let d = BLT_EFFECT_DEFAULT;
    let mut arr: [BlockTypeEffect; 27] = [
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 0 H1
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 1 H2
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 2 H3
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 3 H4
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 4 H5
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 5 H6
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 6 Bullet
        BlockTypeEffect { content_mode: ContentMode::Code,     ..d }, // 7 CF Open
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 8 BQ
        BlockTypeEffect { content_mode: ContentMode::Stripped, para_modifier: ParaModifier::OlStartMustBe1, ..d }, // 9 OL Item
        BlockTypeEffect { corrections: &[Correction::Setext { underline_eq: HsHead1, underline_dash: HsHead2 }], ..d }, // 10 Paragraph
        // 回归修正：旧硬编码 AngleBracket 分支返回注释内容——恢复 Stripped
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 11 Comment
        BlockTypeEffect { content_mode: ContentMode::Table,   ..d }, // 12 TableRow
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 13 Checklist
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 14 HsHead1
        BlockTypeEffect { content_mode: ContentMode::Stripped, ..d }, // 15 HsHead2
        BlockTypeEffect { content_mode: ContentMode::None_,   ..d }, // 16 HsSep
        BlockTypeEffect { content_mode: ContentMode::Raw, para_modifier: ParaModifier::HtmlNot7, ..d }, // 17 HtmlBlock——Raw(签名唯一性——SDL 防漂移锁定);容器前缀剥层在 content_raw 的 hb 特化
        BlockTypeEffect { content_mode: ContentMode::None_,   ..d }, // 18 ThematicBreak
        BlockTypeEffect { content_mode: ContentMode::None_,   ..d }, // 19 CF Close
        BlockTypeEffect { content_mode: ContentMode::Code,    ..d }, // 20 CodeContent
        BlockTypeEffect { content_mode: ContentMode::Raw,     ..d }, // 21 HtmlContent
        BlockTypeEffect { trailing: TrailingPolicy::Backtrack, content_mode: ContentMode::Code, blank_prefix_type: BlockType::IndentedCode, ..d }, // 22 IC
        BlockTypeEffect { content_mode: ContentMode::None_,    ..d }, // 23 TableSeparator
        BlockTypeEffect { content_mode: ContentMode::None_,    ..d }, // 24 Blank
        BlockTypeEffect { content_mode: ContentMode::None_,    ..d }, // 25 RefDef
        BlockTypeEffect { content_mode: ContentMode::Stripped,  ..d }, // 26 HtmlElement
    ];
    arr
};


// LineSpec — 行层投影声明（27 行——BlockType→行几何规则）

// LineSpec 替代 line_geometry / extract_mark / extract_tag / format_payload /
// node_tag / node_mark / content_write_range / tag_write_range 中的 per-type
// 硬编码分支——把行几何知识从代码搬到数据。解析器（parse_bl/parse_ol/...）
// 保留——LineSpec 只替代"哪个类型调用哪个解析器"的路由。

/// 标记宽度规则——marker 在行首占用的宽度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkWidth {
    /// 标题——lv 个 #（Header1-6；HsHead1-2 是 setext——标记 = 下一行下划线）
    LvHashes(u8),
    /// 固定 1 字符 + 可选 1 空白（Bullet / Checklist）
    Char1PlusOptWs,
    /// 数字 + 分隔符 + 1 空白（OrderedListItem）
    DigitsDelimPlusWs,
    /// 固定 3——``` / ~~~（CodeFenceOpen / CodeFenceClose）
    Fence3,
    /// 开标签到第一个 >——含标签名（HtmlElement / HtmlBlock）
    OpenTag,
    /// 固定文本——直接返回——不调解析器
    Fixed(&'static str),
    /// setext 标题（HsHead1/2）——标记 = 下一行的下划线行（====== / ------）——
    /// 标题占两行（标题行 + 下划线行）——title_end/节体起点统一由本列推导
    Setext,
    /// 无标记
    None,
}

/// 闭标记——从 MarkShape + mark_width 推导——不需查表。
/// Delimit → close = open（|→|）；WrapAsymmetric → "-->"（Comment 唯一）；
/// Prefix → None。
#[inline]
pub fn close_mark(t: crate::block::BlockType) -> Option<&'static str> {
    match LINE_SPECS[t as usize].mark_shape {
        MarkShape::Delimit => match LINE_SPECS[t as usize].mark_width {
            MarkWidth::Fixed(s) => Some(s),
            _ => None,
        },
        MarkShape::WrapAsymmetric => Some("-->"),
        MarkShape::Prefix => None,
    }
}

/// 写面 ATX 标题 mark 单源（scaffold 与 heading shift 共用——
/// LINE_SPECS::LvHashes 推导，不再手写 "#".repeat）。
pub fn atx_mark_text(level: u8) -> String {
    let level = level.clamp(1, 6) as usize;
    match LINE_SPECS[level - 1].mark_width {
        MarkWidth::LvHashes(n) => "#".repeat(n as usize),
        _ => "#".to_string(),
    }
}

/// 写面 scaffold 的 canonical bullet marker（读面 parse_bl 接受 `-`/`+`/`*`）。
pub const SCAFFOLD_BULLET_MARK: &str = "- ";

/// tag 相对 mark 的位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagPosition {
    /// tag 在 mark 之后（Bullet key / Heading text / Fence lang / Html tagname）
    AfterMark,
    /// tag 是 mark 的前缀子区间（OL 编号——"1."⊃"1"——tag_range=0..digits⊂mark）
    PrefixOfMark,
}

/// tag 提取规则——tag 文本如何从行中取得。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagRule {
    /// `- key: val` → tag="key"——colon 分隔
    ColonKey,
    /// `- [x] done: yes` → tag="done"——checkbox 后的 colon 键
    CheckboxColonKey,
    /// `# Title` → tag="Title"——标题文本
    HeadingText,
    /// `1. item` → tag="1"——列表编号
    Digits,
    /// ```rust → tag="rust"——围栏语言标识
    Lang,
    /// `<div>` → tag="div"——开标签名
    Tagname,
    /// 无 tag
    None,
}

/// content 起点规则——content 文本从行的哪个位置开始。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentRule {
    /// 前缀式——separator 之后（`- key: val` 的 "val"）
    AfterTagSep,
    /// mark 之后（`> q` 的 "q"——无 tag/sep）
    AfterMark,
    /// 文本区间——fence/comment 体——含内容行尾换行
    TextSpan,
    /// 节点区间——标题章节体——paragraph 收束
    NodeSpan,
}

/// mark 的空间形态——mark 与 content 的位置关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkShape {
    /// mark 后接 content（bullet/head/bq/ol/fence）
    Prefix,
    /// 开闭包裹——开≠闭（comment `<!-- content -->`——开标记和闭标记不同）
    WrapAsymmetric,
    /// 定界（table `| cells |`——mark 是定界符）
    Delimit,
}

/// 行层投影声明——一个 BlockType 的全部行几何规则。
/// 按 BlockType 序数索引（0..26）。
pub struct LineSpec {
    pub mark_width: MarkWidth,
    pub tag_position: TagPosition,
    pub tag_rule: TagRule,
    pub content_rule: ContentRule,
    pub mark_shape: MarkShape,
}

/// 27 行静态表——BlockType 序数 0..26（与 BLOCK_TYPE_META / BLOCK_TYPE_DECLS /
/// BLOCK_TYPE_EFFECTS / BLOCK_ATTRS 对齐）。
pub static LINE_SPECS: [LineSpec; 27] = [
    // 0-5: Header1-6
    LineSpec { mark_width: MarkWidth::LvHashes(1), tag_position: TagPosition::AfterMark, tag_rule: TagRule::HeadingText, content_rule: ContentRule::NodeSpan, mark_shape: MarkShape::Prefix },
    LineSpec { mark_width: MarkWidth::LvHashes(2), tag_position: TagPosition::AfterMark, tag_rule: TagRule::HeadingText, content_rule: ContentRule::NodeSpan, mark_shape: MarkShape::Prefix },
    LineSpec { mark_width: MarkWidth::LvHashes(3), tag_position: TagPosition::AfterMark, tag_rule: TagRule::HeadingText, content_rule: ContentRule::NodeSpan, mark_shape: MarkShape::Prefix },
    LineSpec { mark_width: MarkWidth::LvHashes(4), tag_position: TagPosition::AfterMark, tag_rule: TagRule::HeadingText, content_rule: ContentRule::NodeSpan, mark_shape: MarkShape::Prefix },
    LineSpec { mark_width: MarkWidth::LvHashes(5), tag_position: TagPosition::AfterMark, tag_rule: TagRule::HeadingText, content_rule: ContentRule::NodeSpan, mark_shape: MarkShape::Prefix },
    LineSpec { mark_width: MarkWidth::LvHashes(6), tag_position: TagPosition::AfterMark, tag_rule: TagRule::HeadingText, content_rule: ContentRule::NodeSpan, mark_shape: MarkShape::Prefix },
    // 6: Bullet
    LineSpec { mark_width: MarkWidth::Char1PlusOptWs, tag_position: TagPosition::AfterMark, tag_rule: TagRule::ColonKey, content_rule: ContentRule::AfterTagSep, mark_shape: MarkShape::Prefix },
    // 7: CodeFenceOpen
    LineSpec { mark_width: MarkWidth::Fence3, tag_position: TagPosition::AfterMark, tag_rule: TagRule::Lang, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 8: Blockquote
    LineSpec { mark_width: MarkWidth::Fixed(">"), tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::AfterMark, mark_shape: MarkShape::Prefix },
    // 9: OrderedListItem
    LineSpec { mark_width: MarkWidth::DigitsDelimPlusWs, tag_position: TagPosition::PrefixOfMark, tag_rule: TagRule::Digits, content_rule: ContentRule::AfterMark, mark_shape: MarkShape::Prefix },
    // 10: Paragraph
    LineSpec { mark_width: MarkWidth::None, tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 11: Comment
    LineSpec { mark_width: MarkWidth::Fixed("<!--"), tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::WrapAsymmetric },
    // 12: TableRow
    LineSpec { mark_width: MarkWidth::Fixed("|"), tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Delimit },
    // 13: Checklist
    LineSpec { mark_width: MarkWidth::Char1PlusOptWs, tag_position: TagPosition::AfterMark, tag_rule: TagRule::CheckboxColonKey, content_rule: ContentRule::AfterTagSep, mark_shape: MarkShape::Prefix },
    // 14: HsHead1——setext——标记 = 下一行下划线（MarkWidth::Setext——title 行数 2）
    LineSpec { mark_width: MarkWidth::Setext, tag_position: TagPosition::AfterMark, tag_rule: TagRule::HeadingText, content_rule: ContentRule::NodeSpan, mark_shape: MarkShape::Prefix },
    // 15: HsHead2——setext——标记 = 下一行下划线（MarkWidth::Setext——title 行数 2）
    LineSpec { mark_width: MarkWidth::Setext, tag_position: TagPosition::AfterMark, tag_rule: TagRule::HeadingText, content_rule: ContentRule::NodeSpan, mark_shape: MarkShape::Prefix },
    // 16: HsSep
    LineSpec { mark_width: MarkWidth::None, tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 17: HtmlBlock
    LineSpec { mark_width: MarkWidth::OpenTag, tag_position: TagPosition::AfterMark, tag_rule: TagRule::Tagname, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 18: ThematicBreak
    LineSpec { mark_width: MarkWidth::None, tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 19: CodeFenceClose
    LineSpec { mark_width: MarkWidth::Fence3, tag_position: TagPosition::AfterMark, tag_rule: TagRule::Lang, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 20: CodeContent
    LineSpec { mark_width: MarkWidth::None, tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 21: HtmlContent
    LineSpec { mark_width: MarkWidth::None, tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 22: IndentedCode
    LineSpec { mark_width: MarkWidth::Fixed("    "), tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 23: TableSeparator
    LineSpec { mark_width: MarkWidth::Fixed("|"), tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Delimit },
    // 24: Blank
    LineSpec { mark_width: MarkWidth::None, tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 25: RefDef
    LineSpec { mark_width: MarkWidth::None, tag_position: TagPosition::AfterMark, tag_rule: TagRule::None, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
    // 26: HtmlElement
    LineSpec { mark_width: MarkWidth::OpenTag, tag_position: TagPosition::AfterMark, tag_rule: TagRule::Tagname, content_rule: ContentRule::TextSpan, mark_shape: MarkShape::Prefix },
];

#[inline]
pub fn line_spec(t: crate::block::BlockType) -> &'static LineSpec {
    &LINE_SPECS[t as usize]
}

/// 表派生布尔（S1——ColonKey|CheckboxColonKey|Digits 三连枚举 ×8 的归拢）：
/// key 型 tag（tag 定位/内容跳过的同族判定——content_write_range/compute_write
/// /node_tag 等消费）。重复组合枚举也是散落——表推导的进一步形态。
#[inline]
pub fn is_key_like(tr: TagRule) -> bool {
    matches!(tr, TagRule::ColonKey | TagRule::CheckboxColonKey | TagRule::Digits)
}


// geo 常量区 — 行间缩进几何的条文登记簿（v3 无新表方案——值单一源）

// 原则：几何域阈值数字只存在于本区；判定 = 常量 × 已存表 × 识别器的组合
// 推导；测量 = block.rs 词法层唯一生产。
pub mod geo {
    /// §4.1/§4.2/§4.5/§5.1/§5.2：结构缩进上限（相对容器内容原点）。
    pub const STRUCT_INDENT_MAX: usize = 3;
    /// §5.2 规则 1：列表内容窗口上限（marker 后 N∈[1,4] 在 item 内）。
    pub const LIST_CONTENT_MAX: usize = 4;
    /// §5.2 规则 2 例外：marker 后 ≥ LIST_CONTENT_MAX+1 列 = first-block IC。
    pub const LIST_IC_OFFSET: usize = LIST_CONTENT_MAX + 1;
    /// §2.2：tab stop（值单一源——词法层 tab 展开的唯一常量）。
    pub const TAB_STOP: usize = 4;
    /// 实现约定：真空 item 测量哨兵（非条文）。
    pub const EMPTY_ITEM_SENTINEL: usize = 63;

    /// 推导列：结构缩进（§4.1 结构形态——indent ≤ 3）。
    #[inline]
    pub fn is_structural(indent_col: usize) -> bool {
        indent_col <= STRUCT_INDENT_MAX
    }

    /// 推导列：缩进代码（§4.4——indent ≥ 4；与 is_structural 互补）。
    #[inline]
    pub fn is_indented_code(indent_col: usize) -> bool {
        indent_col > STRUCT_INDENT_MAX
    }
}


// ContainerDecl — 广义效应表的容器 scan 判定列

//
// 分层: 决策 (admit / body) 是数据, 在此声明;
//       算法 + 运行时 classify 是函数, 留在 block.rs (fn 指针引用)。
//       classify 不可避免是函数 (函子性边界: closing fence / 内容分类依赖解析)。
//       判定单级化：classify 直产 (BlockType, ScanAction)；ClassifyFn 并入 OutcomeFn。

/// 运行时函数签名 (block.rs 实现)。
/// classify/outcome 接收统一 `ScanRuntime`（stack + open_lines + rope），
/// 容器需要帧外运行状态时不再改函数类型。
pub type AdmitFn = for<'a> fn(&LineFacts<'a>, &BlockFrame, &[BlockFrame]) -> bool;
pub type OutcomeFn = for<'a> fn(&LineFacts<'a>, &BlockFrame, &crate::block::ScanRuntime<'a>) -> Option<(BlockType, ScanAction)>;

/// admit 表达式。Always 或运行时 fn。
#[derive(Debug, Clone, Copy)]
pub enum AdmitExpr {
    Always,
    Fn(AdmitFn),
}

/// 列①：容器内容行的约束——"行属于本容器内容"必须满足的条件。
/// 约束链：evaluate_decl 遍历栈下层容器，逐层检查（行必须满足所有外层约束）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentConstraint {
    None,
    /// 行须带 > 前缀（原始行 has_gt_prefix——BQ 的 lazy 例外由豁免因子处理）
    RequireBqPrefix,
    /// 行缩进 ≥ 最近列表帧 threshold（effective_cmp 坐标系）
    RequireListIndent,
}

/// 列②：约束链豁免因子（OR 优先级——`content_constraint_ok ∨ lazy_exempt`）。
/// 豁免只决定"不被约束链弹走"——最终归属由 classify 兜底（豁免过宽无害——
/// 如 prev=Blank 时 bq_peel 仍拒绝）。
pub type LazyExemptFn = for<'a> fn(&LineFacts<'a>, &BlockFrame, &[BlockFrame]) -> bool;

/// body 位的 adm/out 分区 (FC1 基础)。mask 落在 body(bits5-0) 上。
#[derive(Debug, Clone, Copy)]
pub struct BodyLayout {
    pub adm_mask: u16,
    pub out_mask: u16,
}

/// 容器声明 — 一个 FrameKind 的 scan 判定列。
#[derive(Debug, Clone, Copy)]
pub struct ContainerDecl {
    pub kind: FrameKind,
    pub body: BodyLayout,
    pub is_nesting: bool,
    pub admit: AdmitExpr,
    /// 判定单级化：classify 直产 (BlockType, ScanAction)；ClassifyFn 并入 OutcomeFn。
    pub classify: OutcomeFn,
    /// Whether this container produces PopPush events.
    pub has_pop_push: bool,
    /// Whether blank lines inside this container trigger LOOSE detection.
    pub has_loose: bool,
    /// [s] 栈谓词：容器内容行的约束（约束链——栈下层遍历——evaluate_decl
    /// 检查）。RequireListIndent 只查最近列表帧（effective_cmp 坐标系——
    /// 嵌套最内层约束最严——外层自动满足）。
    pub content_constraint: ContentConstraint,
    /// [s]+[L] 栈+行谓词：约束链豁免因子（顶帧——段落延续 > 容器约束——
    /// OR 优先级）。
    pub lazy_exempt: Option<LazyExemptFn>,
    /// 公式列：BQ 续行 IC 惰性调整——BlockQuote 行=ParaIfNoClose、
    /// 其余 Keep；消费点 bq_ic_lazy_adjust 读本列现算。
    pub bq_ic_lazy: BqIcLazyRule,
}

/// admit 求值。
pub fn evaluate_admit(admit: &AdmitExpr, facts: &LineFacts, frame: &BlockFrame, stack: &[BlockFrame]) -> bool {
    match admit {
        AdmitExpr::Always => true,
        AdmitExpr::Fn(f) => f(facts, frame, stack),
    }
}

/// 声明求值（判定单级化）: admit → 约束链 → classify 直产结论。
pub fn evaluate_decl(decl: &ContainerDecl, facts: &LineFacts, frame: &BlockFrame, rt: &crate::block::ScanRuntime) -> Option<(BlockType, ScanAction)> {
    let stack = rt.stack;
    {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("sc_decl_admit");
        if !evaluate_admit(&decl.admit, facts, frame, stack) { return None; }
    }
    // 列①+②：约束链——遍历栈下层容器，逐层检查内容约束。
    // 任一失败 → 查豁免因子（lazy_exempt——段落延续 > 容器约束——OR）——
    // 仍失败 → None（约束 Pop——行不属于本容器内容——Pop 循环重扫）。
    // #294（CM §5.2——cmark 佐证 `> 1. > Blockquote\n> continued here.` →
    // continued 归 BQ2 段落）：约束失败的层可能不是栈顶——行 1 剥 BQ1 后
    // `continued here.` 相对 OL 缩进不足（OL 约束失败）——但行归 BQ2（栈顶
    // > 匹配）——豁免逐层查——OL 的 list_lazy_candidate（Outside 非 marker）
    // 豁免——原只查栈顶（BQ2 的 bq_lazy_candidate 需无 >——行有 >——误拒）。
    {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("sc_decl_cc");
    if !constraint_chain_ok(facts, stack) {
        let exempt = decl.lazy_exempt.map_or(false, |f| f(facts, frame, stack))
            || stack[..stack.len().saturating_sub(1)].iter().any(|g| {
                CONTAINER_DECLS[g.kind() as usize].lazy_exempt
                    .map_or(false, |f| f(facts, g, stack))
            });
        if !exempt {
            return None;
        }
    }
    }
    #[cfg(feature = "telemetry")]
    let _g = crate::telemetry::deep_span("sc_decl_cls");
    (decl.classify)(facts, frame, rt)
}

/// 列①：约束链——行必须满足所有栈下层容器的内容约束。
/// RequireListIndent 的 effective_cmp 用 peel 坐标（helper 一次 peel 共享——
/// 嵌套列表最内层约束最严——外层 threshold 更松——全查不改变结果）。
/// 短路：任一失败即拒（豁免由 evaluate_decl 的 lazy_exempt 决定——在链外）。
/// ⑥ 复核（官方例 238 + cmark 反证）：原 CF 内容豁免（CM §4.5 "fence 内容
/// 不要求前缀"）方向错——官方例 238（`> ```` `\nfoo\n````）输出 foo 顶层 p
/// （不在 fence 内——BQ 也结束）——BQ 内 fence 的无前缀行应被约束链拒
/// （弹 CF——弹 BQ——顶层）——豁免让无前缀行进 fence——偏离官方——已回滚
/// （本函数恢复原语义）。
/// pub(crate)（sdl.rs 约束链比对对象）。
// ── 约束链行表（逐帧 约束类×gt×bq_carries×eff_lt → Next/Reject；R7 默认 Next）──

/// 约束判定格（第一方覆盖）：下标＝ContentConstraint 判别式（None=0/BqPrefix=1/ListIndent=2）
/// ——3 格全填、无兜底；格内＝识别器组合公式（gt/bq_carries/eff_lt），peel_once 共享在格内。
#[derive(Clone, Copy)]
enum CcRule { Next, RejectIfNoBqPrefix, RejectIfListFail }

static CC_TABLE: [CcRule; 3] = [
    CcRule::Next,               // 0 None
    CcRule::RejectIfNoBqPrefix, // 1 RequireBqPrefix
    CcRule::RejectIfListFail,   // 2 RequireListIndent
];

pub(crate) fn constraint_chain_ok(facts: &LineFacts, stack: &[BlockFrame]) -> bool {
    let n = stack.len();
    if n <= 1 { return true; }
    let mut peel_once: Option<usize> = None;
    for (g_idx, g) in stack[..n - 1].iter().enumerate() {
        let decl = &CONTAINER_DECLS[g.kind() as usize];
        match CC_TABLE[decl.content_constraint as usize] {
            CcRule::Next => {}
            CcRule::RejectIfNoBqPrefix => {
                if !crate::block::has_gt_prefix(facts.content) { return false; }
            }
            CcRule::RejectIfListFail => {
                if !crate::block::bq_carries_list_constraint(g_idx, stack, facts) {
                    let eff = peel_once.get_or_insert_with(|| {
                        let (peeled, peel_columns) = crate::block::peel_lower_prefixes(facts.content, stack);
                        crate::block::effective_cmp_after_peel(facts, peeled, peel_columns)
                    });
                    let threshold = crate::block::list_effective_threshold(g, facts.prev_type());
                    if *eff < threshold { return false; }
                }
            }
        }
    }
    true
}

/// FC1 等价谓词 (ripple / 扫描互模拟): kind + indent + body_adm 相同。
///
/// 比当前 ripple 的"栈全等"更精确: 只比较 adm 位, 忽略 out 位 (如 LOOSE_BIT),
/// 因此只在 body_out 变化的场景能更早收敛 (预期改进, 非回归)。
pub fn fc1_equiv(a: BlockFrame, b: BlockFrame) -> bool {
    if a.kind() != b.kind() || a.indent() != b.indent() { return false; }
    let decl = &CONTAINER_DECLS[a.kind() as usize];
    (a.0 & decl.body.adm_mask) == (b.0 & decl.body.adm_mask)
}

/// incr 结构身份等价谓词: 与 body_adm_eq 的 adm 位语义一致。
/// 判定两个 frame 是否"同一个容器"(结构身份), 用于 incr 容器 A/D 匹配。
/// kind 相同 + diff 分区 adm 位相同 (CF/Comment/HtmlElement/TableRow 的 diff adm=∅)。
pub fn same_container(a: BlockFrame, b: BlockFrame) -> bool {
    if a.kind() != b.kind() { return false; }
    match a.kind() {
        // BQ body = 内容列（lazy 打断偏移用）——身份不参与，落 _ => true
        FrameKind::BulletList => a.bullet_code() == b.bullet_code(),
        FrameKind::OrderedList => a.delim_code() == b.delim_code(),
        FrameKind::HtmlBlock => a.block_type() == b.block_type(),
        FrameKind::RefDef => a.body() == b.body(),
        FrameKind::Table => a.body() == b.body(),
        _ => true, // BQ（删位）, CF, Comment, HtmlElement, TableRow: diff adm = ∅
    }
}

/// PopPush 相邻配对判定（adm_equal=true 身份语义）——pub API 薄委托：
/// 语义住 infer::is_poppush_with。
#[inline]
pub fn is_poppush(e: u64, next: Option<u64>) -> bool {
    crate::infer::is_poppush_with(e, next, true)
}

/// PopPush 相邻配对判定（adm_equal=false 容器延续语义）——kind-only：
/// LOOSE(out) 差异的 PopPush 仍是容器延续。pub API 薄委托。
#[inline]
pub fn is_poppush_kindonly(e: u64, next: Option<u64>) -> bool {
    crate::infer::is_poppush_with(e, next, false)
}

// ── body 双分区 (ripple FC1 / 扫描互模拟) ──
// adm = 影响 admission (Pop/Continue 判定) 的 body 位; out = 仅影响产出的位。
// 掩码由 block.rs 的 body 位常量派生（body_layout——位位置单源——不照抄
// formal_scan §S.2（其 fence_len 归 out 为假断言——fence_len 影响 closing
// fence 即 admission）——手写掩码与位编码漂移从结构上不可能）。
use crate::block::{BLT_BULLET_MASK, CF_FENCE_BIT, CF_LEN_MASK,
    HTML_TYPE_MASK_NEW, LOOSE_BIT, OL_DELIM_BIT};
const fn body_layout(adm: u16, out: u16) -> BodyLayout {
    BodyLayout { adm_mask: adm, out_mask: out }
}
const BQ_BODY: BodyLayout = body_layout(0x00, 0x00);           // BQ body 恒 0（无 para_open 位——段落由 prev 词法）
const BLT_BODY: BodyLayout = body_layout(BLT_BULLET_MASK, LOOSE_BIT); // bullet_code(bits5-4)=adm, LOOSE(bit3)=out
const OL_BODY: BodyLayout = body_layout(OL_DELIM_BIT, LOOSE_BIT);     // delim(bit5)=adm, LOOSE(bit3)=out
const CF_BODY: BodyLayout = body_layout(CF_FENCE_BIT | CF_LEN_MASK, 0x00); // fence_char(bit5)+fence_len(bits4-0)=adm
const HTMLBLOCK_BODY: BodyLayout = body_layout(HTML_TYPE_MASK_NEW, 0x00); // block_type(bits5-3)=adm
const TABLE_BODY: BodyLayout = body_layout(0x3F, 0x00);         // header_cols(bits5-1)+confirmed(bit0)=adm
const COMMENT_BODY: BodyLayout = body_layout(0x00, 0x00);
const HELEM_BODY: BodyLayout = body_layout(0x00, 0x00);   // 开行行号已移入 scan 运行时 open_lines
const REFDEF_BODY: BodyLayout = body_layout(0x01, 0x00);        // has_content(bit0)=adm
const TABLEROW_BODY: BodyLayout = body_layout(0x00, 0x00);

// （rules×9 判别式数组已随判定单级化删除：scene 中间名与 rules 判别式消失，
//   classify 直产 (BlockType, ScanAction)）

/// 10 行容器声明。
pub static CONTAINER_DECLS: [ContainerDecl; 10] = [
    // 0 BlockQuote
    ContainerDecl { kind: FrameKind::BlockQuote, body: BQ_BODY, is_nesting: true,
        admit: AdmitExpr::Always,
        classify: crate::block::classify_bq_scene,
        has_pop_push: false, has_loose: false,
        content_constraint: ContentConstraint::RequireBqPrefix,
        lazy_exempt: Some(crate::block::bq_lazy_candidate),
        bq_ic_lazy: BqIcLazyRule::ParaIfNoClose },
    // 1 BulletList — 与 OL 共享 list 逻辑
    ContainerDecl { kind: FrameKind::BulletList, body: BLT_BODY, is_nesting: true,
        admit: AdmitExpr::Always, classify: crate::block::classify_list_scene,
        has_pop_push: true, has_loose: true,
        content_constraint: ContentConstraint::RequireListIndent,
        lazy_exempt: Some(crate::block::list_lazy_candidate),
        bq_ic_lazy: BqIcLazyRule::Keep },
    // 2 OrderedList — 与 BLT 共享 list 逻辑
    ContainerDecl { kind: FrameKind::OrderedList, body: OL_BODY, is_nesting: true,
        admit: AdmitExpr::Always, classify: crate::block::classify_list_scene,
        has_pop_push: true, has_loose: true,
        content_constraint: ContentConstraint::RequireListIndent,
        lazy_exempt: Some(crate::block::list_lazy_candidate),
        bq_ic_lazy: BqIcLazyRule::Keep },
    // 3 CodeFence
    ContainerDecl { kind: FrameKind::CodeFence, body: CF_BODY, is_nesting: false,
        admit: AdmitExpr::Always,
        classify: crate::block::classify_cf,
        has_pop_push: false, has_loose: false,
        content_constraint: ContentConstraint::None, lazy_exempt: None,
        bq_ic_lazy: BqIcLazyRule::Keep },
    // 4 HtmlBlock
    ContainerDecl { kind: FrameKind::HtmlBlock, body: HTMLBLOCK_BODY, is_nesting: false,
        admit: AdmitExpr::Always,
        classify: crate::block::classify_html_block,
        has_pop_push: false, has_loose: false,
        content_constraint: ContentConstraint::None, lazy_exempt: None,
        bq_ic_lazy: BqIcLazyRule::Keep },
    // 5 Table — state machine: tentative→confirmed via separator
    ContainerDecl { kind: FrameKind::Table, body: TABLE_BODY, is_nesting: false,
        admit: AdmitExpr::Fn(crate::block::admit_table),
        classify: crate::block::classify_table,
        has_pop_push: false, has_loose: false,
        content_constraint: ContentConstraint::None, lazy_exempt: None,
        bq_ic_lazy: BqIcLazyRule::Keep },
    // 6 Comment
    ContainerDecl { kind: FrameKind::Comment, body: COMMENT_BODY, is_nesting: false,
        admit: AdmitExpr::Always,
        classify: crate::block::classify_comment,
        has_pop_push: false, has_loose: false,
        content_constraint: ContentConstraint::None, lazy_exempt: None,
        bq_ic_lazy: BqIcLazyRule::Keep },
    // 7 HtmlElement
    ContainerDecl { kind: FrameKind::HtmlElement, body: HELEM_BODY, is_nesting: false,
        admit: AdmitExpr::Always,
        classify: crate::block::classify_html_element,
        has_pop_push: false, has_loose: false,
        content_constraint: ContentConstraint::None, lazy_exempt: None,
        bq_ic_lazy: BqIcLazyRule::Keep },
    // 8 RefDef
    ContainerDecl { kind: FrameKind::RefDef, body: REFDEF_BODY, is_nesting: false,
        admit: AdmitExpr::Always,
        classify: crate::block::classify_refdef,
        has_pop_push: false, has_loose: false,
        content_constraint: ContentConstraint::None, lazy_exempt: None,
        bq_ic_lazy: BqIcLazyRule::Keep },
    // 9 TableRow — data row item within confirmed Table; PopPush for consecutive rows
    ContainerDecl { kind: FrameKind::TableRow, body: TABLEROW_BODY, is_nesting: false,
        admit: AdmitExpr::Fn(crate::block::admit_table_row),
        classify: crate::block::classify_table_row,
        has_pop_push: true, has_loose: false,
        content_constraint: ContentConstraint::None, lazy_exempt: None,
        bq_ic_lazy: BqIcLazyRule::Keep },
];


// 呈现通道商集（三通道商集之一——CtxClass = Ctx/≈——
// 行为变体数决定基数——7 类 + Default = 8 变体——消费: apply——
// CTX_OVERRIDES 稀疏表——呈现修改的单源）


/// 呈现通道的上下文类别（效应等价类——ctx1 ≈ ctx2 ⟺ ∀k:
/// apply(k, ctx1) == apply(k, ctx2)——分类器组合派生列——§9.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CtxClass {
    /// item 打开 + 内容 Paragraph——无修改
    ItemOpenPara,
    /// item 打开 + 内容块（标题/fence/thematic/refdef/IC）——SuppressCt
    ItemOpenBlock,
    /// item 打开 + 内容嵌套（BQ/嵌套列表）——SuppressCt
    ItemOpenNest,
    /// item 打开 + 空内容——NoWrapper（#316）
    ItemOpenEmpty,
    /// item 内 tight 段落块——RedirectTo(InlineText)（#301）
    ItemTightPara,
    /// 撤销的 rd（resolved ≠ 帧类型）——Transparent（#200/#198）
    ClosedRd,
    /// guard 触发 ∧ prev 不 closes（#313——scan 侧分类——执行状态——
    /// 不经 classify_ctx）
    GuardLazy,
    /// 其他——无修改
    Default,
}

/// 重定向目标（Override::RedirectTo 的参数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Render {
    /// 内容按 inline 直写（tight item 段落——#301）
    InlineText,
    /// 行按 lazy 段落归属（guard 触发——#313）
    LazyPara,
}

/// 呈现修改（Override——呈现修改代数的一阶实例——四维:
/// 形态（Transparent/RedirectTo）/属性（SuppressCt）/
/// 子结构（NoWrapper）——内容源维度由 content_mode 承担）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Override {
    /// 抑制 item 的 ct（内容以块呈现——挂 ch）
    SuppressCt,
    /// 空 item 无 loose wrapper（#316）
    NoWrapper,
    /// 壳透明（不产节点——rd 撤销/Table 未确认）
    Transparent,
    /// 重定向到其他渲染（InlineText/LazyPara）
    RedirectTo(Render),
}

/// CTX_OVERRIDES——呈现修改的稀疏表（(CtxClass, Override)——单源——
/// apply 查表——SDL 呈现层比对对象——词法表共享合理）。
/// 稀疏行序表（8 变体全显式——6 映射＋2 显式 None：第一方覆盖，未命中=缺陷）。
pub static CTX_OVERRIDES: &[(CtxClass, Option<Override>)] = &[
    (CtxClass::ItemOpenPara, None),
    (CtxClass::ItemOpenBlock, Some(Override::SuppressCt)),
    (CtxClass::ItemOpenNest, Some(Override::SuppressCt)),
    (CtxClass::ItemOpenEmpty, Some(Override::NoWrapper)),
    (CtxClass::ItemTightPara, Some(Override::RedirectTo(Render::InlineText))),
    (CtxClass::ClosedRd, Some(Override::Transparent)),
    (CtxClass::GuardLazy, Some(Override::RedirectTo(Render::LazyPara))),
    (CtxClass::Default, None),
];

/// 块形态类型学（κ' 的声明——头体块/定界块/修正块/
/// 连续块/候选块——统一结构类型学的块层实例——消费: SDL 结构层比对）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// 头体块（item——打开行 + 内容段——HBS 的 H1/H2）
    HeadBody,
    /// 定界块（fence/HtmlBlock/Comment——开标记 + 内容 + 闭标记）
    Delimited,
    /// 修正块（Paragraph——setext 候选）
    Corrected,
    /// 连续块（Paragraph/IC——连续同类型行序列——Part A）
    Continuous,
    /// 候选块（RefDef——候选确认）
    Candidate,
}

/// BLOCK_ASSEMBLY——类型 → 块形态的映射表（κ' 的类型学声明——
/// 单源——SDL 结构层独立推导的输入）。
pub static BLOCK_ASSEMBLY: &[(BlockKind, BlockType)] = &[
    (BlockKind::HeadBody, BlockType::Bullet),
    (BlockKind::HeadBody, BlockType::Checklist),
    (BlockKind::HeadBody, BlockType::OrderedListItem),
    (BlockKind::Delimited, BlockType::CodeFenceOpen),
    (BlockKind::Delimited, BlockType::HtmlBlock),
    (BlockKind::Delimited, BlockType::Comment),
    (BlockKind::Corrected, BlockType::Paragraph),
    (BlockKind::Continuous, BlockType::IndentedCode),
    (BlockKind::Candidate, BlockType::RefDef),
];

/// 查询类型的块形态（O(|表|)——表 ~9 行）。
pub fn block_kind(t: BlockType) -> BlockKind {
    BLOCK_ASSEMBLY.iter()
        .find(|(_, ty)| *ty == t)
        .map(|(k, _)| *k)
        .unwrap_or(BlockKind::Delimited) // 其余定界类（Table/TableRow/HtmlElement 等）
}

/// 查表（稀疏线性——O(|表|)——表 8 行——O(1) 级；第一方覆盖——未命中=缺陷）。
pub fn override_for(cls: CtxClass) -> Option<Override> {
    CTX_OVERRIDES.iter().find(|(c, _)| *c == cls)
        .expect("CTX 未覆盖键组合＝方案缺陷（第一方覆盖）")
        .1
}

/// 渲染形态（apply 的 shape 产出——呈现修改代数四维的应用结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderShape {
    /// 正常（基础效应决定——无上下文修改）
    Normal,
    /// 内容以块呈现（item 内容块——ct 抑制——SuppressCt）
    BlockContent,
    /// 空内容（无 loose wrapper——NoWrapper——#316）
    EmptyContent,
    /// 透明（不产节点——Transparent——rd 撤销/Table 未确认）
    Transparent,
    /// inline 直写（tight item 段落——RedirectTo(InlineText)——#301）
    InlineText,
}

/// 呈现视图（apply 的产出——呈现修改的声明形态——消费: SDL 呈现层比对）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderView {
    /// 上下文修改（override——None = 无修改）
    pub override_mod: Option<Override>,
    /// 渲染形态（应用后）
    pub shape: RenderShape,
}

/// apply——呈现修改入口（基础效应 + 上下文分类（classify_ctx）+ Override
/// 查表——walk 消费点查表。基础透明（eff.is_transparent/Table 未确认——
/// 帧级判定）由 shell_transparent 承担（单源——rd 撤销分支与 ClosedRd 合并）。
/// item_depth（层感知进列）：item 打开行时传该 item
/// 的列表帧栈深度（`- - foo` 外层 0/内层 1）——content_kind per-(line,
/// depth) 直读——item_content 穿参消除（§14.4 关闭）。
pub fn apply(view: &dyn ResolveView, line: usize,
             is_item_open: bool, item_depth: usize) -> RenderView {
    // 纯观测（deep）：呈现修改入口的合计成本（含 classify_ctx 的上下文分类）。
    #[cfg(feature = "telemetry")]
    let _g_apply = crate::telemetry::deep_span("ef_apply");
    let cls = view.classify_ctx(line, is_item_open, item_depth);
    let ov = override_for(cls);
    let shape = match ov {
        Some(Override::SuppressCt) => RenderShape::BlockContent,
        Some(Override::NoWrapper) => RenderShape::EmptyContent,
        Some(Override::Transparent) => RenderShape::Transparent,
        Some(Override::RedirectTo(Render::InlineText)) => RenderShape::InlineText,
        // GuardLazy（RedirectTo(LazyPara)——scan 侧分类——行按 Paragraph
        // 归属——shape 无修改（scan 的归属已处理））
        Some(Override::RedirectTo(Render::LazyPara)) | None => RenderShape::Normal,
    };
    RenderView { override_mod: ov, shape }
}

/// 开壳透明判定——壳层单源——消费: 两 walker 的开壳分支。
/// 三源合一: eff.is_transparent（帧级声明）/ Table 未确认（伪表——内容行
/// 按 Paragraph 渲染）/ rd 撤销（与 classify_ctx 的 ClosedRd 同一判定——不双写）。
/// 泛型化（impl ResolveView——FrameSpans/RowView 双实现）。
pub fn shell_transparent(view: &dyn ResolveView, i: usize, fk: FrameKind) -> bool {
    let eff = effect_for(fk);
    eff.is_transparent
        || (fk == FrameKind::Table && !view.table_confirmed_at(i))
        || (fk == FrameKind::RefDef
            && view.classify_ctx(i, false, 0) == CtxClass::ClosedRd)
}


// classify_delta — η_w⁻¹ 统一事件反推 (incr 的调用点不存在——现仅
// sdl 比对消费 EventClass)


/// η_w⁻¹ 反推的事件类别 (C_info / I_info / T_info 三分).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventClass {
    /// 递归容器 Push → 块树新增容器节点 (zone_descend 下钻, build_item 产容器).
    C_info_Push,
    /// 递归容器 Pop → 填充 ends / 截断 (container_end_at_index 闭).
    C_info_Pop,
    /// 容器壳替换 (异 body/跨kind PopPush) → align hoist / 容器截断.
    C_info_PopPush_cross,
    /// ⊤ 容器 Push (CF/Comment) → 结构透明, 不下钻.
    T_info_Push,
    /// ⊤ 容器 Pop → 结构透明.
    T_info_Pop,
    /// item 边界 (同 body PopPush) → 列表 item 分界.
    I_info_PopPush,
    /// body_out 位属性变 (LOOSE_BIT 翻转) → 整 list REPLACE (刚性 L2).
    T_info_BodyMut_out,
    /// 无栈变化 → 内容行 (叶子续行 / item 内容).
    T_info_Continue,
}

/// G 列在行间差 — classify_delta 的输入。
#[derive(Debug, Clone, Copy, Default)]
pub struct GDelta {
    pub item_boundary: bool,
    pub container_change: bool,
    pub is_loose_flip: bool,
}

/// incr 的全局分发器 — G 列 δ → EventClass。
///
/// 只用 G 列信号, 不查 effect 表 (深度差 Push/Pop 全走 C_info;
/// incr 里所有显式容器都建树节点, 产 JSON 壳和 children,
/// 递归/不递归区分由 is_nesting 在读端处理)。
/// 新容器类型只改 CONTAINER_DECLS 的 is_nesting/item_leaf_type_str 行,
/// 本函数零修改。
pub fn classify_delta(_kind: FrameKind, depth_change: isize, g: &GDelta) -> EventClass {
    match depth_change {
        d if d > 0 => EventClass::C_info_Push,
        d if d < 0 => EventClass::C_info_Pop,
        0 if g.item_boundary  => EventClass::I_info_PopPush,
        0 if g.container_change => EventClass::C_info_PopPush_cross,
        0 if g.is_loose_flip => EventClass::T_info_BodyMut_out,
        _ => EventClass::T_info_Continue,
    }
}

/// 内容字段类型。
///
/// 为什么用枚举而不是 fn 指针?
///   Inline 的 content 提取输入空间是有限的: (raw: &str, fence_len: u8, sub: u8) —
///   所有可能的变换都可以枚举。枚举提供编译期穷举检查, fn 指针没有这个保证。
///   （Block 的 depth_only 判定同模式——也已是 DepthOnlyRule 枚举，带 eval 方法。
///   两者选择相同策略: 谓词空间有限 → 枚举穷举（受检不变量）。）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentField {
    /// raw byte slice — Plain, Escape, HtmlInline (via Projected)
    Text,
    /// normalize_fence_content(raw, fence_len, sub) — Fenced
    Code,
    /// decode_link_dest(project(Content)) — Bracket/link, Bracket/ref
    Href,
    /// src + alt — Bracket/image 特殊 (Content 解码后做 src, Tag 做 alt)
    ImageAttr,
    /// raw[1..len-1] — AngleWrap/html_inline
    AngleWrapText,
    /// 合成常量 — Plain/softbreak→" ", Plain/hardbreak→"\n"
    StaticStr(&'static str),
    /// decode_entity(project(Content)) — Entity
    DecodedEntity,
    /// 无 content — PairDelim
    None_,
}

/// ContentField 的 JSON 输出元数据（动态推导——变体自身是数据——
/// json_spec() 方法产出——渲染器读数据不持有 JSON key 知识）。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ContentFieldJson {
    /// JSON key："" = 无输出（ImageAttr/None_）
    pub json_key: &'static str,
    /// StaticStr 的静态值（其余 None——值来自投影内容）
    pub static_val: Option<&'static str>,
    /// DecodedEntity → decode_entity 变换
    pub decode: bool,
    /// 空值跳过（Text/AngleWrapText/Href）
    pub optional: bool,
}

impl ContentField {
    /// JSON 输出元数据推导（对标 children_shape/payload_spec——动态性——
    /// 从枚举变体推导，非新建静态表）。
    pub(crate) fn json_spec(&self) -> ContentFieldJson {
        use ContentField::*;
        match self {
            Code => ContentFieldJson { json_key: "cd", static_val: None, decode: false, optional: false },
            StaticStr(s) => ContentFieldJson { json_key: "ct", static_val: Some(s), decode: false, optional: false },
            DecodedEntity => ContentFieldJson { json_key: "ct", static_val: None, decode: true, optional: false },
            Text | AngleWrapText | Href => ContentFieldJson { json_key: "ct", static_val: None, decode: false, optional: true },
            ImageAttr | None_ => ContentFieldJson { json_key: "", static_val: None, decode: false, optional: true },
        }
    }
}

/// 属性来源 — 描述 JSON 属性的数据从哪里提取、是否需要后处理。
#[derive(Debug, Clone, Copy)]
pub enum AttrSource {
    /// inline::project(field) — 直接使用 (content, alt)
    Projected(InlineField),
    /// decode_link_dest(inline::project(field)) — URL 解码 (href, src, title)
    ProjectedDecoded(InlineField),
    /// autolink href 归一化 (CM §6.5/6.6): email 加 mailto: 前缀、scheme 转小写
    ProjectedAutolinkHref,
    /// decode_entity(raw) — Entity
    DecodedEntity,
    /// 静态字符串常量
    Static(&'static str),
}

/// 单个 JSON 属性的描述。
#[derive(Debug, Clone, Copy)]
pub struct AttrSpec {
    /// JSON key 名称
    pub name: &'static str,
    /// 数据来源
    pub source: AttrSource,
    /// 条件发射: true 表示仅非空时写入 (如 title)
    pub optional: bool,
}

/// 内联效应表条目——渲染语义的数据化声明。
#[derive(Debug, Clone)]
pub struct InlineEffect {
    /// JSON nodeType
    pub node_type: &'static str,
    /// 是否有 children 数组 (容器型)
    pub is_container: bool,
    /// content 字段的提取/编码方式
    pub content_field: ContentField,
    /// JSON 属性列表 (非 content 的额外字段)
    pub attr_fields: &'static [AttrSpec],
}

// ── Inline 静态表 — 28 行, 按 (GeoKind as usize) * 4 + sub 索引 ──

use InlineField::Content as FldContent;
use InlineField::Tag as FldTag;
use InlineField::Title as FldTitle;

/// 空属性列表
static NO_ATTRS: &[AttrSpec] = &[];

/// link (Bracket sub=0, !ref): href + content + title(optional)
static LINK_ATTRS: &[AttrSpec] = &[
    AttrSpec { name: "h",    source: AttrSource::ProjectedDecoded(FldContent), optional: false },
    AttrSpec { name: "ct", source: AttrSource::Projected(FldTag),            optional: false },
    AttrSpec { name: "ti",   source: AttrSource::ProjectedDecoded(FldTitle),   optional: true },
];

/// image (Bracket sub=1, !ref): src + alt + title(optional)
static IMAGE_ATTRS: &[AttrSpec] = &[
    AttrSpec { name: "s",   source: AttrSource::ProjectedDecoded(FldContent), optional: false },
    AttrSpec { name: "alt",   source: AttrSource::Projected(FldTag),            optional: false },
    AttrSpec { name: "ti", source: AttrSource::ProjectedDecoded(FldTitle),   optional: true },
];

/// ref link (Bracket sub=2): href only
static REF_ATTRS: &[AttrSpec] = &[
    AttrSpec { name: "h", source: AttrSource::ProjectedDecoded(FldContent), optional: false },
];

/// autolink (AngleWrap sub=0): ct 由 content_field=Href 提供（content 投影
/// ——href 原文）；h = 归一化（mailto/scheme 小写）。原表含 ct 条目——
/// write_inline_node 的 else 分支 content_field 已写 ct——重复（
/// `<http://a.b>` 输出两个 ct）——删 ct 条目（h 保留）。
static AUTOLINK_ATTRS: &[AttrSpec] = &[
    AttrSpec { name: "h",    source: AttrSource::ProjectedAutolinkHref, optional: false },
];

/// 28 行全表。
/// 索引 = (GeoKind as usize) * 4 + sub。
/// 子键编码规则见 inline_effect_key()。
pub static INLINE_EFFECTS: [InlineEffect; 28] = [
    // ── Plain: sub 0=Text, 1=SoftBreak, 2=HardBreak, 3=unused ──
    // （表值与 fast path 对齐——Plain 走 write_inline_node 的 fast path
    // （硬编码 "txt"/"sb"/"lb"）——表原 "tx"/"hb" 从未被消费——死声明）
    InlineEffect { node_type: "txt",      is_container: false,
        content_field: ContentField::Text,           attr_fields: NO_ATTRS },
    InlineEffect { node_type: "sb",  is_container: false,
        content_field: ContentField::StaticStr(" "),  attr_fields: NO_ATTRS },
    InlineEffect { node_type: "lb",  is_container: false,
        content_field: ContentField::StaticStr("\n"), attr_fields: NO_ATTRS },
    // sub=3 unused (pad to 4)
    InlineEffect { node_type: "txt",      is_container: false,
        content_field: ContentField::Text,           attr_fields: NO_ATTRS },
    // ── PairDelim: sub 0=degenerate(closer_use=0,不可达), 1=emphasis, 2=strong, 3=strikethrough ──
    InlineEffect { node_type: "em",       is_container: true,
        content_field: ContentField::None_, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "em",       is_container: true,
        content_field: ContentField::None_, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "st",         is_container: true,
        content_field: ContentField::None_, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "dl",  is_container: true,
        content_field: ContentField::None_, attr_fields: NO_ATTRS },
    // ── Fenced: sub 0=code, 1=math, 2=display_math, 3=unused ──
    InlineEffect { node_type: "cd",   is_container: false,
        content_field: ContentField::Code, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "mt",          is_container: false,
        content_field: ContentField::Code, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "dm",  is_container: false,
        content_field: ContentField::Code, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "tx",          is_container: false,
        content_field: ContentField::Text, attr_fields: NO_ATTRS },
    // ── Bracket: sub 0=link(!ref), 1=image(!ref), 2=ref, 3=unused ──
    InlineEffect { node_type: "lk",  is_container: true,
        content_field: ContentField::Href,      attr_fields: LINK_ATTRS },
    InlineEffect { node_type: "im", is_container: true,
        content_field: ContentField::ImageAttr, attr_fields: IMAGE_ATTRS },
    InlineEffect { node_type: "rf",   is_container: true,
        content_field: ContentField::Href,      attr_fields: REF_ATTRS },
    InlineEffect { node_type: "tx",  is_container: false,
        content_field: ContentField::Text,     attr_fields: NO_ATTRS },
    // ── AngleWrap: sub 0=autolink, 1=html_inline, 2,3=unused ──
    InlineEffect { node_type: "al",    is_container: false,
        content_field: ContentField::Href,          attr_fields: AUTOLINK_ATTRS },
    // sub=1 (html_inline): 当前不可达 — consume_anglewrap 对 HTML 标签走 text_fallback,
    // 产生 Plain 节点而非 AngleWrap(sub=1)。此行为类型系统预留。
    InlineEffect { node_type: "hi", is_container: false,
        content_field: ContentField::AngleWrapText, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "tx",        is_container: false,
        content_field: ContentField::Text,          attr_fields: NO_ATTRS },
    InlineEffect { node_type: "tx",        is_container: false,
        content_field: ContentField::Text,          attr_fields: NO_ATTRS },
    // ── Escape: sub 0, 1-3=unused ──
    InlineEffect { node_type: "tx",   is_container: false,
        content_field: ContentField::Text, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "tx",   is_container: false,
        content_field: ContentField::Text, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "tx",   is_container: false,
        content_field: ContentField::Text, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "tx",   is_container: false,
        content_field: ContentField::Text, attr_fields: NO_ATTRS },
    // ── Entity: sub 0, 1-3=unused ──
    InlineEffect { node_type: "entity", is_container: false,
        content_field: ContentField::DecodedEntity, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "text",   is_container: false,
        content_field: ContentField::Text, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "text",   is_container: false,
        content_field: ContentField::Text, attr_fields: NO_ATTRS },
    InlineEffect { node_type: "text",   is_container: false,
        content_field: ContentField::Text, attr_fields: NO_ATTRS },
];

/// 将 InlineFrame 的子类型解码为效应表索引。
///
/// 索引 = (GeoKind as usize) * 4 + sub, O(1) 直接索引。
/// sub 的编码规则与 InlineFrame 的 per-geometry body 解码一致。
/// PairDelim 的 clos_use=0 映射到 sub=0 (emphasis fallback, 在实践中不可达但数组已填满)。
pub fn inline_effect_key(geo: GeoKind, frame: InlineFrame) -> usize {
    let sub = match geo {
        GeoKind::Plain => frame.plain_sub() as usize,
        GeoKind::PairDelim => {
            if frame.delim_byte() == 2 {
                3 // strikethrough
            } else {
                // closer_use: 1=emph, 2=strong, 0=degenerate (unreachable)
                frame.closer_use() as usize
            }
        }
        GeoKind::Fenced => frame.fenced_sub() as usize,
        GeoKind::Bracket => {
            if frame.is_ref() {
                2 // ref
            } else if frame.is_image() {
                1 // image
            } else {
                0 // link
            }
        }
        GeoKind::AngleWrap => frame.angle_sub() as usize,
        GeoKind::Escape => 0,
        GeoKind::Entity => 0,
    };
    (geo as usize) * 4 + sub
}

pub fn inline_effect_for(geo: GeoKind, frame: InlineFrame) -> &'static InlineEffect {
    &INLINE_EFFECTS[inline_effect_key(geo, frame)]
}


// 提取语义声明（原 spec.rs 合并回 effect——语义化流水线的"表化"端——
// sdl_prev_6 §2.3 三层分离）
// 职责: 生产消费（查表——消除双表达）+ SDL 独立推导的比对对象。
// 原则:
//   · 只含标量语义（枚举/常量/参数函数）——复杂逻辑（提取/格式化
//     的实现）保持生产私有（镜像源教训——回归率 ∝ 镜像数）
//   · 语义从 CM 规范推导（注释引用）——SDL 侧独立推导比对（防漂移）
//   · 生产消费点改造 = 零行为变化（策略查表——计算逻辑不变）


// ── Dedent 语义——续行剥几列（5 个语义域——CM §4.4/4.5/5.2）──

/// 续行 dedent 场景（提取语境分类——生产调用方与 SDL 共享——稳定语义域）。
/// 职责边界: 生产 = 场景识别（"我是 IC 首行/续行"）+ 执行（提取）；
///           spec = 行为选择（场景 → 语义 → 参数）——生产只剩"识别 + 执行"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedentScene {
    /// IC 内容（CM §4.4——indented code 是 4 空格缩进）
    ICContent,
    /// fence 内容（CM §4.5——内容行剥开围栏缩进——fence_indent）
    FenceContent,
    /// IC 后续行（CM §5.2 rule #2——4 spaces beyond the edge——阈值）
    ICContinuation,
    /// item 续行几何路径（CM §5.2——内容区边缘——content_offset）
    ItemGeometry,
    /// item 续行 fallback（几何无——Fixed4——KNOWN_DIVERGENCES）
    ItemFallback,
}

/// 续行 dedent 语义（规范值——场景 → 语义的映射结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedentSpec {
    /// IC 内容固定剥 4（CM §4.4）
    IndentedCode4,
    /// fence 内容剥开行缩进（CM §4.5——开行缩进由调用方算）
    FenceIndent,
    /// IC 后续行剥内容阈值（CM §5.2 rule #2——阈值由调用方算）
    ContentThreshold,
    /// item 续行剥内容区边缘（CM §5.2——content_offset 由调用方算）
    ContentEdge,
    /// item fallback 固定 4
    Fixed4,
}

/// 场景 → 语义（规范查表——SDL 独立推导的比对对象）。
pub fn dedent_spec_for(scene: DedentScene) -> DedentSpec {
    match scene {
        DedentScene::ICContent => DedentSpec::IndentedCode4,
        DedentScene::FenceContent => DedentSpec::FenceIndent,
        DedentScene::ICContinuation => DedentSpec::ContentThreshold,
        DedentScene::ItemGeometry => DedentSpec::ContentEdge,
        DedentScene::ItemFallback => DedentSpec::Fixed4,
    }
}

/// 语义 → 参数（dedent 列数——computed 由调用方算几何值——
/// content_offset/fence_indent/threshold——职责分离: 调用方算几何值、
/// spec 选策略）。
pub fn dedent_cols(spec: DedentSpec, computed: usize) -> usize {
    match spec {
        DedentSpec::IndentedCode4 | DedentSpec::Fixed4 => geo::LIST_CONTENT_MAX,
        _ => computed,
    }
}

/// 场景→列数单一入口（内容偏移列：computed 参数的单一来源）——
/// 调用方只传事实（声明几何偏移／未消费边缘／fence 缩进），算式全在声明层。
pub fn dedent_cols_for(
    scene: DedentScene,
    geo_content_offset: usize,
    edge: usize,
    fence_indent: usize,
) -> usize {
    dedent_cols(dedent_spec_for(scene), match scene {
        DedentScene::ItemGeometry => geo_content_offset,
        DedentScene::ItemFallback | DedentScene::ICContent => geo::LIST_CONTENT_MAX,
        DedentScene::ICContinuation => geo::LIST_CONTENT_MAX + edge,
        DedentScene::FenceContent => fence_indent,
    })
}

// （markers_on_line 在 tests/prefix_equiv.rs 作钉①锚；
//   生产站点由 prefix_cut 折叠供给。）

// ── Normalize 语义——段落收束（CM §4.8——段落尾换行不是软换行）──

/// 段落内容收束语义（content 提取的尾换行/前导缩进处理）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalizeSpec {
    /// 收束（paragraph_text_normalize——剥尾换行 + 前导缩进——CM §4.8）
    StripTrailing,
    /// 原样
    Raw,
}

/// 类型 → 收束语义（当前适用域: 仅 Paragraph——F/P 两消费点一致）。
pub fn normalize_spec(nt: BlockType) -> NormalizeSpec {
    match nt {
        BlockType::Paragraph => NormalizeSpec::StripTrailing,
        _ => NormalizeSpec::Raw,
    }
}

// ── 前缀律选择单源（prefix_cut 的 (kinds, stop) 参数；落点 effect）──

/// 消费种类轴（一律两列之一）：BqOnly＝只消费 BQ 层；All＝BQ＋列表层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixKinds { BqOnly, All }

/// 停止轴（一律两列之二）：Chain＝链上帧数配对消费；Counted＝计数消费
/// （BQ 数=链 BQ 帧数、列表数=行上开层数）；None＝不设上限；
/// OneStep＝单层（ic_line_strip_bq 的「仅位置 0 单层」语义——C2 修订：
/// OneStep 既是 scan 原子，也是读侧单层 stop）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixStop { Chain, Counted, None, OneStep }

/// 前导轴（C10 参数轴恢复——对齐补漏）：SkipWs＝层宽含前导空白（step_layer
/// 默认语义）；Exact0＝层必须从位置 0 开始（前导空白非零即不匹配——
/// ic_line_strip_bq 的「不跳前导空白——缩进后的 > 是内容」语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixLead { SkipWs, Exact0 }

/// 尾轴（C10 参数轴——自 infer 迁入：参数选择落点 effect，折叠执行落点 infer）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixTail { None, FinalSkip, PerLayerTrim }

/// 前缀律完整参数（对齐补漏：选择单源供给三轴——消费轴 kinds/stop、
/// 参数轴 lead/tail——调用方只传事实，参数全在声明层）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrefixParams {
    pub kinds: PrefixKinds,
    pub stop: PrefixStop,
    pub lead: PrefixLead,
    pub tail: PrefixTail,
}

/// (t, face, dir) → PrefixParams 并列表（7 行序——键三元一维装不下两维；
/// 全显式第一方覆盖）。t 轴用域谓词（item 类/IC/容器类）而非单值枚举。
/// 行值按前缀等价规则修正（content_face=(BqOnly,None)+FinalSkip；
/// 键值槽写不走 prefix_cut——R4 与 R5 同值 (All,None)）。
/// 对齐补漏（参数轴入表）：R7 转正——format_payload Reconstruct 容器前缀
/// （(BqOnly,None)+FinalSkip——8.3 语义）由 R7 供给，站点消字面量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixFace { Content, Container }
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixDir { Read, Write }

pub fn prefix_policy(
    t: crate::block::BlockType,
    face: PrefixFace,
    dir: PrefixDir,
) -> PrefixParams {
    use crate::block::BlockType as T;
    let is_item = matches!(t, T::Bullet | T::Checklist | T::OrderedListItem)
        || t == T::Blockquote;
    let is_ic = t == T::IndentedCode || t == T::CodeContent;
    let skip_ws = PrefixLead::SkipWs;
    match (is_item, is_ic, face, dir) {
        // R1 item 类内容读（#3 content_face/#4 build_node memo）→ 全剥 BQ 前缀
        //（钉②实证：peel_peeled 无链输入——stop=None 非 Chain；尾=FinalSkip）
        (true, _, PrefixFace::Content, PrefixDir::Read) =>
            PrefixParams { kinds: PrefixKinds::BqOnly, stop: PrefixStop::None,
                lead: skip_ws, tail: PrefixTail::FinalSkip },
        // R2 IC 内容读（#2 ic_line_prefix 现已为 prefix_cut(All,Counted) 包装；
        // #1 markers_on_line 实现已入钉①锚）→ 全层计数
        (_, true, PrefixFace::Content, PrefixDir::Read) =>
            PrefixParams { kinds: PrefixKinds::All, stop: PrefixStop::Counted,
                lead: skip_ws, tail: PrefixTail::None },
        // R3 段落归一（#7 alt 替代）→ 全层链配
        (_, false, PrefixFace::Content, PrefixDir::Read) =>
            PrefixParams { kinds: PrefixKinds::All, stop: PrefixStop::Chain,
                lead: skip_ws, tail: PrefixTail::None },
        // R4 键值槽写（#9——实际走 consume 单源不经 prefix_cut；行保留为声明，
        // 值同 R5——前缀重发口径）
        (true, _, PrefixFace::Content, PrefixDir::Write) =>
            PrefixParams { kinds: PrefixKinds::All, stop: PrefixStop::None,
                lead: skip_ws, tail: PrefixTail::None },
        // R5 写面前缀重发（#8/#11）→ 全层不设限
        (_, _, PrefixFace::Content, PrefixDir::Write) =>
            PrefixParams { kinds: PrefixKinds::All, stop: PrefixStop::None,
                lead: skip_ws, tail: PrefixTail::None },
        // R6 容器面读（node_container——现走几何直取不经 prefix_cut；行保留为声明）
        (_, _, PrefixFace::Container, PrefixDir::Read) =>
            PrefixParams { kinds: PrefixKinds::All, stop: PrefixStop::Chain,
                lead: skip_ws, tail: PrefixTail::None },
        // R7 容器面写（对齐补漏转正）：format_payload Reconstruct 容器前缀——
        // (BqOnly,None)+FinalSkip（8.3 语义——只剥 BQ 前缀，marker 留 Reconstruct）
        (_, _, PrefixFace::Container, PrefixDir::Write) =>
            PrefixParams { kinds: PrefixKinds::BqOnly, stop: PrefixStop::None,
                lead: skip_ws, tail: PrefixTail::FinalSkip },
    }
}

// ── Payload 语义——写端格式化策略（format_payload 的分支）──

/// 写端 payload 策略（format_payload 的 (mark_kind, proj) 分支语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadSpec {
    /// BQ content——逐行加 "> " 前缀（CM §5.1）
    BqPrefix,
    /// 标记重建（BLT/OL tag——marker + 新值 + 剩余——原样保留拼接实现）
    Reconstruct,
    /// 原样（BLT/OL content——payload 不加工——0 缩进续行由
    /// LazyParagraph 并入——CM §5.2）
    Raw,
}

/// (mark_kind, proj) → 写端策略（策略选择表化——分支实现保留生产侧）。
pub fn payload_spec(t: crate::block::BlockType, proj: &str) -> PayloadSpec {
    let spec = line_spec(t);
    match (spec.mark_width, spec.tag_rule, proj) {
        (MarkWidth::Fixed(">"), _, "content") => PayloadSpec::BqPrefix,
        (_, TagRule::ColonKey | TagRule::CheckboxColonKey | TagRule::Digits, "tag") => PayloadSpec::Reconstruct,
        _ => PayloadSpec::Raw,
    }
}

#[cfg(test)]
mod spec_semantics_tests {
    use super::*;

    #[test]
    fn dedent_spec_semantics() {
        // CM §4.4——IC 内容固定剥 4
        assert_eq!(dedent_spec_for(DedentScene::ICContent), DedentSpec::IndentedCode4);
        // CM §4.5——fence 内容剥开行缩进
        assert_eq!(dedent_spec_for(DedentScene::FenceContent), DedentSpec::FenceIndent);
        // CM §5.2 rule #2——IC 后续行剥阈值
        assert_eq!(dedent_spec_for(DedentScene::ICContinuation), DedentSpec::ContentThreshold);
        // CM §5.2——item 续行剥内容区边缘
        assert_eq!(dedent_spec_for(DedentScene::ItemGeometry), DedentSpec::ContentEdge);
        // fallback（KNOWN_DIVERGENCES）
        assert_eq!(dedent_spec_for(DedentScene::ItemFallback), DedentSpec::Fixed4);
        // 参数推导——固定 4 语义恒 4；几何语义用 computed
        assert_eq!(dedent_cols(DedentSpec::IndentedCode4, 999), 4);
        assert_eq!(dedent_cols(DedentSpec::Fixed4, 999), 4);
        assert_eq!(dedent_cols(DedentSpec::ContentEdge, 2), 2);
        assert_eq!(dedent_cols(DedentSpec::FenceIndent, 3), 3);
        assert_eq!(dedent_cols(DedentSpec::ContentThreshold, 7), 7);
    }

    #[test]
    fn normalize_spec_semantics() {
        // CM §4.8——段落尾换行不是软换行——Paragraph 收束
        assert_eq!(normalize_spec(BlockType::Paragraph), NormalizeSpec::StripTrailing);
        assert_eq!(normalize_spec(BlockType::Bullet), NormalizeSpec::Raw);
        assert_eq!(normalize_spec(BlockType::Header1), NormalizeSpec::Raw);
    }

    #[test]
    fn payload_spec_semantics() {
        use crate::block::BlockType;
        // CM §5.1——BQ content 加 "> " 前缀
        assert_eq!(payload_spec(BlockType::Blockquote, "content"), PayloadSpec::BqPrefix);
        // BLT/OL tag——标记重建
        assert_eq!(payload_spec(BlockType::Bullet, "tag"), PayloadSpec::Reconstruct);
        assert_eq!(payload_spec(BlockType::OrderedListItem, "tag"), PayloadSpec::Reconstruct);
        // BLT/OL content——payload 原样（0 缩进续行 LazyParagraph 并入）
        assert_eq!(payload_spec(BlockType::Bullet, "content"), PayloadSpec::Raw);
        assert_eq!(payload_spec(BlockType::OrderedListItem, "content"), PayloadSpec::Raw);
        assert_eq!(payload_spec(BlockType::CodeFenceOpen, "tag"), PayloadSpec::Raw);
    }
}

#[cfg(test)]
mod tests {
    include!("../tests/unit/effect.rs");
}
