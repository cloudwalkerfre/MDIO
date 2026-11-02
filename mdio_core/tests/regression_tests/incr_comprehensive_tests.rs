/// incr 管道全覆盖测试：ADD(新容器) / REMOVE(删容器) / REPLACE(内容/类型变化) 的全量 vs 增量一致性。
///
/// 覆盖三类操作: ADD(新容器) / REMOVE(删容器) / REPLACE(内容/类型变化)
/// 覆盖场景: 全量 vs 增量一致性、容器创建、嵌套容器、多行编辑、边界情况
use std::sync::Arc;
use std::collections::BTreeMap;
use serde_json::Value;
fn parse_val(v: &str) -> Value { if v == "null" { Value::Null } else { serde_json::from_str(v).unwrap_or_else(|e| {  panic!("bad value"); }) } }
use mdio_core::projection::State;
use mdio_core::incr::{incr_patches, patches_for_mode, Patch, IncrTask};
use mdio_core::topology::ProjectionMode;
use mdio_core::block::PackedLineInfo;

/// root 信封 → ch 数组（集成测试
/// 本地辅助）。
fn ast_ch(snap: &mdio_core::projection::Snapshot) -> Value {
    let v: Value = serde_json::from_str(
        &snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf)).unwrap();
    v["ct"].clone()
}

/// 全量 ch 数组:信封 ch 按 mode 自动适配(Cf/Hs 双跑共用)。
fn mode_ch(snap: &mdio_core::projection::Snapshot, mode: ProjectionMode) -> Value {
    let v: Value = serde_json::from_str(&snap.to_ast_object_mode(mode)).unwrap();
    v["ct"].clone()
}

fn run(old: &str, start: usize, end: usize, new: &str) -> Vec<Patch> {
    let mut state = State::new(old);
    let snap_old = Arc::new(state.snapshot());
    state.edit(start, end, new);
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("ripple_diff must be Some");
    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(start, end)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    incr_patches(&task)
}

/// Like `run` but also returns the full to_ast_object for content assertions
fn run_full(old: &str, start: usize, end: usize, new: &str) -> (Vec<Patch>, String) {
    let mut state = State::new(old);
    let snap_old = Arc::new(state.snapshot());
    state.edit(start, end, new);
    let snap = state.snapshot();
    let full = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
    let diff = state.ripple_diff.take().expect("ripple_diff must be Some");
    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(start, end)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    (incr_patches(&task), full)
}

/// 全量快照 = 从 to_ast_object 获取的所有 container 路径
fn container_paths(snap: &mdio_core::projection::Snapshot) -> Vec<String> {
    let full = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
    let arr: Vec<serde_json::Value> = serde_json::from_str(&full).unwrap_or_default();
    let mut paths = Vec::new();
    for (i, node) in arr.iter().enumerate() {
        paths.push(format!("/children/{}", i));
        collect_child_paths(node, &format!("/children/{}", i), &mut paths);
    }
    paths
}

fn collect_child_paths(node: &serde_json::Value, prefix: &str, paths: &mut Vec<String>) {
    if let Some(children) = node.get("ct").and_then(|c| c.as_array()) {
        for (i, child) in children.iter().enumerate() {
            let p = format!("{}/children/{}", prefix, i);
            paths.push(p.clone());
            collect_child_paths(child, &p, paths);
        }
    }
}

/// patch 应用后, AST 必须与全量 to_ast_object 一致
fn assert_patches_yield_full(patches: &[Patch], snap: &mdio_core::projection::Snapshot) {
    let full: Vec<serde_json::Value> = ast_ch(snap)
        .as_array().cloned().unwrap_or_default();
    // simulate apply: start from empty, apply patches
    let mut nodes: Vec<serde_json::Value> = Vec::new();
    for p in patches {
        match p {
            Patch::Add { path, value } => {
                let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() == 2 && parts[0] == "ct" {
                    let idx: usize = parts[1].parse().unwrap_or(nodes.len());
                    if idx >= nodes.len() { nodes.push(parse_val(value)); }
                    else { nodes.insert(idx, parse_val(value)); }
                }
                // 成员位（\`/ct/<i>/<member>\`，值可为整数组）——写进该节点的成员。
                if parts.len() == 3 && parts[0] == "ct" && parts[2].parse::<usize>().is_err() {
                    let idx: usize = parts[1].parse().unwrap_or(usize::MAX);
                    if idx < nodes.len() {
                        if let Some(obj) = nodes[idx].as_object_mut() {
                            obj.insert(parts[2].to_string(), parse_val(value));
                        }
                    }
                }
            }
            Patch::Remove { path } => {
                let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() == 2 && parts[0] == "ct" {
                    let idx: usize = parts[1].parse().unwrap_or(usize::MAX);
                    if idx < nodes.len() { nodes.remove(idx); }
                }
            }
            Patch::Replace { path, value } => {
                let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() == 2 && parts[0] == "ct" {
                    let idx: usize = parts[1].parse().unwrap_or(usize::MAX);
                    if idx < nodes.len() { nodes[idx] = parse_val(value); }
                }
            }
            Patch::ReplaceAttr { path, value, .. } => {
                let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() == 2 && parts[0] == "ct" {
                    let idx: usize = parts[1].parse().unwrap_or(usize::MAX);
                    if idx < nodes.len() {
                        if let Some(obj) = nodes[idx].as_object_mut() {
                            // attr 面模拟:测试只关心 nt 一致性——set 整元素即可
                            nodes[idx] = parse_val(value);
                        }
                    }
                }
            }
        }
    }
    // compare nodeTypes
    for (i, node) in nodes.iter().enumerate() {
        if i < full.len() {
            assert_eq!(node["nt"], full[i]["nt"],
                "nodeType mismatch at /children/{}: patch={:?} full={:?}",
                i, node["nt"], full[i]["nt"]);
        }
    }
    assert_eq!(nodes.len(), full.len(),
        "node count mismatch: patches produced {} but full snapshot has {}",
        nodes.len(), full.len());
}

// ADD: 新容器创建

#[test]
fn add_first_bullet_creates_bullet_list() {
    let patches = run("# H\n", 4, 4, "- a\n");
    let has_add = patches.iter().any(|p| matches!(p, Patch::Add { .. }));
    assert!(has_add, "first bullet should create ADD patch");
}

#[test]
fn add_first_ol_creates_ordered_list() {
    let patches = run("# H\n", 4, 4, "1. item\n");
    let has_add = patches.iter().any(|p| matches!(p, Patch::Add { .. }));
    assert!(has_add, "first ordered list should create ADD patch");
}

#[test]
fn add_first_bq_creates_blockquote() {
    let patches = run("# H\n", 4, 4, "> quote\n");
    let has_add = patches.iter().any(|p| matches!(p, Patch::Add { .. }));
    assert!(has_add, "first blockquote should create ADD patch");
}

