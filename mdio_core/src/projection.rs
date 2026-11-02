//! MDIO Projection — Snapshot, materialization, AST JSON, block vocabulary facade.
//!
//! Owns the mutable projection core (State/ripple/JSON). Block vocabulary is
//! re-exported through this module for legacy callers (bins/test fixtures);
//! upper layers (selector, op) already use crate::infer directly.
//! Dependency direction: projection -> infer (one-way).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::cell::RefCell;
use crop::Rope;
use memchr;
// infer re-export 已清空。
// scan 侧 prev 三态提示统一使用 crate::infer::PrevHint（B8 单源）。
use crate::infer::PrevHint;
// D6 最终搬：impl Snapshot 全方法族自 infer 迁入（bak 前置）——所需 import 随迁。
use crate::selector::BlockBoundaryCache;
use crate::block::cell_ranges_bytes;
use std::ops::Range;
use crate::inline::InlineSegment;
// [S2] 表驱动薄封装：域实例（含口径）进格键、域→旧桶映射与入口分派表。
use crate::domain::{DomainId, TopoDep};

/// Replace U+0000 with U+FFFD per CommonMark §2.3 security requirement.
#[inline]
pub(crate) fn sanitize_nul(text: &str) -> std::borrow::Cow<'_, str> {
    if text.contains('\0') {
        std::borrow::Cow::Owned(text.replace('\0', "\u{FFFD}"))
    } else {
        std::borrow::Cow::Borrowed(text)
    }
}

/// UTF-8 char-boundary 钳制（向下——退到偏移所在字符的起点）。
/// 偏移落在多字节字符中间（`byte(idx)` 是 continuation byte 0x80..=0xBF）时
/// 回退到该字符的 lead byte；偏移 ≥ len → len（恒为 boundary）。
/// 防御偏移漂移（字符/字节坐标系混用——CRDT 回环/cola 多副本）——crop 的
/// line_of_byte/byte_slice/replace 对非 boundary 偏移直接 panic（worker 线程
/// 崩溃——消息静默丢失）——钳制后不崩，偏移最多差一个字符（防御路径可恢复）。
#[inline]
pub(crate) fn snap_down_char_boundary(rope: &Rope, mut idx: usize) -> usize {
    let len = rope.byte_len();
    if idx >= len { return len; }
    while idx > 0 && rope.byte(idx) & 0xC0 == 0x80 { idx -= 1; }
    idx
}

// EXTRACT_DBG/T_EXTRACT 从 incr 迁入（read_block_proj 的提取计时调试——
// 同批迁入 INLINE_DBG/T_INLINE/T_BLOCK/T_ATTR（walk 内部分解——渲染热点下钻）。
// 扩展：壳/章节壳自耗时 + BufWriter 重分配计数（精确下钻非块）。
thread_local! {
    /// 提取总时间（read_block_proj——渲染分配审计——SDL 定位 323KB
    /// 中间拷贝——量化提取真实耗时——直写决策依据）
    pub(crate) static EXTRACT_DBG: std::cell::RefCell<bool> = std::cell::RefCell::new(false);
    pub(crate) static T_EXTRACT: std::cell::RefCell<f64> = std::cell::RefCell::new(0.0);
    /// walk 内部分解（TO_AST_DBG——inline 扫描累计——定位渲染差异的大头——
    /// 门控输出——生产零开销（bool 检查））
    pub(crate) static INLINE_DBG: std::cell::RefCell<bool> = std::cell::RefCell::new(false);
    pub(crate) static T_INLINE: std::cell::RefCell<f64> = std::cell::RefCell::new(0.0);
    /// 块序列化总时间（write_block_node/write_item_leaf——含提取/attr/写）
    pub(crate) static T_BLOCK: std::cell::RefCell<f64> = std::cell::RefCell::new(0.0);
    /// attr 提取总时间（write_block_attrs_pre——read_block_proj/Raw）
    pub(crate) static T_ATTR: std::cell::RefCell<f64> = std::cell::RefCell::new(0.0);
    /// 容器壳自耗时（rn_shell 分支：壳开写 + 壳 attrs，不含 children 递归）
    pub(crate) static T_SHELL: std::cell::RefCell<f64> = std::cell::RefCell::new(0.0);
    /// 章节壳自耗时（rn_shell_sec 分支，不含 children 递归）
    pub(crate) static T_SEC_SHELL: std::cell::RefCell<f64> = std::cell::RefCell::new(0.0);
    /// TableRow 透明透传自耗时（rn_tr 分支）
    pub(crate) static T_TR: std::cell::RefCell<f64> = std::cell::RefCell::new(0.0);
    /// tight 段落直写自耗时（write_para_inline——不走 write_block_node）
    pub(crate) static T_PARA_INLINE: std::cell::RefCell<f64> = std::cell::RefCell::new(0.0);
    /// BufWriter ensure 次数（重分配次数——容量估计算账）
    pub(crate) static BUF_DBG: std::cell::RefCell<bool> = std::cell::RefCell::new(false);
    pub(crate) static BUF_ENSURE: std::cell::RefCell<u64> = std::cell::RefCell::new(0);
    pub(crate) static BUF_CAP_START: std::cell::RefCell<usize> = std::cell::RefCell::new(0);
    pub(crate) static BUF_CAP_FINAL: std::cell::RefCell<usize> = std::cell::RefCell::new(0);
}

/// 渲染内部分解结果（分析脚本消费——渲染热点下钻）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct RenderDiag {
    /// read_block_proj 提取总毫秒。
    pub extract_ms: f64,
    /// inline 扫描总毫秒。
    pub inline_ms: f64,
    /// 块序列化（write_block_node/write_item_leaf）总毫秒。
    pub block_ms: f64,
    /// attr 提取总毫秒。
    pub attr_ms: f64,
    /// 容器壳自耗时（不含 children 递归）。
    pub shell_ms: f64,
    /// 章节壳自耗时（不含 children 递归）。
    pub sec_shell_ms: f64,
    /// TableRow 透传自耗时。
    pub tr_ms: f64,
    /// tight 段落直写自耗时（write_para_inline）。
    pub para_inline_ms: f64,
    /// BufWriter ensure 次数（容量重分配）。
    pub buf_ensure: u64,
    /// 首个 BufWriter 初始容量。
    pub buf_cap_start: usize,
    /// 最后一个 BufWriter 结束容量。
    pub buf_cap_final: usize,
}

/// 渲染内部分解诊断（分析脚本消费——渲染热点下钻）。
/// begin 开启 EXTRACT_DBG/INLINE_DBG/BUF_DBG 累计并清零；end 关闭并返回
/// RenderDiag。生产零开销（bool 门控——false 时不计时）。
pub fn render_diag_begin() {
    EXTRACT_DBG.with(|c| *c.borrow_mut() = true);
    INLINE_DBG.with(|c| *c.borrow_mut() = true);
    BUF_DBG.with(|c| *c.borrow_mut() = true);
    T_EXTRACT.with(|c| *c.borrow_mut() = 0.0);
    T_INLINE.with(|c| *c.borrow_mut() = 0.0);
    T_BLOCK.with(|c| *c.borrow_mut() = 0.0);
    T_ATTR.with(|c| *c.borrow_mut() = 0.0);
    T_SHELL.with(|c| *c.borrow_mut() = 0.0);
    T_SEC_SHELL.with(|c| *c.borrow_mut() = 0.0);
    T_TR.with(|c| *c.borrow_mut() = 0.0);
    T_PARA_INLINE.with(|c| *c.borrow_mut() = 0.0);
    BUF_ENSURE.with(|c| *c.borrow_mut() = 0);
    BUF_CAP_START.with(|c| *c.borrow_mut() = 0);
    BUF_CAP_FINAL.with(|c| *c.borrow_mut() = 0);
}

/// 关闭累计并读取分解结果（见 render_diag_begin）。
pub fn render_diag_end() -> RenderDiag {
    EXTRACT_DBG.with(|c| *c.borrow_mut() = false);
    INLINE_DBG.with(|c| *c.borrow_mut() = false);
    BUF_DBG.with(|c| *c.borrow_mut() = false);
    RenderDiag {
        extract_ms: T_EXTRACT.with(|c| *c.borrow()),
        inline_ms: T_INLINE.with(|c| *c.borrow()),
        block_ms: T_BLOCK.with(|c| *c.borrow()),
        attr_ms: T_ATTR.with(|c| *c.borrow()),
        shell_ms: T_SHELL.with(|c| *c.borrow()),
        sec_shell_ms: T_SEC_SHELL.with(|c| *c.borrow()),
        tr_ms: T_TR.with(|c| *c.borrow()),
        para_inline_ms: T_PARA_INLINE.with(|c| *c.borrow()),
        buf_ensure: BUF_ENSURE.with(|c| *c.borrow()),
        buf_cap_start: BUF_CAP_START.with(|c| *c.borrow()),
        buf_cap_final: BUF_CAP_FINAL.with(|c| *c.borrow()),
    }
}

use crate::block::{BlockFrame, entry_line, entry_is_pop};
#[cfg(test)]
use crate::block::entry_frame;
#[cfg(test)]
use crate::block::entry_is_body_mut;

use crate::block::{
    PackedLineInfo,
    FrameLog,
    FrameKind,
    ScanAction,
    CompositePrimary,
    MAX_BLOCK_NESTING,
    LOOSE_BIT,
    push_list_entry,
    pop_entry,
    body_mut_entry,
    ANCHOR_NONE,
};
use crate::telemetry_span;

pub(crate) use crate::block::{
    is_header,
    trim_start,
};


// Block Vocabulary Facade


pub use crate::block::{
    BlockType,
    BlockNode,
    BlockProj,
    BlockMask,
    BlockProjSpec,
    BlockWriteTarget,
    InlineFeed,
    block_proj_spec,
};



// BufWriter — unsafe zero-overhead JSON buffer writer


pub(crate) struct BufWriter<'a> { buf: &'a mut Vec<u8>, ptr: *mut u8, pos: usize, cap: usize }

impl<'a> BufWriter<'a> {
    pub(crate) fn new(buf: &'a mut Vec<u8>) -> Self {
        let ptr = buf.as_mut_ptr();
        let cap = buf.capacity();
        BufWriter { buf, ptr, pos: 0, cap }
    }
    #[inline(always)]
    pub(crate) unsafe fn ensure(&mut self, extra: usize) {
        if self.pos + extra > self.cap {
            // Reserve for full pos+extra, not just extra.  Vec::reserve
            // ensures capacity >= len+additional, and len is always 0
            // (all writes go through BufWriter's raw pointer).  Using
            // extra alone would under-reserve once pos passes the initial
            // capacity.
            self.buf.reserve(self.pos + extra);
            self.ptr = self.buf.as_mut_ptr();
            self.cap = self.buf.capacity();
        }
    }
    #[inline(always)]
    pub(crate) unsafe fn push(&mut self, b: u8) {
        self.ensure(1);
        *self.ptr.add(self.pos) = b; self.pos += 1;
    }
    #[inline(always)]
    pub(crate) unsafe fn push_slice(&mut self, s: &[u8]) {
        self.ensure(s.len());
        self.ptr.add(self.pos).copy_from_nonoverlapping(s.as_ptr(), s.len());
        self.pos += s.len();
    }
    pub(crate) unsafe fn push_json(&mut self, s: &str) {
        self.push(b'"');
        let bytes = s.as_bytes();
        if !bytes.is_empty()
            && memchr::memchr(b'"', bytes).is_none()
            && memchr::memchr(b'\\', bytes).is_none()
            && !bytes.iter().any(|&b| b < 0x20)
        {
            self.ensure(bytes.len());
            self.ptr.add(self.pos).copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
            self.pos += bytes.len();
            self.push(b'"');
            return;
        }
        let mut safe = 0usize;
        for i in 0..bytes.len() {
            let b = bytes[i];
            if b == b'"' || b == b'\\' || b < 0x20 {
                let n = i - safe;
                if n > 0 {
                    self.ensure(n);
                    self.ptr.add(self.pos).copy_from_nonoverlapping(bytes.as_ptr().add(safe), n);
                    self.pos += n;
                }
                match b {
                    b'"'  => { self.push(b'\\'); self.push(b'"'); }
                    b'\\' => { self.push(b'\\'); self.push(b'\\'); }
                    b'\n' => { self.push(b'\\'); self.push(b'n'); }
                    b'\r' => { self.push(b'\\'); self.push(b'r'); }
                    b'\t' => { self.push(b'\\'); self.push(b't'); }
                    _     => {
                        let d = |v: u8| if v < 10 { b'0' + v } else { b'a' + (v - 10) };
                        self.push_slice(b"\\u00");
                        self.push(d(b >> 4)); self.push(d(b & 0xF));
                    }
                }
                safe = i + 1;
            }
        }
        let n = bytes.len() - safe;
        if n > 0 {
            self.ensure(n);
            self.ptr.add(self.pos).copy_from_nonoverlapping(bytes.as_ptr().add(safe), n);
            self.pos += n;
        }
        self.push(b'"');
    }
    pub(crate) unsafe fn commit(self) { self.buf.set_len(self.pos); }
}


// Thread-Local Scratch Buffer


thread_local! {
    static SCRATCH_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(4096));
}


// State — mutable projection core


/// ripple merge 期间捕获的结构变化
#[derive(Debug, Clone)]
pub struct RippleDiff {
    pub ripple_frame_actions: Vec<u64>,
    pub delta: isize,
    /// ripple 扫描的第一行 (min_dirty_line). 免费午餐 — ripple_lines 已知.
    /// session/sdl/store 经 take() 消费 (incr/hook 任务起点).
    pub ripple_line_start: usize,
    /// ripple 收敛行 (convergence). 后缀从这行开始.
    pub ripple_line_end: usize,
}

/// ripple conv 的旧侧单调重放游标（B3-1 性能列）。
/// 替代每候选行 `forward_replay_anchors(&entries[..prefix_len(old_next)])` 的
/// O(prefix) 重放：mapped_old = curr_line - delta 随扫描单调不减，游标只前进。
/// 折叠机械 = block::fold_replay_entry（与 forward_replay_anchors 单源）。
/// 不变量：
///   I1 ei 单调（mapped_old 每行 +1，delta 在一次 ripple 内为常数）；
///   I2 advance_to(L) 后 (stack, anchors) ==
///      forward_replay_anchors(entries[..prefix_len(L+1)])；
///   I3 line_buf == entries[prefix_len(L)..prefix_len(L+1)]（L 合法时）；
///   I4 mapped_old < 0 时不推进（splice 闸内 conv 必 false）；
///   I5 target 越过最后折叠行时清空 line_buf（mapped_old >= old_n 或无
///      entries 的行）。
#[derive(Debug)]
struct ShadowReplay<'a> {
    entries: &'a [u64],
    ei: usize,
    last_line: isize,
    stack: Vec<BlockFrame>,
    anchors: Vec<Option<usize>>,
    open_lines: Vec<u32>,
    line_buf: Vec<u64>,
    #[cfg(test)]
    fold_count: usize,
}

impl<'a> ShadowReplay<'a> {
    /// 测试与冷路径仍用(from-0 构造);生产已切 with_state(v2 anchor 增量化)。
    #[allow(dead_code)]
    fn new(entries: &'a [u64]) -> Self {
        ShadowReplay {
            entries,
            ei: 0,
            last_line: -1,
            stack: Vec::new(),
            anchors: Vec::new(),
            open_lines: Vec::new(),
            line_buf: Vec::new(),
            #[cfg(test)]
            fold_count: 0,
        }
    }

    /// v2: 以前缀重放状态初始化(shadow 起点 = anchor 而非 0)。
    /// 不变量与 new 一致:fold 是纯左折叠、状态是前缀函数——
    /// advance_to(L ≥ anchor) 后 == forward_replay(entries[..prefix_len(L+1)])。
    /// 等价性守护: shadow_replay_equals_forward_replay_anchors(new 版) +
    /// replay_cost_probe v2 台账(folds 逐条) + ripple_equivalence 130/60k fuzz。
    fn with_state(
        entries: &'a [u64],
        ei: usize,
        stack: Vec<BlockFrame>,
        anchors: Vec<Option<usize>>,
        open_lines: Vec<u32>,
    ) -> Self {
        ShadowReplay {
            entries,
            ei,
            last_line: -1,
            stack,
            anchors,
            open_lines,
            line_buf: Vec::new(),
            #[cfg(test)]
            fold_count: 0,
        }
    }

    /// 折叠到旧文档"处理完 target 行之后"的边界（含 target 行全部 entries）。
    fn advance_to(&mut self, target: usize) {
        debug_assert!(target as isize >= self.last_line, "shadow 游标必须单调");
        while self.ei < self.entries.len() {
            let e = self.entries[self.ei];
            let l = crate::block::entry_line(e) as usize;
            if l > target {
                break;
            }
            if l as isize != self.last_line {
                self.line_buf.clear();
                self.last_line = l as isize;
            }
            crate::block::fold_replay_entry(e, &mut self.stack, &mut self.anchors, &mut self.open_lines);
            self.line_buf.push(e);
            self.ei += 1;
            #[cfg(test)]
            {
                self.fold_count += 1;
            }
        }
        // target 行无 entries，或已越过旧文档尾：旧行 entries 必须为空。
        if target as isize > self.last_line {
            self.line_buf.clear();
        }
    }

    #[cfg(test)]
    fn fold_count(&self) -> usize {
        self.fold_count
    }
}

// 锚坐标唯一生产在测量层（block.rs）——本文件只做锚字段编码与规则 2 守卫。
use crate::block::{list_anchor as anchor_for, next_list_anchor};

/// 规则 2 守卫（CM §5.2）：**同种类**新 item 相对列表锚缩进 > 3 → 列表结束。
/// 跨种类（BLT↔OL）不守卫——marker 类型变化 = 新列表（CM §5.2），新列表位置
/// 由缩进决定（≥ 内容列 → item 内嵌套）——守卫误杀（`- a\n\n    1. b` 实测嵌套）。
/// 触发时调用者弹帧 + 重扫（顶层判定产出 IC/新块）。锚差在列表内同宽时
/// 精确等于"marker 相对锚缩进"（宽度跨位 ±1 已知偏差——记录）。
#[inline]
fn list_end_guard(f: &BlockFrame, anchors: &[Option<usize>], popped_kind: Option<FrameKind>) -> bool {
    anchors.last().copied().flatten().map_or(false, |a| {
        popped_kind == Some(f.kind())
            && crate::effect::is_list_container(f.kind())
            && !crate::block::anchor_continues(&f, a)
    })
}

/// 帧外运行状态：HtmlElement 的 Push 记录开行行号，其余帧为 NONE 哨兵。
#[inline]
fn open_line_for(f: &BlockFrame, li: u32) -> u32 {
    crate::block::frame_open_line(f, li)
}

/// 扫描循环的通用状态（不含 HtmlElement 专用 open_lines；open_lines 由 sink 持有）。
struct ScanState {
    stack: Vec<BlockFrame>,
    anchors: Vec<Option<usize>>,
    prev_hint: Option<PrevHint>,
    pending_loose: Vec<(usize, FrameKind)>,
    prev_semantic_depth: usize,
}

impl ScanState {
    fn new() -> Self {
        ScanState {
            stack: Vec::new(),
            anchors: Vec::new(),
            prev_hint: None,
            pending_loose: Vec::new(),
            prev_semantic_depth: 0,
        }
    }
}

/// 本行对下一行的承诺 = resolve(prev_hint, 本行 packed)。
/// prev_hint 在 action match 之前从原始 (t,a) 计算(投影改写/截断两路
/// 零事件路径)——承诺 = 原始意图,与改写后的 packed 解耦。
fn promise_of(hint: Option<PrevHint>, packed: PackedLineInfo) -> PackedLineInfo {
    match hint {
        Some(PrevHint::Content(t)) => PackedLineInfo::pack(t),
        Some(PrevHint::Blank) => PackedLineInfo::invalid(),
        Some(PrevHint::Default) | None => {
            let t = packed.node_type();
            if t == BlockType::Blank {
                PackedLineInfo::invalid()
            } else {
                PackedLineInfo::pack(t)
            }
        }
    }
}

/// 扫描动作接收器：统一 rebuild_all / ripple_lines 的 FL action 输出。
/// open_lines 由 sink 持有，scan_range 只借用，不拥有。
trait ScanSink {
    fn push_action(&mut self, e: u64);
    fn actions(&self) -> &[u64];
    fn open_lines(&self) -> &[u32];
    fn on_frame_push(&mut self, frame: BlockFrame, line: u32);
    fn on_frame_pop(&mut self);
}

/// 通用 Vec 版 sink：actions 写入外部 Vec，open_lines 内部维护。
struct VecSink<'a> {
    actions: &'a mut Vec<u64>,
    open_lines: Vec<u32>,
}

impl ScanSink for VecSink<'_> {
    fn push_action(&mut self, e: u64) {
        self.actions.push(e);
    }
    fn actions(&self) -> &[u64] {
        self.actions
    }
    fn open_lines(&self) -> &[u32] {
        &self.open_lines
    }
    fn on_frame_push(&mut self, frame: BlockFrame, line: u32) {
        self.open_lines.push(open_line_for(&frame, line));
    }
    fn on_frame_pop(&mut self) {
        self.open_lines.pop();
    }
}

/// 锚字段编码（entry 携带——重放恢复）。
#[inline]
fn anchor_enc(a: Option<usize>) -> u8 {
    a.map_or(ANCHOR_NONE, |v| v as u8)
}

/// 空行后接内容 → 在空行所属列表帧 (深度d, 种类k) 上置 LOOSE（CM：item 间空行
/// 或 item 内容空行 = loose；列表结束不置——tight）。
/// 调用时机 = "目标帧是栈顶" 的时刻：嵌套 Push 在 push **前**置位（旧栈顶=列表帧
/// ——body_mut 的栈顶语义满足——重放顺序 body_mut 先于 Push——帧完整）；行尾解析
/// （PopPush/Continue——新帧或帧不变——栈顶=目标）同样经此。Push 后置位会因栈顶
/// 已是新容器（嵌套）而被整帧替换破坏帧栈。
/// #326/#320/#327（CM §5.2）：空白行分隔的块所属的 item 才 loose——置"行
/// 处理结束时仍在栈上的最内层列表帧"（#326 `* foo\n  * bar\n\n  baz`——
/// baz 归外层——外层 loose 嵌套 tight；#320 `- a\n  - b\n\n    c`——c 归
/// 嵌套——嵌套 loose 外层 tight）；行尾栈无列表帧（内容归列表外——
/// `- a\n\ntext`）→ 回退置空行时的最内层。
fn resolve_pending_loose<S: ScanSink + ?Sized>(
    li: u32,
    pending_loose: &mut Vec<(usize, FrameKind)>,
    stack: &mut Vec<BlockFrame>,
    log: &mut S,
) {
    // 行尾栈上的最内层（pending 从外到内——rev 找第一个仍在栈的）
    let target = pending_loose.iter().rev().find(|&&(d, k)| {
        stack.len() > d && stack[d].kind() == k && !stack[d].loose()
    }).copied();
    let target = target.or_else(|| {
        // 全弹（内容归列表外）——回退空行时的最内层
        pending_loose.last().copied()
    });
    if let Some((d, k)) = target {
        if stack.len() > d && stack[d].kind() == k && !stack[d].loose() {
            stack[d] = BlockFrame(stack[d].0 | LOOSE_BIT);
            log.push_action(body_mut_entry(li, stack[d]));
        }
    }
    pending_loose.clear();
}

/// M0.2 同快照消费次数账本(#54 生产埋点):格查询/消费点计数累加。
/// 零行为——只计数,生产路径不读、不发帧、不改路径;读取出口不公开。
/// Arc 包裹 = Snapshot 的 Clone 派生下同快照账本共享。
/// pub(crate) = 读取出口不公开;不提供事实出口,可见性面与格值类型同纪律。
pub(crate) struct SnapshotConsumption {
    /// 树格消费次数(全量树格查询累加)。
    pub(crate) tree_full: AtomicU64,
    /// 节区间格消费次数(secs 格查询累加)。
    pub(crate) secs: AtomicU64,
    /// [S1] 按格种类的查询次数（下标见 `kind_idx`）。
    pub(crate) cell_query: [AtomicU64; 5],
    /// [S1] 命中次数（继承命中或本代格槽已就绪；**不含**本次构建出来的）。
    pub(crate) cell_hit: [AtomicU64; 5],
    /// [S1] 构建次数（仅当本槽此刻为空——即本次确实触发构建；并发败者不计）。
    pub(crate) cell_built: [AtomicU64; 5],
}

impl SnapshotConsumption {
    /// 格种类 → 账本下标（唯一映射点）。
    #[inline]
    pub(crate) fn kind_idx(kind: CellKind) -> usize {
        match kind {
            CellKind::Tree => 0,
            CellKind::Secs => 1,
            CellKind::RootTable => 2,
            CellKind::RootBlocksTable => 3,
            CellKind::BlockSpans => 4,
        }
    }

    /// 读取出口（测试/判官用）：`(查询, 命中, 构建)`。
    pub(crate) fn cell_stats(&self, kind: CellKind) -> (u64, u64, u64) {
        let i = Self::kind_idx(kind);
        (
            self.cell_query[i].load(Ordering::Relaxed),
            self.cell_hit[i].load(Ordering::Relaxed),
            self.cell_built[i].load(Ordering::Relaxed),
        )
    }
}

/// [S2] 域实例（含口径）→ 旧桶（账本五桶的产物种类轴，**轴不变**）。
///
/// 判据（G.8-2）：**满射 + 全性，不是双射**——每个 `slot == Cell` 的域恰好映到
/// 一个桶、每个桶至少被一个域映到、非格档域映到 `None`；但**不要求**「格实例 ↔
/// 桶」一一对应：`tree` / `sec` 的 cf/hs 两个拓扑实例**共桶**（G.6 显式声明该折叠，
/// 拓扑进键不进桶）。
pub(crate) const fn domain_bucket(d: DomainId) -> Option<CellKind> {
    match d {
        DomainId::RootMerge => Some(CellKind::RootTable),
        DomainId::RootSplit => Some(CellKind::RootBlocksTable),
        DomainId::Tree => Some(CellKind::Tree),
        DomainId::Sec => Some(CellKind::Secs),
        DomainId::Span => Some(CellKind::BlockSpans),
        // 非格档（PerCall）域：无桶（G.6「其余六行」）。
        DomainId::ZoneAuth
        | DomainId::ZoneMat
        | DomainId::NodeEndNav
        | DomainId::NodeEndRec
        | DomainId::ContTailView
        | DomainId::ContTailBuild => None,
    }
}

/// [S2] prewarm-critical 域（G.8-1：只放**有实测证据**的域，不预先冻结未取证者）。
///
/// `RootMerge`：A-0′ 10× 实测（新侧根表不复用 ⇒ 下一代旧侧落冷路径，
/// `root_count` p50 13.90µs → 140.06µs）。
/// **待实测**（机制推导有、缺 p50 证据，测出后再提升进本常量）：
/// `RootSplit`（旧格冷则自建三截）、`Sec`（旧格缺即退全量 fold）——两条 p50 是 S2 交付项。
pub(crate) const PREWARM_CRITICAL: &[DomainId] = &[DomainId::RootMerge];

