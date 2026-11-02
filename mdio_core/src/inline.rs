//! MDIO Inline Markdown Parser — projection-first, zero-copy slices.
//!
//! Input:  &str (block 层去壳后的纯内容)
//! Output: Vec<InlineNode> (12 bytes × N)
//!
//! Architecture: unified stack (Emph + Bracket) → Pass 1 scan → Pass 2 pair → parent chain → InlineFrame projection.
//! Independent of block projection. No write, no Rope access, no selector.

use std::borrow::Cow;
use memchr;

// ── Back-pressure limits ──

/// Maximum consecutive backtick characters in a code span fence.
/// Beyond this, the run is treated as literal text — prevents O(n²)
/// memchr scanning on pathological input.
pub const MAX_BACKTICK_RUN: usize = 32;

/// Maximum nesting depth of parentheses inside an inline link
/// destination, e.g. `[text](url(with(nested(parens))))`.
pub const MAX_LINK_PAREN_DEPTH: usize = 32;



// 1. GeoKind, InlineFrame (u16), InlineType


/// 7 种投影几何。bit 15-12 of InlineFrame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GeoKind {
    Plain     = 0, // Text, SoftBreak, LineBreak
    PairDelim = 1, // Emph, Strong, Strikethrough
    Fenced    = 2, // CodeSpan, Math, DisplayMath
    Bracket   = 3, // Link, Image
    AngleWrap = 4, // Autolink, HtmlInline
    Escape    = 5, // Escaped
    Entity    = 6, // Entity
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineFrame(u16);

impl InlineFrame {
    const GEO_SHIFT: u16 = 12;
    const GEO_MASK:  u16 = 0xF000;
    const BODY_MASK: u16 = 0x0FFF;

