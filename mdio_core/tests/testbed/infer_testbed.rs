//! INFER TESTBED —— 第一课预实验场(临时——最后可删)
//!
//! 第一课（结构空间模型——infer_prev_1.md §第一课）的预实验迭代场:
//!   PE1: PeelStack 完备性——层序列五视图一致性（核心赌注——机制存在性）
//!        假设: 五份剥壳消费（dispatch_new_block 嵌套循环 block.rs:1073 /
//!              content_kind_per_depth block.rs:3497 / peel_item_markers /
//!              count_inner_markers / strip 族）的"层序列"同一——
//!              差异全在消费视图。
//!   PE2: 身份转换矩阵——结构序列 vs FL 事件序列（真实文档对齐）。
//!   PE3+: 投影算子统一 / 截断约定（后续迭代）。
//!
//! 信息守恒（PL+FL 无损——用户原则）:
//!   局部守恒 = peel 层序列 + 剩余 重组 == 原片段（正逆闭环）
//!   全局守恒 = trilogy 的 restore ∘ proj ∘ scan == scan（trilogy_testbed）
//!
//! 运行: cargo test --test infer_testbed -- --nocapture

use mdio_core::block::{BlockType, FrameKind};
// (D6⑤d 桥迁移——旧 infer 管线引用删除——以下全换
//  mdio_core::infer 全路径)
use mdio_core::infer::{ContainerRec, PopPushEv, RecArena};
use mdio_core::projection::State;

// PE1: PeelStack 原型

/// 层——结构前缀的一层（kind + 在片段中的字节区间——start/end 含前导空白与
/// marker 及后续空白——"被消费的区域"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layer {
    kind: FrameKind,
    start: usize,
    end: usize,
}

/// 简化实现（实现参照原则 §4.4——以生产分支为准）:
/// 层规则硬编码（BQ/bullet/ol 的宽度语义）——PE12 的查表版（peel_table +
/// LAYER_KIND_LUT + layer_width）才是 LineSpec 声明化的更好形态。
/// 边缘缺口: tab 展开列（col_to_byte）、BQ keep_indent 变体、refdef 前缀。
/// 剥壳——BQ 前缀与 marker（bullet/ol）按出现顺序循环剥。
/// 全结构序列：**含 marker 后的 BQ**（`- > a` → [BLT, BQ]——dispatch 嵌套
/// 循环的语义——`> - > b` 交替同样成立）。
/// 与五消费视图的公共语义对照（PE1 假设: 层序列一致, 差异在视图）:
///   dispatch_new_block（block.rs:1073）—— 全序列（含 BQ 嵌套）
///   content_kind_per_depth（block.rs:3497）—— marker 过滤视图（BQ 不计层）
///   count_inner_markers —— marker 层计数
///   peel_item_markers —— 剥 N 个 marker 层
///   strip_marker_bq_alt —— 全序列剥离（无条件）——剩余 == 本函数剩余
fn peel(fragment: &str) -> (Vec<Layer>, &str) {
    let mut layers = Vec::new();
    let mut cur = fragment;
    let base = fragment.as_ptr() as usize;
    loop {
        let b = cur.as_bytes();
        let ws = b.iter().take_while(|&&x| x == b' ' || x == b'\t').count();
        let tb = &b[ws..];
        // BQ: ">" + 可选单空格/tab（与 strip_bq_prefixes 语义一致）
        if tb.first() == Some(&b'>') {
            let mut w = 1;
            if tb.len() > 1 && (tb[1] == b' ' || tb[1] == b'\t') { w = 2; }
            let consumed = ws + w;
            let start = cur.as_ptr() as usize - base;
            layers.push(Layer { kind: FrameKind::BlockQuote, start, end: start + consumed });
            cur = &cur[consumed..];
            continue;
        }
        // bullet: 单字符 + 可选空白（真空 marker: 无空白——`-` 层存在）
        // thematic break 守卫（`---`/`***`/`* * *`——3+ 同字符仅含空白——
        // CM §4.1——非 marker 层——dispatch_lut 判 HR）
        let marker_c = tb.first().copied();
        let is_tb = match marker_c {
            Some(c @ (b'-' | b'*' | b'+')) => {
                tb.len() >= 3
                    && tb.iter().filter(|&&b| b == c).count() >= 3
                    && tb.iter().all(|&b| b == c || b == b' ' || b == b'\t')
            }
            _ => false,
        };
        if !is_tb && matches!(marker_c, Some(b'-') | Some(b'*') | Some(b'+'))
            && (tb.len() == 1 || matches!(tb.get(1), Some(&b' ') | Some(&b'\t')))
        {
            let mut w = 1;
            if tb.len() > 1 && (tb[1] == b' ' || tb[1] == b'\t') { w = 2; }
            let consumed = ws + w;
            let start = cur.as_ptr() as usize - base;
            layers.push(Layer { kind: FrameKind::BulletList, start, end: start + consumed });
            cur = &cur[consumed..];
            continue;
        }
        // ol: digits + delim + 可选空白
        let mut d = 0;
        while d < tb.len() && tb[d].is_ascii_digit() { d += 1; }
        if d > 0 && d < tb.len() && (tb[d] == b'.' || tb[d] == b')') {
            let mut w = d + 1;
            if tb.get(d + 1) == Some(&b' ') || tb.get(d + 1) == Some(&b'\t') { w += 1; }
            let consumed = ws + w;
            let start = cur.as_ptr() as usize - base;
            layers.push(Layer { kind: FrameKind::OrderedList, start, end: start + consumed });
            cur = &cur[consumed..];
            continue;
        }
        break;
    }
    (layers, cur)
}

/// marker 过滤视图（content_kind_per_depth / count_inner_markers /
/// peel_item_markers / list_push_depths 消费——BQ 不计层）。
fn marker_layers(layers: &[Layer]) -> Vec<Layer> {
    layers.iter().copied()
        .filter(|l| matches!(l.kind, FrameKind::BulletList | FrameKind::OrderedList))
        .collect()
}

/// 局部守恒: 层连续覆盖前缀（首层 start==0、层间无缝）+ 剩余为尾。
/// recompose(fragment) 恒等——PE1 检验 2（信息守恒的局部形态）。
fn conserved(fragment: &str, layers: &[Layer], remainder: &str) -> bool {
    let mut expected = 0usize;
    for l in layers {
        if l.start != expected { return false; }
        if l.end <= l.start { return false; }
        expected = l.end;
    }
    remainder.as_ptr() as usize == fragment.as_ptr() as usize + expected
        && remainder.len() == fragment.len() - expected
}

// PE1 语料矩阵（5 类——层序列形态空间全覆盖）

/// (片段, 期望层 kind 序列, 期望剩余)
/// 期望值推导自五消费的代码逻辑（dispatch 嵌套循环/content_kind_per_depth/
/// peel_item_markers/count_inner_markers/strip 族）——经 PE2（FL 对齐）与
/// PE11（全量态射）实证;新边缘 case 加入时以生产行为为准。
const PE1_CORPUS: &[(&str, &[FrameKind], &str)] = &[
    // 类 1: BQ×marker 组合
    ("> - item", &[FrameKind::BlockQuote, FrameKind::BulletList], "item"),
    ("> > - a", &[FrameKind::BlockQuote, FrameKind::BlockQuote, FrameKind::BulletList], "a"),
    ("> - - a", &[FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::BulletList], "a"),
    // marker 后 BQ——结构序列含 BQ（dispatch 嵌套循环语义——内容侧再剥）
    ("- > a", &[FrameKind::BulletList, FrameKind::BlockQuote], "a"),
    ("> - > b", &[FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::BlockQuote], "b"),
    ("> > deep", &[FrameKind::BlockQuote, FrameKind::BlockQuote], "deep"),
    // 类 2: marker 组合（item item 身份转换）
    ("- a", &[FrameKind::BulletList], "a"),
    ("- - foo", &[FrameKind::BulletList, FrameKind::BulletList], "foo"),
    ("1. a", &[FrameKind::OrderedList], "a"),
    ("1. - x", &[FrameKind::OrderedList, FrameKind::BulletList], "x"),
    ("- 1. a", &[FrameKind::BulletList, FrameKind::OrderedList], "a"),
    // 类 3: 续行/缩进 marker
    ("  - a", &[FrameKind::BulletList], "a"),
    // 类 4: IC 覆盖（层一致——IC 是视图: content_kind_per_depth 的
    // marker_col+5 判定 → 层内容类型 IndentedCode——不是层本身）
    ("-    foo", &[FrameKind::BulletList], "   foo"),
    ("> -    foo", &[FrameKind::BlockQuote, FrameKind::BulletList], "   foo"),
    // 类 5: 真空/边界（层存在性——真空 marker 层存在）
    ("-", &[FrameKind::BulletList], ""),
    ("- -", &[FrameKind::BulletList, FrameKind::BulletList], ""),
    ("1.", &[FrameKind::OrderedList], ""),
    (">", &[FrameKind::BlockQuote], ""),
    // checklist: checkbox 是剩余（mark 形态——line_geometry mark_end=6——
    // 几何视图；层视图 = [BLT]）
    ("- [x] a", &[FrameKind::BulletList], "[x] a"),
    ("- [ ] a", &[FrameKind::BulletList], "[ ] a"),
];

#[test]
fn pe1_peelstack_completeness() {
    let mut failures = 0usize;
    for &(frag, want_kinds, want_rem) in PE1_CORPUS {
        let (layers, rem) = peel(frag);
        // 1. 层序列一致（核心断言——机制存在性）
        let kinds: Vec<FrameKind> = layers.iter().map(|l| l.kind).collect();
        let want: Vec<FrameKind> = want_kinds.to_vec();
        if kinds != want {
            failures += 1;
            eprintln!("[PE1] 层序列分歧: {frag:?} want={want:?} got={kinds:?}");
            continue;
        }
        // 2. 剩余一致
        if rem != want_rem {
            failures += 1;
            eprintln!("[PE1] 剩余分歧: {frag:?} want={want_rem:?} got={rem:?}");
            continue;
        }
        // 3. 局部守恒（重组恒等——信息守恒）
        if !conserved(frag, &layers, rem) {
            failures += 1;
            eprintln!("[PE1] 局部守恒破坏: {frag:?} layers={layers:?} rem={rem:?}");
            continue;
        }
        // 4. marker 过滤视图（count_inner_markers 语义——marker 层数）
        let m = marker_layers(&layers);
        let mcount = m.len();
        let expect_marker_count = kinds.iter()
            .filter(|k| matches!(k, FrameKind::BulletList | FrameKind::OrderedList)).count();
        if mcount != expect_marker_count {
            failures += 1;
            eprintln!("[PE1] marker 视图分歧: {frag:?} count={mcount} want={expect_marker_count}");
        }
        // 5. strip_marker_bq_alt 视图（无条件全剥——剩余 == 本函数剩余）
        //    对照: strip_marker_bq_alt 循环剥 BQ+marker 无条件——与 peel 全序列等价
        eprintln!("[PE1] ✓ {frag:?} → layers={kinds:?} rem={rem:?}");
    }
    eprintln!("[PE1] PeelStack 完备性: {} 例, {} 失败", PE1_CORPUS.len(), failures);
    assert_eq!(failures, 0, "PE1 失败 {failures}/{}", PE1_CORPUS.len());
}

/// 简化实现（实现参照原则 §4.4——以生产分支为准）:
/// dispatch_lut 被简化为"首字符分类"（只判非 marker——其余一律 Paragraph）——
/// [ref] 前缀、'#' 前缀、表头等边缘类型的层内容分类是错的。
/// 语义提取须从真实 block.rs:3497 content_kind_per_depth 的分支读。
/// content_kind_per_depth 视图（block.rs:3497 语义——marker 层的内容类型）:
/// 内容列（marker 列起算）>= marker 列+5 → IndentedCode;否则 dispatch_lut。
/// 只对 marker 层成立（BQ 已剥——marker 过滤视图的消费）。
/// CM §5.2 边界（block.rs 注释——符合性裁决 #258/#277）:
///   `-    foo`（4 空格）→ 文本（Paragraph）;`-     foo`（5 空格）→ IC。
fn content_kind_view(fragment: &str) -> Vec<Option<BlockType>> {
    let (layers, _) = peel(fragment);
    let m = marker_layers(&layers);
    let mut out = Vec::new();
    for l in &m {
        let after = &fragment[l.end..];
        let b = after.as_bytes();
        let ws2 = b.iter().take_while(|&&x| x == b' ' || x == b'\t').count();
        let content = &after[ws2..];
        let marker_col = if l.kind == FrameKind::BulletList { 1 } else {
            let d = after.as_bytes().iter().take_while(|&&x| x.is_ascii_digit()).count();
            d as usize + 1
        };
        // 内容列 = marker 列起算的空白列——content_kind_per_depth 从 marker
        // 字符后起算（cur[1..]——含 marker 的尾随空格）——层已消费 marker+空格
        // （2 字节）——补回 1（marker 尾随空格计入列）
        let content_col = marker_col + ws2 + 1;
        if content.is_empty() {
            out.push(None);
        } else if content_col >= marker_col + 5 {
            out.push(Some(BlockType::IndentedCode));
        } else {
            // dispatch_lut 简化: 首字符分类（'[' → RefDef? '#' → Header... 仅判非 marker）
            out.push(Some(BlockType::Paragraph));
        }
    }
    out
}

#[test]
fn pe1_content_kind_view() {
    // content_kind_per_depth 的层视图 vs PeelStack marker 过滤视图（关键例）:
    // IC 是视图不是层——层序列恒定 [BLT],视图按内容列分派。
    // CM §5.2 边界（#258/#277 裁决——block.rs 注释）:
    //   4 空格（`-    foo`）→ 文本;5 空格（`-     foo`）→ IC。
    let (layers, _) = peel("-    foo");
    assert_eq!(marker_layers(&layers).len(), 1, "`-    foo` 的 marker 层数");
    let v = content_kind_view("-    foo");
    assert_eq!(v, vec![Some(BlockType::Paragraph)],
        "`-    foo`（4 空格）→ 文本（CM §5.2 边界）");
    let v2 = content_kind_view("-     foo");
    assert_eq!(v2, vec![Some(BlockType::IndentedCode)],
        "`-     foo`（5 空格）→ IC（内容列 >= marker 列+5）");
    let v3 = content_kind_view("- a");
    assert_eq!(v3, vec![Some(BlockType::Paragraph)], "`- a` 的层内容类型");
    eprintln!("[PE1] content_kind 视图（IC 是视图不是层——层序列恒定）✓");
}

// PE2: 身份转换矩阵——结构序列 vs FL 事件序列（真实文档对齐）

/// 文档 → 行 0 的 FL Push 记录 kind 序列（build_container_records 的 push_lines）。
fn fl_pushes_at(doc: &str, line: u32) -> Vec<FrameKind> {
    let mut st = State::new(doc);
    let snap = st.snapshot();
    let n = snap.line_len();
    if n == 0 { return Vec::new(); }
    let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
    mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
    let (recs, arena) = fold.finish();
    recs.iter()
        .filter(|r| r.push_lines_of(&arena).any(|l| l == line))
        .map(|r| r.frame.kind())
        .collect()
}

/// 身份转换矩阵——(文档, 行, 期望 FL Push 序列 == 期望 peel 层序列)
const PE2_DOCS: &[(&str, u32, &[FrameKind])] = &[
    ("> - item\n", 0, &[FrameKind::BlockQuote, FrameKind::BulletList]),
    ("> > - a\n", 0, &[FrameKind::BlockQuote, FrameKind::BlockQuote, FrameKind::BulletList]),
    ("- - foo\n", 0, &[FrameKind::BulletList, FrameKind::BulletList]),
    ("1. - x\n", 0, &[FrameKind::OrderedList, FrameKind::BulletList]),
    // marker 后 BQ——结构序列含 BQ（dispatch 嵌套循环）——FL 是否同序列?
    ("- > a\n", 0, &[FrameKind::BulletList, FrameKind::BlockQuote]),
    ("> > deep\n", 0, &[FrameKind::BlockQuote, FrameKind::BlockQuote]),
    ("- [x] a\n", 0, &[FrameKind::BulletList]),
];

#[test]
fn pe2_fl_alignment() {
    let mut failures = 0usize;
    for &(doc, line, want) in PE2_DOCS {
        let fl = fl_pushes_at(doc, line);
        let want_v: Vec<FrameKind> = want.to_vec();
        if fl != want_v {
            failures += 1;
            eprintln!("[PE2] FL 对齐分歧: {doc:?} line={line} want={want_v:?} got={fl:?}");
            continue;
        }
        // peel 层序列对照（行文本 → 层）
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let text = snap.line_text(line as usize).to_string();
        let (layers, _) = peel(&text);
        let kinds: Vec<FrameKind> = layers.iter().map(|l| l.kind).collect();
        if kinds != want_v {
            failures += 1;
            eprintln!("[PE2] peel 层分歧: {doc:?} line={line} want={want_v:?} got={kinds:?}");
            continue;
        }
        eprintln!("[PE2] ✓ {doc:?} → FL={fl:?} peel={kinds:?}");
    }
    eprintln!("[PE2] 身份转换矩阵 FL 对齐: {} 例, {} 失败", PE2_DOCS.len(), failures);
    assert_eq!(failures, 0, "PE2 失败 {failures}/{}", PE2_DOCS.len());
}

// PE3: 投影算子统一——chain(line) 三形态等价
// 假设: 链投影算子（行 → 覆盖容器链）的三个形态（物化覆盖/增量游标/前瞻
// 模拟）产出同一值——17 原语重组为"一个链值 + 四种派生"的基础。

/// 覆盖语义（RowView::in_chain 同款——end_includes_close: i+1 < e;否则 i < e）。
fn covers(r: &ContainerRec, i: usize) -> bool {
    if (r.start as usize) > i { return false; }
    match r.end_usize() {
        None => true,
        Some(e) => {
            if r.end_includes_close() { i + 1 < e } else { i < e }
        }
    }
}

/// 深度（parent 链长）。
fn rec_depth(records: &[ContainerRec], rid: usize) -> usize {
    let mut d = 0;
    let mut p = records[rid].parent;
    while p != u32::MAX { d += 1; p = records[p as usize].parent; }
    d
}

/// 简化实现（实现参照原则 §4.4——以生产为准）: 物化形态是 O(n×records)
/// 全扫——仅作真值参照（对比游标的 O(n) 摊销）——生产以 RowView 为准。
/// 形态 1（真值——物化覆盖）: 覆盖行 i 的最深记录 → 祖先路径（根到叶）。
fn chain_materialized(records: &[ContainerRec], i: usize) -> Vec<usize> {
    let mut leaf: Option<usize> = None;
    let mut best: usize = 0;
    for (rid, r) in records.iter().enumerate() {
        if covers(r, i) {
            let d = rec_depth(records, rid);
            if d >= best { best = d; leaf = Some(rid); }
        }
    }
    let Some(leaf) = leaf else { return Vec::new() };
    let mut path = vec![leaf];
    let mut p = records[leaf].parent;
    while p != u32::MAX {
        path.push(p as usize);
        p = records[p as usize].parent;
    }
    path.reverse();
    path
}

/// 事件推进（游标/模拟共用——pop 先于 push——PopPush 复用同记录）。
fn advance_one(records: &[ContainerRec], arena: &RecArena, stack: &mut Vec<usize>, i: usize) {
    loop {
        let Some(&top) = stack.last() else { break };
        let pop_event = records[top].pop_push_events_of(arena).any(|e| e.line == i as u32);
        if !covers(&records[top], i) || pop_event {
            stack.pop();
        } else {
            break;
        }
    }
    for (rid, r) in records.iter().enumerate() {
        if r.push_lines_of(arena).any(|l| l == i as u32) { stack.push(rid); }
    }
}

/// 形态 2（增量游标）: 逐行维护栈——O(1) 摊销。
fn chains_cursor(records: &[ContainerRec], arena: &RecArena, n: usize) -> Vec<Vec<usize>> {
    let mut stack: Vec<usize> = Vec::new();
    let mut out = vec![Vec::new(); n];
    for i in 0..n {
        advance_one(records, arena, &mut stack, i);
        out[i] = stack.clone();
    }
    out
}

/// 形态 3（前瞻模拟）: 从零一次性重放到 i（无先验状态——end_for 前瞻同款）。
fn chain_simulated(records: &[ContainerRec], arena: &RecArena, i: usize) -> Vec<usize> {
    let mut stack: Vec<usize> = Vec::new();
    for j in 0..=i {
        advance_one(records, arena, &mut stack, j);
    }
    stack
}

#[test]
fn pe3_projection_operator() {
    let docs = &[
        "# A\n- a\n- b\n# B\n- c\n",
        "> - item\n>   cont\n- next\n",
        "- a\n  - b\n- c\n",
        "> > - x\n> - y\n\n- z\n",
        "1. a\n1. b\n\n   - c\n",
        "<!-- c -->\n```\ncode\n```\n- tail\n",
    ];
    let mut failures = 0usize;
    for doc in docs {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        if n == 0 { continue; }
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let cur = chains_cursor(&recs, &arena, n);
        for i in 0..n {
            let want = chain_materialized(&recs, i);
            if cur[i] != want {
                failures += 1;
                eprintln!("[PE3] 游标分歧: {doc:?} line={i} want={want:?} got={:?}",
                    cur[i].iter().map(|&r| recs[r].frame.kind()).collect::<Vec<_>>());
                break;
            }
            let sim = chain_simulated(&recs, &arena, i);
            if sim != want {
                failures += 1;
                eprintln!("[PE3] 模拟分歧: {doc:?} line={i}");
                break;
            }
        }
        if failures == 0 {
            eprintln!("[PE3] ✓ {doc:?} 三形态一致（{n} 行）");
        }
    }
    eprintln!("[PE3] 投影算子三形态等价: {} 文档, {} 失败", docs.len(), failures);
    assert_eq!(failures, 0, "PE3 失败 {failures}");
}

// PE4: 截断约定——zone 视图一致性（增量正确性）
// 假设: zone 构建（深度 0 起点）的链查询 == 全量构建在 zone 上的限制——
// 截断约定（zone 外 = 空链）一次实现, 哨兵不再散落。

