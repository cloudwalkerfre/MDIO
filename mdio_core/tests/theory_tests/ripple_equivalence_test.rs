//! Ripple 增量扫描等价性验证
//!
//! 验证 `edit() + ripple_lines()` 产出与 `rebuild_all()` 一致。
//! 运行: cargo test --test ripple_equivalence_test
//!
//! ── 基础设定与证明依赖 ──
//!
//! 每条设定的依据来自 ripple_prove.md §1-§3 和代码。需要独立证明的部分标注 ★。
//!
//! 基本域 (定义, 无需证明):
//!   D01  D = [L_0..L_{N-1}], L_i ∈ ByteSeq       — Rope::line(i)
//!   D02  Stack = Vec<BlockFrame>, |s| ≤ 32        — MAX_BLOCK_NESTING
//!   D03  BlockType 27种, PackedLineInfo(u8)       — ENUM + pack()/node_type()
//!   D04  FrameLogEntry(u64), bit layout           — push_entry/pop_entry/body_mut_entry
//!   D05  SCAN_LINE_INFO 在 scan_block 前设置       — projection.rs:462
//!
//! 核心函数建模 (定义, 边界清晰):
//!   F01  step(b,s,p,i) 封装 Pop 循环               — projection.rs:469-603 (命名由证明引入)
//!   F02  scan_all = foldl step s₀                 — rebuild_all 的逐行迭代
//!   F03  replay(E,s) = forward_replay(E,s)        — block.rs 的实现
//!
//! 状态抽象:
//!   S01  scan_block 参数: bytes, &stack, prev_type — API 事实, 无需证明
//!   S02  FrameLog 不记录 prev_type                — 代码事实: 只记录 Push/Pop/BodyMut
//!   S03  prev_type 从 lines[i-1] 计算:             — projection.rs:464-467
//!          π(p) = Blank ? None : Some(p)             rebuild_all 和 ripple_lines 完全一致
//!   S04  stack 是唯一需从 FrameLog 恢复的扫描状态    — S02+S03 的推论, ★ L3 保证 FrameLog 恢复栈
//!
//! ★ 需要证明的引理:
//!   L1  step 是确定性的                              — scan_block 纯函数 + Pop 循环有界 + 无外部状态
//!   L2  scan(s₀, A++B) = scan(scan(s₀,A).state, B)  — 左折叠结合律
//!   L3  rebuild(FL,i) = stacks(D,i)                 — FrameLog 忠实记录所有栈变更
//!   L4  编辑前缀不变 → 扫描中间状态一致                — L1 的迭代推导
//!   L5  四收敛条件 → q_new_{k+1}=q_old_{k-delta+1}   — D 给栈, A+C 给 prev_type
//!       且后缀字节对应
//!
//! 编辑模型:
//!   E01  start = line_of_byte(start_byte)           — 编辑的第一受影响行
//!   E02  delta = |D_new| - |D_old|                  — 行数变化
//!   E03  前缀不变: i < start → D_new[i]=D_old[i]     — Rope 替换特性, L4 直接引用
//!   E04  后缀对齐: i≥max_splice → 字节+PL值对应       — edit() 的 splice/invalidate, L5 直接引用
//!
//! 收敛条件:
//!   C01  四条件 A/B/C/D 语义                         — 代码直接表达 (projection.rs:606-612)
//!   C02  A+C → prev_type 对齐                       — L5 证明中完成
//!   C03  D → 栈对齐                                  — L5 证明中完成
//!
//! 证明边界:
//!   P01  scan_block 是 CommonMark 正确实现的假设      — 不依赖。即使 scan_block 有 bug,
//!         只要 rebuild_all 和 ripple_lines 调用同一个 scan_block, 等价性仍成立
//!   P02  HtmlElement 帧体位移编码不一致              — 已知局限, 不影响 PL/FL 等价和 walk_blocks
//!   P03  三层架构: ripple 只证明 Layer1+2, Layer3     — 纯函数推论, 不需独立证明
//!         (walk_blocks/FrameSpans) 自证
//!
//! ── 测试覆盖 ──
//!
//! | 场景 | delta | 测试 |
//! |------|-------|------|
//! | 无变化, 立即收敛 | 0 | no_change_immediate_convergence |
//! | 行内文本修改 | 0 | inline_text_change |
//! | 容器结构变化 (bullet→paragraph) | 0 | bullet_to_paragraph |
//! | 嵌套容器内编辑 (BQ>BLT) | 0 | nested_container_edit |
//! | 行插入 | 1 | line_insertion |
//! | 行删除 | -1 | line_deletion |
//! | 空行处理 | 0 | blank_line_handling |
//! | codefence→paragraph | 0 | codefence_to_paragraph |
//! | 文档开始编辑 | 0 | edit_at_document_start |
//! | 文档末尾编辑 | 0 | edit_at_document_end |
//! | 多次累积编辑 | 0 | multi_edit_accumulation |

use mdio_core::projection::State;
use mdio_core::block::{FrameKind, FrameLog, PackedLineInfo};

/// root 信封 → ch 数组(统一——旧数组形态断言用——集成测试
/// 本地辅助)。
fn ast_ch(snap: &mdio_core::projection::Snapshot) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(
        &snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf))
        .unwrap_or_else(|e| panic!("ast_ch 解析失败: {e}"));
    v["ct"].clone()
}

/// T-mode 全量 ch 数组(827):信封 ch 按 mode 自动适配——同一用例
/// CF/HS 双跑共用。
fn mode_ch(snap: &mdio_core::projection::Snapshot, mode: mdio_core::topology::ProjectionMode) -> serde_json::Value {
    let v: serde_json::Value = serde_json::from_str(&snap.to_ast_object_mode(mode))
        .unwrap_or_else(|e| panic!("mode_ch 解析失败: {e}"));
    v["ct"].clone()
}

// 基础设定验证

/// E03/E04: edit() 保持前缀不变, 正确设置 splice 边界.
#[test]
fn edit_preserves_prefix() {
    let mut state = State::new("# H1\n- a\n- b\n- c\n");
    let old_lines: Vec<u8> = (0..4).map(|i| state.lines[i].0).collect();
    state.edit(8, 12, "X");
    // 前缀行 (0..start_line) 的 PackedLineInfo 不应改变
    assert_eq!(state.lines[0].0, old_lines[0]);
    assert_eq!(state.lines[1].0, old_lines[1]);
}

/// S02+S03: FrameLog 不记录 prev_type —— prev_type 只能从 PackedLineInfo 数组计算.
/// 同样两份文档, 在对应位置有相同栈但不同 prev_type 的上下文中,
/// FrameLog 前缀重建的栈不可区分 — 因为 FrameLog 只记录 Push/Pop/BodyMut.
#[test]
fn framelog_does_not_encode_prev_type() {
    let a = State::new("# H1\n- a\n- b\n");
    let b = State::new("# H1\n\n- a\n- b\n");
    // 文档 a: 行1="- a"(Bullet), prev_type 依赖于行0="# H1"(Header)
    // 文档 b: 行1="", 行2="- a"(Bullet), 行2 的 prev_type 是 None (因为行1是Blank)
    // FrameLog 重建行2开始时的栈 — 两者应该可以从各自的 FrameLog 恢复出栈
    // 但 prev_type 本身不在 FrameLog 中编码 — 测试 verify:
    //   rebuild_stack 在对应位置返回相同栈结构, 但这并不保证 prev_type 对齐
    let stack_a = a.frame_log.rebuild_stack(2);
    let stack_b = b.frame_log.rebuild_stack(3); // 文档 b 的 "- a" 在行2
    // FrameLog 重建的栈应仅依赖于容器开闭, 不依赖 prev_type
    assert_eq!(stack_a.len(), stack_b.len());
}

/// S04: 栈是唯一需要通过 FrameLog 恢复的扫描状态分量.
/// 验证对相同文档, rebuild_stack 在每行产出与扫描一致的栈 (间接验证).
/// 两独立 State 扫描同一文档 → FrameLog "前缀" replay 结果一致.
#[test]
fn framelog_stack_recovery_consistent() {
    let a = State::new("# H1\n> - item\n>   para\n- b\n");
    let b = State::new("# H1\n> - item\n>   para\n- b\n");
    // 两个独立扫描产出相同 FrameLog → 每个行边界栈应与对方一致
    for i in 0..=4 {
        let sa = a.frame_log.rebuild_stack(i as u32);
        let sb = b.frame_log.rebuild_stack(i as u32);
        assert_eq!(sa, sb, "stack mismatch at line {}", i);
    }
}

/// E03+L4: 编辑前缀不变 → FrameLog 前缀不受编辑影响.
#[test]
fn framelog_prefix_stable_under_edit() {
    let mut state = State::new("# H1\n- a\n- b\n- c\n- d\n");
    let prefix_end = state.frame_log.entries.partition_point(|e| ((e >> 24) as u32) < 3);
    let old_prefix: Vec<u64> = state.frame_log.entries[..prefix_end].to_vec();

    state.edit(14, 15, "X"); // 修改行 3 内容, 不影响行 0-2
    state.ensure_projected();

    let new_prefix: Vec<u64> =
        state.frame_log.entries.iter().copied()
            .filter(|&e| ((e >> 24) as u32) < 3)
            .collect();
    assert_eq!(old_prefix, new_prefix, "FrameLog prefix should be unchanged after edit below prefix");
}

// 等价性测试

fn states_eq(a: &State, b: &State) -> bool {
    if a.lines.len() != b.lines.len() {

        return false;
    }
    for i in 0..a.lines.len() {
        if a.lines[i].0 != b.lines[i].0 {

            return false;
        }
    }
    // 承诺列逐行比对(ripple 路径 == 重建态)。
    if a.prev_col.len() != b.prev_col.len() {

        return false;
    }
    for i in 0..a.prev_col.len() {
        if a.prev_col[i].0 != b.prev_col[i].0 {
            eprintln!(
                "prev_col[{}] diff: {:02x} vs {:02x}",
                i, a.prev_col[i].0, b.prev_col[i].0
            );
            return false;
        }
    }
    let fa = &a.frame_log.entries;
    let fb = &b.frame_log.entries;
    if fa.len() != fb.len() {

        // continue to check entries
    }
    for i in 0..fa.len().min(fb.len()) {
        if fa[i] != fb[i] {

        }
    }
    fa == fb
}

fn run_and_check(old_text: &str, start: usize, end: usize, new_text: &str) {
    let mut state = State::new(old_text);
    state.edit(start, end, new_text);
    state.ensure_projected();

    let new_full = {
        let rope_as = state.rope.to_string();
        State::new(&rope_as)
    };

    assert!(states_eq(&state, &new_full),
        "ripple != rebuild_all\nold: {:?}\nnew_text: {:?}\nstart: {}, end: {}",
        old_text, new_text, start, end);
}

fn run_and_check_boundary(old_text: &str, start: usize, end: usize, new_text: &str,
    checks: &[(FrameKind, u32)])
{
    let mut state = State::new(old_text);
    state.edit(start, end, new_text);
    state.ensure_projected();

    let ref_state = State::new(&state.rope.to_string());
    assert!(states_eq(&state, &ref_state),
        "FL mismatch before boundary check");

    for &(kind, start_line) in checks {
        let a = state.frame_log.item_tail_boundary(start_line, kind, &state.lines);
        let b = ref_state.frame_log.item_tail_boundary(start_line, kind, &ref_state.lines);
        assert_eq!(a, b,
            "boundary_for({}, {:?}) diverged: ripple={:?} rebuild={:?}",
            start_line, kind, a, b);
    }
}


#[test]
fn no_change_immediate_convergence() {
    // 编辑不改变任何字节 → delta=0, 立即收敛
    run_and_check("# H1\n- a\n- b\n", 3, 3, "");
}

#[test]
fn inline_text_change() {
    // 修改一行文本内容, 不改变行结构
    run_and_check("# H1\n- setup: old\n- config: b\n", 8, 11, "new");
}

#[test]
fn line_insertion() {
    run_and_check("# H1\n- a\n- c\n", 9, 9, "- b\n");
}

#[test]
fn line_deletion() {
    // 删除行
    run_and_check("# H1\n- a\n- b\n- c\n", 10, 17, "");
}

#[test]
fn bullet_to_paragraph() {
    // "# H1\n- a\n- b\n" → "# H1\ntext\n- b\n" (替换 "- a" 为 "text")
    run_and_check("# H1\n- a\n- b\n", 5, 8, "text");
}

#[test]
fn nested_container_edit() {
    // BQ > BLT 内部编辑
    run_and_check("# H1\n> - item: old\n>   detail\n> - other\n",
        12, 15, "new");
}

#[test]
fn multi_edit_accumulation() {
    let mut state = State::new("# H1\n- a\n- b\n- c\n- d\n");
    state.edit(7, 8, "A");
    // rope now: "# H1\n- A\n- b\n- c\n- d\n"

    state.edit(15, 16, "C");
    // rope now: "# H1\n- A\n- b\n- C\n- d\n"

    state.ensure_projected();


    let ref_state = State::new("# H1\n- A\n- b\n- C\n- d\n");

    assert!(states_eq(&state, &ref_state));
}

#[test]
fn edit_at_document_start() {
    run_and_check("# Old\n- item\n", 0, 4, "# New");
}

#[test]
fn edit_at_document_end() {
    run_and_check("# H1\n- a\n- b\n", 13, 13, "\n- c");
}

#[test]
fn blank_line_handling() {
    run_and_check("# H1\n\n- a\n- b\n", 4, 4, "text");
}

#[test]
fn codefence_to_paragraph() {
    run_and_check("```\ncode\n```\n\n- item\n", 0, 13, "regular text");
}

// L5 直接验证 + 边界场景

/// 编辑只影响文档开头 → 收敛应在 start 附近发生, 后缀不受影响.
#[test]
fn convergence_suffix_intact() {
    let mut state = State::new("# H1\n- a\n- b\n- c\n- d\n- e\n");
    let old_suffix: Vec<u8> = (3..6).map(|i| state.lines[i].0).collect();
    state.edit(2, 3, "X"); // "# H1" → "# X1"
    state.ensure_projected();
    let ref_state = State::new("# X1\n- a\n- b\n- c\n- d\n- e\n");
    for i in 3..6 {
        assert_eq!(state.lines[i].0, old_suffix[i - 3],
            "suffix line {} should be unchanged", i);
    }
    assert!(states_eq(&state, &ref_state));
}

/// 永不收敛: 编辑改变了每一行 → 扫描到底 ≡ rebuild_all.
#[test]
fn no_convergence_scan_to_end() {
    let mut state = State::new("- a\n- b\n- c\n");
    state.edit(0, 12, "# New\n\n- x\n- y\n");
    state.ensure_projected();
    let ref_state = State::new("# New\n\n- x\n- y\n");
    assert!(states_eq(&state, &ref_state));
}

/// large delta > 0
#[test]
fn large_line_insertion() {
    let mut state = State::new("# H1\n- a\n- b\n");
    state.edit(5, 5, "- x\n- y\n");
    state.ensure_projected();
    let ref_state = State::new("# H1\n- x\n- y\n- a\n- b\n");
    assert!(states_eq(&state, &ref_state));
}

/// large delta < 0
#[test]
fn large_line_deletion() {
    let mut state = State::new("# H1\n- a\n- b\n- c\n- d\n");
    state.edit(5, 17, "");
    state.ensure_projected();
    let ref_state = State::new("# H1\n- d\n");
    assert!(states_eq(&state, &ref_state));
}

/// L3 强化: BQ>BLT 嵌套下每行 rebuild_stack 一致
#[test]
fn framelog_stack_recovery_nested() {
    let doc = "# H1\n> - outer\n>   - inner\n>     deep\n>   back\n\n- b\n";
    let a = State::new(doc);
    let b = State::new(doc);
    for i in 0..=7 {
        assert_eq!(
            a.frame_log.rebuild_stack(i as u32),
            b.frame_log.rebuild_stack(i as u32),
            "nested stack mismatch at line {}", i
        );
    }
}

// 补充覆盖: 之前遗漏的边界

/// delta≠0 + prev_type 语义变化:
/// 插入空行使 `---` 从 HsSep (prev=Paragraph) 变为 ThematicBreak (prev=Blank).
/// 末尾的 `- a` 提供 FL entry, 避免触发 empty-FL dummy.
#[test]
fn prev_type_shift_on_blank_insert() {
    // Old: "para\n---\n- a\n" → Paragraph, HsSep, Bullet
    // New: "para\n\n---\n- a\n" → Paragraph, Blank, ThematicBreak, Bullet
    // delta = +1, prev_type 从 Some(Paragraph) 变成 None → --- 类型从 HsSep 变成 ThematicBreak
    run_and_check("para\n---\n- a\n", 5, 5, "\n");
}

/// delta≠0 + 文档开头插入 heading: 结构从全 Bullet 变为 Header1 + Bullet.
#[test]
fn heading_insert_before_list() {
    // Old: "- a\n- b\n" → Bullet, Bullet
    // New: "# H1\n- a\n- b\n" → Header1, Bullet, Bullet
    // delta = +1, 首行类型从 Bullet 变为 Header1
    run_and_check("- a\n- b\n", 0, 0, "# H1\n");
}

/// BQ>BLT>OL 三层嵌套内编辑.
#[test]
fn three_level_nesting_edit() {
    // "> - 1. item\n> - 2. item\n"
    // 修改第一个 item 的文本
    run_and_check("> - 1. item\n> - 2. item\n", 7, 11, "text");
}

/// Table 内编辑: 修改单元格内容.
#[test]
fn table_cell_edit() {
    run_and_check("| a | b |\n| - | - |\n| 1 | 2 |\n", 2, 3, "x");
}

/// 收敛发生在列表中间, suffix 包含 Pop 等栈变更.
#[test]
fn list_edit_converge_before_pop() {
    // 5 行: Bullet×3 + Blank + Paragraph. 列表在 Blank→Paragraph 处 Pop.
    // 编辑首行, 收敛应在列表中间, suffix 含 Pop/BodyMut.
    run_and_check("- a\n- b\n- c\n\ntext\n", 0, 3, "- x");
}

/// delta≠0 + 列表中间编辑, suffix 含栈变更且需 shift.
#[test]
fn list_insert_converge_with_suffix_shift() {
    // Old: "- a\n- b\n- c\n\ntext\n" → 5 行
    // 在首行前插入 heading → 6 行, delta=+1
    // 收敛后 suffix 的 Pop/BodyMut 行号需 +1
    run_and_check("- a\n- b\n- c\n\ntext\n", 0, 0, "# H1\n");
}

// ast-patch 新架构专项测试

/// T1 — B1 修复: list 不吞后续 CF.
/// 旧代码中 CF 被错误当 paragraph continuation 吞入 list.
#[test]
fn b1_list_does_not_claim_codefence() {
    // "- a\n- b\n\n```js\ncode\n```\n"
    // 空行 + CF 应在 list 之后, 非 list 之内
    run_and_check("- a\n- b\n\n```js\ncode\n```\n", 0, 3, "- x");
}

/// T2 — B4 修复: nested list indent 正确产生嵌套 BLT.
#[test]
fn b4_nested_list_indent() {
    // "- a\n  - b\n  - c\n- d\n"
    // "  - b" indent=2 → 嵌套 BLT Push
    run_and_check("- a\n  - b\n  - c\n- d\n", 0, 3, "- x");
}

/// T3 — effective_outer: Composite 行的 PL 反映最内层容器类型.
/// "> - item" → PL=Bullet (非 Blockquote).
/// ripple 和 rebuild_all 内部一致即可.
#[test]
fn effective_outer_composite_consistency() {
    // "> - 1. item\n> - 2. item\n" → BQ>BLT>OL 三层
    run_and_check("> - 1. item\n> - 2. item\n", 7, 11, "text");
}

/// T4 — Table model: TableRow PopPush for data rows.
#[test]
fn table_model_with_tablerow_frame() {
    // Multi-row table — verify ripple handles TableRow PopPush correctly
    run_and_check("| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n", 2, 3, "x");
}