    #[inline] pub fn geo_kind(self) -> GeoKind {
        match (self.0 & Self::GEO_MASK) >> Self::GEO_SHIFT {
            0 => GeoKind::Plain, 1 => GeoKind::PairDelim, 2 => GeoKind::Fenced,
            3 => GeoKind::Bracket, 4 => GeoKind::AngleWrap, 5 => GeoKind::Escape,
            6 => GeoKind::Entity, _ => GeoKind::Plain, // unreachable
        }
    }
    #[inline] fn from_geo(geo: GeoKind) -> Self { InlineFrame(((geo as u16) << Self::GEO_SHIFT) & Self::GEO_MASK) }

    // ── Plain (geo=0) body: [1:0] sub_type ──
    pub fn plain(sub_type: u8) -> Self { InlineFrame(((sub_type as u16) & 0x3) & Self::BODY_MASK) }
    #[inline] pub fn plain_sub(self) -> u8 { (self.0 & 0x3) as u8 }

    // ── PairDelim (geo=1) body: [8:6] opener_use, [5:3] closer_use, [2:0] delim_byte ──
    pub fn pair_delim(delim_byte: u8, use_count: u8) -> Self {
        let body = ((use_count as u16 & 0x7) << 6) | ((use_count as u16 & 0x7) << 3) | (delim_byte as u16 & 0x7);
        InlineFrame(Self::from_geo(GeoKind::PairDelim).0 | body)
    }
    #[inline] pub fn opener_use(self) -> u8 { ((self.0 >> 6) & 0x7) as u8 }
    #[inline] pub fn closer_use(self) -> u8 { ((self.0 >> 3) & 0x7) as u8 }
    #[inline] pub fn delim_byte(self) -> u8  { (self.0 & 0x7) as u8 }

    // ── Fenced (geo=2) body: [8:7] sub_type, [6:0] fence_len ──
    pub fn fenced(sub_type: u8, fence_len: u8) -> Self {
        let body = ((sub_type as u16 & 0x3) << 7) | (fence_len as u16 & 0x7F);
        InlineFrame(Self::from_geo(GeoKind::Fenced).0 | body)
    }
    #[inline] pub fn fenced_sub(self) -> u8 { ((self.0 >> 7) & 0x3) as u8 }
    #[inline] pub fn fence_len(self)  -> u8 { (self.0 & 0x7F) as u8 }

    // ── Bracket (geo=3) body: [8] is_image, [7] is_ref ──
    pub fn bracket(is_image: bool, is_ref: bool) -> Self {
        let mut f = Self::from_geo(GeoKind::Bracket);
        if is_image { f.0 |= 0x0100; } if is_ref { f.0 |= 0x80; }
        f
    }
    #[inline] pub fn is_image(self) -> bool { self.0 & 0x0100 != 0 }
    #[inline] pub fn is_ref(self)   -> bool { self.0 & 0x0080 != 0 }

    // ── AngleWrap (geo=4) body: [0] sub_type ──
    pub fn angle_wrap(sub_type: u8) -> Self {
        InlineFrame(Self::from_geo(GeoKind::AngleWrap).0 | (sub_type as u16 & 0x1))
    }
    #[inline] pub fn angle_sub(self) -> u8 { (self.0 & 0x1) as u8 }

    // ── Escape (geo=5), Entity (geo=6) body unused (entity pre-decoded in normalize pass) ──
    pub fn escape() -> Self { Self::from_geo(GeoKind::Escape) }
    pub fn entity() -> Self { Self::from_geo(GeoKind::Entity) }

    // ── InlineType derivation ──
    pub fn inline_kind(self) -> InlineType {
        match self.geo_kind() {
            GeoKind::Plain => match self.plain_sub() { 0 => InlineType::Text, 1 => InlineType::SoftBreak, 2 => InlineType::LineBreak, _ => InlineType::Text },
            GeoKind::PairDelim => match self.delim_byte() {
                0|1 => match self.closer_use() { 1 => InlineType::Emph, 2 => InlineType::Strong, _ => InlineType::Text },
                2   => InlineType::Strikethrough, _ => InlineType::Text,
            },
            GeoKind::Fenced => match self.fenced_sub() { 0 => InlineType::CodeSpan, 1 => InlineType::Math, 2 => InlineType::DisplayMath, _ => InlineType::Text },
            GeoKind::Bracket => if self.is_image() { InlineType::Image } else { InlineType::Link },
            GeoKind::AngleWrap => match self.angle_sub() { 0 => InlineType::Autolink, _ => InlineType::HtmlInline },
            GeoKind::Escape => InlineType::Escaped,
            GeoKind::Entity => InlineType::Entity,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineType { Text, SoftBreak, LineBreak, Emph, Strong, Strikethrough, CodeSpan, Math, DisplayMath, Link, Image, Autolink, HtmlInline, Escaped, Entity }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineField { Raw, Content, Mark, Tag, Title }


// 2. EmphSlot (u16)


#[derive(Debug, Clone, Copy)]
struct EmphState {
    pub can_open: bool,
    pub can_close: bool,
    pub tombstone: bool,
    pub num_chars: u8,
    pub delim_code: u8,
}


// 3. InlineNode (12 bytes)


pub const PARENT_NONE: u16 = 0xFFFF;

#[derive(Debug, Clone)]
#[repr(C)]
pub struct InlineNode {
    pub byte_start: u32,
    pub byte_end:   u32,
    pub frame:      InlineFrame,
    pub parent:     u16,
}


// 4. StackEntry — 命名字段


#[derive(Debug, Clone)]
enum StackEntry {
    Emph    { state: EmphState, text_idx: u16 },
    Bracket { text_idx: u16, byte_start: u32, image: bool, bracket_after: bool },
}


// 5. InlineScratch — reusable inline parse buffers


/// Reusable buffers for inline scanning.  Call `scan_inline_into()` to
/// populate, then read `scratch.inlines`.  Old contents are cleared on
/// each scan.
#[derive(Debug, Clone)]
pub struct InlineScratch {
    pub inlines: Vec<InlineNode>,
    pub stack:   Vec<StackEntry>,
    /// Precomputed child adjacency. children[i] = sorted child indices of node i.
    /// Built during parent assignment in handle_close_bracket_fn and process_emphasis_fn.
    pub children: Vec<smallvec::SmallVec<[u16; 2]>>,
}

impl InlineScratch {
    pub fn new() -> Self {
        InlineScratch { inlines: Vec::new(), stack: Vec::new(), children: Vec::new() }
    }
    pub fn clear(&mut self) { self.inlines.clear(); self.stack.clear(); self.children.clear(); }
}

// ── standalone process_emphasis ──

#[inline]
fn tombstone_stack_entry(stack: &mut Vec<StackEntry>, pos: usize) {
    if let StackEntry::Emph { state: ref mut s, .. } = stack[pos] { s.tombstone = true; }
}

#[inline]
fn set_stack_num_chars(stack: &mut Vec<StackEntry>, pos: usize, n: u8) {
    if let StackEntry::Emph { state: ref mut s, .. } = stack[pos] { s.num_chars = n; }
}

fn process_emphasis_fn(inlines: &mut Vec<InlineNode>, stack: &mut Vec<StackEntry>, stack_bottom: usize, children: &mut Vec<smallvec::SmallVec<[u16; 2]>>) {
    // 防御（u16 边界：单次 scan 节点
    // 数接近 0xF000 时 opener 状态错乱（ot/ct 反复指向已消费节点——
    // byte 坐标溢出——debug panic（release 掩盖））——接近上限跳过匹配
    // （closer 留文本——输出不完整但合法——不崩——snap_down_char_boundary
    // 哲学）——真实文档单块永不触达（单块 > 150KB inline 密集段）——
    // 根因（opener 重复匹配）后置追查
    if inlines.len() >= 0xF000 {
        return;
    }
    if !stack[stack_bottom..].iter().any(|e| matches!(e, StackEntry::Emph { .. })) {
        stack.truncate(stack_bottom);
        return;
    }
    let mut openers_bottom = [stack_bottom as u16; 14];
    let mut i = stack_bottom;
    while i < stack.len() {
        let cs: EmphState; let ct: u16;
        match &stack[i] { StackEntry::Bracket { .. } => { i += 1; continue; } StackEntry::Emph { state: s, text_idx: t } => { cs = *s; ct = *t; } }
        if cs.tombstone || !cs.can_close { i += 1; continue; }
        let delim = cs.delim_code;
        let ix = openers_bottom_index(delim, cs.num_chars, cs.can_open);
        let mut op: Option<usize> = None; let mut mod3 = false; let mut j = i as isize - 1;
        while j >= openers_bottom[ix] as isize {
            match &stack[j as usize] {
                StackEntry::Bracket { .. } => { j -= 1; continue; }
                StackEntry::Emph { state: od, .. } => {
                    if !od.tombstone && od.can_open && od.delim_code == delim {
                        let odd = (od.can_close || cs.can_open)
                            && ((od.num_chars + cs.num_chars) % 3 == 0)
                            && !(od.num_chars % 3 == 0 && cs.num_chars % 3 == 0);
                        if !odd { op = Some(j as usize); break; } mod3 = true;
                    }
                }
            }
            j -= 1;
        }
        // 匹配成功时不推进 openers_bottom：嵌套场景推进会破坏语义
        // （asterisk_underscore_mixed 测试红）；推进只发生在未找到 opener
        // 的 else 分支与 u16 防御分支；复杂度由 sdl_inline_scale 断言 R<20 监控。
        if let Some(op_pos) = op {
            // u16 索引防御（根因：text_idx/wi 的 `as u16` 在
            // len 超 65536 时截断回绕——索引错乱——byte 坐标加减溢出——
            // debug panic（release 掩盖）——单次 handle_close_bracket 可
            // 匹配数千次（文档结尾收敛——len 61439→65534）——主循环 guard
            // 余量不够——w 创建处跳过（len 永不超 65500——text_idx 不截断）
            if inlines.len() >= 0xFF00 {
                if !mod3 && !cs.can_open { openers_bottom[ix] = i as u16 + 1; }
                if !cs.can_open { tombstone_stack_entry(stack, i); }
                i += 1;
                continue;
            }
            let (od, ot) = match &stack[op_pos] { StackEntry::Emph { state: s, text_idx: t } => (*s, *t), _ => unreachable!() };
            let total = od.num_chars.min(cs.num_chars);
            let oud = if total % 2 == 1 { 1 } else { 2 };
            inlines[ot as usize].byte_start += oud as u32;
            inlines[ct as usize].byte_end   -= oud as u32;
            if od.num_chars == oud { tombstone_stack_entry(stack, op_pos); } else { set_stack_num_chars(stack, op_pos, od.num_chars - oud); }
            if cs.num_chars == oud { tombstone_stack_entry(stack, i); i += 1; } else { set_stack_num_chars(stack, i, cs.num_chars - oud); }
            for k in (op_pos + 1)..i { tombstone_stack_entry(stack, k); }
            let wi = inlines.len() as u16;
            children.resize((wi + 1) as usize, smallvec::SmallVec::new());
            let w = InlineNode {
                byte_start: inlines[ot as usize].byte_start - oud as u32,
                byte_end:   inlines[ct as usize].byte_end   + oud as u32,
                frame: InlineFrame::pair_delim(delim, oud), parent: PARENT_NONE,
            };
            // 二分/游标尝试会破坏语义，故用原始全扫——复杂度由
            // sdl_inline_scale R<20 监控。
            for idx in 0..(wi as usize) {
                if inlines[idx].parent == PARENT_NONE
                    && inlines[idx].byte_start >= w.byte_start
                    && inlines[idx].byte_end   <= w.byte_end
                {
                    inlines[idx].parent = wi;
                    children[wi as usize].push(idx as u16);
                }
            }
            let w_start = w.byte_start; let w_end = w.byte_end;
            inlines.push(w);
            // 外层容器找最小宽度（原始全扫——语义基线）
            let mut best_parent: Option<(usize, u32)> = None;
            for idx2 in 0..(wi as usize) {
                let outer = &inlines[idx2];
                if matches!(outer.frame.geo_kind(), GeoKind::PairDelim | GeoKind::Bracket)
                    && outer.byte_start <= w_start && outer.byte_end >= w_end
                    && outer.parent == PARENT_NONE
                {
                    let width = outer.byte_end - outer.byte_start;
                    if best_parent.map_or(true, |(_, w)| width < w) {
                        best_parent = Some((idx2, width));
                    }
                }
            }
            if let Some((best_idx, _)) = best_parent {
                inlines[wi as usize].parent = best_idx as u16;
                children[best_idx].push(wi);
                for idx3 in (ot as usize+1)..(ct as usize).min(inlines.len()) {
                    if inlines[idx3].parent == best_idx as u16
                        && inlines[idx3].byte_start >= w_start
                        && inlines[idx3].byte_end   <= w_end
                    {
                        inlines[idx3].parent = wi;
                        children[best_idx].retain(|c| *c != idx3 as u16);
                        children[wi as usize].push(idx3 as u16);
                    }
                }
            }
        } else {
            if !mod3 && !cs.can_open { openers_bottom[ix] = i as u16 + 1; }
            if !cs.can_open { tombstone_stack_entry(stack, i); }
            i += 1;
        }
    }
    stack.truncate(stack_bottom);
}

// ── handle_close_bracket + helpers ──

fn emit_ref_link_fn(inlines: &mut Vec<InlineNode>, text_idx: u16, text_byte_start: u32,
                    image: bool, label: &str, end_pos: usize, refmap: &RefMap, base_offset: usize) -> bool {
    if refmap.get(label).is_none() { return false; }
    inlines[text_idx as usize].byte_start = 0;
    inlines[text_idx as usize].byte_end = 0;
    let li = inlines.len() as u16;
    for idx in text_idx as usize..inlines.len() {
        if inlines[idx].parent == PARENT_NONE { inlines[idx].parent = li; }
    }
    inlines.push(make_vi(text_byte_start as usize, end_pos,
        InlineFrame::bracket(image, true), base_offset));
    true
}

/// Scan forward from `start` (byte after `(` in `](dest)`) to find the matching `)`.
/// Handles angle brackets, backslash escapes, and nested parens.  Returns position
/// AFTER the closing `)` on success, or None if no match found.
fn scan_dest_body(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut depth = 0i32;
    let mut in_angle = false;
    let mut skip_next = false;
    let mut i = 0;
    while start + i < bytes.len() {
        let b = bytes[start + i];
        if skip_next { skip_next = false; i += 1; continue; }
        if b == b'\\' { skip_next = true; i += 1; continue; }
        if in_angle {
            if b == b'>' { in_angle = false; }
            if b == b'\n' || b == b'\r' { return None; }
            i += 1; continue;
        }
        if b == b'<' { in_angle = true; i += 1; continue; }
        if b == b'(' { depth += 1; if depth > MAX_LINK_PAREN_DEPTH as i32 { return None; } }
        else if b == b')' {
            if depth == 0 { return Some(start + i + 1); }
            depth -= 1;
        }
        i += 1;
    }
    None
}

fn resolve_ref_link_fn(inlines: &mut Vec<InlineNode>, input: &str, pos: &mut usize,
                       text_idx: u16, text_byte_start: u32, close_pos: usize, image: bool,
                       refmap: &RefMap, base_offset: usize) -> bool {
    if refmap.is_empty() { return false; }
    let ls = *pos + 1;
    let bytes = input.as_bytes();
    let rest = &bytes[ls..];
    let le = match memchr::memchr(b']', rest) {
        Some(offset) => ls + offset,
        None => { return false; }
    };
    let raw_label = if le > ls {
        &input[ls..le]
    } else {
        let offset = if image { 2 } else { 1 };
        &input[text_byte_start as usize + offset..close_pos]
    };
    let label = normalize_label(raw_label);
    *pos = le + 1;
    emit_ref_link_fn(inlines, text_idx, text_byte_start, image, &label, *pos, refmap, base_offset)
}

fn resolve_shortcut_link_fn(inlines: &mut Vec<InlineNode>, input: &str, pos: &mut usize,
                            text_idx: u16, text_byte_start: u32, close_pos: usize, image: bool,
                            refmap: &RefMap, base_offset: usize) -> bool {
    if refmap.is_empty() { return false; }
    let offset = if image { 2 } else { 1 };
    let label = &input[text_byte_start as usize + offset..close_pos];
    emit_ref_link_fn(inlines, text_idx, text_byte_start, image, &normalize_label(label), *pos, refmap, base_offset)
}

fn handle_close_bracket_fn(stack: &mut Vec<StackEntry>, inlines: &mut Vec<InlineNode>,
                           children: &mut Vec<smallvec::SmallVec<[u16; 2]>>,
                           input: &str, pos: usize, refmap: &RefMap, base_offset: usize,
                           no_link_openers: &mut bool) -> usize {
    // pos is the position of `]`
    let bp = stack.iter().rposition(|e| matches!(e, StackEntry::Bracket { .. }));
    let Some(bp) = bp else {
        inlines.push(make_vi(pos, pos + 1, InlineFrame::plain(0), base_offset));
        return pos + 1;
    };
    let (text_idx, byte_start, image, bracket_after) = match stack.remove(bp) {
        StackEntry::Bracket { text_idx, byte_start, image, bracket_after } =>
            (text_idx, byte_start, image, bracket_after),
        _ => unreachable!(),
    };
    if *no_link_openers && !image {
        // 外层 bracket 被拒（CM §6.3：link 不能嵌套）——opener 节点降级为
        // 普通文本，`]` 按文本——不留 Bracket 类型残留（渲染同为文本）。
        inlines[text_idx as usize].frame = InlineFrame::plain(0);
        inlines.push(make_vi(pos, pos + 1, InlineFrame::plain(0), base_offset));
        if !stack.iter().any(|e| matches!(e, StackEntry::Bracket { .. })) { *no_link_openers = false; }
        return pos + 1;
    }
    if bracket_after {
        let after = &input[pos + 1..];
        if !after.starts_with('(') && !after.starts_with('[') {
            inlines[text_idx as usize].frame = InlineFrame::plain(0);
            inlines.push(make_vi(pos, pos + 1, InlineFrame::plain(0), base_offset));
            return pos + 1;
        }
    }
    process_emphasis_fn(inlines, stack, bp, children);
    let after = &input[pos + 1..];
    let mut new_pos = pos + 1;
    let mut link_created = false;
    if after.starts_with('(') {
        let body_start = new_pos + 1; // byte after `(`
        if let Some(pe) = scan_dest_body(input, body_start) {
            let body = &input[body_start..pe - 1];
            link_created = body.is_empty()
                || parse_dest_body(body).map_or(false, |(_, _, consumed)| {
                    !body[consumed..].as_bytes().iter().any(|&b| b != b' ' && b != b'\t' && b != b'\n' && b != b'\r')
                });
            if link_created { new_pos = pe; }
        }
    } else if after.starts_with('[') {
        link_created = resolve_ref_link_fn(inlines, input, &mut new_pos, text_idx, byte_start, pos, image,
                                           refmap, base_offset);
    } else {
        link_created = resolve_shortcut_link_fn(inlines, input, &mut new_pos, text_idx, byte_start, pos, image,
                                                refmap, base_offset);
    }
    if !link_created {
        inlines.push(make_vi(pos, pos + 1, InlineFrame::plain(0), base_offset));
        return pos + 1;
    }
    // CM §6.3: link 不能嵌套——任何 link 创建（inline/ref/shortcut）后置位，
    // 后续外层 `]` 被拒（no_link_openers 机制）。
    if !image { *no_link_openers = true; }
    // For inline links: create the shell here.  Ref/shortcut links already
    // created theirs via emit_ref_link_fn.
    if after.starts_with('(') {
        inlines[text_idx as usize].byte_start = 0;
        inlines[text_idx as usize].byte_end = 0;
        let li = inlines.len() as u16;
        children.resize(inlines.len() + 1, smallvec::SmallVec::new());
        for idx in text_idx as usize..inlines.len() {
            if inlines[idx].parent == PARENT_NONE {
                inlines[idx].parent = li;
                children[li as usize].push(idx as u16);
            }
        }
        inlines.push(make_vi(byte_start as usize, new_pos,
            InlineFrame::bracket(image, false), base_offset));
    }
    if *no_link_openers && !stack.iter().any(|e| matches!(e, StackEntry::Bracket { .. })) {
        *no_link_openers = false;
    }
    new_pos
}

fn openers_bottom_index(delim_byte: u8, num_chars: u8, can_open: bool) -> usize {
    match delim_byte { 0 => 6 + if can_open { 3 } else { 0 } + (num_chars % 3) as usize, 1 => 5, 2 => 1, _ => 0 }
}

fn is_ascii_punct(b: u8) -> bool {
    (CHAR_CLASS[b as usize] & 1) != 0
}


// 5. INLINE_DISPATCH table



// 6. scan_delims


/// Character class LUT: bit0=punct, bit1=ws, bit2=alnum, bit3=non_ascii.
/// Replaces `is_ascii_punct`, `is_ws`, `is_punct` branch chains with
/// single-byte lookups in the emphasis flanking hot path.
static CHAR_CLASS: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut b = 0u8;
    while b < 128 {
        if b.is_ascii_punctuation() { t[b as usize] |= 1; }
        if b.is_ascii_whitespace() { t[b as usize] |= 2; }
        if b.is_ascii_alphanumeric() { t[b as usize] |= 4; }
        b += 1;
    }
    while b < 255 { t[b as usize] = 8; b += 1; }
    t[255] = 8;
    t
};

fn prev_char(input: &str, pos: usize) -> Option<char> {
    if pos == 0 { return None; }
    let bytes = input.as_bytes();
    // ASCII fast path: most markdown is ASCII, and single-byte chars don't
    // require UTF-8 continuation-byte backtracking.
    if bytes[pos - 1] & 0x80 == 0 { return Some(bytes[pos - 1] as char); }
    let mut p = pos - 1;
    while p > 0 && (bytes[p] & 0xC0) == 0x80 { p -= 1; }
    input[p..pos].chars().next_back()
}

fn next_char(input: &str, pos: usize) -> Option<char> {
    let bytes = input.as_bytes();
    if pos >= bytes.len() { return None; }
    // ASCII fast path
    if bytes[pos] & 0x80 == 0 { return Some(bytes[pos] as char); }
    input[pos..].chars().next()
}

#[inline]
fn is_ws(c: char) -> bool {
    let cu = c as u32;
    if cu < 128 { return (CHAR_CLASS[cu as usize] & 2) != 0; }
    // Unicode whitespace: NBSP, OGHAM, en-quad..hair-space, line/para
    // separator, narrow no-break, medium mathematical, ideographic.
    matches!(c,
        '\u{00A0}' | '\u{1680}' |
        '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' |
        '\u{202F}' | '\u{205F}' | '\u{3000}'
    )
}
#[inline]
fn is_punct(c: char) -> bool {
    let cu = c as u32;
    if cu < 128 { return (CHAR_CLASS[cu as usize] & 1) != 0; }
    // Non-ASCII: letter / number / whitespace → not punctuation;
    // everything else (symbols, marks, Unicode P*) → punctuation.
    !c.is_alphanumeric() && !is_ws(c)
}


// 7. RefMap


use ahash::AHashMap;

#[derive(Debug, Clone, Default)]
pub struct RefMap {
    map: AHashMap<String, (String, String)>, // label → (url, title)
}

/// 空 RefMap 共享实例（walk 每块的 inline 扫描用——成本结构诊断
/// 3068 次/文档的 RefMap::new 是固定开销——空表语义等价
/// （无 ref 定义时 link 解析失败快）——OnceLock 首次创建后零分配）。
pub static EMPTY_REFMAP: std::sync::OnceLock<RefMap> = std::sync::OnceLock::new();

impl RefMap {
    pub fn new() -> Self { RefMap { map: AHashMap::new() } }
    /// Insert a reference definition.  First definition wins per CommonMark §4.7.
    pub fn insert(&mut self, label: String, url: String, title: String) {
        self.map.entry(label).or_insert((url, title));
    }
    pub fn get(&self, label: &str) -> Option<&(String, String)> { self.map.get(label) }
    pub fn resolve(&self, label: &str) -> &str { self.map.get(label).map(|(u, _)| u.as_str()).unwrap_or("") }
    #[inline] pub fn is_empty(&self) -> bool { self.map.is_empty() }

    /// Build RefMap from indexed text. Uses frame keying: consecutive RefDef-typed
    /// lines form one logical refdef. A new refdef starts when the line begins with `[`.
    pub fn from_indexed_text(full_text: &str, lines: &[crate::block::PackedLineInfo],
                             line_offsets: &[usize]) -> RefMap {
        use crate::block::BlockType;
        let mut refmap = RefMap::new();
        let mut i = 0usize;
        while i < lines.len() {
            if lines[i].node_type() != BlockType::RefDef { i += 1; continue; }
            let block_start = line_offsets.get(i).copied().unwrap_or(0);
            let mut block_end = i + 1;
            while block_end < lines.len() && lines[block_end].node_type() == BlockType::RefDef {
                let next_start = line_offsets.get(block_end).copied().unwrap_or(0);
                let next_end = line_offsets.get(block_end + 1).copied().unwrap_or(full_text.len()).min(full_text.len());
                let next_text = full_text[next_start..next_end].trim_end();
                if !next_text.is_empty() && next_text.as_bytes()[0] == b'[' { break; }
                block_end += 1;
            }
            let end_byte = line_offsets.get(block_end).copied().unwrap_or(full_text.len()).min(full_text.len());
            let refdef_text = &full_text[block_start..end_byte];
            if let Some((label, url, title)) = parse_refdef_multi(refdef_text) {
                let normalized = normalize_label(&label);
                refmap.insert(normalized, url.clone(), title.clone());
            }
            i = block_end;
        }
        refmap
    }
}

/// Normalize reference link label: trim, collapse whitespace, ASCII lowercase.
/// Single-pass: no intermediate Vec allocation.
pub fn normalize_label(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    normalize_label_into(raw, &mut out);
    out
}

/// Write the normalized label into `out` (clears it first).
/// Reuse `out` across calls to avoid repeated allocations.
pub fn normalize_label_into(raw: &str, out: &mut String) {
    out.clear();
    let mut in_ws = false;
    let bytes = raw.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // CM §4.7: \[ and \] in labels mean literal [ and ].
        if bytes[i] == b'\\' && i + 1 < bytes.len()
            && (bytes[i + 1] == b'[' || bytes[i + 1] == b']')
        {
            i += 1; // skip backslash
        }
        let ch = raw[i..].chars().next().unwrap_or('\0');
        let char_len = ch.len_utf8();
        if ch.is_whitespace() {
            in_ws = true;
        } else {
            if in_ws && !out.is_empty() { out.push(' '); }
            in_ws = false;
            // Unicode case fold: ß (U+00DF) and ẞ (U+1E9E) → "ss"
            if ch == '\u{00DF}' || ch == '\u{1E9E}' {
                out.push_str("ss");
            } else {
                for c in ch.to_lowercase() { out.push(c); }
            }
        }
        i += char_len;
    }
}

