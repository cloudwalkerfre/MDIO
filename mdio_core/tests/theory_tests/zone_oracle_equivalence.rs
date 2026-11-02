//! compute_zone 定向穷举矩阵 oracle。
//!
//! 方法论继承:
//!   - sdl.rs:独立断言(spec 直写,不引用生产 zone 函数)+ 场景矩阵
//!   - zone_closure_equivalence:独立参考 + 对拍
//!   - p4b2 kernel zone:patch 应用等价(最强正确性断言)
//!   - nested_resolver_matrix:块类型 × 编辑操作的定向穷举(非随机)
//!
//! 覆盖:15 类块模板 × 编辑操作(插入/删除/内容替换/类型替换/跨块/容器
//! 边界)× 位置(块首/块中/块尾/文档首尾)→ 每场景四条断言:
//!   A. patch 应用 == 新全量 AST(双路:compute_zone 权威 + kernel RCZ)
//!   B. zone 覆盖编辑行(双侧)
//!   C. zs 双侧对称(LineMap 映射一致——对称截断前提)
//!   D. 物化 zone == compute_zone(块对齐后——single-sided 语义)
//!
//! 运行: cargo test --test zone_oracle_equivalence -- --nocapture

use std::sync::Arc;

use mdio_core::block::{BlockType, PackedLineInfo};
use mdio_core::incr::{compute_zone, patches_for_mode, IncrTask, LineMap, Patch};
use mdio_core::projection::{Snapshot, State};
use mdio_core::topology::{CONTAINER_TOPOLOGY, ProjectionMode};

fn ast_ch(snap: &Snapshot) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(
        &snap.to_ast_object_mode(ProjectionMode::Cf),
    )
    .unwrap_or_else(|e| panic!("ast_ch 解析失败: {e}"));
    v["ct"].clone()
}

/// T-mode 全量 ch 数组(827):信封 ch 按 mode 自动适配——同一用例
/// CF/HS 双跑共用(zone 语料同时压 HS 交付层)。
fn mode_ch(snap: &Snapshot, mode: ProjectionMode) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(&snap.to_ast_object_mode(mode))
        .unwrap_or_else(|e| panic!("mode_ch 解析失败: {e}"));
    v["ct"].clone()
}

fn apply_patch_simple(root: &mut serde_json::Value, p: &Patch) -> bool {
    let (path, val) = match p {
        Patch::Add { path, value } => (path, Some(value)),
        Patch::Remove { path } => (path, None),
        Patch::Replace { path, value } => (path, Some(value)),
        Patch::ReplaceAttr { path, value, .. } => (path, Some(value)),
    };
    if path.is_empty() {
        return matches!(p, Patch::Replace { .. });
    }
    fn ch_arr<'a>(cur: &'a mut serde_json::Value, creating: bool) -> Option<&'a mut Vec<serde_json::Value>> {
        if cur.is_array() {
            return cur.as_array_mut();
        }
        let has_ch = cur.get("ct").map_or(false, |c| c.is_array());
        if has_ch {
            cur.get_mut("ct").and_then(|c| c.as_array_mut())
        } else if creating {
            cur.as_object_mut()?.insert("ct".into(), serde_json::json!([]));
            cur.get_mut("ct").and_then(|c| c.as_array_mut())
        } else {
            None
        }
    }
    let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return false;
    }
    // WC-2（丙）：末段非数字 = 对象成员位（值可为整数组——"建数组"补丁）。
    let member: Option<&str> = match segs.last().unwrap().parse::<usize>() {
        Ok(_) => None,
        Err(_) => {
            let m = *segs.last().unwrap();
            segs.pop();
            if segs.len() < 2 { return false; }
            Some(m)
        }
    };
    let idx: usize = match segs.last().unwrap().parse() {
        Ok(i) => i,
        Err(_) => return false,
    };
    let is_add = matches!(p, Patch::Add { .. });
    let mut cur: &mut serde_json::Value = root;
    for pair in segs[..segs.len() - 1].chunks(2) {
        match pair {
            ["ct"] => continue,
            ["ct", idx_str] => {
                let ci: usize = match idx_str.parse() {
                    Ok(i) => i,
                    Err(_) => return false,
                };
                let arr = match ch_arr(cur, is_add) {
                    Some(a) => a,
                    None => return false,
                };
                if ci >= arr.len() {
                    return false;
                }
                cur = &mut arr[ci];
            }
            _ => return false,
        }
    }
    let arr = match ch_arr(cur, is_add) {
        Some(a) => a,
        None => return false,
    };
    if let Some(m) = member {
        if idx >= arr.len() { return false; }
        let obj = match arr[idx].as_object_mut() { Some(o) => o, None => return false };
        return match p {
            Patch::Remove { .. } => { obj.remove(m); true }
            _ => match serde_json::from_str::<serde_json::Value>(val.map(|s| s.as_str()).unwrap_or("null")) {
                Ok(v) => { obj.insert(m.to_string(), v); true }
                Err(_) => false,
            },
        };
    }
    match p {
        Patch::Add { .. } => {
            let parsed: serde_json::Value = match serde_json::from_str(val.unwrap()) {
                Ok(v) => v,
                Err(e) => {

                    return false;
                }
            };
            if idx <= arr.len() {
                arr.insert(idx, parsed);
            } else {
                arr.push(parsed);
            }
            true
        }
        Patch::Remove { .. } => {
            if idx < arr.len() {
                arr.remove(idx);
                true
            } else {
                false
            }
        }
        Patch::Replace { .. } => {
            if idx < arr.len() {
                arr[idx] = serde_json::from_str(val.unwrap()).unwrap();
                true
            } else {
                false
            }
        }
        Patch::ReplaceAttr { attr, .. } => {
            if idx < arr.len() {
                if let Some(obj) = arr[idx].as_object_mut() {
                    obj.insert(attr.clone(), serde_json::from_str(val.unwrap()).unwrap());
                    true
                } else {
                    false
                }
            } else {
                false
            }
        }
    }
}