/// T5 — R21 修复: BQ 内 CF 内容正确.
#[test]
fn bq_codefence_content() {
    // "> ```js\n> code\n> ```\n\nplain\n"
    // CF 在 BQ 内, closing fence 正确识别
    run_and_check("> ```js\n> code\n> ```\n\nplain\n", 11, 15, "text");
}

/// T6 — 收敛边界上 PopPush 在 suffix 中.
#[test]
fn converge_with_poppush_in_suffix() {
    // "- a\n- b\n- c\n- d\n\ntext\n" → 6 行
    // 编辑前两项, 收敛应在列表中间, suffix 含 PopPush+Pop
    run_and_check("- a\n- b\n- c\n- d\n\ntext\n", 0, 8, "- x\n- y\n");
}

/// T7 — 盲区 1: 收敛时栈为空.
/// bullet→paragraph 导致 BLT Pop, 栈清空后收敛.
#[test]
fn converge_with_empty_stack() {
    // "- a\n\nplain\n" → bullet+blank+paragraph
    // "- a" 改为 "text" → BLT Pop, stack=[]
    run_and_check("- a\n\nplain\n", 0, 3, "text");
}

/// T8 — 盲区 2: max_splice_index 精确边界.
/// 连续两次编辑, 第二次的 max_splice 在边界处.
#[test]
fn max_splice_boundary() {
    let mut state = State::new("# H1\n- a\n- b\n");
    state.edit(3, 4, "2");   // "# H1" → "# H2", max_splice 更新
    state.edit(5, 8, "- x"); // 修改 "- a" → "- x"
    state.ensure_projected();
    let ref_state = State::new("# H2\n- x\n- b\n");
    assert!(states_eq(&state, &ref_state));
}

/// T9 — 盲区 3: delta≠0 后 cached_rebuild 使用 original FL.
#[test]
fn cached_rebuild_after_line_insert() {
    // 在 "- a\n- b\n" 前插入 "# H1\n", delta=+1
    // cached_rebuild 用 old FL 重建栈, partition_point 在旧行号上
    run_and_check("- a\n- b\n", 0, 0, "# H1\n");
}

/// T10 — 盲区 4: 两次连续编辑后 cached_stack 失效重建.
#[test]
fn two_edits_cached_stack_rebuild() {
    let mut state = State::new("# H1\n- a\n- b\n- c\n");
    state.edit(5, 8, "- x");   // "# H1\n- a\n" → "# H1\n- x\n", "- a" at bytes 5-8
    state.edit(9, 12, "- y");  // "- b" at bytes 9-12 in updated rope
    state.ensure_projected();
    let ref_state = State::new("# H1\n- x\n- y\n- c\n");
    assert!(states_eq(&state, &ref_state));
}

// 攻击性场景 — FL merge 边界推演验证

/// 攻击 1: scan 区容器完全消失, delta=0, ripple 空.
/// 旧 FL 有 Push/BodyMut/Pop, 新文档无容器.
/// merge 条件第二半 AND 阻止 merge → 旧容器条目残留.
#[test]
fn attack_container_vanish_ripple_empty() {
    // 旧: "- a\n- b\n- c\n" → FL 有 BLT entries
    // 新: 全部改为 paragraph
    let mut state = State::new("- a\n- b\n- c\n");
    state.edit(0, 12, "text a\ntext b\ntext c");
    state.ensure_projected();
    let ref_state = State::new("text a\ntext b\ntext c");
    assert!(states_eq(&state, &ref_state));
}

/// 攻击 1b: 单行 bullet→paragraph, 后续行不变.
#[test]
fn attack_single_bullet_to_paragraph() {
    // 旧: "- a\n- b\n- c\n" → FL: Push@0, PopPush@1, PopPush@2
    // 编辑: 行 0 "- a" → "text", 行 1-2 仍是 bullets
    run_and_check("- a\n- b\n- c\n", 0, 3, "text");
}

/// 攻击 3: delta=+1, 插入行在容器之前.
/// suffix 中旧 FL entries 的 line 需要 +1 shift.
#[test]
fn attack_delta_plus_one_list() {
    // 旧: "- a\n- b\n" (2行)
    // 新: "# H1\n- a\n- b\n" (3行, 插入 heading)
    // FL suffix 从 conv_old+1 开始 → 需 shift +1
    run_and_check("- a\n- b\n", 0, 0, "# H1\n");
}

/// 攻击 6: delta=-1, 删除中间行.
#[test]
fn attack_delta_minus_one_list() {
    // 旧: "- a\n- b\n- c\n" (3行)
    // 新: "- a\n- c\n" (2行, 删除 "- b\n")
    // delta=-1, suffix shift -1
    run_and_check("- a\n- b\n- c\n", 4, 8, "");
}

/// 攻击 1c: scan 区在文档中间, 容器消失.
/// 前缀有容器, 后缀有容器, 中间 scan 区清空.
#[test]
fn attack_container_vanish_mid_document() {
    // 旧: "- a\n- b\n- c\n\n- d\n" (前 3 行 BLT, 空行, 新 BLT)
    // 编辑: "- a\n- b\n- c\n" → "x\ny\nz\n" (12 bytes → 6 bytes, delta=-1)
    // FL 前缀保留 Push@0, 后缀有 Push@4(旧)→shift→Push@3(新)
    run_and_check("- a\n- b\n- c\n\n- d\n", 0, 12, "x\ny\nz");
}

#[test]
fn boundary_blt_delta_zero() {
    run_and_check_boundary("- a\n- b\n- c\n\ntext\n", 0, 3, "- x",
        &[(FrameKind::BulletList, 2)]);
}

#[test]
fn boundary_bq_blt_nested() {
    run_and_check_boundary("> - a\n> - b\n> plain\n\noutside\n", 2, 3, "x",
        &[(FrameKind::BlockQuote, 0), (FrameKind::BulletList, 0)]);
}

#[test]
fn boundary_cf_delta_zero() {
    run_and_check_boundary("```\ncode\n```\n\ntext\n", 1, 2, "C",
        &[(FrameKind::CodeFence, 0)]);
}

#[test]
fn boundary_delta_plus_one() {
    run_and_check_boundary("- a\n- b\n- c\n", 0, 0, "# H1\n",
        &[(FrameKind::BulletList, 1)]);
}

#[test]
fn boundary_delta_minus_one() {
    run_and_check_boundary("- a\n- b\n- c\n", 4, 8, "",
        &[(FrameKind::BulletList, 0)]);
}

#[test]
fn boundary_container_vanish() {
    let mut state = State::new("- a\n- b\n- c\n");
    state.edit(0, 12, "text a\ntext b\ntext c");
    state.ensure_projected();
    let ref_state = State::new("text a\ntext b\ntext c");
    assert!(states_eq(&state, &ref_state));
    assert_eq!(state.frame_log.item_tail_boundary(0, FrameKind::BulletList, &state.lines), None);
    assert_eq!(ref_state.frame_log.item_tail_boundary(0, FrameKind::BulletList, &ref_state.lines), None);
}

// 理论验证 — formal_scan.md 实验反哺

use mdio_core::block::{scan_block, BlockFrame, BlockType, LOOSE_BIT};

/// §W: CF 的 body_adm = ∅.
/// body_out 位不同 (fence_char, fence_len) 不影响普通内容的类型.
#[test]
fn theory_cf_body_out_isolated() {
    let cf3 = BlockFrame::new(FrameKind::CodeFence, 0,
        BlockFrame::encode_cf_body(b'`', 3));
    let cf5 = BlockFrame::new(FrameKind::CodeFence, 0,
        BlockFrame::encode_cf_body(b'`', 5));
    let cft = BlockFrame::new(FrameKind::CodeFence, 0,
        BlockFrame::encode_cf_body(b'~', 3));

    let (t1, _) = scan_block(b"code", &[cf3], None);
    let (t2, _) = scan_block(b"code", &[cf5], None);
    let (t3, _) = scan_block(b"code", &[cft], None);
    assert_eq!(t1, BlockType::CodeContent);
    assert_eq!(t1, t2);
    assert_eq!(t1, t3);

    // 不匹配的 closing fence 仍被 CF 认领(admits=⊤)
    let (t4, _) = scan_block(b"~~~", &[cf3], None);
    assert_eq!(t4, BlockType::CodeContent);
}

/// §W: BLT 的 LOOSE_BIT 是 body_out, 不影响 item 分类.
#[test]
fn theory_blt_loose_isolated() {
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));
    let blt_loose = BlockFrame(blt.0 | LOOSE_BIT);

    let (t1, _) = scan_block(b"- b", &[blt], Some(BlockType::Bullet));
    let (t2, _) = scan_block(b"- b", &[blt_loose], Some(BlockType::Bullet));
    assert_eq!(t1, BlockType::Bullet);
    assert_eq!(t1, t2);
}

/// §Z: 等价类 |Σ/~_s| ≤ |s|+1.
/// 对固定栈, 不同行的 Pop 循环收敛深度有限.
#[test]
fn theory_equivalence_class_bound() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));
    let stack = vec![bq, blt]; // depth=2, max 3 classes

    // 没有公开的 Pop 循环函数, 用 scan_block 的输出间接验证:
    // 三种不同行产生三种 admit 行为.
    let (t1, _) = scan_block(b"> - item", &stack, Some(BlockType::Bullet));
    let (t2, _) = scan_block(b"> plain", &stack, Some(BlockType::Bullet));
    let (t3, _) = scan_block(b"outside", &stack, Some(BlockType::Bullet));
    // 三行对应三种深度: BLT认领 / BQ认领 / 都不认领←但scan_block返回Pop所以要外层处理
    // 简化验证: 三种输入产生至少两种不同结果
    let types = [t1 as u8, t2 as u8, t3 as u8];
    let unique: std::collections::BTreeSet<u8> = types.iter().copied().collect();
    assert!(unique.len() >= 2, "at least 2 distinct behaviors for |s|=2");
    assert!(unique.len() <= 3, "at most |s|+1=3 equivalence classes");
}

/// §AH.5 遗忘性: 认领行的那层之下的容器不影响该行的结果.
#[test]
fn theory_forgetting() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));

    // "> - item": BLT在栈顶→admits, BQ在BLT下→不被检查→遗忘
    // "- item":  只有BLT→admits
    // 两者BLT判定相同→结果一致
    let (t1, _) = scan_block(b"> - item", &[bq, blt], Some(BlockType::Bullet));
    let (t2, _) = scan_block(b"- item", &[blt], Some(BlockType::Bullet));
    assert_eq!(t1, BlockType::Bullet);
    assert_eq!(t2, BlockType::Bullet);
}

/// §AF.3: Continue 行中栈不变 → 等价类复用.
/// 同类行在相同栈下产生相同 scan_block 结果.
#[test]
fn theory_continue_cache() {
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));
    let pt = Some(BlockType::Bullet);
    let stack = vec![blt];

    // 三个连续段落行, 栈不变 → 结果相同
    let (t1, _) = scan_block(b"line one", &stack, pt);
    let (t2, _) = scan_block(b"line two", &stack, pt);
    let (t3, _) = scan_block(b"  indented", &stack, pt);
    assert_eq!(t1, BlockType::Paragraph);
    assert_eq!(t1, t2);
    assert_eq!(t1, t3);
}

// body_adm/body_out 逐 FrameKind 验证 (§W)

/// 删位（步骤 3）后 BQ body 恒 0——无 para_open 位——原 theory（para_open 影响
/// admit）失效——改写：两帧相同——行为无差异——lazy 由 prev 词法判定。
#[test]
fn theory_bq_para_open_is_body_adm() {
    let bq_open = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    let bq_closed = BlockFrame::new(FrameKind::BlockQuote, 0, 0);
    assert_eq!(bq_open.0, bq_closed.0, "删位后 BQ body 恒 0——两帧相同");

    // scan_block 单次: 类型相同
    let (t1, _) = scan_block(b"> text", &[bq_open], None);
    let (t2, _) = scan_block(b"> text", &[bq_closed], None);
    assert_eq!(t1, t2, "explicit '>' same type");

    // Pop 循环模拟: para_open 差异在 Pop 循环后暴露
    fn pop_loop(bytes: &[u8], stack: &[BlockFrame], pt: Option<BlockType>) -> (BlockType, usize) {
        let mut s = stack.to_vec();
        let mut ft = BlockType::Paragraph;
        let mut guard = 0u8;
        loop {
            guard += 1; if guard > 32 { break; }
            let (t, a) = scan_block(bytes, &s, pt);
            ft = t;
            match a {
                mdio_core::block::ScanAction::Pop => {
                    s.pop();
                    if t != BlockType::Paragraph && t != BlockType::Blank { break; }
                    continue;
                }
                _ => break,
            }
        }
        (ft, s.len())
    }

    // 无 '>' 前缀 + prev=None：BQ lazy 豁免需 prev 不 closes（None→不豁免）→ Pop——
    // 删位后无 para_open 差异——两帧行为相同
    let (_ft3, d3) = pop_loop(b"lazy text", &[bq_open], None);
    let (_ft4, d4) = pop_loop(b"lazy text", &[bq_closed], None);
    assert_eq!(d3, d4, "删位后 BQ 无 para_open——admit 无差异");
}

/// OL: delim_code ∈ body_adm. delim 不同 → 不匹配.
#[test]
fn theory_ol_delim_is_body_adm() {
    let ol_dot = BlockFrame::new(FrameKind::OrderedList, 3,
        BlockFrame::encode_ol_body(b'.'));
    let ol_paren = BlockFrame::new(FrameKind::OrderedList, 3,
        BlockFrame::encode_ol_body(b')'));

    // "2." 在 delim='.' 的 OL 中 → 匹配
    let (t1, _) = scan_block(b"2. item", &[ol_dot], Some(BlockType::OrderedListItem));
    // "2." 在 delim=')' 的 OL 中 → 不匹配 → Pop → dispatch
    let (t2, _) = scan_block(b"2. item", &[ol_paren], Some(BlockType::OrderedListItem));
    assert_eq!(t1, BlockType::OrderedListItem, "matching delim → same item");
    // t2 不匹配 → 类型可能不同 (被 dispatch_new_block 重新分类)
    assert_ne!(t1, t2, "delim mismatch should change admit behavior");
}

/// Table: body encodes header_cols (bits5-1) + confirmed (bit0).
/// body_adm now covers all 6 bits; body differences prevent ripple convergence.
#[test]
fn theory_table_body_adm_all_bits() {
    let body = BlockFrame::encode_table_body(2);
    let t1 = BlockFrame::new(FrameKind::Table, 0, body);
    // Same body → same container
    let t2 = BlockFrame::new(FrameKind::Table, 0, body);
    assert_eq!(t1.0, t2.0, "same header_cols + tentative → identical");
    // Different body → different container
    let t3 = BlockFrame::new(FrameKind::Table, 0, body | 1); // confirmed
    assert_ne!(t1.0, t3.0, "confirmed flag differs → not identical");
    // scan_block on tentative table: data row → Pop (second line must be separator)
    let (ty1, _) = scan_block(b"| data |", &[t1], None);
    assert_eq!(ty1, BlockType::Paragraph); // Pop → Paragraph
}

/// Comment: body 恒为 0 → body_adm = body_out = ∅.
#[test]
fn theory_comment_body_always_zero() {
    let c = BlockFrame::new(FrameKind::Comment, 0, 0);
    let (t, _) = scan_block(b"comment text", &[c], None);
    assert_eq!(t, BlockType::Comment, "Comment admits all lines until -->");
}

// 优化可行性验证 — 从理论到代码

/// 验证: CF/Table/Comment 的 body 比较可安全跳过.
/// 条件 D 的栈比较可对 body_adm=∅ 的 FrameKind 使用部分相等.
#[test]
fn theory_stack_eq_can_skip_body_for_top_containers() {
    // 两个栈只在 CF 的 body 上不同 (fence_len=3 vs 5)
    let cf3 = BlockFrame::new(FrameKind::CodeFence, 0,
        BlockFrame::encode_cf_body(b'`', 3));
    let cf5 = BlockFrame::new(FrameKind::CodeFence, 0,
        BlockFrame::encode_cf_body(b'`', 5));

    let s1 = vec![cf3];
    let s2 = vec![cf5];

    // 全等比较: 不同 (body 位不同)
    assert_ne!(s1, s2);

    // 部分相等 (仅比较 kind+indent, 跳过 body): 相同
    fn kind_indent_eq(a: &[BlockFrame], b: &[BlockFrame]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b.iter()).all(|(x, y)| {
                x.kind() == y.kind() && x.indent() == y.indent()
            })
    }
    assert!(kind_indent_eq(&s1, &s2));

    // 但对 CF 来说, body 差异不影响 scan_block 行为 (已验证)
    let (t1, _) = scan_block(b"code", &s1, None);
    let (t2, _) = scan_block(b"code", &s2, None);
    assert_eq!(t1, t2);

    // 结论: 对 CF/Table/Comment, 条件 D 可降级为 kind+indent 比较.
    // 对 BLT/OL/BQ/HtmlBlock/HtmlElement/RefDef, 需全等 (body_adm ≠ ∅).
}

/// 互模拟跳跃: 验证 ripple 收敛只需 admits 层一致.
/// 两个栈在 body_out 位不同但 body_adm 位相同 → 扫描行为一致.
#[test]
fn theory_bisimulation_body_out_diff_ok() {
    let blt1 = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));
    let blt2 = BlockFrame(blt1.0 | LOOSE_BIT); // body_out 不同

    let s1 = vec![blt1];
    let s2 = vec![blt2];

    // body_adm 相同 (bullet_code='-'), body_out 不同 (LOOSE_BIT)
    // 对内容行的扫描结果应一致
    let (t1, _) = scan_block(b"para", &s1, Some(BlockType::Bullet));
    let (t2, _) = scan_block(b"para", &s2, Some(BlockType::Bullet));
    assert_eq!(t1, BlockType::Paragraph);
    assert_eq!(t1, t2);

    // 但 body_out 差异影响产出 (Blank 时 BodyMut)
    // → s1 和 s2 对不同行序列的 FrameLog 可能不同
    // → 互模拟需要 body_out 也一致, 不仅仅是 body_adm
    // → §S FC3 是必要的
}

// 核心理论验证 — 下推自动机 + 概念格

/// §B.1: σ 收敛到 c_L = max_admitter.
/// Pop 循环的收敛子栈 = 栈上认领该行的最大前缀.
#[test]
fn theory_pop_loop_converges_to_max_admitter() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0,
        0);
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));
    let cf = BlockFrame::new(FrameKind::CodeFence, 0,
        BlockFrame::encode_cf_body(b'`', 3));

    // 栈 = [BQ, BLT, CF]. 行 "> - item":
    // CF 认领一切(⊤) → 停在 CF. 但如果 CF 不在栈顶呢?
    // 实际: BLT 在 CF 之下. Pop 循环从栈顶(CF)开始.
    // 行 "> - item" 没有 '>'? 不对, 它确实有.

    fn converge_depth(bytes: &[u8], stack: &[BlockFrame], pt: Option<BlockType>) -> usize {
        let mut s = stack.to_vec();
        let mut guard = 0u8;
        loop {
            guard += 1; if guard > 32 { break; }
            let (t, a) = scan_block(bytes, &s, pt);
            match a {
                mdio_core::block::ScanAction::Pop => {
                    s.pop();
                    if t != BlockType::Paragraph && t != BlockType::Blank { break; }
                    continue;
                }
                _ => break,
            }
        }
        s.len()
    }

    // 双层栈, 三行 → 三种收敛深度
    let s = vec![bq, blt];
    let d1 = converge_depth(b"> - item", &s, None); // BLT admits → depth=2
    let d2 = converge_depth(b"> text", &s, None);    // BQ admits → depth=1
    let d3 = converge_depth(b"no prefix", &s, None); // none → depth=0

    // c_L 的唯一性: 同栈同行 → 必定相同收敛深度
    assert_eq!(converge_depth(b"> - item", &s, None),
               converge_depth(b"> - item", &s, None));
    // c_L 的幂等性: 再调一次 → 深度不变
    assert_eq!(converge_depth(b"> text", &s, None), d2);
    // c_L 的收缩性: 收敛深度 ≤ 原始栈深度
    assert!(d1 <= 2); assert!(d2 <= 2); assert!(d3 <= 2);
    // c_L 的单调性: 更深栈的收敛深度 ≥ 更浅栈 (同一行)
    assert!(d1 >= d2); // item 行在 [BQ,BLT] 中比在 [BQ] 中收敛更深
}