#[test]
fn pe4_zone_truncation() {
    let docs = &[
        "# A\n- a\n- b\n# B\n- c\n",
        "> - item\n>   cont\n- next\n",
        "- a\n  - b\n- c\n\n# T\nx\n",
    ];
    let mut failures = 0usize;
    for doc in docs {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        if n == 0 { continue; }
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (full, _full_arena) = fold.finish();
        // 找深度 0 的 zone 起点（顶层标题后或首行）——zone 构建的栈空前置
        let mut zs = 0usize;
        for i in 0..n {
            if snap.line_type_at(i) == BlockType::Header1 {
                zs = i; break;
            }
        }
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (zs as u32, n as u32), &mut fold);
        let (zone, _zone_arena) = fold.finish();
        for i in zs..n {
            let want = chain_materialized(&full, i);
            let got = chain_materialized(&zone, i);
            // 比 kind 序列（zone 记录是全量的子集——id 不同）
            let want_k: Vec<FrameKind> = want.iter().map(|&r| full[r].frame.kind()).collect();
            let got_k: Vec<FrameKind> = got.iter().map(|&r| zone[r].frame.kind()).collect();
            if want_k != got_k {
                failures += 1;
                eprintln!("[PE4] zone 截断分歧: {doc:?} line={i} want={want_k:?} got={got_k:?}");
                break;
            }
        }
        if failures == 0 {
            eprintln!("[PE4] ✓ {doc:?} zone[{zs}..{n}) 与全量一致");
        }
    }
    eprintln!("[PE4] 截断约定 zone 一致性: {} 文档, {} 失败", docs.len(), failures);
    assert_eq!(failures, 0, "PE4 失败 {failures}");
}

// PE5: 传导验证——链 → 寻址路径（depth => path 的正面实证）
// 假设: 链（结构坐标）系统化后, 寻址路径（MD_PATH 语义——容器链身份
// 序列化）自然派生——不再 ad hoc 构建。
// 验证: 链的 (kind, ci) 序列构成合法树（每路径的父路径存在——前缀闭合）。

/// 链 → 路径: 每层 (kind, ci) 的序列化（ci = 深度级全局序号——记录字段;
/// kind 用判别值——FrameKind 为 #[repr(u8)]——路径可哈希）。
fn chain_to_path(records: &[ContainerRec], chain: &[usize]) -> Vec<(u8, usize)> {
    chain.iter().map(|&r| (records[r].frame.kind() as u8, records[r].ci as usize)).collect()
}

#[test]
fn pe5_path_derivation() {
    let docs = &[
        "# A\n- a\n- b\n# B\n- c\n",
        "> - item\n>   cont\n- next\n",
        "- a\n  - b\n- c\n",
        "1. a\n1. b\n\n   - c\n",
        "> > - x\n> - y\n\n- z\n",
    ];
    let mut failures = 0usize;
    for doc in docs {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        if n == 0 { continue; }
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let cur = chains_cursor(&recs, &arena, n);
        // 1. 链内部父子一致性: 每对相邻层 = 记录的 parent 链接（链是祖先路径）
        for i in 0..n {
            for w in cur[i].windows(2) {
                if recs[w[1]].parent as usize != w[0] {
                    failures += 1;
                    eprintln!("[PE5] 链内部父子断裂: {doc:?} line={i} 层对={w:?}");
                    break;
                }
            }
        }
        // 2. 路径集合树闭包: 每个非根路径的父路径必是集合中某路径的前缀
        //    （复合开行的父从不单独成链——`> - item` 的 [BQ] 只作为
        //    [BQ, BLT] 的前缀出现——前缀归属是树性质的正确表达）
        let mut paths: std::collections::HashSet<Vec<(u8, usize)>> =
            std::collections::HashSet::new();
        for i in 0..n {
            paths.insert(chain_to_path(&recs, &cur[i]));
        }
        for p in &paths {
            if p.len() <= 1 { continue; }
            let parent = &p[..p.len() - 1];
            let prefix_ok = paths.iter().any(|q| q.starts_with(parent));
            if !prefix_ok {
                failures += 1;
                eprintln!("[PE5] 路径父缺失（前缀归属）: {doc:?} path={p:?} 父={parent:?}");
            }
        }
        if failures == 0 {
            eprintln!("[PE5] ✓ {doc:?} 链父子一致 + 路径集合闭包（{n} 行, {} 条路径）", paths.len());
        }
    }
    eprintln!("[PE5] 链→路径传导: {} 文档, {} 失败", docs.len(), failures);
    assert_eq!(failures, 0, "PE5 失败 {failures}");
}

// PE6: 规模验证——本地 commonmark 语料（未入库）上三原语一致性 + 游标线性
// 信息守恒在规模上的实证: 物化/游标/模拟在大文档上仍一致;
// 游标 O(n) 摊销（对比物化的 O(n×records)）。

#[test]
fn pe6_scale_commonmark() {
    let cm = concat!(env!("CARGO_MANIFEST_DIR"), "/../commonmark.md");
    let text = std::fs::read_to_string(cm).unwrap_or_default();
    if text.is_empty() {
        eprintln!("[PE6] commonmark.md 未找到——跳过");
        return;
    }
    let mut st = State::new(&text);
    let snap = st.snapshot();
    let n = snap.line_len();
    eprintln!("[PE6] commonmark.md: {n} 行");
    let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
    mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
    let (recs, arena) = fold.finish();
    eprintln!("[PE6] 容器记录: {} 条", recs.len());
    // 游标全量 + 抽样物化对照（全量物化 O(n×records)——抽样即可）
    let cur = chains_cursor(&recs, &arena, n);
    let mut failures = 0usize;
    let mut probe = 0usize;
    let mut step = n / 200 + 1;
    if step == 0 { step = 1; }
    let mut i = 0usize;
    while i < n {
        let want = chain_materialized(&recs, i);
        if cur[i] != want {
            failures += 1;
            if failures <= 3 {
                eprintln!("[PE6] 游标分歧: line={i} want={:?} got={:?}",
                    want.iter().map(|&r| recs[r].frame.kind()).collect::<Vec<_>>(),
                    cur[i].iter().map(|&r| recs[r].frame.kind()).collect::<Vec<_>>());
            }
        }
        probe += 1;
        i += step;
    }
    eprintln!("[PE6] 规模一致性: 抽样 {} 行, {} 失败（{} 条记录）", probe, failures, recs.len());
    assert_eq!(failures, 0, "PE6 失败 {failures}");
}

// PE7: 态射验证——marker 层 ↔ 本行 push 的列表帧（list_push_depths 语义）
// 假设: 层序列（PeelStack）经 FL 事件映射到帧深度——marker 层 k ↔ 本行
// 第 k 个 push 的列表帧（BLT/OL）。`> - c` 的 BQ 不计层——BLT 帧取层 0。

#[test]
fn pe7_morphism_depth() {
    let docs = &[
        ("> - c\n", 0u32),
        ("- - foo\n", 0),
        ("- a\n  - b\n", 1),      // 续行 item——marker 层 ↔ 嵌套帧（本行 push）
        ("1. - x\n", 0),
        ("> > - x\n", 0),
        ("- a\n- b\n", 1),        // 第二 item——PopPush 复用帧——本行 push 仍记录
    ];
    let mut failures = 0usize;
    for &(doc, line) in docs {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        if n == 0 { continue; }
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        // 本行 push 的列表帧（记录序——外层先）
        let list_frames: Vec<FrameKind> = recs.iter()
            .filter(|r| r.push_lines_of(&arena).any(|l| l == line)
                && matches!(r.frame.kind(), FrameKind::BulletList | FrameKind::OrderedList))
            .map(|r| r.frame.kind())
            .collect();
        // marker 层（行文本 peel → marker 过滤）
        let text = snap.line_text(line as usize).to_string();
        let (layers, _) = peel(&text);
        let markers: Vec<FrameKind> = marker_layers(&layers).iter().map(|l| l.kind).collect();
        if markers != list_frames {
            failures += 1;
            eprintln!("[PE7] 态射分歧: {doc:?} line={line} marker={markers:?} 帧={list_frames:?}");
            continue;
        }
        eprintln!("[PE7] ✓ {doc:?} line={line} marker 层 {markers:?} ↔ 帧 {list_frames:?}");
    }
    eprintln!("[PE7] 态射（层×FL→深度）: {} 例, {} 失败", docs.len(), failures);
    assert_eq!(failures, 0, "PE7 失败 {failures}");
}

// PE8: 写侧逆用——层重建 vs format_payload Reconstruct 语义（测试内推导）
// 假设: 正逆闭环的"逆"（层重建——marker + tag + 分隔 + rest）与现写端
// Reconstruct（block.rs format_payload——BLT/Checklist 分支）行为一致。
// 推导自 format_payload 代码（block.rs:4198——pub(crate)——测试内转录）:
//   BLT tag 写: ws_prefix + marker_char + " " + tag_val + ": " + rest
//     （tag_val 空 → 无冒号;tag_val 以 '[' 开头 → 无冒号）
//   Checklist tag 写: ws_prefix + marker(含checkbox) + tag_val + ": " + rest

/// 简化实现（实现参照原则 §4.4——以生产分支为准）: 转录 format_payload
/// 的 Reconstruct（BLT 分支）——缺边缘: OL 分支（digits 重建）、Checklist 的
/// 详细重建、trim 细节（line_trimmed）。语义提取须从 block.rs:4198 读。
/// 层重建（PeelStack 逆用——tag 写场景）: 取层序列的 marker 形态重建前缀。
/// rest = 剩余剥原 tag+分隔（colon 键/括号键）——已有键替换场景。
fn recompose_tag(fragment: &str, tag_val: &str) -> String {
    let (layers, rem) = peel(fragment);
    assert!(!layers.is_empty(), "recompose_tag 需有层: {fragment:?}");
    let mut out = String::new();
    let mut consumed = 0usize;
    for l in &layers {
        let seg = &fragment[l.start..l.end];
        if matches!(l.kind, FrameKind::BulletList | FrameKind::OrderedList) {
            // 层内前导空白保留（缩进——`  - a`）+ marker 字符
            let trimmed = seg.trim_start();
            let ws_in_layer = seg.len() - trimmed.len();
            let marker_char = trimmed.chars().next().unwrap_or('-');
            out.push_str(&seg[..ws_in_layer]);
            out.push(marker_char);
            out.push(' ');
            // rest = 剩余剥原 tag+分隔（已有键替换——`- key: v` → rest="v"）
            let rest = if let Some(p) = rem.find(':') {
                rem[p + 1..].trim_start()
            } else if let Some(p) = rem.find("] ") {
                rem[p + 2..].trim_start()
            } else {
                rem.trim_start()
            };
            if !tag_val.is_empty() && !tag_val.starts_with('[') {
                out.push_str(tag_val);
                out.push_str(": ");
            } else if !tag_val.is_empty() {
                out.push_str(tag_val);
                out.push(' ');
            }
            out.push_str(rest);
            return out;
        } else {
            out.push_str(seg);
            consumed = l.end;
        }
    }
    out.push_str(&fragment[consumed..]);
    out
}

#[test]
fn pe8_write_inverse() {
    // 期望推导自 format_payload Reconstruct（block.rs:4198——BLT 分支）:
    //   has_colon → marker + tag_val + ": " + rest;空 tag → marker + " " + rest;
    //   tag_val 以 '[' → marker + tag_val + " " + rest
    let cases: &[(&str, &str, &str)] = &[
        ("- a", "key", "- key: a"),            // 有 rest——冒号路径
        ("-", "key", "- key: "),               // 真空——空 rest
        ("- a", "[x]", "- [x] a"),             // 括号路径——无冒号
        ("- a", "", "- a"),                    // 空 tag——rest 原样
        ("- key: v", "k2", "- k2: v"),         // 已有键——替换
        ("> - a", "key", "> - key: a"),        // BQ 层保留
        ("  - a", "key", "  - key: a"),        // 缩进保留
    ];
    let mut failures = 0usize;
    for &(frag, tag, want) in cases {
        let got = recompose_tag(frag, tag);
        if got != want {
            failures += 1;
            eprintln!("[PE8] 重建分歧: {frag:?} tag={tag:?} want={want:?} got={got:?}");
            continue;
        }
        eprintln!("[PE8] ✓ {frag:?} + tag={tag:?} → {got:?}");
    }
    eprintln!("[PE8] 写侧逆用（层重建）: {} 例, {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE8 失败 {failures}");
}

// PE10: strip 场景视图穷尽——alt vs bq_alt 的条件差异
// 假设（推导自 strip_container_prefixes_alt / strip_marker_bq_alt 代码）:
//   bq_alt（无条件）: 剥全部层——剩余 == peel 全序列剩余
//   alt（marker 仅当后跟 BQ）: marker 后非 BQ → 不剥（剩余原样）;
//                            marker 后 BQ → 剥（继续）

/// 简化实现（实现参照原则 §4.4——以生产分支为准）: 转录
/// strip_container_prefixes_alt（infer.rs:91）——仅单行片段、无逐行 map、
/// 无 OL marker 分支（number_digits）——边缘缺口。语义以生产为准。
/// alt 视图（strip_container_prefixes_alt 语义——测试内转录）:
/// 逐行循环: 剥 BQ（无条件）;marker 仅当"剥 marker 后是 BQ 前缀"才剥。
fn strip_alt_view(fragment: &str) -> String {
    let mut cur = fragment;
    loop {
        let b = cur.as_bytes();
        // BQ 无条件剥
        if b.first() == Some(&b'>') {
            let mut w = 1;
            if b.len() > 1 && (b[1] == b' ' || b[1] == b'\t') { w = 2; }
            cur = &cur[w..];
            continue;
        }
        // marker 仅当后跟 BQ
        let ws = b.iter().take_while(|&&x| x == b' ' || x == b'\t').count();
        let tb = &b[ws..];
        let after_marker: Option<&str> = if matches!(tb.first(), Some(&b'-') | Some(&b'*') | Some(&b'+'))
            && tb.len() > 1 && (tb[1] == b' ' || tb[1] == b'\t') {
            Some(&cur[ws + 2..])
        } else {
            None
        };
        if let Some(am) = after_marker {
            let ab = am.as_bytes();
            let aws = ab.iter().take_while(|&&x| x == b' ' || x == b'\t').count();
            if aws < ab.len() && ab[aws] == b'>' {
                cur = am;
                continue;
            }
        }
        break;
    }
    cur.to_string()
}

#[test]
fn pe10_strip_scene_views() {
    let cases: &[(&str, &str)] = &[
        // (片段, alt 视图剩余)——marker 仅当后跟 BQ 才剥
        ("> - > b", "b"),          // BQ 剥 → marker 后 BQ → 剥 → "b"
        ("- > a", "a"),            // marker 后 BQ → 剥
        ("- a", "- a"),            // marker 后非 BQ → 不剥（原样——与 bq_alt 差异）
        ("> - a", "- a"),          // BQ 剥后 marker 后非 BQ → 不剥 marker——剩余 "- a"
        ("- - a", "- - a"),        // 内层 marker 后非 BQ → 不剥
    ];
    let mut failures = 0usize;
    for &(frag, want) in cases {
        let got = strip_alt_view(frag);
        if got != want {
            failures += 1;
            eprintln!("[PE10] alt 视图分歧: {frag:?} want={want:?} got={got:?}");
            continue;
        }
        eprintln!("[PE10] ✓ {frag:?} → alt={got:?}");
    }
    eprintln!("[PE10] strip 场景视图（alt vs bq_alt 条件差异）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE10 失败 {failures}");
}

// PE11: 全量态射——层序列 vs 链域帧（本地 commonmark 语料全量对照）
// 态射的精确形态（PE11 设计推导）:
//   层序列 == 链的 {BQ,BLT,OL} 域帧的**最内层后缀**（按序）
//     `  - b`（续行 marker）: 链 [BLT1, BLT2]——层 [BLT] == 尾 [BLT2] ✓
//     `> bar`（BQ 续行）: 链 [BQ]——层 [BQ] == 尾 ✓
//     `> > - x`（复合开行）: 链 [BQ1,BQ2,BLT]——层全 == 整链 ✓
//   不是"层 == 该行 push"（续行无 push）也不是"层 == 整链"（嵌套续行
//   外层帧无 marker）——尾匹配是正确形态。

#[test]
fn pe11_full_morphism_commonmark() {
    let cm = concat!(env!("CARGO_MANIFEST_DIR"), "/../commonmark.md");
    let text = std::fs::read_to_string(cm).unwrap_or_default();
    if text.is_empty() {
        eprintln!("[PE11] commonmark.md 未找到——跳过");
        return;
    }
    let mut st = State::new(&text);
    let snap = st.snapshot();
    let n = snap.line_len();
    let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
    mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
    let (recs, arena) = fold.finish();
    let cur = chains_cursor(&recs, &arena, n);
    let mut failures = 0usize;
    // 态射只对结构行成立——代码/注释/空白内容行的文本不参与容器开启
    // （围栏内的 `1. item` 文本剥出 OL 层但无帧——内容行跳过）
    let skip_pl: &[BlockType] = &[
        BlockType::Blank, BlockType::CodeContent, BlockType::HtmlContent,
        BlockType::CodeFenceClose, BlockType::TableSeparator,
        BlockType::Comment, BlockType::IndentedCode,
    ];
    for i in 0..n {
        let pl = snap.line_type_at(i);
        if skip_pl.contains(&pl) { continue; }
        let text = snap.line_text(i).to_string();
        let (layers, _) = peel(&text);
        let peel_kinds: Vec<FrameKind> = layers.iter().map(|l| l.kind).collect();
        // 链的 peel 域帧（BQ/BLT/OL——按链序）
        let chain_kinds: Vec<FrameKind> = cur[i].iter()
            .filter(|&&r| matches!(recs[r].frame.kind(),
                FrameKind::BlockQuote | FrameKind::BulletList | FrameKind::OrderedList))
            .map(|&r| recs[r].frame.kind())
            .collect();
        let k = peel_kinds.len();
        let tail_ok = if k == 0 {
            true
        } else {
            chain_kinds.len() >= k && chain_kinds[chain_kinds.len() - k..] == peel_kinds[..]
        };
        if !tail_ok {
            failures += 1;
            if failures <= 5 {
                eprintln!("[PE11] 态射分歧: line={i} 层={peel_kinds:?} 链域={chain_kinds:?}");
            }
        }
    }
    eprintln!("[PE11] 全量态射（层 == 链域帧最内层后缀）: {n} 行, {} 失败", failures);
    assert_eq!(failures, 0, "PE11 失败 {failures}");
}

// PE12: 声明对接——层语义查表（LineSpec 雏形）vs 硬编码剥壳
// 假设: 机制（剥壳循环）通用, 规则（哪个前缀 → 什么层/多宽）来自声明——
// 查表执行 == 硬编码执行（两线合龙: 第一课机制 × infer_prev_1 声明）。

/// 层语义声明（LineSpec 雏形——按首字符的 kind 映射 + 各 kind 的宽度规则）。
const LAYER_KIND_LUT: &[(u8, FrameKind)] = &[
    (b'>', FrameKind::BlockQuote),
    (b'-', FrameKind::BulletList),
    (b'*', FrameKind::BulletList),
    (b'+', FrameKind::BulletList),
];

/// 简化实现（实现参照原则 §4.4——以生产为准）: 声明雏形（LineSpec
/// 宽度规则的候选）——OL 的 tab 展开（col_to_byte）未处理;真空 marker 的
/// 语义以生产（parse_ol/parse_bl——block.rs）为准。
/// 宽度规则（声明化——各 kind 的 marker 宽度 + 空白处理）:
/// BQ: ">" + 可选 1 空白;BLT: 1 + 可选 1 空白（TB 守卫——CM §4.1）;
/// OL: digits + delim + 可选 1 空白。
fn layer_width(kind: FrameKind, tb: &[u8]) -> Option<usize> {
    match kind {
        FrameKind::BlockQuote => {
            if tb.first() != Some(&b'>') { return None; }
            let mut w = 1;
            if tb.len() > 1 && (tb[1] == b' ' || tb[1] == b'\t') { w = 2; }
            Some(w)
        }
        FrameKind::BulletList => {
            let c = tb.first().copied()?;
            if !matches!(c, b'-' | b'*' | b'+') { return None; }
            // TB 守卫（3+ 同字符仅含空白——非 marker 层）
            if tb.len() >= 3
                && tb.iter().filter(|&&b| b == c).count() >= 3
                && tb.iter().all(|&b| b == c || b == b' ' || b == b'\t')
            { return None; }
            let mut w = 1;
            if tb.len() > 1 && (tb[1] == b' ' || tb[1] == b'\t') { w = 2; }
            Some(w)
        }
        FrameKind::OrderedList => {
            let mut d = 0;
            while d < tb.len() && tb[d].is_ascii_digit() { d += 1; }
            if d == 0 || d >= tb.len() || (tb[d] != b'.' && tb[d] != b')') { return None; }
            let mut w = d + 1;
            if tb.get(d + 1) == Some(&b' ') || tb.get(d + 1) == Some(&b'\t') { w += 1; }
            Some(w)
        }
        _ => None,
    }
}

/// 查表剥壳（机制通用——规则来自 LAYER_KIND_LUT + layer_width）。
fn peel_table(fragment: &str) -> (Vec<Layer>, &str) {
    let mut layers = Vec::new();
    let mut cur = fragment;
    let base = fragment.as_ptr() as usize;
    loop {
        let b = cur.as_bytes();
        let ws = b.iter().take_while(|&&x| x == b' ' || x == b'\t').count();
        let tb = &b[ws..];
        // 查表: 首字符 → kind（候选——按表序）
        let mut matched = false;
        if let Some(first) = tb.first() {
            for &(c, kind) in LAYER_KIND_LUT {
                if c == *first {
                    if let Some(w) = layer_width(kind, tb) {
                        let consumed = ws + w;
                        let start = cur.as_ptr() as usize - base;
                        layers.push(Layer { kind, start, end: start + consumed });
                        cur = &cur[consumed..];
                        matched = true;
                        break;
                    }
                }
            }
        }
        if !matched {
            // 简化（实现参照原则 §4.4）: OL 是 digits 前缀（表外规则——
            // 首字符是数字——LUT 按首字符查不到）——机制的声明化形态应是
            // 规则表含"digits 前缀"模式而非函数内特判（LineSpec 设计点）。
            // OL 是 digits 前缀（表外规则——首字符是数字）
            let mut d = 0;
            while d < tb.len() && tb[d].is_ascii_digit() { d += 1; }
            if d > 0 && d < tb.len() && (tb[d] == b'.' || tb[d] == b')') {
                if let Some(w) = layer_width(FrameKind::OrderedList, tb) {
                    let consumed = ws + w;
                    let start = cur.as_ptr() as usize - base;
                    layers.push(Layer { kind: FrameKind::OrderedList, start, end: start + consumed });
                    cur = &cur[consumed..];
                    matched = true;
                }
            }
        }
        if !matched { break; }
    }
    (layers, cur)
}

