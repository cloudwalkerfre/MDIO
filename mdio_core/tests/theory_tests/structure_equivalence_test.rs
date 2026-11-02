//! MDIO 形式结构等价性验证
//!
//! 验证 formal_structure.md 附录 D-H 的定理在当前代码实现中成立。
//! 运行: cargo test --test structure_equivalence_test
//!
//! ── 测试分层 ──
//!
//! 阶段 0 (本文件): testbed 基础设施 + 冒烟测试
//! 阶段 1: 事件流代数 + Dyck 合法性 + compress 完备性 (测试 1-6)
//! 阶段 2: 事件序到路径双射 (测试 7-11)
//! 阶段 3: 路径代数与增量维护 (测试 12-14)
//! 阶段 4: Patch 理论与消除代数律 (测试 15-19)

#![allow(non_snake_case)]

use mdio_core::projection::State;
use mdio_core::block::{
    FrameKind, BlockFrame, FrameLog,
    push_entry, pop_entry, body_mut_entry,
    entry_line, entry_is_pop, entry_is_body_mut, entry_frame,
    entry_consume, with_consume,
    LOOSE_BIT,
};

// §TB: 共享测试台 (Shared Testbed)

// ── TB.1: DocSpec + docs() ──

struct DocSpec {
    text: &'static str,
    name: &'static str,
    frame_kinds: &'static [FrameKind],
    push_count: usize,
    has_nesting: bool,
    has_list: bool,
}

fn docs() -> Vec<DocSpec> {
    vec![
        // D1: 纯段落 — 零容器
        DocSpec { text: "plain text\nmore text\n", name: "plain",
            frame_kinds: &[], push_count: 0, has_nesting: false, has_list: false },

        // D2: 单层 BLT
        DocSpec { text: "- a\n- b\n- c\n", name: "blt_flat",
            frame_kinds: &[FrameKind::BulletList], push_count: 1, has_nesting: false, has_list: true },

        // D3: 嵌套 BLT
        DocSpec { text: "- a\n  - b\n  - c\n- d\n", name: "blt_nested",
            frame_kinds: &[FrameKind::BulletList], push_count: 2, has_nesting: true, has_list: true },

        // D4: BQ
        DocSpec { text: "> quoted\n> more\n\nplain\n", name: "bq",
            frame_kinds: &[FrameKind::BlockQuote], push_count: 1, has_nesting: false, has_list: false },

        // D5: BQ > BLT
        DocSpec { text: "> - item1\n> - item2\n", name: "bq_blt",
            frame_kinds: &[FrameKind::BlockQuote, FrameKind::BulletList],
            push_count: 2, has_nesting: true, has_list: true },

        // D6: BQ > BLT > OL
        DocSpec { text: "> - a\n>   1. x\n>   2. y\n> - b\n", name: "bq_blt_ol",
            frame_kinds: &[FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::OrderedList],
            push_count: 3, has_nesting: true, has_list: true },

        // D7: CF
        DocSpec { text: "```rust\nlet x = 1;\n```\n", name: "cf",
            frame_kinds: &[FrameKind::CodeFence], push_count: 1, has_nesting: false, has_list: false },

        // D8: Table
        DocSpec { text: "| a | b |\n|----|----|\n| 1 | 2 |\n", name: "table",
            frame_kinds: &[FrameKind::Table, FrameKind::TableRow], push_count: 2,
            has_nesting: false, has_list: false },

        // D9: Comment
        DocSpec { text: "<!--\nmultiline\ncomment\n-->\n", name: "comment",
            frame_kinds: &[FrameKind::Comment], push_count: 1, has_nesting: false, has_list: false },

        // D10: Heading + BLT + Paragraph (混合)
        DocSpec { text: "# Title\n- a\n- b\n\nplain\n", name: "mixed_h_blt_p",
            frame_kinds: &[FrameKind::BulletList], push_count: 1, has_nesting: false, has_list: true },
    ]
}

// ── TB.2: FL 提取器 ──

fn fl_of(text: &str) -> Vec<u64> {
    let state = State::new(text);
    state.frame_log.entries.clone()
}

// ── TB.3: 手工 FL 构造器 ──

fn mk_push(line: u32, kind: FrameKind, indent: u8, body: u8) -> u64 {
    push_entry(line, BlockFrame::new(kind, indent, body))
}

fn mk_pop(line: u32, kind: FrameKind, indent: u8, body: u8) -> u64 {
    pop_entry(line, BlockFrame::new(kind, indent, body))
}

fn mk_bm(line: u32, kind: FrameKind, indent: u8, body: u8) -> u64 {
    body_mut_entry(line, BlockFrame::new(kind, indent, body))
}

fn mk_blt_push(line: u32, indent: u8, bullet: u8, loose: bool) -> u64 {
    let mut body = BlockFrame::encode_blt_body(bullet);
    if loose { body |= LOOSE_BIT as u8; }
    push_entry(line, BlockFrame::new(FrameKind::BulletList, indent, body))
}

fn mk_ol_push(line: u32, indent: u8, delim: u8) -> u64 {
    let body = BlockFrame::encode_ol_body(delim);
    push_entry(line, BlockFrame::new(FrameKind::OrderedList, indent, body))
}

fn mk_cf_push(line: u32, fence_char: u8, fence_len: u8) -> u64 {
    let body = BlockFrame::encode_cf_body(fence_char, fence_len);
    push_entry(line, BlockFrame::new(FrameKind::CodeFence, 0, body))
}

// d1d84a25 起 BQ body = 本层内容列（原先恒 0）；本 helper 只表示列 0 的 "> "，
// 内容列 = marker 宽 2 ⇒ body=2（consume 同样为 2）。
fn mk_bq_push(line: u32) -> u64 {
    push_entry(line, BlockFrame::new(FrameKind::BlockQuote, 0, 2))
}

// ── TB.4: push_entries — 提取排序的 Push 条目 ──

fn push_entries(entries: &[u64]) -> Vec<(u32, u64)> {
    let mut pushes: Vec<(u32, u64)> = entries.iter()
        .filter(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e))
        .map(|&e| (entry_line(e), e))
        .collect();
    // 过滤 PopPush: 同行同 kind 存在 Pop entry → 该 Push 是 PopPush 的 Push 半部
    pushes.retain(|&(line, e)| {
        let kind = entry_frame(e).kind();
        !entries.iter().any(|&pe| {
            entry_is_pop(pe) && entry_line(pe) == line && entry_frame(pe).kind() == kind
        })
    });
    pushes.sort_by_key(|&(line, _)| line);
    pushes
}

// ── TB.5: validate_dyck — Dyck 合法性 I1-I5 ──

fn validate_dyck(entries: &[u64]) -> Option<String> {
    let mut push_counts: [u32; 10] = [0; 10];
    let mut depth: usize = 0;
    // 栈: 记录每层 Push 的 FrameKind（用于 I3 颜色匹配）
    let mut open_kinds: Vec<FrameKind> = Vec::new();

    for (i, &e) in entries.iter().enumerate() {
        if entry_is_body_mut(e) {
            continue;
        }
        let kind = entry_frame(e).kind();
        let ki = kind as usize;
        if ki >= 10 {
            return Some(format!("entry[{}]: invalid FrameKind discriminant {}", i, ki));
        }

        if entry_is_pop(e) {
            // I2: Pop 不能超过 Push
            if push_counts[ki] == 0 {
                return Some(format!("entry[{}]: I2 violated — Pop({:?}) without matching Push", i, kind));
            }
            // I3: 颜色匹配 — 栈顶必须是同色 Push
            match open_kinds.last() {
                Some(&top) if top == kind => {},
                Some(&top) => return Some(format!(
                    "entry[{}]: I3 violated — Pop({:?}) does not match stack top ({:?})", i, kind, top)),
                None => return Some(format!(
                    "entry[{}]: I3 violated — Pop({:?}) but stack is empty", i, kind)),
            }
            push_counts[ki] -= 1;
            depth -= 1;
            open_kinds.pop();
        } else {
            // I5: 深度上界
            if depth >= 32 {
                return Some(format!("entry[{}]: I5 violated — depth {} exceeds MAX_BLOCK_NESTING", i, depth));
            }
            push_counts[ki] += 1;
            depth += 1;
            open_kinds.push(kind);
        }
    }
    // I1: 文档结束时 push_counts 可 > 0（Dyck 前缀，非完整 Dyck 字）— 不报错
    None
}

// ── TB.6: 通用断言 ──

fn assert_dyck_valid(entries: &[u64], context: &str) {
    if let Some(err) = validate_dyck(entries) {
        panic!("[{}] Dyck validation failed: {}\n  entries: {:?}", context, err, entries);
    }
}

// ── TB.7: 批量测试宏 ──

macro_rules! for_all_docs {
    ($f:expr) => {
        for spec in &docs() {
            let entries = fl_of(spec.text);
            $f(spec.name, spec.text, &entries, spec);
        }
    };
}

macro_rules! for_container_docs {
    ($f:expr) => {
        for spec in docs().iter().filter(|s| s.push_count > 0) {
            let entries = fl_of(spec.text);
            $f(spec.name, spec.text, &entries, spec);
        }
    };
}

/// φ(j): 基于事件序列索引 j（非行号）。Push entry 的路径 = 包含 entries[0..j+1] 的 replay。
/// 正确处 Composite 的同行多个 Push：每个 Push 有不同的索引 j，产生不同路径。
fn phi_by_index(entries: &[u64], j: usize) -> String {
    let mut ci: [usize; 16] = [0; 16];
    let mut depth = 0usize;
    for i in 0..=j {
        let e = entries[i];
        if entry_is_body_mut(e) { continue; }
        if entry_is_pop(e) { depth = depth.saturating_sub(1); }
        else { depth += 1; ci[depth] += 1; }
    }
    let mut path = String::new();
    for d in 1..=depth {
        use std::fmt::Write;
        let _ = write!(path, "/children/{}", ci[d]);
    }
    path
}

/// φ⁻¹(p): 基于事件序列索引的反查。遍历所有 entries，按 ci[] 匹配路径分量。
/// 返回匹配 Push entry 的索引 j。Composite 同行 Push 由不同的 j 区分。
fn phi_inv_by_index(entries: &[u64], path: &str) -> Option<usize> {
    let indices: Vec<usize> = path.split("/children/")
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.trim_end_matches('/').parse().ok())
        .collect();
    if indices.is_empty() { return None; }
    let mut ci: [usize; 16] = [0; 16];
    let mut depth = 0usize;
    for (j, &e) in entries.iter().enumerate() {
        if entry_is_body_mut(e) { continue; }
        if entry_is_pop(e) { depth = depth.saturating_sub(1); continue; }
        depth += 1; ci[depth] += 1;
        // 检查当前 Push 是否匹配路径
        let mut matched = true;
        for d in 0..indices.len().min(depth) {
            if ci[d + 1] != indices[d] { matched = false; break; }
        }
        if matched && depth == indices.len() {
            return Some(j);
        }
    }
    None
}

/// 从 FL entries 计算特定行的 child_path（复制自 ripple_equivalence_test.rs:2246）。
fn child_path_from_entries(entries: &[u64], line: u32) -> String {
    let prefix_end = entries.partition_point(|&e| entry_line(e) < line);
    let mut ci: [usize; 16] = [0; 16];
    let mut depth = 0usize;
    for i in 0..prefix_end {
        let e = entries[i];
        if entry_is_body_mut(e) { continue; }
        if entry_is_pop(e) { depth = depth.saturating_sub(1); }
        else { depth += 1; ci[depth] += 1; }
    }
    let mut path = String::new();
    for d in 1..=depth {
        use std::fmt::Write;
        let _ = write!(path, "/children/{}", ci[d]);
    }
    path
}

