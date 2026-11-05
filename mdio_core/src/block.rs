//! Markdown 块级扫描器
//!
//! Optimizations:
//! - u8 compact storage (1 byte per line)
//! - BLOCK_DISPATCH for O(1) first-character dispatch
//! - memchr for SIMD-powered substring search
//! - CommonMark compliant indent logic (<4 spaces)

use memchr::memmem;
use smallvec::SmallVec;
use crate::effect::geo::{TAB_STOP, EMPTY_ITEM_SENTINEL, STRUCT_INDENT_MAX, LIST_IC_OFFSET, LIST_CONTENT_MAX};

/// Maximum container nesting depth (list / blockquote / fence / table).
pub const MAX_BLOCK_NESTING: usize = 32;

/// Checklist `[x]`/`[X]`/`[ ]` 本体宽度（不含 `- ` 与必选空白）。
pub(crate) const CHECKLIST_BRACKET_LEN: usize = 3;
/// Checklist checkbox + 必选空白宽度：`[ ] `。
pub(crate) const CHECKLIST_CHECKBOX_SPAN: usize = CHECKLIST_BRACKET_LEN + 1;
/// Checklist 列表 marker 宽度：`- `。
pub(crate) const CHECKLIST_LIST_MARK_END: usize = 2;
/// Checklist 完整 marker 宽度：`- ` + `[ ]` + 必选空白。
pub(crate) const CHECKLIST_MARK_END: usize = CHECKLIST_LIST_MARK_END + CHECKLIST_CHECKBOX_SPAN;


// HtmlElement helpers


use crop::Rope;


// BlockType discriminant constants


/// First header discriminant (Header1)
pub const HEADER_LO: u8 = 0;
/// Last header discriminant (Header6)
pub const HEADER_HI: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockType {
    // Block marker lines
    Header1 = 0, Header2 = 1, Header3 = 2,
    Header4 = 3, Header5 = 4, Header6 = 5,
    Bullet = 6,
    CodeFenceOpen = 7,
    Blockquote = 8,
    OrderedListItem = 9,
    Paragraph = 10,
    Comment = 11,
    TableRow = 12,
    Checklist = 13,
    HsHead1 = 14,
    HsHead2 = 15,
    HsSep = 16,
    HtmlBlock = 17,
    ThematicBreak = 18,

    // Content lines (type determined by container context)
    CodeFenceClose = 19,
    CodeContent = 20,
    HtmlContent = 21,
    IndentedCode = 22,
    TableSeparator = 23,
    Blank = 24,

    // 27-63: reserved
    RefDef = 25,
    HtmlElement = 26,
}

impl BlockType {
    /// 判别值 → 变体（`transmute` 的安全替代）。
    ///
    /// `PackedLineInfo::node_type` / blob 解码等「整数 → 枚举」面必须走这里：
    /// 27 个合法变体（0..=26；`HtmlElement` 最大）之外的 u8 返回 `None`。
    /// 注意 0xFF 是 `PackedLineInfo` 的哨兵，其 `& 0x3F` 得到 63 —— 不是合法种类。
    #[inline]
    pub fn from_discriminant(v: u8) -> Option<Self> {
        Some(match v {
            0 => BlockType::Header1,
            1 => BlockType::Header2,
            2 => BlockType::Header3,
            3 => BlockType::Header4,
            4 => BlockType::Header5,
            5 => BlockType::Header6,
            6 => BlockType::Bullet,
            7 => BlockType::CodeFenceOpen,
            8 => BlockType::Blockquote,
            9 => BlockType::OrderedListItem,
            10 => BlockType::Paragraph,
            11 => BlockType::Comment,
            12 => BlockType::TableRow,
            13 => BlockType::Checklist,
            14 => BlockType::HsHead1,
            15 => BlockType::HsHead2,
            16 => BlockType::HsSep,
            17 => BlockType::HtmlBlock,
            18 => BlockType::ThematicBreak,
            19 => BlockType::CodeFenceClose,
            20 => BlockType::CodeContent,
            21 => BlockType::HtmlContent,
            22 => BlockType::IndentedCode,
            23 => BlockType::TableSeparator,
            24 => BlockType::Blank,
            25 => BlockType::RefDef,
            26 => BlockType::HtmlElement,
            _ => return None,
        })
    }
}


// PackedLineInfo


/// 紧凑行信息：一字节存储。
///
/// 位布局：0xFF = invalid（哨兵，无有效类型），否则 bits5-0 = BlockType 判别值
/// （合法 0..=26）、bit7 = 续行标志（`CONT_BIT`），bit6 保留恒 0。
/// bits5-0 的 27..=63 无对应变体——外部字节输入必须经
/// `BlockType::from_discriminant`（blob 解码已做严格校验），**不得 transmute**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct PackedLineInfo(pub u8);

impl PackedLineInfo {
    /// Zero-copy view: `&[PackedLineInfo]` → `&[u8]`。
    /// Safe because `#[repr(transparent)]` guarantees identical layout.
    #[inline(always)]
    pub fn as_bytes(slice: &[Self]) -> &[u8] {
        unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, slice.len()) }
    }

    pub(crate) const TYPE_MASK: u8 = 0x3F;   // bits5-0 — BlockType 判别值（合法 0..=26，见 from_discriminant）
    const INVALID_VAL: u8 = 0xFF; // All bits set = invalid
    pub const CONT_BIT: u8 = 0x80; // bit7 — 块续行标志
    pub(crate) const RESERVED_BIT: u8 = 0x40; // bit6 — 保留位，编码器恒 0

    #[inline(always)]
    pub fn pack(node_type: BlockType) -> Self {
        // BlockType 判别值最大 26（HtmlElement），< 27 恒真；保留断言作为
        // 「枚举扩容必须同步位宽/校验」的文档锚点（历史上误写为 < 64）。
        debug_assert!((node_type as u8) < 27, "BlockType discriminant must be < 27");
        Self(node_type as u8)
    }

    #[inline(always)]
    pub fn cont(&self) -> bool {
        self.0 & Self::CONT_BIT != 0
    }

    #[inline(always)]
    pub fn with_cont(self, cont: bool) -> Self {
        if cont { Self(self.0 | Self::CONT_BIT) } else { self }
    }

    #[inline(always)]
    pub fn pack_with_cont(node_type: BlockType, cont: bool) -> Self {
        let mut val = node_type as u8;
        debug_assert!(val < 27, "BlockType discriminant must be < 27");
        if cont { val |= Self::CONT_BIT; }
        Self(val)
    }

    #[inline(always)]
    pub fn invalid() -> Self { Self(Self::INVALID_VAL) }

    #[inline(always)]
    pub fn is_valid(&self) -> bool { self.0 != Self::INVALID_VAL }

    /// 行种类解码。
    ///
    /// # Panics
    /// 哨兵 0xFF（`is_valid() == false`）或 bits5-0 > 26 的非法字节。
    /// 旧实现 `transmute(self.0 & 0x3F)` 对这两类输入构造非法判别式枚举 = UB；
    /// 现在 fail-fast panic 带出原始字节。合法快照的 `lines` 不含哨兵；
    /// 读 `prev_col` 等可能含哨兵的列前必须先 `is_valid()`。
    #[inline(always)]
    pub fn node_type(&self) -> BlockType {
        let raw = self.0 & Self::TYPE_MASK;
        BlockType::from_discriminant(raw).unwrap_or_else(|| {
            panic!(
                "PackedLineInfo::node_type: invalid BlockType discriminant {raw} \
                 (raw byte {:#04x}; valid 0..=26; 0xFF is the invalid sentinel — check is_valid())",
                self.0
            )
        })
    }
}

/// 连续 Paragraph 行 run 尾（exclusive）。PackedLineInfo 是透明 u8，
/// 直接按字节找第一个非 Paragraph 类型；调用方一个根块跳，不做逐行
/// node_end 分支。`end` 上限由调用方给定。
#[inline]
// （paragraph_run_end 已迁 infer——PL 原子纯函数落点纪律）


// Character LUT for O(1) First-Char Dispatch


/// LUT mapping first byte to preliminary type category:
/// 0 = Paragraph (default)
/// 1 = Possible Header (#)
/// 2 = Possible Bullet / ThematicBreak (-, *, +, _)
/// 3 = Possible Fence (`, ~)
/// 4 = Possible Comment (<)
const fn build_char_lut() -> [u8; 256] {
    let mut table = [0u8; 256];
    table[b'#' as usize] = 1;
    table[b'-' as usize] = 2;
    table[b'*' as usize] = 2;
    table[b'+' as usize] = 2;
    table[b'_' as usize] = 2;
    table[b'`' as usize] = 3;
    table[b'~' as usize] = 3;
    table[b'<' as usize] = 4;
    table[b'>' as usize] = 5;
    table[b'|' as usize] = 7;
    table[b'[' as usize] = 8;
    let mut d = b'0';
    while d <= b'9' {
        table[d as usize] = 6;
        d += 1;
    }
    table
}
static BLOCK_DISPATCH: [u8; 256] = build_char_lut();


// Core Scanning Logic


/// HTML block type 1 结束标签（detect_html_block_type 与 html_block_ends
/// 共用——单源）。
const HTML_TYPE1_TAGS: &[&[u8]] = &[b"script", b"pre", b"style", b"textarea"];

/// Detect HTML block type (1-7) from the start of a `<`-prefixed line.
/// Returns 0 if not an HTML block start.
/// pub(crate)（sdl.rs 感知原子共享）。
pub(crate) fn detect_html_block_type(bytes: &[u8]) -> u8 {
    if bytes.len() < 2 || bytes[0] != b'<' { return 0; }
    let rest = &bytes[1..];

    // Type 1: <script, <pre, <style, <textarea (case-insensitive, CommonMark §4.6)
    let tag_end = |tag: &[u8]| {
        rest.len() == tag.len() || matches!(rest[tag.len()], b' ' | b'\t' | b'>')
    };
    for tag in HTML_TYPE1_TAGS {
        if rest.len() >= tag.len()
            && rest[..tag.len()].eq_ignore_ascii_case(tag)
            && tag_end(tag)
        {
            return 1;
        }
    }

    // Type 2: <!-- (but MDIO treats this as Comment, not HtmlBlock)
    if rest.starts_with(b"!--") { return 0; }
    // Type 3: <?
    if rest.starts_with(b"?") { return 3; }
    // Type 4: <! followed by an ASCII letter (CommonMark §4.6: "an ASCII letter")
    if rest.starts_with(b"!") && rest.len() >= 2 && rest[1].is_ascii_alphabetic() { return 4; }
    // Type 5: <![CDATA[
    if rest.starts_with(b"![CDATA[") { return 5; }
    // Type 6: specific tag names (CommonMark §4.6), case-insensitive.
    // Must be checked before type 7 — type 6 takes priority.
    // Accept optional leading `/` for closing tags (e.g., `</div>`).
    let tag_rest = if rest.first() == Some(&b'/') && rest.len() > 1 { &rest[1..] } else { rest };
    if matches_html_block_tag(tag_rest) { return 6; }
    // Type 7: complete open/close tag, then only whitespace til EOL
    if let Some(end) = find_html_tag_end(rest) {
        let after_tag = &rest[end..];
        if after_tag.iter().all(|&b| b == b' ' || b == b'\t') {
            return 7;
        }
    }
    0
}

/// Find the end of a complete HTML open or close tag.
/// Returns Some(end_index) if a complete tag is found, None otherwise.
fn find_html_tag_end(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() { return None; }
    let start = if bytes[0] == b'/' { 1 } else { 0 };
    if start >= bytes.len() || !bytes[start].is_ascii_alphabetic() { return None; }
    let mut i = start + 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'-') {
        i += 1;
    }
    if i == start { return None; } // empty tag name
    // 闭合标签（`</a>`）——不允许属性（`</a href="foo">` 非法——官方
    // 判段落）——标签名后直接扫描到 `>`
    if bytes[0] == b'/' {
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'>' { return Some(i + 1); }
        return None;
    }
    // Found end of tag name; scan attributes to closing `>`.
    // type 7 属性（官方例 20/30/162/164——完整开标签含属性——commonmark.js
    // 参考实现同构的属性语法:
    //   attr = 空白 + (name | name=value)——属性前必须空白（标签名后/
    //   属性间——`<a href='b'title>` 无空白分隔——非法）
    //   name = [a-zA-Z_:][a-zA-Z0-9_.:-]*（`:` 合法——`<http://auto>` 的
    //   `http:` 后 `//` 非合法属性——type 7 失败——autolink 不误判）
    //   value = 引号串（含 >）| 无引号串（到空白/>）
    //   `/` 仅自闭合（`<a/>`——`/` 后必须 `>`）
    loop {
        let ws_start = i;
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        let skipped = i - ws_start;
        if i >= bytes.len() { return None; } // 未闭合
        if bytes[i] == b'>' { return Some(i + 1); }
        if bytes[i] == b'/' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'>' { return Some(i + 2); }
            return None; // `/` 后非 `>`——非法
        }
        if skipped == 0 { return None; } // 属性前必须空白（属性间无空白——非法）
        // 属性名
        if !(bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' || bytes[i] == b':') {
            return None;
        }
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_alphanumeric()
            || matches!(bytes[i], b'_' | b'.' | b':' | b'-'))
        {
            i += 1;
        }
        // 可选 `= value`
        let mut j = i;
        while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') { j += 1; }
        if j < bytes.len() && bytes[j] == b'=' {
            j += 1;
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r') { j += 1; }
            if j >= bytes.len() { return None; }
            match bytes[j] {
                b'"' | b'\'' => {
                    let q = bytes[j];
                    j += 1;
                    while j < bytes.len() && bytes[j] != q { j += 1; }
                    if j >= bytes.len() { return None; } // 引号未闭合
                    j += 1;
                }
                _ => {
                    while j < bytes.len() && !matches!(bytes[j], b' ' | b'\t' | b'\n' | b'\r' | b'>') {
                        j += 1;
                    }
                }
            }
            i = j;
        } else {
            i = j; // 无值——属性名后直接下一个属性/闭合
        }
    }
}

/// Check if `bytes` starts with one of the CommonMark §4.6 type 6 tag names
/// (case-insensitive), followed by a valid separator.
fn matches_html_block_tag(bytes: &[u8]) -> bool {
    const TAGS: &[&[u8]] = &[
        b"address", b"article", b"aside", b"base", b"basefont", b"blockquote",
        b"body", b"caption", b"center", b"col", b"colgroup", b"dd", b"details",
        b"dialog", b"dir", b"div", b"dl", b"dt", b"fieldset", b"figcaption",
        b"figure", b"footer", b"form", b"frame", b"frameset",
        b"h1", b"h2", b"h3", b"h4", b"h5", b"h6",
        b"head", b"header", b"hr", b"html", b"iframe", b"legend", b"li", b"link",
        b"main", b"menu", b"menuitem", b"nav", b"noframes", b"ol", b"optgroup",
        b"option", b"p", b"param", b"search", b"section", b"summary",
        b"table", b"tbody", b"td", b"tfoot", b"th", b"thead", b"title", b"tr",
        b"track", b"ul",
    ];
    for tag in TAGS {
        if bytes.len() >= tag.len() && bytes[..tag.len()].eq_ignore_ascii_case(tag) {
            if bytes.len() == tag.len() {
                return true;
            }
            let sep = bytes[tag.len()];
            if matches!(sep, b' ' | b'\t' | b'>') {
                return true;
            }
            if sep == b'/' && bytes.len() > tag.len() + 1 && bytes[tag.len() + 1] == b'>' {
                return true;
            }
        }
    }
    false
}


// FrameKind — what kind of container


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    BlockQuote,     // 0 — > prefix；lazy continuation 由 prev 词法判定；body = 内容列（打断偏移）
    BulletList,     // 1 — -/*/+ items
    OrderedList,    // 2 — 1./1) items
    CodeFence,      // 3 — ``` or ~~~；body encodes fence_char + fence_len
    HtmlBlock,      // 4 — HTML block type 1-7; body bits5-3 store block_type
    Table,          // 5 — GFM table
    Comment,        // 6 — <!-- ... -->
    HtmlElement,    // 7 — <div> etc.; indent+body encode push_line low 12 bits
    RefDef,         // 8 — [label]: url "title"
    TableRow,       // 9 — GFM table data row; body unused (always 0)
}

impl FrameKind {
    /// 判别值 → 变体（`transmute` 的安全替代）。
    ///
    /// `BlockFrame::kind` 与 blob 解码等「整数 → 枚举」面必须走这里：
    /// 10 个合法变体（0..=9；`TableRow` 最大）之外的 u8 返回 `None`。
    #[inline]
    pub fn from_discriminant(v: u8) -> Option<Self> {
        Some(match v {
            0 => FrameKind::BlockQuote,
            1 => FrameKind::BulletList,
            2 => FrameKind::OrderedList,
            3 => FrameKind::CodeFence,
            4 => FrameKind::HtmlBlock,
            5 => FrameKind::Table,
            6 => FrameKind::Comment,
            7 => FrameKind::HtmlElement,
            8 => FrameKind::RefDef,
            9 => FrameKind::TableRow,
            _ => return None,
        })
    }
}

/// 判别式 ↔ 枚举的安全性钉：旧实现把「掩码范围（0..=63 / 0..=15）」当成
/// 「枚举变体数（27 / 10）」直接 transmute，外部字节（blob）可构造非法值 = UB。
#[cfg(test)]
mod discriminant_pin {
    use super::*;

    /// 27 个 BlockType 变体逐个 roundtrip（pack / pack_with_cont / node_type）。
    #[test]
    fn block_type_discriminants_roundtrip() {
        for v in 0..=26u8 {
            let t = BlockType::from_discriminant(v).unwrap_or_else(|| panic!("discriminant {v} must be valid"));
            assert_eq!(t as u8, v);
            assert_eq!(PackedLineInfo::pack(t).node_type(), t);
            let with_cont = PackedLineInfo::pack_with_cont(t, true);
            assert!(with_cont.cont(), "cont bit must be set for {t:?}");
            assert_eq!(with_cont.node_type(), t, "cont bit must not disturb node_type");
        }
        for invalid in [27u8, 28, 63] {
            assert!(BlockType::from_discriminant(invalid).is_none(), "{invalid} must be rejected");
        }
    }

    /// 非法字节在 node_type() 上 fail-fast panic（safe），而不是 transmute UB。
    #[test]
    #[should_panic(expected = "invalid BlockType discriminant 31")]
    fn node_type_panics_on_invalid_discriminant() {
        let _ = PackedLineInfo(0x1F).node_type();
    }

    /// 0xFF 哨兵没有类型：调用方必须先 is_valid()（prev_col 等列允许哨兵）。
    #[test]
    #[should_panic(expected = "invalid BlockType discriminant 63")]
    fn node_type_panics_on_invalid_sentinel() {
        let _ = PackedLineInfo::invalid().node_type();
    }

    /// 10 个 FrameKind 变体逐个 roundtrip（new / kind / from_discriminant）。
    #[test]
    fn frame_kind_discriminants_roundtrip() {
        for v in 0..=9u8 {
            let k = FrameKind::from_discriminant(v).unwrap_or_else(|| panic!("discriminant {v} must be valid"));
            assert_eq!(k as u8, v);
            let f = BlockFrame::new(k, 7, 42);
            assert_eq!(f.kind(), k);
            assert_eq!(f.indent(), 7);
            assert_eq!(f.body(), 42);
            // kind 段占用 frame 的 bits15-12，其余位域解码不得回写到 kind。
            assert_eq!(((f.0 >> 12) & 0xF) as u8, v);
        }
        for invalid in [10u8, 11, 15] {
            assert!(FrameKind::from_discriminant(invalid).is_none(), "{invalid} must be rejected");
        }
    }

    /// 非法 kind nibble 在 kind() 上 fail-fast panic（safe），而不是 transmute UB。
    #[test]
    #[should_panic(expected = "invalid FrameKind discriminant 10")]
    fn block_frame_kind_panics_on_invalid_nibble() {
        let _ = BlockFrame(0xA000).kind();
    }
}


// BlockFrame(u16) — definition-is-storage


// Body bit constants (positions within bits5-0 of u16)
// pub(crate)（effect.rs BodyLayout mask 由此派生——位位置单源——
// 防掩码与位编码漂移）
pub const LOOSE_BIT: u16 = 0x0008; // body bit3 — BLT/OL shared loose flag
pub(crate) const CF_FENCE_BIT: u16 = 0x0020; // body bit5 — fence_char (0=`, 1=~)
pub(crate) const CF_LEN_MASK: u16 = 0x001F; // body bits4-0 — fence_len (stored as len-3)
pub(crate) const HTML_TYPE_MASK_NEW: u16 = 0x0038; // body bits5-3 — HtmlBlock block_type
const HTML_TYPE_SHIFT: u16 = 3;
pub(crate) const BLT_BULLET_MASK: u16 = 0x0030; // body bits5-4 — BLT bullet marker encoding
const BLT_BULLET_SHIFT: u16 = 4;
/// BLT body bit0（bits5-4 bullet/bit3 loose 之外的空闲位）——checklist 帧标记。
/// 用途：list_end_guard 需要区分 checklist 的 content 阈值（marker 后 `[ ] `）
/// 与 bullet 的 content 阈值——二者 anchor 比较基准不同。
pub(crate) const BLT_CHECKLIST_BIT: u16 = 0x0001;
pub(crate) const OL_DELIM_BIT: u16 = 0x0020; // body bit5 — OL delimiter (0=., 1=))

/// BlockFrame — 单行容器的判定参数，定义即存储。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockFrame(pub u16);

impl BlockFrame {
    // ── u16 位域解码 ──

    #[inline]
    pub fn kind(self) -> FrameKind {
        let raw = ((self.0 >> 12) & 0xF) as u8;
        FrameKind::from_discriminant(raw).unwrap_or_else(|| {
            panic!(
                "BlockFrame::kind: invalid FrameKind discriminant {raw} \
                 (raw frame {:#06x}; valid 0..=9)", self.0
            )
        })
    }

    #[inline]
    pub fn indent(self) -> u8 {
        ((self.0 >> 6) & 0x3F) as u8
    }

    #[inline]
    pub fn body(self) -> u8 {
        (self.0 & 0x3F) as u8
    }

    /// Alias: spec = body (per-kind decode).
    #[inline]
    pub fn spec(self) -> u8 {
        self.body()
    }

    // ── Per-kind body 解码 ──

    /// BLT/OL shared: is_loose flag at body bit3.
    #[inline]
    pub fn loose(self) -> bool {
        self.0 & LOOSE_BIT != 0
    }

    /// CF: fence_char at body bit5 (0=`, 1=~).
    #[inline]
    pub fn fence_char(self) -> u8 {
        if self.0 & CF_FENCE_BIT != 0 { b'~' } else { b'`' }
    }

    /// CF: fence_len at body bits4-0, +3 offset. Range 3-34.
    #[inline]
    pub fn fence_len(self) -> u8 {
        (self.0 & CF_LEN_MASK) as u8 + 3
    }

    /// Html: block_type at body bits5-3.
    #[inline]
    pub fn block_type(self) -> u8 {
        ((self.0 & HTML_TYPE_MASK_NEW) >> HTML_TYPE_SHIFT) as u8
    }

    // ── Per-kind body 编码 (用于 scanner) ──

    /// BLT bullet marker: 0=-, 1=*, 2=+.
    #[inline]
    pub fn bullet_code(self) -> u8 {
        ((self.0 & BLT_BULLET_MASK) >> BLT_BULLET_SHIFT) as u8
    }

    /// BLT checklist 帧（body bit0——checklist 与普通 bullet 同 kind）。
    #[inline]
    pub fn is_checklist_item(self) -> bool {
        self.kind() == FrameKind::BulletList && self.0 & BLT_CHECKLIST_BIT != 0
    }

    /// OL delimiter: 0=., 1=).
    #[inline]
    pub fn delim_code(self) -> u8 {
        if self.0 & OL_DELIM_BIT != 0 { 1 } else { 0 }
    }

    // ── 构造 ──

    /// Construct a Push frame (bit15=0) from kind + indent + body.
    #[inline]
    pub fn new(kind: FrameKind, indent: u8, body: u8) -> Self {
        let raw = ((kind as u16) << 12)
            | (((indent & 0x3F) as u16) << 6)
            | (body & 0x3F) as u16;
        BlockFrame(raw)
    }

    /// Build body byte for BLT: bullet_code@bits5-4.
    #[inline]
    pub fn encode_blt_body(bullet_char: u8) -> u8 {
        let code: u8 = match bullet_char {
            b'-' => 0,
            b'*' => 1,
            b'+' => 2,
            _ => 0,
        };
        (code << BLT_BULLET_SHIFT as u8) & 0x3F
    }

    /// BLT checklist body：bullet_code + BLT_CHECKLIST_BIT。
    #[inline]
    pub fn encode_ckl_body(bullet_char: u8) -> u8 {
        Self::encode_blt_body(bullet_char) | BLT_CHECKLIST_BIT as u8
    }

    /// Build body byte for OL: delim_code@bit5.
    #[inline]
    pub fn encode_ol_body(delim: u8) -> u8 {
        let code: u8 = if delim == b')' { 1 } else { 0 };
        (code << 5) & 0x3F
    }

    /// Build body byte for CF: fence_code@bit5, len-3@bits4-0.
    #[inline]
    pub fn encode_cf_body(fence_char: u8, len: u8) -> u8 {
        let fc: u8 = if fence_char == b'~' { 1 } else { 0 };
        ((fc << 5) | ((len.wrapping_sub(3)) & 0x1F)) & 0x3F
    }

    /// Build body byte for Html: block_type@bits5-3.
    #[inline]
    pub fn encode_html_body(block_type: u8) -> u8 {
        ((block_type & 0x07) << HTML_TYPE_SHIFT as u8) & 0x3F
    }

    /// Build body byte for Comment (spare).
    #[inline]
    pub fn encode_spare_body() -> u8 {
        0
    }

    /// Build body byte for Table: header_cols in bits5-1, bit0=0 (tentative).
    #[inline]
    pub fn encode_table_body(header_cols: u8) -> u8 {
        ((header_cols & 0x1F) << 1) & 0x3F
    }
}


// ScanAction — what the scanner tells the caller to do with the stack


/// Primary action within a Composite. Independent enum to avoid
/// `ScanAction` self-referencing (which would cause infinite-recursive type).
#[derive(Debug, Clone, Copy)]
pub enum CompositePrimary {
    Continue,
    ContinueAs(BlockFrame),
    Push(BlockFrame),
    PopPush(BlockFrame),
    Pop,
}

/// Returned by scan_block alongside the BlockType.
/// Describes the stack mutation to apply after classifying this line.
#[derive(Debug, Clone)]
pub enum ScanAction {
    /// No stack change. The line belongs to the current top frame (if any).
    Continue,
    /// Like Continue, but replaces the top frame with a modified version.
    /// Used when a line is absorbed by the current container but changes
    /// its state (e.g., LOOSE_BIT flip for list blank lines).
    ContinueAs(BlockFrame),
    /// Push a new frame onto the stack. The line opens a new container.
    Push(BlockFrame),
    /// Pop the top frame from the stack. The container is closed.
    Pop,
    /// Pop the top frame and push a new one of the same kind.
    /// Used for list item transitions (close old item, open new item in same list).
    PopPush(BlockFrame),
    /// Multiple actions on one line: outer primary action + nested frame chain
    /// discovered by peeling container prefixes.
    Composite {
        outer_type: BlockType,
        primary:   CompositePrimary,
        nested:    Vec<BlockFrame>,
        /// 与 nested 平行的逐层 consume——开层位移字节；非层帧=0）。
        nested_consumes: Vec<usize>,
        /// primary 是 Push/PopPush 时的开层位移字节（primary=Continue/其他=0）。
        primary_consume: usize,
    },
}


// FrameLog — persistent, sparse log of container boundaries (u64 entries)


/// Persistent sparse log of container-boundary events (Push/Pop/PopPush/body_mut).
///
/// Entry u64 layout: [pop@63][body_mut@62][consume@61-56][line_idx@55-24][anchor@23-16][frame@15-0]
/// Push sets bit63=0 (push_list_entry); Pop sets bit63=1 (pop_entry);
/// body_mut sets bit62=1 (body_mut_entry).
///
/// Supports partition_point binary search for O(log n) stack recovery
/// during ripple editing.
#[derive(Debug, Clone, Default)]
pub struct FrameLog {
    pub entries: Vec<u64>,
}

impl FrameLog {
    pub fn new() -> Self {
        Self { entries: Vec::with_capacity(64) }
    }

    /// Number of entries with line < line_idx.
    pub fn prefix_len(&self, line_idx: u32) -> usize {
        self.entries.partition_point(|&e| entry_line(e) < line_idx)
    }

    /// Record a Push entry at this line.
    #[inline]
    pub fn record_push(&mut self, line_idx: u32, f: BlockFrame) {
        self.entries.push(push_entry(line_idx, f));
    }

    /// Record a Pop entry at this line (popped frame stored for reverse replay).
    #[inline]
    pub fn record_pop(&mut self, line_idx: u32, popped: BlockFrame) {
        self.entries.push(pop_entry(line_idx, popped));
    }

    /// Rebuild the container stack at the START of the given line.
    /// Replays all actions recorded at lines < line_idx.
    pub fn rebuild_stack(&self, line_idx: u32) -> Vec<BlockFrame> {
        let end = self.prefix_len(line_idx);
        let mut stack: Vec<BlockFrame> = Vec::new();
        forward_replay(&self.entries[..end], &mut stack);
        stack
    }

    /// Rebuild stack using partition_point (reusable slice).
    pub fn rebuild_stack_from(&self, slice: &[u64], line_idx: u32) -> Vec<BlockFrame> {
        let end = slice.partition_point(|&e| entry_line(e) < line_idx);
        let mut stack: Vec<BlockFrame> = Vec::new();
        forward_replay(&slice[..end], &mut stack);
        stack
    }

    /// 关闭行查询——**不按 `is_shell` 分派**：从 `start_line` 起，第一个把该 `kind`
    /// 的相对深度压回阈值的 Pop 处按 `effect::end_for_rule` 结算（PopPush 取 Pop 半）。
    /// ⇒ 对 shell kind（BLT/OL）给出的是**项级**尾，见
    /// `tests/unit/ripple_edge.rs::test_boundary_for_middle_list_item`（中间项 end＝下一个 PopPush 行）。
    /// 与 `infer::container_log_end` 的差别正在此条：后者对 shell kind **跳过** PopPush、给整容器尾。
    /// （原名 `boundary_for`：文档写 container、实现对 shell kind 给 item——名与文档都不符，故改名。）
    /// 关闭行查询——语义已迁 `infer::item_tail_boundary`（FL×PL 语义查询，
    /// 落点纪律）；本方法为薄委托，保留帧日志调用面。
    pub fn item_tail_boundary(&self, start_line: u32, kind: FrameKind, lines: &[PackedLineInfo]) -> Option<u32> {
        crate::infer::item_tail_boundary(&self.entries, start_line, kind, lines)
    }
}


// u64 entry encode/decode


/// Anchor 字段布局：列表身份（第一个 item 的 threshold）随容器动作流物化——
/// Push/PopPush entry 携带锚，重放直接恢复（无需 PopPush 判定推导）。
/// 0xFF = 无锚（非列表帧 / Pop / body_mut——重放不动锚）。
pub const ANCHOR_SHIFT: u32 = 16;
pub const ANCHOR_MASK: u64 = 0xFF;
pub const ANCHOR_NONE: u8 = 0xFF;
const ENTRY_LINE_SHIFT: u32 = 24;

/// consume 字段：开层位移——push 事件在其发生行上的游标位移
/// （该层消费字节数）。bits 61-56（6 位；line 段解码只读 32 位、构造器吃
/// u32 高位恒 0——本段原空闲）。非开层条目（Pop/body_mut）为 0。
pub const CONSUME_SHIFT: u32 = 56;
pub const CONSUME_MASK: u64 = 0x3F << CONSUME_SHIFT;

/// 解码 consume 位（字节）。
#[inline]
pub fn entry_consume(e: u64) -> usize {
    ((e >> CONSUME_SHIFT) & 0x3F) as usize
}

/// 写 consume 位（保 line/anchor/frame 不变）。
#[inline]
pub fn with_consume(e: u64, consume: usize) -> u64 {
    debug_assert!(consume <= 0x3F, "consume 超出 6 位（≤63）");
    (e & !CONSUME_MASK) | ((consume as u64) << CONSUME_SHIFT)
}

/// **consume 字段位宽钉**（红队反例①：语料最深仅 4 层、实测消费值远小于 32，
/// 故「掩码被收窄成 0x1F」这类改错在语料 A/B 与全部测试里都不可见）。
/// 这里直接钉死字段几何（移位 56、掩码 6 位）＋高位回代，收窄即红。
#[cfg(test)]
mod consume_field_pin {
    use super::{CONSUME_MASK, CONSUME_SHIFT, entry_consume, with_consume};

    #[test]
    fn consume_field_is_exactly_six_bits() {
        assert_eq!(CONSUME_SHIFT, 56, "consume 起始位必须是 56");
        assert_eq!(CONSUME_MASK, 0x3F << 56, "consume 字段必须 6 位（bits 61-56）");
    }

    #[test]
    fn consume_roundtrip_covers_field_top() {
        for v in [0usize, 1, 13, 14, 31, 32, 62, 63] {
            let e = with_consume(0, v);
            assert_eq!(entry_consume(e), v, "consume 高位回代失败（掩码被收窄？）：{v}");
        }
        // 相邻位段不得被污染：写 consume 后 line/anchor/frame 段原样。
        let base = 0x0000_1234_5678_9ABCu64 & !CONSUME_MASK;
        assert_eq!(with_consume(base, 63) & !CONSUME_MASK, base, "写 consume 污染了其它位段");
    }
}

/// 本行首层的 consume——开层位移字节＝前导空白＋marker 宽
/// （peel 的 consumed = ws + mark_end；BQ 另有 has_trailing）。
/// 非层行（CF/Table/HELEM/RefDef/段落等）=0。scan 的 Push/PopPush 写入点调用。
pub fn layer_consume_of(bytes: &[u8]) -> usize {
    let ws = skip_leading_whitespace(bytes);
    let ws_n = ws.as_ptr() as usize - bytes.as_ptr() as usize;
    match BLOCK_DISPATCH[ws.first().copied().unwrap_or(0) as usize] {
        5 => parse_bq(ws).map_or(0, |(t, _)| ws_n + 1 + t as usize),
        2 => {
            if thematic_break_matches(ws) { 0 }
            else { parse_bl(ws).map_or(0, |(_, me, _)| ws_n + me) }
        }
        6 => parse_ol(ws).map_or(0, |(_, _, me)| ws_n + me),
        _ => 0,
    }
}

/// 「旧 vs 新」原始条目的 consume 掩码比较——FL 未变＝accessor
/// 语义（与 ripple_unchanged 同款）；8 处比较点统一走本函数。
pub fn entries_eq_ignoring_consume(a: &[u64], b: &[u64]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| (x & !CONSUME_MASK) == (y & !CONSUME_MASK))
}

/// 本行首个 BLT/OL 层 push 的 (FrameKind, consume)——写面 marker_len 单源
/// BQ push 跳过（container_prefix 已剥 BQ）。无=None。
pub fn first_item_layer_push(entries: &[u64], line: u32) -> Option<(FrameKind, usize)> {
    for &e in entries {
        // Pop 条目（bit63）的 bit15 同为 0，会先于 PopPush 的 push 半段被
        // 命中，且 pop 不带 consume——必须先跳过，否则非首条 item 的
        // marker consume 恒 0（tag 写把整行当 content）。
        if entry_line(e) == line && !entry_is_pop(e) && (e & (1 << 15)) == 0 {
            let k = BlockFrame((e & 0xFFFF) as u16).kind();
            if k == FrameKind::BulletList || k == FrameKind::OrderedList {
                return Some((k, entry_consume(e)));
            }
        }
    }
    None
}

/// 帧外运行状态：HtmlElement 的 Push 记录开行行号，其余帧为 NONE 哨兵。
/// 单点维护（projection/block/sdl 共用），避免散落 `if FrameKind::HtmlElement`。
#[inline]
pub(crate) fn frame_open_line(frame: &BlockFrame, line: u32) -> u32 {
    if frame.kind() == FrameKind::HtmlElement {
        line
    } else {
        u32::MAX
    }
}

/// Construct a Push entry: line_idx in high bits, frame (bit15=0) in low bits.
/// anchor (bit16-23) = 列表锚（BLT/OL 第一个 item 的 threshold）；非列表 0xFF。
/// 列表帧的 Push/PopPush 携带锚（列表身份）——重放直接恢复。
#[inline]
pub fn push_list_entry(line: u32, f: BlockFrame, anchor: u8) -> u64 {
    ((line as u64) << ENTRY_LINE_SHIFT) | ((anchor as u64) << ANCHOR_SHIFT) | (f.0 as u64)
}

/// Construct a Push entry without anchor (anchor = 0xFF — 非列表帧 / 旧调用点)。
#[inline]
pub fn push_entry(line: u32, f: BlockFrame) -> u64 {
    push_list_entry(line, f, ANCHOR_NONE)
}

/// Construct a Pop entry: line_idx in high bits, frame in low bits, bit63=1.
#[inline]
pub fn pop_entry(line: u32, popped: BlockFrame) -> u64 {
    push_list_entry(line, popped, ANCHOR_NONE) | (1u64 << 63)
}

/// Body mutation flag (bit62): marks a FrameLog entry that updates the body bits
/// of the current stack-top frame without a Push or Pop. ContinueAs and Continue
/// (LOOSE_BIT) mutations that were previously invisible in the FrameLog are now
/// recorded so ripple stack rebuild sees correct runtime state.
pub const BODY_MUT_FLAG: u64 = 1 << 62;

/// Construct a body-mutation entry: records a new body value for the stack-top
/// frame at `line`. bit63=0 (not Pop), bit62=1 (body mutation).
/// `f` must be the frame AFTER the body modification (kind/indent should match
/// the Push entry; only body bits matter for replay).
#[inline]
pub fn body_mut_entry(line: u32, f: BlockFrame) -> u64 {
    push_list_entry(line, f, ANCHOR_NONE) | BODY_MUT_FLAG
}

/// Check if entry is a body-mutation (bit62=1).
#[inline]
pub fn entry_is_body_mut(e: u64) -> bool {
    e & BODY_MUT_FLAG != 0
}

/// Decode line_idx from entry.
#[inline]
pub fn entry_line(e: u64) -> u32 {
    (e >> ENTRY_LINE_SHIFT) as u32
}

/// Decode anchor field (bit16-23) from entry.
#[inline]
pub fn entry_anchor(e: u64) -> u8 {
    ((e >> ANCHOR_SHIFT) & ANCHOR_MASK) as u8
}