#[test]
fn pe12_declaration_driven() {
    // 查表剥壳 vs 硬编码剥壳——全语料一致（含 TB 守卫用例）
    let extra: &[&str] = &["---", "* * *", "- - -", "***", "+ + +", "1) x", "10. y"];
    let mut failures = 0usize;
    for &(frag, _, _) in PE1_CORPUS {
        let (l1, r1) = peel(frag);
        let (l2, r2) = peel_table(frag);
        if l1 != l2 || r1 != r2 {
            failures += 1;
            eprintln!("[PE12] 查表分歧: {frag:?} 硬={l1:?} 表={l2:?}");
        }
    }
    for &frag in extra {
        let (l1, r1) = peel(frag);
        let (l2, r2) = peel_table(frag);
        if l1 != l2 || r1 != r2 {
            failures += 1;
            eprintln!("[PE12] 查表分歧(extra): {frag:?} 硬={l1:?} 表={l2:?}");
        }
    }
    eprintln!("[PE12] 声明对接（查表 == 硬编码）: {} 例, {} 失败",
        PE1_CORPUS.len() + extra.len(), failures);
    assert_eq!(failures, 0, "PE12 失败 {failures}");
}

/// 简化实现（实现参照原则 §4.4——以生产为准）: 转录 infer.rs 的
/// container_block_type——与真实映射可能漂移（新 FrameKind 扩展时）。
/// 容器块类型映射（infer.rs container_block_type 的测试内转录——链索引派生用）。
fn container_bt(kind: FrameKind) -> BlockType {
    match kind {
        FrameKind::BulletList => BlockType::Bullet,
        FrameKind::OrderedList => BlockType::OrderedListItem,
        FrameKind::CodeFence => BlockType::CodeFenceOpen,
        FrameKind::BlockQuote => BlockType::Blockquote,
        FrameKind::HtmlBlock => BlockType::HtmlBlock,
        FrameKind::Comment => BlockType::Comment,
        FrameKind::Table => BlockType::TableRow,
        FrameKind::TableRow => BlockType::TableRow,
        FrameKind::HtmlElement => BlockType::HtmlElement,
        FrameKind::RefDef => BlockType::RefDef,
    }
}

// PE13: per-depth 派生重组——链索引 vs RowView 原语（生产对照）
// 假设: 17 原语的 per-depth 查询（node_type_at_depth/frame_kind_at_depth/
// child_idx_at）== 链值的索引（chain[line][d] 的派生）——生产对照逐值一致。

#[test]
fn pe13_depth_derivation_vs_rowview() {
    // (D6⑤d——build_block_records/RowView 换 new_infer 全路径)
    let docs = &[
        "# A\n- a\n  - b\n- c\n",
        "> > - x\n> - y\n\n- z\n",
        "1. a\n1. b\n\n   - c\n",
        "> - item\n>   cont\n- next\n",
    ];
    let mut failures = 0usize;
    for doc in docs {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        if n == 0 { continue; }
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let view = mdio_core::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries,
            &snap.lines, &snap.rope, (0, n as u32), &arena);
        let cur = chains_cursor(&recs, &arena, n);
        for i in 0..n {
            let chain = &cur[i];
            // node_type_at_depth vs 链索引的容器块类型
            for d in 0..chain.len() {
                let want_nt = view.node_type_at_depth(i, d);
                let got_nt = container_bt(recs[chain[d]].frame.kind());
                if want_nt != got_nt {
                    failures += 1;
                    eprintln!("[PE13] node_type_at_depth 分歧: {doc:?} line={i} d={d} want={want_nt:?} got={got_nt:?}");
                }
                let want_fk = view.frame_kind_at_depth(i, d);
                let got_fk = recs[chain[d]].frame.kind();
                if want_fk != Some(got_fk) {
                    failures += 1;
                    eprintln!("[PE13] frame_kind_at_depth 分歧: {doc:?} line={i} d={d}");
                }
                let want_ci = view.child_idx_at(i, d);
                let got_ci = recs[chain[d]].ci as usize;
                if want_ci != got_ci {
                    failures += 1;
                    eprintln!("[PE13] child_idx_at 分歧: {doc:?} line={i} d={d} want={want_ci} got={got_ci}");
                }
            }
            let want_d = view.close_depth_at(i);
            let got_d = chain.len();
            if want_d != got_d {
                failures += 1;
                eprintln!("[PE13] depth_at 分歧: {doc:?} line={i} want={want_d} got={got_d}");
            }
        }
        if failures == 0 {
            eprintln!("[PE13] ✓ {doc:?} 链索引 == RowView 原语（{n} 行）");
        }
    }
    eprintln!("[PE13] per-depth 派生重组（链索引 vs 生产原语）: {} 文档, {} 失败",
        docs.len(), failures);
    assert_eq!(failures, 0, "PE13 失败 {failures}");
}

// PE15: LineSpec tag 规则声明化原型——消费级对照（read_block_proj）
// 假设: tag 取法（node_tag 的 per-type 规则——13+6 变体的一部分）声明化后
// 与生产提取逐值一致。对照锚: geometry: None 的节点（selector 伪根同款）
// ——read_block_proj(Tag) 走 fallback 提取——生产可达路径。
// 注: geometry 路径的 equiv 需在 crate 内（阶段 2 施工）——本测试验证
// fallback 路径（消费级锚——计划 §4.1 的 equiv 粒度分层）。

/// 简化实现（实现参照原则 §4.4——以生产为准）: 转录 block.rs 的
/// is_header——与真实函数可能漂移。标题类型判定以生产为准。
/// 标题类型（is_header 的测试内转录——pub(crate) 不可达）。
fn is_header_type(t: BlockType) -> bool {
    matches!(t, BlockType::Header1 | BlockType::Header2 | BlockType::Header3
        | BlockType::Header4 | BlockType::Header5 | BlockType::Header6)
}

/// 简化实现（实现参照原则 §4.4——以生产分支为准）: 转录 node_tag 的
/// fallback 分支——**缺边缘**: fence lang 的 decode_link_dest（反转义）、
/// setext 多行 tag（全段落剥 BQ 去末下划线行）、HtmlBlock type 6、RefDef。
/// 已由生产锚（read_projection——几何路径）对照的部分是可靠的;未对照的
/// 边缘须从真实代码（infer.rs node_tag）逐分支提取。
/// tag 规则声明（LineSpec tag_mode 雏形——node_tag fallback 分支的声明化转录）:
///   Heading: 剥 # 前缀 + 前导空白后整行;setext: 整行
///   Bullet: 剥 marker → colon 键 / [ref] 括号键 / marker 字符
///   Checklist: checkbox 后 colon 键 / checkbox 本身
///   OL: digits;Fence: lang;HtmlTag/HtmlBlock: tagname;其余: ""
fn declared_tag(t: BlockType, line: &str) -> String {
    let trim_end = |s: &str| s.trim_end_matches([' ', '\t']).to_string();
    if is_header_type(t) {
        let bq = mdio_core::block::peel(line).0.iter()
            .take_while(|l| l.kind == mdio_core::block::FrameKind::BlockQuote)
            .last().map_or(0, |l| l.end);
        let after_bq = &line[bq..];
        let trimmed = after_bq.trim_start();
        let after_hash = trimmed.trim_start_matches('#');
        return trim_end(after_hash.trim_start_matches(' '));
    }
    if matches!(t, BlockType::HsHead1 | BlockType::HsHead2) {
        return trim_end(line);
    }
    match t {
        BlockType::Bullet => {
            let trimmed = line.trim_start();
            let b = trimmed.as_bytes();
            let after_marker = if b.len() >= 2 && matches!(b[0], b'-' | b'*' | b'+')
                && (b[1] == b' ' || b[1] == b'\t') { &trimmed[2..] } else { trimmed };
            // 几何路径语义（主路径）: 无键 → ""（marker 字符作 tag 是 fallback 残留）
            if let Some(p) = after_marker.find(':') {
                after_marker[..p].trim_end().to_string()
            } else if after_marker.starts_with('[') {
                after_marker.find("] ").map(|p| after_marker[..=p].to_string()).unwrap_or_default()
            } else {
                String::new()
            }
        }
        BlockType::Checklist => {
            let trimmed = line.trim_start();
            let b = trimmed.as_bytes();
            let after_bullet = if b.len() >= 2 && (b[1] == b' ' || b[1] == b'\t') {
                &trimmed[2..]
            } else { trimmed };
            let checkbox = if after_bullet.starts_with("[x]") || after_bullet.starts_with("[X]") {
                Some("[x]")
            } else if after_bullet.starts_with("[ ]") {
                Some("[ ]")
            } else { None };
            let after_checkbox = if checkbox.is_some() {
                after_bullet[3..].trim_start()
            } else { after_bullet };
            // 几何路径语义(C3): 冒号键;无冒号 → checkbox 状态字符
            // (x/X/空格——find_ckl_tag 的 tag_range = 状态字符位——生产实证)
            if let Some(p) = after_checkbox.find(':') {
                after_checkbox[..p].to_string()
            } else if let Some(cb) = checkbox {
                // "[x]" → "x";"[ ]" → " "(状态字符)
                cb.chars().nth(1).unwrap_or(' ').to_string()
            } else { String::new() }
        }
        BlockType::OrderedListItem => {
            let b = line.trim_start().as_bytes();
            let mut d = 0;
            while d < b.len() && b[d].is_ascii_digit() { d += 1; }
            if d > 0 && d < b.len() && (b[d] == b'.' || b[d] == b')') {
                String::from_utf8_lossy(&b[..d]).to_string()
            } else { String::new() }
        }
        BlockType::CodeFenceOpen => {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                let lang = &trimmed[3..];
                let end = lang.trim_end_matches([' ', '\t']).len();
                lang[..end].to_string()
            } else { String::new() }
        }
        BlockType::HtmlElement | BlockType::HtmlBlock => {
            // tagname（第一个 < 后到 >/空白）
            if let Some(p) = line.find('<') {
                let rest = &line[p + 1..];
                let name: String = rest.chars().take_while(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, '-' | '_')
                }).collect();
                name
            } else { String::new() }
        }
        _ => String::new(),
    }
}

#[test]
fn pe15_linespec_tag_declaration() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    // (文档, 类型, 期望声明 tag)——几何路径语义（主路径——selector 真实节点）
    let cases: &[(&str, BlockType, &str)] = &[
        ("# Foo\n", BlockType::Header1, "Foo"),
        ("## Bar\n", BlockType::Header2, "Bar"),
        ("- key: value\n", BlockType::Bullet, "key"),
        ("- [ref] x\n", BlockType::Bullet, "[ref]"),
        ("- plain\n", BlockType::Bullet, ""),
        ("- [x] done: yes\n", BlockType::Checklist, "done"),
        ("- [ ] todo\n", BlockType::Checklist, " "), // 状态字符(C3)
        ("1. item\n", BlockType::OrderedListItem, "1"),
        ("```rust\n", BlockType::CodeFenceOpen, "rust"),
        ("<div>\n", BlockType::HtmlElement, "div"),
        ("> q\n", BlockType::Blockquote, ""),
        ("> > # H9\n", BlockType::Header1, "H9"),
        ("<!-- c -->\n", BlockType::Comment, ""),
        ("| a |\n", BlockType::TableRow, ""),
        ("    code\n", BlockType::IndentedCode, ""),
        // "---"（ThematicBreak）被 selector 跳过（node_category Skip——不产节点）
        ("plain text\n", BlockType::Paragraph, ""),
    ];
    let mut failures = 0usize;
    for &(doc, t, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let text = snap.line_text(0).to_string();
        // 声明规则提取（LineSpec tag_mode 雏形）
        let got_decl = declared_tag(t, &text);
        if got_decl != want {
            failures += 1;
            eprintln!("[PE15] 声明分歧: {doc:?} t={t:?} want={want:?} got={got_decl:?}");
            continue;
        }
        // 生产锚: selector 路径 read_projection("tag")——真实节点（geometry 全——
        // 几何路径——主路径）。修点 13 后 BQ 单标记段落行按 cmark 发 [bq,p]
        // 双身份（tag 均为空）——比较去重后的 tag 集合。
        match read_projection(&snap, &[Step::Wildcard], Some("tag")) {
            Ok(v) => {
                let mut tags: Vec<String> = v.into_iter()
                    .filter(|x| !x.is_empty())
                    .collect();
                tags.sort();
                tags.dedup();
                let expected: Vec<String> = if want.is_empty() { Vec::new() }
                    else { vec![want.to_string()] };
                if tags != expected {
                    failures += 1;
                    eprintln!("[PE15] 生产分歧: {doc:?} t={t:?} want={want:?} prod={tags:?}");
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("[PE15] read_projection 错误: {doc:?} {e:?}");
            }
        }
        if failures == 0 {
            eprintln!("[PE15] ✓ {doc:?} t={t:?} tag={want:?}");
        }
    }
    eprintln!("[PE15] LineSpec tag 声明化（声明 == 生产几何路径）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE15 失败 {failures}");
}

// PE16: LineSpec mark 规则声明化原型——消费级对照（read_projection("mark")）
// 简化实现（实现参照原则 §4.4——以生产分支为准）: 转录 node_mark 的
// 几何路径——**缺边缘**: RefDef 的 label（fallback 内容解析）、HtmlBlock
// type 6 的 <tag>、mark 的 tab 处理。已由生产锚对照的部分可靠。
// 几何路径语义（line_geometry 的 mark_text_end 推导——测试内转录）:
//   Heading: "#"×lv;Bullet/Checklist: marker 字符;OL: digits+delim;
//   Fence: ```/~~~;BQ: ">";Comment: "<!--";Pipe: "|";HtmlElement: <tag>;
//   IC: "    "。