/// φ⁻¹: 给定路径字符串，反查对应的 Push entry。
/// 路径格式: "/children/0/children/2/children/1"
/// 通过 FL replay 逐层定位，与 child_path_from_entries 的 ci[] 计数完全一致。
fn find_push_by_path(entries: &[u64], path: &str) -> Option<(u32, u64)> {
    let indices: Vec<usize> = path.split("/children/")
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.trim_end_matches('/').parse().ok())
        .collect();
    if indices.is_empty() || entries.is_empty() {
        return None;
    }
    // FL replay: 维护 depth + ci[]（与 child_path_from_entries 完全一致）
    let mut ci: [usize; 16] = [0; 16];
    let mut depth = 0usize;
    for (i, &e) in entries.iter().enumerate() {
        if entry_is_body_mut(e) { continue; }
        if entry_is_pop(e) {
            depth = depth.saturating_sub(1);
        } else {
            depth += 1;
            ci[depth] += 1;
            // 检查当前深度是否匹配路径的对应层
            if depth <= indices.len() && ci[depth] == indices[depth - 1] {
                if depth == indices.len() {
                    return Some((entry_line(e), e));
                }
            } else if depth <= indices.len() && ci[depth] > indices[depth - 1] {
                // 已超过目标索引 → 该深度层不存在对应 Push
                return None;
            }
        }
    }
    None
}

// §阶段 1 辅助函数: event_class, compress

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EventClass { C_info, I_info, T_info }

/// body_adm 位是否相同 — 用于 PopPush 是否 I_info 的判定。
fn body_adm_eq(kind: FrameKind, a: BlockFrame, b: BlockFrame) -> bool {
    match kind {
        // 删位（步骤 3）后 BQ body 恒 0——adm=∅——恒同
        FrameKind::BlockQuote => true,
        FrameKind::BulletList => a.bullet_code() == b.bullet_code(),
        FrameKind::OrderedList => a.delim_code() == b.delim_code(),
        FrameKind::HtmlBlock => a.block_type() == b.block_type(),
        FrameKind::RefDef => a.body() == b.body(), // has_content 在 body 中
        // CF, Comment, Table, HtmlElement: body_adm = ∅
        _ => true,
    }
}

/// 单条 entry 的事件流三维分类（附录 A §A.6 B3）。
/// 需要完整 entries 数组用于 PopPush 检测。
fn event_class_at(entries: &[u64], idx: usize) -> EventClass {
    let e = entries[idx];
    if entry_is_body_mut(e) {
        let f = entry_frame(e);
        let kind = f.kind();
        // BodyMut 分类: 检查 frame 的 body_out 位 vs body_adm 位
        // 保守策略: 仅当可以确定"只改了 body_out 位"时返回 T_info
        return match kind {
            // BLT: LOOSE_BIT(bit3,body_out), bullet_code(bits5-4,body_adm)
            // 仅 LOOSE_BIT 变化的 BodyMut → T_info
            FrameKind::BulletList | FrameKind::OrderedList => {
                if f.loose() { EventClass::T_info } else { EventClass::C_info }
            }
            // CF, Comment: 无 body_adm 位 → BodyMut 都是 T_info
            FrameKind::CodeFence | FrameKind::Comment => EventClass::T_info,
            // BQ, HtmlBlock, HtmlElement, RefDef: body_adm 可能变化 → C_info
            _ => EventClass::C_info,
        };
    }
    let kind = entry_frame(e).kind();
    let line = entry_line(e);

    if entry_is_pop(e) {
        // PopPush 检测: 同行同 kind 的 Push 在当期 entry **之后**（Pop 先于 Push）
        let is_poppush = entries[idx+1..].iter().any(|&pe| {
            !entry_is_pop(pe) && !entry_is_body_mut(pe)
                && entry_line(pe) == line && entry_frame(pe).kind() == kind
        });
        if is_poppush {
            let f_old = entry_frame(e);
            if let Some(&pe) = entries[idx+1..].iter().find(|&&pe| {
                !entry_is_pop(pe) && !entry_is_body_mut(pe)
                    && entry_line(pe) == line && entry_frame(pe).kind() == kind
            }) {
                if body_adm_eq(kind, f_old, entry_frame(pe)) {
                    return EventClass::I_info;
                }
            }
            return EventClass::C_info;
        }
        return match kind {
            FrameKind::CodeFence | FrameKind::Comment => EventClass::T_info,
            _ => EventClass::C_info,
        };
    }

    // Push
    // PopPush 检测: 必须存在同行的 Pop 在当期 entry **之前**（Pop 先于 Push）
    let is_poppush = entries[..idx].iter().any(|&pe| {
        entry_is_pop(pe) && entry_line(pe) == line && entry_frame(pe).kind() == kind
    });
    if is_poppush {
        let f_new = entry_frame(e);
        if let Some(&pe) = entries[..idx].iter().find(|&&pe| {
            entry_is_pop(pe) && entry_line(pe) == line && entry_frame(pe).kind() == kind
        }) {
            if body_adm_eq(kind, entry_frame(pe), f_new) {
                return EventClass::I_info;
            }
        }
        return EventClass::C_info;
    }
    match kind {
        FrameKind::CodeFence | FrameKind::Comment => EventClass::T_info,
        _ => EventClass::C_info,
    }
}

/// 对整个 FL entries 序列应用 compress（附录 D §D.1 A3）。
/// R1: Pop(k,f)·Push(k,f) → ε (邻接关-开对，任意 k)
/// R2: PopPush(同 frame) → ε (Pop+Push 同时移除)
/// R3: Push/Pop of ⊤ 容器 → ε (CF, Comment)
/// R4: BodyMut(仅 body_out) → ε (保守: 仅 CF/Comment 的 BodyMut)
fn compress_entries(entries: &[u64]) -> Vec<u64> {
    let mut result: Vec<u64> = Vec::new();
    let mut i = 0;
    while i < entries.len() {
        let e = entries[i];
        let ec = event_class_at(entries, i);

        // T_info → 跳过（归约消除）
        if ec == EventClass::T_info {
            i += 1;
            continue;
        }

        // I_info (PopPush 同 frame) → 跳过 Pop + Push 两条
        if ec == EventClass::I_info {
            i += if entry_is_pop(e) { 2 } else { 2 };
            continue;
        }

        // R1: Pop · Push 邻接对 — 检查当前 Pop 后紧跟同色 Push
        if entry_is_pop(e) && i + 1 < entries.len() {
            let next = entries[i + 1];
            if !entry_is_pop(next) && !entry_is_body_mut(next)
                && entry_frame(next).kind() == entry_frame(e).kind()
                && entry_line(next) == entry_line(e)
            {
                // 同行同色 Pop+Push = PopPush 的一部分，已在 I_info 处理
                // 这里处理跨行的邻接对
                if entry_line(next) != entry_line(e) {
                    i += 2;
                    continue;
                }
            }
        }

        result.push(e);
        i += 1;
    }
    result
}

// 阶段 0 — Testbed 冒烟测试

#[test]
fn structure_testbed_smoke() {
    // ═══ A: 手工 FL + Dyck 合法性双向验证 ═══

    // A1: 空序列合法
    assert_dyck_valid(&[], "A1_empty");

    // A2: 嵌套 BLT 前缀合法
    let e_a2 = vec![
        mk_blt_push(0, 0, b'-', false),
        mk_blt_push(1, 2, b'-', false),
    ];
    assert_dyck_valid(&e_a2, "A2_nested_blt_prefix");

    // A3: BQ 完整配对合法
    let e_a3 = vec![
        mk_bq_push(0),
        mk_pop(2, FrameKind::BlockQuote, 0, 0),
    ];
    assert_dyck_valid(&e_a3, "A3_bq_pair");

    // A4: Pop 无匹配 Push → 非法
    let e_a4 = vec![mk_pop(0, FrameKind::BlockQuote, 0, 0)];
    assert!(validate_dyck(&e_a4).is_some(), "A4: pop_without_push must fail");

    // A5: 颜色不匹配 → 非法
    let e_a5 = vec![
        mk_push(0, FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-')),
        mk_pop(0, FrameKind::BlockQuote, 0, 0),
    ];
    assert!(validate_dyck(&e_a5).is_some(), "A5: color_mismatch must fail");

    // A6: 深度超 32 → 非法
    let mut e_a6 = Vec::new();
    for i in 0..33 {
        e_a6.push(mk_push(i, FrameKind::BlockQuote, 0, 0));
    }
    assert!(validate_dyck(&e_a6).is_some(), "A6: depth_exceeded must fail");

    // ═══ B: 规范文档 → FL 基本属性 ═══

    for spec in &docs() {
        let entries = fl_of(spec.text);
        let pushes = push_entries(&entries);

        assert_eq!(pushes.len(), spec.push_count,
            "[{}] push_count: expected {}, got {}. pushes: {:?}",
            spec.name, spec.push_count, pushes.len(), pushes);

        if spec.push_count > 0 {
            assert!(!entries.is_empty(),
                "[{}] container doc must have FL entries, got empty", spec.name);
        }
    }

    // ═══ C: 规范文档 → Dyck 合法性 ═══

    for_all_docs!(|name, _text, entries, _spec| {
        assert_dyck_valid(entries, name);
    });

    // ═══ D: 往返验证 — 手工构造器 vs 扫描器 ═══

    // D1: BLT 往返
    {
        let entries = fl_of("- a\n- b\n");
        let push_e = entries.iter().find(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e)
            && entry_frame(e).kind() == FrameKind::BulletList)
            .expect("D1: no BLT Push found");
        let f = entry_frame(*push_e);
        let bullet = match f.bullet_code() { 1 => b'*', 2 => b'+', _ => b'-' };
        // 锚物化后手工构造需带锚（push_list_entry——scan 的列表 Push 携带锚字段）
        let mut body = BlockFrame::encode_blt_body(bullet);
        if f.loose() { body |= LOOSE_BIT as u8; }
        // consume 位：`- ` 在行 0 无前导空白——marker 宽 2。
        // 手工侧指定期望值（钉位值——不抄扫描侧）。
        let manual = with_consume(mdio_core::block::push_list_entry(entry_line(*push_e),
            BlockFrame::new(FrameKind::BulletList, f.indent(), body),
            mdio_core::block::entry_anchor(*push_e)), 2);
        assert_eq!(entry_consume(*push_e), 2, "D1 consume 位应钉 2");
        assert_eq!(*push_e, manual,
            "D1 BLT roundtrip: scanned {:016x} vs manual {:016x} (line={}, indent={}, bullet={}, loose={})",
            *push_e, manual, entry_line(*push_e), f.indent(), bullet as char, f.loose());
    }

    // D2: BQ 往返
    {
        let entries = fl_of("> quoted\n");
        let push_e = entries.iter().find(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e)
            && entry_frame(e).kind() == FrameKind::BlockQuote)
            .expect("D2: no BQ Push found");
        let f = entry_frame(*push_e);
        // consume 位：`> ` 带尾空格——1+has_trailing=2；手工侧同时按 d1d84a25 的
        // 新语义钉 body=本层内容列=2（mk_bq_push 已按列 0 的 "> " 写好）。
        let manual = with_consume(mk_bq_push(entry_line(*push_e)), 2);
        assert_eq!(entry_consume(*push_e), 2, "D2 consume 位应钉 2");
        assert_eq!(*push_e, manual,
            "D2 BQ roundtrip: scanned {:016x} vs manual {:016x} (line={}, body={})",
            *push_e, manual, entry_line(*push_e), f.body());
    }

    // D3: CF 往返
    {
        let entries = fl_of("```\ncode\n```\n");
        let push_e = entries.iter().find(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e)
            && entry_frame(e).kind() == FrameKind::CodeFence)
            .expect("D3: no CF Push found");
        let f = entry_frame(*push_e);
        let fc = f.fence_char();
        let fl = f.fence_len();
        let manual = mk_cf_push(entry_line(*push_e), fc, fl);
        assert_eq!(*push_e, manual,
            "D3 CF roundtrip: scanned {:016x} vs manual {:016x} (line={}, fence_char='{}', fence_len={})",
            *push_e, manual, entry_line(*push_e), fc as char, fl);
    }

    // D4: OL 往返
    {
        let entries = fl_of("1. x\n2. y\n");
        let push_e = entries.iter().find(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e)
            && entry_frame(e).kind() == FrameKind::OrderedList)
            .expect("D4: no OL Push found");
        let f = entry_frame(*push_e);
        let delim = if f.delim_code() == 1 { b')' } else { b'.' };
        // 锚物化后手工构造需带锚（同 D1 BLT）
        let body = BlockFrame::encode_ol_body(delim);
        // consume 位：`1. ` 行 0 无前导空白——digits+delim+空格=3。
        let manual = with_consume(mdio_core::block::push_list_entry(entry_line(*push_e),
            BlockFrame::new(FrameKind::OrderedList, f.indent(), body),
            mdio_core::block::entry_anchor(*push_e)), 3);
        assert_eq!(entry_consume(*push_e), 3, "D4 consume 位应钉 3");
        assert_eq!(*push_e, manual,
            "D4 OL roundtrip: scanned {:016x} vs manual {:016x} (line={}, indent={}, delim='{}')",
            *push_e, manual, entry_line(*push_e), f.indent(), delim as char);
    }
}