/// 表自检（模块级常量：编译期无条件求值）：分派表里每个域都必须有账本桶。
const _: () = {
    let mut i = 0usize;
    while i < CELL_ENTRANCES.len() {
        assert!(
            domain_bucket(CELL_ENTRANCES[i].domain).is_some(),
            "格入口分派表的域必须有账本桶"
        );
        i += 1;
    }
};

/// 派生格键(§4.1):范围(现状两消费者恒 (0,n);局部维度预留)+ 拓扑
/// (数据声明,含 heading/zone/shells——不造"拓扑名字符串")+ 种类。
/// Eq/Hash 按拓扑指针恒等(&'static Topology 恒为进程内三静态之一)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CellKind {
    Tree,
    Secs,
    /// [归位 S1] A3-1 根表（CF 口径——`incr::roots_in_range`）。
    RootTable,
    /// [归位 S1] A3-2 根表（split 口径——`incr::roots_split_in_range`）。
    RootBlocksTable,
    /// [归位 S1] A3-0b 全量口径块跨度 `(start,end)`。
    BlockSpans,
}

#[derive(Clone, Copy)]
pub(crate) struct CellKey {
    pub(crate) range: (usize, usize),
    pub(crate) topo: &'static crate::topology::Topology,
    /// [S2] 第三元 = 域实例（含口径）：两套并行口径各自成键（计划 5.4）。
    pub(crate) domain: DomainId,
}

impl PartialEq for CellKey {
    fn eq(&self, other: &Self) -> bool {
        self.range == other.range
            && std::ptr::eq(self.topo, other.topo)
            && self.domain == other.domain
    }
}
impl Eq for CellKey {}

impl std::hash::Hash for CellKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.range.hash(state);
        (self.topo as *const crate::topology::Topology as usize).hash(state);
        // [S2] `DomainId` 是 `#[repr(u16)]` 域枚举（声明模块不 derive Hash）：
        // 判别值入哈希，与 Eq 的 `domain == domain` 同域。
        (self.domain as u16).hash(state);
    }
}

/// 节区间格值 = 双产物(§7.2):secs 全量四元 + header 容器截断 bounds
/// (身份语义,只从全量单源 fold——header_container_bounds 键 =
/// (标题块 start, 容器帧 kind),值 = 容器截断 bound;根级标题无条目)。
pub(crate) struct SecsCell {
    /// 窄化 secs 四元：(start,end,level,block_type as u8)——12B/条。
    pub(crate) secs: Vec<(u32, u32, u8, u8)>,
    /// 有序 Vec<(start,kind,bound)> + 二分（点查询、条目 ≤ 标题数）。
    header_bounds: Vec<(u32, u8, u32)>,
    /// Phase B: HS 根起点升序列表，供 `hs_off_before` 二分。
    pub(crate) root_starts: Vec<u32>,
    /// [P2c] `section_intervals` 的输入列持久化：供结构编辑做区间维护，
    /// 避免从 0 重新物化标题元数据。
    heads: Vec<crate::infer::HeadingMeta>,
}

impl SecsCell {
    /// usize 视野迭代器（消费面零拷贝映射——BlockType 判别值 0-26 合法）。
    #[inline]
    pub(crate) fn secs_iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (usize, usize, u8, crate::block::BlockType)> + '_ {
        self.secs.iter().map(|&(s, e, lv, t)| {
            let bt = if t <= crate::block::BlockType::HtmlElement as u8 {
                // SAFETY: 构建点只写入合法 BlockType 判别值（0-26）。
                unsafe { std::mem::transmute::<u8, crate::block::BlockType>(t) }
            } else {
                crate::block::BlockType::Paragraph
            };
            (s as usize, e as usize, lv, bt)
        })
    }

    /// header 容器截断点查询（(start,kind) 键——升序二分）。
    #[inline]
    pub(crate) fn header_bound(&self, start: usize, kind: u8) -> Option<usize> {
        let key = (start as u32, kind);
        let idx = self
            .header_bounds
            .binary_search_by(|&(s, k, _)| (s, k).cmp(&key))
            .ok()?;
        Some(self.header_bounds[idx].2 as usize)
    }

    /// Phase B: `line` 之前的 HS 根数量。
    #[inline]
    pub(crate) fn root_count_before(&self, line: usize) -> usize {
        self.root_starts.partition_point(|&s| (s as usize) < line)
    }

    /// [P2d-v1] 结构编辑的保守区间更新。
    ///
    /// [P2] 结构编辑的区间更新：sealed zone 外结构身份不变，只平移行标；
    /// zone 内重算；章节边界变化的旧章节内块按 transition 规则回旧侧枚举。
    pub(crate) fn update_structural(
        &self,
        old_snap: &Snapshot,
        new_snap: &Snapshot,
        new_n: usize,
        new_zone_seg: &crate::cache::Segment,
        old_edit_ranges: &[(u32, u32)],
        zs_old: usize,
        ce_old: usize,
        delta: isize,
        anchor: usize,
        topo: &'static crate::topology::Topology,
    ) -> Option<SecsCell> {
        // [测量插桩] kv 维度：把 O(N) 拆成"全量列表规模"与"zone 位置"。
        // n_roots = 旧 root_starts 全量（852-864 循环的自变量）；zs_old/ce_old
        // = 旧侧 zone（判断哪些条目可整段平移）。
        #[cfg(feature = "telemetry")]
        let _sp_sus = telemetry_span!("secs_update_structural",
            "n_roots" => self.root_starts.len(),
            "n_secs" => self.secs.len(),
            "n_heads" => self.heads.len(),
            "zs_old" => zs_old,
            "ce_old" => ce_old,
            "new_n" => new_n,
            "mid_start" => new_zone_seg.start_line,
            "mid_end" => new_zone_seg.end_line,
            "mid_blocks" => new_zone_seg.blocks.len(),
            "mid_records" => new_zone_seg.records.len());
        let zs_new = new_zone_seg.start_line;
        let ce_new = new_zone_seg.end_line;
        // LineMap 单锚语义：anchor（首编辑旧侧行）及之前不变，之后 +delta。
        let map_new = |v: u32| -> u32 {
            if (v as usize) <= anchor {
                v
            } else {
                ((v as isize) + delta).max(0) as u32
            }
        };

        #[cfg(feature = "telemetry")]
        let _s_meta = crate::telemetry::span("sus_meta");
        let mut new_mid = crate::infer::heading_meta(
            &new_zone_seg.blocks,
            &new_zone_seg.records,
            &new_zone_seg.arena,
            topo.heading_candidates,
            new_n,
        );
        new_mid.sort_unstable_by_key(|h| h.start);
        #[cfg(feature = "telemetry")]
        drop(_s_meta);

        #[cfg(feature = "telemetry")]
        let _s_heads = crate::telemetry::span("sus_heads");
        let mut heads: Vec<crate::infer::HeadingMeta> = self
            .heads
            .iter()
            .copied()
            .filter(|h| (h.start as usize) < zs_old)
            .map(|mut h| {
                h.bound = map_new(h.bound);
                h
            })
            .chain(new_mid.iter().copied())
            .chain(self.heads.iter().copied().filter(|h| {
                (h.start as usize) >= ce_old
            }).map(|mut h| {
                h.start = map_new(h.start);
                h.bound = map_new(h.bound);
                h
            }))
            .collect();
        heads.sort_unstable_by_key(|h| h.start);

        let secs_raw = crate::infer::section_intervals_from_heads(&heads, new_n);
        #[cfg(feature = "telemetry")]
        drop(_s_heads);
        let secs: Vec<(u32, u32, u8, u8)> = secs_raw
            .into_iter()
            .map(|(s, e, lv, t)| (s as u32, e as u32, lv, t as u8))
            .collect();

        #[cfg(feature = "telemetry")]
        let _s_bounds = crate::telemetry::span("sus_bounds");
        let mut header_bounds: Vec<(u32, u8, u32)> = self
            .header_bounds
            .iter()
            .copied()
            .filter(|(s, _, _)| (*s as usize) < zs_old)
            .map(|(s, k, b)| (s, k, map_new(b)))
            .chain(self.header_bounds.iter().copied().filter(|(s, _, _)| {
                (*s as usize) >= ce_old
            }).map(|(s, k, b)| (map_new(s), k, map_new(b))))
            .collect();
        for b in new_zone_seg.blocks.iter() {
            if crate::block::is_header(b.block_type) && b.container_ref != u32::MAX {
                let rec = &new_zone_seg.records[b.container_ref as usize];
                let bound = rec
                    .push_lines_of(&new_zone_seg.arena)
                    .find(|l| *l > b.start)
                    .map(|l| l as usize)
                    .unwrap_or_else(|| rec.end_usize().unwrap_or(new_n));
                header_bounds.push((b.start, rec.frame.kind() as u8, bound as u32));
            }
        }
        header_bounds.sort_unstable_by_key(|&(s, k, _)| (s, k));
        header_bounds.dedup();
        #[cfg(feature = "telemetry")]
        drop(_s_bounds);

        // [A6] 前缀最大 end——把「存在 ss < x < ee」从 O(H) 线性扫降到 O(log H)。
        // secs 按 start 升序且 start 互异；存在性 ⟺ 前缀 [0, idx) 的 max end > x
        // （idx = 首个 ss >= x 的下标）。语义与逐项 any 完全等价。
        let mut max_end: Vec<u32> = Vec::with_capacity(secs.len());
        {
            let mut cur = 0u32;
            for &(_, e, _, _) in &secs {
                cur = cur.max(e);
                max_end.push(cur);
            }
        }
        let in_section = |x: u32| -> bool {
            let idx = secs.partition_point(|&(ss, _, _, _)| ss < x);
            idx > 0 && max_end[idx - 1] > x
        };

        #[cfg(feature = "telemetry")]
        let _s_chg = crate::telemetry::span("sus_changed");
        // [P2] changed 检测：旧章节映射到新侧后找不到同 (start,end,level) 的
        // 对应项，说明章节边界发生变化，需要回旧侧枚举 transition 候选。
        let mut changed: Vec<(usize, usize)> = Vec::new();
        for &(s, e, lv, _) in &self.secs {
            let ns = map_new(s);
            let ne = map_new(e);
            // [A6] secs 的 start 互异 ⟹ 精确键查用二分（原 O(H_old×H_new) 线性扫）。
            let idx = secs.partition_point(|&(s2, _, _, _)| s2 < ns);
            let stable = idx < secs.len()
                && secs[idx].0 == ns
                && secs[idx].1 == ne
                && secs[idx].2 == lv;
            if !stable {
                changed.push((s as usize, e as usize));
            }
        }

        #[cfg(feature = "telemetry")]
        let mut kv_max_b: u32 = 0;
        #[cfg(feature = "telemetry")]
        let mut kv_n_old_roots: usize = 0;

        // 旧侧删除行区间：transition 候选若与任一删除区相交，则其身份没有
        // 存活到新侧，不能按单锚映射平移；这些块由 new_zone_seg 重算承接。
        // 不能用 min/max 凸包：多编辑事务的两次删除之间可能有存活 root。
        let mut deleted_lines: Vec<(usize, usize)> = Vec::new();
        for &(sb, eb) in old_edit_ranges {
            if old_snap.byte_len() == 0 {
                break;
            }
            if (eb as usize) <= (sb as usize) {
                continue; // 纯插入，无删除行
            }
            let sb = (sb as usize).min(old_snap.byte_len().saturating_sub(1));
            let s = old_snap.line_of_byte(sb);
            let e = old_snap.line_of_byte(
                ((eb as usize) - 1)
                    .min(old_snap.byte_len().saturating_sub(1))
                    .max(sb),
            );
            deleted_lines.push((s, e));
        }

        // transition：旧章节内、新章节外的 CF root 块。`root_blocks_before`
        // 目前从 0 扫描到 max_b，再过滤 changed 区间；正确优先。
        #[cfg(feature = "telemetry")]
        drop(_s_chg);
        let mut transition_roots: Vec<u32> = Vec::new();
        #[cfg(feature = "telemetry")]
        let _s_tr = crate::telemetry::span("sus_transition");
        if !changed.is_empty() {
            let max_b = changed
                .iter()
                .map(|&(_, b)| b)
                .max()
                .unwrap_or(0);
            // [区间化] 先算候选区间 = changed ∩（新覆盖的**空隙**映射回旧侧）。
            // 为空 ⟹ 无候选 ⟹ **不查也不建 split 根表格**——实测该格构建本身是
            // O(#roots)（20k 行 ≈ 1.5ms），是本函数残余 O(N) 的唯一来源。
            let inv = |v: usize| -> usize {
                if v <= anchor { v } else { (v as isize - delta).max(0) as usize }
            };
            let mut ranges: Vec<(usize, usize)> = Vec::new();
            {
                let mut cursor = 0usize;
                let mut gaps: Vec<(usize, usize)> = Vec::new();
                for &(s, e, _, _) in &secs {
                    let (s, e) = (s as usize, e as usize);
                    if cursor < s {
                        gaps.push((inv(cursor), inv(s)));
                    }
                    if e > cursor { cursor = e; }
                }
                if cursor < new_n {
                    gaps.push((inv(cursor), inv(new_n)));
                }
                for &(g0, g1) in &gaps {
                    for &(a, b) in &changed {
                        let lo_l = g0.max(a);
                        let hi_l = g1.min(b);
                        if lo_l < hi_l { ranges.push((lo_l, hi_l)); }
                    }
                }
                ranges.sort_unstable();
            }
            // [A3-2] split 口径根表优先（`root_blocks_before` 语义——d_before 分支
            // 不做 setext 合并）；按 start < max_b 二分切片 O(log + k)，end 按 max_b
            // 截断。**注意**：oracle 的截断视图有伪影（自身非 prefix-一致），索引给的是
            // 全量语义；两者仅在「oracle 自洽的 end」上逐值相等（判官已证）。
            let old_roots: Vec<(usize, usize, crate::block::BlockType)> = if ranges.is_empty() {
                Vec::new()
            } else {
                match crate::cache::kernel_root_blocks_table(old_snap) {
                    Some(tbl) => {
                        // 候选区间已在上方算好（changed ∩ 新覆盖空隙的旧侧像）。
                        let mut out: Vec<(usize, usize, crate::block::BlockType)> = Vec::new();
                        for &(lo_l, hi_l) in &ranges {
                            let lo = tbl.partition_point(|&(s, _, _)| (s as usize) < lo_l);
                            let hi = tbl.partition_point(|&(s, _, _)| (s as usize) < hi_l);
                            out.extend(tbl[lo..hi].iter().map(|&(s, e, t)| {
                                (s as usize, (e as usize).min(max_b), t)
                            }));
                        }
                        out.sort_unstable_by_key(|&(s, _, _)| s);
                        out.dedup_by_key(|v| v.0);
                        out
                    }
                    None => crate::infer::root_blocks_before(
                        &old_snap.frame_log.entries,
                        &old_snap.lines,
                        &old_snap.rope,
                        &old_snap.frame_log,
                        max_b,
                    )
                    .into_iter()
                    .map(|(s, e, _, t)| (s, e, t))
                    .collect(),
                }
            };
            #[cfg(feature = "telemetry")]
            {
                kv_max_b = max_b as u32;
                kv_n_old_roots = old_roots.len();
            }
            #[cfg(test)]
            SECS_TRANSITION_ROOTS_ENUMERATED.with(|c| c.set(c.get() + old_roots.len() as u64));
            #[cfg(test)]
            SECS_TRANSITION_STATS.with(|c| c.set((changed.len() as u64, max_b as u64, old_roots.len() as u64)));
            for (s, e, bt) in old_roots {
                if crate::block::is_header(bt) {
                    continue;
                }
                // [区间化] `changed` 过滤已由上面的切片保证，此处不再逐条扫 changed。
                // 删除区内候选不得做纯 δ 平移。
                if deleted_lines
                    .iter()
                    .any(|&(ds, de)| (s as usize) <= de && (e as usize) > ds)
                {
                    continue;
                }
                let ns = map_new(s as u32);
                // 映射落点必须仍是可成根的行：跳过 skip/cont/HsSep/非有效段落。
                let nli = ns as usize;
                if nli >= new_snap.line_len() {
                    continue;
                }
                let nt = new_snap.line_type_at(nli);
                if crate::effect::meta_is_skip(nt)
                    || new_snap.lines[nli].cont()
                    || nt == crate::block::BlockType::HsSep
                    || (nt == crate::block::BlockType::Paragraph
                        && !crate::infer::effective_para_at(
                            &new_snap.lines,
                            &new_snap.rope,
                            nli,
                            new_snap.line_len(),
                            None,
                            None,
                        ))
                {
                    continue;
                }
                if !in_section(ns) {
                    transition_roots.push(ns);
                }
            }
            #[cfg(test)]
            {
                SECS_UPDATE_ROOT_BLOCKS_CALLS.with(|c| c.set(c.get() + 1));
            }
        }

        #[cfg(feature = "telemetry")]
        drop(_s_tr);
        let mut root_starts: Vec<u32> = transition_roots;
        #[cfg(feature = "telemetry")]
        let _s_rs = crate::telemetry::span("sus_rootstarts");
        for &s in &self.root_starts {
            if (s as usize) < zs_old {
                if !in_section(s) {
                    root_starts.push(s);
                }
            } else if (s as usize) >= ce_old {
                let ns = map_new(s);
                if !in_section(ns) {
                    root_starts.push(ns);
                }
            }
        }

        for h in heads.iter() {
            if h.container_ref == u32::MAX && !in_section(h.start) {
                root_starts.push(h.start);
            }
        }
        for r in new_zone_seg
            .records
            .iter()
            .filter(|r| r.parent == u32::MAX && (r.start as usize) >= zs_new && (r.start as usize) < ce_new)
        {
            if !in_section(r.start) {
                root_starts.push(r.start);
            }
        }
        for b in new_zone_seg
            .blocks
            .iter()
            .filter(|b| b.container_ref == u32::MAX && (b.start as usize) >= zs_new && (b.start as usize) < ce_new)
        {
            if crate::block::is_header(b.block_type) {
                continue;
            }
            if !in_section(b.start) {
                root_starts.push(b.start);
            }
        }
        root_starts.sort_unstable();
        root_starts.dedup();
        #[cfg(feature = "telemetry")]
        drop(_s_rs);

        #[cfg(feature = "telemetry")]
        let _sp_sus = _sp_sus
            .kv("changed", changed.len())
            .kv("max_b", kv_max_b)
            .kv("n_old_roots", kv_n_old_roots);
        Some(SecsCell {
            secs,
            header_bounds,
            root_starts,
            heads,
        })
    }
}

/// [P2] 测试专用账本：`update_structural` 进入 transition 分支的次数。
/// 生产路径不读；性能 gate 用它断言“不碰标题的结构编辑零 transition 枚举”。
/// 用 thread_local 隔离并行测试，避免共享全局计数器互相污染。
#[cfg(test)]
thread_local! {
    /// [判官用] transition 分支**实际枚举的旧根条数**（不是分支进入次数）。
    /// 断言它受 `changed` 区间约束——长节文档上必须 ≪ 文档根数。
    pub(crate) static SECS_TRANSITION_ROOTS_ENUMERATED: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
    /// [诊断用] (changed.len(), max_b, old_roots.len())
    pub(crate) static SECS_TRANSITION_STATS: std::cell::Cell<(u64, u64, u64)> =
        const { std::cell::Cell::new((0, 0, 0)) };
    pub(crate) static SECS_UPDATE_ROOT_BLOCKS_CALLS: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

/// 树格值:全量段 + 归属树(三步形态的输入与产物一并入格——渲染层
/// 取格后经 with_arrays 重建借用视图,零重复结构构建)。
pub(crate) struct TreeCell {
    pub(crate) seg: Arc<crate::cache::Segment>,
    pub(crate) tree: Arc<crate::infer::AttrTreeNew>,
}

/// 格值:树格(全量 × Topology)或节区间格。Arc 包裹 = 查询零拷贝。
pub(crate) enum CellValue {
    Tree(Arc<TreeCell>),
    Secs(Arc<SecsCell>),
    /// [归位 S1] 根表载荷（`RootTable` / `RootBlocksTable` 共用——同为 `(start,end,type)`）。
    RootTable(Arc<Vec<(u32, u32, crate::block::BlockType)>>),
    /// [归位 S1] 块跨度载荷。
    BlockSpans(Arc<Vec<(u32, u32)>>),
}

/// 格表(§4.1,与 block_cache 一字同构):外层 Arc<OnceLock<Mutex<..>>>
/// 承担单构建,CellTable 内部 = HashMap<CellKey, Arc<OnceLock<Arc<CellValue>>>>。
/// 键级语义 = **真 per-key OnceLock**:表 Mutex 只覆盖"查/建槽"(O(1)),
/// 构建在锁外由槽的 OnceLock 承担——同键并发仍单构建(首次写赢),异键
/// 构建互不阻塞(与 block_cache 的 merge 语义——后到者白付——仍分歧;
/// "同构"仅指外层容器形态)。随快照生死,零版本管理、零淘汰代码。
#[derive(Default)]
pub struct CellTable {
    map: std::collections::HashMap<CellKey, Arc<std::sync::OnceLock<Arc<CellValue>>>>,
}

/// 派生格**跨代继承**查询：只读**直接前驱**已初始化的格表，不递归、不触发
/// 前驱构建；命中 = Arc clone 零拷贝，未命中返回 None（调用方转构建路径）。
///
/// 正确性依赖声明(§9,825 E 组范式——引用现成资产不重证):派生格复用
/// 判定的前提链,任何一条被放宽,派生格复用随之下线——
/// ① zone 密封性(`sdl_zone_closure_vs_records`);
/// ② splice 段 == 全量段(`assembled_full_equals_full_build`);
/// ③ 后缀 δ 平移恒等(`prefix_invariance_fuzz`);
/// ④ overlay δ=0 上游(session seam 判定 + `validate_content_overlay` 第二道确认);
/// ⑤ 渲染零行号。

/// Phase B: 只继承“直接前驱且已初始化”的格；不递归、不触发前驱构建。
/// prev_snapshot 只在守卫证明结构全等时被设置，所以键命中即可共享。
fn inherit_cell_from_prev(snap: &Snapshot, key: CellKey) -> Option<Arc<CellValue>> {
    let prev = snap.prev_snapshot.upgrade()?;
    let table = prev.cells.as_ref()?;
    let table = table.get()?;
    let slot = {
        let t = table.lock().unwrap();
        t.map.get(&key).cloned()?
    };
    slot.get().cloned()
}

/// [复核补记 A-1] 只读探测**当前快照**已初始化的格：不建、不触发 materialize、
/// 不递归；命中 = Arc clone，未命中 = None（调用方退回原路径）。与
/// `inherit_cell_from_prev` 同款，只是对象换成当前快照自己的格表。
/// 用途：热点点查先看有没有现成答案，免得为一次查询重建整张表/整段结构。
/// 格访问策略（判定层的三个臂）。
///
/// **Build** = 今天四个类型化访问器的行为：继承腿（命中即安装）→ 未命中则构建；
/// **UseIfReady** = 继承腿（命中即安装）→ 本代格槽，未命中返回 `None`，由调用方走自己声明的回退；
/// **Peek** = 只查本代格槽，**不继承、不安装、不构建**（诊断/遥测专用，零写副作用）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CellPolicy {
    Build,
    UseIfReady,
    Peek,
}

/// [S2] 七个格入口的入口 id（账本「入口级归因」的轴；**入口 ≠ 域**：
/// 根表的 Build 入口服务两个口径域，Peek 入口同理，跨度域的同一入口有三个策略臂）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CellEntry {
    Tree,
    Section,
    RootTable,
    BlockSpans,
    PeekRootTable,
    SpansIfReady,
    PeekSpans,
}

/// 入口数（七：四个构建访问器 + 三个策略兄弟）。
pub(crate) const CELL_ENTRY_COUNT: usize = 7;

/// [S2] 格入口分派表（表驱动薄封装）：唯一登记「域实例 + 策略 → 入口 + 拓扑来源」。
/// 七个格入口原先各自硬编码的 domain/policy/topo 收敛到本表；行为逐值不变
/// （表只换分派形态，不换语义）。
pub(crate) struct CellEntranceDecl {
    /// 域实例（含口径）——键的第三元。
    pub(crate) domain: DomainId,
    /// 策略臂。
    pub(crate) policy: CellPolicy,
    /// 入口 id（账本入口级归因用）。
    pub(crate) entry: CellEntry,
    /// 拓扑来源：Pinned 取表内常量，Free 取调用点实参。
    pub(crate) topo: TopoDep,
}

/// 分派表：行数 = 域实例 × 策略臂（树/节各一行；根两口径 × Build/Peek；
/// 跨度域 × Build/UseIfReady/Peek）。
pub(crate) static CELL_ENTRANCES: [CellEntranceDecl; 9] = [
    CellEntranceDecl { domain: DomainId::Tree, policy: CellPolicy::Build, entry: CellEntry::Tree, topo: TopoDep::Free },
    CellEntranceDecl { domain: DomainId::Sec, policy: CellPolicy::Build, entry: CellEntry::Section, topo: TopoDep::Free },
    CellEntranceDecl { domain: DomainId::RootMerge, policy: CellPolicy::Build, entry: CellEntry::RootTable, topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY) },
    CellEntranceDecl { domain: DomainId::RootSplit, policy: CellPolicy::Build, entry: CellEntry::RootTable, topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY) },
    CellEntranceDecl { domain: DomainId::RootMerge, policy: CellPolicy::Peek, entry: CellEntry::PeekRootTable, topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY) },
    CellEntranceDecl { domain: DomainId::RootSplit, policy: CellPolicy::Peek, entry: CellEntry::PeekRootTable, topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY) },
    CellEntranceDecl { domain: DomainId::Span, policy: CellPolicy::Build, entry: CellEntry::BlockSpans, topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY) },
    CellEntranceDecl { domain: DomainId::Span, policy: CellPolicy::UseIfReady, entry: CellEntry::SpansIfReady, topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY) },
    CellEntranceDecl { domain: DomainId::Span, policy: CellPolicy::Peek, entry: CellEntry::PeekSpans, topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY) },
];

/// 表自检（模块级常量：编译期无条件求值）：每个入口 id 至少有一行。
const _: () = {
    let mut e = 0usize;
    while e < CELL_ENTRY_COUNT {
        let mut i = 0usize;
        let mut found = false;
        while i < CELL_ENTRANCES.len() {
            if CELL_ENTRANCES[i].entry as usize == e { found = true; break; }
            i += 1;
        }
        assert!(found, "格入口分派表漏了入口 id");
        e += 1;
    }
};

/// 按「域实例 + 策略」查表（S2 唯一分派点；表是编译期常量，扫描可被内联）。
#[inline]
fn cell_entrance(domain: DomainId, policy: CellPolicy) -> &'static CellEntranceDecl {
    let mut i = 0usize;
    while i < CELL_ENTRANCES.len() {
        let d = &CELL_ENTRANCES[i];
        if d.domain == domain && d.policy == policy {
            return d;
        }
        i += 1;
    }
    panic!("域实例 + 策略没有登记格入口分派行")
}

impl CellKey {
    /// 键的唯一构造点（本文件的 `CellKey { .. }` 字面量收敛于此）。
    #[inline]
    pub(crate) const fn of(
        range: (usize, usize),
        topo: &'static crate::topology::Topology,
        domain: DomainId,
    ) -> CellKey {
        CellKey { range, topo, domain }
    }
}