// ── Unified destination parser ──

pub(crate) fn parse_dest_body(s: &str) -> Option<(&str, &str, usize)> {
    // CM spec §6.5: dest 前允许空白/换行（`[link](   /uri)`）——trim 后解析。
    // consumed 相对传入的 s（trim 前）——调用者的剩余检查用原始索引。
    let trimmed = s.trim_start();
    let skip = trimmed.as_ptr() as usize - s.as_ptr() as usize;
    let s = trimmed;
    let bytes = s.as_bytes();
    if bytes.is_empty() { return None; }
    let (url, rest, has_ws) = if bytes[0] == b'<' {
        let end = memchr::memchr(b'>', &bytes[1..])?;
        let url = &s[1..1 + end];
        let consumed = 1 + end + 1;
        let rest = &s[consumed..];
        let has_ws = rest.as_bytes().first().map_or(true, |&b| b == b' ' || b == b'\t' || b == b'\n');
        (url, rest, has_ws)
    } else {
        let end = bytes.iter().position(|&b| b == b' ' || b == b'\t' || b == b'\n').unwrap_or(bytes.len());
        let url = &s[..end];
        let rest = &s[end..];
        (url, rest, true)
    };
    let rest_trimmed = rest.trim_start();
    let title = if !rest_trimmed.is_empty() && has_ws {
        let b = rest_trimmed.as_bytes();
        match b[0] {
            b'"' | b'\'' => {
                let quote = b[0];
                let inner = &b[1..];
                let mut i = 0;
                let end = loop {
                    match memchr::memchr(quote, &inner[i..]) {
                        Some(pos) => {
                            let abs = i + pos;
                            let mut bs = 0; let mut j = abs;
                            while j > 0 && inner[j - 1] == b'\\' { bs += 1; j -= 1; }
                            if bs % 2 == 0 { break Some(abs); }
                            i = abs + 1;
                        }
                        None => break None,
                    }
                }?;
                Some((&rest_trimmed[1..1 + end], 1 + end + 1))
            }
            b'(' => {
                let end = memchr::memchr(b')', &b[1..])?;
                Some((&rest_trimmed[1..1 + end], 1 + end + 1))
            }
            _ => None,
        }
    } else { None };
    let consumed = skip + (rest_trimmed.as_ptr() as usize - s.as_ptr() as usize)
        + title.as_ref().map(|(_, len)| *len).unwrap_or(0);
    let title_str = title.map(|(t, _)| t).unwrap_or("");
    Some((url, title_str, consumed))
}

/// Parse a multi-line refdef block. Joins continuation lines into a single
/// body string, then delegates to parse_dest_body.
pub(crate) fn parse_refdef_multi(text: &str) -> Option<(String, String, String)> {
    let label_end = crate::block::find_label_end(text.as_bytes())?;
    if label_end + 1 >= text.as_bytes().len() || text.as_bytes()[label_end + 1] != b':' { return None; }
    let label = text[1..label_end].to_string();
    // #553（CM §4.7——label 非空——`[\n ]: /uri`——label="\n " 空白
    // 折叠后为空——官方 p——非 refdef）——空白折叠后空 label 拒绝
    if label.trim().is_empty() { return None; }
    let after_colon = &text[label_end + 2..];
    let mut body = String::with_capacity(after_colon.len());
    let mut first = true;
    for line in after_colon.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }
        if !first { body.push(' '); }
        body.push_str(trimmed);
        first = false;
    }
    let (url, title, consumed) = parse_dest_body(&body)?;
    if consumed < body.len() && body[consumed..].as_bytes().iter().any(|&b| b != b' ' && b != b'\t') { return None; }
    Some((label, url.to_string(), title.to_string()))
}