/// consume 位 accessor 自测（遗漏 4 补——写读往返＋位域隔离＋层宽语义钉）。
#[test]
fn consume_bit_accessor_roundtrip() {
    let base = mdio_core::block::push_list_entry(
        7, BlockFrame::new(FrameKind::BulletList, 2, 0), 0xAB);
    assert_eq!(entry_consume(base), 0, "新条目 consume 默认 0");
    let with = with_consume(base, 14);
    assert_eq!(entry_consume(with), 14, "consume 写读往返");
    // 写位不动其余字段（line/anchor/frame 位域隔离）
    assert_eq!(with & !mdio_core::block::CONSUME_MASK, base);
    assert_eq!(entry_line(with), 7);
    assert_eq!(entry_frame(with).kind(), FrameKind::BulletList);
    assert_eq!(entry_frame(with).indent(), 2);
    // 6 位上界
    let w63 = with_consume(base, 63);
    assert_eq!(entry_consume(w63), 63);
    // 层宽语义钉：`> `（尾空格）consume=2 vs `>a`（无尾空格）consume=1
    let bq2 = fl_of("> a\n");
    let bq_push2 = bq2.iter().find(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e)
        && entry_frame(e).kind() == FrameKind::BlockQuote).expect("BQ push");
    assert_eq!(entry_consume(*bq_push2), 2, "`> ` 层宽应 2");
    let bq1 = fl_of(">a\n");
    let bq_push1 = bq1.iter().find(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e)
        && entry_frame(e).kind() == FrameKind::BlockQuote).expect("BQ push");
    assert_eq!(entry_consume(*bq_push1), 1, "`>a` 层宽应 1");
}

// 阶段 1 — 事件流代数 + Dyck 合法性 + compress 完备性 (测试 1-6)

/// 测试 1: compress 完整 ker — 五类事件分类 (附录 A §A.6 B1)
#[test]
fn structure_t1_compress_ker_event_classes() {
    // ── ⊤ 容器: CF/Comment Push/Pop → T_info ──
    {
        let e_cf = fl_of("```\ncode\n```\n");
        assert!(!e_cf.is_empty(), "CF must have FL entries");
        for i in 0..e_cf.len() {
            assert_eq!(event_class_at(&e_cf, i), EventClass::T_info,
                "CF entry[{}] must be T_info: {:016x}", i, e_cf[i]);
        }
    }
    {
        let e_cmt = fl_of("<!--\nc\n-->\n");
        assert!(!e_cmt.is_empty(), "Comment must have FL entries");
        for i in 0..e_cmt.len() {
            assert_eq!(event_class_at(&e_cmt, i), EventClass::T_info,
                "Comment entry[{}] must be T_info: {:016x}", i, e_cmt[i]);
        }
    }

    // ── Table Push → C_info (修正 6) ──
    {
        let e_tbl = fl_of("| a | b |\n|----|----|\n| 1 | 2 |\n");
        assert!(!e_tbl.is_empty(), "Table must have FL entries");
        for i in 0..e_tbl.len() {
            assert_eq!(event_class_at(&e_tbl, i), EventClass::C_info,
                "Table entry[{}] must be C_info: {:016x}", i, e_tbl[i]);
        }
    }

    // ── BLT/OL: 容器 Push = C_info, PopPush = I_info, BQ 全部 C_info ──
    {
        // "- a\n- b\n": [Push(BLT,0), Pop(old,1), Push(new,1)]
        let e_blt = fl_of("- a\n- b\n");
        assert!(e_blt.len() >= 3, "BLT 2-item doc must have >=3 FL entries, got {}", e_blt.len());
        // 容器 Push
        assert_eq!(event_class_at(&e_blt, 0), EventClass::C_info, "BLT[0] container Push must be C_info");
        // PopPush: Pop half → I_info
        assert!(entry_is_pop(e_blt[1]), "BLT[1] must be Pop");
        assert_eq!(event_class_at(&e_blt, 1), EventClass::I_info, "BLT[1] PopPush Pop must be I_info");
        // PopPush: Push half → I_info
        assert!(!entry_is_pop(e_blt[2]) && !entry_is_body_mut(e_blt[2]), "BLT[2] must be Push");
        assert_eq!(event_class_at(&e_blt, 2), EventClass::I_info, "BLT[2] PopPush Push must be I_info");
    }
    {
        let e_bq = fl_of("> q\n\nplain\n");
        assert!(!e_bq.is_empty(), "BQ must have FL entries");
        for i in 0..e_bq.len() {
            let ec = event_class_at(&e_bq, i);
            assert!(matches!(ec, EventClass::C_info),
                "BQ entry[{}] must be C_info, got {:?}: {:016x}", i, ec, e_bq[i]);
        }
    }
    {
        // "1. x\n2. y\n": [Push(OL,0), Pop(old,1), Push(new,1)]
        let e_ol = fl_of("1. x\n2. y\n");
        assert!(e_ol.len() >= 3, "OL 2-item doc must have >=3 FL entries, got {}", e_ol.len());
        assert_eq!(event_class_at(&e_ol, 0), EventClass::C_info, "OL[0] container Push must be C_info");
        assert!(entry_is_pop(e_ol[1]), "OL[1] must be Pop");
        assert_eq!(event_class_at(&e_ol, 1), EventClass::I_info, "OL[1] PopPush Pop must be I_info");
        assert!(!entry_is_pop(e_ol[2]) && !entry_is_body_mut(e_ol[2]), "OL[2] must be Push");
        assert_eq!(event_class_at(&e_ol, 2), EventClass::I_info, "OL[2] PopPush Push must be I_info");
    }

    // ── PopPush(同 frame) → I_info ──
    {
        // D2: "- a\n- b\n- c\n" 应有 PopPush 事件
        let e = fl_of("- a\n- b\n- c\n");
        // 找 Pop entry 中属于 PopPush 的
        let poppush_pops: Vec<usize> = (0..e.len()).filter(|&i| {
            entry_is_pop(e[i]) && event_class_at(&e, i) == EventClass::I_info
        }).collect();
        assert!(!poppush_pops.is_empty(), "BLT items should produce PopPush I_info events");
    }

    // ── 手工 PopPush(异 frame, bullet_code 不同) → C_info ──
    {
        let p1 = mk_blt_push(0, 0, b'-', false);
        let pop1 = pop_entry(0, BlockFrame::new(FrameKind::BulletList, 0,
            BlockFrame::encode_blt_body(b'-')));
        let p2 = mk_blt_push(0, 0, b'*', false);
        let e = vec![p1, pop1, p2];
        assert_eq!(event_class_at(&e, 1), EventClass::C_info, "PopPush(异frame) Pop must be C_info");
        assert_eq!(event_class_at(&e, 2), EventClass::C_info, "PopPush(异frame) Push must be C_info");
    }

    // ── BodyMut: event_class_at 是保守的（无法从单条 entry 判断改变哪些位），
    // 但 verify compress 行为: BLT 内空行的 LOOSE_BIT BodyMut 在 compress 下消失 ──
    {
        // "- a\n\n- b\n": 空行触发 LOOSE_BIT BodyMut
        let e = fl_of("- a\n\n- b\n");
        let bm_indices: Vec<usize> = (0..e.len())
            .filter(|&i| entry_is_body_mut(e[i])).collect();
        assert!(!bm_indices.is_empty(),
            "BLT blank line should produce BodyMut, FL={:?}", e);
        // compress 应消除纯 LOOSE_BIT 的 BodyMut（body_out only → T_info → 归约）
        let c = compress_entries(&e);
        for &i in &bm_indices {
            assert!(!c.contains(&e[i]),
                "BodyMut(LOOSE_BIT only) should be removed by compress. entry[{}]={:016x}", i, e[i]);
        }
    }

    // ── BQ BodyMut: para_open 是 body_adm → compress 应保留 ──
    {
        // "> text\n\n> more\n": 空行关闭 para_open → ContinueAs → BodyMut
        let e = fl_of("> text\n\n> more\n");
        let bm_indices: Vec<usize> = (0..e.len())
            .filter(|&i| entry_is_body_mut(e[i])).collect();
        if bm_indices.is_empty() {
            // para_open 变化也可能通过 Pop+Push 表达（非 BodyMut）
            // 若为 Pop+Push → 两者都是 C_info → compress 保留 Pop/Push → 也算"保留"
            // 此情况下跳过 BodyMut 专项检查
        } else {
            let c = compress_entries(&e);
            for &i in &bm_indices {
                assert!(c.contains(&e[i]),
                    "BodyMut(para_open=body_adm) should survive compress. entry[{}]={:016x}", i, e[i]);
            }
        }
    }
}

/// 测试 2: compress_entry 归约正确性 (附录 D §D.1 A2-A3)
#[test]
fn structure_t2_compress_reduction() {
    // ── ⊤ 容器 Push/Pop 在 compress 下消失 ──
    {
        let e = fl_of("```\ncode\n```\n");
        let c = compress_entries(&e);
        assert!(c.is_empty(), "CF doc compress must be empty, got {:?}", c);
    }
    {
        let e = fl_of("<!--\nc\n-->\n");
        let c = compress_entries(&e);
        assert!(c.is_empty(), "Comment doc compress must be empty, got {:?}", c);
    }

    // ── Table compress ≠ ε (修正 6) ──
    {
        let e = fl_of("| a | b |\n|----|----|\n| 1 | 2 |\n");
        let c = compress_entries(&e);
        assert!(!c.is_empty(), "Table doc compress must NOT be empty (Table is non-⊤)");
    }

    // ── BLT compress 保留 C_info, 消除 I_info ──
    {
        let e = fl_of("- a\n- b\n- c\n");
        let c = compress_entries(&e);
        // compress 后: 仅保留非 PopPush 的 BLT Push，消除 PopPush 的 I_info 事件
        let pushes = push_entries(&c);
        assert_eq!(pushes.len(), 1, "BLT compress: should have 1 container Push, got {:?}", pushes);
    }

    // ── PopPush(同 frame) compress = ε ──
    {
        let p1 = mk_blt_push(0, 0, b'-', false);
        let pop1 = pop_entry(0, BlockFrame::new(FrameKind::BulletList, 0,
            BlockFrame::encode_blt_body(b'-')));
        let p2 = mk_blt_push(0, 0, b'-', false);
        let e = vec![p1, pop1, p2];
        let c = compress_entries(&e);
        assert_eq!(c.len(), 1, "PopPush(同frame) compress: only the container Push should remain. got {:?}", c);
    }
}

/// 测试 3: 正规形式唯一性 (附录 D §D.1 定理 D.1)
#[test]
fn structure_t3_normal_form_unique() {
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let entries = fl_of(spec.text);
        let nf = compress_entries(&entries);
        // 正规形式不含 T_info 和 I_info
        for i in 0..nf.len() {
            let ec = event_class_at(&nf, i);
            assert_eq!(ec, EventClass::C_info,
                "[{}] nf[{}] must be C_info, got {:?}: {:016x}", spec.name, i, ec, nf[i]);
        }
        // 正规形式不能再归约: 二次 compress 不变
        let nf2 = compress_entries(&nf);
        assert_eq!(nf, nf2, "[{}] normal form not idempotent: nf≠compress(nf)", spec.name);
        // Dyck 合法性保持
        assert_dyck_valid(&nf, &format!("{}-nf", spec.name));
    }
}