/// [S2 账本·入口级] 七个格入口的 (查询, 命中, 构建)——照
/// `incr::ROOTS_RANGE_LINES_WALKED` 的 `#[cfg(test)]` 先例。
///
/// 为什么必须入口级：桶级 `q` 会被**同桶其他入口**抬高（跨度桶有 Build /
/// UseIfReady / Peek 三入口），`PendingWire` 入口的 `q == 0` 无法在桶级证明。
/// 自增点 = `cell_with_entry` 里与桶级账本**同一处**（两本账逐次同步）；
/// 入口 id 由分派表给出（见 `CELL_ENTRANCES`）。
#[cfg(test)]
thread_local! {
    pub(crate) static CELL_ENTRY_LEDGER: std::cell::Cell<[(u64, u64, u64); CELL_ENTRY_COUNT]> =
        const { std::cell::Cell::new([(0, 0, 0); CELL_ENTRY_COUNT]) };
}

#[cfg(test)]
#[inline]
fn entry_ledger_bump(entry: Option<CellEntry>, q: u64, h: u64, b: u64) {
    let Some(e) = entry else { return };
    CELL_ENTRY_LEDGER.with(|c| {
        let mut a = c.get();
        let i = e as usize;
        a[i].0 += q;
        a[i].1 += h;
        a[i].2 += b;
        c.set(a);
    });
}

/// 测试读取出口：单个入口的 `(q, h, b)`。
#[cfg(test)]
pub(crate) fn cell_entry_stats(entry: CellEntry) -> (u64, u64, u64) {
    CELL_ENTRY_LEDGER.with(|c| c.get()[entry as usize])
}

/// 测试读取出口：全部入口的 `(q, h, b)`（下标 = `CellEntry` 判别值）。
#[cfg(test)]
pub(crate) fn cell_entry_totals() -> [(u64, u64, u64); CELL_ENTRY_COUNT] {
    CELL_ENTRY_LEDGER.with(|c| c.get())
}

/// 测试复位（入口级窗口测量前调用）。
#[cfg(test)]
pub(crate) fn cell_entry_reset() {
    CELL_ENTRY_LEDGER.with(|c| c.set([(0, 0, 0); CELL_ENTRY_COUNT]));
}

/// 格读取的唯一入口（六个手写「先查」写法收敛于此）。
///
/// 语义 = 三个臂的并集，**逐臂等价于原有实现**（本函数不新增缓存、不新增状态）；
/// 调用方各自保留签名与后续的 `match`/`unreachable!` 取值逻辑。
/// [S2] 桶位由**域实例 → 旧桶**映射给出（`domain_bucket`）；`entry` = 入口级
/// 账本归因（`None` = 键级内部调用，不记入口账）。
fn cell_with_entry(
    snap: &Snapshot,
    key: CellKey,
    policy: CellPolicy,
    entry: Option<CellEntry>,
    build: impl FnOnce() -> Arc<CellValue>,
) -> Option<Arc<CellValue>> {
    #[cfg(not(test))]
    let _ = entry;
    // [S1] 账本（零行为）：按格种类记 查询 / 命中 / 构建。
    // [S2] 种类由域实例映射而来（映射唯一来源 = `domain_bucket`）。
    let c = &snap.cell_consumption;
    let idx = SnapshotConsumption::kind_idx(
        domain_bucket(key.domain).expect("格键的域实例必须有账本桶"),
    );
    c.cell_query[idx].fetch_add(1, Ordering::Relaxed);
    #[cfg(test)]
    entry_ledger_bump(entry, 1, 0, 0);
    let hit = |v: Option<Arc<CellValue>>| -> Option<Arc<CellValue>> {
        if v.is_some() {
            c.cell_hit[idx].fetch_add(1, Ordering::Relaxed);
            #[cfg(test)]
            entry_ledger_bump(entry, 0, 1, 0);
        }
        v
    };
    match policy {
        CellPolicy::Peek => hit(peek_cell(snap, key)),
        CellPolicy::UseIfReady => match inherit_cell_from_prev(snap, key) {
            Some(inherited) => {
                // [O2] 锁内只占槽位、写入在锁外（OnceLock 首次写赢）。
                let _ = cell_slot(snap, key).set(inherited.clone());
                hit(Some(inherited))
            }
            None => hit(peek_cell(snap, key)),
        },
        CellPolicy::Build => match inherit_cell_from_prev(snap, key) {
            Some(inherited) => {
                // [O2] 锁内只占槽位、写入在锁外（OnceLock 首次写赢）。
                let _ = cell_slot(snap, key).set(inherited.clone());
                hit(Some(inherited))
            }
            None => {
                // 本槽此刻已就绪（并发胜者已建）⇒ 记命中；否则记本次构建。
                if cell_slot(snap, key).get().is_none() {
                    c.cell_built[idx].fetch_add(1, Ordering::Relaxed);
                    #[cfg(test)]
                    entry_ledger_bump(entry, 0, 0, 1);
                } else {
                    c.cell_hit[idx].fetch_add(1, Ordering::Relaxed);
                    #[cfg(test)]
                    entry_ledger_bump(entry, 0, 1, 0);
                }
                Some(get_or_build_cell(snap, key, build))
            }
        },
    }
}

/// 键级格读取（既有键级调用点与键级判官的入口；不记入口账）。
#[inline]
fn cell_with(
    snap: &Snapshot,
    key: CellKey,
    policy: CellPolicy,
    build: impl FnOnce() -> Arc<CellValue>,
) -> Option<Arc<CellValue>> {
    cell_with_entry(snap, key, policy, None, build)
}

/// [S2] 表驱动格入口核心：按「域实例 + 策略」查 `CELL_ENTRANCES`（唯一分派点），
/// 取拓扑来源与入口 id，构造键后转 `cell_with_entry`。
/// 七个格入口都是本函数的一层薄封装。
fn cell_entry(
    snap: &Snapshot,
    domain: DomainId,
    policy: CellPolicy,
    free_topo: Option<&'static crate::topology::Topology>,
    build: impl FnOnce() -> Arc<CellValue>,
) -> Option<Arc<CellValue>> {
    let decl = cell_entrance(domain, policy);
    let topo = match decl.topo {
        TopoDep::Pinned(t) => t,
        TopoDep::Free => free_topo.expect("Free 域实例的入口必须传拓扑实参"),
    };
    let key = CellKey::of((0, snap.line_len()), topo, domain);
    cell_with_entry(snap, key, policy, Some(decl.entry), build)
}

fn peek_cell(snap: &Snapshot, key: CellKey) -> Option<Arc<CellValue>> {
    let table = snap.cells.as_ref()?;
    let table = table.get()?;
    let slot = {
        // 只读：锁不可得即视为未热（不 panic、不阻塞），退回原路径。
        let t = table.lock().ok()?;
        t.map.get(&key).cloned()?
    };
    slot.get().cloned()
}

/// [P2d-v1] 结构编辑时尝试从“直接旧快照已初始化的 SecsCell”做区间更新。
/// 只读旧格，不触发旧快照构建；旧格不存在则返回 `None`，调用方走全量。
fn try_update_secs_from_edit(
    snap: &Snapshot,
    domain: DomainId,
    topo: &'static crate::topology::Topology,
) -> Option<SecsCell> {
    let slot = snap.cache_slot.upgrade()?;
    let node = slot.node.load_full()?;
    let crate::cache::CacheNode::Edit {
        old_snap,
        delta,
        anchor,
        edit_ranges,
        ..
    } = &**node
    else {
        return None;
    };
    let old = old_snap.upgrade()?;
    let old_key = CellKey::of((0, old.line_len()), topo, domain);
    let old_cell = {
        let slot = {
            let table = old.cells.as_ref()?.get()?;
            let t = table.lock().unwrap();
            t.map.get(&old_key)?.clone()
        };
        match slot.get()?.as_ref() {
            CellValue::Secs(s) => s.clone(),
            _ => return None,
        }
    };
    let (zso, ceo) = node.materialize_old_zone()?;
    let mid = node.materialize_mid()?;
    old_cell.update_structural(
        &old,
        snap,
        snap.line_len(),
        &mid,
        edit_ranges,
        zso,
        ceo,
        *delta as isize,
        *anchor as usize,
        topo,
    )
}


/// [O2] 取键槽位：表锁只覆盖"查/建槽"（O(1)），构建在锁外由槽的 `OnceLock`
/// 承担——同键并发仍单构建（首次写赢、后到者读已建值），但 O(doc) 构建
/// 不再持表锁阻塞其它键的查询（跨任务串行点消除）。
fn cell_slot(snap: &Snapshot, key: CellKey) -> Arc<std::sync::OnceLock<Arc<CellValue>>> {
    let table = snap
        .cells
        .as_ref()
        .expect("cells initialized at snapshot()")
        .get_or_init(|| std::sync::Mutex::new(CellTable::default()));
    let mut t = table.lock().unwrap();
    t.map
        .entry(key)
        .or_insert_with(|| Arc::new(std::sync::OnceLock::new()))
        .clone()
}

/// 派生格单构建查询(#36a 自写面之一)：表 Mutex 内只做"查/建槽"(O(1))，
/// 构建在锁外由槽的 OnceLock 承担——同键并发仍单构建（首次写赢）、异键构建
/// 互不阻塞（O2，`0fcb4aad`）；命中 = Arc clone 零拷贝。
fn get_or_build_cell(
    snap: &Snapshot,
    key: CellKey,
    build: impl FnOnce() -> Arc<CellValue>,
) -> Arc<CellValue> {
    cell_slot(snap, key).get_or_init(build).clone()
}

/// 不可变投影快照（D6 契约归属：struct 自 infer 迁入——类型真名 =
/// crate::projection::Snapshot；infer 经 pub use shim 承接旧路径）。
/// impl 全方法族暂居 infer（转 bak 随迁）。
#[derive(Clone)]
pub struct Snapshot {
    pub rope: Rope,
    pub lines: Arc<Vec<PackedLineInfo>>,
    /// prev 承诺列——一行一字节(PackedLineInfo 同构:
    /// pack(t) 或 invalid=无承诺)。"本行对下一行的行级承诺",在 action
    /// match 之前从原始 (t,a) 计算——两条零事件路径(投影改写/32 层
    /// 截断)证明它不是事件可导的(§一 形式修正)。扫描器私有列：读取点
    /// 只在 projection.rs 的 scan/ripple 收敛条件内；无跨模块读者。
    pub prev_col: Arc<Vec<PackedLineInfo>>,
    /// L0 行首深度列（depth_col_plan）：长度 n+1，depth[l]=行 l 事件前栈深，
    /// depth[n]=文档尾残深哨兵。与 lines/prev_col 同款三截维护。
    pub open_depth: Arc<Vec<u8>>,
    pub frame_log: Arc<FrameLog>,
    /// 块边界缓存（§6——infer_iter_plan——懒初始化——selector 跳块层写入/
    /// 读取——只增不改(快照不可变——记录永不过期)——编辑即随 Arc 释放——
    /// 零显式 invalidate——拓扑无关(块边界不因 CF/HS 不同)——FJ/PATCH
    /// 零开销(None 直到 selector 跳块层首次使用)——Arc(Clone 共享——
    /// 缓存只增不改——快照 clone 共享安全)——selector agent 加
    /// (§13 三方约定))。
    pub block_cache: Option<std::sync::Arc<std::sync::OnceLock<std::sync::Mutex<BlockBoundaryCache>>>>,
    /// PRC P4b.1: 写线程发布前注入的弱引用缓存槽。Weak 不构成强引用环；
    /// 旧快照升级失败时各集成点回退全量构建。
    pub(crate) cache_slot: std::sync::Weak<crate::cache::CacheSlot>,
    /// Phase B: 直接前驱快照（Weak，不 pin）。仅在守卫证明结构全等时设置；
    /// 派生格首次消费时只做一步懒继承，前驱死亡/未初始化/无前驱则裸建。
    pub(crate) prev_snapshot: std::sync::Weak<Snapshot>,
    /// M0.2 埋点(#54):格查询/消费点计数账本(零行为——见 SnapshotConsumption)。
    pub(crate) cell_consumption: Arc<SnapshotConsumption>,
    /// 派生格表(§4.1,与 block_cache 一字同构):随快照生死,消费者到达即建。
    pub cells: Option<Arc<std::sync::OnceLock<std::sync::Mutex<CellTable>>>>,
    /// 快照级段 memo(§7.1/#36b,block_cache.structure 的快照级归位):
    /// 全量段单值——写点 = 裸建兜底路径(无槽或链断),仅全量查询(0,n)
    /// 回填;selector 块导航与 op 直调面 memo-first 消费。
    pub(crate) segment_memo: std::sync::OnceLock<Arc<crate::cache::Segment>>,
    // [归位 S2] old_block_index 存储迁入派生格层（CellKind::BlockSpans）；
    // 消费面 old_block_index_of_own 为薄转发。
}

/// MD 渲染错误（公开 API 的错误面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdRenderError {
    /// 选择节点无法映射到 AttrTree 节点（cover 失败）。
    NodeNotFound,
    /// 同 span/type 存在多个同等候选，无法唯一 cover。
    AmbiguousCover,
    /// selected 节点 span 越界或树节点 span 非法。
    SpanOutOfRange,
    /// 当前拓扑不支持 selected MD（保留）。
    UnsupportedTopology,
}

impl std::fmt::Display for MdRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MdRenderError::NodeNotFound => "MD selected node not found in AttrTree",
            MdRenderError::AmbiguousCover => "MD selected node has ambiguous AttrTree cover",
            MdRenderError::SpanOutOfRange => "MD selected node span out of range",
            MdRenderError::UnsupportedTopology => "MD topology does not support selected render",
        })
    }
}

impl std::error::Error for MdRenderError {}

impl Snapshot {
    /// Deep-copy the five construction columns and reset all caches/weak/telemetry.
    /// The result is an independent snapshot equivalent to a fresh scan of the same text.
    pub fn handoff_copy(&self) -> Snapshot {
        Snapshot {
            rope: self.rope.clone(),
            lines: std::sync::Arc::new((*self.lines).clone()),
            prev_col: std::sync::Arc::new((*self.prev_col).clone()),
            open_depth: std::sync::Arc::new((*self.open_depth).clone()),
            frame_log: std::sync::Arc::new((*self.frame_log).clone()),
            block_cache: Some(std::sync::Arc::new(std::sync::OnceLock::new())),
            cache_slot: std::sync::Weak::new(),
            prev_snapshot: std::sync::Weak::new(),
            cell_consumption: std::sync::Arc::new(SnapshotConsumption {
                tree_full: std::sync::atomic::AtomicU64::new(0),
                secs: std::sync::atomic::AtomicU64::new(0),
                cell_query: Default::default(),
                cell_hit: Default::default(),
                cell_built: Default::default(),
            }),
            cells: Some(std::sync::Arc::new(std::sync::OnceLock::new())),
            segment_memo: std::sync::OnceLock::new(),
        }
    }

    // ── Basic accessors (for selector / op) ──

    /// Zero-copy byte range → &str.  Single chunk fast path (borrowed),
    /// multi-chunk fallback via scratch buffer (still no heap alloc).
    #[inline]
    pub(crate) fn byte_range_str<'a>(&'a self, range: std::ops::Range<usize>, scratch: &'a mut Vec<u8>) -> &'a str {
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

    #[inline]
    pub fn byte_len(&self) -> usize {
        self.rope.byte_len()
    }

    #[inline]
    pub fn byte_of_line(&self, line_idx: usize) -> usize {
        self.rope.byte_of_line(line_idx)
    }

    #[inline]
    pub fn line_len(&self) -> usize {
        self.lines.len()
    }

    /// L0 行首深度查询面（depth_col_plan）：行 l 事件前栈深。仅接受
    /// `l < line_len()`；`l == n` 的文档尾残深必须显式走 `eof_open_depth`，
    /// 避免把两个语义混进同一入口。
    #[inline]
    pub fn open_depth_at(&self, l: usize) -> usize {
        assert!(l < self.line_len(), "open_depth_at: l must be < line_len (l={l}); use eof_open_depth for l==n");
        self.open_depth[l] as usize
    }

    /// 文档尾残深哨兵（l==n 的语义），与 `depth_at` 分离。
    #[inline]
    pub fn eof_open_depth(&self) -> usize {
        self.open_depth[self.line_len()] as usize
    }

    /// 向后走到最近的 depth-0 行（取代旧 `infer::snap_to_depth0`，每步 O(1)
    /// 列直读）。line 越界钳到 n；l==n 时用 `eof_open_depth` 判定。
    pub fn snap_to_depth0(&self, line: usize) -> usize {
        let mut l = line.min(self.line_len());
        while l > 0 {
            let d = if l == self.line_len() {
                self.eof_open_depth()
            } else {
                self.open_depth_at(l)
            };
            if d == 0 {
                break;
            }
            l -= 1;
        }
        l
    }

    /// PRC P4b.1: 写线程在发布前注入 CacheSlot 的 Weak；测试也可用它在
    /// 本地 Snapshot 上挂接内核缓存路径。
    pub(crate) fn set_cache_slot(&mut self, slot: std::sync::Weak<crate::cache::CacheSlot>) {
        self.cache_slot = slot;
    }

    /// Phase B: 挂直接前驱（Weak，不 pin），供派生格首次查询懒继承。
    /// 只允许在守卫证明结构全等的 pass-through 分支调用。
    pub(crate) fn set_prev_snapshot(&mut self, prev: &Arc<Snapshot>) {
        self.prev_snapshot = Arc::downgrade(prev);
    }


    /// M0.2 埋点(#54):树格消费计数——零行为,仅累加,生产不读。
    #[inline]
    pub(crate) fn record_tree_full_consumption(&self) {
        self.cell_consumption
            .tree_full
            .fetch_add(1, Ordering::Relaxed);
    }

    /// M0.2 埋点(#54):节区间格消费计数——零行为,仅累加,生产不读。
    #[inline]
    pub(crate) fn record_secs_consumption(&self) {
        self.cell_consumption.secs.fetch_add(1, Ordering::Relaxed);
    }

    /// 节区间格查询(#36a 自写面之一):全量 × 拓扑 heading 维度的双产物
    /// (secs + header_container_bounds),同一次全量 fold 产出。只接受全量段
    /// 物化(身份投影单源律——"派生格只接受全量段物化"的键值契约);段源 =
    /// structure_for_range(0,n) 段格层总入口。键级单构建由 get_or_build_cell
    /// 承担(表 Mutex 内 miss 构建)。
    /// [归位 S1] A3 根表格（`RootTable` / `RootBlocksTable` 两 kind 共用载荷）。
    /// 键的拓扑维度用 `&CONTAINER_TOPOLOGY` 作登记命名空间（A.3：四索引是 CF
    /// 事实或拓扑无关；`TopoTag` 记未决）。本阶段只提供入口，旧 `kernel_*`
    /// 消费面不动（S2/S3 才切换）。
    pub(crate) fn root_table_cell(
        &self,
        split: bool,
    ) -> Arc<Vec<(u32, u32, crate::block::BlockType)>> {
        if split {
            self.root_table_cell_split()
        } else {
            self.root_table_cell_merge()
        }
    }

    /// [S2/G.8-1] 合并口径根表命名出口（域 RootMerge）。
    pub(crate) fn root_table_cell_merge(&self) -> Arc<Vec<(u32, u32, crate::block::BlockType)>> {
        self.root_table_cell_of(DomainId::RootMerge)
    }

    /// [S2/G.8-1] 拆分口径根表命名出口（域 RootSplit）。
    pub(crate) fn root_table_cell_split(&self) -> Arc<Vec<(u32, u32, crate::block::BlockType)>> {
        self.root_table_cell_of(DomainId::RootSplit)
    }

    /// 根表构建内核：两个口径包装共用（域实例 → Build → 表驱动分派）。
    fn root_table_cell_of(
        &self,
        domain: DomainId,
    ) -> Arc<Vec<(u32, u32, crate::block::BlockType)>> {
        let split = matches!(domain, DomainId::RootSplit);
        let v = cell_entry(self, domain, CellPolicy::Build, None, || {
            Arc::new(CellValue::RootTable(crate::cache::build_root_table_cell(self, split)))
        })
        .expect("Build 策略必有值");
        match &*v {
            CellValue::RootTable(v) => v.clone(),
            _ => unreachable!("root table key maps to root table value"),
        }
    }

    /// [复核补记 A-0′] 只读探测根表格（**不建、无副作用**）：供 diag 用。与
    /// `root_table_cell(split)` 同格同口径，仅当格已初始化时返回 true。
    /// 动机：旧写法 `root_count_before(0).is_some()` 会**触发构建**（测量扰动
    /// 被测对象——A-0′ 省掉的新侧根表会被它原地重建，收益被抵消）。
    pub(crate) fn peek_root_table_cell(&self, split: bool) -> bool {
        let domain = if split { DomainId::RootSplit } else { DomainId::RootMerge };
        matches!(
            cell_entry(self, domain, CellPolicy::Peek, None, || unreachable!("Peek 不构建"))
                .as_deref(),
            Some(CellValue::RootTable(_))
        )
    }

    /// [S2 首批] 块跨度格的 `UseIfReady` 臂：继承腿（命中即安装）→ 本代格槽；
    /// 未命中返回 `None`，**不构建**——调用方走自己声明的回退（`block_start_before` = 探针）。
    /// 与 `Peek` 的差别只在继承腿；与 `block_spans_cell`（`Build`）的差别是不构建。
    pub(crate) fn block_spans_cell_if_ready(&self) -> Option<Arc<Vec<(u32, u32)>>> {
        match &*cell_entry(self, DomainId::Span, CellPolicy::UseIfReady, None, || {
            unreachable!("UseIfReady 不构建")
        })? {
            CellValue::BlockSpans(v) => Some(v.clone()),
            _ => None,
        }
    }

    /// [复核补记 A-1] 只读探测块跨度格：**不建、不触发 materialize**。
    /// S2 之后 `block_start_before` 改走 `block_spans_cell_if_ready`（含继承腿），
    /// 本函数保留为 `Peek` 臂的样例与后续诊断位（当前无生产调用）。
    #[allow(dead_code)]
    /// 与 `block_spans_cell` 同格同口径（`CellKind::BlockSpans`），仅当格已
    /// 初始化时返回 Some；调用方未命中即退回原路径（探针 / 表构建）。
    pub(crate) fn peek_block_spans_cell(&self) -> Option<Arc<Vec<(u32, u32)>>> {
        match &*cell_entry(self, DomainId::Span, CellPolicy::Peek, None, || {
            unreachable!("Peek 不构建")
        })? {
            CellValue::BlockSpans(v) => Some(v.clone()),
            _ => None,
        }
    }

    /// [归位 S1] A3-0b 块跨度格。
    pub(crate) fn block_spans_cell(&self) -> Arc<Vec<(u32, u32)>> {
        let v = cell_entry(self, DomainId::Span, CellPolicy::Build, None, || {
            Arc::new(CellValue::BlockSpans(crate::cache::build_block_spans_cell(self)))
        })
        .expect("Build 策略必有值");
        match &*v {
            CellValue::BlockSpans(v) => v.clone(),
            _ => unreachable!("block spans key maps to block spans value"),
        }
    }

    /// 公开只读开口:HS 节区间 (start,end,level,type,depth,in_list), 供 facade 物化。
    pub fn hs_sections_meta(&self) -> Vec<(u32, u32, u8, u8, u8, bool)> {
        let cell = self.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
        cell.secs.iter().zip(cell.heads.iter())
            .map(|(&(s, e, lv, t), h)| (s, e, lv, t, h.depth, h.in_list))
            .collect()
    }

    pub(crate) fn section_cell(
        &self,
        topo: &'static crate::topology::Topology,
    ) -> Arc<SecsCell> {
        self.record_secs_consumption();
        let v = cell_entry(self, DomainId::Sec, CellPolicy::Build, Some(topo), || {
            if let Some(updated) = try_update_secs_from_edit(self, DomainId::Sec, topo) {
                return Arc::new(CellValue::Secs(Arc::new(updated)));
            }
            let n = self.line_len();
            let seg = crate::cache::structure_for_range(self, 0, n);
            // 探查插桩:sec 格构建的细分成本(段构建 = bsp_* 已在
            // structure_for_range 内自计时)——intervals fold 与
            // header_bounds 派生各自成 span,首读成本归因用。
            #[cfg(feature = "telemetry")]
            let _sp_int = crate::telemetry_span!("sec_cell_intervals");
            let heads = crate::infer::heading_meta(
                &seg.blocks,
                &seg.records,
                &seg.arena,
                topo.heading_candidates,
                n,
            );
            let secs_raw = crate::infer::section_intervals_from_heads(&heads, n);
            let secs: Vec<(u32, u32, u8, u8)> = secs_raw
                .into_iter()
                .map(|(s, e, lv, t)| (s as u32, e as u32, lv, t as u8))
                .collect();
            #[cfg(feature = "telemetry")] drop(_sp_int);
            #[cfg(feature = "telemetry")]
            let _sp_hb = crate::telemetry_span!("sec_cell_header_bounds");
            let mut header_bounds: Vec<(u32, u8, u32)> = Vec::new();
            for b in seg.blocks.iter() {
                if crate::block::is_header(b.block_type) && b.container_ref != u32::MAX {
                    let rec = &seg.records[b.container_ref as usize];
                    let bound = rec
                        .push_lines_of(&seg.arena)
                        .find(|l| *l > b.start)
                        .map(|l| l as usize)
                        .unwrap_or_else(|| rec.end_usize().unwrap_or(n));
                    header_bounds.push((b.start, rec.frame.kind() as u8, bound as u32));
                }
            }
            header_bounds.sort_unstable_by_key(|&(s, k, _)| (s, k));
            #[cfg(feature = "telemetry")] drop(_sp_hb);

            // Phase B: HS 根起点（章节壳根 + 节外根块），与
            // incr::root_child_index_hs_from 的根枚举同口径；判官随机全量对拍。
            let mut root_starts: Vec<u32> = Vec::new();
            for &(s, _e, _lv, t) in &secs {
                let bt = if t <= crate::block::BlockType::HtmlElement as u8 {
                    // SAFETY: 构建点只写入合法 BlockType 判别值。
                    unsafe { std::mem::transmute::<u8, crate::block::BlockType>(t) }
                } else {
                    crate::block::BlockType::Paragraph
                };
                let lo = seg.blocks.partition_point(|b| b.start < s);
                let head_ref = seg.blocks[lo..]
                    .iter()
                    .take_while(|b| b.start == s)
                    .find(|b| b.block_type == bt)
                    .map(|b| b.container_ref)
                    .unwrap_or(u32::MAX);
                let nested = secs.iter().any(|&(ss, ee, _, _)| ss < s && s < ee);
                if head_ref == u32::MAX && !nested {
                    root_starts.push(s);
                }
            }
            for r in seg
                .records
                .iter()
                .filter(|r| r.parent == u32::MAX && (r.start as usize) < n)
            {
                let in_section =
                    secs.iter().any(|&(ss, ee, _, _)| ss < r.start && r.start < ee);
                if !in_section {
                    root_starts.push(r.start);
                }
            }
            for b in seg
                .blocks
                .iter()
                .filter(|b| b.container_ref == u32::MAX && (b.start as usize) < n)
            {
                if crate::block::is_header(b.block_type) {
                    continue;
                }
                let in_section =
                    secs.iter().any(|&(ss, ee, _, _)| ss < b.start && b.start < ee);
                if !in_section {
                    root_starts.push(b.start);
                }
            }
            root_starts.sort_unstable();
            Arc::new(CellValue::Secs(Arc::new(SecsCell {
                secs,
                header_bounds,
                root_starts,
                heads,
            })))
        })
        .expect("Build 策略必有值");
        match &*v {
            CellValue::Secs(s) => s.clone(),
            _ => unreachable!("secs key maps to secs value"),
        }
    }

    /// 树格查询(#36a 自写面之一):全量 × 拓扑归属树 + 全量段,现成三步形态
    /// (§4.1,projection.rs render_roots_to 同款):段 Arc 就绪 →
    /// LineView::with_arrays + build_attribution_tree;段未就绪 →
    /// structure_for_range 兜底裸建临时段(不写段格)+ 同三步(树入格)。
    /// 段与树一并入格——渲染层取格后 with_arrays 重建借用视图,零重复构建。
    pub(crate) fn tree_cell(
        &self,
        topo: &'static crate::topology::Topology,
    ) -> Arc<TreeCell> {
        self.record_tree_full_consumption();
        let v = cell_entry(self, DomainId::Tree, CellPolicy::Build, Some(topo), || {
            let n = self.line_len();
            let seg = crate::cache::structure_for_range(self, 0, n);
            let view = crate::infer::LineView::with_arrays(
                &seg.records,
                &seg.blocks,
                &self.frame_log.entries,
                &self.lines,
                &self.rope,
                (seg.start_line as u32, seg.end_line as u32),
                seg.view.clone(),
                &seg.arena,
            );
            let tree = crate::infer::build_attribution_tree(
                &seg.records,
                &seg.blocks,
                &view,
                &seg.arena,
                (0, n as u32),
                topo,
            );
            Arc::new(CellValue::Tree(Arc::new(TreeCell { seg, tree: Arc::new(tree) })))
        })
        .expect("Build 策略必有值");
        match &*v {
            CellValue::Tree(t) => t.clone(),
            _ => unreachable!("tree key maps to tree value"),
        }
    }

    #[inline]
    pub fn line_type_at(&self, line_idx: usize) -> BlockType {
        self.lines[line_idx].node_type()
    }

    #[inline]
    pub fn line_of_byte(&self, byte_idx: usize) -> usize {
        // char-boundary 防御（同 State::edit）——非 boundary 偏移钳制到字符起点
        self.rope.line_of_byte(snap_down_char_boundary(&self.rope, byte_idx))
    }

    #[inline]
    pub fn byte_slice(&self, range: Range<usize>) -> crop::RopeSlice<'_> {
        // char-boundary 防御——两端钳制到字符起点（end<start 时收缩为空）
        let start = snap_down_char_boundary(&self.rope, range.start);
        let mut end = snap_down_char_boundary(&self.rope, range.end);
        if end < start { end = start; }
        self.rope.byte_slice(start..end)
    }