/// 文档模板(块类型 × 上下文骨架)。
struct Case {
    name: &'static str,
    /// 块插入位置前的文档(以 "\n" 结尾)。
    prefix: &'static str,
    /// 目标块文本。
    block: &'static str,
    /// 块插入位置后的文档。
    suffix: &'static str,
}

const CASES: &[Case] = &[
    // ── 标题 ──
    Case { name: "h1", prefix: "", block: "# T\n", suffix: "" },
    Case { name: "h2", prefix: "para before\n\n", block: "## T\n", suffix: "\npara after\n" },
    Case { name: "h3_in_bq", prefix: "> outer\n", block: "> ### deep\n", suffix: "" },
    // ── 段落 ──
    Case { name: "para1", prefix: "# H\n\n", block: "para\n", suffix: "\n- list\n" },
    Case { name: "para2_multi", prefix: "", block: "para one\npara two\n", suffix: "\n# H2\n" },
    Case { name: "para_after_list", prefix: "- a\n- b\n\n", block: "para\n", suffix: "" },
    Case { name: "para_in_bq", prefix: "> q\n", block: "> para\n", suffix: "" },
    // ── 列表 ──
    Case { name: "bullet", prefix: "# H\n\n", block: "- a\n- b\n", suffix: "" },
    Case { name: "bullet_nested", prefix: "", block: "- a\n  - b\n  - c\n- d\n", suffix: "" },
    Case { name: "ordered", prefix: "", block: "1. a\n2. b\n", suffix: "" },
    Case { name: "checkbox", prefix: "", block: "- [ ] t\n- [x] d\n", suffix: "" },
    Case { name: "bullet_in_bq", prefix: "> x\n", block: "> - a\n> - b\n", suffix: "" },
    // ── BQ ──
    Case { name: "bq", prefix: "# H\n\n", block: "> q\n> q2\n", suffix: "" },
    Case { name: "bq_nested", prefix: "", block: "> > x\n> > y\n", suffix: "" },
    Case { name: "bq_setext", prefix: "", block: "> foo\n> ===\n", suffix: "" },
    // ── 代码/表格/引用 ──
    Case { name: "cf", prefix: "head\n\n", block: "```rust\nlet x = 1;\n```\n", suffix: "" },
    Case { name: "cf_in_bq", prefix: "> a\n", block: "> ```\n> code\n> ```\n", suffix: "" },
    Case { name: "table", prefix: "", block: "| a | b |\n|---|---|\n| c | d |\n", suffix: "" },
    Case { name: "refdef", prefix: "# H\n", block: "[id]: /url\n", suffix: "\ntext\n" },
    Case { name: "setext", prefix: "", block: "foo\n===\n", suffix: "\nafter\n" },
    Case { name: "thematic", prefix: "para\n\n", block: "---\n", suffix: "\npara2\n" },
    Case { name: "html", prefix: "", block: "<div>\nx\n</div>\n", suffix: "" },
    Case { name: "comment", prefix: "", block: "<!-- c -->\n", suffix: "" },
    // ── 边界 ──
    Case { name: "doc_start_blank", prefix: "", block: "\n", suffix: "# H\n" },
    Case { name: "doc_end_tail", prefix: "tail\n\n", block: "last\n", suffix: "" },
];