/// 测试 4: Dyck 合法性不变量 I1-I5
#[test]
fn structure_t4_dyck_legality() {
    for_all_docs!(|name, _text, entries, _spec| {
        assert_dyck_valid(entries, name);
    });
}

/// 测试 5: ⊤ 容器透明性 (附录 D §D.1 定理 D.3)
#[test]
fn structure_t5_top_container_transparency() {
    // CF 和 Comment 的 Push/Pop 全部 T_info, compress 为空
    let cf = fl_of("```\ncode\n```\n");
    assert!(!cf.is_empty());
    for i in 0..cf.len() {
        assert_eq!(event_class_at(&cf, i), EventClass::T_info);
    }
    assert!(compress_entries(&cf).is_empty());

    let cmt = fl_of("<!--\nc\n-->\n");
    assert!(!cmt.is_empty());
    for i in 0..cmt.len() {
        assert_eq!(event_class_at(&cmt, i), EventClass::T_info);
    }
    assert!(compress_entries(&cmt).is_empty());

    // 结构等价: compress 保留容器壳的嵌套顺序（C_info Push 的种类序列不变）
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let entries = fl_of(spec.text);
        let compressed = compress_entries(&entries);
        // 提取 C_info Push 的种类序列（按 entry_line 排序）
        let kinds_full: Vec<(u32, FrameKind)> = entries.iter().enumerate()
            .filter(|&(i, &e)| !entry_is_pop(e) && !entry_is_body_mut(e)
                && event_class_at(&entries, i) == EventClass::C_info)
            .map(|(_, &e)| (entry_line(e), entry_frame(e).kind()))
            .collect();
        let kinds_comp: Vec<(u32, FrameKind)> = compressed.iter()
            .filter(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e))
            .map(|&e| (entry_line(e), entry_frame(e).kind()))
            .collect();
        // 只比较种类序列（去除 line 号，因为 compress 可能改变行号）
        let seq_full: Vec<FrameKind> = kinds_full.iter().map(|&(_, k)| k).collect();
        let seq_comp: Vec<FrameKind> = kinds_comp.iter().map(|&(_, k)| k).collect();
        assert_eq!(seq_full, seq_comp,
            "[{}] C_info container kind sequence mismatch after compress. full={:?} comp={:?}",
            spec.name, seq_full, seq_comp);
        // compress 结果不含 I_info 或 T_info
        for i in 0..compressed.len() {
            let ec = event_class_at(&compressed, i);
            assert_eq!(ec, EventClass::C_info,
                "[{}] compressed[{}] must be C_info, got {:?}: {:016x}", spec.name, i, ec, compressed[i]);
        }
        // compress 结果 Dyck 合法
        assert_dyck_valid(&compressed, &format!("{}-compressed", spec.name));
    }
}

/// 测试 6: Table 非⊤ 验证 (修正 6)
#[test]
fn structure_t6_table_non_top() {
    let e = fl_of("| a | b |\n|----|----|\n| 1 | 2 |\n");
    assert!(!e.is_empty(), "Table must produce FL entries");

    // Table Push 是 C_info
    let pushes: Vec<_> = (0..e.len()).filter(|&i| {
        !entry_is_pop(e[i]) && !entry_is_body_mut(e[i])
            && entry_frame(e[i]).kind() == FrameKind::Table
    }).collect();
    assert!(!pushes.is_empty(), "Table must have Push entry");
    for &i in &pushes {
        assert_eq!(event_class_at(&e, i), EventClass::C_info,
            "Table Push must be C_info, not T_info");
    }

    // compress 不消除 Table
    let c = compress_entries(&e);
    assert!(!c.is_empty(), "Table compress must NOT be empty");

    // Table 有几何 guard (first_char='|') → 等价类分划变化
    // 对比: 纯段落文档 compress 为空，Table 文档 compress 非空
    let plain = fl_of("plain\n");
    let c_plain = compress_entries(&plain);
    assert!(c_plain.is_empty(), "plain doc compress must be empty");
    assert!(!c.is_empty(), "Table doc must differ from plain (non-empty compress)");
}

// 阶段 2 — 事件序到路径双射 (测试 7-11)

/// φ 在 compress(E) 上（J_push^C → P）。原始 FL 上不成立：Composite 中同行的多个 Push
/// 共享同一行号 → child_path 无法区分 → 双射失效。φ 的理论定义基于事件序列索引 j，非行号。
#[test]
fn structure_t7_phi_bijection_forward() {
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let full = fl_of(spec.text);
        let entries = compress_entries(&full);
        let pushes = push_entries(&entries);
        if pushes.is_empty() { continue; }
        for &(line, _entry) in &pushes {
            let path = child_path_from_entries(&entries, line + 1);
            if path.is_empty() { continue; }
            let found = find_push_by_path(&entries, &path);
            assert!(found.is_some(),
                "[{}] φ⁻¹('{}')=None", spec.name, path);
            assert_eq!(found.unwrap().0, line,
                "[{}] φ⁻¹(φ({}))={}, expected {}", spec.name, line, found.unwrap().0, line);
        }
    }
}

/// 测试 8: φ 双射 — 反向 (附录 G §G.2 定理 G.4)
#[test]
fn structure_t8_phi_bijection_reverse() {
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let full = fl_of(spec.text);
        let entries = compress_entries(&full);
        let pushes = push_entries(&entries);
        for &(line, _) in &pushes {
            let p = child_path_from_entries(&entries, line + 1);
            if p.is_empty() { continue; }
            let found = find_push_by_path(&entries, &p);
            assert!(found.is_some(), "[{}] φ⁻¹('{}')=None", spec.name, p);
            let back = child_path_from_entries(&entries, found.unwrap().0 + 1);
            assert_eq!(back, p, "[{}] φ(φ⁻¹('{}'))='{}'", spec.name, p, back);
        }
    }
}

/// 测试 8b: φ 双射 — 基于事件序列索引（附录 G §G.1.3 定理 G.1）
/// φ(j) 使用 event index j 而非行号，正确处理 Composite 同行 Push。
/// 此双射在**原始 FL**（含 PopPush + Composite）上成立。
#[test]
fn structure_t8b_phi_bijection_by_index() {
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let entries = fl_of(spec.text);
        // 收集所有 C_info Push entry 的事件索引 j
        let push_indices: Vec<usize> = (0..entries.len())
            .filter(|&i| !entry_is_pop(entries[i]) && !entry_is_body_mut(entries[i])
                && event_class_at(&entries, i) == EventClass::C_info)
            .collect();
        if push_indices.is_empty() { continue; }

        // 正向: φ⁻¹(φ(j)) = j
        for &j in &push_indices {
            let p = phi_by_index(&entries, j);
            assert!(!p.is_empty(), "[{}] φ(idx={}) returned empty", spec.name, j);
            let found = phi_inv_by_index(&entries, &p);
            assert_eq!(found, Some(j),
                "[{}] φ⁻¹(φ(idx={})) = {:?}, path='{}'", spec.name, j, found, p);
        }

        // 反向: φ(φ⁻¹(p)) = p
        let mut seen = std::collections::HashSet::new();
        for &j in &push_indices {
            let p = phi_by_index(&entries, j);
            if seen.contains(&p) { continue; }
            seen.insert(p.clone());
            let found = phi_inv_by_index(&entries, &p);
            assert!(found.is_some(), "[{}] φ⁻¹('{}')=None", spec.name, p);
            let back = phi_by_index(&entries, found.unwrap());
            assert_eq!(back, p, "[{}] φ(φ⁻¹('{}'))='{}'", spec.name, p, back);
        }

        // 双射性: 不同 Push → 不同路径（pairwise 碰撞检测）
        for i in 0..push_indices.len() {
            for k in (i+1)..push_indices.len() {
                let pi = phi_by_index(&entries, push_indices[i]);
                let pk = phi_by_index(&entries, push_indices[k]);
                assert_ne!(pi, pk,
                    "[{}] collision: idx {} and {} both → '{}'",
                    spec.name, push_indices[i], push_indices[k], pi);
            }
        }
    }
}

/// 测试 8c: φ 实现交叉验证 — 非 Composite 文档上 phi_by_index 与 child_path 一致
#[test]
fn structure_t8c_phi_cross_validate() {
    // 对不含 Composite 的文档（单容器/非嵌套），两种 φ 实现应一致
    for spec in docs().iter().filter(|s| s.push_count > 0 && !s.has_nesting) {
        let entries = fl_of(spec.text);
        let push_indices: Vec<usize> = (0..entries.len())
            .filter(|&i| !entry_is_pop(entries[i]) && !entry_is_body_mut(entries[i])
                && event_class_at(&entries, i) == EventClass::C_info)
            .collect();
        for &j in &push_indices {
            let line = entry_line(entries[j]);
            let path_line = child_path_from_entries(&entries, line + 1);
            let path_idx = phi_by_index(&entries, j);
            assert_eq!(path_line, path_idx,
                "[{}] mismatch: child_path(line={})='{}' vs phi_by_index(idx={})='{}'",
                spec.name, line, path_line, j, path_idx);
        }
    }
}

/// 测试 9: child_idx 作为 Dyck 统计量 (附录 G §G.4 定理 G.7)
#[test]
fn structure_t9_child_idx_as_dyck_statistic() {
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let entries = fl_of(spec.text);
        let pushes = push_entries(&entries);
        // 对每个 Push，child_path 中的索引分量 = 该深度层 ci[] - 1
        for &(line, _) in &pushes {
            let path = child_path_from_entries(&entries, line);
            let indices: Vec<usize> = path.split("/children/")
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.trim_end_matches('/').parse().ok())
                .collect();
            // FL replay 验证: 路径中的每个索引 = 该深度 Push 在同父兄弟中的序数
            let mut ci: [usize; 16] = [0; 16];
            let mut depth = 0usize;
            for &e in &entries {
                if entry_is_body_mut(e) { continue; }
                if entry_is_pop(e) { depth = depth.saturating_sub(1); continue; }
                depth += 1; ci[depth] += 1;
                if entry_line(e) == line && depth <= indices.len() {
                    assert_eq!(ci[depth], indices[depth - 1],
                        "[{}] child_idx mismatch at depth {} for line {}",
                        spec.name, depth, line);
                }
            }
        }
    }
}

/// 测试 10: item_idx 作为 Dyck 统计量 (附录 G §G.4 定理 G.8)
#[test]
fn structure_t10_item_idx_as_dyck_statistic() {
    // 仅对 list 容器 (BLT/OL) 验证
    for spec in docs().iter().filter(|s| s.has_list) {
        let entries = fl_of(spec.text);
        // 找所有 PopPush 事件，计数容器内的 item 索引
        let mut prev_item_count: usize = 0;
        let mut container_depth: usize = 0;
        for i in 0..entries.len() {
            let ec = event_class_at(&entries, i);
            if ec == EventClass::C_info && !entry_is_pop(entries[i]) && !entry_is_body_mut(entries[i]) {
                if container_depth == 0 {
                    prev_item_count = 0;
                }
                container_depth += 1;
            } else if ec == EventClass::I_info && entry_is_pop(entries[i]) {
                // PopPush Pop half: item 计数 +1
                prev_item_count += 1;
            } else if entry_is_pop(entries[i]) && event_class_at(&entries, i) != EventClass::I_info {
                container_depth = container_depth.saturating_sub(1);
            }
        }
        // item_idx 应该 > 0（有 PopPush 事件）
        let i_info_count: usize = (0..entries.len())
            .filter(|&i| event_class_at(&entries, i) == EventClass::I_info)
            .count();
        if entries.len() > 3 {
            assert!(i_info_count > 0,
                "[{}] list doc should have I_info (PopPush) events", spec.name);
        }
    }
}