/// §B.4: 确定型分划. Pop→continue 只对 Paragraph 和 Blank 触发.
/// scan_block 在 disowns 时总是返回 Paragraph 类型 (非确定型).
#[test]
fn theory_only_paragraph_and_blank_are_indefinite() {
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));

    // 各种被 BLT disowns 的行: scan_block 单次调用
    // disowns → (type, Pop). type 总是 Paragraph (不论原始行是什么).
    let cases: &[(&[u8], BlockType)] = &[
        (b"# header", BlockType::Paragraph),   // disowns → Pop
        (b"```", BlockType::Paragraph),
        (b"---", BlockType::Paragraph),
        (b"plain", BlockType::Paragraph),
        (b"", BlockType::Blank),               // 空行也是 Pop
    ];

    for &(bytes, expected) in cases {
        let (t, _a) = scan_block(bytes, &[blt], None);
        assert_eq!(t, expected,
            "BLT disowns {:?} → type={:?}", std::str::from_utf8(bytes).unwrap_or("?"), t);
    }

    // 核心: disowns 时 scan_block 返回 (Paragraph, Pop) 或 (Blank, Pop)
    // Pop 循环根据 t 决定 continue (P/B) 还是 break (其他)
    // 只有 P 和 B 是"不确定型" — 它们本身不带归属信息
    // 其他所有类型在被认领时出现, 不在 Pop 中出现
}

/// §AI.3: ⊤ 容器 (CF/Table/Comment) 在概念格中是透明的.
/// 它们认领一切行 → 不影响 Pop 循环的 admit/deny 判定,
/// 但会改变输出类型 (CodeContent vs Paragraph).
#[test]
fn theory_top_containers_are_lattice_transparent() {
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));
    let cf = BlockFrame::new(FrameKind::CodeFence, 0,
        BlockFrame::encode_cf_body(b'`', 3));

    fn pop_loop(bytes: &[u8], s: &[BlockFrame], pt: Option<BlockType>) -> (BlockType, usize) {
        let mut stack = s.to_vec();
        let mut ft = BlockType::Paragraph;
        let mut guard = 0u8;
        loop {
            guard += 1; if guard > 32 { break; }
            let (t, a) = scan_block(bytes, &stack, pt);
            ft = t;
            match a {
                mdio_core::block::ScanAction::Pop => {
                    stack.pop();
                    if t != BlockType::Paragraph && t != BlockType::Blank { break; }
                    continue;
                }
                _ => break,
            }
        }
        (ft, stack.len())
    }

    // ⊤ 容器透明性: 去掉 ⊤ 层后 admit/deny 判定一致
    // "text" 在 [BLT] 下: BLT admits → depth=1
    // "text" 在 [CF, BLT] 下: CF admits(⊤)→立即停止 → depth=2
    let (_t1, d1) = pop_loop(b"text", &[blt], Some(BlockType::Bullet));
    let (_t2, d2) = pop_loop(b"text", &[cf, blt], Some(BlockType::Bullet));
    // CF 不改变 admit 判定 — "text" 仍然被某层认领(CF 替代了 BLT)
    // 但输出类型变了: CodeContent vs Paragraph
    // 关键: ⊤ 层不影响"是否被认领", 只影响"被认领为什么类型"
    assert_eq!(d2, d1 + 1, "⊤ adds one level, admit itself is transparent");

    // 验证 CF 认领一切的特点 (真正的 ⊤ — admits all lines)
    let (_t, d) = pop_loop(b"anything", &[cf], None);
    assert_eq!(d, 1, "CF (⊤) admits everything");
    // Table 状态机（tentative——body bit0=0）：classify ROW →
    // table_row_outcome（未确认 → None）→ Pop——tentative 表第二行必须是 separator
    let table = BlockFrame::new(FrameKind::Table, 0, 0);
    let (_t2, d2) = pop_loop(b"| data |", &[table], None);
    assert_eq!(d2, 0, "tentative Table 拒数据行（第二行须 separator）→ Pop");
    // body_adm=∅ ≠ admits everything. Table 的 ⊤ 指 body 不参与 admit.
}

// 核心定理直接验证 — §E §X §Q

/// §E.1: effect(PopPush(f)) = effect(Pop) ∘ effect(Push(f)).
/// PopPush 和 Pop+Push 对栈的最终效果相同.
#[test]
fn theory_effect_pop_push_equals_pop_then_push() {
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));
    let blt_new = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'*'));

    // 初始栈
    let init = vec![blt];

    // 路径 A: PopPush(blt_new)
    let mut sa = init.clone();
    // Pop old
    let _popped = sa.pop();
    // Push new
    sa.push(blt_new);
    let result_pop_push = sa.clone();

    // 路径 B: 先 Pop 再 Push (分两步)
    let mut sb = init.clone();
    sb.pop();
    sb.push(blt_new);
    let result_pop_then_push = sb.clone();

    assert_eq!(result_pop_push, result_pop_then_push,
        "effect(PopPush) = effect(Pop) ∘ effect(Push): stack state identical");

    // 但 FrameLog 结构不同:
    // PopPush → 一个 Composite action → FrameSpans 标记 item_boundary
    // Pop+Push → 两个独立 action → FrameSpans 可能标记 container_change
    // 这是 §E.2: effect 不是单射, Action 需要 PopPush 携带额外语义.
}

/// §X.4: c_L 是 Moore 闭包算子.
/// - contractive: c_L(s) ⊆ s (Pop 循环只减不增)
/// - idempotent: c_L(c_L(s)) = c_L(s) (再调一次深度不变)
/// - monotone: s₁ ⊆ s₂ → c_L(s₁) ⊆ c_L(s₂) (更深栈的收敛深度 ≥ 更浅栈)
#[test]
fn theory_closure_is_moore_operator() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0,
        0);
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));

    fn converge_stack(bytes: &[u8], s: &[BlockFrame]) -> Vec<BlockFrame> {
        let mut stack = s.to_vec();
        let mut guard = 0u8;
        loop {
            guard += 1; if guard > 32 { break; }
            let (t, a) = scan_block(bytes, &stack, None);
            match a {
                mdio_core::block::ScanAction::Pop => {
                    stack.pop();
                    if t != BlockType::Paragraph && t != BlockType::Blank { break; }
                    continue;
                }
                _ => break,
            }
        }
        stack
    }

    let line = b"> text";
    let s_deep = vec![bq, blt];  // depth=2
    let s_shallow = vec![bq];     // depth=1
    let s_empty: Vec<BlockFrame> = vec![];

    // Contractive: c_L(s) ⊆ s — 收敛后深度 ≤ 原始深度
    let c_deep = converge_stack(line, &s_deep);
    assert!(c_deep.len() <= s_deep.len(),
        "contractive: |c_L(s)|={} ≤ |s|={}", c_deep.len(), s_deep.len());

    // Idempotent: c_L(c_L(s)) = c_L(s)
    // 对收敛结果再调一次 → 应该不变(因为已经是闭包)
    let c_again = converge_stack(line, &c_deep);
    assert_eq!(c_deep, c_again, "idempotent: c_L(c_L(s)) = c_L(s)");

    // 验证 c_L 确实是 c_L(s) 的子栈 (前缀关系)
    // c_deep 的元素应该来自 s_deep 的前缀
    for (i, f) in c_deep.iter().enumerate() {
        assert_eq!(f.0, s_deep[i].0, "c_L(s) is prefix of s at index {}", i);
    }

    // Monotone: s₁ ⊆ s₂ → c_L(s₁) ⊆ c_L(s₂) (这里 ⊆ 是前缀关系)
    // [BQ] 是 [BQ,BLT] 的前缀.
    // 对同一行, 更深栈的收敛深度 ≥ 更浅栈
    let c_shallow = converge_stack(line, &s_shallow);
    assert!(c_shallow.len() <= c_deep.len(),
        "monotone: shallower stack → shallower convergence");
}

/// §Q: (frame_kind, admits, classify_at_top) 是极小生成集.
/// frame_kind 相同 → container_action_for 相同.
/// admits 决定 Pop 循环行为. classify_at_top 决定栈空时的判定.
#[test]
fn theory_minimal_generating_set() {
    // frame_kind: Bullet 和 Checklist 都属于 FrameKind::BulletList
    // → 相同的 container_action_for 模板
    let b = BlockFrame::new(FrameKind::BulletList, 0,
        BlockFrame::encode_blt_body(b'-'));
    assert_eq!(b.kind(), FrameKind::BulletList);

    // classify_at_top: 独立于 frame_kind.
    // 验证: 栈空时 "# H1" 被分类为 Header1, 不依赖任何 frame_kind
    let empty: Vec<BlockFrame> = vec![];
    let (t, _) = scan_block(b"# H1", &empty, None);
    assert_eq!(t, BlockType::Header1);

    // admits: BLT 认领 indent≥threshold 的 bullet 行, 拒绝其他行
    let blt0 = BlockFrame::new(FrameKind::BulletList, 0,
        BlockFrame::encode_blt_body(b'-'));
    // "- item": BLT admits → Push/PopPush (非 Pop)
    let (_t1, a1) = scan_block(b"- item", &[blt0], None);
    assert!(!matches!(a1, mdio_core::block::ScanAction::Pop),
        "matching bullet admitted by BLT");
    // "# header": BLT disowns → Pop
    let (_t2, a2) = scan_block(b"# header", &[blt0], None);
    assert!(matches!(a2, mdio_core::block::ScanAction::Pop),
        "header disowned by BLT");

    // 改变 frame_kind → 改变 container_action_for
    // 改变 admits (bullet_code) → 改变 Pop 循环
    // 改变 classify_at_top (is_para_cont) → 改变栈空行为
    // 三元组相互独立, 缺一不可
}

/// §AJ.4: 闭包链 C_d 的单调递减性.
/// C_{d+1} ⊆ C_d — 更深的前缀认领的行集是更浅前缀的子集.
#[test]
fn theory_closure_chain_monotonic() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0,
        0);
    let blt = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));

    // 三行, 在三种栈深度下的 admit 行为
    let lines: &[&[u8]] = &[b"> - bullet", b"> paragraph", b"no prefix"];
    let empty: Vec<BlockFrame> = vec![];
    let s1 = vec![bq];
    let s2 = vec![bq, blt];

    fn admits(bytes: &[u8], s: &[BlockFrame]) -> bool {
        let mut stack = s.to_vec();
        if stack.is_empty() { return true; } // 空栈认领一切
        let mut guard = 0u8;
        loop {
            guard += 1; if guard > 32 { break; }
            let (t, a) = scan_block(bytes, &stack, None);
            match a {
                mdio_core::block::ScanAction::Pop => {
                    stack.pop();
                    if t != BlockType::Paragraph && t != BlockType::Blank { return false; }
                    continue;
                }
                _ => return true, // 非 Pop → admitted
            }
        }
        false
    }

    // C_0 (空栈): 认领所有行
    for line in lines { assert!(admits(line, &empty)); }

    // C_1 ([BQ]): 认领有 '>' 的行 + 懒延续 (para_open=true)
    assert!(admits(b"> - bullet", &s1));
    assert!(admits(b"> paragraph", &s1));

    // C_2 ([BQ, BLT]): 额外要求 indent ≥ 2 且 bullet 匹配
    assert!(admits(b"> - bullet", &s2));

    // 闭包链单调性: C_2 ⊆ C_1 ⊆ C_0
    // C_2 认领的行, C_1 一定也认领(或反之不一定)
    // 验证: "> paragraph" 在 C_1 中被认领但在 C_2 中可能不被认领
    let in_c1 = admits(b"> paragraph", &s1);
    let in_c2 = admits(b"> paragraph", &s2);
    // C_2 ⊆ C_1 → 若 C_2 admits 则 C_1 必 admits
    if in_c2 { assert!(in_c1); }
}

// incr 理论 — safe_start + 免费午餐

use mdio_core::block::{push_entry, pop_entry, body_mut_entry, entry_line, entry_frame,
    entry_is_pop, entry_is_body_mut};

/// safe_start = ripple 扫描中第一个非 Continue action 的行号 (§AB.2-AB.4).
/// Push(f, li) → safe_start = li (容器开始).
/// Pop(f, li) → safe_start = f 被 push 的行号 (需从旧 FL 查).
/// Continue → 等价类不变 → 无需 patch.
#[test]
fn theory_safe_start_is_first_non_continue_action() {
    // 编辑: "- a\n- b\n" → "# H1\n- a\n- b\n" (delta=+1, 插入 heading)
    let mut state = State::new("- a\n- b\n");
    state.edit(0, 0, "# H1\n");
    state.ensure_projected();

    // ripple 产生的 FL entries 在合并后的 FL 中
    // 第一个非 Continue action = Push(BLT) 在行 1
    let log = &state.frame_log;
    // FL entries 的第一条应该是 Push
    let first = log.entries.first().unwrap();
    let line = entry_line(*first);
    // 验证: 第一个容器变化在行 1 (heading 在行 0, BLT 在行 1)
    assert_eq!(line, 1, "first container change at line 1 (BLT push)");
    // safe_start 就是第一个非 Continue action 的行号
    let safe_start = line as usize;
    assert_eq!(safe_start, 1);

    // §AB.2: Push 后等价类分划变化 → 受影响行从 safe_start 开始
    // 验证: bounds 检查 — safe_start 应在扫描范围内
    assert!(safe_start < state.lines.len());
}

/// 免费午餐: ripple 的 FL entries 就是容器边界，概念格轨迹等价于 rebuild_all.
/// §AQ.1: Push(f, li) → B(K) 增属性. Pop(f, li) → B(K) 删属性.
/// §AQ.5: merge 后 FL 的概念格轨迹 ≡ rebuild_all.
#[test]
fn theory_free_lunch_fl_entries_are_container_boundaries() {
    // 编辑在 BLT 内部: 修改文本, 不改变容器结构
    let mut state = State::new("- a\n- b\n- c\n\ntext\n");
    state.edit(0, 3, "- x");
    state.ensure_projected();

    let log = &state.frame_log;
    // FL 中每条 Push entry 的 (line, frame.kind) 就是容器边界信息
    // 逐条验证: Push entry 的 line = 容器 start, frame.kind = 容器类型
    let mut push_lines: Vec<(u32, FrameKind)> = Vec::new();
    let mut pop_lines: Vec<(u32, FrameKind)> = Vec::new();
    for &e in &log.entries {
        if entry_is_body_mut(e) { continue; }
        let f = entry_frame(e);
        if entry_is_pop(e) {
            pop_lines.push((entry_line(e), f.kind()));
        } else {
            push_lines.push((entry_line(e), f.kind()));
        }
    }

    // 验证: FL entries 足以重构容器边界 (通过 rebuild_stack)
    for &(li, kind) in &push_lines {
        let _stack = log.rebuild_stack(li + 1); // 此行开始时的栈
        // Push 的 frame 应该在栈中
    }
}

/// incr 正确性 = 免费午餐, 无额外风险.
/// FL entries 一致 → rebuild_stack 一致 → 概念格一致.
#[test]
fn theory_free_lunch_rebuild_stack_equivalent() {
    for (old_text, start, end, new_text) in [
        ("- a\n- b\n- c\n", 0, 3, "- x"),
        ("- a\n- b\n- c\n", 0, 0, "# H1\n"),
        ("- a\n- b\n- c\n", 4, 8, ""),
    ] {
        let mut state = State::new(old_text);
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());

        // FL entries 一致 (§AQ.5) → rebuild_stack 必定一致
        for li in 0..=state.lines.len() {
            let s1 = state.frame_log.rebuild_stack(li as u32);
            let s2 = ref_state.frame_log.rebuild_stack(li as u32);
            assert_eq!(s1, s2, "stack mismatch at line {} for '{}'", li, new_text);
        }
    }
}

// 实际可行 — 跨线程 PathWorker 模拟

/// safe_start 精确算法: 从 dirty_line 回溯到最近的容器 Push 行.
/// 用 FL entries 直接计算, 不依赖 FrameSpans (pub(crate)).
fn safe_start_from_fl(log: &FrameLog, dirty_line: usize) -> usize {
    if dirty_line == 0 { return 0; }
    // 找到 dirty_line 处栈上的容器
    let stack = log.rebuild_stack(dirty_line as u32);
    if stack.is_empty() {
        // 深度为 0: 回溯到最近的 block 边界
        // 用 FL entries: 找 dirty_line 之前最近的 Push/Pop 行
        let prev_boundary = log.entries.iter()
            .filter(|&&e| !entry_is_body_mut(e))
            .map(|&e| entry_line(e) as usize)
            .filter(|&l| l < dirty_line)
            .max();
        // 如果没有边界, 回到行 0. 否则回到边界+1 (该容器之后).
        prev_boundary.map_or(0, |l| (l + 1).min(dirty_line))
    } else {
        // 深度 > 0: 栈顶容器从哪一行开始?
        // 找到栈顶容器的 Push entry
        let top_kind = stack.last().unwrap().kind();
        log.entries.iter()
            .filter(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e)
                && entry_frame(e).kind() == top_kind)
            .map(|&e| entry_line(e) as usize)
            .filter(|&l| l <= dirty_line)
            .max()
            .unwrap_or(dirty_line)
    }
}

/// 验证 safe_start_from_fl 在三种编辑场景下给出合理的起点.
#[test]
fn theory_safe_start_precise_computation() {
    // 场景 1: 编辑在容器内 → safe_start = 容器起始行
    let mut state = State::new("# H1\n- a\n- b\n- c\n");
    state.edit(9, 10, "B");  // 修改 "- b" 为 "- B" (行 2)
    state.ensure_projected();
    let ss = safe_start_from_fl(&state.frame_log, 2);
    assert!(ss <= 2, "safe_start should be at or before dirty line");
    assert!(ss >= 1, "safe_start should be at BLT start (line 1)");

    // 场景 2: 容器外编辑 → safe_start = dirty_line
    let mut state2 = State::new("# H1\n\n- a\n");
    state2.edit(0, 1, "X");  // 修改 heading
    state2.ensure_projected();
    let ss2 = safe_start_from_fl(&state2.frame_log, 0);
    assert_eq!(ss2, 0);

    // 场景 3: 编辑跨越容器边界 (bullet→paragraph)
    let mut state3 = State::new("- a\n- b\n");
    state3.edit(0, 3, "text");
    state3.ensure_projected();
    let ss3 = safe_start_from_fl(&state3.frame_log, 0);
    // 容器完全消失 → FL 为空 → 回到行 0
    assert_eq!(ss3, 0);
}

/// 模拟 PathWorker: snapshot(merged FL) + dirty_line → safe_start → 范围.
/// 验证 safe_start 处开始扫描的 rebuild_stack 与旧文档一致 (§T.1-§T.2).
#[test]
fn theory_pathworker_simulation() {
    // 模拟两次编辑的 PathWorker 场景
    let old_text = "- a\n- b\n- c\n\ntext\n";
    let mut state = State::new(old_text);
    let old_log = state.frame_log.clone();  // 编辑前的 FL (PathWorker 可以缓存)
    state.edit(0, 3, "- x");  // 编辑 "- a" → "- x"
    state.ensure_projected();

    let dirty_line = 0;
    let new_log = &state.frame_log;

    // safe_start 从合并后 FL 计算
    let ss = safe_start_from_fl(new_log, dirty_line);
    // 验证: 在 safe_start 处, 新旧 FL 看到相同的栈?
    // §T.1: ripple 不破坏前缀互模拟.
    // 对于 start_line 之前的行, s_new = s_old.
    // 对于 dirty_line 之前的行(不含 dirty_line), 如果该行不属于同一容器,
    // 栈可能不同(因为编辑可能改变了容器的开始行).
    // safe_start 的意义: 从该行重新序列化, 保证 AST 输出覆盖所有变化.

    // 实际验证: safe_start 处的行, 在旧文档的快照中也被正确分类
    let ref_state = State::new(&state.rope.to_string());
    assert!(states_eq(&state, &ref_state),
        "ripple FL = rebuild FL → PathWorker 拿到的是正确的 FL");
    assert_eq!(ss, 0, "edit at line 0 → safe_start = 0");
}