/// #542（CM §4.7——label 跨行——`[Foo\n  bar]: /url`——官方 refdef——
/// scan 单行限制（find_label_end 单行）未识别）——跨行 label 候选确认:
/// 行 0 以 `[` 开头且单行内 label 未闭合 + 合并两行可解析——确认成功
/// 返回 (label, url, title)——label 含换行（normalize_label 折叠）。
/// Part A0.5（不可见化）与 RefMap::from_indexed_text（条目）共用——
/// 单一确认逻辑（分裂族教训——判定不双份）。
pub(crate) fn parse_refdef_cross_line(line0: &str, line1: &str) -> Option<(String, String, String)> {
    let t0 = line0.trim_start();
    if !t0.starts_with('[') { return None; }
    if crate::block::find_label_end(t0.as_bytes()).is_some() { return None; }
    let mut merged = String::with_capacity(line0.len() + line1.len() + 1);
    merged.push_str(t0);
    merged.push('\n');
    merged.push_str(line1.trim_end());
    parse_refdef_multi(&merged)
}


// 8. Content normalization


pub(crate) fn normalize_fence_content(raw: &str, fence_len: usize, _sub_type: u8) -> String {
    let inner = &raw[fence_len..raw.len() - fence_len];
    // CodeSpan: CM spec §6.3
    // 1. replace line endings with spaces
    // 2. if both begins AND ends with space, and not all spaces → strip one from each
    // 3. spaces are NOT collapsed (multiple spaces preserved)
    let s = inner.replace("\r\n", " ").replace('\n', " ").replace('\r', " ");
    if s.starts_with(' ') && s.ends_with(' ') && !s.chars().all(|c| c == ' ') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn named_entity_value(name: &str) -> Option<&'static str> {
    match name {
        "amp" => Some("&"), "lt" => Some("<"), "gt" => Some(">"), "quot" => Some("\""),
        "apos" => Some("'"), "nbsp" => Some("\u{00A0}"),
        "Auml" => Some("\u{00C4}"), "Ouml" => Some("\u{00D6}"), "Uuml" => Some("\u{00DC}"),
        "auml" => Some("\u{00E4}"), "ouml" => Some("\u{00F6}"), "uuml" => Some("\u{00FC}"),
        "copy" => Some("\u{00A9}"), "reg" => Some("\u{00AE}"), "trade" => Some("\u{2122}"),
        "mdash" => Some("\u{2014}"), "ndash" => Some("\u{2013}"), "hellip" => Some("\u{2026}"),
        "ldquo" => Some("\u{201C}"), "rdquo" => Some("\u{201D}"), "lsquo" => Some("\u{2018}"), "rsquo" => Some("\u{2019}"),
        "laquo" => Some("\u{00AB}"), "raquo" => Some("\u{00BB}"),
        _ => None,
    }
}

pub(crate) fn decode_entity(name: &str) -> Cow<'static, str> {
    if name.starts_with('#') {
        let num = if name.starts_with("#x") || name.starts_with("#X") {
            u32::from_str_radix(&name[2..], 16)
        } else {
            u32::from_str_radix(&name[1..], 10)
        };
        match num { Ok(cp) if cp > 0 => char::from_u32(cp).map(|c| c.to_string().into()).unwrap_or_else(|| "\u{FFFD}".into()), Ok(_) | Err(_) => "\u{FFFD}".into(), }
    } else {
        named_entity_value(name).unwrap_or("").into()
    }
}


// 9. Projection — classify + spec 双表驱动


fn classify(geo: GeoKind) -> SegmentClass {
    match geo {
        GeoKind::Plain => SegmentClass::Leaf,
        GeoKind::PairDelim | GeoKind::Fenced | GeoKind::AngleWrap | GeoKind::Escape | GeoKind::Entity => SegmentClass::SymWrap,
        GeoKind::Bracket => SegmentClass::AsymWrap,
    }
}

#[derive(Debug, Clone, Copy)]
/// pub(crate)（sdl_inline_proj_equiv 比对对象——同
/// inline_proj_spec——仅可见性扩展）
pub(crate) struct InlineProjSpec {
    has_raw: bool,
    has_mark: bool,
    has_tag: bool,
    has_content: bool,
    has_container: bool,
}

impl InlineProjSpec {
    /// pub(crate)（sdl_inline_proj_equiv 比对对象——同
    /// inline_proj_spec——仅可见性扩展）
    pub(crate) fn has(&self, field: InlineField) -> bool {
        match field {
            InlineField::Raw => self.has_raw,
            InlineField::Mark => self.has_mark,
            InlineField::Tag => self.has_tag,
            InlineField::Content => self.has_content,
            InlineField::Title => self.has_tag, // Title uses same path as Tag
        }
    }
}

/// pub(crate)（sdl_inline_proj_equiv 比对对象——感知原子
/// 共享原则——SDL 与生产共享词法层——仅可见性扩展）
pub(crate) fn inline_proj_spec(t: InlineType) -> InlineProjSpec {
    use InlineType::*;
    match t {
        Text | SoftBreak | LineBreak => InlineProjSpec { has_raw: true, has_mark: false, has_tag: false, has_content: true, has_container: false },
        // PairDelim（Emph/Strong/Strikethrough）的 Content 在 children——
        // project(Content) 空——has_content=false（SDL 诊断
        // sdl_inline_proj_equiv 抓漂移——与 INLINE_EFFECTS None_ 一致——
        // guard 提前返回空——零行为变化）
        Emph | Strong | Strikethrough
            => InlineProjSpec { has_raw: true, has_mark: true, has_tag: false, has_content: false, has_container: true },
        CodeSpan | Math | DisplayMath
            => InlineProjSpec { has_raw: true, has_mark: true, has_tag: false, has_content: true, has_container: true },
        Link | Image
            => InlineProjSpec { has_raw: true, has_mark: true, has_tag: true, has_content: true, has_container: true },
        Autolink
            => InlineProjSpec { has_raw: true, has_mark: true, has_tag: false, has_content: true, has_container: true },
        Escaped
            => InlineProjSpec { has_raw: true, has_mark: true, has_tag: false, has_content: true, has_container: true },
        Entity
            => InlineProjSpec { has_raw: true, has_mark: true, has_tag: false, has_content: true, has_container: true },
        HtmlInline
            => InlineProjSpec { has_raw: true, has_mark: false, has_tag: false, has_content: true, has_container: false },
    }
}

fn extract_mark(vi: &InlineNode, raw: &str, cls: SegmentClass) -> String {
    match cls {
        SegmentClass::Leaf => String::new(),
        SegmentClass::SymWrap => match vi.frame.geo_kind() {
            GeoKind::PairDelim => {
                let ou = vi.frame.opener_use() as usize;
                format!("{}{}", &raw[..ou], &raw[raw.len() - ou..])
            }
            GeoKind::Fenced => {
                let fl = vi.frame.fence_len() as usize;
                format!("{}{}", &raw[..fl], &raw[raw.len() - fl..])
            }
            GeoKind::AngleWrap => "<>".into(),
            GeoKind::Escape => raw[..1].to_string(),
            GeoKind::Entity => "&;".into(),
            _ => String::new(),
        },
        SegmentClass::AsymWrap => {
            if vi.frame.is_image() { "![]()".into() }
            else if vi.frame.is_ref() { "[][]".into() }
            else { "[]()".into() }
        },
    }
}

fn extract_tag(vi: &InlineNode, raw: &str, cls: SegmentClass, _refmap: &RefMap) -> String {
    match cls {
        SegmentClass::Leaf | SegmentClass::SymWrap => String::new(),
        SegmentClass::AsymWrap => {
            // Tag = bracket text: [text] → text, ![alt] → alt
            let offset = if vi.frame.is_image() { 2 } else { 1 };
            let end = memchr::memchr(b']', raw.as_bytes()).unwrap_or(raw.len());
            raw[offset..end].to_string()
        }
    }
}

pub fn project<'a>(vi: &InlineNode, content: &'a str, refmap: &'a RefMap, field: InlineField) -> Cow<'a, str> {
    let spec = inline_proj_spec(vi.frame.inline_kind());
    if !spec.has(field) { return "".into(); }
    let raw = &content[vi.byte_start as usize..vi.byte_end as usize];
    let cls = classify(vi.frame.geo_kind());
    match field {
        InlineField::Raw => raw.into(),
        InlineField::Content => extract_content(vi, raw, cls, refmap),
        InlineField::Mark => extract_mark(vi, raw, cls).into(),
        InlineField::Tag => extract_tag(vi, raw, cls, refmap).into(),
        InlineField::Title => match vi.frame.geo_kind() {
            GeoKind::Bracket if vi.frame.is_ref() => {
                let label = normalize_label(extract_label(raw));
                refmap.get(&label).map(|(_, t)| Cow::Borrowed(t.as_str())).unwrap_or_default()
            }
            GeoKind::Bracket => Cow::Borrowed(extract_title(raw)),
            _ => "".into(),
        },
    }
}

fn extract_content<'a>(vi: &InlineNode, raw: &'a str, cls: SegmentClass, refmap: &RefMap) -> Cow<'a, str> {
    match cls {
        SegmentClass::Leaf => raw.into(),
        SegmentClass::SymWrap => match vi.frame.geo_kind() {
            GeoKind::PairDelim | GeoKind::Bracket => "".into(),
            GeoKind::Fenced => {
                let f = vi.frame.fence_len() as usize;
                normalize_fence_content(raw, f, vi.frame.fenced_sub()).into()
            }
            GeoKind::AngleWrap => raw[1..raw.len()-1].into(),
            GeoKind::Escape => raw[1..].into(),
            GeoKind::Entity => raw[1..raw.len()-1].into(),
            _ => "".into(),
        },
        SegmentClass::AsymWrap => {
            if vi.frame.is_ref() {
                let label = normalize_label(extract_label(raw));
                Cow::Owned(refmap.resolve(&label).to_string())
            } else {
                // Raw URL — decode happens at JSON boundary (write_inline_node_json)
                Cow::Borrowed(extract_url(raw))
            }
        },
    }
}

/// Recursive Content projection: PairDelim/Bracket collect children via parent chain and join.
pub fn inline_content(inlines: &[InlineNode], content: &str, refmap: &RefMap) -> String {
    let mut indices: Vec<usize> = (0..inlines.len()).collect();
    indices.sort_by_key(|&i| inlines[i].byte_start);
    let mut parts = Vec::new();
    for idx in indices {
        if inlines[idx].parent == PARENT_NONE {
            let s = inline_content_recursive(idx, inlines, content, refmap);
            if !s.is_empty() { parts.push(s); }
        }
    }
    parts.join("")
}