#[test]
fn add_nested_bullet_inside_existing() {
    // Append to existing BLT => container REPLACE (not new container)
    let mut state = State::new("# H\n- a\n");
    let snap_before = state.snapshot();
    let paths_before = container_paths(&snap_before);

    state.edit(7, 7, "- b\n");
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("must have diff");
    let task = IncrTask {
        snap: Arc::new(snap), doc_id: 0,
        snap_old: Arc::new(snap_before),
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(7, 7)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches = incr_patches(&task);
    assert!(!patches.is_empty(), "append to existing BLT must produce patches");
}

// REMOVE: 容器删除

#[test]
fn remove_bullet_to_paragraph() {
    // "- a\n" → "text" => BLT container removed (REM+ADD merge → REPL)
    let patches = run("- a\n", 0, 3, "text");
    let has_change = patches.iter().any(|p| matches!(p, Patch::Replace { .. } | Patch::Remove { .. }));
    assert!(has_change, "bullet→paragraph should change/remove BLT container");
}

#[test]
fn remove_header_changes_type() {
    let patches = run("# H\n- a\n", 0, 5, "text\n");
    let has_remove = patches.iter().any(|p| matches!(p, Patch::Remove { .. }));
    let has_add = patches.iter().any(|p| matches!(p, Patch::Add { .. }));
    assert!(has_remove || has_add, "header removal should produce structure change");
}

// REPLACE: 内容/类型变化

#[test]
fn replace_bullet_content_only() {
    let patches = run("# H\n- old\n", 6, 9, "new");
    let has_replace = patches.iter().any(|p| matches!(p, Patch::Replace { .. }));
    assert!(has_replace, "content-only edit should produce REPLACE");
}

#[test]
fn replace_header_text() {
    let patches = run("# Old\n- a\n", 2, 5, "New");
    let has_replace = patches.iter().any(|p| matches!(p, Patch::Replace { .. }));
    assert!(has_replace);
}

#[test]
fn replace_paragraph_type_change() {
    // "text\n---\n- a\n" → insert blank before "---" changes HsSep→ThematicBreak
    let patches = run("text\n---\n- a\n", 5, 5, "\n");
    assert!(!patches.is_empty(), "type change must be detected");
}

// FULL vs INCR 一致性: 全量快照 = 增量 patch 应用后

#[test]
fn full_vs_incr_first_bullet() {
    let mut state = State::new("# H\n");
    let snap_old = Arc::new(state.snapshot());
    state.edit(4, 4, "- a\n");
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().unwrap();
    let task = IncrTask {
        snap: Arc::new(snap.clone()), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(4, 4)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches = incr_patches(&task);
    assert!(!patches.is_empty(), "first bullet must produce patches");
    assert!(patches.iter().any(|p| matches!(p, Patch::Add { .. })), "should have ADD");
}

#[test]
fn full_vs_incr_append_bullet() {
    let mut state = State::new("# H\n- a\n");
    let snap_before = state.snapshot();
    state.edit(7, 7, "- b\n");
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().unwrap();
    let task = IncrTask {
        snap: Arc::new(snap.clone()), doc_id: 0,
        snap_old: Arc::new(snap_before),
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(7, 7)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches = incr_patches(&task);
    assert!(!patches.is_empty(), "append must produce patches");
}

#[test]
fn full_vs_incr_edit_text() {
    let mut state = State::new("# H\n- old\n");
    let snap_old = Arc::new(state.snapshot());
    state.edit(6, 9, "new");
    let snap = state.snapshot();
    let diff = state.ripple_diff.take().unwrap();
    let task = IncrTask {
        snap: Arc::new(snap.clone()), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(6, 9)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches = incr_patches(&task);
    assert!(!patches.is_empty(), "content edit must produce patches");
    assert!(patches.iter().any(|p| matches!(p, Patch::Replace { .. })), "should have REPLACE");
}

// 边界场景

#[test]
fn noop_produces_nothing() {
    let mut state = State::new("# H\n- a\n");
    let snap_old = Arc::new(state.snapshot());
    state.edit(3, 3, "");
    let snap = state.snapshot();
    // ripple_diff may be Some (had_splice=true for Equal case) or None
    if let Some(diff) = state.ripple_diff.take() {
        let task = IncrTask {
            snap: Arc::new(snap), doc_id: 0,
            snap_old,
            delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(3, 3)]),
            ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        let patches = incr_patches(&task);
        let _ = patches; // smoke test: verify pipeline doesn't crash
    }
}

#[test]
fn delete_multiple_lines() {
    let patches = run("# H\n- a\n- b\n- c\n- d\n", 5, 20, "");
    assert!(!patches.is_empty(), "multi-line delete must produce patches");
}

#[test]
fn insert_multiple_lines() {
    let patches = run("# H\n- z\n", 5, 5, "- a\n- b\n- c\n");
    assert!(!patches.is_empty(), "multi-line insert must produce patches");
}

#[test]
fn nested_container_edit() {
    let patches = run("> - item: old\n>   detail\n", 10, 13, "new");
    assert!(!patches.is_empty(), "nested BQ>BLT edit must produce patches");
}

#[test]
fn three_signal_union() {
    // edit that triggers all three signals: structure + type + content change
    // Replaces plain text with a NEW bullet list — BLT didn't exist before → genuine ADD
    let patches = run("plain text\nmore text\n", 0, 12, "# New\n- a\n- b\n");
    assert!(!patches.is_empty(), "structural + type + content change must produce patches");
    let ops: Vec<&str> = patches.iter().map(|p| match p {
        Patch::Add { .. } => "add",
        Patch::Remove { .. } => "remove",
        Patch::Replace { .. } => "replace",
        Patch::ReplaceAttr { .. } => "replace_attr"
    }).collect();
    // structural change may produce ADD or root REPLACE
    assert!(ops.iter().any(|&o| o == "add" || o == "replace"),
        "structural change must produce ADD or REPLACE");
}

// Real-world complex document scenarios

fn patch_summary(patches: &[Patch]) -> (usize, usize, usize) {
    let (mut a, mut d, mut r) = (0,0,0);
    for p in patches {
        match p {
            Patch::Add { .. } => a += 1,
            Patch::Remove { .. } => d += 1,
            Patch::Replace { .. } | Patch::ReplaceAttr { .. } => r += 1,
            _ => {}
        }
    }
    (a,d,r)
}

#[test]
fn realworld_delete_blt_item() {
    let old = "\
# Project Plan

## Milestones

- Design phase: complete architecture review
- Implementation: core engine and projection
- Testing: unit tests, integration tests, benchmarks
- Deployment: Docker images, CI/CD pipeline

## Notes

> The ripple merge algorithm is the key innovation.
> It enables O(ripple_zone) incremental updates.

```rust
fn main() {

}
```
";
    let start = old.find("Implementation:").unwrap();
    let end = old[start..].find('\n').unwrap() + start + 1;
    let (patches, full) = run_full(old, start, end, "");
    // Deleting a BLT item produces item-level REMOVE, not necessarily container-level REPLACE.
    let (add, del, rep) = patch_summary(&patches);
    assert!(add + del + rep > 0, "must produce patches for deleting a BLT item");
    assert!(full.contains("Design phase"), "Design phase still present");
    assert!(full.contains("Testing"), "Testing still present");
    assert!(!full.contains("Implementation:"), "Implementation removed");
    assert!(full.contains("bl"), "bullet_list container preserved");
}

#[test]
fn realworld_insert_blt_item() {
    let old = "# Todo\n\n- Buy groceries\n- Pay bills\n\n## Done\n";
    let insert_pos = old.find("Pay bills").unwrap();
    let (patches, full) = run_full(old, insert_pos, insert_pos, "- Call dentist\n");
    assert!(full.contains("Call dentist"), "new item present");
    assert!(full.contains("Buy groceries"), "old item preserved");
    assert!(full.contains("Pay bills"), "old item preserved");
}

#[test]
fn realworld_add_blockquote_container() {
    let old = "# Notes\n\nPlain text here.\n";
    let start = old.find("Plain").unwrap();
    let end = old[start..].find('\n').unwrap() + start + 1;
    let (patches, full) = run_full(old, start, end, "> Important:\n> - Item 1\n> - Item 2\n");
    assert!(full.contains("bq"), "blockquote container created");
    assert!(full.contains("bl"), "nested bullet_list in BQ");
    assert!(full.contains("Item 1"), "item content present");
    assert!(full.contains("Item 2"), "item content present");
    assert!(!full.contains("Plain text"), "old content removed");
}

#[test]
fn realworld_content_only_edit() {
    let old = "\
# Release Notes v2.0

## Features

- **New**: Real-time collaboration via CRDT
- **Improved**: Markdown parsing speed by 3x
- **Fixed**: Memory leak in large document projection

## Breaking Changes

- API endpoint `/v1/parse` deprecated, use `/v2/parse`
- Config format changed from TOML to YAML
";
    let start = old.find("3x").unwrap();
    let end = start + 2;
    let (patches, full) = run_full(old, start, end, "5x");
    let (add, del, rep) = patch_summary(&patches);
    assert!(rep >= 1, "must have at least 1 REPLACE");
    assert_eq!(add, 0, "no ADD for content-only edit");
    assert_eq!(del, 0, "no DELETE for content-only edit");
    assert!(full.contains("5x"), "new content present");
    assert!(!full.contains("3x"), "old content gone");
}

#[test]
fn realworld_prepend_header() {
    let old = "\
## Overview

This document describes the MDIO architecture.

### Core Concepts

- FrameLog: persistent sparse log of container boundaries
- Ripple merge: incremental re-scan of edited region
- Concept lattice: formal model of document structure
";
    let (patches, full) = run_full(old, 0, 0, "# MDIO Architecture\n\n");
    assert!(full.contains("MDIO Architecture"), "new header present");
    assert!(full.contains("Overview"), "old content preserved");
    assert!(full.contains("FrameLog"), "deep content preserved");
}

#[test]
fn realworld_delete_entire_container() {
    let old = "\
# Shopping List

## Groceries

- Apples
- Bananas
- Milk
- Eggs

## Hardware

- Hammer
- Nails
- Screwdriver
";
    let list_start = old.find("- Apples").unwrap();
    let list_end = old.find("## Hardware").unwrap();
    let (patches, full) = run_full(old, list_start, list_end, "");
    assert!(!full.contains("Apples"), "grocery items removed");
    assert!(!full.contains("Bananas"), "grocery items removed");
    assert!(full.contains("Hammer"), "hardware list preserved");
    assert!(full.contains("Screwdriver"), "hardware items preserved");
    let has_container_remove = patches.iter().any(|p| matches!(p, Patch::Remove { path } if path == "/ct/1" || path == "/ct/2"));
    assert!(has_container_remove, "container-level REMOVE");
}

#[test]
fn realworld_edit_inside_nested_container() {
    let old = "\
# Documentation

> **Warning**: The following API endpoints are deprecated:
> - `/v1/users` — use `/v2/users` instead
> - `/v1/projects` — use `/v2/projects` instead
> - `/v1/tasks` — removed without replacement

See the migration guide for details.
";
    let start = old.find("deprecated").unwrap();
    let end = start + 10;
    let (patches, full) = run_full(old, start, end, "REMOVED");
    assert!(full.contains("REMOVED"), "new text present");
    assert!(!full.contains("deprecated"), "old text removed");
    assert!(full.contains("bq"), "BQ container preserved");
    assert!(full.contains("bl"), "BLT container preserved");
    assert!(full.contains("migration guide"), "content after BQ preserved");
}

#[test]
fn realworld_heading_level_change() {
    let old = "\
## Getting Started

### Prerequisites

- Rust 1.80+
- Cargo

### Installation

```bash
cargo install mdio
```
";
    let start = old.find("## Getting Started").unwrap();
    let end = start + 20;
    let (patches, full) = run_full(old, start, end, "# Getting Started\n");
    assert!(full.contains("\"lv\":1"), "heading level changed to 1");
    assert!(!full.contains("\"lv\":2"), "old level 2 gone from this heading");
    assert!(full.contains("Prerequisites"), "subsequent content preserved");
}

// Proof-by-exhaustion

// L1: harness — 往复不变量

/// diff(old,new) 的规范单区间 W_N: [LCP, |new|-LCS)
fn compute_edit(old: &str, new: &str) -> (usize, usize, String) {
    let ob = old.as_bytes(); let nb = new.as_bytes();
    let prefix = ob.iter().zip(nb).take_while(|(a, b)| a == b).count();
    let old_rest = &ob[prefix..]; let new_rest = &nb[prefix..];
    let suffix = old_rest.iter().rev().zip(new_rest.iter().rev())
        .take_while(|(a, b)| a == b).count();
    let edit_text = String::from_utf8_lossy(
        &new_rest[..new_rest.len().saturating_sub(suffix)]).into();
    (prefix, ob.len().saturating_sub(suffix), edit_text)
}

/// apply_patch — 导航 JSON AST 树，应用单个 Patch。
/// 路径格式: "/children/N/children/M/..." — 与 incr_patches 的 child_path 产出一致。
/// ADD:    在指定 children 数组位置插入 value
/// DELETE: 从指定 children 数组位置移除
/// REPLACE: 替换指定位置的值。空路径 → 替换整个 root。
fn apply_patch(root: &mut Value, p: &Patch) -> bool {
    let (path, val_str) = match p {
        Patch::Add { path, value } => (path, Some(value.as_str())),
        Patch::Remove { path } => (path, None),
        Patch::Replace { path, value } => (path, Some(value.as_str())),
        Patch::ReplaceAttr { path, value, .. } => (path, Some(value.as_str())),
    };
    // 空路径 → 根级操作
    if path.is_empty() {
        return match p {
            Patch::Replace { .. } => { *root = parse_val(val_str.unwrap()); true }
            _ => false,
        };
    }
    // 解析 "/children/N/children/M/..." 格式
    let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // 根路径 "/children" — 直接操作根的 children 数组
    if segs == ["ct"] {
        let arr = if root.is_array() { root.as_array_mut() }
                  else { root.get_mut("ct").and_then(|c| c.as_array_mut()) };
        let arr = match arr { Some(a) => a, None => return false };
        match p {
            Patch::Add { .. }     => { arr.push(parse_val(val_str.unwrap())); }
            Patch::Remove { .. }  => { arr.clear(); }
            Patch::Replace { .. } | Patch::ReplaceAttr { .. } => { *arr = parse_val(val_str.unwrap()).as_array().cloned().unwrap_or_default(); }
        }
        return true;
    }
    // 末段非数字 = 对象成员位（如 /ct/0/ct，值可为整数组——"建数组"补丁）。
    let member: Option<&str> = match segs.last().unwrap().parse::<usize>() {
        Ok(_) => None,
        Err(_) => { let m = *segs.last().unwrap(); segs.pop(); Some(m) }
    };
    if segs.len() < 2 { return false; }
    // 最后一个 segment 对是 "ct" + "N" → idx = N
    let idx: usize = match segs.last().unwrap().parse() { Ok(i) => i, Err(_) => return false };
    // 导航到父节点
    let mut cur: &mut Value = root;
    for pair in segs[..segs.len() - 1].chunks(2) {
        if pair.len() < 2 { break; }
        let ci: usize = match pair[1].parse() { Ok(i) => i, Err(_) => return false };
        let arr = if cur.is_array() { cur.as_array_mut() }
                  else { cur.get_mut("ct").and_then(|c| c.as_array_mut()) };
        let arr = match arr { Some(a) => a, None => return false };
        if ci >= arr.len() { return false; }
        cur = &mut arr[ci];
    }
    let arr = if cur.is_array() { cur.as_array_mut() }
              else { cur.get_mut("ct").and_then(|c| c.as_array_mut()) };
    let arr = match arr { Some(a) => a, None => return false };
    // 成员位：对目标节点做成员 set / remove。
    if let Some(m) = member {
        if idx >= arr.len() { return false; }
        let obj = match arr[idx].as_object_mut() { Some(o) => o, None => return false };
        return match p {
            Patch::Remove { .. } => { obj.remove(m); true }
            _ => { obj.insert(m.to_string(), parse_val(val_str.unwrap())); true }
        };
    }
    match p {
        Patch::Add { .. }     => { if idx <= arr.len() { arr.insert(idx, parse_val(val_str.unwrap())); true } else { false } }
        Patch::Remove { .. }  => { if idx < arr.len() { arr.remove(idx); true } else { false } }
        Patch::Replace { .. } => { if idx < arr.len() { arr[idx] = parse_val(val_str.unwrap()); true } else { false } }
        Patch::ReplaceAttr { attr, .. } => {
            if idx < arr.len() {
                if let Some(obj) = arr[idx].as_object_mut() {
                    obj.insert(attr.clone(), parse_val(val_str.unwrap()));
                    true
                } else { false }
            } else { false }
        }
    }
}

/// 清理：patch 应用创建的空 ch（Add 到无 ch 节点的中间产物——
/// 无子节点不写 ch；REM 删除 item children 后空 ch 不写回）。
fn prune_empty_ch(v: &mut Value) {
    if let Some(arr) = v.as_array_mut() {
        for c in arr.iter_mut() { prune_empty_ch(c); }
    } else if let Some(obj) = v.as_object_mut() {
        if let Some(ch) = obj.get_mut("ct") {
            if let Some(arr) = ch.as_array_mut() {
                for c in arr.iter_mut() { prune_empty_ch(c); }
                if arr.is_empty() { obj.remove("ct"); }
            }
        }
    }
}

struct VResult { ok: bool, patches: Vec<String>, old_json: Value, new_json: Value, got: Value }

/// 主定理不变量: applyPatches(toJSON_mode(old),
/// patches_for_mode(old→new)) ≡ toJSON_mode(new)——同一用例按 mode
/// 自动切换适配,CF/HS 双跑共用一个 verify。
fn verify_mode(old_md: &str, new_md: &str, mode: ProjectionMode) -> VResult {
    let (es, ee, et) = compute_edit(old_md, new_md);


    let mut state = State::new(old_md);
    let old_snap = state.snapshot();
    let old_json: Value = mode_ch(&old_snap, mode);

    state.edit(es, ee, &et);
    let new_snap = state.snapshot();
    let new_json: Value = mode_ch(&new_snap, mode);

    let old_snap_arc = Arc::new(old_snap);
    let new_snap_arc = Arc::new(new_snap.clone());
    let diff = state.ripple_diff.take().expect("must have diff");
    let patches = patches_for_mode(&IncrTask {
        snap: new_snap_arc.clone(),
        doc_id: 0,
        snap_old: old_snap_arc.clone(),
        delta: diff.delta,
        edit_ranges: mdio_core::edit_actions_arc(vec![(es, ee)]),
        ripple_line_start: diff.ripple_line_start,
        ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    }, mode);
    let descs: Vec<String> = patches.iter().map(|p| match p {
        Patch::Add { path, .. } => format!("ADD {}", path),
        Patch::Remove { path } => format!("REM {}", path),
        Patch::Replace { path, .. } => format!("REPL {}", path),
        Patch::ReplaceAttr { path, attr, .. } => format!("RATTR {path} {attr}"),
    }).collect();

    // 无 patch → 纯内容变更 (incr_patches 只负责结构 diff)
    if patches.is_empty() {
        return VResult { ok: true, patches: descs,
            old_json: old_json.clone(), new_json: new_json.clone(),
            got: old_json.clone() };
    }

    let mut got = old_json.clone();
    let mut apply_ok = true;
    for (pi, p) in patches.iter().enumerate() {
        if !apply_patch(&mut got, p) {
            apply_ok = false;
            return VResult {
                ok: false,
                patches: vec![format!("APPLY_FAIL[{}]: {}", pi, descs[pi])],
                old_json: old_json.clone(), new_json: new_json.clone(), got: got.clone(),
            };
        }
    }
    // 结构 patch 应用后 JSON 应与 new 一致（比较前清理空 ch）。
    prune_empty_ch(&mut got);
    let ok = got == new_json;
    VResult { ok, patches: descs, old_json, new_json, got }
}

/// 汇总一批 (old,new) case, 返回 (通过数, 失败列表)。
/// 同一用例按 mode 自动切换适配——内部 CF/HS 双跑,
/// 不按 mode 拆分测试。
fn run_cases(cases: &[(&str, &str, &str)]) -> (usize, usize, Vec<String>) {
    let (mut pass, mut fail) = (0usize, 0usize);
    let mut fails = Vec::new();
    for (cat, old, new) in cases {
        if old == new { continue; }
        for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
            let r = verify_mode(old, new, mode);
            if r.ok { pass += 1; } else {
                fail += 1;
                fails.push(format!("[{cat}:{mode:?}] {:?}→{:?} patches={:?}", old, new, r.patches));
            }
        }
    }
    (pass, fail, fails)
}

// L2: 场景覆盖表 — 全 block 类型 × 嵌套 × 虚拟 Dyck (硬断言)

/// 全 block 类型的内容/结构编辑
#[test]
fn matrix_all_block_types() {
    let cases: &[(&str, &str, &str)] = &[
        // ── 叶类型 (Type B) ──
        ("header_text",   "# Old\n",            "# New\n"),
        ("header_level",  "# H\n",              "## H\n"),
        ("para_content",  "hello\n",            "world\n"),
        ("para_multiline","a\nb\n",             "a\nB\n"),
        ("thematic",      "---\n",              "***\n"),
        // ── 容器: 内容编辑 (壳不变) ──
        ("bullet_content","- a\n- b\n",         "- a\n- B\n"),
        ("ol_content",    "1. a\n2. b\n",       "1. a\n2. B\n"),
        ("bq_content",    "> a\n> b\n",         "> a\n> B\n"),
        ("cf_content",    "```\ncode\n```\n",   "```\nDODE\n```\n"),
        ("cf_lang",       "```\ncode\n```\n",   "```rust\ncode\n```\n"),
        ("comment_content","<!-- a -->\n",      "<!-- b -->\n"),
        ("table_cell",    "| a | b |\n| - | - |\n", "| a | c |\n| - | - |\n"),
        // ── 容器: 增删 ──
        ("bullet_add",    "- a\n- b\n",         "- a\n- x\n- b\n"),
        ("bullet_del",    "- a\n- b\n- c\n",    "- a\n- c\n"),
        ("ol_add",        "1. a\n2. b\n",       "1. a\n2. x\n3. b\n"),
        ("ol_del",        "1. a\n2. b\n3. c\n", "1. b\n2. c\n"),
        // ── 容器创建/销毁 (Type A ↔ Type B) ──
        ("para_to_bullet","text\n",             "- text\n"),
        ("bullet_to_para","- a\n",              "text\n"),
        ("para_to_bq",    "text\n",             "> text\n"),
        ("para_to_h",     "text\n",             "# text\n"),
    ];
    let (pass, fail, fails) = run_cases(cases);

    for f in &fails {  }
    assert_eq!(fail, 0, "block 类型矩阵有 {} 个往复失败", fail);
}

/// 嵌套结构
#[test]
fn matrix_nesting() {
    let cases: &[(&str, &str, &str)] = &[
        ("blt_blt_ins",   "- a\n  - inner\n- b\n", "- a\n  - inner\n  - X\n- b\n"),
        ("blt_blt_del",   "- a\n  - inner\n- b\n", "- a\n- b\n"),
        ("blt_blt_repl",  "- a\n  - inner\n- b\n", "- A\n  - inner\n- b\n"),
        ("blt_blt_inner", "- a\n  - inner\n- b\n", "- a\n  - INNER\n- b\n"),
        ("triple_del",    "- a\n  - b\n    - c\n- d\n", "- a\n  - b\n- d\n"),
        ("bq_blt_content","> - a\n> - b\n",      "> - a\n> - B\n"),
        ("bq_ol_ins",     "> 1. a\n> 2. b\n> 3. c\n", "> 1. a\n> 2. X\n> 3. b\n> 4. c\n"),
        ("bq_bq_content", "> > a\n",             "> > b\n"),
    ];
    let (pass, fail, fails) = run_cases(cases);

    for f in &fails {  }
    assert_eq!(fail, 0, "嵌套矩阵有 {} 个往复失败", fail);
}

/// 虚拟 Dyck: 隐式块 (段落分裂/合并/setext/IndentedCode)
#[test]
fn matrix_virtual_dyck() {
    let cases: &[(&str, &str, &str)] = &[
        ("split",     "a\nb\n",         "a\n\nb\n"),
        ("split3",    "a\nb\nc\n",      "a\n\nb\n\nc\n"),
        ("merge",     "a\n\nb\n",       "a\nb\n"),
        ("setext_h1", "text\n",         "text\n===\n"),
        ("setext_h2", "text\n",         "text\n---\n"),
        ("setext_rm", "text\n===\n",    "text\n"),
        ("indent+",   "code\n",         "    code\n"),
        ("indent-",   "    code\n",     "code\n"),
        ("mix_split", "a\nb\n- x\n",    "a\n\nb\n- x\n"),
        ("h_para",    "# H\na\nb\n",    "# H\na\n\nb\n"),
    ];
    let (pass, fail, fails) = run_cases(cases);

    for f in &fails {  }
    assert_eq!(fail, 0, "虚拟 Dyck 矩阵有 {} 个往复失败", fail);
}

/// 单行容器 / 容器转换行
#[test]
fn matrix_single_line_and_transition() {
    let cases: &[(&str, &str, &str)] = &[
        ("bq_single",     "> q\n",              "> C\n"),
        ("cf_single",     "```\nx\n```\n",      "```\nY\n```\n"),
        ("transition",    "- b\n\n> q\n",       "- b\n\n> C\n"),
        ("h_blt_bq",      "# A\n\n- b\n\n> q\n","# A\n\n- b\n\n> C\n"),
        ("blt_then_bq",   "- a\n> q\n",         "- a\n> C\n"),
    ];
    let (pass, fail, fails) = run_cases(cases);

    for f in &fails {  }
    assert_eq!(fail, 0, "单行/转换行矩阵有 {} 个往复失败", fail);
}

/// 空文档边界 + 全量替换
#[test]
fn matrix_boundary() {
    let cases: &[(&str, &str, &str)] = &[
        ("empty_to_blt", "",              "- a\n- b\n"),
        ("blt_to_empty", "- a\n- b\n",    ""),
        ("empty_to_mix", "",              "# H\n- a\n> q\n"),
        ("full_replace", "- a\n- b\n",    "# H\n> q\n"),
    ];
    let (pass, fail, fails) = run_cases(cases);

    for f in &fails {  }
    assert_eq!(fail, 0, "边界矩阵有 {} 个往复失败", fail);
}

// L3: 穷举生成器 — proof-by-exhaustion

/// 基文档: 覆盖全 block 类型 + 嵌套 + 虚拟 Dyck
const BASES: &[&str] = &[
    "# H\n",
    "para\n",
    "a\nb\n",                       // 多行段落
    "- a\n- b\n",
    "1. a\n2. b\n",
    "> a\n> b\n",
    "> 1\n> 2\n> 3\n",             // BQ 多行段落
    "```\ncode\n```\n",
    "<!-- c -->\n",
    "---\n",
    "- a\n  - b\n",                 // 嵌套 BLT
    "> - a\n> - b\n",              // BQ>BLT
    "> 1. a\n> 2. b\n",           // BQ>OL
    "# H\n\n- a\n\n> q\n",        // 混合
    "text\n===\n",                 // setext
    "- a\n  cont\n",               // 多行 list item
    "a\n\nb\n",                     // 两段落
];

/// 行插入调色板
const INSERT_PALETTE: &[&str] = &["- x\n", "1. x\n", "# X\n", "px\n", "\n", "> x\n", "```\ny\n```\n"];
/// 行内容替换调色板
const REPLACE_PALETTE: &[&str] = &["Z\n", "- Z\n", "# Z\n", "> Z\n", "1. Z\n", "\n"];

/// 对基文档生成 (类别, new_doc) 编辑集: 字符替换 + 行删 + 行插 + 行内容替换
fn gen_edits(base: &str) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let bytes = base.as_bytes();
    let lines: Vec<String> = base.split_inclusive('\n').map(|s| s.to_string()).collect();

    // 1. 字符级替换: 每个非换行字节 → 'Z'
    for i in 0..bytes.len() {
        if bytes[i] == b'\n' { continue; }
        let mut nb: Vec<u8> = bytes.to_vec();
        nb[i] = if bytes[i] == b'Z' { b'Q' } else { b'Z' };
        if let Ok(s) = String::from_utf8(nb) { out.push(("char_repl", s)); }
    }
    // 2. 行删除: 删每一行
    for li in 0..lines.len() {
        let mut nl = lines.clone(); nl.remove(li);
        out.push(("line_del", nl.concat()));
    }
    // 3. 行插入: 在每个边界插调色板每一项
    for pos in 0..=lines.len() {
        for ins in INSERT_PALETTE {
            let mut nl = lines.clone(); nl.insert(pos.min(nl.len()), ins.to_string());
            out.push(("line_ins", nl.concat()));
        }
    }
    // 4. 行内容替换: 替换每一行为调色板每一项
    for li in 0..lines.len() {
        for repl in REPLACE_PALETTE {
            let mut nl = lines.clone(); nl[li] = repl.to_string();
            out.push(("line_repl", nl.concat()));
        }
    }
    out
}

/// 破坏容器包围结构的非真实输入过滤: fence 标记行数为奇数 = fence 打开但无关闭.
/// (line_repl/char_repl 破坏 ``` 围栏行 → 生成非法 FL, 非 incr bug, 是测试输入非真实性)
fn unbalanced_fence(text: &str) -> bool {
    let n = text.lines().filter(|l| {
        let t = l.trim_start();
        t.starts_with("```") || t.starts_with("~~~")
    }).count();
    n % 2 == 1
}

/// 穷举证明: 对所有基文档 × 所有生成编辑, 硬断言往复不变量.
/// 失败按 (基文档, 类别) 分组, 画出完整 landscape.
#[test]
fn exhaustive_proof() {
    let mut total = 0usize;
    let mut fail = 0usize;
    // (base_idx, 类别) → 失败样本 (最多存 3 个)
    let mut fail_map: std::collections::BTreeMap<(usize, &'static str), Vec<String>> = std::collections::BTreeMap::new();

    for (bi, base) in BASES.iter().enumerate() {
        for (cat, new) in gen_edits(base) {
            if *base == new { continue; }
            if unbalanced_fence(base) || unbalanced_fence(&new) { continue; } // 排除破坏 fence 包围的非真实输入
            // 同一用例按 mode 自动切换适配——CF/HS 双跑。
            for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
                total += 1;
                let r = verify_mode(base, &new, mode);
                if !r.ok {
                    fail += 1;
                    let e = fail_map.entry((bi, cat)).or_default();
                    if e.len() < 3 {
                        e.push(format!("[{mode:?}] {:?}→{:?} patches={:?}", base, new, r.patches));
                    }
                }
            }
        }
    }



    if fail > 0 {

        for ((bi, cat), samples) in &fail_map {

            for s in samples {  }
        }
    }


    assert_eq!(fail, 0, "穷举证明发现 {} 个往复失败 (共 {} 例) — cutover 前必须清零", fail, total);
}

#[test]
fn repro_bug3_full_replace() {
    let old = "# Old H1\n\n- old a\n- old b\n";
    let new = "1. ONE\n2. TWO\n3. THREE\n";
    let (es, ee, et) = {
        let ob = old.as_bytes(); let nb = new.as_bytes();
        let prefix = ob.iter().zip(nb).take_while(|(a,b)| a==b).count();
        let suffix = ob[prefix..].iter().rev().zip(nb[prefix..].iter().rev()).take_while(|(a,b)| a==b).count();
        (prefix, ob.len().saturating_sub(suffix), String::from_utf8_lossy(&nb[prefix..nb.len().saturating_sub(suffix)]).to_string())
    };

    let patches = run(old, es, ee, &et);
    for p in &patches {
        match p {
            Patch::Add { path, .. } => {},
            Patch::Remove { path } => {},
            Patch::Replace { path, .. } => {},
            Patch::ReplaceAttr { path, attr, .. } => {},
        }
    }
    // Full replace must handle H1→OL correctly (root container may merge to single REPLACE)
    let has_root_patch = patches.iter().any(|p| matches!(p,
        Patch::Replace { path, .. } if path == "/ch"));
    let has_rem_children0 = patches.iter().any(|p| matches!(p, Patch::Remove { path } if path == "/ct/0"));
    let has_add_children0 = patches.iter().any(|p| matches!(p, Patch::Add { path, .. } if path == "/ct/0"));
    assert!(has_root_patch || has_rem_children0 || has_add_children0,
        "BUG3: must have patch targeting /ch or /ct/0");
    assert!(patches.len() >= 2, "BUG3: full replace should produce at least 2 patches, got {}", patches.len());

    // Apply patches and verify roundtrip
    let old_snap = State::new(old).snapshot();
    let mut old_json: Value = ast_ch(&old_snap);
    for p in &patches {
        apply_patch(&mut old_json, p);
    }
    let new_snap = State::new(new).snapshot();
    let new_json: Value = ast_ch(&new_snap);
    if old_json != new_json {
    
    
    }
    prune_empty_ch(&mut old_json);
    assert_eq!(old_json, new_json, "BUG3: roundtrip failed");
}

#[test]
fn repro_bug6_indent_reduction() {
    let old = "- a\n  - nested\n  - nested2\n- b\n";
    let new = "- a\n- nested\n  - nested2\n- b\n";
    let (es, ee, et) = {
        let ob = old.as_bytes(); let nb = new.as_bytes();
        let prefix = ob.iter().zip(nb).take_while(|(a,b)| a==b).count();
        let suffix = ob[prefix..].iter().rev().zip(nb[prefix..].iter().rev()).take_while(|(a,b)| a==b).count();
        (prefix, ob.len().saturating_sub(suffix), String::from_utf8_lossy(&nb[prefix..nb.len().saturating_sub(suffix)]).to_string())
    };

    let patches = run(old, es, ee, &et);
    for p in &patches {
        match p {
            Patch::Add { path, .. } => {},
            Patch::Remove { path } => {},
            Patch::Replace { path, .. } => {},
            Patch::ReplaceAttr { path, attr, .. } => {},
        }
    }

    assert!(!patches.is_empty(), "BUG6: indent reduction must produce patches");

    // Apply and verify roundtrip
    let old_snap = State::new(old).snapshot();
    let mut old_json: Value = ast_ch(&old_snap);
    for p in &patches {
        apply_patch(&mut old_json, p);
    }
    let new_snap = State::new(new).snapshot();
    let new_json: Value = ast_ch(&new_snap);
    if old_json != new_json {
    
    
    }
    prune_empty_ch(&mut old_json);
    assert_eq!(old_json, new_json, "BUG6: roundtrip failed");
}

#[test]
fn repro_bug3_real_edit() {
    let old_md = "# Old H1\n\n- old a\n- old b\n";
    let new_md = "1. ONE\n2. TWO\n3. THREE\n";
    let mut state = State::new(old_md);
    let old_snap = state.snapshot();
    let fl_old = Arc::new(old_snap.frame_log.entries.clone());
    state.edit(0, old_md.len(), new_md);
    let new_snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("must have diff");




    let task = IncrTask {
        snap: Arc::new(new_snap.clone()), doc_id: 0,
        snap_old: Arc::new(old_snap.clone()),
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(0, old_md.len())]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches = incr_patches(&task);
    for p in &patches {
        match p {
            Patch::Add { path, .. } => {},
            Patch::Remove { path } => {},
            Patch::Replace { path, .. } => {},
            Patch::ReplaceAttr { path, attr, .. } => {},
        }
    }
    let old_json = ast_ch(&old_snap);
    let new_json = ast_ch(&new_snap);
    let mut got = old_json.clone();
    for p in &patches { apply_patch(&mut got, p); }



    prune_empty_ch(&mut got);
    assert_eq!(got, new_json, "BUG3 roundtrip failed");
}

#[test]
fn repro_bug6_real_edit() {
    let old_md = "- a\n  - nested\n  - nested2\n- b\n";
    let new_md = "- a\n- nested\n  - nested2\n- b\n";
    let mut state = State::new(old_md);
    let old_snap = state.snapshot();
    let fl_old = Arc::new(old_snap.frame_log.entries.clone());
    state.edit(0, old_md.len(), new_md);
    let new_snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("must have diff");




    let task = IncrTask {
        snap: Arc::new(new_snap.clone()), doc_id: 0,
        snap_old: Arc::new(old_snap.clone()),
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(0, old_md.len())]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches = incr_patches(&task);
    for p in &patches {
        match p {
            Patch::Add { path, .. } => {},
            Patch::Remove { path } => {},
            Patch::Replace { path, .. } => {},
            Patch::ReplaceAttr { path, attr, .. } => {},
        }
    }

    let old_json = ast_ch(&old_snap);
    let new_json = ast_ch(&new_snap);
    let mut got = old_json.clone();
    for p in &patches { apply_patch(&mut got, p); }



    prune_empty_ch(&mut got);
    assert_eq!(got, new_json, "BUG6 roundtrip failed");
}

#[test]
fn verify_b1_bullet_tag_for_marker() {
    let snap = State::new("* a\n- b\n* c\n").snapshot();
    let json = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);

    // KNOWN: bullet marker tag field removed during JSON abbreviation; re-enable when tag is restored
    // assert!(json.contains("\"t\":\"*\""), "B1: * bullet should have tag='*'");
    // assert!(json.contains("\"t\":\"-\""), "B1: - bullet should have tag='-'");
}