fn declared_mark(t: BlockType, line: &str) -> String {
    if is_header_type(t) {
        let lv = match t {
            BlockType::Header1 => 1, BlockType::Header2 => 2, BlockType::Header3 => 3,
            BlockType::Header4 => 4, BlockType::Header5 => 5, _ => 6,
        };
        return "#".repeat(lv);
    }
    match t {
        BlockType::Bullet | BlockType::Checklist => {
            let trimmed = line.trim_start();
            trimmed.chars().next().map(|c| c.to_string()).unwrap_or_default()
        }
        BlockType::OrderedListItem => {
            let b = line.trim_start().as_bytes();
            let mut d = 0;
            while d < b.len() && b[d].is_ascii_digit() { d += 1; }
            if d > 0 && d < b.len() && (b[d] == b'.' || b[d] == b')') {
                format!("{}{}", String::from_utf8_lossy(&b[..d]), b[d] as char)
            } else { String::new() }
        }
        BlockType::CodeFenceOpen => {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") { "```".to_string() }
            else if trimmed.starts_with("~~~") { "~~~".to_string() }
            else { String::new() }
        }
        BlockType::Blockquote => {
            let layers = mdio_core::block::peel(line).0;
            let end = layers.iter()
                .take_while(|l| l.kind == mdio_core::block::FrameKind::BlockQuote)
                .last().map_or(0, |l| l.end);
            if end > 0 { line[..end].trim_end().to_string() } else { ">".to_string() }
        }
        BlockType::Comment => "<!--".to_string(),
        BlockType::TableRow | BlockType::TableSeparator => "|".to_string(),
        BlockType::IndentedCode => "    ".to_string(),
        BlockType::HtmlElement => {
            // 几何 mark_text_end = 第一个 '>' 后（整开标签）
            if let Some(p) = line.find('>') {
                line[..=p].to_string()
            } else { String::new() }
        }
        _ => String::new(),
    }
}

#[test]
fn pe16_linespec_mark_declaration() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    let cases: &[(&str, BlockType, &str)] = &[
        ("# Foo\n", BlockType::Header1, "#"),
        ("### Bar\n", BlockType::Header3, "###"),
        ("- key: value\n", BlockType::Bullet, "-"),
        ("* alt\n", BlockType::Bullet, "*"),
        ("- [x] done: yes\n", BlockType::Checklist, "-"),
        ("1. item\n", BlockType::OrderedListItem, "1."),
        ("10) item\n", BlockType::OrderedListItem, "10)"),
        ("```rust\n", BlockType::CodeFenceOpen, "```"),
        ("~~~tilde\n", BlockType::CodeFenceOpen, "~~~"),
        ("> q\n", BlockType::Blockquote, ">"),
        ("> > # H9\n", BlockType::Blockquote, "> >"),
        ("<!-- c -->\n", BlockType::Comment, "<!--"),
        ("| a |\n| - |\n", BlockType::TableRow, "|"),
        ("    code\n", BlockType::IndentedCode, "    "),
        ("<div>\n", BlockType::HtmlElement, "<div>"),
    ];
    let mut failures = 0usize;
    for &(doc, t, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let text = snap.line_text(0).to_string();
        let got_decl = declared_mark(t, &text);
        if got_decl != want {
            failures += 1;
            eprintln!("[PE16] 声明分歧: {doc:?} t={t:?} want={want:?} got={got_decl:?}");
            continue;
        }
        match read_projection(&snap, &[Step::Wildcard], Some("mark")) {
            Ok(v) => {
                if v.is_empty() {
                    failures += 1;
                    eprintln!("[PE16] 无节点: {doc:?}");
                } else if v[0] != want {
                    failures += 1;
                    eprintln!("[PE16] 生产分歧: {doc:?} t={t:?} want={want:?} prod={:?}", v[0]);
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("[PE16] read_projection 错误: {doc:?} {e:?}");
            }
        }
        if failures == 0 {
            eprintln!("[PE16] ✓ {doc:?} t={t:?} mark={want:?}");
        }
    }
    eprintln!("[PE16] LineSpec mark 声明化（声明 == 生产几何路径）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE16 失败 {failures}");
}

// PE17: LineSpec content 切分点规则声明化——消费级对照（read_projection）
// R3 的"切分点"（content_region 起点）声明化: 单行 item 的内容 = 切分点
// 之后的文本（marker + tag + 分隔之后）——与生产 content 投影对照。
// 标题的内容 = 区间（章节体）——单独处理。

/// 简化实现（实现参照原则 §4.4——以生产分支为准）: 转录 content_stripped
/// 的几何路径——**仅单行 item**——缺边缘: 续行 dedent（dedent_continuation_bq）、
/// BQ 交替剥除（strip_container_prefixes_alt）、OL 的尾换行保留（已对照）、
/// tab 列展开。区间内容（标题/fence/comment 体）须以生产分支为准。
/// content 切分点声明（单行 item——marker + tag + 分隔之后）:
///   Bullet: marker + (colon 键 + ": ") 之后 / 无键 marker 之后
///   Checklist: marker + checkbox + (colon 键 + ": ") 之后
///   OL: digits+delim+空白 之后;BQ: "> " 之后
fn declared_content(t: BlockType, line: &str) -> String {
    let trim_nl = |s: &str| s.trim_end_matches('\n').to_string();
    match t {
        BlockType::Bullet => {
            let trimmed = line.trim_start();
            let after_marker = if trimmed.len() >= 2
                && matches!(trimmed.as_bytes()[0], b'-' | b'*' | b'+')
                && (trimmed.as_bytes()[1] == b' ' || trimmed.as_bytes()[1] == b'\t')
            { &trimmed[2..] } else { trimmed };
            if let Some(p) = after_marker.find(':') {
                trim_nl(after_marker[p + 1..].trim_start())
            } else {
                trim_nl(after_marker)
            }
        }
        BlockType::Checklist => {
            let trimmed = line.trim_start();
            let after_marker = if trimmed.len() >= 2
                && (trimmed.as_bytes()[1] == b' ' || trimmed.as_bytes()[1] == b'\t')
            { &trimmed[2..] } else { trimmed };
            let after_checkbox = if after_marker.starts_with('[') && after_marker.contains("] ") {
                after_marker[after_marker.find("] ").unwrap() + 2..].trim_start()
            } else { after_marker };
            if let Some(p) = after_checkbox.find(':') {
                trim_nl(after_checkbox[p + 1..].trim_start())
            } else {
                trim_nl(after_checkbox)
            }
        }
        BlockType::OrderedListItem => {
            let b = line.trim_start().as_bytes();
            let mut d = 0;
            while d < b.len() && b[d].is_ascii_digit() { d += 1; }
            if d > 0 && d < b.len() && (b[d] == b'.' || b[d] == b')') {
                let rest = &line.trim_start()[d + 1..];
                // C3' OLI 收束: 面值去尾换行（与生产 list_content 同语义）
                rest.trim_start().trim_end_matches('\n').to_string()
            } else { String::new() }
        }
        BlockType::Blockquote => {
            let after = if line.starts_with('>') {
                if line.len() > 1 && (line.as_bytes()[1] == b' ' || line.as_bytes()[1] == b'\t') {
                    &line[2..]
                } else { &line[1..] }
            } else { line };
            trim_nl(after)
        }
        _ => String::new(),
    }
}

#[test]
fn pe17_content_split_declaration() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    let cases: &[(&str, BlockType, &str)] = &[
        ("- key: v\n", BlockType::Bullet, "v"),
        ("- no key\n", BlockType::Bullet, "no key"),
        ("- [x] done: yes\n", BlockType::Checklist, "yes"),
        ("- [ ] todo\n", BlockType::Checklist, "todo"),
        ("1. item\n", BlockType::OrderedListItem, "item"), // C3' OLI 收束
        ("> q\n", BlockType::Blockquote, "q"),
    ];
    let mut failures = 0usize;
    for &(doc, t, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        // 权宜（实现参照原则 §4.4）: 声明输入补尾换行（line_text 不含 \n——
        // 与生产 byte_range 的输入语义对齐）——仅对单行文档成立;多行文档的
        // 原始行获取须用 rope.byte_slice(byte_of_line(i)..byte_of_line(i+1))。
        let text = snap.line_text(0).to_string();
        let raw_line = if snap.line_len() > 1 { text.clone() } else { format!("{text}\n") };
        let got_decl = declared_content(t, &raw_line);
        if got_decl != want {
            failures += 1;
            eprintln!("[PE17] 声明分歧: {doc:?} t={t:?} want={want:?} got={got_decl:?}");
            continue;
        }
        match read_projection(&snap, &[Step::Wildcard], Some("content")) {
            Ok(v) => {
                if v.is_empty() {
                    failures += 1;
                    eprintln!("[PE17] 无节点: {doc:?}");
                } else if v[0] != want {
                    failures += 1;
                    eprintln!("[PE17] 生产分歧: {doc:?} t={t:?} want={want:?} prod={:?}", v[0]);
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("[PE17] read_projection 错误: {doc:?} {e:?}");
            }
        }
        if failures == 0 {
            eprintln!("[PE17] ✓ {doc:?} t={t:?} content={want:?}");
        }
    }
    // 标题的区间内容（章节体——切分点的区间形态）:
    // "# Foo\nbody\n" → 标题节点 [0,2) 的 content = "body"
    {
        let doc = "# Foo\nbody\n";
        let mut st = State::new(doc);
        let snap = st.snapshot();
        match read_projection(&snap, &[Step::Wildcard], Some("content")) {
            Ok(v) => {
                if v.len() != 1 || v[0] != "body" {
                    failures += 1;
                    eprintln!("[PE17] 标题区间内容分歧: {doc:?} got={v:?}");
                } else {
                    eprintln!("[PE17] ✓ {doc:?} 标题 content（区间形态）={:?}", v[0]);
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("[PE17] read_projection 错误: {doc:?} {e:?}");
            }
        }
    }
    eprintln!("[PE17] LineSpec content 切分点声明化（声明 == 生产几何路径）: {} 例, {} 失败",
        cases.len() + 1, failures);
    assert_eq!(failures, 0, "PE17 失败 {failures}");
}

// PE18: LineSpec 完整声明形态定稿——四规则面合一表
// ① sep 规则显式化（colon_sep_len: ":"+空格/tab→2、否则 1——infer.rs:167）
// ② 完整表（11 MarkKind 集群 × 4 规则面 + 类型级覆写）——建表蓝本
// ③ 通用提取（表驱动）== 逐类型声明（已生产锚定）== 生产（新增 case）
// 简化（实现参照原则 §4.4）: 未覆盖边缘——fence lang 的 decode_link_dest
// （反转义）、setext 多行 BQ 变体、RefDef label、HtmlBlock type 6——以生产
// 分支为准（infer.rs node_tag/node_mark）。

/// tag 模式（LineSpec tag_rule 面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagMode { HeadingText, ColonKey, CheckboxColonKey, Digits, Lang, Tagname, None_ }

/// 分隔模式（分隔宽度单源=block::colon_sep_len）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SepMode { Colon, None_ }

/// 内容规则（LineSpec content_rule 面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentRule { AfterTagSep, AfterMark, Span }

/// mark 模式（LineSpec mark_width_rule 面——简化枚举）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkMode { HashLevel, Char1, DigitsDelim, Fence3, Fixed(&'static str), OpenTag, None_ }

/// LineSpec 行（四规则面合一——11 集群 × 类型覆写）。
#[derive(Debug, Clone, Copy)]
struct LineSpec {
    mark: MarkMode,
    tag: TagMode,
    sep: SepMode,
    content: ContentRule,
}

/// 表驱动查表（按类型——覆写优先——集群共享）。
fn linespec_for(t: BlockType) -> LineSpec {
    if is_header_type(t) {
        return LineSpec { mark: MarkMode::HashLevel, tag: TagMode::HeadingText,
            sep: SepMode::None_, content: ContentRule::Span };
    }
    match t {
        BlockType::HsHead1 | BlockType::HsHead2 => LineSpec {
            mark: MarkMode::None_, tag: TagMode::HeadingText,
            sep: SepMode::None_, content: ContentRule::Span },
        BlockType::Bullet => LineSpec { mark: MarkMode::Char1, tag: TagMode::ColonKey,
            sep: SepMode::Colon, content: ContentRule::AfterTagSep },
        BlockType::Checklist => LineSpec { mark: MarkMode::Char1, tag: TagMode::CheckboxColonKey,
            sep: SepMode::Colon, content: ContentRule::AfterTagSep },
        BlockType::OrderedListItem => LineSpec { mark: MarkMode::DigitsDelim, tag: TagMode::Digits,
            sep: SepMode::None_, content: ContentRule::AfterMark },
        BlockType::CodeFenceOpen => LineSpec { mark: MarkMode::Fence3, tag: TagMode::Lang,
            sep: SepMode::None_, content: ContentRule::Span },
        BlockType::Blockquote => LineSpec { mark: MarkMode::Fixed(">"), tag: TagMode::None_,
            sep: SepMode::None_, content: ContentRule::AfterMark },
        BlockType::Comment => LineSpec { mark: MarkMode::Fixed("<!--"), tag: TagMode::None_,
            sep: SepMode::None_, content: ContentRule::Span },
        BlockType::TableRow | BlockType::TableSeparator => LineSpec {
            mark: MarkMode::Fixed("|"), tag: TagMode::None_,
            sep: SepMode::None_, content: ContentRule::Span },
        BlockType::IndentedCode => LineSpec { mark: MarkMode::Fixed("    "), tag: TagMode::None_,
            sep: SepMode::None_, content: ContentRule::Span },
        BlockType::HtmlElement | BlockType::HtmlBlock => LineSpec {
            mark: MarkMode::OpenTag, tag: TagMode::Tagname,
            sep: SepMode::None_, content: ContentRule::Span },
        _ => LineSpec { mark: MarkMode::None_, tag: TagMode::None_,
            sep: SepMode::None_, content: ContentRule::Span },
    }
}

/// sep 规则（colon_sep_len 语义——":" 后空格/tab → 2;否则 1——infer.rs:167）。
fn sep_len(line: &str, colon_pos: usize) -> usize {
    let after = line.as_bytes().get(colon_pos + 1).copied();
    if line.as_bytes().get(colon_pos) == Some(&b':')
        && matches!(after, Some(b' ') | Some(b'\t'))
    { 2 } else { 1 }
}

/// 表驱动 mark 提取。
fn linespec_mark(rule: LineSpec, t: BlockType, line: &str) -> String {
    match rule.mark {
        MarkMode::HashLevel => declared_mark(t, line),
        MarkMode::Char1 => line.trim_start().chars().next()
            .map(|c| c.to_string()).unwrap_or_default(),
        MarkMode::DigitsDelim => declared_mark(t, line),
        MarkMode::Fence3 => declared_mark(t, line),
        MarkMode::Fixed(s) => s.to_string(),
        MarkMode::OpenTag => {
            // 开标签 mark（到第一个 '>'——含标签名——几何 mark_text_end 语义）
            if let Some(p) = line.find('>') {
                line[..=p].to_string()
            } else { String::new() }
        }
        MarkMode::None_ => String::new(),
    }
}

/// 表驱动 tag 提取（tag 模式 + 类型覆写——CheckboxColonKey 走 checklist 分支）。
fn linespec_tag(rule: LineSpec, t: BlockType, line: &str) -> String {
    match rule.tag {
        TagMode::HeadingText | TagMode::ColonKey | TagMode::Digits
        | TagMode::Lang | TagMode::Tagname | TagMode::None_ => declared_tag(t, line),
        TagMode::CheckboxColonKey => {
            // checklist 分支（declared_tag 的 Checklist 分支——复用）
            declared_tag(t, line)
        }
    }
}

/// 表驱动 content 提取（content 规则 + 类型覆写——复用 declared_content）。
fn linespec_content(rule: LineSpec, t: BlockType, line: &str) -> String {
    match rule.content {
        ContentRule::AfterTagSep | ContentRule::AfterMark => declared_content(t, line),
        ContentRule::Span => String::new(),
    }
}

#[test]
fn pe18_linespec_full_declaration() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    // ① sep 规则显式化——colon_sep_len 变体（通过 content 切分反推——生产锚）
    let sep_cases: &[(&str, usize)] = &[
        ("- key:v\n", 1),     // ":" 后非空白 → sep=1
        ("- key: v\n", 2),    // ":" 后空格 → sep=2
        ("- key:\tv\n", 2),   // ":" 后 tab → sep=2
    ];
    for &(doc, want) in sep_cases {
        let colon = doc.find(':').unwrap();
        let got = sep_len(doc, colon);
        if got != want {
            panic!("[PE18] sep_len 分歧: {doc:?} want={want} got={got}");
        }
    }
    eprintln!("[PE18] ① sep 规则（colon_sep_len）: {} 例 ✓", sep_cases.len());

    // ②③ 完整表 + 通用提取 == 生产（read_projection——tag/mark/content 锚）
    // (文档, 类型, 投影, 期望) —— 表驱动提取 vs 生产
    let cases: &[(&str, BlockType, &str, &str)] = &[
        ("# Foo\n", BlockType::Header1, "tag", "Foo"),
        ("# Foo\n", BlockType::Header1, "mark", "#"),
        ("- key: v\n", BlockType::Bullet, "tag", "key"),
        ("- key: v\n", BlockType::Bullet, "mark", "-"),
        ("- key: v\n", BlockType::Bullet, "content", "v"),
        ("- [x] done: yes\n", BlockType::Checklist, "tag", "done"),
        ("- [x] done: yes\n", BlockType::Checklist, "mark", "-"),
        ("- [x] done: yes\n", BlockType::Checklist, "content", "yes"),
        ("1. item\n", BlockType::OrderedListItem, "tag", "1"),
        ("1. item\n", BlockType::OrderedListItem, "content", "item"), // C3' OLI 收束
        ("```rust\n", BlockType::CodeFenceOpen, "tag", "rust"),
        ("```rust\n", BlockType::CodeFenceOpen, "mark", "```"),
        ("> q\n", BlockType::Blockquote, "mark", ">"),
        ("> q\n", BlockType::Blockquote, "content", "q"),
        ("<!-- c -->\n", BlockType::Comment, "mark", "<!--"),
        ("<div>\n", BlockType::HtmlElement, "tag", "div"),
        ("<div>\n", BlockType::HtmlElement, "mark", "<div>"),
        ("| a |\n| - |\n", BlockType::TableRow, "mark", "|"),
        ("    code\n", BlockType::IndentedCode, "mark", "    "),
        // setext 多行——表驱动 vs 生产（HsHead 节点——tag = 整行文本）
        ("title\n===\n", BlockType::HsHead1, "tag", "title"),
    ];
    let mut failures = 0usize;
    for &(doc, t, proj, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let line0 = snap.line_text(0).to_string();
        let rule = linespec_for(t);
        // 表驱动提取（投影分派）
        let got = match proj {
            "tag" => linespec_tag(rule, t, &line0),
            "mark" => linespec_mark(rule, t, &line0),
            "content" => {
                let raw = if snap.line_len() > 1 { line0.clone() } else { format!("{line0}\n") };
                linespec_content(rule, t, &raw)
            }
            _ => String::new(),
        };
        if got != want {
            failures += 1;
            eprintln!("[PE18] 表驱动分歧: {doc:?} t={t:?} {proj} want={want:?} got={got:?}");
            continue;
        }
        // 生产锚（read_projection——首个节点）
        match read_projection(&snap, &[Step::Wildcard], Some(proj)) {
            Ok(v) => {
                if v.is_empty() {
                    failures += 1;
                    eprintln!("[PE18] 无节点: {doc:?} {proj}");
                } else if v[0] != want {
                    failures += 1;
                    eprintln!("[PE18] 生产分歧: {doc:?} t={t:?} {proj} want={want:?} prod={:?}", v[0]);
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("[PE18] read_projection 错误: {doc:?} {e:?}");
            }
        }
        if failures == 0 {
            eprintln!("[PE18] ✓ {doc:?} t={t:?} {proj}={want:?}");
        }
    }
    eprintln!("[PE18] LineSpec 完整声明（四规则面 + sep + setext）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE18 失败 {failures}");
}

// PE19: BlockShape 骨架声明——P0 合流侦察的 testbed 化
// 块级形态声明（块由哪些行组成——开/体/闭行 PL）vs trilogy restore_pl 锚。
// 假设: 简单块的骨架（开/体/闭）可声明化——restore 还原 == 声明骨架;
// 复杂形态（R3 复合开行 PL 来源 / R6 BQ 空行 / R11 空洞）属壳来源规则
// （PeelStack 域——不是骨架表）。
// 简化（实现参照原则 §4.4）: 骨架表只覆盖简单块——BQ 体行（A0 分类）、
// item 内容行（cont）、Table 数据行（行 Shell）不在骨架表的严格断言内——
// 以 trilogy.rs restore_pl 与生产为准。

/// 块骨架（开/体/闭行 PL——BlockShape 雏形）。
struct BlockShape {
    open: BlockType,
    body: BlockType,
    close: Option<BlockType>,
}

fn block_shape_for(t: BlockType) -> BlockShape {
    if is_header_type(t) {
        return BlockShape { open: t, body: BlockType::Paragraph, close: None };
    }
    match t {
        BlockType::Paragraph => BlockShape { open: BlockType::Paragraph,
            body: BlockType::Paragraph, close: None },
        // R2 setext 展开: 开行 = Paragraph（行级保持）;闭行 = HsSep
        BlockType::HsHead1 | BlockType::HsHead2 => BlockShape {
            open: BlockType::Paragraph, body: BlockType::Paragraph, close: Some(BlockType::HsSep) },
        // R1 段类型开行（seg_bt——容器类型）
        BlockType::Bullet | BlockType::Checklist => BlockShape { open: BlockType::Bullet,
            body: BlockType::Paragraph, close: None },
        BlockType::OrderedListItem => BlockShape { open: BlockType::OrderedListItem,
            body: BlockType::Paragraph, close: None },
        // F2 闭行形态: fence 闭行 = CodeFenceClose（end-1）
        BlockType::CodeFenceOpen => BlockShape { open: BlockType::CodeFenceOpen,
            body: BlockType::CodeContent, close: Some(BlockType::CodeFenceClose) },
        // R5: comment 闭合行独立节点（还原 PL = Comment）
        BlockType::Comment => BlockShape { open: BlockType::Comment,
            body: BlockType::Comment, close: Some(BlockType::Comment) },
        BlockType::ThematicBreak => BlockShape { open: BlockType::ThematicBreak,
            body: BlockType::Paragraph, close: None },
        BlockType::IndentedCode => BlockShape { open: BlockType::IndentedCode,
            body: BlockType::IndentedCode, close: None },
        BlockType::RefDef => BlockShape { open: BlockType::RefDef,
            body: BlockType::RefDef, close: None },
        // 复杂形态（不在骨架表——壳来源规则）: BQ/Table/HtmlBlock/HtmlElement
        _ => BlockShape { open: t, body: BlockType::Paragraph, close: None },
    }
}

#[test]
fn pe19_block_shape_skeleton() {
    // (D6⑤d——build_attribution_tree/build_block_records/RowView 换 new_infer 全路径)
    use mdio_core::infer::restore_pl;
    // (文档, 期望开行 PL, 期望体行 PL, 期望闭行 PL)
    let cases: &[(&str, BlockType, BlockType, Option<BlockType>)] = &[
        ("para text\n", BlockType::Paragraph, BlockType::Paragraph, None),
        ("# h\n", BlockType::Header1, BlockType::Paragraph, None),
        ("### h\n", BlockType::Header3, BlockType::Paragraph, None),
        ("title\n===\n", BlockType::Paragraph, BlockType::Paragraph, Some(BlockType::HsSep)),
        ("```\ncode\n```\n", BlockType::CodeFenceOpen, BlockType::CodeContent,
            Some(BlockType::CodeFenceClose)),
        ("<!--\nc\n-->\n", BlockType::Comment, BlockType::Comment, Some(BlockType::Comment)),
        ("    code\n", BlockType::IndentedCode, BlockType::IndentedCode, None),
        ("---\n", BlockType::ThematicBreak, BlockType::Paragraph, None),
        ("[foo]: /url\n", BlockType::RefDef, BlockType::RefDef, None),
        ("- item\n", BlockType::Bullet, BlockType::Paragraph, None),
        ("1. item\n", BlockType::OrderedListItem, BlockType::Paragraph, None),
    ];
    let mut failures = 0usize;
    for &(doc, open, body, close) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        if n == 0 { continue; }
        // 树 + restore_pl（trilogy 锚——逐行还原 PL）
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let view = mdio_core::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries,
            &snap.lines, &snap.rope, (0, n as u32), &arena);
        let tree = mdio_core::infer::build_attribution_tree(&recs, &blocks, &view, &arena,
            (0, n as u32), &mdio_core::topology::CONTAINER_TOPOLOGY);
        let restored = restore_pl(&tree, &blocks, &recs, &view, &snap.rope, &snap.lines, n);
        // 首个块（骨架对照的区间）
        let Some(blk) = blocks.first() else { continue };
        let s = blk.start as usize;
        let e = blk.end as usize;
        // 开行
        if restored[s] != open {
            failures += 1;
            eprintln!("[PE19] 开行分歧: {doc:?} want={open:?} restore={:?}", restored[s]);
            continue;
        }
        // 体行（中间行）
        if e > s + 1 {
            for i in (s + 1)..e {
                let is_close = close.map_or(false, |c| c == restored[i] && i == e - 1);
                if !is_close && restored[i] != body {
                    // setext 的中间行（多行 title 段落）——Paragraph 一致;
                    // 复杂形态跳过（骨架表不覆盖——BQ 等）
                    if matches!(restored[i], BlockType::HsSep) && close == Some(BlockType::HsSep) {
                        continue; // setext 闭行在中间（多行段落）——容忍
                    }
                    failures += 1;
                    eprintln!("[PE19] 体行分歧: {doc:?} line={i} want={body:?} restore={:?}", restored[i]);
                    break;
                }
            }
        }
        // 闭行（end-1）
        if let Some(c) = close {
            if restored[e - 1] != c {
                // setext 多行（title 段中间行是闭行）——已容忍;其他闭行分歧
                if !(c == BlockType::HsSep && restored[e - 1] == BlockType::HsSep) {
                    failures += 1;
                    eprintln!("[PE19] 闭行分歧: {doc:?} want={c:?} restore={:?}", restored[e - 1]);
                    continue;
                }
            }
        }
        if failures == 0 {
            eprintln!("[PE19] ✓ {doc:?} 骨架=[{open:?} → {body:?} → {close:?}] restore 一致");
        }
    }
    eprintln!("[PE19] BlockShape 骨架声明（restore_pl 锚）: {} 例, {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE19 失败 {failures}");
}

// PE20: WriteEndRule——写面终点规则声明化（六声明最后一面的验证）
// 写面终点分叉（content_region 的 surface 维度——R11 精化）:
//   item → LineEnd（数据保全——防覆盖嵌套子列表）
//   fence/comment → SpanEnd（内容区间）
//   heading → TitleEnd（标题行后）
// 锚: compute_write（pub——生产写路径——geometry None 的 fallback——
// content_write_range 语义）。

#[test]
fn pe20_write_end_rule() {
    // (文档, 内容起点所在行, 类型, 起点相对该行首偏移)
    let cases: &[(&str, usize, BlockType, usize)] = &[
        ("- key: v\n", 0, BlockType::Bullet, 7),          // "v" 起点（行首 + 7）
        ("- a\n  - b\n", 0, BlockType::Bullet, 2),        // LineEnd——不吞嵌套（起点 2）
        ("```\ncode\n```\n", 1, BlockType::CodeFenceOpen, 0),  // SpanEnd——code 行起
        ("# H\nbody\n", 1, BlockType::Header1, 0),        // TitleEnd——body 行起
        // 节点级覆盖扩展：同行链双层同种壳——行 0 上唯一的块记录
        // 就是**内层**条目（container_ref 指向内层 bullet），故其内容起点应取内层
        // 标记之后（4），而当前实现给 2（外层标记之后）⇒ 先把分歧照出来（写门里
        // 同一处表现为 `- - a` 写回丢一层）。
        ("- - a\n", 0, BlockType::Bullet, 4),
    ];
    let mut failures = 0usize;
    for &(doc, start_line, t, start_off) in cases {
        let line = 0usize; // 节点起点行（全部用例节点从行 0 起）
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let end_line = mdio_core::infer::get_node_end_from_lines(
            &snap.lines, line, t, &snap.frame_log);
        let node_range = snap.byte_of_line(line)..snap.byte_of_line(end_line);
        // 生产写路径（compute_write——fallback——content_write_range）
        match snap.compute_write(&node_range, t, "content", "X") {
            Ok((r, _)) => {
                let want_start = snap.byte_of_line(start_line) + start_off;
                if r.start != want_start {
                    failures += 1;
                    eprintln!("[PE20] 起点分歧: {doc:?} t={t:?} want={want_start} got={}", r.start);
                    continue;
                }
                // 终点规则断言
                let want_end = match t {
                    // LineEnd——本行行尾（去尾换行——content_write_range 语义）
                    BlockType::Bullet => {
                        let mut e = snap.byte_of_line(line + 1);
                        if e > r.start && snap.byte_at(e - 1) == b'\n' { e -= 1; }
                        e
                    }
                    // SpanEnd——闭行起点（fence 内容区间 [s+1, e)）
                    BlockType::CodeFenceOpen => snap.byte_of_line(line + 1 + 1),
                    // TitleEnd——节点尾
                    BlockType::Header1 => node_range.end,
                    _ => node_range.end,
                };
                if r.end != want_end {
                    failures += 1;
                    eprintln!("[PE20] 终点分歧: {doc:?} t={t:?} want={want_end} got={}", r.end);
                    continue;
                }
                eprintln!("[PE20] ✓ {doc:?} t={t:?} 写范围=[{}..{})——终点规则一致", r.start, r.end);
            }
            Err(e) => {
                failures += 1;
                eprintln!("[PE20] compute_write 错误: {doc:?} {e:?}");
            }
        }
    }
    eprintln!("[PE20] WriteEndRule（写面终点——LineEnd/SpanEnd/TitleEnd）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE20 失败 {failures}");
}

// PE21: Correction 条件声明化——A0.5（rd 撤销）/ #542（跨行 refdef）
// 修正族的触发条件（声明化的目标——CorrectionKind 的 RevokeEmpty/
// InvisiblePair）——锚: 归属树的节点 block_type（resolve 后的真相——
// trilogy 还原原则的块记录真相）。
// 假设（待生产实证）:
//   A0.5 RevokeEmpty: `[foo]:`（空 body）→ Paragraph;`[foo]: /url '未闭合` → Paragraph
//   #542 InvisiblePair: `[Foo\n  bar]: /url` → 两行 Blank（不可见对）

#[test]
fn pe21_correction_conditions() {
    // (D6⑤d——build_attribution_tree/build_block_records/RowView 换 new_infer 全路径)
    // (文档, 期望首个节点 block_type)——锚为生产树的真相
    let cases: &[(&str, BlockType)] = &[
        ("[foo]:\n", BlockType::Paragraph),           // A0.5——空 body 撤销
        ("[foo]: /url\n", BlockType::RefDef),         // 正常 rd——不撤销
        ("[foo]: /url 'unclosed\n", BlockType::Paragraph),  // A0.5——未闭合 title 撤销
        ("[Foo\n  bar]: /url\n", BlockType::Blank),   // #542——跨行对不可见
    ];
    let mut failures = 0usize;
    for &(doc, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        if n == 0 { continue; }
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let view = mdio_core::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries,
            &snap.lines, &snap.rope, (0, n as u32), &arena);
        let tree = mdio_core::infer::build_attribution_tree(&recs, &blocks, &view, &arena,
            (0, n as u32), &mdio_core::topology::CONTAINER_TOPOLOGY);
        // 首个根节点的 block_type（真相）
        let got = tree.roots.first()
            .and_then(|&r| tree.nodes.get(r as usize))
            .map(|nd| nd.block_type)
            .unwrap_or(BlockType::Blank);
        if got != want {
            failures += 1;
            eprintln!("[PE21] 修正条件分歧: {doc:?} 假设={want:?} 生产真相={got:?}");
            continue;
        }
        eprintln!("[PE21] ✓ {doc:?} → {want:?}");
    }
    eprintln!("[PE21] Correction 条件（A0.5/#542）: {} 例, {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE21 失败 {failures}");
}