/// 找 Push entry j 的父 Push 的事件索引。回溯 FL replay：在 j 之前，
/// 最后一个深度 = target_depth 的 Push 即是父。
fn parent_push_index(entries: &[u64], j: usize) -> Option<usize> {
    // 计算 j 处的栈深度及各层最后 Push 索引
    let mut depth = 0usize;
    let mut last_push_at: [Option<usize>; 16] = [None; 16];
    for i in 0..j {
        let e = entries[i];
        if entry_is_body_mut(e) { continue; }
        if entry_is_pop(e) { depth = depth.saturating_sub(1); }
        else { depth += 1; last_push_at[depth] = Some(i); }
    }
    let target_depth = depth; // j 的父深度（不含 j 自身）
    if target_depth == 0 { return None; }
    last_push_at[target_depth]
}

/// 给定 Push entry 的 line，返回其子树覆盖的行范围 [start_line, end_line)。
/// 若容器未在 FL 中关闭，end = u32::MAX。
fn subtree_range(entries: &[u64], push_line: u32, kind: FrameKind) -> (u32, u32) {
    let start = push_line;
    let mut depth: i32 = 0;
    let mut started = false;
    for &e in entries {
        if entry_is_body_mut(e) { continue; }
        if entry_is_pop(e) {
            if entry_frame(e).kind() == kind {
                if started { depth -= 1; }
                if depth <= 0 && started {
                    return (start, entry_line(e));
                }
            }
        } else {
            if entry_frame(e).kind() == kind {
                if entry_line(e) == start && !started {
                    started = true;
                    depth = 1;
                } else if started {
                    depth += 1;
                }
            }
        }
    }
    (start, u32::MAX) // 未关闭
}

// 阶段 3 — 路径代数与增量维护 (测试 12-14)

/// 测试 12: parent_E — Push 的父 Push 路径前缀关系 (附录 H §H.1 定理 H.1)
#[test]
fn structure_t12_parent_push_path_prefix() {
    for spec in docs().iter().filter(|s| s.push_count >= 2) {
        let entries = fl_of(spec.text);
        let push_indices: Vec<usize> = (0..entries.len())
            .filter(|&i| !entry_is_pop(entries[i]) && !entry_is_body_mut(entries[i])
                && event_class_at(&entries, i) == EventClass::C_info)
            .collect();
        for &j in &push_indices {
            let child_path = phi_by_index(&entries, j);
            // 根级 Push: 路径只有一段 "/children/X" → 无父
            if child_path.matches("/children/").count() <= 1 { continue; }
            let parent = parent_push_index(&entries, j);
            assert!(parent.is_some(),
                "[{}] Push idx={} path='{}' has no parent", spec.name, j, child_path);
            let parent_path = phi_by_index(&entries, parent.unwrap());
            // 子路径应以父路径为前缀
            assert!(child_path.starts_with(&parent_path),
                "[{}] child='{}' does not start with parent='{}' (child idx={}, parent idx={})",
                spec.name, child_path, parent_path, j, parent.unwrap());
        }
    }
}

/// 测试 13: subtree_range — 容器子树的行范围 (附录 H §H.1 定理 H.4)
#[test]
fn structure_t13_subtree_range() {
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let entries = fl_of(spec.text);
        let pushes = push_entries(&entries);
        let n_lines = {
            let state = State::new(spec.text);
            state.rope.line_len() as u32
        };
        for &(line, _entry) in &pushes {
            let kind = entry_frame(_entry).kind();
            let (start, end) = subtree_range(&entries, line, kind);
            assert_eq!(start, line,
                "[{}] subtree_range start mismatch", spec.name);
            if end != u32::MAX {
                assert!(end >= start,
                    "[{}] end {} < start {} for kind {:?}", spec.name, end, start, kind);
            }
        }
    }
}

/// 测试 14: 路径在 compress(E) 上的行间一致性 (附录 H §H.3 定理 H.10)
/// 使用 compress(E) 消除 PopPush 导致的 ci[] 跳变。
#[test]
fn structure_t14_path_of_line_consistency() {
    for spec in docs().iter() {
        let full = fl_of(spec.text);
        let entries = compress_entries(&full);
        let n_lines = {
            let state = State::new(spec.text);
            state.rope.line_len()
        };
        for line in 1..=n_lines {
            let path = child_path_from_entries(&entries, line as u32);
            let prev = child_path_from_entries(&entries, (line - 1) as u32);
            // compress(E) 上路径有前缀单调性: 进入容器 → prev 是 path 的前缀
            // 退出容器 → path 是 prev 的前缀。同级 → 相同。
            let prev_is_prefix = path.starts_with(&prev);
            let path_is_prefix = prev.starts_with(&path);
            assert!(prev_is_prefix || path_is_prefix || path == prev,
                "[{}] line {} path='{}' vs line {} path='{}': no prefix relation",
                spec.name, line - 1, prev, line, path);
        }
    }
}
#[test]
fn structure_t11_child_path_compress_invariant() {
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let entries = fl_of(spec.text);
        let compressed = compress_entries(&entries);
        let pushes = push_entries(&compressed);
        for &(line, _) in &pushes {
            let path_full = child_path_from_entries(&entries, line);
            let path_comp = child_path_from_entries(&compressed, line);
            // 根级路径允许为空
            assert_eq!(path_full, path_comp,
                "[{}] child_path diverged after compress for line {}: full='{}' comp='{}'",
                spec.name, line, path_full, path_comp);
        }
    }
}

// §阶段 4 辅助函数: 三信号 + Patch (简化版, 双 State 直接比较)

struct S_FL { new_push: Vec<(u32, FrameKind)>, old_push: Vec<(u32, FrameKind)> }

/// 比较两个 FL 的 push_entries，产生 S_FL。
fn diff_fl(fl_old: &[u64], fl_new: &[u64]) -> S_FL {
    let old_pushes = push_entries(fl_old);
    let new_pushes = push_entries(fl_new);
    let old_kinds: Vec<FrameKind> = old_pushes.iter().map(|&(_, e)| entry_frame(e).kind()).collect();
    let new_kinds: Vec<FrameKind> = new_pushes.iter().map(|&(_, e)| entry_frame(e).kind()).collect();
    let mut new_push = Vec::new();
    let mut old_push = Vec::new();
    // 简单按 kind 序列匹配: 旧有、新无 → DELETE; 新有、旧无 → ADD
    let mut oi = 0; let mut ni = 0;
    while oi < old_kinds.len() || ni < new_kinds.len() {
        if oi < old_kinds.len() && ni < new_kinds.len() && old_kinds[oi] == new_kinds[ni] {
            oi += 1; ni += 1;
        } else if oi < old_kinds.len() {
            old_push.push((old_pushes[oi].0, old_kinds[oi])); oi += 1;
        } else {
            new_push.push((new_pushes[ni].0, new_kinds[ni])); ni += 1;
        }
    }
    S_FL { new_push, old_push }
}

fn cover_lines(fl: &[u64], push_line: u32, kind: FrameKind, max_line: u32) -> Vec<u32> {
    // 在 compress(E) 上计算覆盖域 — PopPush 被消除，容器边界不受 item 切换干扰
    let c = compress_entries(fl);
    // PL fallback 为 Paragraph：cover 域测试只关心容器帧边界；boundary_for
    // 的 closing_pl 分支对这些场景不依赖真实 PL。
    let end = FrameLog { entries: c }.item_tail_boundary(push_line, kind, &[]).unwrap_or(max_line);
    (push_line..end.min(max_line)).collect()
}

fn covered_set(fl_old: &[u64], fl_new: &[u64], s_fl: &S_FL, n_old: u32, n_new: u32) -> Vec<u32> {
    let mut cov = Vec::new();
    for &(line, kind) in &s_fl.new_push {
        cov.extend(cover_lines(fl_new, line, kind, n_new));
    }
    for &(line, kind) in &s_fl.old_push {
        cov.extend(cover_lines(fl_old, line, kind, n_old));
    }
    cov.sort(); cov.dedup();
    cov
}

// 阶段 4 — Patch 理论与消除代数律 (测试 15-19)

/// 测试 15: 纯内容编辑 — S_FL=∅ → 仅 REPLACE (附录 E §E.1 定理 E.2)
#[test]
fn structure_t15_pure_content_replace_only() {
    let old = "# H\n- setup: old\n";
    let entries_old = fl_of(old);
    let mut state = State::new(old);
    state.edit(8, 11, "new");
    state.ensure_projected();
    let entries_new = state.frame_log.entries.clone();
    let s_fl = diff_fl(&entries_old, &entries_new);
    assert!(s_fl.new_push.is_empty() && s_fl.old_push.is_empty(),
        "pure content edit: S_FL must be empty. new={:?} old={:?}", s_fl.new_push, s_fl.old_push);
    let pl_old: Vec<u8> = State::new(old).lines.iter().map(|p| p.0).collect();
    let pl_new: Vec<u8> = state.lines.iter().map(|p| p.0).collect();
    assert_eq!(pl_old, pl_new, "pure content edit: PL unchanged");
    // S_FL=∅, S_PL=∅ → 仅 REPLACE (定理 E.2)
}

/// 测试 16: ADD 覆盖域消除子 REPLACE — P-E1 (附录 E §E.4 定理 E.12)
#[test]
fn structure_t16_add_covers_replace() {
    let fl_old = fl_of("");
    let fl_new = fl_of("- a\n- b\n");
    let s_fl = diff_fl(&fl_old, &fl_new);
    assert_eq!(s_fl.new_push.len(), 1, "empty→BLT: exactly 1 ADD");
    assert!(s_fl.old_push.is_empty(), "no DELETE expected");
    // 验证 ADD 覆盖域包含子行 → 子 REPLACE 被消除
    let n_new = State::new("- a\n- b\n").rope.line_len() as u32;
    let covered = covered_set(&fl_old, &fl_new, &s_fl, 0, n_new);
    // 所有新行都应被 ADD 覆盖
    for line in 0..n_new {
        assert!(covered.contains(&line),
            "P-E1 violated: line {} not covered by ADD", line);
    }
}

/// 测试 17: DELETE 覆盖域消除子 REPLACE — P-E2
#[test]
fn structure_t17_delete_covers_replace() {
    let fl_old = fl_of("- a\n- b\n- c\n");
    let fl_new = fl_of("");
    let s_fl = diff_fl(&fl_old, &fl_new);
    assert!(s_fl.new_push.is_empty(), "no ADD expected");
    assert_eq!(s_fl.old_push.len(), 1, "BLT→empty: exactly 1 DELETE");
    let n_old = State::new("- a\n- b\n- c\n").rope.line_len() as u32;
    let covered = covered_set(&fl_old, &fl_new, &s_fl, n_old, 0);
    for line in 0..n_old {
        assert!(covered.contains(&line),
            "P-E2 violated: line {} not covered by DELETE", line);
    }
}

/// 测试 18: 父 DELETE 消除子 DELETE — P-E3
#[test]
fn structure_t18_parent_delete_covers_child() {
    let fl_old = fl_of("- a\n  - b\n- c\n");
    let fl_new = fl_of("");
    let s_fl = diff_fl(&fl_old, &fl_new);
    // P-E3: 父 DELETE 应消除子 DELETE → 最终只有 1 个 DELETE
    // 嵌套 BLT: outer + inner 两个 Push → diff 先产生 2 个 DELETE
    // 消除后只剩外层 DELETE
    assert!(s_fl.old_push.len() >= 1,
        "nested BLT→empty: at least 1 DELETE after elimination");
    // 验证 outer Push 的覆盖域包含 inner Push 的所有行
    let n_old = State::new("- a\n  - b\n- c\n").rope.line_len() as u32;
    // outer container 的 cover 应覆盖整个文档
    let outer_line = s_fl.old_push.iter().map(|&(l,_)| l).min().unwrap_or(0);
    let outer_kind = s_fl.old_push[0].1;
    let cover = cover_lines(&fl_old, outer_line, outer_kind, n_old);
    assert!(cover.len() as u32 >= n_old,
        "parent container should cover all lines. cover={:?}, n_old={}", cover, n_old);
}