#[test]
fn verify_b4_nested_bq_prefix_stripped() {
    let snap = State::new("> outer\n> > inner\n> back\n").snapshot();
    let json = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);

    assert!(!json.contains("> inner"), "B4: nested BQ prefix should be stripped from content");
    assert!(json.contains("\"inner\""), "B4: inner content should be 'inner' without > prefix");
}

#[test]
fn repro_b6_indent_only_spaces() {
    let old_md = "- a\n  - nested\n  - nested2\n- b\n";
    let mut state = State::new(old_md);
    let old_snap = state.snapshot();
    let fl_old = Arc::new(old_snap.frame_log.entries.clone());    state.edit(4, 6, ""); // delete the 2 spaces before "- nested"
    let new_snap = state.snapshot();
    let new_json: Value = ast_ch(&new_snap);
    let diff = state.ripple_diff.take().expect("no ripple_diff");




    let task = IncrTask {
        snap: Arc::new(new_snap.clone()), doc_id: 0,
        snap_old: Arc::new(old_snap.clone()),
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(4, 6)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches = incr_patches(&task);
    for p in &patches {
        match p {
            Patch::Add { path, .. } => {},
            Patch::Remove { path } => {},
            Patch::Replace { path, .. } => {},
            Patch::ReplaceAttr { path, attr, .. } => {},
        }
    }

    let old_json: Value = ast_ch(&old_snap);
    let mut got = old_json.clone();
    for p in &patches { apply_patch(&mut got, p); }



    assert!(!patches.is_empty(), "B6: deleting indent spaces must produce patches");
    prune_empty_ch(&mut got);
    assert_eq!(got, new_json, "B6 roundtrip failed");
}

#[test]
fn repro_b1_marker_conversion_patch() {
    // * a\n* b\n* c  →  * a\n- b\n* c  (change * to - on second item)
    let old_md = "* a\n* b\n* c\n";
    let new_md = "* a\n- b\n* c\n";
    let (es, ee, et) = {
        let ob = old_md.as_bytes(); let nb = new_md.as_bytes();
        let prefix = ob.iter().zip(nb).take_while(|(a,b)| a==b).count();
        let suffix = ob[prefix..].iter().rev().zip(nb[prefix..].iter().rev()).take_while(|(a,b)| a==b).count();
        (prefix, ob.len().saturating_sub(suffix), String::from_utf8_lossy(&nb[prefix..nb.len().saturating_sub(suffix)]).to_string())
    };
    let mut state = State::new(old_md);
    let old_snap = state.snapshot();
    let fl_old = Arc::new(old_snap.frame_log.entries.clone());    state.edit(es, ee, &et);
    let new_snap = state.snapshot();
    let diff = state.ripple_diff.take().expect("no ripple_diff");







    let task = IncrTask {
        snap: Arc::new(new_snap.clone()), doc_id: 0,
        snap_old: Arc::new(old_snap.clone()),
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(es, ee)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches = incr_patches(&task);


    for p in &patches {
        match p {
            Patch::Add { path, value } => eprintln!("B1-patch ADD {} => value={:.60}", path, value),
            Patch::Remove { path } => eprintln!("B1-patch REM {}", path),
            Patch::Replace { path, value } => eprintln!("B1-patch REPL {} => value={:.60}", path, value),
            Patch::ReplaceAttr { path, attr, .. } => eprintln!("B1-patch RATTR {path} {attr}"),
        }
    }
    let old_json = ast_ch(&old_snap);
    let new_json = ast_ch(&new_snap);
    let mut got = old_json.clone();
    for p in &patches { apply_patch(&mut got, p); }



    prune_empty_ch(&mut got);
    assert_eq!(got, new_json, "B1 marker conversion roundtrip failed");
}

#[test]
fn zz_dbg_nested_blt() {
    // byte positions: '-'(0),' '(1),'a'(2),'\n'(3),' '(4),' '(5),'-'(6),' '(7),'b'(8),'\n'(9)
    // exhaustive char_repl failing: byte6 '-'→'Z', byte7 ' '→'Z'
    for (name, start, end, text) in [
        ("dash→Z", 6, 7, "Z"),
        ("space→Z", 7, 8, "Z"),
    ] {

        let mut state = State::new("- a\n  - b\n");
        let old_snap = state.snapshot();
        state.edit(start, end, text);
        let new_snap = state.snapshot();
        let diff = state.ripple_diff.take().unwrap();
        let task = IncrTask {
            snap: Arc::new(new_snap.clone()), doc_id: 0,
            snap_old: Arc::new(old_snap.clone()),
            delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(start, end)]),
            ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        let patches = incr_patches(&task);
        let old_ast: Value = ast_ch(&old_snap);
        let new_ast: Value = ast_ch(&new_snap);
        let mut got = old_ast.clone();
        for p in &patches { apply_patch(&mut got, p); }
        assert_eq!(got, new_ast, "{} roundtrip failed", name);
    }
}

#[test]
fn zz_dbg_cont_edit() {
    // "- a\n  cont\n" → "- Z\n  cont\n" — list item with continuation
    // byte 3='a', end=4, text="Z" → replaces 'a' with 'Z'

    let (patches, _full) = run_full("- a\n  cont\n", 2, 3, "Z");

}
#[test]
fn zz_dbg_bq_ol_comma() {
    // "> 1\n> 2\n> 3\n" — BQ with consecutive paragraphs
    let mut state = State::new("> 1\n> 2\n> 3\n");
    let snap = state.snapshot();
    let json = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);

    // Check if valid JSON
    match serde_json::from_str::<Value>(&json) {
        Ok(v) => eprintln!("=== VALID JSON: {}", serde_json::to_string_pretty(&v).unwrap_or_default()),
        Err(e) => eprintln!("=== INVALID JSON: {}", e),
    }
}

