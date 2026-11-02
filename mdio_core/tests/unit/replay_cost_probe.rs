// Replay 成本探针 —— v2(shadow anchor 增量化)+ v3(cached_stack 跨事务条件存活)
// 实现后的台账验证。硬证据 = 折叠计数(cfg(test) 计数器),软证据 = 墙钟。
//
// 断言语义(实现后):
//   ② shadow 折叠 == (anchor, conv_old] 条目数(zone 级——v2 消掉前缀部分);
//   ① cached_rebuild 折叠(两调用累计):冷启 == 0→start_line(≈全前缀);
//      温存(前一事务缓存位置 ≤ 本 ripple start)== [cached_line, start) 增量(≈zone);
//   对照列(由旧 entries 直接计算——非内部读取):今日账 = 0→conv_old + 0→start_line。
//   回退雪崩语料(列表)anchor=0 → v2 无操作(shadow 折叠不变)——边界记录。

use super::*;
use crate::block::entry_line;
use std::time::Instant;

fn entries_before(entries: &[u64], line: usize) -> usize {
    entries.partition_point(|e| entry_line(*e) < line as u32)
}
fn entries_through(entries: &[u64], line: usize) -> usize {
    entries.partition_point(|e| entry_line(*e) <= line as u32)
}

/// fence 块 + 空行:每块 4 行、2 条目(push 开 / pop 关);关闭行 shape=false 阻断回退。
fn fence_doc(n_blocks: usize) -> String {
    let mut s = String::with_capacity(n_blocks * 20);
    for i in 0..n_blocks {
        s.push_str("```\ncode");
        s.push_str(&i.to_string());
        s.push_str("\n```\n\n");
    }
    s
}

/// 长列表:每行一 item(约 2 条目/行);开行边界链触发全程回退。
fn list_doc(n_items: usize) -> String {
    let mut s = String::with_capacity(n_items * 12);
    for i in 0..n_items {
        s.push_str("- item");
        s.push_str(&i.to_string());
        s.push('\n');
    }
    s
}

struct Ledger {
    e_total: usize,
    rls: usize,
    delta: isize,
    anchor: usize,
    conv_old: usize,
    shadow_folds: usize,      // 实测(cfg test 计数器)
    cached_folds: usize,      // 实测(last_cached_replay_folds——本 ripple 两调用累计)
    cf_shadow_old: usize,     // 对照:若无 v2,shadow = 0→conv_old
    cf_cached_old: usize,     // 对照:若无 v3 温存,cached 冷启 = 0→start_line
    shadow_zone: usize,       // v2 语义期望 = (anchor, conv_old]
    ripple_wall: std::time::Duration,
}

impl Ledger {
    fn total_old(&self) -> usize { self.cf_shadow_old + self.cf_cached_old }
    fn total_now(&self) -> usize { self.shadow_folds + self.cached_folds }
}

fn measure(st: &mut State, old_entries: &[u64], n_old: usize, edit: impl FnOnce(&mut State)) -> Ledger {
    let e_total = old_entries.len();
    edit(st);
    let t0 = Instant::now();
    let _ = st.snapshot();
    let ripple_wall = t0.elapsed();
    let diff = st.ripple_diff.take().expect("ripple diff");
    let (rls, rle, delta) = (diff.ripple_line_start, diff.ripple_line_end, diff.delta);
    let anchor = ((rls as isize - delta).max(0) as usize).min(n_old.saturating_sub(1));
    let conv_old = ((rle as isize - delta).max(0) as usize).min(n_old.saturating_sub(1));
    let cf_shadow_old = entries_through(old_entries, conv_old);
    Ledger {
        e_total,
        rls,
        delta,
        anchor,
        conv_old,
        shadow_folds: st.last_ripple_folds,
        cached_folds: st.last_cached_replay_folds,
        cf_shadow_old,
        cf_cached_old: entries_before(old_entries, rls),
        shadow_zone: cf_shadow_old - entries_before(old_entries, anchor),
        ripple_wall,
    }
}

fn print_ledger(name: &str, l: &Ledger) {
    eprintln!("── {name} ──");
    eprintln!(
        "  E(old)={} rls={} delta={} anchor={} conv_old={} ripple_wall={:?}",
        l.e_total, l.rls, l.delta, l.anchor, l.conv_old, l.ripple_wall
    );
    eprintln!(
        "  ②shadow: 实测={} | v2期望(anchor,conv]={} | 对照(无v2)={}",
        l.shadow_folds, l.shadow_zone, l.cf_shadow_old
    );
    eprintln!(
        "  ①cached_rebuild: 实测={} | 对照(冷启 0→start_line)={}",
        l.cached_folds, l.cf_cached_old
    );
    eprintln!(
        "  总量: 实测={} | 对照(昨天)={}",
        l.total_now(), l.total_old()
    );
}