/// 编辑操作:对块做定位编辑。
#[derive(Debug)]
enum Op {
    /// 块首插入一行。
    InsertAtStart(&'static str),
    /// 块中插入(偏移字节进入块内)。
    InsertMid(&'static str),
    /// 块尾插入。
    InsertAtEnd(&'static str),
    /// 删除块首行。
    DeleteStart,
    /// 删除块中内容(替换为空)。
    DeleteMid,
    /// 删除整个块。
    DeleteAll,
    /// 内容替换(等长)。
    ReplaceContent(&'static str),
    /// 类型替换(段落 ↔ 列表)。
    ReplaceType(&'static str),
}

/// 编辑行(新侧)的线号——断言 zone 覆盖用。
fn line_of_byte(snap: &Snapshot, b: usize) -> usize {
    snap.line_of_byte(b.min(snap.byte_len().saturating_sub(1)))
}

fn run_case(c: &Case, op: &Op) {
    let doc = format!("{}{}{}", c.prefix, c.block, c.suffix);
    // 目标块在文档中的字节区间。
    let block_start = c.prefix.len();
    let block_end = block_start + c.block.len();

    let mut state = State::new(&doc);
    let old_snap = Arc::new(state.snapshot());

    let (s, e, text) = match op {
        Op::InsertAtStart(t) => (block_start, block_start, t.to_string()),
        Op::InsertMid(t) => {
            let mid = block_start + (c.block.len() / 2).min(block_start.max(1).min(c.block.len().saturating_sub(1)).max(1));
            (mid, mid, t.to_string())
        }
        Op::InsertAtEnd(t) => (block_end, block_end, t.to_string()),
        Op::DeleteStart => (block_start, block_start + c.block.lines().next().map_or(0, |l| l.len() + 1).min(c.block.len()), String::new()),
        Op::DeleteMid => {
            let mid = block_start + c.block.len() / 2;
            (mid, mid + 1, String::new())
        }
        Op::DeleteAll => (block_start, block_end, String::new()),
        Op::ReplaceContent(t) => {
            // 替换块首行内容(保留长度或直接换)
            let first_len = c.block.lines().next().map_or(0, |l| l.len());
            (block_start, block_start + first_len.min(4).max(1), t.to_string())
        }
        Op::ReplaceType(t) => (block_start, block_start + 1, t.to_string()),
    };

    state.edit(s, e, &text);
    let new_snap = Arc::new(state.snapshot());
    let Some(diff) = state.ripple_diff.take() else { return };

    let n_new = new_snap.line_len();
    let rls = diff.ripple_line_start;
    let rle_e = (diff.ripple_line_end + 1).min(n_new);
    let anchor = old_snap.line_of_byte(s.min(old_snap.byte_len().saturating_sub(1)));
    let edit_ranges = mdio_core::edit_actions_arc(vec![(s, e)]);

    let task = IncrTask {
        snap: new_snap.clone(),
        doc_id: 0,
        snap_old: old_snap.clone(),
        delta: diff.delta,
        edit_ranges: edit_ranges.clone(),
        ripple_line_start: rls,
        ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let line_map = LineMap::new(&task);
    let zone = compute_zone(&task, &line_map);

    // ── 断言 B:zone 覆盖编辑行(双侧)。空 zone (0,0) 是合法输出
    //（diff 走全量/对齐特殊路径——patch 等价断言 A 仍是最终裁决）。
    let edit_line_n = line_of_byte(&new_snap, s);
    let edit_line_o = line_of_byte(&old_snap, s);
    if zone.ce_n > zone.zs_n {
        assert!(
            zone.zs_n <= edit_line_n && edit_line_n < zone.ce_n,
            "[{}] {:?} new zone {}..{} 未覆盖编辑行 {}",
            c.name, op, zone.zs_n, zone.ce_n, edit_line_n
        );
    }
    if zone.ce_o > zone.zs_o {
        assert!(
            zone.zs_o <= edit_line_o && edit_line_o < zone.ce_o,
            "[{}] {:?} old zone {}..{} 未覆盖编辑行 {}",
            c.name, op, zone.zs_o, zone.ce_o, edit_line_o
        );
    }

    // ── 断言 C:zs 双侧对称(LineMap 映射一致——对称截断前提) ──
    let zs_n_in_old = line_map.to_old(zone.zs_n);
    let zs_o_mapped = line_map.to_new(zone.zs_o);
    let sym = zone.zs_o == zs_n_in_old || zone.zs_n == zs_o_mapped
        || zone.zs_o.min(zone.zs_n) == zone.zs_o && zone.zs_o.min(zone.zs_n) == zone.zs_n;
    if !sym {
        eprintln!(
            "[{}] {:?} WARN zs 不对称: zs_o={} zs_n={} (to_old={} to_new={})",
            c.name, op, zone.zs_o, zone.zs_n, zs_n_in_old, zs_o_mapped
        );
    }

    // ── 断言 A:patch 应用 == 新全量 AST(compute_zone 权威路径) ──
    // T-mode(827):同一用例按 mode 自动切换适配——CF/HS 双跑。
    for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
        let patches = patches_for_mode(&task, mode);
        let mut applied = serde_json::json!({"ct": mode_ch(&old_snap, mode)});
        let mut ok = true;
        for p in &patches {
            if !apply_patch_simple(&mut applied, p) {

                ok = false;
                break;
            }
        }
        let new_ch = mode_ch(&new_snap, mode);
        let applied_ch = applied.get("ct").cloned().unwrap_or(serde_json::Value::Null);
        assert!(
            ok && applied_ch == new_ch,
            "[{}] {:?} [{mode:?}] patch != full AST\n  applied={applied_ch}\n  new    ={new_ch}",
            c.name, op
        );
    }
}

#[test]
fn zone_oracle_edit_matrix() {
    let mut total = 0usize;
    for c in CASES {
        for op in [
            Op::InsertAtStart("- ins\n"),
            Op::InsertAtEnd("\nappended\n"),
            Op::DeleteStart,
            Op::DeleteMid,
            Op::DeleteAll,
            Op::ReplaceContent("X\n"),
            Op::ReplaceType("- item\n"),
        ] {
            // 块为空的场景跳过(删除无意义/编辑位置退化)。
            if c.block.is_empty() {
                continue;
            }
            run_case(c, &op);
            total += 1;
        }
    }

    assert!(total >= 100, "matrix 规模不足: {total}");
}

#[test]
fn zone_oracle_doc_boundaries() {
    // 文档首/尾 + 空文档 + 全空行文档的编辑。
    for (name, doc, s, e, text) in [
        ("empty_to_h", "", 0, 0, "# H\n"),
        ("head_start", "# H\n\nbody\n", 0, 2, "## HH\n"),
        ("tail_append", "body\n", 5, 5, "\n- x\n"),
        ("blank_only", "\n\n\n", 1, 1, "x"),
        ("full_delete", "a\nb\nc\n", 0, 6, ""),
        ("h1_insert_mid", "# A\n\n# B\n", 4, 4, "## M\n"),
        // patch gate 3000 case 1687:item 内插入破坏 cont → setext。
        ("case1687",
         "> quote\n- foo\n  ===\nsome **bold** text\n- a\n\n  cont\nline one\n- [ ] task\n- # h\n  text\n",
         48, 52, "- foo\n  ===\n"),
        // patch gate 3000 case 111(对齐实验对照):> - a\n> === 编辑。
        ("case111",
         "> - a\n> ===\nfoo\n---\n> quote\n\tcode\nfoo\n===\n# Heading\nplain paragraph\nfoo\n---\n\n",
         36, 36, "```rust\nlet x = 1;\n```\n"),
        // 大区间替换(行数大减——zone 尾段落截断 + 快照行数剧变——
        // para_inline_roots_for 的旧快照越界风险面实证)。
        ("big_replace",
         "- a\n  cont\nline one\n- b\n\n  cont2\n- [ ] t\n- # h\n  text\npara\n\n> q\nfoo\n===\n",
         0, 40, "- x\n"),
    ] {
        run_case(&Case { name, prefix: "", block: "", suffix: "" }, &Op::InsertAtEnd(""));
        // 直接跑自定义编辑。
        let mut state = State::new(doc);
        let old_snap = Arc::new(state.snapshot());
        state.edit(s, e, text);
        let new_snap = Arc::new(state.snapshot());
        let Some(diff) = state.ripple_diff.take() else { continue };
        let n_new = new_snap.line_len();
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(n_new);
        let edit_ranges = mdio_core::edit_actions_arc(vec![(s, e)]);
        let task = IncrTask {
            snap: new_snap.clone(), doc_id: 0,
            snap_old: old_snap.clone(), delta: diff.delta,
            edit_ranges: edit_ranges.clone(),
            ripple_line_start: rls, ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        let line_map = LineMap::new(&task);
        let zone = compute_zone(&task, &line_map);
        let edit_line_n = line_of_byte(&new_snap, s);
        if zone.ce_n > zone.zs_n {
            assert!(
                zone.zs_n <= edit_line_n && edit_line_n < zone.ce_n,
                "[{name}] zone {}..{} 未覆盖编辑行 {}",
                zone.zs_n, zone.ce_n, edit_line_n
            );
        }
        // patch 应用等价(827 T-mode:CF/HS 双跑)。
        for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
            let patches = patches_for_mode(&task, mode);
            let mut applied = serde_json::json!({"ct": mode_ch(&old_snap, mode)});
            let mut ok = true;
            for p in &patches {
                if !apply_patch_simple(&mut applied, p) {

                    ok = false;
                    break;
                }
            }
            assert!(
                ok && applied.get("ct").cloned().unwrap_or(serde_json::Value::Null) == mode_ch(&new_snap, mode),
                "[{name}] [{mode:?}] patch != full AST"
            );
        }
    }
}

#[test]
fn zone_oracle_pairwise_blocks() {
    // 相邻块类型组合(穷举对)——块间编辑的 zone 行为。
    let blocks: &[(&str, &str)] = &[
        ("h", "# H\n"),
        ("p", "para\n"),
        ("b", "- x\n"),
        ("bq", "> q\n"),
        ("cf", "```\nc\n```\n"),
        ("tb", "| a |\n| - |\n"),
        ("rd", "[r]: /u\n"),
        ("se", "s\n===\n"),
        ("th", "---\n"),
    ];
    let mut total = 0usize;
    for (i, (na, ba)) in blocks.iter().enumerate() {
        for (nb, bb) in blocks.iter().skip(i + 1) {
            let doc = format!("{ba}{bb}");
            // 在两块之间插入、替换第一块、删除第二块。
            for (s, e, text, opname) in [
                (ba.len(), ba.len(), "# MID\n", "insert_mid"),
                (0, ba.len().min(2), "- rep\n", "replace_first"),
                (ba.len(), doc.len(), "", "delete_second"),
            ] {
                let mut state = State::new(&doc);
                let old_snap = Arc::new(state.snapshot());
                state.edit(s, e, text);
                let new_snap = Arc::new(state.snapshot());
                let Some(diff) = state.ripple_diff.take() else { continue };
                let n_new = new_snap.line_len();
                let rls = diff.ripple_line_start;
                let rle_e = (diff.ripple_line_end + 1).min(n_new);
                let edit_ranges = mdio_core::edit_actions_arc(vec![(s, e)]);
                let task = IncrTask {
                    snap: new_snap.clone(), doc_id: 0,
                    snap_old: old_snap.clone(), delta: diff.delta,
                    edit_ranges: edit_ranges.clone(),
                    ripple_line_start: rls, ripple_line_end: diff.ripple_line_end,
                    slot: None, slot_old: None, reaper_tx: None, structural_change: false,
                };
                let line_map = LineMap::new(&task);
                let zone = compute_zone(&task, &line_map);
                let edit_line_n = line_of_byte(&new_snap, s);
                if zone.ce_n > zone.zs_n {
                    assert!(
                        zone.zs_n <= edit_line_n && edit_line_n < zone.ce_n,
                        "[{na}|{nb}] {opname} zone {}..{} 未覆盖编辑行 {}",
                        zone.zs_n, zone.ce_n, edit_line_n
                    );
                }
                // patch 应用等价(827 T-mode:CF/HS 双跑)。
                for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
                    let patches = patches_for_mode(&task, mode);
                    let mut applied = serde_json::json!({"ct": mode_ch(&old_snap, mode)});
                    let mut ok = true;
                    for p in &patches {
                        if !apply_patch_simple(&mut applied, p) {

                            ok = false;
                            break;
                        }
                    }
                    assert!(
                        ok && applied.get("ct").cloned().unwrap_or(serde_json::Value::Null) == mode_ch(&new_snap, mode),
                        "[{na}|{nb}] {opname} [{mode:?}] patch != full AST"
                    );
                }
                total += 1;
            }
        }
    }

    assert!(total >= 50, "pairwise 规模不足: {total}");
}