fn inline_content_recursive(vi_idx: usize, inlines: &[InlineNode], content: &str, refmap: &RefMap) -> String {
    let vi = &inlines[vi_idx];
    match vi.frame.geo_kind() {
        GeoKind::Plain => match vi.frame.plain_sub() {
            0 /*Text*/ => if vi.byte_start < vi.byte_end { content[vi.byte_start as usize..vi.byte_end as usize].to_string() } else { String::new() },
            1 /*SoftBreak*/ => " ".to_string(),
            2 /*LineBreak*/ => "\n".to_string(),
            _ => String::new(),
        },
        GeoKind::PairDelim | GeoKind::Bracket => {
            let mut children: Vec<usize> = inlines.iter().enumerate()
                .filter(|(_, child)| child.parent as usize == vi_idx && child.byte_start < child.byte_end)
                .map(|(idx, _)| idx)
                .collect();
            children.sort_by_key(|&idx| inlines[idx].byte_start);
            let mut parts = Vec::new();
            for idx in children {
                let s = inline_content_recursive(idx, inlines, content, refmap);
                if !s.is_empty() { parts.push(s); }
            }
            parts.join("")
        }
        GeoKind::Fenced => {
            let f = vi.frame.fence_len() as usize;
            let raw = &content[vi.byte_start as usize..vi.byte_end as usize];
            normalize_fence_content(raw, f, vi.frame.fenced_sub())
        }
        GeoKind::AngleWrap => { let raw = &content[vi.byte_start as usize..vi.byte_end as usize]; raw[1..raw.len()-1].to_string() }
        GeoKind::Escape   => { let raw = &content[vi.byte_start as usize..vi.byte_end as usize]; raw[1..].to_string() }
        GeoKind::Entity   => { let raw = &content[vi.byte_start as usize..vi.byte_end as usize]; raw[1..raw.len()-1].to_string() }
    }
}

// ── extract helpers (used by project Raw/Mark/Tag only; Content goes through recursive path) ──

fn extract_url(raw: &str) -> &str {
    let start = raw.find("](").map(|p| p + 2).unwrap_or(0);
    let after_paren = &raw[start..];
    // 复用判定链（scan_dest_body + parse_dest_body）：balanced parens、
    // angle URL、前导空白——与 link 有效性判定单一事实源。
    // 投影时 link 已确认有效（scan 判定通过），此处应恒命中；fallback 保留防御。
    if let Some(pe) = scan_dest_body(after_paren, 0) {
        if pe > 1 {
            let body = &after_paren[..pe - 1];
            if let Some((url, _, _)) = parse_dest_body(body) {
                return url;
            }
        }
    }
    let bytes = after_paren.as_bytes();
    let mut i = 0;
    // Scan for first unescaped ')' or space/tab/newline that ends the URL
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && (bytes[i+1] == b')' || bytes[i+1] == b'(') {
            i += 2; // skip escaped paren
            continue;
        }
        if bytes[i] == b')' || bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b'\n' {
            break;
        }
        i += 1;
    }
    &after_paren[..i]
}

/// Decode backslash escapes and entities in link destination/title.
pub(crate) fn decode_link_dest(raw: &str) -> Cow<'_, str> {
    if raw.is_empty() { return "".into(); }
    let bytes = raw.as_bytes();
    // Fast path: no backslash or ampersand
    if !bytes.contains(&b'\\') && !bytes.contains(&b'&') { return raw.into(); }
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() && is_ascii_punct(bytes[i + 1]) {
            out.push(bytes[i + 1] as char);
            i += 2;
        } else if bytes[i] == b'&' {
            let j = memchr::memchr(b';', &bytes[i..]).map(|p| i + p);
            if let Some(end) = j {
                let entity = &raw[i + 1..end];
                out.push_str(&decode_entity(entity));
                i = end + 1;
            } else {
                out.push('&');
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out.into()
}

fn extract_title(raw: &str) -> &str {
    let start = raw.find("](").map(|p| p + 2).unwrap_or(0);
    let after_paren = &raw[start..];
    // 复用判定链（scan_dest_body + parse_dest_body）：多行 dest、前导空白、
    // 转义引号——与 link 有效性判定单一事实源。投影时 link 已确认有效。
    if let Some(pe) = scan_dest_body(after_paren, 0) {
        if pe > 1 {
            let body = &after_paren[..pe - 1];
            if let Some((_, title, _)) = parse_dest_body(body) {
                return title;
            }
        }
    }
    ""
}

fn extract_label(raw: &str) -> &str {
    let offset = if raw.starts_with("![") { 2 } else { 1 };
    if let Some(close_pos) = raw.find("][") {
        let label = &raw[close_pos + 2..];
        if label.len() <= 1 { &raw[offset..close_pos] } else { &label[..label.len()-1] }
    } else if let Some(rbracket) = raw.rfind(']') { &raw[offset..rbracket] }
    else { "" }
}


// 10. scan_inline entry points


/// Parse inline elements with zero base offset.  Allocates fresh buffers.
pub fn scan_inline(content: &str, refmap: &RefMap) -> Vec<InlineNode> {
    let mut scratch = InlineScratch::new();
    scan_inline_into(content, refmap, 0, &mut scratch);
    scratch.inlines
}

/// Parse inline elements with a global byte offset applied to all VI
/// positions (no post-scan loop).  Allocates fresh buffers.
pub fn scan_inline_offset(content: &str, refmap: &RefMap, offset: usize) -> Vec<InlineNode> {
    let mut scratch = InlineScratch::new();
    scan_inline_into(content, refmap, offset, &mut scratch);
    scratch.inlines
}

/// Parse inline elements, reusing buffers from `scratch`.  The offset is
/// applied during VI creation — no secondary traversal.
pub fn scan_inline_into(content: &str, refmap: &RefMap, offset: usize, scratch: &mut InlineScratch) {
    #[cfg(feature = "telemetry")] let _s_tot = crate::telemetry_span!("inl_total");
    let bytes = content.as_bytes();
    let mut stack = std::mem::take(&mut scratch.stack);
    let mut inlines = std::mem::take(&mut scratch.inlines);
    let mut children = std::mem::take(&mut scratch.children);
    stack.clear();
    inlines.clear();
    children.clear();
    let mut pos = 0usize;
    let mut no_link_openers = false;

    // [插桩] inline 扫描内部分段(全量路径追到底——每块调用一次——不 cap)
    let _span_il = crate::telemetry_span!("inl_loop");
    while pos < bytes.len() {
        // u16 上限防护（真实边界 bug：
        // InlineNode.parent 是 u16（PARENT_NONE=0xFFFF）——单次 scan 节点数
        // > 0xF000 时 process_emphasis 的 opener 状态错乱（byte 坐标溢出——
        // debug panic（release 掩盖））——余量 4096 给迭代内 push——防御：
        // 上限截断（剩余未解析——输出不完整但结构合法——与
        // snap_down_char_boundary 同哲学——不崩——真实文档单块不触达）
        // （~64K 节点 ≈ 150KB inline 密集单段）
        if inlines.len() >= 0xF000 {
            break;
        }
        let result = dispatch_one(content, pos, &stack, &mut inlines,
                                   offset, no_link_openers);
        match result.stack_action {
            StackAction::Push(e) => stack.push(e),
            StackAction::CloseBracket => {
                #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("inl_close");
                pos = handle_close_bracket_fn(&mut stack, &mut inlines,
                    &mut children,
                    content, result.new_pos, refmap, offset, &mut no_link_openers);
                continue;
            }
            StackAction::None => {}
        }
        pos = result.new_pos;
    }
    drop(_span_il);

    if let Some(last) = inlines.last_mut() {
        if last.frame.geo_kind() == GeoKind::Plain && last.frame.plain_sub() == 0 {
            let end = last.byte_end as usize - offset;
            let begin = last.byte_start as usize - offset;
            let mut trim = 0usize;
            while trim < end - begin && bytes[end - 1 - trim] == b' ' { trim += 1; }
            if trim > 0 { last.byte_end -= trim as u32; }
        }
    }

    let _span_ie = crate::telemetry_span!("inl_emphasis");
    process_emphasis_fn(&mut inlines, &mut stack, 0, &mut children);
    drop(_span_ie);
    children.resize(inlines.len(), smallvec::SmallVec::new());
    scratch.inlines = inlines;
    scratch.stack = stack;
    scratch.children = children;
}



// 函数式 dispatch 新类型 + consume 函数（纯增量，未被调用）



// ── §2 route 子类型常量 ──
const CODE: u8 = 0;
const MATH: u8 = 1;
const DISPLAY_MATH: u8 = 2;
const STAR: u8 = 0;
const USCORE: u8 = 1;
const TILDE: u8 = 2;
const OPEN: u8 = 0;
const CLOSE: u8 = 1;
const BANG: u8 = 2;

// ── 新增类型 ──

#[derive(Debug)]
enum SegmentClass { Leaf, SymWrap, AsymWrap }

struct SegmentResult {
    new_pos: usize,
    stack_action: StackAction,
}

// Phase 2 uses StackAction with StackEntry (same type, unified after 3a.ii).
enum StackAction {
    None,
    Push(StackEntry),
    CloseBracket,
}

#[derive(Debug, Clone, Copy)]
struct FencedSpec {
    open: u8,
    min_fence: u8,
    max_fence: u8,
    normalize: bool,
    sub_type: u8,
}

#[derive(Debug, Clone, Copy)]
struct DelimSpec {
    code: u8,
    byte: u8,
    flanking: FlankingRule,
}

#[derive(Debug, Clone, Copy)]
enum FlankingRule { Standard, Underscore, Tilde }

// ── FENCED_SPECS 表 ──

const FENCED_SPECS: [FencedSpec; 2] = [
    // CodeSpan: `, 1–MAX_BACKTICK_RUN, normalize=true
    FencedSpec { open: b'`', min_fence: 1, max_fence: MAX_BACKTICK_RUN as u8, normalize: true, sub_type: CODE },
    // Math/DisplayMath: $, 1–255, normalize=false, sub_type determined at call site
    FencedSpec { open: b'$', min_fence: 1, max_fence: 255, normalize: false, sub_type: 0 },
];

fn lookup_fenced_spec(fence_byte: u8, run_len: u8) -> Option<&'static FencedSpec> {
    FENCED_SPECS.iter().find(|s| s.open == fence_byte && run_len >= s.min_fence && run_len <= s.max_fence)
}

// ── TRIGGER_LUT — 256 字节静态表，与 route() 触发字节集一致 ──

pub(crate) static TRIGGER_LUT: [u8; 256] = {
    let mut t = [0u8; 256];
    t[b'\r' as usize] = 1; t[b'\n' as usize] = 1;
    t[b'`' as usize] = 1; t[b'*' as usize] = 1; t[b'_' as usize] = 1; t[b'~' as usize] = 1;
    t[b'[' as usize] = 1; t[b']' as usize] = 1; t[b'!' as usize] = 1;
    t[b'\\' as usize] = 1; t[b'&' as usize] = 1; t[b'<' as usize] = 1; t[b'$' as usize] = 1;
    t
};

// ── 公共 helpers ──

#[inline]
fn make_vi(begin: usize, end: usize, frame: InlineFrame, offset: usize) -> InlineNode {
    InlineNode {
        byte_start: (begin + offset) as u32,
        byte_end: (end + offset) as u32,
        frame,
        parent: PARENT_NONE,
    }
}

// ── skip_text_dispatch — 用 TRIGGER_LUT 批量跳过非触发字节 ──