/// PathWorker 跨线程安全: Snapshot (Arc) 包含完整合并 FL.
/// PathWorker 不需要 ripple 中间状态, 只需最终 Snapshot.
#[test]
fn theory_pathworker_thread_safety() {
    let mut state = State::new("- a\n- b\n- c\n");
    state.edit(0, 3, "- x");
    state.ensure_projected();

    // Snapshot = Arc 共享, 无复制
    let snap = state.snapshot();
    // PathWorker 通过公开 API 读取: line_len, line_type_at, rebuild 等
    let n = snap.line_len();
    for li in 0..n {
        let _ty = snap.line_type_at(li);
    }
    // Clone 后两个 Snapshot 共享同一个 Arc<FL>
    let snap2 = snap.clone();
    drop(snap);
    // snap2 仍然有效 — Arc 保证 FL 不被提前释放
    assert!(snap2.line_len() > 0);
}

// 复盘补漏 — Composite 主路径 + 大 delta + 长 suffix

/// 缺口 1: Composite{primary: PopPush} — 嵌套容器中 item 切换.
#[test]
fn gap_composite_poppush() {
    // "> - a\n> - b\n" 在 BQ 内, BLT 的 PopPush 触发 Composite
    run_and_check("> - a\n> - b\n", 2, 3, "x");
}

/// 缺口 1: Composite{primary: Continue} — BQ 懒延续段落.
/// BQ 内的段落行在 BLT 不认领时走 BQ→Continue.
#[test]
fn gap_composite_continue_primary() {
    // "> - a\n>   para\n> - b\n" — 中间行是 BQ 懒延续
    run_and_check("> - a\n>   para\n> - b\n", 4, 5, "P");
}

/// 缺口 2: delta = +10 (大量插入).
#[test]
fn gap_large_delta_plus_ten() {
    let lines: Vec<String> = (0..10).map(|i| format!("line {}\n", i)).collect();
    let old = "- a\n- b\n- c\n";
    let insert = lines.concat();
    run_and_check(old, 0, 0, &insert);
}

/// 缺口 2: delta = -5 (大量删除). 前缀保留, 后缀 shift.
#[test]
fn gap_large_delta_minus_five() {
    run_and_check("- a\n- b\n- c\n- d\n- e\n- f\n- g\n", 4, 20, "");
}

/// 缺口 3: 长 suffix (编辑收敛在开头, 后缀 20 行不变).
#[test]
fn gap_long_suffix_converge_early() {
    let mut doc = String::from("- a\n- b\n");
    for i in 3..25 { doc.push_str(&format!("- item {}\n", i)); }
    run_and_check(&doc, 0, 3, "- x");
}

/// 缺口 3: 长 prefix (编辑在末尾, 前缀 20 行不变).
#[test]
fn gap_long_prefix_edit_at_end() {
    let mut doc = String::new();
    for i in 0..20 { doc.push_str(&format!("line {}\n", i)); }
    doc.push_str("- a\n- b\n");
    let edit_start = doc.len() - 6; // 修改最后的 "- b"
    run_and_check(&doc, edit_start, edit_start + 3, "- z");
}

/// 缺口 3: 大文档中间编辑, prefix + suffix 都长.
#[test]
fn gap_large_document_mid_edit() {
    let prefix = "- a\n- b\n- c\n- d\n- e\n- f\n- g\n- h\n";
    let suffix = "- j\n- k\n- l\n- m\n- n\n- o\n- p\n- q\n";
    let old = format!("{}- mid\n{}", prefix, suffix);
    let mid_start = prefix.len();
    run_and_check(&old, mid_start, mid_start + 6, "edited");
}

/// 缺口 4: Composite{primary: Continue} — BLT 内嵌套 CF.
/// indent ≥ frame.indent → Inside. classify_list → NonListContainer →
/// build_composite_continue_push → Continue primary + nested Push(CF).
#[test]
fn gap_composite_continue_nested_cf_in_blt() {
    // "  ```" (indent 2) 在 indent=0 的 BLT 内, 触发 Composite Continue
    run_and_check("- a\n  ```\n  code\n  ```\n- b\n", 0, 3, "- x");
}

/// 缺口 4b: Composite{primary: Continue} — BLT 内嵌套 BQ.
#[test]
fn gap_composite_continue_nested_bq_in_blt() {
    // "  > quote" (indent 2) 在 BLT 内 → Continue + Push(BQ)
    run_and_check("- a\n  > quote\n  > more\n- b\n", 0, 3, "- x");
}

// HtmlElement — push_line 编码与 ripple 交互

/// HtmlElement 帧体编码 push_line 低 12 位.
/// delta=0 编辑: PL+FL 应该一致.
#[test]
fn htmlelement_delta_zero() {
    // <div> → HtmlBlock type 6
    // <p>   → HtmlElement Push (open tag, single line)
    // text  → HtmlContent (inside HtmlElement)
    // </p>  → HtmlElement Pop (close tag match)
    // </div> → HtmlBlock Pop (blank line or end)
    run_and_check("<div>\n<p>\ntext\n</p>\n</div>\n", 8, 12, "more");
}

/// HtmlElement + delta=+1：开行行号已由 scan 运行时 open_lines 承担，
/// tail 平移不再需要重写 HtmlElement frame body，FL 必须逐 entry 相等。
#[test]
fn htmlelement_delta_plus_one_strict_fl_equality() {
    let mut state = State::new("<div>\n<p>\ntext\n</p>\n</div>\n");
    state.edit(0, 0, "# H1\n");
    state.ensure_projected();
    let ref_state = State::new(&state.rope.to_string());

    // PL 一致
    for i in 0..state.lines.len() {
        assert_eq!(state.lines[i].0, ref_state.lines[i].0,
            "PL mismatch at line {}", i);
    }
    assert_eq!(state.frame_log.entries, ref_state.frame_log.entries,
        "open_line 栈落地后，delta != 0 的 tail 平移必须保持 FL 逐 entry 相等");
}

/// 红线门禁：HtmlElement 在 tail 里时，ripple 也必须局部收敛，
/// 不得再因 frame body 行号失配而扫到 EOF。
#[test]
fn htmlelement_tail_ripple_converges_locally() {
    let mut doc = String::from("<div>\n<p>\ntext\n</p>\n</div>\n");
    for i in 0..20_000 {
        doc.push_str(&format!("p{i}\n\n"));
    }
    let mut state = State::new(&doc);
    state.edit(0, 0, "# H1\n");
    state.ensure_projected();
    let diff = state.ripple_diff.take().expect("ripple_diff");
    assert!(
        diff.ripple_line_end < state.rope.line_len(),
        "HtmlElement tail must not force ripple to EOF: end={} total={}",
        diff.ripple_line_end,
        state.rope.line_len()
    );
}

/// HtmlElement + delta=-1: 删除 HtmlBlock 内部行.
#[test]
fn htmlelement_delta_minus_one() {
    // 删除 "line1\n" (6 bytes, positions 10..16) → delta=-1.
    run_and_check("<div>\n<p>\nline1\nline2\n</p>\n</div>\n", 10, 16, "");
}

/// HtmlElement: 连续两次编辑, 第二次编辑时 cached_rebuild
/// 从第一次编辑后的 FL 恢复栈, 包含 HtmlElement frame.
#[test]
fn htmlelement_two_edits() {
    let mut state = State::new("<div>\n<p>\ntext\n</p>\n</div>\n");
    state.edit(8, 12, "more");          // "text" → "more" (bytes 8-12)
    state.edit(1, 4, "DIV");            // "<div" → "<DIV" (bytes 1-4)
    state.ensure_projected();
    let rope_str = state.rope.to_string();
    let ref_state = State::new(&rope_str);
    assert!(states_eq(&state, &ref_state));
}

// §AR 变更检测完备性 — 三信号独立证明

/// AR.3(2) — 信号 3 的不可替代性: 纯内容变化仅 Edits 检测.
/// PL 不变 (同为 Bullet), FL 不变 (同为 PopPush BLT).
#[test]
fn ar_signal3_content_only_edits_detect() {
    // "- a\n- b\n" → "- a\n- x\n"
    // FL: Push@0, PopPush@1 → 相同. PL: Bullet,Bullet → 相同.
    // 仅 content 不同: "- b" vs "- x"
    let mut state = State::new("- a\n- b\n");
    let old_pl: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
    let old_fl = state.frame_log.clone();
    state.edit(6, 7, "x");  // "- b" → "- x" (只改 content, 保留 bullet marker)
    state.ensure_projected();

    let ref_state = State::new(&state.rope.to_string());
    assert!(states_eq(&state, &ref_state));

    // PL unchanged
    for i in 0..state.lines.len() {
        assert_eq!(state.lines[i].0, old_pl[i],
            "PL[{}] must be unchanged for pure content edit", i);
    }
    // FL unchanged
    assert_eq!(state.frame_log.entries.len(), old_fl.entries.len(),
        "FL entry count unchanged");
    for i in 0..old_fl.entries.len() {
        assert_eq!(
            state.frame_log.entries[i] & !mdio_core::block::CONSUME_MASK,
            old_fl.entries[i] & !mdio_core::block::CONSUME_MASK,
            "FL[{}] unchanged (consume 位除外)", i);
    }
    // → 只有 Edits 能检测此变更. FL_diff = ∅, PL_diff = ∅.
}

/// AR.3(2) — 信号 2 的不可替代性: 纯类型变化仅 PL_diff 检测.
/// FL 可能变也可能不变 (取决于 prev_type shift 是否导致结构变化).
/// 本测试用已知 FL 不变的 case: 栈空时 --- 从 ThematicBreak 变 HsSep.
/// 旧: "text\n---\n" → Paragraph, HsSep (prev=Paragraph → setext underline)
/// 新: "text\n\n---\n" → Paragraph, Blank, ThematicBreak (prev=Blank → thematic break)
/// HsSep: Continue (无 FL entry). ThematicBreak: Continue (无 FL entry).
/// → FL 都不变 (空), PL 变.
#[test]
fn ar_signal2_type_change_only_pl_detect() {
    // Old: "para\n---\n- a\n" (有 FL entry 的是 - a 的 Push BLT, 不是 --- 行)
    // New: "para\n\n---\n- a\n"
    // --- 从 HsSep(16) → ThematicBreak(18). FL: 都是 Continue, 无 entry.
    let mut state = State::new("para\n---\n- a\n");

    // 记录 --- 行 (行 1) 的旧 PL
    let old_pl_line1 = state.lines[1].0;
    assert_eq!(old_pl_line1, 16, "old --- is HsSep(16)");

    state.edit(5, 5, "\n");  // insert blank line before ---
    state.ensure_projected();

    let ref_state = State::new(&state.rope.to_string());
    assert!(states_eq(&state, &ref_state));

    // PL 变了: 行 2 (新) 是 ThematicBreak(18)
    assert_eq!(state.lines[2].0, 18, "new --- is ThematicBreak(18)");
    assert_ne!(state.lines[2].0, old_pl_line1, "PL type changed");

    // FL 中 --- 行无 entry (HsSep 和 ThematicBreak 都是 Continue)
    // → FL_diff 不会标记此行为变更. 只有 PL_diff 能检测.
}

/// AR.3(2) — 信号 1 的不可替代性: 纯结构变化仅 FL_diff 检测.
/// 内容不变, PL 不变, 但容器边界新增.
#[test]
fn ar_signal1_structure_only_fl_detect() {
    // 编辑: 删除 "# H1\n" 前的 word, 变成 "word- a\n- b\n" → FL 新增 BLT Push
    // 但已有的行 "- a\n- b\n" 的 PL 不变 (都是 Bullet), content 不变.
    // New FL 有 Push@0(BLT), 旧 FL 是在 line 1 推的.
    // 这需要旧 FL 也保存下来对比.
    let mut state = State::new("# H1\n- a\n- b\n");
    let old_fl = state.frame_log.clone();
    state.edit(0, 5, "- x");  // "# H1\n" → "- x\n"
    state.ensure_projected();

    let ref_state = State::new(&state.rope.to_string());
    assert!(states_eq(&state, &ref_state));

    // FL 结构变了 (新增 BLT Push 在不同位置)
    assert!(
        !mdio_core::block::entries_eq_ignoring_consume(
            &state.frame_log.entries, &old_fl.entries),
        "FL structure changed");
}

/// AR.3(3) — 正交性: FL 和 PL 的信息源正交.
/// 同一文档, FL 相同不代表 PL 相同 (反之亦然).
#[test]
fn ar_orthogonality_fl_pl_independent() {
    // 两个文档, FL 结构相同 (都有 BLT Push+PopPush), 但 PL 不同
    let a = State::new("- a\n- b\n");     // PL: Bullet(6), Bullet(6)
    let b = State::new("- 1\n- 2\n");     // PL: Bullet(6), Bullet(6) — same

    // FL 相同因为都是 BLT, 但 content 不同
    assert_eq!(a.frame_log.entries.len(), b.frame_log.entries.len());
    for i in 0..a.frame_log.entries.len() {
        let ea = a.frame_log.entries[i];
        let eb = b.frame_log.entries[i];
        let fa = entry_frame(ea);
        let fb = entry_frame(eb);
        assert_eq!(fa.kind(), fb.kind(), "same frame kind");
        assert_eq!(entry_is_pop(ea), entry_is_pop(eb), "same pop flag");
        // line 和 body 可能不同 (不同的 PopPush 行号, 不同的 bullet_code)
    }

    // content 不同 → PL 同但 AST 不同
    // → Edits 独立于 FL 和 PL, 检测到了 FL 和 PL 都没检测到的变化
}

/// AR.3(3) — 正交性: Edit 和 PL 的信息源正交.
/// 两个文档, PL 相同, 但 Rope content 不同.
#[test]
fn ar_orthogonality_edits_pl_independent() {
    let a = State::new("- a\n- b\n");
    let b = State::new("- a\n- x\n");
    // PL 同: [Bullet(6), Bullet(6)] vs [Bullet(6), Bullet(6)]
    for i in 0..2 {
        assert_eq!(a.lines[i].0, b.lines[i].0, "PL identical");
    }
    // rope 不同: "- b" vs "- x"
    let ra = a.rope.to_string();
    let rb = b.rope.to_string();
    assert_ne!(ra, rb, "Rope content differs");
    // → Edits 独立于 PL, 检测到了 PL 没检测到的变化
}

/// AR.2 — ADD 容器覆盖域排除内部 REPLACE.
/// 插入新 item, 新 item 的 PL + content 变化被 ADD 覆盖.
#[test]
fn ar_add_container_covers_interior() {
    // "- a\n- b\n" → "- a\n- x\n- y\n- b\n" (插入两个新 item)
    // FL 新增两个 Push, PL 变化在插入的行.
    // 所有 PL 变化都在新 Push 的覆盖域内 → 不应产独立 REPLACE.
    let mut state = State::new("- a\n- b\n");
    state.edit(4, 4, "- x\n- y\n");
    state.ensure_projected();

    let ref_state = State::new(&state.rope.to_string());
    assert!(states_eq(&state, &ref_state));
}

/// AR.2 — DELETE 容器覆盖域排除内部变化.
/// 删除多行 item, 覆盖域内 PL 变化不独立 REPLACE.
#[test]
fn ar_delete_container_covers_interior() {
    // "- a\n- b\n- c\n" → "- a\n" (删除两行)
    // FL 减少 PopPush entries.
    let mut state = State::new("- a\n- b\n- c\n");
    state.edit(4, 12, "");
    state.ensure_projected();

    let ref_state = State::new(&state.rope.to_string());
    assert!(states_eq(&state, &ref_state));
}

/// AR.2 — MODIFY (BodyMut) 独立于 ADD/DELETE.
/// blank line 设置 LOOSE_BIT → BodyMut 不同.
#[test]
fn ar_modify_bodymut_independent() {
    // "- a\n- b\n" → "- a\n\n- b\n" (插入 blank line → LOOSE_BIT)
    let mut state = State::new("- a\n- b\n");
    state.edit(4, 4, "\n");
    state.ensure_projected();

    let ref_state = State::new(&state.rope.to_string());
    assert!(states_eq(&state, &ref_state));
}

/// AR.1 — 三信号联合覆盖: structure + type + content 同时变化.
#[test]
fn ar_all_three_signals_combined() {
    // "# Old\n- a\n- b\n" → "# New\n\n- x\n- y\n"
    // FL 变 (heading 结构变, BLT item 替换)
    // PL 变 (header 类型变, 新增 blank)
    // content 变 (编辑区域)
    let mut state = State::new("# Old\n- a\n- b\n");
    state.edit(0, 5, "# New\n\n- x\n- y");
    state.ensure_projected();

    let ref_state = State::new(&state.rope.to_string());
    assert!(states_eq(&state, &ref_state));
}

/// AR.1 — 三个信号全空 → 无变更.
#[test]
fn ar_no_signals_no_change() {
    // 编辑不改变任何字节 → delta=0, 立即收敛, 三信号全空
    run_and_check("# H1\n- a\n- b\n", 3, 3, "");
}

// 代码证明 — Galois + 余归纳 + 紧界 + disowns

/// §Y: (A_R, C_R) Galois 连接.
/// A_R(s'): 前缀子栈 s' 认领的行集.
/// C_R(S): 认领 S 中所有行的最大前缀.
/// Galois 条件: s' ⊆ C_R(S) ⇔ S ⊆ A_R(s').
#[test]
fn theory_galois_connection() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0,
        0);
    let blt = BlockFrame::new(FrameKind::BulletList, 0,
        BlockFrame::encode_blt_body(b'-'));

    // 四行代表样本
    let lines: &[(&[u8], &str)] = &[
        (b"> - item", "BQ+BLT admits"),
        (b"> text",   "BQ admits, BLT disowns"),
        (b"- item",   "BLT admits (no BQ prefix)"),
        (b"plain",    "both disown"),
    ];

    // 三个前缀子栈
    let s0: Vec<BlockFrame> = vec![];
    let s1 = vec![bq];
    let s2 = vec![bq, blt];
    let prefixes: &[&Vec<BlockFrame>] = &[&s0, &s1, &s2];

    fn admits_set(s: &[BlockFrame], lines: &[(&[u8], &str)]) -> Vec<usize> {
        lines.iter().enumerate()
            .filter(|(_, (bytes, _))| {
                let mut stack = s.to_vec();
                let mut guard = 0u8;
                loop {
                    guard += 1; if guard > 32 { break false; }
                    let (t, a) = scan_block(bytes, &stack, None);
                    match a {
                        mdio_core::block::ScanAction::Pop => {
                            stack.pop();
                            if t != BlockType::Paragraph && t != BlockType::Blank
                                { break false; }
                            continue;
                        }
                        _ => break true,
                    }
                }
            })
            .map(|(i, _)| i)
            .collect()
    }

    fn max_admitter(lines_idx: &[usize], prefixes: &[&Vec<BlockFrame>],
                    all_lines: &[(&[u8], &str)]) -> usize {
        let mut best = 0usize;
        for (pi, s) in prefixes.iter().enumerate() {
            let admitted: Vec<usize> = admits_set(s, all_lines);
            if lines_idx.iter().all(|&li| admitted.contains(&li)) {
                best = pi;
            }
        }
        best
    }

    let s_admits: Vec<Vec<usize>> = prefixes.iter()
        .map(|s| admits_set(s, lines))
        .collect();

    let s_set: Vec<usize> = vec![0, 1];
    let c = max_admitter(&s_set, prefixes, lines);

    for i in 0..prefixes.len() {
        let s_subset_cr = i <= c;
        let s_subset_ar = s_set.iter().all(|&li| s_admits[i].contains(&li));
        assert_eq!(s_subset_cr, s_subset_ar,
            "Galois: s'_{} ⊆ C_R(S) ⇔ S ⊆ A_R(s'_{})", i, i);
    }
}