/// 列表锚坐标唯一生产：列表帧的锚 = 帧 threshold（第一个 item 的内容列）。
/// 非列表帧无锚。scan/ripple/SDL 共享同一词法生产。
#[inline]
pub(crate) fn list_anchor(f: &BlockFrame) -> Option<usize> {
    if crate::effect::is_list_container(f.kind()) {
        Some(f.indent() as usize)
    } else {
        None
    }
}

/// PopPush 后的锚坐标：同列表（kind 相同）延续列表身份；跨种类重建为
/// 新列表身份（新 anchor = 新帧 indent）。
#[inline]
pub(crate) fn next_list_anchor(
    f: &BlockFrame,
    popped_kind: Option<FrameKind>,
    prev_anchor: Option<usize>,
) -> Option<usize> {
    if crate::effect::is_list_container(f.kind()) {
        if popped_kind == Some(f.kind()) { prev_anchor } else { list_anchor(f) }
    } else {
        None
    }
}

/// 推导列：同种 item 相对锚缩进 ≤ STRUCT_INDENT_MAX → 列表延续。
/// list_end_guard 的唯一比较点（修点 12）。
/// checklist 帧的 indent 是 `[ ] ` 后内容阈值（≥CHECKLIST_MARK_END），
/// 比较锚点时回退 CHECKLIST_MARK_END 得到 marker 相对列。
#[inline]
pub(crate) fn anchor_continues(f: &BlockFrame, anchor: usize) -> bool {
    if f.is_checklist_item() {
        (f.indent() as usize) <= anchor + CHECKLIST_MARK_END + STRUCT_INDENT_MAX
    } else {
        f.indent() as usize <= anchor + STRUCT_INDENT_MAX
    }
}


/// Check if entry is a Pop (bit63=1).
#[inline]
pub fn entry_is_pop(e: u64) -> bool {
    e & (1u64 << 63) != 0
}

/// Decode BlockFrame from entry, clearing Pop flag.
/// Frame occupies low 16 bits; Pop flag is at bit63, not in frame.
#[inline]
pub fn entry_frame(e: u64) -> BlockFrame {
    BlockFrame((e & 0xFFFF) as u16)
}

/// PopPush 配对判定（5 处散写收敛为单源）: entries[ei]
/// 是 Pop，其相邻 entry 是同 line 的 Push（非 Pop 非 body_mut）且同 kind
/// → 配对。adm_equal=true 时另要求 adm 位相同（#318——列表身份——
/// LOOSE(out) 差异不破坏配对——process_entries_at_line 的 item_boundary
/// 判定用）；adm_equal=false（kind-only）供容器延续/事件配对语义
/// （container_log_end/expand_zone/item_tail_boundary/sdl composition_check——
/// LOOSE 差异的 PopPush 仍是容器延续——不影响配对）。
/// 注: hook.rs 是搜索式变体（any 查找 + body_adm_eq 事件分类——非相邻
/// 配对）——语义不同型——不纳入。
#[inline]
// （is_poppush_pair 已迁 infer——FL 原子语义谓词落点纪律）


// Forward / backward replay


/// Replay entries forward onto the stack.
pub fn forward_replay(log: &[u64], stack: &mut Vec<BlockFrame>) {
    for &e in log {
        if entry_is_body_mut(e) {
            let f = entry_frame(e);
            // #318：body_mut 可能指向非栈顶（resolve_pending_loose 置外层
            // 帧——`- b\n\n  [ref]: /url` 的 L3——栈=[BLT, rd]——置 BLT——
            // 无条件更新栈顶会污染 rd）——kind+indent 匹配最近帧
            if let Some(pos) = stack.iter().rposition(|g|
                g.kind() == f.kind() && g.indent() == f.indent())
            {
                stack[pos] = BlockFrame((stack[pos].0 & !0x003F) | (f.0 & 0x003F));
            }
        } else if entry_is_pop(e) {
            stack.pop();
        } else {
            stack.push(entry_frame(e));
        }
    }
}

/// forward_replay 的锚版本——同步维护锚深度映射（与 stack 一一对应）。
/// 锚直接从 entry 的 anchor 字段恢复（Push/PopPush entry 携带列表身份——
/// 第一个 item 的 threshold）——无需 PopPush 判定推导：重放结果 == 扫描时
/// 循环锚栈（entry 由锚栈驱动写入——同源——zone 一致）。
pub fn forward_replay_anchors(log: &[u64], stack: &mut Vec<BlockFrame>, anchors: &mut Vec<Option<usize>>) {
    let mut open_lines = Vec::with_capacity(stack.len());
    forward_replay_runtime(log, stack, anchors, &mut open_lines);
}

/// forward_replay 的运行时版本：同时恢复 open_lines（HtmlElement 开行行号）。
pub(crate) fn forward_replay_runtime(
    log: &[u64],
    stack: &mut Vec<BlockFrame>,
    anchors: &mut Vec<Option<usize>>,
    open_lines: &mut Vec<u32>,
) {
    for &e in log {
        fold_replay_entry(e, stack, anchors, open_lines);
    }
}

/// 单事件锚重放折叠——forward_replay_anchors 与投影层 ShadowReplay 游标
/// 的机械单源。body_mut 命中非栈顶（#318）时按 kind+indent 改写最近帧；
/// pop 弹栈/弹锚/弹开行；push 入栈并按 entry anchor 字段恢复列表身份，
/// HtmlElement 开行行号直接从 Push entry 的 line 字段恢复。
#[inline]
pub(crate) fn fold_replay_entry(
    e: u64,
    stack: &mut Vec<BlockFrame>,
    anchors: &mut Vec<Option<usize>>,
    open_lines: &mut Vec<u32>,
) {
    if entry_is_body_mut(e) {
        let f = entry_frame(e);
        // #318（同 forward_replay）：kind+indent 匹配——非栈顶帧的
        // body_mut（resolve_pending_loose 置外层）
        if let Some(pos) = stack.iter().rposition(|g|
            g.kind() == f.kind() && g.indent() == f.indent())
        {
            stack[pos] = BlockFrame((stack[pos].0 & !0x003F) | (f.0 & 0x003F));
        }
    } else if entry_is_pop(e) {
        stack.pop();
        anchors.pop();
        open_lines.pop();
    } else {
        let f = entry_frame(e);
        stack.push(f);
        let a = entry_anchor(e);
        anchors.push(if a == ANCHOR_NONE { None } else { Some(a as usize) });
        open_lines.push(frame_open_line(&f, entry_line(e)));
    }
}

/// Replay entries in reverse (undo forward replay).
/// Pop→Push, Push→Pop — stack returns to prior state.
/// Body-mutation entries are skipped: they don't affect stack depth,
/// and the previous body value is not stored (irreversible).
pub fn backward_replay(log: &[u64], stack: &mut Vec<BlockFrame>) {
    for &e in log.iter().rev() {
        if entry_is_body_mut(e) {
            continue;
        }
        if entry_is_pop(e) {
            stack.push(entry_frame(e));
        } else {
            stack.pop();
        }
    }
}

/// Rebuild stack at target_line by partitioning log and forward-replaying prefix.
pub fn rebuild_stack_at(log: &[u64], target_line: u32) -> Vec<BlockFrame> {
    let end = log.partition_point(|e| entry_line(*e) < target_line);
    let mut stack = Vec::new();
    forward_replay(&log[..end], &mut stack);
    stack
}


// LineFacts — single-pass line measurement


/// 行的内容关系——三值：空行／只有容器标记／有内容。
/// 由行事实推断，供生产、SDL、还原三面共用（规则共享、状态各持）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ContentRel {
    Blank,
    ContainerOnly,
    NonEmpty,
}

/// Row-level metrics computed once per `scan_block` call.
#[derive(Debug, Clone)]
pub(crate) struct LineFacts<'a> {
    raw: &'a [u8],
    pub(crate) is_blank: bool,
    pub(crate) indent_col: usize,
    first_char: u8,
    pub(crate) content: &'a [u8],
    is_structural: bool,
    /// 前一行的 PL（词法真相——scan_block 的 prev_line_type，Blank 归一化为
    /// None）。列表 classify 用它判"item 段落是否已关闭"（前一行是 Blank 或
    /// IC）——词法层信息（zone 一致），替代 frame 状态位。
    prev_type: Option<BlockType>,
}

impl<'a> LineFacts<'a> {
    /// 内容关系：空行／只有容器标记／有内容。与读面「剥容器前缀后是否
    /// 全空白」是**同一条规则**，但算在**看得见行事实的层** ⇒ 生产、SDL、还原三面
    /// 可共用，该事实不必当形参传、也不必在别处补载体。
    pub(crate) fn content_rel(&self) -> ContentRel {
        if self.is_blank {
            return ContentRel::Blank;
        }
        // 替换 ⑦（§36 顺序收尾）：事实层也由 `Peeled` 的余下部分得出——
        // 「内容为空」＝「把 BQ 层消费掉之后余下部分全空白」。至此 A 段 7/7：block.rs 内
        // 不再有「自己走一遍认 `>`」的调用点，全部经 `Peeled` 的过滤式余下部分。
        if peel_peeled(str_view(self.raw), 0)
            .rest_after_bq_keep_indent()
            .as_bytes()
            .iter()
            .all(|b| b.is_ascii_whitespace())
        {
            ContentRel::ContainerOnly
        } else {
            ContentRel::NonEmpty
        }
    }

    /// pub(crate)（sdl.rs 约束链比对输入构造）。
    pub(crate) fn from(bytes: &'a [u8]) -> Self {
        let mut i = 0;
        let mut col = 0;
        while i < bytes.len() {
            match bytes[i] {
                b' ' => { col += 1; i += 1; }
                b'\t' => { col += tab_width(col); i += 1; }
                0xC2 if i + 1 < bytes.len() && bytes[i + 1] == 0xA0 => { col += 1; i += 2; }
                _ => break,
            }
        }
        let content = &bytes[i..];
        let is_blank = content.is_empty();
        LineFacts {
            raw: bytes,
            is_blank,
            indent_col: col,
            first_char: content.first().copied().unwrap_or(0),
            content,
            is_structural: crate::effect::geo::is_structural(col),
            prev_type: None,
        }
    }

    /// pub(crate)（sdl.rs 豁免比对构造 prev 域——生产调用点（scan_block）不变）。
    pub(crate) fn set_prev_type(&mut self, t: Option<BlockType>) {
        self.prev_type = t;
    }

    /// pub(crate)（effect.rs 约束链 63 哨兵动态阈值——与 list_peel 同源）。
    pub(crate) fn prev_type(&self) -> Option<BlockType> {
        self.prev_type
    }

    /// pub(crate)（sdl.rs 文档级比对构造）。
    pub(crate) fn indent_col(&self) -> usize {
        self.indent_col
    }
}


// ScanCtx — structural context


/// Per-call structural context. Reconstructed on every `scan_block` invocation
/// (and every re-scan within the engine multi-pop loop) because `stack` changes.
#[derive(Debug, Clone)]
pub(crate) struct ScanCtx {
    prev_line_type: Option<BlockType>,
    is_paragraph_continuation: bool,
    is_document_level: bool,
    stack_depth: usize,
}

impl ScanCtx {
    pub(crate) fn from(stack: &[BlockFrame], prev_line_type: Option<BlockType>) -> Self {
        ScanCtx {
            prev_line_type,
            is_paragraph_continuation: prev_line_type == Some(BlockType::Paragraph),
            is_document_level: stack.is_empty(),
            stack_depth: stack.len(),
        }
    }
}


// Container-aware scanner


/// scan 运行时上下文：帧栈 + 与栈平行的开行行号栈 + 当前 Rope。
/// 开行行号用 `u32::MAX` 表示“无开行语义”（叶子/非 HtmlElement 帧）。
/// `rope` 为 None 仅用于旧 `scan_block` 测试入口——生产路径恒 Some。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanRuntime<'a> {
    pub stack: &'a [BlockFrame],
    pub open_lines: &'a [u32],
    pub rope: Option<&'a Rope>,
}

impl<'a> ScanRuntime<'a> {
    #[inline]
    pub(crate) fn top_open_line(&self) -> Option<u32> {
        self.open_lines.last().copied().filter(|&l| l != u32::MAX)
    }
}

/// Core scanning function. Uses a BlockFrame stack for container state tracking.
///
/// Returns (BlockType, ScanAction). If ScanAction is Pop, the caller should pop
/// the top frame and re-call with the new (shorter) stack — the line may match
/// a deeper frame.
///
/// `prev_line_type` is the BlockType of the previous non-blank line, or None
/// if this is the first line or the previous line was blank. Used for Hs
/// (setext) heading detection.
pub fn scan_block(bytes: &[u8], stack: &[BlockFrame], prev_line_type: Option<BlockType>) -> (BlockType, ScanAction) {
    scan_block_rt(bytes, stack, &[], None, prev_line_type)
}

/// 带运行时上下文的扫描入口（生产/SDL 使用）。`open_lines` 与 `stack` 同长。
// ── 决策面⑤ 表化：blank 规则 2 行＋入口三腿 3 行 ──
#[derive(Clone, Copy)]
enum BlankRule { IsBlank, HbPrefix }

const BLANK_RULES: [BlankRule; 2] = [
    BlankRule::IsBlank,  // B1：剥前导空白后空
    BlankRule::HbPrefix, // B2：入口扩展特判——hb 帧带前缀空行（`> ` 剥后空）
];

fn blank_holds(facts: &LineFacts, stack: &[BlockFrame]) -> bool {
    BLANK_RULES.iter().any(|r| match r {
        BlankRule::IsBlank => facts.is_blank,
        BlankRule::HbPrefix => stack.last().is_some_and(|f| f.kind() == FrameKind::HtmlBlock)
            && peel_lower_prefixes(facts.content, stack).0.is_empty(),
    })
}

#[derive(Clone, Copy)]
enum EntryLeg { Blank, Container, NewBlock }

/// 判别式索引数组（第一方覆盖）：下标＝blank<<1 | stack_nonempty——4 格全填
static ENTRY_TABLE: [EntryLeg; 4] = [
    EntryLeg::NewBlock,   // 0 blank=F∧栈空→开新块
    EntryLeg::Container,  // 1 blank=F∧栈非空→容器延续（未认领兜底 (Paragraph,Pop)）
    EntryLeg::Blank,      // 2 blank=T∧栈空
    EntryLeg::Blank,      // 3 blank=T∧栈非空（blank 判定优先——栈任意）
];

pub(crate) fn scan_block_rt(
    bytes: &[u8],
    stack: &[BlockFrame],
    open_lines: &[u32],
    rope: Option<&Rope>,
    prev_line_type: Option<BlockType>,
) -> (BlockType, ScanAction) {
    let rt = ScanRuntime { stack, open_lines, rope };
    let mut facts = {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("sc_facts");
        LineFacts::from(bytes)
    };
    facts.prev_type = prev_line_type;
    let ctx = ScanCtx::from(stack, prev_line_type);
    let blank = blank_holds(&facts, stack);
    let leg = ENTRY_TABLE[(blank as usize) << 1 | (!stack.is_empty() as usize)];
    match leg {
        EntryLeg::Blank => blank_action(&facts, stack),
        EntryLeg::Container => {
            if let Some(top) = stack.last() {
                let result = {
                    #[cfg(feature = "telemetry")]
                    let _g = crate::telemetry::deep_span("sc_decl");
                    crate::effect::evaluate_decl(crate::effect::decl_for(top.kind()), &facts, top, &rt)
                };
                if let Some((t, _a)) = result {
                    return (t, _a);
                }
            }
            (BlockType::Paragraph, ScanAction::Pop)
        }
        EntryLeg::NewBlock => {
            #[cfg(feature = "telemetry")]
            let _g = crate::telemetry::deep_span("sc_newblk");
            dispatch_new_block(&facts, &ctx)
        }
    }
}

/// 识别器 wrapper 产物：词法类型 + 剥 marker 后剩余 + 测量几何。
/// 字段全部有消费方；kind 仅 nesting kinds 为 Some。
#[derive(Debug, Clone, Copy)]
pub(crate) struct Markered<'a> {
    pub node_type: BlockType,
    pub after_marker: &'a [u8],
    /// marker 自身列宽（bullet=1；OL=digits+1；checklist=6；BQ=1；非列表 structural=0）。
    pub marker_col: usize,
    /// marker 后内容列（tab 展开，从 marker 起点起算；真空 item = EMPTY_ITEM_SENTINEL）。
    pub content_col: usize,
    /// 剥 marker（含必选空白）后 remainder 起点列（tab 展开）。
    pub marker_end_col: usize,
    pub frame_body: u8,
}

/// 剥 marker 后 remainder（保留额外缩进；链内继续解析用）。
/// 与 `after_container_prefix` 的差异是表驱动：BQ 吃可选 1 空白但保留其余；
/// list 吃 mark_end（已含必选空白）后不额外吞空白。
pub(crate) fn after_nested_marker(nt: BlockType, content: &[u8]) -> Option<&[u8]> {
    if nt == BlockType::Blockquote {
        strip(nt, content, StripMode::Exact)
    } else if crate::effect::block_decl(nt).is_list_item {
        strip(nt, content, StripMode::All)
    } else {
        strip(nt, content, StripMode::Exact)
    }
}

/// colon tag 分隔符宽度：`:` 后空格/tab → 2（`: `）；否则 1。
/// infer 与 format_payload 共用单源。
#[inline]
pub(crate) fn colon_sep_len(sep: u8, after: u8) -> usize {
    if sep == b':' && (after == b' ' || after == b'\t') { 2 } else { 1 }
}

/// 行级 BQ 前缀提取（写面体现单源——peel 全层、保留原宽；op/infer 共用——
/// 原 op.rs::bq_outer_prefix 的纯函数形态）。
pub(crate) fn bq_prefix_of_line(line: &str) -> String {
    // 第 1 步（只搬不改）：接收 `Peeled`——层与余下部分与原点一起到手，
    // 而不是像原先那样把余下部分丢掉（§29.1 的 4 个调用点里 3 个丢它）。
    let Peeled { layers, .. } = peel_peeled(line, 0);
    let mut prefix = String::new();
    for layer in layers {
        if layer.kind == FrameKind::BlockQuote {
            prefix.push_str(&line[layer.start..layer.end]);
        }
    }
    prefix
}

/// 逐行体现 payload（写面容器前缀策略——prefixed_line_text 的参数化：
/// inline_first=true 时首行内联（行中槽位——bullet 值/comment 标记间），
/// 后续行逐行加前缀；false 时全前缀。空行用无尾空格前缀；保留尾换行）。
pub(crate) fn embody_lines(prefix: &str, payload: &str, inline_first: bool) -> String {
    if payload.is_empty() {
        return String::new();
    }
    let had_trailing_nl = payload.ends_with('\n');
    let body = payload.trim_end_matches('\n');
    let blank_prefix = prefix.trim_end();
    let mut out = String::with_capacity(payload.len() + prefix.len() * 4);
    for (i, line) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        // 空首行伪影修复（写面收口计划第三步）：首行内联豁免必须
        // 先于空行判定——首行为空 = 载荷从新行开始，空行前缀应落在下一行，
        // 而不是内联拼进上一行文本（旧序产出 `> A>` 一类伪影）。
        if i == 0 && inline_first {
            out.push_str(line);
        } else if line.is_empty() {
            out.push_str(blank_prefix);
        } else {
            out.push_str(prefix);
            out.push_str(line);
        }
    }
    if had_trailing_nl {
        out.push('\n');
    }
    out
}

/// 识别器 wrapper：词法识别器族保持纯词法；此处内建
/// `rel_indent <= STRUCT_INDENT_MAX` 准入守卫（同线嵌套链统一 admit）。
/// 覆盖 BQ/fence/列表/ATX 标题——识别失败或缩进超限返回 None。
/// 升级版返回 marker 几何；nesting kinds（BQ/BLT/OL）的 after 保留额外缩进。
pub(crate) fn recognize_marker(bytes: &[u8], rel_indent: usize) -> Option<Markered<'_>> {
    if rel_indent > STRUCT_INDENT_MAX { return None; }
    let ws = skip_leading_whitespace(bytes);
    if ws.is_empty() { return None; }
    let node_type = dispatch_lut(ws);
    if matches!(node_type,
        BlockType::Blockquote | BlockType::Bullet | BlockType::Checklist
        | BlockType::OrderedListItem | BlockType::CodeFenceOpen
    ) || is_header_type(node_type)
    {
        if let Some(kind) = frame_kind(node_type) {
            if is_nesting_container(kind) {
                let m = measure(node_type, ws)?;
                let marker_col = match kind {
                    FrameKind::BulletList if node_type == BlockType::Checklist => CHECKLIST_MARK_END,
                    FrameKind::BulletList => 1,
                    FrameKind::OrderedList => {
                        let d = number_digits(ws);
                        if d > 0 && d < ws.len()
                            && (ws[d] == b'.' || ws[d] == b')')
                        { d + 1 } else { 1 }
                    }
                    _ => 1,
                };
                let after_marker = after_nested_marker(node_type, ws)?;
                return Some(Markered {
                    node_type,
                    after_marker,
                    marker_col,
                    content_col: m.indent_col,
                    marker_end_col: column_width(&ws[..ws.len() - after_marker.len()]),
                    frame_body: m.frame_body,
                });
            }
        }
        let after_marker = strip(node_type, ws, StripMode::Exact)?;
        Some(Markered {
            node_type,
            after_marker,
            marker_col: 0,
            content_col: 0,
            marker_end_col: 0,
            frame_body: 0,
        })
    } else {
        None
    }
}


// NestedResolver 纯函数


/// 下一行 prev_type 覆盖语义（等价现 projection prev_hint 三态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrevOverride {
    Default,
    Content(BlockType),
    Blank,
}

/// 一行的 prev 效应：投影循环读 prev_override。
/// 裁决（B3/C2 修正）：boundary 生产消费者＝0（复核实锤）、
/// ripple 读的是 ROW_FLAG_BOUNDARY 行标志非本字段、唯一读者＝SDL 比对——
/// 「ripple 专列」理由不成立（裁决注释作废）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineEffect {
    pub prev_override: PrevOverride,
}

/// fast path 与 nested path 共用的 prev/zone 效应单源。
/// nested path 不额外产生另一套规则：同一个 (t, action) 就得到同一效应。
// ── 决策面② 表化：键＝动作形状（识别层，单源），行＝结论（数据层）──

/// ② 行效应键提取单源（生产与 SDL 共用——从 (t, a) 只投影形状，不物化、不判业务）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActShape { Continue, Push, PopPush, Composite, Other }

#[derive(Debug, Clone, Copy)]
pub(crate) struct ActionFacts {
    pub shape: ActShape,
    pub outer: Option<BlockType>,
    pub nested_last: Option<FrameKind>,
    /// Push/PopPush/ContinueAs 的帧 kind（R5 的 ItemPrimary/ListPrimary 键）。
    pub primary: Option<FrameKind>,
}