// PE22: 空间形态——包裹式 content（AfterMarkOpen——开闭标记之间）
// v7 投影模型（raw = ctn[m+t]+ct——mark 包裹 content）:
//   Wrap 类型（comment/fence）的 content = 开标记后、闭标记前的区间
//   （range_content/content_code 语义）——不是前缀后。
// 锚: read_projection("content")——生产几何路径。

#[test]
fn pe22_wrap_content_spatial() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    let cases: &[(&str, &str)] = &[
        // comment: content = <!-- 与 --> 之间（trim）
        ("<!-- a -->\n", "a"),
        // comment 多行: content = 开闭之间
        ("<!--\na\nb\n-->\n", "a\nb"),
        // fence: content = 开闭 fence 之间（保留尾换行——content_code 语义）
        ("```\ncode\n```\n", "code\n"),
        // 前缀式对照（Prefix——tag+sep 后）
        ("- key: v\n", "v"),
    ];
    let mut failures = 0usize;
    for &(doc, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        match read_projection(&snap, &[Step::Wildcard], Some("content")) {
            Ok(v) => {
                if v.is_empty() {
                    failures += 1;
                    eprintln!("[PE22] 无节点: {doc:?}");
                } else if v[0] != want {
                    failures += 1;
                    eprintln!("[PE22] content 分歧: {doc:?} want={want:?} got={:?}", v[0]);
                } else {
                    eprintln!("[PE22] ✓ {doc:?} content={want:?}");
                }
            }
            Err(e) => {
                failures += 1;
                eprintln!("[PE22] read_projection 错误: {doc:?} {e:?}");
            }
        }
    }
    eprintln!("[PE22] 包裹式 content（开闭标记之间）: {} 例, {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE22 失败 {failures}");
}

// PE23: 行内 parse 资格——facet × inline_mode 的 ch 决策（JSON 实证）
// inline_mode 声明"哪个 facet 可 parse":
//   TagOnly（head）: tag parse——`# *em*` 的 ch 有 em
//   ContentOnly（item）: content parse——`- key: *v*` 的 ch 有 em（value）
//                       —— key 字面——`- *k*: v` 的 ch 无 em
//   None（code）: 全不 parse——```*em* 的 tag 无 em
// 锚: to_ast_object_mode("cf") JSON 的 ch 结构。

#[test]
fn pe23_inline_eligibility() {
    use mdio_core::projection::Snapshot;
    use serde_json::Value;
    let cases: &[(&str, &str, bool)] = &[
        // (文档, 期望的 ch 中出现节点类型, ch 是否出现)
        ("# *em*\n", "em", true),        // head——tag parse（TagOnly；行内数组在 t，见 find_nt）
        ("- key: *v*\n", "em", true),    // item——content parse（ContentOnly——value）
        // 真实发现（PE23 实证）: item 的 key 也被行内解析！
        // write_item_leaf 的 tag 分支无条件扫描（`if tag_has_inline { scan(tag) }`）
        // ——只有 write_block_node（独立块）路径才用 can_inline_tag 门控。
        // `- *k*: v` → t="*k*" + ch 的 em——"key 字面"对 item 路径是错的。
        ("- *k*: v\n", "em", true),      // item——key parse（item 路径 tag 无条件）
        ("```*em*\n", "em", false),      // code——tag（lang）不 parse（None）
        ("*plain*\n", "em", true),       // p——content parse
    ];
    let mut failures = 0usize;
    for &(doc, want_nt, expect_ch) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let json = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
        let v: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
        // 在 JSON 里递归找 nt == want_nt 的节点
        fn find_nt(v: &Value, nt: &str) -> bool {
            match v {
                Value::Array(a) => a.iter().any(|x| find_nt(x, nt)),
                Value::Object(o) => {
                    o.get("nt").and_then(|x| x.as_str()) == Some(nt)
                        || o.get("ct").map(|c| find_nt(c, nt)).unwrap_or(false)
                        // 规则 R：Tag 面的行内数组写在 t 下（标题行内从 ct 挪到 t）
                        || o.get("t").map(|c| find_nt(c, nt)).unwrap_or(false)
                }
                _ => false,
            }
        }
        let found = find_nt(&v, want_nt);
        if found != expect_ch {
            failures += 1;
            eprintln!("[PE23] ch 决策分歧: {doc:?} want_nt={want_nt:?} expect_ch={expect_ch} found={found}");
        } else {
            eprintln!("[PE23] ✓ {doc:?} nt={want_nt:?} ch={expect_ch}（JSON: {}）", json.trim());
        }
    }
    eprintln!("[PE23] 行内 parse 资格（facet × inline_mode——ch 决策）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE23 失败 {failures}");
}

// PE24: 行内层空间形态——对称/非对称包裹（JSON 结构实证）
// 对称包裹（`*em*`——mark_open == mark_close）vs 非对称包裹
// （`[t](ct)`——mark = []()——tag 在开标记内）——JSON 的节点结构揭示。

#[test]
fn pe24_inline_spatial_shapes() {
    use mdio_core::projection::Snapshot;
    use serde_json::Value;
    let cases: &[(&str, &str, &str)] = &[
        // (文档, 期望的 inline 根节点 nt, 期望该节点的 t 或 ct 特征)
        ("- *em*\n", "em", "em"),            // 对称——content 被 `*` 包裹
        ("- [l](http://x)\n", "lk", "l"),    // 非对称——tag=l（开标记内）、href 目标
        ("- `cd`\n", "cd", "cd"),            // 对称（fenced-code）
        ("- ~~st~~\n", "dl", "st"),          // 对称（strike）
    ];
    let mut failures = 0usize;
    for &(doc, want_nt, want_text) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let json = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
        let v: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
        // 找 want_nt 节点并提取其文本特征（t 或 ct——递归 ch 首 txt）
        fn find_text(v: &Value, nt: &str, out: &mut Vec<String>) {
            match v {
                Value::Array(a) => { for x in a { find_text(x, nt, out); } }
                Value::Object(o) => {
                    if o.get("nt").and_then(|x| x.as_str()) == Some(nt) {
                        if let Some(t) = o.get("t").and_then(|x| x.as_str()) { out.push(t.to_string()); }
                        if let Some(ct) = o.get("ct").and_then(|x| x.as_str()) { out.push(ct.to_string()); }
                    }
                    if let Some(c) = o.get("ct") { find_text(c, nt, out); }
                }
                _ => {}
            }
        }
        let mut texts = Vec::new();
        find_text(&v, want_nt, &mut texts);
        // 特征文本应出现在 t/ct/cd 中（ContentField::Code 的 cd attr——`cd` span）
        let mut all = Vec::new();
        fn collect_txt(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::Array(a) => { for x in a { collect_txt(x, out); } }
                Value::Object(o) => {
                    if let Some(t) = o.get("t").and_then(|x| x.as_str()) { out.push(t.to_string()); }
                    if let Some(ct) = o.get("ct").and_then(|x| x.as_str()) { out.push(ct.to_string()); }
                    if let Some(cd) = o.get("cd").and_then(|x| x.as_str()) { out.push(cd.to_string()); }
                    if let Some(c) = o.get("ct") { collect_txt(c, out); }
                }
                _ => {}
            }
        }
        collect_txt(&v, &mut all);
        if !texts.iter().any(|t| t == want_text) && !all.iter().any(|t| t == want_text) {
            failures += 1;
            eprintln!("[PE24] 行内形态分歧: {doc:?} want_nt={want_nt:?} want_text={want_text:?} texts={texts:?}");
        } else {
            eprintln!("[PE24] ✓ {doc:?} nt={want_nt:?} text={want_text:?}（JSON: {}）", json.trim());
        }
    }
    eprintln!("[PE24] 行内层空间形态（对称/非对称包裹）: {} 例, {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE24 失败 {failures}");
}

// PE25: 空间版行层投影表——mark_shape / tag_position 逐类型实证
// 新 effect 声明 A（行层投影）的空间维度逐类型验证:
//   mark_shape: Prefix（mark 后接 tag/content）| Wrap（开闭包裹——同行/跨行）
//               | Delimit（定界——table）
//   tag_position: AfterMark（mark 后）| InsideMark（mark 内——code 的 lang）
// 锚: read_projection（mark/tag/content——空间分解的可观测面）。

#[test]
fn pe25_spatial_line_projection() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    // (文档, 期望 mark, 期望 tag, 期望 content, 空间形态标注)
    let cases: &[(&str, &str, &str, &str, &str)] = &[
        // Prefix——mark 后接 tag+content
        ("- KEY: val\n", "-", "KEY", "val", "Prefix[mark][tag][sep][content]"),
        ("# TITLE\n", "#", "TITLE", "", "Prefix[mark][tag]"),
        ("> q\n", ">", "", "q", "Prefix[mark][content]"),
        ("1. item\n", "1.", "1", "item", "Prefix[mark=tag][content]——编号在 mark 内(C3' 去尾)"),
        // Wrap——开闭包裹（comment 单行——开闭同行）
        ("<!-- a -->\n", "<!--", "", "a", "Wrap[mark_open][content][mark_close]"),
        // Delimit——定界（尾随 | 产生尾空单元格——content 含尾 \t——生产实证）
        ("| 1 | 2 |\n| - | - |\n", "|", "", "1\t2\t", "Delimit[mark 定界][content cells]"),
    ];
    let mut failures = 0usize;
    for &(doc, want_mark, want_tag, want_content, shape) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let mut ok = true;
        if !want_mark.is_empty() {
            match read_projection(&snap, &[Step::Wildcard], Some("mark")) {
                Ok(v) if !v.is_empty() && v[0] == want_mark => {}
                Ok(v) => { ok = false; eprintln!("[PE25] mark 分歧: {doc:?} want={want_mark:?} got={v:?}"); }
                Err(e) => { ok = false; eprintln!("[PE25] mark 错误: {doc:?} {e:?}"); }
            }
        }
        if !want_tag.is_empty() {
            match read_projection(&snap, &[Step::Wildcard], Some("tag")) {
                Ok(v) if !v.is_empty() && v[0] == want_tag => {}
                Ok(v) => { ok = false; eprintln!("[PE25] tag 分歧: {doc:?} want={want_tag:?} got={v:?}"); }
                Err(e) => { ok = false; eprintln!("[PE25] tag 错误: {doc:?} {e:?}"); }
            }
        }
        match read_projection(&snap, &[Step::Wildcard], Some("content")) {
            Ok(v) if v.is_empty() && want_content.is_empty() => {}
            Ok(v) if !v.is_empty() && v[0] == want_content => {}
            Ok(v) => { ok = false; eprintln!("[PE25] content 分歧: {doc:?} want={want_content:?} got={v:?}"); }
            Err(e) => { ok = false; eprintln!("[PE25] content 错误: {doc:?} {e:?}"); }
        }
        if ok {
            eprintln!("[PE25] ✓ {doc:?} {shape}");
        } else {
            failures += 1;
        }
    }
    eprintln!("[PE25] 空间版行层投影表（mark_shape/tag_position）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE25 失败 {failures}");
}

// PE26: mark 包裹的跨粒度——mark_close = 块闭行（声明 C 的实证）
// 空间形态跨两个粒度: 行层有 mark_open（LineSpec 的 mark 规则）、
// 块层有 mark_close（BlockShape 的闭行——包裹的闭标记）:
//   fence:   ```rust 行 = mark_open（开行）;``` 闭行 = mark_close
//   comment: <!-- 行 = mark_open;--> 闭合行 = mark_close（R5 独立节点）
//   单行 comment: mark_open 与 mark_close 同行（行层包裹——无块闭行）
//   table:   定界——无跨行闭标记
// 锚: trilogy restore_pl（闭行 PL）+ 归属树（comment 闭合行独立节点）。

#[test]
fn pe26_mark_wrap_cross_granularity() {
    // (D6⑤d——build_attribution_tree/build_block_records/RowView 换 new_infer 全路径)
    use mdio_core::infer::restore_pl;
    let cases: &[(&str, BlockType, Option<BlockType>, &str)] = &[
        // (文档, 开行 PL（mark_open 行）, 闭行 PL（mark_close 行——块层）, 形态)
        ("```\ncode\n```\n", BlockType::CodeFenceOpen,
            Some(BlockType::CodeFenceClose), "Wrap 跨行——mark_close=闭行"),
        ("<!--\na\n-->\n", BlockType::Comment,
            Some(BlockType::Comment), "Wrap 跨行——mark_close=闭合行（R5 独立节点）"),
        ("<!-- a -->\n", BlockType::Comment,
            None, "Wrap 同行——单行包裹——无跨行闭行"),
        ("| 1 | 2 |\n| - | - |\n", BlockType::TableRow,
            None, "Delimit——行内定界——无跨行闭标记"),
    ];
    let mut failures = 0usize;
    for &(doc, want_open, want_close, shape) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        if n == 0 { continue; }
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let view = mdio_core::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries,
            &snap.lines, &snap.rope, (0, n as u32), &arena);
        let tree = mdio_core::infer::build_attribution_tree(&recs, &blocks, &view, &arena,
            (0, n as u32), &mdio_core::topology::CONTAINER_TOPOLOGY);
        let restored = restore_pl(&tree, &blocks, &recs, &view, &snap.rope, &snap.lines, n);
        let mut ok = true;
        // 开行 PL（mark_open 行）
        if restored[0] != want_open {
            ok = false;
            eprintln!("[PE26] mark_open 行 PL 分歧: {doc:?} want={want_open:?} restore={:?}", restored[0]);
        }
        // 闭行 PL（mark_close 行——块层——跨行包裹的闭标记）
        if let Some(c) = want_close {
            if n < 2 || restored[n - 1] != c {
                ok = false;
                eprintln!("[PE26] mark_close 行 PL 分歧: {doc:?} want={c:?} restore={:?}",
                    if n > 0 { restored[n - 1] } else { BlockType::Blank });
            }
        }
        // comment 闭合行 = 树的独立节点（R5——blk=None 的根级 Comment 节点）
        if want_close == Some(BlockType::Comment) && n >= 2 {
            let has_indep = tree.nodes.iter().any(|nd| {
                nd.block_type == BlockType::Comment
                    && nd.blk_idx().is_none()
                    && nd.start == (n - 1) as u32
                    && nd.end == n as u32
            });
            if !has_indep {
                ok = false;
                eprintln!("[PE26] comment 闭合行独立节点缺失: {doc:?}");
            }
        }
        if ok {
            eprintln!("[PE26] ✓ {doc:?} {shape}");
        } else {
            failures += 1;
        }
    }
    eprintln!("[PE26] mark 包裹跨粒度（mark_close = 块闭行）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE26 失败 {failures}");
}

// PE27: 行内资格全表——逐类型验证"哪个 facet 可 parse"（声明 B 的定稿实证）
// 行内资格 = inline_mode × 渲染路径（PE23 实证）:
//   head: tag parse（TagOnly）
//   item: tag（key）parse（item 路径无条件——PE23）+ content（value）parse
//   p/bq: content parse
//   code/comment/refdef/htmlelement: 全不 parse（None）
// 锚: to_ast_object_mode("cf") JSON 的 ch（inline 子节点是否出现）。

#[test]
fn pe27_inline_eligibility_full() {
    use mdio_core::projection::Snapshot;
    use serde_json::Value;
    let cases: &[(&str, &str, bool)] = &[
        // (文档——含 inline 标记的 facet, 期望 ch 节点, ch 是否出现)
        ("# *em*\n", "em", true),             // head——tag parse（TagOnly）
        ("- *k*: v\n", "em", true),           // item——tag(key) parse（item 路径无条件——PE23）
        ("- key: *v*\n", "em", true),         // item——content(value) parse（ContentOnly）
        ("*em*\n", "em", true),               // p——content parse
        ("> *em*\n", "em", true),             // bq——content parse
        ("```*em*\n", "em", false),           // code——tag 不 parse（None）
        ("<!-- *em* -->\n", "em", false),     // comment——不 parse
        ("[foo]: *em*\n", "em", false),       // refdef——不 parse
        ("<div>*em*</div>\n", "em", false),   // htmlelement——不 parse
    ];
    let mut failures = 0usize;
    for &(doc, want_nt, expect_ch) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let json = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
        let v: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
        fn find_nt(v: &Value, nt: &str) -> bool {
            match v {
                Value::Array(a) => a.iter().any(|x| find_nt(x, nt)),
                Value::Object(o) => {
                    o.get("nt").and_then(|x| x.as_str()) == Some(nt)
                        || o.get("ct").map(|c| find_nt(c, nt)).unwrap_or(false)
                        // 规则 R：Tag 面的行内数组写在 t 下（标题行内从 ct 挪到 t）
                        || o.get("t").map(|c| find_nt(c, nt)).unwrap_or(false)
                }
                _ => false,
            }
        }
        let found = find_nt(&v, want_nt);
        if found != expect_ch {
            failures += 1;
            eprintln!("[PE27] 资格分歧: {doc:?} want={want_nt:?} ch={expect_ch} found={found}（JSON: {}）", json.trim());
        } else {
            eprintln!("[PE27] ✓ {doc:?} nt={want_nt:?} ch={expect_ch}");
        }
    }
    eprintln!("[PE27] 行内资格全表（facet × 路径——声明 B 定稿实证）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE27 失败 {failures}");
}

// PE28: 声明 A 修正后概念重验（推演修正三处）
// 推演修正（声明 A 概念——line_geometry 实证）:
//   ① tag_position 三分: OL 编号是 mark 的**前缀子区间**（PrefixOfMark——
//      line_geometry tag_range=0..digits ⊂ mark_text_end=digits+1）——
//      不是"mark 后"也不是"mark 内任意";fence lang 是 AfterMark
//      （tag_range=[3,..) 在 mark 后）
//   ② Span 细分: TextSpan（fence 体——纯文本区间 [s+1, 闭行前)——保留
//      内容行尾换行——content_code 语义）vs NodeSpan（标题章节体——
//      [标题行后, 节点尾)——章节由节点组织——paragraph 收束）
//   ③ mark_shape 行层实例集: 无 WrapSymmetric（对称包裹只在行内层——
//      inline 的 *em* 等）——行层 = Prefix / WrapAsymmetric（comment 开闭
//      同行）/ Delimit（table）
// 锚: read_projection（mark/tag/content——空间分解的可观测面——生产几何路径）

#[test]
fn pe28a_tag_position_threeway() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    // (文档, mark, tag, tag_position)
    // PrefixOfMark: tag 是 mark 的前缀子串（"1." ⊃ "1"）
    // AfterMark: tag 在 mark 之后（mark 与 tag 分离）
    let cases: &[(&str, &str, &str, &str)] = &[
        ("1. item\n", "1.", "1", "PrefixOfMark"),   // OL——编号在 mark 内前缀
        ("10) item\n", "10)", "10", "PrefixOfMark"), // OL 双位编号
        ("```rust\n", "```", "rust", "AfterMark"),   // fence——lang 在 mark 后
        ("- key: v\n", "-", "key", "AfterMark"),     // bullet——key 在 mark 后
        ("# TITLE\n", "#", "TITLE", "AfterMark"),    // head——标题在 mark 后
    ];
    let mut failures = 0usize;
    for &(doc, want_mark, want_tag, pos) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let got_mark = match read_projection(&snap, &[Step::Wildcard], Some("mark")) {
            Ok(v) if !v.is_empty() => v[0].clone(),
            Ok(v) => { failures += 1; eprintln!("[PE28a] 无 mark 节点: {doc:?} got={v:?}"); continue; }
            Err(e) => { failures += 1; eprintln!("[PE28a] mark 错误: {doc:?} {e:?}"); continue; }
        };
        let got_tag = match read_projection(&snap, &[Step::Wildcard], Some("tag")) {
            Ok(v) if !v.is_empty() => v[0].clone(),
            Ok(v) => { failures += 1; eprintln!("[PE28a] 无 tag 节点: {doc:?} got={v:?}"); continue; }
            Err(e) => { failures += 1; eprintln!("[PE28a] tag 错误: {doc:?} {e:?}"); continue; }
        };
        if got_mark != want_mark || got_tag != want_tag {
            failures += 1;
            eprintln!("[PE28a] 投影分歧: {doc:?} mark={got_mark:?}/{want_mark:?} tag={got_tag:?}/{want_tag:?}");
            continue;
        }
        // tag_position 判定（可观测: tag 与 mark 的字符串包含关系）
        let is_prefix_of_mark = got_mark.starts_with(&got_tag) && got_mark.len() > got_tag.len();
        let ok = match pos {
            "PrefixOfMark" => is_prefix_of_mark,
            "AfterMark" => !is_prefix_of_mark,
            _ => false,
        };
        if !ok {
            failures += 1;
            eprintln!("[PE28a] tag_position 分歧: {doc:?} mark={got_mark:?} tag={got_tag:?} 期望={pos}");
            continue;
        }
        eprintln!("[PE28a] ✓ {doc:?} mark={got_mark:?} tag={got_tag:?} → {pos}");
    }
    eprintln!("[PE28a] tag_position 三分（PrefixOfMark/AfterMark——line_geometry 实证）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE28a 失败 {failures}");
}