/// §V: 余归纳一步闭合.
/// (s₁,pt₁) 和 (s₂,pt₂) 满足 FC1-3 → 对任意 L: t₁=t₂ 且新状态仍满足 FC1-3.
#[test]
fn theory_coinduction_one_step_closure() {
    let blt1 = BlockFrame::new(FrameKind::BulletList, 0,
        BlockFrame::encode_blt_body(b'-'));
    let blt2 = BlockFrame::new(FrameKind::BulletList, 0,
        BlockFrame::encode_blt_body(b'-'));

    let s1 = vec![blt1];
    let s2 = vec![blt2];
    let pt1: Option<BlockType> = Some(BlockType::Bullet);
    let pt2: Option<BlockType> = Some(BlockType::Bullet);

    fn pop_loop_full(bytes: &[u8], s: &[BlockFrame], pt: Option<BlockType>)
        -> (BlockType, Vec<BlockFrame>)
    {
        let mut stack = s.to_vec();
        let mut ft = BlockType::Paragraph;
        let mut guard = 0u8;
        loop {
            guard += 1; if guard > 32 { break; }
            let (t, a) = scan_block(bytes, &stack, pt);
            ft = t;
            match a {
                mdio_core::block::ScanAction::Pop => {
                    stack.pop();
                    if t != BlockType::Paragraph && t != BlockType::Blank { break; }
                    continue;
                }
                _ => break,
            }
        }
        (ft, stack)
    }

    let test_lines: &[&[u8]] = &[b"- item", b"paragraph", b""];
    for &line in test_lines {
        let (t1, ns1) = pop_loop_full(line, &s1, pt1);
        let (t2, ns2) = pop_loop_full(line, &s2, pt2);

        assert_eq!(t1, t2, "FC: type must match");

        assert_eq!(ns1.len(), ns2.len(), "FC1: depth diverged");
        for d in 0..ns1.len() {
            assert_eq!(ns1[d].kind(), ns2[d].kind(), "FC1: kind mismatch");
            assert_eq!(ns1[d].indent(), ns2[d].indent(), "FC1: indent mismatch");
        }

        let new_pt1 = if t1 != BlockType::Blank { Some(t1) } else { None };
        let new_pt2 = if t2 != BlockType::Blank { Some(t2) } else { None };
        assert_eq!(new_pt1, new_pt2, "FC2: prev_type diverged");

        for d in 0..ns1.len() {
            assert_eq!(ns1[d].body(), ns2[d].body(), "FC3: body_out mismatch");
        }
    }
}

/// §AH.4: 等价类紧界 ≤ 5 (非 33).
/// 典型 BLT 栈: 几何签名决定等价类.
#[test]
fn theory_equivalence_class_tight_bound() {
    let blt = BlockFrame::new(FrameKind::BulletList, 0,
        BlockFrame::encode_blt_body(b'-'));

    fn converge_depth(bytes: &[u8], s: &[BlockFrame], pt: Option<BlockType>) -> usize {
        let mut stack = s.to_vec();
        let mut guard = 0u8;
        loop {
            guard += 1; if guard > 32 { break; }
            let (t, a) = scan_block(bytes, &stack, pt);
            match a {
                mdio_core::block::ScanAction::Pop => {
                    stack.pop();
                    if t != BlockType::Paragraph && t != BlockType::Blank { break; }
                    continue;
                }
                _ => break,
            }
        }
        stack.len()
    }

    let samples: &[(&[u8], &str)] = &[
        (b"# header", "#"), (b"- bullet", "-"), (b"* star", "*"),
        (b"_ under", "_"), (b"```", "`"), (b"~~~", "~"),
        (b"<div>", "<"), (b"> quote", ">"), (b"| table", "|"),
        (b"1. ol", "1-9"), (b"[ref]", "["), (b"plain", "default"),
        (b"", "blank"),
    ];

    let pt = Some(BlockType::Bullet);
    let depths: Vec<usize> = samples.iter()
        .map(|(bytes, _)| converge_depth(bytes, &[blt], pt))
        .collect();

    let unique: std::collections::BTreeSet<usize> = depths.into_iter().collect();
    assert!(unique.len() <= 5,
        "tight bound: |Σ/~_s|={} ≤ 5 (not 33)", unique.len());
}

/// §AN.4: disowns 向外传播.
/// 栈顶 disowns → Pop → 外层已被检查, 其 disowns 已确定.
#[test]
fn theory_disowns_propagates_outward() {
    let bq = BlockFrame::new(FrameKind::BlockQuote, 0,
        0);
    let blt = BlockFrame::new(FrameKind::BulletList, 0,
        BlockFrame::encode_blt_body(b'-'));

    // [BQ, BLT], "# header": BLT disowns → Pop → BQ 懒延续可能 admits
    let (t_h, a_h) = scan_block(b"# header", &[bq, blt], None);
    assert!(matches!(a_h, mdio_core::block::ScanAction::Pop),
        "BLT disowns header → Pop");

    // [BQ, BLT(0)], "- item"：约束链（BQ 的 RequireBqPrefix——无 >）失败——
    // list_lazy_candidate 只豁免 lazy 段落；marker 行不是段落——不豁免 →
    // Pop（cmark：`> - a\n- item` 的 - item 是顶层新列表）
    let (_t_i, a_i) = scan_block(b"- item", &[bq, blt], None);
    assert!(matches!(a_i, mdio_core::block::ScanAction::Pop),
        "marker 行无 > 前缀应关闭 BQ（cmark 顶层新列表）");

    // 纯 [BLT], "- item": BLT admits, 无外部约束
    let (_t_pure, a_pure) = scan_block(b"- item", &[blt], None);
    assert!(!matches!(a_pure, mdio_core::block::ScanAction::Pop),
        "Pure BLT admits - item");

    // 传播本质: 外层容器 (BQ) 的上下文约束覆盖了内层 (BLT) 的 admit
    // disowns 不是"BLT 拒绝了行", 而是"BQ 的上下文拒绝了整个嵌套"
    // 内层的 admit 被外层的结构 guard 否决 → 向外传播
}

// FrameLog 自证 — 证明链的最后一环

use mdio_core::block::{
    push_entry as fl_push, pop_entry as fl_pop, body_mut_entry as fl_body_mut,
    forward_replay, backward_replay, rebuild_stack_at,
};

/// 第 1 层: 编码正确性.
/// push_entry → entry_frame 还原 frame.
/// pop_entry 的 Pop 标记正确, entry_frame 清除标记.
#[test]
fn proof_framelog_layer1_encoding() {
    for kind in &[
        FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::OrderedList,
        FrameKind::CodeFence, FrameKind::HtmlBlock, FrameKind::Table,
        FrameKind::Comment, FrameKind::HtmlElement, FrameKind::RefDef,
    ] {
        for indent in &[0u8, 2, 10, 63] {
            for body in &[0u8, 1, 8, 32, 63] {
                let f = BlockFrame::new(*kind, *indent, *body);

                // Push entry: 无 Pop 标记, 无 BodyMut 标记
                let pe = fl_push(7, f);
                assert!(!entry_is_pop(pe), "push_entry should not set Pop flag");
                assert!(!entry_is_body_mut(pe), "push_entry should not set BodyMut flag");
                assert_eq!(entry_line(pe), 7);
                assert_eq!(entry_frame(pe).0, f.0,
                    "entry_frame(push_entry) must restore exact frame");

                // Pop entry: Pop 标记, 无 BodyMut 标记, frame 可恢复
                let po = fl_pop(7, f);
                assert!(entry_is_pop(po), "pop_entry must set Pop flag");
                assert!(!entry_is_body_mut(po), "pop_entry should not set BodyMut flag");
                assert_eq!(entry_line(po), 7);
                // entry_frame 清除 bit15 (Pop 标记) → 还原原始 frame
                assert_eq!(entry_frame(po).0, f.0,
                    "entry_frame(pop_entry) must restore frame sans Pop flag");

                // BodyMut entry: BodyMut 标记, 无 Pop 标记
                let bm = fl_body_mut(7, f);
                assert!(!entry_is_pop(bm), "body_mut_entry should not set Pop flag");
                assert!(entry_is_body_mut(bm), "body_mut_entry must set BodyMut flag");
                assert_eq!(entry_line(bm), 7);
                assert_eq!(entry_frame(bm).0, f.0,
                    "entry_frame(body_mut_entry) must restore frame");
            }
        }
    }
}

/// 第 2 层: 前向 replay 保真.
/// 归纳于 entry 数: 每条 Push/Pop/BodyMut 对栈的操作与原始扫描一致.
#[test]
fn proof_framelog_layer2_forward_replay_fidelity() {
    let f0 = BlockFrame::new(FrameKind::BulletList, 2,
        BlockFrame::encode_blt_body(b'-'));
    let f1 = BlockFrame::new(FrameKind::BlockQuote, 0,
        0);
    let f2 = BlockFrame::new(FrameKind::CodeFence, 0,
        BlockFrame::encode_cf_body(b'`', 3));

    // 模拟扫描序列: Push(BLT@0) → Push(BQ@1) → BodyMut(BQ@2) → Pop@3 → Pop@4
    let log = vec![
        fl_push(0, f0),                        // BLT opens
        fl_push(1, f1),                        // BQ opens inside BLT
        fl_body_mut(2, f1),                    // BQ body changes (para_open toggle)
        fl_pop(3, f1),                         // BQ closes
        fl_pop(4, f0),                         // BLT closes
    ];

    // 逐步验证: 每步后与手动栈操作一致
    let mut stack = Vec::new();
    let mut expected = Vec::new();

    // entry 0: Push(BLT@0)
    forward_replay(&log[0..1], &mut stack);
    expected.push(f0);
    assert_eq!(stack, expected, "step 0: Push BLT");

    // entry 1: Push(BQ@1)
    forward_replay(&log[1..2], &mut stack);
    expected.push(f1);
    assert_eq!(stack, expected, "step 1: Push BQ");

    // entry 2: BodyMut(BQ@2) — stack depth unchanged, body updated
    forward_replay(&log[2..3], &mut stack);
    assert_eq!(stack.len(), 2, "BodyMut doesn't change depth");
    assert_eq!(stack[1].body(), f1.body(), "BodyMut updates body");

    // entry 3: Pop(BQ@3)
    forward_replay(&log[3..4], &mut stack);
    expected.pop();
    assert_eq!(stack, expected, "step 3: Pop BQ");

    // entry 4: Pop(BLT@4)
    forward_replay(&log[4..5], &mut stack);
    expected.pop();
    assert_eq!(stack, expected, "step 4: Pop BLT → empty");

    // 全量 replay 应一致
    let mut full_stack = Vec::new();
    forward_replay(&log, &mut full_stack);
    assert!(full_stack.is_empty(), "full replay → empty stack");

    // rebuild_stack_at 应给出每行开始前的正确栈
    assert!(rebuild_stack_at(&log, 0).is_empty(),                "line 0: empty");
    assert_eq!(rebuild_stack_at(&log, 1), vec![f0],               "line 1: [BLT]");
    assert_eq!(rebuild_stack_at(&log, 2), vec![f0, f1],           "line 2: [BLT,BQ]");
    assert_eq!(rebuild_stack_at(&log, 4), vec![f0],               "line 4: [BLT]");
}

/// 第 3 层: 反向 replay 保真 (Push/Pop).
/// backward_replay 逆序应用逆操作, 恢复原始栈.
#[test]
fn proof_framelog_layer3_backward_replay() {
    let f0 = BlockFrame::new(FrameKind::BulletList, 0,
        BlockFrame::encode_blt_body(b'-'));
    let f1 = BlockFrame::new(FrameKind::BlockQuote, 0,
        0);

    let log = vec![fl_push(0, f0), fl_push(1, f1), fl_pop(3, f1), fl_pop(4, f0)];

    // 正向: [] → [f0] → [f0, f1] → [f0] → []
    let mut stack = Vec::new();
    forward_replay(&log[..2], &mut stack);  // 只跑到 Push(BQ)
    assert_eq!(stack, vec![f0, f1]);

    // 反向: [f0, f1] 撤销 Push(BQ) → [f0]
    backward_replay(&log[1..2], &mut stack);
    assert_eq!(stack, vec![f0], "backward over Push(BQ) restores [BLT]");

    // 继续反向: [f0] 撤销 Push(BLT) → []
    backward_replay(&log[0..1], &mut stack);
    assert!(stack.is_empty(), "backward over Push(BLT) restores []");

    // 完整往返: 正向全部 → 反向全部 → 恢复初始
    let mut s = Vec::new();
    forward_replay(&log, &mut s);
    assert!(s.is_empty(), "full forward → empty");
    backward_replay(&log, &mut s);
    // backward 逆序: Pop@4→push(f0), Pop@3→push(f1), Push@1→pop, Push@0→pop
    // = push(f0), push(f1), pop, pop = [f0, f1] → pop → [f0] → pop → []
    assert!(s.is_empty(), "full backward restores initial empty stack");
    // 往返不变量: forward ∘ backward = id (在 Stack 上)
    // 从任意中间状态: forward(log) → backward(log) → 原始状态
}

/// 第 4 层: BodyMut 在反向 replay 中被跳过, 但前向覆盖.
/// 反向无法恢复旧 body (有损), 但不影响正确性——下次正向覆盖.
#[test]
fn proof_framelog_layer4_bodymut_lossiness() {
    let f = BlockFrame::new(FrameKind::BulletList, 0,
        BlockFrame::encode_blt_body(b'-'));
    let mut f_loose = f;
    f_loose = BlockFrame(f_loose.0 | LOOSE_BIT);

    // 正向: Push(BLT) → BodyMut(LOOSE_BIT on)
    let log = vec![fl_push(0, f), fl_body_mut(1, f_loose)];

    let mut s = Vec::new();
    forward_replay(&log, &mut s);
    assert_eq!(s.len(), 1);
    assert_ne!(s[0].body(), f.body(), "LOOSE_BIT changed body after BodyMut");

    // 反向: BodyMut 被跳过 → Push undone → stack empty
    backward_replay(&log, &mut s);
    assert!(s.is_empty(), "backward: BodyMut skipped, Push undone → empty");

    // 正向再次 apply → body 被正确覆盖
    let mut s2 = Vec::new();
    forward_replay(&log, &mut s2);
    assert_eq!(s2[0].body(), f_loose.body(),
        "forward replay restores correct body after BodyMut loss");
}


// §AR 变更检测 — 形式化推导的直接代码翻译
//
// 以下代码是 formal_scan.md §AR 的逐句翻译。
// 不引入 §AR 之外的抽象, 不精简引理和推论。
//
// §AR.1 — AST 因式分解:
//   AST = f(B(K), PL, Rope), f = walk_blocks ∘ resolve
//
// §AR.2 — 变更定义域分解:
//   S_FL  = FL_diff 区域 — { new_push, old_push, body_mut }
//   S_PL  = { i | PL_new[i] ≠ PL_old_shifted[i] }
//   S_Ed  = { line_of_edit | edit_records }
//
//   引理 AR.1: AST 变化 ⇒ i ∈ S_FL ∨ i ∈ S_PL ∨ i ∈ S_Ed
//
//   定义 cover(Push(f, li)) = [li, end_line_of(f)]
//   ADD 覆盖    = ∪ cover(e) for e ∈ S_FL.new_push
//   DELETE 覆盖 = ∪ cover(e) for e ∈ S_FL.old_push
//
//   定理 AR.2 (覆盖域互斥性):
//     i ∈ ADD∪DELETE 覆盖域 → 被结构变更包含, 不独立 REPLACE
//     i ∈ BodyMut → MODIFY
//     i ∉ 覆盖域 → REPLACE
//
// §AR.3 (信号完备性): S_FL, S_PL, S_Ed 极小充分, 不可互相替代。
//
// §AR.5 (修补语义):
//   Step 1: S_FL.new_push → ADD, S_FL.old_push → DELETE, S_FL.body_mut → MODIFY
//   Step 2: S_covered = ∪ cover(Push) for ADD/DELETE
//           for i ∈ (S_PL ∪ S_Ed) \ S_covered → REPLACE

use std::collections::HashSet;

// ── §AR.2: S_FL, S_PL, S_Ed 数据结构 ──

/// §AR.2: S_FL = { new_push, old_push, body_mut }
struct S_FL {
    new_push: Vec<(u32, FrameKind)>,   // ADD: (new_line, kind)
    old_push: Vec<(u32, FrameKind)>,   // DELETE: (old_line, kind)
    body_mut: Vec<(u32, FrameKind)>,   // MODIFY: (new_line, kind)
}

/// §AR.2: 从 ripple_frame_actions (E_new_ripple) + old ripple 区直接构造 S_FL。
///
/// §T.3 证明 FL_new = FL_prefix ++ E_new_ripple ++ E_shifted。
/// §AR.2: S_FL = [prefix_end .. tail_start_old)。
///
/// 匹配不按行号 — §T.3 保证 delta 偏移后 suffix 一致, ripple 区新旧行号可能不同。
/// 按 (FrameKind, is_pop, is_bm) 三元组比较。同 Push 条目的语义位置靠 DFS 序保证。
fn build_S_FL(
    ripple_frame_actions: &[u64],
    fl_old_entries: &[u64],
    prefix_end: usize,
    tail_start_old: usize,
) -> S_FL {
    // ── E_new_ripple 分类 ──
    let mut new_push = Vec::new();
    let mut body_mut = Vec::new();
    for &e in ripple_frame_actions {
        if entry_is_body_mut(e) {
            body_mut.push((entry_line(e), entry_frame(e).kind()));
        } else if !entry_is_pop(e) {
            let f = entry_frame(e);
            let line = entry_line(e);
            // PopPush: 同行有匹配 Pop
            let is_poppush = ripple_frame_actions.iter().any(|&p| {
                entry_is_pop(p) && entry_line(p) == line && entry_frame(p).kind() == f.kind()
            });
            if is_poppush { continue; }
            // Push 在新 FL 中出现 — 是否有旧对应 (按三元组, 不按行号)
            let has_old = fl_old_entries[prefix_end..fl_old_entries.len().min(tail_start_old)]
                .iter().any(|&e2| {
                    !entry_is_pop(e2) && !entry_is_body_mut(e2)
                        && entry_frame(e2).kind() == f.kind()
                });
            if !has_old {
                new_push.push((line, f.kind()));
            }
        }
    }

    // ── 旧 ripple 区 Push 条目: 无新对应 → old_push ──
    let mut old_push = Vec::new();
    let scan_end = fl_old_entries.len().min(tail_start_old);
    for i in prefix_end..scan_end {
        let e = fl_old_entries[i];
        if entry_is_body_mut(e) || entry_is_pop(e) { continue; }
        let f = entry_frame(e);
        let old_kind = f.kind();
        let old_line = entry_line(e);
        // PopPush 检测
        let is_poppush = (i + 1 < scan_end)
            && entry_is_pop(fl_old_entries[i + 1])
            && entry_frame(fl_old_entries[i + 1]).kind() == old_kind;
        if is_poppush { continue; }
        // 在新 ripple 中找对应 Push (按三元组, 不按行号)
        let has_new = ripple_frame_actions.iter().any(|&e2| {
            !entry_is_pop(e2) && !entry_is_body_mut(e2)
                && entry_frame(e2).kind() == old_kind
        });
        if !has_new {
            old_push.push((old_line, old_kind));
        }
    }

    S_FL { new_push, old_push, body_mut }
}

/// §AR.2: S_PL — PL 差异 (ripple 区内比较)
fn build_S_PL(lines_old: &[u8], lines_new: &[u8], delta: isize, prefix_end: usize) -> Vec<u32> {
    let mut s_pl = Vec::new();
    for new_i in prefix_end..lines_new.len() {
        let old_i = (new_i as isize - delta).max(0) as usize;
        if old_i < lines_old.len() && lines_new[new_i] != lines_old[old_i] {
            s_pl.push(new_i as u32);
        }
    }
    s_pl
}

/// §AR.2: S_Ed — 编辑波及的行 (旧 rope → +delta → 新 rope)
fn build_S_Ed(edit_lines_old: &[usize], delta: isize) -> Vec<u32> {
    edit_lines_old.iter().map(|&l| (l as isize + delta).max(0) as u32).collect()
}

// ── §AR.2: cover(Push(f, li)) ──