/// A. fence 尾部追加(冷启——首个 ripple):② zone 级;① 仍全前缀(冷)。
/// 总量 ≈E(昨天 2E)——v2 单独即减半;v3 收益体现在温存(场景 D)。
#[test]
fn ledger_fence_tail_append() {
    let doc = fence_doc(25_000);
    let mut st = State::new(&doc);
    let old_entries = st.frame_log.entries.clone();
    let n_old = st.lines.len();
    let pos = st.rope.byte_len();

    let l = measure(&mut st, &old_entries, n_old, |st| st.edit(pos, pos, "```\nz\n```\n"));
    print_ledger("fence tail append(cold)", &l);

    assert_eq!(l.shadow_folds, l.shadow_zone, "② 应 == (anchor,conv_old]");
    assert!(l.shadow_folds <= l.e_total / 100, "② 应 zone 级: {}", l.shadow_folds);
    assert!(l.rls >= n_old * 9 / 10, "回退应短: rls={} n={}", l.rls, n_old);
    // 冷启:① = 0→start_line 全前缀(v3 无能助力——首 ripple 无温存)
    assert_eq!(l.cached_folds, l.cf_cached_old, "① 冷启应 == 0→start_line");
    assert!(l.cached_folds >= l.e_total * 9 / 10, "① 冷启 ≈E: {}", l.cached_folds);
    // 总量对照:昨天 2E → 今天 ≈E
    assert!(l.total_old() >= l.e_total * 18 / 10);
    assert!(l.total_now() <= l.e_total * 11 / 10);
}

/// B. fence 中部内容编辑(冷启,25% 处):② zone 级;① 冷启 ≈E/4。
#[test]
fn ledger_fence_mid_edit() {
    let doc = fence_doc(25_000);
    let mut st = State::new(&doc);
    let old_entries = st.frame_log.entries.clone();
    let n_old = st.lines.len();
    let line = (n_old / 4) + 1;
    let pos = st.rope.byte_of_line(line);

    let l = measure(&mut st, &old_entries, n_old, |st| st.edit(pos, pos + 1, "X"));
    print_ledger("fence mid edit(cold, 25%)", &l);

    assert_eq!(l.shadow_folds, l.shadow_zone);
    assert!(l.shadow_folds <= l.e_total / 100, "② 应 zone 级: {}", l.shadow_folds);
    assert!(l.cached_folds >= l.e_total / 5, "① 冷启 ≈E/4: {}", l.cached_folds);
}

/// C. 打字流(fence 尾部连续两次)——v3 温存的主证据:
/// #1 冷:② zone 级,① ≈E;#2 温:② zone 级,① 也 zone 级(增量续放)。
/// 昨天每次 2E;今天 #2 ≈ zone——"尾部编辑归零"的完整解(v2+v3 组合)。
#[test]
fn ledger_typing_stream_warm_zero() {
    let doc = fence_doc(25_000);
    let mut st = State::new(&doc);

    let old1 = st.frame_log.entries.clone();
    let n1 = st.lines.len();
    let pos1 = st.rope.byte_len();
    let l1 = measure(&mut st, &old1, n1, |st| st.edit(pos1, pos1, "```\nz\n```\n"));
    print_ledger("typing #1(cold)", &l1);
    assert_eq!(l1.shadow_folds, l1.shadow_zone);
    assert!(l1.cached_folds >= l1.e_total * 9 / 10, "#1 冷启 ①≈E");

    let old2 = st.frame_log.entries.clone();
    let n2 = st.lines.len();
    let pos2 = st.rope.byte_len();
    let l2 = measure(&mut st, &old2, n2, |st| st.edit(pos2, pos2, "```\ny\n```\n"));
    print_ledger("typing #2(warm)", &l2);

    assert_eq!(l2.shadow_folds, l2.shadow_zone, "#2 ② 应 zone 级");
    // #2 温存:① = [cached_line(前 rls) → start) 增量——zone 级而非全前缀。
    assert!(
        l2.cached_folds <= l2.e_total / 100,
        "#2 温存 ① 应 zone 级(增量续放),实测={} (E={})",
        l2.cached_folds, l2.e_total
    );
    assert!(
        l2.total_now() <= l2.e_total / 50,
        "#2 总量应 ≪E: 实测={} E={}",
        l2.total_now(), l2.e_total
    );
}