/// 基线实现（逐字节 LUT）——所有平台的语义基准。
fn skip_text_dispatch_baseline(bytes: &[u8], pos: usize) -> usize {
    let mut p = pos;
    while p < bytes.len() && TRIGGER_LUT[bytes[p] as usize] == 0 {
        p += 1;
    }
    p
}

/// 触发字节集（与 TRIGGER_LUT 一致——含 \r \n 反引号 * _ ~ [ ] ! 反斜杠 & < $ 共 13 个）。
const TRIGGER_BYTES: [u8; 13] = [
    b'\r', b'\n', b'\x60', b'*', b'_', b'~', b'[', b']', b'!', b'\\', b'&', b'<', b'$',
];

/// NEON 版（aarch64）——13×vceq 一次 16 字节精确比较（vceq 逐字节无借位——正确性由构造保证）。
/// 找"首个触发字节"= 找首个匹配——掩码最低置位即真匹配。
#[cfg(target_arch = "aarch64")]
fn skip_text_dispatch_neon(bytes: &[u8], pos: usize) -> usize {
    use std::arch::aarch64::*;
    const FULL: u64 = 0x0101_0101_0101_0101;
    let mut p = pos;
    // 长度守卫: 剩余 <16B 直接基线——不付向量构建开销
    // （真实调用段短——触发密集文档的平均段远小于 16B——此守卫是收益关键）
    if bytes.len().saturating_sub(p) < 16 {
        return skip_text_dispatch_baseline(bytes, p);
    }
    unsafe {
        while p + 16 <= bytes.len() {
            let v = vld1q_u8(bytes.as_ptr().add(p));
            let mut cmp = vdupq_n_u8(0);
            // vdupq 在循环体内——首次进入才付——LLVM LICM 提升为循环不变
            for tv in [
                vdupq_n_u8(TRIGGER_BYTES[0]), vdupq_n_u8(TRIGGER_BYTES[1]),
                vdupq_n_u8(TRIGGER_BYTES[2]), vdupq_n_u8(TRIGGER_BYTES[3]),
                vdupq_n_u8(TRIGGER_BYTES[4]), vdupq_n_u8(TRIGGER_BYTES[5]),
                vdupq_n_u8(TRIGGER_BYTES[6]), vdupq_n_u8(TRIGGER_BYTES[7]),
                vdupq_n_u8(TRIGGER_BYTES[8]), vdupq_n_u8(TRIGGER_BYTES[9]),
                vdupq_n_u8(TRIGGER_BYTES[10]), vdupq_n_u8(TRIGGER_BYTES[11]),
                vdupq_n_u8(TRIGGER_BYTES[12]),
            ] {
                cmp = vorrq_u8(cmp, vceqq_u8(v, tv));
            }
            let u = vreinterpretq_u64_u8(cmp);
            let lo = vgetq_lane_u64(u, 0) & FULL;
            let hi = vgetq_lane_u64(u, 1) & FULL;
            if lo == 0 && hi == 0 {
                p += 16;
                continue;
            }
            let mut mask = lo;
            let hit = if mask != 0 {
                (mask.trailing_zeros() / 8) as usize
            } else {
                mask = hi;
                8 + (mask.trailing_zeros() / 8) as usize
            };
            return p + hit;
        }
    }
    skip_text_dispatch_baseline(bytes, p)
}

/// memchr 组合版（x86_64 等非 aarch64 平台）——4×memchr3 + 1×memchr 取 min。
/// memchr 内部按平台选 SWAR/SSE2/AVX2——自身即运行时检测——发布安全。
fn skip_text_dispatch_memchr(bytes: &[u8], pos: usize) -> usize {
    let t = &bytes[pos..];
    let mut best = usize::MAX;
    if let Some(x) = memchr::memchr3(TRIGGER_BYTES[0], TRIGGER_BYTES[1], TRIGGER_BYTES[2], t) { best = best.min(x); }
    if let Some(x) = memchr::memchr3(TRIGGER_BYTES[3], TRIGGER_BYTES[4], TRIGGER_BYTES[5], t) { best = best.min(x); }
    if let Some(x) = memchr::memchr3(TRIGGER_BYTES[6], TRIGGER_BYTES[7], TRIGGER_BYTES[8], t) { best = best.min(x); }
    if let Some(x) = memchr::memchr3(TRIGGER_BYTES[9], TRIGGER_BYTES[10], TRIGGER_BYTES[11], t) { best = best.min(x); }
    if let Some(x) = memchr::memchr(TRIGGER_BYTES[12], t) { best = best.min(x); }
    pos + if best == usize::MAX { t.len() } else { best }
}

/// 入口：按平台选实现——aarch64 NEON（精确）→ x86_64 memchr 组合 → 其他基线。
fn skip_text_dispatch(input: &str, pos: usize) -> usize {
    let bytes = input.as_bytes();
    #[cfg(target_arch = "aarch64")]
    { return skip_text_dispatch_neon(bytes, pos); }
    #[cfg(target_arch = "x86_64")]
    { return skip_text_dispatch_memchr(bytes, pos); }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    { return skip_text_dispatch_baseline(bytes, pos); }
}

// ── count_run / find_closer ──

/// Which block projection produced this inline input segment.
#[derive(Debug, Clone)]
pub enum InlineSegmentRole { Tag, Content, Cell { row: usize, col: usize } }

/// A text segment to feed into the inline parser.
#[derive(Debug, Clone)]
pub struct InlineSegment { pub role: InlineSegmentRole, pub text: String }

pub(crate) fn count_run(bytes: &[u8], pos: usize, byte: u8) -> usize {
    let mut n = 0;
    while pos + n < bytes.len() && bytes[pos + n] == byte {
        n += 1;
    }
    n
}

fn find_closer(bytes: &[u8], pos: usize, fence_byte: u8, run_len: usize) -> Option<usize> {
    let mut close_at = pos;
    while close_at + run_len <= bytes.len() {
        let idx = memchr::memchr(fence_byte, &bytes[close_at..]);
        let Some(idx) = idx else { break; };
        close_at += idx;
        let actual = count_run(bytes, close_at, fence_byte);
        if actual == run_len { return Some(close_at); }
        close_at += actual;
    }
    None
}

// ── route — 签名字节分派 ──

fn route(b: u8, next: Option<u8>) -> Option<(GeoKind, u8)> {
    use GeoKind::*;
    match b {
        b'\\' if next.is_some_and(|n| is_ascii_punct(n)) => Some((Escape, 0)),
        b'`'  => Some((Fenced, CODE)),
        b'$' if !next.is_none_or(|n| n.is_ascii_digit() || n == b' ') => Some((Fenced, MATH)),
        b'*'  => Some((PairDelim, STAR)),
        b'_'  => Some((PairDelim, USCORE)),
        b'~'  => Some((PairDelim, TILDE)),
        b'['  => Some((Bracket, OPEN)),
        b']'  => Some((Bracket, CLOSE)),
        b'!' if next == Some(b'[') => Some((Bracket, BANG)),
        b'<'  => Some((AngleWrap, 0)),
        b'&'  => Some((Entity, 0)),
        b'\n' | b'\r' => Some((Plain, 0)),
        _     => None,
    }
}

// ── text_fallback — 统一 fallback 入口 ──

fn text_fallback(pos: usize, run_len: usize, inlines: &mut Vec<InlineNode>, offset: usize) -> SegmentResult {
    inlines.push(make_vi(pos, pos + run_len, InlineFrame::plain(0), offset));
    SegmentResult { new_pos: pos + run_len, stack_action: StackAction::None }
}

// ── soft_or_hard_break — adv=1 (\n) or adv=2 (\r\n) ──

fn soft_or_hard_break(input: &str, pos: usize, adv: usize, inlines: &mut Vec<InlineNode>, offset: usize) -> InlineNode {
    let bytes = input.as_bytes();
    let is_hard = pos >= 2 && bytes[pos - 1] == b' ' && bytes[pos - 2] == b' ';
    if is_hard {
        if let Some(last) = inlines.last_mut() {
            let off = offset;
            let mut trim = 0usize;
            let end = last.byte_end as usize - off;
            let begin = last.byte_start as usize - off;
            while trim < end - begin && bytes[end - 1 - trim] == b' ' { trim += 1; }
            if trim >= 2 { last.byte_end -= trim as u32; }
        }
    }
    let sub = if is_hard { 2u8 } else { 1u8 };
    make_vi(pos, pos + adv, InlineFrame::plain(sub), offset)
}

// ── pop_shell — 统一 shell 创建 ──

fn pop_shell(inlines: &mut Vec<InlineNode>, text_idx: u16,
             byte_start: u32, byte_end: u32, frame: InlineFrame,
             offset: usize) -> u16 {
    inlines[text_idx as usize].byte_start = 0;
    inlines[text_idx as usize].byte_end = 0;
    let gs = byte_start.wrapping_add(offset as u32);
    let ge = byte_end.wrapping_add(offset as u32);
    let wi = inlines.len() as u16;
    for idx in text_idx as usize..wi as usize {
        if inlines[idx].parent == PARENT_NONE
            && inlines[idx].byte_start >= gs
            && inlines[idx].byte_end <= ge
        {
            inlines[idx].parent = wi;
        }
    }
    inlines.push(InlineNode { byte_start: gs, byte_end: ge, frame, parent: PARENT_NONE });
    wi
}

// ── consume_fenced ──

fn consume_fenced(input: &str, pos: usize, inlines: &mut Vec<InlineNode>, offset: usize) -> SegmentResult {
    let bytes = input.as_bytes();
    let fence_byte = bytes[pos];
    let run_len = count_run(bytes, pos, fence_byte);

    // $ post-run guard: 紧接 digit/space → 拒绝
    let after = bytes.get(pos + run_len).copied().unwrap_or(0);
    if fence_byte == b'$' && (after.is_ascii_digit() || after == b' ') {
        return text_fallback(pos, run_len, inlines, offset);
    }
    if fence_byte == b'`' && run_len > MAX_BACKTICK_RUN as usize {
        return text_fallback(pos, run_len, inlines, offset);
    }

    let spec = match lookup_fenced_spec(fence_byte, run_len as u8) {
        Some(s) => s,
        None => return text_fallback(pos, run_len, inlines, offset),
    };

    let closer_pos = match find_closer(bytes, pos + run_len, fence_byte, run_len) {
        Some(cp) => cp,
        None => return text_fallback(pos, run_len, inlines, offset),
    };
    let actual_len = count_run(bytes, closer_pos, fence_byte);

    // Compute effective sub_type: Math → fenced_sub by run_len
    let sub_type = if spec.open == b'$' {
        if run_len == 1 { MATH } else { DISPLAY_MATH }
    } else {
        spec.sub_type
    };

    match spec.open {
        b'`' => {
            // CodeSpan: create Text opener → pop_shell
            inlines.push(make_vi(pos, pos + run_len, InlineFrame::plain(0), offset));
            let text_idx = (inlines.len() - 1) as u16;
            pop_shell(inlines, text_idx,
                      pos as u32, (closer_pos + actual_len) as u32,
                      InlineFrame::fenced(sub_type, run_len as u8),
                      offset);
        }
        _ => {
            // Math/DisplayMath: direct shell, no Text opener
            inlines.push(make_vi(pos, closer_pos + actual_len,
                                 InlineFrame::fenced(sub_type, run_len as u8), offset));
        }
    }
    SegmentResult { new_pos: closer_pos + actual_len, stack_action: StackAction::None }
}