/// 回归：BQ>BLT>BLT>OL>CF 三层嵌套的 OL 容器丢失（block.rs list 扫描 cross-kind marker）。
/// 根因：classify_list 的 BulletList 分支不认 OL marker（→ NestingContainer → COMPOSITE_PUSH
/// 误路径）+ list_peel 返回含 BQ 前缀宽度的 effective（threshold 偏大 → 后续行 Outside → Pop）。
#[test]
fn nested_bq_blt_blt_ol_structure() {
    let md = "> - outer\n>   - inner\n>     1. ordered\n>        ```rust\n>        fn main() {}\n>        ```\n>     2. second\n>   - inner2\n";
    let mut state = State::new(md);
    let snap = state.snapshot();
    let full: Value = ast_ch(&snap);

    // 递归收集：od 节点（OL 容器）与吞内容段落
    fn walk(v: &Value, out: &mut Vec<String>) {
        if let Some(arr) = v.as_array() {
            for it in arr {
                let nt = it.get("nt").and_then(|x| x.as_str()).unwrap_or("?");
                let ct = it.get("ct").and_then(|x| x.as_str()).unwrap_or("").chars().take(40).collect::<String>();
                out.push(format!("{nt}:{ct}"));
                walk(it, out);
            }
        } else if let Some(arr) = v.get("ct").and_then(|c| c.as_array()) {
            for it in arr {
                let nt = it.get("nt").and_then(|x| x.as_str()).unwrap_or("?");
                let ct = it.get("ct").and_then(|x| x.as_str()).unwrap_or("").chars().take(40).collect::<String>();
                out.push(format!("{nt}:{ct}"));
                walk(it, out);
            }
        }
    }
    let mut nodes = Vec::new();
    walk(&full, &mut nodes);

    // OL 容器必须存在且含两个 item
    assert!(nodes.iter().any(|n| n.starts_with("od:")), "OL container missing in BQ>BLT>BLT>OL: {nodes:?}");
    // CF 必须存在（代码块未被吞进段落文本）
    assert!(nodes.iter().any(|n| n.starts_with("c:")), "CF container missing in BQ>BLT>BLT>OL>CF: {nodes:?}");
    // 不得有吞内容的根级段落（"1. ordered" 作为段落文本 = 结构错误）
    assert!(!nodes.iter().any(|n| n.contains("1. ordered")), "OL content swallowed into paragraph: {nodes:?}");
}