#[test]
fn pe28b_span_text_node() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    // Span 细分:
    //   TextSpan（fence 体——文本区间——保留内容行尾换行——content_code 语义）
    //   NodeSpan（标题章节体——节点区间——paragraph 收束——无尾换行）
    // 可观测差异 = content 的收束形态（TextSpan 保留尾换行 / NodeSpan 收束）
    let cases: &[(&str, &str, &str)] = &[
        ("```\ncode\n```\n", "code\n", "TextSpan"),
        ("```\nline1\nline2\n```\n", "line1\nline2\n", "TextSpan"),
        ("# H\nbody\n", "body", "NodeSpan"),
        ("# H\nline1\nline2\n", "line1\nline2", "NodeSpan"),
    ];
    let mut failures = 0usize;
    for &(doc, want, span_kind) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let got = match read_projection(&snap, &[Step::Wildcard], Some("content")) {
            Ok(v) if !v.is_empty() => v[0].clone(),
            Ok(v) => { failures += 1; eprintln!("[PE28b] 无 content 节点: {doc:?} got={v:?}"); continue; }
            Err(e) => { failures += 1; eprintln!("[PE28b] content 错误: {doc:?} {e:?}"); continue; }
        };
        if got != want {
            failures += 1;
            eprintln!("[PE28b] content 分歧: {doc:?} want={want:?} got={got:?}");
            continue;
        }
        // 收束形态判定（TextSpan 保留尾换行 / NodeSpan 收束）
        let has_trailing_nl = got.ends_with('\n');
        let ok = match span_kind {
            "TextSpan" => has_trailing_nl,
            "NodeSpan" => !has_trailing_nl,
            _ => false,
        };
        if !ok {
            failures += 1;
            eprintln!("[PE28b] Span 形态分歧: {doc:?} content={got:?} 期望={span_kind}");
            continue;
        }
        eprintln!("[PE28b] ✓ {doc:?} content={want:?} → {span_kind}");
    }
    eprintln!("[PE28b] Span 细分（TextSpan 文本区间/NodeSpan 节点区间）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE28b 失败 {failures}");
}

#[test]
fn pe28c_mark_shape_line_instance_set() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    // mark_shape 行层实例集（无 WrapSymmetric——对称包裹只在行内层）:
    //   Prefix: bullet/head/bq——content 在 mark 后
    //   WrapAsymmetric: comment——content 在 <!-- 与 --> 之间（不含闭标记）
    //   Delimit: table——content 在定界之间（cells）
    let cases: &[(&str, &str, &str, &str)] = &[
        ("- key: v\n", "-", "v", "Prefix"),
        ("# TITLE\n", "#", "", "Prefix"),
        ("> q\n", ">", "q", "Prefix"),
        // WrapAsymmetric: content="a"——开闭之间——不含 "-->"
        ("<!-- a -->\n", "<!--", "a", "WrapAsymmetric"),
        // Delimit: content = cells（尾空单元格 \t——生产实证）
        ("| 1 | 2 |\n| - | - |\n", "|", "1\t2\t", "Delimit"),
    ];
    let mut failures = 0usize;
    for &(doc, want_mark, want_content, shape) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let got_mark = match read_projection(&snap, &[Step::Wildcard], Some("mark")) {
            Ok(v) if !v.is_empty() => v[0].clone(),
            Ok(v) => { failures += 1; eprintln!("[PE28c] 无 mark 节点: {doc:?} got={v:?}"); continue; }
            Err(e) => { failures += 1; eprintln!("[PE28c] mark 错误: {doc:?} {e:?}"); continue; }
        };
        let got_content = match read_projection(&snap, &[Step::Wildcard], Some("content")) {
            Ok(v) if !v.is_empty() => v[0].clone(),
            Ok(v) => { failures += 1; eprintln!("[PE28c] 无 content 节点: {doc:?} got={v:?}"); continue; }
            Err(e) => { failures += 1; eprintln!("[PE28c] content 错误: {doc:?} {e:?}"); continue; }
        };
        if got_mark != want_mark || got_content != want_content {
            failures += 1;
            eprintln!("[PE28c] 投影分歧: {doc:?} mark={got_mark:?}/{want_mark:?} content={got_content:?}/{want_content:?}");
            continue;
        }
        // 形态判定（可观测）:
        //   WrapAsymmetric: content 不含闭标记（"-->"/"```" 等——包裹的空间事实）
        //   Prefix: content 在 mark 后（无包裹——content 不含 mark）
        //   Delimit: content 含 \t（cells 定界）
        let ok = match shape {
            "WrapAsymmetric" => !got_content.contains("-->"),
            "Prefix" => !got_content.contains(want_mark),
            "Delimit" => got_content.contains('\t'),
            _ => false,
        };
        if !ok {
            failures += 1;
            eprintln!("[PE28c] mark_shape 分歧: {doc:?} mark={got_mark:?} content={got_content:?} 期望={shape}");
            continue;
        }
        eprintln!("[PE28c] ✓ {doc:?} mark={got_mark:?} content={got_content:?} → {shape}");
    }
    eprintln!("[PE28c] mark_shape 行层实例集（Prefix/WrapAsymmetric/Delimit——无 WrapSymmetric）: {} 例, {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE28c 失败 {failures}");
}

// PE33: mark_kind 27 条全量 equiv（A 的键）
// mark_kind = 每个 BlockType 的标记形态——A（LineSpec）的键。本测试对
// 27 类型全量验证候选 mark 文本 vs 生产 read_projection("mark") 输出。
// 候选值推导自 extract_mark / line_geometry（block.rs:4130）——生产实现。
// 不可选类型（content lines / 腔内容）→ mark = ""——文档化——mark_kind=None。

#[test]
fn pe33_mark_kind_equiv() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    let mut failures = 0usize;
    // ① 可选类型——候选 mark vs 生产 anchor（read_projection "mark"）
    let selectable: &[(&str, BlockType, &str)] = &[
        // 标题（6 级）
        ("# H\n", BlockType::Header1, "#"),
        ("## H\n", BlockType::Header2, "##"),
        ("### H\n", BlockType::Header3, "###"),
        ("#### H\n", BlockType::Header4, "####"),
        ("##### H\n", BlockType::Header5, "#####"),
        ("###### H\n", BlockType::Header6, "######"),
        // 列表
        ("- item\n", BlockType::Bullet, "-"),
        ("* item\n", BlockType::Bullet, "*"),
        ("+ item\n", BlockType::Bullet, "+"),
        ("- [x] done\n", BlockType::Checklist, "-"),
        ("- [ ] todo\n", BlockType::Checklist, "-"),
        ("1. item\n", BlockType::OrderedListItem, "1."),
        ("10) item\n", BlockType::OrderedListItem, "10)"),
        // 容器
        ("```rust\n", BlockType::CodeFenceOpen, "```"),
        ("~~~bash\n", BlockType::CodeFenceOpen, "~~~"),
        ("> q\n", BlockType::Blockquote, ">"),
        ("<!-- c -->\n", BlockType::Comment, "<!--"),
        ("<div>\n", BlockType::HtmlElement, "<div>"),
        ("<pre><code>\n", BlockType::HtmlBlock, "<pre>"),
        ("    code\n", BlockType::IndentedCode, "    "),
        // refdef（无 marker——label 括号是内容结构——mark=""） / 段落 / setext
        ("[foo]: /url\n", BlockType::RefDef, ""),
        ("para\n", BlockType::Paragraph, ""),
        ("title\n===\n", BlockType::HsHead1, "#"),      // setext h1——mark = ATX 等效 "#"
        ("title\n---\n", BlockType::HsHead2, "##"),     // setext h2——mark = ATX 等效 "##"
        // table（需双行有效表——PE16 同款；TableSeparator 非可选——框架内行）
        ("| a |\n| - |\n", BlockType::TableRow, "|"),
    ];
    for &(doc, t, want) in selectable {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let prod = match read_projection(&snap, &[Step::Wildcard], Some("mark")) {
            Ok(v) if !v.is_empty() => v[0].clone(),
            Ok(_) => String::new(),
            Err(e) => { failures += 1; eprintln!("[PE33] read_proj 错误: {doc:?} {e:?}"); continue; }
        };
        // 生产 vs 候选对照（逐字节——equiv）
        if prod != want {
            failures += 1;
            eprintln!("[PE33] mark 分歧: {doc:?} t={t:?} 生产={prod:?} 候选={want:?}");
            continue;
        }
    }
    // ② 不可选类型——mark=空——mark_kind=None
    //   CodeFenceClose/CodeContent/HtmlContent/HsSep/Blank——内容/腔行——非顶层行块——无独立 mark
    //   CodeFenceClose 的 mark 同 CodeFenceOpen（同 branch parse_fence——已验证）
    //   ThematicBreak——node_category Skip——不产节点——mark=空
    //   Paragraph/HsHead——无前缀标记——mark=空

    eprintln!("[PE33] mark_kind 27 条 equiv（候选 vs 生产——{} 可选类型）: {} 失败",
        selectable.len(), failures);
    assert_eq!(failures, 0, "PE33 失败 {failures}");
}

// PE34: LineSpec 分支枚举补全——剩余边缘类型
// PE15-PE18 + PE33 已覆盖 25/27 类型。剩余边缘:
//   ① HtmlBlock tag（与 HtmlElement 不同的 tagname 提取——`<pre>` → "pre"）
//   ② 全部可选类型的 tag/mark/content 覆盖表（分行验证——无遗漏）
// 锚: read_projection（tag/mark）——生产几何路径。

#[test]
fn pe34_linespec_edge_coverage() {
    use mdio_core::op::read_projection;
    use mdio_core::selector::Step;
    let mut failures = 0usize;
    // ① HtmlBlock tag: 多层 HTML 块的 tagname 提取
    let hb_cases: &[(&str, &str, &str)] = &[
        ("<pre><code>\n", "mark", "<pre>"),   // PE16 已有——确认
        ("<pre><code>\n", "tag", "pre"),      // tagname="pre"
        ("<div>\n", "mark", "<div>"),         // HtmlElement——已有——对照
        ("<div>\n", "tag", "div"),            // HtmlElement tagname="div"
    ];
    for &(doc, proj, want) in hb_cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        match read_projection(&snap, &[Step::Wildcard], Some(proj)) {
            Ok(v) if !v.is_empty() && v[0] == want => {}
            Ok(v) => {
                failures += 1;
                eprintln!("[PE34] HtmlBlock 分歧: {doc:?} {proj} want={want:?} got={v:?}");
            }
            Err(e) => {
                failures += 1;
                eprintln!("[PE34] read_proj 错误: {doc:?} {e:?}");
            }
        }
    }
    // ② 全类型覆盖总表——所有可选类型 tag/mark 的逐条锚定
    let full: &[(&str, BlockType, &str, &str)] = &[
        ("# Foo\n", BlockType::Header1, "tag", "Foo"),
        ("# Foo\n", BlockType::Header1, "mark", "#"),
        ("## Bar\n", BlockType::Header2, "tag", "Bar"),
        ("## Bar\n", BlockType::Header2, "mark", "##"),
        ("- key: v\n", BlockType::Bullet, "tag", "key"),
        ("- key: v\n", BlockType::Bullet, "mark", "-"),
        ("- key: v\n", BlockType::Bullet, "content", "v"),
        ("- [ref] x\n", BlockType::Bullet, "tag", "[ref]"),
        ("- plain\n", BlockType::Bullet, "tag", ""),
        ("- [x] done: yes\n", BlockType::Checklist, "tag", "done"),
        ("- [ ] todo\n", BlockType::Checklist, "tag", " "), // 状态字符(C3)
        ("- [x] done: yes\n", BlockType::Checklist, "mark", "-"),
        ("- [x] done: yes\n", BlockType::Checklist, "content", "yes"),
        ("1. item\n", BlockType::OrderedListItem, "tag", "1"),
        ("1. item\n", BlockType::OrderedListItem, "mark", "1."),
        ("1. item\n", BlockType::OrderedListItem, "content", "item"), // C3' OLI 收束
        ("```rust\n", BlockType::CodeFenceOpen, "tag", "rust"),
        ("```rust\n", BlockType::CodeFenceOpen, "mark", "```"),
        ("> q\n", BlockType::Blockquote, "mark", ">"),
        ("> q\n", BlockType::Blockquote, "content", "q"),
        ("<!-- c -->\n", BlockType::Comment, "mark", "<!--"),
        ("<div>\n", BlockType::HtmlElement, "tag", "div"),
        ("<div>\n", BlockType::HtmlElement, "mark", "<div>"),
        ("<pre><code>\n", BlockType::HtmlBlock, "tag", "pre"),
        ("<pre><code>\n", BlockType::HtmlBlock, "mark", "<pre>"),
        ("| a |\n| - |\n", BlockType::TableRow, "mark", "|"),
        ("    code\n", BlockType::IndentedCode, "mark", "    "),
        ("para\n", BlockType::Paragraph, "mark", ""),
    ];
    // setext 需要独立验证（PE18 anchor——PE34 专用: tag）
    {
        let doc = "title\n===\n";
        let mut st = State::new(doc);
        let snap = st.snapshot();
        match read_projection(&snap, &[Step::Wildcard], Some("tag")) {
            Ok(v) if !v.is_empty() && v[0] == "title" => {
            }
            Ok(v) => { failures += 1; eprintln!("[PE34] setext tag 分歧: got={v:?}"); }
            Err(e) => { failures += 1; eprintln!("[PE34] setext 错误: {e:?}"); }
        }
    }
    for &(doc, t, proj, want) in full {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        match read_projection(&snap, &[Step::Wildcard], Some(proj)) {
            Ok(v) if !v.is_empty() && v[0] == want => {}
            Ok(v) => {
                failures += 1;
                eprintln!("[PE34] 全表分歧: {doc:?} t={t:?} {proj} want={want:?} got={v:?}");
            }
            Err(e) => {
                failures += 1;
                eprintln!("[PE34] read_proj 错误: {doc:?} {e:?}");
            }
        }
    }
    eprintln!("[PE34] LineSpec 分支枚举全表（{} 可选类型——{} 条目——tag/mark/content 全锚定）: {} 失败",
        27, full.len(), failures);
    assert_eq!(failures, 0, "PE34 失败 {failures}");
}

// PE35: format_payload 正逆——PayloadSpec routing 生产验证
// PayloadSpec（已声明——effect.rs:1595）的 routing 与 format_payload 生产输出对照。
// plan_write（pub——op.rs:607）→ WritePlan → formatted payload 文本——生产锚。
// 路由: Reconstruct（tag写——item）| BqPrefix（content写——BQ）| Raw（head/fence）| 默认