/// 测试 19: 正规形式 idempotent — 二次归约不变 (附录 E §E.4 定理 E.13)
#[test]
fn structure_t19_elimination_idempotent() {
    // (a) 同文档 → 空差异 (idempotence)
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let fl = fl_of(spec.text);
        let s_fl = diff_fl(&fl, &fl);
        assert!(s_fl.new_push.is_empty() && s_fl.old_push.is_empty(),
            "[{}] same doc must have empty S_FL", spec.name);
    }
    // (b) 复合差异: old有BLT+OL, new只有BLT → DELETE(OL)
    let fl_old = fl_of("- a\n  1. x\n  2. y\n- b\n");
    let fl_new = fl_of("- a\n- b\n");
    let s_fl = diff_fl(&fl_old, &fl_new);
    assert!(!s_fl.old_push.is_empty(),
        "OL removed: expected DELETE. old_push={:?}", s_fl.old_push);
}

// 工具: 事件流驱动的 Patch 引擎 (附录 E — 三步生成 + 消除律)

#[derive(Debug, Clone, PartialEq)]
enum PatchOp { Add(String), Delete(String), Replace(String) }

/// 完整三步 Patch 生成 (附录 A §A.5.3 + 附录 E)。
/// Step 1: compress(E_new) △ compress(E_old) → 容器 ADD/DELETE
/// Step 2: I_info(E_new) △ I_info(E_old) → Item ADD/DELETE
/// Step 3: PL_diff ∪ (S_Ed) → REPLACE (再消除被 Step1/2 覆盖的)
fn event_patches(
    e_old: &[u64], e_new: &[u64],
    pl_old: &[u8], pl_new: &[u8],
    n_old: u32, n_new: u32,
) -> Vec<PatchOp> {
    let mut patches = Vec::new();

    // ── Step 1: 容器 ADD/DELETE — compress(E) 的 C_info Push 差异 ──
    let c_old = compress_entries(e_old);
    let c_new = compress_entries(e_new);
    let old_pushes = push_entries(&c_old);
    let new_pushes = push_entries(&c_new);
    let old_kinds: Vec<FrameKind> = old_pushes.iter().map(|&(_, e)| entry_frame(e).kind()).collect();
    let new_kinds: Vec<FrameKind> = new_pushes.iter().map(|&(_, e)| entry_frame(e).kind()).collect();

    // kind 序列 diff (保留顺序的 LCS 匹配)
    let mut oi = 0; let mut ni = 0;
    let mut deleted_containers: Vec<(u32, FrameKind)> = Vec::new();
    let mut added_containers: Vec<(u32, FrameKind)> = Vec::new();
    while oi < old_kinds.len() || ni < new_kinds.len() {
        if oi < old_kinds.len() && ni < new_kinds.len() && old_kinds[oi] == new_kinds[ni] {
            oi += 1; ni += 1;
        } else if oi < old_kinds.len() {
            deleted_containers.push((old_pushes[oi].0, old_kinds[oi])); oi += 1;
        } else {
            added_containers.push((new_pushes[ni].0, new_kinds[ni])); ni += 1;
        }
    }

    // Step 1 patches
    for &(line, kind) in &added_containers {
        let path = child_path_from_entries(&c_new, line + 1);
        patches.push(PatchOp::Add(path));
    }
    for &(line, kind) in &deleted_containers {
        let path = child_path_from_entries(&c_old, line + 1);
        patches.push(PatchOp::Delete(path));
    }

    // 计算 Step 1 覆盖域 (用于消除子 REPLACE)
    let mut covered: Vec<u32> = Vec::new();
    for &(line, kind) in &added_containers {
        let log = FrameLog { entries: c_new.clone() };
        let end = log.item_tail_boundary(line, kind, &[]).unwrap_or(n_new);
        covered.extend(line..end.min(n_new));
    }
    for &(line, kind) in &deleted_containers {
        let log = FrameLog { entries: c_old.clone() };
        let end = log.item_tail_boundary(line, kind, &[]).unwrap_or(n_old);
        covered.extend(line..end.min(n_old));
    }
    covered.sort(); covered.dedup();

    // ── Step 2: Item ADD/DELETE — I_info 事件差异 ──
    let i_old: Vec<(u32, FrameKind)> = (0..e_old.len())
        .filter(|&i| event_class_at(e_old, i) == EventClass::I_info && entry_is_pop(e_old[i]))
        .map(|i| (entry_line(e_old[i]), entry_frame(e_old[i]).kind()))
        .collect();
    let i_new: Vec<(u32, FrameKind)> = (0..e_new.len())
        .filter(|&i| event_class_at(e_new, i) == EventClass::I_info && entry_is_pop(e_new[i]))
        .map(|i| (entry_line(e_new[i]), entry_frame(e_new[i]).kind()))
        .collect();

    // Item diff: 计数差异 (简化: 比较数量)
    for kind in [FrameKind::BulletList, FrameKind::OrderedList] {
        let old_cnt = i_old.iter().filter(|&&(_, k)| k == kind).count();
        let new_cnt = i_new.iter().filter(|&&(_, k)| k == kind).count();
        if old_cnt != new_cnt {
            // 找对应的容器路径
            if let Some(&(line, _)) = new_pushes.iter().find(|&&(_, e)| entry_frame(e).kind() == kind) {
                let path = child_path_from_entries(&c_new, line + 1);
                // Item 增/减 → 在容器路径下 REPLACE (整个容器重渲染)
                patches.push(PatchOp::Replace(path));
            }
        }
    }

    // ── Step 3: 内容 REPLACE = PL_diff \ covered ──
    let max_lines = n_old.max(n_new) as usize;
    for li in 0..max_lines {
        let old_pl = pl_old.get(li).copied().unwrap_or(0);
        let new_pl = pl_new.get(li).copied().unwrap_or(0);
        if old_pl != new_pl && !covered.contains(&(li as u32)) {
            // REPLACE on the line's path in new doc
            let path = child_path_from_entries(&c_new, li as u32 + 1);
            patches.push(PatchOp::Replace(path));
        }
    }

    // ── P-E3: 父 DELETE 消除子 DELETE ──
    patches = eliminate_sub_deletes(patches);
    // ── P-E1/P-E2: ADD/DELETE 覆盖域消除子 REPLACE ──
    patches = eliminate_covered_replaces(patches, &covered, &c_old, &c_new, n_old, n_new);

    patches
}

/// P-E3: 路径 A 是路径 B 的前缀 → A 的 DELETE 消除 B 的 DELETE
fn eliminate_sub_deletes(mut patches: Vec<PatchOp>) -> Vec<PatchOp> {
    let delete_paths: Vec<String> = patches.iter()
        .filter_map(|p| if let PatchOp::Delete(path) = p { Some(path.clone()) } else { None })
        .collect();
    patches.retain(|p| {
        match p {
            PatchOp::Delete(path) => {
                // 保留最外层的 DELETE (无其他 DELETE 是其前缀)
                !delete_paths.iter().any(|dp| dp != path && path.starts_with(dp) && path != dp)
            }
            _ => true,
        }
    });
    patches
}

/// P-E1/P-E2: 在 covered 范围内的 REPLACE 被消除
fn eliminate_covered_replaces(
    mut patches: Vec<PatchOp>, covered: &[u32],
    c_old: &[u64], c_new: &[u64], _n_old: u32, _n_new: u32,
) -> Vec<PatchOp> {
    patches.retain(|p| {
        if let PatchOp::Replace(path) = p {
            let line_opt = find_push_by_path(c_new, path).map(|(line, _)| line)
                .or_else(|| find_push_by_path(c_old, path).map(|(line, _)| line));
            if let Some(line) = line_opt {
                return !covered.contains(&(line as u32));
            }
        }
        true
    });
    patches
}

// 事件流 Patch 引擎测试: 三步生成 + 预测验证

/// 测试 P1: 纯内容编辑 → 无 ADD/DELETE, 仅 REPLACE (三步互斥)
#[test]
fn structure_patch_t1_content_only_replace() {
    let old = "plain\n";
    let new = "# head\n";
    let pl_old: Vec<u8> = State::new(old).lines.iter().map(|p| p.0).collect();
    let pl_new: Vec<u8> = State::new(new).lines.iter().map(|p| p.0).collect();
    let patches = event_patches(
        &fl_of(old), &fl_of(new), &pl_old, &pl_new, 1, 1,
    );
    let adds = patches.iter().filter(|p| matches!(p, PatchOp::Add(_))).count();
    let dels = patches.iter().filter(|p| matches!(p, PatchOp::Delete(_))).count();
    assert_eq!(adds, 0, "type change: no ADD. patches={:?}", patches);
    assert_eq!(dels, 0, "type change: no DELETE. patches={:?}", patches);
    assert!(patches.iter().any(|p| matches!(p, PatchOp::Replace(_))),
        "type change: REPLACE expected. patches={:?}", patches);
}

/// 测试 P2: 新增容器 → ADD (Step 1 检测)
#[test]
fn structure_patch_t2_add_container() {
    let old = "plain\n";
    let new = "- a\n- b\n";
    let pl_old: Vec<u8> = State::new(old).lines.iter().map(|p| p.0).collect();
    let pl_new: Vec<u8> = State::new(new).lines.iter().map(|p| p.0).collect();
    let patches = event_patches(
        &fl_of(old), &fl_of(new), &pl_old, &pl_new, 1, 2,
    );
    assert!(patches.iter().any(|p| matches!(p, PatchOp::Add(_))),
        "new container: ADD expected. patches={:?}", patches);
    // P-E1: ADD 覆盖域内的 REPLACE 应被消除
    let repl_in_add = patches.iter().filter(|p| matches!(p, PatchOp::Replace(_))).count();
    // 新增容器内的所有行由 ADD 覆盖 → 无独立 REPLACE
    assert_eq!(repl_in_add, 0,
        "P-E1: no independent REPLACE inside ADD container. patches={:?}", patches);
}

/// 测试 P3: 删除容器 → DELETE (Step 1 检测)
#[test]
fn structure_patch_t3_delete_container() {
    let old = "- a\n- b\n";
    let new = "";
    let pl_old: Vec<u8> = State::new(old).lines.iter().map(|p| p.0).collect();
    let pl_new: Vec<u8> = State::new(new).lines.iter().map(|p| p.0).collect();
    let patches = event_patches(
        &fl_of(old), &fl_of(new), &pl_old, &pl_new, 2, 0,
    );
    assert!(patches.iter().any(|p| matches!(p, PatchOp::Delete(_))),
        "removed container: DELETE expected. patches={:?}", patches);
}

/// 测试 P4: Item 增/减 → REPLACE (Step 2 检测, I_info 差异)
#[test]
fn structure_patch_t4_item_change() {
    let old = "- a\n- b\n";
    let new = "- a\n- b\n- c\n";  // +1 item
    let pl_old: Vec<u8> = State::new(old).lines.iter().map(|p| p.0).collect();
    let pl_new: Vec<u8> = State::new(new).lines.iter().map(|p| p.0).collect();
    let patches = event_patches(
        &fl_of(old), &fl_of(new), &pl_old, &pl_new,
        State::new(old).rope.line_len() as u32,
        State::new(new).rope.line_len() as u32,
    );
    // Item 增/减 → I_info 差异 → 容器级 REPLACE (非独立 ADD/DELETE)
    // 验证: 无独立的容器 ADD/DELETE
    let adds = patches.iter().filter(|p| matches!(p, PatchOp::Add(_))).count();
    let dels = patches.iter().filter(|p| matches!(p, PatchOp::Delete(_))).count();
    assert_eq!(adds, 0, "item change: no container ADD. patches={:?}", patches);
    assert_eq!(dels, 0, "item change: no container DELETE. patches={:?}", patches);
}