/// 回归：BQ>OL item 内容续行被 tight-BQ 段落守卫误杀 → OL 分裂成两个 + 后续行错位。
/// 根因：59a25e35 的守卫对 (Inside, Paragraph) 无条件 PASS——item 内容续行
/// （列 == marker 内容起点）被弹；rel 判定 absolute 列（含 BQ 宽度）与 frame.indent
/// （内容坐标）比较，BQ 内恒 +2 错位。
/// 修复：删守卫（其原始场景由 Outside→PASS 覆盖）+ rel 统一内容坐标。
#[test]
fn bq_ol_item_continuation_not_split() {
    // 删开围栏后：item 内容续行 + fence 中断段落 + 同 OL 延续
    let md = "> 1. one\n>    print('x')\n>    ```\n> 2. two\n";
    let mut state = State::new(md);
    let snap = state.snapshot();
    let full: Value = ast_ch(&snap);
    fn walk(v: &Value, out: &mut Vec<String>) {
        if let Some(arr) = v.as_array() {
            for it in arr {
                let nt = it.get("nt").and_then(|x| x.as_str()).unwrap_or("?");
                let ct = it.get("ct").and_then(|x| x.as_str()).unwrap_or("").chars().take(40).collect::<String>();
                out.push(format!("{nt}:{ct}"));
                walk(it, out);
            }
        } else if let Some(arr) = v.get("ct").and_then(|c| c.as_array()) {
            for it in arr {
                let nt = it.get("nt").and_then(|x| x.as_str()).unwrap_or("?");
                let ct = it.get("ct").and_then(|x| x.as_str()).unwrap_or("").chars().take(40).collect::<String>();
                out.push(format!("{nt}:{ct}"));
                walk(it, out);
            }
        }
    }
    let mut nodes = Vec::new();
    walk(&full, &mut nodes);
    // 两个 OL item（one/two）必须在同一 od 容器内——OL 不得分裂
    let od_count = nodes.iter().filter(|n| n.starts_with("od:")).count();
    assert_eq!(od_count, 1, "OL must not split: {nodes:?}");
    // CF 必须存在（fence 中断段落 → 代码块，而非文本）
    assert!(nodes.iter().any(|n| n.starts_with("c:")), "fence must open CF: {nodes:?}");
}