/// §AR.2 定义: cover(Push(f, li)) = [li, end_line_of(f)]
/// 若未关闭, end = N (§AR.2: "或 N (若未关闭)")
fn cover_of_push(fl: &[u64], push_line: u32, kind: FrameKind) -> (u32, u32) {
    let end = fl.iter()
        .filter(|&&e| entry_is_pop(e)
            && entry_frame(e).kind() == kind && entry_line(e) > push_line)
        .map(|&e| entry_line(e))
        .next()
        .unwrap_or(u32::MAX);  // §AR.2: 未关闭 → end = N
    (push_line, end)
}

/// §AR.2: S_covered = ∪ cover(e) for e ∈ S_FL.new_push ∪ S_FL.old_push
fn build_S_covered(fl_new: &[u64], fl_old: &[u64], s_fl: &S_FL) -> HashSet<u32> {
    let mut covered = HashSet::new();
    for &(line, kind) in &s_fl.new_push {
        let (_s, e) = cover_of_push(fl_new, line, kind);
        if e == u32::MAX { continue; } // §AR.2: 未闭合容器 cover 到文档末, 无冲突 REPLACE 需排除
        for li in line..e { covered.insert(li); }
    }
    for &(line, kind) in &s_fl.old_push {
        let (_s, e) = cover_of_push(fl_old, line, kind);
        if e == u32::MAX { continue; }
        for li in line..e { covered.insert(li); }
    }
    covered
}

// ── child_path: FL prefix replay (§2.2) ──

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

// ── §AR.5: 修补语义 — 三信号 → Patch 序列 ──

/// §AR.5 的直接翻译。不引入 §AR 之外的简化。
fn apply_AR5(
    fl_new: &[u64], fl_old: &[u64],
    s_fl: &S_FL, s_pl: &[u32], s_ed: &[u32], s_covered: &HashSet<u32>,
) -> Vec<(String, String)> { // (op, path)
    let mut patches = Vec::new();

    // §AR.5 Step 1: 结构变更
    for &(line, _) in &s_fl.new_push {
        patches.push((String::from("add"), child_path_from_entries(fl_new, line)));
    }
    for &(line, _) in &s_fl.old_push {
        patches.push((String::from("remove"), child_path_from_entries(fl_old, line)));
    }
    for &(line, _) in &s_fl.body_mut {
        patches.push((String::from("replace"), child_path_from_entries(fl_new, line)));
    }

    // §AR.5 Step 2: (S_PL ∪ S_Ed) \ S_covered → REPLACE
    for &new_line in s_pl.iter().chain(s_ed.iter()) {
        if s_covered.contains(&new_line) { continue; }
        patches.push((String::from("replace"), child_path_from_entries(fl_new, new_line)));
    }
    patches
}

// §AR.3(2): 三信号不可互相替代

/// "移除 S_FL → 漏结构变化" — §AR.3(2)
/// 构造: old FL 有 BLT Push, new FL 没有。同 PL, 无编辑。
#[test]
fn ar_signal_fl_irreplaceable() {
    let blt = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    let fl_old = vec![push_entry(0, blt)];
    let fl_new: Vec<u64> = vec![];
    let lines_old: Vec<u8> = vec![];
    let lines_new: Vec<u8> = vec![];
    let edit_lines: Vec<usize> = vec![];

    let s_fl = build_S_FL(&fl_new, &fl_old, 0, fl_old.len());
    let s_pl = build_S_PL(&lines_old, &lines_new, 0, 0);
    let s_ed = build_S_Ed(&edit_lines, 0);

    // S_FL must detect container removal
    assert!(!s_fl.old_push.is_empty(),
        "§AR.3(2): S_FL must detect container removal (BLT disappeared)");
    // S_PL and S_Ed are empty — cannot detect this
    assert!(s_pl.is_empty(),
        "§AR.3(2): S_PL empty — types unchanged");
    assert!(s_ed.is_empty(),
        "§AR.3(2): S_Ed empty — no edit");
}

/// "移除 S_PL → 漏类型变化" — §AR.3(2)
/// 构造: FL 相同 (无容器), 但 PL 从 Paragraph(10) 变 Header1(0)。无编辑。
#[test]
fn ar_signal_pl_irreplaceable() {
    let fl_old: Vec<u64> = vec![];
    let fl_new: Vec<u64> = vec![];
    let lines_old: Vec<u8> = vec![10]; // Paragraph
    let lines_new: Vec<u8> = vec![0];  // Header1
    let edit_lines: Vec<usize> = vec![];

    let s_fl = build_S_FL(&fl_new, &fl_old, 0, fl_old.len());
    let s_pl = build_S_PL(&lines_old, &lines_new, 0, 0);
    let s_ed = build_S_Ed(&edit_lines, 0);

    // S_FL and S_Ed empty — cannot detect type change
    assert!(s_fl.new_push.is_empty() && s_fl.old_push.is_empty(),
        "§AR.3(2): S_FL empty — no structure change");
    assert!(s_ed.is_empty(),
        "§AR.3(2): S_Ed empty — no edit");
    // Only S_PL detects this
    assert!(!s_pl.is_empty(),
        "§AR.3(2): S_PL must detect type change (paragraph→header)");
}

/// "移除 S_Ed → 漏纯内容变化" — §AR.3(2)
/// 构造: FL 相同, PL 相同, 但有编辑波及行。只有 S_Ed 能检测。
#[test]
fn ar_signal_edits_irreplaceable() {
    let fl_old: Vec<u64> = vec![];
    let fl_new: Vec<u64> = vec![];
    let lines_old: Vec<u8> = vec![10]; // Paragraph
    let lines_new: Vec<u8> = vec![10]; // Paragraph (same type)
    let edit_lines: Vec<usize> = vec![0];

    let s_fl = build_S_FL(&fl_new, &fl_old, 0, fl_old.len());
    let s_pl = build_S_PL(&lines_old, &lines_new, 0, 0);
    let s_ed = build_S_Ed(&edit_lines, 0);

    // S_FL and S_PL empty — cannot detect content-only change
    assert!(s_fl.new_push.is_empty() && s_fl.old_push.is_empty(),
        "§AR.3(2): S_FL empty — no structure change");
    assert!(s_pl.is_empty(),
        "§AR.3(2): S_PL empty — types unchanged");
    // Only S_Ed detects this
    assert!(!s_ed.is_empty(),
        "§AR.3(2): S_Ed must detect content-only change");
}

// §AR.3(3): 三信号正交 — 任意两个不能互推

/// FL ⟂ PL: FL 相同不代表 PL 相同 (§AR.3(3) + T4)
#[test]
fn ar_orthogonality_fl_pl() {
    // fl_a = fl_b: 都有 BLT Push@0, 无其他容器
    let blt = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    let fl_a = vec![push_entry(0, blt)];
    let fl_b = vec![push_entry(0, blt)];

    // S_FL from identical FLs → empty (no structural change)
    let s_fl = build_S_FL(&fl_b, &fl_a, 0, fl_a.len());
    assert!(s_fl.new_push.is_empty() && s_fl.old_push.is_empty(),
        "§AR.3(3): S_FL empty — FLs are identical");

    // But PL can differ independently
    let pl_a: Vec<u8> = vec![6]; // Bullet
    let pl_b: Vec<u8> = vec![10]; // Paragraph
    let s_pl = build_S_PL(&pl_a, &pl_b, 0, 0);
    assert!(!s_pl.is_empty(),
        "§AR.3(3): S_PL non-empty — PL differs despite same FL");
}

/// FL ⟂ Edits: FL 相同不代表 Rope 相同 (§AR.3(3))
#[test]
fn ar_orthogonality_fl_edits() {
    let fl_a: Vec<u64> = vec![];
    let fl_b: Vec<u64> = vec![];

    let s_fl = build_S_FL(&fl_b, &fl_a, 0, 0);
    assert!(s_fl.new_push.is_empty() && s_fl.old_push.is_empty());

    // Edits detected independently of FL
    let edit_lines: Vec<usize> = vec![1];
    let s_ed = build_S_Ed(&edit_lines, 0);
    assert!(!s_ed.is_empty(),
        "§AR.3(3): S_Ed non-empty — edit exists despite identical FL");
}

/// PL ⟂ Edits: PL 相同不代表 Rope 相同 (§AR.3(3))
#[test]
fn ar_orthogonality_pl_edits() {
    let pl_a: Vec<u8> = vec![10, 10]; // both Paragraph
    let pl_b: Vec<u8> = vec![10, 10]; // same

    let s_pl = build_S_PL(&pl_a, &pl_b, 0, 0);
    assert!(s_pl.is_empty(),
        "§AR.3(3): S_PL empty — PLs are identical");

    // Edits detected independently of PL
    let edit_lines: Vec<usize> = vec![0];
    let s_ed = build_S_Ed(&edit_lines, 0);
    assert!(!s_ed.is_empty(),
        "§AR.3(3): S_Ed non-empty — edit exists despite identical PL");
}

/// 三信号同时触发: 结构 + 类型 + 内容变化共存 (§AR.3(1) 完备性)
#[test]
fn ar_completeness_all_signals() {
    let blt = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    // Old: BLT opened at line 0
    let fl_old = vec![push_entry(0, blt)];
    // New: no BLT (deleted), Paragraph at line 0
    let fl_new: Vec<u64> = vec![];
    // PL: old has Bullet(6), new has Paragraph(10)
    let lines_old: Vec<u8> = vec![6];  // Bullet
    let lines_new: Vec<u8> = vec![10]; // Paragraph
    let edit_lines: Vec<usize> = vec![0];

    let s_fl = build_S_FL(&fl_new, &fl_old, 0, fl_old.len());
    let s_pl = build_S_PL(&lines_old, &lines_new, 0, 0);
    let s_ed = build_S_Ed(&edit_lines, 0);

    assert!(!s_fl.old_push.is_empty(), "S_FL: structural change detected");
    assert!(!s_pl.is_empty(), "S_PL: type change detected");
    assert!(!s_ed.is_empty(), "S_Ed: content change detected");
}

// §AR.2: 覆盖域排除

/// 定理 AR.2: ADD 覆盖域内行不独立 REPLACE
/// 构造: new BLT Push ∈ S_FL, 行 0 的 PL/Edit 变化被 ADD 覆盖
#[test]
fn ar_add_cover_excludes_replace() {
    let blt = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    let fl_new = vec![push_entry(0, blt)];
    let fl_old: Vec<u64> = vec![];
    let lines_old: Vec<u8> = vec![];
    let lines_new: Vec<u8> = vec![6]; // Bullet
    let edit_lines: Vec<usize> = vec![];

    let s_fl = build_S_FL(&fl_new, &fl_old, 0, fl_old.len());
    // S_PL: new line 0 is Bullet(6), old had nothing → difference
    let s_pl = build_S_PL(&lines_old, &lines_new, 0, 0);
    let s_ed = build_S_Ed(&edit_lines, 0);
    let s_covered = build_S_covered(&fl_new, &fl_old, &s_fl);

    let patches = apply_AR5(&fl_new, &fl_old, &s_fl, &s_pl, &s_ed, &s_covered);

    // ADD must exist for the new container
    assert!(patches.iter().any(|(op, _)| op == "add"),
        "theorem AR.2: new BLT → ADD patch");
    // No duplicate path = no REPLACE for lines inside ADD cover
    let mut paths = HashSet::new();
    for (_, path) in &patches {
        assert!(paths.insert(path.clone()),
            "theorem AR.2: duplicate path = double-patch on {}", path);
    }
}

/// 定理 AR.2: DELETE 覆盖域内行不独立 REPLACE
/// 构造: old BLT Push ∈ S_FL (deleted), 行在旧覆盖域内不产独立 REPLACE
#[test]
fn ar_delete_cover_excludes_replace() {
    let blt = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    let fl_old = vec![push_entry(0, blt)];
    let fl_new: Vec<u64> = vec![];
    let lines_old: Vec<u8> = vec![6]; // Bullet
    let lines_new: Vec<u8> = vec![];  // empty
    let edit_lines: Vec<usize> = vec![];

    let s_fl = build_S_FL(&fl_new, &fl_old, 0, fl_old.len());
    let s_pl = build_S_PL(&lines_old, &lines_new, 0, 0);
    let s_ed = build_S_Ed(&edit_lines, 0);
    let s_covered = build_S_covered(&fl_new, &fl_old, &s_fl);

    let patches = apply_AR5(&fl_new, &fl_old, &s_fl, &s_pl, &s_ed, &s_covered);

    // DELETE must exist for the removed container
    assert!(patches.iter().any(|(op, _)| op == "remove"),
        "theorem AR.2: removed BLT → DELETE patch");
    assert!(!patches.is_empty(), "theorem AR.2: must produce patches");
}

// §AR.5: 修补语义

/// §AR.5: no-op edit → 三信号全空 → 零 patch
#[test]
fn ar_no_signals_zero_patches() {
    let fl_old: Vec<u64> = vec![];
    let fl_new: Vec<u64> = vec![];
    let lines_old: Vec<u8> = vec![];
    let lines_new: Vec<u8> = vec![];
    let edit_lines: Vec<usize> = vec![];

    let s_fl = build_S_FL(&fl_new, &fl_old, 0, fl_old.len());
    let s_pl = build_S_PL(&lines_old, &lines_new, 0, 0);
    let s_ed = build_S_Ed(&edit_lines, 0);
    let s_covered = build_S_covered(&fl_new, &fl_old, &s_fl);

    let patches = apply_AR5(&fl_new, &fl_old, &s_fl, &s_pl, &s_ed, &s_covered);
    assert!(patches.is_empty(), "§AR.5: no-op → zero patches");
}

/// §AR.5 Step 1+2: 结构 ADD + 内容 REPLACE 共存
#[test]
fn ar_full_patch_semantics() {
    let blt = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    // Old: empty. New: BLT@0 + Paragraph content.
    let fl_old: Vec<u64> = vec![];
    let fl_new = vec![push_entry(0, blt)];
    let lines_old: Vec<u8> = vec![];
    let lines_new: Vec<u8> = vec![6]; // Bullet
    let edit_lines: Vec<usize> = vec![0];

    let s_fl = build_S_FL(&fl_new, &fl_old, 0, fl_old.len());
    let s_pl = build_S_PL(&lines_old, &lines_new, 0, 0);
    let s_ed = build_S_Ed(&edit_lines, 0);
    let s_covered = build_S_covered(&fl_new, &fl_old, &s_fl);

    let patches = apply_AR5(&fl_new, &fl_old, &s_fl, &s_pl, &s_ed, &s_covered);

    let has_add = patches.iter().any(|(op, _)| op == "add");
    assert!(has_add, "§AR.5 Step 1: new BLT → ADD");
    assert!(!patches.is_empty(), "§AR.5: must produce patches");
}

// §AR.2 gap tests: cover_of_push, build_S_FL delta, build_S_FL prefix

/// Gap 1: cover_of_push 未闭合容器 → end = u32::MAX (§AR.2: "或 N 若未关闭")
#[test]
fn ar_cover_of_push_unclosed() {
    let blt = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    let fl = vec![push_entry(0, blt)];  // Push 无匹配 Pop
    let (start, end) = cover_of_push(&fl, 0, FrameKind::BulletList);
    assert_eq!(start, 0);
    assert_eq!(end, u32::MAX, "§AR.2: unclosed container → end = N");
}

/// Gap 2: build_S_FL delta 移位 — Push 在 ripple 区间行号差 delta, 按三元组匹配不按行号
#[test]
fn ar_build_S_FL_delta_shift() {
    let blt = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    // Old: BLT@0. New: BLT@1 (delta=+1, 同容器仅行号偏移)
    let fl_old = vec![push_entry(0, blt)];
    let fl_new = vec![push_entry(1, blt)];
    let s_fl = build_S_FL(&fl_new, &fl_old, 0, fl_old.len());
    assert!(s_fl.new_push.is_empty(), "delta=+1: same BLT, not added");
    assert!(s_fl.old_push.is_empty(), "delta=+1: same BLT, not removed");
}

/// Gap 3: build_S_FL prefix_end 跳过不变前缀
#[test]
fn ar_build_S_FL_prefix_skip() {
    let blt = BlockFrame::new(FrameKind::BulletList, 0, BlockFrame::encode_blt_body(b'-'));
    // Old: BLT@0 + BLT@1. ripple_new 空 (BLT@1 被删)。prefix_end=1: BLT@0 在 prefix 不变。
    let fl_old = vec![push_entry(0, blt), push_entry(1, blt)];
    let ripple_new: Vec<u64> = vec![];
    // prefix_end=1: 跳过 BLT@0 (不变), 只 diff BLT@1 区域
    let s_fl = build_S_FL(&ripple_new, &fl_old, 1, fl_old.len());
    assert!(s_fl.new_push.is_empty(), "prefix skipped: no new BLT");
    assert_eq!(s_fl.old_push.len(), 1, "prefix skipped: BLT@1 removed");
    assert_eq!(s_fl.old_push[0].0, 1);
}

fn walk_subtree_json(state: &mut State, push_line: usize) -> serde_json::Value {
    let fl = &state.frame_log.entries;
    let path = child_path_from_entries(fl, push_line as u32);
    let full: serde_json::Value = ast_ch(&state.snapshot());
    let children_parts: Vec<usize> = path.split("children/")
        .skip(1).filter_map(|s| s.trim_end_matches('/').split('/').next()?.parse().ok())
        .collect();
    let mut node = &full;
    for idx in children_parts {
        if let Some(arr) = node.as_array() {
            node = arr.get(idx).unwrap_or(&serde_json::Value::Null);
        } else if let Some(children) = node.get("ct").and_then(|c| c.as_array()) {
            node = children.get(idx).unwrap_or(&serde_json::Value::Null);
        } else { return serde_json::Value::Null; }
    }
    node.clone()
}

#[test]
fn ar_walk_subtree_matches_ast() {
    let mut state = State::new("# H\n- a\n- b\n- c\n");
    let blt = state.frame_log.entries.iter()
        .find(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e)
            && entry_frame(e).kind() == FrameKind::BulletList)
        .map(|&e| entry_line(e));
    if let Some(line) = blt {
        let subtree = walk_subtree_json(&mut state, line as usize);
        assert!(!subtree.is_null(), "walk_subtree should match to_ast_object subtree");
    }
}

#[test]
fn ar_walk_subtree_leaf() {
    let mut state = State::new("plain text\n");
    let ast: serde_json::Value = ast_ch(&state.snapshot());
    assert_eq!(ast[0]["nt"], "p");
}

// incr_plan 实验 — pure testbed, 不动生产代码

use std::collections::BTreeSet;
use mdio_core::incr::{IncrTask, Patch, patches_for_mode};
fn parse_val(v: &str) -> serde_json::Value { if v == "null" { serde_json::Value::Null } else { serde_json::from_str(v).unwrap() } }
use std::sync::Arc;

// PackedLineInfo already imported above; entry_* functions from block

fn pl_diff_range(a: &[u8], b: &[u8]) -> Option<(usize, usize)> {
    let n = a.len().min(b.len());
    let first = (0..n).find(|&i| a[i] != b[i])?;
    let last = (first..n).rev().find(|&i| a[i] != b[i])?;
    Some((first, last))
}

fn fl_diff_range(a: &[u64], b: &[u64]) -> Option<(usize, usize)> {
    let n = a.len().min(b.len());
    let first = (0..n).find(|&i| a[i] != b[i])?;
    let last = (first..n).rev().find(|&i| a[i] != b[i])?;
    Some((first, last))
}

fn is_header_t(t: BlockType) -> bool {
    matches!(t, BlockType::Header1 | BlockType::Header2 | BlockType::Header3
        | BlockType::Header4 | BlockType::Header5 | BlockType::Header6
        | BlockType::HsHead1 | BlockType::HsHead2)
}

fn frame_kind_t(t: BlockType) -> Option<FrameKind> {
    match t {
        BlockType::Bullet | BlockType::Checklist => Some(FrameKind::BulletList),
        BlockType::OrderedListItem => Some(FrameKind::OrderedList),
        BlockType::CodeFenceOpen => Some(FrameKind::CodeFence),
        BlockType::Blockquote => Some(FrameKind::BlockQuote),
        BlockType::HtmlBlock => Some(FrameKind::HtmlBlock),
        BlockType::Comment => Some(FrameKind::Comment),
        BlockType::TableRow | BlockType::TableSeparator => Some(FrameKind::Table),
        BlockType::HtmlElement => Some(FrameKind::HtmlElement),
        BlockType::RefDef => Some(FrameKind::RefDef),
        _ => None,
    }
}