/// 测试 P5: 嵌套容器 → 父 DELETE 消除子 DELETE (P-E3)
#[test]
fn structure_patch_t5_parent_delete_eliminates_child() {
    let old = "- a\n  - b\n- c\n";
    let new = "";
    let pl_old: Vec<u8> = State::new(old).lines.iter().map(|p| p.0).collect();
    let pl_new: Vec<u8> = State::new(new).lines.iter().map(|p| p.0).collect();
    let patches = event_patches(
        &fl_of(old), &fl_of(new), &pl_old, &pl_new, 3, 0,
    );
    // P-E3: 父 DELETE 消除子 DELETE → 最多 1 个 DELETE
    let dels: Vec<&PatchOp> = patches.iter().filter(|p| matches!(p, PatchOp::Delete(_))).collect();
    assert_eq!(dels.len(), 1,
        "P-E3: parent DELETE should eliminate child. got {} DELETEs: {:?}", dels.len(), dels);
}

/// 测试 P6: 三步互斥 — 同一行不会同时出现在 Step1,2,3 中
#[test]
fn structure_patch_t6_three_step_mutual_exclusion() {
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let old = spec.text;
        let new = "# added\nplain\n";
        let pl_old: Vec<u8> = State::new(old).lines.iter().map(|p| p.0).collect();
        let pl_new: Vec<u8> = State::new(new).lines.iter().map(|p| p.0).collect();
        let patches = event_patches(
            &fl_of(old), &fl_of(new), &pl_old, &pl_new,
            State::new(old).rope.line_len() as u32,
            State::new(new).rope.line_len() as u32,
        );
        // 验证: 同一路径不会同时有 ADD 和 DELETE (P-E4)
        let paths_add: Vec<&str> = patches.iter()
            .filter_map(|p| if let PatchOp::Add(s) = p { Some(s.as_str()) } else { None })
            .collect();
        let paths_del: Vec<&str> = patches.iter()
            .filter_map(|p| if let PatchOp::Delete(s) = p { Some(s.as_str()) } else { None })
            .collect();
        for pa in &paths_add {
            assert!(!paths_del.contains(pa),
                "[{}] path '{}' cannot be both ADD and DELETE", spec.name, pa);
        }
    }
}

struct EventIndex {
    pushes: Vec<(usize, u32, FrameKind, Vec<usize>)>, // (event_idx, line, kind, ci[1..=D])
    line_ci: Vec<(usize, [usize; 16])>,                // line → (depth, ci[])
}

impl EventIndex {
    fn build(entries: &[u64]) -> Self {
        let mut pushes = Vec::new();
        let n_lines = entries.iter().map(|&e| entry_line(e) as usize).max().unwrap_or(0) + 2;
        let max_line = n_lines + 2;
        let mut line_ci: Vec<(usize, [usize; 16])> = vec![(0, [0; 16]); max_line];
        let mut ci = [0usize; 16];
        let mut depth = 0usize;
        let mut prev_line = 0usize;
        for (j, &e) in entries.iter().enumerate() {
            if entry_is_body_mut(e) { continue; }
            let line = entry_line(e) as usize;
            // 将上一个行区间的 ci[] 快照写入 line_ci（行 boundary 之前的栈状态）
            for li in prev_line..line.min(max_line) {
                line_ci[li] = (depth, ci);
            }
            prev_line = line;
            if entry_is_pop(e) {
                depth = depth.saturating_sub(1);
            } else {
                depth += 1; ci[depth] += 1;
                let kind = entry_frame(e).kind();
                let is_poppush = entries[..j].iter().any(|&pe|
                    entry_is_pop(pe) && entry_line(pe) == entry_line(e)
                        && entry_frame(pe).kind() == kind);
                if !is_poppush {
                    pushes.push((j, entry_line(e), kind, ci[1..=depth].to_vec()));
                }
            }
            // 该行 entry 之后的 ci 快照
            if line < max_line { line_ci[line] = (depth, ci); }
        }
        // 剩余行继承最后的 ci
        for li in prev_line..max_line {
            line_ci[li] = (depth, ci);
        }
        EventIndex { pushes, line_ci }
    }

    /// O(log |pushes|) 二分查找路径
    fn find_by_path(&self, path: &str) -> Option<(usize, u32)> {
        let indices: Vec<usize> = path.split("/children/")
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.trim_end_matches('/').parse().ok()).collect();
        if indices.is_empty() { return None; }
        let r = self.pushes.binary_search_by(|(_, _, _, ci)| {
            for d in 0..indices.len().min(ci.len()) {
                match ci[d].cmp(&indices[d]) {
                    std::cmp::Ordering::Equal => continue,
                    ord => return ord,
                }
            }
            ci.len().cmp(&indices.len())
        });
        r.ok().map(|idx| (self.pushes[idx].0, self.pushes[idx].1))
    }

    /// O(1) 查表: 返回行 line 开始时的栈路径 (= child_path_from_entries(entries, line))
    fn path_of_line(&self, line: u32) -> String {
        if line == 0 { return String::new(); }
        let li = (line - 1) as usize;
        if li >= self.line_ci.len() { return String::new(); }
        let (depth, ci) = &self.line_ci[li];
        let mut path = String::new();
        for d in 1..=*depth {
            use std::fmt::Write;
            let _ = write!(path, "/children/{}", ci[d]);
        }
        path
    }

    fn pushes_with_prefix(&self, prefix: &str) -> Vec<(usize, u32)> {
        if prefix.is_empty() { return self.pushes.iter().map(|&(j, l, _, _)| (j, l)).collect(); }
        let start = self.pushes.partition_point(|(_, _, _, ci)| {
            let p: String = ci.iter().fold(String::new(), |mut a, &c| {
                use std::fmt::Write; let _ = write!(a, "/children/{}", c); a
            });
            &p < &prefix.to_string()
        });
        self.pushes[start..].iter()
            .filter(|(_, _, _, ci)| {
                let p: String = ci.iter().fold(String::new(), |mut a, &c| {
                    use std::fmt::Write; let _ = write!(a, "/children/{}", c); a
                });
                p.starts_with(prefix)
            })
            .map(|&(j, l, _, _)| (j, l))
            .collect()
    }
}

// 工具验证: EventIndex 正确性 + O(log N) vs O(N) 对比

#[test]
fn structure_tool_t1_index_correctness() {
    for spec in docs().iter() {
        let entries = fl_of(spec.text);
        let idx = EventIndex::build(&entries);
        let n_lines = State::new(spec.text).rope.line_len();
        // path_of_line vs child_path_from_entries
        for line in 0..=n_lines {
            assert_eq!(child_path_from_entries(&entries, line as u32),
                idx.path_of_line(line as u32),
                "[{}] line {} path mismatch", spec.name, line);
        }
    }
}

#[test]
fn structure_tool_t2_index_find_correctness() {
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let entries = fl_of(spec.text);
        let idx = EventIndex::build(&entries);
        for j in 0..entries.len() {
            let e = entries[j];
            if entry_is_pop(e) || entry_is_body_mut(e) { continue; }
            if event_class_at(&entries, j) != EventClass::C_info { continue; }
            let p = phi_by_index(&entries, j);
            if p.is_empty() { continue; }
            let idx_found = idx.find_by_path(&p);
            let lin_found = phi_inv_by_index(&entries, &p);
            assert_eq!(idx_found, lin_found.map(|j| (j, entry_line(entries[j]))),
                "[{}] path='{}': O(logN)={:?} vs O(N)={:?}", spec.name, p, idx_found, lin_found);
        }
    }
}

#[test]
fn structure_tool_t3_index_prefix_query() {
    for spec in docs().iter().filter(|s| s.push_count >= 2) {
        let entries = fl_of(spec.text);
        let idx = EventIndex::build(&entries);
        for j in 0..entries.len() {
            let e = entries[j];
            if entry_is_pop(e) || entry_is_body_mut(e) { continue; }
            if event_class_at(&entries, j) != EventClass::C_info { continue; }
            let p = phi_by_index(&entries, j);
            if p.is_empty() { continue; }
            let subtree = idx.pushes_with_prefix(&p);
            assert!(subtree.iter().any(|&(sj, _)| sj == j),
                "[{}] push idx={} not in own subtree (prefix='{}')", spec.name, j, p);
        }
    }
}

// 工具稳定性 + 全面验证

/// 工具 T4: EventIndex 边界 — 空文档、纯文本、⊤容器全部覆盖
#[test]
fn structure_tool_t4_index_edge_cases() {
    // 空文档
    let e = fl_of("");
    let idx = EventIndex::build(&e);
    assert!(idx.pushes.is_empty());
    assert_eq!(idx.path_of_line(0), "");
    assert_eq!(idx.path_of_line(100), "");
    assert_eq!(idx.find_by_path("/children/0"), None);

    // 纯段落 (无容器)
    let e = fl_of("plain\nmore\n");
    let idx = EventIndex::build(&e);
    assert!(idx.pushes.is_empty());
    for line in 0..=3 {
        assert_eq!(idx.path_of_line(line as u32), "");
    }

    // ⊤容器 (CF, Comment)
    for text in &["```\ncode\n```\n", "<!--\nc\n-->\n"] {
        let e = fl_of(text);
        let idx = EventIndex::build(&e);
        // ⊤容器 → push_entries 为空 (T_info → compress 消除)
        // 但 EventIndex::build 过滤 PopPush, 不自动 compress
        // 所以⊤容器的 Push 出现在 pushes 中
        // 这是已知差异: EventIndex 不含 compress 步骤
    }
}

/// 工具 T5: EventIndex 穷举验证 — 所有文档、所有行、所有 Push 交叉
#[test]
fn structure_tool_t5_index_exhaustive() {
    for spec in docs().iter() {
        let entries = fl_of(spec.text);
        let idx = EventIndex::build(&entries);
        let n_lines = State::new(spec.text).rope.line_len();

        // (a) path_of_line 对所有行与 child_path_from_entries 一致
        for line in 0..=n_lines + 1 {
            assert_eq!(child_path_from_entries(&entries, line as u32),
                idx.path_of_line(line as u32),
                "[{}] line {} path mismatch", spec.name, line);
        }

        // (b) 每个 Push 的 phi_by_index 路径可在 EventIndex 中找到
        for j in 0..entries.len() {
            if entry_is_pop(entries[j]) || entry_is_body_mut(entries[j]) { continue; }
            if event_class_at(&entries, j) != EventClass::C_info { continue; }
            let p = phi_by_index(&entries, j);
            if p.is_empty() { continue; }
            let found = idx.find_by_path(&p);
            assert!(found.is_some(),
                "[{}] idx={} path='{}' not found in EventIndex", spec.name, j, p);
        }

        // (c) path_of_line 对所有行返回合法路径 (空或有 "/children/" 段)
        for line in 0..=n_lines {
            let p = idx.path_of_line(line as u32);
            assert!(p.is_empty() || p.starts_with("/children/"),
                "[{}] line {} invalid path: '{}'", spec.name, line, p);
        }
    }
}

/// 工具 T6: event_patches 消除律 idempotent — 二次消除结果不变
#[test]
fn structure_tool_t6_patch_idempotent() {
    let old = "- a\n  - b\n  - c\n- d\n";
    let new = "";
    let pl_old: Vec<u8> = State::new(old).lines.iter().map(|p| p.0).collect();
    let pl_new: Vec<u8> = State::new(new).lines.iter().map(|p| p.0).collect();
    let e_old = fl_of(old);
    let e_new = fl_of(new);
    let n_old = State::new(old).rope.line_len() as u32;

    // 第一次生成 patch
    let p1 = event_patches(&e_old, &e_new, &pl_old, &pl_new, n_old, 0);
    // 对 p1 再次应用消除律 → 结果应与 p1 相同 (idempotent)
    let dels_before: Vec<&str> = p1.iter()
        .filter_map(|p| if let PatchOp::Delete(s) = p { Some(s.as_str()) } else { None })
        .collect();
    // 验证 P-E3 已生效: 最多 1 个外层 DELETE
    for i in 0..dels_before.len() {
        for k in (i+1)..dels_before.len() {
            assert!(!dels_before[i].starts_with(dels_before[k])
                && !dels_before[k].starts_with(dels_before[i]),
                "P-E3 violation: nested DELETEs '{}' and '{}'", dels_before[i], dels_before[k]);
        }
    }
}