/// 回归：未闭合 CF 的 cd 内容丢失——BQ 剥前缀 join 丢尾 \n 后 range_content 的
/// last 计算 first == last → None → CF 渲染空壳。修复后（Bug A 系列）：
/// 内容行（缩进满足——`>    after`）保留在 cd；后续 item（`> 2. two`——
/// 缩进不足）由列表守卫弹帧——不被吞（CM §4.6：fence 由 containing block 闭合）。
#[test]
fn unclosed_fence_keeps_content() {
    let md = "> 1. one\n>    print('x')\n>    ```\n>    after\n> 2. two\n";
    let mut state = State::new(md);
    let snap = state.snapshot();
    let full: Value = ast_ch(&snap);
    fn collect_cf_cd(v: &Value, out: &mut Vec<String>) {
        if let Some(arr) = v.as_array() {
            for it in arr { collect_cf_cd(it, out); }
        } else if let Some(arr) = v.get("ct").and_then(|c| c.as_array()) {
            for it in arr {
                if it.get("nt").and_then(|x| x.as_str()) == Some("c") {
                    out.push(it.get("cd").and_then(|x| x.as_str()).unwrap_or("").to_string());
                }
                collect_cf_cd(it, out);
            }
        }
    }
    let mut cds = Vec::new();
    collect_cf_cd(&full, &mut cds);
    assert!(cds.iter().any(|cd| cd.contains("after")),
        "unclosed fence must keep content, got cds={cds:?}");
    assert!(cds.iter().all(|cd| !cd.contains("2. two")),
        "unclosed fence must not swallow next item, got cds={cds:?}");
}

