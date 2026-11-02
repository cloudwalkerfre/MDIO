//! infer —— 原子化实现。
//!
//! 全模块 = 从 PL+FL 出发的原子工具集，
//! 一切消费 = 原子组合，消费方不自带遍历。
//!
//! 分区：
//!   §0 原子层：partition / walk 族 / PrevSlot（遍历与状态的唯一实现）
//!   §1 fold 库：EntryFold 的累加器实例（每个 fold 一个语义产物）
//!   §2 reclassify：空间修正（四路 + 表语义直读断言）
//!   §3 span / locate：唯一导航（区域四分派 + 行域三模式；节点域随树消费点对接）
//!   §4 块树：归属树（五步：expand / attach / segment / comment / 章节）
//!   §5 node 面：几何 / 内容投影 / 写侧（对照 Snapshot 方法族）
//!   §6 trilogy：正逆闭环比对（restore_pl / restore_fl）
//!
//! 依赖方向（硬约束）：
//!   生产代码：block（PL/FL 定义）+ inline（词法助手）+ effect（表）。
//!   测试在 tests/unit/infer.rs（文件尾 include! 承接）。
//!
//! 设计律：
//!   参数注入律：遍历器与 fold 机械内无类型分支；类型语义只经表参数进入。
//!   物化层级律：信息物化到产生它的原子的输出层级。
//!   双形态选型律：流式 / 懒物化 / 物化由访问模式决定。
//!   prev 物化禁则：prev 只存在于折叠槽，永不物化。
//!   正逆同列律：同一表列服务扫描与呈现两向。
//!
//! 不变量（地基性质，文件尾内联测试逐条覆盖）：
//!   1. walk* 函数体内无 BlockType/FrameKind 分支（参数注入律的可检查形态）。
//!   2. walk_zone / walk_lines 前置条件：fold 从初始态出发（前缀恢复自 entries[0]
//!      重放；续走律——分两段走 == 一次走——见 zone_continuation_property）。
//!   3. walk_zone 产出阶段 = [start, end) 行事件；前缀只恢复状态不产出
//!      （产出开关 = on_zone_begin）；start > end 时产出为空。
//!   4. walk_lines 的 per_line：区间内每行恰好一次、该行事件折叠之后回调
//!      （STEP 先于 GET）、区间外不回调。
//!   5. prev 槽只传递不更新（更新规则是 scan 语义，由消费方经 per_line 注入）。

use crate::block::{BlockFrame, BlockType, FrameKind, PackedLineInfo, LOOSE_BIT, MAX_BLOCK_NESTING,
    entry_anchor, entry_frame, entry_is_body_mut, entry_is_pop, entry_line};
use crate::effect::end_for_rule; // 结算原子（closing_pl 列求值器——自 effect 迁入）
use std::collections::HashSet;
use std::sync::Arc;


pub(crate) mod md_line {
    use crate::block::{BlockFrame, BlockType, FrameKind, PackedLineInfo};
    use crate::block::{entry_frame, entry_is_body_mut, entry_is_pop, entry_line};

// -- 行级发射（to_md 的行级基座；ungated，arrow 输出可直接用） --

    pub(crate) fn bq_off(line: &str) -> usize {
        let b = line.as_bytes();
        let mut off = 0;
        loop {
            let mut p = off;
            while p < b.len() && (b[p] == b' ' || b[p] == b'\t') { p += 1; }
            if p < b.len() && b[p] == b'>' {
                off = p + 1;
                if off < b.len() && (b[off] == b' ' || b[off] == b'\t') { off += 1; }
            } else { break; }
        }
        off
    }

    /// 原行拆分（标记镜像）：返回 (原缩进片, 内容片)——嵌套缩进保留、
    /// marker 宽度镜像（LineSpec mark_width 语义）。
    pub(crate) fn content_after_marker(line: &str, ft: BlockType) -> (String, String) {
        use BlockType::*;
        let bq = bq_off(line);
        // 结构缩进 = 行首空白段：bq>0 时在 line[..bq] 内（bq_off 已吞并），
        // bq==0 时即整行行首空白。
        let indent = if bq > 0 {
            let pre = &line[..bq.min(line.len())];
            pre[..pre.len() - pre.trim_start_matches([' ', '\t']).len()].to_string()
        } else {
            line[..line.len() - line.trim_start_matches([' ', '\t']).len()].to_string()
        };
        let b = &line.as_bytes()[bq..];
        let mut lw = 0;
        while lw < b.len() && (b[lw] == b' ' || b[lw] == b'\t') { lw += 1; }
        let mb = &b[lw..];
        let mw = match ft {
            Header1 => 1, Header2 => 2, Header3 => 3, Header4 => 4, Header5 => 5, Header6 => 6,
            Bullet => 2,
            Checklist => 5,
            OrderedListItem => {
                let mut d = 0;
                while d < mb.len() && mb[d].is_ascii_digit() { d += 1; }
                d + 2
            }
            _ => 0,
        };
        let mut start = lw + mw.min(mb.len());
        if start < b.len() && b[start] == b' ' { start += 1; }
        let content = line[bq + start..].to_string();
        (indent, content)
    }

    /// restore_content_start 的 7 臂镜像（trilogy.rs:210-294——S5 吸收位）。
    /// **口径（R3 定）**：本函数量的是**Content 投影的起点列**——写面
    /// `block_write_range(Content)` 的起点相对其行首的字节偏移，**不是**「内容文本起点」。
    /// 引用块（BQ）行的 Content 投影 = 整行（「其余→整块」）⇒ 返回 0 **正确**；
    /// 「引用块内文本起点」是另一件事（`strip_bq_layers` 管剥前缀后的列），勿混。
    /// 诚实标注比较域：本版镜像 marker 边界语义（无 colon/bracket tag 域）——
    /// tag 臂（colon key/bracket key/checkbox-tag 的 inline 跳过 finder）留扩展位
    /// （语料加入 tag 行后补 finder 镜像再扩域）。双轨对拍在同一域上成立。
    /// **块级几何起点（围栏开行 CodeFenceOpen / setext HsHead1）不在本函数职责**——
    /// 由 `test_production_write_side_dual_track` 的块级几何比法管（R5）。
    pub fn content_start_of(line: &str, ft: BlockType) -> usize {
        use BlockType::*;
        let bq = bq_off(line);
        let b = &line.as_bytes()[bq..];
        let mut lw = 0;
        while lw < b.len() && (b[lw] == b' ' || b[lw] == b'\t') { lw += 1; }
        let mb = &b[lw..];
        match ft {
            Bullet | Checklist => {
                if mb.len() >= 2 && matches!(mb[0], b'-' | b'*' | b'+')
                    && (mb[1] == b' ' || mb[1] == b'\t') {
                    bq + lw + 2
                } else { 0 }
            }
            OrderedListItem => {
                let mut d = 0;
                while d < mb.len() && mb[d].is_ascii_digit() { d += 1; }
                if d > 0 && d < mb.len() && (mb[d] == b'.' || mb[d] == b')') {
                    bq + lw + d + 2
                } else { 0 }
            }
            Header1 | Header2 | Header3 | Header4 | Header5 | Header6 => {
                let after_bq = &line[bq..];
                bq + (after_bq.len() - after_bq.trim_start_matches(&['#', ' '][..]).len())
            }
            Comment => line.find("<!--").unwrap_or(0) + 4,
            IndentedCode => bq + 4,
            // `CodeFenceOpen` / `HsHead1|2` 两臂**尝试后已回退**：
            // 围栏臂会让内容起点越界（`"```"` 3 字节 -> 4），撞既有「起点不越界」判官；
            // setext 臂未命中（生产口径未读透）。两处缺口仍由双轨测试钉住计数（== 4）。
            _ => 0,
        }
    }


    /// 按深度限层剥 BQ：返回 (结构缩进, 剥 n 层后的内容起点)。
    /// 行首空白=结构缩进；仅剥 depth 层——fence/html 体内的内层 `> ` 是字面内容。
    pub(crate) fn strip_bq_layers(line: &str, depth: usize) -> (usize, usize) {
        let bytes = line.as_bytes();
        let mut lead = 0;
        while lead < bytes.len() && (bytes[lead] == b' ' || bytes[lead] == b'\t') { lead += 1; }
        let mut p = lead;
        for _ in 0..depth {
            if p < bytes.len() && bytes[p] == b'>' {
                p += 1;
                if p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') { p += 1; }
            } else { break; }
        }
        (lead, p)
    }

    /// 行级发射（to_md 的手臂提取，供投影/输出层复用；单一来源：to_md 的行级镜像）。
    /// - `bq_depth` 来自帧栈（调用方按行折叠事件）；
    /// - `prev_ft` 供 setext 下划线判定；
    /// - `content_override` 仅对 content 面生效（Header/Bullet/Checklist/OL/Paragraph/BQ 内容驱动），
    ///   其余 arm（fence/setext/thematic/IC 等）忽略；
    /// - 返回若干行（含尾部 \n；HsHead1/2 返回两行；setext 下划线返回空）。
    pub fn emit_line(
        line: &str,
        ft: BlockType,
        prev_ft: Option<BlockType>,
        bq_depth: usize,
        content_override: Option<&str>,
    ) -> Vec<String> {
        use BlockType::*;
        let prefix = "> ".repeat(bq_depth);
        let mut out: Vec<String> = Vec::new();
        let ov = |c: String| content_override.map(|s| s.to_string()).unwrap_or(c);
        match ft {
            Blank => out.push("\n".to_string()),
            ThematicBreak => {
                let bq = bq_off(line);
                let lead = if bq > 0 {
                    let pre = &line[..bq.min(line.len())];
                    pre.len() - pre.trim_start_matches([' ', '\t']).len()
                } else {
                    line.len() - line.trim_start_matches([' ', '\t']).len()
                };
                out.push(format!("{}{}---\n", &line[..lead.min(line.len())], prefix));
            }
            HsSep => {
                match prev_ft {
                    Some(HsHead1) | Some(HsHead2) => {}
                    _ => {
                        let bq = bq_off(line);
                        let lead = if bq > 0 {
                            let pre = &line[..bq.min(line.len())];
                            pre.len() - pre.trim_start_matches([' ', '\t']).len()
                        } else {
                            line.len() - line.trim_start_matches([' ', '\t']).len()
                        };
                        let content_start = if bq > 0 { bq } else { lead };
                        out.push(format!(
                            "{}{}{}\n",
                            &line[..lead.min(line.len())],
                            prefix,
                            &line[content_start.min(line.len())..]
                        ));
                    }
                }
            }
            CodeFenceOpen | CodeFenceClose => {
                let bq = bq_off(line);
                let lead = if bq > 0 {
                    let pre = &line[..bq.min(line.len())];
                    pre.len() - pre.trim_start_matches([' ', '\t']).len()
                } else {
                    line.len() - line.trim_start_matches([' ', '\t']).len()
                };
                let rest = if bq > 0 {
                    &line[bq.min(line.len())..]
                } else {
                    &line[lead.min(line.len())..]
                };
                let c = rest.as_bytes()[0];
                let mut k = 0;
                while k < rest.len() && rest.as_bytes()[k] == c { k += 1; }
                let mut s = format!("{}{}", &line[..lead.min(line.len())], prefix);
                for _ in 0..k { s.push(c as char); }
                if ft == CodeFenceOpen { s.push_str(&rest[k..]); }
                s.push('\n');
                out.push(s);
            }
            Header1 | Header2 | Header3 | Header4 | Header5 | Header6 => {
                let lv = match ft {
                    Header1 => 1, Header2 => 2, Header3 => 3, Header4 => 4, Header5 => 5, _ => 6,
                };
                let (indent, content) = content_after_marker(line, ft);
                out.push(format!("{}{}{} {}\n", indent, prefix, "#".repeat(lv), ov(content)));
            }
            HsHead1 => {
                let bq = bq_off(line);
                let lead = if bq > 0 {
                    let pre = &line[..bq.min(line.len())];
                    pre.len() - pre.trim_start_matches([' ', '\t']).len()
                } else {
                    line.len() - line.trim_start_matches([' ', '\t']).len()
                };
                let ind = &line[..lead.min(line.len())];
                out.push(format!("{}{}{}\n", ind, prefix, &line[bq..]));
                out.push(format!("{}{}===\n", ind, prefix));
            }
            HsHead2 => {
                let bq = bq_off(line);
                let lead = if bq > 0 {
                    let pre = &line[..bq.min(line.len())];
                    pre.len() - pre.trim_start_matches([' ', '\t']).len()
                } else {
                    line.len() - line.trim_start_matches([' ', '\t']).len()
                };
                let ind = &line[..lead.min(line.len())];
                out.push(format!("{}{}{}\n", ind, prefix, &line[bq..]));
                out.push(format!("{}{}---\n", ind, prefix));
            }
            Bullet => {
                let (indent, content) = content_after_marker(line, ft);
                let bq = bq_off(line);
                let b = &line.as_bytes()[bq..];
                let mut lw = 0;
                while lw < b.len() && (b[lw] == b' ' || b[lw] == b'\t') { lw += 1; }
                let marker = if lw < b.len() && matches!(b[lw], b'-' | b'*' | b'+') { b[lw] as char } else { '-' };
                // BQ 内嵌套列表：item 缩进落在 BQ 前缀之后、marker 之前。
                let inner = if bq > 0 { &line[bq..bq + lw] } else { "" };
                out.push(format!("{}{}{}{} {}\n", indent, prefix, inner, marker, ov(content)));
            }
            Checklist => {
                let (indent, content) = content_after_marker(line, ft);
                let bq = bq_off(line);
                let b = &line.as_bytes()[bq..];
                let mut lw = 0;
                while lw < b.len() && (b[lw] == b' ' || b[lw] == b'\t') { lw += 1; }
                let inner = if bq > 0 { &line[bq..bq + lw] } else { "" };
                out.push(format!("{}{}{}- [ ] {}\n", indent, prefix, inner, ov(content)));
            }
            OrderedListItem => {
                let bq = bq_off(line);
                let b = &line.as_bytes()[bq..];
                let mut lw = 0;
                while lw < b.len() && (b[lw] == b' ' || b[lw] == b'\t') { lw += 1; }
                let indent = if bq > 0 {
                    let pre = &line[..bq.min(line.len())];
                    pre[..pre.len() - pre.trim_start_matches([' ', '\t']).len()].to_string()
                } else {
                    line[..line.len() - line.trim_start_matches([' ', '\t']).len()].to_string()
                };
                let inner = if bq > 0 { &line[bq..bq + lw] } else { "" };
                let mb = &b[lw..];
                let mut d = 0;
                while d < mb.len() && mb[d].is_ascii_digit() { d += 1; }
                let delim = if d < mb.len() { mb[d] } else { b'.' };
                let mut start = lw + d + 1;
                if start < b.len() && b[start] == b' ' { start += 1; }
                let content = ov(line[bq + start..].to_string());
                out.push(format!(
                    "{}{}{}{}{} {}\n",
                    indent,
                    prefix,
                    inner,
                    &line[bq + lw..bq + lw + d],
                    delim as char,
                    content
                ));
            }
            IndentedCode => {
                let bq = bq_off(line);
                let lead = if bq > 0 {
                    let pre = &line[..bq.min(line.len())];
                    pre.len() - pre.trim_start_matches([' ', '\t']).len()
                } else { 0 };
                let rest = &line[bq..];
                out.push(format!("{}{}    {}\n", &line[..lead.min(line.len())], prefix, rest.trim_start_matches([' ', '\t'])));
            }
            Paragraph => {
                let (lead, rest_start) = strip_bq_layers(line, bq_depth);
                let stripped = rest_start > lead;
                if !stripped {
                    out.push(format!("{}\n", ov(line.to_string())));
                } else if line[rest_start..].is_empty() && line.trim_end().ends_with('>') {
                    out.push(format!("{}{}\n", &line[..lead], "> ".repeat(bq_depth).trim_end()));
                } else {
                    let content = ov(line[rest_start..].to_string());
                    out.push(format!("{}{}{}\n", &line[..lead], prefix, content));
                }
            }
            _ => {
                let (lead, rest_start) = strip_bq_layers(line, bq_depth);
                if bq_depth == 0 || rest_start == lead {
                    out.push(format!("{}\n", ov(line.to_string())));
                } else if line[rest_start..].is_empty() && line.trim_end().ends_with('>') {
                    out.push(format!("{}{}\n", &line[..lead], "> ".repeat(bq_depth).trim_end()));
                } else {
                    let content = ov(line[rest_start..].to_string());
                    out.push(format!("{}{}{}\n", &line[..lead], prefix, content));
                }
            }
        }
        out
    }

    /// 每行引用的 BQ 深度（帧栈折叠；调用方用于 emit_line 的 bq_depth）。

    /// 每行 BQ 深度（纯 parts 版；to_md / arrow 共用）。
    pub fn bq_depths(entries: &[u64], lines: &[PackedLineInfo]) -> Vec<usize> {
        use crate::block::{entry_frame, entry_is_body_mut, entry_is_pop, entry_line, BlockFrame, FrameKind};
        let n = lines.len();
        let mut depths = vec![0usize; n];
        let mut ei = 0usize;
        let mut stack: Vec<BlockFrame> = Vec::new();
        for i in 0..n {
            while ei < entries.len() && entry_line(entries[ei]) as usize <= i {
                let e = entries[ei];
                if entry_is_body_mut(e) {
                    let f = entry_frame(e);
                    if let Some(pos) = stack.iter().rposition(|g| g.kind() == f.kind() && g.indent() == f.indent()) {
                        stack[pos] = f;
                    }
                } else if entry_is_pop(e) {
                    stack.pop();
                } else {
                    stack.push(entry_frame(e));
                }
                ei += 1;
            }
            depths[i] = stack.iter().filter(|f| f.kind() == FrameKind::BlockQuote).count();
        }
        depths
    }
}

// §0 原子层


// ── partition：时间定位 ──

/// 事件流按行号二分定位：返回首个 entry_line >= line 的条目下标。
/// 弥合 FL 时间序存储 vs PL 行号序存储的失配——时间轴进入空间坐标的唯一入口。
/// 契约：对任意 line，结果 == 满足 entry_line(e) < line 的条目数。
#[inline]
pub fn partition(entries: &[u64], line: u32) -> usize {
    entries.partition_point(|&e| crate::block::entry_line(e) < line)
}

// （markers_on_line 的锚在 tests/prefix_equiv.rs：markers_on_line_ref；
//  生产站点（ic_line_prefix/list_content）由 prefix_cut(All,Counted) 折叠供给，
//  钉①实证 0 分歧。）

/// 单层切割副产品（prefix_cut 的 layers 输出；层边界=折叠副产品零重解析）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerCut {
    pub kind: crate::block::FrameKind,
    pub start: usize,
    pub end: usize,
}

/// 前缀折叠结果（一律两向两面的唯一坐标源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixCut {
    /// 前缀字节数（取余向：剥；保留向：留）。
    pub bytes: usize,
    /// 前缀列宽（tab 展开——column_width）。
    pub cols: usize,
    /// 链上列表帧 (indent−bytes) 累加（markers_on_line 的 edge 语义）。
    pub edge: usize,
    /// 逐层切割（折叠副产品）。
    pub layers: smallvec::SmallVec<[LayerCut; 4]>,
}

/// 前缀折叠（一律）：BQ/列表层逐层消费——层宽取词法 step_layer
/// （行帧宽=ws+marker）。consume 位（FL 61-56）=光标帧宽（=词法宽，或
/// 词法宽−该层前被列表剥走的前导空白）——两者关系由★前缀等价钉⑥全语料
/// 逐行对拍钉住（分歧即红）；位的生产消费者=写面 format_payload（7.3）。
/// 「折叠直接读位、枚举源=链帧+行开层队列（ws+位零重解析）」=账记待办
/// （钉⑥实证位≠行帧宽在前导空白场景——读位需连带 ws 平移重构）。
/// 停止轴：Chain＝链上帧数配对（BQ 数=链 BQ 帧数、列表数=行上开层数）、
/// Counted＝同计数、None＝不设上限、OneStep＝单层（ic_line_strip_bq）。
/// 前导轴（对齐补漏）：SkipWs＝层宽含前导空白；Exact0＝层必须从位置 0 开始。
/// tail=FinalSkip 时末层后再跳剩余前导空白（content_face 的 rest_after_bq 语义）。
pub fn prefix_cut(
    line: &str,
    chain: &[crate::block::BlockFrame],
    line_entries: &[u64],
    kinds: crate::effect::PrefixKinds,
    stop: crate::effect::PrefixStop,
    lead: crate::effect::PrefixLead,
    tail: crate::effect::PrefixTail,
) -> PrefixCut {
    use crate::effect::{PrefixKinds, PrefixLead, PrefixStop, PrefixTail};
    // 计数来源（两轴分源——markers_on_line 口径实证）：
    //   Counted：BQ 数=链 BQ 帧数（行上自开 BQ 不在栈内）、列表数=行上开层数；
    //   Chain：=链帧数＋行上自开数（剥全语义——content_face 的 peel_peeled
    //   剥全部 BQ 层、node_container 直取前缀含行上自开层）。
    let mut bq_left = chain
        .iter()
        .filter(|f| f.kind() == crate::block::FrameKind::BlockQuote)
        .count();
    let mut list_left = if matches!(stop, PrefixStop::Chain) {
        chain
            .iter()
            .filter(|f| crate::effect::is_list_container(f.kind()))
            .count()
    } else {
        0usize
    };
    for &e in line_entries {
        // P0 修复：防御性跳过 pop——被弹帧的 kind 不得灌进预算（契约仍要求
        // 调用方传本行切片，行号过滤由 line_entries_at 承担）。
        if crate::block::entry_is_pop(e) {
            continue;
        }
        let k = crate::block::entry_frame(e).kind();
        if k == crate::block::FrameKind::BlockQuote {
            if matches!(stop, PrefixStop::Chain) {
                bq_left += 1;
            }
        } else if crate::effect::is_list_container(k) {
            list_left += 1;
        }
    }
    let mut layers: smallvec::SmallVec<[LayerCut; 4]> = smallvec::SmallVec::new();
    let mut cur = line;
    let base = line.as_ptr() as usize;
    let mut cut = 0usize;
    let mut steps = 0usize;
    while let Some((kind, consumed)) = crate::block::step_layer(cur.as_bytes()) {
        let is_bq = kind == crate::block::FrameKind::BlockQuote;
        let is_list = crate::effect::is_list_container(kind);
        // 前导轴（Exact0）：层必须从位置 0 开始——前导空白非零即不匹配
        //（ic_line_strip_bq「缩进后的 > 是内容」语义）。
        if matches!(lead, PrefixLead::Exact0)
            && crate::block::skip_leading_whitespace(cur.as_bytes()).as_ptr() != cur.as_ptr()
        {
            break;
        }
        let allowed = match kinds {
            PrefixKinds::BqOnly => is_bq,
            PrefixKinds::All => is_bq || is_list,
        };
        if !allowed {
            break;
        }
        // 停止判据（Chain 与 Counted 的计数来源同为链/行事实——markers_on_line 同款）
        let keep = match stop {
            PrefixStop::None | PrefixStop::OneStep => true,
            PrefixStop::Chain | PrefixStop::Counted => {
                if is_bq {
                    if bq_left > 0 { bq_left -= 1; true } else { false }
                } else {
                    if list_left > 0 { list_left -= 1; true } else { false }
                }
            }
        };
        if !keep {
            break;
        }
        let start = cur.as_ptr() as usize - base;
        cut = start + consumed;
        layers.push(LayerCut { kind, start, end: cut });
        cur = &cur[consumed..];
        steps += 1;
        if matches!(stop, PrefixStop::OneStep) {
            break;
        }
        // 逐层尾 trim（peel_markers 的 trim_start 语义——marker 后多余空白也计入消费）
        if matches!(tail, PrefixTail::PerLayerTrim) {
            let extra = cur.len() - crate::block::skip_leading_whitespace(cur.as_bytes()).len();
            cut += extra;
            cur = &cur[extra..];
        }
    }
    // 尾处理：FinalSkip＝末层后跳剩余前导空白（content_face 的 rest_after_bq）。
    if matches!(tail, PrefixTail::FinalSkip) {
        let rest = &line[cut..];
        cut += rest.len() - crate::block::skip_leading_whitespace(rest.as_bytes()).len();
    }
    let edge: usize = chain
        .iter()
        .filter(|f| crate::effect::is_list_container(f.kind()))
        .map(|f| (f.indent() as usize).saturating_sub(cut))
        .sum();
    PrefixCut {
        bytes: cut,
        cols: crate::block::column_width(line.as_bytes()[..cut].as_ref()),
        edge,
        layers,
    }
}

/// 段落游程终点：从 start 起的连续 Paragraph 行末（PL 原子纯函数，落点 infer）。
pub(crate) fn paragraph_run_end(lines: &[crate::block::PackedLineInfo], start: usize, end: usize) -> usize {
    let bytes = crate::block::PackedLineInfo::as_bytes(lines);
    let rel = bytes[start.min(end)..end]
        .iter()
        .position(|&b| (b & crate::block::PackedLineInfo::TYPE_MASK) != crate::block::BlockType::Paragraph as u8)
        .unwrap_or(end - start.min(end));
    start + rel
}

/// IC 内容行剥壳（命名规则）：仅位置 0 的单层 `>`＋可选 1 空白；
/// **不跳前导空白**——缩进后的 `>` 是内容（与 strip_bq_one 的 skip-ws
/// 语义不同，勿混用）。normalized_content IC 臂专用。
fn ic_line_strip_bq(s: &str) -> &str {
    // 对齐补漏（六件旧函子归位的最后一件）：prefix_cut 单层折叠——
    // (BqOnly, OneStep, Exact0, None)＝仅位置 0 的单层 `>`＋可选 1 空白、
    // 不跳前导空白（缩进后的 `>` 是内容）——与旧 ic_line_strip_bq 逐字节同义。
    let c = prefix_cut(
        s,
        &[],
        &[],
        crate::effect::PrefixKinds::BqOnly,
        crate::effect::PrefixStop::OneStep,
        crate::effect::PrefixLead::Exact0,
        crate::effect::PrefixTail::None,
    );
    &s[c.bytes.min(s.len())..]
}

/// 关闭行查询——从 start_line 起，第一个把该 kind 相对深度压回阈值的 Pop 处
/// 按 effect::end_for_rule 结算（PopPush 取 Pop 半）。（原 FrameLog 方法迁来——
/// FL×PL 语义查询落点 infer；块侧留薄委托。）
pub fn item_tail_boundary(
    entries: &[u64],
    start_line: u32,
    kind: crate::block::FrameKind,
    lines: &[crate::block::PackedLineInfo],
) -> Option<u32> {
    let prefix = partition(entries, start_line);
    let mut depth: i32 = 0;
    for i in 0..prefix {
        if crate::block::entry_is_body_mut(entries[i]) { continue; }
        let f = crate::block::entry_frame(entries[i]);
        if f.kind() == kind {
            if crate::block::entry_is_pop(entries[i]) {
                depth -= 1;
            } else {
                depth += 1;
            }
        }
    }
    let target_depth = depth;
    let mut i = prefix;
    while i < entries.len() {
        if crate::block::entry_is_body_mut(entries[i]) { i += 1; continue; }
        let line = crate::block::entry_line(entries[i]);
        let f = crate::block::entry_frame(entries[i]);
        let is_pop = crate::block::entry_is_pop(entries[i]);
        if f.kind() == kind {
            if is_pop {
                let is_poppush = is_poppush_pair(entries, i, false);
                if is_poppush {
                    depth -= 1;
                    if depth <= target_depth && line > start_line {
                        let pop_pl = lines.get(line as usize).map(|p| p.node_type())
                            .unwrap_or(crate::block::BlockType::Paragraph);
                        let end = crate::effect::end_for_rule(kind, line as usize, pop_pl);
                        return Some(end as u32);
                    }
                    depth += 1;
                    i += 2;
                    continue;
                }
                depth -= 1;
                if depth <= target_depth && line > start_line {
                    let pop_pl = lines.get(line as usize).map(|p| p.node_type())
                        .unwrap_or(crate::block::BlockType::Paragraph);
                    let end = crate::effect::end_for_rule(kind, line as usize, pop_pl);
                    return Some(end as u32);
                }
            } else {
                depth += 1;
            }
        }
        i += 1;
    }
    None
}

/// 统一坐标入口（节点几何）：行内内容位置的唯一身份＝
/// 列轴律（line_geometry 声明几何）在「全剥 BQ 前缀后字节」上的测量——
/// 与 build_node 的 memo（peel_width/geometry）同构同源（memo 钉背书）。
/// 行前缀全层消费（BQ＋列表）见 markers_on_line——两律两量，勿混用。
pub fn content_face(
    rope_line: &str,
    nt: crate::block::BlockType,
) -> (usize, Option<crate::block::LineGeo>) {
    // R1 对齐（站点迁移）：剥 BQ 前缀改由 prefix_cut 折叠供给——钉②实证
    // (BqOnly,None)+FinalSkip ≡ peel_peeled rest_after_bq（语料 0 分歧）；
    // 只做等价替换（空链、不补几何——与 #3/#4 消回退的递归断点划清边界）。
    let cut = prefix_cut(
        rope_line,
        &[],
        &[],
        crate::effect::PrefixKinds::BqOnly,
        crate::effect::PrefixStop::None,
        crate::effect::PrefixLead::SkipWs,
        crate::effect::PrefixTail::FinalSkip,
    );
    (
        cut.bytes,
        crate::block::line_geometry(nt, &rope_line.as_bytes()[cut.bytes..]),
    )
}

// ── EntryFold / walk 族：双轴遍历器 ──

/// 时间轴折叠接口——双轴遍历器的事件半轴。
/// 三个方法 = 栈三态（开 / 关 / 改）。机械严格类型无关：一切类型语义
/// 经参数注入（表列 + 行值 + 字节），不进入本 trait。
pub trait EntryFold {
    /// zone 起点恢复完成后、产出阶段开始前的钩子。
    /// 需要区分"前缀恢复"与"产出"的 fold（如记录构建）在此切换状态。
    #[inline]
    fn on_zone_begin(&mut self) {}
    fn push(&mut self, e: u64);
    /// Pop 事件。`next` = 事件流中紧随其后的条目（无则为 None）——
    /// PopPush 相邻配对（机械性质）需要它；不需要的 fold 可忽略。
    fn pop(&mut self, e: u64, next: Option<u64>);
    fn body_mut(&mut self, e: u64);
}

/// 单事件折叠（walk_entries / walk_lines 共用的分派单源——三态分派）。
#[inline]
fn fold_entry<F: EntryFold>(fold: &mut F, e: u64, next: Option<u64>) {
    if entry_is_body_mut(e) {
        fold.body_mut(e)
    } else if entry_is_pop(e) {
        fold.pop(e, next)
    } else {
        fold.push(e)
    }
}

/// 时间轴遍历器——全架构唯一允许存在的 entry 循环。
/// 秩序：body_mut → pop → push（三态分派）。无类型分支（不变量 1）。
#[inline]
pub fn walk_entries<F: EntryFold>(entries: &[u64], fold: &mut F) {
    let mut i = 0;
    while i < entries.len() {
        fold_entry(fold, entries[i], entries.get(i + 1).copied());
        i += 1;
    }
}

/// 带 zone 的时间轴遍历：partition 定位 → 前缀恢复（复用三态，不起产出）
/// → on_zone_begin → 区间内事件折叠。zone = (start_line, end_line)，行号开区间。
/// 契约：fold 须从初始态出发（不变量 2）；start > end 时产出为空、前缀仍恢复
/// （不变量 3）。
/// 段边界 next=None 安全：PopPush 配对要求同行，而段边界是行边界——
/// 跨段配对不可能成立，不损失配对信息。
#[inline]
pub fn walk_zone<F: EntryFold>(entries: &[u64], zone: (u32, u32), fold: &mut F) {
    // 纯观测插桩（feature OFF 展开为 ()）：fold 段——时间轴折叠遍历。
    let _sp_fd_walk = crate::telemetry_span!("fd_walk");
    let start = partition(entries, zone.0);
    walk_entries(&entries[..start], fold); // 前缀恢复：栈状态与前缀重放一致
    fold.on_zone_begin();
    let end = partition(&entries[start..], zone.1) + start;
    walk_entries(&entries[start..end], fold);
}

/// 双轴组合器（空间推进 × 时间折叠）。
/// 每行：先折叠该行全部事件（STEP），再读行值回调（GET）。
/// lines 可传空片——行值非必需时 GET 退化为 Paragraph（深度/链这类
/// 纯结构折叠不读行值）。
/// 契约：per_line 区间内每行恰好一次、行事件之后回调、区间外不回调（不变量 4）。
/// prev 槽是 [prev] 类参数的唯一通道——walker 只传递不更新（不变量 5），
/// 更新规则（BQ Composite / item 开行 / 裸空行）由消费方经 per_line 注入。
#[inline]
pub fn walk_lines<F: EntryFold>(
    entries: &[u64],
    lines: &[PackedLineInfo],
    zone: (u32, u32),
    _prev: &mut PrevSlot,
    fold: &mut F,
    mut per_line: impl FnMut(&mut F, usize, BlockType),
) {
    let zs = zone.0 as usize;
    let ze = zone.1 as usize;
    let mut ei = partition(entries, zone.0);
    walk_entries(&entries[..ei], fold);
    fold.on_zone_begin();
    for li in zs..ze {
        // 游标推进（摊销 O(entries)——每行 partition 二分是 view arrays
        // 2x 的根因；语义不变：行 li 的事件在 per_line 前全部折叠）。
        while ei < entries.len() && entry_line(entries[ei]) as usize <= li {
            fold_entry(fold, entries[ei], entries.get(ei + 1).copied());
            ei += 1;
        }
        let lt = lines.get(li).map(|p| p.node_type()).unwrap_or(BlockType::Paragraph);
        per_line(fold, li, lt);
    }
}

// ── is_poppush：相邻配对（机械性质）──

/// PopPush 相邻配对判定（实现已迁 effect 表层，此处保留薄委托）。
#[inline]
/// PopPush 相邻配对判定（adm_equal=true 身份语义）——纯机械性质：
/// Pop 后紧随同行、同 kind、非 Pop/body_mut 的 Push，且 adm 位相同。
/// adm 掩码来自表（参数注入）；kind 只做相等比较，无枚举列表。
/// 落点纪律：FL 原子谓词语义住 infer；effect 侧为 pub API 薄委托。
#[inline]
pub fn is_poppush(e: u64, next: Option<u64>) -> bool {
    is_poppush_with(e, next, true)
}

/// PopPush 相邻配对判定（adm_equal=false 容器延续语义）——kind-only：
/// LOOSE(out) 差异的 PopPush 仍是容器延续。
#[inline]
pub fn is_poppush_kindonly(e: u64, next: Option<u64>) -> bool {
    is_poppush_with(e, next, false)
}

#[inline]
pub fn is_poppush_with(e: u64, next: Option<u64>, adm_equal: bool) -> bool {
    if !crate::block::entry_is_pop(e) { return false; }
    let Some(n) = next else { return false; };
    if crate::block::entry_line(n) != crate::block::entry_line(e) { return false; }
    if crate::block::entry_is_pop(n) || crate::block::entry_is_body_mut(n) { return false; }
    let f = crate::block::entry_frame(e);
    if crate::block::entry_frame(n).kind() != f.kind() { return false; }
    if !adm_equal {
        return true;
    }
    let mask = crate::effect::decl_for(f.kind()).body.adm_mask;
    (crate::block::entry_frame(n).0 & mask) == (f.0 & mask)
}

/// PopPush 相邻配对判定——切片＋下标形态（FL 原子语义谓词，落点 infer）。
#[inline]
pub fn is_poppush_pair(entries: &[u64], ei: usize, adm_equal: bool) -> bool {
    let e = entries.get(ei).copied();
    let next = entries.get(ei + 1).copied();
    if adm_equal {
        crate::effect::is_poppush(e.unwrap_or(0), next)
    } else {
        crate::effect::is_poppush_kindonly(e.unwrap_or(0), next)
    }
}

/// 同 line pop+push 同 kind 是否算「列表 sibling 替换」（不产生新根）。
/// 只有列表容器有 PopPush 替换身份；BQ/其他 shell 的同行 pop+push
/// 是「关旧开新」（blank 重开 / 边界重开）→ 必须算新根。
/// 根计数 walker 四处调用点的单源判定（root_blocks_count_cf /
/// root_blocks_before / roots_in_range_mode / count_root_blocks_hs）。
#[inline]
pub(crate) fn poppush_is_list_replacement(entries: &[u64], ei: usize) -> bool {
    if ei == 0 {
        return false;
    }
    let e = entries.get(ei).copied().unwrap_or(0);
    let kind = crate::block::entry_frame(e).kind();
    (crate::effect::is_list_container(kind) || kind == FrameKind::TableRow)
        && is_poppush_pair(entries, ei - 1, true)
}

// ── PrevSlot：折叠槽的 prev 半 ──

/// prev 提示——[prev] 类参数的载体。三态语义同 scan 侧：
/// Content（上一行有效类型）/ Blank（上一行是段落关闭的裸空行）/ 回退（读 lines[i-1]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrevHint {
    Default,
    Content(BlockType),
    Blank,
}

/// 折叠槽的 prev 成员。过程状态——永不物化（prev 物化禁则）：
/// 只在双轴遍历的推进中存在；PL+FL 定稿后可从重放中重建。
/// 当前 fold 实现不写入该槽；prev 承诺列在扫描器内部维护
/// （Snapshot::prev_col 为扫描器私有列，见 projection.rs 读点）。
#[derive(Debug, Clone, Copy, Default)]
pub struct PrevSlot(pub Option<PrevHint>);


// §1 fold 库


/// ReplayFold：帧栈重放——forward_replay 的原子形态。
/// 纯机械基准 fold：栈三态的直接累加器，零语义、零类型分支。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReplayFold {
    pub stack: Vec<BlockFrame>,
}

impl EntryFold for ReplayFold {
    fn push(&mut self, e: u64) {
        self.stack.push(entry_frame(e));
    }
    fn pop(&mut self, _e: u64, _next: Option<u64>) {
        self.stack.pop();
    }
    fn body_mut(&mut self, e: u64) {
        let f = entry_frame(e);
        // body_mut 可能指向非栈顶——kind+indent 匹配最近帧。
        if let Some(pos) = self.stack.iter().rposition(|g| {
            g.kind() == f.kind() && g.indent() == f.indent()
        }) {
            self.stack[pos] = BlockFrame((self.stack[pos].0 & !0x003F) | (f.0 & 0x003F));
        }
    }
}

/// 容器记录——一个容器一条（V2：52B 紧凑布局，Vec 列全移入 RecArena）。
/// 结构性 Eq 被显式 structural_eq 取代（arena 化后 derived Eq 范围不等会假阳性）。
#[derive(Debug, Clone)]
pub struct ContainerRec {
    pub frame: BlockFrame,
    /// 首次 Push 行。
    pub start: u32,
    /// 壳关闭行（真实 Pop 结算）；PopPush 配对不结算（容器延续）；zone 尾未弹 → None。
    /// NonZeroU32：end ≥ start+1 > 0（证明链见 cache_compact_v2 §3B）。
    pub end: Option<std::num::NonZeroU32>,
    /// bit0=pure_push bit1=end_includes_close bit2=end_poppush_kindonly。
    pub flags: u8,
    pub anchor: u8,
    /// 父容器（根 = u32::MAX）。
    pub parent: u32,
    /// 创建时的深度级全局序号（跨父容器共享——非父 children 序）。
    pub ci: u32,
    pub children: Range32,
    pub pop_push_events: Range32,
    /// 该容器的全部 Push 行（首次 + PopPush 复用 + Composite 多 Push + 弹层重扫新记录）。
    pub push_lines: Range32,
    /// 该记录的 body_mut 更新行。
    pub body_mut_lines: Range32,
}

impl ContainerRec {
    #[inline]
    pub fn pure_push(&self) -> bool {
        self.flags & FLAG_PURE_PUSH != 0
    }
    #[inline]
    pub fn end_includes_close(&self) -> bool {
        self.flags & FLAG_END_INCLUDES_CLOSE != 0
    }
    #[inline]
    pub fn end_poppush_kindonly(&self) -> bool {
        self.flags & FLAG_END_POPPUSH_KINDONLY != 0
    }
    #[inline]
    pub fn children_of<'a>(&self, arena: &'a RecArena) -> impl Iterator<Item = u32> + 'a {
        arena.children_iter(self.children)
    }
    #[inline]
    pub fn push_lines_of<'a>(&self, arena: &'a RecArena) -> impl Iterator<Item = u32> + 'a {
        arena.push_lines_iter(self.push_lines)
    }
    #[inline]
    pub fn body_mut_lines_of<'a>(&self, arena: &'a RecArena) -> impl Iterator<Item = u32> + 'a {
        arena.body_mut_iter(self.body_mut_lines)
    }
    #[inline]
    pub fn pop_push_events_of<'a>(&self, arena: &'a RecArena) -> PopPushEvents<'a> {
        arena.pp_events_iter(self.pop_push_events)
    }
    /// 窄迭代：只要 (line, is_change) 的场景（如 build_view_arrays 写 row_flags 两位）
    /// **不触碰 pp_kind / pp_rid 两列**——每事件 2 条列流而非 4 条。实测那趟
    /// 91.6ms 是 arrays 里最大单项，主要是列读取带宽。
    #[inline]
    pub fn pop_push_flags_of<'a>(&self, arena: &'a RecArena) -> impl Iterator<Item = (u32, bool)> + 'a {
        arena
            .pp_line_iter(self.pop_push_events)
            .zip(arena.pp_flag_iter(self.pop_push_events))
            .map(|(l, f)| (l, f != 0))
    }
    #[inline]
    pub fn end_usize(&self) -> Option<usize> {
        self.end.map(|e| e.get() as usize)
    }
    #[inline]
    pub fn set_flags(&mut self, pure: bool, incl_close: bool, kindonly: bool) {
        self.flags = 0;
        if pure { self.flags |= FLAG_PURE_PUSH; }
        if incl_close { self.flags |= FLAG_END_INCLUDES_CLOSE; }
        if kindonly { self.flags |= FLAG_END_POPPUSH_KINDONLY; }
    }
}

pub const FLAG_PURE_PUSH: u8 = 1;
pub const FLAG_END_INCLUDES_CLOSE: u8 = 2;
pub const FLAG_END_POPPUSH_KINDONLY: u8 = 4;

/// arena 列区间（SoA CSR 的二分量）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Range32 {
    pub off: u32,
    pub len: u32,
}

/// 池列段：Owned 物理段或 Shared 跨代零拷贝引用段（Δ 虚拟平移）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolSeg {
    /// 独占物理段。
    Owned(Vec<u32>),
    /// 指向 src 逻辑区间 [base, base+len)，读时每个值 + delta。
    Shared {
        src: Arc<PoolCol>,
        base: u32,
        len: u32,
        delta: i32,
        src_gen: u64,
    },
}

impl PoolSeg {
    #[inline]
    fn len(&self) -> u32 {
        match self {
            PoolSeg::Owned(v) => v.len() as u32,
            PoolSeg::Shared { len, .. } => *len,
        }
    }
}

/// 池列：逻辑索引空间由若干段拼成；append 只进最后 Owned 段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PoolCol {
    segs: Vec<PoolSeg>,
    total_len: u32,
}

impl PoolCol {
    #[inline]
    pub fn new() -> Self {
        PoolCol { segs: Vec::new(), total_len: 0 }
    }

    /// U1 批量追加（cache 拼接用）：把 src 列区间 r 逐值平移后并入本列的
    /// **Owned 单段**（读面无 Shared 间接）；preserve_max 保 u32::MAX 哨兵。
    pub(crate) fn extend_range_shifted(
        &mut self,
        src: &PoolCol,
        r: Range32,
        shift: isize,
        preserve_max: bool,
    ) {
        if r.len == 0 {
            return;
        }
        let need_new = self.segs.last().map_or(true, |s| !matches!(s, PoolSeg::Owned(_)));
        if need_new {
            self.segs.push(PoolSeg::Owned(Vec::with_capacity(r.len as usize)));
        }
        let owned = match self.segs.last_mut().unwrap() {
            PoolSeg::Owned(v) => v,
            _ => unreachable!(),
        };
        owned.reserve(r.len as usize);
        if preserve_max {
            for v in src.iter_range(r) {
                owned.push(if v == u32::MAX { v } else { (v as isize + shift).max(0) as u32 });
            }
        } else {
            for v in src.iter_range(r) {
                owned.push((v as isize + shift).max(0) as u32);
            }
        }
        self.total_len += r.len;
    }

    /// 追加一个 Owned 值：若末段非 Owned 则 append 新 Owned 段；否则 push。
    #[inline]
    pub fn push_owned(&mut self, v: u32) {
        match self.segs.last_mut() {
            Some(PoolSeg::Owned(owned)) => owned.push(v),
            _ => self.segs.push(PoolSeg::Owned(vec![v])),
        }
        self.total_len += 1;
    }

    /// 追加一个 Shared 段（跨代零拷贝共享——引用 src 的逻辑区间并 +delta）。
    pub fn share(
        &mut self,
        src: Arc<PoolCol>,
        base: u32,
        len: u32,
        delta: i32,
        src_gen: u64,
    ) {
        self.segs.push(PoolSeg::Shared { src, base, len, delta, src_gen });
        self.total_len += len;
    }

    #[inline]
    pub fn range_len(&self, r: Range32) -> usize {
        r.len as usize
    }

    /// 逻辑区间 [r.off, r.off+r.len) 的逐值迭代（PoolIter 显式栈实现）。
    pub fn iter_range(&self, r: Range32) -> PoolIter<'_> {
        PoolIter::new(self, r.off, r.len)
    }

    /// 两区间长度相等且逐值相等。
    pub fn range_eq(&self, r: Range32, other: &PoolCol, o_r: Range32) -> bool {
        if r.len != o_r.len {
            return false;
        }
        self.iter_range(r).zip(other.iter_range(o_r)).all(|(a, b)| a == b)
    }

    #[inline]
    pub fn total_len(&self) -> u32 {
        self.total_len
    }

    /// 可选诊断：列是否全为 Owned 段。
    #[inline]
    pub fn is_all_owned(&self) -> bool {
        self.segs.iter().all(|s| matches!(s, PoolSeg::Owned(_)))
    }

    /// 暴露段表（供 cache splice 判断）。
    #[inline]
    pub fn segs(&self) -> &[PoolSeg] {
        &self.segs
    }
}

/// 池迭代帧的源池：根帧引用借用；Shared 子帧持有 Arc（跨代共享锚点）。
#[derive(Clone)]
enum PoolSrc<'a> {
    Root(&'a PoolCol),
    Shared(Arc<PoolCol>),
}

impl<'a> PoolSrc<'a> {
    #[inline]
    fn pool(&self) -> &PoolCol {
        match self {
            PoolSrc::Root(p) => p,
            PoolSrc::Shared(a) => a,
        }
    }
}

/// 池迭代帧：源池逻辑区间 [pos, end)（pos 为源池内逻辑位置，delta 为累计平移）。
struct PoolFrame<'a> {
    src: PoolSrc<'a>,
    pos: u32,
    end: u32,
    delta: i32,
    /// 当前段下标（src.segs 中的位置）。
    seg_idx: usize,
    /// 当前段在源池逻辑空间中的起点（随 seg_idx 前进增量维护）。
    seg_start: u32,
}

impl<'a> PoolFrame<'a> {
    /// 构造覆盖某源池逻辑区间 [pos, end) 的帧：定位包含 pos 的段。
    fn new(src: PoolSrc<'a>, pos: u32, end: u32, delta: i32) -> PoolFrame<'a> {
        let segs = src.pool().segs();
        let mut seg_idx = 0usize;
        let mut acc = 0u32;
        while seg_idx < segs.len() {
            if pos - acc < segs[seg_idx].len() {
                break;
            }
            acc += segs[seg_idx].len();
            seg_idx += 1;
        }
        PoolFrame { src, pos, end, delta, seg_idx, seg_start: acc }
    }
}

/// 池列迭代器：显式栈，Shared 段递归进入 src 并累计 delta。
pub struct PoolIter<'a> {
    stack: Vec<PoolFrame<'a>>,
}

impl<'a> PoolIter<'a> {
    fn new(root: &'a PoolCol, off: u32, len: u32) -> PoolIter<'a> {
        let mut stack = Vec::new();
        if len > 0 {
            stack.push(PoolFrame::new(PoolSrc::Root(root), off, off + len, 0));
        }
        PoolIter { stack }
    }
}

impl<'a> Iterator for PoolIter<'a> {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        loop {
            let frame = self.stack.last()?;
            if frame.pos >= frame.end {
                self.stack.pop();
                continue;
            }
            // 快照当前帧状态（避免在改写栈时持借用）。
            let (pos, end, delta, seg_idx, seg_start) =
                (frame.pos, frame.end, frame.delta, frame.seg_idx, frame.seg_start);
            let src = frame.src.clone();
            let pool = src.pool();
            let segs = pool.segs();
            // 前进到包含 pos 的段（跳过已结束段）。
            let mut idx = seg_idx;
            let mut start = seg_start;
            loop {
                if idx >= segs.len() {
                    break;
                }
                let l = segs[idx].len();
                if pos - start < l {
                    break;
                }
                start += l;
                idx += 1;
            }
            if idx >= segs.len() {
                self.stack.pop();
                continue;
            }
            match &segs[idx] {
                PoolSeg::Owned(owned) => {
                    let local = (pos - start) as usize;
                    let val = (owned[local] as i32).wrapping_add(delta);
                    let f = self.stack.last_mut().unwrap();
                    f.pos = pos + 1;
                    return Some(val as u32);
                }
                PoolSeg::Shared { src, base, len, delta: sdelta, .. } => {
                    let seg_end = start + *len;
                    let hi = end.min(seg_end);
                    let src_pos = *base + (pos - start);
                    let src_end = *base + (hi - start);
                    let f = self.stack.last_mut().unwrap();
                    f.pos = hi;
                    f.seg_idx = idx + 1;
                    f.seg_start = start + *len;
                    // 子帧：把 [pos, hi) 的委托范围映射进 src。
                    let child =
                        PoolFrame::new(PoolSrc::Shared(src.clone()), src_pos, src_end, delta + *sdelta);
                    self.stack.push(child);
                    // 回到循环顶部处理子帧。
                }
            }
        }
    }
}

/// 记录列池（叶数据：纯标量列，无传递引用，跨代零拷贝共享）。构建期独占写、
/// 发布后只读；分区不变量：各列区间无空洞/重叠/恰好覆盖。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecArena {
    pub children: PoolCol,
    pub push_lines: PoolCol,
    pub body_mut_lines: PoolCol,
    pub pp_line: PoolCol,
    pub pp_kind: PoolCol,
    pub pp_flag: PoolCol,
    pub pp_rid: PoolCol,
    /// 记录代际（cache 代理在 Segment::build 层覆写）。
    pub gen: u64,
}

impl RecArena {
    #[inline]
    pub fn children_iter(&self, r: Range32) -> impl Iterator<Item = u32> + '_ {
        self.children.iter_range(r)
    }
    #[inline]
    pub fn push_lines_iter(&self, r: Range32) -> impl Iterator<Item = u32> + '_ {
        self.push_lines.iter_range(r)
    }
    #[inline]
    pub fn body_mut_iter(&self, r: Range32) -> impl Iterator<Item = u32> + '_ {
        self.body_mut_lines.iter_range(r)
    }
    #[inline]
    pub fn pp_line_iter(&self, r: Range32) -> impl Iterator<Item = u32> + '_ {
        self.pp_line.iter_range(r)
    }
    #[inline]
    pub fn pp_kind_iter(&self, r: Range32) -> impl Iterator<Item = u32> + '_ {
        self.pp_kind.iter_range(r)
    }
    #[inline]
    pub fn pp_flag_iter(&self, r: Range32) -> impl Iterator<Item = u32> + '_ {
        self.pp_flag.iter_range(r)
    }
    #[inline]
    pub fn pp_rid_iter(&self, r: Range32) -> impl Iterator<Item = u32> + '_ {
        self.pp_rid.iter_range(r)
    }
    /// PopPush 事件视图：由 pp 四列 PoolIter zip 实现。
    #[inline]
    pub fn pp_events_iter(&self, r: Range32) -> PopPushEvents<'_> {
        PopPushEvents {
            lines: self.pp_line.iter_range(r),
            kinds: self.pp_kind.iter_range(r),
            flags: self.pp_flag.iter_range(r),
            rids: self.pp_rid.iter_range(r),
        }
    }
}

/// PopPushEv 的 arena 读视图（逐值等价——SoA 列解包）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopPushEvRef {
    pub line: u32,
    pub is_change: bool,
    pub popped_kind: FrameKind,
    pub popped_rid: u32,
}

pub struct PopPushEvents<'a> {
    lines: PoolIter<'a>,
    kinds: PoolIter<'a>,
    flags: PoolIter<'a>,
    rids: PoolIter<'a>,
}

impl<'a> Iterator for PopPushEvents<'a> {
    type Item = PopPushEvRef;
    fn next(&mut self) -> Option<PopPushEvRef> {
        let line = self.lines.next()?;
        let kind = self.kinds.next()?;
        let flag = self.flags.next()?;
        let rid = self.rids.next()?;
        Some(PopPushEvRef {
            line,
            is_change: flag != 0,
            popped_kind: unsafe { std::mem::transmute::<u8, FrameKind>(kind as u8) },
            popped_rid: rid,
        })
    }
}

/// PopPushEv（语义原型——arena 化前的外部 API 兼容名）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopPushEv {
    pub line: u32,
    /// false = item_boundary（同 kind 同 adm——item 切换——容器延续）；
    /// true  = container_change（异体——弹层重扫 / 配对异体——壳关闭+重开）。
    pub is_change: bool,
    /// Pop 半部弹掉的帧 kind（与 Push 半部同 kind 或跨 kind）。
    pub popped_kind: FrameKind,
    /// Pop 半部弹掉的记录 id（min_push / 守卫消费）。
    pub popped_rid: u32,
}

/// RecordFold：L1 容器记录——build_container_records 的原子形态。
/// 前置条件：zone 起点是 depth-0 锚（栈从空开始，前缀事件
/// 折叠为无操作）。保证方 = span / locate 的 snap_to_depth0（P6）。
/// 参数注入：配对判定读 has_pop_push / adm_mask（表列），结算调 end_for_rule
/// （P1）——机械体内无 kind 枚举列表。
pub struct RecordFold<'a> {
    pub records: Vec<ContainerRec>,
    pub arena: RecArena,
    lines: &'a [PackedLineInfo],
    entries: &'a [u64],
    stack: Vec<u32>,
    ci_at_depth: [u32; MAX_BLOCK_NESTING],
    pending_pop: Option<(FrameKind, u8, u32)>,
    last_li: usize,
    producing: bool,
    /// 构建期临时列（finish 时按 rid 序物化为 RecArena 的连续 run——
    /// 复用记录/交错追加不能直写单 run 分区）。
    tmp_children: Vec<Vec<u32>>,
    tmp_push_lines: Vec<Vec<u32>>,
    tmp_body_mut: Vec<Vec<u32>>,
    tmp_pp: Vec<Vec<PopPushEv>>,
}

/// 行 li 是否存在 Pop entry（局部查询，替代全量 pop_lines 集合）。
#[inline]
pub fn has_pop_at(entries: &[u64], line: u32) -> bool {
    let start = partition(entries, line);
    let end = partition(entries, line + 1);
    entries[start..end].iter().any(|&e| entry_is_pop(e))
}

impl<'a> RecordFold<'a> {
    pub fn new(entries: &'a [u64], lines: &'a [PackedLineInfo]) -> Self {
        // 纯观测插桩：fold 段装配。
        let _sp_fd_new = crate::telemetry_span!("fd_new");
        RecordFold {
            records: Vec::new(),
            arena: RecArena::default(),
            lines,
            entries,
            stack: Vec::new(),
            ci_at_depth: [0; MAX_BLOCK_NESTING],
            pending_pop: None,
            last_li: usize::MAX,
            producing: false,
            tmp_children: Vec::new(),
            tmp_push_lines: Vec::new(),
            tmp_body_mut: Vec::new(),
            tmp_pp: Vec::new(),
        }
    }

    fn touch_line(&mut self, li: usize) {
        if li != self.last_li {
            self.pending_pop = None;
            self.last_li = li;
        }
    }

    fn create(&mut self, frame: BlockFrame, li: u32, anchor: u8) -> u32 {
        let parent = self.stack.last().copied().unwrap_or(u32::MAX);
        let d = self.stack.len();
        let ci = self.ci_at_depth[d];
        self.ci_at_depth[d] += 1;
        let rid = self.records.len() as u32;
        let rec = ContainerRec {
            frame,
            start: li,
            end: None,
            flags: 0,
            anchor,
            parent,
            ci,
            children: Range32::default(),
            pop_push_events: Range32::default(),
            push_lines: Range32::default(),
            body_mut_lines: Range32::default(),
        };
        self.records.push(rec);
        self.tmp_children.push(Vec::new());
        self.tmp_push_lines.push(vec![li]);
        self.tmp_body_mut.push(Vec::new());
        self.tmp_pp.push(Vec::new());
        if parent != u32::MAX {
            self.tmp_children[parent as usize].push(rid);
        }
        rid
    }

    /// 结算 pure_push（回填 flags）并按 rid 序物化 arena（临时列 → 连续 run）。
    pub fn finish(mut self) -> (Vec<ContainerRec>, RecArena) {
        // 纯观测插桩：fold 段结算（flags 回填 + arena 物化）。
        let _sp_fd_finish = crate::telemetry_span!("fd_finish");
        let entries = self.entries;
        let mut arena = RecArena::default();
        for (i, r) in self.records.iter_mut().enumerate() {
            let pure = !has_pop_at(entries, r.start);
            r.flags = if pure { r.flags | FLAG_PURE_PUSH } else { r.flags };

            r.children = Range32 { off: arena.children.total_len(), len: 0 };
            for &v in &self.tmp_children[i] {
                arena.children.push_owned(v);
                r.children.len += 1;
            }
            r.push_lines = Range32 { off: arena.push_lines.total_len(), len: 0 };
            for &v in &self.tmp_push_lines[i] {
                arena.push_lines.push_owned(v);
                r.push_lines.len += 1;
            }
            r.body_mut_lines = Range32 { off: arena.body_mut_lines.total_len(), len: 0 };
            for &v in &self.tmp_body_mut[i] {
                arena.body_mut_lines.push_owned(v);
                r.body_mut_lines.len += 1;
            }
            r.pop_push_events = Range32 { off: arena.pp_line.total_len(), len: 0 };
            for ev in &self.tmp_pp[i] {
                arena.pp_line.push_owned(ev.line);
                arena.pp_kind.push_owned(ev.popped_kind as u8 as u32);
                arena.pp_flag.push_owned(ev.is_change as u8 as u32);
                arena.pp_rid.push_owned(ev.popped_rid);
                r.pop_push_events.len += 1;
            }
        }
        (self.records, arena)
    }
}

/// 通用 zone 局部折叠：不重放 prefix entries（契约与 `walk_zone` 不同——
/// 要求 zone 起点 depth-0 / 调用方自备栈上下文）。`record_fold_walk_zone`
/// 与 Step 8 前缀计数守卫共用此原子。
pub fn walk_zone_no_prefix<F: EntryFold>(entries: &[u64], zone: (u32, u32), fold: &mut F) {
    let start = partition(entries, zone.0);
    let end = partition(&entries[start..], zone.1) + start;
    fold.on_zone_begin();
    walk_entries(&entries[start..end], fold);
}

/// RecordFold 专用 zone 遍历：zone 起点为 depth-0，前缀事件不产生记录，
/// 因此不做 prefix 物理遍历，直接进入 producing 阶段。
pub fn record_fold_walk_zone(entries: &[u64], zone: (u32, u32), fold: &mut RecordFold<'_>) {
    walk_zone_no_prefix(entries, zone, fold);
}

impl<'a> EntryFold for RecordFold<'a> {
    fn on_zone_begin(&mut self) {
        self.producing = true;
    }

    fn push(&mut self, e: u64) {
        if !self.producing { return; }
        let li = entry_line(e) as usize;
        self.touch_line(li);
        let frame = entry_frame(e);
        let anchor = entry_anchor(e);
        let rid = if let Some((pk, pb, prev_rid)) = self.pending_pop.take() {
            let decl = crate::effect::decl_for(pk);
            let adm_eq = pb == ((frame.0 & decl.body.adm_mask) as u8);
            if pk == frame.kind() && decl.has_pop_push && adm_eq {
                // item_boundary：复用被弹记录（容器身份延续）。
                // 保留 LOOSE 位（out 位——空行后置 loose 的列表被复用时不丢）。
                let r = &mut self.records[prev_rid as usize];
                r.frame = BlockFrame((r.frame.0 & LOOSE_BIT) | (frame.0 & !LOOSE_BIT));
                self.tmp_push_lines[prev_rid as usize].push(li as u32);
                self.tmp_pp[prev_rid as usize].push(PopPushEv {
                    line: li as u32,
                    is_change: false,
                    popped_kind: pk,
                    popped_rid: prev_rid,
                });
                prev_rid
            } else {
                // container_change：新记录（壳关闭+重开）。
                let rid = self.create(frame, li as u32, anchor);
                self.tmp_pp[rid as usize].push(PopPushEv {
                    line: li as u32,
                    is_change: true,
                    popped_kind: pk,
                    popped_rid: prev_rid,
                });
                rid
            }
        } else {
            self.create(frame, li as u32, anchor)
        };
        self.stack.push(rid);
    }

    fn pop(&mut self, e: u64, next: Option<u64>) {
        if !self.producing { return; }
        let li = entry_line(e) as usize;
        self.touch_line(li);
        let frame = entry_frame(e);
        if let Some(rid) = self.stack.pop() {
            let pop_pl = self.lines.get(li)
                .map(|p| p.node_type())
                .unwrap_or(BlockType::Paragraph);
            let end = end_for_rule(frame.kind(), li, pop_pl);
            let is_pp = is_poppush(e, next);
            // 参数注入：is_list_pop = has_pop_push 表列（消 kind 枚举列表）。
            let is_list_pop = crate::effect::decl_for(frame.kind()).has_pop_push;
            let adm = (frame.0 & crate::effect::decl_for(frame.kind()).body.adm_mask) as u8;
            if is_pp && is_list_pop {
                // 容器延续：end 不结算，Push 半部复用（待配对）。
                self.pending_pop = Some((frame.kind(), adm, rid));
            } else {
                // end_includes_close ⟺ end == li + 1（end_for_rule 结果直接蕴含——
                // 消 closing_pl 双求值复制）。
                let includes_close = end > li;
                let r = &mut self.records[rid as usize];
                r.end = std::num::NonZeroU32::new(end as u32);
                debug_assert!(
                    r.end.is_some(),
                    "NonZeroU32 end invariant violated @{li}"
                );
                // B2-5：关闭 Pop 紧跟同 kind Push（kindonly）→ shell 容器
                // 闭包链需跨记录跟随（container_log_end 跳过该 PopPush）。
                r.set_flags(false, includes_close, is_poppush_kindonly(e, next));
                self.pending_pop = Some((frame.kind(), adm, rid));
            }
        }
    }

    fn body_mut(&mut self, e: u64) {
        if !self.producing { return; }
        let li = entry_line(e) as usize;
        self.touch_line(li);
        let frame = entry_frame(e);
        // body_mut 可能指向非栈顶——kind+indent 匹配最近帧。
        if let Some(pos) = self.stack.iter().rposition(|&rid| {
            let r = &self.records[rid as usize];
            r.frame.kind() == frame.kind() && r.frame.indent() == frame.indent()
        }) {
            let rid = self.stack[pos];
            let r = &mut self.records[rid as usize];
            r.frame = BlockFrame((r.frame.0 & !0x003F) | (frame.0 & 0x003F));
            self.tmp_body_mut[rid as usize].push(li as u32);
        }
    }
}

/// 行级物化——line 面的 8 列（对照 build_view_arrays 逐列等价）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewArrays {
    /// 行 → 该行 Push 的记录 id（摊平 CSR：offsets[l]..offsets[l+1]，同行为 Push 序）。
    /// Arc 拥有：clone 变浅（§6 承诺）——LineView 按值持有、渲染/incr 每查询
    /// clone 一次，裸 Vec 会整列 memcpy。
    pub push_offsets: Arc<[u32]>,
    pub push_values: Arc<[u32]>,
    /// start 行 → 记录 id 列表（摊平 CSR）。
    pub start_offsets: Arc<[u32]>,
    pub start_values: Arc<[u32]>,
    /// body_mut 行 → kind 列表（摊平 CSR）。
    pub body_mut_offsets: Arc<[u32]>,
    pub body_mut_values: Arc<[FrameKind]>,
    /// 行级位标志（V2 三列合一）：bit0=container_boundary，bit1=container_change，
    /// bit2=loose。只读经 boundary_at/change_at/loose_at。
    pub row_flags: Arc<[u8]>,
    /// 行级链池（CSR 两列）：chain_offsets len = 行数 + 1；
    /// 行 rel 的链 = &chain_data[chain_offsets[rel]..chain_offsets[rel + 1]]。
    pub chain_offsets: Arc<[u32]>,
    pub chain_data: Arc<[u32]>,
}

/// row_flags bit0：container_boundary（item_boundary PopPush 行）。
pub(crate) const ROW_FLAG_BOUNDARY: u8 = 1;
/// row_flags bit1：container_change（container_change PopPush 行）。
pub(crate) const ROW_FLAG_CHANGE: u8 = 2;
/// row_flags bit2：loose（传播区间 → 行数组）。
pub(crate) const ROW_FLAG_LOOSE: u8 = 4;

impl ViewArrays {
    /// V1 C4:路由包装段(prefix/suffix 平移)的占位视图——消费方
    /// (selector 路由)只读 records/blocks,不读行面列。
    pub(crate) fn empty() -> Self {
        ViewArrays {
            push_offsets: Arc::from(Vec::<u32>::new()),
            push_values: Arc::from(Vec::<u32>::new()),
            start_offsets: Arc::from(Vec::<u32>::new()),
            start_values: Arc::from(Vec::<u32>::new()),
            body_mut_offsets: Arc::from(Vec::<u32>::new()),
            body_mut_values: Arc::from(Vec::<FrameKind>::new()),
            row_flags: Arc::from(Vec::<u8>::new()),
            chain_offsets: Arc::from(vec![0u32]),
            chain_data: Arc::from(Vec::<u32>::new()),
        }
    }

    #[inline]
    pub(crate) fn push_row(&self, rel: usize) -> &[u32] {
        let lo = self.push_offsets[rel] as usize;
        let hi = self.push_offsets[rel + 1] as usize;
        &self.push_values[lo..hi]
    }
    #[inline]
    pub(crate) fn start_row(&self, rel: usize) -> &[u32] {
        let lo = self.start_offsets[rel] as usize;
        let hi = self.start_offsets[rel + 1] as usize;
        &self.start_values[lo..hi]
    }
    #[inline]
    pub(crate) fn body_mut_row(&self, rel: usize) -> &[FrameKind] {
        let lo = self.body_mut_offsets[rel] as usize;
        let hi = self.body_mut_offsets[rel + 1] as usize;
        &self.body_mut_values[lo..hi]
    }
    /// 行 rel 是否 container_boundary（row_flags bit0）。
    #[inline]
    pub(crate) fn boundary_at(&self, rel: usize) -> bool {
        self.row_flags.get(rel).map_or(false, |&f| f & ROW_FLAG_BOUNDARY != 0)
    }
    /// 行 rel 是否 container_change（row_flags bit1）。
    #[inline]
    pub(crate) fn change_at(&self, rel: usize) -> bool {
        self.row_flags.get(rel).map_or(false, |&f| f & ROW_FLAG_CHANGE != 0)
    }
    /// 行 rel 是否 loose（row_flags bit2）。
    #[inline]
    pub(crate) fn loose_at(&self, rel: usize) -> bool {
        self.row_flags.get(rel).map_or(false, |&f| f & ROW_FLAG_LOOSE != 0)
    }
    /// 行 rel 的链快照（去重后链池切片——行级深度 = len）。
    #[inline]
    pub(crate) fn chain_row(&self, rel: usize) -> &[u32] {
        let lo = self.chain_offsets[rel] as usize;
        let hi = self.chain_offsets[rel + 1] as usize;
        &self.chain_data[lo..hi]
    }
}

// 链池布局（逐行 CSR）——**由 `DepthChainFold` 在重放时直接追加产出**：
// 既省 3M 个小 Arc，也省二次摊平遍历（实测该遍历 ~39ms ＋ Arc 层 ~105ms）。
//
// 布局：每行链在 `flat` 里占一个连续 run，`offs[rel]..offs[rel+1]` 即该行链。
// 空链占 0 长度 run（offs[rel] == offs[rel+1]）。这正是
// `chain_row(rel) = &chain_data[chain_offsets[rel]..chain_offsets[rel+1]]` 能
// 精确返回每行链的唯一布局——共享 run 无法用该切片公式表达（深度可 1→2→1
// 回退，同值链可非相邻重现）。

/// loose 传播区间（对照 loose_intervals 逐值等价）。
/// 纯 Push 列表过滤 {BLT, OL} 查表 has_loose（集合相同——
/// 消 kind 枚举列表，参数注入律）。
pub fn loose_intervals(
    records: &[ContainerRec],
    arena: &RecArena,
    ze: usize,
) -> Vec<(usize, usize)> {
    /// 递归收集（**缓冲复用版**）：`set` 栈式父读子集拼接，`cur`/`next` 减法双缓冲；
    /// 省掉每 record 一个 Vec 的分配/拷贝（实测 `loose_intervals` 主要成本在此）。
    /// `set`（栈式：父读 `[mark..]` 即子集合拼接）、`cur`/`next`（减法双缓冲）。
    struct Ctx<'a> {
        records: &'a [ContainerRec],
        arena: &'a RecArena,
        ze: usize,
        out: Vec<(usize, usize)>,
        set: Vec<(usize, usize)>,
        kids: Vec<u32>,
        cur: Vec<(usize, usize)>,
        next: Vec<(usize, usize)>,
    }
    impl Ctx<'_> {
        fn collect(&mut self, rid: u32) {
            let mark = self.set.len();
            let kmark = self.kids.len();
            {
                let r = &self.records[rid as usize];
                for c in r.children_of(self.arena) {
                    self.kids.push(c);
                }
            }
            for i in kmark..self.kids.len() {
                let c = self.kids[i];
                self.collect(c);
            }
            self.kids.truncate(kmark);
            let (start, is_loose, end) = {
                let r = &self.records[rid as usize];
                (r.start as usize, r.frame.loose(), r.end_usize().unwrap_or(self.ze))
            };
            // 传播（自身 loose）——[start, end) 减子树纯 Push 列表并集（set——不含自身）。
            if is_loose {
                self.cur.clear();
                self.cur.push((start, end));
                for &(bs, be) in &self.set[mark..] {
                    // 同 start 的 Composite 同行 Push 不阻断（严格大于）。
                    if bs <= start {
                        continue;
                    }
                    self.next.clear();
                    for &(s, e) in &self.cur {
                        if be <= s || bs >= e {
                            self.next.push((s, e));
                        } else {
                            if s < bs {
                                self.next.push((s, bs));
                            }
                            if be < e {
                                self.next.push((be, e));
                            }
                        }
                    }
                    std::mem::swap(&mut self.cur, &mut self.next);
                }
                self.out.extend(self.cur.iter().copied());
            }
            // 自身纯 Push 列表 → 加入集合（供父的传播减法）。
            if {
                let r = &self.records[rid as usize];
                r.pure_push() && crate::effect::decl_for(r.frame.kind()).has_loose
                    && r.end_usize().unwrap_or(self.ze) > r.start as usize
            } {
                self.set.push((start, end));
            }
        }
    }
    let mut ctx = Ctx {
        records,
        arena,
        ze,
        out: Vec::new(),
        set: Vec::new(),
        kids: Vec::new(),
        cur: Vec::new(),
        next: Vec::new(),
    };
    for (rid, r) in records.iter().enumerate() {
        if r.parent == u32::MAX {
            ctx.collect(rid as u32);
            // 每个 root 的返回集合直接丢弃 ⇒ 等价地清空栈式缓冲。
            ctx.set.clear();
            ctx.kids.clear();
        }
    }
    let mut out = ctx.out;
    out.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in out {
        if let Some(last) = merged.last_mut() {
            if s <= last.1 {
                if e > last.1 {
                    last.1 = e;
                }
                continue;
            }
        }
        merged.push((s, e));
    }
    merged
}

/// DepthChainFold：depth / chain 两列的重放折叠（build_view_arrays 的重放半部）。
/// push 的 rid 经 push_map 行内计数取（记录序 == 事件序——同行为 Push 序）。
/// 前置条件同 RecordFold：zone 起点是 depth-0 锚（前缀事件折叠为无操作）。
pub struct DepthChainFold<'a> {
    push_offsets: &'a [u32],
    push_values: &'a [u32],
    zone_start: u32,
    dstack: Vec<u32>,
    last_rel: usize,
    line_count: usize,
    producing: bool,
    /// 链池的平坦两列（CSR）：`offs[i]` = 行 i 链在 `flat` 中的起点（长度 zlen+1）。
    /// 重放时直接追加——省掉 3M 次 Arc 分配/引用计数与一次全量遍历（实测该遍历 ~40ms）。
    pub flat: Vec<u32>,
    pub offs: Vec<u32>,
}

impl<'a> DepthChainFold<'a> {
    pub fn new(push_offsets: &'a [u32], push_values: &'a [u32], zone_start: u32, zlen: usize) -> Self {
        DepthChainFold {
            push_offsets,
            push_values,
            zone_start,
            dstack: Vec::new(),
            last_rel: usize::MAX,
            line_count: 0,
            producing: false,
            flat: Vec::new(),
            offs: Vec::with_capacity(zlen + 1),
        }
    }

    /// 行级快照（每行事件折叠后调用，**按行序、每行恰好一次**——walk_lines 不变量 4）：
    /// 当前链直接追加进平坦池（`offs` 记起点）。深度不另存列：depth == 链长。
    fn snapshot(&mut self, rel: usize) {
        debug_assert_eq!(rel, self.offs.len(), "snapshot 必须按行序调用（walk_lines 不变量 4）");
        self.offs.push(self.flat.len() as u32);
        if !self.dstack.is_empty() {
            self.flat.extend_from_slice(&self.dstack);
        }
    }
}

impl<'a> EntryFold for DepthChainFold<'a> {
    fn on_zone_begin(&mut self) {
        self.producing = true;
    }
    fn push(&mut self, e: u64) {
        if !self.producing { return; }
        let rel = entry_line(e) as usize - self.zone_start as usize;
        if rel != self.last_rel {
            self.last_rel = rel;
            self.line_count = 0;
        }
        let lo = self.push_offsets[rel] as usize;
        let rid = self.push_values[lo + self.line_count];
        self.line_count += 1;
        self.dstack.push(rid);
    }
    fn pop(&mut self, _e: u64, _next: Option<u64>) {
        if !self.producing { return; }
        self.dstack.pop();
    }
    fn body_mut(&mut self, _e: u64) {}
}

/// line 面 8 列构建（对照 build_view_arrays 逐列等价）。
/// 两半部：records 投影（push_map/body_mut/boundary/change/start + loose）
/// 与 entries 重放（depth/chain——DepthChainFold）。
pub fn build_view_arrays(
    records: &[ContainerRec],
    arena: &RecArena,
    entries: &[u64],
    zone: (u32, u32),
) -> ViewArrays {
    // 纯观测插桩：view 段——派生列数组构建（CSR 两遍 + entries 重放）。
    let _sp_vw_arrays = crate::telemetry_span!("vw_arrays");
    let (zs, ze) = (zone.0 as usize, zone.1 as usize);
    let zlen = ze - zs;

    // ── CSR 两遍构建（P3-A：替代 zlen 个 Vec 的 per-line 分配）──
    // 第一遍计数 → 前缀和 offsets；第二遍按游标填充 values。
    // 纯观测插桩（每调用级，成本可忽略）：把 508ms 拆成五半部，供优化定位。
    let mut push_counts = vec![0u32; zlen];
    let mut start_counts = vec![0u32; zlen];
    let mut body_counts = vec![0u32; zlen];
    {
        let _sp = crate::telemetry_span!("vw_csr_count");
        for r in records {
            for l in r.push_lines_of(arena) {
                let l = l as usize;
                if l >= zs && l < ze {
                    push_counts[l - zs] += 1;
                }
            }
            for l in r.body_mut_lines_of(arena) {
                let l = l as usize;
                if l >= zs && l < ze {
                    body_counts[l - zs] += 1;
                }
            }
            let s = r.start as usize;
            if s >= zs && s < ze {
                start_counts[s - zs] += 1;
            }
        }
    }
    let mut push_offsets = vec![0u32; zlen + 1];
    let mut start_offsets = vec![0u32; zlen + 1];
    let mut body_mut_offsets = vec![0u32; zlen + 1];
    {
        let _sp = crate::telemetry_span!("vw_prefix");
        for l in 0..zlen {
            push_offsets[l + 1] = push_offsets[l] + push_counts[l];
            start_offsets[l + 1] = start_offsets[l] + start_counts[l];
            body_mut_offsets[l + 1] = body_mut_offsets[l] + body_counts[l];
        }
    }
    let mut push_values = vec![0u32; push_offsets[zlen] as usize];
    let mut start_values = vec![0u32; start_offsets[zlen] as usize];
    let mut body_mut_values = vec![FrameKind::BlockQuote; body_mut_offsets[zlen] as usize];
    // counts 在 prefix 之后不再需要——就地转为游标（省 3 次 zlen+1 clone）。
    for l in 0..zlen {
        push_counts[l] = push_offsets[l];
        start_counts[l] = start_offsets[l];
        body_counts[l] = body_mut_offsets[l];
    }
    {
        let _sp = crate::telemetry_span!("vw_csr_fill");
    for (rid, r) in records.iter().enumerate() {
        for l in r.push_lines_of(arena) {
            let l = l as usize;
            if l >= zs && l < ze {
                let rel = l - zs;
                push_values[push_counts[rel] as usize] = rid as u32;
                push_counts[rel] += 1;
            }
        }
        for l in r.body_mut_lines_of(arena) {
            let l = l as usize;
            if l >= zs && l < ze {
                let rel = l - zs;
                body_mut_values[body_counts[rel] as usize] = r.frame.kind();
                body_counts[rel] += 1;
            }
        }
        let s = r.start as usize;
        if s >= zs && s < ze {
            let rel = s - zs;
            start_values[start_counts[rel] as usize] = rid as u32;
            start_counts[rel] += 1;
        }
    }
    }

    let mut row_flags = vec![0u8; zlen];
    {
        let _sp = crate::telemetry_span!("vw_pp_loose");
    {
        let _sp_pp = crate::telemetry_span!("vw_pp_bits");
        for r in records {
            for (l, is_change) in r.pop_push_flags_of(arena) {
                let l = l as usize;
                if l >= zs && l < ze {
                    if is_change {
                        row_flags[l - zs] |= ROW_FLAG_CHANGE;
                    } else {
                        row_flags[l - zs] |= ROW_FLAG_BOUNDARY;
                    }
                }
            }
        }
    }
    let loose_int;
    {
        let _sp_int = crate::telemetry_span!("vw_loose_int");
        loose_int = loose_intervals(records, arena, ze);
    }
    {
        let _sp_lf = crate::telemetry_span!("vw_loose_fill");
    for &(s, e) in &loose_int {
        let s = s.saturating_sub(zs);
        let e = e.saturating_sub(zs).min(zlen);
        if s < e {
            for b in row_flags[s..e].iter_mut() {
                *b |= ROW_FLAG_LOOSE;
            }
        }
    }
    }
    }
    let (chain_offsets, chain_data) = {
        let _sp = crate::telemetry_span!("vw_chain");
        let mut fold = DepthChainFold::new(&push_offsets, &push_values, zone.0, zlen);
        let mut prev = PrevSlot::default();
        {
            let _sp_r = crate::telemetry_span!("vw_chain_replay");
            walk_lines(entries, &[], zone, &mut prev, &mut fold, |f, li, _t| {
                f.snapshot(li - zs);
            });
        }
        // 池化已在重放中完成：补一个收尾 offset（长度 = 行数 + 1，语义同旧
        // build_chain_pool 的尾项），不再二次遍历。
        let DepthChainFold { mut offs, flat, .. } = fold;
        offs.push(flat.len() as u32);
        (offs, flat)
    };
    ViewArrays {
        push_offsets: Arc::from(push_offsets),
        push_values: Arc::from(push_values),
        start_offsets: Arc::from(start_offsets),
        start_values: Arc::from(start_values),
        body_mut_offsets: Arc::from(body_mut_offsets),
        body_mut_values: Arc::from(body_mut_values),
        row_flags: Arc::from(row_flags),
        chain_offsets: Arc::from(chain_offsets),
        chain_data: Arc::from(chain_data),
    }
}

/// next_pop_end 的平铺形态：从 ei 前扫，找 target 记录的第一个 Pop，
/// 结算其块尾（end_for_rule）。push CSR 行内计数取 rid。
fn next_pop_end(
    entries: &[u64],
    lines: &[PackedLineInfo],
    push_offsets: &[u32],
    push_values: &[u32],
    zs: usize,
    ze: usize,
    stack: &[u32],
    target: u32,
    ei: usize,
) -> Option<usize> {
    let mut sim_stack = stack.to_vec();
    let mut sim_ei = ei;
    let mut last_rel: Option<usize> = None;
    let mut line_count = 0usize;
    while sim_ei < entries.len() {
        let e = entries[sim_ei];
        let l = entry_line(e) as usize;
        if l >= ze {
            break;
        }
        if entry_is_body_mut(e) {
            sim_ei += 1;
            continue;
        }
        if entry_is_pop(e) {
            if sim_stack.pop() == Some(target) {
                let frame = entry_frame(e);
                let pop_pl = lines.get(l).map(|p| p.node_type())
                    .unwrap_or(BlockType::Paragraph);
                return Some(end_for_rule(frame.kind(), l, pop_pl));
            }
            sim_ei += 1;
            continue;
        }
        if l < zs {
            sim_ei += 1;
            continue; // 段外（防御——不应发生）
        }
        let rel = l - zs;
        if Some(rel) != last_rel {
            last_rel = Some(rel);
            line_count = 0;
        }
        let lo = push_offsets[rel] as usize;
        sim_stack.push(push_values[lo + line_count]);
        line_count += 1;
        sim_ei += 1;
    }
    Some(ze)
}

/// LineView：line 查询面——8 列 + 块记录之上的完整行级查询。
/// 数组支撑方法（P3）+ 块支撑方法（P5）——对照 RowView 逐值等价。
/// 查询全部 zone 守卫（越界回退值固定）。
pub struct LineView<'a> {
    pub records: &'a [ContainerRec],
    pub blocks: &'a [BlockRec],
    pub arena: &'a RecArena,
    entries: &'a [u64],
    lines: &'a [PackedLineInfo],
    rope: &'a crop::Rope,
    pub view: ViewArrays,
    zone_start: usize,
    zone_end: usize,
}

impl<'a> LineView<'a> {
    pub fn new(
        records: &'a [ContainerRec],
        blocks: &'a [BlockRec],
        entries: &'a [u64],
        lines: &'a [PackedLineInfo],
        rope: &'a crop::Rope,
        zone: (u32, u32),
        arena: &'a RecArena,
    ) -> Self {
        // 纯观测插桩：view 段外层。
        let _sp_vw_total = crate::telemetry_span!("vw_total");
        let view = build_view_arrays(records, arena, entries, zone);
        Self::with_arrays(records, blocks, entries, lines, rope, zone, view, arena)
    }

    /// 注入版（P10 ③——物化一次、注入消费——与 build_block_records_with
    /// 共用同一份 view arrays）。
    pub fn with_arrays(
        records: &'a [ContainerRec],
        blocks: &'a [BlockRec],
        entries: &'a [u64],
        lines: &'a [PackedLineInfo],
        rope: &'a crop::Rope,
        zone: (u32, u32),
        view: ViewArrays,
        arena: &'a RecArena,
    ) -> Self {
        // 纯观测插桩：view 段装配。
        let _sp_vw_with = crate::telemetry_span!("vw_with");
        LineView {
            records,
            blocks,
            arena,
            entries,
            lines,
            rope,
            view,
            zone_start: zone.0 as usize,
            zone_end: zone.1 as usize,
        }
    }

    /// 含行块（locate 区间模式的视图封装）。
    #[inline]
    pub fn block_at(&self, line: usize) -> Option<&'a BlockRec> {
        locate_block(self.blocks, line).map(|i| &self.blocks[i])
    }

    #[inline]
    fn rel(&self, line: usize) -> Option<usize> {
        if line < self.zone_start || line >= self.zone_end {
            None
        } else {
            Some(line - self.zone_start)
        }
    }

    pub fn zone_end(&self) -> usize {
        self.zone_end
    }

    pub fn close_depth_at(&self, line: usize) -> usize {
        self.rel(line)
            .map(|r| self.view.chain_row(r).len())
            .unwrap_or(0)
    }

    pub fn node_type_at_depth(&self, line: usize, depth_idx: usize) -> BlockType {
        self.rel(line)
            .and_then(|r| self.view.chain_row(r).get(depth_idx))
            .map(|&rid| crate::effect::container_block_type(self.records[rid as usize].frame.kind()))
            .unwrap_or(BlockType::Paragraph)
    }

    pub fn frame_kind_at_depth(&self, line: usize, depth_idx: usize) -> Option<FrameKind> {
        self.rel(line)
            .and_then(|r| self.view.chain_row(r).get(depth_idx))
            .map(|&rid| self.records[rid as usize].frame.kind())
    }

    pub fn frame_at_depth(&self, line: usize, depth_idx: usize) -> Option<BlockFrame> {
        self.rel(line)
            .and_then(|r| self.view.chain_row(r).get(depth_idx))
            .map(|&rid| self.records[rid as usize].frame)
    }

    pub fn chain_at(&self, line: usize) -> Vec<u32> {
        self.rel(line)
            .map(|r| self.view.chain_row(r).to_vec())
            .unwrap_or_default()
    }

    pub fn is_container_boundary(&self, line: usize) -> bool {
        self.rel(line).map(|r| self.view.boundary_at(r)).unwrap_or(false)
    }

    pub fn is_container_change(&self, line: usize) -> bool {
        self.rel(line).map(|r| self.view.change_at(r)).unwrap_or(false)
    }

    pub fn is_loose_at(&self, line: usize) -> bool {
        self.rel(line).map(|r| self.view.loose_at(r)).unwrap_or(false)
    }

    /// 链上 any 确认 Table 帧（body bit0——body_mut 更新）。
    pub fn table_confirmed_at(&self, line: usize) -> bool {
        self.rel(line)
            .map(|r| {
                self.view.chain_row(r).iter().any(|&rid| {
                    let rec = &self.records[rid as usize];
                    rec.frame.kind() == FrameKind::Table && rec.frame.body() & 1 != 0
                })
            })
            .unwrap_or(false)
    }

    pub fn child_idx_at(&self, line: usize, depth_idx: usize) -> usize {
        self.rel(line)
            .and_then(|r| self.view.chain_row(r).get(depth_idx))
            .map(|&rid| self.records[rid as usize].ci as usize)
            .unwrap_or(0)
    }

    /// 该行 start 记录的 end 最大值（None → ze 兜底）。
    pub fn container_end_for(&self, line: usize) -> Option<usize> {
        let r = self.rel(line)?;
        let ze = self.zone_end;
        self.view.start_row(r)
            .iter()
            .map(|&rid| self.records[rid as usize].end_usize().unwrap_or(ze))
            .max()
    }

    /// 该行 start 帧的 end（is_leaf 守卫 + start==line + 影子栈前瞻）。
    pub fn end_for(&self, line: usize) -> Option<usize> {
        let r = self.rel(line)?;
        // is_leaf 守卫：该行 push/body_mut 含行 PL 的帧 kind 才非 leaf。
        if let Some(k) = crate::block::frame_kind(self.lines[line].node_type()) {
            let pushed = self.view.push_row(r).iter()
                .any(|&rid| self.records[rid as usize].frame.kind() == k)
                || self.view.body_mut_row(r).contains(&k);
            if !pushed {
                return None;
            }
        }
        let chain = self.view.chain_row(r);
        let target = *chain.last()?;
        if self.records[target as usize].start as usize != line {
            return None;
        }
        let ei = partition(self.entries, line as u32 + 1); // entry_line <= line 之后
        next_pop_end(
            self.entries,
            self.lines,
            &self.view.push_offsets,
            &self.view.push_values,
            self.zone_start,
            self.zone_end,
            chain,
            target,
            ei,
        )
    }

    pub fn advance(&self, t: BlockType, line: usize) -> Option<usize> {
        if crate::block::frame_kind(t).is_some() {
            self.end_for(line)
        } else {
            None
        }
    }

    /// 行 ∈ 任一记录 push_lines（push CSR 非空 ⟺ 该行有 Push）。
    pub fn is_block_start(&self, line: usize) -> bool {
        self.rel(line).map(|r| !self.view.push_row(r).is_empty()).unwrap_or(false)
    }

    /// 行 PL 是容器类型且该行无该 kind 的 push/body_mut（is_leaf 守卫）。
    pub fn is_leaf(&self, line: usize) -> bool {
        let Some(r) = self.rel(line) else { return false; };
        let nt = self.lines[line].node_type();
        crate::block::frame_kind(nt).map_or(false, |k| {
            !(self.view.push_row(r).iter()
                .any(|&rid| self.records[rid as usize].frame.kind() == k)
                || self.view.body_mut_row(r).contains(&k))
        })
    }

    /// 有效 rt：块起点行 = block_type；隐式段内行 = 行级有效 rt；无块行 = 行级。
    pub fn resolved_type_at(&self, line: usize) -> BlockType {
        if self.rel(line).is_none() {
            return BlockType::Paragraph;
        }
        if let Some(b) = self.block_at(line) {
            if line == b.start as usize {
                return b.block_type;
            }
            if b.implicit() {
                let pl = self.lines[line].node_type();
                if pl == BlockType::Blockquote {
                    return with_line_str(self.rope.line(line), |s| {
                        crate::block::classify_bq_content(s, b.block_type == BlockType::Paragraph)
                    });
                }
                if pl == BlockType::Blank || pl == BlockType::HsSep {
                    return BlockType::Blank;
                }
                return pl;
            }
        }
        let pl = self.lines[line].node_type();
        if pl == BlockType::Blockquote {
            return with_line_str(self.rope.line(line), |s| {
                crate::block::classify_bq_content(s, false)
            });
        }
        if pl == BlockType::Paragraph
            && !effective_para_at(self.lines, self.rope, line, self.zone_end, None, None)
        {
            return BlockType::Blank;
        }
        if pl == BlockType::HsSep
            && line > self.zone_start
            && setext_prev_is_para(self.lines, self.rope, self.blocks, line)
        {
            return BlockType::Blank;
        }
        pl
    }

    pub fn block_start_at(&self, line: usize) -> bool {
        self.block_at(line).is_some_and(|b| {
            line == b.start as usize && b.implicit()
        })
    }

    pub fn block_end_at(&self, line: usize) -> bool {
        match self.block_at(line) {
            Some(b) => line == b.end as usize - 1 && b.implicit(),
            None => {
                let pl = self.lines[line].node_type();
                pl == BlockType::HsSep
                    && line > self.zone_start
                    && setext_prev_is_para(self.lines, self.rope, self.blocks, line)
            }
        }
    }

    /// per-(line,depth) 内容分类：块起点行 = 块值；段内行有列表 Push 现算回退。
    pub fn content_kind_at(&self, line: usize, depth_idx: usize) -> Option<BlockType> {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("lv_ck_at");
        if let Some(b) = self.block_at(line) {
            if line == b.start as usize {
                return b.content_kind_at(depth_idx);
            }
        }
        let has_list_push = {
            #[cfg(feature = "telemetry")]
            let _g_scan = crate::telemetry::deep_span("lv_ck_scan");
            self.records.iter().any(|r| {
                r.push_lines_of(self.arena).any(|l| l as usize == line)
                    && crate::effect::decl_for(r.frame.kind()).has_loose
            })
        };
        if !has_list_push {
            return None;
        }
        let per_depth = with_line_str(self.rope.line(line), |s| {
            crate::block::content_kind_per_depth(s)
        });
        let depth = self.close_depth_at(line);
        let list_depths = list_push_depths(self.entries, line, depth);
        for (k, &d) in list_depths.iter().enumerate() {
            if d == depth_idx {
                return per_depth.get(k).copied().flatten();
            }
        }
        None
    }

    /// 呈现通道分类器（组合派生——CtxClass）。
    pub fn classify_ctx(
        &self,
        line: usize,
        is_item_open: bool,
        item_depth: usize,
    ) -> crate::effect::CtxClass {
        use crate::effect::CtxClass as CC;
        let d = self.close_depth_at(line);
        if d > 0
            && self.node_type_at_depth(line, d - 1) == BlockType::RefDef
            && self.resolved_type_at(line) != BlockType::RefDef
        {
            return CC::ClosedRd;
        }
        if d > 0
            && crate::effect::block_decl(self.node_type_at_depth(line, d - 1)).is_list_item
            && !is_item_open
            && self.resolved_type_at(line) == BlockType::Paragraph
            && !self.block_start_at(line)
            && !self.is_loose_at(line)
        {
            return CC::ItemTightPara;
        }
        if is_item_open {
            return match self.content_kind_at(line, item_depth) {
                None => CC::ItemOpenEmpty,
                Some(bt) if crate::effect::block_decl(bt).is_list_item
                    || bt == BlockType::Blockquote => CC::ItemOpenNest,
                Some(BlockType::Paragraph) => CC::ItemOpenPara,
                Some(_) => CC::ItemOpenBlock,
            };
        }
        CC::Default
    }

    /// resolve 消费端直读（块起点行直读；item 行与非起点行回退四分支）。
    pub fn resolve_fast(&self, line: usize) -> (BlockType, usize) {
        if let Some(b) = self.block_at(line) {
            if line == b.start as usize
                && !crate::effect::block_decl(b.block_type).is_list_item
            {
                return (b.block_type, b.end as usize);
            }
        }
        resolve_from_cursor(self, line)
    }
}

/// S3 相位拆分：LineView 的行相位实现（建期可用原语——effect 谓词走这一组）。
impl crate::effect::LineResolveView for LineView<'_> {
    fn table_confirmed_at(&self, line: usize) -> bool {
        LineView::table_confirmed_at(self, line)
    }
    fn zone_end(&self) -> usize {
        LineView::zone_end(self)
    }
    fn end_for(&self, line: usize) -> Option<usize> {
        LineView::end_for(self, line)
    }
    fn close_depth_at(&self, line: usize) -> usize {
        LineView::close_depth_at(self, line)
    }
    fn container_end_for(&self, line: usize) -> Option<usize> {
        LineView::container_end_for(self, line)
    }
}

/// S8 挂表：LineView 的结构就绪相位实现（其余原语转发固有方法——
/// effect 求值器 / sdl 对照 / 渲染侧消费的查询契约）。
impl crate::effect::ResolveView for LineView<'_> {
    fn chain_frames_at(&self, line: usize) -> smallvec::SmallVec<[crate::block::BlockFrame; 4]> {
        // 零重放链供给（O(n²) 根因修复——8.4「LineView 走 chain_row」接线）：
        // 行前链＝chain_row 物化池逐 rid 取帧，无任何条目重放。
        let mut out = smallvec::SmallVec::new();
        if let Some(rel) = LineView::rel(self, line) {
            for &rid in self.view.chain_row(rel) {
                out.push(self.records[rid as usize].frame);
            }
        }
        out
    }
    fn resolved_type_at(&self, line: usize) -> BlockType {
        LineView::resolved_type_at(self, line)
    }
    fn block_start_at(&self, line: usize) -> bool {
        LineView::block_start_at(self, line)
    }
    fn block_end_at(&self, line: usize) -> bool {
        LineView::block_end_at(self, line)
    }
    fn node_type_at_depth(&self, line: usize, depth_idx: usize) -> BlockType {
        LineView::node_type_at_depth(self, line, depth_idx)
    }
    fn frame_kind_at_depth(&self, line: usize, depth_idx: usize) -> Option<FrameKind> {
        LineView::frame_kind_at_depth(self, line, depth_idx)
    }
    fn frame_at_depth(&self, line: usize, depth_idx: usize) -> Option<BlockFrame> {
        LineView::frame_at_depth(self, line, depth_idx)
    }
    fn is_container_boundary(&self, line: usize) -> bool {
        LineView::is_container_boundary(self, line)
    }
    fn is_container_change(&self, line: usize) -> bool {
        LineView::is_container_change(self, line)
    }
    fn is_loose_at(&self, line: usize) -> bool {
        LineView::is_loose_at(self, line)
    }
    fn content_kind_at(&self, line: usize, depth_idx: usize) -> Option<BlockType> {
        LineView::content_kind_at(self, line, depth_idx)
    }
    fn is_block_start(&self, line: usize) -> bool {
        LineView::is_block_start(self, line)
    }
    fn is_leaf(&self, line: usize) -> bool {
        LineView::is_leaf(self, line)
    }
    fn advance(&self, t: BlockType, line: usize) -> Option<usize> {
        LineView::advance(self, t, line)
    }
    fn classify_ctx(&self, line: usize, is_item_open: bool, item_depth: usize) -> crate::effect::CtxClass {
        LineView::classify_ctx(self, line, is_item_open, item_depth)
    }
    fn resolve_fast(&self, line: usize) -> (BlockType, usize) {
        LineView::resolve_fast(self, line)
    }
}

/// setext 下划线的前行是否为段落（Part B 同律——item 开行不看 block_type）。
/// 与块机内联版（build_block_records_with 的 Part B）语义有差：本版 item
/// 分支只查 push_line（不查 is_list_item(b.block_type)），块机版双查——对
/// 「已被 setext 改写成 HsHead 的前块」本版 true、块机版 false。两者各自对照全绿
/// （本版对照 setext_prev_is_para、块机版对照 build_block_records），
/// 分歧路径 = RowView block_end_at 的悬空标记回退（无
/// block_start 前导的消费死路径）——钉此注释，不归一份（归并反而破坏对照）。
pub fn setext_prev_is_para(
    lines: &[PackedLineInfo],
    rope: &crop::Rope,
    blocks: &[BlockRec],
    i: usize,
) -> bool {
    let prev_ft = lines[i - 1].node_type();
    match prev_ft {
        BlockType::Paragraph => true,
        BlockType::Blockquote => with_line_str(rope.line(i - 1), |s| {
            let t = crate::block::peel(s).1;
            !t.is_empty() && crate::block::dispatch_lut(t.as_bytes()) == BlockType::Paragraph
        }),
        bt if crate::effect::block_decl(bt).is_list_item => {
            let idx = blocks.partition_point(|b| (b.start as usize) <= i - 1);
            idx > 0 && blocks[idx - 1].start as usize == i - 1 && blocks[idx - 1].push_line()
        }
        _ => false,
    }
}

/// resolve 四分支组合（隐式块 / 列表 item 深度截断 / framed / 单行）。
pub fn resolve_from_cursor(v: &dyn crate::effect::ResolveView, line: usize) -> (BlockType, usize) {
    let t = v.resolved_type_at(line);
    if v.block_start_at(line) {
        let end = (line..v.zone_end())
            .find(|&j| v.block_end_at(j))
            .map(|e| e + 1)
            .unwrap_or(v.zone_end());
        return (t, end);
    }
    if crate::effect::block_decl(t).is_list_item {
        let be = v.end_for(line).unwrap_or(line + 1);
        let d = v.close_depth_at(line);
        let n = line + 1;
        let nd = v.close_depth_at(n);
        if nd > d {
            (t, n.min(be))
        } else {
            (t, be)
        }
    } else if crate::block::frame_kind(t).is_some() {
        (t, v.end_for(line).unwrap_or(line + 1))
    } else {
        (t, line + 1)
    }
}


// §2 reclassify（空间修正）


/// BuildView：reclassify 的构建期视图（BlockView 的平铺等价）——
/// 块记录尚未建出时的行级查询面：栈顶容器尾 + 表确认位图 + 影子栈前瞻。
/// 状态由块遍驱动逐行维护（stack = 行 i 处理后的记录链，ei = 其后游标）。
pub struct BuildView<'a> {
    pub records: &'a [ContainerRec],
    pub entries: &'a [u64],
    pub lines: &'a [PackedLineInfo],
    pub push_offsets: &'a [u32],
    pub push_values: &'a [u32],
    pub table_confirmed: &'a [bool],
    pub zs: usize,
    pub ze: usize,
    pub stack: Vec<u32>,
    pub ei: usize,
    pub line: usize,
    /// is_leaf 守卫（该行 push/body_mut 含行 PL 的帧 kind 才非 leaf）。
    pub leaf: bool,
}

impl BuildView<'_> {
    /// 容器尾（构建期语义）：栈顶记录 start==line → Some(end)，否则 None。
    pub fn container_end_for(&self, line: usize) -> Option<usize> {
        let target = *self.stack.last()?;
        let r = &self.records[target as usize];
        (r.start as usize == line)
            .then_some(r.end_usize().unwrap_or(self.ze))
    }

    pub fn zone_end(&self) -> usize {
        self.ze
    }

    /// 该行 start 帧的 end（leaf 守卫 + start==line + 影子栈前瞻）。
    pub fn end_for(&self, line: usize) -> Option<usize> {
        if self.leaf {
            return None;
        }
        let target = *self.stack.last()?;
        if self.records[target as usize].start as usize != line {
            return None;
        }
        next_pop_end(
            self.entries,
            self.lines,
            self.push_offsets,
            self.push_values,
            self.zs,
            self.ze,
            &self.stack,
            target,
            self.ei,
        )
    }

    /// 构建期深度：line == 当前行 → 栈深；否则影子栈模拟应用该行 entries。
    pub fn sim_open_depth_at(&self, line: usize) -> usize {
        if line == self.line {
            return self.stack.len();
        }
        let mut sim_stack = self.stack.clone();
        let mut sim_ei = self.ei;
        let mut line_count = 0usize;
        let mut last_rel: Option<usize> = None;
        while sim_ei < self.entries.len()
            && entry_line(self.entries[sim_ei]) as usize == line
        {
            let e = self.entries[sim_ei];
            if entry_is_body_mut(e) {
                sim_ei += 1;
                continue;
            }
            if entry_is_pop(e) {
                sim_stack.pop();
            } else {
                let rel = line - self.zs;
                if Some(rel) != last_rel {
                    last_rel = Some(rel);
                    line_count = 0;
                }
                let lo = self.push_offsets[rel] as usize;
                sim_stack.push(self.push_values[lo + line_count]);
                line_count += 1;
            }
            sim_ei += 1;
        }
        sim_stack.len()
    }
}

/// S3 相位拆分：BuildView 只实现**行相位**——挂载谓词（table_row_keep /
/// a05_keep）只问行相位原语（表行确认 / 容器尾 / 窗口尾），故构建期视图可直接入位；
/// 而有效类型 / 块起点 / 块尾 / 呈现通道分类在构建期没有可信答案，本视图**不再提供**
/// （原四个常量假值随之删除）——在错误相位提问现在是编译错误。
impl crate::effect::LineResolveView for BuildView<'_> {
    fn table_confirmed_at(&self, line: usize) -> bool {
        line >= self.zs && line < self.ze && self.table_confirmed[line - self.zs]
    }
    fn zone_end(&self) -> usize {
        self.ze
    }
    fn end_for(&self, line: usize) -> Option<usize> {
        BuildView::end_for(self, line)
    }
    fn close_depth_at(&self, line: usize) -> usize {
        BuildView::sim_open_depth_at(self, line)
    }
    fn container_end_for(&self, line: usize) -> Option<usize> {
        BuildView::container_end_for(self, line)
    }
}

/// 零拷贝行读取（单 chunk 借用 / 多 chunk 回退分配——旧 with_line_str 同款）。
#[inline]
/// UTF-8 安全取行号：入参常来自 `x.saturating_sub(1)`，当**末字符是多字节**（中文）
/// 时该偏移落在字符内部，crop 的 `line_of_byte` 会直接 panic（worker 线程崩溃 →
/// 网关写路径静默悬挂：实测 append 一条中文任务即触发）。
#[inline]
fn line_of_byte_safe(rope: &crop::Rope, byte: usize) -> usize {
    rope.line_of_byte(crate::projection::snap_down_char_boundary(rope, byte))
}

fn with_line_str<T>(line: crop::RopeSlice<'_>, f: impl FnOnce(&str) -> T) -> T {
    let mut chunks = line.chunks();
    let first = chunks.next().unwrap_or("");
    if first.len() == line.byte_len() {
        f(first)
    } else {
        f(&line.to_string())
    }
}

/// 行首 '[' 预检（byte 级——#542 检查先跳过空白看首字符）。
#[inline]
pub fn line_has_bracket_prefix(rope: &crop::Rope, i: usize) -> bool {
    // 只取行首字节游标扫前导空白（空格/tab → 继续；'[' → true；
    // 其余含换行 → false），不物化整行 RopeSlice。
    let mut p = rope.byte_of_line(i);
    let len = rope.byte_len();
    while p < len {
        match rope.byte(p) {
            b' ' | b'\t' => p += 1,
            b'[' => return true,
            _ => return false,
        }
    }
    false
}

/// 行 i 的有效 rt 是否 == Paragraph（#542 两行 Blank 的共享判定）：
/// 本行是 #542 起点（'[' + 跨行 parse）或上一行的第二行 → false。
/// cur_bracket / prev_bracket：顺序扫描状态（None = 自查）。
pub fn effective_para_at(
    lines: &[PackedLineInfo],
    rope: &crop::Rope,
    i: usize,
    ze: usize,
    cur_bracket: Option<bool>,
    prev_bracket: Option<bool>,
) -> bool {
    if lines[i].node_type() != BlockType::Paragraph {
        return false;
    }
    if i + 1 < ze && cur_bracket.unwrap_or_else(|| line_has_bracket_prefix(rope, i)) {
        if with_line_str(rope.line(i), |l0| l0.trim_start().starts_with('['))
            && with_line_str(rope.line(i), |l0| {
                with_line_str(rope.line(i + 1), |l1| {
                    crate::inline::parse_refdef_cross_line(l0, l1).is_some()
                })
            })
        {
            return false;
        }
    }
    if i > 0
        && lines[i - 1].node_type() == BlockType::Paragraph
        && prev_bracket.unwrap_or_else(|| line_has_bracket_prefix(rope, i - 1))
    {
        if with_line_str(rope.line(i - 1), |p0| p0.trim_start().starts_with('['))
            && with_line_str(rope.line(i - 1), |p0| {
                with_line_str(rope.line(i), |p1| {
                    crate::inline::parse_refdef_cross_line(p0, p1).is_some()
                })
            })
        {
            return false;
        }
    }
    true
}

/// 后向确认改写（S8 终态——挂表）：表行声明的 context_fallback + 谓词
/// （effect 自足——table_row_keep/a05_keep 已迁 effect.rs）——分派臂消失，
/// 正逆同规则（scan 侧 context_resolve 与推断层同表同谓词同求值器）。
/// 守卫：reclassify_rewrite_arms_audit 断言表行集合 == {TableRow, RefDef}。
pub fn reclassify_rewrite(
    pl: BlockType,
    view: &BuildView,
    rope: &crop::Rope,
    i: usize,
) -> BlockType {
    crate::effect::context_resolve(
        pl,
        &crate::effect::BlockRewriteCtx { view, rope },
        i,
    )
}

/// reclassify（空间修正）：(行值, 栈, 字节, 表) → 有效类型。
/// 四路：BQ 内容层（A0——prev_rt 是 [prev] 参数）/ rd 撤销 / 伪表回退 /
/// #542 跨行 rd。与旧 rt 四路同律；BQ 分类与跨行 rd 判定共享词法原子。
pub fn reclassify(
    pl: BlockType,
    lines: &[PackedLineInfo],
    rope: &crop::Rope,
    i: usize,
    zs: usize,
    ze: usize,
    view: Option<&BuildView>,
    prev_rt: Option<BlockType>,
    cur_bracket: Option<bool>,
    prev_bracket: Option<bool>,
) -> BlockType {
    if pl == BlockType::Blockquote {
        let is_para_cont = i > zs && prev_rt == Some(BlockType::Paragraph);
        with_line_str(rope.line(i), |s| crate::block::classify_bq_content(s, is_para_cont))
    } else if pl == BlockType::RefDef {
        let v = view.expect("reclassify: RefDef 行必须传 view");
        let rel = i - zs;
        if v.push_offsets[rel] != v.push_offsets[rel + 1] {
            reclassify_rewrite(pl, v, rope, i)
        } else {
            pl
        }
    } else if pl == BlockType::TableRow {
        let v = view.expect("reclassify: TableRow 行必须传 view");
        reclassify_rewrite(pl, v, rope, i)
    } else if pl == BlockType::Paragraph {
        if effective_para_at(lines, rope, i, ze, cur_bracket, prev_bracket) {
            pl
        } else {
            BlockType::Blank
        }
    } else {
        pl
    }
}

/// 行 i 是否在传播区间（二分——intervals 按 start 排序且重叠合并）。
#[inline]
pub fn in_loose_intervals(intervals: &[(usize, usize)], i: usize) -> bool {
    let idx = intervals.partition_point(|&(s, _)| s <= i);
    idx > 0 && intervals[idx - 1].1 > i
}

/// 列表 push 帧深度序列（Part C 的 per-depth 映射）——entries 倒序重放：
/// push 顺序 ↔ marker 层。列表帧过滤 {BLT, OL} 查表
/// has_loose（集合相同——消 kind 枚举列表）。
pub fn list_push_depths(entries: &[u64], line: usize, depth: usize) -> Vec<usize> {
    // 公共签名不变（trilogy 对照面用 Vec）——热路径两遍扫描。
    let mut output = Vec::new();
    for_each_loose_depth(entries, line, depth, |_k, d| output.push(d));
    // 回调为反序（k 递减）——翻回前向序（原实现 push 后 reverse 的同义）。
    output.reverse();
    output
}

/// O4（性能专项·无缓冲版）：本行 loose-capable 列表 Push 的 (k, d) 逐项回调
/// （k＝前向 marker 序下标、d＝深度），返回总数。原实现两次堆分配/行
/// （out:Vec ＋ line_entries:Vec）；缓冲版还要每行 memset 256B；本版两遍扫描：
/// ①数出总数 ②按条目反序（=层次反序）推进，j 为反序序位 ⇒ 前向下标 k=total-1-j。
pub(crate) fn for_each_loose_depth(
    entries: &[u64],
    line: usize,
    depth: usize,
    mut f: impl FnMut(usize, usize),
) -> usize {
    let start = partition(entries, line as u32);
    let mut end = start;
    let mut total = 0usize;
    while end < entries.len() && entry_line(entries[end]) as usize == line {
        let e = entries[end];
        if !entry_is_pop(e)
            && !entry_is_body_mut(e)
            && crate::effect::decl_for(entry_frame(e).kind()).has_loose
        {
            total += 1;
        }
        end += 1;
    }
    let mut run = depth as isize;
    let mut j = 0usize;
    for &e in entries[start..end].iter().rev() {
        if entry_is_pop(e) {
            run += 1;
        } else if !entry_is_body_mut(e) {
            run -= 1;
            if crate::effect::decl_for(entry_frame(e).kind()).has_loose {
                f(total - 1 - j, run as usize);
                j += 1;
            }
        }
    }
    total
}

/// 块记录——一个语义块一条（V2：32B 布局，flags 位打包 + content_kind u8 哨兵列）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRec {
    pub start: u32,
    pub end: u32,
    pub block_type: BlockType,
    pub content_kind: Option<Arc<[u8]>>,
    pub container_ref: u32,
    /// bit0=implicit bit1=push_line bit2=loose。
    pub flags: u8,
}

pub const CONTENT_NONE: u8 = 0xFF;
pub const BLK_FLAG_IMPLICIT: u8 = 1;
pub const BLK_FLAG_PUSH_LINE: u8 = 2;
pub const BLK_FLAG_LOOSE: u8 = 4;

impl BlockRec {
    #[inline]
    pub fn implicit(&self) -> bool {
        self.flags & BLK_FLAG_IMPLICIT != 0
    }
    #[inline]
    pub fn push_line(&self) -> bool {
        self.flags & BLK_FLAG_PUSH_LINE != 0
    }
    #[inline]
    pub fn loose(&self) -> bool {
        self.flags & BLK_FLAG_LOOSE != 0
    }
    #[inline]
    pub fn set_flags(&mut self, implicit: bool, push_line: bool, loose: bool) {
        self.flags = 0;
        if implicit { self.flags |= BLK_FLAG_IMPLICIT; }
        if push_line { self.flags |= BLK_FLAG_PUSH_LINE; }
        if loose { self.flags |= BLK_FLAG_LOOSE; }
    }
    #[inline]
    pub fn content_kind_at(&self, depth: usize) -> Option<BlockType> {
        self.content_kind
            .as_ref()
            .and_then(|arr| arr.get(depth).copied())
            .and_then(block_type_from_content)
    }
}

/// 0xFF 哨兵 → Option<BlockType> 的单点映射（CONTENT_NONE 语义）。
#[inline]
pub fn block_type_from_content(v: u8) -> Option<BlockType> {
    if v == CONTENT_NONE || v > BlockType::HtmlElement as u8 {
        None
    } else {
        Some(unsafe { std::mem::transmute::<u8, BlockType>(v) })
    }
}

/// Option<BlockType> → u8 哨兵列（CONTENT_NONE 语义，逆映射单点）。
#[inline]
pub fn content_from_block_type(t: Option<BlockType>) -> u8 {
    t.map(|bt| bt as u8).unwrap_or(CONTENT_NONE)
}

/// Part A 段状态（merges 连续段——逐行推进——产出时构造块记录）。
struct Seg {
    rt: BlockType,
    depth: usize,
    start: usize,
    end: usize,
    container_ref: u32,
    push_line: bool,
    loose: bool,
    content_kind: Option<Arc<[u8]>>,
}

/// 段结束产出（Backtrack——IC 尾部 Blank 回退——TrailingPolicy::Backtrack 语义）。
fn push_seg(blocks: &mut Vec<BlockRec>, s: &Seg, lines: &[PackedLineInfo]) {
    let mut end = s.end;
    if crate::effect::BLOCK_TYPE_EFFECTS[s.rt as usize].trailing
        == crate::effect::TrailingPolicy::Backtrack
    {
        while end > s.start + 1 && lines[end - 1].node_type() == BlockType::Blank {
            end -= 1;
        }
    }
    let mut b = BlockRec {
        start: s.start as u32,
        end: end as u32,
        block_type: s.rt,
        content_kind: s.content_kind.clone(),
        container_ref: s.container_ref,
        flags: 0,
    };
    b.set_flags(true, s.push_line, s.loose);
    blocks.push(b);
}

/// 块记录构建——build_block_records 的原子组合形态。
/// 行级状态机（空间轴——含跳段：framed 内容行并入块区间、item 续行前瞻）。
/// entry 折叠按行内批次（walk_lines 同款纪律：push_map 行内计数取 rid）。
/// 消费原子：partition / reclassify / end_for_rule（经 BuildView.end_for）/
/// 表参数（merges / blank_policy / trailing / corrections / is_list_item /
/// has_loose / meta_is_skip）——机械内无 kind 枚举列表。

pub fn build_block_records(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    rope: &crop::Rope,
    records: &[ContainerRec],
    arena: &RecArena,
    zone: (u32, u32),
) -> Vec<BlockRec> {
    // 纯观测插桩：blocks 段外层 + 两个前置面（view arrays / 表确认列）。
    let _sp_blk_total = crate::telemetry_span!("blk_total");
    let va = { let _g = crate::telemetry_span!("blk_arrays"); build_view_arrays(records, arena, entries, zone) };
    let tc = { let _g = crate::telemetry_span!("blk_tc"); table_confirmed_flags(entries, zone) };
    build_block_records_with(entries, lines, depths, rope, records, arena, zone, &va, &tc)
}

/// 注入版（P10 ③ 形态选型：line 面物化一次、注入消费——块机不自带
/// line 面构建；loose 判定直读 va.loose_at（O(1) 替代区间二分）。
pub fn build_block_records_with(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    rope: &crop::Rope,
    records: &[ContainerRec],
    arena: &RecArena,
    zone: (u32, u32),
    va: &ViewArrays,
    tc: &[bool],
) -> Vec<BlockRec> {
    // 纯观测插桩：blocks 段主体（逐行四段用 deep 计数器 span，见 §10.7 分层下钻）。
    let _sp_blk_with = crate::telemetry_span!("blk_with");
    let zs = zone.0 as usize;
    let ze = zone.1 as usize;
    let push_offsets = &va.push_offsets;
    let push_values = &va.push_values;
    let mut blocks: Vec<BlockRec> = Vec::with_capacity(
        ((ze - zs) >> 2).max(16).min(1 << 20));
    let mut stack: Vec<u32> = Vec::new();
    let mut ei = partition(entries, zone.0);
    let mut last_rel = usize::MAX;
    let mut line_count = 0usize;
    let mut prev_rt: Option<BlockType> = None;
    let mut prev_para_bracket = false;
    let mut seg: Option<Seg> = None;
    // content_kind 的 per-depth 缓冲（跨行复用）。
    let mut ck_buf: Vec<Option<BlockType>> = Vec::new();
    let mut i = zs;
    while i < ze {
        // 行 i 的 entry 折叠前栈深 = 行 i-1 处理后的容器深度（setext
        // 同容器检查用——下划线行不得改变容器）。
        let stack_len_before = stack.len();
        // 行内批次：折叠 entries ≤ i（STEP 先于 GET）。
        // 行内批次：折叠 entries ≤ i（STEP 先于 GET）。
        {
            #[cfg(feature = "telemetry")]
            let _g_deep = crate::telemetry::deep_span("blk_entries");
            while ei < entries.len() && entry_line(entries[ei]) as usize <= i {
                let e = entries[ei];
                if entry_is_body_mut(e) {
                    ei += 1;
                    continue;
                }
                if entry_is_pop(e) {
                    stack.pop();
                } else {
                    let rel = entry_line(e) as usize - zs;
                    if rel != last_rel {
                        last_rel = rel;
                        line_count = 0;
                    }
                    let lo = push_offsets[rel] as usize;
                    stack.push(push_values[lo + line_count]);
                    line_count += 1;
                }
                ei += 1;
            }
        }
        let pl = lines[i].node_type();
        // 懒 BuildView（P10 ③）：段落行 / BQ 行 / 跳过行不付栈克隆与
        // leaf 回扫——reclassify 的 rd/伪表分支与 framed end_for 现场构造。
        let mk_view = |leaf: bool| BuildView {
            records,
            entries,
            lines,
            push_offsets,
            push_values,
            table_confirmed: tc,
            zs,
            ze,
            stack: stack.clone(),
            ei,
            line: i,
            leaf,
        };
        let cur_bracket = pl == BlockType::Paragraph && {
            #[cfg(feature = "telemetry")]
            let _g_deep = crate::telemetry::deep_span("blk_brk");
            line_has_bracket_prefix(rope, i)
        };
        let view_pre = (pl == BlockType::RefDef || pl == BlockType::TableRow)
            .then(|| mk_view(false));
        let rt = {
            #[cfg(feature = "telemetry")]
            let _g_deep = crate::telemetry::deep_span("blk_recl");
            reclassify(pl, lines, rope, i, zs, ze, view_pre.as_ref(),
                prev_rt, Some(cur_bracket), Some(prev_para_bracket))
        };
        prev_para_bracket = cur_bracket;
        prev_rt = Some(rt);

        // Part C——content_kind（该行 entry 流含列表 Push/PopPush——结构真相）。
        let has_loose_push = {
            #[cfg(feature = "telemetry")]
            let _g_deep = crate::telemetry::deep_span("blk_prow");
            va.push_row(i - zs).iter().any(|&rid| {
                crate::effect::decl_for(records[rid as usize].frame.kind()).has_loose
            })
        };
        let content_kind = if has_loose_push {
            #[cfg(feature = "telemetry")]
            let _g_deep_ck = crate::telemetry::deep_span("blk_ck");
            let per_depth: &[Option<BlockType>] = {
                #[cfg(feature = "telemetry")]
                let _g = crate::telemetry::deep_span("blk_ck_scan");
                let line = {
                    #[cfg(feature = "telemetry")]
                    let _g_l = crate::telemetry::deep_span("blk_ck_line");
                    rope.line(i)
                };
                with_line_str(line, |s| {
                    #[cfg(feature = "telemetry")]
                    let _g_p = crate::telemetry::deep_span("blk_ck_prefix");
                    crate::block::content_kind_per_depth_into(s, &mut ck_buf)
                });
                &ck_buf
            };
            let depth = stack.len();
            // O4（性能专项）：栈缓冲替代 vec![CONTENT_NONE; depth]（每 loose 行
            // 一次堆分配）——depth ≤ MAX_BLOCK_NESTING(32)；Arc::from(&arr[..depth])
            // 保持一次 Arc 分配（既有语义）。
            let mut arr = [CONTENT_NONE; MAX_BLOCK_NESTING];
            // O4（性能专项）：两遍扫描回调——零分配零初始化替原
            // out:Vec＋line_entries:Vec（每行两次堆分配）。
            let mut last_d: Option<usize> = None;
            let _n_ld = {
                #[cfg(feature = "telemetry")]
                let _g = crate::telemetry::deep_span("blk_ck_ld");
                for_each_loose_depth(entries, i, depth, |k, d| {
                    if let Some(v) = per_depth.get(k).copied().flatten() {
                        arr[d] = content_from_block_type(Some(v));
                    }
                    // 回调按反序（k 递减）——首项即前向末项（≡ 原 list_depths.last()）。
                    if last_d.is_none() {
                        last_d = Some(d);
                    }
                })
            };
            // 827-CM:item 内 setext 前瞻——标题身份在内容类型层诞生(与
            // ATX 同层),覆盖表 ItemOpenBlock/SuppressCt 自动接管渲染/差分/
            // 适配。判据与 Part B 同源:下一行同容器 HsSep + 本 item 内容为
            // 段落。选型读现成修正表(不手写 HsHead 常量)。
            {
                #[cfg(feature = "telemetry")]
                let _g = crate::telemetry::deep_span("blk_ck_setext");
            if i + 1 < ze
                && lines[i + 1].node_type() == BlockType::HsSep
                && depths[i + 1] as usize == depth
                && depths[i + 2] as usize == depth
            {
                if let Some(d) = last_d {
                    if arr[d] == BlockType::Paragraph as u8 {
                        let sep_ch = with_line_str(rope.line(i + 1), |s| {
                            s.trim_start().as_bytes().first().copied().unwrap_or(0)
                        });
                        if let Some(crate::effect::Correction::Setext {
                            underline_eq, underline_dash,
                        }) = crate::effect::BLOCK_TYPE_EFFECTS
                            [BlockType::Paragraph as usize]
                            .corrections
                            .first()
                        {
                            arr[d] = if sep_ch == b'=' { *underline_eq } else { *underline_dash } as u8;
                        }
                    }
                }
            }
            }
            {
                #[cfg(feature = "telemetry")]
                let _g = crate::telemetry::deep_span("blk_ck_arc");
                Some(Arc::from(&arr[..depth]))
            }
        } else {
            None
        };

        // 段延续（先于 skip/cont——Blank 吸收判 PL；延续加 push_map 空守卫）。
        if let Some(s) = seg.as_ref() {
            #[cfg(feature = "telemetry")]
            let _g_deep_seg = crate::telemetry::deep_span("blk_seg");
            if (rt == s.rt && stack.len() == s.depth && va.push_row(i - zs).is_empty())
                || (crate::effect::BLOCK_TYPE_DECLS[s.rt as usize].blank_policy
                    == crate::effect::BlankPolicy::Allow
                    && pl == BlockType::Blank)
            {
                seg.as_mut().unwrap().end = i + 1;
                i += 1;
                continue;
            }
            push_seg(&mut blocks, &seg.take().unwrap(), lines);
        }

        // Part B——setext 修正（下划线行 prev 块 = 段或 item 块）。
        // 同容器检查：下划线行的容器深度必须与标题行一致（行 i 折叠前
        // 的栈深）——`plain > - a\n> ===` 的下划线行开 BQ（push）——
        // 跨容器 setext 被接受会产区间重叠的空 BQ + setext 块（patch
        // gate 3000 case 328——diff 索引错乱）。合法场景 `> foo\n> ===`
        // 与 `foo\n===` 下划线行无容器事件（stack 深度不变）。
        if pl == BlockType::HsSep && i > zs && stack.len() == stack_len_before {
            let prev_ft = lines[i - 1].node_type();
            let prev_is_para = match prev_ft {
                BlockType::Paragraph => true,
                BlockType::Blockquote => with_line_str(rope.line(i - 1), |s| {
                    let t = crate::block::peel(s).1;
                    !t.is_empty()
                        && crate::block::dispatch_lut(t.as_bytes()) == BlockType::Paragraph
                }),
                bt if crate::effect::block_decl(bt).is_list_item => {
                    blocks.last().is_some_and(|b| {
                        (b.start as usize) == i - 1
                            && b.push_line()
                            && crate::effect::block_decl(b.block_type).is_list_item
                    })
                }
                _ => false,
            };
            if prev_is_para {
                let eff_ft = if prev_ft == BlockType::Blockquote
                    || crate::effect::block_decl(prev_ft).is_list_item
                {
                    BlockType::Paragraph
                } else {
                    prev_ft
                };
                // 防御兜底:修正表缺 Setext 行时退回 (HsHead1, HsHead2)——
                // 与 hs_setext_level 的表外兜底同值,由 sdl 1.2c 判官钉住。
                let (eq, dash) = match crate::effect::BLOCK_TYPE_EFFECTS[eff_ft as usize]
                    .corrections
                    .first()
                {
                    Some(crate::effect::Correction::Setext { underline_eq, underline_dash }) => {
                        (*underline_eq, *underline_dash)
                    }
                    _ => (BlockType::HsHead1, BlockType::HsHead2),
                };
                let ch = with_line_str(rope.line(i), |s| {
                    // ⑤b：prefix_cut 单源（钉②背书——(BqOnly,None)+FinalSkip ≡ rest_after_bq）
                    let cut = crate::infer::prefix_cut(s, &[], &[], crate::effect::PrefixKinds::BqOnly,
                        crate::effect::PrefixStop::None, crate::effect::PrefixLead::SkipWs,
                        crate::effect::PrefixTail::FinalSkip).bytes;
                    s.as_bytes().get(cut).copied().unwrap_or(0)
                });
                let b = blocks.last_mut().unwrap();
                b.block_type = if ch == b'=' { eq } else { dash };
                // 827-CM:标题块无条件拥有下划线行(与 ATX 标题块拥有自己行
                // 同律)——item 内容投影归零,渲染/差分/适配同走
                // ItemOpenBlock/SuppressCt 现成链,无双渲染。
                b.end = (i + 1) as u32;
                i += 1;
                continue;
            }
        }

        // skip 判定用有效 rt（空 BQ 开行产单行 Blank 块）。
        if crate::effect::meta_is_skip(rt) {
            if rt == BlockType::Blank && pl == BlockType::Blockquote
                && !va.push_row(i - zs).is_empty()
            {
                let mut b = BlockRec {
                    start: i as u32,
                    end: (i + 1) as u32,
                    block_type: rt,
                    content_kind: None,
                    container_ref: stack.last().copied().unwrap_or(u32::MAX),
                    flags: 0,
                };
                b.set_flags(false, true, va.loose_at(i - zs));
                blocks.push(b);
            }
            i += 1;
            continue;
        }
        // cont 跳过防御（仅前段可并入时跳过）。
        if lines[i].cont() && seg.is_some() {
            i += 1;
            continue;
        }

        if crate::effect::BLOCK_TYPE_DECLS[rt as usize].merges {
            seg = Some(Seg {
                rt,
                depth: stack.len(),
                start: i,
                end: i + 1,
                container_ref: stack.last().copied().unwrap_or(u32::MAX),
                push_line: !va.push_row(i - zs).is_empty(),
                loose: va.loose_at(i - zs),
                content_kind,
            });
            i += 1;
        } else if crate::effect::block_decl(rt).is_list_item {
            // item 块：end = 续行段落前瞻（cont + effective_para）。
            // 通道统一（rt 四路同判据——reclassify 的 Paragraph 分支即
            // effective_para_at——行为零变化——未来 rt 判据演化自动传导）。
            let mut e = i + 1;
            {
                #[cfg(feature = "telemetry")]
                let _g_ie = crate::telemetry::deep_span("blk_item_end");
            while e < ze
                && lines[e].cont()
                && reclassify(BlockType::Paragraph, lines, rope, e, zs, ze, None, prev_rt, None, None)
                    == BlockType::Paragraph
            {
                e += 1;
            }
            }
            let mut b = BlockRec {
                start: i as u32,
                end: e as u32,
                block_type: rt,
                content_kind,
                container_ref: stack.last().copied().unwrap_or(u32::MAX),
                flags: 0,
            };
            b.set_flags(false, !va.push_row(i - zs).is_empty(), va.loose_at(i - zs));
            blocks.push(b);
            i = e.max(i + 1);
        } else {
            let be = if crate::block::frame_kind(rt).is_some() {
                #[cfg(feature = "telemetry")]
                let _g = crate::telemetry::deep_span("blk_frame");
                // is_leaf：该行"非 Pop 或 body_mut"条目的 kind 集合不含行 PL
                // 的帧 kind——va 两列直读（push_map 含 PopPush 的 Push 半部、
                // body_mut_rows 含 BMut——与旧 entries 回扫同集合；消第三份
                // is_leaf 机械，O(1) 查表）。
                let leaf = crate::block::frame_kind(pl).map_or(false, |k| {
                    !(va.push_row(i - zs).iter()
                        .any(|&rid| records[rid as usize].frame.kind() == k)
                        || va.body_mut_row(i - zs).contains(&k))
                });
                let v = {
                    #[cfg(feature = "telemetry")]
                    let _g2 = crate::telemetry::deep_span("blk_view_mk");
                    mk_view(leaf)
                };
                {
                    #[cfg(feature = "telemetry")]
                    let _g3 = crate::telemetry::deep_span("blk_view_end");
                    v.end_for(i).unwrap_or(i + 1)
                }
            } else {
                i + 1
            };
            let mut b = BlockRec {
                start: i as u32,
                end: be as u32,
                block_type: rt,
                content_kind,
                container_ref: stack.last().copied().unwrap_or(u32::MAX),
                flags: 0,
            };
            b.set_flags(false, !va.push_row(i - zs).is_empty(), va.loose_at(i - zs));
            blocks.push(b);
            i = be.max(i + 1);
        }
    }
    if let Some(s) = seg.take() {
        push_seg(&mut blocks, &s, lines);
    }
    blocks
}

// ── Phase A：组合原子（build_structure_parts / with_structure）──

/// 标准流水线原子：一次构建 records + blocks + ViewArrays（view arrays 单算）。
/// 不返回 LineView，避免自引用；调用方用 `LineView::with_arrays` 或 `with_structure`。
pub fn build_structure_parts(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    rope: &crop::Rope,
    zone: (u32, u32),
) -> (Vec<ContainerRec>, Vec<BlockRec>, ViewArrays, RecArena) {
    #[cfg(feature = "telemetry")] let _s_l1 = crate::telemetry_span!("bsp_l1");
    let mut fold = RecordFold::new(entries, lines);
    record_fold_walk_zone(entries, zone, &mut fold);
    let (records, arena) = fold.finish();
    #[cfg(feature = "telemetry")] drop(_s_l1);
    #[cfg(feature = "telemetry")] let _s_l2b = crate::telemetry_span!("bsp_l2b");
    let va = build_view_arrays(&records, &arena, entries, zone);
    #[cfg(feature = "telemetry")] drop(_s_l2b);
    #[cfg(feature = "telemetry")] let _s_tc = crate::telemetry_span!("bsp_tc");
    let tc = table_confirmed_flags(entries, zone);
    #[cfg(feature = "telemetry")] drop(_s_tc);
    #[cfg(feature = "telemetry")] let _s_l2a = crate::telemetry_span!("bsp_l2a");
    let blocks = build_block_records_with(entries, lines, depths, rope, &records, &arena, zone, &va, &tc);
    #[cfg(feature = "telemetry")] drop(_s_l2a);
    (records, blocks, va, arena)
}

/// 组合原子的作用域形态：在闭包内同时给出 records / blocks / LineView。
/// 生命周期由借用作用域保证，调用方无需自己拼三件套。
pub fn with_structure<R>(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    rope: &crop::Rope,
    zone: (u32, u32),
    f: impl FnOnce(&[ContainerRec], &[BlockRec], &LineView<'_>) -> R,
) -> R {
    let (records, blocks, va, arena) = build_structure_parts(entries, lines, depths, rope, zone);
    let view = LineView::with_arrays(&records, &blocks, entries, lines, rope, zone, va, &arena);
    f(&records, &blocks, &view)
}

/// refold 半程——l1(RecordFold) + l2b(build_view_arrays),
/// 跳过 l2a(块层——唯一读文本层)。full 组装用:records 由构造恒等
/// (与 Segment::build 同一 fold——id 白来),view 值列由 records 派生。
pub fn build_records_view(
    entries: &[u64],
    lines: &[PackedLineInfo],
    _depths: &[u8],
    zone: (u32, u32),
) -> (Vec<ContainerRec>, ViewArrays, RecArena) {
    let mut fold = RecordFold::new(entries, lines);
    record_fold_walk_zone(entries, zone, &mut fold);
    let (records, arena) = fold.finish();
    let va = build_view_arrays(&records, &arena, entries, zone);
    (records, va, arena)
}


// §3 span 原子 + locate（唯一导航）


/// 行首深度列的独立 fold 对拍 oracle（仅测试——生产列由 State 三截维护）。
/// out[l] = 行 l 事件前的栈深；out[n] = 文档尾残深哨兵。
#[cfg(test)]
pub fn depth_prefix(entries: &[u64], n: usize) -> Vec<u8> {
    // 长度 n+1——out[n] = 文档尾哨兵深度（全部折叠后残深——未关闭容器）
    // ——expand_zone 锚定需要（zone 尾在文档尾时按残深回退）。
    let mut out = vec![0u8; n + 1];
    let mut fold = ReplayFold::default();
    let mut cur = 0usize;
    let mut ei = 0usize;
    while ei < entries.len() {
        let l = crate::block::entry_line(entries[ei]) as usize;
        if l >= n {
            break;
        }
        // 行首深度（事件前）写入
        while cur <= l {
            out[cur] = fold.stack.len() as u8;
            cur += 1;
        }
        // 折叠行 l 全部事件
        while ei < entries.len() && crate::block::entry_line(entries[ei]) as usize == l {
            fold_entry(&mut fold, entries[ei], entries.get(ei + 1).copied());
            ei += 1;
        }
    }
    while cur <= n {
        out[cur] = fold.stack.len() as u8;
        cur += 1;
    }
    out
}

/// zone 容器闭包（B2-5 重设计——镜像旧 incr::expand_zone_to_containers 的
/// 单次前向括号匹配，记录查询无循环重放）：
///   1. cs 先锚 depth-0（depth_prefix 列直读）；
///   2. 只考虑 start ∈ [cs, ce) 的记录：普通记录取 end（None → n）；
///      shell 记录若关闭 Pop 是 kindonly PopPush，则跨记录跟随同 kind
///      链到最终真实 Pop（container_log_end 的跳过语义）；
///   3. ce 取所有有效 end 的最大值。
/// min_push 无效：锚定后扫描区间内所有 Push 行都 ≥ cs，不可能产生 < cs 的 min_push。
/// 等价性由 sdl/生成式守卫对拍旧 incr 版本（全 zone 全 kind）。
pub fn expand_zone_to_containers(
    records: &[ContainerRec],
    arena: &RecArena,
    depths: &[u8],
    n: usize,
    mut cs: usize,
    mut ce: usize,
) -> (usize, usize) {
    cs = cs.min(n);
    ce = ce.min(n);
    // 1. 锚定 depth-0（snap_to_depth0）。
    while cs > 0 && depths.get(cs).copied().unwrap_or(0) > 0 {
        cs -= 1;
    }
    let anchor = cs;
    let mut max_end = ce;
    // 2. 只镜像旧扫描区间 [anchor, ce) 内 Push 的记录。
    for r in records {
        let s = r.start as usize;
        if s < anchor || s >= ce {
            continue;
        }
        // 同 kind PopPush 的 Push 半部被旧扫描的 i+=2 一并消费，不独立入栈；
        // 其贡献由被弹记录的闭包链表达（shell 链跟随 / 非 shell 止于弹点）。
        if r.pop_push_events_of(arena).any(|ev| {
            ev.line as usize == s && ev.popped_kind == r.frame.kind()
        }) {
            continue;
        }
        let mut end = r.end_usize().unwrap_or(n);
        // shell 记录：end_poppush_kindonly 表示该 Pop 会被
        // container_log_end 跳过——沿同 kind 链跟随到最终真实 Pop。
        if crate::effect::effect_for(r.frame.kind()).is_shell
            && r.end_poppush_kindonly()
        {
            let mut hops = 0usize;
            loop {
                if hops >= records.len() {
                    break;
                }
                let Some(next) = records.iter().find(|x| {
                    x.start as usize == end
                        && x.frame.kind() == r.frame.kind()
                        && x.start != r.start
                }) else {
                    break;
                };
                end = next.end_usize().unwrap_or(n);
                if !next.end_poppush_kindonly() {
                    break;
                }
                hops += 1;
            }
        }
        if end > ce {
            max_end = max_end.max(end.min(n));
        }
    }
    (anchor, max_end.min(n))
}

/// FL 版 zone 容器闭包（生产形态）：entries + lines 直接单次前向括号匹配。
/// 与记录版 `expand_zone_to_containers` 逐值等价，由 tests/zone_closure_equivalence
/// 三方守卫维护；记录版保留为 SDL/测试 oracle。
pub fn expand_zone_to_containers_fl(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    n: usize,
    cs: usize,
    ce: usize,
) -> (usize, usize) {
    let mut cs = cs.min(n);
    let mut ce = ce.min(n);

    // 先锚定 cs 到容器外（depth 0），否则扫描区间不含容器的 Push。
    while cs > 0 && depths.get(cs).copied().unwrap_or(0) > 0 {
        cs -= 1;
    }

    let ei_start = partition(entries, cs as u32);
    let ei_end = partition(entries, ce as u32);
    let mut stack: Vec<(FrameKind, usize)> = Vec::new();
    let mut min_push = usize::MAX;
    let mut i = ei_start;
    while i < ei_end.min(entries.len()) {
        let e = entries[i];
        if entry_is_body_mut(e) {
            i += 1;
            continue;
        }
        let li = entry_line(e) as usize;
        let kind = entry_frame(e).kind();
        if entry_is_pop(e) {
            let is_poppush = crate::effect::is_poppush_kindonly(e, entries.get(i + 1).copied());
            if is_poppush {
                // 容器延续：PopPush 不关闭，Push 半部复用旧 start。
                if let Some((pk, pl)) = stack.pop() {
                    if pk == kind {
                        min_push = min_push.min(pl);
                        stack.push((pk, pl));
                    }
                }
                i += 2;
                continue;
            }
            // 真实 Pop：结算 end（end_for_rule 替代旧 pop_includes_close）。
            if let Some((pk, pl)) = stack.pop() {
                if pk == kind {
                    min_push = min_push.min(pl);
                    let pop_pl = lines.get(li).map(|p| p.node_type()).unwrap_or(BlockType::Blank);
                    let end = end_for_rule(kind, li, pop_pl);
                    if end > ce {
                        ce = end.min(n);
                    }
                }
            }
            i += 1;
        } else {
            stack.push((kind, li));
            i += 1;
        }
    }

    // 未关闭容器：container_log_end 补结算；查不到即文档尾。
    for &(kind, pl) in &stack {
        if let Some(end) = container_log_end(entries, lines, pl as u32, kind) {
            if end > ce {
                ce = end.min(n);
            }
        } else {
            ce = n;
        }
    }

    if min_push != usize::MAX && min_push < cs {
        cs = min_push;
    }
    while cs > 0 && depths.get(cs).copied().unwrap_or(0) > 0 {
        cs -= 1;
    }
    (cs, ce)
}

/// 树节点父的 kind(通用——拓扑无关):None = 根节点;Some(None) =
/// 父为块/节节点;Some(Some(k)) = 父为容器(kind = k)。
pub fn parent_kind(tree: &AttrTreeNew, idx: usize) -> Option<Option<FrameKind>> {
    for n in &tree.nodes {
        if n.children_of(tree).contains(&(idx as u32)) {
            return Some(n.kind);
        }
    }
    None
}

/// 含 line 的最内层区间(通用——拓扑无关):区间包含 s <= line < e 中
/// start 最大者。sealed_interval_of 的上移起点(照搬 shell_at 谓词——
/// 827 重写将其通用化移居此层)。
pub fn innermost_interval(intervals: &[(usize, usize)], line: usize) -> Option<(usize, usize)> {
    intervals
        .iter()
        .filter(|&&(s, e)| s <= line && line < e)
        .max_by_key(|&&(s, _)| s)
        .copied()
}

/// 密封区间选择(通用——拓扑无关):含 line 的最外层区间(区间包含链
/// 上移:存在 (ss,se) 满足 ss < s && s < se 时上移)+ 空隙兜底 +
/// 双侧容器密封(expand_zone_to_containers_fl——域起点必须 depth-0,
/// RecordFold 前置)。调用方提供任意区间列表(adapt 传节区间;未来
/// 拓扑/多域消费方复用)。零新语义:谓词与容器密封全为现成件。
pub fn sealed_interval_of(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    n: usize,
    intervals: &[(usize, usize)],
    line: usize,
) -> (usize, usize) {
    // 探查插桩(零行为):密封前后的区间——用于判定"域=章节还是后缀"
    // (adapt_parts 的 ds/de 从哪来)。
    #[cfg(feature = "telemetry")]
    let _sp_si = crate::telemetry_span!("si_seal",
        "line" => line, "n_iv" => intervals.len(), "n" => n,
        "in" => format!("{}..{}", innermost_interval(intervals, line).map(|x| x.0).unwrap_or(usize::MAX),
                        innermost_interval(intervals, line).map(|x| x.1).unwrap_or(usize::MAX)));
    let mut cur: Option<(usize, usize)> = innermost_interval(intervals, line);
    let (s, e) = match cur {
        None => {
            let prev = intervals
                .iter()
                .filter(|&&(_, e)| e <= line)
                .map(|&(_, e)| e)
                .max()
                .unwrap_or(0);
            let next = intervals
                .iter()
                .filter(|&&(s, _)| s > line)
                .map(|&(s, _)| s)
                .min()
                .unwrap_or(n);
            (prev, next)
        }
        Some((mut s, mut e)) => {
            loop {
                let parent = intervals
                    .iter()
                    .filter(|&&(ss, se)| ss < s && s < se)
                    .max_by_key(|&&(ss, _)| ss)
                    .copied();
                match parent {
                    Some((ps, pe)) => {
                        s = ps;
                        e = pe;
                    }
                    None => break,
                }
            }
            (s, e)
        }
    };
    // 右界:容器闭包(expand_zone_to_containers_fl 的 ce 半部——域内开启的
    // 容器/未闭合容器补尾;其左锚是事件前保守语义,域工具不取)。
    let (_, ce) = expand_zone_to_containers_fl(entries, lines, depths, n, s, e);
    // 左界:节起点/空隙起点自身即密封界——仅当起点仍处在开放容器内
    // (容器内标题:bq 跨过标题行)才向左扩到最外层容器起点;标题打断
    // 容器(容器恰在起点处关闭)时不扩——扩入前一节内部会让 off+comps
    // 双计(域根与文档根不再对齐)。
    let cs = if depths.get((s + 1).min(n)).copied().unwrap_or(0) > 0 {
        let mut l = s;
        while l > 0 && depths.get(l).copied().unwrap_or(0) > 0 {
            l -= 1;
        }
        l
    } else {
        s
    };
    #[cfg(feature = "telemetry")]
    drop(_sp_si.kv("out", format!("{cs}..{ce}")));
    (cs, ce)
}

/// 行 li 的 entry 流是否含列表帧 push（C5 迁入——表化：has_loose 表列
/// 替代 BLT|OL 枚举——集合相同）。
pub fn has_list_frame_at(entries: &[u64], li: usize) -> bool {
    let mut ei = entries.partition_point(|e| entry_line(*e) < li as u32);
    while ei < entries.len() && entry_line(entries[ei]) as usize == li {
        let e = entries[ei];
        if !entry_is_pop(e) && !entry_is_body_mut(e)
            && crate::effect::decl_for(entry_frame(e).kind()).has_loose
        {
            return true;
        }
        ei += 1;
    }
    false
}

/// 行 li 的 entry 流是否含指定帧的 push（非 Pop/body_mut——C5 迁入）。
pub fn has_frame_push_at(entries: &[u64], li: usize, kind: FrameKind) -> bool {
    let mut ei = entries.partition_point(|e| entry_line(*e) < li as u32);
    while ei < entries.len() && entry_line(entries[ei]) as usize == li {
        let e = entries[ei];
        if !entry_is_pop(e) && !entry_is_body_mut(e) {
            if entry_frame(e).kind() == kind {
                return true;
            }
        }
        ei += 1;
    }
    false
}

/// 向前跳过连续 Blank 行找最后非 Blank 类型（C5 迁入——纯 PL 查询）。
pub fn lookback_skipping_blanks(i: usize, lines: &[PackedLineInfo]) -> Option<BlockType> {
    let mut p = i;
    while p > 0 {
        p -= 1;
        let nt = lines[p].node_type();
        if nt == BlockType::Blank {
            continue;
        }
        return Some(nt);
    }
    None
}

/// 容器关闭行查询（C5 迁入——end_for_rule 直调替代旧 pop_includes_close）：
/// shell 容器（BLT/OL）：跳过 PopPush，返回第一个真实 Pop 行；
/// 非 shell：返回第一个 Pop（含 PopPush）的 end_for_rule 结算。
pub fn container_log_end(
    entries: &[u64],
    lines: &[PackedLineInfo],
    start_line: u32,
    fk: FrameKind,
) -> Option<usize> {
    let is_shell = crate::effect::effect_for(fk).is_shell;
    let mut ei = entries.partition_point(|e| entry_line(*e) < start_line);
    while ei < entries.len()
        && entry_line(entries[ei]) as usize == start_line as usize
        && entry_is_pop(entries[ei])
    {
        ei += 1;
    }
    let mut depth = 0i32;
    while ei < entries.len() {
        let e = entries[ei];
        if entry_is_body_mut(e) {
            ei += 1;
            continue;
        }
        let li = entry_line(e) as usize;
        let f = entry_frame(e);
        if f.kind() != fk {
            ei += 1;
            continue;
        }
        if entry_is_pop(e) {
            let is_poppush = is_poppush_kindonly(entries[ei], entries.get(ei + 1).copied());
            // 列表 sibling 替换 = 容器延续（PopPush）；BQ 等其他 shell 的
            // 同行 pop+push 是「关旧开新」（blank 重开）→ 真关闭。
            if is_poppush
                && (crate::effect::is_list_container(fk) || fk == FrameKind::TableRow)
            {
                depth = depth.max(1);
                ei += 2;
                continue;
            }
            if is_poppush && !is_shell {
                let pop_pl = lines.get(li).map(|p| p.node_type())
                    .unwrap_or(BlockType::Blank);
                return Some(end_for_rule(fk, li, pop_pl));
            }
            depth -= 1;
            if depth <= 0 && li >= start_line as usize {
                let pop_pl = lines.get(li).map(|p| p.node_type())
                    .unwrap_or(BlockType::Blank);
                return Some(end_for_rule(fk, li, pop_pl));
            }
        } else {
            depth += 1;
        }
        ei += 1;
    }
    None
}

/// 容器根尾的结算：优先 container_log_end；
/// None 时区分两种形态：
///  - 该 kind 存在任意条目（push/pop）→ 容器 EOF 仍开 → n
///  - 该 kind 无条目（如 TableRow 帧不入日志）→ 下一个事件行（或 n）
pub(crate) fn container_end_fallback(
    entries: &[u64],
    lines: &[PackedLineInfo],
    start_line: u32,
    fk: FrameKind,
    ei: usize,
    n: usize,
) -> usize {
    if let Some(end) = container_log_end(entries, lines, start_line, fk) {
        return end;
    }
    // TableRow 帧不镜像 Table 容器（稀疏记录），根尾由外层 Table pop 结算
    // → 用下一个事件行。
    if fk == FrameKind::TableRow {
        return if ei < entries.len() {
            crate::block::entry_line(entries[ei]) as usize
        } else {
            n
        };
    }
    let has_kind = entries.iter().any(|&x| {
        !crate::block::entry_is_body_mut(x)
            && crate::block::entry_frame(x).kind() == fk
            && (crate::block::entry_line(x) as usize) < n
    });
    if has_kind {
        // 该 kind 在范围内有条目（push 或 pop）且无真实关闭 → 容器 EOF 仍开 → n
        n
    } else if ei < entries.len() {
        crate::block::entry_line(entries[ei]) as usize
    } else {
        n
    }
}

/// zone 块级扩展（C5 迁入——深度列直读替代旧 depth_at 前缀重放）：
/// cs 回溯 cont 链 + Blank 延续（同层）；ce 前推 cont + setext 对。
pub fn expand_zone_to_blocks(
    lines: &[PackedLineInfo],
    depths: &[u8],
    n: usize,
    mut cs: usize,
    mut ce: usize,
) -> (usize, usize) {
    cs = cs.min(n);
    ce = ce.min(n);
    if cs > 0 && cs < n {
        let d = depths[cs] as usize;
        let mut s = cs;
        loop {
            if s == 0 {
                break;
            }
            let prev = &lines[s - 1];
            if prev.cont() && depths[s - 1] as usize == d {
                s -= 1;
                continue;
            }
            if prev.node_type() == BlockType::Blank {
                let mut p = s - 1;
                while p > 0 && lines[p - 1].node_type() == BlockType::Blank {
                    p -= 1;
                }
                if p > 0 {
                    let pt = lines[p - 1].node_type();
                    let decl = &crate::effect::BLOCK_TYPE_DECLS[pt as usize];
                    if decl.merges
                        && decl.blank_policy == crate::effect::BlankPolicy::Allow
                        && depths[p - 1] as usize == d
                    {
                        s -= 1;
                        continue;
                    }
                }
            }
            break;
        }
        if s < n && lines[s].cont() && s > 0 {
            s -= 1;
        }
        cs = s;
    }
    if ce > 0 && ce <= n {
        let idx = (ce - 1).min(n - 1);
        let d = depths[idx] as usize;
        let mut e = ce;
        while e < n {
            if lines[e].cont() && depths[e] as usize == d {
                e += 1;
                continue;
            }
            if lines[e].node_type() == BlockType::HsSep
                && !lines[e - 1].cont()
                && depths[e] as usize == d
            {
                e += 1;
                continue;
            }
            break;
        }
        ce = e;
    }
    (cs, ce)
}

/// FL 版块级扩展（生产形态）：不物化全量 depth 列。所有可能的
/// `depth_before` 查询先按 PL 形状收集成超集，排序去重后一次前向重放，
/// 语义循环内 O(1) 查表；避免每个边界独立重放 prefix entries。
pub fn expand_zone_to_blocks_fl(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    n: usize,
    mut cs: usize,
    mut ce: usize,
) -> (usize, usize) {
    cs = cs.min(n);
    ce = ce.min(n);

    let depth_at = |line: usize| -> usize {
        depths.get(line).copied().unwrap_or(0) as usize
    };

    if cs > 0 && cs < n {
        let d = depth_at(cs);
        let mut s = cs;
        loop {
            if s == 0 {
                break;
            }
            let prev = &lines[s - 1];
            if prev.cont() && depth_at(s - 1) == d {
                s -= 1;
                continue;
            }
            if prev.node_type() == BlockType::Blank {
                let mut p = s - 1;
                while p > 0 && lines[p - 1].node_type() == BlockType::Blank {
                    p -= 1;
                }
                if p > 0 {
                    let pt = lines[p - 1].node_type();
                    let decl = &crate::effect::BLOCK_TYPE_DECLS[pt as usize];
                    if decl.merges
                        && decl.blank_policy == crate::effect::BlankPolicy::Allow
                        && depth_at(p - 1) == d
                    {
                        s -= 1;
                        continue;
                    }
                }
            }
            break;
        }
        if s < n && lines[s].cont() && s > 0 {
            s -= 1;
        }
        cs = s;
    }
    if ce > 0 && ce <= n {
        let idx = (ce - 1).min(n - 1);
        let d = depth_at(idx);
        let mut e = ce;
        while e < n {
            if lines[e].cont() && depth_at(e) == d {
                e += 1;
                continue;
            }
            if lines[e].node_type() == BlockType::HsSep
                && !lines[e - 1].cont()
                && depth_at(e) == d
            {
                e += 1;
                continue;
            }
            break;
        }
        ce = e;
    }
    (cs, ce)
}

/// item 子树尾（Cell 区域的 span 机械——entries 深度跟踪）：
/// 从 item 行起，深度跌回 d0-1 的 Pop 行即子树尾（不含兄弟 items）；
/// 未弹 → 文档尾。
pub fn item_subtree_end(entries: &[u64], depths: &[u8], li: usize, n: usize) -> usize {
    let d0 = depths.get(li).copied().unwrap_or(0) as i32;
    let mut depth = d0 as i32;
    let mut ei = partition(entries, li as u32 + 1);
    while ei < entries.len() {
        let e = entries[ei];
        if entry_is_body_mut(e) {
            ei += 1;
            continue;
        }
        if entry_is_pop(e) {
            depth -= 1;
            if depth < d0 as i32 {
                return entry_line(e) as usize;
            }
        } else {
            depth += 1;
        }
        ei += 1;
    }
    n
}

/// 章节区间（Section 区域的 span 机械）：标题块列表 + level + bound
/// （标题容器 push_lines 下一行 / 记录 end）——secs = [start,
/// min(下一 level≤ 标题, bound))。
/// 标题只截断**同容器深度、同列表态**的 ≤ 级标题：BQ/item 内标题是
/// 该容器的局部章节，不截断外层（root 直系）章节。
pub fn section_intervals(
    blocks: &[BlockRec],
    records: &[ContainerRec],
    arena: &RecArena,
    heading: &[BlockType],
    n_lines: usize,
) -> Vec<(usize, usize, u8, BlockType)> {
    let heads = heading_meta(blocks, records, arena, heading, n_lines);
    section_intervals_from_heads(&heads, n_lines)
}

/// 标题元数据（`section_intervals` 的输入列持久化形态）：
/// 每个标题的 `start / level / block_type / container_depth / in_list / bound`。
/// `bound` = 容器 push_lines 下一行或记录 end；根级标题为文档尾。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeadingMeta {
    pub(crate) start: u32,
    pub(crate) level: u8,
    pub(crate) block_type: BlockType,
    pub(crate) depth: u8,
    pub(crate) in_list: bool,
    pub(crate) bound: u32,
    pub(crate) container_ref: u32,
}

/// 从块记录 + 容器记录物化标题元数据（与 `section_intervals` 同一公式）。
pub(crate) fn heading_meta(
    blocks: &[BlockRec],
    records: &[ContainerRec],
    arena: &RecArena,
    heading: &[BlockType],
    n_lines: usize,
) -> Vec<HeadingMeta> {
    // [微优化] 拓扑不认标题（CF：`heading_candidates = &[]`）时直接空返回——
    // 否则要白扫整段的块（实测 20k 块段 15.7µs → 0）。
    if heading.is_empty() {
        return Vec::new();
    }
    let container_depth = |bl: &BlockRec| -> usize {
        let mut d = 0usize;
        let mut rid = bl.container_ref;
        while rid != u32::MAX {
            d += 1;
            rid = records[rid as usize].parent;
        }
        d
    };
    let in_list = |bl: &BlockRec| -> bool {
        let mut rid = bl.container_ref;
        while rid != u32::MAX {
            let r = &records[rid as usize];
            if crate::effect::is_list_container(r.frame.kind()) {
                return true;
            }
            rid = r.parent;
        }
        false
    };
    blocks.iter()
        .filter(|bl| heading.contains(&bl.block_type))
        .map(|bl| {
            let bound = if bl.container_ref == u32::MAX {
                n_lines
            } else {
                let rec = &records[bl.container_ref as usize];
                rec.push_lines_of(arena)
                    .find(|&l| l > bl.start)
                    .map(|l| l as usize)
                    .unwrap_or_else(|| rec.end_usize().unwrap_or(n_lines))
            };
            HeadingMeta {
                start: bl.start,
                level: crate::block::header_level(bl.block_type).unwrap(),
                block_type: bl.block_type,
                depth: container_depth(bl) as u8,
                in_list: in_list(bl),
                bound: bound as u32,
                container_ref: bl.container_ref,
            }
        })
        .collect()
}

/// 由标题元数据计算章节区间（与 `section_intervals` 同一闭包规则）：
/// 下一同级/上级标题按 `level <= lv && depth == d && in_list == il` 截断，
/// 再与容器 bound 取 min。
pub(crate) fn section_intervals_from_heads(
    heads: &[HeadingMeta],
    n_lines: usize,
) -> Vec<(usize, usize, u8, BlockType)> {
    heads.iter().enumerate().map(|(i, h)| {
        let next = heads[i + 1..].iter()
            .find(|n| n.level <= h.level && n.depth == h.depth && n.in_list == h.in_list)
            .map(|n| n.start as usize)
            .unwrap_or(n_lines);
        (h.start as usize, next.min(h.bound as usize), h.level, h.block_type)
    }).collect()
}

/// CF 拓扑根计数（前缀热路径原子——[0, end_line) 内 depth-0 非 skip 根块数）。
/// 保持高效扫描实现；树只做测试预言机。
pub fn root_blocks_count_cf(
    entries: &[u64],
    lines: &[PackedLineInfo],
    rope: &crop::Rope,
    end_line: usize,
) -> usize {
    let n = end_line.min(lines.len());
    let mut count = 0usize;
    let mut depth = 0i32;
    let mut ei = 0usize;
    let mut skip_until = 0usize;  // 裸 setext(段落+分隔合并为 HsHead 块——计 1)
    for i in 0..n {
        if i < skip_until { continue; }
        let d_before = depth;
        let mut new_root_container = false;
        while ei < entries.len() && crate::block::entry_line(entries[ei]) as usize == i {
            let e = entries[ei];
            if !crate::block::entry_is_body_mut(e) {
                if crate::block::entry_is_pop(e) {
                    depth -= 1;
                } else {
                    let before = depth;
                    depth += 1;
                    if before == 0 {
                        let fk = crate::block::entry_frame(entries[ei]).kind();
                        let is_shell = crate::effect::effect_for(fk).is_shell;
                        let is_pp = is_shell
                            && crate::infer::poppush_is_list_replacement(entries, ei);
                        if !is_pp { new_root_container = true; }
                    }
                }
            }
            ei += 1;
        }
        if d_before != 0 {
            if depth == 0 {
                let t = lines[i].node_type();
                if !crate::effect::meta_is_skip(t)
                    && !(i > 0 && lines[i].cont())
                {
                    count += 1;
                }
            }
            if new_root_container {
                count += 1;
            }
            continue;
        }
        let t = lines[i].node_type();
        if t == BlockType::Paragraph && !(i > 0 && lines[i].cont()) {
            if !effective_para_at(lines, rope, i, n, None, None) {
                continue;
            }
            let j = crate::infer::paragraph_run_end(lines, i + 1, n);
            if j < n && lines[j].node_type() == BlockType::HsSep {
                count += 1;
                skip_until = j + 1;
                continue;
            }
            count += 1;
            continue;
        }
        if crate::effect::meta_is_skip(t) { continue; }
        if i > 0 && lines[i].cont() { continue; }
        count += 1;
    }
    count
}

/// Root 导航家族：CF root 块区间枚举与单块尾结算。
/// 这些函数只消费 PL/FL/rope，不依赖 Snapshot/Segment/cache，是 L3 导航层
/// 的通用原语。计数快路径由 root_blocks_count_cf 提供；枚举由
/// root_blocks_before 提供；块尾结算由 root_end_for 提供。

/// 结算一个 CF root 块的 [start,end) 尾。
/// `ei` 是 entries 中“当前行事件已折叠完”之后的游标；容器类块优先用
/// container_log_end，否则回退到下一个事件行或 n。
pub(crate) fn root_end_for(
    entries: &[u64],
    lines: &[PackedLineInfo],
    frame_log: &crate::block::FrameLog,
    i: usize,
    t: BlockType,
    n: usize,
    ei: usize,
) -> usize {
    if let Some(fk) = crate::block::frame_kind(t) {
        container_end_fallback(entries, lines, i as u32, fk, ei, n)
            .max(i + 1)
            .min(n)
    } else if crate::block::is_header(t) {
        i + 1
    } else {
        get_node_end_from_lines(lines, i, t, frame_log)
            .min(n)
            .max(i + 1)
    }
}

/// 枚举 [0, end_line) 内的 CF root 块区间 `(start, end, is_container, block_type)`。
/// 与 root_blocks_count_cf 使用同一套行级判定；长度必须等于计数结果。
/// 消费方：测试 oracle、HS 根计数推导、身份格 transition 枚举。
pub fn root_blocks_before(
    entries: &[u64],
    lines: &[PackedLineInfo],
    rope: &crop::Rope,
    frame_log: &crate::block::FrameLog,
    end_line: usize,
) -> Vec<(usize, usize, bool, BlockType)> {
    let n = end_line.min(lines.len());
    let mut out: Vec<(usize, usize, bool, BlockType)> = Vec::new();
    let mut depth = 0i32;
    let mut ei = 0usize;
    let mut skip_until = 0usize;
    let mut i = 0usize;
    while i < n {
        if i < skip_until {
            i += 1;
            continue;
        }
        let d_before = depth;
        let mut new_root = false;
        let mut new_root_kind: Option<FrameKind> = None;
        while ei < entries.len() && crate::block::entry_line(entries[ei]) as usize == i {
            let e = entries[ei];
            if !crate::block::entry_is_body_mut(e) {
                if crate::block::entry_is_pop(e) {
                    depth -= 1;
                } else {
                    let before = depth;
                    depth += 1;
                    if before == 0 {
                        let fk = crate::block::entry_frame(e).kind();
                        let is_shell = crate::effect::effect_for(fk).is_shell;
                        let is_pp = is_shell
                            && crate::infer::poppush_is_list_replacement(entries, ei);
                        if !is_pp {
                            new_root = true;
                            new_root_kind.get_or_insert(fk);
                        }
                    }
                }
            }
            ei += 1;
        }
        let t = lines[i].node_type();
        if d_before != 0 {
            if depth == 0
                && !crate::effect::meta_is_skip(t)
                && !(i > 0 && lines[i].cont())
            {
                let end = root_end_for(entries, lines, frame_log, i, t, n, ei);
                out.push((i, end.min(n).max(i + 1), crate::block::frame_kind(t).is_some(), t));
            }
            if new_root {
                if let Some(fk) = new_root_kind {
                    let end = container_end_fallback(entries, lines, i as u32, fk, ei, n);
                    out.push((i, end.min(n).max(i + 1), true, t));
                }
            }
            if depth == 0 {
                i += 1;
            } else {
                let next = if ei < entries.len() {
                    crate::block::entry_line(entries[ei]) as usize
                } else {
                    n
                };
                i = next.min(n).max(i + 1);
            }
            continue;
        }
        if t == BlockType::Paragraph && !(i > 0 && lines[i].cont()) {
            if !effective_para_at(lines, rope, i, n, None, None) {
                i += 1;
                continue;
            }
            let j = crate::infer::paragraph_run_end(lines, i + 1, n);
            if j < n && lines[j].node_type() == BlockType::HsSep {
                out.push((i, (j + 1).min(n), false, t));
                skip_until = j + 1;
                i = skip_until;
            } else {
                out.push((i, j, false, t));
                i = j;
            }
            continue;
        }
        if crate::effect::meta_is_skip(t) {
            i += 1;
            continue;
        }
        if i > 0 && lines[i].cont() {
            i += 1;
            continue;
        }
        let be = root_end_for(entries, lines, frame_log, i, t, n, ei);
        out.push((i, be.min(n).max(i + 1), crate::block::frame_kind(t).is_some(), t));
        while ei < entries.len()
            && (crate::block::entry_line(entries[ei]) as usize) < be.min(n)
        {
            let e = entries[ei];
            if !crate::block::entry_is_body_mut(e) {
                if crate::block::entry_is_pop(e) {
                    depth -= 1;
                } else {
                    depth += 1;
                }
            }
            ei += 1;
        }
        i = be.min(n).max(i + 1);
    }
    out
}


/// locate——区间模式：含行块（块按 start 有序且不重叠——最后一个
/// start<=line 且 end>line）。
#[inline]
pub fn locate_block(blocks: &[BlockRec], line: usize) -> Option<usize> {
    let idx = blocks.partition_point(|b| (b.start as usize) <= line);
    if idx == 0 {
        None
    } else {
        let b = &blocks[idx - 1];
        ((b.end as usize) > line).then_some(idx - 1)
    }
}

// 路径分量 = 树孩子位置（incr::ast_path_of——含 ItemSegments 槽位
// 与 item 值 p 前缀），不是 ContainerRec.ci 的 **ci 轴**（按深度的全局容器计数器，
// 跨父容器共享——非父 children 序）；两者在"同深度多父各带容器"时分叉
// （对账判官 `tests/unit/infer.rs::ci_axis_differs_from_parent_relative_index` 已钉死）。
// 生产唯一的 align 容器路径消费者（incr::align_container_path）已改走树轴。


// §4 块树（归属树）


// ── 文本辅助（node 面消费）──

/// 段落收束（CM §4.8——逐行剥前导缩进，尾换行保留）。
pub fn paragraph_text_normalize(raw: &str) -> String {
    if !raw.contains('\n') {
        return raw.trim_start().to_string();
    }
    let mut out = String::with_capacity(raw.len());
    let mut first = true;
    for l in raw.lines() {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(l.trim_start());
    }
    out
}

/// 容器前缀交替剥除（CM §5.2 as raw md reparse）：peel 层序列——
/// BQ 无条件剥、marker 仅当下一层是 BQ 才剥（邻接谓词）。
/// 行前栈＋行开层条目的单遍前向重放（8.3 写面三连共用——#8/#10/#11 的 ctx 供给）。
/// 调用方按行序推进；栈只在 <k 的条目上重放、行条目读用临时游标（8.2 双游标教训）。
pub struct LineCtxReplay<'a> {
    entries: &'a [u64],
    stack: Vec<crate::block::BlockFrame>,
    // 统一坐标恢复（A）：栈重放走 fold_replay_entry 机械单核——锚/开行
    // 与栈同步维护（前缀律只用 kind+indent，两 Vec 成本换真单核）。
    anchors: Vec<Option<usize>>,
    open_lines: Vec<u32>,
    applied: usize,
    next_line: u32,
}

impl<'a> LineCtxReplay<'a> {
    pub fn new(entries: &'a [u64], start_line: u32) -> Self {
        Self {
            entries,
            stack: Vec::new(),
            anchors: Vec::new(),
            open_lines: Vec::new(),
            applied: 0,
            next_line: start_line,
        }
    }

    /// 段内折叠器（O(n²) 根因修复）：跳过 [0, start_line) 的全部条目——栈由外部
    /// 零重放供给（chain_row），applied 直接二分跳到 start_line。段落行无容器
    /// 事件，段内折叠量≈0。
    pub fn with_stack(
        entries: &'a [u64],
        start_line: u32,
        stack: Vec<crate::block::BlockFrame>,
    ) -> Self {
        let applied = entries.partition_point(|&e| crate::block::entry_line(e) < start_line);
        Self {
            entries,
            stack,
            anchors: Vec::new(),
            open_lines: Vec::new(),
            applied,
            next_line: start_line,
        }
    }

    /// 行 k 的（行前栈, 行开层条目）。行序推进；跳行=栈推进行但不读条目。
    pub fn line_ctx(
        &mut self,
        k: u32,
    ) -> (&[crate::block::BlockFrame], smallvec::SmallVec<[u64; 4]>) {
        debug_assert!(k >= self.next_line, "LineCtxReplay 行序推进契约");
        // 机械单源（统一项恢复 A）：事件折叠调 fold_replay_entry——
        // body_mut kind+indent 改写 / pop 弹三栈 / push 入栈+锚+开行恢复——
        // 与投影层 ShadowReplay 同核。body_mut
        // 只动 body 位（kind/indent 不动）——前缀律无感知，双钉背书行为中性。
        while self.applied < self.entries.len()
            && crate::block::entry_line(self.entries[self.applied]) < k
        {
            crate::block::fold_replay_entry(
                self.entries[self.applied],
                &mut self.stack,
                &mut self.anchors,
                &mut self.open_lines,
            );
            self.applied += 1;
        }
        let mut pushes: smallvec::SmallVec<[u64; 4]> = smallvec::SmallVec::new();
        let mut j = self.applied;
        while j < self.entries.len() && crate::block::entry_line(self.entries[j]) == k {
            let e = self.entries[j];
            if !crate::block::entry_is_body_mut(e) && !crate::block::entry_is_pop(e) {
                pushes.push(e);
            }
            j += 1;
        }
        self.next_line = k + 1;
        (&self.stack, pushes)
    }
}

/// 段落归一剥层：逐行 (All, Chain) 折叠——BQ 恒剥、
/// 列表层按链/行条目供数（Adjacency 的 ctx 版——其消费者都有 ctx：
/// incr/projection/normalized_content 均持 entries）。零重解析——prefix_cut
/// 副产品；单遍前向重放（O(条目)），行前栈＋行条目逐行供给。
pub fn paragraph_lines_stripped(
    raw: &str,
    entries: &[u64],
    start_line: u32,
    start_stack: &[crate::block::BlockFrame],
) -> String {
    // 选择单源（决策 2 接线）：R3 段落归一 = (All, Chain)——prefix_policy 表行。
    let p = crate::effect::prefix_policy(
        crate::block::BlockType::Paragraph,
        crate::effect::PrefixFace::Content,
        crate::effect::PrefixDir::Read,
    );
    // O(n²) 根因修复：链由外部零重放供给（view.chain_row），重放器跳过段前条目。
    let mut replay = LineCtxReplay::with_stack(entries, start_line, start_stack.to_vec());
    let mut out = String::with_capacity(raw.len());
    let mut first = true;
    for (i, l) in raw.lines().enumerate() {
        if !first {
            out.push('\n');
        }
        first = false;
        let k = start_line + i as u32;
        let (stack, pushes) = replay.line_ctx(k);
        let cut = prefix_cut(
            l,
            stack,
            &pushes,
            p.kinds,
            p.stop,
            p.lead,
            p.tail,
        )
        .bytes;
        out.push_str(&l[cut..]);
    }
    out
}

/// colon tag 分隔符宽度：`:` 后空格/tab → 2（`: `）；否则 1。
#[inline]
pub fn colon_sep_len(sep: u8, after: u8) -> usize {
    crate::block::colon_sep_len(sep, after)
}

/// fence 开行的缩进基准（CM §4.5 内容行 dedent）：剥容器前缀后的
/// 内容缩进 + 前缀宽度（item 内容 fence / BQ 内 fence / 顶层 0）。
pub fn fence_indent_base(line: &str) -> usize {
    let stripped = crate::block::peel(line).1;
    let prefix_bytes = line.len() - stripped.len();
    let prefix_cols = crate::block::column_width(&line.as_bytes()[..prefix_bytes]);
    crate::block::compute_indent(stripped.as_bytes()) + prefix_cols
}

/// item 内容段续行归一：逐行剥 BQ 前缀（保留缩进）+ 内容列 dedent，
/// 尾换行保留（OL 的 ct="a\n" 语义）。
/// 8.5（#6）：逐行剥离走 prefix_cut((BqOnly,None,None)) 折叠——与
/// rest_after_bq_keep_indent 同口径（BQ 层到首个非 BQ、不跳尾空白——钉⑤背书）。
/// 选择单源注（决策 2）：自由函数无 (t,face,dir) 键语境——dedent 域自持
/// 字面量（同 dedent_cols_for 先例——参数=(BqOnly,None,SkipWs,None)）。
pub fn dedent_continuation_bq(s: &str, content_col: usize) -> String {
    use crate::effect::{PrefixKinds, PrefixLead, PrefixStop, PrefixTail};
    let trailing_nl = s.ends_with('\n');
    let mut out = String::with_capacity(s.len());
    let mut first = true;
    for l in s.lines() {
        if !first {
            out.push('\n');
        }
        first = false;
        let cut = prefix_cut(
            l,
            &[],
            &[],
            PrefixKinds::BqOnly,
            PrefixStop::None,
            PrefixLead::SkipWs,
            PrefixTail::None,
        )
        .bytes;
        out.push_str(dedent_columns(&l[cut..], content_col));
    }
    if trailing_nl {
        out.push('\n');
    }
    out
}

/// 列感知 dedent：剥 ≤ max_col 列的前导空白（先检查后前进——
/// 部分 tab 不剥）。tab 展开走测量层唯一函数（block::tab_width）。
pub fn dedent_columns(s: &str, max_col: usize) -> &str {
    let bytes = s.as_bytes();
    let mut col = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b' ' => {
                if col + 1 > max_col {
                    break;
                }
                col += 1;
                i += 1;
            }
            b'\t' => {
                let step = crate::block::tab_width(col);
                if col + step > max_col {
                    break;
                }
                col += step;
                i += 1;
            }
            _ => break,
        }
    }
    &s[i..]
}


/// V2：cache_compact_v2 阶段 3 形态——标量列 + flags 位打包 + children
/// Range32 指向 AttrTreeNew.child_arena。rec/blk 用 u32::MAX 哨兵。
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub kind: Option<FrameKind>,
    pub block_type: BlockType,
    /// bit0=is_item bit1=implicit bit2=loose。
    pub flags: u8,
    pub depth: u8,
    pub seg_depth: u8,
    pub start: u32,
    pub end: u32,
    pub content_end: u32,
    /// u32::MAX 哨兵（对应原 rec: Option<usize> 的 None）。
    pub rec: u32,
    /// u32::MAX 哨兵（对应原 blk: Option<usize> 的 None）。
    pub blk: u32,
    /// 指向 AttrTreeNew.child_arena 的子区间。
    pub children: Range32,
}

pub const REC_NONE: u32 = u32::MAX;
pub const BLK_NONE: u32 = u32::MAX;

impl TreeNode {
    pub const FLAG_IS_ITEM: u8 = 1;
    pub const FLAG_IMPLICIT: u8 = 1 << 1;
    pub const FLAG_LOOSE: u8 = 1 << 2;

    #[inline]
    pub fn is_item(&self) -> bool {
        self.flags & Self::FLAG_IS_ITEM != 0
    }
    #[inline]
    pub fn implicit(&self) -> bool {
        self.flags & Self::FLAG_IMPLICIT != 0
    }
    #[inline]
    pub fn loose(&self) -> bool {
        self.flags & Self::FLAG_LOOSE != 0
    }
    #[inline]
    pub fn set_flags(&mut self, is_item: bool, implicit: bool, loose: bool) {
        self.flags = 0;
        if is_item {
            self.flags |= Self::FLAG_IS_ITEM;
        }
        if implicit {
            self.flags |= Self::FLAG_IMPLICIT;
        }
        if loose {
            self.flags |= Self::FLAG_LOOSE;
        }
    }

    /// rec: u32::MAX → None（记录索引解包）。
    #[inline]
    pub fn rec_idx(&self) -> Option<usize> {
        if self.rec == REC_NONE {
            None
        } else {
            Some(self.rec as usize)
        }
    }
    /// blk: u32::MAX → None（块索引解包）。
    #[inline]
    pub fn blk_idx(&self) -> Option<usize> {
        if self.blk == BLK_NONE {
            None
        } else {
            Some(self.blk as usize)
        }
    }
    #[inline]
    pub fn children_of_arena<'t>(&self, arena: &'t [u32]) -> &'t [u32] {
        &arena[self.children.off as usize..(self.children.off + self.children.len) as usize]
    }
    #[inline]
    pub fn children_of<'t>(&self, tree: &'t AttrTreeNew) -> &'t [u32] {
        self.children_of_arena(&tree.child_arena)
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        rec: u32,
        blk: u32,
        kind: Option<FrameKind>,
        block_type: BlockType,
        start: u32,
        end: u32,
        is_item: bool,
        implicit: bool,
        seg_depth: u8,
        loose: bool,
        depth: u8,
    ) -> Self {
        let mut node = TreeNode {
            kind,
            block_type,
            flags: 0,
            depth,
            seg_depth,
            start,
            end,
            content_end: end,
            rec,
            blk,
            children: Range32::default(),
        };
        node.set_flags(is_item, implicit, loose);
        node
    }
}

/// 树（节点 + 子节点 arena + 根）。
#[derive(Debug, Clone)]
pub struct AttrTreeNew {
    pub nodes: Vec<TreeNode>,
    pub child_arena: Vec<u32>,
    pub roots: Vec<u32>,
}

impl AttrTreeNew {
    /// 结构同一性（树 V2 无 derived PartialEq/Eq——逐节点比较标量 +
    /// children Range32 + 对应 arena 切片）。
    pub fn structurally_eq(&self, other: &AttrTreeNew) -> bool {
        tree_nodes_structurally_eq(
            &self.nodes,
            &self.child_arena,
            &other.nodes,
            &other.child_arena,
        ) && self.roots == other.roots
    }
}

/// 逐节点结构同一性（nodes + arena 双参——供 AttrTreeNew::structurally_eq
/// 及外部两树比较）。
pub fn tree_nodes_structurally_eq(
    a: &[TreeNode],
    arena_a: &[u32],
    b: &[TreeNode],
    arena_b: &[u32],
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    for (na, nb) in a.iter().zip(b.iter()) {
        if na.kind != nb.kind
            || na.block_type != nb.block_type
            || na.flags != nb.flags
            || na.depth != nb.depth
            || na.seg_depth != nb.seg_depth
            || na.start != nb.start
            || na.end != nb.end
            || na.content_end != nb.content_end
            || na.rec != nb.rec
            || na.blk != nb.blk
            || na.children.off != nb.children.off
            || na.children.len != nb.children.len
        {
            return false;
        }
        if na.children_of_arena(arena_a) != nb.children_of_arena(arena_b) {
            return false;
        }
    }
    true
}

/// 渲染结构判定（JSON renderer 与 MD renderer 共享的唯一真相）。
///
/// 只描述 AttrTree 节点在输出层的结构类别，不写字节、不读内容、不分配。
/// 位置在 TreeNode/AttrTreeNew 旁边：infer 已依赖 topology/effect，
/// incr 已依赖 infer，JSON/MD 都从 crate::infer::render_shape 取谓词，
/// 不新增文件、不引入反向依赖。
pub(crate) mod render_shape {
    use super::TreeNode;
    use crate::block::{BlockType, FrameKind};
    use crate::topology::{ShellDecl, Topology};

    /// render_node 的五个结构分支；MD 用它做 root 覆盖/缩进，JSON 只调用谓词。
    #[derive(Clone, Copy, Debug)]
    pub(crate) enum NodeShape {
        Transparent,
        Container {
            fk: FrameKind,
            shell: &'static ShellDecl,
            allows_children: bool,
        },
        Item,
        Section {
            shell: &'static ShellDecl,
        },
        Leaf {
            skip: bool,
            tight_para: bool,
        },
    }

    /// TableRow 记录节点（无 blk）透明：不写壳，children 上提到父数组。
    #[inline]
    pub(crate) fn is_table_row_transparent(n: &TreeNode) -> bool {
        matches!(n.kind, Some(FrameKind::TableRow)) && n.blk_idx().is_none()
    }

    /// HtmlBlock 的元素化 children 不进 JSON 通道（防第二个 ct 键）。
    #[inline]
    pub(crate) fn allows_children(fk: FrameKind) -> bool {
        fk != FrameKind::HtmlBlock
    }

    /// 只有 kind=None 的 item 走 ItemLeaf；kind=Some 的 TableRow 等仍走容器壳。
    #[inline]
    pub(crate) fn is_item_branch(n: &TreeNode) -> bool {
        n.kind.is_none() && n.is_item()
    }

    /// 章节壳：kind=None、非 item、块类型在 topology.shells_by_block 中。
    #[inline]
    pub(crate) fn section_shell(
        topology: &Topology,
        n: &TreeNode,
    ) -> Option<&'static ShellDecl> {
        if n.kind.is_none() && !n.is_item() {
            topology.shell_by_block(n.block_type)
        } else {
            None
        }
    }

    /// 块叶 skip 集：Blank/CodeContent/HtmlContent/TableSeparator 等。
    #[inline]
    pub(crate) fn is_skip_leaf(block_type: BlockType) -> bool {
        crate::effect::meta_is_skip(block_type)
    }

    /// tight item 内段落（CM §5.2）：不包 p，inline 直写。
    #[inline]
    pub(crate) fn is_tight_para(parent_item: bool, n: &TreeNode) -> bool {
        parent_item && n.block_type == BlockType::Paragraph && !n.loose()
    }

    /// 组合谓词；顺序必须与 incr::render_node 完全一致。
    #[inline]
    pub(crate) fn shape(
        n: &TreeNode,
        topology: &Topology,
        parent_item: bool,
    ) -> NodeShape {
        if is_table_row_transparent(n) {
            return NodeShape::Transparent;
        }
        if let Some(fk) = n.kind {
            return NodeShape::Container {
                fk,
                shell: topology.shell_for(fk),
                allows_children: allows_children(fk),
            };
        }
        if is_item_branch(n) {
            return NodeShape::Item;
        }
        if let Some(shell) = section_shell(topology, n) {
            return NodeShape::Section { shell };
        }
        NodeShape::Leaf {
            skip: is_skip_leaf(n.block_type),
            tight_para: is_tight_para(parent_item, n),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::block::{BlockType, FrameKind};
        use crate::infer::{Range32, TreeNode};
        use crate::topology::{CONTAINER_TOPOLOGY, HEADINGSECTION_TOPOLOGY};

        fn tn(kind: Option<FrameKind>, bt: BlockType, item: bool, loose: bool) -> TreeNode {
            let mut n = TreeNode {
                kind,
                block_type: bt,
                flags: 0,
                depth: 0,
                seg_depth: 0,
                start: 0,
                end: 1,
                content_end: 1,
                rec: u32::MAX,
                blk: u32::MAX,
                children: Range32 { off: 0, len: 0 },
            };
            n.set_flags(item, false, loose);
            n
        }

        #[test]
        fn predicates_match_render_node_conditions() {
            let tr = tn(Some(FrameKind::TableRow), BlockType::TableRow, false, false);
            assert!(is_table_row_transparent(&tr));
            assert!(!is_table_row_transparent(&tn(Some(FrameKind::BulletList), BlockType::Bullet, false, false)));
            let mut tr_blk = tr;
            tr_blk.blk = 0;
            assert!(!is_table_row_transparent(&tr_blk));

            assert!(!allows_children(FrameKind::HtmlBlock));
            assert!(allows_children(FrameKind::BulletList));

            assert!(is_item_branch(&tn(None, BlockType::Bullet, true, false)));
            assert!(!is_item_branch(&tn(None, BlockType::Bullet, false, false)));
            assert!(!is_item_branch(&tn(Some(FrameKind::BulletList), BlockType::Bullet, true, false)));

            let h = tn(None, BlockType::Header1, false, false);
            assert!(section_shell(&HEADINGSECTION_TOPOLOGY, &h).is_some());
            assert!(section_shell(&CONTAINER_TOPOLOGY, &h).is_none());
            assert!(section_shell(&HEADINGSECTION_TOPOLOGY, &tn(None, BlockType::Paragraph, false, false)).is_none());

            assert!(is_skip_leaf(BlockType::Blank));
            assert!(!is_skip_leaf(BlockType::Paragraph));

            assert!(is_tight_para(true, &tn(None, BlockType::Paragraph, false, false)));
            assert!(!is_tight_para(false, &tn(None, BlockType::Paragraph, false, false)));
            assert!(!is_tight_para(true, &tn(None, BlockType::Paragraph, false, true)));
        }

        #[test]
        fn shape_order_and_variants() {
            let table = shape(&tn(Some(FrameKind::BulletList), BlockType::Bullet, false, false),
                &CONTAINER_TOPOLOGY, false);
            assert!(matches!(table, NodeShape::Container { .. }));
            let transparent = shape(&tn(Some(FrameKind::TableRow), BlockType::TableRow, false, false),
                &CONTAINER_TOPOLOGY, false);
            assert!(matches!(transparent, NodeShape::Transparent));
            let item = shape(&tn(None, BlockType::Bullet, true, false), &CONTAINER_TOPOLOGY, false);
            assert!(matches!(item, NodeShape::Item));
            let section = shape(&tn(None, BlockType::Header1, false, false),
                &HEADINGSECTION_TOPOLOGY, false);
            assert!(matches!(section, NodeShape::Section { .. }));
            let leaf = shape(&tn(None, BlockType::Paragraph, false, false),
                &CONTAINER_TOPOLOGY, false);
            assert!(matches!(leaf, NodeShape::Leaf { skip: false, tight_para: false }));
            let skip = shape(&tn(None, BlockType::Blank, false, false),
                &CONTAINER_TOPOLOGY, false);
            assert!(matches!(skip, NodeShape::Leaf { skip: true, .. }));
        }
    }
}

/// 树构建器（对照 AttrBuilder 同构——五步：expand / attach / segment /
/// comment / 章节）。
struct TreeBuilder<'a> {
    records: &'a [ContainerRec],
    blocks: &'a [BlockRec],
    view: &'a LineView<'a>,
    arena: &'a RecArena,
    ze: usize,
    topology: &'a crate::topology::Topology,
    nodes: Vec<TreeNode>,
    /// 构建期 children 临时结构（与 nodes 平行；末尾物化进 child_arena）。
    tmp_children: Vec<Vec<u32>>,
    rec_to_node: Vec<Option<usize>>,
    /// `segment` 的临时缓冲（跨调用复用；nesting 上 segment 被调 ~1.59M 次，
    /// 实测 `tr_segment` 285ms）。
    scr_pl: Vec<u32>,
    scr_seg_counts: Vec<usize>,
    scr_seg_of: Vec<usize>,
    scr_seg_offsets: Vec<usize>,
    scr_seg_flat: Vec<u32>,
    scr_seg_cursor: Vec<usize>,
}

impl<'a> TreeBuilder<'a> {
    fn new(
        records: &'a [ContainerRec],
        blocks: &'a [BlockRec],
        view: &'a LineView<'a>,
        arena: &'a RecArena,
        ze: usize,
        topology: &'a crate::topology::Topology,
    ) -> Self {
        TreeBuilder {
            records,
            blocks,
            view,
            arena,
            ze,
            topology,
            nodes: Vec::new(),
            tmp_children: Vec::new(),
            rec_to_node: vec![None; records.len()],
            scr_pl: Vec::new(),
            scr_seg_counts: Vec::new(),
            scr_seg_of: Vec::new(),
            scr_seg_offsets: Vec::new(),
            scr_seg_flat: Vec::new(),
            scr_seg_cursor: Vec::new(),
        }
    }

    /// 追加节点并维护平行 tmp_children（返回节点索引）。
    fn push_node(&mut self, nd: TreeNode) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(nd);
        self.tmp_children.push(Vec::new());
        idx
    }

    /// 步骤 1：容器展开（透明穿透）——透明容器不建节点，children 上提。
    fn expand(&mut self, rid: usize, parent_node: Option<usize>) -> Option<usize> {
        let r = &self.records[rid];
        let candidate = self.topology.has_rule(
            r.frame.kind(),
            crate::topology::CandidateRule::ContainerRecord,
        );
        let transparent = !candidate
            || crate::effect::shell_transparent(self.view, r.start as usize, r.frame.kind());
        let node = if transparent {
            None
        } else {
            let depth = parent_node
                .map(|p| self.nodes[p].depth.saturating_add(1))
                .unwrap_or(0);
            let end = r.end_usize().unwrap_or(self.ze);
            let nd = TreeNode::new(
                rid as u32,
                BLK_NONE,
                Some(r.frame.kind()),
                crate::effect::container_block_type(r.frame.kind()),
                r.start,
                end as u32,
                false,
                false,
                0,
                r.frame.loose(),
                depth,
            );
            let idx = self.push_node(nd);
            Some(idx)
        };
        self.rec_to_node[rid] = node;
        // 先拷贝 children（NLL 陷阱：迭代器借用跨递归调用会提前终止）。
        let kids: Vec<u32> = r.children_of(self.arena).collect();
        for &c in &kids {
            let child_node = self.expand(c as usize, node.or(parent_node));
            if let (Some(pn), Some(cn)) = (node, child_node) {
                self.tmp_children[pn].push(cn as u32);
            }
        }
        node
    }

    /// 容器记录链上最近的非透明祖先节点（块挂接用——链顶透明时上提）。
    fn nearest_opaque(&self, mut rid: usize) -> Option<usize> {
        loop {
            if let Some(n) = self.rec_to_node[rid] {
                return Some(n);
            }
            let parent = self.records[rid].parent;
            if parent == u32::MAX {
                return None;
            }
            rid = parent as usize;
        }
    }

    /// 步骤 2：children 归并（块挂 container_ref 链顶 → 最近非透明祖先）。
    fn attach_blocks(&mut self) -> Vec<usize> {
        let mut roots = Vec::new();
        for bi in 0..self.blocks.len() {
            let b = &self.blocks[bi];
            let bt = b.block_type;
            if !self.topology.block_candidates.is_empty()
                && !self.topology.block_candidates.contains(&bt)
            {
                continue;
            }
            if b.container_ref != u32::MAX {
                let r = &self.records[b.container_ref as usize];
                if crate::effect::decl_for(r.frame.kind()).has_loose
                    && crate::effect::block_decl(b.block_type).is_list_item
                    && self.topology.has_rule(
                        r.frame.kind(),
                        crate::topology::CandidateRule::ItemSegments,
                    )
                {
                    continue;
                }
                let opaque = self.rec_to_node[b.container_ref as usize].is_some();
                if opaque
                    && !crate::effect::effect_for(r.frame.kind()).is_shell
                    && r.start == b.start
                    && r.frame.kind() != FrameKind::TableRow
                {
                    if let Some(t) = self.rec_to_node[b.container_ref as usize] {
                        self.nodes[t].blk = bi as u32;
                        self.nodes[t].content_end = b.end;
                    }
                    continue;
                }
            }
            let nd = TreeNode::new(
                REC_NONE,
                bi as u32,
                None,
                bt,
                b.start,
                b.end,
                false,
                b.implicit(),
                0,
                b.loose(),
                0,
            );
            let idx = self.push_node(nd);
            if b.container_ref == u32::MAX {
                roots.push(idx);
            } else {
                match self.nearest_opaque(b.container_ref as usize) {
                    Some(t) => self.tmp_children[t].push(idx as u32),
                    None => roots.push(idx),
                }
            }
        }
        roots
    }

    /// 容器 children 全序恢复——expand(①)先挂容器、attach(②)后挂块,
    /// 同父 children 为类别序(容器全体在块前),非文档序;稳定排序按
    /// start 归并回文档序。幂等:segment 已排节点/透明上提批序已有序,
    /// 重排无操作;同 start 兄弟保持挂载序(稳定语义,零变化)。CF/Bf
    /// 无章节门(步骤⑤跳过),此即最终兜底;HS 在章节 reorg 后(节点
    /// 移动)须再调用一次——两处并存幂等。
    fn sort_children_all(&mut self) {
        for i in 0..self.nodes.len() {
            if self.tmp_children[i].len() > 1 {
                let kids = std::mem::take(&mut self.tmp_children[i]);
                let mut pairs: Vec<(u32, u32)> =
                    kids.iter().map(|&c| (self.nodes[c as usize].start, c)).collect();
                pairs.sort();
                self.tmp_children[i] = pairs.into_iter().map(|(_, c)| c).collect();
            }
        }
    }

    /// 步骤 3：分段（has_loose 容器 → ItemLeaf 段；TableRow → 行 Shell 段）。
    fn segment(&mut self, node_idx: usize) {
        let kind = self.nodes[node_idx].kind;
        let rec = self.nodes[node_idx].rec_idx();
        let depth = self.nodes[node_idx].depth;
        let Some(rec) = rec else { return; };
        // 临时缓冲跨调用复用（scr_*）。
        let mut pl = std::mem::take(&mut self.scr_pl);
        pl.clear();
        pl.extend(self.records[rec].push_lines_of(self.arena));
        if pl.is_empty() {
            self.scr_pl = pl;
            return;
        }
        let loose = self.nodes[node_idx].loose();
        let children = std::mem::take(&mut self.tmp_children[node_idx]);
        let mut kids: Vec<u32> = children;
        kids.sort_by_key(|&c| self.nodes[c as usize].start);
        // P3-B：segs 摊平（offsets+flat 替代每段一个 Vec），段分配按
        // pl 的 partition 顺序计数一次，再游标填充。sort 保留稳定序
        // （同 start 兄弟存在——稳定语义不能换 unstable）。
        let mut seg_counts = std::mem::take(&mut self.scr_seg_counts);
        seg_counts.clear();
        seg_counts.resize(pl.len(), 0);
        let mut seg_of = std::mem::take(&mut self.scr_seg_of);
        seg_of.clear();
        seg_of.resize(kids.len(), 0);
        let mut seg_idx = 0usize;
        for (ci, &c) in kids.iter().enumerate() {
            let cs = self.nodes[c as usize].start;
            while seg_idx + 1 < pl.len() && pl[seg_idx + 1] <= cs {
                seg_idx += 1;
            }
            seg_of[ci] = seg_idx;
            seg_counts[seg_idx] += 1;
        }
        let mut seg_offsets = std::mem::take(&mut self.scr_seg_offsets);
        seg_offsets.clear();
        seg_offsets.resize(pl.len() + 1, 0);
        for j in 0..pl.len() {
            seg_offsets[j + 1] = seg_offsets[j] + seg_counts[j];
        }
        let mut seg_flat = std::mem::take(&mut self.scr_seg_flat);
        seg_flat.clear();
        seg_flat.resize(kids.len(), 0);
        let mut seg_cursor = std::mem::take(&mut self.scr_seg_cursor);
        seg_cursor.clear();
        seg_cursor.extend_from_slice(&seg_offsets);
        for (ci, &c) in kids.iter().enumerate() {
            let s = seg_of[ci];
            seg_flat[seg_cursor[s]] = c;
            seg_cursor[s] += 1;
        }
        let mut new_children: Vec<u32> = Vec::new();
        // blocks 查找：**每次调用一次二分定位 + 调用内单调游标**
        // （pl 升序、blocks 按 start 升序）。
        let mut bp = match pl.first() {
            Some(&l0) => self.blocks.partition_point(|b| (b.start as u32) < l0),
            None => 0,
        };
        for (j, &l) in pl.iter().enumerate() {
            while bp < self.blocks.len() && (self.blocks[bp].start as u32) < l {
                bp += 1;
            }
            let content = {
                let lo = bp;
                self.blocks[lo..]
                    .iter()
                    .take_while(|b| b.start == l)
                    .enumerate()
                    .find(|(_, b)| {
                        crate::effect::block_decl(b.block_type).is_list_item
                            || (kind == Some(FrameKind::TableRow)
                                && b.block_type == BlockType::TableRow)
                    })
                    .map(|(bi, b)| (lo + bi, b))
            };
            let members = &seg_flat[seg_offsets[j]..seg_offsets[j + 1]];
            let content_blk = content.map(|(bi, _)| bi);
            let seg_children: Vec<u32> = if content_blk.is_some() {
                members
                    .iter()
                    .filter(|&&c| self.nodes[c as usize].blk_idx() != content_blk)
                    .copied()
                    .collect()
            } else {
                members.to_vec()
            };
            let seg_end = content
                .map(|(_, b)| b.end)
                .unwrap_or_else(|| {
                    pl.get(j + 1)
                        .copied()
                        .unwrap_or_else(|| self.records[rec].end_usize().unwrap_or(self.ze) as u32)
                });
            let is_table_row = kind == Some(FrameKind::TableRow);
            let seg_loose = content.map(|(_, b)| b.loose()).unwrap_or(loose);
            let seg_bt = if is_table_row {
                BlockType::TableRow
            } else {
                crate::effect::container_block_type(kind.unwrap())
            };
            let nd = TreeNode::new(
                if is_table_row { rec as u32 } else { REC_NONE },
                content_blk.map_or(BLK_NONE, |bi| bi as u32),
                None,
                seg_bt,
                l,
                seg_end,
                !is_table_row,
                false,
                depth,
                seg_loose,
                depth.saturating_add(1),
            );
            let idx = self.push_node(nd);
            self.tmp_children[idx] = seg_children;
            new_children.push(idx as u32);
        }
        self.tmp_children[node_idx] = new_children;
        // 归还临时缓冲（下一节点继续用）。
        self.scr_pl = pl;
        self.scr_seg_counts = seg_counts;
        self.scr_seg_of = seg_of;
        self.scr_seg_offsets = seg_offsets;
        self.scr_seg_flat = seg_flat;
        self.scr_seg_cursor = seg_cursor;
    }
}

/// 章节壳父查找（覆盖章节壳区间 [start, e) 的父章节——最近 start 且
/// e <= e2——排除自己）。
pub fn find_parent_section(
    sec_nodes: &[(usize, usize, u8, usize)],
    start: usize,
    e: usize,
    self_idx: usize,
) -> Option<usize> {
    let sec_pos = sec_nodes.partition_point(|&(s, _, _, _)| s <= start);
    for i in (0..sec_pos).rev() {
        let (_, e2, _, idx2) = sec_nodes[i];
        if idx2 != self_idx && e <= e2 {
            return Some(idx2);
        }
    }
    None
}

/// 覆盖区间 [as, ae) 的最深章节（最近覆盖 = 最深——嵌套章节 start 更大）。
pub fn deepest_section(
    secs: &[(usize, usize, u8, usize)],
    as_: usize,
    ae: usize,
) -> Option<usize> {
    let idx = secs.partition_point(|&(s, _, _, _)| s <= as_);
    for i in (0..idx).rev() {
        let (_, e, _, node_idx) = secs[i];
        if ae <= e {
            return Some(node_idx);
        }
    }
    None
}

/// 单层 children 重组（章节归属——最深覆盖章节；标题块 → 章节壳；
/// exempt 父链保持原位防环；ItemLeaf 随父）。
fn reorg_children(
    b: &mut TreeBuilder,
    sec_nodes: &[(usize, usize, u8, usize)],
    sec_by_start: &std::collections::HashMap<usize, (usize, usize)>,
    exempt: &HashSet<usize>,
    kids: Vec<usize>,
    keep_atoms: bool,
) -> Vec<usize> {
    let heading = b.topology.heading_candidates;
    let mut new_kids: Vec<usize> = Vec::new();
    for &c in &kids {
        let (c_start, c_end) = {
            let n = &b.nodes[c];
            (n.start as usize, n.end as usize)
        };
        let (c_heading, c_shell) = {
            let n = &b.nodes[c];
            (
                heading.contains(&n.block_type) && n.blk_idx().is_some(),
                heading.contains(&n.block_type) && n.blk_idx().is_none(),
            )
        };
        if c_shell {
            new_kids.push(c);
            continue;
        }
        if c_heading {
            if let Some(&(sec_idx, sec_e)) = sec_by_start.get(&c_start) {
                if keep_atoms {
                    // 容器/item 内部的标题：章节壳应留在当前容器内，保持局部嵌套。
                    new_kids.push(sec_idx);
                } else {
                    match find_parent_section(sec_nodes, c_start, sec_e, sec_idx) {
                        Some(p) => b.tmp_children[p].push(sec_idx as u32),
                        None => new_kids.push(sec_idx),
                    }
                }
                continue;
            }
        }
        if exempt.contains(&c) && !b.nodes[c].kind.is_some() {
            new_kids.push(c);
            continue;
        }
        if b.nodes[c].is_item() {
            new_kids.push(c);
            continue;
        }
        if keep_atoms {
            new_kids.push(c);
            continue;
        }
        let sec = if b.nodes[c].kind.is_some() {
            // 容器只能挂到“不是自己后代”的章节（章节 start < 容器 start），
            // 否则会把容器挂进自己内部的 heading section，形成环。
            let idx = sec_nodes.partition_point(|&(s, _, _, _)| s < c_start);
            let mut found = None;
            for i in (0..idx).rev() {
                let (_, e, _, node_idx) = sec_nodes[i];
                if c_end <= e {
                    found = Some(node_idx);
                    break;
                }
            }
            found
        } else {
            deepest_section(sec_nodes, c_start, c_end)
        };
        match sec {
            Some(sec) => b.tmp_children[sec].push(c as u32),
            None => new_kids.push(c),
        }
    }
    new_kids
}

/// 章节展开（HS——level 单调栈）：章节壳节点 + 归属重组 + children 排序。
fn section_reorganize(b: &mut TreeBuilder, roots: Vec<usize>) -> Vec<usize> {
    let heading = b.topology.heading_candidates;
    let n_lines = b.ze;
    let secs: Vec<(usize, usize, u8, BlockType)> =
        section_intervals(b.blocks, b.records, b.arena, heading, n_lines);
    if secs.is_empty() {
        return roots;
    }
    let mut sec_by_start: std::collections::HashMap<usize, (usize, usize)> =
        std::collections::HashMap::new();
    let sec_nodes: Vec<(usize, usize, u8, usize)> = secs
        .iter()
        .map(|&(s, e, lv, t)| {
            let bt_end = {
                let lo = b.blocks.partition_point(|bl| bl.start < s as u32);
                b.blocks[lo..]
                    .iter()
                    .take_while(|bl| bl.start as usize == s)
                    .find(|bl| bl.block_type == t)
                    .map(|bl| bl.end as usize)
                    .unwrap_or(e)
            };
            let mut nd = TreeNode::new(REC_NONE, BLK_NONE, None, t, s as u32, e as u32, false, false, 0, false, 0);
            nd.content_end = bt_end as u32;
            let idx = b.push_node(nd);
            sec_by_start.insert(s, (idx, e));
            (s, e, lv, idx)
        })
        .collect();
    let mut parent_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in 0..b.nodes.len() {
        for &c in &b.tmp_children[i] {
            parent_of.insert(c as usize, i);
        }
    }
    let mut exempt: HashSet<usize> = HashSet::new();
    for i in 0..b.nodes.len() {
        let n = &b.nodes[i];
        if heading.contains(&n.block_type) && n.blk_idx().is_some() {
            let mut p = parent_of.get(&i).copied();
            while let Some(pi) = p {
                exempt.insert(pi);
                p = parent_of.get(&pi).copied();
            }
        }
    }
    let parents: Vec<usize> = (0..b.nodes.len())
        .filter(|&i| b.nodes[i].kind.is_some() || b.nodes[i].is_item())
        .collect();
    for p in parents {
        let kids: Vec<usize> = std::mem::take(&mut b.tmp_children[p])
            .into_iter()
            .map(|i| i as usize)
            .collect();
        b.tmp_children[p] = reorg_children(b, &sec_nodes, &sec_by_start, &exempt, kids, true)
            .into_iter()
            .map(|i| i as u32)
            .collect();
    }
    let roots = reorg_children(b, &sec_nodes, &sec_by_start, &exempt, roots, false);
    // reorg 移动节点后全序恢复(章节壳重组——children 归属变更)。
    b.sort_children_all();
    roots
}

/// 归属树构建——build_attribution_tree 的原子组合形态（五步：
/// expand / attach / segment / comment / 章节）。
pub fn build_attribution_tree(
    records: &[ContainerRec],
    blocks: &[BlockRec],
    view: &LineView,
    arena: &RecArena,
    zone: (u32, u32),
    topology: &crate::topology::Topology,
) -> AttrTreeNew {
    // 纯观测插桩（feature OFF 展开为 ()）：tree 段——五步各自的成本。
    let _sp_tr_total = crate::telemetry_span!("tr_total");
    let ze = zone.1 as usize;
    let mut b = { let _g = crate::telemetry_span!("tr_new");
                  TreeBuilder::new(records, blocks, view, arena, ze, topology) };
    let mut roots = Vec::new();
    // 1. 容器展开（记录树根 DFS）。
    {
    let _g = crate::telemetry_span!("tr_expand");
    for (rid, r) in records.iter().enumerate() {
        if r.parent == u32::MAX {
            if let Some(n) = b.expand(rid, None) {
                roots.push(n);
            }
        }
    }
    }
    // 2. 块挂接。
    roots.extend({ let _g = crate::telemetry_span!("tr_attach"); b.attach_blocks() });
    // 3. 分段（候选判定查拓扑声明——ItemSegments 规则）。
    let list_nodes: Vec<usize> = {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("tr_pick");
        b.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.rec_idx().is_some()
                    && n.kind.map_or(false, |fk| {
                        b.topology.has_rule(fk, crate::topology::CandidateRule::ItemSegments)
                    })
            })
            .map(|(i, _)| i)
            .collect()
    };
    {
        let _g = crate::telemetry_span!("tr_segment");
        for idx in list_nodes {
            b.segment(idx);
        }
    }
    // 4. Comment 闭合行（弹栈后 depth=0 的 Comment 行渲染独立 cm leaf）。
    let comment_rows: Vec<(u32, u32)> = b
        .nodes
        .iter()
        .filter(|n| n.kind == Some(FrameKind::Comment) && n.end > n.start + 1)
        .map(|n| (n.start, n.end))
        .collect();
    for (_, ce) in comment_rows {
        let nd = TreeNode::new(REC_NONE, BLK_NONE, None, BlockType::Comment, ce - 1, ce, false, false, 0, false, 0);
        let idx = b.push_node(nd);
        roots.push(idx);
    }
    // 4.5 容器 children 全序恢复(CF/Bf 兜底——无章节门,步骤⑤ 跳过;
    // HS 亦先排一次,reorg 后 sort_children_all 再排,幂等)。
    {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("tr_sort");
        b.sort_children_all();
    }
    // 5. 章节展开（HS——heading_candidates 非空）。
    if !b.topology.heading_candidates.is_empty() {
        roots = section_reorganize(&mut b, roots);
    }
    // 根排序（同 start 容器先——展开先于挂块）。
    roots.sort_by_key(|&r| b.nodes[r].start);
    // 物化临时 children → child_arena（按节点序写 (off,len)）。
    let mut child_arena: Vec<u32> = Vec::new();
    {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("tr_final");
    for i in 0..b.nodes.len() {
        let off = child_arena.len() as u32;
        let kids = std::mem::take(&mut b.tmp_children[i]);
        child_arena.extend(kids);
        b.nodes[i].children = Range32 {
            off,
            len: child_arena.len() as u32 - off,
        };
    }
    }
    let roots = roots.into_iter().map(|r| r as u32).collect();
    AttrTreeNew {
        nodes: b.nodes,
        child_arena,
        roots,
    }
}


// §5 node 面（几何 / 内容投影——Snapshot 读侧方法族的新实现）


/// 行内级派生查询：行 `line` 的非 Pop 条目数（= 旧 row_pushes 整列的值）。
/// 与 `State::rebuild_all` 的整列构建逐值一致（对拍：row_pushes_at_matches_rebuild）。
/// 成本 O(log E + k)，k 受 `block::MAX_BLOCK_NESTING` 约束。
pub fn row_pushes_at(entries: &[u64], line: usize) -> usize {
    let start = entries.partition_point(|e| (crate::block::entry_line(*e) as usize) < line);
    let mut n = 0usize;
    for e in &entries[start..] {
        if crate::block::entry_line(*e) as usize != line {
            break;
        }
        if !crate::block::entry_is_pop(*e) {
            n += 1;
        }
    }
    n
}

/// 行上开层条目数（严格 push——不含 pop 与 body_mut；O(log N + k)）。
pub(crate) fn entries_pushes_at(entries: &[u64], line: u32) -> usize {
    let start = entries.partition_point(|e| crate::block::entry_line(*e) < line);
    let mut n = 0usize;
    for e in &entries[start..] {
        if crate::block::entry_line(*e) != line {
            break;
        }
        if !crate::block::entry_is_pop(*e) && !crate::block::entry_is_body_mut(*e) {
            n += 1;
        }
    }
    n
}

/// 本行条目切片（行号过滤的连续区间 [start,end)）——prefix_cut 的
/// line_entries 契约：调用方必须传本行切片（折叠无行号参数、无法自滤行号）；
/// pop 条目由折叠内部防御性跳过（P0 修复：整表传参曾使列表预算扩为全文规模，
/// 容器内 IC 行的 `- ` 被当层吃掉）。
pub fn line_entries_at(entries: &[u64], line: u32) -> &[u64] {
    let start = entries.partition_point(|e| crate::block::entry_line(*e) < line);
    let end = entries.partition_point(|e| crate::block::entry_line(*e) <= line);
    &entries[start..end]
}

/// NodeProj：node 面——BlockNode（block.rs 共享类型）的几何 / 五面投影 /
/// 标记查询。对照 Snapshot 读侧方法族逐字节等价。
/// 零状态——持有三个输入引用（rope / lines / entries）。
/// 批量面读取(带零重放链供给):复用 `Segment` 的链池(`LineView::chain_frames_at`),
/// 避免 API 路径 `NodeProj::new(chain=None)` 逐节点 `rebuild_stack_at` 的 O(n²)。
/// 语义与 `Snapshot::node_proj().read_block_proj` 逐节点相等(由调用方对拍)。
pub fn read_block_proj_batch(
    snap: &crate::projection::Snapshot,
    nodes: &[crate::block::BlockNode],
    surface: crate::block::BlockProj,
) -> Vec<String> {
    let n = snap.line_len();
    let seg = crate::cache::structure_for_range(snap, 0, n);
    let view = LineView::new(
        &seg.records,
        &seg.blocks,
        &snap.frame_log.entries,
        &snap.lines,
        &snap.rope,
        (seg.start_line as u32, seg.end_line as u32),
        &seg.arena,
    );
    let chain = |l: usize| <LineView as crate::effect::ResolveView>::chain_frames_at(&view, l);
    let proj = NodeProj::with_chain(
        &snap.rope,
        &snap.lines,
        &snap.frame_log.entries,
        &chain,
    );
    nodes.iter().map(|nd| proj.read_block_proj(nd, surface)).collect()
}

/// 批量面 span 结果：与节点一一对应；None = 无法切片表达（回退 String 列）。
pub struct FaceSpans {
    pub container: Vec<Option<(u32, u32)>>,
    pub mark: Vec<Option<(u32, u32)>>,
    pub tag: Vec<Option<(u32, u32)>>,
}

/// 批量面 span（零重放链池；P2）：与 read_block_proj_batch 同一 Segment/LineView。
/// container 的 content 面由调用方传入（避免重复组装）；None 时 container 全 None
///（守卫需要 content）。mark/tag 恒可算（行内几何，无需链）。
/// 候选 span 不在此自对拍——由消费方用已算出的面 String 逐字节校验（零额外成本）。
pub fn read_block_proj_batch_spans(
    snap: &crate::projection::Snapshot,
    nodes: &[crate::block::BlockNode],
    content: Option<&[String]>,
) -> FaceSpans {
    let n = snap.line_len();
    let seg = crate::cache::structure_for_range(snap, 0, n);
    let view = LineView::new(
        &seg.records,
        &seg.blocks,
        &snap.frame_log.entries,
        &snap.lines,
        &snap.rope,
        (seg.start_line as u32, seg.end_line as u32),
        &seg.arena,
    );
    let chain = |l: usize| <LineView as crate::effect::ResolveView>::chain_frames_at(&view, l);
    let proj = NodeProj::with_chain(
        &snap.rope,
        &snap.lines,
        &snap.frame_log.entries,
        &chain,
    );
    let mut out = FaceSpans {
        container: Vec::with_capacity(nodes.len()),
        mark: Vec::with_capacity(nodes.len()),
        tag: Vec::with_capacity(nodes.len()),
    };
    for (i, nd) in nodes.iter().enumerate() {
        out.container.push(match content {
            Some(cs) => proj
                .container_span_with_content(nd, &cs[i])
                .map(|(o, l)| (o as u32, l as u32)),
            None => None,
        });
        out.mark.push(proj.mark_span(nd).map(|(o, l)| (o as u32, l as u32)));
        out.tag.push(proj.tag_span(nd).map(|(o, l)| (o as u32, l as u32)));
    }
    out
}

pub struct NodeProj<'a> {
    pub rope: &'a crop::Rope,
    pub lines: &'a [PackedLineInfo],
    pub entries: &'a [u64],
    /// 零重放链供给（O(n²) 修复）：render 路径注入 view.chain_row 访问器；
    /// None＝API 路径回退一次 rebuild_stack_at（正确但慢）。
    /// 改 Box 持有——API 路径（node_proj）才能注入捕获 Arc<Segment>
    /// 的自持闭包（&dyn 形态要求调用方外持，与返回值的借用冲突）。
    pub chain: Option<Box<dyn Fn(usize) -> smallvec::SmallVec<[crate::block::BlockFrame; 4]> + 'a>>,
}

impl<'a> NodeProj<'a> {
    pub fn new(
        rope: &'a crop::Rope,
        lines: &'a [PackedLineInfo],
        entries: &'a [u64],
    ) -> Self {
        NodeProj { rope, lines, entries, chain: None }
    }

    pub fn with_chain(
        rope: &'a crop::Rope,
        lines: &'a [PackedLineInfo],
        entries: &'a [u64],
        chain: impl Fn(usize) -> smallvec::SmallVec<[crate::block::BlockFrame; 4]> + 'a,
    ) -> Self {
        NodeProj { rope, lines, entries, chain: Some(Box::new(chain)) }
    }

    /// 行 k 的行前栈：链供给（零重放）优先；无供给回退一次点查询重建。
    fn start_stack(&self, k: u32) -> Vec<crate::block::BlockFrame> {
        match &self.chain {
            Some(f) => f(k as usize).into_vec(),
            None => crate::block::rebuild_stack_at(self.entries, k + 1),
        }
    }

    /// 行内级：本行自开帧数（派生，无列）。
    pub fn line_pushes(&self, line: usize) -> usize {
        // 对齐补漏（#9 前置·量定义钉死）：本行开层数＝严格 push 条目数
        // （非 Pop 且非 body_mut）——body_mut 是栈顶换态不是开层。分歧形状
        //（同行 body_mut∧push 共存）由 prefix_equiv 的 reachability 钉证
        // 全语料不可达；strict 助手与 row_pushes_at（非 Pop 计数）语义分家。
        entries_pushes_at(self.entries, line as u32)
    }

    /// BlockNode 构造（对照 Snapshot::build_node）：剥全部嵌套 BQ 前缀后
    /// line_geometry 测量（peel_width = 前缀字节宽）。
    pub fn build_node(
        &self,
        start_line: usize,
        end_line: usize,
        node_type: BlockType,
        loose: bool,
    ) -> crate::block::BlockNode {
        // 防御:start 越界 clamp(与 end 同保护——快照-树错配时防 panic;
        // 配对契约由调用方保证，此处只防崩溃；clamp 用「最后一个合法下标」，
        // 并在 0 行 rope（空文档）时显式短路。
        let start_line = if self.rope.line_len() == 0 {
            0
        } else {
            start_line.min(self.rope.line_len() - 1)
        };
        let end_line = end_line.max(start_line);
        let start_byte = self.rope.byte_of_line(start_line);
        let end_byte = if end_line < self.lines.len() {
            self.rope.byte_of_line(end_line)
        } else {
            self.rope.byte_len()
        };
        // P3-2：0 行 rope 没有可取的行 ⇒ 前缀与几何按空处理（避免 `rope.line(0)` panic ✗）。
        let (peel_width, geometry) = if self.rope.line_len() == 0 {
            (0usize, None)
        } else {
            with_line_str(self.rope.line(start_line), |s| {
                // ⑤b：memo 层 peel_peeled 迁移 prefix_cut——钉②实证
                // (BqOnly,None)+FinalSkip ≡ peel_peeled rest_after_bq（语料 0 分歧）。
                let cut = crate::infer::prefix_cut(s, &[], &[], crate::effect::PrefixKinds::BqOnly,
                    crate::effect::PrefixStop::None, crate::effect::PrefixLead::SkipWs,
                    crate::effect::PrefixTail::FinalSkip).bytes;
                (cut, crate::block::line_geometry(node_type, &s.as_bytes()[cut..]))
            })
        };
        crate::block::BlockNode {
            start_line,
            end_line,
            byte_range: start_byte..end_byte,
            node_type,
            loose,
            geometry,
            peel_width,
            frame_ordinal: 0,
        }
    }

    /// Chain-aware BlockNode construction (experimental, S1): peels exactly
    /// the shell layers in `chain` (PrefixStop::Chain) instead of all BQ
    /// prefixes. Same-line multi-frame nodes get their own layer's
    /// peel_width and geometry.
    pub fn build_node_chain(
        &self,
        start_line: usize,
        end_line: usize,
        node_type: BlockType,
        loose: bool,
        chain: &[crate::block::BlockFrame],
        line_entries: &[u64],
    ) -> crate::block::BlockNode {
        let start_line = if self.rope.line_len() == 0 {
            0
        } else {
            start_line.min(self.rope.line_len() - 1)
        };
        let end_line = end_line.max(start_line);
        let start_byte = self.rope.byte_of_line(start_line);
        let end_byte = if end_line < self.lines.len() {
            self.rope.byte_of_line(end_line)
        } else {
            self.rope.byte_len()
        };
        let (peel_width, geometry) = if self.rope.line_len() == 0 {
            (0usize, None)
        } else {
            with_line_str(self.rope.line(start_line), |s| {
                let cut = crate::infer::prefix_cut(s, chain, line_entries,
                    crate::effect::PrefixKinds::BqOnly,
                    crate::effect::PrefixStop::Chain,
                    crate::effect::PrefixLead::SkipWs,
                    crate::effect::PrefixTail::FinalSkip).bytes;
                (cut, crate::block::line_geometry(node_type, &s.as_bytes()[cut..]))
            })
        };
        crate::block::BlockNode {
            start_line,
            end_line,
            byte_range: start_byte..end_byte,
            node_type,
            loose,
            geometry,
            peel_width,
            frame_ordinal: 0,
        }
    }

    fn byte_range_str(&self, range: std::ops::Range<usize>) -> String {
        self.rope.byte_slice(range).to_string()
    }

    /// 借用版（单 chunk 零拷贝 / 多 chunk scratch 回退——渲染直写优化——
    /// 旧 Snapshot::byte_range_str 同款）。
    pub(crate) fn byte_range_str_borrow<'b>(
        &'b self,
        range: std::ops::Range<usize>,
        scratch: &'b mut Vec<u8>,
    ) -> &'b str {
        let slice = self.rope.byte_slice(range);
        let mut chunks = slice.chunks();
        if let Some(first) = chunks.next() {
            if first.len() == slice.byte_len() {
                return first;
            }
        }
        scratch.clear();
        for chunk in slice.chunks() {
            scratch.extend_from_slice(chunk.as_bytes());
        }
        crate::block::str_view(scratch)
    }

    pub fn block_type_name(&self, node: &crate::block::BlockNode) -> String {
        crate::block::node_type_to_str(node.node_type).to_string()
    }

    pub fn heading_text(&self, node: &crate::block::BlockNode) -> String {
        if crate::block::is_header(node.node_type) {
            if let Some(ref geo) = node.geometry {
                if geo.tag_range.start < geo.tag_range.end {
                    return with_line_str(self.rope.line(node.start_line), |s| {
                        let cleaned = crate::block::peel(s).1;
                        cleaned
                            [geo.tag_range.start..geo.tag_range.end.min(cleaned.len())]
                            .to_string()
                    });
                }
                return String::new();
            }
            return with_line_str(self.rope.line(node.start_line), |s| {
                let s = crate::block::peel(s).1;
                crate::block::extract_heading_text(s).to_string()
            });
        }
        String::new()
    }

    pub fn list_key(&self, node: &crate::block::BlockNode) -> Option<String> {
        if matches!(
            crate::effect::line_spec(node.node_type).tag_rule,
            crate::effect::TagRule::ColonKey | crate::effect::TagRule::CheckboxColonKey
        ) {
            if let Some(ref geo) = node.geometry {
                if geo.tag_range.start < geo.tag_range.end {
                    return Some(with_line_str(self.rope.line(node.start_line), |s| {
                        // ⑤b：prefix_cut 单源（钉②背书）
                        let cut = crate::infer::prefix_cut(s, &[], &[], crate::effect::PrefixKinds::BqOnly,
                            crate::effect::PrefixStop::None, crate::effect::PrefixLead::SkipWs,
                            crate::effect::PrefixTail::FinalSkip).bytes;
                        let cleaned = &s[cut..];
                        cleaned[geo.tag_range.start..geo.tag_range.end.min(cleaned.len())]
                            .to_string()
                    }));
                }
                return None;
            }
            return with_line_str(self.rope.line(node.start_line), |s| {
                let tag = crate::block::extract_kv_tag(node.node_type, s);
                if tag.is_empty() { None } else { Some(tag.to_string()) }
            });
        }
        None
    }

    pub fn tag_eq(&self, node: &crate::block::BlockNode, target: &str) -> bool {
        if let Some(ref geo) = node.geometry {
            if geo.tag_range.start < geo.tag_range.end {
                return with_line_str(self.rope.line(node.start_line), |s| {
                    // ⑤b：prefix_cut 单源（钉②背书）
                    let cut = crate::infer::prefix_cut(s, &[], &[], crate::effect::PrefixKinds::BqOnly,
                        crate::effect::PrefixStop::None, crate::effect::PrefixLead::SkipWs,
                        crate::effect::PrefixTail::FinalSkip).bytes;
                    let cleaned = &s[cut..];
                    let end = geo.tag_range.end.min(cleaned.len());
                    &cleaned[geo.tag_range.start..end] == target
                });
            }
        }
        false
    }

    /// 五面投影（对照 Snapshot::read_block_proj）。
    pub fn read_block_proj(
        &self,
        node: &crate::block::BlockNode,
        surface: crate::block::BlockProj,
    ) -> String {
        let spec = crate::block::block_proj_spec(node.node_type);
        if !spec.can_read.contains(surface) {
            return String::new();
        }
        match surface {
            crate::block::BlockProj::Raw => self.node_raw(node),
            crate::block::BlockProj::Mark => self.node_mark(node),
            crate::block::BlockProj::Tag => self.node_tag(node),
            crate::block::BlockProj::Content => self.node_content(node),
            crate::block::BlockProj::Container => self.node_container(node),
        }
    }

    /// Raw 面：不做投影计算，直接按 byte_range 从 Rope 切原文。
    pub fn node_raw(&self, node: &crate::block::BlockNode) -> String {
        self.rope.byte_slice(node.byte_range.clone()).to_string()
    }

    pub fn node_tag(&self, node: &crate::block::BlockNode) -> String {
        // setext 特判（守卫快照——表无「setext 型」列：line_spec 的 tag_rule
        // 与 ATX 同为 HeadingText——词法语义区分）。827 合并:提取逻辑 =
        // setext_title_from_raw 共享单点(与 incr extract_tag_str 同源)。
        if matches!(node.node_type, BlockType::HsHead1 | BlockType::HsHead2) {
            return with_line_str(self.rope.byte_slice(node.byte_range.clone()), |raw| {
                crate::block::setext_title_from_raw(raw)
            });
        }
        if let Some(ref geo) = node.geometry {
            if geo.tag_range.start < geo.tag_range.end {
                let tag = with_line_str(self.rope.line(node.start_line), |s| {
                    // S1 (experimental): tag range is relative to the node's
                    // own layer; use the chain-aware peel_width.
                    let cut = node.peel_width.min(s.len());
                    let cleaned = &s[cut..];
                    cleaned
                        [geo.tag_range.start..geo.tag_range.end.min(cleaned.len())]
                        .to_string()
                });
                if node.node_type == BlockType::CodeFenceOpen {
                    return crate::inline::decode_link_dest(&tag).into_owned();
                }
                if crate::block::is_header(node.node_type)
                {
                    return tag.trim().to_string();
                }
                return tag;
            }
            if matches!(
                crate::effect::line_spec(node.node_type).tag_rule,
                crate::effect::TagRule::ColonKey | crate::effect::TagRule::CheckboxColonKey
            ) {
                return String::new();
            }
        }
        match crate::effect::line_spec(node.node_type).tag_rule {
            crate::effect::TagRule::HeadingText => {
                with_line_str(self.rope.line(node.start_line), |s| {
                    let s = crate::block::peel(s).1;
                    crate::block::extract_heading_text(s).to_string()
                })
            }
            crate::effect::TagRule::Lang => {
                with_line_str(self.rope.line(node.start_line), |s| {
                    crate::inline::decode_link_dest(crate::block::extract_fence_lang(s)).into_owned()
                })
            }
            crate::effect::TagRule::Digits => {
                with_line_str(self.rope.line(node.start_line), |s| {
                    let lw = s.len() - s.trim_start().len();
                    let b = &s.as_bytes()[lw..];
                    let mut d = 0;
                    while d < b.len() && b[d].is_ascii_digit() {
                        d += 1;
                    }
                    if d > 0 && d < b.len() && (b[d] == b'.' || b[d] == b')') {
                        crate::block::str_view(&b[..d]).to_string()
                    } else {
                        String::new()
                    }
                })
            }
            crate::effect::TagRule::ColonKey | crate::effect::TagRule::CheckboxColonKey => {
                let t = node.node_type;
                with_line_str(self.rope.line(node.start_line), |s| {
                    // P7 查漏：kv/checklist tag 回退统一走 block 词法
                    // （find_colon_or_bracket_tag / find_ckl_tag）。
                    crate::block::extract_kv_tag(t, s).to_string()
                })
            }
            crate::effect::TagRule::Tagname => {
                with_line_str(self.rope.line(node.start_line), |s| {
                    crate::block::extract_html_tagname(s).to_string()
                })
            }
            _ => String::new(),
        }
    }

    pub fn node_mark(&self, node: &crate::block::BlockNode) -> String {
        // S1 (experimental): the mark lives at the node's own layer — use the
        // chain-aware peel_width, not a peel-all re-derivation.
        if let Some(ref geo) = node.geometry {
            if geo.mark_text_end > 0 {
                return with_line_str(self.rope.line(node.start_line), |s| {
                    let cut = node.peel_width.min(s.len());
                    let rest = &s[cut..];
                    rest[..geo.mark_text_end.min(rest.len())].to_string()
                });
            }
        }
        if node.node_type == BlockType::Blockquote {
            return with_line_str(self.rope.line(node.start_line), |s| {
                crate::block::bq_mark_text(s)
            });
        }
        // setext 特判同 node_tag（守卫快照——表无 setext 型列）
        if crate::block::is_header(node.node_type)
        {
            return crate::effect::atx_mark_text(
                crate::block::header_level(node.node_type).unwrap_or(1),
            );
        }
        if node.node_type == BlockType::RefDef {
            return with_line_str(self.rope.byte_slice(node.byte_range.clone()), |raw| {
                crate::inline::parse_refdef_multi(raw)
                    .map(|(label, _, _)| label)
                    .unwrap_or_default()
            });
        }
        with_line_str(self.rope.line(node.start_line), |s| {
            crate::block::extract_mark(node.node_type, s).to_string()
        })
    }

    pub fn node_container(&self, node: &crate::block::BlockNode) -> String {
        if node.start_line >= self.rope.line_len() {
            return String::new();
        }
        let content = self.read_block_proj(node, crate::block::BlockProj::Content);
        match self.container_span_with_content(node, &content) {
            Some((start, len)) => self.rope.byte_slice(start..start + len).to_string(),
            None => String::new(),
        }
    }

    /// container span 的守卫单源（node_container 与 span 消费方共用；P2）。
    /// 8.4：直取前缀——几何单源给出内容起点，前缀 = raw[..内容起点]；
    /// 有效性守卫保留旧语义（内容投影须恰占 raw 后缀——多行 BQ/列表内容
    /// 前后缀不吻合时=None——旧 ends_with 语义的直取版）。
    /// 判定序＝旧序（content 空/等 raw → None 先于 NodeSpan 特例）。
    /// 返回绝对 (start, len)（start = node.byte_range.start 起步）。
    pub fn container_span_with_content(
        &self,
        node: &crate::block::BlockNode,
        content: &str,
    ) -> Option<(usize, usize)> {
        if node.start_line >= self.rope.line_len() {
            return None;
        }
        let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
        if content.is_empty() || content == raw {
            return None;
        }
        if matches!(
            crate::effect::line_spec(node.node_type).content_rule,
            crate::effect::ContentRule::NodeSpan
        ) {
            let heading_end = self.rope.byte_of_line(node.start_line + 1).min(node.byte_range.end);
            if heading_end > node.byte_range.start {
                return Some((node.byte_range.start, heading_end - node.byte_range.start));
            }
        }
        let (peel_width, geo) = self.node_geometry(node);
        let r = raw.trim_end_matches('\n');
        let c = content.trim_end_matches('\n');
        // 几何可问（geo Some——mark 类型）：直取前缀＝raw[..内容起点]；
        // 不可问（IC 等无 mark——line_geometry None）：保留旧后缀反推
        // （ic_line_prefix 迁移属站迁移范围，非本步）。
        let content_start = geo.map(|g| {
            let base = self.rope.byte_of_line(node.start_line);
            let tag_rule = crate::effect::line_spec(node.node_type).tag_rule;
            if matches!(
                tag_rule,
                crate::effect::TagRule::ColonKey | crate::effect::TagRule::CheckboxColonKey
            ) && g.tag_range.start < g.tag_range.end
            {
                let pos = base + peel_width + g.tag_range.end;
                if pos < self.rope.byte_len() && self.rope.byte(pos) == b':' {
                    // 边界守卫（写面收口计划第一步）：冒号恰在行末（- k: 末行）
                    // 时 byte(pos+1) 越界——取 0 与行尾语义一致。
                    let after_byte = if pos + 1 < self.rope.byte_len() { self.rope.byte(pos + 1) } else { 0 };
                    return pos + colon_sep_len(b':', after_byte);
                }
                if tag_rule == crate::effect::TagRule::ColonKey {
                    // 括号键形式（`[台] v`——find_colon_or_bracket_tag 的 bracket
                    // 臂——内容在 `] ` 后）。
                    return pos + 1;
                }
                // CheckboxColonKey 无键（`[x]` 状态框——tag 是框内状态字符）→
                // 内容在框后（content_offset 已含框宽）。
            }
            base + peel_width + g.content_offset
        });
        match content_start {
            Some(cs) if cs >= node.byte_range.start => {
                let off = (cs - node.byte_range.start).min(raw.len());
                // 有效性守卫（旧 ends_with 语义）：内容投影须恰占 raw 后缀。
                if r.len() >= off + c.len() && &r[off..] == c {
                    Some((node.byte_range.start, off))
                } else {
                    None
                }
            }
            _ => {
                // 旧后缀相减（几何不可问类型的保留反推）。
                if r.starts_with(&c) {
                    None
                } else if r.ends_with(&c) {
                    Some((node.byte_range.start, r.len() - c.len()))
                } else {
                    None
                }
            }
        }
    }

    /// mark span 候选（行内几何；setext 等 fabricated mark None）。绝对偏移。
    fn mark_span(&self, node: &crate::block::BlockNode) -> Option<(usize, usize)> {
        if node.start_line >= self.rope.line_len() {
            return None;
        }
        if node.node_type == crate::block::BlockType::Blockquote {
            return with_line_str(self.rope.line(node.start_line), |line| {
                let (off, len) = crate::block::bq_mark_span(line);
                Some((node.byte_range.start + off, len))
            });
        }
        if matches!(
            node.node_type,
            crate::block::BlockType::HsHead1 | crate::block::BlockType::HsHead2
        ) {
            return None; // setext：mark 为 fabricated atx_mark_text（非切片）
        }
        let geo = node.geometry.as_ref()?;
        if geo.mark_text_end == 0 {
            return Some((node.byte_range.start, 0));
        }
        with_line_str(self.rope.line(node.start_line), |line| {
            let stripped = crate::block::strip_blockquote_prefix(line);
            let ws = line.len() - stripped.len();
            let len = geo.mark_text_end.min(stripped.len());
            Some((node.byte_range.start + ws, len))
        })
    }

    /// tag span 候选（prefix_cut + 行内 tag_range；decode/trim/setext 变换面
    /// None）。绝对偏移。
    fn tag_span(&self, node: &crate::block::BlockNode) -> Option<(usize, usize)> {
        if node.start_line >= self.rope.line_len() {
            return None;
        }
        if matches!(
            node.node_type,
            crate::block::BlockType::HsHead1 | crate::block::BlockType::HsHead2
        ) {
            return None; // setext：标题文本经 setext_title_from_raw（非切片）
        }
        let geo = node.geometry.as_ref()?;
        if geo.tag_range.start >= geo.tag_range.end {
            return Some((node.byte_range.start, 0)); // tag 恒 ""
        }
        if node.node_type == crate::block::BlockType::CodeFenceOpen {
            return None; // decode_link_dest 变换
        }
        if crate::block::is_header(node.node_type) {
            return None; // trim 变换
        }
        with_line_str(self.rope.line(node.start_line), |line| {
            let cut = crate::infer::prefix_cut(
                line,
                &[],
                &[],
                crate::effect::PrefixKinds::BqOnly,
                crate::effect::PrefixStop::None,
                crate::effect::PrefixLead::SkipWs,
                crate::effect::PrefixTail::FinalSkip,
            )
            .bytes;
            let start = (cut + geo.tag_range.start).min(line.len());
            let end = (cut + geo.tag_range.end).min(line.len());
            Some((node.byte_range.start + start, end.saturating_sub(start)))
        })
    }

    /// 几何可问（8.4）：memo 有则用；无则首行现算（content_face——零依赖）。
    /// 无 mark 类型（Paragraph/IC 等——line_geometry None）保持 None——由
    /// 消费点决定回退（node_container 走旧后缀反推；写面走既有 fallback）。
    pub fn node_geometry(
        &self,
        node: &crate::block::BlockNode,
    ) -> (usize, Option<crate::block::LineGeo>) {
        if let Some(g) = node.geometry.as_ref() {
            return (node.peel_width, Some(g.clone()));
        }
        if node.start_line >= self.rope.line_len() {
            return (0, None);
        }
        // 零拷贝①：整行 to_string → with_line_str（单 chunk 零拷贝、多 chunk 才分配）。
        with_line_str(self.rope.line(node.start_line), |line| {
            content_face(line, node.node_type)
        })
    }

    /// content_mode 表驱动入口（唯一声明 BLOCK_TYPE_EFFECTS）。
    pub fn node_content(&self, node: &crate::block::BlockNode) -> String {
        use crate::effect::ContentMode;
        match crate::effect::content_mode_for(node.node_type) {
            ContentMode::None_ => String::new(),
            ContentMode::Code => self.content_code(node),
            ContentMode::Raw => self.content_raw(node),
            ContentMode::Table => self.content_table(node),
            ContentMode::Stripped => self.content_stripped(node),
        }
    }

    /// IC 内容臂统一核心（性能专项）：逐行前缀折叠→dedent→sink（行体与
    /// \n 分隔逐段送）——content_code 与渲染流式共用；字节同一性由同一
    /// 核心构造性保证。
    pub fn content_code_ic_stream(
        &self,
        node: &crate::block::BlockNode,
        sink: impl FnMut(&str),
    ) {
        let mut seed = self.start_stack(node.start_line as u32);
        let own_start = entries_pushes_at(self.entries, node.start_line as u32);
        for _ in 0..own_start {
            seed.pop();
        }
        self.content_code_ic_stream_seeded(node, &seed, sink);
    }

    /// 链供给变体（性能专项·O(N²) 修复）：渲染路径已持有零重放链
    /// （ResolveView::chain_frames_at＝行前链——与 start_stack 弹掉本行自开
    /// 后同语义，无重建）。调用方保证 seed=行前链。
    pub fn content_code_ic_stream_seeded(
        &self,
        node: &crate::block::BlockNode,
        seed: &[crate::block::BlockFrame],
        mut sink: impl FnMut(&str),
    ) {
        let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
        // `first_bq` 下游给 `skip_leading_whitespace`（要字节）⇒ 补 `.as_bytes()`。
        let first_bq = crate::block::peel_peeled(raw.lines().next().unwrap_or(""), 0)
            .rest_after_bq_keep_indent()
            .as_bytes();
        let fb = crate::block::skip_leading_whitespace(first_bq);
        let fb_str = crate::block::str_view(fb);
        let is_marker_line = crate::block::peel(fb_str).0.iter().any(|l| {
            crate::effect::is_list_container(l.kind)
        });
        // 阈值仅在历史旧写法中消费（现走 dedent_cols_for(ICContinuation)）——
        // 保留计算维持与旧路径同形的副作用零差（is_marker_line 判定曾参与
        // 阈值语义，现保留为文档性判定）。
        let _threshold = if is_marker_line {
            crate::effect::geo::LIST_CONTENT_MAX
        } else {
            crate::block::compute_indent(first_bq)
        };
        // O(n²) 修复：共享重放取代逐行 rebuild_stack_at——行前栈递推
        // （before_k = after_{k-1}），本行开层在行末折叠追加（after 语义
        // 与旧 rebuild_stack_at(line_no+1) 一致，行为中性）。
        let mut replay = LineCtxReplay::with_stack(
            self.entries,
            node.start_line as u32,
            seed.to_vec(),
        );
        // 性能专项（A+B）：raw.lines() 的 l 与 rope.line 内容同、前缀
        // 折叠对尾换行不敏感——text 直用 l（省每行 String 分配）。
        let mut first_line = true;
        for (i, l) in raw.lines().enumerate() {
            // 前缀消费按事实分流（栈给的层数），不再无条件剥：
            //   已有容器的续行 ⇒ 按行尾栈 after 计数限剥；
            //   无容器 ⇒ 整行不动（`    > a` 的 `>` 是内容）。
            let line_no = (node.start_line + i) as u32;
            let (before, pushes) = replay.line_ctx(line_no);
            // after＝本行行尾栈（无容器 ⇒ 整行不动：`    > a` 的
            // `>` 是内容）。不用栈深或净增量索引——PopPush 行的
            // 净增量是 1 却在行上有两个标记：两者都会让索引错位、
            // 前缀漏剥。零拷贝：行上无开层 ⇒ after≡before 直传
            //（O(depth) 拷贝只在开层行发生）。
            let mut tmp: smallvec::SmallVec<[crate::block::BlockFrame; 8]> = smallvec::SmallVec::new();
            let after: &[crate::block::BlockFrame] = if pushes.is_empty() {
                before
            } else {
                tmp.extend_from_slice(before);
                for &e in pushes.iter() {
                    tmp.push(crate::block::entry_frame(e));
                }
                &tmp
            };
            let (cut, list_unconsumed) = if after.is_empty() {
                (0usize, 0usize)
            } else {
                // 读写同源：与 ic_line_prefix 同一份 prefix_cut(All,Counted) 单源
                //（R2 对齐——markers_on_line 站点迁移）。
                let pc = crate::infer::prefix_cut(
                    l, &after,
                    crate::infer::line_entries_at(self.entries, line_no),
                    crate::effect::PrefixKinds::All,
                    crate::effect::PrefixStop::Counted,
                    crate::effect::PrefixLead::SkipWs,
                    crate::effect::PrefixTail::None);
                let cut = pc.bytes;
                let edge = pc.edge;
                let indent = crate::effect::geo::LIST_CONTENT_MAX + edge;
                (cut, indent.saturating_sub(crate::effect::geo::LIST_CONTENT_MAX))
            };
            let body: &str = &l[cut..];
            if !first_line {
                sink("\n");
            }
            first_line = false;
            sink(dedent_columns(
                body,
                // 内容缩进 = 4 + 未消费的列表内容边缘（算式已归位
                // 声明层 dedent_cols_for——旧写法取 `threshold`
                // 会过剥 4：`  >         deep` 的 8 格被全剥）
                crate::effect::dedent_cols_for(
                    crate::effect::DedentScene::ICContinuation,
                    0,
                    list_unconsumed,
                    0,
                ),
            ));
        }
    }

    /// 围栏内容臂统一核心（性能专项）：BQ 剥＋dedent 逐行 sink——content_code
    /// 与渲染流式共用；字节同一性由同一核心构造性保证。
    /// fence 容器的 BQ 层数——开行 peel 层序列（词法单源；scan 保证 bq 每行带 mark；
    /// double check 更正：chain_frames_at 是 ViewArrays 方法、NodeProj 就地无）。
    fn fence_bq_depth(&self, node: &crate::block::BlockNode) -> usize {
        with_line_str(self.rope.line(node.start_line), |l| {
            crate::block::peel_peeled(l, 0)
                .layers
                .iter()
                .filter(|ly| ly.kind == crate::block::FrameKind::BlockQuote)
                .count()
        })
    }
    pub fn content_code_fence_stream(
        &self,
        node: &crate::block::BlockNode,
        mut sink: impl FnMut(&str),
    ) {
        let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
        // k = 容器 BQ 层数（词法单源）；剥恰 k 层——正文自身以 > 开头的行保真
        // （缺陷 B：全剥会把代码行 > > foo 吃成 foo）。k==0 时零拷贝直借。
        let k = self.fence_bq_depth(node);
        let working: std::borrow::Cow<'_, str> = if k == 0 {
            std::borrow::Cow::Borrowed(&raw)
        } else {
            let mut w = String::with_capacity(raw.len());
            let mut first = true;
            for l in raw.split('\n') {
                if !first {
                    w.push('\n');
                }
                first = false;
                w.push_str(crate::block::peel_peeled(l, 0).rest_after_layer(k));
            }
            std::borrow::Cow::Owned(w)
        };
        let Some(cr) = crate::block::range_content(node.node_type, &working) else {
            return;
        };
        let fence_indent = fence_indent_base(working.lines().next().map_or("", |l| l));
        let cols = crate::effect::dedent_cols(
            crate::effect::dedent_spec_for(crate::effect::DedentScene::FenceContent),
            fence_indent,
        );
        let content = &working[cr];
        if cols == 0 {
            sink(content);
            return;
        }
        let mut first = true;
        for l in content.split('\n') {
            if !first {
                sink("\n");
            }
            first = false;
            sink(dedent_columns(l, cols));
        }
    }

    fn content_code(&self, node: &crate::block::BlockNode) -> String {
        if node.geometry.is_some() {
            let spec = crate::effect::line_spec(node.node_type);
            if matches!(spec.tag_rule, crate::effect::TagRule::Lang) {
                let mut out = String::with_capacity(
                    node.byte_range.end.saturating_sub(node.byte_range.start) + 64);
                self.content_code_fence_stream(node, |s| out.push_str(s));
                return out;
            }
        }
        match crate::effect::line_spec(node.node_type).tag_rule {
            crate::effect::TagRule::Lang => {
                let s = node.start_line + 1;
                let e = if node.end_line > node.start_line + 1
                    && self
                        .lines
                        .get(node.end_line.saturating_sub(1))
                        .map(|p| p.node_type() == BlockType::CodeFenceClose)
                        .unwrap_or(false)
                {
                    node.end_line.saturating_sub(1)
                } else {
                    node.end_line
                };
                if e > s {
                    let start_byte = self.rope.byte_of_line(s);
                    let end_byte = if e >= self.lines.len() {
                        node.byte_range.end
                    } else {
                        self.rope.byte_of_line(e).min(node.byte_range.end)
                    };
                    let raw = self.rope.byte_slice(start_byte..end_byte).to_string();
                    // k 同口径（与 fence_stream 同一规则）：剥恰 k 层——无条件剥
                    // （k==0 时 rest_after_layer(0) 原样通过，全函数）。
                    let (k, fence_indent) = with_line_str(
                        self.rope.line(node.start_line),
                        |fence_line| {
                            (
                                crate::block::peel_peeled(fence_line, 0)
                                    .layers
                                    .iter()
                                    .filter(|ly| ly.kind == crate::block::FrameKind::BlockQuote)
                                    .count(),
                                fence_indent_base(fence_line),
                            )
                        },
                    );
                    // k==0 短路（性能平价：顶层 fence 不 peel——与旧 else raw 同）
                    let working = if k == 0 {
                        raw
                    } else {
                        raw.split('\n')
                            .map(|l| crate::block::peel_peeled(l, 0).rest_after_layer(k))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    let cols = crate::effect::dedent_cols(
                        crate::effect::dedent_spec_for(crate::effect::DedentScene::FenceContent),
                        fence_indent,
                    );
                    if cols == 0 {
                        return working.to_string();
                    }
                    working
                        .split('\n')
                        .map(|l| dedent_columns(l, cols))
                        .collect::<Vec<_>>()
                        .join("\n")
                } else {
                    String::new()
                }
            }
            crate::effect::TagRule::None
                if matches!(
                    crate::effect::line_spec(node.node_type).mark_width,
                    crate::effect::MarkWidth::Fixed("    ")
                ) =>
            {
                let mut out = String::with_capacity(
                    node.byte_range.end.saturating_sub(node.byte_range.start) + 64);
                self.content_code_ic_stream(node, |s| out.push_str(s));
                out
            }
            _ => String::new(),
        }
    }

    fn content_raw(&self, node: &crate::block::BlockNode) -> String {
        let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
        let first_nl = raw.find('\n').unwrap_or(raw.len());
        // 丢掉最后一个字符（多为行尾标记符）：按**字符**截而不是按字节 len-1
        // ——末字符多字节时 `&raw[..len-1]` 直接 panic。
        let cut_at = raw.char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
        let without_close = &raw[..cut_at];
        let last_nl = without_close.rfind('\n').unwrap_or(0);
        if first_nl < raw.len() && last_nl > first_nl {
            raw[first_nl + 1..last_nl].to_string()
        } else if first_nl < raw.len() {
            raw[first_nl + 1..].trim_end().to_string()
        } else {
            raw
        }
    }

    fn content_table(&self, node: &crate::block::BlockNode) -> String {
        let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
        let (_, ranges) = crate::block::cell_ranges_bytes(raw.as_bytes());
        if ranges.is_empty() {
            raw.trim().to_string()
        } else {
            ranges
                .iter()
                .map(|r| raw[r.clone()].trim().to_string())
                .collect::<Vec<_>>()
                .join("\t")
        }
    }

    fn content_stripped(&self, node: &crate::block::BlockNode) -> String {
        let spec = crate::effect::line_spec(node.node_type);
        match (spec.content_rule, spec.tag_rule, spec.mark_shape) {
            (crate::effect::ContentRule::NodeSpan, _, _) => self.heading_body(node),
            (_, tr, _) if crate::effect::is_key_like(tr) => self.list_content(node, spec.tag_rule),
            (_, _, crate::effect::MarkShape::WrapAsymmetric)
            | (crate::effect::ContentRule::TextSpan, crate::effect::TagRule::Tagname, _) => {
                self.range_content(node)
            }
            (crate::effect::ContentRule::AfterMark, crate::effect::TagRule::None, _) => {
                self.bq_content(node)
            }
            (
                crate::effect::ContentRule::TextSpan,
                crate::effect::TagRule::None,
                crate::effect::MarkShape::Prefix,
            ) => self.normalized_content(node),
            _ => String::new(),
        }
    }

    fn heading_body(&self, node: &crate::block::BlockNode) -> String {
        // C3' setext 面值收束(解锁原锁定):下划线行(HsSep 标记)
        // 不进内容面——标题行(s)在 tag(t);下划线后的节体行保留(节点区间
        // 无空行并入的正文)。写面 content_write_range 已表化跳过下划线。
        let is_setext = matches!(
            node.node_type,
            crate::block::BlockType::HsHead1 | crate::block::BlockType::HsHead2
        );
        // setext:找标题行后第一个下划线样式行(全 '=' 或全 '-'——HsSep
        // 词法);内容起点 = 该行下一行(越过标记);找不到(理论不发生)按
        // 旧语义从第 2 行起(防御)。
        let underline_after = if is_setext {
            // 端界防御:区间尾字节若恰在行首 → line_of_byte 越界(行号 = 行数)
            let mut end_line = self
                .rope
                .line_of_byte(node.byte_range.end.min(self.rope.byte_len()));
            let line_cnt = self.rope.line_len();
            if end_line >= line_cnt {
                end_line = line_cnt.saturating_sub(1);
            }
            (node.start_line + 1..=end_line).find(|&l| {
                with_line_str(self.rope.line(l), |s| {
                    // 剥 BQ 前缀(BQ 内 setext 的下划线行 = "> ---")后判
                    // 全 '='/'-' 下划线样式。
                    // `s` 来自 `with_line_str` 闭包（`&str`）⇒ 传 `&s`；
                    // 下游 `trim_*`／`t.as_bytes()`（`&str` 语境）⇒ 不加 `.as_bytes()`。
                    let stripped = crate::block::peel_peeled(&s, 0).rest_after_bq_keep_indent();
                    let t = stripped.trim_end_matches(['\n', '\r']).trim_start();
                    !t.is_empty()
                        && t.as_bytes().iter().all(|&b| b == b'=' || b == b'-')
                })
            })
        } else {
            None
        };
        let body_start_line = underline_after.map(|l| l + 1);
        // 内容起点行:setext 有下划线 → 下划线后;否则标题行后(旧语义)。
        let content_first_line = body_start_line.unwrap_or(node.start_line + 1);
        if let Some(ref geo) = node.geometry {
            let heading_line_end = self
                .rope
                .byte_of_line(content_first_line)
                .min(node.byte_range.end);
            if heading_line_end < node.byte_range.end {
                let start_byte = heading_line_end.max(
                    self.rope.byte_of_line(node.start_line) + geo.content_offset,
                );
                let mut scratch = Vec::new();
                let raw = self.byte_range_str_borrow(start_byte..node.byte_range.end, &mut scratch);
                return raw
                    .lines()
                    .map(|l| {
                        crate::block::peel_peeled(&l, 0).rest_after_bq_keep_indent()
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            return String::new();
        }
        let heading_line_end = self
            .rope
            .byte_of_line(content_first_line)
            .min(node.byte_range.end);
        if heading_line_end < node.byte_range.end {
            let mut scratch = Vec::new();
            let raw = self.byte_range_str_borrow(heading_line_end..node.byte_range.end, &mut scratch);
            raw.lines()
                .map(|l| {
                    crate::block::peel_peeled(&l, 0).rest_after_bq_keep_indent()
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        }
    }

    fn list_content(
        &self,
        node: &crate::block::BlockNode,
        tag_rule: crate::effect::TagRule,
    ) -> String {
        if let Some(ref geo) = node.geometry {
            let line_start = self.rope.byte_of_line(node.start_line);
            let bq_off = node.peel_width;
            // C3 后无键 ck 的 tag = checkbox 内状态字符(1 字符,起点 < 6 =
            // CHECKLIST_LIST_MARK_END + CHECKLIST_CHECKBOX_SPAN)——不参与
            // content 切分(content 从 checkbox 后起,走 content_offset)。
            let tag_in_checkbox = matches!(
                tag_rule,
                crate::effect::TagRule::CheckboxColonKey
            ) && geo.tag_range.start < geo.tag_range.end
                && geo.tag_range.start
                    < crate::block::CHECKLIST_LIST_MARK_END
                        + crate::block::CHECKLIST_CHECKBOX_SPAN;
            // 面重写（步骤 2）：几何经统一入口 content_face（与 memo 同源，
            // memo 钉背书等价——两 memo 读收敛为一次轮子调用）
            let face_geo = crate::infer::content_face(
                &self.rope.line(node.start_line).to_string(),
                node.node_type,
            );
            let face_off = face_geo.1.as_ref().map_or(0, |g| g.content_offset);
            let content_start = if matches!(
                tag_rule,
                crate::effect::TagRule::ColonKey | crate::effect::TagRule::CheckboxColonKey
            ) && geo.tag_range.start < geo.tag_range.end
                && !tag_in_checkbox
            {
                let pos = line_start + bq_off + geo.tag_range.end;
                // 边界：pos 可能正落在文本末尾（`- k:` 无尾换行——冒号即末字节），
                // 此时没有分隔符可读，sep_len 取 0。旧写法直接 rope.byte(pos+1)
                // 越界 panic（byte index 4 but length 4）。
                let sep_len = if pos + 1 < self.rope.byte_len() {
                    colon_sep_len(self.rope.byte(pos), self.rope.byte(pos + 1))
                } else if pos < self.rope.byte_len() {
                    colon_sep_len(self.rope.byte(pos), 0)
                } else {
                    0
                };
                pos + sep_len
            } else {
                line_start + face_geo.0 + face_off
            };
            let mut scratch = Vec::new();
            let raw = self.byte_range_str_borrow(content_start..node.byte_range.end, &mut scratch);
            if !raw.is_empty() {
                let first_nl = raw.find('\n').unwrap_or(raw.len());
                if raw.len() > first_nl {
                    let body = &raw[..first_nl];
                    let dedented = body.to_string()
                        + &dedent_continuation_bq(
                            &raw[first_nl..],
                            crate::effect::dedent_cols_for(
                                crate::effect::DedentScene::ItemGeometry,
                                face_off,
                                0,
                                0,
                            ),
                        );
                    if crate::effect::is_key_like(tag_rule) {
                        // C3' OLI 收束(含 Digits): item 面值去尾换行——行尾 \n
                        // 是行几何非内容;ColonKey/CheckboxColonKey 另去尾空白。
                        let d = if matches!(
                            tag_rule,
                            crate::effect::TagRule::ColonKey
                                | crate::effect::TagRule::CheckboxColonKey
                        ) {
                            dedented.trim_end_matches('\n').trim_end().to_string()
                        } else {
                            dedented.trim_end_matches('\n').to_string()
                        };
                        // L1:剥 marker 前先剥夹层 BQ(与零拷贝同源;
                        // keep_indent=true——值前导空格是内容)。
                        // 真正病因：源 `d` 是 `String` ⇒ 传 `&d`；目标 `d` 给
                        // `peel_markers(&str)` ⇒ 返回值不加 `.as_bytes()`）。
                        let d = crate::block::peel_peeled(&d, 0).rest_after_bq_keep_indent();
                        return crate::block::peel_markers(
                            d,
                            crate::effect::item_inner_layers(
                                self.line_pushes(node.start_line),
                                true,
                            ),
                        )
                        .to_string();
                    }
                    return dedented;
                }
                if crate::effect::is_key_like(tag_rule) {
                    // L1:剥 marker 前先剥夹层 BQ(同 dedent 分支;
                    // keep_indent=true)。
                    // 同上：传 `&raw`（`String`／`&str` 都能 deref 到 `&str`），
                    // 目标给 `peel_markers(&str)` ⇒ 返回值不加 `.as_bytes()`。
                    let raw = crate::block::peel_peeled(&raw, 0).rest_after_bq_keep_indent();
                    return crate::block::peel_markers(
                        raw,
                        crate::effect::item_inner_layers(
                            self.line_pushes(node.start_line),
                            true,
                        ),
                    )
                    .to_string();
                }
                return raw.to_string();
            }
            return String::new();
        }
        match tag_rule {
            crate::effect::TagRule::ColonKey | crate::effect::TagRule::CheckboxColonKey => {
                let mut scratch = Vec::new();
                let raw = self.byte_range_str_borrow(node.byte_range.clone(), &mut scratch);
                let first_line_end = raw.find('\n').unwrap_or(raw.len());
                let first_line = &raw[..first_line_end];
                let t = node.node_type;
                let stripped = crate::block::strip_list_marker(first_line, t);
                let body = if t == BlockType::Checklist {
                    stripped.to_string()
                } else {
                    let tag_range = crate::block::find_colon_or_bracket_tag(
                        stripped.as_bytes(),
                        0,
                    );
                    if tag_range.start < tag_range.end {
                        let after_tag = &stripped[tag_range.end..];
                        let skip = if after_tag.starts_with(':') {
                            match after_tag.as_bytes().get(1) {
                                Some(b' ' | b'\t') => 2,
                                _ => 1,
                            }
                        } else {
                            1
                        };
                        after_tag[skip.min(after_tag.len())..].trim_start().to_string()
                    } else {
                        stripped.to_string()
                    }
                };
                // L1:剥 marker 前先剥夹层 BQ(- > - b 型;keep_indent=true)。
                let body = crate::block::peel_markers(
                    crate::block::peel_peeled(&body, 0).rest_after_bq_keep_indent(),
                    crate::effect::item_inner_layers(
                        self.line_pushes(node.start_line),
                        stripped.len() < first_line.trim_start().len(),
                    ),
                );
                let mut v = if raw.len() > first_line_end {
                    body.to_string()
                        + &dedent_continuation_bq(
                            &raw[first_line_end..],
                            crate::effect::dedent_cols_for(
                                crate::effect::DedentScene::ItemFallback,
                                0,
                                0,
                                0,
                            ),
                        )
                } else {
                    body.to_string()
                };
                v.truncate(v.trim_end().len());
                v
            }
            crate::effect::TagRule::Digits => {
                let mut scratch = Vec::new();
                let raw = self.byte_range_str_borrow(node.byte_range.clone(), &mut scratch);
                let first_line_end = raw.find('\n').unwrap_or(raw.len());
                let first_line = &raw[..first_line_end];
                let stripped = crate::block::strip_list_marker(
                    first_line,
                    BlockType::OrderedListItem,
                );
                // L1:剥 marker 前先剥夹层 BQ(同 ColonKey 分支;keep_indent=true)。
                let stripped = crate::block::peel_markers(
                    crate::block::peel_peeled(&stripped, 0).rest_after_bq_keep_indent(),
                    crate::effect::item_inner_layers(
                        self.line_pushes(node.start_line),
                        stripped.len() < first_line.trim_start().len(),
                    ),
                );
                let body = stripped.to_string();
                if raw.len() > first_line_end {
                    body + &dedent_continuation_bq(
                        &raw[first_line_end..],
                        crate::effect::dedent_cols(
                            crate::effect::dedent_spec_for(
                                crate::effect::DedentScene::ItemFallback,
                            ),
                            0,
                        ),
                    )
                } else {
                    body
                }
            }
            _ => String::new(),
        }
    }

    fn range_content(&self, node: &crate::block::BlockNode) -> String {
        if let Some(ref _geo) = node.geometry {
            let nt = node.node_type;
            let mut scratch = Vec::new();
            let raw = self.byte_range_str_borrow(node.byte_range.clone(), &mut scratch);
            if let Some(cr) = crate::block::range_content(nt, &raw) {
                return raw[cr].trim().to_string();
            }
            return String::new();
        }
        if node.node_type == BlockType::HtmlElement {
            let mut scratch = Vec::new();
            let raw = self.byte_range_str_borrow(node.byte_range.clone(), &mut scratch);
            let first_nl = raw.find('\n').unwrap_or(raw.len());
            let last_nl = raw.rfind('\n').unwrap_or(0);
            if first_nl < last_nl {
                let first_line = &raw[..first_nl];
                let tag_end = first_line.find('>').map(|p| p + 1).unwrap_or(0);
                let close_start = raw[last_nl..].find("</").unwrap_or(0);
                let body = &raw[tag_end + 1..last_nl + close_start];
                body.trim_end_matches('\n').to_string()
            } else {
                let tag_end = raw.find('>').map(|p| p + 1).unwrap_or(0);
                let close_start = raw.rfind("</").unwrap_or(raw.len());
                if tag_end < close_start {
                    raw[tag_end..close_start].to_string()
                } else {
                    String::new()
                }
            }
        } else {
            String::new()
        }
    }

    fn bq_content(&self, node: &crate::block::BlockNode) -> String {
        let mut scratch = Vec::new();
        let raw_text = self.byte_range_str_borrow(node.byte_range.clone(), &mut scratch);
        // 手写剥 BQ 循环 → 既有封装 strip_bq(false)(单源律——逐层
        // skip_leading_whitespace+parse_bq+吃一空白+尾 trim,与循环同构)。
        raw_text
            .lines()
            .map(|l| crate::block::peel_peeled(l, 0).rest_after_bq())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// hb 内容——统一读面剥层(容器前缀——任意层任意类型):
    /// 首行剥全部容器前缀(peel 层序列——BQ+列表 marker);续行逐行剥
    /// 全部 BQ 层(bq_content 同款循环)+ dedent(首行容器前缀列宽——
    /// fence_indent_base,与 CF content_code 同构)。开/闭行保留(整块
    /// 原文语义——官方例 2850 期望 <div>\nfoo——含开行剥前缀)。
    pub(crate) fn html_block_content(&self, node: &crate::block::BlockNode) -> String {
        let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
        let trailing = raw.ends_with('\n');
        let body = raw.trim_end_matches('\n');
        if body.is_empty() { return String::new(); }
        // dedent 列 = 首行的列表 marker 层宽度(BQ 层由逐行剥承担,
        // 列表层是容器缩进——CM 剥;内容自身缩进是 raw 的一部分——保留)。
        // `s` 下游给 `peel(s)` 与 `s.as_bytes()`（要 `&str`）⇒ 不加 `.as_bytes()`。
        let s = crate::block::peel_peeled(body.lines().next().unwrap_or(""), 0)
            .rest_after_bq_keep_indent();
        // k = 容器 BQ 层数（fence 修复同构）：续行剥恰 k 层——
        // 顶层 hb 的字面 > 是内容（k==0 原样，cmark passthrough）。
        let k = crate::block::peel_peeled(body.lines().next().unwrap_or(""), 0)
            .layers
            .iter()
            .filter(|ly| ly.kind == crate::block::FrameKind::BlockQuote)
            .count();
        let cols: usize = crate::block::peel(s).0.iter()
            .filter(|l| crate::effect::is_list_container(l.kind))
            .map(|l| crate::block::column_width(&s.as_bytes()[l.start..l.end]))
            .sum();
        let mut out = String::with_capacity(body.len());
        let mut is_first = true;
        for l in body.split('\n') {
            if !is_first { out.push('\n'); }
            // 首行且确有容器前缀:剥全部层(BQ+marker——peel 层序列);
            // 续行(或首行无前缀):逐行剥全部 BQ 层 + dedent 内容列。
            if is_first {
                let peeled = crate::block::peel(l).1;
                out.push_str(if peeled.len() < l.len() {
                    crate::block::str_view(peeled.as_bytes())
                } else {
                    l
                });
            } else {
                // 剥恰 k 层：容器 mark 剥、字面 > 保真；
                // 内容自身缩进是 raw 的一部分——保留（rest_after_layer 按层末偏移）。
                let s = crate::block::peel_peeled(l, 0).rest_after_layer(k);
                out.push_str(crate::infer::dedent_columns(s, cols));
            }
            is_first = false;
        }
        if trailing { out.push('\n'); }
        out
    }

    fn normalized_content(&self, node: &crate::block::BlockNode) -> String {
        match crate::effect::normalize_spec(node.node_type) {
            crate::effect::NormalizeSpec::StripTrailing => {
                let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
                if raw.as_bytes().iter().any(|&b| b == b'>') {
                    // 8.2：(All,Chain) ctx 折叠。
                    // O(n²) 修复：链优先走零重放供给（render 注入 view.chain_row）；
                    // 无供给（API 路径）回退一次点查询重建。
                    let stack = self.start_stack(node.start_line as u32);
                    paragraph_text_normalize(&paragraph_lines_stripped(
                        &raw, self.entries, node.start_line as u32, &stack))
                } else {
                    paragraph_text_normalize(&raw)
                }
            }
            crate::effect::NormalizeSpec::Raw => {
                self.rope.byte_slice(node.byte_range.clone()).to_string()
            }
        }
    }

    /// 渲染接入候选（当前主要测试消费；Phase F 裁决：保留为 API，暂 test-only）。
    pub fn inline_segments(&self, node: &crate::block::BlockNode) -> Vec<crate::inline::InlineSegment> {
        let spec = crate::block::block_proj_spec(node.node_type);
        let seg = |role: crate::inline::InlineSegmentRole, text: String| -> Option<crate::inline::InlineSegment> {
            if text.is_empty() { None } else { Some(crate::inline::InlineSegment { role, text }) }
        };
        match spec.inline_mode {
            crate::block::InlineFeed::None | crate::block::InlineFeed::RawText => vec![],
            crate::block::InlineFeed::TagOnly => {
                seg(crate::inline::InlineSegmentRole::Tag, self.inline_source(node)).into_iter().collect()
            }
            crate::block::InlineFeed::ContentOnly => {
                seg(crate::inline::InlineSegmentRole::Content, self.inline_source(node)).into_iter().collect()
            }
            crate::block::InlineFeed::TagThenContent => {
                let mut v = Vec::new();
                if let Some(s) = seg(crate::inline::InlineSegmentRole::Tag, self.inline_source(node)) { v.push(s); }
                if let Some(s) = seg(crate::inline::InlineSegmentRole::Content, self.read_block_proj(node, crate::block::BlockProj::Content)) { v.push(s); }
                v
            }
            crate::block::InlineFeed::Cells => {
                let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
                let (_, ranges) = crate::block::cell_ranges_bytes(raw.as_bytes());
                ranges.iter().enumerate().map(|(col, r)| {
                    crate::inline::InlineSegment {
                        role: crate::inline::InlineSegmentRole::Cell { row: node.start_line, col },
                        text: raw[r.clone()].trim().to_string(),
                    }
                }).collect()
            }
        }
    }

    pub fn inline_source(&self, node: &crate::block::BlockNode) -> String {
        let t = node.node_type;
        let mut text = if crate::block::is_header(t) {
            with_line_str(self.rope.line(node.start_line), |s| {
                let s = crate::block::peel_peeled(s, 0).rest_after_bq();
                crate::block::extract_heading_text(s).to_string()
            })
        } else if crate::effect::block_decl(t).is_list_item {
            if let Some(ref geo) = node.geometry {
                let start = node.byte_range.start as usize + geo.content_offset;
                self.rope.byte_slice(start..node.byte_range.end).to_string()
            } else {
                let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
                crate::block::strip_list_marker(&raw, t).to_string()
            }
        } else if t == crate::block::BlockType::Blockquote {
            let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
            // 手写剥 BQ 循环 → strip_bq(false)(与 bq_content 同款封装)。
            raw.lines()
                .map(|l| crate::block::peel_peeled(l, 0).rest_after_bq())
                .collect::<Vec<_>>()
                .join("\n")
        } else if t == crate::block::BlockType::Paragraph {
            let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
            // 手写剥 BQ 循环 → strip_bq(false);外层 trim_start 保留
            // (Unicode 空白语义不变——BQ 剥层只认 ASCII 空白)。
            let stripped: String = raw.lines()
                .map(|l| crate::block::peel_peeled(l, 0).rest_after_bq())
                .collect::<Vec<_>>()
                .join("\n");
            if stripped.contains('\n') {
                let mut out = String::with_capacity(stripped.len());
                let mut first = true;
                for l in stripped.lines() {
                    if first { out.push_str(l.trim_start()); first = false; }
                    else { out.push('\n'); out.push_str(l.trim_start()); }
                }
                out
            } else {
                stripped.trim_start().to_string()
            }
        } else if t == crate::block::BlockType::IndentedCode {
            let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
            raw.lines().map(|l| {
                // IC 内容行剥壳（命名规则）：仅位置 0 的单层 `>`＋可选 1 空白；
                // 不跳前导空白——缩进后的 `>` 是内容（与 strip_bq_one 的 skip-ws
                // 语义不同，勿混用）。dedent 走声明层列。
                let s = ic_line_strip_bq(l);
                dedent_columns(s, crate::effect::dedent_cols_for(
                    crate::effect::DedentScene::ICContent, 0, 0, 0))
            }).collect::<Vec<_>>().join("\n")
        } else if t == crate::block::BlockType::HtmlBlock {
            let raw = self.rope.byte_slice(node.byte_range.clone()).to_string();
            raw.trim_end_matches('\n').to_string()
        } else {
            String::new()
        };
        let trimmed_len = text.trim_end_matches('\n').len();
        text.truncate(trimmed_len);
        text
    }

    /// 标题行结束（节体起点）——表驱动统一入口（读面 heading_body / 写面
    /// content_write_range 共用）：
    ///   ATX（LvHashes）→ 标题行 +1；
    ///   setext（Setext）→ 连续段落标题行 + 下划线行（HsSep）之后——扫描 PL
    ///     层（标题行 PL=Paragraph、下划线行 PL=HsSep——trilogy 实证）——
    ///     多行段落 setext 自动正确（不依赖固定行数）；下划线行后即节体。
    /// 标题在文档末行（无节体）→ 节尾 = byte_len（title_end >= 节尾 → 读面空
    /// content / 写面零宽 else）。
    pub fn find_title_end(&self, range_start: usize, node_type: BlockType) -> usize {
        let len = self.rope.byte_len();
        if range_start >= len {
            return range_start;
        }
        let line_idx = self.rope.line_of_byte(range_start);
        if crate::effect::line_spec(node_type).mark_width
            == crate::effect::MarkWidth::Setext
        {
            // setext：标题 = 连续段落行 + 下划线行（HsSep）——下划线行后即节体
            // （Paragraph 继续 = 多行段落标题续行；HsSep 处停——body 段落不被吞）。
            let mut l = line_idx;
            while l < self.lines.len() {
                match self.lines[l].node_type() {
                    BlockType::Paragraph => l += 1,
                    BlockType::HsSep => {
                        l += 1;
                        break;
                    }
                    _ => break,
                }
            }
            return if l < self.lines.len() {
                self.rope.byte_of_line(l)
            } else {
                len
            };
        }
        if line_idx + 1 < self.rope.line_len() {
            self.rope.byte_of_line(line_idx + 1)
        } else {
            len
        }
    }

    // ── 写面（对照 Snapshot 写侧方法族）──

    pub fn tag_write_range(
        &self,
        range: &std::ops::Range<usize>,
        node_type: BlockType,
    ) -> Result<std::ops::Range<usize>, String> {
        if matches!(
            crate::effect::line_spec(node_type).tag_rule,
            crate::effect::TagRule::None
        ) {
            return Err(format!("{:?} has no writable tag", node_type));
        }
        match crate::effect::line_spec(node_type).tag_rule {
            crate::effect::TagRule::HeadingText => {
                let line_idx = self.rope.line_of_byte(range.start);
                let ls = self.rope.byte_of_line(line_idx);
                with_line_str(self.rope.line(line_idx), |line| {
                    // P7b：先剥全部 BQ 前缀（strip_bq 单源——与 geometry 的
                    // peel_width 同源），再交给 line_geometry 推导 tag 范围
                    // （mark_end + skip ws）；不再 trim 所有 '#'——`# # foo`
                    // 不会多剥正文。
                    let cleaned = crate::block::peel_peeled(line, 0).rest_after_bq().as_bytes();
                    let bq = line.len() - cleaned.len();
                    let tag_start = crate::block::line_geometry(node_type, cleaned)
                        .map(|g| ls + bq + g.tag_range.start)
                        .unwrap_or(ls + bq);
                    let tag_end = ls + line.trim_end().len();
                    if tag_end > tag_start {
                        Ok(tag_start..tag_end)
                    } else {
                        Err("heading has no tag".into())
                    }
                })
            }
            crate::effect::TagRule::Lang => {
                let line_idx = self.rope.line_of_byte(range.start);
                let ls = self.rope.byte_of_line(line_idx);
                with_line_str(self.rope.line(line_idx), |line| {
                    let trimmed = crate::block::trim_start(line);
                    let lw = line.len() - trimmed.len();
                    // P7b：tag 范围由 line_geometry 推导（mark_end + skip ws），
                    // 不再固定 +3——长 fence 才能取对 lang。
                    let (tag_start, tag_end) = crate::block::line_geometry(node_type, trimmed.as_bytes())
                        .map(|g| (ls + lw + g.tag_range.start, ls + lw + g.tag_range.end))
                        .unwrap_or((ls + lw, ls + line.trim_end().len()));
                    if tag_end > tag_start {
                        Ok(tag_start..tag_end)
                    } else {
                        Ok(tag_start..tag_start)
                    }
                })
            }
            crate::effect::TagRule::Digits => {
                let line_idx = self.rope.line_of_byte(range.start);
                let ls = self.rope.byte_of_line(line_idx);
                with_line_str(self.rope.line(line_idx), |line| {
                    // 几何单源：strip_bq——BQ 内 OL（`> 1. foo`）的数字 tag。
                    let cleaned = crate::block::peel_peeled(line, 0).rest_after_bq().as_bytes();
                    let prefix_len = line.len() - cleaned.len();
                    let b = cleaned;
                    let mut d = 0;
                    while d < b.len() && b[d].is_ascii_digit() {
                        d += 1;
                    }
                    if d > 0 && d < b.len() && (b[d] == b'.' || b[d] == b')') {
                        Ok((ls + prefix_len)..(ls + prefix_len + d))
                    } else {
                        Err("ol has no numeric tag".into())
                    }
                })
            }
            crate::effect::TagRule::ColonKey => {
                let line_idx = self.rope.line_of_byte(range.start);
                let ls = self.rope.byte_of_line(line_idx);
                with_line_str(self.rope.line(line_idx), |line| {
                    // 几何单源：strip_bq（与 build_node 的 peel_width 同源——
                    // 全层 BQ + trim 前导空白）——BQ 内 bullet（`> - k: v`）的
                    // marker 判定必须剥 BQ，否则 b[0] 是 `>` 判定失败。
                    let cleaned = crate::block::peel_peeled(line, 0).rest_after_bq().as_bytes();
                    let prefix_len = line.len() - cleaned.len();
                    let b = cleaned;
                    let marker_len = if !b.is_empty()
                        && (b[0] == b'-' || b[0] == b'*' || b[0] == b'+')
                    {
                        if b.len() == 1 {
                            1
                        } else if b[1] == b' ' || b[1] == b'\t' {
                            2
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    if marker_len > 0 {
                        let inner = &cleaned[marker_len..];
                        let tag_start = ls + prefix_len + marker_len;
                        // P7 查漏：与读面/geometry 同用 inline-skip 的 tag 提取，
                        // 不再裸 find(':')（`- [link](http://x)` 的 URL 冒号）。
                        let range = crate::block::find_colon_or_bracket_tag(inner, 0);
                        if range.start < range.end {
                            Ok(tag_start + range.start..tag_start + range.end)
                        } else {
                            Ok(tag_start..tag_start)
                        }
                    } else {
                        Err("bullet has no tag position".into())
                    }
                })
            }
            crate::effect::TagRule::CheckboxColonKey => {
                let raw = self.rope.byte_slice(range.clone()).to_string();
                let first_line_end = raw.find('\n').unwrap_or(raw.len());
                let first_line = &raw[..first_line_end];
                // P7 查漏：剥 BQ 后交给 find_ckl_tag——colon key 优先、
                // 无 colon 退回 checkbox；不再手写 find('[')/find(']')。
                // `cleaned` 下游要**字节**（`len()` 与 `find_ckl_tag`）⇒ 补 `.as_bytes()`。
                let cleaned = crate::block::peel_peeled(first_line, 0).rest_after_bq().as_bytes();
                let offset = first_line.len() - cleaned.len();
                let tag = crate::block::find_ckl_tag(
                    cleaned,
                    crate::block::CHECKLIST_LIST_MARK_END,
                );
                let line_byte_start = range.start + offset;
                Ok(line_byte_start + tag.start..line_byte_start + tag.end)
            }
            _ => Err(format!("{:?} has no writable tag", node_type)),
        }
    }

    pub fn block_write_range(
        &self,
        target: &crate::block::BlockWriteTarget,
        surface: crate::block::BlockProj,
    ) -> Result<std::ops::Range<usize>, String> {
        let spec = crate::block::block_proj_spec(target.node_type);
        if !spec.can_write.contains(surface) {
            return Err(format!(
                "{:?} write not supported for {:?}",
                surface, target.node_type
            ));
        }
        match surface {
            crate::block::BlockProj::Raw => Ok(target.range.clone()),
            crate::block::BlockProj::Mark | crate::block::BlockProj::Container => {
                Err(format!("{:?} is read-only", surface))
            }
            crate::block::BlockProj::Tag => self.tag_write_range(&target.range, target.node_type),
            crate::block::BlockProj::Content => {
                self.content_write_range(&target.range, target.node_type)
            }
        }
    }

    pub fn proj_write_range(
        &self,
        range: &std::ops::Range<usize>,
        node_type: BlockType,
        proj: &str,
    ) -> Result<std::ops::Range<usize>, String> {
        let surface = crate::block::BlockProj::from_proj_str(proj)
            .ok_or_else(|| format!("unknown projection: {}", proj))?;
        let target = crate::block::BlockWriteTarget {
            range: range.clone(),
            node_type,
        };
        self.block_write_range(&target, surface)
    }

    pub fn compute_write(
        &self,
        node_range: &std::ops::Range<usize>,
        node_type: BlockType,
        proj: &str,
        payload: &str,
    ) -> Result<(std::ops::Range<usize>, String), String> {
        let start_line = self.rope.line_of_byte(node_range.start);
        let end_line = if node_range.end > node_range.start {
            line_of_byte_safe(&self.rope, node_range.end.saturating_sub(1)) + 1
        } else {
            start_line + 1
        };
        let node = crate::block::BlockNode {
            start_line,
            end_line,
            byte_range: node_range.clone(),
            node_type,
            loose: false,
            geometry: None,
            peel_width: 0,
            frame_ordinal: 0,
        };
        // 注意（8.4 偏差入账）：此处 geometry 保持 None——None 路径是
        // transform_payload_from_geometry 内容起点单源化的递归断点
        // （compute_write→transform→compute_write 的终止靠 None 回退支），
        // 补齐几何会造成同范围无限递归（content_write_keeps_sublist 栈溢出实测）。
        // #3/#4 消回退需先重构内容起点单源化的递归结构——账记待办。
        self.compute_write_from_node(&node, proj, payload)
    }

    pub fn compute_write_from_node(
        &self,
        node: &crate::block::BlockNode,
        proj: &str,
        payload: &str,
    ) -> Result<(std::ops::Range<usize>, String), String> {
        use crate::block::BlockProj;
        let surface = BlockProj::from_proj_str(proj)
            .ok_or_else(|| format!("unknown projection: {}", proj))?;
        let spec = crate::block::block_proj_spec(node.node_type);
        if !spec.can_write.contains(surface) {
            return Err(format!(
                "{:?} write not supported for {:?}",
                surface, node.node_type
            ));
        }
        // ── geometry fast path ──
        let write_range = if let Some(ref geo) = node.geometry {
            match surface {
                BlockProj::Raw => node.byte_range.clone(),
                BlockProj::Mark | BlockProj::Container => {
                    return Err(format!("{:?} is read-only", surface))
                }
                BlockProj::Tag => {
                    // 几何单源：peel_width = 容器前缀（BQ 等）剥离宽度——
                    // geometry 的 tag_range 相对剥 BQ 后的行。手算行首空白
                    // 会在 BQ/缩进存在时错位（双源）。
                    let base = self.rope.byte_of_line(node.start_line) + node.peel_width;
                    base + geo.tag_range.start..base + geo.tag_range.end
                }
                BlockProj::Content => {
                    // 多行内容类型（单行几何无法表达内容区间）→ 回退。
                    // 守卫快照（表无「多行内容」列——语义来源标注）：
                    //   content_mode_for ∈ {Code（CF/IC）, Raw（HtmlBlock）}
                    //   ∪ {Comment（None 多行特例）, BQ（Stripped 多行特例）}
                    //   ∪ is_header（NodeSpan）。
                    // 转正期评估：加表列或保持快照 + 本注释的审计断言。
                    if matches!(
                        node.node_type,
                        BlockType::CodeFenceOpen
                            | BlockType::Comment
                            | BlockType::HtmlElement
                            | BlockType::HtmlBlock
                            | BlockType::Blockquote
                    ) || crate::block::is_header(node.node_type)
                    {
                        std::ops::Range::default()
                    } else {
                        // 几何单源：peel_width = 容器前缀（BQ 等）剥离宽度——
                        // geometry 的 tag_range/content_offset 相对剥 BQ 后的行。
                        // 手算行首空白会在 BQ/缩进存在时错位（双源）。
                        let base = self.rope.byte_of_line(node.start_line) + node.peel_width;
                        let content_start = if matches!(
                            crate::effect::line_spec(node.node_type).tag_rule,
                            crate::effect::TagRule::ColonKey
                                | crate::effect::TagRule::CheckboxColonKey
                        ) && geo.tag_range.start < geo.tag_range.end
                        {
                            let pos = base + geo.tag_range.end;
                            // 边界守卫（写面收口计划第一步）：tag 末端恰在行末
                            // （- k: 且为末行无尾换行）时 byte(pos+1) 越界——
                            // 取不到按 0：colon_sep_len(':',0)=1 与「行尾无分隔」一致。
                            let sep_byte = if pos < self.rope.byte_len() { self.rope.byte(pos) } else { 0 };
                            let after_byte = if pos + 1 < self.rope.byte_len() { self.rope.byte(pos + 1) } else { 0 };
                            let sep_len = colon_sep_len(sep_byte, after_byte);
                            pos + sep_len
                        } else {
                            base + geo.content_offset
                        };
                        let mut content_start = content_start;
                        // 单源化：内容起点一律以 content_write_range 的算法为准
                        // （经 compute_write 公共入口）；取不到时回退到上面的几何算法。
                        if let Ok((r, _)) = self.compute_write(
                            &(base..node.byte_range.end),
                            node.node_type,
                            "content",
                            "",
                        ) {
                            content_start = r.start;
                        }
                        // 落点③：键型（含 OL Digits）终点与表同一条规则（末槽右端）——
                        // 不允许两处各写一遍（Digits 曾漏在快路径单行终点——G3 抓出）。
                        let end = if matches!(
                            crate::effect::line_spec(node.node_type).tag_rule,
                            crate::effect::TagRule::ColonKey
                                | crate::effect::TagRule::CheckboxColonKey
                                | crate::effect::TagRule::Digits
                        ) {
                            self.key_content_end(content_start, node.start_line)
                        } else if node.start_line + 1 < self.lines.len() {
                            self.rope.byte_of_line(node.start_line + 1)
                        } else {
                            self.rope.byte_len()
                        };
                        let end = if end > content_start && self.rope.byte(end - 1) == b'\n' {
                            let mut e2 = end - 1;
                            if e2 > content_start && self.rope.byte(e2 - 1) == b'\r' {
                                e2 -= 1;
                            }
                            e2
                        } else {
                            end
                        };
                        content_start..end
                    }
                }
            }
        } else {
            std::ops::Range::default()
        };

        let write_range = if write_range.is_empty() {
            let target = crate::block::BlockWriteTarget {
                range: node.byte_range.clone(),
                node_type: node.node_type,
            };
            self.block_write_range(&target, surface)?
        } else {
            write_range
        };

        // ── payload formatting ──
        let needs_line = crate::effect::is_key_like(
            crate::effect::line_spec(node.node_type).tag_rule)
            && surface == BlockProj::Tag;
        let original_line = if needs_line {
            let line_idx = self.rope.line_of_byte(node.byte_range.start);
            self.rope.line(line_idx).to_string()
        } else {
            String::new()
        };

        let (text, full_range) = self.transform_payload_from_geometry(
            node,
            surface,
            payload,
            &write_range,
            &original_line,
        );
        // PE20 已声明的写端规则：CodeFenceOpen 是 SpanEnd——闭行起点（区间含
        // 尾换行，读面内容也以换行收尾，两者同字节）。旧路径给的是不含尾换行
        // 的终点，写回会多出一空行（写门抓到的围栏那格）。
        let final_range = full_range.unwrap_or(write_range);
        let final_range = if surface == BlockProj::Content
            && node.node_type == BlockType::CodeFenceOpen
            && node.end_line > node.start_line + 1
        {
            let end = self.rope.byte_of_line(node.end_line - 1);
            final_range.start..end.max(final_range.start)
        } else {
            final_range
        };
        Ok((final_range, text))
    }

    /// 写面前缀重发单源（写面收口第四步先行收口）：(All,None) 政策 +
    /// prefix_cut 的单一入口——guard 的 inline_first 判定与逐行前缀发射共用，
    /// 不再各自取政策（先行形态；完整 face 迁移待后续）。
    /// 写面守卫的 BQ 存在性判定（P2-2 政策单源收口）：与发射侧的
    /// write_prefix_cut(All,None) 是一对——守卫只问「本行有没有 BQ 层」（BqOnly），
    /// 发射才取全层前缀。两者共用 prefix_policy 的同族入口，不再各自散写政策。
    fn write_bq_present_cut(&self, line: &str) -> usize {
        crate::infer::prefix_cut(
            line,
            &[],
            &[],
            crate::effect::PrefixKinds::BqOnly,
            crate::effect::PrefixStop::None,
            crate::effect::PrefixLead::SkipWs,
            crate::effect::PrefixTail::None,
        )
        .bytes
    }
    fn write_prefix_cut(&self, nt: crate::block::BlockType, line: &str) -> usize {
        let p = crate::effect::prefix_policy(
            nt,
            crate::effect::PrefixFace::Content,
            crate::effect::PrefixDir::Write,
        );
        crate::infer::prefix_cut(line, &[], &[], p.kinds, p.stop, p.lead, p.tail).bytes
    }
    fn transform_payload_from_geometry(
        &self,
        node: &crate::block::BlockNode,
        surface: crate::block::BlockProj,
        payload: &str,
        write_range: &std::ops::Range<usize>,
        original_line: &str,
    ) -> (String, Option<std::ops::Range<usize>>) {
        use crate::block::BlockProj;
        // P3-B（空文档短路，研究用）：空 rope **没有任何「节点行」可取** —— 整篇写入空文档时
        // 节点 `start_line = 0`，会在下面的 guard 里 `self.rope.line(0)` 越界 panic ✗（`index 0, len 0`）。
        // 空文档本就没有容器前缀可体现 ⇒ 原文直写（与 `-pj raw` 同效）。
        if self.rope.byte_len() == 0 {
            return (payload.to_string(), None);
        }
        let spec = crate::effect::line_spec(node.node_type);
        match (spec.mark_width, spec.tag_rule, surface) {
            (crate::effect::MarkWidth::Fixed("|"), _, BlockProj::Content) => {
                self.transform_payload("content", node.node_type, payload, write_range)
            }
            (_, tr, BlockProj::Content)
                if (crate::effect::is_key_like(tr)
                    && !(node.peel_width > 0 && payload.contains('\n')))
                    || (tr == crate::effect::TagRule::None
                    && node.node_type == BlockType::Blockquote)
                    || (crate::effect::block_decl(node.node_type).is_list_item
                        && payload.trim_end_matches('\n').contains('\n')) =>
            {
                if crate::effect::block_decl(node.node_type).is_list_item
                    && payload.trim_end_matches('\n').contains('\n')
                {
                    // 多行条目内容（S2）：范围右端取节点记录的内容跨度（去尾换行）——
                    // 条目记录本就止于内容行末、不含嵌套子列表（`Bullet 0..2` 之于
                    // `- a` / `  b` / `  - c`），故不会吞子列表。
                    // 两轴续行发射（G3 懒续行恒等修正）：
                    // ① 原文已有的续行 = 保留其实际前缀（懒续行空、缩进续行其缩进——
                    //   写回面恒等）；② 载荷新增行（超出原文行数）= 规范条目缩进
                    //   （保持条目不闭合）；首行落值槽无前缀。
                    let orig = self.rope.byte_slice(write_range.clone()).to_string();
                    let orig_lines: Vec<&str> = orig.lines().collect();
                    // 载荷新增行的规范前缀：
                    //   BQ 内条目 = 该行容器层前缀（保持续行留在容器内——
                    //     A4/R3 的差异根因：裸缩进会把续行写出 BQ）；
                    //   顶层条目 = 条目内容缩进。
                    let node_line = if node.start_line < self.rope.line_len() {
                        self.rope.line(node.start_line).to_string()
                    } else {
                        String::new()
                    };
                    // BQ 内条目的续行前缀 = 容器 BQ 层（懒续行口径，与旧 A 组一致：
                    // 不含条目自身 marker，也不含条目缩进——`> - k` 的续行是 `> B`）。
                    let bq_rest = crate::block::peel_peeled(&node_line, 0).rest_after_bq();
                    let bq_prefix = &node_line[..node_line.len() - bq_rest.len()];
                    let canonical = if node.peel_width > 0 {
                        bq_prefix.to_string()
                    } else {
                        " ".repeat(node.geometry.as_ref().map_or(0, |g| g.content_offset))
                    };
                    let mut out = String::new();
                    let had_trailing_nl = payload.ends_with('\n');
                    let body = payload.trim_end_matches('\n');
                    for (i, pl) in body.split('\n').enumerate() {
                        if i > 0 {
                            out.push('\n');
                        }
                        if i == 0 {
                            out.push_str(pl);
                        } else {
                            // 前缀 = 原文行首空白 − 载荷行首空白（读面 dedent 的逆）——
                            // 恒等时精确还原原文（懒续行 0、缩进续行恢复原缩进）；
                            // 载荷显式多写缩进时前缀归零（用户缩进优先）。
                            let prefix = match orig_lines.get(i).copied() {
                                Some(l) => {
                                    let sw = l.len() - l.trim_start().len();
                                    let pw = pl.len() - pl.trim_start().len();
                                    l[..sw.saturating_sub(pw)].to_string()
                                }
                                None => canonical.clone(),
                            };
                            if pl.is_empty() {
                                out.push_str(prefix.trim_end());
                            } else {
                                out.push_str(&prefix);
                                out.push_str(pl);
                            }
                        }
                    }
                    if had_trailing_nl {
                        out.push('\n');
                    }
                    let mut end = node.byte_range.end;
                    while end > write_range.end && self.rope.byte(end - 1) == b'\n' {
                        end -= 1;
                    }
                    return (out, Some(write_range.start..end));
                }
                if node.node_type == BlockType::Blockquote {
                    // 两轴发射：首行原样（落内容范围），续行 = 首行容器面切片
                    // （位置 + mark 的块级常量，mark 逐行、位置整体） + 载荷行；
                    // 空行用面的 trim 形态（与旧 BqPrefix 空行 ">" 同族）。
                    let first = self.rope.line(node.start_line).to_string();
                    let rest = crate::block::peel_peeled(&first, 0).rest_after_bq();
                    let face = &first[..first.len() - rest.len()];
                    return (crate::block::embody_lines(face, payload, true), None);
                }
                (
                    crate::block::format_payload(node.node_type, "content", payload, "", None, &[], &[]),
                    None,
                )
            }
            (
                _,
                crate::effect::TagRule::Digits
                | crate::effect::TagRule::ColonKey
                | crate::effect::TagRule::CheckboxColonKey,
                BlockProj::Tag,
            ) => {
                // 行首 BLT/OL 层 push 的 consume 单源（kind 与
                // 节点容器 kind 匹配才启用；不匹配回退 format_payload 内的旧解析径）。
                let consume_opt = crate::block::first_item_layer_push(
                    self.entries, node.start_line as u32)
                    .and_then(|(k, c)| {
                        let nk = crate::effect::decl_for(
                            crate::block::frame_kind(node.node_type)
                                .expect("list item node has frame kind")).kind;
                        if k == nk { Some(c) } else { None }
                    });
                // 8.3（#10）：chain/line_entries 供 format_payload 的 container_prefix
                // 折叠（LineCtxReplay 单遍重放——行前栈+行开层条目）。
                let mut ctx = LineCtxReplay::new(self.entries, node.start_line as u32);
                let (chain, pushes) = ctx.line_ctx(node.start_line as u32);
                let text = crate::block::format_payload(
                    node.node_type, "tag", payload, original_line, consume_opt, chain, &pushes);
                // Reconstruct 策略产出完整行（format_payload 的 container_prefix
                // 已含 BQ 前缀 + 行首缩进）——替换范围必须是整行（否则会把
                // tag 范围替换成整行，出现双 marker/双内容；行首空白再加会
                // 与 container_prefix 双算——缩进翻倍）。
                let line_start = self.rope.byte_of_line(node.start_line);
                let line_end = self
                    .rope
                    .byte_of_line(node.start_line + 1)
                    .min(self.rope.byte_len());
                let full_r = Some(line_start..line_end);
                (text, full_r)
            }
            // 容器前缀体现（BUG-E/I 家族架构解——A 组）：
            // 事实 = 节点行携带 BQ 前缀（peel_width>0）× 范围首行是否起始于
            // 前缀区（行首/前缀内 → 首行前缀；行中槽位 → 首行内联）× payload
            // 是否多行（后续行在容器内需逐行前缀）。键是列×事实，非类型枚举——
            // header/paragraph/fence/comment/bullet 多行等未来容器内类型自动覆盖。
            (_, _, BlockProj::Content)
                if (node.peel_width > 0
                    || (node.start_line < self.rope.line_len()
                        && self
                            .write_bq_present_cut(&self.rope.line(node.start_line).to_string())
                            > 0))
                    && (self.range_line1_in_prefix(node, write_range)
                        || payload.contains('\n')) =>
            {
                let inline_first = {
                    // 首行是否内联＝范围起点是否落在「范围首行的容器前缀区」之后。
                    // 基准行必须是 write_range 所在行，不能用 node.start_line：
                    // NodeSpan/TextSpan 的写范围起点在节体/围栏体行
                    // （header/fence 在 BQ 内时 start_line 是标题/开围栏行），
                    // 用标题行算会把「行首」误判成「内联」，payload 裸行落出容器。
                    // 前缀宽取本行 peel 层区间（不能用 peel_width：`- > q` 一族
                    // 的 peel_width 为 0，会把落在行首的范围误判为内联）。
                    // 8.3（#11）：prefix_cut((All,None)) 单源（钉③口径）。
                    let base_line = line_of_byte_safe(&self.rope, write_range.start);
                    let orig0 = if base_line < self.rope.line_len() {
                        self.rope.line(base_line).to_string()
                    } else {
                        String::new()
                    };
                    let w = self.write_prefix_cut(node.node_type, &orig0);
                    write_range.start >= self.rope.byte_of_line(base_line) + w
                };
                // 逐行前缀（统一体现）：载荷第 i 行配原文第 i 行的**容器层前缀**
                // （peel 的层区间；原文行数不足时沿用最后一行）。引用单独一族每行
                // 都是 `> `，与旧口径一致；引用在条目里一族首行 `- > `、续行
                // `  > `——单一前缀表达不了（`- > q` 那格的根因）。
                let orig = self
                    .rope
                    .byte_slice(write_range.clone())
                    .to_string();
                // 读写同源：缩进码的每行前缀（容器前缀 ＋ 内容缩进）由 `ic_line_prefix`
                // 单源给出——与写范围、读面同一份算式；不按栈帧限量取前缀。
                let base_line = line_of_byte_safe(&self.rope, write_range.start);
                let is_ic = node.node_type == crate::block::BlockType::IndentedCode;
                // 逐行前缀现算直写（不物化 Vec<String>——前缀借自 orig 切片；
                // 原文行数不足时沿用最后一行前缀）。
                // 8.3（#8）：非 IC 前缀改 prefix_cut((All,None)) 单源——与 peel
                // 同口径（钉③实证 58k 行 0 分歧）。
                let prefix_of = |i: usize, l: &str| -> String {
                    if is_ic {
                        let (_, prefix, indent) = self.ic_line_prefix(base_line + i);
                        format!("{prefix}{}", " ".repeat(indent))
                    } else {
                        let cut = self.write_prefix_cut(node.node_type, l);
                        l[..cut].to_string()
                    }
                };
                let last = orig
                    .lines()
                    .enumerate()
                    .last()
                    .map(|(i, l)| prefix_of(i, l))
                    .unwrap_or_default();
                let mut out = String::new();
                let mut orig_iter = orig.lines().enumerate();
                for (i, pl) in payload.lines().enumerate() {
                    if i > 0 {
                        out.push('\n');
                    }
                    if !(i == 0 && inline_first) {
                        match orig_iter.next() {
                            Some((_, l)) => out.push_str(&prefix_of(i, l)),
                            None => out.push_str(&last),
                        }
                    } else {
                        // inline_first 首行：不推前缀，但仍消耗第 0 行配对
                        //（保持后续行与原文行的逐行对齐）。
                        let _ = orig_iter.next();
                    }
                    out.push_str(pl);
                }
                if payload.ends_with('\n')
                    && node.node_type != crate::block::BlockType::IndentedCode
                {
                    // 缩进码例外：其写范围**不含尾换行**（与读面 `cd` 同向，实测范围
                    // `21..32` 而在行长 33 之前）⇒ 载荷末尾那一个 `\n` 应视为本行终止符：
                    // 吞掉、不补 ⇒ 带与不带尾换行的载荷落到**同一份文档**（否则多出一空行）。
                    out.push('\n');
                }
                (out, None)
            }
            _ => (payload.to_string(), None),
        }
    }

    /// 写面行级事实：范围首行是否起始于容器前缀区（行首或前缀内）——
    /// 决定 payload 首行的体现形态（前缀 or 内联）。peel_width 与
    /// bq_prefix_of_line 同源（strip_bq/peel 单源）。
    fn range_line1_in_prefix(
        &self,
        node: &crate::block::BlockNode,
        range: &std::ops::Range<usize>,
    ) -> bool {
        if node.peel_width == 0 {
            return false;
        }
        let r_line = self.rope.line_of_byte(range.start);
        let r_line_start = self.rope.byte_of_line(r_line);
        range.start <= r_line_start + node.peel_width as usize
    }

    pub fn transform_payload(
        &self,
        proj: &str,
        node_type: BlockType,
        payload: &str,
        range: &std::ops::Range<usize>,
    ) -> (String, Option<std::ops::Range<usize>>) {
        if matches!(
            crate::effect::line_spec(node_type).mark_width,
            crate::effect::MarkWidth::Fixed("|")
        ) && proj == "content"
        {
            let original = self.rope.byte_slice(range.clone()).to_string();
            let orig_lines: Vec<&str> = original.lines().collect();
            let sep_idx = orig_lines.iter().position(|l| {
                let (_, ranges) = crate::block::cell_ranges_bytes(l.as_bytes());
                !ranges.is_empty()
                    && ranges.iter().all(|r| {
                        let cell = &l.as_bytes()[r.start..r.end];
                        cell.iter()
                            .all(|&b| b == b'-' || b == b':' || b == b' ' || b == b'\t')
                    })
            });
            if let Some(si) = sep_idx {
                let (sep_cols, _) = crate::block::cell_ranges_bytes(orig_lines[si].as_bytes());
                let payload_trimmed = payload.trim();
                let payload_rows: Vec<Vec<&str>> = payload_trimmed
                    .lines()
                    .map(|l| l.split('\t').collect())
                    .collect();
                if payload_rows.iter().all(|r| r.len() == sep_cols) {
                    let mut rebuilt = Vec::new();
                    for (i, row) in payload_rows.iter().enumerate() {
                        rebuilt.push(format!("| {} |", row.join(" | ")));
                        if i == 0 && si < orig_lines.len() {
                            rebuilt.push(orig_lines[si].to_string());
                        }
                    }
                    let new_payload = rebuilt.join("\n");
                    (new_payload, Some(range.clone()))
                } else {
                    let payload_trimmed = payload.trim();
                    let cells: Vec<&str> = payload_trimmed.split('\t').collect();
                    if !cells.is_empty() && cells.iter().all(|c| !c.contains('|')) {
                        let formatted = format!("| {} |", cells.join(" | "));
                        (formatted, Some(range.clone()))
                    } else {
                        (payload.to_string(), None)
                    }
                }
            } else {
                let payload_trimmed = payload.trim();
                let cells: Vec<&str> = payload_trimmed.split('\t').collect();
                if !cells.is_empty() && cells.iter().all(|c| !c.contains('|')) {
                    let formatted = format!("| {} |", cells.join(" | "));
                    (formatted, Some(range.clone()))
                } else {
                    (payload.to_string(), None)
                }
            }
        } else {
            (payload.to_string(), None)
        }
    }

    /// 缩进码的**内容行前缀**——读写同源的唯一算式。按该行 `peel` 层与栈帧的**种类顺序**
    /// 匹配消费容器前缀：引用层受该行栈里的引用帧数限、列表层受**本行新推**的 list 帧数限
    /// （失配即停 ⇒ 内容里的 `>`／`- ` 不被当标记）；内容缩进 ＝ `geo::LIST_CONTENT_MAX`
    /// ＋ 未消费的列表内容边缘。返回 (前缀字节宽, 前缀字符串, 内容缩进列)。
    /// 读面剥它、写面的范围与逐行重发都加它——三处共用这一份，避免各自实现漂移。
    pub fn ic_line_prefix(&self, line: usize) -> (usize, String, usize) {
    /// 读写同源：容器前缀消费与内容缩进由 prefix_cut(All,Counted) 单源给出
        //（R2 对齐——markers_on_line 站点迁移；钉①实证 0 分歧）。
        // 零拷贝：单 chunk 行借用、多 chunk 才回退分配（with_line_str）。
        with_line_str(self.rope.line(line), |text| {
            let pc = crate::infer::prefix_cut(
                text,
                &crate::block::rebuild_stack_at(self.entries, line as u32 + 1),
                crate::infer::line_entries_at(self.entries, line as u32),
                crate::effect::PrefixKinds::All,
                crate::effect::PrefixStop::Counted,
                crate::effect::PrefixLead::SkipWs,
                crate::effect::PrefixTail::None,
            );
            let indent = crate::effect::geo::LIST_CONTENT_MAX + pc.edge;
            (pc.bytes, text[..pc.bytes.min(text.len())].to_string(), indent)
        })
    }

    /// 无字符串版（一眼优化：content_write_range 只取 (cut, indent)，前缀文本
    /// 白分配移除——不构建 String）。
    pub fn ic_line_prefix_cut(&self, line: usize) -> (usize, usize) {
        with_line_str(self.rope.line(line), |text| {
            let pc = crate::infer::prefix_cut(
                text,
                &crate::block::rebuild_stack_at(self.entries, line as u32 + 1),
                crate::infer::line_entries_at(self.entries, line as u32),
                crate::effect::PrefixKinds::All,
                crate::effect::PrefixStop::Counted,
                crate::effect::PrefixLead::SkipWs,
                crate::effect::PrefixTail::None,
            );
            (pc.bytes, crate::effect::geo::LIST_CONTENT_MAX + pc.edge)
        })
    }

    /// 落点③（写面收口计划第二步）：键型条目内容面终点 = 末槽右端——
    /// 从首行沿 cont 位扫到最后一个续行，取其行尾（剥尾换行）；无续行 = 首行行尾
    /// （现状）。类B（含非段落子块）的续行不是 cont 行——自然停在子列表之前。
    /// P-ct 对齐：空值槽 + 子块（bq/嵌套列表）的 bullet，读面
    /// `-pj ct` 含子块，写面原先只到首行值槽（零宽）——/> 残渣、-> no-op。
    /// 这里把 end 沿“更深子块行”延伸，与读面跨度一致；同深度行（兄弟条目）
    /// 与更深行之前的普通续行都不受影响。
    fn key_content_end(&self, content_start: usize, first_line: usize) -> usize {
        let mut last = first_line;
        while last + 1 < self.lines.len() && self.lines[last + 1].cont() {
            last += 1;
        }
        // 子块行走：首行行尾的开放深度为基准，后续行更深（子块）则并入值槽；
        // 但嵌套列表（sublist）不并入——它是独立 item 作用域（content_write_keeps_sublist）。
        let base = self.line_open_depth(first_line);
        let mut child_last = last;
        while child_last + 1 < self.lines.len() {
            let li = child_last + 1;
            if self.line_open_depth(li) <= base {
                break;
            }
            if self.line_pushes_list(li) {
                break;
            }
            child_last += 1;
        }
        last = last.max(child_last);
        let mut e = if last + 1 < self.lines.len() {
            self.rope.byte_of_line(last + 1)
        } else {
            self.rope.byte_len()
        };
        if e > content_start && self.rope.byte(e - 1) == b'\n' {
            e -= 1;
            if e > content_start && self.rope.byte(e - 1) == b'\r' {
                e -= 1;
            }
        }
        e
    }

    /// 行开放深度（处理完该行后的栈深）——按 frame log 重放；供 key_content_end
    /// 判断“后续行是否属于本 item 的子块”。
    fn line_open_depth(&self, line: usize) -> usize {
        let mut depth = 0usize;
        for &e in self.entries {
            let l = entry_line(e) as usize;
            if l > line {
                break;
            }
            if entry_is_pop(e) {
                depth = depth.saturating_sub(1);
            } else if !entry_is_body_mut(e) {
                depth += 1;
            }
        }
        depth
    }

    /// 该行是否 push 了任何 list 容器帧（sublist 行）。
    fn line_pushes_list(&self, line: usize) -> bool {
        self.entries
            .iter()
            .any(|&e| {
                entry_line(e) as usize == line
                    && !entry_is_pop(e)
                    && !entry_is_body_mut(e)
                    && crate::effect::is_list_container(entry_frame(e).kind())
            })
    }
    pub fn content_write_range(
        &self,
        range: &std::ops::Range<usize>,
        node_type: BlockType,
    ) -> Result<std::ops::Range<usize>, String> {
        // 组 C 表化：per-type 手卷分支 → LINE_SPECS 列组合单一 dispatch。
        // 分派键 = (content_rule, mark_shape, tag_rule)——与读面 content_stripped
        // 同构；可达集（can_write Content 门 + geometry 快路径截走后）行为等价类：
        //   NodeSpan×headers → 章节体；TextSpan+Prefix+Lang×fence → 围栏体；
        //   WrapAsymmetric×comment → 标记间（close_mark 接线）；key-like →
        //   值槽；其余（BQ/para/table/html/blank/refdef）→ 整块。
        // 等价性：content_write_range_equiv 测试（legacy oracle 对比）。
        let spec = crate::effect::line_spec(node_type);
        match (spec.content_rule, spec.mark_shape, spec.tag_rule) {
            // 缩进码（IC）：内容跨度——与读面 IC 臂同算式：按该行 peel 层与栈帧的
            // 种类顺序匹配消费容器前缀（两个计数器），内容起点 = 前缀末端 + 4 +
            // 未消费的列表内容边缘；终点取末内容行行末（去尾换行，与读面 cd 不带
            // 尾换行一致）。放在最前，避免 IC 落到下面的 `_ => 整块`。
            _ if matches!(spec.mark_width, crate::effect::MarkWidth::Fixed("    ")) => {
                let start_line = line_of_byte_safe(&self.rope, range.start);
                let last_line = line_of_byte_safe(&self.rope, range.end.saturating_sub(1));
                // 读写同源：容器前缀消费与内容缩进由 `ic_line_prefix` 单源给出。
                // 一眼优化：只取 (cut, indent)——前缀文本白分配移除。
                let (cut, indent) = self.ic_line_prefix_cut(start_line);
                let edge = indent.saturating_sub(crate::effect::geo::LIST_CONTENT_MAX);
                let start = self.rope.byte_of_line(start_line)
                    + cut
                    + crate::effect::geo::LIST_CONTENT_MAX
                    + edge;
                let mut end = self.rope.byte_of_line(last_line)
                    + with_line_str(self.rope.line(last_line), |s| {
                        s.trim_end_matches('\n').len()
                    });
                if end < start {
                    end = start;
                }
                Ok(start..end)
            }
            // NodeSpan：标题章节体（find_title_end 表驱动单源——setext 跳过
            // 下划线行；末行回退 range_start；title_end>=end 零宽 else）。
            // 节体尾 trim 尾随空行（容器闭合空行不属于节体——读面 lines().join
            // 折叠语义对称——追加点漂移修复：append 不再落到容器外）。
            (crate::effect::ContentRule::NodeSpan, _, _) => {
                let title_end = self.find_title_end(range.start, node_type);
                if title_end < range.end {
                    let mut end = range.end;
                    while end > title_end {
                        let last_line = line_of_byte_safe(&self.rope, end.saturating_sub(1));
                        let line_start = self.rope.byte_of_line(last_line);
                        if line_start < end {
                            let is_blank = with_line_str(
                                self.rope.line(last_line),
                                |s| s.trim().is_empty(),
                            );
                            if is_blank {
                                end = line_start;
                                continue;
                            }
                        }
                        break;
                    }
                    Ok(title_end..end)
                } else {
                    Ok(range.end..range.end)
                }
            }
            // TextSpan+Prefix+Lang：围栏体（CodeFenceOpen 唯一可达；CodeFenceClose
            // 同键但 can_write no_w 门不可达——歧义由门消解）。
            (
                crate::effect::ContentRule::TextSpan,
                crate::effect::MarkShape::Prefix,
                crate::effect::TagRule::Lang,
            ) => {
                let s = self.rope.line_of_byte(range.start);
                let e = line_of_byte_safe(&self.rope, range.end.saturating_sub(1));
                if e > s + 1 {
                    let start = self.rope.byte_of_line(s + 1);
                    let end = self.rope.byte_of_line(e);
                    Ok(start..end)
                } else {
                    // 空围栏（开行后紧跟闭行）→ 零宽于起始（等价保留）
                    Ok(range.start..range.start)
                }
            }
            // WrapAsymmetric：注释标记间（close_mark 表化——替代 rfind("-->")
            // 硬编码——Comment 唯一 WrapAsymmetric 类型）。
            (_, crate::effect::MarkShape::WrapAsymmetric, _) => {
                let raw = self.rope.byte_slice(range.clone()).to_string();
                let trimmed_start = raw.find("<!--").unwrap_or(0);
                let content_start = trimmed_start + crate::block::comment_mark_width();
                let content_end = raw
                    .rfind(crate::effect::close_mark(node_type).unwrap_or("-->"))
                    .unwrap_or(raw.len());
                let abs_start = range.start + content_start;
                let abs_end = range.start + content_end;
                if abs_end > abs_start {
                    Ok(abs_start..abs_end)
                } else {
                    // 空内容（<!---->）→ 整块（等价保留）
                    Ok(range.clone())
                }
            }
            // BQ 写面与读面同构（第四步修订）：start = 首行剥自身 mark 后
            // （与读面 bq_content 同算式 rest_after_bq——mark 逐行事务、位置留外），
            // end = 块尾剥尾换行（BQ 内容行 = 块的各行，含空 > 行——不用 cont 扫尾，
            // cont 位对 bq 空行不成立）。
            (crate::effect::ContentRule::AfterMark, _, crate::effect::TagRule::None) => {
                let line_idx = self.rope.line_of_byte(range.start);
                let first = self.rope.line(line_idx).to_string();
                let rest = crate::block::peel_peeled(&first, 0).rest_after_bq();
                let consumed = first.len() - rest.len();
                let start = self.rope.byte_of_line(line_idx) + consumed;
                let mut end = range.end;
                if end > start && self.rope.byte(end - 1) == b'\n' {
                    end -= 1;
                    if end > start && self.rope.byte(end - 1) == b'\r' {
                        end -= 1;
                    }
                }
                Ok(start..end)
            }
            // key-like：值槽（ColonKey/CheckboxColonKey/Digits 三分支——
            // 现状语义逐字节保留：真空 marker 整块（marker_len<2）、无冒号
            // skip+1、bracket 冒号前全段 tag、Digits 无 marker 检查与
            // digit-scan 退化起点、end=行尾/节点尾）。
            (_, _, tr) if crate::effect::is_key_like(tr) => {
                let line_idx = self.rope.line_of_byte(range.start);
                let mut ls = self.rope.byte_of_line(line_idx);
                // 几何单源：strip_bq（与 build_node 的 peel_width 同源——全层 BQ
                // + trim 前导空白）——消除双源（BQ 剥离语义必须与读面/geometry 一致）。
                // 对齐补漏（#9 迁移）：BQ 剥改 prefix_cut 单源——(BqOnly,None,
                // SkipWs,FinalSkip) ≡ peel_peeled rest_after_bq（content_face 同参）。
                let (bq_off, line) = with_line_str(self.rope.line(line_idx), |l| {
                    let c = crate::infer::prefix_cut(
                        l,
                        &[],
                        &[],
                        crate::effect::PrefixKinds::BqOnly,
                        crate::effect::PrefixStop::None,
                        crate::effect::PrefixLead::SkipWs,
                        crate::effect::PrefixTail::FinalSkip,
                    );
                    let off = c.bytes.min(l.len());
                    (off, crate::block::str_view(&l.as_bytes()[off..]).to_string())
                });
                ls += bq_off;
                let mut lw = line.len() - crate::block::trim_start(&line).len();
                // 同行链双层同种壳（`- - a` 一族）：本块是链上**最后一个**列表层，
                // 故要跳过前面 N-1 个标记（N = 本行列表层数；单层时 N-1 = 0，行为
                // 不变）。对齐补漏（#9 迁移）：层计数与 N-1 跳过改 layers 副产品
                // （消 peel 全链重认——同量单源）；跳过的层段只认「前 N-1 个被消费
                // 层全是列表层」的连续前缀（夹层 BQ 形状保持旧 break 语义→整块）。
                let cut = crate::infer::prefix_cut(
                    &line[lw..],
                    &[],
                    &[],
                    crate::effect::PrefixKinds::All,
                    crate::effect::PrefixStop::None,
                    crate::effect::PrefixLead::SkipWs,
                    crate::effect::PrefixTail::None,
                );
                let list_pos: smallvec::SmallVec<[usize; 4]> = cut
                    .layers
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| crate::effect::is_list_container(l.kind))
                    .map(|(i, _)| i)
                    .collect();
                let n_list = list_pos.len();
                if n_list >= 2 && list_pos[n_list - 2] == n_list - 2 {
                    // 前 N-1 层全为列表层（连续前缀）——跳到最后层的 start。
                    lw += cut.layers[list_pos[n_list - 2]].end;
                }
                let b = &line.as_bytes()[lw..];
                // marker_len 单源（#9）：行首一眼量 step_layer（认 BLT/OL——
                // OL 跳标缺口闭合：外层 OL 层被 layers 跳过、本块 OL 标记被认）；
                // 前导空白非零 → 0（旧「b 含 ws 非 marker → 整块」语义保留）；
                // 非列表层（夹层 BQ）→ 0。
                let marker_len = if b.len() > crate::block::skip_leading_whitespace(b).len() {
                    0
                } else {
                    crate::block::step_layer(b)
                        .filter(|(k, _)| crate::effect::is_list_container(*k))
                        .map_or(0, |(_, consumed)| consumed)
                };
                let content_start = match tr {
                    crate::effect::TagRule::ColonKey => {
                        if marker_len < 2 {
                            return Ok(range.clone());
                        }
                        let inner = &line[lw + marker_len..];
                        let mut cs = lw + marker_len;
                        let tag_range =
                            crate::block::find_colon_or_bracket_tag(inner.as_bytes(), 0);
                        if tag_range.start < tag_range.end {
                            if inner.as_bytes().get(tag_range.end) == Some(&b':') {
                                cs += tag_range.end + 1;
                                // 读写正逆（G3 行132 实证）：值前导空白 = 分隔符恰一个
                                // （": "），多出的空白是值自身内容（读面 content_face/
                                // 几何 content_offset 口径）——不再全剥。
                                let rest = &inner[tag_range.end + 1..];
                                cs += usize::from(rest.starts_with(' ') || rest.starts_with('\t'));
                            } else {
                                cs += tag_range.end + 1;
                            }
                        }
                        cs
                    }
                    crate::effect::TagRule::CheckboxColonKey => {
                        if marker_len < 2 {
                            return Ok(range.clone());
                        }
                        let inner = &line[lw + marker_len..];
                        let mut cs = lw + marker_len;
                        let tag_range =
                            crate::block::find_colon_or_bracket_tag(inner.as_bytes(), 0);
                        if tag_range.start < tag_range.end {
                            if inner.as_bytes().get(tag_range.end) == Some(&b':') {
                                cs += tag_range.end + 1;
                                // 读写正逆（G3 行132 实证）：值前导空白 = 分隔符恰一个
                                // （": "），多出的空白是值自身内容（读面 content_face/
                                // 几何 content_offset 口径）——不再全剥。
                                let rest = &inner[tag_range.end + 1..];
                                cs += usize::from(rest.starts_with(' ') || rest.starts_with('\t'));
                            } else {
                                cs += tag_range.end + 1;
                            }
                        }
                        cs
                    }
                    _ => {
                        // Digits：裸 digit-scan + 定界符（不要求定界符后空白——
                        // 1.x 退化起点等价保留；parse_ol 语义不同属函数级直调面）
                        let mut d = 0;
                        while d < b.len() && b[d].is_ascii_digit() {
                            d += 1;
                        }
                        if d > 0 && d < b.len() && (b[d] == b'.' || b[d] == b')') {
                            // G3 恒等修正：值起点与读面同构——定界符后的
                            // 值前导空白不是内容（1.  foo 与 1. foo 的内容同为 foo）。
                            let mut cs = d + 2;
                            while cs < b.len() && (b[cs] == b' ' || b[cs] == b'\t') {
                                cs += 1;
                            }
                            lw + cs
                        } else {
                            lw
                        }
                    }
                };
                let content_start = ls + content_start;
                let use_line_end = matches!(
                    tr,
                    crate::effect::TagRule::ColonKey | crate::effect::TagRule::CheckboxColonKey
                );
                let end = if use_line_end {
                    // 落点③：末槽右端（cont 续行扫尾），不再只取本行行尾。
                    self.key_content_end(content_start, line_idx)
                } else {
                    let mut e = range.end;
                    if e > content_start && self.rope.byte(e - 1) == b'\n' {
                        e -= 1;
                        if e > content_start && self.rope.byte(e - 1) == b'\r' {
                            e -= 1;
                        }
                    }
                    e
                };
                Ok(content_start..end)
            }
            // 其余（BQ/Paragraph/TableRow/TableSeparator/HtmlBlock/HtmlElement/
            // Blank/RefDef 等）：整块（现状 else 语义）
            _ => Ok(range.clone()),
        }
    }
}



// §6 trilogy（正逆闭环比对——restore ∘ build == scan）



/// 树 → 逐行 PL（还原——R1 块类型真相 / R2 setext 展开 / R3 壳开行 /
/// R11 最内层壳原则）。独立反推实现（restore ∘ build == scan 的左半）。
pub fn restore_pl(
    tree: &AttrTreeNew,
    blocks: &[BlockRec],
    records: &[ContainerRec],
    view: &LineView,
    rope: &crop::Rope,
    lines: &[PackedLineInfo],
    n: usize,
) -> Vec<BlockType> {
    let mut out = vec![BlockType::Blank; n];
    let node_count = tree.nodes.len();
    let mut parent_of: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for i in 0..node_count {
        for &c in tree.nodes[i].children_of(tree) {
            parent_of.insert(c as usize, i);
        }
    }
    // 覆盖链扫掠：活动集 = {eligible ∧ start<=i ∧ i<end}；
    // 排序键 (width, Reverse(idx)) 与行号无关，BTreeSet 序确定。
    let mut node_order: Vec<usize> = (0..node_count).collect();
    node_order.sort_by_key(|&idx| (tree.nodes[idx].start, idx));
    let mut active: std::collections::BTreeSet<(u32, std::cmp::Reverse<usize>)> = std::collections::BTreeSet::new();
    let mut expiring: std::collections::BinaryHeap<std::cmp::Reverse<(u32, u32, std::cmp::Reverse<usize>)>> =
        std::collections::BinaryHeap::new();
    let mut next_node = 0usize;
    for i in 0..n {
        let li = i as u32;
        while next_node < node_count && tree.nodes[node_order[next_node]].start <= li {
            let idx = node_order[next_node];
            next_node += 1;
            let nd = &tree.nodes[idx];
            // 章节壳（无 rec 无 blk 无 is_item）是组织壳不是内容源——不参与
            // 内容层还原（其覆盖的行由内容节点或空洞回退决定——HS 与 CF
            // 语义一致：sec 只重组归属，不改内容真相）。
            if (nd.kind.is_some() || nd.blk_idx().is_some() || nd.is_item()) {
                let width = nd.end - nd.start;
                let key = (width, std::cmp::Reverse(idx));
                active.insert(key);
                expiring.push(std::cmp::Reverse((nd.end, width, std::cmp::Reverse(idx))));
            }
        }
        while let Some(std::cmp::Reverse((e, w, k))) = expiring.peek().copied() {
            if e <= li {
                expiring.pop();
                active.remove(&(w, k));
            } else {
                break;
            }
        }
        // 覆盖链（宽度升序——链头=最内层最窄；同宽后建=内层）。
        let chain: Vec<usize> = active.iter().map(|&(_, std::cmp::Reverse(idx))| idx).collect();
        if chain.is_empty() {
            // 空洞回退：树节点区间不覆盖的行——两类：
            //   a) 透明帧（树里无节点——伪表 / rd 撤销）的内容行；
            //   b) 容器的关闭行（end_includes_close=false——Pop 行不属于块，
            //      但其 PL 是容器的关闭内容——Table 的尾分隔行等）。
            // 反读容器记录：Table → TableSeparator、RefDef → RefDef、
            // 其他 → Blank。
            let mut t = BlockType::Blank;
            for r in records {
                let end = r.end_usize().unwrap_or(n);
                let inside = r.start as usize <= i && i < end;
                let closing = i == end && !r.end_includes_close();
                if inside {
                    // 区间内内容行：Table → TableSeparator、RefDef → RefDef
                    // （多行 title 续行）、其他 → Blank。
                    t = match r.frame.kind() {
                        FrameKind::Table => BlockType::TableSeparator,
                        FrameKind::RefDef => BlockType::RefDef,
                        _ => BlockType::Blank,
                    };
                    break;
                }
                if closing && r.frame.kind() == FrameKind::Table {
                    // 关闭行（Pop 行不属于块）：按行内容镜像——空行 → Blank、
                    // 分隔行（单元格仅 -/:/空白）→ TableSeparator——
                    // 其余关闭行由自己的块/节点覆盖。
                    t = with_line_str(rope.line(i), |s| {
                        if s.trim().is_empty() {
                            BlockType::Blank
                        } else {
                            let (_, ranges) = crate::block::cell_ranges_bytes(s.as_bytes());
                            let is_sep = !ranges.is_empty()
                                && ranges.iter().all(|r| {
                                    s.as_bytes()[r.start..r.end].iter().all(|&b| {
                                        b == b'-' || b == b':' || b == b' ' || b == b'\t'
                                    })
                                });
                            if is_sep { BlockType::TableSeparator } else { BlockType::Blank }
                        }
                    });
                    break;
                }
            }
            out[i] = t;
            continue;
        }
        // R3：最外层 PL 型壳（BQ/HE）开行——复合开行 PL=壳类型。
        let mut pl_shell: Option<BlockType> = None;
        for &idx in chain.iter().rev() {
            let nd = &tree.nodes[idx];
            let shell_t = match nd.kind {
                Some(FrameKind::BlockQuote) => Some(BlockType::Blockquote),
                Some(FrameKind::HtmlElement) => Some(BlockType::HtmlElement),
                _ => None,
            };
            let Some(st) = shell_t else { continue; };
            if tree.nodes[idx].start as usize != i {
                continue;
            }
            let inner_override = chain.iter().find(|&&j| {
                let n = &tree.nodes[j];
                n.start as usize == i && n.kind.is_none() && n.blk_idx().is_some() && !n.is_item()
            });
            pl_shell = Some(match inner_override {
                Some(&j) => {
                    let bi = tree.nodes[j].blk_idx().unwrap();
                    let bt = blocks[bi].block_type;
                    let mut has_open_parent = false;
                    let mut p = parent_of.get(&idx).copied();
                    while let Some(pi) = p {
                        let pk = tree.nodes[pi].kind;
                        if pk == Some(FrameKind::BlockQuote)
                            && (tree.nodes[pi].start as usize) < i
                        {
                            has_open_parent = true;
                            break;
                        }
                        p = parent_of.get(&pi).copied();
                    }
                    // 守卫快照（表无「壳开行 PL 覆盖集」列）：复合开行上不强加
                    // 自身 PL 的内容类块集合——scan 侧 dispatch after_ws 分支的
                    // 经验镜像（段落型内容 → 壳类型；其余内容块 → 块类型）。
                    if !has_open_parent
                        && matches!(
                            bt,
                            BlockType::Paragraph
                                | BlockType::Blank
                                | BlockType::HsHead1
                                | BlockType::HsHead2
                        )
                    {
                        st
                    } else {
                        bt
                    }
                }
                None => st,
            });
            break;
        }
        if let Some(t) = pl_shell {
            out[i] = t;
            continue;
        }
        // 链头（最内层）——PL 层内容块节点优先，再 item 段，然后壳（R11）。
        // 与 scan effective_outer 的内容层信任成对(正逆同源律):容器开行
        // 的 PL 层内容块(hb/comment/ATX 标题)还原 = 块类型。排除 HsHead:
        // setext 标题身份在 content_kind 层、PL 层仍是 item 开行(scan 侧
        // effective_outer 对 setext 的 final_content=Paragraph 也回 item
        // 层——同规则);与 inner_override(壳开行覆盖集)的 HsHead 排除同构。
        let blk_content = chain.iter().find(|&&j| {
            let n = &tree.nodes[j];
            !n.is_item()
                && n.blk_idx().is_some()
                && !matches!(
                    blocks[n.blk_idx().unwrap()].block_type,
                    BlockType::HsHead1 | BlockType::HsHead2
                )
        });
        let idx = blk_content
            .or_else(|| chain.iter().find(|&&j| tree.nodes[j].is_item()))
            .or_else(|| chain.iter().find(|&&j| tree.nodes[j].blk_idx().is_some()))
            .copied()
            .or_else(|| chain.first().copied())
            .unwrap();
        let nd = &tree.nodes[idx];
        if nd.is_item() && nd.blk_idx().is_none() {
            out[i] = if i == nd.start as usize {
                nd.block_type
            } else {
                BlockType::HsSep
            };
            continue;
        }
        if let Some(bi) = nd.blk_idx() {
            let b = &blocks[bi];
            if i < b.start as usize || i >= b.end as usize {
                if let Some(k) = nd.kind {
                    out[i] = match k {
                        FrameKind::Table => BlockType::TableSeparator,
                        _ => BlockType::Blank,
                    };
                }
                continue;
            }
            let bt = b.block_type;
            // 逆改写（正逆同规则）：伪表行 / rd 撤销行还原词法真相。
            let bt = crate::effect::context_restore(
                bt, lines[i].node_type(),
                &crate::effect::BlockRewriteCtx { view, rope }, i,
            ).unwrap_or(bt);
            out[i] = if i == b.start as usize {
                if matches!(bt, BlockType::HsHead1 | BlockType::HsHead2) {
                    BlockType::Paragraph
                } else {
                    bt
                }
            } else if i == b.end as usize - 1
                && matches!(bt, BlockType::HsHead1 | BlockType::HsHead2)
            {
                BlockType::HsSep
            } else {
                match bt {
                    BlockType::CodeFenceOpen => {
                        // 关闭行判定用 end_includes_close（约束关闭——
                        // BQ 内 fence 被无前缀行关——无 CFClose 行——
                        // end.is_some() 会把末内容行误判为关闭行）。
                        let includes_close = nd
                            .rec_idx()
                            .map_or(false, |rid| records[rid].end_includes_close());
                        if i == b.end as usize - 1 && includes_close {
                            BlockType::CodeFenceClose
                        } else {
                            BlockType::CodeContent
                        }
                    }
                    BlockType::Comment => BlockType::Comment,
                    BlockType::HtmlBlock => {
                        let has_open = with_line_str(rope.line(i), |line| {
                            let tokens = crate::block::tokenize_html_line(line.as_bytes());
                            tokens.iter().any(|t| {
                                matches!(
                                    t,
                                    crate::block::HtmlLineToken::Open { .. }
                                        | crate::block::HtmlLineToken::Inline
                                )
                            })
                        });
                        if has_open {
                            BlockType::HtmlElement
                        } else {
                            BlockType::HtmlContent
                        }
                    }
                    BlockType::RefDef => BlockType::RefDef,
                    BlockType::IndentedCode => {
                        if with_line_str(rope.line(i), |s| s.trim().is_empty()) {
                            BlockType::Blank
                        } else {
                            BlockType::IndentedCode
                        }
                    }
                    _ => crate::effect::context_restore(
                        bt, lines[i].node_type(),
                        &crate::effect::BlockRewriteCtx { view, rope }, i,
                    )
                    .unwrap_or(BlockType::Paragraph),
                }
            };
        } else if let Some(k) = nd.kind {
            out[i] = match k {
                FrameKind::BlockQuote => BlockType::Blockquote,
                FrameKind::HtmlElement => BlockType::HtmlElement,
                _ => BlockType::Blank,
            };
        }
    }
    out
}

/// 树 → 帧流（还原——F1 壳 Push / F2 壳 Pop（closing 语义反读）/
/// F3 段弹推 / BMut（loose + 表确认）/ F5 透明帧）。
/// 返回 (行, FrameKind, op)：op 0=Push 1=Pop 2=BMut。
pub fn restore_fl(
    tree: &AttrTreeNew,
    records: &[ContainerRec],
    blocks: &[BlockRec],
    lines: &[PackedLineInfo],
    n: usize,
) -> Vec<(u32, FrameKind, u8)> {
    use FrameKind as FK;
    let node_count = tree.nodes.len();
    // 容器嵌套深度——records[rid].parent 链（非树 children：HS 的章节
    // 重组会把容器节点摘挂到章节壳下，树父子 ≠ 容器嵌套——同行 Pop 的
    // 「内层先关」排序必须用容器真相）。
    let depth_of = |rid: usize| -> u32 {
        let mut d = 0u32;
        let mut p = records[rid].parent;
        while p != u32::MAX {
            d += 1;
            p = records[p as usize].parent;
        }
        d
    };
    let mut out: Vec<(u32, FK, u8, u32)> = Vec::new();
    // 1. 壳节点（rec=Some 且 kind=Some——非透明壳）。
    let mut shells: Vec<(u32, u32, FK, usize, usize)> = (0..node_count)
        .filter_map(|idx| {
            tree.nodes[idx]
                .rec_idx()
                .zip(tree.nodes[idx].kind)
                .map(|(r, k)| (tree.nodes[idx].start, tree.nodes[idx].end, k, r, idx))
        })
        .collect();
    shells.sort_by_key(|&(s, _, k, _, _)| (s, k as u8));
    for &(s, _e, k, rid, _idx) in &shells {
        let d = depth_of(rid);
        out.push((s, k, 0, d)); // Push
        if let Some(end) = records[rid].end {
            // F2：Pop 行 = end - includes_close（记录携带的结算语义——
            // 约束关闭时关闭行不在块内——Pop@endo）。
            let end_v = end.get();
            let pop_line = end_v - records[rid].end_includes_close() as u32;
            if (pop_line as usize) < n {
                out.push((pop_line, k, 1, d));
            }
        }
    }
    // 1.5/1.6 TableRow 弹推 + BLT/OL BMut + Table BMut。
    // gap BMut 候选（先收集后合并——resolve_pending_loose 每行只取最内层
    // 未 loose 帧——同行多壳候选取 max depth）。
    let mut gap_bmuts: Vec<(u32, FK, u32)> = Vec::new();
    let shell_recs: std::collections::HashSet<usize> =
        shells.iter().map(|&(_, _, _, rid, _)| rid).collect();
    for &(s, e, k, rid, _idx) in &shells {
        let d = depth_of(rid);
        if k == FK::TableRow {
            let mut row_starts: Vec<u32> = (0..node_count)
                .filter(|&j| {
                    tree.nodes[j].blk_idx().is_some()
                        && tree.nodes[j].start >= s
                        && tree.nodes[j].end <= e
                })
                .map(|j| tree.nodes[j].start)
                .collect();
            row_starts.sort();
            row_starts.dedup();
            for &rs in row_starts.iter().skip(1) {
                out.push((rs, k, 1, d));
                out.push((rs, k, 0, d));
            }
        } else if crate::effect::decl_for(k).has_loose {
            let mut contents: Vec<(u32, u32)> = (0..node_count)
                .filter(|&j| {
                    tree.nodes[j].blk_idx().is_some()
                        && tree.nodes[j].start >= s
                        && tree.nodes[j].end <= e
                })
                .map(|j| (tree.nodes[j].start, tree.nodes[j].end))
                .collect();
            contents.sort();
            contents.dedup();
            let mut first_gap = true;
            for w in contents.windows(2) {
                // 间隙须含空行（Blank）——嵌套内容间隙不算（scan 的 pending
                // 只在空行收集——P9/生成语料实测）。
                let has_blank = w[1].0 > w[0].1
                    && (w[0].1 as usize..w[1].0 as usize).any(|j| {
                        j < lines.len() && lines[j].node_type() == BlockType::Blank
                    });
                if has_blank && first_gap {
                    gap_bmuts.push((w[1].0, k, d));
                    first_gap = false;
                }
            }
        } else if k == FK::Table {
            if let Some(bi) = (0..node_count)
                .find(|&j| tree.nodes[j].rec_idx() == Some(rid) && tree.nodes[j].blk_idx().is_some())
                .and_then(|j| tree.nodes[j].blk_idx())
            {
                let b = &blocks[bi];
                let sep_line = b.end;
                if (sep_line as usize) < n {
                    out.push((sep_line, k, 2, d));
                }
            }
        }
    }
    // 1.7 gap BMut 合并（同行取最内层——resolve_pending_loose 语义）。
    gap_bmuts.sort_by_key(|&(l, k, _d)| (l, k as u8));
    let mut merged_bmuts: Vec<(u32, FK, u32)> = Vec::new();
    for (l, k, d) in gap_bmuts {
        if let Some(last) = merged_bmuts.last_mut() {
            if last.0 == l {
                if d > last.2 {
                    *last = (l, k, d);
                }
                continue;
            }
        }
        merged_bmuts.push((l, k, d));
    }
    // 2. 段节点（item）——非首段的弹推（覆盖 start 的最内层列表壳）。
    let mut items: Vec<(u32, u32, Option<usize>)> = (0..node_count)
        .filter(|&j| tree.nodes[j].is_item())
        .map(|j| (tree.nodes[j].start, tree.nodes[j].end, tree.nodes[j].rec_idx()))
        .collect();
    items.sort();
    let mut per_start: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for &(s, _e, rec) in &items {
        let seq = per_start.entry(s).or_insert(0);
        *seq += 1;
        if let Some(rid) = rec {
            let k = records[rid].frame.kind();
            out.push((s, k, 0, 0));
            continue;
        }
        if *seq > 1 {
            continue;
        }
        let mut best: Option<(u32, u32, u32, FK, usize)> = None;
        for &(cs, ce, k, _, idx) in &shells {
            if cs <= s && s < ce && crate::effect::decl_for(k).has_loose {
                let w = ce - cs;
                if best.map_or(true, |(bw, _, _, _, _)| w < bw) {
                    best = Some((w, cs, ce, k, idx));
                }
            }
        }
        if let Some((_, cs, _ce, k, idx)) = best {
            if cs < s {
                let d = tree.nodes[idx].rec_idx().map(|r| depth_of(r)).unwrap_or(0);
                out.push((s, k, 1, d));
                out.push((s, k, 0, d));
            }
        }
    }
    // 3. F5 透明帧（伪表 / rd 撤销——树里无壳节点的容器记录）。
    let mut transparent: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for b in blocks {
        let mut rid = b.container_ref as usize;
        while rid != u32::MAX as usize {
            if !shell_recs.contains(&rid) {
                transparent.insert(rid);
            }
            let parent = records[rid].parent as usize;
            if parent == u32::MAX as usize {
                break;
            }
            rid = parent;
        }
    }
    for &rid in &transparent {
        let r = &records[rid];
        let k = r.frame.kind();
        let d = depth_of(rid);
        out.push((r.start, k, 0, d));
        if let Some(end) = r.end {
            let end_v = end.get();
            let pop_line = end_v - r.end_includes_close() as u32;
            if (pop_line as usize) < n {
                out.push((pop_line, k, 1, d));
            }
        }
    }
    // 4. 排序（规范序——无时间箭头）：同行 Pop 先（深度降序——内层先关）、
    //    Push 次（深度升序——外层先开）、BMut 恒末（body_mut = 行尾快照
    //    语义直出——且天然不插入 has_pop_push 帧的同 kind Pop-Push 相邻对，
    //    配对判定不受影响）。scan 的 BMut 前置/中置是实现时点——重放等价
    //    （body_mut 按 kind+indent 匹配非栈顶）类内自由——正逆同列律：
    //    restore 从表推导规范序，不镜像 scan 动作序（闭环比对标准 = 重放
    //    等价——多重集 + 行尾栈 kind 序列——见 trilogy_testbed）。
    for (l, k, d) in merged_bmuts {
        out.push((l, k, 2, d));
    }
    out.sort_by_key(|&(l, k, op, d)| {
        let op_order = match op { 1 => 0, 0 => 1, _ => 2 };
        let depth_key = if op == 1 { u32::MAX - d } else { d };
        (l, op_order, depth_key, k as u8)
    });
    out.into_iter().map(|(l, k, op, _)| (l, k, op)).collect()
}

/// Table 确认位图（对照 table_confirmed_flags 逐值等价）——跳跃扫描机械。
/// 每个 tentative Table Push（body bit0==0）前扫到配对 Pop，检查确认分隔行
/// （Table body_mut bit0==1），确认则整帧区间置位。含帧开行。
/// kind 只做相等比较（Table），无枚举列表。
pub fn table_confirmed_flags(entries: &[u64], zone: (u32, u32)) -> Vec<bool> {
    let (zs, ze) = (zone.0 as usize, zone.1 as usize);
    let zlen = ze.saturating_sub(zs);
    let mut flags = vec![false; zlen];
    let mut ei = partition(entries, zone.0);
    while ei < entries.len() {
        let e = entries[ei];
        let el = entry_line(e) as usize;
        if el >= ze {
            break;
        }
        if !entry_is_pop(e) && !entry_is_body_mut(e) {
            let f = entry_frame(e);
            if f.kind() == FrameKind::Table && f.body() & 1 == 0 {
                let start = el;
                let mut d: i32 = 1;
                let mut is_confirmed = false;
                let mut ej = ei + 1;
                while ej < entries.len() && d > 0 {
                    let ejv = entries[ej];
                    if entry_is_body_mut(ejv) {
                        let mf = entry_frame(ejv);
                        if mf.kind() == FrameKind::Table && (mf.body() & 1) != 0 {
                            is_confirmed = true;
                        }
                    } else if !entry_is_pop(ejv) && entry_frame(ejv).kind() == FrameKind::Table {
                        d += 1;
                    } else if entry_is_pop(ejv) && entry_frame(ejv).kind() == FrameKind::Table {
                        d -= 1;
                    }
                    ej += 1;
                }
                if is_confirmed {
                    let end = if d == 0 { entry_line(entries[ej - 1]) as usize } else { ze };
                    let lo = start.max(zs);
                    let hi = end.min(ze);
                    for li in lo..hi {
                        flags[li - zs] = true;
                    }
                }
                ei = ej;
                continue;
            }
        }
        ei += 1;
    }
    flags
}


// 通用工具链(自 incr/engine 迁入——zone 闭包 / ripple 对账 / 标题扫描)


/// 阶段 1 纯推导(compute_zone 与缓存物化共用——禁止复制):
/// 编辑行区间(旧侧 sf_o/sl_o、新侧 sf_n/sl_n)+ ripple 区间 + delta →
/// 双侧原始 cs/ce(与 compute_zone 阶段 1 逐值同款)。
/// 注:cs_o 公式引用新侧 rs 与 rs-delta(坐标混合 hull——compute_zone 原语义)。
pub(crate) fn zone_derive_stage1(
    n_old: usize,
    n_new: usize,
    delta: isize,
    rs: usize,
    re: usize,
    sf_o: usize,
    sl_o: usize,
    sf_n: usize,
    sl_n: usize,
) -> (usize, usize, usize, usize) {
    let cs_n = rs.min(sf_n).min(n_new.saturating_sub(1));
    let ce_n = (re + 1)
        .max(sl_n.saturating_add(1))
        .max(cs_n.saturating_add(1))
        .min(n_new);
    let cs_o = rs
        .min(sf_o)
        .min(n_old.saturating_sub(1))
        .min(((rs as isize - delta).max(0) as usize).min(n_old.saturating_sub(1)));
    let re_old = ((re as isize - delta).max(0) as usize)
        .max(rs)
        .min(n_old.saturating_sub(1));
    let ce_o = (re_old + 1)
        .max(sl_o.saturating_add(1))
        .max(((rs as isize - delta).max(0) as usize).saturating_add(1))
        .min(n_old)
        .max(cs_o.saturating_add(1));
    (cs_o, ce_o, cs_n, ce_n)
}

/// cs 侧块级扩展(cont 回溯 + merges&Allow 空行吸收 +
/// 末行 cont 前移)。ce 侧由 end-patch 补尾，解析止于 rle。
pub(crate) fn expand_zone_cs_blocks_fl(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    n: usize,
    cs: usize,
) -> usize {
    let mut cs = cs.min(n);
    if cs == 0 || cs >= n {
        return cs;
    }
    let depth_at = |line: usize| -> usize {
        depths.get(line).copied().unwrap_or(0) as usize
    };
    let d = depth_at(cs);
    let mut s = cs;
    loop {
        if s == 0 {
            break;
        }
        let prev = &lines[s - 1];
        if prev.cont() && depth_at(s - 1) == d {
            s -= 1;
            continue;
        }
        if prev.node_type() == BlockType::Blank {
            let mut p = s - 1;
            while p > 0 && lines[p - 1].node_type() == BlockType::Blank {
                p -= 1;
            }
            if p > 0 {
                let pt = lines[p - 1].node_type();
                let decl = &crate::effect::BLOCK_TYPE_DECLS[pt as usize];
                if decl.merges
                    && decl.blank_policy == crate::effect::BlankPolicy::Allow
                    && depth_at(p - 1) == d
                {
                    s -= 1;
                    continue;
                }
            }
        }
        break;
    }
    if s < n && lines[s].cont() && s > 0 {
        s -= 1;
    }
    s
}

/// cs 侧容器闭包(depth0 锚 + min_push 前移——RecordFold
/// 前置与 diff 根对齐)。ce 侧由 end-patch 补尾。
pub(crate) fn expand_zone_cs_containers_fl(
    entries: &[u64],
    depths: &[u8],
    n: usize,
    cs: usize,
    ce: usize,
) -> usize {
    let mut cs = cs.min(n);
    let ce = ce.min(n);
    while cs > 0 && depths.get(cs).copied().unwrap_or(0) > 0 {
        cs -= 1;
    }
    let ei_start = partition(entries, cs as u32);
    let ei_end = partition(entries, ce as u32);
    let mut stack: Vec<(FrameKind, usize)> = Vec::new();
    let mut min_push = usize::MAX;
    let mut i = ei_start;
    while i < ei_end.min(entries.len()) {
        let e = entries[i];
        if entry_is_body_mut(e) {
            i += 1;
            continue;
        }
        let li = entry_line(e) as usize;
        let kind = entry_frame(e).kind();
        if entry_is_pop(e) {
            let is_poppush = crate::effect::is_poppush_kindonly(e, entries.get(i + 1).copied());
            if is_poppush {
                if let Some((pk, pl)) = stack.pop() {
                    if pk == kind {
                        min_push = min_push.min(pl);
                        stack.push((pk, pl));
                    }
                }
                i += 2;
                continue;
            }
            stack.pop();
            i += 1;
        } else {
            stack.push((kind, li));
            i += 1;
        }
    }
    if min_push != usize::MAX && min_push < cs {
        cs = min_push;
    }
    while cs > 0 && depths.get(cs).copied().unwrap_or(0) > 0 {
        cs -= 1;
    }
    cs
}

/// 防御条款(patch gate 3000 case 23):
/// 跨界归因引理不敌 setext 跨界对——`> foo\n> ---` 的行级 PL 双侧相同
/// (conv 成立、容器跨 rle),但块级 setext 解析依赖界外分隔行。回退保留
/// ce 侧 cont/HsSep 前看(成本极小——外扩不解析)。
/// case 111 扩展:setext 段落可多行(下划线行前一行为 cont 行)——HsSep
/// 前看不再要求前一行为非 cont;深度同 d 的 HsSep 一律纳入(纯外扩、
/// 不解析;zone 重建对扩入区段是确定且上下文完备的——安全上近似)。
/// case 64(chain 探针)扩展:切点落在开放围栏/HTML 块内时,其内容与
/// 关闭行是跨界状态依赖(关闭行 pop 的锚点不同则 conv 必断)——纳入
/// CodeContent/CodeFenceClose/HtmlContent 完成块尾(围栏/HTML 原子
/// 容器,深度不随内容变化;止于首个非内容行,界是干净块边界)。
pub(crate) fn expand_zone_ce_blocks_fl(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    n: usize,
    ce: usize,
    anchor: usize,
    old_block_idx: Option<&[(u32, u32)]>,
    ce_o: usize,
) -> usize {
    let mut ce = ce.min(n);
    if ce == 0 || ce >= n {
        return ce;
    }
    // 不可补尾判定——切点未闭合块的开启行在 prefix
    // (opener ≤ anchor——anchor 为旧侧行,行 ≤ anchor 时新旧坐标恒等)且旧侧
    // 同 start 块存在且跨旧切点(end > ce_o)时,恒等配对可补尾——立即停,
    // 零前看(para300k 病根:无条件 cont 前看把 30 万行纯 suffix 拉进 zone)。
    // old_block_idx = None 是冷路径降级(无条件 walk 保留,正确性不变)。
    if let Some(idx) = old_block_idx {
        // opener = 包含 ce-1 的块开启行:沿 cont 链回退(cont 位标续行——
        // 检查当前行,回退到首个非 cont 行即块起点)。
        let mut opener = ce.saturating_sub(1);
        while opener > 0 && lines[opener].cont() {
            opener -= 1;
        }
        if opener <= anchor {
            let pi = idx.partition_point(|&(s, _)| (s as usize) < opener);
            if pi < idx.len() && idx[pi].0 as usize == opener && (idx[pi].1 as usize) > ce_o {
                return ce;
            }
        }
    }
    let is_state_dep = |t: BlockType| {
        matches!(
            t,
            BlockType::CodeContent
                | BlockType::CodeFenceClose
                | BlockType::HtmlContent
        )
    };
    let depth_at = |line: usize| -> usize {
        depths.get(line).copied().unwrap_or(0) as usize
    };
    // d = 切点行(ce)事件前深度 = ce-1 事件后的解析上下文——下划线/续行
    // 所属上下文(case 209:切点前一行是 item 开行 "- foo",下划线在 item
    // 内(d 更深);case 23 "> foo\n> ---" 同构——锚在 ce-1 会漏检)。
    let d = depth_at(ce);
    let mut e2 = ce;
    while e2 < n {
        if lines[e2].cont() && depth_at(e2) == d {
            e2 += 1;
            continue;
        }
        if lines[e2].node_type() == BlockType::HsSep
            && depth_at(e2) == d
        {
            e2 += 1;
            continue;
        }
        if is_state_dep(lines[e2].node_type()) && depth_at(e2) == d {
            e2 += 1;
            continue;
        }
        break;
    }
    e2
}

/// cache_fix_824 §1：ce 侧闭包不动点（共享——compute_zone 与 zone_both_sides
/// 共用，禁止复制）。迭代 ① 容器闭包（FL 前向括号匹配——zone 内开启的容器
/// 必须在 zone 内闭合）→ ② 块闭包（认证/兜底——块尾必须整体入区或零前看
/// 归因旧块）至稳定。
/// - `cs`：已过 `expand_zone_cs_containers_fl` 的入口锚（depth-0）。
/// - `ce`：当前切点（rle+1 类；半开区间尾）；返回闭包后的 ce。
/// - `anchor`/`old_idx`/`ce_o_raw`：② 认证分支参数（块 start 在未变前缀 + 旧侧
///   同 start 块存在且跨旧切点 → 零前看）；`old_idx=None` = 冷路径（无条件行走
///   兜底，正确性不变——成本修复见 §6.3 A2）。
pub(crate) fn zone_ce_closure_fl(
    entries: &[u64],
    lines: &[PackedLineInfo],
    depths: &[u8],
    n: usize,
    cs: usize,
    ce: usize,
    anchor: usize,
    old_idx: Option<&[(u32, u32)]>,
    ce_o_raw: usize,
) -> usize {
    let mut ce = ce.min(n);
    for _ in 0..8 {
        // ① 容器闭包:扩展目标 = 记录端 + 1——含"恰在边界闭合"的 NoClose 记录
        // (端 == pop 行 == ce;fold [zs, ce) 必须含 pop 行——否则记录保持开放)。
        let (_, ce1_raw) = expand_zone_to_containers_fl(entries, lines, depths, n, cs, ce);
        let boundary_pop = ce1_raw == ce
            && ce < n
            && (|| {
                let ei = entries.partition_point(|e| crate::block::entry_line(*e) < ce as u32);
                entries
                    .get(ei)
                    .map_or(false, |e| crate::block::entry_line(*e) as usize == ce && crate::block::entry_is_pop(*e))
            })();
        let ce1 = if ce1_raw > ce || boundary_pop {
            (ce1_raw + 1).min(n)
        } else {
            ce1_raw
        };
        // ② 块闭包: 认证(零前看)或深度守卫行走
        let ce2 = expand_zone_ce_blocks_fl(entries, lines, depths, n, ce1, anchor, old_idx, ce_o_raw);
        let next = ce2.max(ce1).min(n);
        if next == ce {
            return next;
        }
        ce = next;
    }
    ce
}

/// 结构未变 ⟺ ripple actions 与旧 FL 对应段逐条相同。
/// ripple_lines 对扫描动作无条件记录（projection.ripple_lines）——内容编辑
/// （打字）会记录与旧 FL 完全相同的 PopPush——"ripple 空"不是结构未变的
/// 充分条件，逐条比较才是。
pub fn ripple_unchanged(
    old_entries: &[u64],
    ripple_actions: &[u64],
    rls: usize,
    rle: usize,
) -> bool {
    let start = old_entries.partition_point(|e| entry_line(*e) < rls as u32);
    let end = old_entries.partition_point(|e| entry_line(*e) < rle as u32 + 1);
    let old_slice = &old_entries[start..end];
    ripple_actions.len() == old_slice.len()
        && ripple_actions.iter().zip(old_slice).all(|(&a, &oe)| {
            entry_is_body_mut(a) == entry_is_body_mut(oe)
                && entry_line(a) == entry_line(oe)
                && entry_is_pop(a) == entry_is_pop(oe)
                && entry_frame(a) == entry_frame(oe)
        })
}

/// 升序标题列表上的最近前驱（等价旧 prev_heading 的逐行语义）。
pub(crate) fn prev_heading_in(heads: &[(usize, u8)], before: usize) -> Option<(usize, u8)> {
    let idx = heads.partition_point(|&(p, _)| p < before);
    idx.checked_sub(1).map(|i| heads[i])
}

/// 升序标题列表上的最近后继且 level <= lv；无则 n。
pub(crate) fn next_heading_le_in(heads: &[(usize, u8)], after: usize, lv: u8, n: usize) -> usize {
    let idx = heads.partition_point(|&(p, _)| p <= after);
    heads[idx..].iter()
        .find(|&&(_, l)| l <= lv)
        .map(|&(p, _)| p)
        .unwrap_or(n)
}

/// 行级标题列表(行类型 ∈ header——含 HsHead——行 → level)。
/// 修(setext):setext 标题首行(行 i)的 line_type 是 Paragraph (HsHead1/2 是
/// 块级类型——resolve 层)——行级扫描漏 setext 标题——SectionClosure 检测
/// 不到 setext 标题文本变——内容截断——补:下一行是 HsSep 且本行是段落 →
/// setext 标题(读 HsSep 行文本判断 level:===→1/---→2)。
/// 放宽(zone 效率优化):BQ 开行(Blockquote——`> foo\n> ===` 的 BQ 内 setext——
/// 行级无法精确判定 BQ 开行内容是否段落——列表项(`> - a\n> ===`)会假阳性——
/// **假阳性安全**(多报标题 = 区间多 = affected 多 = zone 大——
/// over-approximation——正确性不损)——用于 section_closure_zone(性能:行级
/// 扫描 vs 块级 blocks 构建——轻一个数量级)。
/// 局限:BQ 内 setext 首行行类型是 Blockquote 非 Paragraph——行级无法精确
/// 判定 BQ 开行内容是否段落(列表项不 setext——需块层)。精确对照由
/// block_header_lines 承担(SDL/树级)。
pub(crate) fn header_lines(lines: &[PackedLineInfo], rope: &crop::Rope, from: usize, to: usize) -> Vec<(usize, u8)> {
    let n = lines.len();
    (from..to.min(n)).filter_map(|li| {
        if let Some(lv) = crate::block::header_level(lines[li].node_type()) {
            return Some((li, lv));
        }
        if li + 1 < n
            && lines[li + 1].node_type() == BlockType::HsSep
            && matches!(lines[li].node_type(),
                BlockType::Paragraph | BlockType::Blockquote)
        {
            let lv = with_line_str(rope.line(li + 1), |sep| {
                if sep.trim_start().starts_with('=') { 1 } else { 2 }
            });
            return Some((li, lv));
        }
        None
    }).collect()
}

pub fn get_node_end_from_lines(lines: &[crate::block::PackedLineInfo], start_line: usize, t: crate::block::BlockType, log: &crate::block::FrameLog) -> usize {
    if let Some(kind) = crate::block::frame_kind(t) {
        if kind == crate::block::FrameKind::HtmlElement || kind == crate::block::FrameKind::TableRow {
            // Distinguish leaf (no Push for this FrameKind) from frame (Push exists)
            let prefix = log.prefix_len(start_line as u32);
            let has_push = prefix < log.entries.len()
                && crate::block::entry_line(log.entries[prefix]) == start_line as u32
                && !crate::block::entry_is_pop(log.entries[prefix])
                && crate::block::entry_frame(log.entries[prefix]).kind() == kind;
            if has_push {
                return log.item_tail_boundary(start_line as u32, kind, lines)
                    .map(|l| l as usize).unwrap_or(lines.len());
            }
            return start_line + 1; // leaf
        }
        if let Some(end_line) = log.item_tail_boundary(start_line as u32, kind, lines) {
            return end_line as usize;
        }
        if kind == crate::block::FrameKind::Comment {
            return start_line + 1;
        }
        return lines.len();
    }

    if matches!(t, crate::block::BlockType::HsHead1 | crate::block::BlockType::HsHead2) {
        let mut end = start_line + 1;
        while end < lines.len()
            && matches!(lines[end].node_type(), crate::block::BlockType::HsHead1 | crate::block::BlockType::HsHead2)
        {
            end += 1;
        }
        return end;
    }
    if let Some(level) = crate::block::header_level(t) {
        for (offset, info) in lines[start_line + 1..].iter().enumerate() {
            if let Some(next_level) = crate::block::header_level(info.node_type()) {
                if next_level <= level { return start_line + 1 + offset; }
            }
        }
        lines.len()
    } else if t == crate::block::BlockType::Paragraph {
        // HsSep is naturally excluded — the run stops at the first
        // non-Paragraph line, which becomes the exclusive boundary.
        // Walk-level forward-look handles the nt→HsHead override.
        crate::infer::paragraph_run_end(lines, start_line + 1, lines.len())
    } else if t == crate::block::BlockType::IndentedCode {
        let mut end = start_line + 1;
        while end < lines.len() {
            let nt = lines[end].node_type();
            if nt == crate::block::BlockType::IndentedCode || nt == crate::block::BlockType::Blank {
                end += 1;
            } else {
                break;
            }
        }
        // trailing blank backtrack: CommonMark strips trailing blank lines
        while end > start_line + 1 && lines[end - 1].node_type() == crate::block::BlockType::Blank {
            end -= 1;
        }
        end
    } else {
        start_line + 1
    }
}


#[cfg(test)]
mod tests {
    include!("../tests/unit/infer.rs");
}