    #[inline]
    pub fn byte_at(&self, idx: usize) -> u8 {
        self.rope.byte(idx)
    }

    #[inline]
    pub fn line_text(&self, line_idx: usize) -> crop::RopeSlice<'_> {
        self.rope.line(line_idx)
    }

    #[inline]
    pub fn frame_boundary_for(&self, start_line: u32, kind: crate::block::FrameKind) -> Option<u32> {
        self.frame_log.item_tail_boundary(start_line, kind, &self.lines)
    }

    #[inline]
    pub fn is_char_boundary(&self, byte: usize) -> bool {
        self.rope.is_char_boundary(byte)
    }

    /// Zero-copy line text access — single chunk fast path.
    #[inline(always)]
    pub fn with_line_str<T>(line: crop::RopeSlice<'_>, f: impl FnOnce(&str) -> T) -> T {
        let mut chunks = line.chunks();
        let first = chunks.next().unwrap_or("");
        if first.len() == line.byte_len() { f(first) } else { f(&line.to_string()) }
    }

    // ── BlockNode materialization ──
    // pub（content_route_testbed 集成测试实验场访问）

    /// 行内级查询面：本行自开帧数（从 FL 派生，O(log E + k)）。
    /// 与旧 row_pushes 整列逐值一致（对拍见 `row_pushes_at_matches_rebuild`）。
    pub fn row_pushes_at(&self, line: usize) -> usize {
        crate::infer::row_pushes_at(&self.frame_log.entries, line)
    }

    /// 行内级（兼容入口）——与 `row_pushes_at` 同源。
    pub fn line_pushes(&self, line: usize) -> usize {
        self.row_pushes_at(line)
    }

    /// A3-1：行 `line` 之前的 CF 根块数（全量口径，与段域无关）。
    /// 索引可用时二分（O(log M)）；无槽快照/链断返回 `None`——调用方回退
    /// `incr::root_child_index`（正确性优先，见 A3 §6 风险 3）。
    /// [路径统一] 索引表 + run-aware 排序（rtb_sort 67→22µs）。
    /// **携带标量版已实现并实测、随后回退**：它在快照链存活时是 O(1)（判官
    /// `root_count_query_is_local` 绿），但网关里旧快照/槽链极短——链断后
    /// `materialize_zone` 退化成全量 (0,n)，携带与表两条路都变 O(n)
    /// （实测 380-780µs/编辑，远超表路径 15.6µs）。留档在 cache.rs。
    pub fn root_count_before(&self, line: usize) -> Option<usize> {
        let tbl = crate::cache::kernel_root_table(self)?;
        Some(tbl.partition_point(|&(s, _, _)| (s as usize) < line))
    }

    /// NodeProj 构造器（node 投影规范实现——Snapshot 薄委托）。
    /// O(N²) 修复：API 路径注入链供给——一次结构构建（memo 缓存）
    /// + 每节点 O(1) chain_frames_at，替代逐节点 rebuild_stack_at 的 O(前缀)。
    pub(crate) fn node_proj(&self) -> crate::infer::NodeProj<'_> {
        let n = self.line_len();
        let seg = crate::cache::structure_for_range(self, 0, n);
        let entries = &self.frame_log.entries;
        let lines = &self.lines;
        let rope = &self.rope;
        crate::infer::NodeProj::with_chain(&self.rope, &self.lines, &self.frame_log.entries, move |line: usize| {
            let view = crate::infer::LineView::with_arrays(
                &seg.records,
                &seg.blocks,
                entries,
                lines,
                rope,
                (seg.start_line as u32, seg.end_line as u32),
                seg.view.clone(),
                &seg.arena,
            );
            crate::effect::ResolveView::chain_frames_at(&view, line).into_iter().collect()
        })
    }

    /// 带链供给的 NodeProj（O(n²) 修复——render 路径注入 view.chain_row 零重放）。
    pub(crate) fn node_proj_with_chain<'a>(
        &'a self,
        chain: &'a dyn Fn(usize) -> smallvec::SmallVec<[crate::block::BlockFrame; 4]>,
    ) -> crate::infer::NodeProj<'a> {
        crate::infer::NodeProj::with_chain(&self.rope, &self.lines, &self.frame_log.entries, move |l| chain(l))
    }

    /// 带链供给的投影读取（render 热路径专用；API 路径仍走 read_block_proj）。
    pub(crate) fn read_block_proj_view<'a>(
        &'a self,
        node: &BlockNode,
        surface: BlockProj,
        chain: &'a dyn Fn(usize) -> smallvec::SmallVec<[crate::block::BlockFrame; 4]>,
    ) -> String {
        self.node_proj_with_chain(chain).read_block_proj(node, surface)
    }

    /// cow 的链供给变体（读面同款接线——GAP 遗留①）：与 read_block_proj_cow
    /// 逐字节一致；唯一差异＝IC Content 走链供给种子（chain_frames_at＝覆盖
    /// 链含本行自开——弹本行自开帧回到严格行前链，与写面 seeded 同语义），
    /// 其余面委托 cow（零差异）。chain 借用与返回 Cow 生命周期解耦（仅本调
    /// 存活——IC 臂结果 Owned 不借链）。
    pub(crate) fn read_block_proj_cow_chain<'a>(
        &'a self,
        node: &'a BlockNode,
        surface: BlockProj,
        scratch: &'a mut Vec<u8>,
        chain: &(dyn Fn(usize) -> smallvec::SmallVec<[crate::block::BlockFrame; 4]> + '_),
    ) -> std::borrow::Cow<'a, str> {
        if surface == BlockProj::Content && node.node_type == BlockType::IndentedCode {
            let mut seed = chain(node.start_line);
            let own = crate::infer::entries_pushes_at(
                &self.frame_log.entries, node.start_line as u32);
            for _ in 0..own {
                seed.pop();
            }
            let mut out = String::with_capacity(
                node.byte_range.end.saturating_sub(node.byte_range.start) + 64);
            self.node_proj().content_code_ic_stream_seeded(node, &seed, |s| out.push_str(s));
            std::borrow::Cow::Owned(out)
        } else {
            self.read_block_proj_cow(node, surface, scratch)
        }
    }

    pub fn build_node(&self, start_line: usize, end_line: usize, node_type: BlockType, loose: bool) -> BlockNode {
        self.node_proj().build_node(start_line, end_line, node_type, loose)
    }

    // ── Block field accessor (selector / op / FFI entry point) ──

    /// Resolve the effective BlockType and block-end line for a given line.
    /// Reads the NT-inference column (resolved_type) unconditionally — C
    /// (Part A0 + Part B) has already written the content classification for
    /// every line (BQ 行内容分类、setext 修正), so no `resolved != t` probe
    /// is needed: resolved_type is the single truth for both walkers.
    /// Handles: setext forward-look (Paragraph → HsHead), HsHead merge,
    /// Paragraph merge, IndentedCode merge.  Other types delegate to
    /// `FrameSpans::end_for`.
    ///
    /// This is the canonical entry point for both walkers so that
    /// F-snapshot and P-patch share identical block classification logic.
    /// 输入消费契约接口（统一 &dyn——&FrameSpans（未迁移消费点）
    /// 与 &RowView（迁移后——记录查询）都实现 ResolveView——输出逐值一致
    /// （row_view.rs 的 assert_row_view_equiv 对照测试护航）。
    pub fn resolve_block_by_line(&self, line: usize, view: &dyn crate::effect::ResolveView) -> (BlockType, usize) {
        crate::infer::resolve_from_cursor(view, line)
    }

    pub fn block_type_name(&self, node: &BlockNode) -> String {
        self.node_proj().block_type_name(node)
    }

    pub fn heading_text(&self, node: &BlockNode) -> String {
        self.node_proj().heading_text(node)
    }

    pub fn list_key(&self, node: &BlockNode) -> Option<String> {
        self.node_proj().list_key(node)
    }

    // ── Block projection reads ──

    /// hb 渲染内容(剥容器前缀后的整块原文——渲染/差分同源入口;
    /// 与读面 Content 投影(content_raw 剥首尾)语义分离)。
    pub fn hb_block_content(&self, node: &BlockNode) -> String {
        self.node_proj().html_block_content(node)
    }

    pub fn read_block_proj(&self, node: &BlockNode, surface: BlockProj) -> String {
        // 测量探针(纯插桩——feature OFF 零开销;read_projection 成本分解用)
        let _sp_rbp = telemetry_span!("read_block_proj", "surface" => format!("{surface:?}"));
        let _t0 = if EXTRACT_DBG.with(|c| *c.borrow()) {
            Some(std::time::Instant::now())
        } else {
            None
        };
        let r = self.node_proj().read_block_proj(node, surface);
        if let Some(t0) = _t0 {
            T_EXTRACT.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
        }
        r
    }

    /// 借用版投影（渲染直写优化——CF 的 cd（顶层 fence——
    /// content_code cols==0 零变换）借用 rope（省 97KB 中间 String——
    /// 渲染分配审计定位 323KB 中间拷贝——提取 0.53ms 块序列化
    /// 最大单项）——借用 scratch buffer（write 期间有效）——其他回退
    /// read_block_proj（String）——输出逐字节一致（content_code fast path
    /// 等价——range_content + cols==0 直返）
    pub(crate) fn block_proj_borrow<'a>(&'a self, node: &'a BlockNode,
                                        surface: BlockProj,
                                        scratch: &'a mut Vec<u8>) -> std::borrow::Cow<'a, str> {
        if surface == BlockProj::Content && node.node_type == BlockType::CodeFenceOpen {
            if node.geometry.is_some() {
                // 借用快路径只服务无容器前缀的顶层 fence（k==0——与 fence_stream
                // 同口径；k>0 回退 face 读走剥层路径）。
                let k = Self::with_line_str(self.rope.line(node.start_line), |l| {
                    crate::block::peel_peeled(l, 0)
                        .layers
                        .iter()
                        .filter(|ly| ly.kind == crate::block::FrameKind::BlockQuote)
                        .count()
                });
                if k == 0 {
                    let raw = self.byte_range_str(node.byte_range.clone(), scratch);
                    if let Some(cr) = crate::block::range_content(node.node_type, raw) {
                        let fence_indent = crate::infer::fence_indent_base(
                            raw.lines().next().map_or("", |l| l));
                        let cols = crate::effect::dedent_cols(
                            crate::effect::dedent_spec_for(
                                crate::effect::DedentScene::FenceContent),
                            fence_indent);
                        if cols == 0 {
                            return std::borrow::Cow::Borrowed(&raw[cr]);
                        }
                    }
                }
            }
        }
        std::borrow::Cow::Owned(self.read_block_proj(node, surface))
    }

    /// 通用零拷贝快路径（P2）：与 read_block_proj 输出逐字节一致，但
    /// 对"变换为 no-op 且单 chunk"的高频形态返回 Borrowed。
    /// 覆盖：Raw 全部 / Paragraph 单行（trim_start 借用）/ CodeFence Content
    /// （block_proj_borrow）。其余一律 Owned(read_block_proj)——行为不变。
    pub(crate) fn read_block_proj_cow<'a>(
        &'a self,
        node: &'a BlockNode,
        surface: BlockProj,
        scratch: &'a mut Vec<u8>,
    ) -> std::borrow::Cow<'a, str> {
        match surface {
            BlockProj::Raw => {
                std::borrow::Cow::Borrowed(self.byte_range_str(node.byte_range.clone(), scratch))
            }
            BlockProj::Tag if node.node_type == BlockType::CodeFenceOpen => {
                // 单行借用 + decode 快路径，与 node_tag 逐字节一致。
                let ls = self.rope.byte_of_line(node.start_line);
                let le = if node.start_line + 1 < self.lines.len() {
                    self.rope.byte_of_line(node.start_line + 1)
                } else {
                    self.rope.byte_len()
                };
                let raw = self.byte_range_str(ls..le, scratch);
                let line = raw.trim_end_matches(['\n', '\r']);
                if let Some(ref geo) = node.geometry {
                    if geo.tag_range.start < geo.tag_range.end {
                        let cleaned = std::str::from_utf8(
                            crate::block::strip_bq(line.as_bytes(), false)).unwrap_or(line);
                        let end = geo.tag_range.end.min(cleaned.len());
                        if geo.tag_range.start < end {
                            crate::inline::decode_link_dest(
                                &cleaned[geo.tag_range.start..end])
                        } else {
                            std::borrow::Cow::Borrowed("")
                        }
                    } else {
                        crate::inline::decode_link_dest(
                            crate::block::extract_fence_lang(line))
                    }
                } else {
                    crate::inline::decode_link_dest(
                        crate::block::extract_fence_lang(line))
                }
            }
            BlockProj::Content if node.node_type == BlockType::CodeFenceOpen => {
                self.block_proj_borrow(node, surface, scratch)
            }
            BlockProj::Content if crate::block::is_header(node.node_type)
                && node.end_line <= node.start_line + 1 =>
            {
                // 单行 header 的 heading_body 恒空。
                std::borrow::Cow::Borrowed("")
            }
            BlockProj::Content if crate::effect::block_decl(node.node_type).is_list_item
                && node.geometry.is_some() =>
            {
                // 单行 item：与 list_content 几何快路径逐字节一致。
                let geo = node.geometry.as_ref().unwrap();
                let tag_rule = crate::effect::line_spec(node.node_type).tag_rule;
                let line_start = self.rope.byte_of_line(node.start_line);
                let bq_off = node.peel_width;
                let content_start = if matches!(tag_rule,
                    crate::effect::TagRule::ColonKey | crate::effect::TagRule::CheckboxColonKey)
                    && geo.tag_range.start < geo.tag_range.end
                {
                    let pos = line_start + bq_off + geo.tag_range.end;
                    // 边界守卫（写面收口补正 P0-2）：tag 末端恰在行末（- k:/- k
                    // 为末行无尾换行）时 byte(pos)/byte(pos+1) 越界——取 0 与
                    // 行尾语义一致（colon_sep_len(':',0)=1）。
                    let sep = if pos < self.rope.byte_len() { self.rope.byte(pos) } else { 0 };
                    let after = if pos + 1 < self.rope.byte_len() { self.rope.byte(pos + 1) } else { 0 };
                    pos + crate::infer::colon_sep_len(sep, after)
                } else {
                    line_start + bq_off + geo.content_offset
                };
                let raw = self.byte_range_str(content_start..node.byte_range.end, scratch);
                let inner = crate::effect::item_inner_layers(
                    self.line_pushes(node.start_line), true);
                match raw.find('\n') {
                    Some(p) if p + 1 == raw.len() => {
                        if matches!(tag_rule,
                            crate::effect::TagRule::ColonKey
                            | crate::effect::TagRule::CheckboxColonKey)
                        {
                            std::borrow::Cow::Borrowed(
                                crate::block::peel_markers(raw[..p].trim_end(), inner))
                        } else {
                            std::borrow::Cow::Borrowed(
                                crate::block::peel_markers(raw, inner))
                        }
                    }
                    None => std::borrow::Cow::Borrowed(
                        crate::block::peel_markers(raw, inner)),
                    _ => std::borrow::Cow::Owned(self.read_block_proj(node, surface)),
                }
            }
            BlockProj::Content if node.node_type == BlockType::Paragraph
                && node.peel_width == 0 =>
            {
                let raw = self.byte_range_str(node.byte_range.clone(), scratch);
                if raw.as_bytes().iter().any(|&b| b == b'>') {
                    // 8.2：(All,Chain) ctx 折叠（chain=行前栈、条目供数）。
                    // O(n²) 修复：本路径无 view，栈 rebuild_stack_at 一次（与旧价同）。
                    std::borrow::Cow::Owned(
                        crate::infer::paragraph_text_normalize(
                            &crate::infer::paragraph_lines_stripped(
                                &raw, &self.frame_log.entries, node.start_line as u32,
                                &crate::block::rebuild_stack_at(
                                    &self.frame_log.entries, node.start_line as u32 + 1))))
                } else if !raw.contains('\n') {
                    std::borrow::Cow::Borrowed(raw.trim_start())
                } else {
                    std::borrow::Cow::Owned(
                        crate::infer::paragraph_text_normalize(raw))
                }
            }
            _ => std::borrow::Cow::Owned(self.read_block_proj(node, surface)),
        }
    }

    pub(crate) fn tag_eq(&self, node: &BlockNode, target: &str) -> bool {
        self.node_proj().tag_eq(node, target)
    }

    pub fn inline_segments(&self, node: &BlockNode) -> Vec<InlineSegment> {
        self.node_proj().inline_segments(node)
    }

    /// Debug / verification: depth-aware block tree walker.
    ///
    /// Produces a nested JSON array. Structure: `["type", "text", [children]]`.
    /// Container open/close follows `FrameSpans` depth changes exactly.
    /// No inline parsing — line text is included raw.
    pub fn debug_block_tree(&self) -> String {
        if self.lines.is_empty() { return "[]".to_string(); }

        // debug walker 用组合原子 + LineView（消费面仅
        // depth_at / node_type_at_depth 两原语——行尾深度语义等价）。
        let n = self.lines.len() as u32;
        let (recs, blocks, va, arena) = crate::infer::build_structure_parts(
            &self.frame_log.entries, &self.lines, &self.open_depth, &self.rope, (0, n));
        let spans = crate::infer::LineView::with_arrays(&recs, &blocks,
            &self.frame_log.entries, &self.lines, &self.rope, (0, n), va, &arena);

        // ── build tree using a depth stack ──
        // Each level is (container_type, children: Vec<TreeNode>)
        #[derive(Debug, Clone)]
        struct TreeNode {
            block_type: BlockType,
            text: String,
            children: Vec<TreeNode>,
        }
        let mut level_stack: Vec<(BlockType, Vec<TreeNode>)> = Vec::new();
        level_stack.push((BlockType::Paragraph, Vec::new())); // root

        let mut i = 0usize;
        let mut prev_depth = 0usize;
        let max_iter = self.lines.len().saturating_mul(4).max(256);
        let mut iter_guard = 0usize;
        while i < self.lines.len() {
            iter_guard += 1;
            if iter_guard > max_iter {
                // Safety valve: should never trigger in normal operation.
                // If it does, the depth/frame data is inconsistent.
                break;
            }
            let t = spans.resolved_type_at(i);
            let cur_depth = spans.close_depth_at(i);

            // open containers
            while cur_depth > prev_depth {
                let ct = spans.node_type_at_depth(i, prev_depth);
                level_stack.push((ct, Vec::new()));
                prev_depth += 1;
            }
            // close containers
            while cur_depth < prev_depth && level_stack.len() > 1 {
                let (ct, children) = level_stack.pop().unwrap();
                let parent = level_stack.last_mut().unwrap();
                let display_text = children.first()
                    .map(|n: &TreeNode| n.text.clone())
                    .unwrap_or_default();
                parent.1.push(TreeNode { block_type: ct, text: display_text, children });
                prev_depth -= 1;
            }
            // skip invisible lines
            if crate::effect::meta_is_skip(t) { i += 1; continue; }
            // collect text
            let mut text = String::new();
            let block_end = if t == BlockType::Paragraph {
                let mut e = i + 1;
                while e < self.lines.len() && self.lines[e].node_type() == BlockType::Paragraph { e += 1; }
                e
            } else { i + 1 };
            for li in i..block_end.min(self.lines.len()) {
                Snapshot::with_line_str(self.rope.line(li), |s| {
                    if li > i { text.push('\n'); }
                    text.push_str(s);
                });
            }
            level_stack.last_mut().unwrap().1.push(TreeNode { block_type: t, text, children: Vec::new() });
            i = if t == BlockType::Paragraph { block_end } else { i + 1 };
        }
        // close remaining
        while level_stack.len() > 1 {
            let (ct, children) = level_stack.pop().unwrap();
            let parent = level_stack.last_mut().unwrap();
            let display_text = children.first().map(|n: &TreeNode| n.text.clone()).unwrap_or_default();
            parent.1.push(TreeNode { block_type: ct, text: display_text, children });
        }
        let root_nodes = level_stack.pop().unwrap().1;

        // ── serialize ──
        fn serialize_nodes(bw: &mut BufWriter, nodes: &[TreeNode]) {
            unsafe { bw.push(b'['); }
            let mut first = true;
            for n in nodes {
                if !first { unsafe { bw.push(b','); } }
                first = false;
                unsafe {
                    bw.push(b'[');
                    bw.push_slice(ast_block_type_bytes(n.block_type));
                    bw.push(b',');
                    bw.push_json(&n.text);
                }
                if !n.children.is_empty() {
                    unsafe { bw.push(b','); }
                    serialize_nodes(bw, &n.children);
                }
                unsafe { bw.push(b']'); }
            }
            unsafe { bw.push(b']'); }
        }
        let mut buf: Vec<u8> = Vec::with_capacity(4096);
        let mut bw = BufWriter::new(&mut buf);
        serialize_nodes(&mut bw, &root_nodes);
        unsafe { bw.commit(); String::from_utf8_unchecked(buf) }
    }

    pub fn inline_source(&self, node: &BlockNode) -> String {
        self.node_proj().inline_source(node)
    }

    /// 测试探针专用薄委托（production 渲染走 `NodeProj::read_block_proj`）。
    #[cfg(test)]
    pub(crate) fn node_content(&self, node: &BlockNode) -> String {
        self.node_proj().node_content(node)
    }

    pub fn find_title_end(&self, range_start: usize, node_type: BlockType) -> usize {
        self.node_proj().find_title_end(range_start, node_type)
    }

    // ── Block projection writes（全部薄委托 NodeProj——写面单源）──

    pub(crate) fn tag_write_range(&self, range: &std::ops::Range<usize>, node_type: BlockType) -> Result<std::ops::Range<usize>, String> {
        self.node_proj().tag_write_range(range, node_type)
    }

    pub fn block_write_range(&self, target: &BlockWriteTarget, surface: BlockProj) -> Result<std::ops::Range<usize>, String> {
        self.node_proj().block_write_range(target, surface)
    }

    pub fn proj_write_range(&self, range: &std::ops::Range<usize>, node_type: BlockType, proj: &str) -> Result<std::ops::Range<usize>, String> {
        self.node_proj().proj_write_range(range, node_type, proj)
    }

    /// Legacy entry point.  Delegates to NodeProj（geometry: None → full fallback path）。
    pub fn compute_write(
        &self, node_range: &std::ops::Range<usize>, node_type: BlockType,
        proj: &str, payload: &str,
    ) -> Result<(std::ops::Range<usize>, String), String> {
        self.node_proj().compute_write(node_range, node_type, proj, payload)
    }

    /// Accepts full BlockNode.  Geometry present → O(1) write range; absent → fallback。
    pub fn compute_write_from_node(
        &self, node: &BlockNode, proj: &str, payload: &str,
    ) -> Result<(std::ops::Range<usize>, String), String> {
        self.node_proj().compute_write_from_node(node, proj, payload)
    }

    /// Fallback payload formatter —— 薄委托 NodeProj（Pipe-content 分支单源）。
    pub(crate) fn transform_payload(
        &self, proj: &str, node_type: BlockType, payload: &str, range: &std::ops::Range<usize>,
    ) -> (String, Option<std::ops::Range<usize>>) {
        self.node_proj().transform_payload(proj, node_type, payload, range)
    }

    pub(crate) fn content_write_range(&self, range: &std::ops::Range<usize>, node_type: BlockType) -> Result<std::ops::Range<usize>, String> {
        self.node_proj().content_write_range(range, node_type)
    }
}


/// classify_ctx 的自由函数实现（呈现通道分类器——CtxClass——组合派生列——
/// FrameSpans 方法/trait 默认实现共用——逻辑单源）。
/// 输入域: 派生列（content_kind/loose/resolved/depth/node_types——词法）——
/// 执行状态（is_item_open——item 打开行的结构真相——消费侧（walk 的帧/栈）
/// 知道——传入——修正 4 边界）。GuardLazy 是 scan 侧分类（执行状态——不经
/// 此函数）。item_depth（层感知进列）: item 打开行时传该 item 的列表帧栈
/// 深度——content_kind per-(line,depth) 直读。


    fn ast_block_type_bytes(t: BlockType) -> &'static [u8] {
        if is_header(t) { return b"\"head\""; }
        match t {
            BlockType::Bullet | BlockType::Checklist => b"\"bullet\"",
            BlockType::OrderedListItem => b"\"ordered_list\"",
            BlockType::Paragraph => b"\"p\"",
            BlockType::Blockquote => b"\"blockquote\"",
            BlockType::CodeFenceOpen | BlockType::IndentedCode => b"\"code_block\"",
            BlockType::Comment => b"\"comment\"",
            BlockType::TableRow => b"\"table_row\"",
            BlockType::ThematicBreak => b"\"thematic_break\"",
            BlockType::HtmlBlock => b"\"html_block\"",
            BlockType::HtmlElement => b"\"html_element\"",
            BlockType::RefDef => b"\"refdef\"",
            _ => b"\"unknown\"",
        }
    }