impl ActionFacts {
    /// 键提取单源：生产与 SDL 同用（SDL 侧不再自带一份）。
    pub(crate) fn from_action(a: &ScanAction) -> ActionFacts {
        match a {
            ScanAction::Continue => ActionFacts { shape: ActShape::Continue, outer: None, nested_last: None, primary: None },
            ScanAction::ContinueAs(f) => ActionFacts { shape: ActShape::Other, outer: None, nested_last: None, primary: Some(f.kind()) },
            ScanAction::Push(f) => ActionFacts { shape: ActShape::Push, outer: None, nested_last: None, primary: Some(f.kind()) },
            ScanAction::Pop => ActionFacts { shape: ActShape::Other, outer: None, nested_last: None, primary: None },
            ScanAction::PopPush(f) => ActionFacts { shape: ActShape::PopPush, outer: None, nested_last: None, primary: Some(f.kind()) },
            ScanAction::Composite { outer_type, nested, .. } => ActionFacts {
                shape: ActShape::Composite,
                outer: Some(*outer_type),
                nested_last: nested.last().map(|f| f.kind()),
                primary: None,
            },
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PrevRule { Blank, Para, IfEmptyPara, IfEmptyInner, ItemPrimary, Default }

/// 键全 Option（None＝不限）；行序＝旧 match 序（R2 先于 R5a/b——Push∧t=BQ 交集；R3 先于 R4/R6）；
/// R8 默认行在表内（v2 去兜底铁律）。来源见各行。
struct LeRow {
    shape: Option<ActShape>,
    t_bq: Option<bool>,
    outer_bq: Option<bool>,
    outer_list: Option<bool>,
    last_list: Option<bool>,
    prev: PrevRule,
}

impl LeRow {
    fn matches(&self, t: BlockType, f: &ActionFacts) -> bool {
        let ok = |k: Option<bool>, v: bool| k != Some(!v);
        ok(self.t_bq, t == BlockType::Blockquote)
            && ok(self.outer_bq, f.outer == Some(BlockType::Blockquote))
            && ok(self.outer_list, f.outer.map_or(false, |o| crate::effect::block_decl(o).is_list_item))
            && ok(self.last_list, f.nested_last.map_or(false, |k| crate::effect::is_list_container(k)))
            && self.shape.map_or(true, |s| f.shape == s)
    }
}

const LINE_EFFECT_ROWS: [LeRow; 9] = [
    // R1 Continue∧t=BQ：`>` 裸空行段落关闭
    LeRow { shape: Some(ActShape::Continue), t_bq: Some(true), outer_bq: None, outer_list: None, last_list: None, prev: PrevRule::Blank },
    // R2 Push∧t=BQ：(d)(g) 反例单源点（Push 单态——PopPush 走 R5b）
    LeRow { shape: Some(ActShape::Push), t_bq: Some(true), outer_bq: None, outer_list: None, last_list: None, prev: PrevRule::IfEmptyPara },
    // R3 Composite∧outer=BQ：inner 语义不能动（官方例 238）
    LeRow { shape: Some(ActShape::Composite), t_bq: None, outer_bq: Some(true), outer_list: None, last_list: None, prev: PrevRule::IfEmptyInner },
    // R4 Composite∧outer 列表项∧last 列表容器：与 Push/PopPush 列表开行同构
    LeRow { shape: Some(ActShape::Composite), t_bq: None, outer_bq: None, outer_list: Some(true), last_list: Some(true), prev: PrevRule::Para },
    // R5a Push
    LeRow { shape: Some(ActShape::Push), t_bq: None, outer_bq: None, outer_list: None, last_list: None, prev: PrevRule::ItemPrimary },
    // R5b PopPush：同 R5a
    LeRow { shape: Some(ActShape::PopPush), t_bq: None, outer_bq: None, outer_list: None, last_list: None, prev: PrevRule::ItemPrimary },
    // R6 其他 Composite：同线多层扩 zone
    LeRow { shape: Some(ActShape::Composite), t_bq: None, outer_bq: None, outer_list: None, last_list: None, prev: PrevRule::Default },
    // R7 Continue∧非BQ（原 `_` 臂展开——第一方覆盖）
    LeRow { shape: Some(ActShape::Continue), t_bq: Some(false), outer_bq: None, outer_list: None, last_list: None, prev: PrevRule::Default },
    // R8 ContinueAs/Pop 等其余动作（原 `_` 臂展开——第一方覆盖）
    LeRow { shape: Some(ActShape::Other), t_bq: None, outer_bq: None, outer_list: None, last_list: None, prev: PrevRule::Default },
];

pub(crate) fn line_effect_from_action(t: BlockType, a: &ScanAction, content_empty: bool) -> LineEffect {
    let facts = ActionFacts::from_action(a);
    let row = LINE_EFFECT_ROWS.iter().find(|r| r.matches(t, &facts))
        .expect("② 未覆盖键组合＝方案缺陷（第一方覆盖）");
    let prev_override = match row.prev {
        PrevRule::Blank => PrevOverride::Blank,
        PrevRule::Para => PrevOverride::Content(BlockType::Paragraph),
        PrevRule::IfEmptyPara => {
            if content_empty { PrevOverride::Blank }
            else { PrevOverride::Content(BlockType::Paragraph) }
        }
        PrevRule::IfEmptyInner => {
            let inner = facts.nested_last
                .map(|k| if crate::effect::is_list_container(k) {
                    BlockType::Paragraph
                } else {
                    crate::effect::container_block_type(k)
                })
                .unwrap_or(BlockType::Paragraph);
            if content_empty { PrevOverride::Blank }
            else { PrevOverride::Content(inner) }
        }
        PrevRule::ItemPrimary => {
            let is_list_primary = facts.primary
                .map_or(false, |k| crate::effect::is_list_container(k));
            if is_list_primary && crate::effect::block_decl(t).is_list_item {
                PrevOverride::Content(BlockType::Paragraph)
            } else {
                PrevOverride::Default
            }
        }
        PrevRule::Default => PrevOverride::Default,
    };
    LineEffect { prev_override }
}

/// 同线链内首个列表内容是否为 first-block IC。
/// 等价旧 item_ic_override 的阈值，但只保留几何判定（thematic 优先级归 outcome）。
#[inline]
pub(crate) fn first_content_is_ic(m: &Markered<'_>) -> bool {
    m.content_col != EMPTY_ITEM_SENTINEL
        && m.content_col >= m.marker_col + LIST_IC_OFFSET
}

/// primary/链内列表帧阈值单源。
/// content_is_ic=true 时用 #274 例外（threshold = marker_col + 1）。
#[inline]
pub(crate) fn list_frame_threshold(peel_origin: usize, m: &Markered<'_>, content_is_ic: bool) -> usize {
    if m.content_col == EMPTY_ITEM_SENTINEL {
        EMPTY_ITEM_SENTINEL
    } else if content_is_ic {
        peel_origin + m.marker_col + 1
    } else {
        peel_origin + m.content_col
    }
}

/// 数行实际可解析的连续 BQ 前缀层数（`>> x` = 2）。
pub(crate) fn count_bq_prefixes(content: &[u8]) -> usize {
    let mut cur = skip_leading_whitespace(content);
    let mut n = 0usize;
    loop {
        let before = cur;
        cur = strip_bq_one(cur);
        if cur.as_ptr() == before.as_ptr() { break; }
        n += 1;
    }
    n
}

/// wl4：BQ 前缀的**列**校验——`>` 前缀存在 ≠ 列对得上。
/// 行内第 i 个 `>` 的列必须到达第 i 个 BQ 层的外层容器内容列；否则该行不属于
/// 这些 BQ 层（cmark 分界：外层 item 内容列 6 时，`    > x` 在 4 应关 BQ/关 item，
/// 在 6/8/10 续行）。仅当 `>` 个数 == 栈内 BQ 层数、且该 BQ 层下方无更外层 BQ
/// （item 内 BQ 的简单链）时校验；部分前缀行与嵌套 BQ 链保持旧语义
/// （列坐标系待几何归一化后统一）。
fn bq_prefix_columns_ok(facts: &LineFacts<'_>, stack: &[BlockFrame]) -> bool {
    let bq_idx: Vec<usize> = stack.iter().enumerate()
        .filter(|(_, f)| f.kind() == FrameKind::BlockQuote)
        .map(|(i, _)| i)
        .collect();
    if bq_idx.is_empty() { return true; }
    let mut markers: Vec<usize> = Vec::new();
    let mut cur = facts.content;
    let mut col = facts.indent_col;
    loop {
        let before = cur;
        let stripped = strip_bq_one(cur);
        if stripped.as_ptr() == before.as_ptr() { break; }
        let ws = skip_leading_whitespace(before);
        let ws_len = (ws.as_ptr() as usize) - (before.as_ptr() as usize);
        markers.push(col + column_width_from(&before[..ws_len], col));
        let removed = (stripped.as_ptr() as usize) - (before.as_ptr() as usize);
        col += column_width_from(&before[..removed], col);
        cur = stripped;
    }
    if markers.len() != bq_idx.len() { return true; }
    for (k, &fi) in bq_idx.iter().enumerate() {
        if stack[..fi].iter().any(|f| f.kind() == FrameKind::BlockQuote) {
            continue;
        }
        let bound = stack[..fi].iter().rev()
            .find(|f| crate::effect::is_list_container(f.kind()))
            .map(|f| f.indent() as usize);
        if let Some(b) = bound {
            if markers[k] < b { return false; }
        }
    }
    true
}

/// §3.5：BQ 前缀承载列表约束。约束帧 g 之上有 BQ 层，且本行携带的 `>`
/// 前缀覆盖 g 以下全部 BQ 层 + g 之上的全部 BQ 层时，g 的裸缩进约束由
/// BQ 层承载）。g 之上无 BQ → 旧语义（不豁免）。
pub(crate) fn bq_carries_list_constraint(
    g_idx: usize,
    stack: &[BlockFrame],
    facts: &LineFacts<'_>,
) -> bool {
    let bq_above = stack[g_idx + 1..].iter()
        .filter(|f| f.kind() == FrameKind::BlockQuote)
        .count();
    if bq_above == 0 { return false; }
    let bq_below = stack[..=g_idx].iter()
        .filter(|f| f.kind() == FrameKind::BlockQuote)
        .count();
    count_bq_prefixes(facts.content) >= bq_below + bq_above
}

/// 两种列坐标 cursor。raw = 非列表帧轴；peel = 列表帧轴。
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChainOrigin {
    pub raw: usize,
    pub peel: usize,
}

/// 嵌套链入口语义。
#[derive(Debug, Clone, Copy)]
pub(crate) enum ListPrimaryMode {
    /// list_outcome_pop_push：primary = PopPush(new_frame)。
    ItemPopPush,
    /// list_outcome_push_nested：primary = Push(new_frame)。
    ItemPushNested,
    /// list_outcome_push_cross：primary = Push(new_frame)，outer 恒 primary_kind。
    ItemPushCross,
    /// list_outcome_composite_push：primary = Continue，after_ws 本身是第一层。
    CompositeContinue,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ChainEntry {
    /// dispatch_new_block：primary = Push(first_frame)。
    FreshTop { first_type: BlockType, first_frame: BlockFrame, raw_origin: usize },
    /// BQ 已认领，剥完已开 BQ 前缀后的续行：primary = Continue。
    BqContinuation,
    /// 列表 outcome 已认领新 item：primary 由调用方持有。
    ListPrimary {
        mode: ListPrimaryMode,
        first_type: BlockType,
        primary_kind: FrameKind,
        origin: ChainOrigin,
        /// CompositeContinue 的 after_ws 已 skip：首层 rel 由调用方传入。
        initial_rel: Option<usize>,
        /// CompositeContinue 的首层 raw 帧轴（= effective_indent，不重复加 rel）。
        initial_raw_col: Option<usize>,
    },
}

/// 分派出口境况（line parsing state）——链构造可达集的闭枚举名。
/// 值域 = 4 构造点（2069/3990/4158/4197 区）的可达形态；产出 = scan_block_rt
/// 的（腿, 顶帧, scene）投影；payload（first_type/origin/rel/primary_kind）随名同行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lps {
    /// dispatch_new_block 首开：primary = Push(first_frame)。
    FreshTop,
    /// BQ 已认领、剥完已开 BQ 前缀后的续行：primary = Continue。
    BqContinuation,
    /// list_outcome_push_cross：outer 恒 primary_kind。
    ListCross,
    /// list_outcome_composite_push：after_ws 本身是第一层。
    ListCompositeContinue,
    /// list_outcome_pop_push / push_nested：item 边界/嵌套。
    ListPopNested,
}

/// 入口投影（→3 值）：final 律 6 格判别式的键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LpsEntry {
    FreshTop,
    BqContinuation,
    ListPrimary,
}

impl Lps {
    /// 入口投影（→3 值）。
    pub(crate) fn entry(self) -> LpsEntry {
        match self {
            Lps::FreshTop => LpsEntry::FreshTop,
            Lps::BqContinuation => LpsEntry::BqContinuation,
            Lps::ListCross | Lps::ListCompositeContinue | Lps::ListPopNested => LpsEntry::ListPrimary,
        }
    }
}

impl ChainEntry {
    /// LPS 派生（最小派生形态——ChainEntry 保留，消费点经 lps() 换键）。
    pub(crate) fn lps(&self) -> Lps {
        match self {
            ChainEntry::FreshTop { .. } => Lps::FreshTop,
            ChainEntry::BqContinuation => Lps::BqContinuation,
            ChainEntry::ListPrimary { mode, .. } => match mode {
                ListPrimaryMode::ItemPushCross => Lps::ListCross,
                ListPrimaryMode::CompositeContinue => Lps::ListCompositeContinue,
                ListPrimaryMode::ItemPopPush | ListPrimaryMode::ItemPushNested => Lps::ListPopNested,
            },
        }
    }
    /// 测试构造（SDL cfc 钉用——字段保持私有；本工具链不支持枚举字段可见性修饰）。
    #[cfg(test)]
    pub(crate) fn test_fresh_top(first_type: BlockType) -> ChainEntry {
        ChainEntry::FreshTop {
            first_type,
            first_frame: BlockFrame::new(FrameKind::BlockQuote, 0, 0),
            raw_origin: 0,
        }
    }
    #[cfg(test)]
    pub(crate) fn test_list_primary(
        mode: ListPrimaryMode,
        first_type: BlockType,
        primary_kind: FrameKind,
    ) -> ChainEntry {
        ChainEntry::ListPrimary {
            mode,
            first_type,
            primary_kind,
            origin: ChainOrigin { raw: 0, peel: 0 },
            initial_rel: None,
            initial_raw_col: None,
        }
    }
}

/// 唯一嵌套解析产物。
#[derive(Debug, Clone)]
pub(crate) struct NestedResolve {
    pub nested: SmallVec<[BlockFrame; 4]>,
    /// 与 nested 平行的逐层 consume（每层 raw.len()-after_marker.len()）。
    pub nested_consumes: SmallVec<[usize; 4]>,
    pub final_content: BlockType,
    pub outer_type: BlockType,
}

/// chain 内最终内容 reparse（entry × nested 是否为空，保留现行 setext/段落修饰）。
/// **调整：setext 下划线检查**——上一行是段落时，才把 `---`／`===` 认成 setext 下划线（**来源 CM §4.3** ✓）。
/// 从 `classify_final_content` 的 FreshTop 分支提出（与另两条调整同族、同摆放原则）。
pub(crate) fn setext_adjust(ct: BlockType, ws: &[u8], prev: Option<BlockType>) -> BlockType {
    if prev == Some(BlockType::Paragraph) {
        check_hssep(ct, ws)
    } else {
        ct
    }
}

// ── B7：CFC 行表（entry×nested_empty → 结论规则；行序＝旧判定序；R6 默认行 Lut）──

#[derive(Clone, Copy)]
enum CfRulePro { ShapeSetext, Lut, InContainer }

/// 判别式索引数组（第一方覆盖）：下标＝LpsEntry as usize * 2 + nested_empty as usize
/// （bool 轴：非空=0、空=1）——6 格全填、无兜底；越界＝panic＝缺陷可见。
static CFC_TABLE: [CfRulePro; 6] = [
    CfRulePro::Lut,          // 0 FreshTop×非空
    CfRulePro::ShapeSetext,  // 1 FreshTop×空
    CfRulePro::Lut,          // 2 BqContinuation×非空
    CfRulePro::InContainer,  // 3 BqContinuation×空
    CfRulePro::Lut,          // 4 ListPrimary×非空
    CfRulePro::Lut,          // 5 ListPrimary×空
];

pub(crate) fn classify_final_content(
    entry: ChainEntry,
    nested_empty: bool,
    ws: &[u8],
    rel: usize,
    facts: &LineFacts<'_>,
) -> BlockType {
    let cls = entry.lps().entry();
    let prev_para = matches!(facts.prev_type, Some(BlockType::Paragraph | BlockType::Blockquote));
    let rule = CFC_TABLE[cls as usize * 2 + nested_empty as usize];
    let t = match rule {
        // 现 dispatch_new_block fallback：classify_by_shape + setext 下划线。
        CfRulePro::ShapeSetext => setext_adjust(classify_by_shape(ws, rel), ws, facts.prev_type),
        CfRulePro::Lut => dispatch_lut(ws),
        CfRulePro::InContainer => classify_in_container(ws, rel, prev_para),
    };
    // 单 dash 下划线的统一后置（prev-aware）——覆盖 Lut/InC/
    // ShapeSetext 全部规则（Lut 路径的 "-" 从不到 SetextInC 步骤）。prev 是段落
    // → HsSep（折叠 prev_is_para 转 HsHead2）；prev 非段落（空列表项）→ Bullet。
    if ws.len() == 1 && ws[0] == b'-' && matches!(t, BlockType::Bullet | BlockType::Paragraph | BlockType::HsSep) {
        if prev_para { BlockType::HsSep } else { BlockType::Bullet }
    } else {
        t
    }
}

pub(crate) fn override_item_ic(eo: BlockType, first_item: Option<&Markered<'_>>, nested: &[BlockFrame]) -> BlockType {
    if let Some(m) = first_item {
        if nested.last().is_some_and(|f| {
            crate::effect::is_list_container(f.kind())
        }) && first_content_is_ic(m)
        {
            return BlockType::IndentedCode;
        }
    }
    eo
}

// ── B6：outer 归因行表（Lps×事实 → 结论规则；行序＝旧判定序）──
// 18 行＝旧 EO_ROWS[15]＋OUTER_TABLE[12 格] 折叠（outer 律；
// 等价性 18/18：ContainerName 恒 Blockquote 故 first_bq 行=BqName；
// IC∧last_bq 两格=Content 且必须先于 first_bq 行——原 12 格的 R1 先于 R2 已折进格值）。
// 承重行＝R1（IC∧last_bq→Content：原 12 格之格 6/7——摘掉打红 ic_reported_shapes_match_cm）。

#[derive(Clone, Copy)]
enum EoRule { Content, FirstType, BqName, ContentTypeOfLast, ContentTypeOfPrimary }

struct EoFacts {
    entry: LpsEntry,
    nested_empty: bool,
    mode: Option<Lps>,
    ic: bool,
    fc_para: bool,
    fc_blank: bool,
    frameless: bool,
    first_bq: bool,
    last_bq: bool,
}

struct EoRow {
    entry: Option<LpsEntry>,
    nested_empty: Option<bool>,
    mode: Option<Lps>,
    ic: Option<bool>,
    fc_para: Option<bool>,
    fc_blank: Option<bool>,
    frameless: Option<bool>,
    first_bq: Option<bool>,
    last_bq: Option<bool>,
    rule: EoRule,
    wrap_ic: bool,
}

impl EoRow {
    fn matches(&self, f: &EoFacts) -> bool {
        let ok = |k: Option<bool>, v: bool| k != Some(!v);
        self.entry.map_or(true, |e| e == f.entry)
            && ok(self.nested_empty, f.nested_empty)
            && self.mode.map_or(true, |m| f.mode == Some(m))
            && ok(self.ic, f.ic)
            && ok(self.fc_para, f.fc_para)
            && ok(self.fc_blank, f.fc_blank)
            && ok(self.frameless, f.frameless)
            && ok(self.first_bq, f.first_bq)
            && ok(self.last_bq, f.last_bq)
    }
}

/// 行序铁律：FreshTop∧非空 4 优先级行 R1 先于 R2 先于 R3；R5 先于 R6；R13 先于 R14。
/// 无兜底行（第一方覆盖——可达域已由 18 行显式覆盖，未命中=panic）。
const OUTER_ROWS: &[EoRow] = &[
    // FreshTop∧非空：4 优先级行（旧 EO R1＋OUTER_TABLE 12 格折叠）
    EoRow { entry: Some(LpsEntry::FreshTop), nested_empty: Some(false), mode: None, ic: Some(true), fc_para: None, fc_blank: None, frameless: None, first_bq: None, last_bq: Some(true), rule: EoRule::Content, wrap_ic: true },
    EoRow { entry: Some(LpsEntry::FreshTop), nested_empty: Some(false), mode: None, ic: None, fc_para: None, fc_blank: None, frameless: None, first_bq: Some(true), last_bq: None, rule: EoRule::BqName, wrap_ic: true },
    EoRow { entry: Some(LpsEntry::FreshTop), nested_empty: Some(false), mode: None, ic: None, fc_para: None, fc_blank: None, frameless: Some(true), first_bq: None, last_bq: Some(true), rule: EoRule::Content, wrap_ic: true },
    EoRow { entry: Some(LpsEntry::FreshTop), nested_empty: Some(false), mode: None, ic: None, fc_para: None, fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::ContentTypeOfLast, wrap_ic: true },
    // FreshTop∧空：3 行
    EoRow { entry: Some(LpsEntry::FreshTop), nested_empty: Some(true), mode: None, ic: None, fc_para: Some(false), fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::Content, wrap_ic: false },
    EoRow { entry: Some(LpsEntry::FreshTop), nested_empty: Some(true), mode: None, ic: None, fc_para: Some(true), fc_blank: None, frameless: None, first_bq: Some(true), last_bq: None, rule: EoRule::BqName, wrap_ic: false },
    EoRow { entry: Some(LpsEntry::FreshTop), nested_empty: Some(true), mode: None, ic: None, fc_para: Some(true), fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::FirstType, wrap_ic: false },
    // BqContinuation：3 行
    EoRow { entry: Some(LpsEntry::BqContinuation), nested_empty: Some(false), mode: None, ic: None, fc_para: None, fc_blank: None, frameless: Some(true), first_bq: None, last_bq: Some(true), rule: EoRule::Content, wrap_ic: true },
    EoRow { entry: Some(LpsEntry::BqContinuation), nested_empty: Some(false), mode: None, ic: None, fc_para: None, fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::ContentTypeOfLast, wrap_ic: true },
    EoRow { entry: Some(LpsEntry::BqContinuation), nested_empty: Some(true), mode: None, ic: None, fc_para: None, fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::Content, wrap_ic: false },
    // ListCross：1 行
    EoRow { entry: Some(LpsEntry::ListPrimary), nested_empty: None, mode: Some(Lps::ListCross), ic: None, fc_para: None, fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::ContentTypeOfPrimary, wrap_ic: false },
    // ListCompositeContinue：2 行
    EoRow { entry: Some(LpsEntry::ListPrimary), nested_empty: None, mode: Some(Lps::ListCompositeContinue), ic: None, fc_para: Some(false), fc_blank: None, frameless: Some(true), first_bq: None, last_bq: Some(true), rule: EoRule::Content, wrap_ic: false },
    EoRow { entry: Some(LpsEntry::ListPrimary), nested_empty: None, mode: Some(Lps::ListCompositeContinue), ic: None, fc_para: None, fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::FirstType, wrap_ic: false },
    // ListPopNested：5 行
    EoRow { entry: Some(LpsEntry::ListPrimary), nested_empty: Some(false), mode: Some(Lps::ListPopNested), ic: None, fc_para: Some(false), fc_blank: None, frameless: Some(true), first_bq: None, last_bq: Some(true), rule: EoRule::Content, wrap_ic: false },
    EoRow { entry: Some(LpsEntry::ListPrimary), nested_empty: Some(false), mode: Some(Lps::ListPopNested), ic: None, fc_para: None, fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::ContentTypeOfLast, wrap_ic: true },
    EoRow { entry: Some(LpsEntry::ListPrimary), nested_empty: Some(true), mode: Some(Lps::ListPopNested), ic: None, fc_para: None, fc_blank: Some(true), frameless: None, first_bq: None, last_bq: None, rule: EoRule::FirstType, wrap_ic: false },
    EoRow { entry: Some(LpsEntry::ListPrimary), nested_empty: Some(true), mode: Some(Lps::ListPopNested), ic: None, fc_para: Some(false), fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::Content, wrap_ic: false },
    EoRow { entry: Some(LpsEntry::ListPrimary), nested_empty: Some(true), mode: Some(Lps::ListPopNested), ic: None, fc_para: Some(true), fc_blank: None, frameless: None, first_bq: None, last_bq: None, rule: EoRule::FirstType, wrap_ic: false },
];

pub(crate) fn effective_outer(
    entry: ChainEntry,
    nested: &[BlockFrame],
    final_content: BlockType,
    first_item: Option<&Markered<'_>>,
) -> BlockType {
    // LPS 派生换键（cls/mode 来自出口名）；载荷字段仍由 ChainEntry payload 供给。
    let (cls, mode) = match entry.lps() {
        Lps::FreshTop => (LpsEntry::FreshTop, None),
        Lps::BqContinuation => (LpsEntry::BqContinuation, None),
        Lps::ListCross => (LpsEntry::ListPrimary, Some(Lps::ListCross)),
        Lps::ListCompositeContinue => (LpsEntry::ListPrimary, Some(Lps::ListCompositeContinue)),
        Lps::ListPopNested => (LpsEntry::ListPrimary, Some(Lps::ListPopNested)),
    };
    let (first_type, primary_kind) = match entry {
        ChainEntry::FreshTop { first_type, .. } => (first_type, FrameKind::BlockQuote),
        ChainEntry::BqContinuation => (BlockType::Paragraph, FrameKind::BlockQuote),
        ChainEntry::ListPrimary { first_type, primary_kind, .. } => (first_type, primary_kind),
    };
    let f = EoFacts {
        entry: cls,
        nested_empty: nested.is_empty(),
        mode,
        ic: final_content == BlockType::IndentedCode,
        fc_para: final_content == BlockType::Paragraph,
        fc_blank: final_content == BlockType::Blank,
        frameless: frame_kind(final_content).is_none(),
        first_bq: first_type == BlockType::Blockquote,
        last_bq: nested.last().is_some_and(|x| x.kind() == FrameKind::BlockQuote),
    };
    let row = OUTER_ROWS.iter().find(|r| r.matches(&f))
        .expect("B6 未覆盖键组合＝方案缺陷（第一方覆盖）");
    let eo = match row.rule {
        EoRule::Content => final_content,
        EoRule::FirstType => first_type,
        EoRule::BqName => BlockType::Blockquote,
        EoRule::ContentTypeOfLast => crate::effect::container_block_type(nested.last().unwrap().kind()),
        EoRule::ContentTypeOfPrimary => crate::effect::container_block_type(primary_kind),
    };
    if row.wrap_ic { override_item_ic(eo, first_item, nested) } else { eo }
}

/// 唯一嵌套解析：逐层 rel → skip_ws → dispatch → 建帧 → 推进；
/// 终止层只做一次 reparse。无递归调用。
// ── D2：循环终止表（事实→终止结论；行序=旧判定序——guard 先于 ws 先于 rel 先于 recognize）──
// 立项书修订（读码细化）：原键含 Lps×nested_empty——现码五值同律、终止与 nested 无关，
// 该两维不取；recognize 事实在 rel/ws 检查之后才可计算（单点表需预计算＝对 rel>3 行
// 多付一次 marker 解析——与 ⑥ 探寻「scan 相位 2-3→1」冲突）——故表只承载「事实→结论」
// 映射，检查点保持依赖序（四个终止点改读本表，第一方覆盖）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum TermFact { GuardHi, WsEmpty, RelHi, RecognizeNone }

static TERM_TABLE: &[(TermFact, BlockType)] = &[
    (TermFact::GuardHi, BlockType::Paragraph),
    (TermFact::WsEmpty, BlockType::Blank),
    (TermFact::RelHi, BlockType::IndentedCode),
    (TermFact::RecognizeNone, BlockType::IndentedCode),
];

#[inline]
fn term_finish(fact: TermFact) -> BlockType {
    TERM_TABLE.iter().find(|(f, _)| *f == fact)
        .expect("D2 未覆盖终止事实＝方案缺陷（第一方覆盖）")
        .1
}

pub(crate) fn resolve_nested_chain<'a>(
    remainder: &'a [u8],
    entry: ChainEntry,
    facts: &LineFacts<'_>,
) -> NestedResolve {
    let (origin, mut initial_rel, mut initial_raw_col) = match entry {
        ChainEntry::FreshTop { first_frame, raw_origin, .. } => (
            ChainOrigin {
                raw: raw_origin,
                peel: first_frame.indent() as usize,
            },
            None,
            None,
        ),
        ChainEntry::BqContinuation => (ChainOrigin { raw: 0, peel: 0 }, None, None),
        ChainEntry::ListPrimary { origin, initial_rel, initial_raw_col, .. } =>
            (origin, initial_rel, initial_raw_col),
    };
    let mut cursor = remainder;
    // 统一坐标恢复（B）：raw 轴单变量=累计列宽（face cut_cols 投影）、
    // cursor=累计字节宽（face cut_bytes 投影）；peel 轴退化为「最近列表帧
    // indent」派生（peel_col 闭包——帧记忆，非前缀簿记）。三轴坐标
    // （raw/cursor/peel）＝Face 两字段＋帧记忆（vision「raw/peel 游标
    // 退化为 face 派生量」的完整落地）。
    let mut raw_col = origin.raw;
    let peel_col = |nested: &SmallVec<[BlockFrame; 4]>| -> usize {
        nested
            .iter()
            .rev()
            .find(|f| crate::effect::is_list_container(f.kind()))
            .map(|f| f.indent() as usize)
            .unwrap_or(origin.peel)
    };
    let mut nested: SmallVec<[BlockFrame; 4]> = SmallVec::new();
    let mut nested_consumes: SmallVec<[usize; 4]> = SmallVec::new();
    let mut first_item: Option<Markered<'_>> = None;
    let mut final_content: Option<BlockType> = None;
    let mut guard = 0usize;

    loop {
        guard += 1;
        if guard > MAX_BLOCK_NESTING {
            final_content = Some(term_finish(TermFact::GuardHi));
            break;
        }
        let raw = cursor;
        let ws = skip_leading_whitespace(raw);
        // raw 的绝对起点是 raw_col——tab 展开必须从 raw_col
        // 续算（`- \t> x` 的 `\t` 从列 2 展开到 4，不能从 0 算成 4）。
        // 与旧 compute_indent_at(raw, raw_origin)-raw_origin 同律等价
        // （column_width_from 同 tab 律、ws 段只含空白——统一项恢复轮 1 已证）。
        let rel = initial_rel.take().unwrap_or_else(|| {
            column_width_from(&raw[..raw.len() - ws.len()], raw_col)
        });
        if ws.is_empty() {
            final_content = Some(term_finish(TermFact::WsEmpty));
            break;
        }
        // 统一 admit：同线嵌套下一层/首内容的窗口。
        if rel > STRUCT_INDENT_MAX {
            final_content = Some(term_finish(TermFact::RelHi));
            break;
        }
        let t = dispatch_lut(ws);
        match frame_kind(t) {
            Some(kind) if is_nesting_container(kind) => {
                let Some(m) = recognize_marker(raw, rel) else {
                    final_content = Some(term_finish(TermFact::RecognizeNone));
                    break;
                };
                if first_item.is_none() && crate::effect::is_list_container(kind) {
                    first_item = Some(m);
                }
                let frame = if kind == FrameKind::BlockQuote {
                    // BQ body = 本层内容列（marker 列 + 宽——m.marker_end_col 已含
                    // 可选空白；坐标随链原点）——lazy 打断偏移判定（classify_bq_scene
                    // / bq_lazy_candidate）取 Σ 栈内全部 BQ body 与行绝对缩进求偏移。
                    BlockFrame::new(kind, 0, (raw_col + rel + m.marker_end_col) as u8)
                } else {
                    let threshold = list_frame_threshold(peel_col(&nested), &m, first_content_is_ic(&m));
                    BlockFrame::new(kind, threshold as u8, m.frame_body)
                };
                nested.push(frame);
                // consume：层起点（含前导空白）到 marker 尾的字节宽
                // ——after_marker 是 raw 的连续后缀（recognize_marker 实测）。
                nested_consumes.push(raw.len() - m.after_marker.len());
                if crate::effect::is_list_container(kind) {
                    // raw 轴推进到 marker_end（cursor 实际位置）；
                    // 帧 indent 是列表续行阈值（peel 轴），marker 后额外空白
                    // 不能再次计入下一层的 raw 起点。
                    raw_col = raw_col + rel + m.marker_end_col;
                }
                cursor = m.after_marker;
                continue;
            }
            Some(_kind) => {
                // 非 nesting framed：按 raw 轴建帧后终止（现 dispatch_new_block 语义）。
                // CompositeContinue 首层 raw 轴 = effective_indent；其他层 =
                // 链起点 raw + 全部 BQ 层内容列之和（Σ body——BQ 链 raw_col 不推进，
                // 保 Σ-body = 最深内容列的求和语义）+ 本层前导空白 rel。
                let raw_col_k = initial_raw_col.take().unwrap_or_else(|| {
                    let first_bq = match entry {
                        ChainEntry::FreshTop { first_frame, .. }
                            if first_frame.kind() == FrameKind::BlockQuote =>
                        {
                            first_frame.body() as usize
                        }
                        _ => 0,
                    };
                    let nested_bq: usize = nested
                        .iter()
                        .filter(|f| f.kind() == FrameKind::BlockQuote)
                        .map(|f| f.body() as usize)
                        .sum();
                    raw_col + first_bq + nested_bq + rel
                });
                if let ScanAction::Push(nf) = container_action_for(t, ws, raw_col_k) {
                    nested.push(nf);
                    nested_consumes.push(0); // 非前缀层（CF/Table 等）——consume=0
                }
                final_content = Some(t);
                break;
            }
            None => {
                final_content = Some(classify_final_content(entry, nested.is_empty(), ws, rel, facts));
                break;
            }
        }
    }

    let final_content = final_content.unwrap_or(BlockType::Blank);
    let outer_type = effective_outer(entry, &nested, final_content, first_item.as_ref());
    NestedResolve { nested, nested_consumes, final_content, outer_type }
}

/// CF 闭合识别 wrapper：窗口守卫已由 classify_cf 的 fence_close_window
/// 完成——这里做纯词法闭合匹配（fence char/length/尾部空白）。
pub(crate) fn fence_close_recognized(content: &[u8], frame: &BlockFrame) -> bool {
    let trimmed = skip_leading_whitespace(content);
    if let Some((fc, n)) = parse_fence(trimmed) {
        fc == frame.fence_char()
            && n as usize >= frame.fence_len() as usize
            && trimmed[n as usize..].iter().all(|&b| b == b' ' || b == b'\t')
    } else {
        false
    }
}

/// Check that all bytes are either `ch` or trailing whitespace.
/// Rejects sequences with whitespace between chars (e.g., "= = =").
#[inline]
pub(crate) fn all_hs_char(bytes: &[u8], ch: u8) -> bool {
    let mut seen_char = false;
    let mut in_trailing_ws = false;
    for &b in bytes {
        if b == ch {
            if in_trailing_ws { return false; } // gap — space before more chars
            seen_char = true;
        } else if b == b' ' || b == b'\t' {
            in_trailing_ws = true;
        } else {
            return false;
        }
    }
    seen_char
}

/// setext 标题文本提取(共享单点——node_tag/extract_tag_str 同源,827 合并):
/// 取块原始字节,截断首个纯 `=`/`-` 行(下划线行)及之后;余下逐行剥
/// marker/BQ + trim 拼接。与 all_hs_char 同词法(纯 = 或纯 -,空行不认)。
pub(crate) fn setext_title_from_raw(raw: &str) -> String {
    let mut lines: Vec<&str> = raw.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        // 替换 ③（§36 顺序）：逐行取余下部分也统一经 `Peeled`（原先自带 as_bytes→strip→str_view 三步）。
        let stripped = peel_peeled(l, 0).rest_after_bq();
        let t = stripped.trim_end();
        if !t.is_empty() && t.chars().all(|c| c == '=' || c == '-') {
            lines.truncate(i);
            break;
        }
    }
    lines.iter()
        .map(|l| peel_peeled(l, 0).rest().trim())
        .collect::<Vec<_>>()
        .join("\n")
}


// extracted classifiers — reusable across top-level and container dispatch


/// Shape classification: indent + first-char LUT.
/// Context-independent — does not know about paragraph continuation,
/// OL number-1 rule, or HTML type 7. Those are applied separately.
/// pub(crate)（sdl.rs 消费点一致性性质测试）。
#[inline]
pub(crate) fn classify_by_shape(content: &[u8], indent_col: usize) -> BlockType {
    if crate::effect::geo::is_indented_code(indent_col) { return BlockType::IndentedCode; }
    dispatch_lut(content)
}

#[inline]
pub(crate) fn check_hssep(t: BlockType, content: &[u8]) -> BlockType {
    if t == BlockType::Paragraph && !content.is_empty() {
        if content[0] == b'=' && all_hs_char(content, b'=') { return BlockType::HsSep; }
        if content[0] == b'-' && all_hs_char(content, b'-') { return BlockType::HsSep; }
    }
    t
}

/// Paragraph modifiers（C′ 收编——列读组合：BLOCK_TYPE_EFFECTS.para_modifier
/// 选规则名，共享识别器 number_is_one/detect_html_block_type 已就位；可达域行为等价）。
/// Only applicable in top-level paragraph continuation context.
pub(crate) fn apply_para_modifiers(t: BlockType, content: &[u8], is_para_cont: bool) -> BlockType {
    if !is_para_cont { return t; }
    match crate::effect::block_type_effects(t).para_modifier {
        crate::effect::ParaModifier::None => t,
        crate::effect::ParaModifier::OlStartMustBe1 => {
            if !number_is_one(content) {
                BlockType::Paragraph
            } else {
                t
            }
        }
        crate::effect::ParaModifier::HtmlNot7 => {
            if detect_html_block_type(content) == 7 {
                BlockType::Paragraph
            } else {
                t
            }
        }
    }
}

/// + HsSep + para_modifiers。
/// 拒绝规则（CM §4.1 can/cannot interrupt）入表——interrupts_para 统一消费
/// （消除顶层手写与 lazy 表条目的双表达）。位置分两段：
///   早期（IC/RefDef——形态事实：IC 是 ≥4 缩进（setext 下划线 ≤3 缩进——
///     CM §4.2——`    ---` 是 lazy 文本非下划线）、RefDef 以 [ 开头（非 -/=
///     下划线形态）——拒绝后不进 setext 链）
///   晚期（空 item/number-1/type7——转 Paragraph 后继续 setext 链——`- ` 空
///     item 可被 check_hssep 吸收为下划线——CM §5.2 + §4.2）
/// setext 链（`---`/单 `-` 转换 + check_hssep）是"段落延续转换"（CM §4.3
/// setext 优先）——非打断规则——保持 classify 层。
/// pub(crate)（sdl.rs SDLScan 顶层分类共享）。
// ── 决策面④ 表化：步骤链＝数据（行集），步骤处理＝识别层 ──
#[derive(Clone, Copy)]
enum ClsStep { EarlyRejectTop, SetextTop, SetextInC, Interrupt, HsSep, Modifiers }

fn classify_steps(t: &mut BlockType, content: &[u8], indent_col: usize, steps: &[ClsStep]) {
    for &s in steps {
        match s {
            // S2：早期拒绝（顶层专属——早退语义：中止链；`    ---` 反例——IC 转 Paragraph 后
            // 若继续管线会被 HsSep 步再转 HsSep ✗——旧 return 语义保持）
            ClsStep::EarlyRejectTop => {
                if matches!(*t, BlockType::IndentedCode | BlockType::RefDef) {
                    *t = BlockType::Paragraph;
                    return;
                }
            }
            // S3（顶层版）：setext 两分支（CM §4.3——单 - 也是下划线）
            ClsStep::SetextTop => {
                if (*t == BlockType::ThematicBreak && !content.is_empty() && content[0] == b'-' && all_hs_char(content, b'-'))
                    || ((*t == BlockType::Bullet || *t == BlockType::ThematicBreak) && content.len() == 1 && content[0] == b'-')
                { *t = BlockType::Paragraph; }
            }
            // S3′（容器版）：TB∧全'-' 支（单 dash 支 = classify_final_content 的统一
            // 后置——prev-aware，覆盖 Lut/InC 全部规则）。
            ClsStep::SetextInC => {
                if *t == BlockType::ThematicBreak && !content.is_empty() && content[0] == b'-' && all_hs_char(content, b'-')
                { *t = BlockType::Paragraph; }
            }
            // S4：晚期拒绝（interrupt_rule 列已表化；Paragraph 跳过——setext 产物不再拒绝）
            ClsStep::Interrupt => {
                if *t != BlockType::Paragraph && !crate::effect::can_interrupt(*t, content, indent_col) { *t = BlockType::Paragraph; }
            }
            ClsStep::HsSep => { *t = check_hssep(*t, content); }
            ClsStep::Modifiers => { *t = apply_para_modifiers(*t, content, true); }
        }
    }
}

/// 行集：AT_TOP＝全链；IN_CONTAINER＝[S3′/S5/S6]（容器内无 IC 降级、无 interrupt 拒绝）
/// [L] 相位（纯行谓词——Knowable<LinePhase>）：步骤只读 content/indent/当前 t，
/// 不读结构视图；S4 已消费 BLOCK_TYPE_DECLS.interrupt_rule 列（无新表原则）。
const AT_TOP_STEPS: &[ClsStep] = &[
    ClsStep::EarlyRejectTop, ClsStep::SetextTop, ClsStep::Interrupt, ClsStep::HsSep, ClsStep::Modifiers,
];
const IN_CONTAINER_STEPS: &[ClsStep] = &[
    ClsStep::SetextInC, ClsStep::HsSep, ClsStep::Modifiers,
];

/// 行集选择表（第一方覆盖）：下标＝CtxCls 判别式——2 格全填
#[derive(Clone, Copy)]
enum CtxCls { Top, InContainer }

const STEPS_BY_CTX: [&[ClsStep]; 2] = [&AT_TOP_STEPS, &IN_CONTAINER_STEPS];

fn classify_with_steps(content: &[u8], indent_col: usize, is_para_cont: bool, ctx: CtxCls) -> BlockType {
    let t0 = classify_by_shape(content, indent_col);
    if !is_para_cont { return t0; }   // 管线级早退
    let mut t = t0;
    classify_steps(&mut t, content, indent_col, STEPS_BY_CTX[ctx as usize]);
    t
}

pub(crate) fn classify_at_top(content: &[u8], indent_col: usize, is_para_cont: bool) -> BlockType {
    classify_with_steps(content, indent_col, is_para_cont, CtxCls::Top)
}

/// In-container classification: shape + HsSep + para_modifiers.
/// No IndentedCode downgrade — containers accept indented code.
/// pub(crate)（sdl.rs SDLScan 共享）。
pub(crate) fn classify_in_container(content: &[u8], indent_col: usize, is_para_cont: bool) -> BlockType {
    classify_with_steps(content, indent_col, is_para_cont, CtxCls::InContainer)
}

/// New-block dispatch. Called when no container claims the line
/// (stack empty or after Pop cleared all non-claiming containers).
/// pub(crate)（sdl.rs SDLScan 共享）。
pub(crate) fn dispatch_new_block(facts: &LineFacts, ctx: &ScanCtx) -> (BlockType, ScanAction) {
    let t = classify_at_top(facts.content, facts.indent_col, ctx.is_paragraph_continuation);
    // HsSep returns Continue — walk_blocks forward-look handles Paragraph→HsHead.
    if t == BlockType::HsSep {
        return (BlockType::HsSep, ScanAction::Continue);
    }
    let action = container_action_for(t, facts.content, facts.indent_col);
    // 嵌套链统一走 NestedResolver。
    // 入口 classify 与 primary frame 构建保留；旧 7 个 BQ 特判与两套坐标账删除。
    if let Some(kind) = frame_kind(t) {
        if is_nesting_container(kind) {
            if let ScanAction::Push(first_frame) = action {
                // marker 后随空格属于 marker；BQ 吃可选 1 空白、列表保留额外缩进。
                let after_first = if kind == FrameKind::BlockQuote {
                    strip_bq_one(facts.content)
                } else {
                    after_container_prefix(facts.content, kind)
                };
                if !skip_leading_whitespace(after_first).is_empty() {
                    // raw 轴 = remainder 起点列；BQ 内容坐标系从 0 起。
                    let raw_origin = if kind == FrameKind::BlockQuote {
                        0
                    } else {
                        recognize_marker(facts.content, 0)
                            .map(|m| facts.indent_col + m.marker_end_col)
                            .unwrap_or(first_frame.indent() as usize)
                    };
                    let entry = ChainEntry::FreshTop { first_type: t, first_frame, raw_origin };
                    let res = resolve_nested_chain(after_first, entry, facts);
                    if !res.nested.is_empty() {
                        let outer = res.outer_type;
                        return (outer, ScanAction::Composite {
                            outer_type: outer,
                            primary: CompositePrimary::Push(first_frame),
                            nested: res.nested.into_vec(),
                            nested_consumes: res.nested_consumes.into_vec(),
                            primary_consume: layer_consume_of(facts.content),
                        });
                    }
                    // 单层内容：resolver 的 final_content/outer_type 已含
                    // first-block IC 与 setext fallback 语义。
                    return (res.outer_type, ScanAction::Push(first_frame));
                }
                return (t, ScanAction::Push(first_frame));
            }
        }
    }
    (t, action)
}

/// Blank-line dispatch. Called when the line has no content
/// (either truly blank or all-indent with nothing after stripping).
/// pub(crate)（sdl.rs blank 组比对对象）。
// ── B8：blank 腿行表（kind 臂→(t, pop)；行序＝旧判定序；R8 默认行 Pop）──

struct BlankFacts {
    stack_empty: bool,
    kind: FrameKind,
    in_bq: bool,
    gt: bool,
    html_ge6: bool,
}

enum TBlank { BlankT, ContentTypeFor, HtmlContent }

struct BlankRow {
    stack_empty: Option<bool>,
    cf_comment: Option<bool>,
    html_block: Option<bool>,
    list_kind: Option<bool>,
    kind: Option<FrameKind>,
    in_bq: Option<bool>,
    gt: Option<bool>,
    html_ge6: Option<bool>,
    t_rule: TBlank,
    pop: bool,
}

impl BlankRow {
    fn matches(&self, f: &BlankFacts) -> bool {
        let ok = |k: Option<bool>, v: bool| k != Some(!v);
        ok(self.stack_empty, f.stack_empty)
            && ok(self.cf_comment, matches!(f.kind, FrameKind::CodeFence | FrameKind::Comment))
            && ok(self.html_block, f.kind == FrameKind::HtmlBlock)
            && ok(self.list_kind, crate::effect::is_list_container(f.kind))
            && self.kind.map_or(true, |k| k == f.kind)
            && ok(self.in_bq, f.in_bq)
            && ok(self.gt, f.gt)
            && ok(self.html_ge6, f.html_ge6)
    }
}

/// 行序铁律：R2 先于 R3（BQ 内无前缀 CF/Comment → Pop 链）；R4/R5 先于 R6；R8-R12 按 kind 显式（无兜底——第一方覆盖）。
const BLANK_ROWS: &[BlankRow] = &[
    // R1 栈空 → (Blank, Continue)
    BlankRow { stack_empty: Some(true), cf_comment: None, html_block: None, list_kind: None, kind: None, in_bq: None, gt: None, html_ge6: None, t_rule: TBlank::BlankT, pop: false },
    // R2 CF/Comment∧BQ 内∧无前缀 → (Blank, Pop)（空行结束容器链——CM：containing block 结束）
    BlankRow { stack_empty: None, cf_comment: Some(true), html_block: None, list_kind: None, kind: None, in_bq: Some(true), gt: Some(false), html_ge6: None, t_rule: TBlank::BlankT, pop: true },
    // R3 CF/Comment 其余 → (content_type_for, Continue)（顶层空行是内容）
    BlankRow { stack_empty: None, cf_comment: Some(true), html_block: None, list_kind: None, kind: None, in_bq: None, gt: None, html_ge6: None, t_rule: TBlank::ContentTypeFor, pop: false },
    // R4 HtmlBlock∧BQ 内∧无前缀 → (Blank, Pop)（与 CF/Comment 分支对称）
    BlankRow { stack_empty: None, cf_comment: None, html_block: Some(true), list_kind: None, kind: None, in_bq: Some(true), gt: Some(false), html_ge6: None, t_rule: TBlank::BlankT, pop: true },
    // R5 HtmlBlock∧type≥6 → (Blank, Pop)（官方例 2850：type6/7 结束空行不是块内容）
    BlankRow { stack_empty: None, cf_comment: None, html_block: Some(true), list_kind: None, kind: None, in_bq: None, gt: None, html_ge6: Some(true), t_rule: TBlank::BlankT, pop: true },
    // R6 HtmlBlock∧type<6 → (HtmlContent, Continue)（CDATA 官方例：空行是内容）
    BlankRow { stack_empty: None, cf_comment: None, html_block: Some(true), list_kind: None, kind: None, in_bq: None, gt: None, html_ge6: None, t_rule: TBlank::HtmlContent, pop: false },
    // R7 列表容器 → (Blank, Continue)（空行是列表内容——loose 检测）
    BlankRow { stack_empty: None, cf_comment: None, html_block: None, list_kind: Some(true), kind: None, in_bq: None, gt: None, html_ge6: None, t_rule: TBlank::BlankT, pop: false },
    // R8 BQ → (Blank, Pop)（空行结束 BQ）
    BlankRow { stack_empty: None, cf_comment: None, html_block: None, list_kind: None, kind: Some(FrameKind::BlockQuote), in_bq: None, gt: None, html_ge6: None, t_rule: TBlank::BlankT, pop: true },
    // R9 HtmlElement → (Blank, Pop)
    BlankRow { stack_empty: None, cf_comment: None, html_block: None, list_kind: None, kind: Some(FrameKind::HtmlElement), in_bq: None, gt: None, html_ge6: None, t_rule: TBlank::BlankT, pop: true },
    // R10 Table → (Blank, Pop)
    BlankRow { stack_empty: None, cf_comment: None, html_block: None, list_kind: None, kind: Some(FrameKind::Table), in_bq: None, gt: None, html_ge6: None, t_rule: TBlank::BlankT, pop: true },
    // R11 TableRow → (Blank, Pop)
    BlankRow { stack_empty: None, cf_comment: None, html_block: None, list_kind: None, kind: Some(FrameKind::TableRow), in_bq: None, gt: None, html_ge6: None, t_rule: TBlank::BlankT, pop: true },
    // R12 RefDef → (Blank, Pop)
    BlankRow { stack_empty: None, cf_comment: None, html_block: None, list_kind: None, kind: Some(FrameKind::RefDef), in_bq: None, gt: None, html_ge6: None, t_rule: TBlank::BlankT, pop: true },
];

#[inline]
pub(crate) fn blank_action(facts: &LineFacts, stack: &[BlockFrame]) -> (BlockType, ScanAction) {
    let top = stack.last();
    let (kind, in_bq, html_ge6) = match top {
        None => (FrameKind::BlockQuote, false, false), // 占位——stack_empty 先行
        Some(t) => (
            t.kind(),
            stack[..stack.len().saturating_sub(1)]
                .iter()
                .any(|f| f.kind() == FrameKind::BlockQuote),
            t.kind() == FrameKind::HtmlBlock && t.block_type() >= 6,
        ),
    };
    let f = BlankFacts {
        stack_empty: top.is_none(),
        kind,
        in_bq,
        gt: has_gt_prefix(facts.content),
        html_ge6,
    };
    let row = BLANK_ROWS.iter().find(|r| r.matches(&f))
        .expect("B8 未覆盖键组合＝方案缺陷（第一方覆盖）");
    let t = match row.t_rule {
        TBlank::BlankT => BlockType::Blank,
        TBlank::ContentTypeFor => content_type_for(kind),
        TBlank::HtmlContent => BlockType::HtmlContent,
    };
    let a = if row.pop { ScanAction::Pop } else { ScanAction::Continue };
    (t, a)
}

/// Map FrameKind to its content line type.
/// pub(crate)（sdl.rs SDLScan blank 吸收归属共享）。
#[inline]
pub(crate) fn content_type_for(kind: FrameKind) -> BlockType {
    match kind {
        FrameKind::CodeFence => BlockType::CodeContent,
        FrameKind::HtmlBlock => BlockType::HtmlContent,
        FrameKind::Comment => BlockType::Comment,
        _ => BlockType::Paragraph,
    }
}


/// 测量层唯一 tab 展开：tab 前进到下一个 TAB_STOP 倍数列。
/// 全仓 tab 列计算唯一函数（geo::TAB_STOP 值单一源）。
#[inline]
pub(crate) fn tab_width(col: usize) -> usize {
    TAB_STOP - (col % TAB_STOP)
}



/// Compute indent in virtual columns (TAB_STOP-aligned).
/// Handles ASCII space, tab, and common Unicode whitespace (NBSP, etc.).
#[inline]
pub(crate) fn compute_indent(bytes: &[u8]) -> usize {
    compute_indent_at(bytes, 0)
}

/// Compute indent in virtual columns starting from `start_col`（tab 展开的
/// 基准列）。与 compute_indent 的差异：marker 后的 tab 从 marker 列起算
/// （`-\t` 的 tab 从列 1 展开到 4——compute_indent 从 0 起算会算成 4→多 1——
/// Bug B：threshold/内容列错位）。
#[inline]
pub(crate) fn compute_indent_at(bytes: &[u8], start_col: usize) -> usize {
    let mut column = start_col;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' => { column += 1; i += 1; }
            b'\t' => { column += tab_width(column); i += 1; }
            // NBSP U+00A0
            0xC2 if i + 1 < bytes.len() && bytes[i + 1] == 0xA0 => { column += 1; i += 2; }
            _ => break,
        }
    }
    column
}