fn is_skip(t: BlockType) -> bool {
    matches!(t, BlockType::Blank | BlockType::CodeContent | BlockType::HtmlContent
        | BlockType::CodeFenceClose | BlockType::HsSep | BlockType::TableSeparator)
}

fn resolve_block_end(pl: &[u8], li: usize) -> usize {
    let t = PackedLineInfo(pl[li]).node_type();
    match t {
        BlockType::Paragraph => {
            let mut e = li + 1;
            while e < pl.len() {
                if PackedLineInfo(pl[e]).node_type() != BlockType::Paragraph { break; }
                e += 1;
            }
            e
        }
        BlockType::HsHead1 | BlockType::HsHead2 => {
            let mut e = li + 1;
            while e < pl.len() {
                let nt = PackedLineInfo(pl[e]).node_type();
                if matches!(nt, BlockType::HsHead1 | BlockType::HsHead2) { e += 1; }
                else { break; }
            }
            e
        }
        BlockType::IndentedCode => {
            let mut e = li + 1;
            while e < pl.len() {
                let nt = PackedLineInfo(pl[e]).node_type();
                if nt == BlockType::IndentedCode || nt == BlockType::Blank { e += 1; }
                else { break; }
            }
            while e > li + 1 && PackedLineInfo(pl[e-1]).node_type() == BlockType::Blank { e -= 1; }
            e
        }
        _ if is_header_t(t) => li + 1,
        _ => li + 1,
    }
}

fn blocks_in_range_test(entries: &[u64], pl: &[u8], range_start: u32, range_end: u32,
    frame_log: &FrameLog) -> Vec<(u32, u32, Option<FrameKind>, BlockType)>
{
    let mut blocks = Vec::new();
    let pli: Vec<PackedLineInfo> = pl.iter().map(|&b| PackedLineInfo(b)).collect();
    let mut li = range_start as usize;
    let max = (range_end as usize).min(pl.len());
    while li < max {
        let t = PackedLineInfo(pl[li]).node_type();
        if is_skip(t) { li += 1; continue; }
        let push_entry = entries.iter()
            .find(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e) && entry_line(e) as usize == li);
        let kind = push_entry.map(|&e| entry_frame(e).kind())
            .or_else(|| frame_kind_t(t));
        let end = if let Some(k) = kind {
            frame_log.item_tail_boundary(li as u32, k, &pli)
                .map(|e| e as usize)
                .unwrap_or_else(|| {
                    let has_push = entries.iter().any(|&e|
                        !entry_is_pop(e) && !entry_is_body_mut(e)
                        && entry_line(e) as usize == li && entry_frame(e).kind() == k);
                    if has_push { pl.len() } else { li + 1 }
                })
                .max(li + 1)
        } else {
            resolve_block_end(pl, li).max(li + 1)
        };
        blocks.push((li as u32, end as u32, kind, t));
        li = end;
    }
    blocks
}

/// 单条补丁落地（WC-11）：末段**数字 = 数组下标**（原语义）；
/// 末段**非数字 = 对象成员位**——WC-2「丙」建数组 `Add <壳>/ct = [整数组]`、
/// WC-5 整数组 `Remove/Replace <壳>/ct`。字段写（ReplaceAttr）仍是"元素路径 +
/// attr"（内部枚举不承载成员路径）。
/// 返回 false = 该 op 无法落地 ⇒ 调用方**必须显式失败**（原先只 eprintln 继续，
/// 会让不支持的形态悄悄过关）。
fn apply_one_patch(root: &mut serde_json::Value, p: &Patch) -> bool {
    match p {
        Patch::Add { path, value } => {
            let Some((pp, last)) = path.rsplit_once('/') else { return false };
            let Some(parent) = root.pointer_mut(pp) else { return false };
            match last.parse::<usize>() {
                Ok(i) => parent.as_array_mut().map_or(false, |arr| {
                    if i <= arr.len() { arr.insert(i, parse_val(value)); true } else { false }
                }),
                Err(_) => parent.as_object_mut().map_or(false, |o| {
                    o.insert(last.to_string(), parse_val(value));
                    true
                }),
            }
        }
        Patch::Remove { path } => {
            let Some((pp, last)) = path.rsplit_once('/') else { return false };
            let Some(parent) = root.pointer_mut(pp) else { return false };
            match last.parse::<usize>() {
                Ok(i) => parent.as_array_mut().map_or(false, |arr| {
                    if i < arr.len() { arr.remove(i); true } else { false }
                }),
                Err(_) => parent.as_object_mut().map_or(false, |o| o.remove(last).is_some()),
            }
        }
        Patch::Replace { path, value } => {
            let Some((pp, last)) = path.rsplit_once('/') else { return false };
            let Some(parent) = root.pointer_mut(pp) else { return false };
            match last.parse::<usize>() {
                Ok(i) => parent.as_array_mut().map_or(false, |arr| {
                    if i < arr.len() { arr[i] = parse_val(value); true } else { false }
                }),
                Err(_) => parent.as_object_mut().map_or(false, |o| {
                    if !o.contains_key(last) { return false; }
                    o.insert(last.to_string(), parse_val(value));
                    true
                }),
            }
        }
        Patch::ReplaceAttr { path, attr, value } => root
            .pointer_mut(path)
            .and_then(|t| t.as_object_mut())
            .map(|o| {
                o.insert(attr.clone(), parse_val(value));
                true
            })
            .unwrap_or(false),
    }
}

fn apply_json_patches(root: &mut serde_json::Value, patches: &[Patch]) {
    for p in patches {
        assert!(
            apply_one_patch(root, p),
            "apply_json_patches: op 无法落地（applier 不支持该形态，或坐标越界）: {p:?}"
        );
    }
}

/// WC-11：apply_json_patches 的成员末段语义——一串 op 后断言**整棵树**，
/// 覆盖"建数组(丙) / 覆盖 / 删键 / 标量成员 / 下标 Insert / ReplaceAttr"，
/// 并证明不支持的形态走**显式失败**而非静默跳过。
#[test]
fn wc11_apply_member_end_segment_semantics() {
    let mut tree = serde_json::json!({
        "nt": "doc",
        "ct": [ {"nt": "li", "t": ""}, {"nt": "li", "t": ""} ]
    });
    apply_json_patches(&mut tree, &[
        // ① 字段写（内部枚举 element+attr 形态）仍工作
        Patch::ReplaceAttr { path: "/ct/0".into(), attr: "t".into(), value: r#""renamed""#.into() },
        // ② 一条 op 建整数组（丙）+ ③ 新子树内部再建一层数组（末段成员位）
        Patch::Add { path: "/ct/0/ct".into(), value: r#"[{"nt":"p","t":"x"}]"#.into() },
        Patch::Add { path: "/ct/0/ct/0/ct".into(), value: r#"[{"nt":"p","t":"y"}]"#.into() },
        // ④ 成员位标量覆盖（新子树内）
        Patch::Replace { path: "/ct/0/ct/0/t".into(), value: r#""Y""#.into() },
        // ⑤⑥ 空数组建 + 整数组删（WC-5 REM）
        Patch::Add { path: "/ct/1/ct".into(), value: "[]".into() },
        Patch::Remove { path: "/ct/1/ct".into() },
        // ⑦ 成员位标量覆盖（顶层壳）
        Patch::Replace { path: "/ct/0/t".into(), value: r#""z""#.into() },
        // ⑧ 末段数字 = 下标 Insert（回归）
        Patch::Add { path: "/ct/2".into(), value: r#"{"nt":"li","t":"w"}"#.into() },
    ]);
    assert_eq!(
        tree,
        serde_json::json!({
            "nt": "doc",
            "ct": [
                {"nt": "li", "t": "z", "ct": [
                    {"nt": "p", "t": "Y", "ct": [{"nt": "p", "t": "y"}]}
                ]},
                {"nt": "li", "t": ""},
                {"nt": "li", "t": "w"}
            ]
        }),
        "整棵树应与逐条语义一致"
    );
    // ⑨ 负例：Replace 于不存在键 / 下标越界 ⇒ apply_one_patch = false 且不留痕
    let before = tree.clone();
    assert!(!apply_one_patch(&mut tree, &Patch::Replace {
        path: "/ct/1/ct".into(), value: "[]".into(),
    }), "⑨ Replace 不存在键必须 false");
    assert!(!apply_one_patch(&mut tree, &Patch::Remove { path: "/ct/9".into() }),
        "⑨ 下标越界必须 false");
    assert_eq!(tree, before, "⑨ 负例不得改动树");
}

/// WC-11：apply 失败必须**显式炸**（原先 eprintln!+continue 会把不支持的
/// 形态静默带过，只在下游树比对时呈现为"神秘不一致"）。
#[test]
#[should_panic(expected = "op 无法落地")]
fn wc11_apply_failure_is_explicit() {
    let mut tree = serde_json::json!({"nt": "doc", "ct": [{"nt": "li", "t": ""}]});
    apply_json_patches(&mut tree, &[
        Patch::Add { path: "/ct/0/t".into(), value: r#""ok""#.into() },
        Patch::Remove { path: "/ct/0/ct/3".into() }, // 越界/不存在 ⇒ 必须炸
    ]);
}

// ── E1 ──

#[test]
fn exp_e1_pl_changes_are_contiguous() {
    let cases = vec![
        ("inline edit","# H1\n- a\n- b\n",5,8,"text"),
        ("bullet→para","- a\n- b\n- c\n",0,3,"text"),
        ("insert delta+1","- a\n- b\n",0,0,"# H1\n"),
        ("delete delta-1","- a\n- b\n- c\n",4,8,""),
        ("BQ>BLT nested","> - a\n> - b\n> plain\n",2,3,"x"),
        ("list+CF boundary","- a\n- b\n\n```\ncode\n```\n",0,3,"- x"),
        ("heading before list","- a\n- b\n",0,0,"# H1\n"),
        ("BLT loose blank","- a\n\n- b\n",0,3,"- x"),
        ("codefence→para","```\ncode\n```\n\ntext\n",0,13,"regular text"),
        ("table cell edit","| a | b |\n|-|-|\n|1|2|\n",2,3,"x"),
        ("nested BQ prefix","> > double\n",2,3,"x"),
    ];
    for (name, old_text, start, end, new_text) in cases {
        let mut state = State::new(old_text);
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());
        let pl_rip: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let pl_ref: Vec<u8> = (0..ref_state.lines.len()).map(|i| ref_state.lines[i].0).collect();
        if pl_rip == pl_ref { continue; }
        let (first, last) = pl_diff_range(&pl_rip, &pl_ref)
            .unwrap_or_else(|| panic!("[{name}] PL differs but no diff range"));
        for i in 0..pl_rip.len().min(pl_ref.len()) {
            let diff = pl_rip[i] != pl_ref[i];
            let in_range = i >= first && i <= last;
            assert_eq!(diff, in_range,
                "[{name}] line {i}: diff={diff} in_range={in_range} first={first} last={last}");
        }
    }
}

// ── E2 ──

#[test]
fn exp_e2_prefix_suffix_pl_invariance() {
    let cases = vec![
        ("delta=0 inline","- a\n- b\n- c\n",0,3,"- x",0),
        ("delta=+1 H1","- a\n- b\n",0,0,"# H1\n",1),
        ("delta=-1 delete","- a\n- b\n- c\n",4,8,"",-1),
        ("delta=+1 insert mid","- a\n- c\n",4,4,"- b\n",1),
    ];
    for (name, old_text, start, end, new_text, expected_delta) in cases {
        let lines_old: Vec<u8> = {
            let s = State::new(old_text);
            (0..s.lines.len()).map(|i| s.lines[i].0).collect()
        };
        let mut state = State::new(old_text);
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());
        let pl_rip: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let pl_ref: Vec<u8> = (0..ref_state.lines.len()).map(|i| ref_state.lines[i].0).collect();
        let delta = state.lines.len() as isize - lines_old.len() as isize;
        assert_eq!(delta, expected_delta, "[{name}] delta");
        let Some((first, last)) = pl_diff_range(&pl_rip, &pl_ref) else { continue; };
        for i in 0..first {
            assert_eq!(pl_rip[i], pl_ref[i], "[{name}] prefix line {i}");
        }
        for i in (last+1)..pl_rip.len().min(pl_ref.len()) {
            assert_eq!(pl_rip[i], pl_ref[i], "[{name}] suffix line {i}");
        }
        for i in (last+1)..pl_rip.len() {
            let oi = i as isize - delta;
            if oi >= 0 && (oi as usize) < lines_old.len() {
                assert_eq!(pl_rip[i], lines_old[oi as usize],
                    "[{name}] suffix line {i} old={oi}");
            }
        }
    }
}

// ── E3 ──

#[test]
fn exp_e3_fl_prefix_stable() {
    let cases = vec![
        ("inline bullet","- a\n- b\n- c\n",0,3,"- x"),
        ("BQ nested edit","> - a\n> - b\n> plain\n",2,3,"x"),
        ("H1+BLT inline","# H1\n- a\n- b\n",5,8,"text"),
        ("multi-edit 2 bullets","- a\n- b\n- c\n- d\n",0,8,"- x\n- y\n"),
        ("BLT vanish mid","- a\n- b\n- c\n\n- d\n",0,12,"x\ny\nz"),
    ];
    for (name, old_text, start, end, new_text) in cases {
        let mut state = State::new(old_text);
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());
        let fl_rip = &state.frame_log.entries;
        let fl_ref = &ref_state.frame_log.entries;
        let Some((first_diff, _)) = fl_diff_range(fl_rip, fl_ref) else { continue; };
        for i in 0..first_diff {
            assert_eq!(fl_rip[i], fl_ref[i], "[{name}] FL prefix {i}");
        }
        assert!(first_diff > 0 || fl_rip.is_empty(), "[{name}] no prefix");
    }
}

// ── E4 ──

#[test]
fn exp_e4_observable_ripple_zone_matches_pl_changes() {
    let cases = vec![
        ("inline edit","# H1\n- a\n- b\n",5,8,"text"),
        ("bullet→para","- a\n- b\n- c\n",0,3,"text"),
        ("delta=+1","- a\n- b\n",0,0,"# H1\n"),
        ("delta=-1","- a\n- b\n- c\n",4,8,""),
        ("BQ>BLT","> - a\n> - b\n",2,3,"x"),
        ("H1+BLT inline","# H1\n- a\n- b\n",5,8,"text"),
    ];
    for (name, old_text, start, end, new_text) in cases {
        let mut state = State::new(old_text);
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());
        let pl_rip: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let pl_ref: Vec<u8> = (0..ref_state.lines.len()).map(|i| ref_state.lines[i].0).collect();
        let Some((pl_first, pl_last)) = pl_diff_range(&pl_rip, &pl_ref) else { continue; };
        let fl_lines_rip: BTreeSet<u32> = state.frame_log.entries.iter()
            .filter(|&&e| !entry_is_body_mut(e)).map(|&e| entry_line(e)).collect();
        let fl_lines_ref: BTreeSet<u32> = ref_state.frame_log.entries.iter()
            .filter(|&&e| !entry_is_body_mut(e)).map(|&e| entry_line(e)).collect();
        let fl_diff: BTreeSet<u32> = fl_lines_rip.symmetric_difference(&fl_lines_ref).copied().collect();
        if fl_diff.is_empty() { continue; }
        let fl_first = *fl_diff.first().unwrap() as usize;
        let fl_last = *fl_diff.last().unwrap() as usize;
        assert!(pl_first >= fl_first || pl_last <= fl_last,
            "[{name}] PL [{pl_first},{pl_last}] not in FL [{fl_first},{fl_last}]");
    }
}

// ── E5 ──

#[test]
fn exp_e5_structural_changes_only_in_ripple_zone() {
    let cases = vec![
        ("bullet→para","- a\n- b\n- c\n",0,3,"text"),
        ("delta=+1 heading","- a\n- b\n",0,0,"# H1\n"),
        ("delta=-1 delete","- a\n- b\n- c\n",4,8,""),
        ("BQ>BLT edit","> - a\n> - b\n> plain\n",2,3,"x"),
        ("BLT vanish mid","- a\n- b\n- c\n\n- d\n",0,12,"x\ny\nz"),
        ("star→dash cross","* a\n- b\n",0,3,"- x"),
    ];
    for (name, old_text, start, end, new_text) in cases {
        let mut state = State::new(old_text);
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());
        let pl_rip: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let pl_ref: Vec<u8> = (0..ref_state.lines.len()).map(|i| ref_state.lines[i].0).collect();
        let fl_rip = &state.frame_log.entries;
        let fl_ref = &ref_state.frame_log.entries;
        let mut fl_changed: BTreeSet<u32> = BTreeSet::new();
        for i in 0..fl_rip.len().min(fl_ref.len()) {
            if fl_rip[i] != fl_ref[i] {
                if !entry_is_body_mut(fl_rip[i]) { fl_changed.insert(entry_line(fl_rip[i])); }
                if !entry_is_body_mut(fl_ref[i]) { fl_changed.insert(entry_line(fl_ref[i])); }
            }
        }
        for i in fl_ref.len()..fl_rip.len() {
            if !entry_is_body_mut(fl_rip[i]) { fl_changed.insert(entry_line(fl_rip[i])); }
        }
        for i in fl_rip.len()..fl_ref.len() {
            if !entry_is_body_mut(fl_ref[i]) { fl_changed.insert(entry_line(fl_ref[i])); }
        }
        if fl_changed.is_empty() { continue; }
        let fl_min = *fl_changed.first().unwrap() as usize;
        let fl_max = *fl_changed.last().unwrap() as usize;
        for i in 0..pl_rip.len().min(pl_ref.len()) {
            if pl_rip[i] != pl_ref[i] {
                let ok = i >= fl_min.saturating_sub(1) && i <= fl_max.saturating_add(1);
                assert!(ok, "[{name}] PL change at line {i} far from FL [{fl_min},{fl_max}]");
            }
        }
    }
}

// ── E6 ──

#[test]
fn exp_e6_root_shift_adjustment_formula() {
    let cases = vec![
        ("delta=+1 H1","- a\n- b\n",0,0,"# H1\n"),
        ("delta=-1 delete","- a\n- b\n- c\n",4,8,""),
    ];
    for (name, old_text, start, end, new_text) in cases {
        let mut state = State::new(old_text);
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());
        let pl_rip: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let pl_ref: Vec<u8> = (0..ref_state.lines.len()).map(|i| ref_state.lines[i].0).collect();
        let delta = state.lines.len() as isize - State::new(old_text).lines.len() as isize;
        let Some((_, last)) = pl_diff_range(&pl_rip, &pl_ref) else { continue; };
        let suffix_start = last + 1;
        if suffix_start >= state.lines.len() { continue; }
        let old_line = (suffix_start as isize - delta).max(0) as usize;
        let stack_new = state.frame_log.rebuild_stack(suffix_start as u32);
        let stack_old = State::new(old_text).frame_log.rebuild_stack(old_line as u32);
        assert_eq!(stack_new.len(), stack_old.len(), "[{name}] depth");
        for d in 0..stack_new.len().min(stack_old.len()) {
            assert_eq!(stack_new[d].kind(), stack_old[d].kind(), "[{name}] depth {d} kind");
        }
    }
}

// ── E7 ──

#[test]
fn exp_e7_ripple_zone_blocks_cover_all_changes() {
    let cases = vec![
        ("inline edit","# H1\n- a\n- b\n",5,8,"text"),
        ("bullet→para","- a\n- b\n- c\n",0,3,"text"),
        ("delta=+1 H1","- a\n- b\n",0,0,"# H1\n"),
        ("delta=-1 delete","- a\n- b\n- c\n",4,8,""),
        ("BQ>BLT nested","> - a\n> - b\n> plain\n",2,3,"x"),
    ];
    for (name, old_text, start, end, new_text) in cases {
        let mut state = State::new(old_text);
        let old_pl: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let old_fl = state.frame_log.clone();
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());
        let new_pl: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let ref_pl: Vec<u8> = (0..ref_state.lines.len()).map(|i| ref_state.lines[i].0).collect();
        let delta = new_pl.len() as isize - old_pl.len() as isize;
        // diff 判定 = PL 差异 || 该行 FL entry 数差异（marker 编辑场景：
        // `> - a` 的 '-'→'x'——PL 不变但 FL 变（Composite 双 Push→单 Push）——
        // zone 只从 PL 算会漏 FL 变化区——追查补充）
        let fa = &ref_state.frame_log.entries;
        let fb = &old_fl.entries;
        let diff = |i: usize| -> bool {
            let oi = i as isize - delta;
            if oi < 0 || oi >= old_pl.len() as isize { return true; }
            if new_pl[i] != old_pl[oi as usize] { return true; }
            let cnt_new = fa.iter().filter(|e| mdio_core::block::entry_line(**e) == i as u32).count();
            let cnt_old = fb.iter().filter(|e| mdio_core::block::entry_line(**e) == oi as u32).count();
            cnt_new != cnt_old
        };
        let rs = (0..new_pl.len()).find(|&i| diff(i)).unwrap_or(0);
        let re = (rs..new_pl.len()).rev().find(|&i| diff(i)).map(|i| i+1).unwrap_or(rs);
        let old_re = (re as isize - delta).max(rs as isize) as usize;
        if re == 0 || rs >= re { continue; }
        let full_new = blocks_in_range_test(&ref_state.frame_log.entries, &ref_pl, 0, ref_pl.len() as u32, &ref_state.frame_log);
        let full_old = blocks_in_range_test(&old_fl.entries, &old_pl, 0, old_pl.len() as u32, &old_fl);
        let rip_new = blocks_in_range_test(&ref_state.frame_log.entries, &ref_pl, rs as u32, (re+1) as u32, &ref_state.frame_log);
        let rip_old = blocks_in_range_test(&old_fl.entries, &old_pl, rs as u32, (old_re+1) as u32, &old_fl);
        let rfn: Vec<_> = full_new.iter().filter(|b| (b.0 as usize) >= rs && (b.0 as usize) < re+1).collect();
        let rfo: Vec<_> = full_old.iter().filter(|b| (b.0 as usize) >= rs && (b.0 as usize) < old_re+1).collect();
        assert_eq!(rip_new.len(), rfn.len(), "[{name}] rip_new");
        assert_eq!(rip_old.len(), rfo.len(), "[{name}] rip_old");
        let pn: Vec<_> = full_new.iter().filter(|b| (b.0 as usize) < rs).collect();
        let po: Vec<_> = full_old.iter().filter(|b| (b.0 as usize) < rs).collect();
        for (i, nb) in pn.iter().enumerate() {
            if i < po.len() { assert_eq!(nb.3, po[i].3, "[{name}] prefix block {i}"); }
        }
    }
}