pub struct State {
    pub rope: Rope,
    pub lines: Arc<Vec<PackedLineInfo>>,
    /// prev 承诺列(与 Snapshot.prev_col 同源——Arc 共享)。
    pub prev_col: Arc<Vec<PackedLineInfo>>,
    /// L0 行首深度列（depth_col_plan）：长度 n+1，depth[l]=行 l 事件前栈深，
    /// depth[n]=文档尾残深哨兵。与 lines/prev_col 同款三截（edit 镜像 splice，
    /// ripple 原地重写）；不参与 conv 判据。
    pub open_depth: Arc<Vec<u8>>,
    pub frame_log: Arc<FrameLog>,
    pub min_dirty_line: Option<usize>,
    max_splice_index: Option<usize>,
    splice_old_len: Option<usize>,
    cached_stack: (u32, Vec<BlockFrame>),
    /// 与 cached_stack 同步缓存的锚深度映射（列表身份——第一个 item 的
    /// threshold）——cached_rebuild 重放恢复，ripple 起点锚正确。
    cached_anchors: Vec<Option<usize>>,
    /// 与 stack/anchors 平行的开行行号栈（HtmlElement close 匹配用；
    /// u32::MAX = 无开行语义）。
    cached_open_lines: Vec<u32>,
    /// ripple merge 期间捕获, 由 snapshot() 调用方通过 take() 提取。
    /// snapshot 后、下次 edit 前必须提取, 否则被下一次 ripple 覆盖。
    pub ripple_diff: Option<RippleDiff>,
    /// B3-1 性能回归：最近一次 ripple 的旧侧折叠总次数（测试断言 ≤ E）。
    #[cfg(test)]
    pub last_ripple_folds: usize,
    /// v3 证据列：cached_rebuild 最近一次调用的折叠条目数（增量/冷启）。
    #[cfg(test)]
    pub last_cached_replay_folds: usize,
}

impl State {
    #[inline(always)]
    fn with_line_bytes<T>(line: crop::RopeSlice<'_>, buf: &mut Vec<u8>, f: impl FnOnce(&[u8]) -> T) -> T {
        let mut chunks = line.chunks();
        if let Some(first) = chunks.next() {
            if first.len() == line.byte_len() { return f(first.as_bytes()); }
            buf.clear();
            buf.extend_from_slice(first.as_bytes());
            for chunk in chunks { buf.extend_from_slice(chunk.as_bytes()); }
            return f(buf.as_slice());
        }
        f(&[])
    }

    /// Build a clean engine state from a handoff-copied Snapshot.
    pub fn from_snapshot(s: &Snapshot) -> Self {
        Self {
            rope: s.rope.clone(),
            lines: s.lines.clone(),
            prev_col: s.prev_col.clone(),
            open_depth: s.open_depth.clone(),
            frame_log: s.frame_log.clone(),
            min_dirty_line: None,
            max_splice_index: None,
            splice_old_len: None,
            cached_stack: (0, Vec::new()),
            cached_anchors: Vec::new(),
            cached_open_lines: Vec::new(),
            ripple_diff: None,
            #[cfg(test)]
            last_ripple_folds: 0,
            #[cfg(test)]
            last_cached_replay_folds: 0,
        }
    }

    pub fn new(text: &str) -> Self {
        // 纯观测插桩（feature OFF 展开为 ()）：parse 段外层——rope 构建＋全量扫描。
        let _sp_state_new = crate::telemetry_span!("sc_state_new");
        let rope = Rope::from(&*sanitize_nul(text));
        let mut state = Self {
            lines: Arc::new(Vec::with_capacity(rope.line_len())),
            prev_col: Arc::new(Vec::with_capacity(rope.line_len())),
            open_depth: Arc::new(Vec::with_capacity(rope.line_len() + 1)),
            rope,
            frame_log: Arc::new(FrameLog::new()),
            min_dirty_line: Some(0),
            max_splice_index: None,
            splice_old_len: None,
            cached_stack: (0, Vec::new()),
            cached_anchors: Vec::new(),
            cached_open_lines: Vec::new(),
            ripple_diff: None,
            #[cfg(test)]
            last_ripple_folds: 0,
            #[cfg(test)]
            last_cached_replay_folds: 0,
        };
        state.rebuild_all();
        state
    }

    pub fn edit(&mut self, start_byte: usize, end_byte: usize, text: &str) {
        // 注入点（测试用）：MDIO_TEST_PANIC=exec 时在这里炸一次。这条路径走的是
        // **请求线程**（exec → write_locator → session → engine → State::edit），
        // 用来验证「Rust panic 跨 pyo3 会把什么一起带下去」。
        #[cfg(test)]
        {
            if crate::testhooks::panic_armed("exec") {
                panic!("MDIO_TEST_PANIC=exec 注入的测试 panic");
            }
        }
        let text = sanitize_nul(text);
        // char-boundary 防御：偏移漂移（字符/字节坐标系混用——CRDT 回环等）时
        // crop 的 replace/line_of_byte 会 panic（worker 线程崩溃——消息静默丢失）。
        // 钳制到字符起点——不崩，偏移最多差一个字符（防御路径可恢复）。
        let start_byte = snap_down_char_boundary(&self.rope, start_byte);
        let mut end_byte = snap_down_char_boundary(&self.rope, end_byte);
        if end_byte < start_byte { end_byte = start_byte; }
        let start_line_idx = self.rope.line_of_byte(start_byte);
        let old_line_count = self.rope.line_len();
        // 旧侧最后受影响行：end_byte 是 snap 后的 char 边界（exclusive），
        // end_byte-1 可能落在多字节字符内部——用 end_byte 本身所在行
        // 保守外扩一行（守卫略大安全，绝不 panic）。
        let old_end_line = if end_byte > start_byte {
            // byte_len()-1 可能落在**最后一个多字节字符内部**（中文结尾的文档）——
            // crop 的 line_of_byte 对非 boundary 偏移直接 panic（worker 线程崩溃、
            // WS handler TaskGroup 连带失败、后续 exec 全部悬挂）。先钳到字符起点。
            let probe = snap_down_char_boundary(
                &self.rope,
                end_byte.min(self.rope.byte_len().saturating_sub(1)),
            );
            self.rope.line_of_byte(probe)
        } else {
            start_line_idx
        };
        let _span_rep = telemetry_span!("edit_replace");
        self.rope.replace(start_byte..end_byte, &*text);
        let _ = _span_rep;
        let _span_spl = telemetry_span!("edit_splice");
        let target_len = self.rope.line_len();
        let edit_end_byte = start_byte + text.len();
        let edit_end_line = self.rope.line_of_byte(edit_end_byte);
        let equal_affected_end = (edit_end_line + 1).min(target_len);
        self.mirror_line_splice(start_line_idx, target_len, equal_affected_end);
        // 收敛闸必须覆盖"被替换字节覆盖的所有行"，而不仅是 splice 插入/
        // 删除点：多行替换 + 行数变化时，仅用 splice_end 会让旧 tail 与
        // 新行字节不再逐行对应，导致提前收敛（fence 反例）。
        // safe_old_side = 旧侧最后受影响行映射到新坐标后的下界。
        {
            let edit_delta = target_len as isize - old_line_count as isize;
            let prev_guard = self.max_splice_index.unwrap_or(0);
            // 前次 guard 是当前坐标系的新行号；本次 edit 的行数变化把它平移。
            // 删除时保留旧值（过大）——保守安全。
            let shifted_prev = (prev_guard as isize + edit_delta.max(0)).max(0) as usize;
            let new_end_line = if edit_end_byte > 0 {
                // 同上：先向下钳到字符起点（末字符多字节时 byte_len()-1 在字符内部）
                let np = snap_down_char_boundary(
                    &self.rope,
                    edit_end_byte.min(self.rope.byte_len().saturating_sub(1)),
                );
                self.rope.line_of_byte(np)
            } else {
                start_line_idx
            };
            let safe_new_side = (new_end_line + 1).min(target_len);
            let safe_old_side = (old_end_line as isize + edit_delta + 1).max(0) as usize;
            self.max_splice_index = Some(shifted_prev.max(safe_new_side).max(safe_old_side));
        }
        let _ = _span_spl;
        self.min_dirty_line = Some(self.min_dirty_line.unwrap_or(usize::MAX).min(start_line_idx));
        // v3: edit 不再清 cached_stack——前缀不变性(I1,tests/unit/prefix_invariance_probe.rs
        // 2000 例)保证 rls 之前 entries 不变;失效裁决推迟到 ripple_lines 的越界检查
        // (cached_line > start_line 才清——回退越过缓存位置)。
    }

    /// 三根行级列的镜像 splice（lines / prev_col / open_depth）。三分支只在
    /// 这一处存在：新增行级列时只需在此函数内补一列，不存在"漏一支"的
    /// 平行代码落点。返回本次 splice 的受影响端（供 max_splice_index 闸）。
    fn mirror_line_splice(
        &mut self,
        start_line_idx: usize,
        target_len: usize,
        equal_affected_end: usize,
    ) -> usize {
        let lines_mut = Arc::make_mut(&mut self.lines);
        // prev 承诺列与 lines 同位置镜像 splice(插入占位
        // / drain / 置无效——三截纪律同构;尾段值随 splice 平移,无需另移)。
        let col_mut = Arc::make_mut(&mut self.prev_col);
        // L0 深度列（长度 n+1，比 lines 多一个尾哨兵）：行区间与 lines 完全
        // 相同的镜像 splice；尾哨兵随 Vec 自身位置保持在末尾，无需单独移动。
        let depth_mut = Arc::make_mut(&mut self.open_depth);
        if self.splice_old_len.is_none() {
            self.splice_old_len = Some(lines_mut.len());
        }

        let guard = match lines_mut.len().cmp(&target_len) {
            std::cmp::Ordering::Less => {
                let diff = target_len - lines_mut.len();
                let insert_pos = start_line_idx.min(lines_mut.len());
                lines_mut.splice(insert_pos..insert_pos, std::iter::repeat(PackedLineInfo::invalid()).take(diff));
                col_mut.splice(insert_pos..insert_pos, std::iter::repeat(PackedLineInfo::invalid()).take(diff));
                depth_mut.splice(insert_pos..insert_pos, std::iter::repeat(u8::MAX).take(diff));
                insert_pos + diff
            },
            std::cmp::Ordering::Greater => {
                let diff = lines_mut.len() - target_len;
                let drain_end = (start_line_idx + diff).min(lines_mut.len());
                if start_line_idx < lines_mut.len() {
                    lines_mut.drain(start_line_idx..drain_end);
                    col_mut.drain(start_line_idx..drain_end);
                    depth_mut.drain(start_line_idx..drain_end);
                }
                start_line_idx
            },
            std::cmp::Ordering::Equal => {
                let affected_end = equal_affected_end.min(lines_mut.len());
                for i in start_line_idx..affected_end {
                    lines_mut[i] = PackedLineInfo::invalid();
                    col_mut[i] = PackedLineInfo::invalid();
                    depth_mut[i] = u8::MAX;
                }
                affected_end
            }
        };
        self.max_splice_index = Some(self.max_splice_index.unwrap_or(0).max(guard));
        guard
    }

    #[inline]
    fn invalidate_cache(&mut self) {
        self.cached_stack = (0, Vec::new());
        self.cached_anchors.clear();
        self.cached_open_lines.clear();
    }

    #[inline]
    fn cached_rebuild(&mut self, log: &[u64], target: u32) -> (Vec<BlockFrame>, Vec<Option<usize>>, Vec<u32>) {
        let cached_line = self.cached_stack.0;
        if target >= cached_line {
            let start = if cached_line == 0 { 0 } else {
                log.partition_point(|e| entry_line(*e) < cached_line)
            };
            let end = log.partition_point(|e| entry_line(*e) < target);
            crate::block::forward_replay_runtime(&log[start..end], &mut self.cached_stack.1, &mut self.cached_anchors, &mut self.cached_open_lines);
            #[cfg(test)]
            { self.last_cached_replay_folds += end - start; }
        } else {
            let end = log.partition_point(|e| entry_line(*e) < target);
            self.cached_stack.1.clear();
            self.cached_anchors.clear();
            self.cached_open_lines.clear();
            crate::block::forward_replay_runtime(&log[..end], &mut self.cached_stack.1, &mut self.cached_anchors, &mut self.cached_open_lines);
            #[cfg(test)]
            { self.last_cached_replay_folds += end; }
        }
        self.cached_stack.0 = target;
        (self.cached_stack.1.clone(), self.cached_anchors.clone(), self.cached_open_lines.clone())
    }