/// 任意前缀字节的列宽（tab 展开；非空白按 1 列）。
/// fence_indent_base / peel 层前缀列宽单源。
#[inline]
pub(crate) fn column_width(bytes: &[u8]) -> usize {
    column_width_from(bytes, 0)
}

/// 从 `start_col` 起算的前缀字节列宽（tab 停靠取决于绝对列——
/// 分段列宽不可分别从 0 相加；peel_lower_prefixes 逐段续算用）。
#[inline]
pub(crate) fn column_width_from(bytes: &[u8], start_col: usize) -> usize {
    let mut col = start_col;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\t' => { col += tab_width(col); i += 1; }
            0xC2 if i + 1 < bytes.len() && bytes[i + 1] == 0xA0 => { col += 1; i += 2; }
            _ => { col += 1; i += 1; }
        }
    }
    col - start_col
}

// ── Per-FrameKind continuation checks ──


// 容器承认判定 — 统一走广义效应表 (evaluate_decl)。
//
// 广义效应表 (CONTAINER_DECLS) 是唯一实现。

// ── 结论分类函数 (classify) — 运行时, 依赖解析 (函子性边界) ──

/// fence 闭合窗口推导列：闭合行缩进必须落在 [fi, fi + STRUCT_INDENT_MAX]。
/// classify_cf 字节/列混算修复的唯一比较点。
#[inline]
pub(crate) fn fence_close_window(line_indent: usize, fi: usize) -> bool {
    line_indent >= fi && line_indent <= fi + STRUCT_INDENT_MAX
}

/// CF classify（单级化）——内容行 vs 闭合 fence 直接产结论。
pub(crate) fn classify_cf(facts: &LineFacts, frame: &BlockFrame, rt: &ScanRuntime) -> Option<(BlockType, ScanAction)> {
    let stack = rt.stack;
    // BQ/列表下层约束（无 > 前缀/缩进不足 → 不属于 CF——CM §4.6 未闭合 fence
    // 由 containing block 结束闭合）已由 evaluate_decl 的约束链处理。
    let (content, peel_columns) = match peel_lower_prefixes_from(facts.content, stack, facts.indent_col) {
        LowerPrefixMatch::Ok { peeled, cols } => (peeled, cols),
        LowerPrefixMatch::Fail { .. } => peel_lower_prefixes_partial(facts.content, stack, facts.indent_col),
    };
    let has_container = stack.len() > 1;
    if !facts.is_structural {
        if !has_container { return Some((BlockType::CodeContent, ScanAction::Continue)); }
        // 全部使用列坐标——peel 偏移从 facts.indent_col 连续 tab 展开。
        let line_indent = facts.indent_col + peel_columns;
        let fi = frame.indent() as usize;
        if !fence_close_window(line_indent, fi) { return Some((BlockType::CodeContent, ScanAction::Continue)); }
    }
    let trimmed = skip_leading_whitespace(content);
    if fence_close_recognized(trimmed, frame) {
        Some((BlockType::CodeFenceClose, ScanAction::Pop))
    } else {
        Some((BlockType::CodeContent, ScanAction::Continue))
    }
}

/// Comment classify（单级化）——内容行 vs 含 --> 的结束行。
pub(crate) fn classify_comment(facts: &LineFacts, _frame: &BlockFrame, rt: &ScanRuntime) -> Option<(BlockType, ScanAction)> {
    let stack = rt.stack;
    let (content, _) = peel_lower_prefixes(facts.content, stack);
    if memchr::memmem::find(content, b"-->").is_some() {
        Some((BlockType::Comment, ScanAction::Pop))
    } else {
        Some((BlockType::Comment, ScanAction::Continue))
    }
}

/// Table admit — geometric guard: the row's peeled content column must fall
/// inside the table frame's content window (header column .. +3), matching the
/// CodeFence close-window convention; a leading `|` is then structural by
/// itself, no raw-line indent gate. This closes the list-inner BQ table gap
/// (raw indent 4 was rejected by the old is_structural <=3 gate).
fn table_window_indent(stack: &[BlockFrame]) -> usize {
    stack
        .iter()
        .rev()
        .find(|f| f.kind() == FrameKind::Table)
        .map(|f| f.indent() as usize)
        .unwrap_or(0)
}

pub(crate) fn admit_table(facts: &LineFacts, _frame: &BlockFrame, stack: &[BlockFrame]) -> bool {
    let (content, peel_columns) = match peel_lower_prefixes_from(facts.content, stack, facts.indent_col) {
        LowerPrefixMatch::Ok { peeled, cols } => (peeled, cols),
        LowerPrefixMatch::Fail { .. } => return false,
    };
    let line_indent = facts.indent_col + peel_columns;
    let frame_col = table_window_indent(stack);
    if line_indent > frame_col + STRUCT_INDENT_MAX { return false; }
    let content = skip_leading_whitespace(content);
    !content.is_empty() && content[0] == b'|'
}

/// Table classify（单级化）——分隔行确认/数据行开 Tablerow 直接产结论（原
/// table_separator_outcome/table_row_outcome 两件并入；peel 一次复用）。
pub(crate) fn classify_table(facts: &LineFacts, frame: &BlockFrame, rt: &ScanRuntime) -> Option<(BlockType, ScanAction)> {
    let stack = rt.stack;
    let (content, _) = peel_lower_prefixes(facts.content, stack);
    let content = skip_leading_whitespace(content);
    let is_sep = !content.is_empty() && is_table_separator(&content[1..]);
    if is_sep {
        // separator: tentative → confirm (validate cols); confirmed → end table
        let confirmed = frame.body() & 1 != 0;
        if confirmed {
            return None; // extra separator in confirmed table → end
        }
        let header_cols = frame.body() >> 1;
        let (sep_cols, _) = cell_ranges_bytes(content);
        if header_cols != sep_cols as u8 {
            return None; // column mismatch → abandon
        }
        let new_body = frame.body() | 1;
        let new_frame = BlockFrame::new(FrameKind::Table, frame.indent(), new_body);
        Some((BlockType::TableSeparator, ScanAction::ContinueAs(new_frame)))
    } else {
        // data row: tentative → abandon; confirmed → push first TableRow
        if frame.body() & 1 == 0 {
            return None; // tentative table, second line must be separator
        }
        Some((BlockType::TableRow, ScanAction::Push(
            BlockFrame::new(FrameKind::TableRow, 0, 0)
        )))
    }
}

// ── TableRow (data row item within a confirmed Table) ──

/// TableRow admit: peeled `|` prefix within the table window, not a separator.
pub(crate) fn admit_table_row(facts: &LineFacts, _frame: &BlockFrame, stack: &[BlockFrame]) -> bool {
    let (content, peel_columns) = match peel_lower_prefixes_from(facts.content, stack, facts.indent_col) {
        LowerPrefixMatch::Ok { peeled, cols } => (peeled, cols),
        LowerPrefixMatch::Fail { .. } => return false,
    };
    let line_indent = facts.indent_col + peel_columns;
    let frame_col = table_window_indent(stack);
    if line_indent > frame_col + STRUCT_INDENT_MAX { return false; }
    let content = skip_leading_whitespace(content);
    if content.is_empty() || content[0] != b'|' { return false; }
    !is_table_separator(&content[1..])
}

/// TableRow classify（单级化）——admit 已过滤，恒 PopPush（原 table_row_pop_push 并入）。
pub(crate) fn classify_table_row(_facts: &LineFacts, _frame: &BlockFrame, _rt: &ScanRuntime) -> Option<(BlockType, ScanAction)> {
    Some((BlockType::TableRow, ScanAction::PopPush(
        BlockFrame::new(FrameKind::TableRow, 0, 0)
    )))
}

/// HtmlBlock classify（单级化）——Open/Inline/Ends/Content 四臂直接产结论
/// （原 html_block_open_outcome 并入；tokenize 一次复用）。
pub(crate) fn classify_html_block(facts: &LineFacts, frame: &BlockFrame, rt: &ScanRuntime) -> Option<(BlockType, ScanAction)> {
    let stack = rt.stack;
    let (peeled, _) = peel_lower_prefixes(facts.content, stack);
    let block_type = frame.block_type();
    let tokens = tokenize_html_line(peeled);
    if !tokens.is_empty() {
        if tokens.iter().any(|t| matches!(t, HtmlLineToken::Open { .. })) {
            // OPEN 场景 outcome（tokenize 重建嵌套 HtmlElement 帧——不可表化）
            let mut nested: Vec<BlockFrame> = Vec::new();
            for tok in &tokens {
                if let HtmlLineToken::Open { .. } = tok {
                    nested.push(BlockFrame::new(FrameKind::HtmlElement, 0, 0));
                }
            }
            if nested.is_empty() { return None; }
            let n_nested = nested.len();
            return Some((BlockType::HtmlElement, ScanAction::Composite {
                outer_type: BlockType::HtmlElement,
                primary: CompositePrimary::Continue,
                nested,
                // consume 位声明不保证：HELEM 复合行的 nested 层位移
                // 恒 0——唯一生产读者 first_item_layer_push 跳过非 BLT/OL 层；
                // 未来读位消费点若覆盖 HELEM 复合行，须先补真实位移。
                nested_consumes: vec![0; n_nested],
                primary_consume: 0,
            }));
        }
        if tokens.iter().any(|t| matches!(t, HtmlLineToken::Inline)) {
            return Some((BlockType::HtmlElement, ScanAction::Continue));
        }
        // Close-only → 落入 html_block_ends
    }
    if html_block_ends(block_type, peeled) {
        Some((BlockType::HtmlContent, ScanAction::Pop))
    } else {
        Some((BlockType::HtmlContent, ScanAction::Continue))
    }
}

/// RefDef classify（单级化）——Continue/Pass/Blank 直接产结论。
pub(crate) fn classify_refdef(facts: &LineFacts, _frame: &BlockFrame, rt: &ScanRuntime) -> Option<(BlockType, ScanAction)> {
    let stack = rt.stack;
    if facts.is_blank { return Some((BlockType::Blank, ScanAction::Pop)); }
    let t = dispatch_lut(facts.content);
    let first = facts.first_char;
    if (first == b'=' || first == b'-' || first == b'*' || first == b'_' || first == b'~')
        && all_hs_char(facts.content, first) {
        return None;
    }
    // dest 行吸收判定（RefDef 的多行 dest 吸收优先于行首容器判定——
    // CM §4.7——`[Foo bar]:\n<my url>` 的 `<my url>`（angle dest——type 7
    // 修复后判 HtmlBlock）——帧守卫若先 PASS 则截断（URL/title 被吸走）——
    // dest 解析优先。两处消费:
    //   帧守卫内（帧类行——body 0——dest pending——angle/普通 dest 吸收）
    //   非帧类行（保持现状语义）
    let dest_line = |content: &[u8]| -> bool {
        let trimmed = crate::block::str_view(content).trim_start();
        if let Some((_, _, consumed)) = crate::inline::parse_dest_body(trimmed) {
            let remaining = &trimmed[consumed..];
            return !remaining.as_bytes().iter().any(|&b| b != b' ' && b != b'\t');
        }
        false
    };
    if frame_kind(t).is_some() {
        // 帧类行（HtmlBlock/fence 等）——body 0（空 dest 开行——dest
        // pending）且 dest 行形态 → 吸收（不 PASS）——dest 已完成
        // （body 1——如 `[a]: /url` 后 `<div>`）→ PASS（独立块——正确）
        if let Some(top) = stack.last() {
            if top.body() & 1 == 0 && dest_line(facts.content) {
                return Some((BlockType::RefDef, ScanAction::Continue));
            }
        }
        return None;
    }
    let top = match stack.last() { Some(t) => t, None => return None };
    // #197（CM §4.7——多行 dest）：title 引号/括号开头的续行行
    // （`[foo]: \n      /url\n           'the title'` 的 title 行）——body=0
    // 帧（空 dest 开行）时 parse_dest_body 会把 `'the` 当 url（截到空格）
    // 误 PASS——引号/括号开头且闭合、title 后仅空白 → title 续行（吸收——
    // RefMap 构建拼接）。#211：`"title" ok`——title 后多余内容——非法——
    // 不吸收（rd 结束——官方顶层 p）。
    if matches!(first, b'"' | b'\'' | b'(') {
        let close_ch = if first == b'(' { b')' } else { first };
        let rest = &facts.content[1..];
        if let Some(end) = memchr::memchr(close_ch, rest) {
            let after = &rest[end + 1..];
            if after.iter().all(|&b| b == b' ' || b == b'\t') {
                return Some((BlockType::RefDef, ScanAction::Continue));
            }
        }
    }
    // #197：title 未闭合帧（body bit1——开行 `/url '`）——续行行是 title
    // 内容（无引号——`title`/`line1`）——守卫 4 例外——吸收
    if top.body() & 1 == 1 && top.body() & 2 == 0
        && first != b'"' && first != b'\'' && first != b'(' {
        return None;
    }
    let trimmed = crate::block::str_view(facts.content).trim_start();
    if let Some((_, _, consumed)) = crate::inline::parse_dest_body(trimmed) {
        let remaining = &trimmed[consumed..];
        if remaining.as_bytes().iter().any(|&b| b != b' ' && b != b'\t') {
            return None;
        }
    }
    Some((BlockType::RefDef, ScanAction::Continue))
}

/// 行表格（单级化）：臂→动作直接，无 scene 中间名。
#[derive(Clone, Copy, PartialEq, Eq)]
enum BqAct { BlankPrefix, BlankNoPrefix, Pass, Dispatch, DispatchOrPara, ParaContinue, Lazy }

/// 行表键事实（本函数已算中间事实——零新算）。
struct BqActFacts {
    empty: bool,
    rel_has_prefix: bool,
    rel_no_prefix: bool,
    interrupt: bool,
    content: BqContent,
}

/// 键全 Option（None＝不限）；键＝本函数已算中间事实（零新算）。
struct BqActRow {
    empty: Option<bool>,
    rel_has_prefix: Option<bool>,
    rel_no_prefix: Option<bool>,
    interrupt: Option<bool>,
    content_container: Option<bool>,
    content_noncontainer: Option<bool>,
    content_paragraph: Option<bool>,
    act: BqAct,
}

impl BqActRow {
    fn matches(&self, f: &BqActFacts) -> bool {
        let ok = |k: Option<bool>, v: bool| k != Some(!v);
        ok(self.empty, f.empty)
            && ok(self.rel_has_prefix, f.rel_has_prefix)
            && ok(self.rel_no_prefix, f.rel_no_prefix)
            && ok(self.interrupt, f.interrupt)
            && ok(self.content_container, f.content == BqContent::Container)
            && ok(self.content_noncontainer, f.content == BqContent::NonContainer)
            && ok(self.content_paragraph, f.content == BqContent::Paragraph)
    }
}

/// 行序铁律：R3 先于 R7（打断行不进内容臂——旧 return 序）；R8 默认行在表内。
const BQ_ACT_ROWS: &[BqActRow] = &[
    // R1 空∧有前缀 → BLANK_PREFIX（#241 空 BQ 行）
    BqActRow { empty: Some(true), rel_has_prefix: Some(true), rel_no_prefix: None, interrupt: None, content_container: None, content_noncontainer: None, content_paragraph: None, act: BqAct::BlankPrefix },
    // R2 空∧无前缀（含 Outside）→ BLANK_NOPREFIX
    BqActRow { empty: Some(true), rel_has_prefix: Some(false), rel_no_prefix: None, interrupt: None, content_container: None, content_noncontainer: None, content_paragraph: None, act: BqAct::BlankNoPrefix },
    // R3 lazy 打断 → PASS（重扫——新块位置由 admit 决定）
    BqActRow { empty: None, rel_has_prefix: None, rel_no_prefix: Some(true), interrupt: Some(true), content_container: None, content_noncontainer: None, content_paragraph: None, act: BqAct::Pass },
    // R3b 部分前缀结构行打断 → PASS（关内层 BQ 重扫——`> > x\n> - x` 的 `> - x`）
    BqActRow { empty: None, rel_has_prefix: Some(true), rel_no_prefix: None, interrupt: Some(true), content_container: None, content_noncontainer: None, content_paragraph: None, act: BqAct::Pass },
    // R4 有前缀∧容器 → DISPATCH
    BqActRow { empty: None, rel_has_prefix: Some(true), rel_no_prefix: None, interrupt: None, content_container: Some(true), content_noncontainer: None, content_paragraph: None, act: BqAct::Dispatch },
    // R5 有前缀∧非容器 → DISPATCH_OR_PARA
    BqActRow { empty: None, rel_has_prefix: Some(true), rel_no_prefix: None, interrupt: None, content_container: None, content_noncontainer: Some(true), content_paragraph: None, act: BqAct::DispatchOrPara },
    // R6 有前缀∧段落 → PARA_CONTINUE
    BqActRow { empty: None, rel_has_prefix: Some(true), rel_no_prefix: None, interrupt: None, content_container: None, content_noncontainer: None, content_paragraph: Some(true), act: BqAct::ParaContinue },
    // R7 无前缀∧段落 → LAZY
    BqActRow { empty: None, rel_has_prefix: None, rel_no_prefix: Some(true), interrupt: Some(false), content_container: None, content_noncontainer: None, content_paragraph: Some(true), act: BqAct::Lazy },
    // R8a Outside（既非 HasPrefix 也非 NoPrefix）→ PASS（原 `_` 臂展开——第一方覆盖）
    BqActRow { empty: None, rel_has_prefix: Some(false), rel_no_prefix: Some(false), interrupt: None, content_container: None, content_noncontainer: None, content_paragraph: None, act: BqAct::Pass },
    // R8b NoPrefix∧不打断∧容器内容 → PASS（原 `_` 臂展开）
    BqActRow { empty: None, rel_has_prefix: None, rel_no_prefix: Some(true), interrupt: Some(false), content_container: Some(true), content_noncontainer: None, content_paragraph: None, act: BqAct::Pass },
    // R8c NoPrefix∧不打断∧非容器内容 → PASS（原 `_` 臂展开）
    BqActRow { empty: None, rel_has_prefix: None, rel_no_prefix: Some(true), interrupt: Some(false), content_container: None, content_noncontainer: Some(true), content_paragraph: None, act: BqAct::Pass },
];

/// 打断判定的单源核心：offset = 行内容列 − Σ 栈内全部 BQ body（最内层 BQ 的
/// 绝对内容列，cmark 实证）。offset <= 0 且剩余内容 can_interrupt → 打断
/// （关 BQ，块开在父层）；offset > 0 或不可打断 → lazy。
/// `is_no_prefix`：行无 `>` 前缀，offset 基准 = facts.indent_col（历史口径）；
/// 部分前缀行（0 < 行前缀数 < 栈 BQ 层数）用 `facts.indent_col + consumed + 剩余
/// 前导空白` 作行内容列（前缀已剥，内容列落在最内层 BQ 坐标里）。
fn bq_interrupt_core(
    facts: &LineFacts,
    stack: &[BlockFrame],
    consumed_prefix_cols: usize,
    remainder: &[u8],
    is_no_prefix: bool,
) -> bool {
    let after_ws = skip_leading_whitespace(remainder);
    if after_ws.is_empty() {
        return false;
    }
    let content_col: usize = stack
        .iter()
        .filter(|f| f.kind() == FrameKind::BlockQuote)
        .map(|f| f.body() as usize)
        .sum();
    let offset = if is_no_prefix {
        facts.indent_col.saturating_sub(content_col)
    } else {
        facts
            .indent_col
            .saturating_add(consumed_prefix_cols)
            .saturating_add(compute_indent(remainder))
            .saturating_sub(content_col)
    };
    offset <= 0 && crate::effect::can_interrupt(dispatch_lut(after_ws), after_ws, 0)
}

/// 行内 BQ 前缀消费列宽（相对 facts.content 起点，连续 tab 展开）。
fn bq_prefix_consumed_cols(facts: &LineFacts, n: usize) -> usize {
    let mut cur = facts.content;
    let mut col = 0usize;
    for _ in 0..n {
        let before = cur;
        cur = strip_bq_one(cur);
        let removed = (cur.as_ptr() as usize) - (before.as_ptr() as usize);
        if removed == 0 {
            break;
        }
        col += column_width_from(&before[..removed], col);
    }
    col
}

/// 下层 BQ 失配时（Fail{frame_index}）的打断门控，供 projection 前置匹配用。
/// 与 classify_bq_scene 同一判官：部分前缀结构行（marker 等）才关帧；
/// 段落文本（lazy）不关。
pub(crate) fn bq_lower_interrupt(facts: &LineFacts, stack: &[BlockFrame]) -> bool {
    if stack.is_empty() {
        return false;
    }
    let line_bq = count_bq_prefixes(facts.content);
    let open_bq = stack
        .iter()
        .filter(|f| f.kind() == FrameKind::BlockQuote)
        .count();
    if line_bq >= open_bq {
        return false;
    }
    let consumed = bq_prefix_consumed_cols(facts, line_bq);
    let mut rem = facts.content;
    for _ in 0..line_bq {
        let before = rem;
        rem = strip_bq_one(rem);
        if rem.as_ptr() == before.as_ptr() {
            break;
        }
    }
    bq_interrupt_core(facts, stack, consumed, rem, line_bq == 0)
}

pub(crate) fn classify_bq_scene(facts: &LineFacts, frame: &BlockFrame, rt: &ScanRuntime) -> Option<(BlockType, ScanAction)> {
    let stack = rt.stack;
    let (rel, after_marker) = bq_peel(facts, stack);
    // 测量基准：缩进 = 剥完整 marker（`>`+可选 1 空格）后剩余前导空白列。
    let marker_remainder_indent = compute_indent(after_marker);
    let after_ws = skip_leading_whitespace(after_marker);
    // 中间事实（全部已算输入——零新算）：空→打断→内容；打断/内容在空行上短路（旧 return 序）。
    let empty = after_ws.is_empty();
    let line_bq = count_bq_prefixes(facts.content);
    let open_bq = stack
        .iter()
        .filter(|f| f.kind() == FrameKind::BlockQuote)
        .count();
    let interrupt = if empty {
        false
    } else {
        match rel {
            BqRel::InsideNoPrefix => bq_interrupt_core(facts, stack, 0, after_marker, true),
            // 部分前缀的结构行（前缀数 < 栈 BQ 层数）也能打断最内层 BQ：
            // `> > x\n> - x` 的 `> - x` 关内层 BQ、列表开在外层（cmark 实证）。
            BqRel::InsideHasPrefix if line_bq < open_bq => {
                let consumed = bq_prefix_consumed_cols(facts, line_bq);
                bq_interrupt_core(facts, stack, consumed, after_marker, false)
            }
            _ => false,
        }
    };
    // is_para_cont 的 prev 替代（setext 下划线需紧跟段落文本行——CM §4.2）。
    let prev_para = matches!(facts.prev_type,
        Some(BlockType::Paragraph | BlockType::Blockquote));
    let content = if empty {
        BqContent::Paragraph // 空行短路——不进 classify_bq（占位）
    } else {
        classify_bq(after_ws, marker_remainder_indent, prev_para, rel == BqRel::InsideHasPrefix)
    };
    let f = BqActFacts {
        empty,
        rel_has_prefix: rel == BqRel::InsideHasPrefix,
        rel_no_prefix: rel == BqRel::InsideNoPrefix,
        interrupt,
        content,
    };
    let row = BQ_ACT_ROWS.iter().find(|r| r.matches(&f))
        .expect("B1 未覆盖键组合＝方案缺陷（第一方覆盖）");
    // 格直连（判定律单级化）：行匹配后直接应用格——outcome fn 的二次 bq_peel 消失。
    match row.act {
        BqAct::BlankPrefix => {
            let prev = facts.prev_type.unwrap_or(BlockType::Blockquote);
            let t = crate::effect::block_type_effects(prev).blank_prefix_type;
            Some((t, ScanAction::Continue))
        }
        BqAct::BlankNoPrefix => Some((BlockType::Blank, ScanAction::Pop)),
        BqAct::Pass => None,
        BqAct::Dispatch => dispatch_after_bq_prefix(after_marker, facts, stack),
        BqAct::DispatchOrPara => dispatch_after_bq_prefix(after_marker, facts, stack).or_else(|| bq_para_continue()),
        BqAct::ParaContinue => bq_para_continue(),
        BqAct::Lazy => Some((BlockType::Paragraph, ScanAction::Continue)),
    }
}

/// 打断判定：CM §4.1 规则单源＝effect::can_interrupt（interrupt_rule 列）。
/// 壳已迁移（interrupts_para 删除——业务全在列，壳仅剩引用＝被替代）。

/// CM §5.2：空列表项（marker 后无内容）不能打断段落——`foo\n*` 例（顶层
/// classify_at_top 已有同检查——lazy 侧对称——`> foo\n- ` 应 lazy 文本）。
/// pub(crate)（sdl.rs 感知原子共享）。
pub(crate) fn is_empty_list_item(after_ws: &[u8]) -> bool {
    strip(BlockType::Bullet, after_ws, StripMode::All)
        .or_else(|| strip(BlockType::OrderedListItem, after_ws, StripMode::All))
        .map_or(false, |r| skip_leading_whitespace(r).is_empty())
}

/// CM §5.3 number-1 规则：start number must be 1——数值语义（`01.` 的 start
/// number 是 1——CM 例 4415 行"A start number may begin with 0s"——`01.` 可
/// 打断）——顶层 parse 数值与 lazy 一位检查的双实现分裂在此统一。
/// pub(crate)（sdl.rs 感知原子共享）。
pub(crate) fn number_is_one(content: &[u8]) -> bool {
    let d = number_digits(content);
    if d == 0 || d >= content.len() || (content[d] != b'.' && content[d] != b')') {
        return false;
    }
    crate::block::str_view(&content[..d])
        .parse::<u32>()
        .map_or(false, |n| n == 1)
}

/// 数字位数（≤9——CM 有序列表 marker 上限）——原 8 处内联循环收敛
/// （number_is_one/is_ordered_match/marker_col/parse_ol 等共用）。
#[inline]
pub(crate) fn number_digits(bytes: &[u8]) -> usize {
    let mut d = 0;
    while d < bytes.len() && d < 9 && bytes[d].is_ascii_digit() { d += 1; }
    d
}

/// 通用 marker 检测（原 4+ 处内联模式）：
/// bullet marker：`- * +` 后空格/tab。
#[inline]
pub(crate) fn is_bullet_marker(content: &[u8]) -> bool {
    content.len() >= 2 && matches!(content[0], b'-' | b'*' | b'+')
        && (content[1] == b' ' || content[1] == b'\t')
}

/// 通用 marker 检测：OL marker——`N.`/`N)` 后空格/tab（位数 ≤9）。
#[inline]
pub(crate) fn is_ol_marker(content: &[u8]) -> bool {
    let d = number_digits(content);
    d > 0 && d + 1 < content.len()
        && (content[d] == b'.' || content[d] == b')')
        && (content[d + 1] == b' ' || content[d + 1] == b'\t')
}

/// 通用 marker 检测：checklist 模式——`[x]`/`[X]`/`[ ]` 后空格/tab
/// （content 以 bullet marker 开头——索引 2 起为 checkbox）。
#[inline]
pub(crate) fn is_checklist_marker(content: &[u8]) -> bool {
    content.len() >= CHECKLIST_MARK_END && content[CHECKLIST_LIST_MARK_END] == b'['
        && (content[CHECKLIST_LIST_MARK_END + 1] == b'x'
            || content[CHECKLIST_LIST_MARK_END + 1] == b'X'
            || content[CHECKLIST_LIST_MARK_END + 1] == b' ')
        && content[CHECKLIST_LIST_MARK_END + 2] == b']'
        && (content[CHECKLIST_MARK_END - 1] == b' ' || content[CHECKLIST_MARK_END - 1] == b'\t')
}

/// ═══════════════════════════════════════════════════════════════════
/// 剥除公共接口
/// ═══════════════════════════════════════════════════════════════════
/// 层序列 = 前缀的所有容器层——BQ + marker（bullet/ol）按出现顺序循环。
/// 消费已有的 parse_bl / parse_ol / parse_bq / is_bullet_marker /
/// is_ol_marker / has_gt_prefix / number_digits / skip_leading_whitespace。
/// 信息守恒：层覆盖前缀 + 剩余 == 原片段。

/// 剥除的一层——kind + 在片段中的字节区间。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    pub kind: FrameKind,
    pub start: usize,
    pub end: usize,
}

/// 剥离结果——把「层 ＋ 余下部分 ＋ 坐标原点」当作**一个值**传递（§30 推演）。
/// 类型不变式＝源码早已声明的守恒（`peel` 的文档注释：「层覆盖前缀 ＋ 剩余 == 原片段」，
/// PE1 检验 2）；`origin` 记录「余下部分行首列＝0」所对应的外层坐标，使这件事可传递。
#[derive(Debug, Clone)]
pub struct Peeled<'a> {
    pub fragment: &'a str,
    pub layers: SmallVec<[Layer; 4]>,
    pub rest_off: usize,
    pub origin: usize,
}

impl<'a> Peeled<'a> {
    /// 余下部分＝应当被当作**新的 raw md** 再判的那一段。
    pub fn rest(&self) -> &'a str {
        &self.fragment[self.rest_off.min(self.fragment.len())..]
    }

    /// 容器前缀＝层覆盖的那一段（写侧重发前缀时用）。
    pub fn prefix(&self) -> &'a str {
        &self.fragment[..self.rest_off.min(self.fragment.len())]
    }

    /// **只消费指定种类的层**，其余层留在余下部分里；`keep_indent` 与 `strip_bq` 的第二参同义
    /// （false ⇒ 末尾再跳过水平空白）。
    /// 注意：`layer.end` 与 `strip_bq` 的逐层消费量是否逐字一致**尚未验证** ⇒
    /// 替换任何 `strip_bq` 调用点之前，必须先跑等价性打点（见 §34.4）。
    pub fn rest_after(&self, kinds: &[FrameKind], keep_indent: bool) -> &'a str {
        let mut off = 0usize;
        for layer in &self.layers {
            if !kinds.contains(&layer.kind) {
                break;
            }
            off = layer.end;
        }
        let rest = &self.fragment[off.min(self.fragment.len())..];
        if keep_indent {
            rest
        } else {
            str_view(skip_leading_whitespace(rest.as_bytes()))
        }
    }

    /// ＝ 现 `strip_bq(bytes, false)`（只剥 BQ 层、不保留缩进）。
    pub fn rest_after_bq(&self) -> &'a str {
        self.rest_after(&[FrameKind::BlockQuote], false)
    }

    /// ＝ 现 `strip_bq(bytes, true)`（只剥 BQ 层、保留缩进）。
    pub fn rest_after_bq_keep_indent(&self) -> &'a str {
        self.rest_after(&[FrameKind::BlockQuote], true)
    }

    /// **消费前 `i` 层**之后的余下部分——这就是「逐层坐标系」的基（`origin` 字段是为它准备的）。
    /// `i = 0` ⇒ 原片段；`i = layers.len()` ⇒ 全部层之后（**不**做 `rest()` 那样的尾部空白跳过 ✗，
    /// 两者区别要记住：`rest()` 是「换行后要当 raw md 用的那一段」，本方法是「第 i 层的坐标起点」）。
    pub fn rest_after_layer(&self, i: usize) -> &'a str {
        // `take(i)` 一次表达三种情形：i=0 ⇒ 0（不消费）、i=1 ⇒ 第 0 层之后、i>len ⇒ 全部（末层之后）。
        // （首版用 `get(i.saturating_sub(1))` 是**错的**：i=0 时会取到 `layers[0].end` ✗ —— 由探针抓到。）
        let off = self.layers.iter().take(i).last().map_or(0, |l| l.end);
        &self.fragment[off.min(self.fragment.len())..]
    }

    /// 第 `i` 层之后余下部分的**坐标原点**（外层字节偏移 ＋ 本层前缀宽度）——逐层事实的输入。
    pub fn origin_after_layer(&self, i: usize) -> usize {
        self.origin + self.layers.iter().take(i).last().map_or(0, |l| l.end)
    }

    /// **第 `i` 层坐标系里的事实**（逐层事实）：在 `rest_after_layer(i)` 上**复用 `LineFacts::from`
    /// 的既有语义**重算——所以它与「单层世界」的读法**同源**（规则没有第二份 ✓）。
    /// 这是「决策逐层化」（§「应有形态」）的输入面；目前只提供、尚未接入决策。
    pub fn facts_after_layer(&self, i: usize) -> LineFacts<'a> {
        LineFacts::from(self.rest_after_layer(i).as_bytes())
    }
}

#[cfg(test)]
mod peeled_layer_probe {
    use super::*;

    /// 逐层事实打点：第 i 层的事实 == 在「消费前 i 层」的余下部分上按既有读法算出的东西
    /// （这里用 `content`／`indent_col()` 两个可观测量交叉验证；`LineFacts` 的字段是 `pub(crate)`，
    /// 所以这个小打点只能放在 crate 内 ✗ 集成测试看不到）。
    #[test]
    fn facts_after_layer_matches_single_layer_reading() {
        // 期望值以**引擎语义**核准（`content` = 行首空白之后的余下部分；层会吃掉标记后一个空格）：
        // `> >   x` ⇒ i=0 整行、i=1 剩 `>   x`、i=2 剩 `  x`（其 content 为 `x`、indent 2）。
        for (src, want_content, want_indent) in [
            ("> >   x", &["> >   x", ">   x", "x"][..], &[0usize, 0, 2][..]),
            ("- > y", &["- > y", "> y", "y"][..], &[0, 0, 0][..]),
            ("x", &["x"][..], &[0][..]),
        ] {
            let p = peel_peeled(src, 0);
            for i in 0..=p.layers.len() {
                let f = p.facts_after_layer(i);
                assert_eq!(f.content, want_content[i].as_bytes(), "{src:?} 第 {i} 层 content");
                assert_eq!(f.indent_col(), want_indent[i], "{src:?} 第 {i} 层 indent_col");
            }
        }
    }
}

/// 「逐层视角」观察探针（只打印、**不断言**）：对契约形状逐层打印
/// （层类·内容关系·缩进）——用来回答「单次决策看到的」与「逐层决策会看到的」差在哪，
/// 这是「删 ⑥ 补偿」的准入证据第一半（第二半＝差异表与现状结论对照）。
#[cfg(test)]
mod layer_decision_probe {
    use super::*;

    #[test]
    fn per_layer_view_of_contract_shapes() {
        for src in [
            "> > > > deep",   // 契约族一：期望 PL = 容器名（Blockquote）
            "- >   x",        // 契约族二：期望 PL = 内容名
            "- a:",
            "> >     C",
            "  >     B",
            ">",
        ] {
            let p = peel_peeled(src, 0);
            let mut line = format!("{src:?}（{} 层）：", p.layers.len());
            for i in 0..=p.layers.len() {
                let f = p.facts_after_layer(i);
                let kind = if i == 0 {
                    "-".to_string()
                } else {
                    format!("{:?}", p.layers[i - 1].kind)
                };
                line.push_str(&format!(
                    " [i={i} 层={kind} 内容关系={:?} 缩进={}]",
                    f.content_rel(),
                    f.indent_col()
                ));
            }
            println!("{line}");
        }
    }
}

/// 「逐层**相位二**视角」观察探针（只打印、不断言）：对每一层，取「该层之后的余下部分」，
/// 按 CM 相位二（仓内入口 `classify_in_container`）问一句「这里开始的块是什么」——
/// 用来回答 §69 的悬问：**「余下部分首块类型」能否把两族契约分开**（`> > > > deep` 要容器名 /
/// `> >     1` 要内容名）✓。
#[cfg(test)]
mod per_layer_phase2_probe {
    use super::*;

    #[test]
    fn per_layer_phase2_of_contract_shapes() {
        for src in [
            "> > > > deep",
            "- >   x",
            "> >     1",
            "  >     B",
            "- a:",
        ] {
            let p = peel_peeled(src, 0);
            let mut parts: Vec<String> = Vec::new();
            for i in 0..p.layers.len() {
                let f = p.facts_after_layer(i + 1);
                let ws = skip_leading_whitespace(f.content);
                // 【审计修正 D4】原先用 `compute_indent(f.content)` 是**错的** ——
                // `LineFacts::from` 里 `content = &bytes[i..]` 已跳过行首空白，缩进存在 `indent_col`，
                // 故 `compute_indent(content)` 恒为 0，缩进码永远判不出来 ✗。改用 `f.indent_col()` ✓。
                let indent = f.indent_col();
                let ct = if ws.is_empty() {
                    BlockType::Blank
                } else {
                    classify_in_container(ws, indent, false)
                };
                parts.push(format!(
                    "i={} 层={:?} 余下={:?} ⇒ 相位二={:?}",
                    i,
                    p.layers[i].kind,
                    String::from_utf8_lossy(f.content),
                    ct
                ));
            }
            println!("{src:?}\n    {}", parts.join("\n    "));
        }
    }
}

/// §128.3 写面退化节点探针（**只观测** ✓）：用真实入口 `snap.build_node(…)` 造节点，
/// 再直喂 `compute_write_from_node` —— 逐例 `catch_unwind`，报告哪一例 panic ✗。
#[cfg(test)]
mod write_degenerate_probe {
    use super::*;

    #[test]
    fn degenerate_node_write_path() {
        let mut out = Vec::new();
        for doc in ["", "a\nb\n"] {
            let mut st = crate::projection::State::new(doc);
            let snap = st.snapshot();
            let cases: [(usize, usize, &str); 4] = [
                (0, 0, "空区间"),
                (10, 12, "远超文档"),
                (0, 99, "尾越界"),
                (5, 5, "起点越界"),
            ];
            for (s, e, tag) in cases {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let n = snap.build_node(s, e, BlockType::Paragraph, false);
                    let w = snap.compute_write_from_node(&n, "content", "x");
                    format!("Ok({w:?})")
                }));
                match r {
                    Ok(v) => out.push(format!("[写面退化] doc={doc:?} {tag}：start={s} end={e} ⇒ {v}")),
                    Err(_) => out.push(format!(
                        "[写面退化] doc={doc:?} {tag}：start={s} end={e} ⇒ **PANIC** ✗"
                    )),
                }
            }
        }
        for l in out {
            println!("{l}");
        }
    }

    /// **P3-2 回归判据**：退化节点**不得 panic**（2 文档 × 4 例 ＝ 8 例 ✓）——
    /// 修复（clamp 最后合法下标 ＋ 空 rope 短路 ✓）之后应为 8/8 Ok ✓。
    #[test]
    fn degenerate_node_never_panics() {
        let mut bad = 0;
        for doc in ["", "a\nb\n"] {
            let mut st = crate::projection::State::new(doc);
            let snap = st.snapshot();
            for (s, e) in [(0usize, 0usize), (10, 12), (0, 99), (5, 5)] {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let n = snap.build_node(s, e, BlockType::Paragraph, false);
                    let _ = snap.compute_write_from_node(&n, "content", "x");
                }));
                if r.is_err() {
                    bad += 1;
                    println!("PANIC: doc={doc:?} start={s} end={e}");
                }
            }
        }
        assert_eq!(bad, 0, "退化节点不得 panic（P3-2 回归判据）");
    }
}