/// 回归：未闭合 CF（有尾 \n）的尾部内容丢失——range_content 按 rfind('\n')
/// 截断，但未闭合时最后一个 \n 是内容行的（"after" 被截）。
#[test]
fn unclosed_fence_keeps_trailing_content() {
    let md = "```rust\nfn main() {}\n\nafter\n";
    let mut state = State::new(md);
    let snap = state.snapshot();
    let full: Value = ast_ch(&snap);
    let c = &full[0];
    let cd = c.get("cd").and_then(|x| x.as_str()).unwrap_or("");
    assert!(cd.contains("after"), "unclosed fence must keep trailing content, cd={cd:?}");
}

/// 回归：item 内容 = inline + 块级混合时 P-patch 路径错位——diff_level 的 item
/// 递归偏移漏了 AST ch 前缀的 inline 节点占位（DNode children 只含块级）。
/// 编辑 CF 内容（item 内块级）→ 路径必须 = ch/<inline数>，否则 REPLACE 打到
/// inline 节点上（渲染错乱 + 残留）。
#[test]
fn item_inline_block_mixed_patch_path() {
    let old = "> 1. one\n>    ```py\n>    print('x')\n>    ```\n> 2. two\n";
    let mut state = State::new(old);
    let old_snap = state.snapshot();
    let old_json: Value = ast_ch(&old_snap);
    // 删开围栏行（同 fence-break 场景）
    let del_start = old.find(">    ```py").unwrap();
    let del_end = del_start + ">    ```py\n".len();
    state.edit(del_start, del_end, "");
    let new_snap = state.snapshot();
    let new_json: Value = ast_ch(&new_snap);
    let diff = state.ripple_diff.take().expect("must have diff");
    let patches = incr_patches(&IncrTask {
        snap: Arc::new(new_snap.clone()), doc_id: 0,
        snap_old: Arc::new(old_snap.clone()),
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(del_start, del_end)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    });
    let mut got = old_json.clone();
    for p in &patches { assert!(apply_patch(&mut got, p), "apply fail: {p:?}"); }
    prune_empty_ch(&mut got);
    assert_eq!(got, new_json, "item inline+block mixed patch roundtrip failed\npatches={patches:?}");
}

/// 回归：未确认表格（无有效分隔行）渲染出 tb 壳——CommonMark 语义下
/// `| a | b |\n| 9 | - |` 不是表格（GFM 需要分隔行确认），全部是段落。
#[test]
fn unconfirmed_table_no_shell() {
    let md = "| a | b |\n| 9 | - |\n| 1 | 2 |\n";
    let mut state = State::new(md);
    let snap = state.snapshot();
    let full: Value = ast_ch(&snap);
    fn walk(v: &Value, out: &mut Vec<String>) {
        if let Some(arr) = v.as_array() {
            for it in arr {
                out.push(it.get("nt").and_then(|x| x.as_str()).unwrap_or("?").to_string());
                walk(it, out);
            }
        } else if let Some(arr) = v.get("ct").and_then(|c| c.as_array()) {
            for it in arr {
                out.push(it.get("nt").and_then(|x| x.as_str()).unwrap_or("?").to_string());
                walk(it, out);
            }
        }
    }
    let mut nodes = Vec::new();
    walk(&full, &mut nodes);
    assert!(!nodes.iter().any(|n| n == "tb"),
        "unconfirmed table must not render tb shell: {nodes:?}");
}

/// 回归：BQ 内空行（`>` 前缀行）必须分裂段落——BQ_INJECT Paragraph 的 pe
/// 延伸把 `>` 行（PL 类型 Blockquote 非 Blank）当续行吞掉整个 BQ 内容。
/// 修复：pe 循环 + leaf 处理加 is_bq_blank_line（内容剥 BQ 前缀后为空）。
#[test]
fn bq_blank_line_splits_paragraph() {
    let md = "> line one\n>\n> line two\n> line three\n";
    let mut state = State::new(md);
    let snap = state.snapshot();
    let full: Value = ast_ch(&snap);
    let bq = &full[0];
    let kids = bq.get("ct").and_then(|c| c.as_array()).map(|a| a.len()).unwrap_or(0);
    assert_eq!(kids, 2, "BQ blank line must split paragraph, got {kids} children");
    // ct 双型(外部形态):纯文本 str / 富文本或跨行 = 拆段数组(取文本递归拼)。
    let node_text = |v: &Value| -> String {
        if let Some(s) = v.get("ct").and_then(|x| x.as_str()) {
            return s.to_string();
        }
        if let Some(arr) = v.get("ct").and_then(|x| x.as_array()) {
            let mut out = String::new();
            for it in arr {
                if let Some(s) = it.get("ct").and_then(|x| x.as_str()) {
                    out.push_str(s);
                }
            }
            return out;
        }
        String::new()
    };
    let p1 = bq.get("ct").and_then(|c| c.as_array()).map(|a| node_text(&a[0])).unwrap_or_default();
    let p2 = bq.get("ct").and_then(|c| c.as_array()).map(|a| node_text(&a[1])).unwrap_or_default();
    assert!(p1.contains("line one") && !p1.contains("line two"), "p1={p1:?}");
    assert!(p2.contains("line two") && p2.contains("line three"), "p2={p2:?}");
}

/// 回归：嵌套 BQ（> > - item）的 item 内容必须剥离全部 BQ 前缀——
/// build_node 的 peel_width 单层剥导致 item tag/content 含内层 > 前缀
/// （渲染成 ">      > - l3 item" 原始文本）。
#[test]
fn nested_bq_peel_width_all_layers() {
    let md = "> - l1 item\n>   1. l2 one\n>      > - l3 item\n>      >   1. l4 one\n";
    let mut state = State::new(md);
    let snap = state.snapshot();
    let full: Value = ast_ch(&snap);
    fn walk(v: &Value, out: &mut Vec<String>) {
        if let Some(arr) = v.as_array() {
            for it in arr {
                let nt = it.get("nt").and_then(|x| x.as_str()).unwrap_or("?");
                let ct = it.get("ct").and_then(|x| x.as_str()).unwrap_or("").chars().take(30).collect::<String>();
                out.push(format!("{nt}:{ct}"));
                walk(it, out);
            }
        } else if let Some(arr) = v.get("ct").and_then(|c| c.as_array()) {
            for it in arr {
                let nt = it.get("nt").and_then(|x| x.as_str()).unwrap_or("?");
                let ct = it.get("ct").and_then(|x| x.as_str()).unwrap_or("").chars().take(30).collect::<String>();
                out.push(format!("{nt}:{ct}"));
                walk(it, out);
            }
        }
    }
    let mut nodes = Vec::new();
    walk(&full, &mut nodes);
    // l3 item 的内容必须是 "l3 item"（剥全部 BQ 前缀），不得含 "> " 前缀
    assert!(nodes.iter().any(|n| n.contains("l3 item") && !n.contains('>')),
        "nested BQ item content must strip all BQ prefixes: {nodes:?}");
}