    /// 统一扫描循环：rebuild_all / ripple_lines 共用。
    /// - `lines` 全量模式为空 Vec（逐行 push），增量模式为已有 Vec（原地写）。
    /// - `sink` 接收 FL actions 并持有 open_lines。
    /// - `should_stop` 在“当前行已提交到 sink”之后调用；返回 true 时停止，返回停止行号。
    fn scan_range<'a, S: ScanSink>(
        rope: &'a Rope,
        lines: &mut Vec<PackedLineInfo>,
        col: &mut Vec<PackedLineInfo>,
        depth: &mut Vec<u8>,
        start_line: usize,
        end_line: usize,
        mut state: ScanState,
        sink: &mut S,
        mut seq_lines: Option<crop::iter::Lines<'a>>,
        mut should_stop: impl FnMut(
            usize,
            PackedLineInfo,
            PackedLineInfo,
            &[u64],
            &ScanState,
            &[u32],
        ) -> bool,
    ) -> usize {
        // 纯观测插桩：扫描区间（parse/ripple 共用同一实现——两个场景都能看到）。
        let _sp_scan_range = crate::telemetry_span!("sc_range");
        let mut curr_line = start_line;
        let mut line_actions_start = 0usize;

        SCRATCH_BUFFER.with(|cell| {
            let mut buf = cell.borrow_mut();
            while curr_line < end_line {
                // L0 深度列：写行首栈深（scan_block_rt 循环之前——上一行行尾栈
                // 即本行行首栈）。禁止使用 prev_semantic_depth（行尾语义深度）。
                if curr_line < depth.len() {
                    depth[curr_line] = state.stack.len() as u8;
                } else {
                    depth.push(state.stack.len() as u8);
                }
                // [perf 0830] 全量顺序路径(rebuild_all)用 Lines 迭代器——一次树遍历
                // amortized O(1)/行 vs rope.line(i) 每次树下降(probe 实测 102→24 ns/行, 4.24x)
                let line_slice = {
                    // 计数器 span（MDIO_TELE_DEEP=1 才开）：每行取行的成本。
                    #[cfg(feature = "telemetry")]
                    let _g_deep = crate::telemetry::deep_span("sc_line_get");
                    if let Some(it) = seq_lines.as_mut() {
                        match it.next() {
                            Some(s) => s,
                            None => break,
                        }
                    } else {
                        rope.line(curr_line)
                    }
                };
                let li_u32 = curr_line as u32;

                // prev 承诺列直读(上一行写入的承诺——
                // 前缀照抄恒有效;行内 Pop 重扫的 prev 归零保持)。
                let mut prev_line_type = if curr_line > 0 {
                    let p = col[curr_line - 1];
                    if p.is_valid() { Some(p.node_type()) } else { None }
                } else { None };

                let (ft, new_packed) = Self::with_line_bytes(line_slice, &mut buf, |bytes| {
                    let mut ft = BlockType::Paragraph;
                    let mut is_block_start = false;
                    let mut pop_guard = 0u8;
                    loop {
                        pop_guard += 1;
                        if pop_guard > MAX_BLOCK_NESTING as u8 { break; }
                        // 下层前缀匹配前置——
                        // 失配（某个下层 BQ 帧的前缀不匹配）→ 关该帧及其上方帧 →
                        // 重扫本行。替换 peel_lower_prefixes 的旧静默 break 语义。
                        {
                            let facts = crate::block::LineFacts::from(bytes);
                            if !facts.is_blank {
                                // prev_blank 关链——空行已结束
                                // BQ 链（CM），下一行（任意内容）的下层 BQ 帧全关。
                                // prev_line_type None = Blank 归一化（doc 首无栈、find None 安全）。
                                // 只认真实空行（前一行文本全空白——词法事实，
                                // 不靠 prev 类型/信号——doc 首与 pop 重扫天然不触发）
                                if curr_line > 0 && rope.line(curr_line - 1).chunks()
                                    .all(|c| c.is_empty() || c.chars().all(|ch| ch.is_whitespace())) {
                                    if let Some(bq_idx) = (0..state.stack.len().saturating_sub(1))
                                        .find(|&i| state.stack[i].kind() == FrameKind::BlockQuote)
                                    {
                                        while state.stack.len() > bq_idx {
                                            if let Some(popped) = state.stack.last().copied() {
                                                sink.push_action(pop_entry(li_u32, popped));
                                            }
                                            state.stack.pop();
                                            state.anchors.pop();
                                            sink.on_frame_pop();
                                        }
                                        prev_line_type = None;
                                        continue;
                                    }
                                }
                                // 部分前缀结构行才关帧重扫（与 classify_bq_scene 的
                                // interrupt 同一判官——bq_lower_interrupt）：列表
                                // marker/标题等结构块打断；段落文本 lazy 不关。
                                if crate::block::bq_lower_interrupt(&facts, &state.stack) {
                                    if let crate::block::LowerPrefixMatch::Fail { frame_index } =
                                        crate::block::peel_lower_prefixes_from(
                                            facts.content, &state.stack, facts.indent_col)
                                    {
                                        while state.stack.len() > frame_index {
                                            if let Some(popped) = state.stack.last().copied() {
                                                sink.push_action(pop_entry(li_u32, popped));
                                            }
                                            state.stack.pop();
                                            state.anchors.pop();
                                            sink.on_frame_pop();
                                        }
                                        prev_line_type = None;
                                        continue;
                                    }
                                }
                            }
                        }
                        let (t, a) = {
                            // 计数器 span（deep 才开）：每行判定器成本。
                            #[cfg(feature = "telemetry")]
                            let _g_deep = crate::telemetry::deep_span("sc_line_block");
                            crate::block::scan_block_rt(
                                bytes, &state.stack, sink.open_lines(), Some(rope), prev_line_type)
                        };
                        // 「内容关系」改由**行事实**推断（E1 实验）：同一条规则算在看得见
                        // 行事实的层，供三面共用（规则共享、状态各持）——与原先内联的
                        // `strip_bq` 判定等价，改由 `LineFacts::content_rel` 承载。
                        let content_empty = t == BlockType::Blockquote
                            && {
                                #[cfg(feature = "telemetry")]
                                let _g_deep2 = crate::telemetry::deep_span("sc_facts2");
                                crate::block::LineFacts::from(bytes).content_rel()
                            }
                                == crate::block::ContentRel::ContainerOnly;
                        state.prev_hint = match crate::block::line_effect_from_action(t, &a, content_empty).prev_override {
                            crate::block::PrevOverride::Default => None,
                            crate::block::PrevOverride::Content(x) => Some(PrevHint::Content(x)),
                            crate::block::PrevOverride::Blank => Some(PrevHint::Blank),
                        };
                        ft = t;
                        match a {
                            ScanAction::Pop => {
                                if let Some(popped) = state.stack.last() {
                                    sink.push_action(pop_entry(li_u32, *popped));
                                }
                                state.stack.pop();
                                state.anchors.pop();
                                sink.on_frame_pop();
                                if t != BlockType::Paragraph && t != BlockType::Blank {
                                    break;
                                }
                                prev_line_type = None;
                                continue;
                            }
                            ScanAction::Push(f) => {
                                is_block_start = true;
                                resolve_pending_loose(li_u32, &mut state.pending_loose, &mut state.stack, sink);
                                if state.stack.len() >= MAX_BLOCK_NESTING { break; }
                                let a = anchor_for(&f);
                                state.anchors.push(a);
                                sink.on_frame_push(f, li_u32);
                                sink.push_action(crate::block::with_consume(
                                    push_list_entry(li_u32, f, anchor_enc(a)),
                                    crate::block::layer_consume_of(bytes)));
                                state.stack.push(f);
                                break;
                            }
                            ScanAction::PopPush(f) => {
                                is_block_start = true;
                                let popped_frame = state.stack.last().copied();
                                if list_end_guard(&f, &state.anchors, popped_frame.map(|p| p.kind())) {
                                    if !crate::block::is_empty_item_sentinel(f.indent()) && prev_line_type.is_some_and(|t|
                                        !crate::effect::block_decl(t).closes_paragraph)
                                    {
                                        is_block_start = false;
                                        ft = BlockType::Paragraph;
                                        break;
                                    }
                                    if let Some(popped) = popped_frame {
                                        sink.push_action(pop_entry(li_u32, popped));
                                    }
                                    state.stack.pop();
                                    state.anchors.pop();
                                    sink.on_frame_pop();
                                    prev_line_type = None;
                                    continue;
                                }
                                if let Some(popped) = popped_frame {
                                    sink.push_action(pop_entry(li_u32, popped));
                                }
                                let popped_kind = popped_frame.map(|p| p.kind());
                                state.stack.pop();
                                sink.on_frame_pop();
                                if state.stack.len() >= MAX_BLOCK_NESTING { break; }
                                let prev_anchor = state.anchors.pop().flatten();
                                let a = next_list_anchor(&f, popped_kind, prev_anchor);
                                state.anchors.push(a);
                                sink.on_frame_push(f, li_u32);
                                sink.push_action(crate::block::with_consume(
                                    push_list_entry(li_u32, f, anchor_enc(a)),
                                    crate::block::layer_consume_of(bytes)));
                                state.stack.push(f);
                                break;
                            }
                            ScanAction::ContinueAs(f) => {
                                if let Some(top) = state.stack.last_mut() {
                                    *top = f;
                                }
                                sink.push_action(body_mut_entry(li_u32, f));
                                break;
                            }
                            ScanAction::Continue => {
                                if ft == BlockType::Blank {
                                    for (d, top) in state.stack.iter().enumerate() {
                                        if crate::effect::decl_for(top.kind()).has_loose {
                                            state.pending_loose.push((d, top.kind()));
                                        }
                                    }
                                }
                                break;
                            }
                            ScanAction::Composite { outer_type, primary, nested, nested_consumes, primary_consume } => {
                                ft = outer_type;
                                if matches!(primary, CompositePrimary::Push(_) | CompositePrimary::PopPush(_))
                                    || !nested.is_empty()
                                {
                                    is_block_start = true;
                                }
                                match primary {
                                    CompositePrimary::Push(f) => {
                                        resolve_pending_loose(li_u32, &mut state.pending_loose, &mut state.stack, sink);
                                        if state.stack.len() >= MAX_BLOCK_NESTING { break; }
                                        let a = anchor_for(&f);
                                        state.anchors.push(a);
                                        sink.on_frame_push(f, li_u32);
                                        sink.push_action(crate::block::with_consume(
                                            push_list_entry(li_u32, f, anchor_enc(a)), primary_consume));
                                        state.stack.push(f);
                                    }
                                    CompositePrimary::ContinueAs(f) => {
                                        if let Some(top) = state.stack.last_mut() { *top = f; }
                                        sink.push_action(body_mut_entry(li_u32, f));
                                    }
                                    CompositePrimary::PopPush(f) => {
                                        let popped_frame = state.stack.last().copied();
                                        if list_end_guard(&f, &state.anchors, popped_frame.map(|p| p.kind())) {
                                            if !crate::block::is_empty_item_sentinel(f.indent()) && prev_line_type.is_some_and(|t|
                                                !crate::effect::block_decl(t).closes_paragraph)
                                            {
                                                is_block_start = false;
                                                ft = BlockType::Paragraph;
                                                break;
                                            }
                                            if let Some(popped) = popped_frame {
                                                sink.push_action(pop_entry(li_u32, popped));
                                            }
                                            state.stack.pop();
                                            state.anchors.pop();
                                            sink.on_frame_pop();
                                            continue;
                                        }
                                        if let Some(popped) = popped_frame {
                                            sink.push_action(pop_entry(li_u32, popped));
                                        }
                                        let popped_kind = popped_frame.map(|p| p.kind());
                                        state.stack.pop();
                                        sink.on_frame_pop();
                                        if state.stack.len() >= MAX_BLOCK_NESTING { break; }
                                        let prev_anchor = state.anchors.pop().flatten();
                                        let a = next_list_anchor(&f, popped_kind, prev_anchor);
                                        state.anchors.push(a);
                                        sink.on_frame_push(f, li_u32);
                                        sink.push_action(crate::block::with_consume(
                                            push_list_entry(li_u32, f, anchor_enc(a)), primary_consume));
                                        state.stack.push(f);
                                    }
                                    CompositePrimary::Pop => {
                                        if let Some(popped) = state.stack.last() {
                                            sink.push_action(pop_entry(li_u32, *popped));
                                        }
                                        state.stack.pop();
                                        state.anchors.pop();
                                        sink.on_frame_pop();
                                    }
                                    CompositePrimary::Continue => {
                                        if ft == BlockType::Blank {
                                            for (d, top) in state.stack.iter().enumerate() {
                                                if crate::effect::decl_for(top.kind()).has_loose {
                                                    state.pending_loose.push((d, top.kind()));
                                                }
                                            }
                                        }
                                    }
                                }
                                for (nf, c) in nested.into_iter().zip(nested_consumes) {
                                    if state.stack.len() >= MAX_BLOCK_NESTING { break; }
                                    let a = anchor_for(&nf);
                                    state.anchors.push(a);
                                    sink.on_frame_push(nf, li_u32);
                                    sink.push_action(crate::block::with_consume(
                                        push_list_entry(li_u32, nf, anchor_enc(a)), c));
                                    state.stack.push(nf);
                                }
                                break;
                            }
                        }
                    }
                    if ft != BlockType::Blank {
                        resolve_pending_loose(li_u32, &mut state.pending_loose, &mut state.stack, sink);
                    }
                    let decl = &crate::effect::BLOCK_TYPE_DECLS[ft as usize];
                    let prev = if decl.blank_policy == crate::effect::BlankPolicy::Allow {
                        crate::infer::lookback_skipping_blanks(curr_line, lines.as_slice())
                    } else {
                        prev_line_type
                    };
                    let is_pop_line = state.prev_semantic_depth > state.stack.len();
                    let cont = decl.merges && (prev == Some(ft)) && !is_block_start && !is_pop_line;
                    (ft, PackedLineInfo::pack_with_cont(ft, cont))
                });

                let old_packed = lines.get(curr_line).copied().unwrap_or(PackedLineInfo::invalid());
                if curr_line < lines.len() {
                    lines[curr_line] = new_packed;
                } else {
                    lines.push(new_packed);
                }
                // 写完 packed 后写承诺(本行对下一行的承诺 =
                // resolve(prev_hint, packed)——与旧回退语义同式)。
                let promise = promise_of(state.prev_hint, new_packed);
                // (d)：**内容为空的引用行**（如裸 `>`）对下一行的承诺 = invalid——与空白行
                // 同构。依据：`LineFacts` 的 prev 语义是「前一个**非空行**的类型」
                // （block.rs:1059），而 prev 供给处（本文件 :2620）以 valid／invalid 区分
                // Some／None。否则裸 `>` 行以 Blockquote 的有效列喂给下一行，令
                // prev_not_closes 成立、把下一行的缩进码降级为 Paragraph——反例 (d)：
                // `>` 与 `>     B` 在 CM 下应是一个 ic（内容 `B`）。收窄影响面：仅当
                // `ft == Blockquote` 才做这一步；Blank 行走原路径；`> > > > deep` 与
                // `- >   x` 两族内容非空 ⇒ 列仍有效 ⇒ 契约不受影响。

                // 注：该事实由 `line_effect_from_action` 的形参承担，PL 的 bit6 为空闲位。
                if curr_line < col.len() {
                    col[curr_line] = promise;
                } else {
                    col.push(promise);
                }
                let line_actions = &sink.actions()[line_actions_start..];
                if should_stop(curr_line, old_packed, new_packed, line_actions, &state, sink.open_lines()) {
                    break;
                }
                line_actions_start = sink.actions().len();
                if ft != BlockType::Blank {
                    state.prev_semantic_depth = state.stack.len();
                }
                curr_line += 1;
            }
            // L0 深度列尾哨兵：循环自然走到 end_line（未提前 conv 停止）时写
            // 文档尾残深；提前停止不动哨兵（suffix 结构未变，edit 时旧哨兵已随
            // splice 保留）。
            if curr_line >= end_line {
                let tail_depth = state.stack.len() as u8;
                if depth.len() <= end_line {
                    depth.push(tail_depth);
                } else {
                    depth[end_line] = tail_depth;
                }
            }
        });

        curr_line
    }

    fn rebuild_all(&mut self) {
        // 纯观测插桩：全量重建（parse 段主体）。
        let _sp_rebuild = crate::telemetry_span!("sc_rebuild");
        let mut lines: Vec<PackedLineInfo> = Vec::with_capacity(self.rope.line_len());
        let mut col: Vec<PackedLineInfo> = Vec::with_capacity(self.rope.line_len());
        let mut depth: Vec<u8> = Vec::with_capacity(self.rope.line_len() + 1);
        let mut frame_log = FrameLog::new();
        let mut sink = VecSink {
            actions: &mut frame_log.entries,
            open_lines: Vec::new(),
        };
        let state = ScanState::new();
        let _ = Self::scan_range(
            &self.rope,
            &mut lines,
            &mut col,
            &mut depth,
            0,
            self.rope.line_len(),
            state,
            &mut sink,
            Some(self.rope.lines()),
            |_, _, _, _, _, _| false,
        );

        self.frame_log = Arc::new(frame_log);
        self.lines = Arc::new(lines);
        self.prev_col = Arc::new(col);
        self.open_depth = Arc::new(depth);
        self.min_dirty_line = None;
    }

    fn ripple_lines(&mut self) {
        let _span_rp = telemetry_span!("ripple_total");
        let start_line_idx = match self.min_dirty_line {
            Some(l) => l,
            None => return,
        };
        self.min_dirty_line = None;
        // 起点 = min_dirty 直读——prev_semantic_boundary 回退路径由承诺列替代。
        // v3 条件失效:start_line 越过缓存位置才清(cached_line > start_line)。
        // 否则前缀不变性(I1)保证缓存栈对当前旧日志仍有效——增量续放。
        if (self.cached_stack.0 as usize) > start_line_idx {
            self.invalidate_cache();
        }
        #[cfg(test)]
        { self.last_cached_replay_folds = 0; }
        let total = self.rope.line_len();

        let old_line_count = self.splice_old_len.unwrap_or(total);
        let delta = total as isize - old_line_count as isize;
        let had_splice = self.max_splice_index.is_some();

        let original_fl = self.frame_log.clone();

        // line 字段掩码（entry 逐条对账时忽略行号，行号经 mapped_old 单独对齐）。
        const LINE_FIELD_MASK: u64 = ((1u64 << 38) - 1) << 24;

        // 旧侧单调重放游标（conv 四账对账的 O(E) 摊销来源）。
        // v2: shadow 起点移到旧侧首映射 anchor = start_line − delta(钳 [0, old_n−1]),
        //     初始三轴 = 前缀重放到 anchor 的状态——与从 entries[0] 全程重放逐值等价
        //    (fold 纯左折叠、状态是前缀函数;conv 六条件只消费 advance_to 输出,不变)。
        // v3: 三轴复用 cached_rebuild(同一 fold 机械)——跨事务温存时增量续放。
        //     anchor ≤ start_line(插入/内容)时先 anchor 后 start_line(cached_rebuild
        //     目标单调,缓存位置结束于 start_line = 前缀边界);
        //     anchor > start_line(删除)时 shadow 独立重放——不污染缓存位置
        //    (缓存越过前缀边界会在下一事务复用到 ripple 区旧栈)。
        let anchor = ((start_line_idx as isize - delta).max(0) as usize)
            .min(old_line_count.saturating_sub(1)) as u32;
        #[cfg(feature = "telemetry")] let _sp_rb = crate::telemetry_span!("rp_rebuild");
        let (mut stack, mut anchors, mut open_lines, sh_state) =
            if (anchor as usize) <= start_line_idx {
                let (ss, sa, so) = self.cached_rebuild(&original_fl.entries, anchor);
                let sh = (ss, sa, so);
                let (s2, a2, o2) = self.cached_rebuild(&original_fl.entries, start_line_idx as u32);
                (s2, a2, o2, sh)
            } else {
                let (s2, a2, o2) = self.cached_rebuild(&original_fl.entries, start_line_idx as u32);
                // 独立重放到 anchor(不进缓存——见上)
                let mut ss = Vec::new();
                let mut sa = Vec::new();
                let mut so = Vec::new();
                crate::block::forward_replay_runtime(
                    &original_fl.entries[..original_fl.prefix_len(anchor)],
                    &mut ss, &mut sa, &mut so);
                (s2, a2, o2, (ss, sa, so))
            };
        let mut shadow = ShadowReplay::with_state(
            &original_fl.entries,
            original_fl.prefix_len(anchor),
            sh_state.0, sh_state.1, sh_state.2,
        );
        #[cfg(feature = "telemetry")] drop(_sp_rb);
        let mut ripple_frame_actions: Vec<u64> = Vec::with_capacity(32);
        // 收敛印证：上一行是否 conv（行 packed 同 + 栈同）。
        // 初始 true——编辑点前的行未变（传播不可能从编辑点前开始）——
        // 编辑行恒不 conv（max_splice_index 闸）→ 实际收敛 = 编辑行 + 2；
        // 结构变（编辑行不 conv）后需连续两行 conv 才 break（prev_hint 链传播终止验证）。
        let mut prev_conv = true;
        // 同 rebuild_all: 空行所属列表帧 (深度,种类) 的 pending loose。
        // 跨 ripple 起点恢复：空行在起点前（重放不含 pending——空行不产 entry）——
        // 起点前一行是空行且起点行非空 → 空行所属列表帧 = 起点栈顶列表帧
        // （空行不产 entry——帧与空行时相同——同层→嵌套编辑的 loose 不丢）。
        // #326：收集栈上全部 has_loose 帧（与空行时的收集一致）。
        let mut pending_loose: Vec<(usize, FrameKind)> = Vec::new();
        if start_line_idx > 0
            && start_line_idx - 1 < self.lines.len()
            && start_line_idx < self.rope.line_len()
        {
            let prev_is_blank = self.lines[start_line_idx - 1].node_type() == BlockType::Blank;
            if prev_is_blank {
                // rope 行切片可能跨物理 chunk——逐 chunk 检查（首 chunk 全空白
                // 不代表整行空白）。
                let cur_nonblank = {
                    let line = self.rope.line(start_line_idx);
                    let mut nonblank = false;
                    for c in line.chunks() {
                        if !c.as_bytes().iter().all(|&b| b == b' ' || b == b'\t') {
                            nonblank = true;
                            break;
                        }
                    }
                    nonblank
                };
                if cur_nonblank {
                    for (d, top) in stack.iter().enumerate() {
                        if crate::effect::decl_for(top.kind()).has_loose {
                            pending_loose.push((d, top.kind()));
                        }
                    }
                }
            }
        }

        #[cfg(feature = "telemetry")]
        let _sp_cow = telemetry_span!("col_cow");
        let lines_mut = Arc::make_mut(&mut self.lines);
        // 承诺列原地重写(与 lines 同款);旧值先取 Arc 引用
        // (COW 前的旧承诺——conv 第七条件的对账面;make_mut 若触发 COW,
        // 旧 Arc 仍指向原始缓冲)。
        let old_col = self.prev_col.clone();
        let col_mut = Arc::make_mut(&mut self.prev_col);
        // L0 深度列原地重写（与 lines/prev_col 同款；edit 时已镜像 splice，
        // 尾段位置已对齐，无需 row_pushes 式 δ 重建）。
        let depth_mut = Arc::make_mut(&mut self.open_depth);
        #[cfg(feature = "telemetry")] drop(_sp_cow);

        let mut sink = VecSink {
            actions: &mut ripple_frame_actions,
            open_lines,
        };
        let prev_semantic_depth = stack.len();
        let state = ScanState {
            stack,
            anchors,
            prev_hint: None,
            pending_loose,
            prev_semantic_depth,
        };
        let max_splice_index = self.max_splice_index;
        #[cfg(feature = "telemetry")] let _sp_sc = crate::telemetry_span!("rp_scan");
        let convergence = Self::scan_range(
            &self.rope,
            lines_mut,
            col_mut,
            depth_mut,
            start_line_idx,
            total,
            state,
            &mut sink,
            None, // 增量路径: 任意起点——保持 indexed line()
            |curr_line, old_packed, new_packed, line_actions, state, open_lines| {
                // shadow 无条件推进：scan 之后、conv 之前，且不在 conv 分支内。
                let mapped_old = curr_line as isize - delta;
                if mapped_old >= 0 {
                    shadow.advance_to(
                        (mapped_old as usize).min(old_line_count.saturating_sub(1)));
                }
                // HtmlElement 脏区守卫：开行落在本次重扫区间内且仍未关闭时，
                // 不允许收敛（旧 tail 的 close 可能已失效）。
                let dirty_open = open_lines.iter()
                    .any(|&l| l != u32::MAX && l as usize >= start_line_idx);
                if dirty_open {
                    return false;
                }
                let entries_eq = mapped_old >= 0
                    && line_actions.len() == shadow.line_buf.len()
                    && line_actions.iter().zip(&shadow.line_buf)
                        .all(|(a, b)| (a & !LINE_FIELD_MASK) == (b & !LINE_FIELD_MASK));
                // conv 第七条件:新算承诺 == 拼接数组旧值。
                // 改写路径(packed 等、承诺异——list_end_guard/32 层截断)会让
                // suffix 首行读到错误承诺——承诺异必不 conv(与 old_packed
                // 同款、写前比较、零重放成本)。
                let promise_eq = promise_of(state.prev_hint, new_packed)
                    == old_col
                        .get(curr_line)
                        .copied()
                        .unwrap_or(PackedLineInfo::invalid());
                let conv = old_packed.0 == new_packed.0 && old_packed.is_valid()
                    && max_splice_index.map_or(true, |m| curr_line >= m)
                    && shadow.stack == state.stack
                    && shadow.anchors == state.anchors
                    && shadow.open_lines == open_lines
                    && entries_eq
                    && promise_eq;
                if conv && (prev_conv || curr_line + 1 >= total) {
                    true
                } else {
                    prev_conv = conv;
                    false
                }
            },
        );
        #[cfg(feature = "telemetry")] drop(_sp_sc);
        self.max_splice_index = None;
        self.splice_old_len = None;

        // ── incr: capture ripple merge byproducts (§T.3 free lunch) ──
        let mut ripple_actions_copy: Option<Vec<u64>> = None;
        let mut delta_copy: isize = 0;

        if convergence > start_line_idx || had_splice || delta != 0 {
            #[cfg(feature = "telemetry")]
            let _sp_cow2 = telemetry_span!("col_cow");
            let fl_mut = Arc::make_mut(&mut self.frame_log);
            #[cfg(feature = "telemetry")] drop(_sp_cow2);
            #[cfg(feature = "telemetry")]
            let _sp_fls = telemetry_span!("fl_splice");
            fl_mut.entries.clear();
            let prefix_end = original_fl.prefix_len(start_line_idx as u32);
            fl_mut.entries.extend_from_slice(&original_fl.entries[..prefix_end]);
            ripple_actions_copy = Some(ripple_frame_actions.iter().cloned().collect());
            fl_mut.entries.extend(ripple_frame_actions);
            if convergence < total {
                let conv_old = ((convergence as isize - delta).max(0) as usize).max(start_line_idx);
                let tail_start = original_fl.prefix_len((conv_old + 1) as u32);
                delta_copy = delta;
                if delta != 0 {
                    for i in tail_start..original_fl.entries.len() {
                        let old_e = original_fl.entries[i];
                        let old_line = entry_line(old_e);
                        let shifted = old_line as i64 + delta as i64;
                        if shifted < 0 { continue; } // entry before doc start after shift
                        let shifted_line = shifted as u32;
                        // 新 entry 布局：保留 frame(bit0-15) + anchor(bit16-23) +
                        // 标记位(bit62-63) + consume(bit56-61)，重写 line 字段(bit24-55)。
                        let shifted_e = (old_e & 0xFF00_0000_00FF_FFFF) | ((shifted_line as u64) << 24);
                        fl_mut.entries.push(shifted_e);
                    }
                } else {
                    fl_mut.entries.extend_from_slice(&original_fl.entries[tail_start..]);
                }
            } else {
                delta_copy = delta;
            }
            #[cfg(feature = "telemetry")] drop(_sp_fls);
        }
        if let Some(actions) = ripple_actions_copy {
            self.ripple_diff = Some(RippleDiff {
                ripple_frame_actions: actions,
                delta: delta_copy,
                ripple_line_start: start_line_idx,
                ripple_line_end: convergence,
            });
        }
        #[cfg(test)]
        {
            self.last_ripple_folds = shadow.fold_count();
        }
        // v3: ripple 结束不再 invalidate——缓存位置 = start_line(前缀边界),
        // 栈状态对下一个事务的新文档仍有效(I1);下一次 ripple 的越界检查负责失效。
    }

    pub fn ensure_projected(&mut self) {
        if self.min_dirty_line.is_some() {
            self.ripple_lines();
        }
    }

    pub fn snapshot(&mut self) -> Snapshot {
        // 纯观测插桩：parse 段后半（确保投影 + Snapshot 装配）。
        let _sp_snapshot = crate::telemetry_span!("sc_snapshot");
        self.ensure_projected();
        Snapshot {
            rope: self.rope.clone(),
            lines: self.lines.clone(),
            prev_col: self.prev_col.clone(),
            open_depth: self.open_depth.clone(),
            frame_log: self.frame_log.clone(),
            // §6 懒缓存:外层 Arc 构造即就绪(一次 Arc 分配——可忽略);
            // 内部 OnceLock(Mutex)首次 get_or_init 才建(selector 跳块层
            // 首次使用——FJ/PATCH 零缓存数据开销)。
            block_cache: Some(std::sync::Arc::new(std::sync::OnceLock::new())),
            cache_slot: std::sync::Weak::new(),
            prev_snapshot: std::sync::Weak::new(),
            cell_consumption: Arc::new(SnapshotConsumption {
                tree_full: AtomicU64::new(0),
                secs: AtomicU64::new(0),
                cell_query: Default::default(),
                cell_hit: Default::default(),
                cell_built: Default::default(),
            }),
            cells: Some(Arc::new(std::sync::OnceLock::new())),
            segment_memo: std::sync::OnceLock::new(),
        }
    }
}


// 推断链（Snapshot/records/blocks/树/视图/投影面）已迁至 crate::infer——
// 本模块保留 State/ripple/JSON 渲染——推断符号经 use crate::infer 承接


pub fn project_text(text: &str) -> Snapshot {
    let mut state = State::new(text);
    state.snapshot()
}


// ResolveView 契约在 crate::effect（语义层自足——本模块不承接）





// to_ast_object_mode/envelope — named-property JSON (crdt_ast_patch_plan §2.2)


    /// Frame kinds that produce a container shell (BLT, OL, BQ only).
impl Snapshot {
    /// 构建归属树 + 渲染根序列到 buf（root 信封的 ch 数组形态）。
    /// topology 参数——HS 模式按 HEADINGSECTION_TOPOLOGY 建树/渲染
    /// (HeadingSection 章节壳)。
    /// 渲染层收编：复用判定走新三接口（mode-free——
    /// try_full_reuse/reuse_roots/store_roots），根清单唯一真理 = 本层
    /// 建的树（tree.roots）；渲染（建树 + render_tree）全部在此层——
    /// 拓扑在此合法入场。
    /// 结构来源 = cache 消费（欠账补线，不是可选优化项）：渲染主路径是
    /// tree_cell；structure_for_range 是 tree/section 等格的冷构建段入口
    /// （section_cell 也调它）。Base 全量命中共享 full / Edit 组装 full
    /// （splice 等价门保证 == 全量 build）/ 透传 Base（δ=0 守卫结构全等）/
    /// 无 slot 回退全量 build（与旧行为逐字节一致）。
    /// 首次读触发守卫裁决（ensure_node——overlay 透传或提升 Edit）。
    fn render_roots_to(&self, buf: &mut Vec<u8>, topology: &'static crate::topology::Topology) {
        self.render_roots_to_with(buf, topology, &())
    }

    /// 归属注入版（N5，F 帧）：claims **非空** ⇒ 禁用字节复用（复用表按无归属
    /// 字节存）并走全量渲染；claims 空 ⇒ 与 render_roots_to 同一路径（逐字节不变）。
    fn render_roots_to_with(&self, buf: &mut Vec<u8>, topology: &'static crate::topology::Topology,
                            claims: &dyn crate::incr::ClaimLookup) {
        let with_claims = !claims.is_empty();
        // ① 整段复用快路径（零树零渲染——Base 命中 97ms→~7ms 那一级）。
        if !with_claims {
            if let Some(slot) = self.cache_slot.upgrade() {
                if slot.try_full_reuse(buf, topology.name) {
                    return;
                }
            }
        }

        // A5 全量树归口(#38):树格供给(到达即建,全量 × 拓扑)——格内
        // 段 + 树一次构建、逐快照共享;视图由格段经 with_arrays 重建
        // (借用形态,零重复构建)。渲染等价由 825 判官逐字节裁决。
        let _span_fc = telemetry_span!("toast_build");
        let cell = self.tree_cell(topology);
        let seg = &cell.seg;
        let view = crate::infer::LineView::with_arrays(&seg.records, &seg.blocks,
            &self.frame_log.entries, &self.lines, &self.rope,
            (seg.start_line as u32, seg.end_line as u32), seg.view.clone(), &seg.arena);
        let tree = &cell.tree;
        let _ = _span_fc;
        // ② 逐根复用（cache 按树根序列回答）——有 slot 时恒走逐根循环
        // 并 store 回（无论 cache 是否帮得上：冷路径=全 None 也存表，
        // 下次 try_full_reuse 命中）；无 slot 时纯冷路径全渲染。
        if with_claims {
            // 含归属：复用字节不含 match 键 ⇒ 直接全量渲染（无认领时走下面原路径）。
            crate::incr::render_tree_roots_with(buf, self, &view, &tree, topology, claims);
            return;
        }
        if let Some(slot) = self.cache_slot.upgrade() {
            let spans: Vec<(u32, u32)> = tree.roots.iter()
                .map(|&r| (tree.nodes[r as usize].start, tree.nodes[r as usize].end)).collect();
            let per_root = slot.reuse_roots(self, &spans, topology.name);
            let mut filled: Vec<crate::cache::RootBytes> =
                Vec::with_capacity(spans.len());
            // S1（性能专项）：scratch/ls 跨根共享——roots 按 start 升序、
            // LineSupply 只进不退（incr.rs:1182 debug_assert），逐根渲染从
            // 「每根新建 supply 自行 0 重扫」降为「共用一个只前进」，总步数
            // O(总行数)。复用（Some）分支不碰它们。
            let mut scratch = crate::inline::InlineScratch::new();
            let mut ls = crate::incr::LineSupply::new(&self.rope);
            for (i, (&(rs, re), &root)) in spans.iter().zip(tree.roots.iter()).enumerate() {
                if i > 0 {
                    buf.push(b',');
                }
                let value = match per_root.get(i).and_then(|x| x.as_ref()) {
                    Some(b) => {
                        buf.extend_from_slice(b.as_bytes());
                        b.clone()
                    }
                    None => {
                        let mut rb = Vec::new();
                        crate::incr::render_tree_with_shared(&mut rb, self, &view, &tree, root as usize, topology, claims, &mut scratch, &mut ls);
                        let s = String::from_utf8(rb).unwrap_or_default();
                        buf.extend_from_slice(s.as_bytes());
                        s
                    }
                };
                filled.push(crate::cache::RootBytes { start: rs, end: re, bytes: value });
            }
            slot.store_roots(filled, topology.name);
            return;
        }
        // ③ 无 slot 冷路径：全渲染（与现状一致——无缓存可存）。
        let _span_rd = telemetry_span!("toast_render");
        crate::incr::render_tree_roots_with(buf, self, &view, &tree, topology, claims);
        let _ = _span_rd;
    }

    /// 数组形态接口单一入口 = to_ast_object_mode（拓扑分派）——测试断言用
    /// ast_ch 辅助(取 root.ch)。
    pub fn to_ast_object_mode(&self, mode: crate::topology::ProjectionMode) -> String {
        self.to_ast_object_envelope(mode, None)
    }

    /// 全篇 canonical md（0..n 行游标，与 to_md 同构；不建树）。
    ///
    /// 契约：这是**唯一整篇保真**入口——空行、表格分隔线、块间布局行都
    /// 逐行保留；对非规范形输入按 canonical 输出（恒等仅对 canonical 输入成立）。
    pub fn render_md_all(&self, mode: crate::topology::ProjectionMode) -> String {
        crate::incr::md_render::render_all(self, mode)
    }

    /// 选中 IDENT/BlockNode 的 md（R0 只做 Select；结构编辑后续走 OutputDoc）。
    ///
    /// 契约（canonical 块拼接）：
    /// - 按输入顺序拼接；允许重复与重叠（组装特性），不 dedupe；
    /// - 无块间分隔符；每个选中块自带宽尾 `\n`（含最后一块）；
    /// - 只输出属于选中块的 span；块间空行、顶层表格分隔线等“布局行”不输出；
    /// - 文档整体（含布局行）用 `render_md_all`（arrow 的合成根 -1 映射到它）；
    /// - 失败返回结构化 `MdRenderError`，不降级为猜测。
    pub fn render_md_selected(
        &self,
        mode: crate::topology::ProjectionMode,
        nodes: &[crate::block::BlockNode],
    ) -> Result<String, MdRenderError> {
        crate::incr::md_render::render_selected(self, mode, nodes)
    }

    /// 全量输出 root 信封（§5.6 附加信息）:
    /// `{"nt":"root","mode":"cf"|"hs"|"bf","timestamp":<epoch 秒>,
    ///   "md":"文档名"(可选——调用方传),"ct":[原本的内容...]}`。
    /// mode = 拓扑分派的 JSON 表达;timestamp = 生成时刻(epoch 秒);
    /// md = 文档名(可选——None 省略——测试/无文档场景)。
    /// 信封在本函数拼——渲染器零改动(拓扑无关性保持)。
    pub fn to_ast_object_envelope(&self, mode: crate::topology::ProjectionMode,
                                  md: Option<&str>) -> String {
        self.to_ast_object_envelope_with_children(mode, md, None, &())
    }

    /// 归属注入版（N5，F 帧）：每块级节点对象带 "match"（无认领 ⇒ 逐字节不变）。
    pub fn to_ast_object_mode_with_claims(&self, mode: crate::topology::ProjectionMode,
                                          claims: &dyn crate::incr::ClaimLookup) -> String {
        self.to_ast_object_envelope_with_children(mode, None, None, claims)
    }

    pub(crate) fn to_ast_object_envelope_with_children(&self,
                                                       mode: crate::topology::ProjectionMode,
                                                       md: Option<&str>,
                                                       children: Option<String>,
                                                       claims: &dyn crate::incr::ClaimLookup) -> String {
        let est = (self.rope.byte_len() as f64 * 3.5) as usize + 4096;
        let mut buf = Vec::with_capacity(est);
        buf.extend_from_slice(b"{\"nt\":\"root\",\"mode\":\"");
        buf.extend_from_slice(mode.name().as_bytes());
        buf.extend_from_slice(b"\",\"timestamp\":");
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs()).unwrap_or(0);
        buf.extend_from_slice(ts.to_string().as_bytes());
        if let Some(name) = md {
            buf.extend_from_slice(b",\"md\":\"");
            for &b in name.as_bytes() {
                match b {
                    b'"' => buf.extend_from_slice(b"\\\""),
                    b'\\' => buf.extend_from_slice(b"\\\\"),
                    b'\n' => buf.extend_from_slice(b"\\n"),
                    b'\r' => buf.extend_from_slice(b"\\r"),
                    b'\t' => buf.extend_from_slice(b"\\t"),
                    0x00..=0x1F => {
                        buf.extend_from_slice(b"\\u00");
                        buf.push(if (b >> 4) < 10 { b'0' + (b >> 4) } else { b'a' + (b >> 4) - 10 });
                        buf.push(if (b & 0xF) < 10 { b'0' + (b & 0xF) } else { b'a' + (b & 0xF) - 10 });
                    }
                    _ => buf.push(b),
                }
            }
            buf.push(b'"');
        }
        buf.extend_from_slice(b",\"ct\":[");
        match children {
            Some(s) => buf.extend_from_slice(s.as_bytes()),
            None => {
                if !self.lines.is_empty() {
                    self.render_roots_to_with(&mut buf, mode.topology(), claims);
                }
            }
        }
        buf.push(b']');
        buf.push(b'}');
        unsafe { String::from_utf8_unchecked(buf) }
    }
}

/// 测试辅助:root 信封 → ch 数组(旧数组形态断言用——统一后
/// 单一 to_ast_object_mode 接口——unit 测试(mod 内)取 ch 断言)。
#[cfg(test)]
pub(crate) fn ast_ch(snap: &Snapshot) -> serde_json::Value {
    let json = snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
    let v: serde_json::Value = serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("ast_ch 解析失败: {e}\n{json}"));
    v["ct"].clone()
}


// M0.2 埋点验证 + 段 memo 生效判官(#54/#36b;C3 记账口径复测面):
// 计数只累加不读生产路径;段 memo 单值、memo-first 命中间一 Arc。

#[cfg(test)]
mod m02_memo_tests {
    use super::*;

    /// M0.2 埋点(#54):格查询计数累加;同快照重复查询不重复构建
    /// (per-key 单构建)——计数与格值一一对应。
    #[test]
    fn m02_consumption_counters_increment_once_per_build() {
        let mut state = State::new("# A\n\ntext\n\n## B\n\n- a\n- b\n");
        let snap = state.snapshot();
        let _ = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
        let c1 = snap.cell_consumption.secs.load(Ordering::Relaxed);
        assert_eq!(c1, 1, "first secs query = one consumption");
        let _ = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
        assert_eq!(
            snap.cell_consumption.secs.load(Ordering::Relaxed),
            2,
            "each query counts (账本口径),cell builds once"
        );
        let _ = snap.tree_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
        assert_eq!(snap.cell_consumption.tree_full.load(Ordering::Relaxed), 1);
        let _ = snap.tree_cell(&crate::topology::CONTAINER_TOPOLOGY);
        assert_eq!(snap.cell_consumption.tree_full.load(Ordering::Relaxed), 2);
    }