/// §112.3 修正版探针：**不造栈、不复制驱动**，直接读生产口径 —— 建快照后逐行打印
/// PL（`snap.lines[i].node_type()`，判据看的就是它），并与「余下部分首块类型」（相位二）并排。
#[cfg(test)]
mod prev_fact_probe {
    use super::*;

    #[test]
    fn doc_pl_vs_rem_first_block() {
        for doc in [
            "> >\n> >     C\n",
            "- >   x\n",
            "> > > > deep\n",
            "> a:\n> >     B\n",
            "> >     1\n",
            "  >     B\n",
        ] {
            let mut st = crate::projection::State::new(doc);
            let snap = st.snapshot();
            println!("{doc:?}");
            for (i, p) in snap.lines.iter().enumerate() {
                let raw = doc.lines().nth(i).unwrap_or("");
                let rem = peel_peeled(raw, 0).rest();
                let ws = skip_leading_whitespace(rem.as_bytes());
                let ind = compute_indent(rem.as_bytes());
                let rem_bt = if ws.is_empty() {
                    BlockType::Blank
                } else {
                    classify_in_container(ws, ind, false)
                };
                println!(
                    "   L{i}: PL={:?}  ｜ 原文={raw:?} 余下={rem:?} 余下首块={rem_bt:?}",
                    p.node_type()
                );
            }
        }
    }
}

/// `peel` 的「值化」包装：行为与 `peel` **完全相同**，只是把两半与原点一起交出去
/// （第 1 步：只搬不改 —— 调用者行为不变，验收看计数不变）。
pub fn peel_peeled<'a>(fragment: &'a str, origin: usize) -> Peeled<'a> {
    let (layers, rest) = peel(fragment);
    let rest_off = fragment.len() - rest.len();
    Peeled { fragment, layers, rest_off, origin }
}

/// 层步原子：剥 cursor 处**一层**容器标记（单层无分配）。
/// 返回 (kind, consumed)——consumed = ws + mark_end（BQ 另加 has_trailing）。
/// 无层 → None（空/未识别首字符/TB 守卫）。
/// peel 循环体与 scan 侧单层探针共用。
pub(crate) fn step_layer(cursor: &[u8]) -> Option<(FrameKind, usize)> {
    let ws = cursor.iter().take_while(|&&x| x == b' ' || x == b'\t').count();
    let tb = &cursor[ws..];
    if tb.is_empty() { return None; }
    match BLOCK_DISPATCH[tb[0] as usize] {
        // 5 = BQ（parse_bq——">" + 可选单空格/tab——宽度 = 1 + has_trailing）
        5 => {
            let (has_trailing, _) = parse_bq(tb)?;
            Some((FrameKind::BlockQuote, ws + 1 + has_trailing as usize))
        }
        // 2 = bullet / thematic break（TB 守卫优先——镜像 dispatch_lut arm 2
        // 顺序——CM §4.1——TB 无 container——非层）
        2 => {
            if thematic_break_matches(tb) { return None; }
            let (_, mark_end, _) = parse_bl(tb)?;
            Some((FrameKind::BulletList, ws + mark_end))
        }
        // 6 = ol（parse_ol——严格——delim 后空格/行尾——DigitsDelimPlusWs）
        6 => {
            let (_, _, mark_end) = parse_ol(tb)?;
            Some((FrameKind::OrderedList, ws + mark_end))
        }
        _ => None,
    }
}

/// 剥除片段前缀的全部容器层。返回（层序列，剩余文本）。
/// BQ + marker（bullet/ol）交替——层顺序 = 出现顺序。
/// 信息守恒：层覆盖前缀 + 剩余 == 原片段（PE1 检验 2）。
/// thematic break 守卫（`---`/`***`/`* * *`——CM §4.1——非 marker 层）。
pub fn peel(fragment: &str) -> (SmallVec<[Layer; 4]>, &str) {
    let mut layers: SmallVec<[Layer; 4]> = SmallVec::new();
    let mut cur = fragment;
    let base = fragment.as_ptr() as usize;
    // 层检测 = dispatch_lut 分类规则的分派
    // （BLOCK_DISPATCH 首字符表——一个规则两个粒度: 1:1 单层几何 vs
    // 非 1:1 多层序列）——检测全部走解析族（严格——含真空）+ TB 守卫。
    while let Some((kind, consumed)) = step_layer(cur.as_bytes()) {
        let start = cur.as_ptr() as usize - base;
        layers.push(Layer { kind, start, end: start + consumed });
        cur = &cur[consumed..];
    }
    (layers, cur)
}

/// 剥全部 BQ 前缀。keep_indent=true→保留内容缩进（fence/IC 内容行）。
/// 复用的生产逻辑——strip_bq_one 的每层 skip-ws 语义。
/// 嵌套 BQ（`> > item`/`> > ```` ）必须剥全部层——单层剥离会让 tag/content
/// 起点含内层 > 前缀，与 build_node 的 peel_width 计算不一致。
/// 每层剥前 trim（`>  > item` 前缀间多余空格也剥）——keep_indent 时 parse
/// 失败回退 skip 前（内容缩进保留）；false 最后 trim 前导空白。
/// rope/&str 派生字节的安全 &str 还原——MD 入口链保证 UTF-8:
/// edit(&str)/Rope::from(&str)/sanitize_nul 字符级替换/PyO3 str 五道关;
/// strip_bq/skip_leading_whitespace/ASCII 边界切片不破坏多字节序列——
/// 本函数输入恒合法。debug_assert 验证前提(判官 debug 模式全量兜底),
/// release 零验证(from_utf8_unchecked)。
#[inline]
pub(crate) fn str_view(bytes: &[u8]) -> &str {
    debug_assert!(
        std::str::from_utf8(bytes).is_ok(),
        "str_view: 非 UTF-8 字节——入口链假设被破坏"
    );
    unsafe { std::str::from_utf8_unchecked(bytes) }
}

pub fn strip_bq(content: &[u8], keep_indent: bool) -> &[u8] {
    let mut cur = content;
    // ⑤a：层检测单源——step_layer 取代手写 skip-ws＋parse_bq（行为等价：
    // BQ 宽=ws+1+has_trailing；非 BQ 层遇停原样返回）。
    while let Some((kind, consumed)) = step_layer(cur) {
        if kind != FrameKind::BlockQuote {
            break;
        }
        cur = &cur[consumed..];
    }
    if keep_indent { cur } else { skip_leading_whitespace(cur) }
}

/// marker 层过滤（BQ 不计）——壳已迁移：调用方直呼 is_list_container 过滤
/// （marker_layers 删除——生产已由 Peeled/层域事实替代，仅剩测试引用＝被替代）。

/// 从文本剥 N 个 marker 层——N 层 peel 后取剩余。
/// ⑤a：层检测单源——step_layer 取代手写 BLOCK_DISPATCH 分派（行为等价：
/// 仅列表层推进、BQ/None 遇停；每层 trim_start 保留）。
pub fn peel_markers(content: &str, n: usize) -> &str {
    let mut c = content;
    for _ in 0..n {
        let Some((kind, consumed)) = step_layer(c.as_bytes()) else { break };
        if !crate::effect::is_list_container(kind) {
            break;
        }
        c = &c[consumed..].trim_start();
    }
    c
}

/// 层序列的总消费字节数——用于写面 bq_off 等偏移计算。
pub fn layer_offset(layers: &[Layer]) -> usize {
    layers.last().map_or(0, |l| l.end)
}

/// BQ 壳 mark 文本：全部连续 BQ 前缀（trim 尾空白）——multi-`>` 行不再只取
/// 单层 `>`。非 BQ 行返回空串。
pub(crate) fn bq_mark_text(line: &str) -> String {
    // 第 1 步（只搬不改）：本函数只要**层**，但仍经 `Peeled` 拿——把「我只要哪一半」
    // 从隐含（解构丢 rest）变成显式（下面 `..` 明示丢弃）。
    let Peeled { fragment, layers, .. } = peel_peeled(line, 0);
    let end = layers.iter()
        .take_while(|l| l.kind == FrameKind::BlockQuote)
        .last()
        .map_or(0, |l| l.end);
    if end > 0 { fragment[..end].trim_end().to_string() } else { String::new() }
}

/// BQ 壳 mark 的切片跨度（行内）：起点 0，长度 = 连续 BQ 层末端的 trim_end 长度。
/// 与 `bq_mark_text` 同源——span 消费方直接切 `line[..len]`。
pub(crate) fn bq_mark_span(line: &str) -> (usize, usize) {
    let Peeled { layers, .. } = peel_peeled(line, 0);
    let end = layers.iter()
        .take_while(|l| l.kind == FrameKind::BlockQuote)
        .last()
        .map_or(0, |l| l.end);
    (0, line[..end].trim_end().len())
}

// （bq_outcome_* 四件已并入 classify_bq_scene 格直连）

// （格直连后无 scene u8 常量；BqAct/ListAct 臂自注释）

/// List classify — 复用 list_peel + classify_list, 返回结论 (含 blank)。
/// BLT/OL 共享: frame.kind() 内部区分 bullet/ordered 匹配。
/// BQ-below 守卫（无 > 前缀行 → PASS）由 evaluate_decl 约束链 + 豁免
/// 处理（`> - item\nlazy` 的 lazy 经 list_lazy_candidate 豁免 → item lazy）。
/// 纯 `---`/`===`（setext 下划线形态——CM §4.2——`- Bar\n  ---` 的 ---）。
#[inline]
pub(crate) fn is_setext_underline(after_ws: &[u8]) -> bool {
    !after_ws.is_empty() && (after_ws[0] == b'-' || after_ws[0] == b'=')
        && after_ws.len() >= 3 && all_hs_char(after_ws, after_ws[0])
}

// ── B2：LIST 场景行表（臂→scene；行序＝旧判定序：空→setext→bq_pass→内容臂→默认 PASS）──

/// 行表格（单级化）：臂→动作直接，无 scene 中间名。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ListAct { Blank, PopPush, PushNested, PushCross, CompositePush, Paragraph, Pass, IndentedCode, Setext, Content }

/// 行表键事实（本函数已算中间事实——零新算）。
struct ListActFacts {
    empty: bool,
    setext: bool,
    rel_outside: bool,
    rel_inside: bool,
    bq_pass: bool,
    gt: bool,
    content: ListContent,
}

/// 键全 Option（None＝不限）；content 用 Option<ListContent>（None＝不限）。
struct ListActRow {
    empty: Option<bool>,
    setext: Option<bool>,
    rel_outside: Option<bool>,
    rel_inside: Option<bool>,
    bq_pass: Option<bool>,
    gt: Option<bool>,
    content: Option<ListContent>,
    act: ListAct,
}

impl ListActRow {
    fn matches(&self, f: &ListActFacts) -> bool {
        let ok = |k: Option<bool>, v: bool| k != Some(!v);
        ok(self.empty, f.empty)
            && ok(self.setext, f.setext)
            && ok(self.rel_outside, f.rel_outside)
            && ok(self.rel_inside, f.rel_inside)
            && ok(self.bq_pass, f.bq_pass)
            && ok(self.gt, f.gt)
            && self.content.map_or(true, |c| f.content == c)
    }
}

/// 行序铁律：R3（bq_pass）先于 R4-R6（Outside 内容臂）；R11（NonContainer∧gt）先于 R12；无兜底行（第一方覆盖——未命中=panic）。
const LIST_ACT_ROWS: &[ListActRow] = &[
    // R1 空 → BLANK
    ListActRow { empty: Some(true), setext: None, rel_outside: None, rel_inside: None, bq_pass: None, gt: None, content: None, act: ListAct::Blank },
    // R2 item 内 setext 下划线（#301——CM §4.2）→ SETEXT
    ListActRow { empty: None, setext: Some(true), rel_outside: None, rel_inside: None, bq_pass: None, gt: None, content: None, act: ListAct::Setext },
    // R3 Outside∧BQ 前缀 marker（列表之上无 BQ 层）→ PASS（BQ 顶层打开）
    ListActRow { empty: None, setext: None, rel_outside: Some(true), rel_inside: None, bq_pass: Some(true), gt: None, content: None, act: ListAct::Pass },
    // R4 Outside∧SameListItem → POP_PUSH
    ListActRow { empty: None, setext: None, rel_outside: Some(true), rel_inside: None, bq_pass: None, gt: None, content: Some(ListContent::SameListItem), act: ListAct::PopPush },
    // R5 Outside∧LazyParagraph → PARAGRAPH（lazy 续行并入 item 段落）
    ListActRow { empty: None, setext: None, rel_outside: Some(true), rel_inside: None, bq_pass: None, gt: None, content: Some(ListContent::LazyParagraph), act: ListAct::Paragraph },
    // R6 Outside 其余（bq_pass 归 R3）→ PASS
    ListActRow { empty: None, setext: None, rel_outside: Some(true), rel_inside: None, bq_pass: Some(false), gt: None, content: None, act: ListAct::Pass },
    // R7 Inside∧SameListItem → PUSH_NESTED
    ListActRow { empty: None, setext: None, rel_outside: None, rel_inside: Some(true), bq_pass: None, gt: None, content: Some(ListContent::SameListItem), act: ListAct::PushNested },
    // R8 Inside∧OtherListItem → PUSH_CROSS
    ListActRow { empty: None, setext: None, rel_outside: None, rel_inside: Some(true), bq_pass: None, gt: None, content: Some(ListContent::OtherListItem), act: ListAct::PushCross },
    // R9 Inside∧NonListContainer → COMPOSITE_PUSH
    ListActRow { empty: None, setext: None, rel_outside: None, rel_inside: Some(true), bq_pass: None, gt: None, content: Some(ListContent::NonListContainer), act: ListAct::CompositePush },
    // R10 Inside∧NestingContainer → COMPOSITE_PUSH
    ListActRow { empty: None, setext: None, rel_outside: None, rel_inside: Some(true), bq_pass: None, gt: None, content: Some(ListContent::NestingContainer), act: ListAct::CompositePush },
    // R11 Inside∧NonContainer∧BQ 前缀 → CONTENT（B 场景：标题/主题块保持 item）
    ListActRow { empty: None, setext: None, rel_outside: None, rel_inside: Some(true), bq_pass: None, gt: Some(true), content: Some(ListContent::NonContainer), act: ListAct::Content },
    // R12 Inside∧NonContainer（无 BQ 前缀）→ PASS
    ListActRow { empty: None, setext: None, rel_outside: None, rel_inside: Some(true), bq_pass: None, gt: Some(false), content: Some(ListContent::NonContainer), act: ListAct::Pass },
    // R13 Inside∧Paragraph → PARAGRAPH
    ListActRow { empty: None, setext: None, rel_outside: None, rel_inside: Some(true), bq_pass: None, gt: None, content: Some(ListContent::Paragraph), act: ListAct::Paragraph },
    // R14 Inside∧LazyParagraph → PARAGRAPH（逻辑不可达——按列表内段落处理）
    ListActRow { empty: None, setext: None, rel_outside: None, rel_inside: Some(true), bq_pass: None, gt: None, content: Some(ListContent::LazyParagraph), act: ListAct::Paragraph },
    // R15 Inside∧IndentedCode → INDENTED_CODE
    ListActRow { empty: None, setext: None, rel_outside: None, rel_inside: Some(true), bq_pass: None, gt: None, content: Some(ListContent::IndentedCode), act: ListAct::IndentedCode },
];

pub(crate) fn classify_list_scene(facts: &LineFacts, frame: &BlockFrame, rt: &ScanRuntime) -> Option<(BlockType, ScanAction)> {
    let stack = rt.stack;
    let (rel, effective, after_ws) = list_peel(facts, frame, stack);
    let empty = after_ws.is_empty();
    // item 内 setext 下划线（#301）——仅 Inside 且非空（旧 return 序）。
    let setext = !empty && rel == ListRel::Inside && is_setext_underline(after_ws)
        && facts.prev_type.is_some_and(|p| matches!(p,
            BlockType::Paragraph | BlockType::Blockquote));
    // §3.5：BQ 前缀 marker 行只属于「列表之上有 BQ 层」的嵌套场景。
    let bq_above = stack[..stack.len().saturating_sub(1)]
        .iter().any(|f| f.kind() == FrameKind::BlockQuote);
    let bq_pass = !empty && rel == ListRel::Outside && !bq_above
        && count_bq_prefixes(facts.content) > 0
        && {
            let after_bq = peel_peeled(str_view(facts.content), 0).rest_after_bq().as_bytes();
            is_list_marker(after_bq)
        };
    let gt = has_gt_prefix(facts.content);
    let content = if empty {
        ListContent::Paragraph // 空行短路——不进 classify_list（占位）
    } else {
        classify_list(facts, after_ws, frame.kind(), frame, effective)
    };
    let f = ListActFacts {
        empty,
        setext,
        rel_outside: rel == ListRel::Outside,
        rel_inside: rel == ListRel::Inside,
        bq_pass,
        gt,
        content,
    };
    let row = LIST_ACT_ROWS.iter().find(|r| r.matches(&f))
        .expect("B2 未覆盖键组合＝方案缺陷（第一方覆盖）");
    // 格直连（判定律单级化）：行匹配后直接应用格——outcome fn 的二次 list_peel 消失。
    match row.act {
        ListAct::Blank => Some((BlockType::Blank, ScanAction::Continue)),
        ListAct::Setext => Some((BlockType::HsSep, ScanAction::Continue)),
        ListAct::Pass => None,
        ListAct::Paragraph => Some((BlockType::Paragraph, ScanAction::Continue)),
        ListAct::IndentedCode => Some((BlockType::IndentedCode, ScanAction::Continue)),
        ListAct::PopPush => {
            // item 行内容 IC 覆盖（Bug B 的列表内版本）。IC 优先于 thematic。
            if item_first_content_is_ic(frame.kind(), after_ws) {
                build_pop_push(frame, after_ws, effective, stack, facts)
                    .map(|(_, a)| (BlockType::IndentedCode, a))
            } else if thematic_break_matches(after_ws) {
                // 符合性：item 内容 thematic（`- * * *`）需整行检查。
                Some((BlockType::ThematicBreak, ScanAction::Pop))
            } else {
                build_pop_push(frame, after_ws, effective, stack, facts)
            }
        }
        ListAct::PushNested => {
            if item_first_content_is_ic(frame.kind(), after_ws) {
                build_push_nested(frame, after_ws, effective, facts)
                    .map(|(_, a)| (BlockType::IndentedCode, a))
            } else {
                build_push_nested(frame, after_ws, effective, facts)
            }
        }
        ListAct::PushCross => build_push_cross_kind(frame.kind(), after_ws, effective, facts),
        ListAct::CompositePush => build_composite_continue_push(after_ws, effective, frame, facts),
        ListAct::Content => {
            if after_ws.is_empty() { None } else { Some((dispatch_lut(after_ws), ScanAction::Continue)) }
        }
    }
}

// （list_outcome_* 五件已并入 classify_list_scene 格直连）

// ── HtmlElement (tokenize + Close 匹配) ──

/// HtmlElement token 分析结果 (classify 与 composite outcome 共享, 避免逻辑重复)。
pub(crate) struct HtmlElementAnalysis {
    pub primary: CompositePrimary,
    pub nested: Vec<BlockFrame>,
    pub has_pop: bool,
    pub has_open: bool,
}

/// HtmlElement token 循环的单一实现（classify 与 composite outcome 共用）。
/// 开行行号来自 scan 运行时（open_lines 栈）；开标签文本直接从 Rope 读，
/// 不再用线程本地 SCAN_LINE_INFO 反推 frame body 里的 low12。
pub(crate) fn analyze_html_element(
    content: &[u8],
    open_line: Option<u32>,
    rope: Option<&Rope>,
) -> HtmlElementAnalysis {
    let tokens = tokenize_html_line(content);
    let mut primary: CompositePrimary = CompositePrimary::Continue;
    let mut nested: Vec<BlockFrame> = Vec::new();
    let mut has_pop = false;
    let mut has_open = false;
    for tok in &tokens {
        match tok {
            HtmlLineToken::Open { .. } => {
                let f = BlockFrame::new(FrameKind::HtmlElement, 0, 0);
                if has_pop || !matches!(primary, CompositePrimary::Continue) {
                    nested.push(f);
                } else {
                    primary = CompositePrimary::Push(f);
                }
                has_open = true;
            }
            HtmlLineToken::Close { tag } => {
                let open_tag = open_line
                    .zip(rope)
                    .map(|(push_line, rope)| {
                        crate::projection::Snapshot::with_line_str(
                            rope.line(push_line as usize),
                            |s| extract_html_tagname(s).to_string(),
                        )
                    })
                    .unwrap_or_default();
                if open_tag.eq_ignore_ascii_case(tag) {
                    has_pop = true;
                    if matches!(primary, CompositePrimary::Continue) {
                        primary = CompositePrimary::Pop;
                    }
                }
            }
            HtmlLineToken::Inline => { has_open = true; }
        }
    }
    HtmlElementAnalysis { primary, nested, has_pop, has_open }
}

fn he_is_composite(a: &HtmlElementAnalysis) -> bool {
    a.has_pop || (a.has_open && (!matches!(a.primary, CompositePrimary::Continue) || !a.nested.is_empty()))
}

/// HtmlElement classify（单级化）——analyze 一次，三臂共用（原
/// html_element_composite_outcome 并入——双跑消除）。
pub(crate) fn classify_html_element(facts: &LineFacts, _frame: &BlockFrame, rt: &ScanRuntime) -> Option<(BlockType, ScanAction)> {
    let a = analyze_html_element(facts.content, rt.top_open_line(), rt.rope);
    if he_is_composite(&a) {
        let outer_type = if a.has_open { BlockType::HtmlElement } else { BlockType::HtmlContent };
        let n_nested = a.nested.len();
        Some((outer_type, ScanAction::Composite { outer_type, primary: a.primary, nested: a.nested,
            // consume 位声明不保证：同上——HELEM 复合行 nested 位移恒 0。
            nested_consumes: vec![0; n_nested], primary_consume: 0 }))
    } else if a.has_open {
        Some((BlockType::HtmlElement, ScanAction::Continue))
    } else {
        Some((BlockType::HtmlContent, ScanAction::Continue))
    }
}

/// Type 1-5 have explicit end markers; type 6-7 end on blank line.
/// Blank-line ending is handled in step 1 (blank line detection),
/// so here we only check for explicit end markers.
fn html_block_ends(block_type: u8, content: &[u8]) -> bool {
    match block_type {
        1 => {
            // Type 1 ends with </script>, </pre>, </style>, or </textarea> (case-insensitive)
            // TAGS 与 detect_html_block_type 的 type 1 常量统一（HTML_TYPE1_TAGS）
            HTML_TYPE1_TAGS.iter().any(|tag| {
                content.windows(tag.len() + 3).any(|w| {
                    w.starts_with(b"</") && w.ends_with(b">")
                        && w[2..2 + tag.len()].eq_ignore_ascii_case(tag)
                })
            })
        }
        2 => {
            // <!-- → search for -->
            memchr::memmem::find(content, b"-->").is_some()
        }
        3 => {
            // <? → search for ?>
            memchr::memmem::find(content, b"?>").is_some()
        }
        4 => {
            // <! → ends at first >
            content.contains(&b'>')
        }
        5 => {
            // <![CDATA[ → search for ]]>
            memchr::memmem::find(content, b"]]>").is_some()
        }
        _ => false, // type 6-7: end on blank line (handled in step 1)
    }
}

/// Check if a byte slice is a thematic break candidate (---, ***, ___, with optional spaces).
/// thematic_break_matches 统一判定（dispatch_lut/list_outcome 共用——单源）。
/// pub(crate)（sdl.rs SDLScan 共享）。
#[inline]
pub(crate) fn thematic_break_matches(content: &[u8]) -> bool {
    let c = content[0];
    if !matches!(c, b'-' | b'*' | b'_') { return false; }
    let mut count = 0u8;
    let mut all_valid = true;
    for &b in content {
        if b == c { count += 1; }
        else if b != b' ' && b != b'\t' { all_valid = false; break; }
    }
    count >= 3 && all_valid
}

// ── Peel/Classify/Action enums ──

#[derive(Debug, Clone, Copy, PartialEq)]
/// pub(crate)（Inside 近似实验）。
pub enum ListRel { Outside, Inside }

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BqRel { Outside, InsideHasPrefix, InsideNoPrefix }

#[derive(Debug, Clone, Copy, PartialEq)]
/// pub(crate)（sdl.rs SDLScan 共享）。
pub(crate) enum ListContent {
    SameListItem,       // 同类 marker
    OtherListItem,      // BLT 内 OL 或反之
    NonListContainer,   // CF, Table, HtmlBlock 等（frame_kind 存在且非 nesting）
    NestingContainer,   // BQ, 另一个 BLT/OL
    NonContainer,       // Header, HR, IC
    Paragraph,
    IndentedCode,       // item 段落结束后（Blank 清 para）相对缩进 ≥4 列
    /// lazy 续行（CM §5.2）：item 段落打开（前一行非 Blank/IC）且缩进不足
    /// （Outside）——无缩进续行并入 item 段落（不 Pop——区别于列表外）。
    LazyParagraph,
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// pub(crate)（sdl.rs SDLScan 共享）。
pub(crate) enum BqContent { Paragraph, Container, NonContainer }

// ── Byte match helpers ──

pub(crate) fn is_bullet_match(after_ws: &[u8], bullet_code: u8) -> bool {
    is_bullet_marker(after_ws) && {
        let expected = match bullet_code {
            1 => b'*', 2 => b'+', _ => b'-',
        };
        after_ws[0] == expected
    }
}

pub(crate) fn is_bullet_any(after_ws: &[u8]) -> bool {
    is_bullet_marker(after_ws)
}

pub(crate) fn is_ordered_match(after_ws: &[u8], delim_code: u8) -> bool {
    ordered_list_matches(after_ws).is_some() && {
        let d = number_digits(after_ws);
        d < after_ws.len() && {
            let delim = after_ws[d];
            let expected_delim = if delim_code == 1 { b')' } else { b'.' };
            delim == expected_delim
        }
    }
}

pub(crate) fn is_ordered_any(after_ws: &[u8]) -> bool {
    ordered_list_matches(after_ws).is_some()
}

/// 真空 item 哨兵判定（u8 帧字段形态）。
#[inline]
pub(crate) fn is_empty_item_sentinel(indent: u8) -> bool {
    indent as usize == EMPTY_ITEM_SENTINEL
}

// ── Peel functions ──

/// 行在 peel 坐标系下的有效列（effective_cmp——list_peel/约束链共用）。
/// peel 已算时传入 (peeled, peel_columns)——避免重复 peel_lower_prefixes。
/// #261（cmark 佐证）：行首缩进在行是 BQ 前缀行时是 BQ 标记的一部分
/// （strip_bq_one skip 吃掉——`  >  > two` 剥后 two 缩进 0——空行后 item
/// 续行判定不虚高——官方 two 是 BQ2 段落 li 外）；段落打开时的 lazy 由
/// classify 的 LazyParagraph 兜底（`> - b\n  > cont` 同样 li 内）。
/// `peel_columns` 与 `compute_indent(peeled)` 均以列计算（tab 展开），
/// 不再把剥除字节数当列用。
#[inline]
pub(crate) fn effective_cmp_after_peel(
    facts: &LineFacts,
    peeled: &[u8],
    peel_columns: usize,
) -> usize {
    let peeled_ws_columns = compute_indent(peeled);
    facts.indent_col
        .saturating_sub(peel_columns)
        .max(peeled_ws_columns)
}


/// 列表阈值单源（#279 真空 item 63 哨兵）：紧接行（prev 非空——item 段落
/// 打开）用 marker_col+1（1）；空行后（prev=None——列表外）保持哨兵。
/// list_peel 与 classify_list 共享——几何域禁止双源判定。
#[inline]
pub(crate) fn list_effective_threshold(frame: &BlockFrame, prev_type: Option<BlockType>) -> usize {
    if frame.indent() as usize == EMPTY_ITEM_SENTINEL {
        if prev_type.is_some() { 1 } else { EMPTY_ITEM_SENTINEL }
    } else {
        frame.indent() as usize
    }
}

/// pub(crate)（Inside 近似实验）。
pub(crate) fn list_peel<'a>(facts: &LineFacts<'a>, frame: &BlockFrame, stack: &[BlockFrame])
    -> (ListRel, usize, &'a [u8])
{
    let (peeled, peel_columns) = peel_lower_prefixes(facts.content, stack);
    // §AX.4: 行缩进统一以 peel 坐标原点（内容坐标）为基准——frame.indent 是
    // 内容坐标（BQ 剥除后 marker 列），比较必须同坐标系，否则 BQ 内 item 内容
    // 续行（≥ marker 内容起点）会因绝对列 +2 偏移与 threshold 错位。
    let mut effective_cmp = effective_cmp_after_peel(facts, peeled, peel_columns);
    let after_ws = skip_leading_whitespace(peeled);
    // I4 修（终版）：bq 内列表的比较轴 = 列表自己的坐标——
    // **仅当行内容是列表 marker（兄弟候选）**时减外层 item 的内容原点（其阈值），
    // 且须「列表之上有 BQ 层 + 栈内有外层 item 与内层列表两个列表帧」。
    // 非 marker 行（BQ 续行等）不减——n5 的 "        > > x" 曾因过宽被误弹列表。
    let is_marker_line = is_list_marker(after_ws) || is_list_marker(peeled);
    let list_frames: Vec<&BlockFrame> = stack
        .iter()
        .filter(|f| crate::effect::is_list_container(f.kind()))
        .collect();
    if is_marker_line && list_frames.len() >= 2 {
        // 精确门（回归修正 #2/#3）：只对「item → 恰一层 bq → 列表」的 p5 形态减——
        // 列表之下 bq 层数 == 1（deep_cross 的 B4 链有两层 bq、真嵌套不减；
        // 双 bq 形态——其坐标链算术更深）。且 marker 须恰落外层 item 的
        // 内容原点（兄弟位置）。
        let bq_below = stack[..stack.len().saturating_sub(1)]
            .iter()
            .filter(|f| f.kind() == FrameKind::BlockQuote)
            .count();
        if bq_below == 1 {
            let item_origin = list_frames[0].indent() as usize;
            if effective_cmp == item_origin {
                effective_cmp = 0;
            }
        }
    }
    let eff_threshold = list_effective_threshold(frame, facts.prev_type);
    let rel = if effective_cmp < eff_threshold { ListRel::Outside }
              else { ListRel::Inside };
    (rel, effective_cmp, after_ws)
}

/// Check if original line content has a `>` prefix (after optional leading whitespace).
/// Used to gate BQ lazy continuation: lazy 行必须无任何 > 前缀。
#[inline]
pub(crate) fn has_gt_prefix(content: &[u8]) -> bool {
    skip_leading_whitespace(content).first() == Some(&b'>')
}

pub(crate) fn bq_peel<'a>(facts: &LineFacts<'a>, stack: &[BlockFrame])
    -> (BqRel, &'a [u8])
{
    let (peeled, _) = peel_lower_prefixes(facts.content, stack);
    if has_gt_prefix(facts.content) {
        // 符合性（CM §5.2——cmark 确认 `> > x\n> bar` → bar 第 2 层）：
        // 行的 > 匹配栈顶（最内层）——部分前缀行（> 数 < BQ 层数）剥给栈顶——
        // 其余外层 lazy（段落覆盖）——原 peel_lower_prefixes 从外剥（栈下层）
        // 行的 > 给错层——strip 失败判 Outside——误弹——剥 min(BQ 层数,
        // 行的 > 数)——多出的 >（行 > 层）由 dispatch 处理（嵌套开行）。
        // #261（cmark 佐证）：每层剥取前 skip ws——`>  > two`（`>` 后多余
        // 空格）的 BQ2 前缀 ` >` 在空格后也能剥——否则残留 `>` 被当嵌套
        // BQ 开行（MDIO 原误开 BQ3——官方 two 是 BQ2 段落）。
        let n_layers = stack.iter()
            .filter(|f| f.kind() == FrameKind::BlockQuote).count();
        // wl4：`>` 前缀存在 ≠ 列对得上——列不匹配时行不属于这些 BQ 层，
        // 让 Pop 循环关 BQ（cmark：外层 item 内容列 6 时 `    > x` 在 4 应关 BQ/关 item，
        // 在 6/8/10 续行）。
        if n_layers > 0 && !bq_prefix_columns_ok(facts, stack) {
            return (BqRel::Outside, peeled);
        }
        let mut content = facts.content;
        for _ in 0..n_layers {
            let before = content;
            content = strip_bq_one(content);
            if content.as_ptr() == before.as_ptr() { break; }
        }
        // 空 BQ 行（`>`——剥后空）在多层栈：cmark 弹 BQ2（text 重开——分开——
        // bq_inner_text_para_after_fence 测试——CF 与 para 的 BQ2 分开）——
        // 保持旧路径（Outside——BLANK_NOPREFIX——弹）——剥后非空的才用
        // 新语义（内容归属栈顶）
        if skip_leading_whitespace(content).is_empty() && n_layers > 1 {
            (BqRel::Outside, peeled)
        } else {
            (BqRel::InsideHasPrefix, content)
        }
    } else if !has_gt_prefix(facts.content)
        // CM §5.1：lazy continuation 只覆盖打开的段落——空行中断段落（prev_type
        // 归一化 None=Blank）；prev 是段落闭包类型（closes_paragraph——列⑤——
        // CodeContent/CodeFenceClose 等）时无 lazy——两者后都无 BQ lazy（词法
        // 真相——para_open 位不存在——prev 词法独立判定——查询域 = prev 等价域定理）。
        && facts.prev_type.is_some_and(|t| !crate::effect::block_decl(t).closes_paragraph)
    {
        (BqRel::InsideNoPrefix, peeled)
    } else {
        (BqRel::Outside, peeled)
    }
}

/// BQ 前缀剥取一层（cmark 语义）：skip ws 后剥 `>` + 可选 1 空白——
/// `>  > two` 的 BQ2 前缀 ` >` 在多余空格后也能剥（#261——原 strip 要求
/// 行首 `>`——多余空格使前缀残留——误判嵌套 BQ 开行）。
/// 返回剥取后的 content；非 BQ 前缀（剥前）时返回原样（指针相等判定）。
#[inline]
pub(crate) fn strip_bq_one<'a>(content: &'a [u8]) -> &'a [u8] {
    // ⑤a：层检测单源——step_layer 取代手写 skip-ws＋parse_bq（行为等价：
    // BQ 宽=ws+1+has_trailing；非 BQ 层原样返回）。
    match step_layer(content) {
        Some((FrameKind::BlockQuote, consumed)) => &content[consumed..],
        _ => content,
    }
}

/// 列②：BQ 约束链豁免——段落延续（prev 词法——para_open 位不存在——
/// !prev_closes 等价：前一行非段落闭包——`> > foo\nlazy` 嵌套 BQ 场景
/// prev=Paragraph ✓；`> > - item\nlazy` prev=Bullet 不 closes → 豁免 ✓）。
/// classify 兜底（bq_peel prev 词法——豁免过宽无害）。
/// #236/#238（符合性裁决——cmark 佐证）：能打断段落的行不豁免（`- bar` 新
/// 列表——`> foo\n- bar` bar 顶层——`> ```\nfoo` foo 顶层）；不能打断的
/// （`2. bar`——OL number≠1——CM §5.3——lazy 文本——BQ 段落）豁免——
/// interrupts_para 统一判定（与 classify_bq_scene 的 lazy 打断一致）。
// ── B10：BQ lazy 豁免行表（五谓词短路序；R6 默认 true）──

struct BqLazyRow {
    empty: Option<bool>,
    gt: Option<bool>,
    interrupt: Option<bool>,
    prev_none: Option<bool>,
    prev_closes: Option<bool>,
    out: bool,
}

const BQ_LAZY_ROWS: &[BqLazyRow] = &[
    BqLazyRow { empty: Some(true), gt: None, interrupt: None, prev_none: None, prev_closes: None, out: false },
    BqLazyRow { empty: None, gt: Some(true), interrupt: None, prev_none: None, prev_closes: None, out: false },
    BqLazyRow { empty: None, gt: None, interrupt: Some(true), prev_none: None, prev_closes: None, out: false },
    // prev==None：is_some_and 失败——不豁免（与 prev 闭段同结论、不同键——铁律：键不合并语义）
    BqLazyRow { empty: None, gt: None, interrupt: None, prev_none: Some(true), prev_closes: None, out: false },
    BqLazyRow { empty: None, gt: None, interrupt: None, prev_none: None, prev_closes: Some(true), out: false },
    // 豁免（原兜底收紧——第一方覆盖）：非空∧无前缀∧不打断∧prev 存在∧prev 不闭段
    BqLazyRow { empty: Some(false), gt: Some(false), interrupt: Some(false), prev_none: Some(false), prev_closes: Some(false), out: true },
];

pub(crate) fn bq_lazy_candidate(facts: &LineFacts, _frame: &BlockFrame, stack: &[BlockFrame]) -> bool {
    let after_ws = skip_leading_whitespace(facts.content);
    let empty = after_ws.is_empty();
    let gt = has_gt_prefix(facts.content);
    // 与 classify_bq_scene 同基准：打断 = 相对最内层 BQ 内容列的偏移（Σ 栈内
    // 全部 BQ frame.body = 最内层绝对内容列）；offset > 0 恒 lazy 文本，
    // <= 0 按内容判定打断（cmark 实证）。
    let content_col: usize = stack.iter()
        .filter(|f| f.kind() == FrameKind::BlockQuote)
        .map(|f| f.body() as usize)
        .sum();
    let offset = facts.indent_col().saturating_sub(content_col);
    let interrupt = !empty
        && offset <= 0
        && crate::effect::can_interrupt(dispatch_lut(after_ws), after_ws, 0);
    let prev_none = facts.prev_type.is_none();
    let prev_closes = facts.prev_type
        .is_some_and(|t| crate::effect::block_decl(t).closes_paragraph);
    BQ_LAZY_ROWS
        .iter()
        .find(|r| {
            let ok = |k: Option<bool>, v: bool| k != Some(!v);
            ok(r.empty, empty) && ok(r.gt, gt) && ok(r.interrupt, interrupt)
                && ok(r.prev_none, prev_none) && ok(r.prev_closes, prev_closes)
        })
        .expect("B10 未覆盖键组合＝方案缺陷（第一方覆盖）")
        .out
}