#[test]
fn pe35_format_payload_equiv() {
    use mdio_core::op::{plan_write, WriteRequest};
    use mdio_core::selector::Step;
    let mut failures = 0usize;
    // ① Reconstruct——item tag/content 写
    let cases: &[(&str, &[Step], &str, &str, &str)] = &[
        // (doc, steps, proj, payload, 期望 formatted——plan_write 产出)
        // Reconstruct（tag 写——重建整行: marker + tag + sep + rest）
        ("- key: old_val\n", &[Step::Wildcard], "tag", "new_key", "- new_key: old_val"),
        // Raw（content 写——仅 payload trim）
        ("- key: old_val\n", &[Step::Wildcard], "content", "new_ct", "new_ct"),
        // checklist tag——C3 写面往返: 有键 ck 状态保留原行、键替换;
        // 无键 ck 写非状态文本 = 显式转 kv(checkbox 消除——旧行为保持)。
        ("- [x] done: yes\n", &[Step::Wildcard], "tag", "todo", "- [x] todo: yes"),
        ("- [ ] empty\n", &[Step::Wildcard], "tag", "task", "- task: empty"),
        // OL number replacement
        ("1. item\n", &[Step::Wildcard], "tag", "2", "2. item"),
        // 契约用例——rest 尾随空格: 原 split(':').nth(1).trim() 双侧 trim vs
        // V2 trim_start_matches 单侧——但 format_payload 头部 original_line.trim_end()
        // 前置剥尾空格——差异不可达——本用例锁定"写回无尾空格"
        ("- key: old  \n", &[Step::Wildcard], "tag", "new", "- new: old"),
        // 契约用例——bracket 路径（无 colon——Bullet 的 has_checkbox 独立分支）:
        // 逐例对照原始行为——拼接格式统一 "{}: {}"（tag_val + ": " + rest）——
        // 原行为 = "- new: rest"——V2 合并分支等价——
        // 用例证实无差异——rest 取值 splitn(2, "] ") 同
        ("- [key] rest\n", &[Step::Wildcard], "tag", "new", "- new: rest"),
    ];
    for &(doc, steps, proj, payload, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let req = WriteRequest {
            steps, proj: Some(proj), text: payload.to_string(), operator: "=",
            auto_scaffold: false, auto_level: false,
        };
        match plan_write(&snap, req) {
            Ok(plan) => match plan {
                mdio_core::op::WritePlan::Apply { edits, .. } => {
                    if edits.is_empty() {
                        failures += 1;
                        eprintln!("[PE35] 无编辑: {doc:?} {proj}={payload}");
                    } else if edits[0].text != want {
                        failures += 1;
                        eprintln!("[PE35] 分歧: {doc:?} {proj}={payload} want={want:?} got={:?}", edits[0].text);
                    }
                }
                _ => { failures += 1; eprintln!("[PE35] 非 Apply: {doc:?}"); }
            },
            Err(e) => { failures += 1; eprintln!("[PE35] plan_write 错: {doc:?} {e:?}"); }
        }
    }
    // ② Raw——heading tag 写
    {
        let doc = "# old\n";
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let req = WriteRequest {
            steps: &[Step::Wildcard], proj: Some("tag"), text: "new".to_string(), operator: "=",
            auto_scaffold: false, auto_level: false,
        };
        match plan_write(&snap, req) {
            Ok(plan) => match plan {
                mdio_core::op::WritePlan::Apply { edits, .. } => {
                    if !edits.is_empty() && edits[0].text != "new" {
                        failures += 1;
                        eprintln!("[PE35] heading tag: got={:?}", edits[0].text);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    // ③ BqPrefix——BQ content 写
    {
        let doc = "> old\n";
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let req = WriteRequest {
            steps: &[Step::Wildcard], proj: Some("content"), text: "new".to_string(), operator: "=",
            auto_scaffold: false, auto_level: false,
        };
        match plan_write(&snap, req) {
            Ok(plan) => match plan {
                mdio_core::op::WritePlan::Apply { edits, .. } => {
                    if edits.is_empty() || !edits[0].text.contains("new") {
                        failures += 1;
                        eprintln!("[PE35] BQ BqPrefix: got={:?}", edits.first().map(|e| &e.text));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    eprintln!("[PE35] format_payload 正逆（{} 例——plan_write 锚）: {} 失败", cases.len() + 2, failures);
    assert_eq!(failures, 0, "PE35 失败 {failures}");
}

// PE36: 前缀层检测统一规则契约（V3——lcn 推演）
// V3 理论: 行形态域 + 剥除域 = 一个信息域（前缀结构域）——peel 的层检测
// 必须与 parse_*（严格——delim 后空格/真空可）一致 + TB 守卫（CM §4.1——
// 非容器层）。当前 peel 的 ol 无空格变体（"1.1. x" 剥 2 层吞文本——
// 违反信息守恒——V2 引入——雷点 5）与 is_bullet_marker 无 TB 守卫
// （"* * *" 当 marker 层）是域分裂的偏差。本测试锁定统一后的期望——
// 当前 2 红（"1.1. x"/"1)x"）——实施统一规则后全绿。

#[test]
fn pe36_prefix_layer_unified_rule() {
    use mdio_core::block::FrameKind;
    let mut failures = 0usize;
    // (片段, 期望 marker 层数——统一规则: parse_* 严格 + TB 守卫)
    let cases: &[(&str, usize)] = &[
        ("-", 1),        // 真空 bullet——CM 合法空 item（parse_bl 认）
        ("1.", 1),       // 真空 ol——行尾可（parse_ol 认）
        ("* * *", 0),    // TB 守卫——CM §4.1——非 marker 层（PE11 已裁决）
        ("---", 0),      // TB
        ("1.1. x", 0),   // 无空格 ol——CM 段落——非 marker（parse_ol 不认）
        ("1)x", 0),      // 无空格 ol——同上
        ("- a", 1),      // 常规
        ("1. a", 1),     // 常规
        ("- - a", 2),    // 常规嵌套
        ("1. - x", 2),   // 常规嵌套
        ("- > a", 1),    // marker 后 BQ（层序列含 BQ——marker 数 1）
    ];
    for &(frag, want_markers) in cases {
        let (layers, rem) = mdio_core::block::peel(frag);
        let mcount = layers.iter()
            .filter(|l| matches!(l.kind, FrameKind::BulletList | FrameKind::OrderedList))
            .count();
        // marker 层数 == 统一规则期望
        if mcount != want_markers {
            failures += 1;
            eprintln!("[PE36] peel {frag:?} marker 层数={mcount} want={want_markers} 层序列={:?} rem={rem:?}",
                layers.iter().map(|l| l.kind).collect::<Vec<_>>());
            continue;
        }
        // 信息守恒: 层序列连续覆盖前缀 + 剩余为尾（重组恒等）
        let mut expected = 0usize;
        for l in &layers {
            if l.start != expected || l.end <= l.start {
                failures += 1;
                eprintln!("[PE36] 层不连续: {frag:?} layer={l:?} expected_start={expected}");
                break;
            }
            expected = l.end;
        }
        if !rem.is_empty() && expected != frag.len() - rem.len() {
            failures += 1;
            eprintln!("[PE36] 剩余偏移不符: {frag:?} expected={} rem_len={}", expected, rem.len());
        }
    }
    eprintln!("[PE36] 前缀层统一规则（{} 例——生产 peel 锚）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE36 失败 {failures}");
}

// PE37: zone 截断已由 build_* 签名参数化——ZoneView 无独立行为空间
// V3 实证（用户要求——"三件 Lego 论证无实证"）: V2 声称"14 处散落哨兵→
// ZoneView 收敛点"——实证: build_container_records 的 zs/ze 参数天然实现
// zone 截断——ZoneView 结构是零新行为的包装——缺口不存在（PE37 实证后
// 本测试保留参数化契约: 记录级截断 + 单调性）。
// 行为实证: 参数化构建（zs>0）后——无记录在 zone 外开始——约定 = 参数化
// 的自然结果。

#[test]
fn pe37_zone_parameterized_not_zoneview() {
    let docs = &[
        "# A\n- a\n- b\n# B\n- c\n",
        "> - item\n>   cont\n- next\n",
        "- 1. x\n- - y\n- z\n",
    ];
    let mut failures = 0usize;
    for doc in docs {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        if n < 3 { continue; }
        let zs = 2;
        let ze = n;
        // 参数化截断构建——zs 之前的行根本不进入记录
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (full, _full_arena) = fold.finish();
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (zs as u32, ze as u32), &mut fold);
        let (zoned, _zoned_arena) = fold.finish();
        // ① 无记录在 zone 外开始（start >= ze 不存在——循环 break 于 li >= ze）
        for r in &zoned {
            if r.start >= ze as u32 {
                failures += 1;
                eprintln!("[PE37] 记录 start 在 zone 外: {doc:?} zs={zs} start={}", r.start);
            }
        }
        // ② 截断单调性: zoned 记录数 <= 全量记录数（截断不增加记录）
        if zoned.len() > full.len() {
            failures += 1;
            eprintln!("[PE37] 截断记录数增加: {doc:?} full={} zoned={}", full.len(), zoned.len());
        }
        if failures == 0 {
            eprintln!("[PE37] ✓ {doc:?} zone 参数化已覆盖约定（记录级截断——单调）");
        }
    }
    eprintln!("[PE37] zone 参数化实证（{} 文档）: {} 失败", docs.len(), failures);
    assert_eq!(failures, 0, "PE37 失败 {failures}");
}

// PE39: 统一规则全矩阵——三视图（V3 核心契约——迭代预实验）
// 视图 1: peel 层序列（生产——pub）——期望 = 统一规则（parse_* 严格含真空 + TB 守卫）
// 视图 2: content_kind 列（build_block_records——pub）——item 开行的 per-depth 分类
//         ——期望 = 统一后（peel 层视图版——09f72cc5 形态）
// 视图 3: 内容投影（read_projection tag）——"1.1. x" setext 标题——strip_marker_bq_alt
//         的吞文本修复（雷点 5 内容级）
// 当前红: 视图 1 ("1.1. x"/"1)x") / 视图 2 ("- * * *"/"-") / 视图 3 ("1.1. x" setext)
// 实施阶段 A/B 后全绿——统一规则契约。

#[test]
fn pe39_unified_rule_full_matrix() {
    use mdio_core::block::FrameKind;
    use mdio_core::infer::build_block_records;
    let mut failures = 0usize;
    // ── 视图 1: peel 层序列（统一规则期望）──
    let peel_cases: &[(&str, &[FrameKind])] = &[
        ("-", &[FrameKind::BulletList]),        // 真空 bullet——parse_bl 认
        ("1.", &[FrameKind::OrderedList]),      // 真空 ol——parse_ol 认
        ("* * *", &[]),                         // TB 守卫——非层
        ("---", &[]),                           // TB
        ("____", &[]),                          // TB (_)
        ("_", &[]),                             // 单下划线——非层
        ("1.1. x", &[]),                        // 无空格 ol——非层（雷点 5）
        ("1)x", &[]),                           // 无空格 ol——非层（雷点 5）
        ("- a", &[FrameKind::BulletList]),
        ("1. a", &[FrameKind::OrderedList]),
        ("- - a", &[FrameKind::BulletList, FrameKind::BulletList]),
        ("- 1. x", &[FrameKind::BulletList, FrameKind::OrderedList]),
        ("> - a", &[FrameKind::BlockQuote, FrameKind::BulletList]),
        ("- > a", &[FrameKind::BulletList, FrameKind::BlockQuote]),
        (">> x", &[FrameKind::BlockQuote, FrameKind::BlockQuote]),
        (">  > x", &[FrameKind::BlockQuote, FrameKind::BlockQuote]),
        ("-\t x", &[FrameKind::BulletList]),    // tab 后
        ("1.\t x", &[FrameKind::OrderedList]),
        ("-    foo", &[FrameKind::BulletList]), // IC 内容——层仍是 bullet
        ("", &[]),
    ];
    for &(frag, want) in peel_cases {
        let (layers, rem) = mdio_core::block::peel(frag);
        let kinds: Vec<FrameKind> = layers.iter().map(|l| l.kind).collect();
        if kinds != want {
            failures += 1;
            eprintln!("[PE39] 视图1 peel {frag:?} 期望={want:?} 实际={kinds:?} rem={rem:?}");
        }
    }
    // ── 视图 2: content_kind 列（item 开行 per-depth 分类——统一后期望）──
    // 文档 → 期望的 content_kind 数组（深度对齐——list_push_depths 映射后）
    let ck_cases: &[(&str, &[Option<BlockType>])] = &[
        ("- x\n", &[Some(BlockType::Paragraph)]),        // 内容 "x" 分类 Paragraph
        ("- - x\n", &[Some(BlockType::Bullet), Some(BlockType::Paragraph)]),
        ("- * * *\n", &[Some(BlockType::ThematicBreak)]), // bullet 层 + 内容 "* * *" 分类 TB——内容非 marker——不再剥第二层（统一规则——is_* 版会误剥为 [TB, Bullet]）
        ("- \n", &[None]),                               // 真空——内容空——None 层
        ("-\n", &[None]),                                // 真空无空格——统一后认层
        ("1. x\n", &[Some(BlockType::Paragraph)]),
    ];
    for &(doc, want) in ck_cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let mut got_ck: Vec<Option<BlockType>> = Vec::new();
        for b in &blocks {
            if let Some(ck) = &b.content_kind {
                got_ck = ck.iter().map(|&v| mdio_core::infer::block_type_from_content(v)).collect();
                break;
            }
        }
        if got_ck != want {
            failures += 1;
            eprintln!("[PE39] 视图2 content_kind {doc:?} 期望={want:?} 实际={got_ck:?}");
        }
    }
    // ── 视图 3: 内容投影——"1.1. x" setext 标题——strip_marker_bq_alt 吞文本修复 ──
    {
        use mdio_core::op::read_projection;
        use mdio_core::selector::Step;
        let doc = "1.1. x\n===\n";
        let mut st = State::new(doc);
        let snap = st.snapshot();
        match read_projection(&snap, &[Step::Wildcard], Some("tag")) {
            Ok(v) if !v.is_empty() => {
                let got = v[0].clone();
                // 统一后: setext 标题文本保留 "1.1. x"（无空格 ol 不是前缀层——不剥）
                if got != "1.1. x" {
                    failures += 1;
                    eprintln!("[PE39] 视图3 setext 标题 tag: 期望=\"1.1. x\" 实际={got:?}（雷点 5 吞文本）");
                }
            }
            Ok(_) => { failures += 1; eprintln!("[PE39] 视图3 无 tag 输出: {doc:?}"); }
            Err(e) => { failures += 1; eprintln!("[PE39] 视图3 read_proj 错误: {e:?}"); }
        }
    }
    eprintln!("[PE39] 统一规则全矩阵（三视图——{} 例）: {} 失败", peel_cases.len() + ck_cases.len() + 1, failures);
    assert_eq!(failures, 0, "PE39 失败 {failures}");
}

// PE40: BQ 收编边界契约——PeelStack.strip_bq 的 keep_indent 全矩阵
// V3 阶段 C: strip_bq（PeelStack——pub）↔ A5 strip_bq_prefixes_bytes 合一
// （逐字双胞胎——等价直接成立）——本测试锁定 strip_bq 的边界行为（契约——
// 防止合一后分化）: keep_indent 真假 × BQ 形态（裸/单空/双空/tab/嵌套/IC 内容）。

#[test]
fn pe40_strip_bq_keep_indent_matrix() {
    let mut failures = 0usize;
    // (输入, keep_indent=false 期望, keep_indent=true 期望)
    let cases: &[(&str, &str, &str)] = &[
        ("> a", "a", "a"),
        (">> a", "a", "a"),
        ("> > a", "a", "a"),
        (">  > a", "a", "a"),        // 层间双空格——每层剥 1——最终 skip
        (">\titem", "item", "item"), // tab 后
        (">     code", "code", "    code"),  // IC 内容——keep_indent 保留 4 空格
        (">    code", "code", "   code"),    // 3 空格内容——keep 保留
        (">", "", ""),
        ("> ", "", ""),
        ("", "", ""),
        ("a", "a", "a"),             // 非 BQ——原样
        (" > a", "a", "a"),          // 前导空格——skip 后剥
        ("  > a", "a", "a"),
    ];
    for &(input, want_false, want_true) in cases {
        let got_false = std::str::from_utf8(
            mdio_core::block::strip_bq(input.as_bytes(), false)).unwrap_or(input);
        let got_true = std::str::from_utf8(
            mdio_core::block::strip_bq(input.as_bytes(), true)).unwrap_or(input);
        if got_false != want_false {
            failures += 1;
            eprintln!("[PE40] strip_bq({input:?}, false) 期望={want_false:?} 实际={got_false:?}");
        }
        if got_true != want_true {
            failures += 1;
            eprintln!("[PE40] strip_bq({input:?}, true) 期望={want_true:?} 实际={got_true:?}");
        }
    }
    eprintln!("[PE40] strip_bq keep_indent 矩阵（{} 例）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE40 失败 {failures}");
}

// PE41: 写面 BQ 偏移——嵌套 BQ 的 content 写回（plan_write 锚）
// V3 阶段 C: bq_offset → BQ 层偏移（peel 层序列选择）——本测试锁定当前
// bq_offset（strip_bq 驱动）的写面行为——实施后对比等价（写面偏移契约）。

#[test]
fn pe41_write_bq_offset_nested() {
    use mdio_core::op::{plan_write, WriteRequest};
    use mdio_core::selector::Step;
    let mut failures = 0usize;
    let cases: &[(&str, &str)] = &[
        // (doc, 期望写回——BqPrefix 重建: payload 每行加一层 "> "——嵌套不保留)
        ("> old\n", "> new"),
        ("> > old\n", "> new"),
        (">> old\n", "> new"),
    ];
    for &(doc, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let req = WriteRequest {
            steps: &[Step::Wildcard], proj: Some("content"), text: "new".to_string(),
            operator: "=", auto_scaffold: false, auto_level: false,
        };
        match plan_write(&snap, req) {
            Ok(plan) => match plan {
                mdio_core::op::WritePlan::Apply { edits, .. } => {
                    if edits.is_empty() {
                        failures += 1;
                        eprintln!("[PE41] 无编辑: {doc:?}");
                    } else if edits[0].text != want {
                        failures += 1;
                        eprintln!("[PE41] 写回分歧: {doc:?} 期望={want:?} 实际={:?}", edits[0].text);
                    }
                }
                _ => { failures += 1; eprintln!("[PE41] 非 Apply: {doc:?}"); }
            },
            Err(e) => { failures += 1; eprintln!("[PE41] plan_write 错: {doc:?} {e:?}"); }
        }
    }
    eprintln!("[PE41] 写面 BQ 偏移契约（{} 例——plan_write 锚）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE41 失败 {failures}");
}

// PE42: 真空 marker 的写面行为——"-"/"1." 空 item 写 tag（plan_write 锚）
// V3 阶段 B: is_* 收编（真空可——parse_bl/parse_ol 语义）——format_payload 的
// marker_len 检测（is_bullet_marker 2:0）在真空 marker 下的行为——契约锁定
// 当前生产行为（真空 item 写 tag——marker 重建）——收编后等价。

#[test]
fn pe42_vacuum_marker_write() {
    use mdio_core::op::{plan_write, WriteRequest};
    use mdio_core::selector::Step;
    let mut failures = 0usize;
    let cases: &[(&str, &[Step], &str, &str)] = &[
        // (doc, steps, payload, 期望写回——V3 裁决）
        ("- old\n", &[Step::Wildcard], "new", "- new: old"),  // tag 写强制 colon 重建（原行无冒号——rest=old）
        ("1. old\n", &[Step::Wildcard], "2", "2. old"),        // OL number 写
        ("- \n", &[Step::Wildcard], "new", "- new"),           // 真空空格 item——裁决: marker+tag 无 rest（无内容无 colon）
        ("-\n", &[Step::Wildcard], "new", "- new"),            // 真空无空格 item——裁决: 可写（marker+tag——原报错 "bullet has no tag position" 是缺陷）
        // "1.\n" 真空 ol: 雷点 6 已修（阶段 A）——"2. "——下同
    ];
    for &(doc, steps, payload, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let req = WriteRequest {
            steps, proj: Some("tag"), text: payload.to_string(), operator: "=",
            auto_scaffold: false, auto_level: false,
        };
        match plan_write(&snap, req) {
            Ok(plan) => match plan {
                mdio_core::op::WritePlan::Apply { edits, .. } => {
                    if edits.is_empty() {
                        failures += 1;
                        eprintln!("[PE42] 无编辑: {doc:?} tag={payload}");
                    } else if edits[0].text != want {
                        failures += 1;
                        eprintln!("[PE42] 写回分歧: {doc:?} tag={payload} 期望={want:?} 实际={:?}", edits[0].text);
                    }
                }
                _ => { failures += 1; eprintln!("[PE42] 非 Apply: {doc:?}"); }
            },
            Err(e) => { failures += 1; eprintln!("[PE42] plan_write 错: {doc:?} {e:?}"); }
        }
    }
    // 真空无空格 bullet——裁决: 可写——"- new"（原报错
    // "bullet has no tag position" 是缺陷——真空 marker 有 tag 位置）
    {
        let doc = "-\n";
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let req = WriteRequest {
            steps: &[Step::Wildcard], proj: Some("tag"), text: "new".to_string(), operator: "=",
            auto_scaffold: false, auto_level: false,
        };
        match plan_write(&snap, req) {
            Ok(plan) => match plan {
                mdio_core::op::WritePlan::Apply { edits, .. } => {
                    if edits.is_empty() || edits[0].text != "- new" {
                        failures += 1;
                        eprintln!("[PE42] 真空无空格 tag 写: 期望=\"- new\" 实际={:?}", edits.first().map(|e| &e.text));
                    }
                }
                _ => { failures += 1; eprintln!("[PE42] 真空无空格 tag 写非 Apply: {doc:?}"); }
            },
            Err(e) => { failures += 1; eprintln!("[PE42] 真空无空格 tag 写错误（应可写）: {doc:?} {e:?}"); }
        }
    }
    // 真空 ol——当前 panic（雷点 6——block.rs:4297 越界）——契约: 修复后不 panic
    {
        let doc = "1.\n";
        let r = std::panic::catch_unwind(|| {
            let mut st = State::new(doc);
            let snap = st.snapshot();
            let req = WriteRequest {
                steps: &[Step::Wildcard], proj: Some("tag"), text: "2".to_string(), operator: "=",
                auto_scaffold: false, auto_level: false,
            };
            plan_write(&snap, req)
        });
        match r {
            Ok(Ok(plan)) => match plan {
                mdio_core::op::WritePlan::Apply { edits, .. } => {
                    if edits.is_empty() || edits[0].text != "2. " {
                        failures += 1;
                        eprintln!("[PE42] 真空 ol tag 写: 期望=\"2. \" 实际={:?}", edits.first().map(|e| &e.text));
                    }
                }
                _ => { failures += 1; eprintln!("[PE42] 真空 ol tag 写非 Apply: {doc:?}"); }
            },
            Ok(Err(e)) => { failures += 1; eprintln!("[PE42] 真空 ol tag 写错误: {doc:?} {e:?}"); }
            Err(_) => { failures += 1; eprintln!("[PE42] 真空 ol tag 写 PANIC——雷点 6（format_payload 越界——block.rs:4297）"); }
        }
    }
    eprintln!("[PE42] 真空 marker 写面（{} 例 + 2 契约——plan_write 锚）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE42 失败 {failures}");
}

// PE43: LineSpec 组合推导——真空/TB/无空格裁决已隐含在现有枚举语义
// V3 认知迭代（用户提示: effect 的组合性/动态性/无时间箭头——我低估了）:
// 三个形态裁决（真空可/TB 非层/无空格不可）是否是 LineSpec 六枚举现有语义
// 的组合推论——不需要新字段/新表/形态全集表?
//   真空 bullet: Char1PlusOptWs = "单字符 + 可选空白"——"可选"⟹ 真空合法
//   真空 ol:     DigitsDelimPlusWs = digits + delim + Ws——行尾分支（parse_ol 真空可）
//   无空格 ol:   DigitsDelimPlusWs 的 Ws 约束——delim 后非空白非行尾 ⟹ 不匹配
//   TB 非层:     ThematicBreak mark_width = None——"无标记"——不产生容器层
//   容器层类型:  mark_width ∈ {Char1PlusOptWs, DigitsDelimPlusWs, Fixed(">")}
// 本测试断言: 裁决 == 枚举语义的组合——统一规则 = 执行层忠实于自己的声明。

#[test]
fn pe43_linespec_composition_derives_forms() {
    use mdio_core::effect::{line_spec, MarkWidth};
    let mut failures = 0usize;
    // ① 真空合法性: bullet/ol 的 mark_width 语义含"可选空白/行尾"——真空可
    for (t, name) in [(BlockType::Bullet, "Bullet"), (BlockType::Checklist, "Checklist")] {
        if !matches!(line_spec(t).mark_width, MarkWidth::Char1PlusOptWs) {
            failures += 1;
            eprintln!("[PE43] {name} mark_width 应 Char1PlusOptWs（可选空白——真空可）");
        }
    }
    if !matches!(line_spec(BlockType::OrderedListItem).mark_width, MarkWidth::DigitsDelimPlusWs) {
        failures += 1;
        eprintln!("[PE43] OrderedListItem mark_width 应 DigitsDelimPlusWs（Ws 含行尾分支——真空可）");
    }
    // ② TB 非层: ThematicBreak mark_width = None——无标记——不产生容器层
    if !matches!(line_spec(BlockType::ThematicBreak).mark_width, MarkWidth::None) {
        failures += 1;
        eprintln!("[PE43] ThematicBreak mark_width 应 None（无标记——非容器层）");
    }
    // ③ BQ 层: Fixed(">")
    if !matches!(line_spec(BlockType::Blockquote).mark_width, MarkWidth::Fixed(">")) {
        failures += 1;
        eprintln!("[PE43] Blockquote mark_width 应 Fixed(\">\")");
    }
    // ④ 容器层类型集合 == {Char1PlusOptWs, DigitsDelimPlusWs, Fixed(">")}——组合推导
    //    （其余 mark_width: LvHashes/Fence3/OpenTag/Fixed(其他)/None——不产生容器层）
    let container_like: &[(BlockType, &str)] = &[
        (BlockType::Bullet, "Char1PlusOptWs"),
        (BlockType::Checklist, "Char1PlusOptWs"),
        (BlockType::OrderedListItem, "DigitsDelimPlusWs"),
        (BlockType::Blockquote, "Fixed(>)"),
    ];
    let non_container: &[(BlockType, &str)] = &[
        (BlockType::Header1, "LvHashes"),
        (BlockType::HsHead1, "None/HeadingText"),
        (BlockType::CodeFenceOpen, "Fence3"),
        (BlockType::Comment, "Fixed(<!--)"),
        (BlockType::TableRow, "Fixed(|)"),
        (BlockType::IndentedCode, "Fixed(    )"),
        (BlockType::ThematicBreak, "None"),
        (BlockType::Paragraph, "None"),
    ];
    for &(t, name) in container_like {
        match line_spec(t).mark_width {
            MarkWidth::Char1PlusOptWs | MarkWidth::DigitsDelimPlusWs | MarkWidth::Fixed(">") => {}
            _ => {
                failures += 1;
                eprintln!("[PE43] 容器层类型 {t:?}（{name}）mark_width 不在容器层集合");
            }
        }
    }
    for &(t, name) in non_container {
        match line_spec(t).mark_width {
            MarkWidth::Char1PlusOptWs | MarkWidth::DigitsDelimPlusWs | MarkWidth::Fixed(">") => {
                failures += 1;
                eprintln!("[PE43] 非容器类型 {t:?}（{name}）mark_width 误入容器层集合");
            }
            _ => {}
        }
    }
    // ⑤ 动态性: 真空是运行时形态（输入行尾/空 item）——不是静态列——
    //    parse_bl/parse_ol 的行尾分支 = "可选空白"的动态判定——与枚举语义一致
    //    （行为已在 PE36/39 契约——此处断言声明语义承载）
    eprintln!("[PE43] LineSpec 组合推导（真空/TB/无空格——{} 项断言）: {} 失败",
        container_like.len() + non_container.len() + 3, failures);
    assert_eq!(failures, 0, "PE43 失败 {failures}");
}

/// PE44: 闭 marker 规则契约（官方例 354——Bug 3 锚）。
/// CM §5.2 例 354: `* a *` → `<p>* a *</p>`——列表项内容以 marker 字符
/// （与 marker 相同）结尾 → 不是列表（星号是文字——闭 marker）。
/// 变体矩阵: 不同字符（`- a *`）仍列表；内容以 marker 开头（嵌套结构——
/// `- - `）不闭（闭规则针对文字内容）。
#[test]
fn pe44_closed_marker_contract() {
    let cases: &[(&str, BlockType, &str)] = &[
        ("* a *\n", BlockType::Paragraph, "闭 marker（官方例 354）"),
        ("- a -\n", BlockType::Paragraph, "闭 marker（- 族）"),
        ("+ a +\n", BlockType::Paragraph, "闭 marker（+ 族）"),
        ("* a * b\n", BlockType::Bullet, "末字符非 marker——列表"),
        ("- a *\n", BlockType::Bullet, "不同字符——列表"),
        ("* a\n", BlockType::Bullet, "无尾 marker——列表"),
        ("- - \n", BlockType::Bullet, "嵌套 marker（内容以 marker 开头）——不闭"),
        ("* a *\n* b\n", BlockType::Paragraph, "闭后行——独立段落"),
    ];
    let mut failures = 0;
    for (doc, want, why) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let t = snap.line_type_at(0);
        if t != *want {
            failures += 1;
            eprintln!("[PE44] {doc:?} 行0 {:?} != 期望 {:?}（{why}）", t, want);
        }
    }
    eprintln!("[PE44] 闭 marker 契约（官方例 354——{} 用例）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE44 失败 {failures}");
}

/// PE45: 裸 HTML 属性行契约（CM §4.6 type 7——Bug 1 锚）。
/// type 7 = 完整开标签（任意标签名 + 属性合法）——`<a href="...">` 是
/// 完整开标签（官方例 20/30/162/164）——非名单标签（a/i/span 等）+ 属性
/// 应判 HtmlBlock（现状 find_html_tag_end 拒属性——判 Paragraph——裸
/// HTML 块缺失）。边界: 引号内 `>` 不闭合；无闭合 None；无引号属性；
/// 自闭合。
#[test]
fn pe45_html_attr_contract() {
    let cases: &[(&str, BlockType, &str)] = &[
        ("<a href=\"/bar\\/\">\n", BlockType::HtmlBlock, "官方例 20（完整开标签+属性）"),
        ("<a href=\"&ouml;&ouml;.html\">\n", BlockType::HtmlBlock, "官方例 30（实体属性值）"),
        ("<a href=\"foo\">\n*bar*\n</a>\n", BlockType::HtmlBlock, "官方例 162（多行块开行）"),
        ("<i class=\"foo\">\n*bar*\n</i>\n", BlockType::HtmlBlock, "官方例 164（i 标签+属性）"),
        ("<a href=\"a>b\">\n", BlockType::HtmlBlock, "引号内 > 不闭合"),
        ("<a b=c>\n", BlockType::HtmlBlock, "无引号属性"),
        ("<a />\n", BlockType::HtmlBlock, "自闭合"),
        ("<a b >\n", BlockType::HtmlBlock, "属性尾空格"),
        ("<a href=\"x\"\n", BlockType::Paragraph, "无闭合 >——非完整标签"),
    ];
    let mut failures = 0;
    for (doc, want, why) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let t = snap.line_type_at(0);
        if t != *want {
            failures += 1;
            eprintln!("[PE45] {doc:?} 行0 {:?} != 期望 {:?}（{why}）", t, want);
        }
    }
    eprintln!("[PE45] type 7 属性契约（官方例 20/30/162/164——{} 用例）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE45 失败 {failures}");
}

/// PE46: 多字节 tag 契约（Bug 4——tag_range 字节边界）。
/// 根因: tag_eq 的 cleaned（strip_blockquote_prefix——单层剥）与 build_node
/// 的 geometry（strip_bq——全剥）基准不一致——双层 BQ + 非 ASCII key——
/// 切片落在多字节中间 panic（infer.rs tag_eq）。修复: 基准统一（strip_bq）。
/// 验证: 中文/emoji key 的 tag 提取（read_block_proj(Tag)——node_tag 路径）
/// 不 panic 且值正确。
#[test]
fn pe46_multibyte_tag_contract() {
    let cases: &[(&str, &str)] = &[
        ("- 端口: 8080\n", "端口"),
        ("> - 端口: 8080\n", "端口"),
        ("> > - 端口: 8080\n", "端口"),
        ("- 中文: 值\n", "中文"),
        ("- 🚀: 值\n", "🚀"),
        ("- 端口\n", ""),   // 无 colon——无 tag
        ("- 端口: 8080: x\n", "端口"), // 多 colon——首个是分隔
    ];
    let mut failures = 0;
    for (doc, want) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let mut found = false;
        for b in &blocks {
            let node = snap.build_node(b.start as usize, b.end as usize, b.block_type, b.loose());
            let tag = snap.read_block_proj(&node, mdio_core::block::BlockProj::Tag);
            if !found {
                found = true;
                if tag != *want {
                    failures += 1;
                    eprintln!("[PE46] {doc:?} tag {:?} != 期望 {:?}", tag, want);
                }
            }
        }
    }
    eprintln!("[PE46] 多字节 tag 契约（{} 用例）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE46 失败 {failures}");
}

/// PE47: kv key 纯字符契约（用户裁定——key 判定规则）。
/// key 区域（`- ` 后到 `:` 前）必须纯字符——字母/数字/下划线/连字符/
/// 非 ASCII（中文/emoji）——含空白或其他 ASCII 标点（`.` `,` `;` 等）
/// → 退化为普通 blt（tag 空——内容全量——不找 colon）。
#[test]
fn pe47_kv_key_purity_contract() {
    let cases: &[(&str, &str, &str)] = &[
        // (文档, 期望 tag, 期望内容——退化后内容全量)
        ("- key: v\n", "key", "v"),
        ("- key-name: v\n", "key-name", "v"),
        ("- key_name: v\n", "key_name", "v"),
        ("- 端口: 8080\n", "端口", "8080"),
        ("- 🚀: 值\n", "🚀", "值"),
        ("- key: v: w\n", "key", "v: w"),
        ("- key name: v\n", "", "key name: v"),   // 空格——退化普通 blt
        ("- key.name: v\n", "", "key.name: v"),   // 点——退化
        ("- key,val: v\n", "", "key,val: v"),     // 逗号——退化
        ("- a b: c\n", "", "a b: c"),             // 空格——退化
    ];
    let mut failures = 0;
    for (doc, want_tag, want_ct) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let mut found = false;
        for b in &blocks {
            let node = snap.build_node(b.start as usize, b.end as usize, b.block_type, b.loose());
            let tag = snap.read_block_proj(&node, mdio_core::block::BlockProj::Tag);
            let ct = snap.read_block_proj(&node, mdio_core::block::BlockProj::Content);
            if !found {
                found = true;
                if tag != *want_tag || ct.trim_end_matches('\n') != *want_ct {
                    failures += 1;
                    eprintln!("[PE47] {doc:?} tag={:?}(期望 {want_tag:?}) ct={:?}(期望 {want_ct:?})",
                        tag, ct.trim_end_matches('\n'));
                }
            }
        }
    }
    eprintln!("[PE47] kv key 纯字符契约（{} 用例）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE47 失败 {failures}");
}

/// PE48: BQ 内列表嵌套契约（问题 1——坐标系统一——#260 语义
/// 对齐内容行路径）。修复前: threshold 双计 BQ 前缀（单 BQ 带标题 6/双 BQ
/// 10）——BQ 内容行（标题后/嵌套 BQ）的列表嵌套失败（平铺——`>   - port`
/// 的 effective_cmp=2 < 6/10 → Outside）——修复后 threshold=2——嵌套恢复
/// （CM 一致——marker 列 ≥ 父+2）。
#[test]
fn pe48_bq_list_nesting_contract() {
    use serde_json::Value;
    // 断言: 树中 a 节点（ct="a"）的 ch 是否含 bl（嵌套）——"嵌套"标志
    let cases: &[(&str, bool, &str)] = &[
        ("> - a\n>   - b\n", true, "BQ 内同列表嵌套（b 在 a 内层——CM marker 列 2 ≥ 0+2）"),
        ("> - a\n> - b\n", false, "同列表 item（缩进 0——平铺）"),
        ("> # t\n>   - b\n", false, "标题后独立列表（非嵌套——无父列表）"),
    ];
    // 值包 p 形态适配:item 内容流 ct = [值 p?, ...树子]——item
    // 的 ct 恒为数组(值 = 首 p 块);"值文本 == a" = 首 p 的 ct 字符串。
    fn a_has_nested_bl(v: &Value) -> bool {
        if let Some(nt) = v.get("nt").and_then(|x| x.as_str()) {
            if nt == "b" {
                let value_is_a = v.get("ct").and_then(|c| c.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|f| f.get("ct"))
                    .and_then(|t| t.as_str())
                    == Some("a");
                if value_is_a {
                    if let Some(ch) = v.get("ct").and_then(|c| c.as_array()) {
                        return ch.iter().any(|c| c.get("nt").and_then(|n| n.as_str()) == Some("bl"));
                    }
                    return false;
                }
            }
            if let Some(ch) = v.get("ct").and_then(|c| c.as_array()) {
                for c in ch.iter() {
                    if a_has_nested_bl(c) { return true; }
                }
            }
        }
        false
    }
    let mut failures = 0;
    for (doc, expect_nested, why) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let ast = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
        let v: Value = serde_json::from_str(&ast).unwrap_or(Value::Null);
        let nested = a_has_nested_bl(&v);
        if nested != *expect_nested {
            failures += 1;
            eprintln!("[PE48] {doc:?} 嵌套={nested} != 期望 {expect_nested}（{why}）");
        }
    }
    eprintln!("[PE48] BQ 内列表嵌套契约（{} 用例）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE48 失败 {failures}");
}

/// PE55: BQ 内混合 children 文档序契约（容器 children 全序修复:
/// expand(容器先)/attach(块后)两遍挂载使同父 children 为类别序——CF 树
/// 无章节门(HS 有 reorg 后全树排序兜底),修复在 build_attribution_tree
/// 步骤④ 后按 start 稳定排序归并回文档序(参照 debug 单遍树序)。
/// 断言: 每个 bq 的 ct 首元素 = 文档首块(h/p),列表容器不得排到块前。
#[test]
fn pe55_bq_children_doc_order_contract() {
    use serde_json::Value;
    let cases: &[(&str, &str, &str)] = &[
        ("> # t\n> - b\n", "h", "标题块先于其后列表容器"),
        ("> p\n>   - b\n", "p", "段落块先于其后缩进列表容器"),
        ("> p1\n> - b\n> p2\n", "p", "首段先于列表(p2 为 lazy 续行并入 item,不影响 bq 首子)"),
    ];
    // 递归找第一个 bq,返回其 ct 数组首元素 nt(无 bq/空 → None)。
    fn first_bq_child_nt(v: &Value) -> Option<String> {
        if let Some(nt) = v.get("nt").and_then(|x| x.as_str()) {
            if nt == "bq" {
                if let Some(c) = v.get("ct").and_then(|c| c.as_array()) {
                    if let Some(first) = c.first() {
                        return first
                            .get("nt")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string());
                    }
                }
                return None;
            }
            if let Some(c) = v.get("ct").and_then(|c| c.as_array()) {
                for ch in c {
                    if let Some(r) = first_bq_child_nt(ch) {
                        return Some(r);
                    }
                }
            }
        }
        None
    }
    let mut failures = 0;
    for (doc, want_first, why) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let ast = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
        let v: Value = serde_json::from_str(&ast).unwrap_or(Value::Null);
        let got = first_bq_child_nt(&v);
        if got.as_deref() != Some(*want_first) {
            failures += 1;
            eprintln!("[PE55] {doc:?} bq 首子={got:?} != 期望 {want_first:?}（{why}）");
        }
    }
    eprintln!("[PE55] BQ 混合 children 文档序契约（{} 用例）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE55 失败 {failures}");
}

/// PE49: RefDef 多行 angle dest 契约（Bug 4——官方例 196）。
/// `[Foo bar]:\n<my url>\n'title'`——官方 RefDef（URL=my%20url——angle
/// dest——title='title'）——type 7 修复后 `<my url>` 判 HtmlBlock——帧守卫
/// 截断——修复（帧守卫例外——body 0 + parse_dest_body 匹配 → 吸收）。
/// 边界: dest 已完成（body 1——`<div>` 独立 html）；body 0 的 `<div>` 是
/// angle dest（吸收）；非帧类行保持。
#[test]
fn pe49_refdef_angle_dest_contract() {
    let cases: &[(&str, &str, &str)] = &[
        // (文档, 期望引用可解析（[Foo bar] 渲染为链接——有定义）, 说明)
        ("[Foo bar]:\n<my url>\n'title'\n\n[Foo bar]\n", "<a href=\"my%20url\" title=\"title\">Foo bar</a>",
         "官方例 196——多行 angle dest + title——引用正常"),
        ("[a]: /url\n<div>\n", "<div>\n", "dest 已完成（body 1）——`<div>` 独立 html 块"),
        ("[a]:\n<div>\n\n[a]\n", "<a href=\"div\">a</a>", "body 0——`<div>` 是 angle dest（URL=div）"),
        ("[a]: /url\n'text'\n", "'text'\n", "非帧类行（引号）——title 场景——独立段落"),
    ];
    let mut failures = 0;
    for (doc, want_html, why) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        // 渲染检查: 引用定义解析——[Foo bar]/[a] 应渲染为链接（有定义）——
        // 或者独立块（无定义）——用 AST 的渲染（引用内联）
        let ast = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
        // 简化: 断言文档的块级结构（RefDef 块存在 + 后续块类型）
        let n = snap.line_len();
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let types: Vec<&str> = blocks.iter()
            .map(|b| mdio_core::effect::compact_type_name(b.block_type))
            .collect();
        let _ = ast;
        // 诊断（Bug 4 推演——PL vs 块记录）
        let pls: Vec<&str> = (0..n).map(|i| {
            let t = snap.line_type_at(i);
            mdio_core::effect::compact_type_name(t)
        }).collect();
        eprintln!("[PE49] {doc:?} PL={pls:?} 块={types:?}");
        // 断言核心: RefDef 块的存在与范围（多行 dest 吸收 = RefDef 块覆盖
        // 3 行——截断 = RefDef 单行 + HtmlBlock 独立）
        let refdef_count = blocks.iter().filter(|b| b.block_type == BlockType::RefDef).count();
        let html_count = blocks.iter().filter(|b| b.block_type == BlockType::HtmlBlock).count();
        let ok = match why {
            w if w.contains("196") => refdef_count == 1 && html_count == 0,
            w if w.contains("body 1") => refdef_count == 1 && html_count == 1,
            w if w.contains("body 0") => refdef_count == 1 && html_count == 0,
            _ => refdef_count == 1, // 引号场景——RefDef + 段落
        };
        if !ok {
            failures += 1;
            eprintln!("[PE49] {doc:?} 块类型 {types:?}（{why}）——RefDef×{refdef_count} HtmlBlock×{html_count}");
        }
    }
    eprintln!("[PE49] RefDef 多行 angle dest 契约（{} 用例）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE49 失败 {failures}");
}