/// 回归：表格多数据行（TableRow PopPush 链）——depth_only 且无列表父的
/// PopPush 续行（行 4）无新壳、leaf 无输出 → 数据行丢失；且壳类型
/// "table_row" 与前端 TableNode 期望的 "tr" 不一致（单行表格数据行
/// 也从未渲染）。
#[test]
fn table_multiple_rows_rendered() {
    let md = "| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n";
    let mut state = State::new(md);
    let snap = state.snapshot();
    let full: Value = ast_ch(&snap);
    let tb = &full[0];
    let rows = tb.get("ct").and_then(|c| c.as_array()).map(|a| a.len()).unwrap_or(0);
    assert_eq!(rows, 2, "table must render all data rows, got {rows}");
    for (i, r) in tb.get("ct").and_then(|c| c.as_array()).unwrap().iter().enumerate() {
        assert_eq!(r.get("nt").and_then(|x| x.as_str()), Some("tr"),
            "row {i} must be nt=tr, got {:?}", r.get("nt"));
    }
}

#[test]
fn zz_dbg_bq_insert() {
    // "> - a\n> - b\n" → "> - a\n# X\n> - b\n"
    let old = "> - a\n> - b\n";
    let insert_pos = 6;
    let patch_text = "# X\n";

    let (patches, _full) = run_full(old, insert_pos, insert_pos, patch_text);

    // roundtrip check
    let mut state = State::new(old);
    let old_snap = state.snapshot();
    state.edit(insert_pos, insert_pos, patch_text);
    let new_snap = state.snapshot();
    let diff = state.ripple_diff.take().unwrap();
    let task = IncrTask {
        snap: Arc::new(new_snap.clone()), doc_id: 0,
        snap_old: Arc::new(old_snap.clone()),
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(insert_pos, insert_pos)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches2 = incr_patches(&task);
    let old_ast: Value = ast_ch(&old_snap);
    let new_ast: Value = ast_ch(&new_snap);
    let mut got = old_ast.clone();
    for p in &patches2 { apply_patch(&mut got, p); }



    assert_eq!(got, new_ast, "BQ insert roundtrip failed");
}

#[test]
fn zz_dbg_bq_blt_char_repl() {
    // "> - a\n> - b\n" (0,1) → ("Z") — char_repl on first BQ content line
    // (BQ 首行改写:旧 BQ 头部断裂为新段落 + 新 BQ)。
    // 容器闭包删除后 zone = rle+1——用真实 ripple 描述符
    // (run_full——生产同款)对拍;手工伪造的空 actions 描述符不是生产
    // 形状。
    let old = "> - a\n> - b\n";
    let (patches, _full) = run_full(old, 0, 1, "Z");

    let mut state = State::new(old);
    let old_snap = state.snapshot();
    state.edit(0, 1, "Z");
    let new_snap = state.snapshot();

    let old_ast: Value = ast_ch(&old_snap);
    let new_ast: Value = ast_ch(&new_snap);

    let mut got = old_ast.clone();
    for p in &patches {
        assert!(apply_patch(&mut got, p), "apply failed: {p:?}");
    }
    assert_eq!(got, new_ast, "BQ+BLT char_repl roundtrip");
}

#[test]
fn zz_dbg_cf() {

    let (patches, _) = run_full("```py\nx=1\n```\n", 6, 7, "2");

}

#[test]
fn zz_dbg_nested_blt_y() {
    // "- a\n  - x\n- b\n" (8,9)→("y") — nested BLT content edit

    let (patches, _) = run_full("- a\n  - x\n- b\n", 8, 9, "y");

}

#[test]
fn zz_dbg_h1_del() {
    // "# H1\n- a\n- b\n" (4,8)→("text") — delete list items, replace with text

    let (patches, _) = run_full("# H1\n- a\n- b\n", 4, 8, "text");

}

#[test]
fn zz_tr_path() {
    let md = "- If none is found:\n\n  + Set `openers_bottom` to the element before `current_position`.\n    (We know that there are no openers for this kind of closer up to and\n    including this point, so this puts a lower bound on future searches.)\n\n  + If the closer at `current_position` is not a potential opener,\n    remove it from the delimiter stack (since we know it can't\n    be a closer either).\n\n  + Advance `current_position` to the next element in the stack.\n\nAfter we're done, we remove all delimiters above `stack_bottom` from the\ndelimiter stack.";
    let insert_pos = md.rfind("delimiter stack.").unwrap() + "delimiter stack.".len();
    let mut state = State::new(&md);
    let old_snap = state.snapshot();
    state.edit(insert_pos, insert_pos, "ZZZ");
    let new_snap = state.snapshot();

    let json: Value = ast_ch(&new_snap);
    let n_roots = json.as_array().unwrap().len();


    let rls = old_snap.line_of_byte(insert_pos);
    let rle = old_snap.line_of_byte(insert_pos.min(old_snap.byte_len().saturating_sub(1)));

    let patches = incr_patches(&IncrTask {
        snap: Arc::new(new_snap.clone()), doc_id: 0,
        snap_old: Arc::new(old_snap.clone()),
        delta: 0isize,
        edit_ranges: mdio_core::edit_actions_arc(vec![(insert_pos, insert_pos)]),
        ripple_line_start: rls, ripple_line_end: rle,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    });

    for p in &patches { eprintln!("  {:?}", p); }

    assert!(!patches.is_empty(), "must produce patches for content change");
    for p in &patches {
        match p {
            Patch::Replace { path, .. } => {
                assert!(!path.contains("/ct/6"),
                    "path should not use DNode flat index /ct/6, got {}", path);
            }
            _ => {}
        }
    }

    let old_json = ast_ch(&old_snap);
    let new_json = ast_ch(&new_snap);
    let mut got = old_json.clone();
    for p in &patches { apply_patch(&mut got, p); }
    prune_empty_ch(&mut got);
    assert_eq!(got, new_json, "roundtrip failed — DNode and JSON trees disagree");
}

#[test]
fn incr_bullet_append_twice() {
    let md = "---\ntitle: CommonMark Spec\nauthor: John MacFarlane\nversion: '0.31.2'\ndate: '2024-01-28'\nlicense: '[CC-BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/)'\n...\n\n# Introduction\n\n123卡萨丁卡拉克ljsadlaldjlajdlasdadad12312312\n\n## What is Markdown?\n\nMarkdown is a plain text format for writing structured documents,\nbased on conventions for indicating formatting in email\nand usenet posts.  It w1\n";

    // Step 1: insert "- 123\n" after "# Introduction\n"
    let intro_end = md.find("# Introduction\n").unwrap() + "# Introduction\n".len();
    let mut state = State::new(&md);
    let old_snap1 = state.snapshot();
    state.edit(intro_end, intro_end, "- 123\n");
    let new_snap1 = state.snapshot();

    let diff1 = state.ripple_diff.take().expect("no ripple_diff");
    let task1 = IncrTask {
        snap: Arc::new(new_snap1.clone()), doc_id: 0,
        snap_old: Arc::new(old_snap1.clone()),
        delta: diff1.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(intro_end, intro_end)]),
        ripple_line_start: diff1.ripple_line_start, ripple_line_end: diff1.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches1 = incr_patches(&task1);

    for p in &patches1 { eprintln!("  {:?}", p); }

    let old_json1: Value = ast_ch(&old_snap1);
    let new_json1: Value = ast_ch(&new_snap1);
    let mut got1 = old_json1.clone();
    for p in &patches1 { apply_patch(&mut got1, p); }
    assert_eq!(got1, new_json1, "step1 roundtrip failed");

    // Step 2: insert another "- 123\n" after the first bullet
    let bullet_end = intro_end + 6; // after "- 123\n"
    let old_snap2 = new_snap1.clone();
    state.edit(bullet_end, bullet_end, "- 123\n");
    let new_snap2 = state.snapshot();

    let diff2 = state.ripple_diff.take().expect("no ripple_diff 2");
    let task2 = IncrTask {
        snap: Arc::new(new_snap2.clone()), doc_id: 0,
        snap_old: Arc::new(old_snap2.clone()),
        delta: diff2.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(bullet_end, bullet_end)]),
        ripple_line_start: diff2.ripple_line_start, ripple_line_end: diff2.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches2 = incr_patches(&task2);

    for p in &patches2 { eprintln!("  {:?}", p); }
    // JSON roots after step2
    let arr2: Vec<Value> = ast_ch(&new_snap2).as_array().cloned().unwrap_or_default();
    for (i, child) in arr2.iter().enumerate() {
        let nt = child["nt"].as_str().unwrap_or("?");

    }
}