/// 列②：列表约束链豁免——"行被列表接受"：Inside（嵌套/内容——`  - bar` 嵌套、
/// `  bar` item 段落）或 Outside 非 marker（lazy 段落——`lazy` item 续行）→ 豁免
/// BQ 前缀约束；Outside marker（`- bar`）→ 不豁免（打断路径——BQ 列⑥判定）。
/// #238 配套（cmark 佐证 `- <div>\n  x\nbar` → bar 顶层）：Outside 非 marker
/// 的 lazy 需要段落打开（prev 不 closes）——`bar` 在 HtmlBlock 内容后
/// （prev=HtmlContent——closes）——不豁免——弹列表——官方顶层。
// ── B11：LIST lazy 豁免行表（短路序＝旧判定序；R5 默认 false）──

struct ListLazyRow {
    empty: Option<bool>,
    gt: Option<bool>,
    marker: Option<bool>,
    inside: Option<bool>,
    outside: Option<bool>,
    lazy: Option<bool>,
    out: bool,
}

/// 豁免律（行序 3 行＋默认 false——第一方覆盖；原 64 格位掩码折叠：
/// R2 8 格（gt∧¬marker，含不可达 inside∧outside 格同语义）、R3 2 格（¬gt∧Inside∧lazy——
/// gt 约束是原格值：gt∧marker∧Inside∧lazy 原为 false）、R4 1 格（¬marker∧Outside∧lazy））。
static LIST_LAZY_ROWS: &[ListLazyRow] = &[
    // R2 gt∧¬marker：BQ 前缀非 marker 行由 BQ 层承载（B 场景 `>      >   # H5`）
    ListLazyRow { empty: Some(false), gt: Some(true), marker: Some(false), inside: None, outside: None, lazy: None, out: true },
    // R3 ¬gt∧Inside∧lazy
    ListLazyRow { empty: Some(false), gt: Some(false), marker: None, inside: Some(true), outside: None, lazy: Some(true), out: true },
    // R4 ¬marker∧Outside∧lazy
    ListLazyRow { empty: Some(false), gt: None, marker: Some(false), inside: None, outside: Some(true), lazy: Some(true), out: true },
    // 默认 false（空行/其余组合不豁免）
];

pub(crate) fn list_lazy_candidate(facts: &LineFacts, frame: &BlockFrame, stack: &[BlockFrame]) -> bool {
    let (rel, _, after_ws) = list_peel(facts, frame, stack);
    let empty = after_ws.is_empty();
    let gt = has_gt_prefix(facts.content);
    let marker = !empty && is_list_marker(after_ws);
    // 无 BQ 前缀时只有 lazy 段落（dispatch_lut == Paragraph 且 prev 不闭段）豁免外层约束。
    let lazy = !empty
        && dispatch_lut(after_ws) == BlockType::Paragraph
        && facts.prev_type.is_some_and(|t| !crate::effect::block_decl(t).closes_paragraph);
    LIST_LAZY_ROWS
        .iter()
        .find(|r| {
            let ok = |k: Option<bool>, v: bool| k != Some(!v);
            ok(r.empty, empty)
                && ok(r.gt, gt)
                && ok(r.marker, marker)
                && ok(r.inside, rel == ListRel::Inside)
                && ok(r.outside, rel == ListRel::Outside)
                && ok(r.lazy, lazy)
        })
        .map_or(false, |r| r.out)
}

/// 列表 marker 行判定（Bullet/OL/Checklist——`- bar`/`2. bar`/`- [ ] x`）。
/// 空防御（SDL 工具第 6 个真 bug）：
/// `> - a\n>   - b\n> ` 的 `> ` 行在 [BQ, BLT, BLT]——peel 剥 BQ 前缀后
/// after_ws 空——dispatch_lut(空) 越界 panic（真实输入触发）——空行不是
/// marker——false——list_lazy_candidate 豁免（classify 兜底 BLANK——
/// CM：带前缀空行被列表吞——loose 检测）。
/// pub(crate)（sdl.rs 豁免组感知原子共享）。
pub(crate) fn is_list_marker(after_ws: &[u8]) -> bool {
    if after_ws.is_empty() { return false; }
    matches!(dispatch_lut(after_ws),
        BlockType::Bullet | BlockType::OrderedListItem | BlockType::Checklist)
}

// ── Classify functions ──

/// pub(crate)（sdl.rs SDLScan 共享）。
// ── B4：list 内容类行表（前置 setext 调整＋18 行；行序＝旧判定序；R18 默认 NonContainer）──

/// item 内 setext 下划线调整（③ 族同型——CM §4.2 #301）：`---` 在 item 内且 prev 段落 → Paragraph。
#[inline]
pub(crate) fn list_setext_adjust(
    t: BlockType,
    after_ws: &[u8],
    indent: usize,
    eff_threshold: usize,
    prev: Option<BlockType>,
) -> BlockType {
    if t == BlockType::ThematicBreak
        && !after_ws.is_empty()
        && after_ws[0] == b'-'
        && all_hs_char(after_ws, b'-')
        && indent >= eff_threshold
        && prev.is_some_and(|p| matches!(p, BlockType::Paragraph | BlockType::Blockquote))
    {
        return BlockType::Paragraph;
    }
    t
}

struct ListContentRow {
    t_bq: Option<bool>,
    t_para: Option<bool>,
    nesting_framed: Option<bool>,
    framed: Option<bool>,
    kind_blt: Option<bool>,
    kind_ol: Option<bool>,
    bullet_match: Option<bool>,
    bullet_any: Option<bool>,
    ordered_match: Option<bool>,
    ordered_any: Option<bool>,
    bq_rel_gt: Option<bool>,
    para_closed: Option<bool>,
    vacuum: Option<bool>,
    ge_ic: Option<bool>,
    ge_content: Option<bool>,
    lt_eff: Option<bool>,
    out: ListContent,
}

/// 行序铁律：R2 先于 R3（BQ 特判）；marker 臂先于 t 臂；R10/R11 先于 R12；R13/R14 先于 R15。
const LIST_CONTENT_ROWS: &[ListContentRow] = &[
    // R2 BQ∧缩进过深∧段闭 → IC（`- B:\n      > x` 段落关闭 → item 内 IC）
    ListContentRow { t_bq: Some(true), t_para: None, nesting_framed: None, framed: None, kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: Some(true), para_closed: Some(true), vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::IndentedCode },
    // R3 BQ∧缩进过深∧段开 → lazy 段落文本
    ListContentRow { t_bq: Some(true), t_para: None, nesting_framed: None, framed: None, kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: Some(true), para_closed: Some(false), vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::LazyParagraph },
    // R4-R6 BLT marker
    ListContentRow { t_bq: None, t_para: None, nesting_framed: None, framed: None, kind_blt: Some(true), kind_ol: None, bullet_match: Some(true), bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::SameListItem },
    ListContentRow { t_bq: None, t_para: None, nesting_framed: None, framed: None, kind_blt: Some(true), kind_ol: None, bullet_match: None, bullet_any: Some(true), ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::OtherListItem },
    ListContentRow { t_bq: None, t_para: None, nesting_framed: None, framed: None, kind_blt: Some(true), kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: Some(true), bq_rel_gt: None, para_closed: None, vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::OtherListItem },
    // R7-R9 OL marker
    ListContentRow { t_bq: None, t_para: None, nesting_framed: None, framed: None, kind_blt: None, kind_ol: Some(true), bullet_match: None, bullet_any: None, ordered_match: Some(true), ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::SameListItem },
    ListContentRow { t_bq: None, t_para: None, nesting_framed: None, framed: None, kind_blt: None, kind_ol: Some(true), bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: Some(true), bq_rel_gt: None, para_closed: None, vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::OtherListItem },
    ListContentRow { t_bq: None, t_para: None, nesting_framed: None, framed: None, kind_blt: None, kind_ol: Some(true), bullet_match: None, bullet_any: Some(true), ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::OtherListItem },
    // R10-R12 Paragraph∧真空 item（#279——`-\n      baz` → 官方 li>pre）
    ListContentRow { t_bq: None, t_para: Some(true), nesting_framed: None, framed: None, kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: Some(true), ge_ic: Some(true), ge_content: None, lt_eff: None, out: ListContent::IndentedCode },
    ListContentRow { t_bq: None, t_para: Some(true), nesting_framed: None, framed: None, kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: Some(false), vacuum: Some(true), ge_ic: None, ge_content: None, lt_eff: Some(true), out: ListContent::LazyParagraph },
    ListContentRow { t_bq: None, t_para: Some(true), nesting_framed: None, framed: None, kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: Some(true), ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::Paragraph },
    // R13-R15 Paragraph∧非真空
    ListContentRow { t_bq: None, t_para: Some(true), nesting_framed: None, framed: None, kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: Some(true), vacuum: Some(false), ge_ic: None, ge_content: Some(true), lt_eff: None, out: ListContent::IndentedCode },
    ListContentRow { t_bq: None, t_para: Some(true), nesting_framed: None, framed: None, kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: Some(false), vacuum: Some(false), ge_ic: None, ge_content: None, lt_eff: Some(true), out: ListContent::LazyParagraph },
    ListContentRow { t_bq: None, t_para: Some(true), nesting_framed: None, framed: None, kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: Some(false), ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::Paragraph },
    // R16-R18 非段落
    ListContentRow { t_bq: None, t_para: None, nesting_framed: Some(true), framed: None, kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::NestingContainer },
    ListContentRow { t_bq: None, t_para: None, nesting_framed: None, framed: Some(true), kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::NonListContainer },
    // R18 无帧非容器非段落非 BQ（IC/ThematicBreak/RefDef 等）→ NonContainer
    // （原兜底收紧——第一方覆盖：四键显式）
    ListContentRow { t_bq: Some(false), t_para: Some(false), nesting_framed: Some(false), framed: Some(false), kind_blt: None, kind_ol: None, bullet_match: None, bullet_any: None, ordered_match: None, ordered_any: None, bq_rel_gt: None, para_closed: None, vacuum: None, ge_ic: None, ge_content: None, lt_eff: None, out: ListContent::NonContainer },
];

pub(crate) fn classify_list(facts: &LineFacts, after_ws: &[u8], kind: FrameKind, frame: &BlockFrame, indent: usize) -> ListContent {
    // 列表阈值单源（#279 真空 item——同 list_peel）。
    let eff_threshold = list_effective_threshold(frame, facts.prev_type);
    // 前置调整（③ 族）：item 内 setext 下划线 → Paragraph（#301——CM §4.2）。
    let t = list_setext_adjust(
        dispatch_lut(after_ws),
        after_ws,
        indent,
        eff_threshold,
        facts.prev_type,
    );
    let para_closed = facts.prev_type.map_or(true, |p| {
        crate::effect::block_decl(p).closes_paragraph
    });
    let vacuum = frame.indent() as usize == EMPTY_ITEM_SENTINEL;
    let bq_rel_gt = t == BlockType::Blockquote
        && indent >= eff_threshold
        && indent.saturating_sub(eff_threshold) > STRUCT_INDENT_MAX;
    let f = (
        t == BlockType::Blockquote,
        t == BlockType::Paragraph,
        frame_kind(t).map_or(false, |k| crate::effect::is_nesting_container(k)),
        frame_kind(t).is_some(),
        kind == FrameKind::BulletList,
        kind == FrameKind::OrderedList,
        is_bullet_match(after_ws, frame.bullet_code()),
        is_bullet_any(after_ws),
        is_ordered_match(after_ws, frame.delim_code()),
        is_ordered_any(after_ws),
        bq_rel_gt,
        para_closed,
        vacuum,
        indent >= eff_threshold + crate::effect::geo::LIST_IC_OFFSET,
        indent >= eff_threshold + crate::effect::geo::LIST_CONTENT_MAX,
        indent < eff_threshold,
    );
    LIST_CONTENT_ROWS
        .iter()
        .find(|r| {
            let ok = |k: Option<bool>, v: bool| k != Some(!v);
            ok(r.t_bq, f.0)
                && ok(r.t_para, f.1)
                && ok(r.nesting_framed, f.2)
                && ok(r.framed, f.3)
                && ok(r.kind_blt, f.4)
                && ok(r.kind_ol, f.5)
                && ok(r.bullet_match, f.6)
                && ok(r.bullet_any, f.7)
                && ok(r.ordered_match, f.8)
                && ok(r.ordered_any, f.9)
                && ok(r.bq_rel_gt, f.10)
                && ok(r.para_closed, f.11)
                && ok(r.vacuum, f.12)
                && ok(r.ge_ic, f.13)
                && ok(r.ge_content, f.14)
                && ok(r.lt_eff, f.15)
        })
        .expect("B4 未覆盖键组合＝方案缺陷（第一方覆盖）")
        .out
}

/// pub(crate)（sdl.rs SDLScan 共享）。
// ── B5：BQ 内容类行表（has_prefix×t 类 → BqContent；R4 默认 NonContainer）──

struct BqContentRow {
    has_prefix: Option<bool>,
    t_para: Option<bool>,
    framed: Option<bool>,
    out: BqContent,
}

/// 行序铁律：R1（lazy 恒段落）先行；R2 先于 R3；R4 默认行。
const BQ_CONTENT_ROWS: &[BqContentRow] = &[
    // R1 lazy（无 > 前缀）恒段落文本（CM §5.1 例 11——marker 行是段落文本，非子列表/IC）
    BqContentRow { has_prefix: Some(false), t_para: None, framed: None, out: BqContent::Paragraph },
    // R2 t==Paragraph → Paragraph
    BqContentRow { has_prefix: Some(true), t_para: Some(true), framed: None, out: BqContent::Paragraph },
    // R3 framed → Container
    BqContentRow { has_prefix: Some(true), t_para: None, framed: Some(true), out: BqContent::Container },
    // R4 有前缀∧非段落∧无帧 → NonContainer（原兜底收紧——第一方覆盖）
    BqContentRow { has_prefix: Some(true), t_para: Some(false), framed: Some(false), out: BqContent::NonContainer },
];

pub(crate) fn classify_bq(after_ws: &[u8], indent: usize, para_open: bool, has_prefix: bool) -> BqContent {
    // "as raw md reparse"：有 > 前缀的内容用完整顶层分类（含 setext 下划线识别）。
    let t = if has_prefix {
        classify_in_container(after_ws, indent, para_open)
    } else {
        BlockType::Paragraph
    };
    let f = (has_prefix, t == BlockType::Paragraph, frame_kind(t).is_some());
    BQ_CONTENT_ROWS
        .iter()
        .find(|r| {
            let ok = |k: Option<bool>, v: bool| k != Some(!v);
            ok(r.has_prefix, f.0) && ok(r.t_para, f.1) && ok(r.framed, f.2)
        })
        .expect("B5 未覆盖键组合＝方案缺陷（第一方覆盖）")
        .out
}

// ── Build helpers ──

/// item 行 marker 的 Markered（recognize_marker 单源）。
fn item_marker<'a>(kind: FrameKind, marker_line: &'a [u8]) -> Option<Markered<'a>> {
    let mt = if kind == FrameKind::OrderedList {
        BlockType::OrderedListItem
    } else {
        BlockType::Bullet
    };
    let m = recognize_marker(marker_line, 0)?;
    if m.node_type == mt { Some(m) } else { None }
}

/// item 行内容块的 IC 判定（Bug B 的 BQ 内版本）：marker 后空白列（tab 展开、
/// 从 marker 列起算——measure 的 indent_col）≥ marker 列+5 → 内容块是
/// indented code——调用者把 outer_type 覆盖为 IndentedCode（与顶层 after_exact
/// 判定对称——`> -    foo` → bq > bl > b{ ic }）。
/// 阈值判定收敛到 `first_content_is_ic(Markered)`（recognize_marker 单源）。
fn item_first_content_is_ic(kind: FrameKind, marker_line: &[u8]) -> bool {
    item_marker(kind, marker_line).is_some_and(|m| first_content_is_ic(&m))
}

fn resolve_item_chain(
    mode: ListPrimaryMode,
    item_type: BlockType,
    primary_kind: FrameKind,
    new_frame: &BlockFrame,
    raw_origin: usize,
    after_marker: &[u8],
    facts: &LineFacts<'_>,
) -> NestedResolve {
    let origin = ChainOrigin {
        raw: raw_origin,
        peel: new_frame.indent() as usize,
    };
    let entry = ChainEntry::ListPrimary {
        mode,
        first_type: item_type,
        primary_kind,
        origin,
        initial_rel: None,
        initial_raw_col: None,
    };
    resolve_nested_chain(after_marker, entry, facts)
}

fn build_pop_push(frame: &BlockFrame, after_ws: &[u8], effective_indent: usize, stack: &[BlockFrame], facts: &LineFacts<'_>)
    -> Option<(BlockType, ScanAction)>
{
    let item_type = open_list_item_type(after_ws, frame.kind());
    let marker = item_marker(frame.kind(), after_ws);
    let (threshold, body) = match marker {
        Some(m) => (
            list_frame_threshold(effective_indent, &m, first_content_is_ic(&m)) as u8,
            if frame.loose() { m.frame_body | LOOSE_BIT as u8 } else { m.frame_body },
        ),
        None => {
            let m = measure(item_type, after_ws).unwrap_or(ScanMeasure { prefix_bytes: 0, indent_col: 0, frame_body: 0 });
            ((effective_indent + m.indent_col) as u8,
             if frame.loose() { m.frame_body | LOOSE_BIT as u8 } else { m.frame_body })
        }
    };
    let new_frame = BlockFrame::new(frame.kind(), threshold, body);

    let after_marker = after_nested_marker(item_type, after_ws).unwrap_or(after_ws);
    let raw_origin = marker.map(|m| effective_indent + m.marker_end_col)
        .unwrap_or(new_frame.indent() as usize);
    let res = resolve_item_chain(
        ListPrimaryMode::ItemPopPush, item_type, frame.kind(), &new_frame, raw_origin, after_marker, facts);

    // indent falls below deeper frames → let Pop loop handle
    if stack.iter().rev().skip(1).any(|f| (f.indent() as usize) > effective_indent) {
        return None;
    }
    if res.nested.is_empty() {
        return Some((res.outer_type, ScanAction::PopPush(new_frame)));
    }
    Some((res.outer_type, ScanAction::Composite {
        outer_type: res.outer_type,
        primary: CompositePrimary::PopPush(new_frame),
        nested: res.nested.into_vec(),
        nested_consumes: res.nested_consumes.into_vec(),
        primary_consume: after_ws.len() - after_marker.len(),
    }))
}

pub(crate) fn build_push_nested(frame: &BlockFrame, after_ws: &[u8], effective_indent: usize, facts: &LineFacts<'_>)
    -> Option<(BlockType, ScanAction)>
{
    let item_type = open_list_item_type(after_ws, frame.kind());
    let marker = item_marker(frame.kind(), after_ws);
    // #327（CM §5.2——#108 语义）：嵌套列表的 loose 独立判定——不继承父帧
    // loose 位（`- a\n  - b\n  - c\n\n- d\n  - e` 的 - e 嵌套在 loose 外层
    // 下——官方 tight——e/f 无 p）。原 `if frame.loose()` 继承在"外层已置
    // loose 后开嵌套"（#326 的空行后）时错置——嵌套的 own loose 由自身
    // 空行（pending）置位。
    let (threshold, body) = match marker {
        Some(m) => (
            list_frame_threshold(effective_indent, &m, first_content_is_ic(&m)) as u8,
            m.frame_body,
        ),
        None => {
            let m = measure(item_type, after_ws).unwrap_or(ScanMeasure { prefix_bytes: 0, indent_col: 0, frame_body: 0 });
            ((effective_indent + m.indent_col) as u8, m.frame_body)
        }
    };
    let new_frame = BlockFrame::new(frame.kind(), threshold, body);

    let after_marker = after_nested_marker(item_type, after_ws).unwrap_or(after_ws);
    let raw_origin = marker.map(|m| effective_indent + m.marker_end_col)
        .unwrap_or(new_frame.indent() as usize);
    let res = resolve_item_chain(
        ListPrimaryMode::ItemPushNested, item_type, frame.kind(), &new_frame, raw_origin, after_marker, facts);

    if res.nested.is_empty() {
        return Some((res.outer_type, ScanAction::Push(new_frame)));
    }
    Some((res.outer_type, ScanAction::Composite {
        outer_type: res.outer_type,
        primary: CompositePrimary::Push(new_frame),
        nested: res.nested.into_vec(),
        nested_consumes: res.nested_consumes.into_vec(),
        primary_consume: after_ws.len() - after_marker.len(),
    }))
}

// ── B17a：cross fiber 选择行表（kind×t_oli → 目标 fiber；R4 默认 Reject）──

/// 跨 fiber 换键（对齐 SDL cross_fiber_spec 的 match kind 形态——3 规则；原 8 格位掩码折叠）。
pub(crate) fn cross_fiber(kind: FrameKind, after_ws: &[u8]) -> Option<(FrameKind, BlockType)> {
    match kind {
        FrameKind::BulletList => {
            if dispatch_lut(after_ws) == BlockType::OrderedListItem {
                // BLT∧OLI → 跨 OL fiber
                Some((FrameKind::OrderedList, BlockType::OrderedListItem))
            } else {
                // BLT 其余 → 留在 BLT
                Some((
                    FrameKind::BulletList,
                    open_list_item_type(after_ws, FrameKind::BulletList),
                ))
            }
        }
        // OL→BLT（t 不限——§F.1）
        FrameKind::OrderedList => Some((
            FrameKind::BulletList,
            open_list_item_type(after_ws, FrameKind::BulletList),
        )),
        _ => None,
    }
}

pub(crate) fn build_push_cross_kind(kind: FrameKind, after_ws: &[u8], effective_indent: usize, facts: &LineFacts<'_>)
    -> Option<(BlockType, ScanAction)>
{
    let (nested_kind, nested_nt) = cross_fiber(kind, after_ws)?;
    let marker = item_marker(nested_kind, after_ws);
    let (threshold, body) = match marker {
        Some(m) => (
            list_frame_threshold(effective_indent, &m, first_content_is_ic(&m)) as u8,
            m.frame_body,
        ),
        None => {
            let m = measure(nested_nt, after_ws).unwrap_or(ScanMeasure { prefix_bytes: 0, indent_col: 0, frame_body: 0 });
            ((effective_indent + m.indent_col) as u8, m.frame_body)
        }
    };
    let new_frame = BlockFrame::new(nested_kind, threshold, body);

    let after_marker = after_nested_marker(nested_nt, after_ws).unwrap_or(after_ws);
    let raw_origin = marker.map(|m| effective_indent + m.marker_end_col)
        .unwrap_or(new_frame.indent() as usize);
    let res = resolve_item_chain(
        ListPrimaryMode::ItemPushCross, nested_nt, nested_kind, &new_frame, raw_origin, after_marker, facts);
    // PUSH_CROSS 只在 rel=Inside（M ≥ 内容列）到达——CM：marker 类型变化 = 新列表，
    // 位置由缩进决定（≥ 内容列 → item 内嵌套）——Push（嵌套）而非 PopPush（同层
    // 替换——那会结束当前列表——与 `- a\n\n  1. b` 的 CM 嵌套语义冲突）。
    if res.nested.is_empty() {
        return Some((nested_nt, ScanAction::Push(new_frame)));
    }
    let mut all_nested = vec![new_frame];
    all_nested.extend(res.nested);
    Some((nested_nt, ScanAction::Composite {
        outer_type: nested_nt,
        primary: CompositePrimary::Push(new_frame),
        nested: all_nested[1..].to_vec(),
        nested_consumes: res.nested_consumes.into_vec(),
        primary_consume: after_ws.len() - after_marker.len(),
    }))
}

pub(crate) fn build_composite_continue_push(after_ws: &[u8], effective_indent: usize, frame: &BlockFrame, facts: &LineFacts<'_>)
    -> Option<(BlockType, ScanAction)>
{
    let extra = effective_indent.saturating_sub(frame.indent() as usize);
    // extra > STRUCT_INDENT_MAX 时内容已按 IC 处理（classify_by_shape 会返回 IC），
    // resolver 同线 admit 同判据——这里显式拒绝，避免 CompositeContinue 产出空链。
    if extra > STRUCT_INDENT_MAX { return None; }
    let t = classify_by_shape(after_ws, extra);
    let t = check_hssep(t, after_ws);
    let Some(kind) = frame_kind(t) else { return None };
    let origin = ChainOrigin { raw: effective_indent, peel: effective_indent };
    let entry = ChainEntry::ListPrimary {
        mode: ListPrimaryMode::CompositeContinue,
        first_type: t,
        primary_kind: kind,
        origin,
        initial_rel: Some(extra),
        initial_raw_col: Some(effective_indent),
    };
    let res = resolve_nested_chain(after_ws, entry, facts);
    if res.nested.is_empty() { return None; }
    Some((res.outer_type, ScanAction::Composite {
        outer_type: res.outer_type,
        primary: CompositePrimary::Continue,
        nested: res.nested.into_vec(),
        nested_consumes: res.nested_consumes.into_vec(),
        primary_consume: 0,
    }))
}

// ── B12：BQ 续行 IC 惰性调整（CONTAINER_DECLS[BlockQuote].bq_ic_lazy 公式列；
// BlockQuote 行=ParaIfNoClose、其余 Keep——原 4 格判别式数组收编）──

pub(crate) fn bq_ic_lazy_adjust(ct: BlockType, prev_not_closes: bool) -> BlockType {
    match crate::effect::decl_for(FrameKind::BlockQuote).bq_ic_lazy {
        crate::effect::BqIcLazyRule::Keep => ct,
        crate::effect::BqIcLazyRule::ParaIfNoClose => {
            if prev_not_closes && ct == BlockType::IndentedCode {
                BlockType::Paragraph
            } else {
                ct
            }
        }
    }
}

pub(crate) fn dispatch_after_bq_prefix(after_gt: &[u8], facts: &LineFacts, _stack: &[BlockFrame])
    -> Option<(BlockType, ScanAction)>
{
    // bq 内单 dash 下划线的 prev-aware 拦截——prev 是段落
    // → HsSep（折叠 prev_is_para 转 HsHead2）；prev 非段落（空列表项）→ 交给
    // resolver（列表项照常）。此处必须先于 resolver——"-" 会被它当嵌套列表。
    let ws_gt = skip_leading_whitespace(after_gt);
    if ws_gt.len() == 1 && ws_gt[0] == b'-'
        && matches!(facts.prev_type, Some(BlockType::Paragraph | BlockType::Blockquote))
    {
        return Some((BlockType::HsSep, ScanAction::Continue));
    }
    // BQ 续行剥前缀后的 reparse 唯一走
    // resolve_nested_chain。after_gt 可能带前导空白；resolver 先 skip_ws
    // 再 dispatch——修复 `> # H\n>   # H2` 被降级为段落的问题。
    let res = resolve_nested_chain(after_gt, ChainEntry::BqContinuation, facts);
    if !res.nested.is_empty() {
        let outer = res.outer_type;
        return Some((outer, ScanAction::Composite {
            outer_type: outer,
            primary: CompositePrimary::Continue,
            nested: res.nested.into_vec(),
            nested_consumes: res.nested_consumes.into_vec(),
            primary_consume: 0,
        }));
    }
    // 旧语义保留：prev 不闭段落时，IC 形态的 BQ 续行是 lazy paragraph。
    let prev_not_closes = facts.prev_type
        .is_some_and(|t| !crate::effect::block_decl(t).closes_paragraph);
    let ct = bq_ic_lazy_adjust(res.final_content, prev_not_closes);
    if ct != BlockType::Paragraph {
        Some((ct, ScanAction::Continue))
    } else {
        None
    }
}

pub(crate) fn bq_para_continue() -> Option<(BlockType, ScanAction)> {
    // para_open 位不存在——段落打开由 prev 词法判定——直接
    // Continue（无 ContinueAs/置位——FrameLog 无 body_mut）。
    Some((BlockType::Paragraph, ScanAction::Continue))
}

// ── Action tables ──

/// Skip leading whitespace (ASCII space/tab + NBSP) without interpreting tabs as column positions.
/// pub(crate)（sdl.rs 感知原子共享）。
/// 基线位置扫描（逐字节）——所有平台语义基准。
fn skip_ws_baseline_pos(bytes: &[u8]) -> usize {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            i += 1;
        } else if bytes[i] == 0xC2 && i + 1 < bytes.len() && bytes[i + 1] == 0xA0 {
            i += 2; // NBSP U+00A0
        } else {
            break;
        }
    }
    i
}

/// NEON 加速（aarch64）——16B 窗口 vceq 精确比较（逐字节无借位链——正确性由构造保证）。
/// NBSP(0xC2 0xA0) 为伪非空白——命中后检查下一字节并 continue 继续扫描。
#[cfg(target_arch = "aarch64")]
fn skip_ws_neon_pos(bytes: &[u8]) -> usize {
    use std::arch::aarch64::*;
    unsafe {
        let sp = vdupq_n_u8(b' ');
        let tb = vdupq_n_u8(b'\t');
        const FULL: u64 = 0x0101_0101_0101_0101;
        let mut i = 0;
        while i + 16 <= bytes.len() {
            let v = vld1q_u8(bytes.as_ptr().add(i));
            let cmp = vorrq_u8(vceqq_u8(v, sp), vceqq_u8(v, tb));
            let u = vreinterpretq_u64_u8(cmp);
            let lo = vgetq_lane_u64(u, 0);
            let hi = vgetq_lane_u64(u, 1);
            if lo == 0xFFFF_FFFF_FFFF_FFFF && hi == 0xFFFF_FFFF_FFFF_FFFF {
                i += 16;
                continue;
            }
            // vceq 精确: 置位 = 空白——首个非空白 = 首个零通道
            let mut mask = (!lo) & FULL;
            let pos = if mask != 0 {
                (mask.trailing_zeros() / 8) as usize
            } else {
                mask = (!hi) & FULL;
                8 + (mask.trailing_zeros() / 8) as usize
            };
            let byte = bytes[i + pos];
            if byte == 0xC2 && i + pos + 1 < bytes.len() && bytes[i + pos + 1] == 0xA0 {
                i += pos + 2;
                continue;
            }
            return i + pos;
        }
        i
    }
    // 尾部 <16B 或全空白窗口耗尽后: 从 i 继续基线
    // （注: 上块 return i 后走此——全空白推进到此）
}

/// Skip leading whitespace (ASCII space/tab + NBSP) without interpreting tabs as column positions.
/// pub(crate)（sdl.rs 感知原子共享）。
pub(crate) fn skip_leading_whitespace(bytes: &[u8]) -> &[u8] {
    let i = skip_ws_pos(bytes);
    &bytes[i..]
}

/// 位置扫描入口：aarch64 → NEON（vceq 精确）；其他平台 → 基线。
/// x86_64 的 SSE2 版未验证（本机 aarch64 无法测）——实现验证后另行接入。
fn skip_ws_pos(bytes: &[u8]) -> usize {
    #[cfg(target_arch = "aarch64")]
    {
        let i = skip_ws_neon_pos(bytes);
        // NEON 循环退出时剩余必 <16B（或 0）——从 i 起基线补齐
        skip_ws_baseline_pos(&bytes[i..]) + i
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        skip_ws_baseline_pos(bytes)
    }
}

pub(crate) fn open_list_item_type(content: &[u8], kind: FrameKind) -> BlockType {
    match kind {
        FrameKind::BulletList => {
            // Check for checklist — validate bullet prefix via line_geometry
            if line_geometry(BlockType::Bullet, content).is_some()
                && is_checklist_marker(content)
            {
                return BlockType::Checklist;
            }
            BlockType::Bullet
        }
        FrameKind::OrderedList => BlockType::OrderedListItem,
        _ => BlockType::Paragraph,
    }
}
/// pub(crate)（sdl.rs admit 组感知原子共享）。
pub(crate) fn is_table_separator(inner: &[u8]) -> bool {
    if inner.is_empty() { return false; }
    // Strip trailing pipe and whitespace
    let effective = match inner.iter().rposition(|&b| b != b' ' && b != b'\t') {
        Some(pos) => pos,
        None => return false,
    };
    let inner = &inner[..=effective];
    let inner = if inner.last() == Some(&b'|') { &inner[..inner.len()-1] } else { inner };
    // All cells must be sep-like
    let mut start = 0;
    let mut has_cell = false;
    for (i, &b) in inner.iter().enumerate() {
        if b == b'|' {
            let cell = trim_ascii(&inner[start..i]);
            if !cell.iter().all(|&b| b == b'-' || b == b':' || b == b' ' || b == b'\t') { return false; }
            if !cell.is_empty() { has_cell = true; }
            start = i + 1;
        }
    }
    if start <= inner.len() {
        let cell = trim_ascii(&inner[start..]);
        if !cell.is_empty() {
            if !cell.iter().all(|&b| b == b'-' || b == b':' || b == b' ' || b == b'\t') { return false; }
            has_cell = true;
        }
    }
    has_cell
}

/// Find the closing `]` of a refdef label, skipping `\]` escape pairs.
/// Returns None if no unescaped `]` is found within 1000 bytes.
pub(crate) fn find_label_end(content: &[u8]) -> Option<usize> {
    let mut i = 1;
    while i < content.len() && i <= 1000 {
        if content[i] == b']' {
            // Count consecutive backslashes before `]`
            let mut bs = 0;
            let mut j = i;
            while j > 0 && content[j - 1] == b'\\' { bs += 1; j -= 1; }
            if bs % 2 == 0 { return Some(i); } // unescaped ]
        }
        i += 1;
    }
    None
}

/// Check if content has a refdef label prefix: `[label]:`.
/// Returns the byte index after `]:` on success.
/// If the line body can be fully parsed and has trailing non-whitespace,
/// returns None — the line is NOT a refdef (CM spec "no further characters").
fn refdef_prefix_matches(content: &[u8]) -> Option<usize> {
    if content.is_empty() || content[0] != b'[' { return None; }
    let close = find_label_end(content)?;
    // #547/#552（CM §4.7 label 规则）：label 非空 + 不含未转义 `[`
    // （`[]: /uri` 空 label 不成立——官方 p；`[ref[]: /uri` 的 label
    // 含未转义 `[` 不成立——官方 p）
    if close <= 1 { return None; }
    let mut i = 1;
    while i < close {
        if content[i] == b'\\' { i += 2; continue; }
        if content[i] == b'[' { return None; }
        i += 1;
    }
    if close + 1 >= content.len() || content[close + 1] != b':' { return None; }
    let after = &content[close + 2..];
    let body = crate::block::str_view(after).trim_start();
    if !body.is_empty() {
        if let Some((_, _, consumed)) = crate::inline::parse_dest_body(body) {
            let remaining = &body[consumed..];
            if remaining.as_bytes().iter().any(|&b| b != b' ' && b != b'\t') {
                // title 已闭合但后有多余内容（`'x' extra`——#211）→ 拒绝；
                // 未闭合（跨行 title——#197——CM §4.7）→ 接受
                if !unclosed_title(remaining) {
                    return None;
                }
            }
        } else {
            // #197：parse 失败——url 后的剩余是未闭合引号/括号（`/url '`）
            // ——title 跨行——接受（rd 候选——块级吸收由 classify_refdef +
            // 前端拼接完成）
            let bb = body.as_bytes();
            let mut uend = 0;
            while uend < bb.len() && bb[uend] != b' ' && bb[uend] != b'\t' { uend += 1; }
            let rest = &body[uend..];
            if !unclosed_title(rest) {
                return None;
            }
        }
    }
    Some(close + 2)
}

/// #197/#198/#211（CM §4.7）：title 引号/括号未闭合（跨行 title——接受）；
/// 已闭合但后有多余内容（拒绝）。pub(crate)（projection.rs 空 rd 撤销用）。
pub(crate) fn unclosed_title(rest: &str) -> bool {
    let r = rest.trim_start().as_bytes();
    (r.len() >= 1 && matches!(r[0], b'"' | b'\'')
        && memchr::memchr(r[0], &r[1..]).is_none())
        || (r.len() >= 1 && r[0] == b'('
            && memchr::memchr(b')', &r[1..]).is_none())
}

/// Check if content matches a link reference definition: `[label]: url ["title"]`.
fn refdef_matches(content: &[u8]) -> bool {
    refdef_prefix_matches(content).is_some()
}

/// LUT dispatch for new container types.
pub(crate) fn dispatch_lut(content: &[u8]) -> BlockType {
    match BLOCK_DISPATCH[content[0] as usize] {
        0 => BlockType::Paragraph,
        1 => atx_heading_matches(content).map_or(BlockType::Paragraph, |level| {
            // level is 1-indexed, discriminant = level - 1
            unsafe { std::mem::transmute((level - 1) as u8) }
        }),
        2 => {
            if thematic_break_matches(content) {
                BlockType::ThematicBreak
            } else {
                bullet_or_checklist_matches(content).unwrap_or(BlockType::Paragraph)
            }
        },
        3 => fence_open_matches(content).map_or(BlockType::Paragraph, |_| BlockType::CodeFenceOpen),
        4 => {
            let html_type = detect_html_block_type(content);
            if html_type > 0 {
                BlockType::HtmlBlock
            } else {
                if comment_start_matches(content) { BlockType::Comment } else { BlockType::Paragraph }
            }
        }
        5 => if bq_marker_matches(content) { BlockType::Blockquote } else { BlockType::Paragraph },
        6 => ordered_list_matches(content).map_or(BlockType::Paragraph, |_| BlockType::OrderedListItem),
        7 => parse_table(content),
        8 => {
            if refdef_matches(content) { BlockType::RefDef }
            else { BlockType::Paragraph }
        }
        _ => BlockType::Paragraph,
    }
}

/// Determine ScanAction for a freshly detected container.
/// `raw_col` 是 marker 首字符在 raw 轴的列坐标（顶层 = facts.indent_col；
/// resolver = raw_origin + rel——列域单源，禁止再传字节偏移）。
pub(crate) fn container_action_for(t: BlockType, content: &[u8], raw_col: usize) -> ScanAction {
    match t {
        _ if is_header_type(t) => ScanAction::Continue,
        BlockType::Bullet | BlockType::Checklist => {
            if let Some(m) = recognize_marker(content, 0).filter(|m| m.node_type == t) {
                let threshold = list_frame_threshold(raw_col, &m, first_content_is_ic(&m)) as u8;
                            ScanAction::Push(BlockFrame::new(FrameKind::BulletList, threshold, m.frame_body))
            } else {
                let m = measure(t, content).unwrap_or(ScanMeasure { prefix_bytes: 0, indent_col: 0, frame_body: 0 });
                ScanAction::Push(BlockFrame::new(FrameKind::BulletList, raw_col as u8 + m.indent_col as u8, m.frame_body))
            }
        }
        BlockType::OrderedListItem => {
            if let Some(m) = recognize_marker(content, 0).filter(|m| m.node_type == t) {
                let threshold = list_frame_threshold(raw_col, &m, first_content_is_ic(&m)) as u8;
                ScanAction::Push(BlockFrame::new(FrameKind::OrderedList, threshold, m.frame_body))
            } else {
                let m = measure(t, content).unwrap_or(ScanMeasure { prefix_bytes: 0, indent_col: 0, frame_body: 0 });
                ScanAction::Push(BlockFrame::new(FrameKind::OrderedList, raw_col as u8 + m.indent_col as u8, m.frame_body))
            }
        }
        BlockType::CodeFenceOpen => {
            let m = measure(t, content).unwrap_or(ScanMeasure { prefix_bytes: 0, indent_col: 0, frame_body: 0 });
            ScanAction::Push(BlockFrame::new(FrameKind::CodeFence, raw_col as u8, m.frame_body))
        }
        BlockType::Blockquote => {
            // BQ body = 本层内容列（raw_col + marker 宽：`>` +1 / `> ` +2；raw_col 随 push
            // 链原点，BqContinuation 链为相对值）——lazy 打断判定取「Σ 栈内全部 BQ body」
            // 与行绝对缩进求偏移（classify_bq_scene / bq_lazy_candidate 同基准）；
            // indent 保持 0（通用 fit/pop 逻辑吃 indent，动它会误弹零缩进 lazy 行）。
            let width = if content.len() > 1 && (content[1] == b' ' || content[1] == b'\t') { 2 } else { 1 };
            ScanAction::Push(BlockFrame::new(FrameKind::BlockQuote, 0, (raw_col + width) as u8))
        }
        BlockType::HtmlElement => {
            ScanAction::Push(BlockFrame::new(FrameKind::HtmlElement, 0, 0))
        }
        BlockType::Comment => {
            // Push frame for multi-line comment (<!-- without --> on same line)
            if content.starts_with(b"<!--") && !memmem::find(content, b"-->").is_some() {
                ScanAction::Push(BlockFrame::new(FrameKind::Comment, 0, BlockFrame::encode_spare_body()))
            } else {
                ScanAction::Continue
            }
        }
        BlockType::HtmlBlock => {
            // Detect block type (1-7) from content
            let block_type = detect_html_block_type(content);
            // §AD.5: if end marker is on the opening line, single-line block (like Comment)
            if html_block_ends(block_type, content) {
                ScanAction::Continue
            } else {
                ScanAction::Push(BlockFrame::new(FrameKind::HtmlBlock, 0, BlockFrame::encode_html_body(block_type)))
            }
        }
        BlockType::TableRow => {
            let (n_cols, _) = cell_ranges_bytes(content);
            let body = BlockFrame::encode_table_body(n_cols as u8);
            // 表帧 content 列 = 表头行剥后内容列（与 CodeFence 同源）；
            // admit_table/admit_table_row 用它做窗口判定，list 内 BQ 才对齐。
            ScanAction::Push(BlockFrame::new(FrameKind::Table, raw_col as u8, body))
        }
        BlockType::RefDef => {
            let after = &content[refdef_prefix_matches(content).unwrap_or(2).min(content.len())..];
            let body_trimmed = crate::block::str_view(after).trim_start();
            let has_content = !body_trimmed.is_empty();
            // #197（CM §4.7）：title 引号未闭合（跨行 title——`/url '`）——
            // body bit1 标记——classify_refdef 的守卫 4 例外（续行行是 title
            // 内容——无引号——也吸收）
            let title_unclosed = has_content && {
                let bb = body_trimmed.as_bytes();
                let mut uend = 0;
                while uend < bb.len() && bb[uend] != b' ' && bb[uend] != b'\t' { uend += 1; }
                unclosed_title(&body_trimmed[uend..])
            };
            ScanAction::Push(BlockFrame::new(FrameKind::RefDef, 0,
                (if has_content { 1 } else { 0 }) | (if title_unclosed { 2 } else { 0 })))
        }
        BlockType::ThematicBreak | BlockType::Paragraph | BlockType::Blank
        | BlockType::CodeContent | BlockType::CodeFenceClose
        | BlockType::HtmlContent
        | BlockType::IndentedCode
        | BlockType::TableSeparator | BlockType::HsSep => ScanAction::Continue,
        _ => ScanAction::Continue,
    }
}


// HTML Element Tokenizer


#[derive(Debug, Clone)]
pub(crate) enum HtmlLineToken {
    Open  { tag: String },
    Close { tag: String },
    Inline,
}

/// Void elements that never create a frame.
static VOID_ELEMENTS: phf::Set<&'static str> = phf::phf_set! {
    "area", "base", "br", "col", "embed", "hr", "img", "input",
    "link", "meta", "param", "source", "track", "wbr", "keygen",
};

fn is_void(tag: &str) -> bool {
    VOID_ELEMENTS.contains(tag)
}

/// Phase 1: scan line for HTML tokens.
pub(crate) fn tokenize_html_line(content: &[u8]) -> Vec<HtmlLineToken> {
    let mut tokens = Vec::new();
    let n = content.len();
    let mut i = 0;
    while i < n {
        let lt = memchr::memchr(b'<', &content[i..]);
        if lt.is_none() { break; }
        let lt = i + lt.unwrap();
        i = lt + 1;
        if i >= n { continue; }
        let c1 = content[i];

        // Comment
        if n - lt >= 4 && &content[lt..lt+4] == b"<!--" {
            if let Some(end) = memchr::memmem::find(&content[lt+4..], b"-->") {
                i = lt + 4 + end + 3;
            } else { i = n; }
            continue;
        }
        // PI / DOCTYPE
        if c1 == b'!' || c1 == b'?' {
            if let Some(end) = memchr::memchr(b'>', &content[i..]) {
                i = i + end + 1;
            } else { i = n; }
            continue;
        }
        // Whitespace after < → not a tag
        if c1 == b' ' || c1 == b'\t' || c1 == b'\n' || c1 == b'\r' {
            continue;
        }
        // Close tag: </tag>
        if c1 == b'/' {
            if i + 1 >= n || !content[i + 1].is_ascii_alphabetic() { i += 1; continue; }
            let tag_start = i + 1;
            let mut j = i + 1;
            while j < n && (content[j].is_ascii_alphanumeric() || content[j] == b'-') { j += 1; }
            let tag = crate::block::str_view(&content[tag_start..j]).to_lowercase();
            if tag.is_empty() { continue; }
            if let Some(gt) = memchr::memchr(b'>', &content[j..]) {
                i = j + gt + 1;
                tokens.push(HtmlLineToken::Close { tag });
            } else { i = n; }
            continue;
        }
        // Open tag
        if !c1.is_ascii_alphabetic() { continue; }
        let tag_start = i;
        let mut j = i;
        while j < n && (content[j].is_ascii_alphanumeric() || content[j] == b'-') { j += 1; }
        let tag = crate::block::str_view(&content[tag_start..j]).to_lowercase();
        if tag.is_empty() { continue; }
        let end = find_html_tag_end_quoted(&content[j..]);
        let self_closing = end.map_or(false, |p| p > 0 && content[j + p - 1] == b'/');
        if let Some(gt) = end {
            i = j + gt + 1;
            if self_closing || is_void(&tag) {
                tokens.push(HtmlLineToken::Inline);
            } else {
                tokens.push(HtmlLineToken::Open { tag });
            }
        } else { i = n; }
    }

    // Phase 2: same-line pairing
    let mut paired_opens: Vec<bool> = vec![false; tokens.len()];
    let mut paired_closes: Vec<bool> = vec![false; tokens.len()];
    let mut stack: Vec<(usize, &str)> = Vec::new();
    for (idx, tok) in tokens.iter().enumerate() {
        match tok {
            HtmlLineToken::Open { tag } => stack.push((idx, tag.as_str())),
            HtmlLineToken::Close { tag } => {
                if let Some(&(oi, _)) = stack.last() {
                    if tokens[oi].tag_eq(tag) {
                        paired_opens[oi] = true;
                        paired_closes[idx] = true;
                        stack.pop();
                    }
                }
            }
            HtmlLineToken::Inline => {}
        }
    }
    let mut result = Vec::with_capacity(tokens.len());
    for (idx, tok) in tokens.into_iter().enumerate() {
        if paired_opens[idx] || paired_closes[idx] {
            result.push(HtmlLineToken::Inline);
        } else {
            result.push(tok);
        }
    }
    result
}

impl HtmlLineToken {
    fn tag_eq(&self, other: &str) -> bool {
        match self {
            HtmlLineToken::Open { tag } | HtmlLineToken::Close { tag } => tag == other,
            HtmlLineToken::Inline => false,
        }
    }
}

/// Find '>' ending an HTML tag, skipping quoted attribute values.
fn find_html_tag_end_quoted(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'>' => return Some(i),
            b'"' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    if bytes[i] == b'\\' { i += 1; }
                    i += 1;
                }
                if i < bytes.len() { i += 1; }
            }
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    if bytes[i] == b'\\' { i += 1; }
                    i += 1;
                }
                if i < bytes.len() { i += 1; }
            }
            _ => i += 1,
        }
    }
    None
}