// ── consume_escape ──

fn consume_escape(_input: &str, pos: usize, inlines: &mut Vec<InlineNode>, offset: usize) -> SegmentResult {
    // route 已保证 next 是 ASCII punct
    inlines.push(make_vi(pos, pos + 2, InlineFrame::escape(), offset));
    SegmentResult { new_pos: pos + 2, stack_action: StackAction::None }
}

// ── consume_entity ──

fn consume_entity(input: &str, pos: usize, inlines: &mut Vec<InlineNode>, offset: usize) -> SegmentResult {
    let bytes = input.as_bytes();
    let mut end = pos + 1;
    // Scan entity body: &...;
    if end < bytes.len() && bytes[end] == b'#' {
        while end < bytes.len() && bytes[end] != b';' { end += 1; }
        if end < bytes.len() {
            end += 1; // include ;
            inlines.push(make_vi(pos, end, InlineFrame::entity(), offset));
            return SegmentResult { new_pos: end, stack_action: StackAction::None };
        }
        // No ; — fallback, entire &...#... as text
        return text_fallback(pos, end - pos, inlines, offset);
    } else {
        while end < bytes.len() && bytes[end].is_ascii_alphanumeric() { end += 1; }
        if end < bytes.len() && bytes[end] == b';' {
            let name = &input[pos + 1..end];
            if named_entity_value(name).is_some() {
                end += 1; // include ;
                inlines.push(make_vi(pos, end, InlineFrame::entity(), offset));
                return SegmentResult { new_pos: end, stack_action: StackAction::None };
            }
            end += 1; // include ; even for unknown entity — text fallback
        }
        return text_fallback(pos, end - pos, inlines, offset);
    }
}

// ── consume_anglewrap ──

fn consume_anglewrap(input: &str, pos: usize, inlines: &mut Vec<InlineNode>, offset: usize) -> SegmentResult {
    let bytes = input.as_bytes();
    let mut end = pos + 1;
    // Quote-aware search for >
    while end < bytes.len() && bytes[end] != b'>' {
        if bytes[end] == b'"' || bytes[end] == b'\'' {
            let quote = bytes[end]; end += 1;
            while end < bytes.len() && bytes[end] != quote { end += 1; }
            if end >= bytes.len() { break; }
        }
        end += 1;
    }
    let has_gt = end < bytes.len();
    if has_gt { end += 1; } // include >
    if has_gt && is_autolink(&input[pos + 1..end - 1]) {
        inlines.push(make_vi(pos, end, InlineFrame::angle_wrap(0), offset));
        return SegmentResult { new_pos: end, stack_action: StackAction::None };
    }
    if !has_gt {
        return text_fallback(pos, 1, inlines, offset);
    }
    // has_gt but not autolink: check if valid HTML inline tag
    // (starts with ASCII letter per CM §6.6)
    let after_lt = bytes.get(pos + 1).copied().unwrap_or(0);
    if after_lt.is_ascii_alphabetic() {
        text_fallback(pos, end - pos, inlines, offset)
    } else {
        // Not a valid tag: only consume '<', rest parsed by main loop
        text_fallback(pos, 1, inlines, offset)
    }
}