/// D. 列表语料尾部追加——回退不存在:rls == min_dirty
/// (30000 行列表尾 append,rls=29999,零回退),anchor = start−δ 不再回
/// 0;账本断言换轨到承诺列语义(起点直读——旧"anchor=0 回退两份原价"
/// 断言随 prev_semantic_boundary 移除）。
#[test]
fn ledger_list_tail_append_retreat_regime() {
    let doc = list_doc(30_000);
    let mut st = State::new(&doc);
    let old_entries = st.frame_log.entries.clone();
    let n_old = st.lines.len();
    let pos = st.rope.byte_len();

    let l = measure(&mut st, &old_entries, n_old, |st| st.edit(pos, pos, "- zz\n"));
    print_ledger("list tail append(no-retreat)", &l);

    // 4.4 守门:回退不存在——rls 即脏行(尾部 append 的新行号 = n_old),
    // anchor = start − δ = n_old−1,不再是 0。
    assert_eq!(l.rls, n_old, "回退退役:rls 应为脏行(旧行为 rls=0 雪崩)");
    assert_eq!(l.anchor, n_old - 1, "anchor = rls − δ(回退退役)");
    // 重放成本归 zone 级:shadow 折叠 ≈ 旧侧 zone 折叠(非全前缀)。
    assert!(l.shadow_folds <= l.cf_shadow_old.max(64), "尾部 append 折叠应 zone 级");
}

/// E. 前缀重放单价(参照系;debug build)。
#[test]
fn prefix_replay_unit_cost() {
    let doc = fence_doc(25_000);
    let st = State::new(&doc);
    let entries = &st.frame_log.entries;
    let e = entries.len();
    let mut stack = Vec::new();
    let mut anchors = Vec::new();
    let mut open = Vec::new();
    crate::block::forward_replay_runtime(entries, &mut stack, &mut anchors, &mut open);
    let mut best = f64::MAX;
    for _ in 0..5 {
        stack.clear();
        anchors.clear();
        open.clear();
        let t0 = Instant::now();
        crate::block::forward_replay_runtime(entries, &mut stack, &mut anchors, &mut open);
        best = best.min(t0.elapsed().as_nanos() as f64);
    }
    eprintln!(
        "forward_replay_runtime: E={} best={:.0}ns → {:.1}ns/entry (debug)",
        e,
        best,
        best / e as f64
    );
}

/// 承诺列哨兵(4.4 后——原 line_effect_shape 探针的语义继任):
///   真空行(0 chunks)→ 承诺 invalid(空行不设 hint);
///   BQ 裸行("> ")→ 承诺 invalid(PrevHint::Blank 语义保留,CM 例 22)。
#[test]
fn mini_boundary_probe() {
    // 6 个 BQ 块(空行分隔)+ 一个 CM22 裸行段:"> foo\n>\nlazy"
    let mut doc = String::new();
    for i in 0..6 {
        doc.push_str("> quote");
        doc.push_str(&i.to_string());
        doc.push_str("\n\n");
    }
    doc.push_str("> foo\n>\nlazy\n");
    let mut st0 = State::new(&doc);
    let col = st0.snapshot().prev_col.clone();
    let mut blank_invalid = 0usize;
    let mut blank_zero_chunks = 0usize;
    let mut bare_gt_invalid = 0usize;
    for l in 0..st0.lines.len() {
        let txt = st0.rope.line(l).to_string();
        let n_chunks = st0.rope.line(l).chunks().count();
        if st0.lines[l].node_type() == crate::block::BlockType::Blank
            && txt.trim().is_empty()
            && n_chunks == 0
        {
            blank_zero_chunks += 1;
            if !col[l].is_valid() {
                blank_invalid += 1;
            }
            eprintln!("  L{} 真空行 chunks={} 承诺 invalid={}(应 true)", l, n_chunks, !col[l].is_valid());
        }
        if txt.trim() == ">" {
            if !col[l].is_valid() {
                bare_gt_invalid += 1;
            }
            eprintln!("  L{} BQ 裸行 承诺 invalid={}(应 true——CM22 hint 语义保留)", l, !col[l].is_valid());
        }
    }
    eprintln!(
        "blanks: zero_chunks={} invalid={} | bare_gt invalid={}",
        blank_zero_chunks, blank_invalid, bare_gt_invalid
    );
    assert!(blank_zero_chunks >= 5, "空行样本: {}", blank_zero_chunks);
    assert_eq!(blank_invalid, blank_zero_chunks, "真空行承诺应全部 invalid");
    assert!(bare_gt_invalid >= 1, "BQ 裸行应保持承诺 invalid(CM22): {}", bare_gt_invalid);
    drop(st0);
}