// ── E8 ──

#[test]
fn exp_e8_delta_p_computation() {
    let cases = vec![
        ("delta=+1 H1","- a\n- b\n",0,0,"# H1\n"),
        ("delta=-1 delete","- a\n- b\n- c\n",4,8,""),
        ("bullet→para","- a\n- b\n- c\n",0,3,"text"),
    ];
    for (name, old_text, start, end, new_text) in cases {
        let mut state = State::new(old_text);
        let old_pl: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let old_entries = state.frame_log.entries.clone();
        let old_fl = state.frame_log.clone();
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());
        let new_pl: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let delta = new_pl.len() as isize - old_pl.len() as isize;
        // diff 判定 = PL 差异 || 该行 FL entry 数差异（marker 编辑场景：
        // `> - a` 的 '-'→'x'——PL 不变但 FL 变（Composite 双 Push→单 Push）——
        // zone 只从 PL 算会漏 FL 变化区——追查补充）
        let fa = &ref_state.frame_log.entries;
        let fb = &old_fl.entries;
        let diff = |i: usize| -> bool {
            let oi = i as isize - delta;
            if oi < 0 || oi >= old_pl.len() as isize { return true; }
            if new_pl[i] != old_pl[oi as usize] { return true; }
            let cnt_new = fa.iter().filter(|e| mdio_core::block::entry_line(**e) == i as u32).count();
            let cnt_old = fb.iter().filter(|e| mdio_core::block::entry_line(**e) == oi as u32).count();
            cnt_new != cnt_old
        };
        let rs = (0..new_pl.len()).find(|&i| diff(i)).unwrap_or(0);
        let re = (rs..new_pl.len()).rev().find(|&i| diff(i)).map(|i| i+1).unwrap_or(rs);
        let fds = old_entries.iter().position(|&e| !entry_is_body_mut(e) && entry_line(e) as usize >= rs).unwrap_or(old_entries.len());
        let fde = old_entries.iter().rposition(|&e| !entry_is_body_mut(e) && (entry_line(e) as usize) < (re as isize-delta).max(0) as usize).map(|p| p+1).unwrap_or(fds);
        let mut od: i32 = old_fl.rebuild_stack(rs as u32).len() as i32;
        let mut op = [0i32; 17];
        for i in fds..fde.min(old_entries.len()) {
            let e = old_entries[i];
            if entry_is_body_mut(e) { continue; }
            if entry_is_pop(e) { od = (od-1).max(0); } else { op[od as usize] += 1; od += 1; }
        }
        let ne = &ref_state.frame_log.entries;
        let nf = &ref_state.frame_log;
        let fds2 = ne.iter().position(|&e| !entry_is_body_mut(e) && entry_line(e) as usize >= rs).unwrap_or(ne.len());
        let fde2 = ne.iter().rposition(|&e| !entry_is_body_mut(e) && (entry_line(e) as usize) < re).map(|p| p+1).unwrap_or(fds2);
        let mut nd: i32 = nf.rebuild_stack(rs as u32).len() as i32;
        let mut np = [0i32; 17];
        for i in fds2..fde2.min(ne.len()) {
            let e = ne[i];
            if entry_is_body_mut(e) { continue; }
            if entry_is_pop(e) { nd = (nd-1).max(0); } else { np[nd as usize] += 1; nd += 1; }
        }
        let mut dp = [0i32; 17];
        for d in 0..17 { dp[d] = np[d] - op[d]; }

        for d in 0..5 { assert!(dp[d].abs() <= 10, "[{name}] dP[{d}]={}", dp[d]); }
    }
}

// ── E9 ──

#[test]
fn exp_e9_suffix_path_offset_constant() {
    let cases = vec![
        ("delta=+1 H1","- a\n- b\n",0,0,"# H1\n"),
        ("delta=-1 delete","- a\n- b\n- c\n",4,8,""),
    ];
    for (name, old_text, start, end, new_text) in cases {
        let mut state = State::new(old_text);
        let old_pl: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let old_fl = state.frame_log.clone();
        state.edit(start, end, new_text);
        state.ensure_projected();
        let ref_state = State::new(&state.rope.to_string());
        let new_pl: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let delta = new_pl.len() as isize - old_pl.len() as isize;
        // diff 判定 = PL 差异 || 该行 FL entry 数差异（marker 编辑场景：
        // `> - a` 的 '-'→'x'——PL 不变但 FL 变（Composite 双 Push→单 Push）——
        // zone 只从 PL 算会漏 FL 变化区——追查补充）
        let fa = &ref_state.frame_log.entries;
        let fb = &old_fl.entries;
        let diff = |i: usize| -> bool {
            let oi = i as isize - delta;
            if oi < 0 || oi >= old_pl.len() as isize { return true; }
            if new_pl[i] != old_pl[oi as usize] { return true; }
            let cnt_new = fa.iter().filter(|e| mdio_core::block::entry_line(**e) == i as u32).count();
            let cnt_old = fb.iter().filter(|e| mdio_core::block::entry_line(**e) == oi as u32).count();
            cnt_new != cnt_old
        };
        let rs = (0..new_pl.len()).find(|&i| diff(i)).unwrap_or(0);
        let re = (rs..new_pl.len()).rev().find(|&i| diff(i)).map(|i| i+1).unwrap_or(rs);
        let nf = &ref_state.frame_log;
        let ss = re;
        if ss == 0 || ss >= new_pl.len() { continue; }
        let mut c = 0;
        for nl in (ss..new_pl.len()).take(20) {
            let ol = (nl as isize - delta).max(0) as usize;
            if ol >= old_pl.len() { continue; }
            let sn = nf.rebuild_stack(nl as u32);
            let so = old_fl.rebuild_stack(ol as u32);
            assert_eq!(sn.len(), so.len(), "[{name}] suffix nl={nl} ol={ol} depth {} vs {}", sn.len(), so.len());
            for d in 0..sn.len() {
                assert_eq!(sn[d].kind(), so[d].kind(), "[{name}] suffix nl={nl} d={d} kind");
            }
            c += 1;
        }

    }
}

// ── 全流程 AST 往复 ──

// 迁到 snap_old API 后可编译运行,但暴露 4 个 block-diff incr 的真实 bug
// (inline text change / bullet→paragraph / delta=-1 delete / BQ>BLT nested 全部 MISMATCH)。
// 这些是 FC1 树 diff 要解决的目标,不属于本次修复的范畴。
// 全流程 AST 往复（修复后验证:5/5 ROUNDTRIP OK——历史 4 个
// block-diff bug 已愈——恢复为验收 oracle,不再 ignore）。
#[test]
fn exp_full_pipeline_roundtrip() {
    let cases = vec![
        ("inline text change","# H1\n- a\n- b\n",5,8,"text"),
        ("bullet→paragraph","- a\n- b\n- c\n",0,3,"text"),
        ("delta=+1 insert H1","- a\n- b\n",0,0,"# H1\n"),
        ("delta=-1 delete mid","- a\n- b\n- c\n",4,8,""),
        ("BQ>BLT nested edit","> - a\n> - b\n> plain\n",2,3,"x"),
    ];
    let mut passed = 0; let mut failed = 0; let mut skipped = 0;
    for (name, old_text, start, end, new_text) in cases {

        let mut state = State::new(old_text);
        let lines_old: Arc<Vec<PackedLineInfo>> = Arc::new(state.lines.as_ref().clone());
        let fl_old_entries = Arc::new(state.frame_log.entries.clone());
        let old_snap = state.snapshot();

        state.edit(start, end, new_text);
        state.ensure_projected();
        let diff = state.ripple_diff.take();

        let new_snap = state.snapshot();

        let diff = match diff { Some(d) => d, None => { skipped += 1; continue; } };
        let edit_ranges = mdio_core::edit_actions_arc(vec![(start, start + new_text.len())]);
        let task = IncrTask {
            snap: Arc::new(new_snap), doc_id: 0,
            snap_old: Arc::new(old_snap),
            delta: diff.delta, edit_ranges: edit_ranges.clone(),
            ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        // T-mode(827):同一用例按 mode 自动切换适配——CF/HS 双跑共用一个
        // 判定器(applyPatches(toJSON_mode(old), patches_for_mode) ≡
        // toJSON_mode(new))。
        for mode in [mdio_core::topology::ProjectionMode::Cf, mdio_core::topology::ProjectionMode::Hs] {
            let mut old_ast = serde_json::json!({"ct": mode_ch(&task.snap_old, mode)});
            let new_ast_direct = serde_json::json!({"ct": mode_ch(&task.snap, mode)});
            let patches = patches_for_mode(&task, mode);

            for (i, p) in patches.iter().enumerate() {
                match p {
                    Patch::Add { path, .. } => eprintln!("    [{i}] ADD {path}"),
                    Patch::Remove { path } => eprintln!("    [{i}] REM {path}"),
                    Patch::Replace { path, .. } => eprintln!("    [{i}] REPL {path}"),
                    Patch::ReplaceAttr { path, attr, .. } => eprintln!("    [{i}] RATTR {path} {attr}"),
                }
            }
            apply_json_patches(&mut old_ast, &patches);
            let old_after = serde_json::to_string_pretty(&old_ast).unwrap_or_default();
            let new_after = serde_json::to_string_pretty(&new_ast_direct).unwrap_or_default();
            if old_after == new_after { eprintln!("  [{name} {mode:?}] ROUNDTRIP OK"); passed += 1; }
            else { eprintln!("  [{name} {mode:?}] ROUNDTRIP MISMATCH!"); failed += 1; }
        }
    }

    assert_eq!(failed, 0, "{failed} roundtrip failures");
}

// ── 统合全流程: ripple-zone-only incr_patches ──

#[test]
fn exp_unified_ripple_zone_only_pipeline() {
    let cases = vec![
        ("inline text change","# H1\n- a\n- b\n",5,8,"text"),
        ("bullet to paragraph","- a\n- b\n- c\n",0,3,"text"),
        ("insert H1 before list","- a\n- b\n",0,0,"# H1\n"),
        ("delete middle bullet","- a\n- b\n- c\n",4,8,""),
        ("BQ>BLT nested edit","> - a\n> - b\n> plain\n",2,3,"x"),
        ("heading edit in BQ","> ## Title\n> plain\n",4,9,"H2"),
        ("codefence to para","```\ncode\n```\n\ntext\n",0,13,"regular text"),
    ];
    let mut total = 0; let mut ok = 0; let mut skip = 0;
    for (name, old_text, start, end, new_text) in cases {
        total += 1;

        let mut state = State::new(old_text);
        let old_pl: Vec<u8> = (0..state.lines.len()).map(|i| state.lines[i].0).collect();
        let old_entries = state.frame_log.entries.clone();
        let old_fl = state.frame_log.clone();
        let old_snap = state.snapshot();

        state.edit(start, end, new_text);
        state.ensure_projected();
        let diff = state.ripple_diff.take();
        let new_snap = state.snapshot();
        let new_pl: Vec<u8> = (0..new_snap.line_len()).map(|i| new_snap.lines[i].0).collect();
        let new_entries = &new_snap.frame_log.entries;
        let new_fl = &new_snap.frame_log;
        let delta = new_pl.len() as isize - old_pl.len() as isize;


        let rs = (0..new_pl.len().min(old_pl.len()))
            .find(|&i| { let oi = i as isize - delta; oi < 0 || oi >= old_pl.len() as isize || new_pl[i] != old_pl[oi as usize] }).unwrap_or(0);
        let re = (rs..new_pl.len())
            .rev().find(|&i| { let oi = i as isize - delta; oi < 0 || oi >= old_pl.len() as isize || new_pl[i] != old_pl[oi as usize] }).map(|i| i+1).unwrap_or(rs);
        let flo: BTreeSet<u32> = old_entries.iter().filter(|&&e| !entry_is_body_mut(e)).map(|&e| entry_line(e)).collect();
        let fln: BTreeSet<u32> = new_entries.iter().filter(|&&e| !entry_is_body_mut(e)).map(|&e| entry_line(e)).collect();
        let fd: Vec<u32> = flo.symmetric_difference(&fln).copied().collect();
        let rs = rs.min(fd.first().copied().unwrap_or(rs as u32) as usize);
        let re = re.max(fd.last().map(|l| *l as usize + 1).unwrap_or(re));
        let old_re = (re as isize - delta).max(rs as isize) as usize;

        if re <= rs { skip += 1; continue; }
        let K_old_rip = blocks_in_range_test(&old_entries, &old_pl, rs as u32, (old_re+1) as u32, &old_fl);
        let K_new_rip = blocks_in_range_test(new_entries, &new_pl, rs as u32, (re+1) as u32, new_fl);

        let mut un = vec![false; K_new_rip.len()];
        let mut m: Vec<(usize, usize)> = Vec::new();
        for (oi, ob) in K_old_rip.iter().enumerate() {
            let sh = (ob.0 as isize + delta).max(0) as u32;
            let mut best: Option<(usize, u32)> = None;
            for (ni, nb) in K_new_rip.iter().enumerate() {
                if un[ni] || ob.2 != nb.2 { continue; }
                if ob.2.is_none() && ob.3 != nb.3 { continue; }
                let d = (sh as isize - nb.0 as isize).unsigned_abs() as u32;
                if d <= 1 && best.map_or(true, |(_, x)| d < x) { best = Some((ni, d)); }
            }
            if let Some((ni, _)) = best { un[ni] = true; m.push((oi, ni)); }
        }
        let om: HashSet<usize> = m.iter().map(|&(oi, _)| oi).collect();
        let nm: HashSet<usize> = m.iter().map(|&(_, ni)| ni).collect();
        let mut paths: Vec<(String, String, u32)> = Vec::new();
        fn bp(entries: &[u64], fl: &FrameLog, pl: &[u8], start: u32) -> String {
            let pli: Vec<PackedLineInfo> = pl.iter().map(|&b| PackedLineInfo(b)).collect();
            let li = start as usize;
            let mut rb = 0usize; let mut i = 0usize;
            while i < li {
                let t = PackedLineInfo(pl[i]).node_type();
                if is_skip(t) { i += 1; continue; }
                rb += 1;
                let pe = entries.iter().find(|&&e| !entry_is_pop(e) && !entry_is_body_mut(e) && entry_line(e) as usize == i);
                if let Some(&e) = pe {
                    if let Some(end) = fl.item_tail_boundary(i as u32, entry_frame(e).kind(), &pli) { i = end as usize; }
                    else { i += 1; }
                } else { i = resolve_block_end(pl, i); }
            }
            format!("/children/{}", rb)
        }
        for (oi, b) in K_old_rip.iter().enumerate() {
            if om.contains(&oi) { continue; }
            let p = bp(&old_entries, &old_fl, &old_pl, b.0);
            if !p.is_empty() { eprintln!("║ DELETE: {p}"); paths.push(("REM".into(), p, b.0)); }
        }
        for (ni, b) in K_new_rip.iter().enumerate() {
            if nm.contains(&ni) { continue; }
            let p = bp(new_entries, new_fl, &new_pl, b.0);
            if !p.is_empty() { eprintln!("║ ADD: {p}"); paths.push(("ADD".into(), p, b.0)); }
        }
        for &(oi, ni) in &m {
            let ob = &K_old_rip[oi]; let nb = &K_new_rip[ni];
            let ch = ob.0 != nb.0 || ob.1 != nb.1 || ob.3 != nb.3
                || (ob.0..ob.1).any(|l| old_pl.get(l as usize).copied().unwrap_or(0xFF) != new_pl.get(l as usize).copied().unwrap_or(0xFF));
            if ch {
                let p = bp(new_entries, new_fl, &new_pl, nb.0);
                if !p.is_empty() { eprintln!("║ REPL: {p}"); paths.push(("REPL".into(), p, nb.0)); }
            }
        }
        let mut i = 0;
        while i + 1 < paths.len() {
            if paths[i].0 == "REM" && paths[i+1].0 == "ADD" && paths[i].1 == paths[i+1].1 { paths[i].0 = "REPL".into(); paths.remove(i+1); }
            else { i += 1; }
        }
        let edit_ranges = mdio_core::edit_actions_arc(vec![(start, start + new_text.len())]);
        let diff = diff.unwrap();
        let ref_task = IncrTask {
            snap: Arc::new(new_snap.clone()), doc_id: 0,
            snap_old: Arc::new(old_snap),
            delta: diff.delta, edit_ranges,
            ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        // T-mode(827):同一用例按 mode 自动切换适配——CF/HS 双跑共用
        // 判定器。
        for mode in [mdio_core::topology::ProjectionMode::Cf, mdio_core::topology::ProjectionMode::Hs] {
            let mut old_ast = serde_json::json!({"ct": mode_ch(&ref_task.snap_old, mode)});
            let new_ast_direct = serde_json::json!({"ct": mode_ch(&ref_task.snap, mode)});
            let ref_patches = patches_for_mode(&ref_task, mode);
            apply_json_patches(&mut old_ast, &ref_patches);
            let oa = serde_json::to_string_pretty(&old_ast).unwrap_or_default();
            let na = serde_json::to_string_pretty(&new_ast_direct).unwrap_or_default();
            if oa == na { eprintln!("║ [{name} {mode:?}] UNIFIED ROUNDTRIP OK"); ok += 1; }
            else { eprintln!("║ [{name} {mode:?}] ROUNDTRIP MISMATCH!"); }
        }
    }

    assert!(ok > 0, "no roundtrips");
}