    /// 段 memo 生效(op-only 重复查询夹具,§9 强制级):无槽快照上连续
    /// 全量查询命中间一 Arc(裸建一次回填),局部查询不读不写 memo。
    #[test]
    fn m02_segment_memo_single_value_memo_first() {
        let mut state = State::new("# A\n\ntext\n\n## B\n\n- a\n- b\n\npara\n");
        let snap = state.snapshot();
        let s1 = crate::cache::structure_for_range(&snap, 0, snap.line_len());
        let s2 = crate::cache::structure_for_range(&snap, 0, snap.line_len());
        assert!(
            Arc::ptr_eq(&s1, &s2),
            "op-only 重复全量查询命中间一 Arc(段 memo 生效)"
        );
        assert!(
            snap.segment_memo.get().is_some(),
            "全量裸建回填快照级段 memo"
        );
        // 局部查询不写 memo(单值 = 全量段)。
        let _partial = crate::cache::structure_for_range(&snap, 2, 4);
        assert!(Arc::ptr_eq(snap.segment_memo.get().unwrap(), &s1));
    }

    /// 格表并发单构建(§8 并发互斥等待的机制面):同快照同键 8 线程
    /// 并发查询,构建恰一次、全体命中同一 Arc(OnceLock<Mutex<表>>
    /// 与 block_cache 同构的单构建契约——派生格并发安全的前提)。
    /// 计数口径 = 查询次数(消费账本),单构建由 ptr_eq 全体一致证明。
    #[test]
    fn m02_concurrent_cell_queries_build_once() {
        let mut state = State::new("# A\n\ntext\n\n## B\n\n- a\n- b\n\n> q\n");
        let snap = Arc::new(state.snapshot());
        let mut handles = Vec::new();
        for _ in 0..8 {
            let snap = snap.clone();
            handles.push(std::thread::spawn(move || {
                snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY)
            }));
        }
        let cells: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("cell query thread"))
            .collect();
        assert_eq!(
            snap.cell_consumption.secs.load(Ordering::Relaxed),
            8,
            "并发 8 查询 = 8 次消费计数(账本口径)"
        );
        for w in cells.windows(2) {
            assert!(Arc::ptr_eq(&w[0], &w[1]), "并发查询全部命中同一 Arc(单构建)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MdioEngine;

    // [S2 交付项] 新测试文件（只新增；探针/判官/分配数门），各自独立子模块避免同名助手冲突。
    mod cell_entry_matrix_tests {
        include!("../tests/unit/cell_entry_matrix.rs");
    }
    mod cell_key_no_collision_tests {
        include!("../tests/unit/cell_key_no_collision.rs");
    }
    mod cell_prewarm_replay_tests {
        include!("../tests/unit/cell_prewarm_replay.rs");
    }
    mod cell_alloc_gate_tests {
        include!("../tests/unit/cell_alloc_gate.rs");
    }

    /// [S4 · J3] 判定层三臂的状态/结果一致：`Peek` 与 `UseIfReady` **不得构建**，
    /// `Build` 恰好一次且入槽；账本 `built` 与行为一致（并发败者不计）。
    #[test]
    fn verdict_arms_never_build_when_not_ready() {
        let mut st = State::new("# H\n\npara\n");
        let snap = st.snapshot();
        let key = CellKey::of(
            (0, snap.line_len()),
            &crate::topology::CONTAINER_TOPOLOGY,
            crate::domain::DomainId::Span,
        );
        let (_, _, b0) = snap.cell_consumption.cell_stats(CellKind::BlockSpans);
        assert!(cell_with(&snap, key, CellPolicy::Peek, || panic!("Peek 不得构建")).is_none());
        assert!(
            cell_with(&snap, key, CellPolicy::UseIfReady, || panic!("UseIfReady 未命中不得构建"))
                .is_none()
        );
        let (_, _, b1) = snap.cell_consumption.cell_stats(CellKind::BlockSpans);
        assert_eq!(b1, b0, "Peek/UseIfReady 在未就绪时不得构建");
        let v = cell_with(&snap, key, CellPolicy::Build, || {
            Arc::new(CellValue::BlockSpans(Arc::new(vec![(0u32, 1u32)])))
        });
        assert!(v.is_some());
        let (_, h2, b2) = snap.cell_consumption.cell_stats(CellKind::BlockSpans);
        assert_eq!(b2, b1 + 1, "Build 必须恰好构建一次");
        let _ = cell_with(&snap, key, CellPolicy::Build, || panic!("已就绪不得再构建"));
        let (_, h3, b3) = snap.cell_consumption.cell_stats(CellKind::BlockSpans);
        assert_eq!(b3, b2, "已就绪不得再构建");
        assert_eq!(h3, h2 + 1, "第二次 Build 应记为命中");
        assert!(cell_with(&snap, key, CellPolicy::UseIfReady, || panic!("不得构建")).is_some());
    }

    /// root 信封契约(§5.6 附加信息):`{nt:root,
    /// mode, timestamp, md?, ch:[...]}`——mode 字段 = 拓扑分派的 JSON 表达
    /// ——timestamp = epoch 秒——md 可选(调用方传)——ch 与数组形态逐字节
    /// 一致——空文档 root 包裹空 ch。
    #[test]
    fn root_envelope_contract() {
        use crate::topology::ProjectionMode;
        let cases: &[&str] = &["", "# H\n\ntext\n", "- a\n- b\n", "> - a\n> - b\n"];
        for md in cases {
            let snap = State::new(md).snapshot();
            let json = snap.to_ast_object_envelope(ProjectionMode::Cf, Some("doc name \"quoted\".md"));
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(v["nt"], "root", "md={md:?}");
            assert_eq!(v["mode"], "cf", "md={md:?}");
            assert!(v["timestamp"].is_number(), "timestamp = epoch 秒: {json}");
            assert_eq!(v["md"], "doc name \"quoted\".md", "md 名(转义): {json}");
            let ch = v["ct"].as_array().unwrap_or_else(|| panic!("md={md:?}: ch 缺失"));
            // 信封 ch 即内容——与 mode 输出同源
            let _ = ch;
        }
        // md 省略场景(to_ast_object_mode——无文档上下文)
        let snap = State::new("text\n").snapshot();
        let v: serde_json::Value = serde_json::from_str(
            &snap.to_ast_object_mode(ProjectionMode::Cf)).unwrap();
        assert!(v.get("md").is_none(), "md 省略: {v}");
        assert!(v["timestamp"].is_number(), "timestamp 恒在: {v}");
    }

    /// 构造文档的类别断言（ItemOpen* 分类 vs content_kind 列——
    /// 与 is_item_content_block 的集合判据一致——语义定义先行）。
    #[test]
    fn test_classify_ctx_semantics() {
        use crate::effect::CtxClass as CC;
        let md = "- a\n- # Foo\n- > b\n-     foo\n- \n> - c\n- x\n  - y\n- a\n  cont\n- [z]: /u\n";
        let mut st = State::new(md);
        let snap = st.snapshot();
        let mut fold = crate::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        crate::infer::walk_zone(&snap.frame_log.entries, (0, snap.line_len() as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = crate::infer::build_block_records(&snap.frame_log.entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, snap.line_len() as u32));
        let spans = crate::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries, &snap.lines, &snap.rope, (0, snap.line_len() as u32), &arena);
        let is_item_open = |i: usize| -> bool {
            let entries = &snap.frame_log.entries;
            let mut ei = entries.partition_point(|e| entry_line(*e) < i as u32);
            while ei < entries.len() && entry_line(entries[ei]) as usize == i {
                let e = entries[ei];
                if !entry_is_pop(e) && !entry_is_body_mut(e) {
                    if matches!(entry_frame(e).kind(),
                        FrameKind::BulletList | FrameKind::OrderedList) { return true; }
                }
                ei += 1;
            }
            false
        };
        // 行号 → 期望类别（item_depth 语义——item 的
        // 列表帧栈深度——行号按文档实际（行 5="> - c" BQ 内帧深度 1/
        // 行 6="- x" 帧深度 0/行 7="  - y" 嵌套帧深度 1））
        let expect_open: &[(usize, usize, CC)] = &[
            (0, 0, CC::ItemOpenPara),         // - a
            (1, 0, CC::ItemOpenBlock),        // - # Foo
            (2, 0, CC::ItemOpenNest),         // - > b
            (3, 0, CC::ItemOpenBlock),        // -     foo（IC——列兜底）
            (4, 0, CC::ItemOpenEmpty),        // - （空）
            (5, 1, CC::ItemOpenPara),         // > - c（BQ 内——列表帧深度 1）
            (6, 0, CC::ItemOpenPara),         // - x（BQ 后新列表——帧深度 0）
            (7, 1, CC::ItemOpenPara),         //   - y（嵌套——帧深度 1）
            (10, 0, CC::ItemOpenBlock),       // - [z]: /u（refdef）
        ];
        for &(i, item_depth, expect) in expect_open {
            assert!(is_item_open(i), "行 {i} 应为 item 打开行");
            let got = spans.classify_ctx(i, true, item_depth);
            assert_eq!(got, expect,
                "行 {i} ({:?}): 期望 {expect:?} 实际 {got:?}",
                snap.rope.line(i).to_string());
        }
        // tight 段落（- a\n  cont——cont 行 9——非打开行——ItemTightPara）
        let got = spans.classify_ctx(9, false, 0);
        assert_eq!(got, CC::ItemTightPara,
            "行 9（cont——tight 段落）: 期望 ItemTightPara 实际 {got:?}");
        // 非 item 行 → Default
        let got = spans.classify_ctx(5, false, 0);
        assert_eq!(got, CC::Default,
            "行 5（> - c 行——BQ 开行——非 item）: 期望 Default 实际 {got:?}");
        // 层感知（B3——`- - foo` 同行双 item——层感知进列——content_kind
        // per-(line,depth): 外层帧深度 0（Bullet——Nest——抑制）——
        // 内层帧深度 1（Paragraph——Para——不抑制））
        let md3 = "- - foo\n";
        let mut st3 = State::new(md3);
        let snap3 = st3.snapshot();
        let mut fold3 = crate::infer::RecordFold::new(&snap3.frame_log.entries, &snap3.lines);
        crate::infer::walk_zone(&snap3.frame_log.entries, (0, snap3.line_len() as u32), &mut fold3);
        let (recs3, arena3) = fold3.finish();
        let blocks3 = crate::infer::build_block_records(&snap3.frame_log.entries, &snap3.lines, &snap3.open_depth, &snap3.rope, &recs3, &arena3, (0, snap3.line_len() as u32));
        let spans3 = crate::infer::LineView::new(&recs3, &blocks3, &snap3.frame_log.entries, &snap3.lines, &snap3.rope, (0, snap3.line_len() as u32), &arena3);
        assert_eq!(spans3.close_depth_at(0), 2, "行 0 应两层列表帧");
        let got_outer = spans3.classify_ctx(0, true, 0);
        assert_eq!(got_outer, CC::ItemOpenNest,
            "外层（帧深度 0）: 期望 ItemOpenNest 实际 {got_outer:?}");
        let got_inner = spans3.classify_ctx(0, true, 1);
        assert_eq!(got_inner, CC::ItemOpenPara,
            "内层（帧深度 1——层感知）: 期望 ItemOpenPara 实际 {got_inner:?}");
        // 空内层 item（B3 查漏——`- - `——旧"内容不可得→保守 Para"回退
        // 被 per-depth 列取代——空内层 → ItemOpenEmpty（#316 语义——空
        // item 无 wrapper）——比旧回退更精确）
        let md4 = "- - \n";
        let mut st4 = State::new(md4);
        let snap4 = st4.snapshot();
        let mut fold4 = crate::infer::RecordFold::new(&snap4.frame_log.entries, &snap4.lines);
        crate::infer::walk_zone(&snap4.frame_log.entries, (0, snap4.line_len() as u32), &mut fold4);
        let (recs4, arena4) = fold4.finish();
        let blocks4 = crate::infer::build_block_records(&snap4.frame_log.entries, &snap4.lines, &snap4.open_depth, &snap4.rope, &recs4, &arena4, (0, snap4.line_len() as u32));
        let spans4 = crate::infer::LineView::new(&recs4, &blocks4, &snap4.frame_log.entries, &snap4.lines, &snap4.rope, (0, snap4.line_len() as u32), &arena4);
        assert_eq!(spans4.close_depth_at(0), 2, "行 0 应两层列表帧");
        let outer = spans4.classify_ctx(0, true, 0);
        assert_eq!(outer, CC::ItemOpenNest,
            "空内层外层（内容 `- `——嵌套 marker）: 期望 ItemOpenNest 实际 {outer:?}");
        let inner = spans4.classify_ctx(0, true, 1);
        assert_eq!(inner, CC::ItemOpenEmpty,
            "空内层（帧深度 1——真空）: 期望 ItemOpenEmpty 实际 {inner:?}");
    }

    /// apply（呈现修改入口）——语义等价——shape 断言
    /// （CtxClass 驱动的修改——与既有判定一致）。
    #[test]
    fn test_apply_shape_semantics() {
        use crate::effect::{RenderShape, apply};
        let md = "- a\n- # Foo\n- > b\n-     foo\n- \n> - c\n- x\n  - y\n- a\n  cont\n- [z]: /u\n";
        let mut st = State::new(md);
        let snap = st.snapshot();
        let mut fold = crate::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
        crate::infer::walk_zone(&snap.frame_log.entries, (0, snap.line_len() as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let blocks = crate::infer::build_block_records(&snap.frame_log.entries, &snap.lines, &snap.open_depth, &snap.rope, &recs, &arena, (0, snap.line_len() as u32));
        let spans = crate::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries, &snap.lines, &snap.rope, (0, snap.line_len() as u32), &arena);
        // item_depth 语义（item 的列表帧栈深度——
        // 行 5="> - c" 帧深度 1/行 6="- x" 帧深度 0/行 7="  - y" 帧深度 1）
        let expect: &[(usize, bool, usize, RenderShape)] = &[
            (0, true, 0, RenderShape::Normal),        // - a（段落——无修改）
            (1, true, 0, RenderShape::BlockContent),  // - # Foo（内容块）
            (2, true, 0, RenderShape::BlockContent),  // - > b（嵌套）
            (3, true, 0, RenderShape::BlockContent),  // -     foo（IC）
            (4, true, 0, RenderShape::EmptyContent),  // - （空——NoWrapper）
            (5, true, 1, RenderShape::Normal),        // > - c（BQ 内——帧深度 1）
            (6, true, 0, RenderShape::Normal),        // - x（帧深度 0）
            (7, true, 1, RenderShape::Normal),        //   - y（嵌套——帧深度 1）
            (10, true, 0, RenderShape::BlockContent), // - [z]: /u（refdef）
            (9, false, 0, RenderShape::InlineText),   // cont（tight 段落）
        ];
        for &(i, is_open, item_depth, expect_shape) in expect {
            let v = apply(&spans, i, is_open, item_depth);
            assert_eq!(v.shape, expect_shape,
                "行 {i} ({:?}): 期望 shape {expect_shape:?} 实际 {:?}（override {:?}）",
                snap.rope.line(i).to_string(), v.shape, v.override_mod);
        }
        // ClosedRd（rd 撤销——Transparent）——构造: `[foo]:` 空 body 撤销
        let md2 = "- a\n[foo]:\n- b\n";
        let mut st2 = State::new(md2);
        let snap2 = st2.snapshot();
        let mut fold2 = crate::infer::RecordFold::new(&snap2.frame_log.entries, &snap2.lines);
        crate::infer::walk_zone(&snap2.frame_log.entries, (0, snap2.line_len() as u32), &mut fold2);
        let (recs2, arena2) = fold2.finish();
        let blocks2 = crate::infer::build_block_records(&snap2.frame_log.entries, &snap2.lines, &snap2.open_depth, &snap2.rope, &recs2, &arena2, (0, snap2.line_len() as u32));
        let spans2 = crate::infer::LineView::new(&recs2, &blocks2, &snap2.frame_log.entries, &snap2.lines, &snap2.rope, (0, snap2.line_len() as u32), &arena2);
        // 行 1（[foo]:——被 Part A0.5 撤销——resolved=Paragraph——帧是 RefDef）
        let v = apply(&spans2, 1, false, 0);
        assert_eq!(v.shape, RenderShape::Transparent,
            "行 1（rd 撤销）: 期望 Transparent 实际 {:?}", v.shape);
    }

    /// block_kind（块形态类型学）——语义断言——
    /// 类型 → 块形态（κ' 声明——与既有组装行为一致）。
    #[test]
    fn test_block_kind_semantics() {
        use crate::effect::{BlockKind, block_kind};
        let cases: &[(BlockType, BlockKind)] = &[
            (BlockType::Bullet, BlockKind::HeadBody),
            (BlockType::Checklist, BlockKind::HeadBody),
            (BlockType::OrderedListItem, BlockKind::HeadBody),
            (BlockType::CodeFenceOpen, BlockKind::Delimited),
            (BlockType::HtmlBlock, BlockKind::Delimited),
            (BlockType::Comment, BlockKind::Delimited),
            (BlockType::Paragraph, BlockKind::Corrected),
            (BlockType::IndentedCode, BlockKind::Continuous),
            (BlockType::RefDef, BlockKind::Candidate),
        ];
        for &(t, expect) in cases {
            assert_eq!(block_kind(t), expect, "类型 {t:?} 块形态期望 {expect:?}");
        }
    }

    #[test]
    fn test_to_ast_bullet_list_then_code_fence() {
        // 4 bullets + blank line + code_fence — verify code_fence is sibling, not child
        let doc = "# Chat: demo\n- user: hello\n- assistant: hi there!\n- user: how are you?\n- assistant: I'm great!\n\n```js\nconst x = 1;\n```\n";
        let mut engine = MdioEngine::new(doc);
        let snap = engine.make_snapshot();
        let full = crate::projection::ast_ch(&snap);
        let arr = full.as_array().unwrap();

        // code_fence should be top-level sibling (/children/2), not inside bullet_list (/children/1/children/4)
        let has_code_fence_top = arr.iter().any(|n| n["nt"] == "c");
        assert!(has_code_fence_top, "code_fence should be top-level, got top-level types: {:?}",
            arr.iter().map(|n| n["nt"].as_str().unwrap_or("?")).collect::<Vec<_>>());
    }

    /// char-boundary 防御：投影层入口对非 boundary 偏移钳制而非 panic。
    /// 回归场景：`- user: 用**粗体**和`行内代码`各举一个例子` 的两步法写入——
    /// gateway 实测 panic（byte offset 81 落在 '粗' 中间——line_of_byte panic）。
    #[test]
    fn test_char_boundary_clamp_defense() {
        let doc = "# CHATBOT\n## SESSION1\n### HISTORY\n- user: 你好\n- assistant: 你好！我是你的AI助手\n- user: 用**粗体**和`行内代码`各举一个例子";
        let mut state = State::new(doc);
        // 找"粗"字符的中间偏移（延续字节——非 boundary）
        let c_pos = doc.find("粗").unwrap();
        let mid = c_pos + 1; // 落在"粗"的第二个字节
        assert!(!state.rope.is_char_boundary(mid), "测试前提：mid 必须非 boundary");

        // 1. snap_down_char_boundary：钳制到字符起点且合法
        let clamped = snap_down_char_boundary(&state.rope, mid);
        assert!(state.rope.is_char_boundary(clamped));
        assert_eq!(clamped, c_pos, "钳制应退到该字符起点");

        // 2. Snapshot::line_of_byte 不 panic + 返回合法行号（钳制后行查询语义不变——
        //    多字节字符不跨行）
        let snap = state.snapshot();
        let line = snap.line_of_byte(mid);
        assert!(line < state.rope.line_len(), "行号合法");
        let line_b = snap.line_of_byte(c_pos);
        assert_eq!(line, line_b, "字符起点与中间偏移同属一行");

        // 3. Snapshot::byte_slice 不 panic（端点钳制）
        let _ = snap.byte_slice(mid..mid + 4);
        let _ = snap.byte_slice(mid + 4..mid); // 反转范围——收缩为空不 panic

        // 4. State::edit 不 panic（写路径入口钳制）
        state.edit(mid, mid, "x");
        assert!(state.rope.to_string().contains('x'), "插入生效");

        // 5. 越界偏移 → len（恒 boundary）
        let over = snap_down_char_boundary(&state.rope, state.rope.byte_len() + 5);
        assert_eq!(over, state.rope.byte_len());
    }

    /// B3-1 性能列 I2/I3：ShadowReplay 与 forward_replay_anchors 逐行等价。
    #[test]
    fn shadow_replay_equals_forward_replay_anchors() {
        let docs: &[&str] = &[
            "",
            "# h\npara\n",
            "- a\n- b\n  - c\n\n- d\n",
            "> - x\n    code\n1. a\npara\n  1. c\ncont\n     - e\n  1. c\n  para\n",
            "| a | b |\n|---|---|\n| c | d |\n",
            "<div>\n  1. c\n      - f\n</div>\n",
        ];
        const LINE_MASK: u64 = ((1u64 << 38) - 1) << 24;
        for doc in docs {
            let snap = State::new(doc).snapshot();
            let entries = &snap.frame_log.entries;
            let n = snap.line_len();
            let mut shadow = ShadowReplay::new(entries);
            for l in 0..n {
                shadow.advance_to(l);
                let end = snap.frame_log.prefix_len((l + 1) as u32);
                let mut os: Vec<BlockFrame> = Vec::new();
                let mut oa: Vec<Option<usize>> = Vec::new();
                crate::block::forward_replay_anchors(&entries[..end], &mut os, &mut oa);
                assert_eq!(shadow.stack, os, "stack 分叉 doc={doc:?} l={l}");
                assert_eq!(shadow.anchors, oa, "anchors 分叉 doc={doc:?} l={l}");
                let lo = snap.frame_log.prefix_len(l as u32);
                let hi = snap.frame_log.prefix_len((l + 1) as u32);
                let masked = |e: &u64| *e & !LINE_MASK;
                let expect: Vec<u64> = entries[lo..hi].iter().map(masked).collect();
                let got: Vec<u64> = shadow.line_buf.iter().map(masked).collect();
                assert_eq!(got, expect, "line_buf 分叉 doc={doc:?} l={l}");
            }
        }
    }

    /// B3-1 性能回归：table header 破坏使 ripple 扫到 EOF；shadow 折叠次数
    /// 必须 ≤ FL 总条目数（O(E)），而不是 O(scan × E)。
    #[test]
    fn shadow_fold_count_regression_table() {
        let doc = "| a | b |\n|---|---|\n".repeat(4000);
        let e0 = State::new(&doc).frame_log.entries.len();
        let mut st = State::new(&doc);
        st.edit(0, 1, "para");
        st.ensure_projected();
        let scan = st.ripple_diff.as_ref()
            .map(|d| d.ripple_line_end - d.ripple_line_start)
            .unwrap_or(0);
        assert!(scan >= st.rope.line_len().saturating_sub(4),
            "该语料应触发长尾扫描，scan={scan}");
        assert!(st.last_ripple_folds <= e0,
            "shadow 折叠次数 {} 应 ≤ 旧 FL 条目 {}（O(E) 单调游标）",
            st.last_ripple_folds, e0);
        // 正确性仍由全量重扫等价保证。
        let ref_state = State::new(&st.rope.to_string());
        assert_eq!(st.lines, ref_state.lines, "lines 分叉");
        assert_eq!(st.frame_log.entries, ref_state.frame_log.entries, "FL 分叉");
    }

    /// P2b：行内级派生查询必须与独立重算的整列逐值一致。
    /// 随机小文档 + 单次随机编辑后，`row_pushes_at` 必须等于用最终文本
    /// 重新 State::new 后独立重算的整列（此为派生面判官）。
    #[test]
    fn row_pushes_at_matches_rebuild() {
        fn next_rand(state: &mut u64) -> u64 {
            *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *state
        }
        fn random_edit(state: &mut u64, doc: &str, tokens: &[&str]) -> (usize, usize, String) {
            let len = doc.len();
            let op = (next_rand(state) % 3) as usize;
            if len == 0 || op == 0 {
                let pos = next_rand(state) as usize % (len + 1);
                let text = tokens[next_rand(state) as usize % tokens.len()];
                (pos, pos, text.to_string())
            } else if op == 1 {
                let start = next_rand(state) as usize % len;
                let end = start + 1 + next_rand(state) as usize % (len - start);
                (start, end, String::new())
            } else {
                let start = next_rand(state) as usize % len;
                let end = start + 1 + next_rand(state) as usize % (len - start);
                let removed = &doc[start..end];
                let picked = tokens[next_rand(state) as usize % tokens.len()];
                let text = if picked != removed {
                    picked.to_string()
                } else if removed == "x" {
                    "y".to_string()
                } else {
                    "x".to_string()
                };
                (start, end, text)
            }
        }

        let tokens: &[&str] = &[
            "plain paragraph\n",
            "some **bold** text\n",
            "line one\n",
            "# Heading\n",
            "## Subheading\n",
            "### Deep\n",
            "- item\n",
            "- [ ] task\n",
            "1. first\n",
            "  - nested\n",
            "> quote\n",
            "> > nested\n",
            "```rust\nlet x = 1;\n```\n",
            "~~~\ncode\n~~~\n",
            "| a | b |\n|---|---|\n| c | d |\n",
            "[ref]: /url\n",
            "[id]: https://example.com \"t\"\n",
            "\n",
            "    indented code\n",
            "\tcode\n",
        ];

        let mut rng: u64 = 0x243F_6A88_85A3_08D3;
        for case in 0..500u64 {
            let mut doc = String::new();
            let line_count = (next_rand(&mut rng) % 10) as usize;
            for _ in 0..line_count {
                doc.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
            }

            let mut st = State::new(&doc);
            let _before = st.snapshot();

            let (start, end, text) = random_edit(&mut rng, &doc, tokens);
            st.edit(start, end, &text);
            let final_text = st.rope.to_string();
            assert_ne!(final_text, doc, "case {case}: generated edit was a no-op");

            let incremental = st.snapshot();
            let full = State::new(&final_text).snapshot();
            let expected: Vec<usize> = {
                let n = full.line_len();
                let mut v = vec![0usize; n];
                for e in full.frame_log.entries.iter() {
                    let l = crate::block::entry_line(*e) as usize;
                    if l < n && !crate::block::entry_is_pop(*e) {
                        v[l] += 1;
                    }
                }
                v
            };
            let got: Vec<usize> = (0..incremental.line_len())
                .map(|i| incremental.row_pushes_at(i))
                .collect();
            assert_eq!(
                got, expected,
                "case {case}: row_pushes_at 派生与独立重算不一致\n--- doc ---\n{doc}\n--- final ---\n{final_text}",
            );
        }
    }
    /// [P2d] 结构编辑不触及标题身份时，`SecsCell::update_structural`
    /// 必须与全量 `section_cell` 逐字段一致。
    #[test]
    fn secs_cell_update_structural_matches_full() {
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        let mut st = State::new("# A\n\na one\n\na two\n\n# B\n\nb one\n\n");
        let old_snap = Arc::new(st.snapshot());
        let old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

        // 在 B 章节内容里插入一个 bullet：结构编辑，但标题身份不变。
        let pos = old_snap.byte_of_line(6) + 2;
        st.edit(pos, pos, "- x\n");
        let new_snap = Arc::new(st.snapshot());
        let new_n = new_snap.line_len();
        let diff = st.ripple_diff.take().expect("ripple diff");
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(new_n);
        let anchor = old_snap.line_of_byte(pos.min(old_snap.byte_len().saturating_sub(1)));
        let node = crate::cache::CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(pos, pos)]),
            diff.delta,
            anchor,
        );
        let (zso, ceo) = node.materialize_old_zone().expect("old zone");
        let mid = node.materialize_mid().expect("new zone segment");

        let updated = old_cell
            .update_structural(
                &old_snap,
                &new_snap,
                new_n,
                &mid,
                &[(pos as u32, pos as u32)],
                zso,
                ceo,
                diff.delta,
                anchor,
                &HEADINGSECTION_TOPOLOGY,
            )
            .expect("heading identity unchanged inside zone");

        let oracle = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
        assert_eq!(updated.secs, oracle.secs, "secs must equal full build");
        assert_eq!(updated.header_bounds, oracle.header_bounds, "header bounds must equal full build");
        assert_eq!(updated.root_starts, oracle.root_starts, "root starts must equal full build");
    }

    /// [P2d] 结构编辑改变标题 level 时，区间更新仍必须等于全量 oracle。
    #[test]
    fn secs_cell_update_structural_heading_level_matches_full() {
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        let mut st = State::new("# A\n\na one\n\n## B\n\nb one\n\n");
        let old_snap = Arc::new(st.snapshot());
        let old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

        let pos = old_snap.byte_of_line(4);
        st.edit(pos, pos + 3, "# ");
        let new_snap = Arc::new(st.snapshot());
        let new_n = new_snap.line_len();
        let diff = st.ripple_diff.take().expect("ripple diff");
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(new_n);
        let anchor = old_snap.line_of_byte(pos.min(old_snap.byte_len().saturating_sub(1)));
        let node = crate::cache::CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(pos, pos + 3)]),
            diff.delta,
            anchor,
        );
        let (zso, ceo) = node.materialize_old_zone().expect("old zone");
        let mid = node.materialize_mid().expect("new zone segment");

        let updated = old_cell
            .update_structural(&old_snap, &new_snap, new_n, &mid, &[(pos as u32, (pos + 3) as u32)], zso, ceo, diff.delta, anchor, &HEADINGSECTION_TOPOLOGY)
            .expect("heading boundary propagation should handle level change");
        let oracle = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
        assert_eq!(updated.secs, oracle.secs, "secs must equal full build");
        assert_eq!(updated.header_bounds, oracle.header_bounds, "header bounds must equal full build");
        assert_eq!(updated.root_starts, oracle.root_starts, "root starts must equal full build");
    }

    /// [P2d] 随机结构编辑：区间更新必须始终等于全量 oracle。
    #[test]
    fn secs_cell_update_structural_random_equals_full() {
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        fn next_rand(state: &mut u64) -> u64 {
            *state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *state
        }
        let tokens = [
            "# H1\n", "## H2\n", "### H3\n", "para\n", "  cont\n",
            "- a\n", "1. b\n", "> q\n", "```\nx\n```\n", "\n",
            "foo\n===\n", "| a | b |\n| - | - |\n", "[a]: /u\n",
        ];
        let mut rng: u64 = 0xC0FFEE_828_0002;
        for case in 0..300u64 {
            let mut doc = String::new();
            let n_lines = 4 + (next_rand(&mut rng) % 9) as usize;
            for _ in 0..n_lines {
                doc.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
            }
            let mut st = State::new(&doc);
            let old_snap = Arc::new(st.snapshot());
            let old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

            let len = doc.len();
            let pos = (next_rand(&mut rng) as usize) % (len + 1);
            let (s, e, text) = if len == 0 || pos == len || next_rand(&mut rng) % 2 == 0 {
                (pos, pos, tokens[next_rand(&mut rng) as usize % tokens.len()].to_string())
            } else {
                let end = (pos + 1 + (next_rand(&mut rng) as usize % (len - pos))).min(len);
                (pos, end, String::new())
            };
            st.edit(s, e, &text);
            let new_snap = Arc::new(st.snapshot());
            if st.rope.to_string() == doc {
                continue;
            }
            let new_n = new_snap.line_len();
            let diff = st.ripple_diff.take().expect("ripple diff");
            let rls = diff.ripple_line_start;
            let rle_e = (diff.ripple_line_end + 1).min(new_n);
            let anchor = old_snap.line_of_byte(s.min(old_snap.byte_len().saturating_sub(1)));
            let node = crate::cache::CacheNode::new_edit_full(
                &old_snap,
                new_snap.clone(),
                (rls, rle_e),
                crate::cache::edit_actions_arc(vec![(s, e)]),
                diff.delta,
                anchor,
            );
            let (zso, ceo) = node.materialize_old_zone().expect("old zone");
            let mid = node.materialize_mid().expect("mid");
            let updated = old_cell
                .update_structural(&old_snap, &new_snap, new_n, &mid, &[(s as u32, e as u32)], zso, ceo, diff.delta, anchor, &HEADINGSECTION_TOPOLOGY)
                .expect("random structural update must succeed");
            let oracle = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
            assert_eq!(updated.secs, oracle.secs, "case {case}: secs mismatch\ndoc={doc:?}\nedit=({s},{e},{text:?})");
            assert_eq!(updated.header_bounds, oracle.header_bounds, "case {case}: header_bounds mismatch\ndoc={doc:?}\nedit=({s},{e},{text:?})");
            assert_eq!(updated.root_starts, oracle.root_starts, "case {case}: root_starts mismatch\ndoc={doc:?}\nedit=({s},{e},{text:?})");
        }
    }

    /// transition-root 反例：删除 `# A` 后，原 A 章节内段落应全部成为
    /// 节外根块；当前实现只保留 root / zone root / suffix root，会漏掉它们。
    #[test]
    fn secs_cell_update_structural_transition_roots_match_full() {
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        let body: String = (0..20).map(|i| format!("p{i}\n\n")).collect();
        let doc = format!("# A\n\n{body}# C\n");
        let mut st = State::new(&doc);
        let old_snap = Arc::new(st.snapshot());
        let old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

        st.edit(0, 2, "");
        let new_snap = Arc::new(st.snapshot());
        let new_n = new_snap.line_len();
        let diff = st.ripple_diff.take().expect("ripple diff");
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(new_n);
        let anchor = old_snap.line_of_byte(0);
        let node = crate::cache::CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(0, 2)]),
            diff.delta,
            anchor,
        );
        let (zso, ceo) = node.materialize_old_zone().expect("old zone");
        let mid = node.materialize_mid().expect("mid");
        let updated = old_cell
            .update_structural(&old_snap, &new_snap, new_n, &mid, &[(0u32, 2u32)], zso, ceo, diff.delta, anchor, &HEADINGSECTION_TOPOLOGY)
            .expect("transition update must succeed");
        let oracle = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
        assert_eq!(updated.secs, oracle.secs, "secs mismatch");
        assert_eq!(updated.header_bounds, oracle.header_bounds, "header_bounds mismatch");
        assert_eq!(updated.root_starts, oracle.root_starts, "root_starts transition mismatch");
    }

    /// [P0] 标题改级 + suffix 段落：边界传播后 prefix section 截断变化，
    /// transition 候选也覆盖 level 变化场景。
    #[test]
    fn secs_cell_update_structural_heading_level_suffix_matches_full() {
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        let mut doc = String::from("# A\n\n");
        for i in 0..6 { doc.push_str(&format!("a{i}\n\n")); }
        doc.push_str("## B\n\n");
        for i in 0..6 { doc.push_str(&format!("b{i}\n\n")); }
        let mut st = State::new(&doc);
        let old_snap = Arc::new(st.snapshot());
        let old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

        let pos = old_snap.byte_of_line(8);
        st.edit(pos, pos + 3, "# ");
        let new_snap = Arc::new(st.snapshot());
        let new_n = new_snap.line_len();
        let diff = st.ripple_diff.take().expect("ripple diff");
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(new_n);
        let anchor = old_snap.line_of_byte(pos.min(old_snap.byte_len().saturating_sub(1)));
        let node = crate::cache::CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(pos, pos + 3)]),
            diff.delta,
            anchor,
        );
        let (zso, ceo) = node.materialize_old_zone().expect("old zone");
        let mid = node.materialize_mid().expect("mid");
        let updated = old_cell
            .update_structural(&old_snap, &new_snap, new_n, &mid, &[(pos as u32, (pos + 3) as u32)], zso, ceo, diff.delta, anchor, &HEADINGSECTION_TOPOLOGY)
            .expect("heading level suffix update must succeed");
        let oracle = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
        assert_eq!(updated.secs, oracle.secs, "secs mismatch");
        assert_eq!(updated.header_bounds, oracle.header_bounds, "header_bounds mismatch");
        assert_eq!(updated.root_starts, oracle.root_starts, "root_starts mismatch");
    }

    /// [P2] 多编辑事务的两个不相邻删除：transition 删除过滤必须逐区间判断，
    /// 不能用首末删除行的凸包覆盖中间存活 root。
    #[test]
    fn secs_cell_update_structural_multi_edit_disjoint_deletions_match_full() {
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        let doc = "# A\n\na1\n\na2\n\na3\n\n# B\n";
        let mut st = State::new(doc);
        let old_snap = Arc::new(st.snapshot());
        let old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

        let del1 = (0usize, 4usize);   // 删除 "# A\n"
        let del2 = (9usize, 13usize);  // 删除 "a2\n\n"
        st.edit(del1.0, del1.1, "");
        st.edit(del2.0, del2.1, "");
        let new_snap = Arc::new(st.snapshot());
        let new_n = new_snap.line_len();
        let diff = st.ripple_diff.take().expect("ripple diff");
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(new_n);
        let anchor = old_snap.line_of_byte(del1.0.min(old_snap.byte_len().saturating_sub(1)));
        let node = crate::cache::CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![del1, del2]),
            diff.delta,
            anchor,
        );
        let (zso, ceo) = node.materialize_old_zone().expect("old zone");
        let mid = node.materialize_mid().expect("mid");
        let updated = old_cell
            .update_structural(
                &old_snap,
                &new_snap,
                new_n,
                &mid,
                &[(del1.0 as u32, del1.1 as u32), (del2.0 as u32, del2.1 as u32)],
                zso,
                ceo,
                diff.delta,
                anchor,
                &HEADINGSECTION_TOPOLOGY,
            )
            .expect("multi-edit transition update must succeed");
        let oracle = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
        assert_eq!(updated.secs, oracle.secs, "secs mismatch");
        assert_eq!(updated.header_bounds, oracle.header_bounds, "header_bounds mismatch");
        assert_eq!(updated.root_starts, oracle.root_starts, "root_starts mismatch");
    }

    /// T3b/A6：`SecsCell::update_structural` 的 changed 检测对每个旧章节
    /// 扫一遍新章节表。用“旧文档 H 个 h1、新文档 H 个 h2（同位置不同 level）”
    /// 使每个旧章节都找不到 stable 对应，逼出最坏分支；计时曲线确认
    /// 是否超线性。注意该函数在 changed 非空时还会执行 `root_blocks_before`
    /// 的 O(H) 扫描，曲线可能由两者叠加。
    #[test]
    fn curve_update_structural_changed_scan() {
        let _perf_guard = crate::PERF_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use crate::cache::{CacheNode, CacheSlot};
        use crate::topology::HEADINGSECTION_TOPOLOGY;
        use std::time::{Duration, Instant};

        let build = |h: usize, mark: &str| -> std::sync::Arc<Snapshot> {
            let mut doc = String::new();
            for i in 0..h {
                doc.push_str(&format!("{mark} s{i}\n"));
            }
            std::sync::Arc::new(State::new(&doc).snapshot())
        };
        let time = |h: usize| {
            let old_snap = build(h, "#");
            let new_snap = build(h, "##");
            let old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
            let n = h;
            let new_zone_seg = crate::cache::Segment::build(&new_snap, 0, n);
            let mut best = Duration::from_secs(60);
            for _ in 0..5 {
                let t = Instant::now();
                let _ = old_cell.update_structural(
                    &old_snap,
                    &new_snap,
                    n,
                    &new_zone_seg,
                    &[],
                    0,
                    n,
                    0,
                    0,
                    &HEADINGSECTION_TOPOLOGY,
                );
                best = best.min(t.elapsed());
            }
            best
        };
        let t5 = time(500);
        let t20 = time(2_000);
        let ratio = t20.as_nanos() as f64 / t5.as_nanos() as f64;
        // [A6 修复后判官] 二分化后应为线性。修复前实测 debug 15.38x / release 13.28x；
        // 修复后单独跑 3.94x、并行跑 4.19x。阈值取 8.0（并行全量跑时 6.0
        // 曾误红一次——改 min-of-5 降抖动，8.0 仍能判出回归到二次方）。
        assert!(
            ratio < 8.0,
            "update_structural changed-scan 应为线性（4x H ≤4x 时间）: H=500={t5:?} H=2000={t20:?} ratio={ratio:.2}x"
        );
    }

    /// [P2d] `section_cell` 生产路径：旧格已初始化时走区间更新，不走全量构建。
    #[test]
    fn section_cell_updates_from_old_cell_on_structural_edit() {
        use crate::cache::{CacheNode, CacheSlot};
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        let mut st = State::new("# A\n\na one\n\na two\n\n# B\n\nb one\n\n");
        let mut old_snap = Arc::new(st.snapshot());
        let old_slot = Arc::new(CacheSlot::new());
        Arc::get_mut(&mut old_snap)
            .unwrap()
            .set_cache_slot(Arc::downgrade(&old_slot));
        let _old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

        let pos = old_snap.byte_of_line(8) + 2;
        st.edit(pos, pos, "- x\n");
        let mut new_snap = Arc::new(st.snapshot());
        let new_slot = Arc::new(CacheSlot::new());
        Arc::get_mut(&mut new_snap)
            .unwrap()
            .set_cache_slot(Arc::downgrade(&new_slot));
        let new_n = new_snap.line_len();
        let diff = st.ripple_diff.take().expect("ripple diff");
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(new_n);
        let anchor = old_snap.line_of_byte(pos.min(old_snap.byte_len().saturating_sub(1)));
        let node = CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(pos, pos)]),
            diff.delta,
            anchor,
        );
        new_slot.node.store(Some(Arc::new(node)));

        let updated = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
        let oracle = State::new(&st.rope.to_string())
            .snapshot()
            .section_cell(&HEADINGSECTION_TOPOLOGY);
        assert_eq!(updated.secs, oracle.secs, "secs must equal full oracle");
        assert_eq!(updated.header_bounds, oracle.header_bounds, "header bounds must equal full oracle");
        assert_eq!(updated.root_starts, oracle.root_starts, "root starts must equal full oracle");
    }

    /// [P2d] 生产路径在标题 level 改变时仍走区间更新并对拍全量 oracle。
    #[test]
    fn section_cell_updates_heading_level_from_old_cell() {
        use crate::cache::{CacheNode, CacheSlot};
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        let mut st = State::new("# A\n\na one\n\n## B\n\nb one\n\n");
        let mut old_snap = Arc::new(st.snapshot());
        let old_slot = Arc::new(CacheSlot::new());
        Arc::get_mut(&mut old_snap)
            .unwrap()
            .set_cache_slot(Arc::downgrade(&old_slot));
        let _old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

        let pos = old_snap.byte_of_line(4);
        st.edit(pos, pos + 3, "# ");
        let mut new_snap = Arc::new(st.snapshot());
        let new_slot = Arc::new(CacheSlot::new());
        Arc::get_mut(&mut new_snap)
            .unwrap()
            .set_cache_slot(Arc::downgrade(&new_slot));
        let new_n = new_snap.line_len();
        let diff = st.ripple_diff.take().expect("ripple diff");
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(new_n);
        let anchor = old_snap.line_of_byte(pos.min(old_snap.byte_len().saturating_sub(1)));
        let node = CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(pos, pos + 3)]),
            diff.delta,
            anchor,
        );
        new_slot.node.store(Some(Arc::new(node)));

        let updated = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
        let oracle = State::new(&st.rope.to_string())
            .snapshot()
            .section_cell(&HEADINGSECTION_TOPOLOGY);
        assert_eq!(updated.secs, oracle.secs, "secs must equal full oracle");
        assert_eq!(updated.header_bounds, oracle.header_bounds, "header bounds must equal full oracle");
        assert_eq!(updated.root_starts, oracle.root_starts, "root starts must equal full oracle");
    }

    /// [P2] 生产路径：旧格已初始化 + transition 标题删除，`section_cell`
    /// 必须走区间更新且与无槽全量 oracle 一致。
    #[test]
    fn section_cell_updates_transition_from_old_cell() {
        use crate::cache::{CacheNode, CacheSlot};
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        let body: String = (0..20).map(|i| format!("p{i}\n\n")).collect();
        let doc = format!("# A\n\n{body}# C\n");
        let mut st = State::new(&doc);
        let mut old_snap = Arc::new(st.snapshot());
        let old_slot = Arc::new(CacheSlot::new());
        Arc::get_mut(&mut old_snap)
            .unwrap()
            .set_cache_slot(Arc::downgrade(&old_slot));
        let _old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

        st.edit(0, 2, "");
        let mut new_snap = Arc::new(st.snapshot());
        let new_slot = Arc::new(CacheSlot::new());
        Arc::get_mut(&mut new_snap)
            .unwrap()
            .set_cache_slot(Arc::downgrade(&new_slot));
        let new_n = new_snap.line_len();
        let diff = st.ripple_diff.take().expect("ripple diff");
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(new_n);
        let anchor = old_snap.line_of_byte(0);
        let node = CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(0, 2)]),
            diff.delta,
            anchor,
        );
        new_slot.node.store(Some(Arc::new(node)));

        let updated = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
        let oracle = State::new(&st.rope.to_string())
            .snapshot()
            .section_cell(&HEADINGSECTION_TOPOLOGY);
        assert_eq!(updated.secs, oracle.secs, "secs must equal full oracle");
        assert_eq!(updated.header_bounds, oracle.header_bounds, "header bounds must equal full oracle");
        assert_eq!(updated.root_starts, oracle.root_starts, "root starts must equal full oracle");
    }
    /// [P2] 性能 gate：结构编辑不改变章节区间时不得进入 transition 枚举。
    #[test]
    fn secs_cell_update_skips_transition_when_sections_stable() {
        use crate::cache::{CacheNode, CacheSlot};
        use crate::topology::HEADINGSECTION_TOPOLOGY;

        SECS_UPDATE_ROOT_BLOCKS_CALLS.with(|c| c.set(0));
        let mut st = State::new("# A\n\na one\n\na two\n\n# B\n\nb one\n\n");
        let mut old_snap = Arc::new(st.snapshot());
        let old_slot = Arc::new(CacheSlot::new());
        Arc::get_mut(&mut old_snap)
            .unwrap()
            .set_cache_slot(Arc::downgrade(&old_slot));
        let _old_cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);

        let pos = old_snap.byte_of_line(6) + 2;
        st.edit(pos, pos, "- x\n");
        let mut new_snap = Arc::new(st.snapshot());
        let new_slot = Arc::new(CacheSlot::new());
        Arc::get_mut(&mut new_snap)
            .unwrap()
            .set_cache_slot(Arc::downgrade(&new_slot));
        let new_n = new_snap.line_len();
        let diff = st.ripple_diff.take().expect("ripple diff");
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(new_n);
        let anchor = old_snap.line_of_byte(pos.min(old_snap.byte_len().saturating_sub(1)));
        let node = CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(pos, pos)]),
            diff.delta,
            anchor,
        );
        new_slot.node.store(Some(Arc::new(node)));

        let _updated = new_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
        assert_eq!(
            SECS_UPDATE_ROOT_BLOCKS_CALLS.with(|c| c.get()),
            0,
            "stable sections must not trigger root_blocks_before transition enumeration"
        );
    }


    // depth_col_plan 阶段 A 判官：L0 深度列对拍 depth_prefix


    fn assert_open_depth_equals_prefix(state: &State) {
        let n = state.rope.line_len();
        let want = crate::infer::depth_prefix(&state.frame_log.entries, n);
        assert_eq!(
            state.open_depth.as_slice(),
            want.as_slice(),
            "open_depth 必须与 depth_prefix 逐值一致（含尾哨兵）"
        );
    }

    #[test]
    fn open_depth_equals_depth_prefix_full() {
        let cases: &[&str] = &[
            "",
            "para\n",
            "> - x\n> - x\n",
            "# H\n\n- a\n- b\n\n```\ncode\n```\n",
            "> q\n> - x\n\n> ```\nf\n```\n",
        ];
        for doc in cases {
            let state = State::new(doc);
            assert_open_depth_equals_prefix(&state);
        }
        let tokens = ["a", "para", "  para", "    code", "- a", "  - b",
            "1. a", "- [ ] t", "> q", "> - x", ">", "# h", "## h", "```",
            "```x", "| a |", "| - |", "[a]: /u", "", "  "];
        let mut x: u64 = 0x5EED_0000_0000_0001;
        for _ in 0..300 {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let n_lines = 1 + (x as usize) % 12;
            let mut doc = String::new();
            for _ in 0..n_lines {
                x ^= x << 13; x ^= x >> 7; x ^= x << 17;
                doc.push_str(tokens[(x as usize) % tokens.len()]);
                doc.push('\n');
            }
            let state = State::new(&doc);
            assert_open_depth_equals_prefix(&state);
        }
    }

    #[test]
    fn open_depth_equals_depth_prefix_after_edit() {
        let mut state = State::new("# A\n\na one\n\n> q\n> - x\n\n```\nf\n```\n");
        assert_open_depth_equals_prefix(&state);
        let edits: &[(usize, usize, &str)] = &[
            (6, 7, "B"),
            (0, 0, "- item\n"),
            (12, 14, ""),
            (0, 3, "## C"),
        ];
        for (s, e, t) in edits {
            state.edit(*s, *e, t);
            let _ = state.snapshot();
            assert_open_depth_equals_prefix(&state);
        }
        let mut x: u64 = 0xCAFE_0000_0000_0002;
        let mut text = "# H\n\npara one\n\n- a\n- b\n\n".to_string();
        for _ in 0..40 {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let pos = (x as usize) % (text.len() + 1);
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let end = (pos + (x as usize) % 3).min(text.len());
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let ins = ["x", "\n", "- n\n", "> q\n", ""][(x as usize) % 5];
            state.edit(pos, end, ins);
            text.replace_range(pos..end, ins);
            let _ = state.snapshot();
            assert_open_depth_equals_prefix(&state);
        }
    }
}