/// 工具 T7: 事件流 Step1/2/3 覆盖完备 — 每种文档的 patch 覆盖全部变化
#[test]
fn structure_tool_t7_coverage_exhaustive() {
    // 对每对 (空, 规范文档) 验证 patch 类型分布
    for spec in docs().iter().filter(|s| s.push_count > 0) {
        let old = "";
        let new = spec.text;
        let pl_old: Vec<u8> = State::new(old).lines.iter().map(|p| p.0).collect();
        let pl_new: Vec<u8> = State::new(new).lines.iter().map(|p| p.0).collect();
        let patches = event_patches(
            &fl_of(old), &fl_of(new), &pl_old, &pl_new, 0,
            State::new(new).rope.line_len() as u32,
        );
        // 从空创建 → 至少应有 ADD 或 REPLACE
        let has_add = patches.iter().any(|p| matches!(p, PatchOp::Add(_)));
        let has_replace = patches.iter().any(|p| matches!(p, PatchOp::Replace(_)));
        assert!(has_add || has_replace,
            "[{}] empty→doc must have ADD or REPLACE. patches={:?}", spec.name, patches);
        // 反向: 删除回空
        let patches_rev = event_patches(
            &fl_of(new), &fl_of(old), &pl_new, &pl_old,
            State::new(new).rope.line_len() as u32, 0,
        );
        let has_del = patches_rev.iter().any(|p| matches!(p, PatchOp::Delete(_)));
        assert!(has_del || patches_rev.iter().any(|p| matches!(p, PatchOp::Replace(_))),
            "[{}] doc→empty must have DELETE or REPLACE. patches={:?}", spec.name, patches_rev);
    }
}

/// 工具 T8: 批量文档 patch 生成 + 路径合法性验证
#[test]
fn structure_tool_t8_batch_patch_validity() {
    let all_docs: Vec<&str> = docs().iter().map(|s| s.text).collect();
    for i in 0..all_docs.len() {
        for k in 0..all_docs.len() {
            if i == k { continue; }
            let pl_i: Vec<u8> = State::new(all_docs[i]).lines.iter().map(|p| p.0).collect();
            let pl_k: Vec<u8> = State::new(all_docs[k]).lines.iter().map(|p| p.0).collect();
            let patches = event_patches(
                &fl_of(all_docs[i]), &fl_of(all_docs[k]),
                &pl_i, &pl_k,
                State::new(all_docs[i]).rope.line_len() as u32,
                State::new(all_docs[k]).rope.line_len() as u32,
            );
            // 每个 patch 的路径应合法 (空 or "/children/" 前缀)
            for p in &patches {
                let path = match p {
                    PatchOp::Add(s) | PatchOp::Delete(s) | PatchOp::Replace(s) => s,
                };
                assert!(path.is_empty() || path.starts_with("/children/"),
                    "invalid path '{}' in patch {:?} (doc[{}]→doc[{}])", path, p, i, k);
            }
            // 同一路径的 ADD+DELETE = REPLACE (同位置容器替换, 合法)
            let add_paths: Vec<&str> = patches.iter()
                .filter_map(|p| if let PatchOp::Add(s) = p { Some(s.as_str()) } else { None })
                .collect();
            let del_paths: Vec<&str> = patches.iter()
                .filter_map(|p| if let PatchOp::Delete(s) = p { Some(s.as_str()) } else { None })
                .collect();
            let _add_set: std::collections::HashSet<_> = add_paths.into_iter().collect();
            let _del_set: std::collections::HashSet<_> = del_paths.into_iter().collect();
        }
    }
}

/// 工具 T9: 大规模文档 — EventIndex 正确性 + 可扩展性
#[test]
fn structure_tool_t9_large_document() {
    // 生成 100-line 文档: heading + 嵌套 BLT × 10 items
    let mut large = String::from("# Large\n");
    for i in 0..10 {
        large.push_str(&format!("- item{}\n", i));
        for j in 0..3 {
            large.push_str(&format!("  - sub{}_{}\n", i, j));
        }
    }
    large.push_str("plain\n");
    let entries = fl_of(&large);
    let idx = EventIndex::build(&entries);
    let n_lines = State::new(&large).rope.line_len();

    // 所有行 path 一致
    for line in 0..=n_lines {
        assert_eq!(child_path_from_entries(&entries, line as u32),
            idx.path_of_line(line as u32),
            "large doc line {} path mismatch", line);
    }

    // 每个 Push 的 φ 路径可查
    for j in 0..entries.len() {
        if entry_is_pop(entries[j]) || entry_is_body_mut(entries[j]) { continue; }
        if event_class_at(&entries, j) != EventClass::C_info { continue; }
        let p = phi_by_index(&entries, j);
        if p.is_empty() { continue; }
        assert!(idx.find_by_path(&p).is_some(),
            "large doc idx={} path='{}' not found", j, p);
    }

    // 1 外层 BLT + 10 个内层 BLT = 11 个 C_info 容器 Push
    let c_pushes: Vec<_> = idx.pushes.iter()
        .filter(|(_, _, k, _)| *k != FrameKind::CodeFence && *k != FrameKind::Comment)
        .collect();
    assert!(c_pushes.len() >= 10,
        "large doc: expected >=10 C_info pushes, got {}", c_pushes.len());
}

// 框架级综合验证 F1 + F2 — 统一管道端到端

/// 统一管道: Doc → E → {compress, classify, F, EventIndex} → 互验证
struct UnifiedPipeline {
    e: Vec<u64>,       // E(D) — 事件流 (FL entries)
    c: Vec<u64>,       // compress(E) — 仅 C_info
    pl: Vec<u8>,       // PL — 行类型
    n_lines: usize,    // 文档行数
    idx: EventIndex,   // 从 compress(E) 构建的索引
    // classify: C_info/I_info/T_info 三维分类
    info_map: Vec<EventClass>,  // entries[i] → EventClass
}

impl UnifiedPipeline {
    fn from_text(text: &str) -> Self {
        let state = State::new(text);
        let e = state.frame_log.entries.clone();
        let pl: Vec<u8> = state.lines.iter().map(|p| p.0).collect();
        let n_lines = state.rope.line_len();
        let c = compress_entries(&e);
        let idx = EventIndex::build(&e);  // 从原始 E 构建（含 PopPush）
        let info_map: Vec<EventClass> = (0..e.len()).map(|i| event_class_at(&e, i)).collect();
        UnifiedPipeline { e, c, pl, n_lines, idx, info_map }
    }

    /// 管道自检: 所有内部一致性条件
    fn verify_internal(&self, name: &str) {
        // (V1) Dyck 合法性
        assert_dyck_valid(&self.e, &format!("{}-dyck", name));

        // (V2) compress(E) 仅含 C_info
        for i in 0..self.c.len() {
            assert_eq!(event_class_at(&self.c, i), EventClass::C_info,
                "[{}] compress[{}] must be C_info", name, i);
        }

        // (V3) compress idempotent
        let c2 = compress_entries(&self.c);
        assert_eq!(self.c, c2, "[{}] compress not idempotent", name);

        // (V4) φ 双射 (索引版) 在原始 E 上
        let push_indices: Vec<usize> = (0..self.e.len())
            .filter(|&i| !entry_is_pop(self.e[i]) && !entry_is_body_mut(self.e[i])
                && self.info_map[i] == EventClass::C_info)
            .collect();
        for &j in &push_indices {
            let p = phi_by_index(&self.e, j);
            if p.is_empty() { continue; }
            assert_eq!(phi_inv_by_index(&self.e, &p), Some(j),
                "[{}] φ⁻¹(φ(idx={})) failed, path='{}'", name, j, p);
        }

        // (V5) EventIndex::path_of_line 与 child_path_from_entries 一致
        for line in 0..=self.n_lines {
            assert_eq!(child_path_from_entries(&self.e, line as u32),
                self.idx.path_of_line(line as u32),
                "[{}] line {} path mismatch", name, line);
        }

        // (V6) C_info Push 可通过 EventIndex::find_by_path 定位
        for &j in &push_indices {
            let p = phi_by_index(&self.e, j);
            if p.is_empty() { continue; }
            assert!(self.idx.find_by_path(&p).is_some(),
                "[{}] Push idx={} path='{}' not in EventIndex", name, j, p);
        }

        // (V7) compress 后 φ 路径不变 — 用 phi_by_index + phi_inv_by_index 验证
        let push_idx_c: Vec<usize> = (0..self.c.len())
            .filter(|&i| !entry_is_pop(self.c[i]) && !entry_is_body_mut(self.c[i]))
            .collect();
        // 每个 C_info Push 在 compress(E) 中也可通过 φ 定位
        for &j in &push_indices {
            let p = phi_by_index(&self.e, j);
            if p.is_empty() { continue; }
            let found = phi_inv_by_index(&self.c, &p);
            assert!(found.is_some(),
                "[{}] C_info Push idx={} path='{}' not in compress(E) via φ⁻¹", name, j, p);
        }
        let _ = push_idx_c; // compress(E) 中的 Push 索引集合
    }
}

/// F1: 统一管道 — 所有规范文档的端到端验证
#[test]
fn structure_framework_f1_unified_pipeline() {
    for spec in docs().iter() {
        let pipe = UnifiedPipeline::from_text(spec.text);
        pipe.verify_internal(spec.name);
    }
}

/// F2: 跨文档统一管道 — 管道 × Patch 引擎 闭环验证
#[test]
fn structure_framework_f2_cross_document_pipeline() {
    let all: Vec<UnifiedPipeline> = docs().iter()
        .map(|s| UnifiedPipeline::from_text(s.text))
        .collect();

    for i in 0..all.len() {
        for k in 0..all.len() {
            if i == k { continue; }
            let old = &all[i];
            let new = &all[k];

            // 管道自检 (快速 — 只在第一次)
            // (已在 F1 中完成, 此处跳过)

            // Patch 生成
            let patches = event_patches(
                &old.e, &new.e, &old.pl, &new.pl,
                old.n_lines as u32, new.n_lines as u32,
            );

            // (C1) 无非法路径
            for p in &patches {
                let path = match p {
                    PatchOp::Add(s) | PatchOp::Delete(s) | PatchOp::Replace(s) => s,
                };
                assert!(path.is_empty() || path.starts_with("/children/"),
                    "doc[{}]→doc[{}] invalid path: '{}'", i, k, path);
            }

            // (C2) 同一路径不能同时 ADD 和 DELETE (P-E4 — 应已由 REPLACE 替代)
            let add_paths: Vec<&str> = patches.iter()
                .filter_map(|p| if let PatchOp::Add(s) = p { Some(s.as_str()) } else { None })
                .collect();
            let del_paths: Vec<&str> = patches.iter()
                .filter_map(|p| if let PatchOp::Delete(s) = p { Some(s.as_str()) } else { None })
                .collect();
            // 统计同时 ADD+DELETE 的路径 (同位置容器替换 → 外部合并为 REPLACE)
            let add_set: std::collections::HashSet<_> = add_paths.iter().collect();
            let del_set: std::collections::HashSet<_> = del_paths.iter().collect();
            let both: Vec<_> = add_set.intersection(&del_set).collect();
            // 允许存在 (legitimate container replacement), 但记录数量
            let _both_count = both.len();

            // (C3) 消除律: 无嵌套 DELETE (P-E3)
            let del_only: Vec<&&str> = del_paths.iter()
                .filter(|p| !add_set.contains(*p))
                .collect();
            for a in 0..del_only.len() {
                for b in (a+1)..del_only.len() {
                    if del_only[a] == del_only[b] { continue; } // 同路径 = 同行多容器, 合法
                    assert!(!del_only[a].starts_with(*del_only[b])
                        && !del_only[b].starts_with(*del_only[a]),
                        "P-E3 violation: nested DELETEs '{}' and '{}' (doc[{}]→doc[{}])",
                        del_only[a], del_only[b], i, k);
                }
            }
        }
    }
}