#[inline]
fn is_header_type(t: BlockType) -> bool {
    let d = t as u8;
    d >= HEADER_LO && d <= HEADER_HI
}


// Nesting helpers — peel / dispatch chain for Composite action


/// Only BQ, BLT, OL can contain further nested containers.
/// Other container types (CodeFence, HtmlBlock, Comment, Table) stop
/// the recursive dispatch chain.
#[inline]
pub(crate) fn is_nesting_container(kind: FrameKind) -> bool {
    crate::effect::is_nesting_container(kind)
}

/// Strip container prefix including ALL trailing whitespace.
/// Used by recursive dispatch to get the content after a container marker.
/// Example: `">   item"` → `"item"` (3 spaces gone).
pub(crate) fn after_container_prefix(content: &[u8], kind: FrameKind) -> &[u8] {
    let nt = crate::effect::container_block_type(kind);
    strip(nt, content, StripMode::All).unwrap_or(content)
}

/// 帧 kind → 开行内容类型（prev_content 维护用——BQ Composite 开行的
/// "内容类型"——供 prev 词法（closes_paragraph）判定——`> [a]: b` 的
/// 内容=RefDef（closes——lazy 拒）、`> line two` 内容=Paragraph（lazy 过））。
// 统一用 crate::effect::container_block_type（单源）。

/// Peel lower frame prefixes from content, walking stack from
/// bottom (outermost) to one-before-top.
/// Returns (peeled_content, removed_columns)——offset 从列 0 起算；
/// 需要绝对列续算的调用点用 `peel_lower_prefixes_from`。
pub(crate) fn peel_lower_prefixes<'a>(content: &'a [u8], stack: &[BlockFrame]) -> (&'a [u8], usize) {
    // 旧签名保留（9 调用方零改动）：Fail 转回旧静默 break 的等价结果
    // （部分剥取的内容 + 列宽）——新匹配点（P2）直接用 _from 取 Fail 索引。
    match peel_lower_prefixes_from(content, stack, 0) {
        LowerPrefixMatch::Ok { peeled, cols } => (peeled, cols),
        LowerPrefixMatch::Fail { .. } => {
            // 与旧 break 后返回等价：用 start_col=0 重算一次拿到部分结果。
            let (peeled, cols) = peel_lower_prefixes_partial(content, stack, 0);
            (peeled, cols)
        }
    }
}

/// `start_col` = `content` 首字节的绝对列（生产传 facts.content 时 =
/// facts.indent_col；传完整行时 = 0）。返回的 offset 是剥除前缀的列宽
/// （逐段连续 tab 展开，不从 0 重算）。
/// 下层前缀匹配结果：失配时携带失败帧的
/// **栈索引**（非 BQ 计数）——调用方据此关帧重扫；Ok = 剥后内容与列宽。
pub(crate) enum LowerPrefixMatch<'a> {
    Ok { peeled: &'a [u8], cols: usize },
    Fail { frame_index: usize },
}

pub(crate) fn peel_lower_prefixes_from<'a>(
    content: &'a [u8],
    stack: &[BlockFrame],
    start_col: usize,
) -> LowerPrefixMatch<'a> {
    let mut peeled = content;
    let mut col = start_col;
    // 前导空白同样按绝对列连续展开（SDL 比对传完整行时此段非空）。
    let after_ws = skip_leading_whitespace(peeled);
    let ws_len = (after_ws.as_ptr() as usize) - (peeled.as_ptr() as usize);
    col += column_width_from(&peeled[..ws_len], col);
    peeled = after_ws;
    // Fast path: no lower frames to peel.
    if stack.len() <= 1 {
        return LowerPrefixMatch::Ok { peeled, cols: col - start_col };
    }
    // Walk from bottom (outermost) to top-1 (skip the innermost frame).
    // Only peel BQ prefixes — list indent is already in facts.indent_col
    // (LineFacts consumed the leading whitespace).
    // #261（cmark 佐证）：BQ 前缀剥取用 strip_bq_one（每层 skip ws 后剥 `>`——
    // `>  > two` 的多余空格不再阻断 BQ2 前缀剥取——`eff_cmp` 与 after_ws
    // 不残留 `>`——否则 classify_list 误判嵌套 BQ 开行）。
    let top_idx = stack.len().saturating_sub(1);
    for fi in 0..top_idx {
        let kind = stack[fi].kind();
        if kind == FrameKind::BlockQuote {
            let before = peeled;
            peeled = strip_bq_one(peeled);
            let removed = (peeled.as_ptr() as usize) - (before.as_ptr() as usize);
            col += column_width_from(&before[..removed], col);
            // 失配（非 BQ 前缀）——返回失败帧索引（栈索引 fi）——
            // 替换旧静默 break。
            if removed == 0 {
                return LowerPrefixMatch::Fail { frame_index: fi };
            }
        }
    }
    LowerPrefixMatch::Ok { peeled, cols: col - start_col }
}

/// 旧静默 break 的部分结果（Fail 的等价形态——旧调用方语义保留）。
fn peel_lower_prefixes_partial<'a>(content: &'a [u8], stack: &[BlockFrame], start_col: usize) -> (&'a [u8], usize) {
    let mut peeled = content;
    let mut col = start_col;
    let after_ws = skip_leading_whitespace(peeled);
    let ws_len = (after_ws.as_ptr() as usize) - (peeled.as_ptr() as usize);
    col += column_width_from(&peeled[..ws_len], col);
    peeled = after_ws;
    let top_idx = stack.len().saturating_sub(1);
    for fi in 0..top_idx {
        let kind = stack[fi].kind();
        if kind == FrameKind::BlockQuote {
            let before = peeled;
            peeled = strip_bq_one(peeled);
            let removed = (peeled.as_ptr() as usize) - (before.as_ptr() as usize);
            col += column_width_from(&before[..removed], col);
            if removed == 0 { break; }
        }
    }
    (peeled, col - start_col)
}


// Block pattern matchers


#[inline(always)]
fn atx_heading_matches(content: &[u8]) -> Option<u8> {
    parse_heading(content).map(|(level, _, _)| level)
}

#[inline(always)]
fn fence_open_matches(content: &[u8]) -> Option<(u8, u8)> {
    parse_fence(content)
}

#[inline(always)]
fn bullet_or_checklist_matches(content: &[u8]) -> Option<BlockType> {
    parse_bl(content)?; // validates bullet prefix
    if is_checklist_marker(content) {
        return Some(BlockType::Checklist);
    }
    Some(BlockType::Bullet)
}

#[inline(always)]
fn comment_start_matches(content: &[u8]) -> bool {
    content.len() >= comment_mark_width() && &content[..comment_mark_width()] == b"<!--"
}

/// Comment 开标记宽度单源（LINE_SPECS::MarkWidth::Fixed("<!--")）。
#[inline]
pub(crate) fn comment_mark_width() -> usize {
    match crate::effect::line_spec(BlockType::Comment).mark_width {
        crate::effect::MarkWidth::Fixed(s) => s.len(),
        _ => 0,
    }
}

/// 层检测单源: 与 peel 的 BQ 层检测同一函数（parse_bq——严格——真空）——
/// 原宽松判定（首字符 `>`）是 dispatch_lut 与 peel 的唯一检测缺口——
/// 统一后"一个规则两个粒度"完全收口（bl/ol/fence 已同源 wrapper）。
#[inline(always)]
fn bq_marker_matches(content: &[u8]) -> bool {
    parse_bq(content).is_some()
}

#[inline(always)]
fn ordered_list_matches(content: &[u8]) -> Option<(u8, u8)> {
    parse_ol(content).map(|(delim, digits, _)| (digits as u8, delim))
}


// Table parsing


/// Trim ASCII whitespace from both ends.
#[inline(always)]
fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < bytes.len() && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }
    let mut end = bytes.len();
    while end > start && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    &bytes[start..end]
}

/// Parse a table row or separator. Line must start with `|`.
/// 统一 is_table_separator（内部剥尾 pipe——parse_table 的预处理幂等——单源——
/// SDL 映射面保留 is_table_separator——Atom::TableSeparator 直调）。
fn parse_table(content: &[u8]) -> BlockType {
    if content.is_empty() || content[0] != b'|' {
        return BlockType::Paragraph;
    }
    let inner = &content[1..];
    if inner.is_empty() || inner.iter().all(|&b| b == b' ' || b == b'\t') {
        return BlockType::Paragraph;
    }
    let effective = match inner.iter().rposition(|&b| b != b' ' && b != b'\t') {
        Some(pos) => pos,
        None => return BlockType::Paragraph,
    };
    let inner = &inner[..=effective];
    let inner = if inner.last() == Some(&b'|') { &inner[..inner.len()-1] } else { inner };

    if is_table_separator(inner) {
        BlockType::TableSeparator
    } else {
        BlockType::TableRow
    }
}


// Cell range extraction



/// Parse cell ranges from a table row line. Handles `\|` escape.
/// Returns (column_count, vec of byte ranges within the input slice).
pub fn cell_ranges_bytes(bytes: &[u8]) -> (usize, Vec<std::ops::Range<usize>>) {
    let mut ranges = Vec::new();
    let mut col_count = 0;

    let mut pipe_positions: SmallVec<[usize; 16]> = SmallVec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'|' {
            pipe_positions.push(i);
        }
        i += 1;
    }

    if pipe_positions.is_empty() {
        return (0, ranges);
    }

    for w in pipe_positions.windows(2) {
        let cell_bytes = &bytes[w[0] + 1..w[1]];
        let trimmed = trim_ascii(cell_bytes);
        let offset = trimmed.as_ptr() as usize - bytes.as_ptr() as usize;
        ranges.push(offset..offset + trimmed.len());
        col_count += 1;
    }

    // Handle trailing cell when no trailing pipe (CommonMark: optional trailing |)
    let last_pipe = *pipe_positions.last().unwrap();
    if last_pipe + 1 < bytes.len() {
        let trailing = &bytes[last_pipe + 1..];
        let trimmed = trim_ascii(trailing);
        if !trimmed.is_empty() {
            let offset = trimmed.as_ptr() as usize - bytes.as_ptr() as usize;
            ranges.push(offset..offset + trimmed.len());
            col_count += 1;
        }
    }

    (col_count, ranges)
}


// Block Vocabulary and Projection Policy


/// Block-level projection layer (mirrors public `-pj` layers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockProj { Raw, Mark, Tag, Content, Container }

impl BlockProj {
    pub fn from_proj_str(s: &str) -> Option<Self> {
        match s {
            "raw" => Some(BlockProj::Raw), "content" => Some(BlockProj::Content),
            "tag" => Some(BlockProj::Tag), "mark" => Some(BlockProj::Mark),
            "container" => Some(BlockProj::Container), _ => None,
        }
    }
}

/// Which projection(s) feed the inline parser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineFeed { None, TagOnly, ContentOnly, TagThenContent, Cells, RawText }

/// Bitmask for readable/writable projection layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockMask(pub(crate) u8);

impl BlockMask {
    pub const fn new(mask: u8) -> Self { BlockMask(mask) }
    #[inline] pub fn raw(self) -> bool { self.0 & 1 != 0 }
    #[inline] pub fn mark(self) -> bool { self.0 & 2 != 0 }
    #[inline] pub fn tag(self) -> bool { self.0 & 4 != 0 }
    #[inline] pub fn content(self) -> bool { self.0 & 8 != 0 }
    #[inline] pub fn container(self) -> bool { self.0 & 16 != 0 }
    #[inline] pub fn contains(self, s: BlockProj) -> bool {
        let bit = match s { BlockProj::Raw => 1, BlockProj::Mark => 2,
            BlockProj::Tag => 4, BlockProj::Content => 8, BlockProj::Container => 16 };
        self.0 & bit != 0
    }
}

/// Static projection geometry for a BlockType — single source of truth.
#[derive(Debug, Clone, Copy)]
pub struct BlockProjSpec {
    pub can_read: BlockMask,
    pub can_write: BlockMask,
    pub inline_mode: InlineFeed,
}

/// Returns the static projection spec for any BlockType.
pub fn block_proj_spec(t: BlockType) -> BlockProjSpec {
    use BlockType::*;
    let full    = BlockMask::new(1|2|4|8|16); // Raw+Mark+Tag+Content+Container
    let rw_tc   = BlockMask::new(4|8);         // Tag+Content writable
    let rw_c    = BlockMask::new(1|8);         // Raw+Content writable
    let no_w    = BlockMask::new(0);           // read-only
    let rmc     = BlockMask::new(1|2|8);       // Raw+Mark+Content
    let rmc_cn  = BlockMask::new(1|2|8|16);    // Raw+Mark+Content+Container
    let rc      = BlockMask::new(1|8);         // Raw+Content
    let r_only  = BlockMask::new(1);           // Raw only
    let rmtc    = BlockMask::new(1|2|4|8);     // Raw+Mark+Tag+Content (HtmlElement)

    let (read, write, inline) = match t {
        _ if is_header(t)        => (full,   rw_tc,  InlineFeed::TagOnly),
        Bullet | Checklist       => (full,   rw_tc,  InlineFeed::ContentOnly),
        OrderedListItem          => (full,   rw_tc,  InlineFeed::ContentOnly),
        Paragraph                => (full,   rw_c,   InlineFeed::ContentOnly),
        CodeFenceOpen            => (full,   rw_tc,  InlineFeed::None),
        CodeFenceClose           => (full,   no_w,   InlineFeed::None),
        CodeContent              => (rc,     no_w,   InlineFeed::None),
        Blockquote               => (rmc_cn, rw_c,   InlineFeed::ContentOnly),
        Comment                  => (rmc_cn, rw_c,   InlineFeed::None),
        TableRow | TableSeparator=> (rmc,    rw_c,   InlineFeed::Cells),
        HsSep | ThematicBreak    => (rmc,    no_w,   InlineFeed::None),
        HtmlBlock                => (rmtc,   rw_c,   InlineFeed::RawText),    // CM §4.6: raw passthrough, no inline parse
        HtmlContent              => (rc,     no_w,   InlineFeed::None),
        IndentedCode             => (rmc,    rw_c,   InlineFeed::RawText),
        Blank                   => (rc,     rw_c,   InlineFeed::None),
        HtmlElement             => (rmtc,   rw_c,   InlineFeed::None),
        RefDef                  => (rmtc,   rw_c,   InlineFeed::None),
        _ => (r_only, no_w, InlineFeed::None),
    };
    BlockProjSpec { can_read: read, can_write: write, inline_mode: inline }
}

/// Write target — matches `select_for_write()` output.
#[derive(Debug, Clone)]
pub struct BlockWriteTarget { pub range: std::ops::Range<usize>, pub node_type: BlockType }

impl BlockWriteTarget {
    pub fn from_select(range: std::ops::Range<usize>, node_type: BlockType) -> Self {
        BlockWriteTarget { range, node_type }
    }
}

/// Block node — materialized from the line tape by projection.
#[derive(Debug, Clone)]
pub struct BlockNode {
    pub start_line: usize,
    pub end_line: usize,
    pub byte_range: std::ops::Range<usize>,
    pub node_type: BlockType,
    /// True when this item is inside a loose list (LOOSE_BIT on BLT/OL frame).
    pub loose: bool,
    /// Geometry computed once at build time.  None for table nodes or
    /// pre-geometry callers.
    pub geometry: Option<LineGeo>,
    /// Byte width of outer container prefixes stripped before geometry
    /// computation (BQ `> `, etc.).  Zero at document root.
    pub peel_width: usize,
    /// 同线帧链序号：该节点是行上第几个真开帧壳（0 = 单帧/非壳/最外层）。
    /// `- - foo` 的两个 Bullet 分别是 0 和 1——父壳直接子取链后缀 [k+1..]。
    pub frame_ordinal: u8,
}

impl BlockNode {
    pub fn dummy() -> Self {
        BlockNode {
            start_line: 0,
            end_line: 0,
            byte_range: 0..0,
            node_type: BlockType::Blank,
            loose: false,
            geometry: None,
            peel_width: 0,
            frame_ordinal: 0,
        }
    }
}



// Pure Block Helper Functions


pub(crate) fn is_header(t: BlockType) -> bool {
    crate::effect::BLOCK_TYPE_DECLS[t as usize].header_level.is_some()
}

/// 章节壳判定(header_level.is_some()——缓存/树增量消费)。
/// 827 表化:全仓"是否标题 + 级别"单一来源 = BLOCK_TYPE_DECLS 的
/// header_level 列(Header1..6 = 1..6;HsHead1 = 1、HsHead2 = 2——等价
/// = 两行同值,不再有 Hs 特判)。
pub fn header_level(t: BlockType) -> Option<u8> {
    crate::effect::BLOCK_TYPE_DECLS[t as usize].header_level
}

pub(crate) fn trim_start(s: &str) -> &str {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') { i += 1; }
    &s[i..]
}

/// Type-conditional list marker stripping.
/// Bullet: strips `- ` prefix. OrderedListItem: strips `N. ` prefix.
pub(crate) fn strip_list_marker<'a>(s: &'a str, t: BlockType) -> &'a str {
    let trimmed = trim_start(s);
    if let Some(geo) = line_geometry(t, trimmed.as_bytes()) {
        &trimmed[geo.mark_end.min(trimmed.len())..]
    } else {
        trimmed
    }
}

/// Extract heading text from a line: `"## My Title"` → `"My Title"`.
/// For Setext headings (no `#` prefix), returns the entire cleaned text,
/// with trailing whitespace (including \n) trimmed.
#[inline]
pub(crate) fn extract_heading_text(s: &str) -> &str {
    let cleaned = strip_blockquote_prefix(s).trim_start();
    if !cleaned.is_empty() && cleaned.as_bytes()[0] == b'#' {
        return extract_tag(BlockType::Header1, cleaned);
    }
    cleaned.trim_end_matches(&[' ', '\t', '\n', '\r'][..])
}

/// Extract fence language: `"```rust"` → `"rust"`, `"~~~python"` → `"python"`.
#[inline]
pub(crate) fn extract_fence_lang(s: &str) -> &str {
    extract_tag(BlockType::CodeFenceOpen, s)
}

/// Extract tag name from an HTML open/close tag: `<p class='x'>` → `"p"`.
pub(crate) fn extract_html_tagname(s: &str) -> &str {
    extract_tag(BlockType::HtmlElement, s)
}

/// Extract tag name from raw bytes: `<p class='x'>` → `"p"`.
/// (测试 html_element.rs 消费)
pub(crate) fn extract_html_tagname_from_bytes(bytes: &[u8]) -> &str {
    let bytes = trim_start_bytes(bytes);
    if bytes.len() < 2 || bytes[0] != b'<' { return ""; }
    let start = if bytes[1] == b'/' { 2 } else { 1 };
    if start >= bytes.len() || !bytes[start].is_ascii_alphabetic() { return ""; }
    let mut end = start + 1;
    while end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-' || bytes[end] == b'_')
    { end += 1; }
    crate::block::str_view(&bytes[start..end])
}

fn trim_start_bytes(bytes: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') { i += 1; }
    &bytes[i..]
}

/// Extract mark from an HTML tag: `<p class='x'>` → `"<p>"`.
pub(crate) fn extract_html_mark(s: &str) -> &str {
    extract_mark(BlockType::HtmlElement, s)
}

/// Strip blockquote `"> "` or `">"` prefix from a line.
#[inline]
pub(crate) fn strip_blockquote_prefix(s: &str) -> &str {
    let trimmed = trim_start(s);
    if parse_bq(trimmed.as_bytes()).is_some() {
        let after = &trimmed[1..];
        after.strip_prefix(' ').or(after.strip_prefix('\t')).unwrap_or(after)
    } else {
        trimmed
    }
}

/// BQ 内容行分类（resolved 全量写）：剥全部 BQ 前缀保留缩进，
/// 按内容缩进分类（"as raw md reparse"的单行分类器）。
/// 与 scan（dispatch_after_bq_prefix）的差异：indent 用剥标记后的实际内容
/// 缩进（scan 用 > 后全部列数，多算容器空白列——判定阈值偏 1 的 off-by-one，
/// 此处按内容层语义修正：3 空格内容不是 IndentedCode）。空内容 → Blank。
#[inline]
/// **调整：BQ 段落续行里的「4 空格缩进 = lazy continuation」**（来源 **CM §4.3** 惰性续行 ✓）。
/// 从 `classify_bq_content` 里提出来（与 `outer_of_fresh_top` 同族、同摆放原则）。
fn bq_lazy_adjust(t: BlockType, is_para_cont: bool) -> BlockType {
    if is_para_cont && t == BlockType::IndentedCode {
        BlockType::Paragraph
    } else {
        t
    }
}

/// **调整：同线链镜像**——列表 marker 行剥**全部**容器前缀（`peel` 层序列，任意层任意类型）后判最终内容：
/// 非 `Paragraph`／`Blank` ⇒ 取该内容类型；否则保持列表行（`rt = Bullet`，item 块依赖）。
///
/// **来源是引擎规则，不是 CM** ✗（与 `effective_outer` 的 `ListPrimary` 支同规则）——
/// 后人不要误当自然规则删掉；要动它，先跑 `incr` 与 trilogy 两族判据。
fn chain_mirror_adjust(t: BlockType, s: &str) -> BlockType {
    if !matches!(t, BlockType::Bullet | BlockType::Checklist | BlockType::OrderedListItem) {
        return t;
    }
    let rem = peel(s).1;
    let inner = skip_leading_whitespace(rem.as_bytes());
    if inner.is_empty() {
        return t;
    }
    let it = dispatch_lut(inner);
    if it != BlockType::Paragraph && it != BlockType::Blank {
        it
    } else {
        t
    }
}

pub(crate) fn classify_bq_content(s: &str, is_para_cont: bool) -> BlockType {
    // 替换 ④（§36 顺序）：keep_indent 版也经 `Peeled`（等价性由探针 0/26 覆盖 keep=true 分支）。
    let keep = peel_peeled(s, 0).rest_after_bq_keep_indent();
    let indent = compute_indent(keep.as_bytes());
    let stripped = skip_leading_whitespace(keep.as_bytes());
    if stripped.is_empty() { return BlockType::Blank; }
    // 调整提成命名函数 `bq_lazy_adjust`（来源 CM §4.3 惰性续行），
    // 并在其注释里标注来源；判据照旧＝零行为变化。
    let mut t = bq_lazy_adjust(classify_in_container(stripped, indent, is_para_cont), is_para_cont);
    // 第②条调整提成 `chain_mirror_adjust`（来源＝**引擎规则**，非 CM ✗——注释已标明）。
    t = chain_mirror_adjust(t, s);
    t
}

/// item 打开行的逐层内容分类（per-(line,depth) 列计算——marker 层循环:
/// 每层剥 1 marker → 内容列（marker 列起算——tab 展开——#258/#277 阈值
/// marker_col+5）→ IC 覆盖 → dispatch_lut（无 setext——与 build_pop_push
/// 的 outer_item_type 判定语义一致）——内容仍是 marker → 下一层。
/// 返回 Vec 按 marker 层索引（`- - foo` → [Bullet, Paragraph]）；真空
/// item（剥 marker 后空）→ 该层 None。BQ 前缀（词法全部层）先剥。
pub(crate) fn content_kind_per_depth(s: &str) -> Vec<Option<BlockType>> {
    let mut out = Vec::new();
    content_kind_per_depth_into(s, &mut out);
    out
}

/// 缓冲复用版（热路径专用）：把结果推进调用方持有的 `out`——避免每次调用
/// 都新建一个 `Vec`（`build_block_records_with` 对每个 loose 行调一次；
/// nesting 实测 1.59M 次，那一趟 `blk_ck_scan` 415ms（deep 档））。
pub(crate) fn content_kind_per_depth_into(s: &str, out: &mut Vec<Option<BlockType>>) {
    // O3a（性能专项）：单折重构——原为两次 prefix_cut（BqOnly 折求 cut1 ＋
    // 余下部分再 All 折）。All 折的 layers 已含首段 BQ 与后续列表层，
    // 首段 BQ 消费量由 layers 前缀求得（≡ 原 cut1），其余按原语义 rebase
    // （base = 首段 BQ 末 ＋ 其后前导空白）。
    out.clear();
    let pc = crate::infer::prefix_cut(s, &[], &[], crate::effect::PrefixKinds::All,
        crate::effect::PrefixStop::None, crate::effect::PrefixLead::SkipWs,
        crate::effect::PrefixTail::None);
    let mut k = 0usize;
    while k < pc.layers.len() && pc.layers[k].kind == FrameKind::BlockQuote {
        k += 1;
    }
    let bq_end = if k > 0 { pc.layers[k - 1].end } else { 0 };
    let keep = &s[bq_end..];
    let cur = skip_leading_whitespace(keep.as_bytes());
    let base = bq_end + (keep.len() - cur.len());
    for l in &pc.layers[k..] {
        if !crate::effect::is_list_container(l.kind) {
            continue; // BQ 夹层穿过（边界已含于 l.start/l.end）
        }
        let ls = l.start.saturating_sub(base);
        let le = l.end.saturating_sub(base);
        let Some(m) = recognize_marker(&cur[ls..], 0) else { break };
        let content = skip_leading_whitespace(&cur[le..]);
        if content.is_empty() { out.push(None); break; }
        out.push(Some(if first_content_is_ic(&m) {
            BlockType::IndentedCode
        } else {
            dispatch_lut(content)
        }));
    }
}

/// Map BlockType to the FrameKind it opens (if any).
/// Returns None for types that never open a container frame
/// (headings, paragraphs, thematic breaks, etc.).
///
/// `_ => None` catch-all is intentional: new BlockType variants default to
/// "not a container" unless explicitly added.
pub fn frame_kind(t: BlockType) -> Option<FrameKind> {
    match t {
        BlockType::Bullet | BlockType::Checklist => Some(FrameKind::BulletList),
        BlockType::OrderedListItem              => Some(FrameKind::OrderedList),
        BlockType::CodeFenceOpen                => Some(FrameKind::CodeFence),
        BlockType::Blockquote                   => Some(FrameKind::BlockQuote),
        BlockType::HtmlBlock                    => Some(FrameKind::HtmlBlock),
        BlockType::Comment                      => Some(FrameKind::Comment),
        BlockType::TableRow                    => Some(FrameKind::TableRow),
        BlockType::TableSeparator              => Some(FrameKind::Table),
        BlockType::HtmlElement                  => Some(FrameKind::HtmlElement),
        BlockType::RefDef                      => Some(FrameKind::RefDef),
        _ => None,
    }
}

pub(crate) fn node_type_to_str(t: BlockType) -> &'static str {
    // 实现改查 BLOCK_TYPE_META 表（表值与原 match 逐值对照已验）
    crate::effect::BLOCK_TYPE_META[t as usize].debug_name
}


// Unified Projection Geometry

// Inner-world geometry primitives.


/// Line-level projection ranges.  All offsets are relative to the line's
/// `&[u8]` / `&str` start.  Pure integers, no lifetimes — Copy-capable.
/// pub（content_route_testbed 访问 BlockNode.geometry——pub 字段类型须
/// pub——字段保持 pub(crate)）
#[derive(Debug, Clone, PartialEq)]
pub struct LineGeo {
    /// End of the mark display text (exclusive).
    /// OL: d+1 (includes delimiter, excludes mandatory space).
    /// IndentedCode: 0 (mark is fabricated constant "    ").
    pub mark_text_end: usize,

    /// Structural boundary including mandatory trailing whitespace.
    /// BL=2, CKL=6, OL=d+2, BQ=1, Fence=fence_len, Heading=level.
    pub mark_end: usize,

    /// Tag text range.  No tag → 0..0.
    pub tag_range: std::ops::Range<usize>,

    /// Frame body encoding.  Non-container types → 0.
    pub frame_body: u8,

    /// Content start offset.
    /// Lists: mark_end + min(compute_indent(&line[mark_end..]), 4).
    /// BQ: mark_end.  Multi-line types: 0.
    pub content_offset: usize,
}

/// Outer-world facade: scan path only touches these two types.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScanMeasure {
    /// 内容起点字节偏移（marker 字节 + 空白字节——内容剥离用）
    pub prefix_bytes: usize,
    /// marker 列 + 空白列（CM §5.2 内容缩进——threshold/嵌套判定用；
    /// marker 列按字符数：`-`=1、`1.`=2，空白列 tab 展开）
    pub indent_col: usize,
    pub frame_body: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StripMode { All, Exact }

// ── Internal parsers (block.rs private, &[u8] input) ──

/// Parse ATX heading: count `#`, locate tag range.
/// Returns (level, tag_start, tag_end). Tag is the cleaned heading text.
fn parse_heading(line: &[u8]) -> Option<(u8, usize, usize)> {
    let mut level = 0u8;
    while level < line.len() as u8 && level < 6 && line[level as usize] == b'#' { level += 1; }
    if level == 0 { return None; }
    let after_hashes = &line[level as usize..];
    // Must be followed by space/tab or end-of-line
    if !after_hashes.is_empty() && after_hashes[0] != b' ' && after_hashes[0] != b'\t' { return None; }
    let body_start = level as usize + skip_leading_ws_bytes(&line[level as usize..]);
    let body = &line[body_start..];
    // Trim trailing whitespace
    let mut body_end = body.len();
    while body_end > 0 && (body[body_end - 1] == b' ' || body[body_end - 1] == b'\t') { body_end -= 1; }
    // Strip trailing # sequence if preceded by space
    let b = &body[..body_end];
    if b.len() >= 2 {
        let mut i = b.len();
        while i > 0 && b[i - 1] == b'#' { i -= 1; }
        if b.len() > i && i > 0 && (b[i - 1] == b' ' || b[i - 1] == b'\t') {
            body_end = i - 1; // strip the space + # run
            // Re-trim trailing whitespace after # run removal
            while body_end > 0 && (body[body_end - 1] == b' ' || body[body_end - 1] == b'\t') { body_end -= 1; }
        }
    }
    Some((level, body_start, body_start + body_end))
}