// Handoff S1: handoff_copy + State::from_snapshot equivalence.
// 仅在 test 构建中定义，验证 5 列深拷贝 + 空缓存后能否得到与 fresh scan 相同的 arrow 构造数据。
#[cfg(test)]
mod handoff_equivalence_tests {
    use super::{Snapshot, SnapshotConsumption, State};
    use crate::block::BlockNode;
    use crate::selector::all_nodes;
    use crate::topology::ProjectionMode;
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, OnceLock, Weak};

    fn copy_for_handoff_test(s: &Snapshot) -> Snapshot {
        Snapshot {
            rope: s.rope.clone(),
            lines: Arc::new((*s.lines).clone()),
            prev_col: Arc::new((*s.prev_col).clone()),
            open_depth: Arc::new((*s.open_depth).clone()),
            frame_log: Arc::new((*s.frame_log).clone()),
            block_cache: Some(std::sync::Arc::new(std::sync::OnceLock::new())),
            cache_slot: Weak::new(),
            prev_snapshot: Weak::new(),
            cell_consumption: Arc::new(SnapshotConsumption {
                tree_full: AtomicU64::new(0),
                secs: AtomicU64::new(0),
                cell_query: Default::default(),
                cell_hit: Default::default(),
                cell_built: Default::default(),
            }),
            cells: Some(std::sync::Arc::new(std::sync::OnceLock::new())),
            segment_memo: OnceLock::new(),
        }
    }

    fn state_from_snapshot_for_test(s: &Snapshot) -> State {
        State {
            rope: s.rope.clone(),
            lines: s.lines.clone(),
            prev_col: s.prev_col.clone(),
            open_depth: s.open_depth.clone(),
            frame_log: s.frame_log.clone(),
            min_dirty_line: None,
            max_splice_index: None,
            splice_old_len: None,
            cached_stack: (0, Vec::new()),
            cached_anchors: Vec::new(),
            cached_open_lines: Vec::new(),
            ripple_diff: None,
            #[cfg(test)]
            last_ripple_folds: 0,
            #[cfg(test)]
            last_cached_replay_folds: 0,
        }
    }

    fn assert_same_nodes(a: &[BlockNode], b: &[BlockNode]) {
        assert_eq!(a.len(), b.len(), "node count");
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(
                (x.start_line, x.end_line, x.byte_range.clone(), x.node_type,
                 x.loose, x.peel_width, x.frame_ordinal),
                (y.start_line, y.end_line, y.byte_range.clone(), y.node_type,
                 y.loose, y.peel_width, y.frame_ordinal),
                "node {i} identity"
            );
            assert_eq!(format!("{:?}", x.geometry), format!("{:?}", y.geometry), "node {i} geometry");
        }
    }

    #[test]
    fn handoff_copy_and_state_from_snapshot_equal_fresh_scan() {
        let text = "# T\n\npara one\n\n- a\n- b\n\n~~~rust\nlet x = 1;\n~~~\n\n|a|b|\n|-|-|\n|1|2|\n";
        let mut fresh_state = State::new(text);
        let fresh = fresh_state.snapshot();
        let copy = copy_for_handoff_test(&fresh);
        let mut copied_state = state_from_snapshot_for_test(&copy);
        let copied = copied_state.snapshot();

        assert_eq!(fresh.rope.to_string(), copied.rope.to_string(), "rope text");
        assert_eq!(&*fresh.lines, &*copied.lines, "lines");
        assert_eq!(&*fresh.prev_col, &*copied.prev_col, "prev_col");
        assert_eq!(&*fresh.open_depth, &*copied.open_depth, "open_depth");
        assert_eq!(fresh.frame_log.entries, copied.frame_log.entries, "frame_log");

        let n_fresh = all_nodes(&fresh);
        let n_copied = all_nodes(&copied);
        assert_same_nodes(&n_fresh, &n_copied);

        assert_eq!(
            fresh.render_md_all(ProjectionMode::Hs),
            copied.render_md_all(ProjectionMode::Hs),
            "render_md_all"
        );
    }
}

// D6：node_end/block_proj 测试挂载自 infer 迁入（impl Snapshot 随迁）。
#[cfg(test)]
mod node_end_tests {
    use crate::engine::{MdioEngine, MdioSnapshot};
    include!("../tests/unit/node_end.rs");
}

#[cfg(test)]
mod block_proj_tests {
    use crate::engine::{MdioEngine, MdioSnapshot};
    use crate::inline::InlineSegmentRole;
    include!("../tests/unit/block_proj.rs");
}