fn is_autolink(content: &str) -> bool {
    let bytes = content.as_bytes();
    if bytes.is_empty() { return false; }
    // CM spec §6.5: autolink destination cannot contain spaces/tabs/newlines
    if bytes.iter().any(|&b| b == b' ' || b == b'\t' || b == b'\n' || b == b'\r') {
        return false;
    }
    // A single \ at start means \< was escaped (not an autolink opener)
    if bytes.first() == Some(&b'\\') { return false; }
    // URI scheme check: scheme (letters+digits+.-) followed by ':'
    let uri = memchr::memchr(b':', bytes)
        .map(|cp| cp >= 2 && bytes[..cp].iter().all(|&b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.'))
        .unwrap_or(false);
    // Email check: has '@' (no space already verified above).
    // CM §6.7: backslash escapes do not work in email autolinks.
    let email = memchr::memchr(b'@', bytes).is_some()
        && !bytes.contains(&b'\\');
    uri || email
}

// ── push_emph ──

fn push_emph(input: &str, pos: usize, inlines: &mut Vec<InlineNode>, offset: usize, spec: &DelimSpec) -> SegmentResult {
    let (num, can_open, can_close) = scan_delims_flanking(input, pos, spec.byte, spec.flanking);
    inlines.push(make_vi(pos, pos + num as usize, InlineFrame::plain(0), offset));
    if !can_open && !can_close {
        return SegmentResult { new_pos: pos + num as usize, stack_action: StackAction::None };
    }
    SegmentResult {
        new_pos: pos + num as usize,
        stack_action: StackAction::Push(StackEntry::Emph {
            state: EmphState { can_open, can_close, tombstone: false, num_chars: num, delim_code: spec.code },
            text_idx: (inlines.len() - 1) as u16,
        }),
    }
}

// ── scan_delims_flanking — 接受 FlankingRule 的 scan_delims 变体 ──

fn scan_delims_flanking(input: &str, pos: usize, delim_byte: u8, rule: FlankingRule) -> (u8, bool, bool) {
    let before = prev_char(input, pos);
    let bytes = input.as_bytes();
    let mut num_chars = 0;
    while pos + num_chars < bytes.len() && bytes[pos + num_chars] == delim_byte {
        num_chars += 1;
    }
    let num_chars = num_chars.min(15) as u8;
    let after = next_char(input, pos + num_chars as usize);

    let left_flanking = after.map_or(false, |c| !is_ws(c))
        && after.map_or(true, |c| !is_punct(c) || before.map_or(true, is_ws) || before.map_or(true, is_punct));
    let right_flanking = before.map_or(false, |c| !is_ws(c))
        && before.map_or(true, |c| !is_punct(c) || after.map_or(true, is_ws) || after.map_or(true, is_punct));

    match rule {
        FlankingRule::Standard => (num_chars, left_flanking, right_flanking),
        FlankingRule::Underscore => (
            num_chars,
            left_flanking && (!right_flanking || before.map_or(false, is_punct)),
            right_flanking && (!left_flanking || after.map_or(false, is_punct)),
        ),
        FlankingRule::Tilde => {
            let can = num_chars == 2;
            (num_chars, can && left_flanking, can && right_flanking)
        }
    }
}

// ── push_bracket ──

fn push_bracket(input: &str, pos: usize, inlines: &mut Vec<InlineNode>, offset: usize, image: bool) -> SegmentResult {
    let len = if image { 2 } else { 1 };
    let bracket_after = pos > 0 && input.as_bytes()[pos - 1] == b']';
    inlines.push(make_vi(pos, pos + len, InlineFrame::plain(0), offset));
    SegmentResult {
        new_pos: pos + len,
        stack_action: StackAction::Push(StackEntry::Bracket {
            text_idx: (inlines.len() - 1) as u16,
            byte_start: pos as u32,
            image,
            bracket_after,
        }),
    }
}

// ── delim_spec — STAR/USCORE/TILDE → DelimSpec ──

fn delim_spec(code: u8) -> DelimSpec {
    match code {
        STAR => DelimSpec { code: STAR, byte: b'*', flanking: FlankingRule::Standard },
        USCORE => DelimSpec { code: USCORE, byte: b'_', flanking: FlankingRule::Underscore },
        TILDE => DelimSpec { code: TILDE, byte: b'~', flanking: FlankingRule::Tilde },
        _ => DelimSpec { code: STAR, byte: b'*', flanking: FlankingRule::Standard },
    }
}

// ── dispatch_one — scan_segments 主循环的核心分派函数 ──

#[inline]
fn dispatch_one(input: &str, pos: usize, _stack: &[StackEntry],
                inlines: &mut Vec<InlineNode>,
                offset: usize, no_link_openers: bool) -> SegmentResult {
    let bytes = input.as_bytes();

    // 1. skip text
    let text_end = skip_text_dispatch(input, pos);
    if text_end > pos { inlines.push(make_vi(pos, text_end, InlineFrame::plain(0), offset)); }
    if text_end >= bytes.len() {
        return SegmentResult { new_pos: text_end, stack_action: StackAction::None };
    }

    // 2. multi-byte merge
    let b = bytes[text_end];
    let b_next = bytes.get(text_end + 1).copied();
    match (b, b_next) {
        (b'\\', Some(b'\n') | Some(b'\r')) => {
            let adv = if b_next == Some(b'\r') && bytes.get(text_end + 2) == Some(&b'\n') { 3 } else { 2 };
            inlines.push(make_vi(text_end, text_end + adv, InlineFrame::plain(2), offset));
            return SegmentResult { new_pos: text_end + adv, stack_action: StackAction::None };
        }
        (b'\r', Some(b'\n')) => {
            let node = soft_or_hard_break(input, text_end, 2, inlines, offset);
            inlines.push(node);
            return SegmentResult { new_pos: text_end + 2, stack_action: StackAction::None };
        }
        _ => {}
    }

    // 3. route + dispatch
    match route(b, b_next) {
        Some((GeoKind::Fenced, CODE | MATH)) => consume_fenced(input, text_end, inlines, offset),
        Some((GeoKind::Fenced, _)) => {
            inlines.push(make_vi(text_end, text_end + 1, InlineFrame::plain(0), offset));
            SegmentResult { new_pos: text_end + 1, stack_action: StackAction::None }
        }
        Some((GeoKind::Escape, _)) => consume_escape(input, text_end, inlines, offset),
        Some((GeoKind::Entity, _)) => consume_entity(input, text_end, inlines, offset),
        Some((GeoKind::AngleWrap, _)) => consume_anglewrap(input, text_end, inlines, offset),
        Some((GeoKind::PairDelim, star)) => push_emph(input, text_end, inlines, offset, &delim_spec(star)),
        Some((GeoKind::Bracket, OPEN)) if !no_link_openers => push_bracket(input, text_end, inlines, offset, false),
        Some((GeoKind::Bracket, BANG)) => push_bracket(input, text_end, inlines, offset, true),
        Some((GeoKind::Bracket, CLOSE)) => SegmentResult { new_pos: text_end,
            stack_action: StackAction::CloseBracket },
        Some((GeoKind::Bracket, _)) => {
            inlines.push(make_vi(text_end, text_end + 1, InlineFrame::plain(0), offset));
            SegmentResult { new_pos: text_end + 1, stack_action: StackAction::None }
        }
        Some((GeoKind::Plain, _)) => {
            let node = soft_or_hard_break(input, text_end, 1, inlines, offset);
            inlines.push(node);
            SegmentResult { new_pos: text_end + 1, stack_action: StackAction::None }
        }
        None => {
            inlines.push(make_vi(text_end, text_end + 1, InlineFrame::plain(0), offset));
            SegmentResult { new_pos: text_end + 1, stack_action: StackAction::None }
        }
    }
}


// Tests for the inline scan functions


#[cfg(test)]
mod phase2_tests {
    use super::*;

    // ── TRIGGER_LUT ──

    #[test]
    fn trigger_lut_coverage() {
        assert_eq!(TRIGGER_LUT[b'\n' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'\r' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'`' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'*' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'_' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'~' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'[' as usize], 1);
        assert_eq!(TRIGGER_LUT[b']' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'!' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'\\' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'&' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'<' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'$' as usize], 1);
        assert_eq!(TRIGGER_LUT[b'a' as usize], 0);
        assert_eq!(TRIGGER_LUT[b'Z' as usize], 0);
        assert_eq!(TRIGGER_LUT[0x80], 0); // high byte
    }

    // ── route ──

    #[test]
    fn route_backtick_fenced() {
        assert_eq!(route(b'`', None), Some((GeoKind::Fenced, CODE)));
    }

    #[test]
    fn route_asterisk_pairdelim() {
        assert_eq!(route(b'*', None), Some((GeoKind::PairDelim, STAR)));
    }

    #[test]
    fn route_dollar_math() {
        assert_eq!(route(b'$', Some(b'x')), Some((GeoKind::Fenced, MATH)));
        assert!(route(b'$', Some(b' ')).is_none()); // digit/space guard
        assert!(route(b'$', Some(b'1')).is_none());
    }

    #[test]
    fn route_backslash_escape() {
        assert_eq!(route(b'\\', Some(b'*')), Some((GeoKind::Escape, 0)));
        assert!(route(b'\\', Some(b'a')).is_none());
    }

    #[test]
    fn route_bracket_open_close_bang() {
        assert_eq!(route(b'[', None), Some((GeoKind::Bracket, OPEN)));
        assert_eq!(route(b']', None), Some((GeoKind::Bracket, CLOSE)));
        assert_eq!(route(b'!', Some(b'[')), Some((GeoKind::Bracket, BANG)));
        assert!(route(b'!', Some(b'a')).is_none());
    }

    #[test]
    fn route_newline_plain() {
        assert_eq!(route(b'\n', None), Some((GeoKind::Plain, 0)));
        assert_eq!(route(b'\r', None), Some((GeoKind::Plain, 0)));
    }

    #[test]
    fn route_plain_letter() {
        assert_eq!(route(b'a', None), None);
        assert_eq!(route(b'1', None), None);
    }

    // ── skip_text_dispatch ──

    #[test]
    fn skip_text_dispatch_plain() {
        assert_eq!(skip_text_dispatch("hello", 0), 5);
        assert_eq!(skip_text_dispatch("foo*bar", 0), 3);
        assert_eq!(skip_text_dispatch("abc`def", 0), 3);
    }

    #[test]
    fn skip_text_dispatch_empty() {
        assert_eq!(skip_text_dispatch("", 0), 0);
    }

    #[test]
    fn skip_text_dispatch_trigger_at_pos() {
        assert_eq!(skip_text_dispatch("*foo", 0), 0);
    }

    // ── count_run / find_closer ──

    #[test]
    fn count_run_backtick() {
        assert_eq!(count_run(b"```rest", 0, b'`'), 3);
        assert_eq!(count_run(b"a``b", 1, b'`'), 2);
        assert_eq!(count_run(b"nope", 0, b'`'), 0);
    }

    #[test]
    fn find_closer_exact_match() {
        assert_eq!(find_closer(b"``code``", 2, b'`', 2), Some(6));
    }

    #[test]
    fn find_closer_no_match() {
        assert_eq!(find_closer(b"`code", 1, b'`', 1), None);
    }

    // ── text_fallback ──

    #[test]
    fn text_fallback_basic() {
        let mut inlines = vec![];
        let r = text_fallback(0, 3, &mut inlines, 0);
        assert_eq!(r.new_pos, 3);
        assert!(matches!(r.stack_action, StackAction::None));
        assert_eq!(inlines.len(), 1);
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::Plain);
    }

    // ── consume_escape ──

    #[test]
    fn consume_escape_star() {
        let mut inlines = vec![];
        let r = consume_escape(r"\*foo", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 2);
        assert_eq!(inlines.len(), 1);
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::Escape);
        assert_eq!(inlines[0].byte_start, 0);
        assert_eq!(inlines[0].byte_end, 2);
    }

    // ── consume_entity ──

    #[test]
    fn consume_entity_named() {
        let mut inlines = vec![];
        let r = consume_entity("&amp;", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 5);
        assert_eq!(inlines.len(), 1);
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::Entity);
    }

    #[test]
    fn consume_entity_numeric() {
        let mut inlines = vec![];
        let r = consume_entity("&#62;", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 5);
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::Entity);
    }

    #[test]
    fn consume_entity_unknown() {
        let mut inlines = vec![];
        let r = consume_entity("&zzz;x", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 5); // consumed as text
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::Plain);
    }

    // ── consume_anglewrap ──

    #[test]
    fn consume_anglewrap_autolink() {
        let mut inlines = vec![];
        let r = consume_anglewrap("<https://x.com>", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 15);
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::AngleWrap);
    }

    #[test]
    fn consume_anglewrap_fallback() {
        let mut inlines = vec![];
        let r = consume_anglewrap("<notalink", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 1); // only < consumed
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::Plain);
    }

    // ── consume_fenced ──

    #[test]
    fn consume_fenced_code_span() {
        let refmap = RefMap::new();
        let mut inlines = vec![];
        let r = consume_fenced("`code`", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 6);
        // Should have text opener (zeroed) + shell
        assert!(inlines.len() >= 2);
        // Use existing inline_content to verify
        let _ = inline_content(&inlines, "`code`", &refmap);
    }

    #[test]
    fn consume_fenced_math() {
        let mut inlines = vec![];
        let r = consume_fenced("$x=1$", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 5);
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::Fenced);
        assert_eq!(inlines[0].frame.fenced_sub(), MATH);
    }

    #[test]
    fn consume_fenced_no_closer() {
        let mut inlines = vec![];
        let r = consume_fenced("`code", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 1); // only opener consumed
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::Plain);
    }

    #[test]
    fn consume_fenced_dollar_digit_rejected() {
        let mut inlines = vec![];
        let r = consume_fenced("$100", 0, &mut inlines, 0);
        assert_eq!(r.new_pos, 1); // fallback: $
        assert_eq!(inlines[0].frame.geo_kind(), GeoKind::Plain);
    }

    // ── push_emph ──

    #[test]
    fn push_emph_asterisk() {
        let spec = DelimSpec { code: STAR, byte: b'*', flanking: FlankingRule::Standard };
        let mut inlines = vec![];
        let r = push_emph("*foo", 0, &mut inlines, 0, &spec);
        assert_eq!(r.new_pos, 1);
        assert!(inlines.len() >= 1);
    }

    #[test]
    fn push_emph_cant_open_or_close() {
        let spec = DelimSpec { code: TILDE, byte: b'~', flanking: FlankingRule::Tilde };
        let mut inlines = vec![];
        let r = push_emph("~foo", 0, &mut inlines, 0, &spec);
        assert_eq!(r.new_pos, 1);
        assert!(matches!(r.stack_action, StackAction::None));
    }

    // ── push_bracket ──

    #[test]
    fn push_bracket_normal() {
        let mut inlines = vec![];
        let r = push_bracket("[text", 0, &mut inlines, 0, false);
        assert_eq!(r.new_pos, 1);
        if let StackAction::Push(StackEntry::Bracket { text_idx, image, .. }) = r.stack_action {
            assert!(!image);
            assert_eq!(text_idx, 0);
        } else {
            panic!("expected Push(Bracket)");
        }
    }

    #[test]
    fn push_bracket_image() {
        let mut inlines = vec![];
        let r = push_bracket("![alt", 0, &mut inlines, 0, true);
        assert_eq!(r.new_pos, 2);
        if let StackAction::Push(StackEntry::Bracket { image, .. }) = r.stack_action {
            assert!(image);
        } else {
            panic!("expected Push(Bracket)");
        }
    }

    // ── pop_shell ──

    #[test]
    fn pop_shell_zeros_opener() {
        let mut inlines = vec![
            make_vi(0, 1, InlineFrame::plain(0), 0), // opener text
            make_vi(1, 4, InlineFrame::plain(0), 0), // text inside
        ];
        let wi = pop_shell(&mut inlines, 0, 0, 5, InlineFrame::fenced(CODE, 1), 0);
        assert_eq!(inlines[0].byte_start, 0);
        assert_eq!(inlines[0].byte_end, 0); // zeroed
        assert_eq!(inlines[1].parent, wi); // child assigned
        assert_eq!(inlines[2].frame.geo_kind(), GeoKind::Fenced);
    }

    // ── soft_or_hard_break ──

    #[test]
    fn soft_or_hard_break_normal() {
        let mut inlines = vec![make_vi(0, 1, InlineFrame::plain(0), 0)];
        let vi = soft_or_hard_break("a\nb", 1, 1, &mut inlines, 0);
        assert_eq!(vi.frame.plain_sub(), 1); // SoftBreak
    }

    #[test]
    fn soft_or_hard_break_hard() {
        let mut inlines = vec![make_vi(0, 1, InlineFrame::plain(0), 0)];
        let vi = soft_or_hard_break("x  \nb", 3, 1, &mut inlines, 0);
        assert_eq!(vi.frame.plain_sub(), 2); // LineBreak (hard)
    }

    // ── scan_delims_flanking ──

    #[test]
    fn scan_delims_flanking_standard() {
        let (n, open, close) = scan_delims_flanking("*foo", 0, b'*', FlankingRule::Standard);
        assert_eq!(n, 1);
        assert!(open);
        assert!(!close);
    }

    #[test]
    fn scan_delims_flanking_underscore() {
        // snake_case: _ between alphanumeric chars should not open or close
        let (n, open, close) = scan_delims_flanking("x_x", 1, b'_', FlankingRule::Underscore);
        assert_eq!(n, 1);
        assert!(!open);
        assert!(!close);
    }

    #[test]
    fn scan_delims_flanking_tilde() {
        let (n, open, close) = scan_delims_flanking("~~foo", 0, b'~', FlankingRule::Tilde);
        assert_eq!(n, 2);
        assert!(open);
        assert!(!close);
    }

    #[test]
    fn scan_delims_flanking_tilde_single() {
        let (n, open, close) = scan_delims_flanking("~foo", 0, b'~', FlankingRule::Tilde);
        assert_eq!(n, 1);
        assert!(!open);
        assert!(!close);
    }
}

#[cfg(test)]
mod tests {
    include!("../tests/unit/inline_scan.rs");
}