/// Parse bullet list marker. Content is already without leading ws.
/// Returns (bullet_code, mark_end, after_pos) or None.
/// bullet_code: 0='-', 1='*', 2='+'.
/// 闭 marker 规则（官方例 354）: 内容（marker 后——trim 尾）以与 marker
/// 相同的字符结尾 → 非列表（`* a *` 的星号是文字——CM §5.2 例 354 权威）——
/// 只限同字符（`- a *` 不同字符——仍列表）。
fn parse_bl(line: &[u8]) -> Option<(u8, usize, usize)> {
    if line.is_empty() { return None; }
    let c = line[0];
    let bullet_code = match c { b'-' => 0, b'*' => 1, b'+' => 2, _ => return None };
    // Reject patterns like "-text" where second char is not whitespace
    if line.len() > 1 && line[1] != b' ' && line[1] != b'\t' { return None; }
    // 闭 marker 规则不存在：原规则引 CM §5.2 例 354
    // 判 `* a *` 为文字非列表——cmark 例 354 实为 list，引证错译。
    // `- a --` / `* a *` / `- a ---` 一律为列表项（cmark 同）。
    let mark_end = if line.len() >= 2 { 2 } else { 1 };
    Some((bullet_code, mark_end, mark_end))
}

/// Parse ordered list marker. Content is already without leading ws.
/// Returns (delim, digits, mark_end) or None.
fn parse_ol(line: &[u8]) -> Option<(u8, usize, usize)> {
    let d = number_digits(line);
    if d == 0 || d >= line.len() { return None; }
    let delim = line[d];
    if delim != b'.' && delim != b')' { return None; }
    // Must be end-of-line or mandatory space/tab after
    if d + 1 < line.len() && line[d + 1] != b' ' && line[d + 1] != b'\t' { return None; }
    let mark_end = (d + 2).min(line.len());
    Some((delim, d, mark_end))
}

/// Parse code fence open/close. Content is already without leading ws.
/// Returns (fence_char, fence_len) or None.
fn parse_fence(line: &[u8]) -> Option<(u8, u8)> {
    if line.len() < 3 { return None; }
    let fc = line[0];
    if fc != b'`' && fc != b'~' { return None; }
    let mut n = 0u8;
    while n < line.len() as u8 && line[n as usize] == fc { n += 1; }
    if n < 3 { return None; }
    // Backtick fence: info string must not contain backtick
    if fc == b'`' {
        let info = &line[n as usize..];
        if info.contains(&b'`') { return None; }
    }
    Some((fc, n))
}

/// Parse blockquote marker. Content is already without leading ws.
/// Returns (has_trailing_space, mark_end=1).
fn parse_bq(line: &[u8]) -> Option<(bool, usize)> {
    if line.is_empty() || line[0] != b'>' { return None; }
    let has_trailing = line.len() > 1 && (line[1] == b' ' || line[1] == b'\t');
    Some((has_trailing, 1))
}

// ── Helpers ──

/// 相对 `mark_end` 的 capped continuation indent。
/// tab 展开必须从 mark_end 起算（`-\tfoo` 的 tab 从列 1 到 4——
/// 相对宽度 3，不能从 0 算成 4）。
fn indent_capped_at(after: &[u8], mark_end: usize) -> usize {
    compute_indent_at(after, mark_end)
        .saturating_sub(mark_end)
        .min(LIST_CONTENT_MAX)
}

/// 列（tab 展开）→ 字节偏移。content_offset 是列语义（marker 列 + tab 展开的
/// 空白），但行内容剥取按字节——tab 行（`-\t\tfoo`：content_offset=6 列 > 行
/// 字节 4）会剥超导致内容空（gauntlet 现象 3）。tab 的部分列定位到 tab 起始
/// 字节（内容从 tab 开始——与 strip 的剥取一致）。
pub(crate) fn col_to_byte(bytes: &[u8], col: usize) -> usize {
    let mut c = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if c >= col { return i; }
        c += if b == b'\t' { tab_width(c) } else { 1 };
    }
    bytes.len()
}

/// Skip leading whitespace and return byte offset.
fn skip_leading_ws_bytes(bytes: &[u8]) -> usize {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' { i += 1; }
        else if bytes[i] == 0xC2 && i + 1 < bytes.len() && bytes[i + 1] == 0xA0 { i += 2; }
        else { break; }
    }
    i
}

/// Find trim-end position: last byte index (exclusive) after stripping trailing whitespace.
fn trim_end_bytes(line: &[u8], start: usize) -> usize {
    let mut end = line.len();
    while end > start && (line[end - 1] == b' ' || line[end - 1] == b'\t') { end -= 1; }
    end
}

/// Find colon-key or bracket-key tag range in bullet content (after mark_end).
/// Colon key: text before ':' → tag.  Bracket key: "[" ... "]" → tag.
/// Returns tag_range within line.  No tag → 0..0.
/// Colon/bracket tag 检测（ColonKey 语义声明实现——快路径 line_geometry
/// 与回退（AfterTagSep——infer.rs）共用——内联语法跳过（link/code/html——
/// URL 冒号误判修复）。
/// mark_end=0 时从行首检测（回退——stripped 已剥外层 marker）。
pub(crate) fn find_colon_or_bracket_tag(line: &[u8], mark_end: usize) -> std::ops::Range<usize> {
    let after = &line[mark_end..];
    // ── Checklist 形态（selector 树化前置）──
    // checkbox 前缀（`[x] `/`[X] `/`[ ] `——与 is_checklist_marker 同款条件——
    // `[X]:` 是 RefDef label 不触发）——checkbox 后 colon 前 = key（与
    // find_ckl_tag 同构——树化段显示 Bullet——几何层统一——flat 的
    // Checklist 行走 CheckboxColonKey（find_ckl_tag）不经此——影响面
    // 局限树化段 + 渲染 attrs t）。
    if is_checklist_marker(line) {
        let after_ckb = &after[CHECKLIST_CHECKBOX_SPAN..];
        if let Some(colon_pos) = memchr::memchr(b':', after_ckb) {
            return mark_end + CHECKLIST_CHECKBOX_SPAN
                ..mark_end + CHECKLIST_CHECKBOX_SPAN + colon_pos;
        }
        return mark_end..mark_end + CHECKLIST_BRACKET_LEN; // checkbox 本身作为 tag("[x]"/"[X]"/"[ ]")
    }
    let mut pos = 0usize;
    while pos < after.len() {
        let byte = after[pos];
        // ── Inline-syntax skip: autolink / html_inline (<...>) ──
        if byte == b'<' {
            if let Some(gt) = memchr::memchr(b'>', &after[pos..]) {
                pos += gt + 1;
                continue;
            }
            break; // unmatched < — not a colon tag
        }
        // ── Inline-syntax skip: code span (`...`) ──
        if byte == b'`' {
            let run = crate::inline::count_run(after, pos, b'`');
            if run <= crate::inline::MAX_BACKTICK_RUN {
                if let Some(close) = memchr::memchr(b'`', &after[pos + run..]) {
                    // check that closing run matches
                    let close_run = crate::inline::count_run(after, pos + run + close, b'`');
                    if close_run >= run {
                        pos += run + close + close_run;
                        continue;
                    }
                }
            }
            pos += run;
            continue;
        }
        // ── Inline-syntax skip: link/image（[...](dest) / [...][label]——
        // URL 冒号误判 tag 分隔符——`- [link](http://x)` 的 http: 被当
        // tag 分隔——tag="[link](http"——link 被切开——bracket key（[x]）
        // 仍走末尾分支）──
        if byte == b'[' {
            if let Some(bracket_end) = memchr::memchr(b']', &after[pos..]) {
                let abs = pos + bracket_end;
                let rest = &after[abs + 1..];
                if rest.starts_with(b"(") {
                    // 跳 dest 到匹配 )（简单计数——scan_dest_body 同型）
                    let mut d = 1usize;
                    let mut j = 1usize;
                    while j < rest.len() {
                        match rest[j] {
                            b'(' => d += 1,
                            b')' => { d -= 1; if d == 0 { break; } }
                            _ => {}
                        }
                        j += 1;
                    }
                    pos = abs + 1 + j + 1;
                    continue;
                }
                if rest.starts_with(b"[") {
                    pos = abs + 1;
                    continue;
                }
            }
        }
        // ── Colon tag delimiter ──
        if byte == b':' {
            if pos == 0 {
                // colon at very start — treat as tag delimiter
                pos += 1; continue;
            }
            // kv key 纯字符规则: key 区域（mark_end 后到 colon）必须纯字符
            // ——字母数字/下划线/连字符/非 ASCII（中文/emoji）——含空白或
            // 其他 ASCII 标点（`.` `,` `;` 等）
            // → 退化为普通 blt（无 tag——内容全量——`- key name: v` 的
            // "key name" 是内容文本不是 key）。bracket 开头（`[a]: /u`——
            // item 内 RefDef label 形态）跳过检查（bracket key 是独立 tag
            // 形态——RefDef 的合法 label——不适用纯字符规则）
            let key_region = crate::block::str_view(&after[..pos]);
            let is_bracket_key = after.starts_with(b"[");
            let pure = is_bracket_key || key_region.chars().all(|c|
                c.is_alphanumeric() || c == '_' || c == '-' || !c.is_ascii());
            if pure {
                return mark_end..mark_end + pos;
            }
            return 0..0; // 退化——普通 blt（不继续找后续 colon）
        }
        pos += 1;
    }
    // ── Bracket key: "[...]" at start of after ──
    if after.starts_with(b"[") {
        if let Some(bracket_end) = memchr::memchr(b']', after) {
            if bracket_end > 1 {
                let rest = &after[bracket_end + 1..];
                if rest.starts_with(b" ") || rest.starts_with(b"\t") || rest.is_empty() {
                    return mark_end..mark_end + 1 + bracket_end;
                }
            }
        }
    }
    0..0
}

/// Find checklist tag range.  `mark_end` must equal `CHECKLIST_LIST_MARK_END`
/// (after `- `).
/// Prioritizes colon key over checkbox.
pub(crate) fn find_ckl_tag(line: &[u8], mark_end: usize) -> std::ops::Range<usize> {
    debug_assert_eq!(mark_end, CHECKLIST_LIST_MARK_END);
    // mark_end is CHECKLIST_LIST_MARK_END (after "- ").  If the line is a
    // checklist, the next CHECKLIST_CHECKBOX_SPAN bytes are "[x] " /
    // "[X] " / "[ ] ".
    if line.len() < mark_end + CHECKLIST_CHECKBOX_SPAN { return 0..0; }
    if line[mark_end] != b'[' { return 0..0; }
    let checked = line[mark_end + 1];
    if checked != b'x' && checked != b'X' && checked != b' ' { return 0..0; }
    if line[mark_end + 2] != b']' { return 0..0; }
    let after_ckb = &line[mark_end + CHECKLIST_CHECKBOX_SPAN..];
    // Colon key after checkbox — SIMD search
    if let Some(colon_pos) = memchr::memchr(b':', after_ckb) {
        return mark_end + CHECKLIST_CHECKBOX_SPAN
            ..mark_end + CHECKLIST_CHECKBOX_SPAN + colon_pos;
    }
    // 无键 ck(无 colon 键):tag = checkbox 状态字符 x / X / 空格(C3 定案
    // ——按投影:有键=键(CheckboxColonKey 原样);无键=状态字符,框文本
    // 不进 tag;写面 format_payload 重建 "[x] " 前缀往返)。
    mark_end + 1..mark_end + 2
}

/// Extract HTML tag name range from a line like `<p class='x'>`.
/// Returns range of the tag name within line.
fn extract_html_tagname_range(line: &[u8]) -> std::ops::Range<usize> {
    let bytes = trim_start_bytes(line);
    if bytes.len() < 2 || bytes[0] != b'<' { return 0..0; }
    let start = if bytes.len() > 1 && bytes[1] == b'/' { 2 } else { 1 };
    if start >= bytes.len() || !bytes[start].is_ascii_alphabetic() { return 0..0; }
    let mut end = start + 1;
    while end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-' || bytes[end] == b'_')
    { end += 1; }
    let offset = bytes.as_ptr() as usize - line.as_ptr() as usize;
    offset + start..offset + end
}

// ── Core primitive: line_geometry ──

// === nl trim — normalize *_from_full callers that pass line+newline ===

/// Strip trailing \n or \r\n.  *from_full callers (inline_source_from_full,
/// node_tag_from_full etc.) slice with line_offsets[i+1] as endpoint, which
/// always includes the preceding line's terminator.  Normalizing here once
/// protects every BlockType from the same issue.
#[inline]
fn trim_trailing_nl_bytes(bytes: &[u8]) -> &[u8] {
    match bytes.last() {
        Some(&b'\n') => {
            let end = bytes.len() - 1;
            if end > 0 && bytes[end - 1] == b'\r' { &bytes[..end - 1] } else { &bytes[..end] }
        }
        _ => bytes,
    }
}

/// Compute projection offsets for a single line of a given BlockType.
/// `line_bytes` is the line content WITHOUT leading whitespace
/// (the caller strips indent; for BQ-nested nodes, BQ prefix must
/// already be stripped by the caller).
/// Returns None for types with no geometric decomposition
/// (Paragraph, ThematicBreak, HsSep, Blank, CodeContent, HtmlContent,
/// IndentedCode, RefDef).
pub(crate) fn line_geometry(nt: BlockType, line_bytes: &[u8]) -> Option<LineGeo> {
    use crate::effect::{MarkWidth, TagRule};
    let line_bytes = trim_trailing_nl_bytes(line_bytes);
    let spec = crate::effect::line_spec(nt);
    if matches!(spec.mark_width, crate::effect::MarkWidth::LvHashes(_)) {
        // ATX heading
        let (level, tag_start, tag_end) = parse_heading(line_bytes)?;
        Some(LineGeo {
            mark_text_end: level as usize,
            mark_end: level as usize,
            tag_range: tag_start..tag_end,
            frame_body: 0,
            content_offset: level as usize,
        })
    } else {
        match (spec.mark_width, spec.tag_rule) {
            // ── Char1PlusOptWs: Bullet / Checklist ──
            (MarkWidth::Char1PlusOptWs, TagRule::ColonKey) => {
                let (bullet_code, mark_end, _) = parse_bl(line_bytes)?;
                let body = BlockFrame::encode_blt_body(match bullet_code { 1 => b'*', 2 => b'+', _ => b'-' });
                let tag_range = find_colon_or_bracket_tag(line_bytes, mark_end);
                Some(LineGeo { mark_text_end: 1, mark_end, tag_range, frame_body: body,
                    content_offset: col_to_byte(line_bytes, mark_end + indent_capped_at(&line_bytes[mark_end..], mark_end)) })
            }
            (MarkWidth::Char1PlusOptWs, TagRule::CheckboxColonKey) => {
                let _ = parse_bl(line_bytes)?;
                let body = BlockFrame::encode_ckl_body(line_bytes[0]);
                let ckl_mark_end = if is_checklist_marker(line_bytes) { CHECKLIST_MARK_END } else { return None; };
                let tag_range = find_ckl_tag(line_bytes, CHECKLIST_LIST_MARK_END);
                Some(LineGeo { mark_text_end: 1, mark_end: ckl_mark_end, tag_range, frame_body: body,
                    content_offset: col_to_byte(line_bytes, ckl_mark_end + indent_capped_at(&line_bytes[ckl_mark_end..], ckl_mark_end)) })
            }
            // ── DigitsDelimPlusWs: OL ──
            (MarkWidth::DigitsDelimPlusWs, _) => {
                let (delim, digits, mark_end) = parse_ol(line_bytes)?;
                let body = BlockFrame::encode_ol_body(delim);
                Some(LineGeo { mark_text_end: digits + 1, mark_end, tag_range: 0..digits, frame_body: body,
                    content_offset: col_to_byte(line_bytes, mark_end + indent_capped_at(&line_bytes[mark_end..], mark_end)) })
            }
            // ── Fence3: CodeFence ──
            (MarkWidth::Fence3, _) => {
                let (fc, n) = parse_fence(line_bytes)?;
                let body = BlockFrame::encode_cf_body(fc, n);
                let tag_start = n as usize + skip_leading_ws_bytes(&line_bytes[n as usize..]);
                let tag_end = trim_end_bytes(line_bytes, tag_start);
                Some(LineGeo { mark_text_end: n as usize, mark_end: n as usize, tag_range: tag_start..tag_end,
                    frame_body: body, content_offset: 0 })
            }
            // ── Fixed: static geometry ──
            (MarkWidth::Fixed(">"), _) => {
                let (_, _) = parse_bq(line_bytes)?;
                Some(LineGeo { mark_text_end: 1, mark_end: 1, tag_range: 0..0, frame_body: 0, content_offset: 1 })
            }
            (MarkWidth::Fixed("<!--"), _) => {
                if !line_bytes.starts_with(b"<!--") { return None; }
                Some(LineGeo { mark_text_end: comment_mark_width(), mark_end: comment_mark_width(), tag_range: 0..0, frame_body: 0, content_offset: 0 })
            }
            (MarkWidth::Fixed("|"), _) => {
                if line_bytes.is_empty() || line_bytes[0] != b'|' { return None; }
                Some(LineGeo { mark_text_end: 1, mark_end: 1, tag_range: 0..0, frame_body: 0, content_offset: 0 })
            }
            (MarkWidth::Fixed("    "), _) => None,
            // ── OpenTag: HtmlBlock / HtmlElement ──
            (MarkWidth::OpenTag, TagRule::Tagname) => {
                let gt_pos = line_bytes.iter().position(|&b| b == b'>').unwrap_or(0);
                let mt = if gt_pos > 0 { gt_pos + 1 } else { 0 };
                let tag_range = extract_html_tagname_range(line_bytes);
                // HtmlBlock: requires detect_html_block_type pre-check
                if nt == BlockType::HtmlBlock {
                    let html_type = detect_html_block_type(line_bytes);
                    if html_type == 0 { return None; }
                    Some(LineGeo { mark_text_end: mt, mark_end: mt, tag_range, frame_body: BlockFrame::encode_html_body(html_type), content_offset: 0 })
                } else {
                    Some(LineGeo { mark_text_end: mt, mark_end: mt, tag_range, frame_body: 0, content_offset: 0 })
                }
            }
            // ── Setext: 标记在下一行（下划线）——本行无前缀，tag = 整行 ──
            (MarkWidth::Setext, TagRule::HeadingText) => {
                // Setext heading: no # prefix. Tag = whole line.
                let end = trim_end_bytes(line_bytes, 0);
                Some(LineGeo { mark_text_end: 0, mark_end: 0, tag_range: 0..end, frame_body: 0, content_offset: 0 })
            }
            // ── None: no marker ──
            (MarkWidth::None, _) => None,
            // ── catch-all ──
            _ => None,
        }
    }
}

// ── Outer-world interface (scan path) ──

/// Return prefix_bytes (=content_offset) and frame_body for scan path use.
/// Does NOT expose LineGeo.
pub(crate) fn measure(nt: BlockType, content: &[u8]) -> Option<ScanMeasure> {
    match nt {
        BlockType::Checklist => {
            // Checklist 与 Bullet 几何分源——content 原点在 `[ ] ` 之后。
            if !is_checklist_marker(content) { return None; }
            let (bullet_code, _, _) = parse_bl(content)?;
            let ckl_mark_end = CHECKLIST_MARK_END; // "- " + "[ ]" + 必选空白（列/ASCII 字节）
            let prefix_bytes = col_to_byte(content, ckl_mark_end + indent_capped_at(&content[ckl_mark_end..], ckl_mark_end));
            let indent_col = compute_indent_at(&content[ckl_mark_end.saturating_sub(1)..], ckl_mark_end - 1);
            let frame_body = BlockFrame::encode_ckl_body(match bullet_code { 1 => b'*', 2 => b'+', _ => b'-' });
            Some(ScanMeasure { prefix_bytes, indent_col, frame_body })
        }
        BlockType::Bullet => {
            let (bullet_code, mark_end, _) = parse_bl(content)?;
            let prefix_bytes = col_to_byte(content, mark_end + indent_capped_at(&content[mark_end..], mark_end));
            // CM §5.2: 内容缩进 = marker 列（`-`=1） + 空白列（mark_end 已含
            // marker 后 1 个空白——从 mark_end-1 起算列，tab 展开从 marker 列
            // 起算（Bug B：compute_indent 从 0 起算会多一列——`-\t\tfoo` 内容
            // 列错位）。
            // 真空 item（`-` marker 后无空白——mark_end=1）内容缩进未定义——
            // 后续行不延续（indent_col=63 远超实际缩进）。
            let indent_col = if mark_end == 1 {
                EMPTY_ITEM_SENTINEL
            } else {
                compute_indent_at(&content[mark_end.saturating_sub(1)..], 1)
            };
            let frame_body = BlockFrame::encode_blt_body(match bullet_code { 1 => b'*', 2 => b'+', _ => b'-' });
            Some(ScanMeasure { prefix_bytes, indent_col, frame_body })
        }
        BlockType::OrderedListItem => {
            let (delim, digits, mark_end) = parse_ol(content)?;
            let prefix_bytes = col_to_byte(content, mark_end + indent_capped_at(&content[mark_end..], mark_end));
            // marker 列 = 数字位数 + delim（`1.`=2）；空白列从 mark_end-1 起算
            // （tab 从 digits+1 列起算——Bug B）。
            // 真空 item（`1.` delim 后无空白——mark_end==digits+1）不延续内容。
            let indent_col = if mark_end == digits + 1 {
                EMPTY_ITEM_SENTINEL
            } else {
                compute_indent_at(&content[mark_end.saturating_sub(1)..], digits + 1)
            };
            let frame_body = BlockFrame::encode_ol_body(delim);
            Some(ScanMeasure { prefix_bytes, indent_col, frame_body })
        }
        BlockType::CodeFenceOpen | BlockType::CodeFenceClose => {
            let (fc, n) = parse_fence(content)?;
            Some(ScanMeasure { prefix_bytes: 0, indent_col: 0, frame_body: BlockFrame::encode_cf_body(fc, n) })
        }
        BlockType::Blockquote => {
            parse_bq(content)?;
            Some(ScanMeasure { prefix_bytes: 1, indent_col: 1, frame_body: 0 })
        }
        // Rare/other types — fall back to full geometry
        _ => {
            let geo = line_geometry(nt, content)?;
            Some(ScanMeasure { prefix_bytes: geo.content_offset, indent_col: 0, frame_body: geo.frame_body })
        }
    }
}

/// Strip container prefix from content.  Content must already have
/// leading whitespace removed (as in `LineFacts.content`).
/// Returns None if the line does not start with nt's marker.
pub(crate) fn strip<'a>(nt: BlockType, content: &'a [u8], mode: StripMode) -> Option<&'a [u8]> {
    let mark_end = match nt {
        BlockType::Bullet | BlockType::Checklist => parse_bl(content)?.1,
        BlockType::OrderedListItem => parse_ol(content)?.2,
        BlockType::CodeFenceOpen | BlockType::CodeFenceClose =>
            parse_fence(content)?.1 as usize,
        BlockType::Blockquote => parse_bq(content)?.1,
        _ => {
            let geo = line_geometry(nt, content)?;
            geo.mark_end
        }
    };
    let pos = mark_end.min(content.len());
    let after = &content[pos..];
    Some(match mode {
        StripMode::All if crate::effect::block_decl(nt).is_list_item => after,
        StripMode::All => skip_leading_whitespace(after),
        StripMode::Exact => match after.first() {
            Some(b' ' | b'\t') => &after[1..],
            _ => after,
        },
    })
}

// ── Inner-world read exports (projection.rs read path) ──

/// Extract mark display text. IndentedCode → "    ". No-geometry types → "".
pub(crate) fn extract_mark(nt: BlockType, line: &str) -> &str {
    use crate::effect::MarkWidth;
    match crate::effect::line_spec(nt).mark_width {
        MarkWidth::Fixed(s) => s,
        MarkWidth::None => "",
        _ => {
            let Some(geo) = line_geometry(nt, line.as_bytes()) else { return "" };
            &line[..geo.mark_text_end.min(line.len())]
        }
    }
}

/// Extract tag text. Fence returns raw lang (unescape is caller's job).
pub(crate) fn extract_tag(nt: BlockType, line: &str) -> &str {
    if matches!(crate::effect::line_spec(nt).tag_rule, crate::effect::TagRule::None) { return ""; }
    let Some(geo) = line_geometry(nt, line.as_bytes()) else { return "" };
    if geo.tag_range.start >= geo.tag_range.end || geo.tag_range.start >= line.len() { return ""; }
    &line[geo.tag_range.start..geo.tag_range.end.min(line.len())]
}

/// node_tag 回退（无 geometry 时）的 kv/checklist tag 单源：
/// 剥 BQ/前导空白后，Bullet 走 find_colon_or_bracket_tag，
/// Checklist 走 find_ckl_tag（colon 优先、无 colon 回 checkbox）。
/// 无 tag 时 Bullet 返回 marker 字符（旧回退语义）。
pub(crate) fn extract_kv_tag(t: BlockType, line: &str) -> &str {
    // 替换 ①（§36 顺序）：原先自己走一遍 strip_bq 认 `>`；现在一律经 `Peeled` 的**过滤式余下部分**
    // ——「消费哪几种层」由此成为显式参数（此处＝只要 BQ 层、不保留缩进）；等价性已由探针 0/26 证明。
    let cleaned = peel_peeled(line, 0).rest_after_bq();
    let b = cleaned.as_bytes();
    if t == BlockType::Checklist {
        let range = find_ckl_tag(b, CHECKLIST_LIST_MARK_END);
        return if range.start < range.end && range.end <= cleaned.len() {
            &cleaned[range]
        } else {
            ""
        };
    }
    let Some((_, mark_end, _)) = parse_bl(b) else { return "" };
    if mark_end == 0 { return ""; }
    let range = find_colon_or_bracket_tag(b, mark_end);
    if range.start < range.end && range.end <= cleaned.len() {
        &cleaned[range]
    } else {
        cleaned.get(..1).unwrap_or("")
    }
}

/// Multi-line body byte range within node raw text.  Single-line → None.
/// Offsets are relative to the raw text slice start.
pub(crate) fn range_content(nt: BlockType, raw: &str) -> Option<std::ops::Range<usize>> {
    match nt {
        _ if is_header(nt) => {
            let nl = raw.find('\n')?;
            if nl + 1 < raw.len() { Some(nl + 1..raw.len()) } else { None }
        }
        BlockType::CodeFenceOpen | BlockType::CodeFenceClose => {
            let first = raw.find('\n')? + 1;
            // 最后一行是闭围栏（```/~~~）→ 内容到其前；否则（未闭合 CF 内容
            // 到文档尾）→ 到末尾。有尾 \n 的未闭合 CF 不能按 rfind 截断
            // （最后一个 \n 是内容行的）。
            let trimmed = raw.trim_end_matches(['\n', '\r']);
            // closer 候选 = trim 后末行，且必须不是开行自身（closer≠开行 不变式入
            // 结构——开行也以 ``` 开头，未闭合且内容全为尾换行时 trim 会把开行
            // 顶成末行：情形④）。last = 闭行行首偏移，否则 raw.len()
            // （未闭合内容到文档尾——尾 \n 是内容行）。
            let last_line_start = trimmed.rfind('\n').map(|p| p + 1).unwrap_or(0);
            let last_line = &trimmed[last_line_start..];
            let is_closing = last_line_start > 0
                && (last_line.trim_start().starts_with("```")
                    || last_line.trim_start().starts_with("~~~"));
            let last = if is_closing { last_line_start } else { raw.len() };
            if first < last { Some(first..last) } else { None }
        }
        BlockType::Comment => {
            let start = raw.find("<!--")? + comment_mark_width();
            let end = raw.rfind("-->")?;
            if start < end { Some(start..end) } else { None }
        }
        BlockType::HtmlElement => {
            let start = raw.find('>')? + 1;
            let end = raw.rfind("</").unwrap_or(raw.len());
            if start < end { Some(start..end) } else { None }
        }
        BlockType::HtmlBlock => {
            let first = raw.find('\n')? + 1;
            // 最后一行是闭标签（</）→ 内容到其前；未闭合（内容到文档尾）→ 末尾。
            let trimmed = raw.trim_end_matches(['\n', '\r']);
            let last_line = trimmed.rsplit('\n').next().unwrap_or("");
            let last = if last_line.contains("</") {
                // 同上：按字符截，避免多字节末字符落在切点上
                let cut_at = raw.char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
                let without_close = &raw[..cut_at];
                without_close.rfind('\n')?
            } else {
                raw.len()
            };
            if first < last { Some(first..last) } else { None }
        }
        _ => None,
    }
}

// ── Inner-world write export ──

/// Format payload for writing back through projection geometry.
/// `original_line` is the complete line text (including leading ws + marker).
pub(crate) fn format_payload(
    nt: BlockType,
    proj: &str,
    payload: &str,
    original_line: &str,
    consume_opt: Option<usize>,
    chain: &[BlockFrame],
    line_entries: &[u64],
) -> String {
    let spec = crate::effect::line_spec(nt);
    // 语义化（payload_spec 策略查表——策略选择单源——分支实现保留生产侧——
    // 语义定义见 effect.rs）
    match (crate::effect::payload_spec(nt, proj), proj) {
        (crate::effect::PayloadSpec::BqPrefix, _) => {
            payload.lines()
                .map(|l| if l.is_empty() { ">".into() } else { format!("> {}", l) })
                .collect::<Vec<_>>()
                .join("\n")
        }
        (crate::effect::PayloadSpec::Reconstruct, _) => {
            let line_trimmed = original_line.trim_end();
            // 8.3（#10）：container_prefix 改 prefix_cut 直取——原 peel_peeled
            // rest_after_bq + ws_len 两算式由折叠单源（零重解析）。
            // 对齐补漏（tail 入表）：参数由 prefix_policy R7（Container×Write
            // =(BqOnly,None,SkipWs,FinalSkip)）供给——站点消字面量。
            let p = crate::effect::prefix_policy(
                nt,
                crate::effect::PrefixFace::Container,
                crate::effect::PrefixDir::Write,
            );
            let c = crate::infer::prefix_cut(
                line_trimmed,
                chain,
                line_entries,
                p.kinds,
                p.stop,
                p.lead,
                p.tail,
            );
            let bq_prefix_len = c.layers.last().map_or(0, |l| l.end);
            let ws_len = c.bytes - bq_prefix_len;
            let container_prefix = &line_trimmed[..c.bytes];
            let line_ws = &line_trimmed[c.bytes..];
            let b = line_ws.as_bytes();
            let tag_val = payload.trim_end_matches('\n');
            match spec.tag_rule {
                // OL: digits replacement
                crate::effect::TagRule::Digits => {
                    let marker_char = parse_ol(b).map(|(delim, _, _)| delim as char).unwrap_or('.');
                    // 真空保护: 真空 ol（"1."——行尾）的 rest 越界——marker_len
                    // 已是 marker + 必选空白（或行尾）的几何单源：
                    // 首选 consume−ws 单源；kind 不匹配回退 parse_ol 旧径）。
                    let rest = match consume_opt {
                        Some(c) => {
                            let marker_len = c.saturating_sub(ws_len);
                            if marker_len < line_ws.len() { &line_ws[marker_len..] } else { "" }
                        }
                        None => parse_ol(b)
                            .map(|(_, _, mark_end)| {
                                if (mark_end as usize) < line_ws.len() {
                                    &line_ws[mark_end as usize..]
                                } else { "" }
                            })
                            .unwrap_or(""),
                    };
                    format!("{}{}{} {}", container_prefix, tag_val, marker_char, rest)
                }
                // Checklist: marker + checkbox + colon（C3 写面往返：
                // 无键 ck tag = 状态字符 x/X/空格——重建 "[c] " 前缀；有键 ck
                // tag = 键——checkbox 状态保留原行（状态无 tag 表达）；
                // 直接给 "[x]" 框值 / 任意键文本均兼容）。
                crate::effect::TagRule::CheckboxColonKey => {
                    // 统一 marker 检测（真空 marker 宽 1/有空格 2）：
                    // 首选 consume−ws 单源；kind 不匹配回退 parse_bl 旧径）
                    let marker_len = match consume_opt {
                        // c<=ws_len 时 consume 全被容器前缀吃掉——marker 未剥；
                        // 回退 parse_bl（非首条 PopPush 行会走到这里）。
                        Some(c) if c > ws_len => c.saturating_sub(ws_len).min(line_ws.len()),
                        _ => if let Some((_, mark_end, _)) = parse_bl(b) {
                            mark_end as usize } else { 0 },
                    };
                    let after_marker = &line_ws[marker_len..];
                    let is_ck = is_checklist_marker(b);
                    let rest = if is_ck { &line_ws[CHECKLIST_MARK_END..] } else { after_marker };
                    // 原行 checkbox 状态字符（有键 ck 的状态无 tag 表达——写键保留原状态）
                    let ck_state = if is_ck && line_ws.len() > CHECKLIST_LIST_MARK_END + 1 {
                        line_ws.as_bytes()[CHECKLIST_LIST_MARK_END + 1] as char
                    } else { ' ' };
                    // rest 键结构判定（"<key>: "——checkbox 后 colon 键——
                    // 与读面 find_ckl_tag colon 分支同构）
                    let key_rest = if is_ck && !rest.is_empty() {
                        let after = rest;
                        let word_end = after
                            .bytes()
                            .position(|c| c == b':' || c == b' ' || c == b'\t')
                            .unwrap_or(after.len());
                        if word_end > 0 && after.as_bytes().get(word_end) == Some(&b':') {
                            let skip = word_end
                                + colon_sep_len(
                                    b':',
                                    after.as_bytes().get(word_end + 1).copied().unwrap_or(0),
                                );
                            Some((&after[..word_end], &after[skip.min(after.len())..]))
                        } else { None }
                    } else { None };
                    if tag_val.is_empty() {
                        format!("{}{}{}", container_prefix, &line_ws[..marker_len], rest)
                    } else if tag_val.starts_with('[') {
                        format!("{}{}{} {}", container_prefix, &line_ws[..marker_len], tag_val, rest)
                    } else if let Some((_key, krest)) = key_rest {
                        // 有键 ck：状态保留原行，键替换
                        format!("{}{}[{}] {}: {}", container_prefix, &line_ws[..marker_len], ck_state, tag_val, krest)
                    } else if is_ck
                        && tag_val.len() == 1
                        && matches!(tag_val.as_bytes()[0], b'x' | b'X' | b' ')
                    {
                        // 无键 ck：tag = 状态字符——重建 checkbox 框
                        format!("{}{}[{}] {}", container_prefix, &line_ws[..marker_len], tag_val, rest)
                    } else if is_ck {
                        // 无键 ck 写非状态文本 = 显式转 kv(checkbox 消除——
                        // 旧行为保持:checked → key-value 单向转换)
                        format!("{}{}{}: {}", container_prefix, &line_ws[..marker_len], tag_val, rest)
                    } else {
                        // 非 ck 原行（防御——CheckboxColonKey 语义仅 ck 行）
                        format!("{}{}{}: {}", container_prefix, &line_ws[..marker_len], tag_val, rest)
                    }
                }
                // Bullet / other: colon or bracket key
                _ => {
                    // 统一 marker 检测（真空 marker 宽 1/有空格 2——line_ws 已去
                    // 容器前缀和列表缩进——"- " → "-"；首选
                    // consume−ws 单源；kind 不匹配回退 parse_bl 旧径）
                    let marker_len = match consume_opt {
                        // c<=ws_len 时 consume 全被容器前缀吃掉——marker 未剥；
                        // 回退 parse_bl（非首条 PopPush 行会走到这里）。
                        Some(c) if c > ws_len => c.saturating_sub(ws_len).min(line_ws.len()),
                        _ => if let Some((_, mark_end, _)) = parse_bl(b) {
                            mark_end as usize } else { 0 },
                    };
                    let after_marker = if marker_len > 0 { &line_ws[marker_len..] } else { line_ws };
                    // has_colon/contains("] ") 猜 → find_colon_or_bracket_tag
                    // ——与读面（content_stripped）同构——内联语法跳过——
                    // 正逆同（写面 rest 判定 = 读面 tag 判定）
                    let tag_range = crate::block::find_colon_or_bracket_tag(after_marker.as_bytes(), 0);
                    let (rest, had_colon) = if tag_range.start < tag_range.end {
                        let after_tag = &after_marker[tag_range.end..];
                        if after_tag.starts_with(':') {
                            let skip = colon_sep_len(b':', after_tag.as_bytes().get(1).copied().unwrap_or(0));
                            (after_tag[skip.min(after_tag.len())..].trim().to_string(), true)
                        } else {
                            let skip = 1;
                            (after_tag[skip.min(after_tag.len())..].to_string(), false)
                        }
                    } else { (after_marker.to_string(), false) };
                    let marker_char = if marker_len > 0 { b[0] as char } else { '-' };
                    if tag_val.is_empty() {
                        format!("{}{} {}", container_prefix, marker_char, rest)
                    } else if tag_val.starts_with('[') {
                        format!("{}{} {} {}", container_prefix, marker_char, tag_val, rest)
                    } else if rest.is_empty() && !had_colon {
                        // 真空 item（无 rest 且原行无 colon）——marker + tag——无 colon
                        // 无尾空格（"- new"）
                        format!("{}{} {}", container_prefix, marker_char, tag_val)
                    } else if rest.is_empty() {
                        // 原行有 colon 但 rest 为空：保留 `:`（"- key:"）
                        format!("{}{} {}:", container_prefix, marker_char, tag_val)
                    } else {
                        format!("{}{} {}: {}", container_prefix, marker_char, tag_val, rest)
                    }
                }
            }
        }
        (crate::effect::PayloadSpec::Raw, "content") => {
            let trimmed = payload.trim_end_matches('\n');
            // #88（原"进来加 4"设定（续行固定 4 空格）与读端 dedent
            // （content_offset——顶层=2/几何无=4）不对称——统一减 4 破坏手写
            // CM 语义——取消设定: payload 原样（0 缩进）——纯文本续行由
            // LazyParagraph 并入（CM §5.2——classify_list）；含 marker 续行按
            // markdown 打断（内容里的结构按结构解析——与 CM 一致）。
            // 去尾换行保留（content 投影语义——与读端对称——CM §4.8）。
            trimmed.to_string()
        }
        _ => payload.to_string(),
    }
}


// Golden tests — line_geometry for every BlockType
// (moved to tests/unit/block_geometry.rs)

#[cfg(test)]
mod geometry_tests {
    include!("../tests/unit/block_geometry.rs");
}



// Tests


#[cfg(test)]
mod tests {
    include!("../tests/unit/block.rs");
    include!("../tests/unit/effect_decl.rs");
}