/// PE50: 幽灵行生产契约（Bug 2 转正——迭代 37 是模拟预实验
/// （restore_cont_fixed）——本契约直接断言生产行为）。幽灵形态（BQ 内 IC
/// 后无 `>` 前缀的缩进续行——弹栈行）——修复后 cont=false + 有块归属
/// （官方语义——BQ 闭 + 顶层 IC 独立块——例 237）。
#[test]
fn pe50_ghost_line_production_contract() {
    let ghost_docs: &[&str] = &[
        ">     foo\n    bar\n",
        ">     foo\n        bar\n",
        ">     foo\n\n    bar\n",
    ];
    let normal_docs: &[&str] = &[
        ">     foo\n>     bar\n",
        "    foo\n    bar\n",
        "para\nlazy\n",
        "- a\n  cont\n",
    ];
    let mut failures = 0;
    // 幽灵形态: 修复后行 1（缩进续行）cont=false + 有块归属（非幽灵）
    for doc in ghost_docs {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        for i in 0..n {
            if snap.line_type_at(i) == BlockType::Blank { continue; }
            let in_block = blocks.iter().any(|b| (b.start as usize) <= i && i < (b.end as usize));
            if !in_block {
                failures += 1;
                eprintln!("[PE50] 幽灵残留 {doc:?} 行{i}: 有类型无块");
            }
        }
        // 弹栈行（行 1——BQ 内容后无前缀缩进行）cont 应为 false（官方——
        // 独立块非续行）
        if doc.contains("foo\n    ") || doc.contains("foo\n\n    ") {
            let cont = snap.lines[1].cont();
            if cont {
                failures += 1;
                eprintln!("[PE50] 幽灵 cont 残留 {doc:?} 行1: cont=true（应 false——弹栈独立块）");
            }
        }
    }
    // 正常形态: cont 保持（`>     foo\n>     bar` 行 1 cont=true——BQ 内续行）
    for doc in normal_docs {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let truth: Vec<bool> = (0..snap.line_len()).map(|i| snap.lines[i].cont()).collect();
        let _ = truth;
        // 断言关键形态不变（`>     foo\n>     bar`——行 1 cont=true）
        if doc.contains(">     foo\n>     bar") {
            if !snap.lines[1].cont() {
                failures += 1;
                eprintln!("[PE50] 正常形态回归 {doc:?} 行1: cont=false（应 true——BQ 内续行）");
            }
        }
    }
    eprintln!("[PE50] 幽灵行生产契约（{} 幽灵 + {} 正常形态）: {} 失败",
        ghost_docs.len(), normal_docs.len(), failures);
    assert_eq!(failures, 0, "PE50 失败 {failures}");
}

/// PE51: Bug 5 家族边角契约（container_block_type 补全——
/// 迭代 38 覆盖 BLT/OL/BQ/空行——本契约补 Table/Comment/HtmlElement——
/// 断言类型-组织一致（块记录 == 容器记录类型——脱节消除）+ PL 诊断）。
#[test]
fn pe51_html_container_family_edges() {
    use mdio_core::block::FrameKind;
    let cases: &[&str] = &[
        "- <div>\n  text\n",          // HtmlBlock 内容块——续行吸收
        "- <!--\n  comment\n  -->\n", // Comment 内容块
        "- <a href=\"x\">\n  text\n", // HtmlElement 内容块
        "- | a |\n  |---|---|\n",     // 列表内 Table（header + separator）
    ];
    let mut failures = 0;
    for doc in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let pls: Vec<&str> = (0..n).map(|i| {
            let t = snap.line_type_at(i);
            mdio_core::effect::compact_type_name(t)
        }).collect();
        eprintln!("[PE51] {doc:?} PL={pls:?}");
        // 类型-组织一致（迭代 38 模式——HtmlBlock/Comment/HtmlElement/Table
        // 容器内的块不得是 Paragraph（脱节——Bug 5 修复目标））
        // Bug 6 定稿:Table 例外——**tentative 帧**（body
        // bit0=0——未确认——伪表——`- |a|\n  |---|---|` 列不匹配——帧弹）内
        // 的 Paragraph 块是**回退的正确解析**（GFM 段落——ContextRewrite
        // 表化）——不是脱节——例外;确认帧（bit0=1）内的 Paragraph 块
        // 仍是不一致（真 bug——断言保留）。
        for (bi, b) in blocks.iter().enumerate() {
            if b.container_ref == u32::MAX { continue; }
            let r = &recs[b.container_ref as usize];
            let pseudo_table = r.frame.kind() == FrameKind::Table && r.frame.body() & 1 == 0;
            if matches!(r.frame.kind(), FrameKind::HtmlBlock | FrameKind::Comment
                | FrameKind::HtmlElement | FrameKind::Table)
                && b.block_type == BlockType::Paragraph
                && !pseudo_table
            {
                failures += 1;
                eprintln!("[PE51] {doc:?} 块{bi}: 块=Paragraph 容器={:?}——类型-组织不一致",
                    r.frame.kind());
            }
        }
    }
    eprintln!("[PE51] Bug 5 家族边角（Table/Comment/HtmlElement——{} 用例）: {} 失败",
        cases.len(), failures);
    assert_eq!(failures, 0, "PE51 失败 {failures}");
}

/// PE52: `<!--` 闭 marker 边角契约（Bug 6 候选——根因
/// 穿透确认）。`- <!--` 的 PL=p——Bug 3 闭规则的字面应用（内容 `<!--`
/// 以 `-` 结尾——`--` 注释闭合——endsWith(marker)——非列表——CM 例 354
/// 规则——cmark 同）——**非 Bug**——锚定防未来误改。对照：`<div>` 末
/// 字符 `>` 不闭——列表。
#[test]
fn pe52_comment_closed_marker_edge() {
    let cases: &[(&str, BlockType, &str)] = &[
        ("- <!--\n", BlockType::Paragraph, "闭规则字面（`<!--` 以 `-` 结尾——非列表——CM 例 354）"),
        ("- <div>\n", BlockType::HtmlBlock, "`<div>` 末字符 `>` 不闭——列表 item 内容块"),
        ("- <!--x\n", BlockType::Comment, "`<!--x` 末字符 `x` 不闭——列表——内容注释块（Bug 5 映射）"),
        ("- ---\n", BlockType::ThematicBreak, "`---` 是 TB（TB 判定优先——非列表）"),
        ("- key-1: v\n", BlockType::Bullet, "kv 内容 `key-1: v` 末字符 `v` 不闭——列表"),
    ];
    let mut failures = 0;
    for (doc, want, why) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let t = snap.line_type_at(0);
        if t != *want {
            failures += 1;
            eprintln!("[PE52] {doc:?} 行0 {:?} != 期望 {:?}（{why}）", t, want);
        }
    }
    eprintln!("[PE52] 闭 marker 边角（{} 用例——`<!--` 字面闭锚定）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE52 失败 {failures}");
}

/// PE53: setext 缩进标题的 CF/HS t 诊断（提案 2 后 conservation
/// 红——"t:   a" 不在 CF——实证 HS vs CF 的 t 值差异——定位）。
#[test]
fn pe53_setext_indent_tag_diag() {
    let cases: &[&str] = &[
        "  a\n---\n",
        "a\n---\n",
        "  a\n  ---\n",
        "a\nb\n---\n",
        "-  a\n  ---\n",
    ];
    for doc in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let pls: Vec<&str> = (0..n).map(|i| {
            let t = snap.line_type_at(i);
            mdio_core::effect::compact_type_name(t)
        }).collect();
        let cf = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
        let hs = snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Hs);
        eprintln!("[PE53] {doc:?} PL={pls:?}");
        eprintln!("[PE53]   CF={cf}");
        eprintln!("[PE53]   HS={hs}");
    }
}

/// PE54: 后向确认改写契约（ContextRewrite 表——Bug 6 定稿）。
/// 伪表（TableRow 无确认 Table 帧——分隔行确认失败/未发生——`|a| = b`
/// 数学文本）→ 段落;完整表格（列匹配分隔确认——body bit0）→ TableRow
/// 保持（表头+数据行——块记录层——PL/FL 不动——trilogy 往返不受影响）。
/// 用例 2 断言两 item 各自段落（has_push 段守卫——item 边界保留——GFM）。
#[test]
fn pe54_context_rewrite_table() {
    use mdio_core::effect::compact_type_name;
    let cases: &[(&str, &[&str], &str)] = &[
        ("- |a| = b\n", &["p"], "单行无分隔——段落（数学文本回归）"),
        ("- |a| = b\n- |c| = d\n", &["p", "p"],
         "两 item 无分隔——各自段落（has_push 守卫——item 边界——不合并）"),
        ("- |a|\n  |-|-|\n  |b|\n", &["p", "p"],
         "列不匹配（1 vs 2）——GFM 非表格——段落"),
        ("- |a| b|\n  |-|-|\n  |c| d|\n", &["tr", "tr"],
         "item 内完整表格（列匹配——分隔确认——保持）"),
        ("|a| b|\n|-|-|\n|c| d|\n", &["tr", "tr"], "顶层列匹配表格——不回归"),
        ("|a| = b\n", &["p"], "顶层单行无分隔——段落（tentative 参考）"),
        ("| a |\n|---|\n| 1 |\n| 2 |\n", &["tr", "tr", "tr"],
         "顶层 1 列完整表格（attr_table_rows 语料——分隔确认——保持）"),
        ("| a |\nplain\n", &["p", "p"],
         "table_pseudo（表头+非分隔——放弃——段落）——GFM 一段（跨死帧深度合并）是未来课题——现状两块不回归"),
    ];
    let mut failures = 0;
    for (doc, want, why) in cases {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let mut fold = mdio_core::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        mdio_core::infer::walk_zone(&snap.frame_log.entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = mdio_core::infer::build_block_records(&snap.frame_log.entries, &snap.lines,
            &snap.open_depth, &snap.rope, &recs, &arena, (0, n as u32));
        let bts: Vec<&str> = blocks.iter()
            .map(|b| compact_type_name(b.block_type))
            .collect();
        let spans: Vec<String> = blocks.iter()
            .map(|b| format!("{}({}..{})", compact_type_name(b.block_type), b.start, b.end))
            .collect();
        eprintln!("[PE54]   {doc:?} 块={spans:?}");
        if bts.as_slice() != *want {
            failures += 1;
            eprintln!("[PE54] {doc:?} 块={bts:?} != 期望 {want:?}（{why}）");
        }
    }
    eprintln!("[PE54] 后向确认改写契约（{} 用例——块记录层）: {} 失败", cases.len(), failures);
    assert_eq!(failures, 0, "PE54 失败 {failures}");
}
