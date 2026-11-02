//! MDIO Incr — 增量 diff（zone/align/diff_level）+ 归属树渲染。
//!
//! 两大部分:
//!   diff 侧:   LineMap/ZoneTransform/compute_zone（三路扩展）→ compute_align
//!     （共享容器层）→ diff_level（树 diff——匹配/REM/ADD/REPL 偏移）。
//!   render 侧: render_tree/render_tree_roots（归属树驱动——递归三分支：
//!     容器壳/ItemLeaf 段/块 leaf——write_block_node/write_item_leaf/
//!     write_para_inline 函数体复用）。
//!
//! 结构: 归属树 + render_tree_roots 唯一渲染形态。
//!
//! 依赖: 单向依赖 crate::infer（环已打破）——本模块消费 infer 的
//! build_container_records/build_block_records/RowView/build_attribution_tree +
//! 行级结构判定（expand_zone_to_blocks/expand_zone_to_containers/
//! container_log_end/has_list_frame_at——迁入 infer）+
//! 渲染诊断标志（INLINE_DBG/T_*——同批迁入）。

use std::sync::Arc;
use crate::block::{BlockFrame, FrameKind, BlockType, frame_kind,
    entry_line, entry_is_pop, entry_is_body_mut, entry_frame};
use crate::projection::Snapshot;
// D6⑤i：container_log_end/expand_zone_to_containers 旧签名自 infer 迁入
// （span_end→infer::end_for_rule、snap_to_depth0/depth_at→new_infer——
// C4 收尾——旧签名保留至 compute_zone 编排升级）。


// 渲染诊断族已迁 projection（D6⑤）。
use crate::projection::{INLINE_DBG, T_INLINE, T_BLOCK, T_ATTR,
    T_SHELL, T_SEC_SHELL, T_TR, T_PARA_INLINE,
    BUF_DBG, BUF_ENSURE, BUF_CAP_START, BUF_CAP_FINAL};
// ResolveView 契约已迁 effect.rs（S8——语义层自足）
use crate::effect::ResolveView;
// C2 家归位：定位原子走 new_infer（P6 逐值等价护航）
use crate::infer::item_subtree_end;

/// 全文档路径：虚根 → 目标节点。 /ct/2/ct/1 表示为 [2, 1]。
/// 双形态：值（child 不可变派生）与栈（push/pop 递归累积——diff_level）。
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MD_PATH(Vec<usize>);

impl MD_PATH {
    fn root() -> Self { MD_PATH(Vec::new()) }
    fn push(&mut self, idx: usize) { self.0.push(idx); }
    fn pop(&mut self) -> Option<usize> { self.0.pop() }
    /// 出 patch：clone + push + 序列化一次完成（分配只发生在有 patch 时）。
    fn with_child(&self, idx: usize) -> String {
        let mut p = self.0.clone();
        p.push(idx);
        MD_PATH(p).to_patch_string()
    }
    fn to_patch_string(&self) -> String {
        #[cfg(feature = "telemetry")]
        let _sp_dl_path = crate::telemetry_span!("dl_path", "seg" => self.0.len());
        let mut s = String::with_capacity(self.0.len() * 6);
        for i in &self.0 { s.push_str(&format!("/ct/{i}")); }
        s
    }
}

/// 首编辑起点字节 → 该快照上的行（anchor 单一来源——seam 描述符与
/// compute_zone 共用）。钳制防御：多编辑事务后段字节可能在中间坐标系、
/// > snap.byte_len，crop::line_of_byte 越界 panic；空表回退 0 = 无前缀知识、判定式最保守（空表仅在测试构造
/// 出现，生产 Edit 必带编辑）。
pub(crate) fn anchor_line(snap: &Snapshot, edit_ranges: &[(u32, u32)]) -> usize {
    // [§13-14 #5 修] 取**最小起点**（位置序）而非首个元素：edit_ranges 由
    // session 按**记录序** drain（`edit_spans.drain`，无排序），多 hunk 下首元素未必是最左编辑
    // ⇒ 前缀恒等边界被破坏，expand_zone_ce_blocks_fl 的 opener <= anchor 守卫会
    // 对位置在 anchor 之后的 hunk 放行、ce 扩展被错误短路（外部审计 #5，已复核）。
    // 取 min 后「anchor 之前的内容未变」由构造保证（所有编辑起点 ≥ anchor）。
    edit_ranges
        .iter()
        .map(|&(sb, _)| sb)
        .min()
        .map(|sb| snap.line_of_byte((sb as usize).min(snap.byte_len().saturating_sub(1))))
        .unwrap_or(0)
}

/// 行号坐标变换——后缀偏移公式：
/// 编辑点前的行号不偏移 δ，编辑点后的行号平移 δ。
#[derive(Debug, Clone)]
/// pub（zone_oracle_equivalence 集成工具消费——compute_zone 公共验证面）。
pub struct LineMap {
    pub delta: isize,
    pub edit_old: usize,
    pub edit_new: usize,
}

impl LineMap {
    /// 单锚：首个编辑区首行（与 session 层 ripple 的 min 聚合语义一致）。
    pub fn new(task: &IncrTask) -> Self {
        Self::from_parts(task.delta, &task.snap_old, &task.snap, &task.edit_ranges)
    }

    /// 无任务形态（cache 物化路径 zone_both_sides——与 new 同源同钳制语义，
    /// 复制漂移归一见 anchor_line）。
    pub(crate) fn from_parts(
        delta: isize,
        snap_old: &Snapshot,
        snap_new: &Snapshot,
        edit_ranges: &[(u32, u32)],
    ) -> Self {
        LineMap {
            delta,
            edit_old: anchor_line(snap_old, edit_ranges),
            edit_new: anchor_line(snap_new, edit_ranges),
        }
    }
    pub fn to_new(&self, line: usize) -> usize {
        if line <= self.edit_old {
            line
        } else if self.delta >= 0 {
            line.saturating_add(self.delta as usize)
        } else {
            line.saturating_sub((-self.delta) as usize)
        }
    }
    pub fn to_old(&self, line: usize) -> usize {
        if line <= self.edit_new {
            line
        } else if self.delta >= 0 {
            line.saturating_sub(self.delta as usize)
        } else {
            line.saturating_add((-self.delta) as usize)
        }
    }
    /// 编辑区后的 zone 尾对称对齐：old/new zone 尾通过映射对应。
    /// max 保守：保留两侧各自扩展的较大者。
    /// pub(crate)（compute_zone 与缓存物化共用——fix md §0 结论 2 单侧版
    /// 含 stage4 对称对齐；禁止复制）。
    pub(crate) fn symmetrize_ce(&self, ce_o: usize, ce_n: usize, n_old: usize, n_new: usize) -> (usize, usize) {
        let ce_o_final = ce_o.max(self.to_old(ce_n)).min(n_old);
        let ce_n_final = self.to_new(ce_o_final).max(ce_n).min(n_new);
        (ce_o_final, ce_n_final)
    }
    /// 树起点锚：编辑点前的 depth-0 行——两侧取 min（保守对齐）。
    fn align_zs(&self, zs_o: usize, zs_n: usize) -> (usize, usize) {
        let a = zs_o.min(zs_n);
        (a, a)
    }
}

/// zone → 全文档 坐标系变换。在 diff_patches_mode 一处构造，diff_level 全程使用。
#[derive(Debug, Clone)]
struct ZoneTransform {
    align_doc: MD_PATH,  // align 容器全文档路径，align=0 时为 root
    first_old: usize,    // zone[0] 在旧文档中的 child index 偏移
    first_new: usize,    // zone[0] 在新文档中的 child index 偏移
}

impl ZoneTransform {
    /// zs 所在块的真实起点(off 计数用——case 1253 切块修正)。
    /// 精确来源:kernel_block_starts 二分(快照有 Base——严格排序的真实
    /// 块起点,零构建);fallback:非 cont 扫描 + 确认探针(zs 侧,与
    /// zone_align_to_blocks 同律——正确性由 patch gate 3000 裁决)。
    /// **不共用 zone_align_to_blocks**:off 计数对 zs 对齐的语义敏感
    /// (无 slot 路径的 off 场景——incr_bullet_append_twice 实证),独立
    /// 实现避免物化对齐的语义漂移传染 diff 侧。
    fn block_start_before(snap: &Snapshot, line: usize) -> usize {
        // 表查询语义唯一实现（两处分支共用）：含 line 的块起点；不在任何块内
        // （空隙/EOF）时按探针语义返回 line 本身。
        let spans_lookup = |spans: &[(u32, u32)]| -> usize {
            let idx = spans.partition_point(|&(s, _)| (s as usize) <= line);
            if idx > 0 {
                let (s, e) = spans[idx - 1];
                if (line as u32) < e {
                    return s as usize;
                }
            }
            line.min(snap.line_len())
        };
        // [路径统一 阶段二·改动5] **Edit 节点不走全表的构建**：`block_spans` 的
        // Edit 分支是 O(M) 重建（cache.rs 三截拷贝 + sort），而本查询只要"含 line
        // 的块起点"。Base 侧维持索引二分（表建一次，O(log M) 查询划算）。
        // [复核补记·只读快路] 但**已初始化**的格无需重建——同一趟 compute_zone 的
        // `old_block_index_of_own` 恒 Some，常已把旧侧格烘热。故先只读探测一次，
        // 命中即二分返回；未热则完全退回原路径（探针 / 表构建），语义不变
        // （判官 `block_start_before_matches_probe` 逐值对拍）。
        let edit_node = snap
            .cache_slot
            .upgrade()
            .and_then(|s| s.node.load_full())
            .map(|n| matches!(&**n, crate::cache::CacheNode::Edit { .. }))
            .unwrap_or(false);
        if edit_node {
            // [S2 首批] 站点声明：`UseIfReady`（先查后建，含继承腿）+ Cheap 回退（下方探针）。
            // 与既有 `peek_block_spans_cell` 语义等价（Edit 分支按构造无 prev_snapshot，
            // 继承腿今天不会命中），差别只在"策略写在站点上"。
            if let Some(spans) = snap.block_spans_cell_if_ready() {
                return spans_lookup(&spans);
            }
        } else {
            if let Some(spans) = crate::cache::kernel_block_spans_full(snap) {
                return spans_lookup(&spans);
            }
            // 回退 1：Base 的 block_starts 二分(精确真实起点)。
            if let Some(starts) = crate::cache::kernel_block_starts(snap) {
                let idx = starts.partition_point(|&s| (s as usize) <= line);
                if idx > 0 {
                    return starts[idx - 1] as usize;
                }
                return 0;
            }
        }
        Self::block_start_before_probe(snap, line)
    }

    /// 旧语义 zs 探针（A3-0b 起降为回退 + 测试 oracle）：指数扩大找
    /// 「包含 line 的块起点」。**不共用 `zone_align_to_blocks`**：off 计数
    /// 对 zs 对齐的语义敏感（无 slot 路径的 off 场景——`incr_bullet_append_twice`
    /// 实证），独立实现避免物化对齐的语义漂移传染 diff 侧。
    fn block_start_before_probe(snap: &Snapshot, line: usize) -> usize {
        let n = snap.line_len();
        let mut z = line.min(n);
        let mut span = 8usize;
        loop {
            let lo = z.saturating_sub(span);
            let probe = crate::cache::Segment::build(snap, lo, (z + 1).min(n));
            match probe
                .blocks
                .iter()
                .find(|b| (b.start as usize) <= z && z < (b.end as usize))
            {
                Some(b) => {
                    let bs = b.start as usize;
                    if bs > lo {
                        return bs;
                    }
                    if lo == 0 {
                        return 0;
                    }
                    span = span.saturating_mul(8);
                }
                None => return z,
            }
        }
    }

    /// 根级偏移计算（A3-0：调用方上提一次，两处共用——`for_root` 与
    /// `ctx_off_*` 的实参逐字相同）。
    /// 切块修正（patch gate 3000 case 1253）：zs 是 depth-0 锚定（非块
    /// 锚定）——可能切在块内（```` ```rust ```` 等）。此时树 root[0] 是
    /// 截断块（start=zs），root_child_index(zs) 高估（把 zs 前起点的
    /// 截断块也计入）→ off 错位 → patch 绝对索引越界。故计数前先取 zs
    /// 所在块的真实起点（kernel_block_starts 二分 / 扫描+确认）。
    /// [红线 1] L2 恒产 CF 事实——根计数恒走 CF（root_child_index）；
    /// HS 根计数由 L3 适配层的 `hs_off_before` 直读 `SecsCell.root_starts`。
    /// `reuse_new_block_start`：A-1 守卫（由调用方证明）。两侧查询同一锚 α 且
    /// `α + 1 < ripple_line_start` 时，[0, α] 前缀逐字节恒等 ⇒ 新侧块起点 =
    /// 旧侧块起点，可省一次新侧探针/表查询；守卫不成立时保持原路径。
    fn root_offsets(snap_old: &Snapshot, snap_new: &Snapshot, zs_o: usize, zs_n: usize,
                    reuse_new_block_start: bool)
        -> (usize, usize) {
        // 纯插桩（单 span 多 tag——自定义分析按 tag 拆分，不新增 span 数量）：
        // 四步子调用各自耗时 + 两侧是否走索引（idx_* false = 回退 O(doc) oracle）。
        #[cfg(feature = "telemetry")]
        let mut tick = crate::telemetry::ticks_ns();
        let zs_o_a = Self::block_start_before(snap_old, zs_o);
        #[cfg(feature = "telemetry")]
        let d_bsb_o = { let now = crate::telemetry::ticks_ns(); let d = now - tick; tick = now; d };
        // [A-1] 守卫成立 ⇒ 直接复用旧侧结果（同一锚 + 前缀恒等）；否则原路计算。
        let zs_n_a = if reuse_new_block_start {
            zs_o_a
        } else {
            Self::block_start_before(snap_new, zs_n)
        };
        #[cfg(feature = "telemetry")]
        let d_bsb_n = { let now = crate::telemetry::ticks_ns(); let d = now - tick; tick = now; d };
        // A3-1：优先走索引（O(log M)）；无槽/链断回退逐行 oracle。
        let o = snap_old
            .root_count_before(zs_o_a)
            .unwrap_or_else(|| root_child_index(snap_old, zs_o_a));
        #[cfg(feature = "telemetry")]
        let d_rcb_o = { let now = crate::telemetry::ticks_ns(); let d = now - tick; tick = now; d };
        // [A-0′ 实测负收益] 新侧计数**不可**复用旧侧值：新侧 `root_count_before`
        // 的构建同时充当**下一代的预热**（下一写里该快照成为 snap_old，其 RootTable 格已热；
        // 本次构建又常走 `inherit_cell_from_prev` 的便宜三截继承）。复用后新侧格永不物化 ⇒
        // 下一次写的旧侧查询落冷路径（裸建 / 全量 oracle）：实测 root_count p50
        // 13.9µs → 140µs（10×），`rtb_edit` 消失但总代价更高。
        let n = snap_new
            .root_count_before(zs_n_a)
            .unwrap_or_else(|| root_child_index(snap_new, zs_n_a));
        #[cfg(feature = "telemetry")]
        let d_rcb_n = { let now = crate::telemetry::ticks_ns(); let d = now - tick; tick = now; d };
        #[cfg(feature = "telemetry")]
        {
            let _sp = crate::telemetry_span!("root_count_detail",
                "bsb_o" => d_bsb_o, "bsb_n" => d_bsb_n, "rcb_o" => d_rcb_o, "rcb_n" => d_rcb_n,
                "idx_o" => snap_old.peek_root_table_cell(false),
                "idx_n" => snap_new.peek_root_table_cell(false));
        }
        (o, n)
    }

    /// 根级偏移的组装（A3-0 起偏移由调用方传入——同一对值不再算两遍）。
    fn for_root(first_old: usize, first_new: usize) -> Self {
        ZoneTransform { align_doc: MD_PATH::root(), first_old, first_new }
    }

    fn for_align(align_doc: MD_PATH, ci: usize) -> Self {
        ZoneTransform { align_doc, first_old: ci, first_new: ci }
    }

    /// 初始 diff_level 参数：(旧 base path, 新 base path, 旧 offset, 新 offset)。
    fn init_paths(&self) -> (MD_PATH, MD_PATH, usize, usize) {
        (self.align_doc.clone(), self.align_doc.clone(), self.first_old, self.first_new)
    }
}

// ── BufWriter — 绕过 Vec::len() 检查的裸指针直写 ──
// 所有方法都是 safe: 不变量 (无其他代码修改 Vec) 由类型系统保证。
pub(crate) struct BufWriter<'a> {
    buf: &'a mut Vec<u8>,
    ptr: *mut u8,
    pos: usize,
    cap: usize,
}

impl<'a> BufWriter<'a> {
    pub(crate) fn new(buf: &'a mut Vec<u8>) -> Self {
        let ptr = buf.as_mut_ptr();
        let cap = buf.capacity();
        let pos = buf.len();
        if BUF_DBG.with(|c| *c.borrow()) {
            BUF_CAP_START.with(|c| {
                let mut c = c.borrow_mut();
                if *c == 0 { *c = cap; }
            });
        }
        BufWriter { buf, ptr, pos, cap }
    }

    #[inline(always)]
    fn ensure(&mut self, extra: usize) {
        if self.pos + extra > self.cap {
            self.buf.reserve(self.pos + extra);
            self.ptr = self.buf.as_mut_ptr();
            self.cap = self.buf.capacity();
            if BUF_DBG.with(|c| *c.borrow()) {
                BUF_ENSURE.with(|c| *c.borrow_mut() += 1);
                BUF_CAP_FINAL.with(|c| *c.borrow_mut() = self.cap);
            }
        }
    }

    #[inline(always)]
    pub(crate) fn write_byte(&mut self, b: u8) {
        self.ensure(1);
        unsafe { *self.ptr.add(self.pos) = b; }
        self.pos += 1;
    }

    #[inline(always)]
    pub(crate) fn write_slice(&mut self, s: &[u8]) {
        self.ensure(s.len());
        unsafe { self.ptr.add(self.pos).copy_from_nonoverlapping(s.as_ptr(), s.len()); }
        self.pos += s.len();
    }

    pub(crate) fn write_u32(&mut self, n: u32) {
        if n == 0 { self.write_byte(b'0'); return; }
        let mut tmp = [0u8; 10];
        let mut i = 10;
        let mut v = n;
        while v > 0 { i -= 1; tmp[i] = b'0' + (v % 10) as u8; v /= 10; }
        self.write_slice(&tmp[i..]);
    }

    /// JSON-escape + entity-decode a string（裸指针直写）。
    pub(crate) fn write_str_escaped(&mut self, s: &str) {
        #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("push_esc");
        let bytes = s.as_bytes();
        // 常见控制（\n \r \t）一次 memchr3 定界；其前用单遍 SWAR
        // 同时找 `"` `\` `&` 与稀有控制（<0x20——区间内无 \n\r\t，
        // 命中即稀有）。每段 1 次 SIMD + 1 次 SWAR。
        let find_special = |from: usize| -> Option<usize> {
            let tail = &bytes[from..];
            let n = memchr::memchr3(b'\n', b'\r', b'\t', tail);
            let end = n.unwrap_or(tail.len());
            let ones = 0x0101_0101_0101_0101u64;
            let h80 = 0x8080_8080_8080_8080u64;
            let has_lt_20 = |w: u64| {
                w.wrapping_sub(0x2020_2020_2020_2020u64) & !w & h80 != 0
            };
            let has_byte = |w: u64, b: u8| {
                let x = w ^ (ones * b as u64);
                x.wrapping_sub(ones) & !x & h80 != 0
            };
            let mut special = None;
            let mut off = 0usize;
            while off + 8 <= end {
                let word = u64::from_le_bytes(tail[off..off + 8].try_into().unwrap());
                if has_lt_20(word)
                    || has_byte(word, b'"')
                    || has_byte(word, b'\\')
                    || has_byte(word, b'&')
                {
                    for j in 0..8 {
                        let b = tail[off + j];
                        if b < 0x20 || b == b'"' || b == b'\\' || b == b'&' {
                            special = Some(off + j);
                            break;
                        }
                    }
                    break;
                }
                off += 8;
            }
            if special.is_none() {
                special = tail[off..end].iter().position(|&b| {
                    b < 0x20 || b == b'"' || b == b'\\' || b == b'&'
                }).map(|p| off + p);
            }
            match (special, n) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            }
        };
        let mut i = 0usize;
        while i < bytes.len() {
            match find_special(i) {
                None => {
                    self.write_slice(&bytes[i..]);
                    break;
                }
                Some(offset) => {
                    if offset > 0 { self.write_slice(&bytes[i..i + offset]); }
                    i += offset;
                    let b = bytes[i];
                    match b {
                        b'"'  => self.write_slice(b"\\\""),
                        b'\\' => self.write_slice(b"\\\\"),
                        b'\n' => self.write_slice(b"\\n"),
                        b'\r' => self.write_slice(b"\\r"),
                        b'\t' => self.write_slice(b"\\t"),
                        b'&' => {
                            if let Some((ch, skip)) = decode_html_entity(&bytes[i..]) {
                                match ch {
                                    b'"'  => self.write_slice(b"\\\""),
                                    b'\\' => self.write_slice(b"\\\\"),
                                    c if c < 0x20 => self.write_unicode_escape(c),
                                    c => self.write_byte(c),
                                }
                                i += skip;
                                continue;
                            }
                            self.write_byte(b'&');
                        }
                        b if b < 0x20 => self.write_unicode_escape(b),
                        _ => self.write_byte(b),
                    }
                    i += 1;
                }
            }
        }
    }

    /// [F23] 仅转义 JSON 必需的 `\\` 与 `"`（与旧写法
    /// `s.replace('\\',"\\\\").replace('"',"\\\"")` **逐字节等价**）。
    /// 不复用 `write_str_escaped`：后者含 HTML 实体转义（`&` 等），不属于 JSON 语义。
    pub(crate) fn write_json_escaped(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let mut start = 0usize;
        for i in 0..bytes.len() {
            let b = bytes[i];
            if b == b'\\' || b == b'"' {
                if i > start { self.write_slice(&bytes[start..i]); }
                self.write_byte(b'\\');
                self.write_byte(b);
                start = i + 1;
            }
        }
        if start < bytes.len() { self.write_slice(&bytes[start..]); }
    }

    fn write_unicode_escape(&mut self, c: u8) {
        self.write_slice(b"\\u00");
        let hi = c >> 4;
        let lo = c & 0xF;
        self.write_byte(if hi < 10 { b'0' + hi } else { b'a' + hi - 10 });
        self.write_byte(if lo < 10 { b'0' + lo } else { b'a' + lo - 10 });
    }

    pub(crate) fn commit(self) {
        unsafe { self.buf.set_len(self.pos); }
    }

    pub(crate) fn peek_last(&self) -> Option<u8> {
        if self.pos == 0 { None }
        else { unsafe { Some(*self.ptr.add(self.pos - 1)) } }
    }

    pub(crate) fn pop_last(&mut self) {
        if self.pos > 0 { self.pos -= 1; }
    }
}

/// Try to decode an HTML entity. Returns (decoded_byte, bytes_consumed).
fn decode_html_entity(bytes: &[u8]) -> Option<(u8, usize)> {
    if bytes.len() < 4 { return None; }
    match &bytes[..4] {
        b"&lt;" => Some((b'<', 4)),
        b"&gt;" => Some((b'>', 4)),
        _ => {
            if bytes.len() >= 5 {
                match &bytes[..5] {
                    b"&amp;" => Some((b'&', 5)),
                    b"&#39;" => Some((b'\'', 5)),
                    _ => {
                        if bytes.len() >= 6 {
                            match &bytes[..6] {
                                b"&quot;" => Some((b'"', 6)),
                                b"&#x27;" => Some((b'\'', 6)),
                                _ => None,
                            }
                        } else { None }
                    }
                }
            } else { None }
        }
    }
}

// ── inline 直写 ──

fn write_inline_node(bw: &mut BufWriter<'_>, nc: &mut bool, source: &str,
                     idx: usize, inlines: &[crate::inline::InlineNode],
                     refmap: &crate::inline::RefMap, children: &[smallvec::SmallVec<[u16; 2]>]) {
    use crate::effect::ContentField;
    let node = &inlines[idx];
    let geo = node.frame.geo_kind();

    // Fast path: Plain nodes (~85% of all inline nodes) — skip effect table
    if geo == crate::inline::GeoKind::Plain {
        #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("inl_plain");
        let sub = node.frame.plain_sub();
        if sub == 1 {
            if *nc { bw.write_byte(b','); } *nc = true;
            bw.write_slice(b"{\"nt\":\"sb\",\"ct\":\" \"}");
            return;
        }
        if sub == 2 {
            if *nc { bw.write_byte(b','); } *nc = true;
            bw.write_slice(b"{\"nt\":\"lb\",\"ct\":\"\\n\"}");
            return;
        }
        if node.byte_start >= node.byte_end { return; }
        if *nc { bw.write_byte(b','); } *nc = true;
        let raw = &source[node.byte_start as usize..node.byte_end as usize];
        bw.write_slice(b"{\"nt\":\"txt\",\"ct\":\"");
        bw.write_str_escaped(raw);
        bw.write_slice(b"\"}");
        return;
    }

    #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("inl_eff");
    let eff = crate::effect::inline_effect_for(geo, node.frame);
    let nt = eff.node_type;
    let is_container = eff.is_container;
    #[cfg(feature = "telemetry")] let _s_p = crate::telemetry_span!("inl_eff_proj");
    let projected_content = crate::inline::project(node, source, refmap, crate::inline::InlineField::Content);
    let projected_tag = crate::inline::project(node, source, refmap, crate::inline::InlineField::Tag);
    #[cfg(feature = "telemetry")] drop(_s_p);
    let child_idxs = &children[idx];

    if *nc { bw.write_byte(b','); }
    *nc = true;
    bw.write_slice(b"{\"nt\":\"");
    bw.write_slice(nt.as_bytes());
    bw.write_slice(b"\",");

    if !child_idxs.is_empty() && is_container {
        bw.write_slice(b"\"ct\":[");
        let mut cn = false;
        for &ci in child_idxs {
            write_inline_node(bw, &mut cn, source, ci as usize, inlines, refmap, children);
        }
        bw.write_slice(b"],");
        #[cfg(feature = "telemetry")] let _s_a = crate::telemetry_span!("inl_eff_attr");
        write_inline_attrs(bw, node, source, refmap, eff, &projected_content, &projected_tag, true);
        #[cfg(feature = "telemetry")] drop(_s_a);
    } else {
        let content_str = &*projected_content;
        // ContentField::json_spec 推导（JSON key 语义从渲染器移到声明层——
        // 变体自身是数据——渲染器读数据）
        let cf_spec = eff.content_field.json_spec();
        if !cf_spec.json_key.is_empty() {
            let value: std::borrow::Cow<'_, str> = if let Some(s) = cf_spec.static_val {
                std::borrow::Cow::Borrowed(s)
            } else if cf_spec.decode {
                crate::inline::decode_entity(content_str)
            } else {
                std::borrow::Cow::Borrowed(content_str)
            };
            if !cf_spec.optional || !value.is_empty() {
                bw.write_slice(b"\"");
                bw.write_slice(cf_spec.json_key.as_bytes());
                bw.write_slice(b"\":\"");
                bw.write_str_escaped(&value);
                bw.write_slice(b"\",");
            }
        }
        #[cfg(feature = "telemetry")] let _s_a2 = crate::telemetry_span!("inl_eff_attr");
        write_inline_attrs(bw, node, source, refmap, eff, &projected_content, &projected_tag, false);
        #[cfg(feature = "telemetry")] drop(_s_a2);
    }
    // Remove trailing comma — replace with '}'
    if bw.peek_last() == Some(b',') { bw.pop_last(); }
    bw.write_byte(b'}');
}

/// autolink href 归一化（CM spec §6.5/6.6）：email（无 scheme）加 mailto: 前缀；
/// URI scheme 一律转小写（`<MAILTO:...>` → `mailto:...`）。
fn normalize_autolink_href(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let has_scheme = memchr::memchr(b':', bytes)
        .map(|cp| cp >= 2 && bytes[..cp].iter().all(|&b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.'))
        .unwrap_or(false);
    let s = if !has_scheme && bytes.contains(&b'@') {
        let mut out = String::with_capacity(raw.len() + 7);
        out.push_str("mailto:");
        out.push_str(raw);
        out
    } else {
        raw.to_string()
    };
    if let Some(colon) = s.find(':') {
        let scheme = &s[..colon];
        if !scheme.is_empty() && scheme.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.') {
            let mut out = String::with_capacity(s.len());
            out.push_str(&scheme.to_ascii_lowercase());
            out.push_str(&s[colon..]);
            return out;
        }
    }
    s
}

fn write_inline_attrs(
    bw: &mut BufWriter<'_>, node: &crate::inline::InlineNode,
    source: &str, refmap: &crate::inline::RefMap,
    eff: &crate::effect::InlineEffect,
    projected_content: &std::borrow::Cow<'_, str>,
    projected_tag: &std::borrow::Cow<'_, str>,
    skip_content_for_container_link: bool,
) {
    use crate::effect::AttrSource;
    use crate::inline::InlineField;
    use std::borrow::Cow;
    for attr in eff.attr_fields {
        if skip_content_for_container_link && attr.name == "ct" { continue; }
        let value: Cow<'_, str> = match attr.source {
            AttrSource::Projected(field) => match field {
                InlineField::Content => Cow::Borrowed(projected_content.as_ref()),
                InlineField::Tag => Cow::Borrowed(projected_tag.as_ref()),
                _ => crate::inline::project(node, source, refmap, field),
            },
            AttrSource::ProjectedDecoded(field) => {
                let raw = match field {
                    InlineField::Content => Cow::Borrowed(projected_content.as_ref()),
                    InlineField::Tag => Cow::Borrowed(projected_tag.as_ref()),
                    _ => crate::inline::project(node, source, refmap, field),
                };
                if raw.is_empty() { Cow::Borrowed("") } else { Cow::Owned(crate::inline::decode_link_dest(&raw).into_owned()) }
            }
            AttrSource::ProjectedAutolinkHref => {
                let raw = projected_content.as_ref();
                if raw.is_empty() {
                    Cow::Borrowed("")
                } else {
                    Cow::Owned(normalize_autolink_href(raw))
                }
            }
            AttrSource::DecodedEntity => crate::inline::decode_entity(projected_content),
            AttrSource::Static(s) => Cow::Borrowed(s),
        };
        if attr.optional && value.is_empty() { continue; }
        bw.write_slice(b"\"");
        bw.write_slice(attr.name.as_bytes());
        bw.write_slice(b"\":\"");
        bw.write_str_escaped(&value);
        bw.write_slice(b"\",");
    }
}

fn write_inline_children_json(bw: &mut BufWriter<'_>, nc: &mut bool,
                              source: &str, scratch: &crate::inline::InlineScratch,
                              refmap: &crate::inline::RefMap) {
    #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("inl_wcj");
    let inlines = &scratch.inlines;
    let children = &scratch.children;
    let n = inlines.len();
    if n == 0 { return; }
    #[cfg(feature = "telemetry")] let _s_r = crate::telemetry_span!("inl_roots");
    let mut roots: Vec<usize> = (0..n).filter(|&i| inlines[i].parent == crate::inline::PARENT_NONE).collect();
    roots.sort_by_key(|&i| inlines[i].byte_start);
    #[cfg(feature = "telemetry")] drop(_s_r);
    for &i in &roots {
        write_inline_node(bw, nc, source, i, inlines, refmap, children);
    }
}

// ── 块直写 ──

/// 通道数组写到“喂 inline 那一面自己的键”上（M3：键名查 BLOCK_ATTRS 表，
/// 消灭 "ct" 字面量与“面与键名漂移”）。`want_tag` = 该面的 inline 输入来自
/// Tag 面（标题）→ `t`；否则（段落/引用/列表项）→ `ct`。
fn inline_feed_attr_name(t: crate::block::BlockType, want_tag: bool) -> &'static str {
    use crate::effect::{BlockAttrSrc, BLOCK_ATTRS};
    for attr in BLOCK_ATTRS[t as usize] {
        if want_tag && matches!(attr.src, BlockAttrSrc::Tag) { return attr.name; }
        if !want_tag && matches!(attr.src, BlockAttrSrc::Content) { return attr.name; }
    }
    if want_tag { "t" } else { "ct" }
}

/// 块 attrs 渲染——规格从 BLOCK_ATTRS[t] 参数传入
/// （壳声明驱动——拓扑的 ShellDecl.attrs——行为不变——
/// CONTAINER_TOPOLOGY.shells 引用同一 BLOCK_ATTRS 数组）。
fn write_block_attrs(bw: &mut BufWriter<'_>, snap: &Snapshot, spans: &dyn crate::effect::ResolveView,
                     node: &crate::block::BlockNode,
                     attrs: &[crate::effect::BlockAttrSpec],
                     suppress_content: bool, suppress_tag: bool) {
    #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("render_attrs");
    use crate::effect::BlockAttrSrc;
    use crate::block::BlockProj;
    let t = node.node_type;
    let is_loose_li = node.loose
        && crate::effect::block_decl(t).is_list_item;
    for attr in attrs {
        if attr.suppress_when_loose && is_loose_li { continue; }
        match attr.src {
            BlockAttrSrc::Level => {
                if let Some(lv) = crate::block::header_level(t) {
                    bw.write_slice(b"\"lv\":");
                    bw.write_u32(lv as u32);
                    bw.write_byte(b',');
                }
            }
            BlockAttrSrc::RefDefTitle => {
                let raw = snap.read_block_proj(node, BlockProj::Raw);
                if let Some((_, _, title)) = crate::inline::parse_refdef_multi(&raw) {
                    // WC-1：RefDef 的字段键同样恒在——空 title 写空串（原：!title.is_empty() 守卫）。
                    bw.write_slice(b"\"ti\":\"");
                    bw.write_str_escaped(&title);
                    bw.write_slice(b"\",");
                }
            }
            BlockAttrSrc::RefDefLabel | BlockAttrSrc::RefDefHref => {
                // label/href 不在 Tag/Content 投影（RefDef 无内容）——raw 额外提取。
                let raw = snap.read_block_proj(node, BlockProj::Raw);
                if let Some((label, url, _)) = crate::inline::parse_refdef_multi(&raw) {
                    let v = if matches!(attr.src, BlockAttrSrc::RefDefLabel) { label } else { url };
                    // WC-1：同上（空值写空串，原：!v.is_empty() 守卫）。
                    bw.write_slice(b"\"");
                    bw.write_slice(attr.name.as_bytes());
                    bw.write_slice(b"\":\"");
                    bw.write_str_escaped(&v);
                    bw.write_slice(b"\",");
                }
            }
            proj_src => {
                // 抑制：该面以数组表达时不再写字符串（一面一键）。
                if suppress_content && matches!(proj_src, BlockAttrSrc::Content) { continue; }
                if suppress_tag && matches!(proj_src, BlockAttrSrc::Tag) { continue; }
                let proj = match proj_src {
                    BlockAttrSrc::Tag => BlockProj::Tag,
                    BlockAttrSrc::Content => BlockProj::Content,
                    BlockAttrSrc::Raw => BlockProj::Raw,
                    _ => continue,
                };
                // 通用零拷贝快路径（P2——Raw/段落单行/CF 借用，其余回退 String）
                // hb 的 Content attr:统一渲染入口(剥容器前缀后整块原文)
                let mut _sc = Vec::new();
                let v: std::borrow::Cow<'_, str> = if t == crate::block::BlockType::HtmlBlock
                    && matches!(proj_src, BlockAttrSrc::Content)
                {
                    std::borrow::Cow::Owned(snap.hb_block_content(node))
                } else if t == crate::block::BlockType::Paragraph
                    && matches!(proj_src, BlockAttrSrc::Content)
                {
                    // O(n²) 修复：段落 ct 不再经 read_block_proj 的 per-段落重放——
                    // 直接走链供给零重放路径（与 normalized_content 逐字节同语义）。
                    std::borrow::Cow::Owned(paragraph_content_normalized(snap, spans, node))
                } else {
                    snap.read_block_proj_cow(node, proj, &mut _sc)
                };
                // 字段键恒在（patch_6902_plan §2.1 + 任务卡 WC-1）：可空**字段**取空也写键（空串）。
                // 只覆盖块层字段键；结构通道（容器/列表项/章节壳的 ct 数组）与行内层不在本卡。
                // 原实现：if attr.optional && v.is_empty() { continue; }
                bw.write_slice(b"\"");
                bw.write_slice(attr.name.as_bytes());
                bw.write_slice(b"\":\"");
                bw.write_str_escaped(&v);
                bw.write_slice(b"\",");
            }
        }
    }
}

fn write_block_attrs_pre(bw: &mut BufWriter<'_>, snap: &Snapshot, node: &crate::block::BlockNode,
                          tag: &str, content: &str,
                          suppress_content: bool, suppress_tag: bool) {
    #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("walk_attrs");
    use crate::effect::{BlockAttrSrc, BLOCK_ATTRS};
    let _t0 = if INLINE_DBG.with(|c| *c.borrow()) {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let t = node.node_type;
    let is_loose_li = node.loose
        && crate::effect::block_decl(t).is_list_item;
    for attr in BLOCK_ATTRS[t as usize] {
        if attr.suppress_when_loose && is_loose_li { continue; }
        match attr.src {
            BlockAttrSrc::Level => {
                if let Some(lv) = crate::block::header_level(t) {
                    bw.write_slice(b"\"lv\":");
                    bw.write_u32(lv as u32);
                    bw.write_byte(b',');
                }
            }
            BlockAttrSrc::RefDefTitle => {
                let raw_title = snap.read_block_proj(node, crate::block::BlockProj::Raw);
                if let Some((_, _, title)) = crate::inline::parse_refdef_multi(&raw_title) {
                    // WC-1：同上（空 title 写空串）。
                    bw.write_slice(b"\"ti\":\"");
                    bw.write_str_escaped(&title);
                    bw.write_slice(b"\",");
                }
            }
            BlockAttrSrc::RefDefLabel | BlockAttrSrc::RefDefHref => {
                // label/href 不在 Tag/Content 投影（RefDef 无内容）——raw 额外提取。
                let raw_ref = snap.read_block_proj(node, crate::block::BlockProj::Raw);
                if let Some((label, url, _)) = crate::inline::parse_refdef_multi(&raw_ref) {
                    let v = if matches!(attr.src, BlockAttrSrc::RefDefLabel) { label } else { url };
                    // WC-1：同上（空值写空串）。
                    bw.write_slice(b"\"");
                    bw.write_slice(attr.name.as_bytes());
                    bw.write_slice(b"\":\"");
                    bw.write_str_escaped(&v);
                    bw.write_slice(b"\",");
                }
            }
            proj_src => {
                #[cfg(feature = "telemetry")] let _s_r = crate::telemetry_span!("attr_read");
                // C1:内容以通道数组表达(富文本拆段)时,Content attr 抑制——
                // 避免 ct 键双义(文本 vs 内容流)。
                if suppress_content && matches!(proj_src, BlockAttrSrc::Content) { continue; }
                // 同律推广到 Tag 面：Tag 以数组写进自己的键(t)时，抑制 t 字符串。
                if suppress_tag && matches!(proj_src, BlockAttrSrc::Tag) { continue; }
                let val = match proj_src {
                    BlockAttrSrc::Tag => tag,
                    BlockAttrSrc::Content => content,
                    BlockAttrSrc::Raw => &snap.read_block_proj(node, crate::block::BlockProj::Raw),
                    _ => continue,
                };
                // 字段键恒在（同 write_block_attrs；WC-1）。
                // 原实现：if attr.optional && val.is_empty() { continue; }
                bw.write_slice(b"\"");
                bw.write_slice(attr.name.as_bytes());
                bw.write_slice(b"\":\"");
                bw.write_str_escaped(val);
                bw.write_slice(b"\",");
            }
        }
    }
    if let Some(t0) = _t0 {
        T_ATTR.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
    }
}

fn write_inline_children_pre(bw: &mut BufWriter<'_>, nc: &mut bool,
                         snap: &Snapshot, node: &crate::block::BlockNode,
                         tag: &str, content: &str,
                         tag_has_inline: bool,
                         content_has_inline: bool,
                         scratch: &mut crate::inline::InlineScratch) {
    #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("walk_inline");
    let _t0 = if INLINE_DBG.with(|c| *c.borrow()) {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let t = node.node_type;
    let spec = crate::block::block_proj_spec(t);

    if tag_has_inline {
        #[cfg(feature = "telemetry")] let _s_sc = crate::telemetry_span!("inline_scan");
        #[cfg(feature = "telemetry")] let _s_cl = crate::telemetry_span!("inl_clear");
        scratch.clear();
        #[cfg(feature = "telemetry")] drop(_s_cl);
        #[cfg(feature = "telemetry")] let _s_go = crate::telemetry_span!("inl_getref");
        let refmap = crate::inline::EMPTY_REFMAP.get_or_init(crate::inline::RefMap::new);
        #[cfg(feature = "telemetry")] drop(_s_go);
        crate::inline::scan_inline_into(tag, &refmap, 0, scratch);
        #[cfg(feature = "telemetry")] drop(_s_sc);
        if !scratch.inlines.is_empty() {
            #[cfg(feature = "telemetry")] let _s_w = crate::telemetry_span!("inline_write_json");
            write_inline_children_json(bw, nc, tag, scratch, &refmap);
        } else {
            #[cfg(feature = "telemetry")] { let _s_e = crate::telemetry_span!("il_empty"); }
        }
    }

    if content_has_inline {
        // 原 `can_inline && !matches!(t, CodeFenceOpen|CodeFenceClose|CodeContent)`——
        // CF 三兄弟 inline_mode=None——can_inline（ContentOnly|TagThenContent）
        // 已排除——冗余条件删除（inline_mode 声明单源）
        let can_inline = matches!(spec.inline_mode, crate::block::InlineFeed::ContentOnly | crate::block::InlineFeed::TagThenContent);
        if can_inline {
            #[cfg(feature = "telemetry")] let _s_sc = crate::telemetry_span!("inline_scan");
            #[cfg(feature = "telemetry")] let _s_cl = crate::telemetry_span!("inl_clear");
            scratch.clear();
            #[cfg(feature = "telemetry")] drop(_s_cl);
            #[cfg(feature = "telemetry")] let _s_go = crate::telemetry_span!("inl_getref");
            let refmap = crate::inline::EMPTY_REFMAP.get_or_init(crate::inline::RefMap::new);
            #[cfg(feature = "telemetry")] drop(_s_go);
            crate::inline::scan_inline_into(content, &refmap, 0, scratch);
            #[cfg(feature = "telemetry")] drop(_s_sc);
            if !scratch.inlines.is_empty() {
                #[cfg(feature = "telemetry")] let _s_w = crate::telemetry_span!("inline_write_json");
                write_inline_children_json(bw, nc, content, scratch, &refmap);
            } else {
                #[cfg(feature = "telemetry")] { let _s_e = crate::telemetry_span!("il_empty"); }
            }
        }
        if let Some(t0) = _t0 {
            T_INLINE.with(|c| {
                *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0;
            });
        }
    }
}

/// Try zero-copy inline content for list items with geometry.
/// peel_layers: item 的列表帧层数（`- - foo` 外层=1/内层=2——
/// 内容剥 (k-1) 层 marker——外层保留内层 marker（`- foo`——嵌套——
/// 抑制判定用）——内层剥到内容（"foo"））。
fn try_list_inline_zero_copy<'a>(
    snap: &'a Snapshot,
    node: &crate::block::BlockNode,
    geo: &crate::block::LineGeo,
    t: crate::block::BlockType,
    peel_layers: usize,
    line_start: usize,
    scratch: &'a mut Vec<u8>,
) -> Option<std::borrow::Cow<'a, str>> {
    // LineSpec mark_width 类判定——BulletDash|OrderedListNum 子集单射
    // Char1PlusOptWs|DigitsDelimPlusWs
    let is_item_marker = matches!(crate::effect::line_spec(t).mark_width,
        crate::effect::MarkWidth::Char1PlusOptWs
        | crate::effect::MarkWidth::DigitsDelimPlusWs);
    if !is_item_marker { return None; }
    let is_bullet = matches!(crate::effect::line_spec(t).mark_width,
        crate::effect::MarkWidth::Char1PlusOptWs);
    let bq_off = node.peel_width;
    let content_start = if is_bullet && geo.tag_range.start < geo.tag_range.end {
        let pos = line_start + bq_off + geo.tag_range.end;
        // 边界守卫（写面收口计划第一步）：tag 末端恰在行末（- k: 末行无尾
        // 换行）时 byte_at(pos+1) 越界——取 0 与行尾语义一致。
        let sep_byte = if pos < snap.byte_len() { snap.byte_at(pos) } else { 0 };
        let after_byte = if pos + 1 < snap.byte_len() { snap.byte_at(pos + 1) } else { 0 };
        let sep_len = crate::infer::colon_sep_len(sep_byte, after_byte);
        pos + sep_len
    } else {
        line_start + bq_off + geo.content_offset
    };
    if content_start >= node.byte_range.end { return Some(std::borrow::Cow::Borrowed("")); }
    let full = snap.byte_range_str(content_start..node.byte_range.end, scratch);
    let full_bytes = full.as_bytes();
    let nl_count = full_bytes.iter().filter(|&&b| b == b'\n').count();
    if nl_count > 1 { return None; }
    // #255：item 内容段并入续行后 full 含多行内容（\n 后非空——非行尾
    // 换行）——零拷贝不剥续行缩进——fallback 到 node_content 的
    // dedent_continuation 路径（CM §5.2 内容剥内容列缩进）
    if nl_count == 1 && !full.trim_end_matches(['\n', '\r']).is_empty()
        && full.trim_end_matches(['\n', '\r']).contains('\n')
    {
        return None;
    }
    if is_bullet {
        let content_end = if nl_count == 1 && full_bytes.last() == Some(&b'\n') { full_bytes.len() - 1 } else { full_bytes.len() };
        if content_end > 0 {
            let last = full_bytes[content_end - 1];
            if last == b' ' || last == b'\t' { return None; }
        }
        let trimmed = if nl_count == 1 && full_bytes.last() == Some(&b'\n') { &full[..full.len() - 1] } else { full };
        // L1:剥 marker 前先剥夹层 BQ(- > - b 型——BQ 夹在列表层间;
        // keep_indent=true:非 BQ 前导空格是值内容的一部分(' 123'——
        // selector 语义固化),不得剥)。
        let trimmed = crate::block::str_view(crate::block::strip_bq(trimmed.as_bytes(), true));
        // 按层剥内层 marker（peel_markers——剥 (k-1) 层——外层
        // （k=1）剥 0 保留嵌套 marker（`- foo`——抑制判定）——内层
        // （k=2）剥 1 到文本（"foo"）——与 read_block_proj 路径共用——
        // F/P 一致）
        return Some(std::borrow::Cow::Borrowed(
            crate::block::peel_markers(trimmed, peel_layers.saturating_sub(1))));
    }
    // OL 分支——同样按层剥内层 marker（`- 1. y` 的内层——`1. y`→"y"——
    // C3' OLI 收束:面值去尾换行(与 list_content 同语义——F/P 一致;
    // 旧注释"尾换行保留(OL 语义)"撤销——行尾 \n 是行几何非内容)。
    let ol_full = if full_bytes.last() == Some(&b'\n') {
        &full[..full.len() - 1]
    } else {
        full
    };
    // L1:剥 marker 前先剥夹层 BQ(同 bullet 分支——单源;keep_indent=true)。
    let ol_full = crate::block::str_view(crate::block::strip_bq(ol_full.as_bytes(), true));
    Some(std::borrow::Cow::Borrowed(
        crate::block::peel_markers(ol_full, peel_layers.saturating_sub(1))))
}

/// Zero-copy tag extraction from Rope.
fn extract_tag_str<'a>(
    snap: &'a Snapshot,
    node: &crate::block::BlockNode,
) -> (std::borrow::Cow<'a, str>, bool) {
    extract_tag_str_line(snap, node, snap.rope.line(node.start_line))
}

/// extract_tag_str 的行供给变体（性能专项）：行由调用方预取（顺序 LineSupply
/// 摊销 O(1)/行），免 per-node rope.line 树下降（度量重算＋gap 切片）。
/// 与 extract_tag_str 逐字节同输出——行尾 \n 有无对 tag 提取无影响
/// （tag 定位在行首、range 不含行尾）。
fn extract_tag_str_line<'a>(
    snap: &'a Snapshot,
    node: &crate::block::BlockNode,
    line: crop::RopeSlice<'a>,
) -> (std::borrow::Cow<'a, str>, bool) {
    use crate::block::BlockType as BT;
    // C3 渲染层识别(改动层级 = 渲染输出,不动共享词法/归属树):ck 在
    // effect/scan 层本质是 BLT(帧位)——渲染树节点类型恒 Bullet(容器
    // 类型分层),行内容才是 ck 真相。此处按行检测 checklist 形态 →
    // tag 走 ck 词法(find_ckl_tag:无键 = 状态字符 x/X/空格、有键 = 键),
    // 与读面(node_tag/行类型 Checklist)同语义。
    if node.node_type == BT::Bullet {
        // rope 行可能跨 chunk 边界——单 chunk 零拷贝,多 chunk 复制后判
        // (ck 识别必须覆盖两者,否则同文档两路径渲染分叉)。
        let mut chunks = line.chunks();
        let first = chunks.next();
        let single = first.map_or(false, |f| f.len() == line.byte_len());
        let mut owned: Option<String> = None;
        let raw: &str = if let Some(f) = first {
            if single {
                f
            } else {
                owned = Some(line.to_string());
                owned.as_deref().unwrap()
            }
        } else {
            ""
        };
        let stripped = crate::block::str_view(crate::block::strip_bq(raw.as_bytes(), false));
        if crate::block::is_checklist_marker(stripped.as_bytes()) {
            let range = crate::block::find_ckl_tag(
                stripped.as_bytes(),
                crate::block::CHECKLIST_LIST_MARK_END,
            );
            if range.start < range.end && range.end <= stripped.len() {
                let tag = &stripped[range];
                let has_inline = tag
                    .as_bytes()
                    .iter()
                    .any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0);
                return (std::borrow::Cow::Owned(tag.to_string()), has_inline);
            }
        }
    }
    // setext（HsHead1/2）：tag = 全部段落文本（剥下划线行——827 合并:
    // setext_title_from_raw 共享单点,与 infer node_tag 同源——端界修复后
    // 下划线行恒为块末行,"去末行"与"扫描下划线行"等价)。
    if matches!(node.node_type, BT::HsHead1 | BT::HsHead2) {
        let raw = snap.rope.byte_slice(node.byte_range.clone()).to_string();
        let tag = crate::block::setext_title_from_raw(&raw);
        let has_inline = tag.as_bytes().iter().any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0);
        return (std::borrow::Cow::Owned(tag), has_inline);
    }
    // 空 tag 快路径（等价 node_tag：TagRule::None 恒空；ColonKey/
    // CheckboxColonKey 且 geometry tag_range 空时也在 geometry 分支直接
    // 返回空）——免一次 read_block_proj 空 String 提取。
    if matches!(crate::effect::line_spec(node.node_type).tag_rule,
            crate::effect::TagRule::None)
        && node.geometry.as_ref()
            .map_or(true, |g| g.tag_range.start >= g.tag_range.end)
    {
        return (std::borrow::Cow::Borrowed(""), false);
    }
    if matches!(crate::effect::line_spec(node.node_type).tag_rule,
            crate::effect::TagRule::ColonKey | crate::effect::TagRule::CheckboxColonKey)
        && node.geometry.as_ref()
            .map_or(false, |g| g.tag_range.start >= g.tag_range.end)
    {
        return (std::borrow::Cow::Borrowed(""), false);
    }
    if let Some(ref geo) = node.geometry {
        if geo.tag_range.start < geo.tag_range.end {
            // ③：几何分支复用调用方已供的行（ls 顺序供给）——原 shadow
            // 重读 rope.line(node.start_line) 纯浪费（per-node gap 分裂，
            // RawLineMetric/rope_slice::from 采样实锤）；行内容与行尾 \n
            // 语义对 tag 提取无差（1027 已注明）。
            let mut chunks = line.chunks();
            if let Some(first) = chunks.next() {
                if first.len() == line.byte_len() {
                    let trimmed = crate::block::str_view(crate::block::strip_bq(first.as_bytes(), false));
                    let end = geo.tag_range.end.min(trimmed.len());
                    if geo.tag_range.start < end {
                        let tag = &trimmed[geo.tag_range.start..end];
                        let tag = if crate::block::is_header(node.node_type)
                        {
                            tag.trim()
                        } else {
                            tag
                        };
                        let has_inline = tag.as_bytes().iter().any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0);
                        if node.node_type == crate::block::BlockType::CodeFenceOpen {
                            return (crate::inline::decode_link_dest(tag), has_inline);
                        }
                        return (std::borrow::Cow::Borrowed(tag), has_inline);
                    }
                }
            }
        }
    }
    let tag = snap.read_block_proj(node, crate::block::BlockProj::Tag);
    let has_inline = tag.as_bytes().iter().any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0);
    (std::borrow::Cow::Owned(tag), has_inline)
}

/// 段落归一（O(n²) 修复——链零重放供给；与 NodeProj::normalized_content
/// 逐字节同语义：raw 含 `>` → 折叠剥前缀；否则 paragraph_text_normalize）。
fn paragraph_content_normalized(
    snap: &Snapshot,
    spans: &dyn crate::effect::ResolveView,
    node: &crate::block::BlockNode,
) -> String {
    let mut sc = Vec::new();
    let raw = snap.byte_range_str(node.byte_range.clone(), &mut sc);
    if raw.as_bytes().iter().any(|&b| b == b'>') {
        crate::infer::paragraph_text_normalize(&crate::infer::paragraph_lines_stripped(
            &raw,
            &snap.frame_log.entries,
            node.start_line as u32,
            &spans.chain_frames_at(node.start_line),
        ))
    } else {
        crate::infer::paragraph_text_normalize(raw)
    }
}

/// 顺序行供给（性能专项）：render 树按 start_line 单调推进——crop Lines
/// 迭代器分摊 O(1)/行（scan 侧 [perf 0830] 同款 102→24ns/行），替代
/// per-node rope.line 树下降（度量重算＋gap 切片＋分配）。
/// S1（性能专项）：跨根共享版渲染用——projection 的 ③ 分支需在循环外
/// 持有它，故从模块私有提为 pub(crate)。行为零变化。
pub(crate) struct LineSupply<'a> {
    lines: crop::iter::Lines<'a>,
    idx: usize,
    byte_pos: usize, // 性能专项·③：已消费行的累计字节起点（行字节长＋1 递推）
    cached: Option<(usize, crop::RopeSlice<'a>, usize)>, // 幂等：同行重复请求直接命中
}
impl<'a> LineSupply<'a> {
    pub(crate) fn new(rope: &'a crop::Rope) -> Self {
        Self { lines: crop::iter::Lines::from(rope), idx: 0, byte_pos: 0, cached: None }
    }
    /// 推进到 line 并返回该行（调用方保证单调不减；行内容无行尾 \n——
    /// 与 rope.line 的差异对 tag 提取无影响，extract_tag_str_line 已注明）。
    fn line_at(&mut self, line: usize) -> crop::RopeSlice<'a> {
        self.line_at_with_off(line).0
    }
    /// 带字节起点版（③）：off = 该行在 rope 的字节偏移——与
    /// rope.byte_of_line(line) 逐值一致（行字节长＋1 递推；\r\n 文件的
    /// \r 含在行字节内、+1 为 \n——偏移仍对）。**幂等**：同一行重复
    /// 请求命中缓存，不重复拉取（write_block_node 内 tag 行与内容行同源）。
    fn line_at_with_off(&mut self, line: usize) -> (crop::RopeSlice<'a>, usize) {
        if let Some((cl, cs, co)) = self.cached.as_ref() {
            if *cl == line {
                return (cs.clone(), *co);
            }
        }
        debug_assert!(line >= self.idx, "LineSupply 只支持单调前进");
        while self.idx < line {
            let s = self.lines.next().expect("LineSupply 行越界");
            self.byte_pos += s.byte_len() + 1;
            self.idx += 1;
        }
        let s = self.lines.next().expect("LineSupply 行越界");
        let off = self.byte_pos;
        self.byte_pos += s.byte_len() + 1;
        self.idx += 1;
        self.cached = Some((line, s.clone(), off));
        (s, off)
    }
}

pub fn write_block_node<'r>(bw: &mut BufWriter<'_>, nc: &mut bool,
                                snap: &'r Snapshot, spans: &dyn crate::effect::ResolveView,
                                node: &crate::block::BlockNode,
                                scratch: &mut crate::inline::InlineScratch,
                                claims: &dyn ClaimLookup,
                                ls: &mut LineSupply<'r>) {
    #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("blk_node");
    use crate::block::BlockType as BT;
    let _t0 = if INLINE_DBG.with(|c| *c.borrow()) {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let t = node.node_type;
    let nt_str = crate::effect::compact_type_name(t);

    if *nc { bw.write_byte(b','); }
    *nc = true;

    bw.write_slice(b"{\"nt\":\"");
    bw.write_slice(nt_str.as_bytes());
    bw.write_slice(b"\",");

    // 性能专项：IC 流式渲染——逐行 dedent 后直接逃逸写入，消 content
    // String 组装（与 write_block_attrs_pre(IC) 逐字节一致：t 恒在空值
    // 写空串（WC-1——extract_tag_str 对 TagRule::None∧geometry None 快路径
    // 返回空）、cd 走 Content、suppress 恒 false——inline_mode=RawText
    // 不喂 inline；claims 键同尾）。
    if t == BT::IndentedCode {
        bw.write_slice(b"\"t\":\"\",\"cd\":\"");
        // 性能专项·O(N²) 修复：链由零重放供给（chain_frames_at=覆盖链含本行
        // 自开——与旧 start_stack 同语义）——弹掉本行自开帧回到严格行前栈
        // （与旧 start_stack−own 逐帧一致），免 per-node rebuild_stack_at。
        let mut seed = spans.chain_frames_at(node.start_line);
        let own = crate::infer::entries_pushes_at(&snap.frame_log.entries, node.start_line as u32);
        for _ in 0..own {
            seed.pop();
        }
        snap.node_proj().content_code_ic_stream_seeded(node, &seed, |s| bw.write_str_escaped(s));
        bw.write_slice(b"\"");
        write_claims_key(bw, claims, node.start_line as u32, node.end_line as u32);
        bw.write_byte(b'}');
        // 早退分支自计（性能专项：T_BLOCK 的尾部累加被早退跳过——此处补齐，
        // 否则 IC 流式时间不入账，诊断失明）。
        if let Some(t0) = _t0 {
            T_BLOCK.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
        }
        return;
    }

    // 性能专项：围栏流式渲染（同 IC——content_code_fence_stream 与
    // write_block_attrs_pre(CF) 逐字节一致：t=Lang tag、cd=Content，
    // CODE_FENCE_ATTRS 序=t→cd）。仅 tag 无行内触发时走流式（有触发
    // 走原数组通道——逐字节保持）。
    if t == BT::CodeFenceOpen {
        let f_line = ls.line_at(node.start_line);
        let (f_tag, f_tag_inline) = extract_tag_str_line(snap, node, f_line);
        if !f_tag_inline {
            bw.write_slice(b"\"t\":\"");
            bw.write_str_escaped(&f_tag);
            bw.write_slice(b"\",\"cd\":\"");
            snap.node_proj().content_code_fence_stream(node, |s| bw.write_str_escaped(s));
            bw.write_slice(b"\"");
            write_claims_key(bw, claims, node.start_line as u32, node.end_line as u32);
            bw.write_byte(b'}');
            // 早退分支自计（同 IC 分支）。
            if let Some(t0) = _t0 {
                T_BLOCK.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
            }
            return;
        }
    }

    let mut _cs: Vec<u8> = Vec::new();
    let line = ls.line_at(node.start_line);
    let (tag, tag_has_inline) = extract_tag_str_line(snap, node, line);
    // inline_mode 声明分派——替代硬编码 3 类型排除：
    //   None → 空（CF 三兄弟原排除 + Comment/HtmlElement/RefDef 壳——不调
    //     write_block_node——安全；ThematicBreak leaf——原 node_content
    //     None_ 也空——一致）
    //   RawText + HtmlBlock → 原始（原特判）
    //   其余（含 IndentedCode 的 RawText——content_code 剥 4——内容语义
    //     非原始——原案"RawText→原始"会破坏 ct——修正）
    let spec = crate::block::block_proj_spec(t);
    let content: std::borrow::Cow<'_, str> = match spec.inline_mode {
        crate::block::InlineFeed::None if t == BT::Comment => {
            // 单行 Comment 无帧(Path B 块)——原假设"Comment 不调
            // write_block_node(走壳)"被打破——inline_mode=None 但内容
            // 存在(range_content:开标记后到闭标记前)——读面同源输出。
            snap.read_block_proj_cow(node, crate::block::BlockProj::Content, &mut _cs)
        }
        crate::block::InlineFeed::None => std::borrow::Cow::Borrowed(""),
        crate::block::InlineFeed::RawText if t == BT::HtmlBlock => {
            // 统一渲染入口:剥容器前缀后的整块原文(与 diff 侧同源)
            std::borrow::Cow::Owned(snap.hb_block_content(node))
        }
        _ => {
            if t == BT::Paragraph && node.peel_width == 0 {
                // 段落内容统一收束（CM §4.8——normalize_spec(Paragraph)=StripTrailing——
                // 语义单源——与 node_content 的 paragraph_text_normalize 一致——
                // 类型匹配是场景识别——语义在 effect.rs）。
                // item 内容块 BQ 段落（`- > b`——peel_width=0——strip BQ 失败）——
                // 交替剥除（BQ+marker）——`- > b` → `b`
                let raw = snap.byte_range_str(node.byte_range.clone(), &mut _cs);
                if raw.as_bytes().iter().any(|&b| b == b'>') {
                    // P16 修复：剥 BQ+marker 后须收束（normalize_spec=StripTrailing——
                    // CM §4.8）——与 projection normalized_content 同源——
                    // 修复前仅剥不归（BQ 段落内容含前导缩进分歧）。
                    // 8.2：(All,Chain) ctx 折叠（chain=行前栈、条目供数）。
                    // O(n²) 修复：链由 view.chain_row 零重放供给。
                    std::borrow::Cow::Owned(
                        crate::infer::paragraph_text_normalize(
                            &crate::infer::paragraph_lines_stripped(
                                &raw, &snap.frame_log.entries, node.start_line as u32,
                                &spans.chain_frames_at(node.start_line))))
                } else if !raw.contains('\n') {
                    // 单行借用（trim_start to_string 拷贝→借用——零拷贝——
                    // 语义等价（paragraph_text_normalize 单行 = trim_start））
                    std::borrow::Cow::Borrowed(raw.trim_start())
                } else {
                    std::borrow::Cow::Owned(
                        crate::infer::paragraph_text_normalize(raw))
                }
            } else if t == BT::Paragraph {
                // O(n²) 修复：BQ 内段落（peel_width>0）不再经 read_block_proj_cow
                // 的 per-段落 rebuild_stack_at——统一走链零重放供给（与
                // peel_width==0 臂同语义：raw 含 `>` → 折叠剥前缀；否则归一）。
                std::borrow::Cow::Owned(paragraph_content_normalized(snap, spans, node))
            } else if let Some(ref geo) = node.geometry {
                // 层数经 ResolveView 直读(与 ItemLeaf 的 list_frame_count 同构——
                // 途径③归一化):该行的列表帧数 = 外层 marker + 内层 marker
                // (BQ 不计——与原词法剥离逐值一致:`> - x` 栈[BQ,BLT]→1、
                // `- - x` 栈[BLT,BLT]→2——外层由 content_offset 已剥)。
                let k = list_frame_count(spans, node.start_line,
                    spans.close_depth_at(node.start_line));
                // ③：行字节起点走顺序供给（byte_of_line 单调递推）——
                // 与 rope.byte_of_line 逐值一致。
                let (_, nl_off) = ls.line_at_with_off(node.start_line);
                match try_list_inline_zero_copy(snap, node, geo, t, k, nl_off, &mut _cs) {
                    Some(v) => v,
                    None => snap.read_block_proj_cow(
                        node, crate::block::BlockProj::Content, &mut _cs),
                }
            } else {
                snap.read_block_proj_cow(node, crate::block::BlockProj::Content, &mut _cs)
            }
        }
    };
    // may_have_ch 用 inline_mode 门控（CHILDREN_SHAPE None_ 违反：IC(RawText)
    // content 含 TRIGGER 但 inline_mode=None 不渲染 inline——ch 开空 `"ct":[]`
    // 残留——tag/content 的 inline 输入按 inline_mode 声明（TagOnly/
    // TagThenContent/ContentOnly）——TagThenContent 两者皆可）
    let can_inline_tag = matches!(spec.inline_mode,
        crate::block::InlineFeed::TagOnly | crate::block::InlineFeed::TagThenContent);
    let can_inline_content = matches!(spec.inline_mode,
        crate::block::InlineFeed::ContentOnly | crate::block::InlineFeed::TagThenContent);
    // 性能专项：inline 不喂的类型（RawText/None——IC 即此）跳过全量
    // TRIGGER_LUT 扫描——200MB code 文档省一次全量扫描（行为中性：
    // can_inline_content=false ⇒ may_have_ch 恒 false，与旧扫描结论同）。
    let content_has_inline = can_inline_content
        && !content.is_empty()
        && content.as_bytes().iter().any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0);
    // C1:content 有富文本拆段且该类型 content 可喂 inline 时——内容以
    // 通道 ct 数组表达,抑制 attrs 的 Content(ct 键单义)。
    let suppress_content = content_has_inline
        && matches!(spec.inline_mode,
            crate::block::InlineFeed::ContentOnly | crate::block::InlineFeed::TagThenContent);
    // Tag 面同律：喂 inline 的 Tag 面以数组表达时抑制其字符串写（含触发即数组）。
    let suppress_tag = tag_has_inline
        && matches!(spec.inline_mode,
            crate::block::InlineFeed::TagOnly | crate::block::InlineFeed::TagThenContent);
    write_block_attrs_pre(bw, snap, node, tag.as_ref(), content.as_ref(), suppress_content, suppress_tag);

    // 值包 p 收敛后 write_block_node 只收"块 leaf"(非 ItemLeaf——item 恒
    // 走 write_item_leaf)——旧 loose wrapper(is_loose_li)分支无触发路径,
    // 删除;内容通道 = inline 拆段直写。
    let may_have_ch = (can_inline_tag && tag_has_inline)
        || (can_inline_content && content_has_inline);
    if may_have_ch {
        // 键名查表（M3）：Tag 面喂 inline → `t`；Content 面喂 inline → `ct`。
        // TagThenContent 声明存在但当前无类型使用；两者皆真时按 Tag 面（旧行为单数组）。
        let feed_key = if can_inline_tag && tag_has_inline {
            inline_feed_attr_name(t, true)
        } else {
            inline_feed_attr_name(t, false)
        };
        bw.write_slice(b"\"");
        bw.write_slice(feed_key.as_bytes());
        bw.write_slice(b"\":[");
        if tag_has_inline || content_has_inline {
            write_inline_children_pre(bw, &mut false, snap, node, tag.as_ref(), content.as_ref(), tag_has_inline, content_has_inline, scratch);
        }
        bw.write_slice(b"]");
        write_claims_key(bw, claims, node.start_line as u32, node.end_line as u32);
        bw.write_byte(b'}');
    } else {
        let last = bw.peek_last();
        if last == Some(b',') { bw.pop_last(); }
        write_claims_key(bw, claims, node.start_line as u32, node.end_line as u32);
        bw.write_byte(b'}');
    }
    if let Some(t0) = _t0 {
        T_BLOCK.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
    }
}

/// 归属查找（N5 渲染期注入；P 值路径与无注册 F 帧传 `&()` ⇒ 输出逐字节不变）。
pub trait ClaimLookup {
    fn claims(&self, start: u32, end: u32) -> Option<&[std::sync::Arc<str>]>;
    /// 无认领 ⇒ true（默认）；非空 ⇒ 禁用字节复用（复用字节不含 match 键）。
    fn is_empty(&self) -> bool {
        true
    }
}
impl ClaimLookup for () {
    #[inline]
    fn claims(&self, _start: u32, _end: u32) -> Option<&[std::sync::Arc<str>]> {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Patch {
    Add     { path: String, value: String },
    Remove  { path: String },
    Replace { path: String, value: String },
    /// 面级粒度(proj):字段级替代——path 定位**节点对象**(内部形态保留
    /// path/attr 两段;线路形态见 store.rs:路径改写为 `path/attr`,WC-3),
    /// attr 为节点 JSON 字段名(t/cd/ct…)。
    /// **编码契约(WC-4)**:value 是**已序列化的裸 JSON 片段**——字符串带引号
    /// (`"txt"`)、行内数组带方括号(`[{...}]`),由 store.rs 原样内插进帧,
    /// 不得再包一层 JSON 字符串。
    ReplaceAttr { path: String, attr: String, value: String },
}

pub struct IncrTask {
    pub snap: Arc<Snapshot>,
    pub doc_id: u32,
    pub snap_old: Arc<Snapshot>,
    pub delta: isize,
    pub edit_ranges: crate::cache::EditActions,
    pub ripple_line_start: usize,
    pub ripple_line_end: usize,
    pub slot: Option<crate::cache::CacheSlotHandle>,
    /// B2b(#27):旧侧槽锚——表改 Weak 后旧代槽随前任务收尾归零,本锚
    /// 在任务在途期间保活旧代(block_header_lines 旧侧 / old_block_index_of
    /// 命中旧 full,免每代 779µs 自建);收尾半钩子
    /// (B3)对它执行"最后持者取物"。
    pub slot_old: Option<crate::cache::CacheSlotHandle>,
    /// B3 接线:worker 收尾半钩子通道(session 投递时 clone 塞入,免查
    /// sessions 表竞态)——任务收尾对 slot/slot_old 执行"最后持者取物"。
    pub reaper_tx: Option<crate::session::ReaperTx>,
    /// v2 S3：本代是否结构编辑（PL+FL 逐项等价的否定；判据 = session 的
    /// seam_classify 同源信号——唯一来源在 session.rs）。match 的跨写结果缓存
    /// 据此保守失效：结构编辑 ⇒ 全缓存重算（章节边界可能全局移动）。
    /// 写侧字段：生产构造于 session.rs 的 IncrTask 投递，当前无任何读者；
    /// 删除需同步 74 处构造点（含 2 个 bench bin），故保留并标注 write-only。
    pub structural_change: bool,
}

/// 增量入口（默认 = CF 容器拓扑——生产 Path Worker 广播/测试共用——
/// 参数化 = diff_patches_mode）。
/// incr_unify_826：生产路径 = 事实层（diff_patches_mode_with_ctx）+ 适配层
/// （adapt——CF 目标恒等，行为不变；适配层是拓扑唯一入口）。
/// F/P 分派（827 阶段 3 读点 + T-mode 测试统一入口）：同一事实 diff，
/// 按目标拓扑选入口——Hs → hs_patches（适配层），其余 → incr_patches（CF）。
/// 无节文档 HS 与 CF 同构（adapt 恒等）；apply 后 AST 相等由 golden 判官担保。
pub fn patches_for_mode(task: &IncrTask, mode: crate::topology::ProjectionMode) -> Vec<Patch> {
    match mode {
        crate::topology::ProjectionMode::Hs => hs_patches(task),
        _ => incr_patches(task),
    }
}

pub fn incr_patches(task: &IncrTask) -> Vec<Patch> {
    let (patches, ctx) = diff_patches_mode_with_ctx(task, &crate::topology::CONTAINER_TOPOLOGY);
    adapt(task, &patches, &ctx, &crate::topology::CONTAINER_TOPOLOGY)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerModel { Leaf, Nesting, Listing }

fn container_path_model(kind: FrameKind) -> ContainerModel {
    let decl = crate::effect::decl_for(kind);
    if !decl.is_nesting { return ContainerModel::Leaf; }
    if !crate::effect::effect_for(kind).item_leaf_type_str.is_empty() { return ContainerModel::Listing; }
    ContainerModel::Nesting
}

// diff_level 直接消费归属树（唯一树形态），生产走 build_attribution_tree。

/// item 内容段续行判定（两 walker 的 (4.5)——cont 行并入栈顶 ItemLeaf——
/// 不独立产块——与 item_content_end 同一谓词）。
/// pub(crate)（同 item_content_end——sdl 比对对象）
pub(crate) fn is_item_continuation(snap: &Snapshot, spans: &dyn ResolveView, i: usize) -> bool {
    snap.lines[i].cont()
        && spans.resolved_type_at(i) == crate::block::BlockType::Paragraph
        && !spans.block_start_at(i)
}

/// item 的列表帧层数（`- - foo` 的同行双 item——外层 k=1/内层 k=2——
/// 剥取深度依据——(0..=d) 中列表帧计数——`> - a` 的 BQ 非列表不计——
/// 嵌套 `- a\n  - b` 的 b 独立行 k=1）。
// 被 &dyn（DiffCtx/item_inline_roots）调用——&dyn 化（调用点
// &impl 自动 coerce）——与 trait 方法返回类型（child_idx_at(line,d)）对齐。
fn list_frame_count(spans: &dyn ResolveView, i: usize, d: usize) -> usize {
    (0..=d).filter(|&k| crate::effect::block_decl(
        spans.node_type_at_depth(i, k)).is_list_item).count()
}


// item / tight-item 段落 内容形态——渲染与 diff 的单一事实源
// （统一内容提取路径 + 抑制判定——消灭 oracle/渲染漂移——patch gate 3000
//  case 1687 根因:两侧各拼各的原语,一侧修复另一侧漂移）。
// 决策查既有表(effect::apply 呈现修改表);事实走既有 scan(inline.rs);
// 本层只做"按唯一顺序组合"(算法是函数)。


/// item 内容形态(渲染 write_item_leaf 与偏移 item_inline_roots 共用)。
struct ItemInlinePlan<'a> {
    tag: std::borrow::Cow<'a, str>,
    tag_has_inline: bool,
    /// BlockContent 抑制后的内容(块级内容 → 空——与渲染一致)。
    content: std::borrow::Cow<'a, str>,
    content_has_inline: bool,
    is_content_block: bool,
    is_empty_content: bool,
}

/// 值 p 占位(外部形态定案:item 值 = 内容流首 p 块)——内容流前缀槽数:
/// 有文本值 → 1(值 p 占槽 0,树子从 1 起);BlockContent/空 → 0。
/// tag 不再 inline 拆段(身份),富文本值在 p 内——均不占 item 槽。
///
/// 判定必须与渲染侧「值 p 判定」(blk_item_val, :5954) 同条件——按 content
/// 非空,不是按 is_empty_content(shape 级):键化空值 item(- OUTER: + 嵌套
/// 子块)shape 非 EmptyContent 而 content 为空——渲染不发值 p(children 从槽
/// 0 起),shape 判法会多算一槽,children 全被推到 /ct/1(min_repro
/// 「多算一层」:交付树该 item ct 只有 [0] 一个孩子 → 客户端 Not a child)。
/// 真空 item(shape=EmptyContent)有续行并入时 content 非空——仍占槽(与渲染一致)。
fn value_p_prefix(plan: &ItemInlinePlan) -> usize {
    if !plan.is_content_block && !plan.content.is_empty() { 1 } else { 0 }
}

/// 统一 item 内容提取:零拷贝优先(几何路径),拒绝时 fallback 投影 +
/// peel_markers(peel_layers-1)(#255:与零拷贝同 k 层总剥——F/P 一致)。
/// 抑制判定查 effect::apply(呈现修改表——单一事实源)。
fn item_inline_plan<'a>(
    snap: &'a Snapshot,
    node: &crate::block::BlockNode,
    spans: &dyn ResolveView,
    frame_depth: usize,
    peel_layers: usize,
    scratch: &'a mut Vec<u8>,
) -> ItemInlinePlan<'a> {
    #[cfg(feature = "telemetry")] let _sp_dl_plan = crate::telemetry_span!("dl_plan");
    // [0831] tag 归一 extract_tag_str（单行借用零拷贝）——替代 read_block_proj
    // Owned 全拷贝（语义同源——extract_tag_str 与 node_tag 同款快路径）。
    let (tag, tag_has_inline) = {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("dl_tag");
        extract_tag_str(snap, &node)
    };
    // 内容提取:零拷贝优先(多行/嵌套拒绝时 fallback)——单一路径。
    let content_raw: std::borrow::Cow<'a, str> = {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("dl_zc");
        if let Some(ref geo) = node.geometry {
            {
                #[cfg(feature = "telemetry")]
                let _g = crate::telemetry::deep_span("dl_zc_hit");
            if let Some(hit) = try_list_inline_zero_copy(snap, node, geo, node.node_type, peel_layers,
                snap.byte_of_line(node.start_line), scratch) {
                hit
            } else {
                #[cfg(feature = "telemetry")]
                let _g = crate::telemetry::deep_span("dl_zc_fb");
                let proj = snap.read_block_proj_view(&node, crate::block::BlockProj::Content, &|l| spans.chain_frames_at(l));
                // L1:剥 marker 前先剥夹层 BQ(与零拷贝同源;读面已剥时幂等;
                // keep_indent=true——值前导空格是内容)。
                let proj = crate::block::str_view(crate::block::strip_bq(proj.as_bytes(), true));
                std::borrow::Cow::Owned(
                    crate::block::peel_markers(proj, peel_layers.saturating_sub(1)).to_string())
            }
            }
        } else {
            let proj = snap.read_block_proj(&node, crate::block::BlockProj::Content);
            // L1:同上(无 geometry 路径;keep_indent=true)。
            let proj = crate::block::str_view(crate::block::strip_bq(proj.as_bytes(), true));
            std::borrow::Cow::Owned(
                crate::block::peel_markers(proj, peel_layers.saturating_sub(1)).to_string())
        }
    };
    // 抑制判定:item 内容块(BlockContent——内容在 ch 的块节点,不渲染 inline)。
    let rv = crate::effect::apply(spans, node.start_line, true, frame_depth);
    let is_content_block = rv.shape == crate::effect::RenderShape::BlockContent;
    let content: std::borrow::Cow<'a, str> = if is_content_block {
        std::borrow::Cow::Borrowed("")
    } else {
        content_raw
    };
    let content_has_inline = !content.is_empty() && {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("dl_lut");
        content.as_bytes().iter().any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0)
    };
    ItemInlinePlan {
        tag,
        tag_has_inline,
        content,
        content_has_inline,
        is_content_block,
        is_empty_content: rv.shape == crate::effect::RenderShape::EmptyContent,
    }
}

/// inline 根计数(scan 一次——PARENT_NONE 根)。无触发/空 → 0。
fn inline_root_count(src: &str) -> usize {
    if src.is_empty() || !src.as_bytes().iter().any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0) {
        return 0;
    }
    let mut scratch = crate::inline::InlineScratch::new();
    let refmap = crate::inline::EMPTY_REFMAP.get_or_init(crate::inline::RefMap::new);
    crate::inline::scan_inline_into(src, &refmap, 0, &mut scratch);
    scratch.inlines.iter()
        .filter(|inl| inl.parent == crate::inline::PARENT_NONE)
        .count()
}

/// tight-item 段落内容形态(渲染 write_para_inline 与 diff 偏移共用)。
/// 返回 (content, has_inline)——空内容 has_inline=false(0 槽位)。
fn para_inline_plan<'a>(
    snap: &'a Snapshot,
    node: &'a crate::block::BlockNode,
    scratch: &'a mut Vec<u8>,
) -> (std::borrow::Cow<'a, str>, bool) {
    let content = snap.read_block_proj_cow(node, crate::block::BlockProj::Content, scratch);
    let has_inline = content.as_bytes().iter().any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0);
    (content, has_inline)
}

/// P 侧单节点渲染(diff 的 ADD/REPL 值——统一到 render_tree 单根入口——
/// 一份渲染器)。空容器(start>=end)渲染 null(diff 语义——render_tree 无此分支)。
fn subtree_value(ctx: &DiffCtx, tree: &crate::infer::AttrTreeNew, idx: usize) -> String {
    #[cfg(feature = "telemetry")]
    let _sp_dl_render = crate::telemetry_span!("dl_render",
        "lines" => tree.nodes[idx].end.saturating_sub(tree.nodes[idx].start));
    let n = &tree.nodes[idx];
    if n.kind.is_some() && n.start >= n.end { return "null".to_string(); }
    let mut buf = Vec::with_capacity(2048);
    // [G4/红线 3] 值 = L4 渲染产物；L2 只产 CF 事实——渲染恒走 CF 壳表
    // （CONTAINER_TOPOLOGY——L2 契约 = 模式无关事实层）。
    render_tree(&mut buf, ctx.n_snap, ctx.n_spans, tree, idx,
        &crate::topology::CONTAINER_TOPOLOGY);
    unsafe { String::from_utf8_unchecked(buf) }
}


/// tight item 内段落块的 ch 项数（#301——F/P 一致——render_node 渲染 inline
/// 节点——diff_level 的前缀和偏移用）。无 inline 触发 → 1（单 txt 节点）；
/// 空内容 → 0（write_para_inline 不写——ch 无对应项）。
/// 内容形态经 para_inline_plan 单源（与渲染同路径）。
/// snap 必须与树匹配（o_tree → 旧快照 / n_tree → 新快照）——旧坐标在
/// 新快照上 build_node 会越界（大区间替换时 zone 尾段落 end=旧行数）。
fn para_inline_roots_for(snap: &crate::projection::Snapshot,
                         c: &crate::infer::TreeNode) -> usize {
    let node = snap.build_node(c.start as usize, c.end as usize,
        crate::block::BlockType::Paragraph, false);
    let mut scratch = Vec::new();
    let (content, has_inline) = para_inline_plan(snap, &node, &mut scratch);
    if content.is_empty() { return 0; }
    if !has_inline { return 1; }
    inline_root_count(&content)
}

/// tight item 内段落块的**逐槽位**值（与 write_para_inline 同渲染——每
/// inline 根节点一个独立 JSON 字符串）。
///
/// AST 槽位语义（#301）：tight item 的 ch 前缀 = inline 根节点序列——多
/// inline 段落占多槽位——patch Add/REPL 必须逐槽位操作。单值拼接
/// （`{..},{..}`）在槽位处是非法 JSON——多行块语料 patch gate 3000 抓到的
/// 生产 bug（case 69：item 内容 "=== -## h q" 的 Add value）。
fn para_inline_values(ctx: &DiffCtx, c: &crate::infer::TreeNode) -> Vec<String> {
    let mut out = Vec::new();
    let node = ctx.n_snap.build_node(c.start as usize, c.end as usize,
        crate::block::BlockType::Paragraph, false);
    let mut _cs = Vec::new();
    let (content, has_inline) = para_inline_plan(ctx.n_snap, &node, &mut _cs);
    if content.is_empty() { return out; }
    if !has_inline {
        // 单 txt 节点（与 write_para_inline 的非 inline 分支一致）。
        let mut buf = Vec::with_capacity(32);
        let mut bw = BufWriter::new(&mut buf);
        bw.write_slice(b"{\"nt\":\"txt\",\"ct\":\"");
        bw.write_str_escaped(content.as_ref());
        bw.write_slice(b"\"}");
        bw.commit();
        out.push(unsafe { String::from_utf8_unchecked(buf) });
        return out;
    }
    let mut scratch = crate::inline::InlineScratch::new();
    let refmap = crate::inline::EMPTY_REFMAP.get_or_init(crate::inline::RefMap::new);
    crate::inline::scan_inline_into(content.as_ref(), &refmap, 0, &mut scratch);
    let inlines = &scratch.inlines;
    let children = &scratch.children;
    let n = inlines.len();
    let mut roots: Vec<usize> = (0..n)
        .filter(|&i| inlines[i].parent == crate::inline::PARENT_NONE)
        .collect();
    roots.sort_by_key(|&i| inlines[i].byte_start);
    for &i in &roots {
        let mut buf = Vec::with_capacity(64);
        let mut bw = BufWriter::new(&mut buf);
        let mut nc = false;
        write_inline_node(&mut bw, &mut nc, content.as_ref(), i, inlines, &refmap, children);
        bw.commit();
        out.push(unsafe { String::from_utf8_unchecked(buf) });
    }
    out
}

// 字段 dyn 化（结构体字段不支持 impl Trait——消费查询接口）。
// [G4/红线 1] topology 已摘除——L2 一律模式无关（壳语义归 L3 适配层）。
struct DiffCtx<'a> { o_snap: &'a Snapshot, o_spans: &'a dyn ResolveView,
                     n_snap: &'a Snapshot, n_spans: &'a dyn ResolveView,
                     // [§13-4] 窗口（段）末端：节点 end 顶到它、而它未到文末 ⇒ 被窗口截断，
                     // 身份比较退**公共重叠**（两侧都存在的行数）而不是各自的截断投影。
                     o_ze: usize, n_ze: usize }

/// proj 粒度分诊(面级):整块 REPL 前,若同型配对(o/n)的差异收敛于
/// 单面(tag 面 / 叶内容面),返回字段级 ReplaceAttr;否则 None(调用点
/// 回退整块 REPL——等价安全,block 粒度路径不达本函数)。
/// 面值读取零渲染(现成原语:extract_tag_str / item_inline_plan /
/// read_block_proj Content)——不重复 ripple 已做的配对与原文判定,
/// 只在其上做"面定位"。
fn maybe_attr(
    ctx: &DiffCtx,
    o_tree: &crate::infer::AttrTreeNew,
    n_tree: &crate::infer::AttrTreeNew,
    no: &crate::infer::TreeNode,
    nn: &crate::infer::TreeNode,
    path: &str,
) -> Option<Patch> {
    #[cfg(feature = "telemetry")] let _sp_dl_attr = crate::telemetry_span!("dl_attr");
    use crate::block::BlockType as BT;
    // 守卫:children 无渲染差(结构同 + 各自区间内容同——渲染确定性)才可
    // 面级——否则差异可能在子块,面级补丁覆盖不到(item REPL 的 content_eq
    // false 可能源于 children 区——case 120 实测)。
    // 例外:Leaf 容器(CF/hb/rd——children = 内部内容块 CodeContent/
    // HtmlContent——渲染并入壳 attrs 内容,不独立输出)——children 差即
    // 壳内容差(区间字节比较已捕获)——不挡,字段级覆盖。
    let leaf_container = no.kind.is_some()
        && nn.kind.is_some()
        && no.kind == nn.kind
        && container_path_model(no.kind.unwrap()) == ContainerModel::Leaf;
    let children_same = leaf_container
        || (no.children_of(o_tree).len() == nn.children_of(n_tree).len()
            && no.children_of(o_tree)
                .iter()
                .zip(nn.children_of(n_tree).iter())
                .all(|(&oa, &na)| {
                    let (a, b) = (&o_tree.nodes[oa as usize], &n_tree.nodes[na as usize]);
                    a.block_type == b.block_type
                        && a.kind == b.kind
                        && content_eq(ctx, a, b)
                }));
    if !children_same {
        return None;
    }
    // 渲染同源 tmp 区间:容器 = content_end(壳 attrs 内容源块——渲染容器
    // 壳同:content_end=0 回退节点 end);块叶/item = 自身区间。
    let build = |snap: &Snapshot, nd: &crate::infer::TreeNode, use_ce: bool| {
        let end = if use_ce && nd.content_end > 0 {
            nd.content_end as usize
        } else {
            nd.end as usize
        };
        snap.build_node(nd.start as usize, end, nd.block_type, nd.loose())
    };
    let is_container = no.kind.is_some() && nn.kind.is_some();
    let o_node = build(ctx.o_snap, no, is_container);
    let n_node = build(ctx.n_snap, nn, is_container);
    let (o_tag, o_tag_inline) = extract_tag_str(ctx.o_snap, &o_node);
    let (n_tag, n_tag_inline) = extract_tag_str(ctx.n_snap, &n_node);
    // tag 含 inline 触发 → 渲染联动(tag 拆段进 ct 数组/suppress 态翻转)——
    // 单字段 set 无法覆盖(应用不重渲染)——面级不安全 → 回退 REPL。
    let tag_inline_linked = o_tag_inline || n_tag_inline;

    // ── item(值包 p):attrs 只发 t(Content 恒抑制);值 = 通道首 p 的
    // ct(路径 {path}/ct/0——§5.5 注入 p 值变规则)──
    if no.is_item() && nn.is_item() {
        let tag_diff = o_tag != n_tag;
        let read_item = |snap: &Snapshot,
                         nd: &crate::infer::TreeNode,
                         node: &crate::block::BlockNode|
         -> (String, bool) {
            let spans: &dyn ResolveView = if std::ptr::eq(snap, ctx.o_snap) {
                ctx.o_spans
            } else {
                ctx.n_spans
            };
            let fd = nd.seg_depth as usize;
            let pl = list_frame_count(spans, nd.start as usize, nd.seg_depth as usize);
            let mut sc = Vec::new();
            let plan = item_inline_plan(snap, node, spans, fd, pl, &mut sc);
            // WC-4：值按渲染形态取 JSON 片段（字符串 / 行内数组）+ "值是否存在"标志。
            // 注意：**项值为空 ⇒ 整个项的 ct 数组都不存在**（不是空串），那是结构变化，
            // 只能走整块 REPL——故值面字段写要求两侧都有值（present）。
            let present = !plan.content.is_empty();
            (render_face_value_json(snap, node, "", plan.content.as_ref(), false, plan.content_has_inline), present)
        };
        let (o_val, o_present) = read_item(ctx.o_snap, no, &o_node);
        let (n_val, n_present) = read_item(ctx.n_snap, nn, &n_node);
        let val_diff = o_val != n_val;
        let value_path = format!("{path}/ct/0");
        return match (tag_diff, val_diff) {
            // WC-4：tag 面按渲染形态出值（空即 ""；含行内触发即数组），不再因空/触发回退。
            (true, false) => Some(Patch::ReplaceAttr {
                path: path.to_string(),
                attr: "t".into(),
                value: render_face_value_json(ctx.n_snap, &n_node, &n_tag, "", true, n_tag_inline),
            }),
            // 值差：两侧都有值 ⇒ 一条 op（值已是渲染片段，字符串或行内数组）；
            // 任一侧无值 ⇒ 项的 ct 数组存在性变了（结构）→ 回退整块 REPL。
            (false, true) if o_present && n_present => Some(Patch::ReplaceAttr {
                path: value_path,
                attr: "ct".into(),
                value: n_val,
            }),
            _ => None,
        };
    }

    // ── 容器:仅 Leaf 容器(content 在壳 attrs 渲染——CF/hb/rd/he);
    // 非 Leaf(bl/bq/od——children 流 = 结构级 REM/ADD)无字段可发 ──
    if no.kind.is_some() || nn.kind.is_some() {
        let k = no.kind.or(nn.kind)?;
        if no.kind != nn.kind || container_path_model(k) != ContainerModel::Leaf {
            return None;
        }
    }
    if no.block_type != nn.block_type {
        return None;
    }
    // ── 块叶 / Leaf 容器:BLOCK_ATTRS[block_type] 逐字段(表驱动)──
    attrs_field_diff(ctx, &o_node, &n_node, o_tag.as_ref(), n_tag.as_ref(),
        tag_inline_linked, path)
}

/// WC-4：把一个面的**渲染值**序列化成 JSON 片段——字符串（可空）或行内数组。
/// 与渲染端同一读法：能否走数组由该类型的 inline_mode 决定（TagOnly/TagThenContent
/// 的 Tag 面、ContentOnly/TagThenContent 的 Content 面），数组体走
/// write_inline_children_pre（render_node/壳分支同一个函数），字符串走 JSON 引号。
/// 返回值直接充当 \`Patch::ReplaceAttr.value\`——**契约：已序列化的裸 JSON**
/// （store.rs 原样内插；数组绝不能退化成字符串字面量）。
fn render_face_value_json(
    snap: &Snapshot,
    node: &crate::block::BlockNode,
    tag: &str,
    content: &str,
    use_tag: bool,
    has_trigger: bool,
) -> String {
    use crate::block::InlineFeed;
    let spec = crate::block::block_proj_spec(node.node_type);
    let can_inline = if use_tag {
        matches!(spec.inline_mode, InlineFeed::TagOnly | InlineFeed::TagThenContent)
    } else {
        matches!(spec.inline_mode, InlineFeed::ContentOnly | InlineFeed::TagThenContent)
    };
    let s = if use_tag { tag } else { content };
    if !has_trigger || !can_inline {
        return serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string());
    }
    let mut buf = Vec::with_capacity(64);
    {
        let mut bw = BufWriter::new(&mut buf);
        let mut scratch = crate::inline::InlineScratch::new();
        bw.write_slice(b"[");
        write_inline_children_pre(&mut bw, &mut false, snap, node, tag, content,
            use_tag, !use_tag, &mut scratch);
        bw.write_slice(b"]");
        bw.commit();
    }
    unsafe { String::from_utf8_unchecked(buf) }
}

/// 表驱动字段级(BLOCK_ATTRS[t] 逐 attr):读新旧渲染值,单字段差(可字段
/// 化)→ ReplaceAttr{attr};否则 None = 回退整块 REPL(调用点)。值读法 =
/// 渲染同源(read_block_proj Content/Raw、extract_tag_str 的 tag、RefDef
/// raw 解析)——与 write_block_attrs(渲染 attrs 表驱动)同一读法镜像。
/// 守卫:tag 含 inline(联动拆段/suppress 翻转)、Content 差且非叶(富文本
/// 拆段 = 结构级)、optional 空新(渲染省略——set 空串残留空字段)、Level
/// (§5.5:类型/级别变 = 块级 REPL)、多字段差——均回退 None。
fn attrs_field_diff(
    ctx: &DiffCtx,
    o_node: &crate::block::BlockNode,
    n_node: &crate::block::BlockNode,
    o_tag: &str,
    n_tag: &str,
    tag_inline_linked: bool,
    path: &str,
) -> Option<Patch> {
    use crate::block::BlockType as BT;
    use crate::effect::{BlockAttrSrc, BLOCK_ATTRS};
    let attrs = BLOCK_ATTRS[o_node.node_type as usize];
    // WC-4：两侧 tag 面的"含行内触发"标志（与 extract_tag_str 同谓词）——用于按
    // 渲染形态出面值（行内数组 or 字符串）。
    let trig_tag = |s: &str| {
        !s.is_empty()
            && s.as_bytes().iter().any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0)
    };
    let o_tag_inline = trig_tag(o_tag);
    let n_tag_inline = trig_tag(n_tag);
    let mut single: Option<(&'static str, String)> = None;
    for attr in attrs {
        if matches!(attr.src, BlockAttrSrc::Level) {
            // §5.5:类型/级别变 = 块级 REPL(不字段化 lv)。
            continue;
        }
        let (o_v, n_v, safe) = match attr.src {
            BlockAttrSrc::Tag => {
                // WC-4：面值 = **渲染片段**（字符串可空 / 行内数组）——两侧比较片段，
                // 差即字段级；不再因"含行内触发"整体回退（数组形态已有值通道）。
                (
                    render_face_value_json(ctx.o_snap, &o_node, &o_tag, "", true, o_tag_inline),
                    render_face_value_json(ctx.n_snap, &n_node, &n_tag, "", true, n_tag_inline),
                    true,
                )
            }
            BlockAttrSrc::Content => {
                // 内容字段化仅限"内容渲染为单值字符串"的类型(§5.5 表驱动
                // 目标:Paragraph/Bullet 叶内容、CF/IC 的 cd)——结构化内容
                // (Table/TableRow cells、BQ/容器区间等)渲染为通道/结构,
                // 字段级覆盖不到 → 回退整块 REPL。
                let str_content = matches!(
                    o_node.node_type,
                    BT::Paragraph
                        | BT::Bullet
                        | BT::CodeFenceOpen
                        | BT::IndentedCode
                        | BT::Comment
                        | BT::HtmlBlock
                );
                let mut sc = Vec::new();
                // hb:渲染-diff 同源——读统一渲染入口(剥容器前缀整块原文),
                // 与渲染 write_block_attrs 同值(读面 content_raw 剥首尾
                // 是另一契约,不用于字段级)。
                let o_c: std::borrow::Cow<'_, str> = if o_node.node_type == BT::HtmlBlock {
                    std::borrow::Cow::Owned(ctx.o_snap.hb_block_content(o_node))
                } else {
                    ctx.o_snap
                        .read_block_proj_cow_chain(o_node, crate::block::BlockProj::Content, &mut sc,
                            &|l| ctx.o_spans.chain_frames_at(l))
                };
                let mut sc2 = Vec::new();
                let n_c: std::borrow::Cow<'_, str> = if n_node.node_type == BT::HtmlBlock {
                    std::borrow::Cow::Owned(ctx.n_snap.hb_block_content(n_node))
                } else {
                    ctx.n_snap
                        .read_block_proj_cow_chain(n_node, crate::block::BlockProj::Content, &mut sc2,
                            &|l| ctx.n_spans.chain_frames_at(l))
                };
                let trig = |c: &std::borrow::Cow<'_, str>| {
                    !c.is_empty()
                        && c.as_bytes()
                            .iter()
                            .any(|&b| crate::inline::TRIGGER_LUT[b as usize] != 0)
                };
                // WC-4：面值 = **渲染片段**（字符串可空 / 行内数组）；字段化只要求
                // 该类型的内容是"单值面"（str_content；结构型内容仍回退整块）。
                // 两侧比较片段——空串差、形态切换（字符串↔数组）都能一条 op 表达。
                let safe = str_content;
                (
                    render_face_value_json(ctx.o_snap, &o_node, "", &o_c, false, trig(&o_c)),
                    render_face_value_json(ctx.n_snap, &n_node, "", &n_c, false, trig(&n_c)),
                    safe,
                )
            }
            BlockAttrSrc::Raw => {
                let mut sc = Vec::new();
                let o_r = ctx.o_snap
                    .read_block_proj_cow(o_node, crate::block::BlockProj::Raw, &mut sc);
                let mut sc2 = Vec::new();
                let n_r = ctx.n_snap
                    .read_block_proj_cow(n_node, crate::block::BlockProj::Raw, &mut sc2);
                (o_r.to_string(), n_r.to_string(), true)
            }
            // RefDef 不字段化:rd 是 block 层承载的 inline 引用定义
            // (伪装成 block 的 inline node)——u/ti 是 RefMap 的 inline 面
            // 键值,不是 block 渲染 attr——保持整块 REPL(§11.4 裁决)。
            BlockAttrSrc::RefDefLabel | BlockAttrSrc::RefDefHref
            | BlockAttrSrc::RefDefTitle => continue,
            _ => continue,
        };
        if o_v != n_v {
            if !safe {
                return None;
            }
            // WC-4：optional 空新值不再回退——WC-1 之后渲染端对可空字段**恒写键**
            // （空即 ""），故 set 空串正是 full 的形态（"清空 = 赋值空值"）。
            if single.is_some() {
                return None; // 多字段差 → 整块 REPL。
            }
            single = Some((attr.name, n_v));
        }
    }
    let (name, v) = single?;
    Some(Patch::ReplaceAttr {
        path: path.to_string(),
        attr: name.to_string(),
        // WC-4 契约：v 已是**裸 JSON 片段**（字符串含引号 / 数组带括号），不再二次序列化。
        value: v,
    })
}

/// Rope 内容字节比较（零拷贝）。渲染 JSON 是内容的确定性函数——
/// 内容相同 + type/loose 相同 ⟹ 渲染相同，无需渲染即可判定。
fn content_eq(ctx: &DiffCtx, o: &crate::infer::TreeNode, n: &crate::infer::TreeNode) -> bool {
    #[cfg(feature = "telemetry")] let _sp_dl_cmp = crate::telemetry_span!("dl_cmp",
        "lo" => o.end.saturating_sub(o.start), "ln" => n.end.saturating_sub(n.start));
    // 防御:树节点行值越界回退不等(渲染比对兜底)——防 byte_of_line panic 族
    let (on, nn) = (ctx.o_snap.line_len(), ctx.n_snap.line_len());
    if (o.start as usize) > on || (o.end as usize) > on || (n.start as usize) > nn || (n.end as usize) > nn {
        return false;
    }
    let s_o = ctx.o_snap.rope.byte_of_line(o.start as usize);
    let e_o = if (o.end as usize) < ctx.o_snap.line_len() { ctx.o_snap.rope.byte_of_line(o.end as usize) } else { ctx.o_snap.rope.byte_len() };
    let s_n = ctx.n_snap.rope.byte_of_line(n.start as usize);
    let e_n = if (n.end as usize) < ctx.n_snap.line_len() { ctx.n_snap.rope.byte_of_line(n.end as usize) } else { ctx.n_snap.rope.byte_len() };
    let so_slice = ctx.o_snap.rope.byte_slice(s_o..e_o);
    let sn_slice = ctx.n_snap.rope.byte_slice(s_n..e_n);
    // [§13-4 未落地] 端点（尾换行）规范化实测**不安全**，本轮不做：
    // ① 无条件剥尾换行会塌掉代码块内容比较（随机等价 gate case 460 实测失败）；
    // ② 按块型（仅 Paragraph，effect::normalize_spec = StripTrailing）+ 完整窗口守卫后，
    //    SecBody/Add{First,Mid}/Same/Five/Hs 四格又出现 apply != 全量（截断/域树节点被误当完整）。
    // ⇒ G 类（EOF 无尾换行 ⇒ 冗余 REPL）保持为**形态问题**（不变量本就成立），
    // 待 A 层双锚夹落地后在窗口层解决。
    so_slice == sn_slice
}

/// [§13-4] 规范化内容键（**仅身份判定**用；值渲染 / 槽位语义 / 递归判据仍走原始字节）：
/// 行尾符归一（CRLF→LF）与**单个**尾部换行差（文档尾块无终止换行——G 类）。
/// 为什么不能直接改 content_eq：端点规范化实测会塌掉代码块内容比较（随机 gate case 460），
/// 且按块型 StripTrailing + 窗口守卫会让 SecBody/Add 四格 apply != 全量（§13-4 记录）。
/// 快路径：原始字节相等直接返回（热路径零拷贝、零分配）。
fn norm_key_eq(ctx: &DiffCtx, o: &crate::infer::TreeNode, n: &crate::infer::TreeNode) -> bool {
    let (on, nn) = (ctx.o_snap.line_len(), ctx.n_snap.line_len());
    if (o.start as usize) > on || (o.end as usize) > on || (n.start as usize) > nn || (n.end as usize) > nn {
        return false;
    }
    let s_o = ctx.o_snap.rope.byte_of_line(o.start as usize);
    let e_o = if (o.end as usize) < on { ctx.o_snap.rope.byte_of_line(o.end as usize) } else { ctx.o_snap.rope.byte_len() };
    let s_n = ctx.n_snap.rope.byte_of_line(n.start as usize);
    let e_n = if (n.end as usize) < nn { ctx.n_snap.rope.byte_of_line(n.end as usize) } else { ctx.n_snap.rope.byte_len() };
    // [§13-4] 两侧都被窗口截断（end 顶到段尾、段尾未到文末）时比**公共重叠**：两条截断
    // 投影长度可以不同（各自的 zone 尾），逐字节比会把"同一块的两种截断"判成不等。
    // 取两侧较短的行数为公共长度，各比自己的前 common 行（同一规范化）。
    let on = ctx.o_snap.line_len();
    let nline = ctx.n_snap.line_len();
    let trunc_o = (o.end as usize) >= ctx.o_ze && ctx.o_ze < on;
    let trunc_n = (n.end as usize) >= ctx.n_ze && ctx.n_ze < nline;
    let common = if trunc_o && trunc_n {
        Some(((o.end - o.start) as usize).min((n.end - n.start) as usize))
    } else { None };
    let s_o = if let Some(c) = common { ctx.o_snap.rope.byte_of_line(o.start as usize) } else { s_o };
    let e_o = if let Some(c) = common { ctx.o_snap.rope.byte_of_line((o.start as usize + c).min(on)) } else { e_o };
    let s_n = if let Some(c) = common { ctx.n_snap.rope.byte_of_line(n.start as usize) } else { s_n };
    let e_n = if let Some(c) = common { ctx.n_snap.rope.byte_of_line((n.start as usize + c).min(nline)) } else { e_n };
    let a = ctx.o_snap.rope.byte_slice(s_o..e_o);
    let b = ctx.n_snap.rope.byte_slice(s_n..e_n);
    if a == b { return true; }
    let norm = |it: &mut dyn Iterator<Item = u8>| -> Vec<u8> {
        let mut v: Vec<u8> = it.filter(|&c| c != b'\r').collect();
        if v.last() == Some(&b'\n') { v.pop(); }
        v
    };
    norm(&mut a.bytes()) == norm(&mut b.bytes())
}

/// 树 diff 的层递归——消费 AttrTree（树引用 + 兄弟索引数组——扁平索引——
/// children 从 tree.nodes[idx].children 取——零拷贝切片）。
/// o_tree/n_tree 是旧/新树，o/n 是当前层的兄弟索引数组。
/// WC-13 通道槽位（渲染 vs diff 的**唯一**权威）：该 child 在**父层 ct 数组**里占
/// 几个元素；0 = 不进通道（渲染侧一个字节都不写）。
///
/// 分支顺序**必须**与 render_node 一致（render_node:5197 起）：
///   1) `kind.is_some()`（容器壳 / ItemLeaf / TableRow 行壳）⇒ 恒写一个元素 ⇒ 1；
///   2) 块 leaf：章节壳（`topology.shell_by_block` 命中）在 render_node 里先于
///      块 leaf 分支 ⇒ 1（本函数对"非 skip 的 leaf"统一回 1，故等价）；
///   3) `meta_is_skip` ⇒ **0**（render_node 的块 leaf 早退——Blank / CodeContent /
///      HtmlContent / TableSeparator / CodeFenceClose）；
///   4) 父是 item 且 tight 段落 ⇒ `para_inline_roots_for`（inline 根平铺进父数组——
///      多槽；0 根 = 0 槽）；
///   5) 其余 ⇒ 1。
///
/// 根因（WC-13）：规则 3 从前只在渲染侧生效，diff 侧一律记 1 槽 ⇒ 路径坐标比
/// 渲染多算 skip 孩子的位（`">\n"` → `"> text\n"` 曾发出越界 REM）。
fn channel_slots(snap: &crate::projection::Snapshot,
                 tree: &crate::infer::AttrTreeNew, idx: usize,
                 parent_item: bool) -> usize {
    let c = &tree.nodes[idx];
    if let Some(fk) = c.kind {
        // render_node:5197 的 kind.is_some() 两路：
        //  (a) TableRow 透明记录节点（blk_idx 无，:5205-5217）自身不写壳，children
        //      上提到**父数组** ⇒ 槽位 = Σ 子槽位（本卡前提：此类节点恒无孩子，
        //      漂移判官硬校验；有孩子 ⇒ 路径层级 ≠ 树层级，另开卡）；
        //  (b) 其余容器壳/行壳（:5221+）恒写一个元素 ⇒ 1。
        if fk == FrameKind::TableRow && c.blk_idx().is_none() {
            return c.children_of(tree).iter()
                .map(|&x| channel_slots(snap, tree, x as usize, parent_item)).sum();
        }
        return 1;
    }
    // render_node:5259 的 item 段在 skip 判定**之前** ⇒ 1。
    if c.is_item() { return 1; }
    // render_node:5335 的块 leaf skip 早退 ⇒ **0 槽**（Blank / CodeContent /
    // HtmlContent / TableSeparator / CodeFenceClose）。
    // （render_node:5287-5333 的章节壳分支在 skip 之前，但章节壳既不在 skip 集合、
    //   也不是 Paragraph ⇒ 落到本函数末尾回 1，与渲染一致。）
    if crate::effect::meta_is_skip(c.block_type) { return 0; }
    // render_node:5344：父是 item 且 tight 段落 ⇒ 行内根平铺进父数组（多槽；0 根 = 0 槽）。
    if parent_item && c.block_type == BlockType::Paragraph && !c.loose() {
        return para_inline_roots_for(snap, c);
    }
    1
}

/// WC-13：通道序 → 内部孩子下标（并给出该孩子在通道里的起始槽）。
/// 0 槽孩子不占通道位；`chan` 落在某多槽孩子**内部**时返回 `(child, start)` 且 `start < chan`
/// ——由调用方按"槽位层"处理（tight 段落的行内根平铺在父数组里）。越界 ⇒ None。
fn channel_to_internal(snap: &crate::projection::Snapshot,
                       tree: &crate::infer::AttrTreeNew,
                       parent: usize, chan: usize, parent_item: bool)
    -> Option<(usize, usize)> {
    let mut acc = 0usize;
    for &c in tree.nodes[parent].children_of(tree) {
        let k = channel_slots(snap, tree, c as usize, parent_item);
        if k == 0 { continue; }
        if chan < acc + k { return Some((c as usize, acc)); }
        acc += k;
    }
    None
}

/// [§13-5] 有界位移前瞻的窗口（块位移 ≤ 此值时一步对齐，不退化成 REPL 链）。
const SHIFT_K: usize = 8;

fn diff_level(ctx: &DiffCtx, o_tree: &crate::infer::AttrTreeNew, o: &[usize],
              n_tree: &crate::infer::AttrTreeNew, n: &[usize],
              o_stack: &mut MD_PATH, n_stack: &mut MD_PATH,
              offsets: (usize, usize),
              // #301：本层 children 的父是 tight item——段落块渲染 inline
              // （ch 项数 ≠ 1——path 偏移逐 child 累积）
              parent_tight_item: bool,
              rem: &mut Vec<(String, String, String)>,
              add: &mut Vec<(String, String, String)>,
              repl: &mut Vec<(String, String, String)>,
              // proj 粒度:面级补丁直通桶(ReplaceAttr——无需 path/value 三元)
              rattrs: &mut Vec<Patch>,
              // 粒度参数(入口读 env 一次——测试可显式传,免全局并行干扰)
              g: crate::topology::Granularity) {
    let (off_o, off_n) = offsets;
    // #301（F/P 一致）：tight item 内段落块的 ch 项数 = inline 根数——path
    // 偏移用前缀和（pre[oi] = Σ_{k<oi} ch_items）——普通层每块 1 项（退化
    // 为 oi + off 的现状语义）。
    // 配对契约提升为**闭包捕获构造**：o/n 各一个闭包，快照-树配对在构造
    // 点固定——调用点不可能传错（build_node 的 start/end 只防崩溃，
    // 不修正错配语义）。
    // WC-13：槽位 = 通道槽位（channel_slots——与渲染同一函数）。0 槽孩子
    // （skip 型 / 表行记录叶）不占位、不发 op、不递归。
    let ch_items_o = |idx: usize| -> usize {
        channel_slots(ctx.o_snap, o_tree, idx, parent_tight_item)
    };
    let ch_items_n = |idx: usize| -> usize {
        channel_slots(ctx.n_snap, n_tree, idx, parent_tight_item)
    };
    // 探查插桩(零行为——feature OFF 展开为 ()):diff_level 四相位切片
    // (pair/rem/add/match)。递归调用各自的 dl_* 嵌套于外层 dl_match,
    // 故按 name 求和 = 递归总量,按 parent=patch_diff_level 过滤 = 顶层分布。
    #[cfg(feature = "telemetry")]
    let _sp_dl_pair = crate::telemetry_span!("dl_pair",
        "n_o" => o.len(), "n_n" => n.len());
    let pre_o: Vec<usize> = {
        let mut acc = 0usize;
        o.iter().map(|&i| { let v = acc; acc += ch_items_o(i); v }).collect()
    };
    let pre_n: Vec<usize> = {
        let mut acc = 0usize;
        n.iter().map(|&i| { let v = acc; acc += ch_items_n(i); v }).collect()
    };
    let mut matches = Vec::new(); let mut old_only = Vec::new(); let mut new_only = Vec::new();
    {
        // 两个配对谓词在函数作用域定义一次（原在 while 体内逐轮重建——无必要）。
        let pm = |oi: usize, ni: usize| -> bool {
            let (no, nn) = (&o_tree.nodes[oi], &n_tree.nodes[ni]);
            if no.is_item() && nn.is_item() {
                // [§13-4] item 身份 = 自身内容（规范化键）+ **loose 同源位**（树位；loose 不在字节里）。
                norm_key_eq(ctx, no, nn) && no.loose() == nn.loose()
            } else if no.kind.is_some() && nn.kind.is_some() {
                // [§13-7] 容器配对按 **kind**（壳属性同源由匹配相位的闸判定）：容器的内容合法地
                // 会变（move/para_into_quote：引用吞进一个段落），把内容当硬闸会让它 REM+ADD 整壳。
                // 重复候选的消歧交给下面的"内容优先"有界前瞻。
                no.kind == nn.kind
            } else if no.kind.is_none() && nn.kind.is_none() {
                // [§13-4] 身份 = 内容键（block_type + 内容）——implicit 与非 implicit 同律，
                // **不比行号**（位置型身份是 REPL 链的根因）。
                // 被窗口截断的边界节点：内容不等即不配（交给核内分类发 REM+ADD），不猜同。
                // [§13-4] 内容比较用**规范化键**（CRLF/尾换行差归零）——仅身份判定，值渲染仍原始字节。
                // 块型只比 block_type（§13-9；rel 从不进身份，见 §13-1 订正）。
                no.block_type == nn.block_type && norm_key_eq(ctx, no, nn)
            } else { false }
        };
        // [§13-5] **内容优先**（同类重复候选的定序规则）：pm 只按 kind/块型+容器的壳同律配对，
        // 当同位同类但**内容不等**、且有界窗口内存在**内容相等**的同侧候选时，把中间者记为纯
        // 新增/删除（全等 run 归头锚）。依据：§9.2 步骤 3 的 same() = 同类且内容相等；实测
        // ShellBq/Add（old=[BQ(p100)] → new=[BQ(p902), BQ(p100)]）只按 kind 会配错成
        // [REPL /ct/0/ct/0, ADD /ct/1]（2 op），内容优先给出规范的单条 ADD /ct/0。
        let pm_eq = |oi: usize, ni: usize| -> bool {
            let (no, nn) = (&o_tree.nodes[oi], &n_tree.nodes[ni]);
            if no.is_item() && nn.is_item() {
                norm_key_eq(ctx, no, nn) && no.loose() == nn.loose()
            } else if no.kind.is_some() && nn.kind.is_some() {
                no.kind == nn.kind && norm_key_eq(ctx, no, nn)
            } else if no.kind.is_none() && nn.kind.is_none() {
                no.block_type == nn.block_type && norm_key_eq(ctx, no, nn)
            } else { false }
        };
        let sl = |oi: usize, ni: usize| -> bool {
            let (no, nn) = (&o_tree.nodes[oi], &n_tree.nodes[ni]);
            if no.is_item() && nn.is_item() { return true; }
            if no.kind.is_some() && nn.kind.is_some() {
                if no.kind == nn.kind { return true; }
                return container_path_model(no.kind.unwrap()) == ContainerModel::Listing
                    && container_path_model(nn.kind.unwrap()) == ContainerModel::Listing;
            }
            if no.kind.is_none() && nn.kind.is_none() {
                // [§13-7] 同型兜底**不比行号**（signature 不含相对坐标；
                // zone_start 为 trait 默认 0 ⇒ rel 恒等于绝对行号，δ 平移下
                // 「位置对应」的块必不等 ⇒ 落成 REM+ADD 而非同址 REPL——形态代价，
                // 非不变量风险）。身份=内容键由 pm 承担；sl 只判「同型可替」。
                return no.block_type == nn.block_type;
            }
            false
        };
        // [§13-5 有界位移前瞻] 原"仅 1 步前瞻"桥不过 ≥2 格的块位移（整行替换/节首插入 ⇒
        // 位移链 REPL：SecBody/Add 族 apply != 全量）。
        // 本轮把前瞻从 1 步扩到 K 步（K = SHIFT_K）：o[i] 与后面第 k 个新块同 ⇒ 前 k 个是纯新增
        // （旧侧对称同理）。这是方案 §3-A"有界键对齐"的最小形态：不重排循环、不动匹配相位、
        // 不改谓词，只把"找不到同块"的判定放宽到有界窗口内。
        // 简单版双锚夹按**位置**硬配尾锚（不查谓词），把 Blank 与 Paragraph 配成一对
        // ⇒ 该层零 op（wc13_empty_bq_child）：身份谓词是锚的唯一合法依据。
        // [§13-5] 双锚夹（A 层完整形态）：前锚自头部、后锚自尾部**逐位按 pm** 配对，至两游标相遇；
        // 锚区间不相交（anchor_head + anchor_tail <= min(两侧长度)——相等 run 全归头锚）；
        // 容器锚的 children 由匹配相位递归（下方 matches 消费点），核 = 中间剩余。
        // 核内仍走下面既有的有界位移前瞻贪心：无锚可用时逐步行为与从前一致（整循环
        // 重排之所以回退，是因为它同时动了核内配对与谓词——本轮只加锚，不动核内判定）。
        // [实验回退] pm 严格双锚夹：修 6 格 nest/inner_item_del，但在 random#3/Hs
        // 暴露既有 REM 侧路径缺陷（Remove /ct/5 不存在 ⇒ apply 失败）。先回退到绿基线，
        // 待把"REM 恒指旧侧"全链修净、并给锚配定向单测后再落地。
        // [§13-5] **双锚夹**（A 层完整形态）：前锚自头部、后锚自尾部**逐位按 pm** 配对，至两
        // 游标相遇；锚区间不相交（anchor_head + anchor_tail ≤ min(两侧长度)）；相等 run 归头锚。
        // 核（中间剩余）走下面的有界前瞻贪心 + 内容优先消歧。锚必须过 pm（身份谓词）——按位置
        // 硬配会把 0 槽节点与 1 槽节点配成对，而 matches 的第一道守卫（ch_items 任一为 0 即
        // continue）会**静默吞掉**整对（wc13 零 op 的机理）。
        // 锚判据 = §9.2 步骤 3 的 **same() = 同类且内容相等**（pm_eq）——不是 kind 级 pm：
        // 用 pm 做锚会把"同类但内容不同"的重复候选（ShellBq/Add：old BQ(p100) vs new BQ(p902)）
        // 钉死成锚对 ⇒ 内容优先消歧失效、退回 REPL+ADD（实测账本 0 → 7）。
        let mut anchor_head = 0usize;
        while anchor_head < o.len() && anchor_head < n.len() && pm_eq(o[anchor_head], n[anchor_head]) {
            anchor_head += 1;
        }
        let mut anchor_tail = 0usize;
        while anchor_head + anchor_tail < o.len() && anchor_head + anchor_tail < n.len()
            && pm_eq(o[o.len() - 1 - anchor_tail], n[n.len() - 1 - anchor_tail]) {
            anchor_tail += 1;
        }
        for t in 0..anchor_head { matches.push((t, t)); }
        let (o_end, n_end) = (o.len() - anchor_tail, n.len() - anchor_tail);
        let (mut i, mut j) = (anchor_head, anchor_head);
        while i < o_end && j < n_end {
            if pm(o[i], n[j]) {
                // [§13-5] 内容优先：同位同类但内容不等时，先在有界窗口内找内容相等的候选。
                if !pm_eq(o[i], n[j]) {
                    if let Some(k) = (1..=SHIFT_K).find(|&k| j + k < n_end && pm_eq(o[i], n[j + k])) {
                        // 新侧前 k 个是纯新增（内容相等的那个才是同一块）。
                        for _ in 0..k { new_only.push(j); j += 1; }
                        continue;
                    }
                    if let Some(k) = (1..=SHIFT_K).find(|&k| i + k < o_end && pm_eq(o[i + k], n[j])) {
                        // 旧侧前 k 个是纯删除。
                        for _ in 0..k { old_only.push(i); i += 1; }
                        continue;
                    }
                }
                matches.push((i, j)); i += 1; j += 1;
            }
            else if i + 1 < o_end && pm(o[i + 1], n[j]) { old_only.push(i); i += 1; }
            else if j + 1 < n_end && pm(o[i], n[j + 1]) { new_only.push(j); j += 1; }
            else if let Some(k) = (2..=SHIFT_K).find(|&k| i + k < o_end && pm(o[i + k], n[j])) {
                // 旧侧前方 k 格处才是同一块 ⇒ 旧侧前 k 个是纯删除
                for _ in 0..k { old_only.push(i); i += 1; }
            }
            else if let Some(k) = (2..=SHIFT_K).find(|&k| j + k < n_end && pm(o[i], n[j + k])) {
                // 新侧前方 k 格处才是同一块 ⇒ 新侧前 k 个是纯新增
                for _ in 0..k { new_only.push(j); j += 1; }
            }
            else if sl(o[i], n[j]) { matches.push((i, j)); i += 1; j += 1; }
            else { old_only.push(i); new_only.push(j); i += 1; j += 1; }
        }
        while i < o_end { old_only.push(i); i += 1; }
        while j < n_end { new_only.push(j); j += 1; }
        for t in 0..anchor_tail {
            matches.push((o.len() - anchor_tail + t, n.len() - anchor_tail + t));
        }
    }
    #[cfg(feature = "telemetry")] drop(_sp_dl_pair);

    #[cfg(feature = "telemetry")]
    let _sp_dl_rem = crate::telemetry_span!("dl_rem", "n" => old_only.len());
    for &oi in &old_only {
        // #301：tight item 内段落块占多槽位（inline 根数）——REM 必须逐
        // 槽位（只删起始槽会残留其余 inline——patch gate 3000 case 147）。
        // WC-13：0 槽孩子（skip 型 / 表行记录叶）不占通道、不发 REM。
        let c = &o_tree.nodes[o[oi]];
        let slots = ch_items_o(o[oi]);
        if slots == 0 { continue; }
        if parent_tight_item && c.kind.is_none() && c.block_type == BlockType::Paragraph {
            // 升序 push——diff_patches_mode 统一 rem.reverse() 变降序
            //（apply 先删大 index 避免兄弟错位）。
            for k in 0..slots {
                rem.push(("REM".into(), o_stack.with_child(off_o + pre_o[oi] + k), "null".into()));
            }
        } else {
            rem.push(("REM".into(), o_stack.with_child(off_o + pre_o[oi]), "null".into()));
        }
    }
    #[cfg(feature = "telemetry")] drop(_sp_dl_rem);
    #[cfg(feature = "telemetry")]
    let _sp_dl_add = crate::telemetry_span!("dl_add", "n" => new_only.len());
    for &ni in &new_only {
        // #301：tight item 内新增段落——inline 渲染（与 render_node 一致）
        // 注意：new_only 存的是兄弟数组索引——节点索引 = n[ni]
        let c = &n_tree.nodes[n[ni]];
        // WC-13：0 槽孩子不占通道、不发 ADD（旧侧 skip 孩子不该有对应新元素）。
        if ch_items_n(n[ni]) == 0 { continue; }
        if parent_tight_item && c.kind.is_none() && c.block_type == BlockType::Paragraph {
            // 多槽位段落：逐 inline 根节点一个 Add（index 递增——AST ch
            // 前缀槽位语义——单值拼接是非法 JSON——patch gate 3000 修复）。
            let vals = para_inline_values(ctx, c);
            for (k, v) in vals.iter().enumerate() {
                add.push(("ADD".into(), n_stack.with_child(off_n + pre_n[ni] + k), v.clone()));
            }
        } else {
            let v = subtree_value(ctx, n_tree, n[ni]);
            // 空节点（空段落等）渲染无输出——0 槽位——ADD 空值非法
            //（patch gate 3000 case 238：BQ 内段落 "q" 删空后无 ADD）。
            if !v.is_empty() {
                add.push(("ADD".into(), n_stack.with_child(off_n + pre_n[ni]), v));
            }
        }
    }
    #[cfg(feature = "telemetry")] drop(_sp_dl_add);
    #[cfg(feature = "telemetry")]
    let _sp_dl_match = crate::telemetry_span!("dl_match", "n" => matches.len());
    for &(oi, ni) in &matches {
        // 兄弟数组索引 → 节点池索引（o[oi]/n[ni]——旧代码 old[oi] 的
        // 数组语义——直接用 nodes[oi] 取错节点——BQ>BLT 场景 matches=(0,0)
        // 却取 nodes[0]=BQ 而非 BLT——children 链循环——栈溢出）
        // WC-13：任一侧 0 槽（skip 型 / 表行记录叶）⇒ 不产 op、不递归（内部不可寻址）。
        if ch_items_o(o[oi]) == 0 || ch_items_n(n[ni]) == 0 { continue; }
        let (no, nn) = (&o_tree.nodes[o[oi]], &n_tree.nodes[n[ni]]);
        let path_str = n_stack.with_child(off_n + pre_n[ni]);
        if no.is_item() && nn.is_item() {
            // 完整实验记录（值面/子面分开判 + tight 守卫）：
            //  - 值面判据（item_inline_plan 的 tag/content/形态位）＋ tight 守卫可让四项曾红测试
            //    全绿（golden(Hs)、随机 gate、sublist_*、hs_diff_container_heading_edit）；
            //  - 且修掉 30 格 nest/* 形态（loose item 深插改走递归）；
            //  - **但** loose item 递归改变了 264 格的既有形态（期望集是按 item REPL 写的）⇒ 账本
            //    84 → 318，净负 ⇒ 回退。
            //  结论（下轮据此重做）：tight item 必须保持 item REPL（inline 槽语义）；loose item 的
            //  递归化是**真实的最小性改进**，但要与 264 格期望的同步重写一起做（单独一轮）。
            // [值面/子面] 值面 / 子面分开判（验证：正确性可达；tight item 必须走 item REPL——
            // tight 下子段落渲染为裸内联槽，per-slot 递归只能产出 p 包裹；loose item 递归是真实最小性
            // 改进，本轮与其期望迁移成对落地）。
            let o_bn = ctx.o_snap.build_node(no.start as usize, no.end as usize, no.block_type, true);
            let mut o_scratch = Vec::new();
            let o_plan = item_inline_plan(ctx.o_snap, &o_bn, ctx.o_spans, no.seg_depth as usize,
                list_frame_count(ctx.o_spans, no.start as usize, no.seg_depth as usize), &mut o_scratch);
            let n_bn = ctx.n_snap.build_node(nn.start as usize, nn.end as usize, nn.block_type, true);
            let mut n_scratch = Vec::new();
            let n_plan = item_inline_plan(ctx.n_snap, &n_bn, ctx.n_spans, nn.seg_depth as usize,
                list_frame_count(ctx.n_spans, nn.start as usize, nn.seg_depth as usize), &mut n_scratch);
            let value_changed = o_plan.tag != n_plan.tag
                || o_plan.content != n_plan.content
                || o_plan.is_content_block != n_plan.is_content_block
                || o_plan.is_empty_content != n_plan.is_empty_content
                || o_plan.content_has_inline != n_plan.content_has_inline
                || o_plan.tag_has_inline != n_plan.tag_has_inline;
            let loose_diff = ctx.o_spans.is_loose_at(no.start as usize)
                != ctx.n_spans.is_loose_at(nn.start as usize);
            let kids_flip = !no.children_of(o_tree).is_empty() && nn.children_of(n_tree).is_empty();
            let n_tight = !ctx.n_spans.is_loose_at(nn.start as usize);
            // [§13-8 精化] tight 守卫只对**段落子层**成立：tight item 的子段落渲染为裸内联槽
            // （per-slot 递归只能产出 p 包裹 ⇒ 必须整项 REPL）；而容器子（引用/列表/表等）
            // 在父数组里占独立槽位、可寻址 ⇒ 其内部编辑应递归（原实现 n_tight 一律 REPL ⇒
            // nest/item_quote_add、nest/inner_item_add、nest/3level_add 等族非最小：整项 REPL）。
            let has_p_child = |tree: &crate::infer::AttrTreeNew, nd: &crate::infer::TreeNode| -> bool {
                nd.children_of(tree).iter().any(|&c| {
                    let cn = &tree.nodes[c as usize];
                    cn.kind.is_none() && cn.block_type == BlockType::Paragraph
                })
            };
            let inline_risk = has_p_child(o_tree, no) || has_p_child(n_tree, nn);
            // 先判"内容是否变化"：内容全同 ⇒ 直接递归（子层无操作，原行为）；
            // 内容有变化时才按 值面/子面/tight 分流（否则会把**未动的项**也 REPL 一遍——
            // 实测 List/* 族因此多出 192 条偏差）。
            if !content_eq(ctx, no, nn) && (loose_diff || kids_flip || value_changed || (n_tight && inline_risk))
            {
                // proj 粒度:差异收敛单面(键 t / 值 p 叶)时字段级
                if g == crate::topology::Granularity::Proj {
                    if let Some(attr_p) = maybe_attr(ctx, o_tree, n_tree, no, nn, &path_str) {
                        rattrs.push(attr_p);
                        continue;
                    }
                }
                let vn = subtree_value(ctx, n_tree, n[ni]);
                repl.push(("REPL".into(), path_str, vn));
            } else if !no.children_of(o_tree).is_empty() && nn.children_of(n_tree).is_empty() {
                // children 从有到无：渲染形态在"带 ch"与"省略空 ch"间翻转——
                // 逐槽位 REM 会残留 `"ct":[]`（渲染省略空数组——patch gate
                // 3000 case 103：item 内容段 "===" 被删）。直接 REPL item
                // （subtree_value = 渲染形态，无 ch 字段）。
                let vn = subtree_value(ctx, n_tree, n[ni]);
                repl.push(("REPL".into(), path_str, vn));
            } else {
                // item 的内容流 = [值 p?] + 树子(外部形态定案:值包 p;
                // tag 不再拆段;富文本值在 p 内不占槽)——递归的块索引偏移
                // = value_p_prefix(有文本值 ? 1 : 0)。内容形态经
                // item_inline_plan 单源(与渲染同路径)。
                let value_of = |snap: &Snapshot,
                                 node: &crate::infer::TreeNode,
                                 spans: &dyn ResolveView| -> usize {
                    let bn = snap.build_node(node.start as usize, node.end as usize,
                        node.block_type, true);
                    let mut scratch = Vec::new();
                    let plan = item_inline_plan(snap, &bn, spans, node.seg_depth as usize,
                        list_frame_count(spans, node.start as usize, node.seg_depth as usize),
                        &mut scratch);
                    value_p_prefix(&plan)
                };
                let o_off = value_of(ctx.o_snap, no, ctx.o_spans);
                let n_off = value_of(ctx.n_snap, nn, ctx.n_spans);
                o_stack.push(off_o + pre_o[oi]);
                n_stack.push(off_n + pre_n[ni]);
                // #301：tight item 的 children 层——段落块渲染 inline——
                // 子层偏移逐 child 累积（parent_tight_item）
                let child_tight = !ctx.n_spans.is_loose_at(nn.start as usize);
                let o_child_u: Vec<usize> =
                    no.children_of(o_tree).iter().map(|&c| c as usize).collect();
                let n_child_u: Vec<usize> =
                    nn.children_of(n_tree).iter().map(|&c| c as usize).collect();
                diff_level(ctx, o_tree, &o_child_u, n_tree, &n_child_u,
                    o_stack, n_stack, (o_off, n_off), child_tight, rem, add, repl, rattrs, g);
                o_stack.pop();
                n_stack.pop();
            }
        } else if no.kind.is_some() && nn.kind.is_some() {
            if container_path_model(no.kind.unwrap()) != ContainerModel::Leaf {
                let same_kind = no.kind == nn.kind;
                let loose_flip = container_path_model(no.kind.unwrap()) == ContainerModel::Listing
                    && ctx.o_spans.is_loose_at(no.start as usize)
                    != ctx.n_spans.is_loose_at(nn.start as usize);
                // [§13-7] 删 body_change（S 层）：它是**窗口派生的行级信号**——"容器内部有变化"
                // 不等于"子树不可递归"。把它当整壳 REPL 的理由会产出非最小形态（实测 triage：
                // nest/item_quote_add 的深层插入被 REPL 成整个外层 item；3level_* 同型）。
                // 容器内部变化交由递归 + 逐槽 op 处理；只有 same_kind 为假或 loose 翻转
                // （壳属性同源比较）才走整壳 REPL——"loose 翻转 ⇒ 整壳 REPL"这条刚性决策保留。
                if !same_kind || loose_flip {
                    repl.push(("REPL".into(), path_str, subtree_value(ctx, n_tree, n[ni])));
                } else {
                    // Non-shell containers (CF, Table, Comment, HtmlBlock, RefDef,
                    // HtmlElement) carry their content in shell attrs, not in
                    // children — compare content bytes (memcmp, zero-copy); only
                    // render vn once when they actually differ.
                    if !crate::effect::effect_for(no.kind.unwrap()).is_shell {
                        if !content_eq(ctx, no, nn) {
                            let vn = subtree_value(ctx, n_tree, n[ni]);
                            repl.push(("REPL".into(), path_str, vn));
                            continue;
                        }
                    }
                    o_stack.push(off_o + pre_o[oi]);
                    n_stack.push(off_n + pre_n[ni]);
                    let o_child_u: Vec<usize> =
                        no.children_of(o_tree).iter().map(|&c| c as usize).collect();
                    let n_child_u: Vec<usize> =
                        nn.children_of(n_tree).iter().map(|&c| c as usize).collect();
                    diff_level(ctx, o_tree, &o_child_u, n_tree, &n_child_u,
                        o_stack, n_stack, (0, 0), false, rem, add, repl, rattrs, g);
                    o_stack.pop();
                    n_stack.pop();
                }
            } else {
                // Leaf 容器（非 nesting，如 HtmlElement/RefDef）——内容在壳，
                // 内容同 + type 同 ⟹ 渲染同——memcmp 预比较，零渲染。
                if !content_eq(ctx, no, nn) {
                    // proj 粒度:字段级(容器壳 attrs 表驱动——CF(t/cd)、
                    // rd(t/u/ti)、hb(t/ct Raw)等 Leaf 容器单字段变)。
                    if g == crate::topology::Granularity::Proj {
                        if let Some(attr_p) =
                            maybe_attr(ctx, o_tree, n_tree, no, nn, &path_str)
                        {
                            rattrs.push(attr_p);
                            continue;
                        }
                    }
                    let vn = subtree_value(ctx, n_tree, n[ni]);
                    repl.push(("REPL".into(), path_str, vn));
                }
            }
        } else if no.kind.is_none() && nn.kind.is_none() {
            // [G4/红线 1] 章节壳分支已摘除——拓扑只许出现在 L3/L4，L2 一律
            // 模式无关。壳级语义（叶→壳升级/壳区间渲染/传播域）由适配层
            // （L3）承担；diff_level 只做块/容器级事实。
            // sl 匹配（type 同）允许内容不同——memcmp 捕获；内容同 + type 同
            // ⟹ 渲染同（投影是内容的确定性函数），零渲染。
            if !content_eq(ctx, no, nn) {
                if parent_tight_item && nn.block_type == BlockType::Paragraph {
                    // #301：tight item 内段落块——inline 渲染（与 render_node
                    // 一致——p 壳会偏离 F 渲染）。多槽位语义：旧槽位 REM（倒序）
                    // + 新槽位逐 ADD（全局排序 REM 先、ADD 后——apply 安全）。
                    let vals = para_inline_values(ctx, nn);
                    let old_slots = if parent_tight_item && no.block_type == BlockType::Paragraph {
                        para_inline_roots_for(ctx.o_snap, no)
                    } else {
                        1
                    };
                    // 升序 push——全局 rem.reverse() 变降序（apply 安全）。
                    // [§13-14 同区域缺陷] REM 必须指向**旧侧**路径：配对 (oi, ni) 在双锚夹下
                    // 可 i≠j（锚配尾部时两侧下标不等），用新侧栈 + 新侧槽前缀会把删除指到
                    // 旧文档里不存在的槽（random#3/Hs：Remove /ct/5 不存在 ⇒ apply 失败）。
                    for k in 0..old_slots {
                        rem.push(("REM".into(), o_stack.with_child(off_o + pre_o[oi] + k), "null".into()));
                    }
                    for (k, v) in vals.iter().enumerate() {
                        add.push(("ADD".into(), n_stack.with_child(off_n + pre_n[ni] + k), v.clone()));
                    }
                } else {
                    // proj 粒度:块叶(段落等)内容面/标题 tag 面单差 → 字段级
                    if g == crate::topology::Granularity::Proj {
                        if let Some(attr_p) = maybe_attr(ctx, o_tree, n_tree, no, nn, &path_str) {
                            rattrs.push(attr_p);
                            continue;
                        }
                    }
                    let vn = subtree_value(ctx, n_tree, n[ni]);
                    if vn.is_empty() {
                        // 节点渲染空（内容删空）——REPL 空值非法——改 REM 槽位。
                        // [§13-14 同区域缺陷] 与 :2253 同律：REM 必须指**旧侧**路径——
                        // 配对 (oi, ni) 在双锚夹下可 i≠j，借新侧 path_str 会删到旧文档
                        // 不存在的槽（random#3/Hs：Remove /ct/5 不存在 ⇒ apply 失败）。
                        rem.push(("REM".into(),
                            o_stack.with_child(off_o + pre_o[oi]), "null".into()));
                    } else {
                        repl.push(("REPL".into(), path_str, vn));
                    }
                }
            }
        }
    }
}


/// CF path 根序号：根起点块跳计数 [0, end_line) 内的 root 块数。
/// 与逐行 oracle `infer::root_blocks_count_cf` 逐值一致：
/// - depth==0 时逐块访问根块并按块尾跳跃；
/// - depth>0 时只访问下一个 entry 行，不在容器内容行上空转。
/// 等价性由 tests/unit/incr.rs 随机文档 × 全部 end_line 穷举对拍。
fn root_child_index(snap: &Snapshot, end_line: usize) -> usize {
    roots_in_range(snap, 0, end_line).len()
}

/// A3-1：`[zs, ce)` 行区间内的 CF 根起点（升序，**保多重性**——同一行可能
/// 同时「pop 归 0」与「new_root」，oracle 计 2，索引必须同样计 2）。
/// 与 `root_child_index` 共用本实现（口径唯一律：oracle 已是它的
/// `[0, end)` 计数薄包装）。区间起点从 `open_depth` 列直读行首深度、
/// `partition` 定位 entry 游标，故可从任意 depth-0 锚点起跳，成本 O(zone)。
/// 判官：`root_count_before_equals_root_child_index`（Base + Edit 双路径）。
pub(crate) fn roots_in_range(snap: &Snapshot, zs: usize, ce: usize) -> Vec<(u32, u32, crate::block::BlockType)> {
    roots_in_range_mode(snap, zs, ce, false)
}

/// A3-2：`infer::root_blocks_before` 口径（**split**——其 d_before 分支不做
/// setext 合并，把「段落 + HsSep」拆成两条根）。两套口径是代码库既有的不一致
/// （`root_child_index` 合并、`root_blocks_before` 拆分），索引各自匹配自己的
/// oracle，**勿混用**。
pub(crate) fn roots_split_in_range(snap: &Snapshot, zs: usize, ce: usize) -> Vec<(u32, u32, crate::block::BlockType)> {
    roots_in_range_mode(snap, zs, ce, true)
}

#[cfg(test)]
thread_local! {
    /// [判官用] `roots_in_range_mode` 主循环**实际走过的行数**。
    /// 局部性断言：查询"某点之前的根数"不得走完整篇前缀
    /// （`root_count_query_does_not_rebuild_table` 用它盯 oracle 分支）。
    pub(crate) static ROOTS_RANGE_LINES_WALKED: std::cell::Cell<u64> =
        const { std::cell::Cell::new(0) };
}

fn roots_in_range_mode(
    snap: &Snapshot,
    zs: usize,
    ce: usize,
    split: bool,
) -> Vec<(u32, u32, crate::block::BlockType)> {
    let entries = &snap.frame_log.entries;
    let lines = &snap.lines;
    let rope = &snap.rope;
    let n = snap.line_len();
    let zs = zs.min(n);
    let ce = ce.min(n);

    // 块间逐行推进 → kernel block_starts 二分跳
    // (Base 命中时;Edit/无槽回退 i+1——语义不变)。块间行(blank/skip/
    // cont)在 depth 0 不产生计数也不携带 entries——跳过等价。
    let starts = crate::cache::kernel_block_starts(snap);
    let jump = |i: usize| -> usize {
        match &starts {
            Some(s) => {
                let next = s.partition_point(|&b| (b as usize) <= i);
                s.get(next).map(|&b| b as usize).unwrap_or(ce).min(ce)
            }
            None => (i + 1).min(ce),
        }
    };

    let mut out: Vec<(u32, u32, crate::block::BlockType)> = Vec::new();
    let mut depth = if zs >= n {
        snap.eof_open_depth() as i32
    } else {
        snap.open_depth_at(zs) as i32
    };
    let mut ei = crate::infer::partition(entries, zs as u32);
    let mut skip_until = zs;
    let mut i = zs;

    while i < ce {
        #[cfg(test)]
        ROOTS_RANGE_LINES_WALKED.with(|c| c.set(c.get() + 1));
        if i < skip_until {
            i += 1;
            continue;
        }

        // 行 i 之前的所有 entries 已在上一跳中折叠，depth 为 d_before。
        let d_before = depth;
        let mut new_root = false;
        let mut new_root_kind: Option<crate::block::FrameKind> = None;
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
                // pop 归 0 行也可能构成 setext 标题首行（容器关闭后的段落 +
                // 下划线——与 1588 段落分支同律）。不合并会重复计数
                // （行 4 段落 + 行 5 HsSep 各计 1——渲染只 1 个 h1——
                // patch gate 3000 case 1253 off 高估 → 索引越界）。
                if !split && t == crate::block::BlockType::Paragraph {
                    let j = crate::infer::paragraph_run_end(lines, i + 1, n);
                    if j < n && lines[j].node_type() == crate::block::BlockType::HsSep {
                        out.push((i as u32, ((j + 1).min(n)) as u32, t));
                        skip_until = j + 1;
                        i = skip_until;
                        continue;
                    }
                }
                let e = crate::infer::root_end_for(entries, lines, &snap.frame_log, i, t, n, ei);
                out.push((i as u32, e.min(n).max(i + 1) as u32, t));
            }
            if new_root {
                if let Some(fk) = new_root_kind {
                    let e = crate::infer::container_end_fallback(entries, lines, i as u32, fk, ei, n);
                    out.push((i as u32, e.min(n).max(i + 1) as u32, t));
                }
            }
            if depth == 0 {
                i = jump(i);
            } else {
                // 容器内部：下一个结构事件行才有根计数意义。
                let next = if ei < entries.len() {
                    crate::block::entry_line(entries[ei]) as usize
                } else {
                    n
                };
                i = next.min(ce).max(i + 1);
            }
            continue;
        }

        if t == crate::block::BlockType::Paragraph && !(i > 0 && lines[i].cont()) {
            if !crate::infer::effective_para_at(lines, rope, i, n, None, None) {
                i = jump(i);
                continue;
            }
            let j = crate::infer::paragraph_run_end(lines, i + 1, n);
            if j < n && lines[j].node_type() == crate::block::BlockType::HsSep {
                out.push((i as u32, ((j + 1).min(n)) as u32, t));
                skip_until = j + 1;
                i = skip_until;
            } else {
                out.push((i as u32, j as u32, t));
                i = j;
            }
            continue;
        }

        if crate::effect::meta_is_skip(t) {
            i = jump(i);
            continue;
        }
        if i > 0 && lines[i].cont() {
            i = jump(i);
            continue;
        }

        let be = if let Some(fk) = crate::block::frame_kind(t) {
            // 与 infer::root_end_for 同源：真实关闭取关闭行；EOF 仍开取 n；
            // 帧不入日志的形态（TableRow）取下一个事件行。
            crate::infer::container_end_fallback(entries, lines, i as u32, fk, ei, n)
        } else if crate::block::is_header(t) {
            i + 1
        } else {
            crate::infer::get_node_end_from_lines(&snap.lines, i, t, &snap.frame_log)
                .min(n).max(i + 1)
        };

        out.push((i as u32, be.min(n).max(i + 1) as u32, t));

        // 跳块前，把 [i+1, be) 区间内的 entries 折叠进 depth；
        // 根容器内部的深度不会归 0，因此这些行不产生根计数，
        // 但必须推进游标，使 i=be 时 d_before 正确。
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



/// 章节版根计数(HS——§九——章节壳合并区间内块——根 = 章节壳(标题计 1)+
/// 标题前的块——标题后的块(章节区间内)不独立计数。
/// pub(crate)（SDL 拓扑投影审计——双实现比对）。
/// 修:原 in_section"标题后永不复位"无容器感知——`> # h\nlazy` 的 lazy
/// (容器外根块——标题不 open 段落——lazy 不延续——BQ 外独立段)被误吞——
/// count(1) ≠ HS 树根数(2)——patch path 基准错。修:章节区间改树级语义
/// (与 section_reorganize 同款——标题块 + bound = min(下一 level≤, 容器
/// push_lines/end)——容器感知)——in_section = 行 ∈ 某章节区间。

/// [P1] `root_child_index_hs` 不存在：
/// 无生产消费者，且其唯一的全量 `structure_for_range(0,n)` 属于伪 full 消费。
/// 保留下来的 `root_child_index_hs_from` 只作测试区对拍预言机。

/// 注意(#32 契约回退,实现账本):内部自算节区间保留——前缀根计数语义
/// 依赖"截断构建"的块级重分类(structure_for_range(0,n) 的截断段内
/// setext/容器内标题按前缀视图重分类),全量节区间格(只接受全量段物化)
/// 供给不了截断口径;root_child_index_hs_equals_full_oracle(8000 文档 ×
/// 全 end 穷举)是此语义的契约判官。走查修正⑥的"同快照同范围级联冗余"
/// 前提不成立(secs_of 全量 vs 根计数截断,非同范围同函数)。
#[cfg(test)]
fn root_child_index_hs_from(seg: &crate::cache::Segment, end_line: usize, heading: &[crate::block::BlockType]) -> usize {
    // 827 阶段二:节区间与根块枚举都读缓存段——节区间与 count_root_blocks_hs
    // (全量参照)同源;根块 = 父为空的容器记录 + 无容器引用的块记录
    // (六形状等价实测),替代手工行走(+1/+2 常数截断多行 setext——D1)。
    // 对拍:root_child_index_hs_equals_full_oracle(随机文档 × 全 end_line)。
    let n = end_line.min(seg.end_line as usize);
    // 区间与根块都钳制到 n(旧扫描账同语义:前缀计数只数 n 前的结构)。
    let secs: Vec<(usize, usize, u8, crate::block::BlockType)> =
        crate::infer::section_intervals(&seg.blocks, &seg.records, &seg.arena, heading, n)
            .into_iter()
            .filter(|&(s, e, _, _)| s < n && e > s)
            .collect();
    // 顶层节壳根 = 标题块无容器引用(文档级)+ 不被任何先开节区间包含
    // (等价旧扫描账的 d==0 双重条件;容器内节的标题块挂在容器上)。
    let head_ref = |s: u32, t: crate::block::BlockType| -> u32 {
        let lo = seg.blocks.partition_point(|b| b.start < s);
        seg.blocks[lo..].iter()
            .take_while(|b| b.start == s)
            .find(|b| b.block_type == t)
            .map(|b| b.container_ref)
            .unwrap_or(u32::MAX)
    };
    let mut count = secs.iter()
        .filter(|&&(s, _e, _lv, t)| {
            head_ref(s as u32, t) == u32::MAX
                && !secs.iter().any(|&(ss, se, _, _)| ss < s && s < se)
        })
        .count();
    // 根块枚举(记录父空 = 容器根;块无容器引用 = 块根),按起点合并。
    let mut roots: Vec<(usize, usize, bool, crate::block::BlockType)> = Vec::new();
    for r in seg.records.iter().filter(|r| r.parent == u32::MAX && (r.start as usize) < n) {
        let end = r.end_usize().unwrap_or(n).min(n);
        roots.push((r.start as usize, end, true,
            crate::effect::container_block_type(r.frame.kind())));
    }
    for b in seg.blocks.iter().filter(|b| b.container_ref == u32::MAX && (b.start as usize) < n) {
        roots.push((b.start as usize, b.end as usize,
            crate::block::frame_kind(b.block_type).is_some(), b.block_type));
    }
    roots.sort_by_key(|r| r.0);
    for (s, _e, _is_container, t) in roots {
        if heading.contains(&t) {
            continue;
        }
        let in_section = secs.iter().any(|&(ss, se, _, _)| ss < s && s < se);
        if !in_section {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
pub(crate) fn count_root_blocks_hs(snap: &Snapshot, end_line: usize,
                                   heading: &[crate::block::BlockType]) -> usize {
    let entries = &snap.frame_log.entries;
    let n = end_line.min(snap.line_len());
    // 1. 章节区间(树级语义——与 section_reorganize 同款 bound)
    // 优化:构建范围截断到 end_line(原全文档 0..n——每 diff 2 次 O(n)——调用方
    // 传 zs(编辑点前)——O(zs))
    let (recs, blocks, _va, _arena) = crate::infer::build_structure_parts(
        entries, &snap.lines, &snap.open_depth, &snap.rope, (0, n as u32));
    let secs: Vec<(usize, usize)> = crate::infer::section_intervals(&blocks, &recs, &_arena, heading, n)
        .into_iter()
        .map(|(s, e, _, _)| (s, e))
        .collect();
    // 2. 扫描计数:in_section ⟺ 行 ∈ 某活动章节区间(区间栈——
    // start 有序——指针推进 + 栈——O(n + m))。
    // 章节壳根(计 1):标题行开区间 + 帧深度 0 + 入栈前无活动
    // (容器内标题的章节壳挂容器——嵌套章节挂父章节——都不根)。
    // 容器壳根:new_root_container(容器替换)且不在 section 内——
    // 容器含标题不再豁免(HS 只决定 section,容器内部保持嵌套)。
    let mut count = 0usize;
    let mut depth = 0i32;
    let mut ei = 0usize;
    let mut sec_idx = 0usize;
    let mut active: Vec<usize> = Vec::new();
    let mut skip_until = 0usize;  // IC 多行块(前瞻合并——计 1)
    for i in 0..n {
        if i < skip_until {
            // 跳过 IC 内容行(entry 指针推进——depth 真实更新——防错位)
            while ei < entries.len() && entry_line(entries[ei]) as usize == i {
                let e = entries[ei];
                if !entry_is_body_mut(e) {
                    if entry_is_pop(e) { depth -= 1; } else { depth += 1; }
                }
                ei += 1;
            }
            continue;
        }
        // 弹出已结束区间(栈顶 end ≤ i——可多层)
        while let Some(&top) = active.last() {
            if secs[top].1 <= i { active.pop(); } else { break; }
        }
        let was_active = !active.is_empty();
        let d_before = depth;
        let mut new_root_container = false;
        while ei < entries.len() && entry_line(entries[ei]) as usize == i {
            let e = entries[ei];
            if !entry_is_body_mut(e) {
                if entry_is_pop(e) {
                    depth -= 1;
                } else {
                    let before = depth;
                    depth += 1;
                    // 根级新容器(容器替换——空行 Pop+Push——同 count_root_blocks
                    // 修——PopPush 延续排除(仅 shell 容器——非 shell 独立))
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
        // 入栈:start == i 的区间(嵌套——多个)
        let mut is_sec_start = false;
        while sec_idx < secs.len() && secs[sec_idx].0 == i {
            active.push(sec_idx);
            sec_idx += 1;
            is_sec_start = true;
        }
        let in_section = !active.is_empty();
        // 章节壳根:顶层(深度 0 + 无父区间)——容器内标题(深度>0)/嵌套不根
        let sec_root = is_sec_start && !was_active && depth == 0;
        let is_block_line = |t: BlockType| {
            !crate::effect::meta_is_skip(t) && !(i > 0 && snap.lines[i].cont())
        };
        if d_before != 0 {
            // 容器关闭行(Pop 行):处理后深度归 0 且该行是新块开头
            if depth == 0 {
                let t = snap.line_type_at(i);
                if is_block_line(t) && (!in_section || sec_root) {
                    count += 1;
                }
            }
            if new_root_container && !was_active {
                // 容器替换(空行)——仅在“外层 section”外的新根容器计数；
                // 同一行内部 heading section 不算外层。
                count += 1;
            }
            continue;
        }
        let t = snap.line_type_at(i);
        if !is_block_line(t) { continue; }
        if t == BlockType::Paragraph && !(i > 0 && snap.lines[i].cont()) {
            // P14/B0-3 类型面对齐（同 count_root_blocks）：#542 跨行 rd 撤销——
            // resolved Blank——树不产块——计数须与树对齐。
            if !crate::infer::effective_para_at(&snap.lines, &snap.rope, i, n, None, None) {
                continue;
            }
        }
        if t == BlockType::IndentedCode && !(i > 0 && snap.lines[i].cont()) {
            // IC 多行块(连续 IndentedCode 行——树 1 块——行级逐行 count
            // 会多计——前瞻合并)
            let mut j = i + 1;
            while j < n && snap.line_type_at(j) == BlockType::IndentedCode { j += 1; }
            if !in_section || sec_root || (new_root_container && !was_active) {
                count += 1;
            }
            skip_until = j;
            continue;
        }
        if !in_section || sec_root || (new_root_container && !was_active) {
            count += 1;
        }
    }
    count
}

/// zone 决策的单一入口——平衡闭包的实现（定理 5.4）。
/// 输出对称的 old/new zone（不变量：new zone 覆盖 old zone 的行号映射）。
#[derive(Debug, Clone, Copy)]
pub struct ZonePair {
    pub cs_o: usize, pub ce_o: usize, pub zs_o: usize,
    pub cs_n: usize, pub ce_n: usize, pub zs_n: usize,
}


/// pub(crate)（SDL zone 效率分析——S ⊇ A 结构断言 + 比值）。
pub fn compute_zone(task: &IncrTask, line_map: &LineMap) -> ZonePair {
    let n_old = task.snap_old.line_len();
    let n_new = task.snap.line_len();
    let delta = task.delta;

    let rs = task.ripple_line_start.min(n_new.saturating_sub(1));
    let re = task.ripple_line_end.max(rs).min(n_new);
    // 阶段 1：编辑范围 → 原始 cs/ce。
    // 一次遍历 edit_ranges 得 sf/sl(字节 → 旧侧行区间)——
    // 三路标志的解析角色已随 ce 侧闭包删除,stage1 只消费 sf/sl;
    // cs/ce 推导用共享 zone_derive_stage1。
    let mut sf_o = n_old;
    let mut sl_o = 0usize;
    for &(sb, eb) in &*task.edit_ranges {
        let (sb, eb) = (sb as usize, eb as usize);
        // 钳制防御:多编辑事务后段字节在
        // 中间坐标系、sb/eb 可越过旧快照 byte_len——line_of_byte 越界
        // panic。sb 先钳一次,s 与 e 的 .max 均用钳后值(否则越界 sb 会经
        // .max(sb) 原样传给 line_of_byte,在 e 处照样 panic)。
        let sb_c = sb.min(task.snap_old.byte_len().saturating_sub(1));
        let s = task.snap_old.line_of_byte(sb_c);
        let e = if eb > sb {
            task.snap_old
                .line_of_byte((eb - 1).min(task.snap_old.byte_len().saturating_sub(1)).max(sb_c))
        } else {
            s
        };
        sf_o = sf_o.min(s);
        sl_o = sl_o.max(e);
        // 三路标志(touches_nonshell/item/structural)的
        // 解析角色已随 ce 侧闭包删除——stage1 只消费 sf/sl(字节 → 行)。
    }
    if sf_o > sl_o { (sf_o, sl_o) = (0, 0); }
    // 空文档特判保底(原内联初值语义)。
    let _structural_empty = n_old == 0 || n_new == 0;
    let sf_n = ((sf_o as isize + delta).max(0) as usize).min(n_new.saturating_sub(1));
    let sl_n = ((sl_o as isize + delta).max(0) as usize).min(n_new.saturating_sub(1));
    let (cs_o, ce_o, cs_n, ce_n) = crate::infer::zone_derive_stage1(
        n_old, n_new, delta, rs, re, sf_o, sl_o, sf_n, sl_n);

    // [cache_fix_824 纠错] patch gate case 190 实证——diff 的 μ
    // 配对需要块对齐 zone(切断段落 rel-span 不齐 ⟹ 截断 REM+ADD ⟹ 错 REPL);
    // 权威路径(kernel 切断时回退)必须块对齐;物化侧(splice)保持就近。
    let cs_o = crate::infer::expand_zone_cs_blocks_fl(
        &task.snap_old.frame_log.entries, &task.snap_old.lines, &task.snap_old.open_depth,
        task.snap_old.line_len(), cs_o);
    let cs_n = crate::infer::expand_zone_cs_blocks_fl(
        &task.snap.frame_log.entries, &task.snap.lines, &task.snap.open_depth,
        task.snap.line_len(), cs_n);
    // 阶段 3:cs 侧容器闭包(depth0 锚 + min_push);ce = rle+1(工作量界)。
    // 跨界对象(容器/块跨 rle)不进解析区——diff 侧经 end-patch 补尾。
    // A1 举证：10 文档 × 4 编辑 × CF/HS = 80 组 patch 流
    // hash 与该块接线版逐条全等——本块输出不参与可观察补丁；保留死算备查，
    // 删除或接线须另立性能/语义单。
    let conv_old = ((re as isize - delta).max(0) as usize).min(n_old.saturating_sub(1));
    // zone 单套 = ContainerClosure（军令状 B1/B4：壳域由适配层自算——
    // 阶段 3：cs 侧容器闭包 + ce 块闭包（直接遮蔽赋值，无分派）。
    {
        let cs_o = crate::infer::expand_zone_cs_containers_fl(
                &task.snap_old.frame_log.entries, &task.snap_old.open_depth, n_old, cs_o,
                (conv_old + 1).min(n_old));
            let cs_n = crate::infer::expand_zone_cs_containers_fl(
                &task.snap.frame_log.entries, &task.snap.open_depth, n_new, cs_n, (re + 1).min(n_new));
            // 防御条款(case 23 反例):ce 侧 cont/setext 前看以"不可补尾"
            // 为条件(case 23 反例——setext 跨界对——不可补尾时
            // walk 重解析)。anchor = 首编辑起点字节的旧侧行(与 seam 描述符
            // 共用 anchor_line——单一来源);旧块索引廉价门——旧 full 不可得
            // 时降级无条件 walk。
            // [cache_fix_824 §1] 注意:diff 侧 keep 工作限(typing 块级 O(块)——
            // P0 契约门)——容器闭包只属物化侧(splice 密封域),compute_zone
            // 不接线 zone_ce_closure_fl。
            let anchor = crate::incr::anchor_line(&task.snap_old, &task.edit_ranges);
            // cache_fix_824 §6.3 A2:索引进化——slot full 不可得时从当手
            // old_full 派生(冷路径不再无条件 walk——巨段落打字 zone 不膨胀)。
            let old_idx = crate::cache::old_block_index_of_own(&task.snap_old);
            let ce_o_raw = (conv_old + 1).min(n_old);
            let ce_n_raw = (re + 1).min(n_new);
            let ce_o = crate::infer::expand_zone_ce_blocks_fl(
                &task.snap_old.frame_log.entries,
                &task.snap_old.lines,
                &task.snap_old.open_depth,
                n_old,
                ce_o_raw,
                anchor,
                old_idx.as_deref().map(|v| v.as_slice()),
                ce_o_raw,
            );
            let ce_n = crate::infer::expand_zone_ce_blocks_fl(
                &task.snap.frame_log.entries,
                &task.snap.lines,
                &task.snap.open_depth,
                n_new,
                ce_n_raw,
                anchor,
                old_idx.as_deref().map(|v| v.as_slice()),
                ce_o_raw,
            );
    }
    // 阶段 4:zone 尾对称对齐(symmetrize 保留 diff 算术——纯算术,删除
    // 的只是它"扩大解析区"的角色;新旧 suffix 切点由 rle±δ 自动对齐)。
    let (ce_o, ce_n) = line_map.symmetrize_ce(ce_o, ce_n, n_old, n_new);
    // [§13-1 判据②] 与物化侧**同一个**尾对称不动点（禁止各写一份）：symmetrize 只做行映射，
    // 而 to_old/to_new 在锚点悬崖非单调（§10.1-2）⇒ 它可能把 ce 重新推回块中，
    // 故尾扩展必须排在 symmetrize **之后**。
    let (ce_o, ce_n) = crate::cache::tail_symmetry_fixpoint(
        &task.snap_old, &task.snap, ce_o, ce_n, line_map);
    // 阶段 5：cs-1 上下文 + zs 锚（编辑点前的行号不偏移 δ）。
    let cs_o = cs_o.saturating_sub(1);
    let cs_n = cs_n.saturating_sub(1);
    let mut zs_o = task.snap_old.snap_to_depth0(cs_o) as usize;
    let mut zs_n = task.snap.snap_to_depth0(cs_n) as usize;
    // [§13-12] 权威路径补 zs **块起点裁决**（与物化侧 cache::zone_align_to_blocks 同律）：
    // 原实现只做 snap_to_depth0——depth-0 区不回退到块首 ⇒ zs 可落在块中/空行上来
    // （判据①"zs 在两侧都是块起点"在权威路径不成立，审计 N2/T07）。两侧各自裁决后
    // 仍由 align_zs 取 min（与物化侧"各自对齐再由调用方 min"语义一致）。
    zs_o = crate::cache::zone_align_to_blocks(&task.snap_old, zs_o, ce_o).0;
    zs_n = crate::cache::zone_align_to_blocks(&task.snap, zs_n, ce_n).0;
    // 适配层域从壳区间出发，天然不切章节；不再引用 hs_section_anchor。
    let (zs_o, zs_n) = line_map.align_zs(zs_o, zs_n);
    ZonePair { cs_o, ce_o, zs_o, cs_n, ce_n, zs_n }
}


// PRC P4b.2 kernel-zone relation for diff_patches_mode.
// Exact-match mode is always safe: a cached mid whose RCZ exactly equals the
// computed zone is, by construction, the same Segment build.  Containment-
// override mode is enabled only after the randomized containment gate
// (Task 2) and the randomized patch-equivalence gate (Task 5) both pass.

pub(crate) struct KernelDiffParts {
    pub(crate) old: Option<Arc<crate::cache::Segment>>,
    pub(crate) new: Option<Arc<crate::cache::Segment>>,
    pub(crate) override_zone: bool,
}

/// Conservative kernel-zone wiring decision.  Only `ContainerClosure` zones
/// may consult the kernel; `SectionClosure` must always fall back to the
/// authoritative `compute_zone` + `build_structure_parts` path.
/// RCZ-authority path: returns kernel old/new mids without consulting
/// `compute_zone`. Used by `diff_patches_mode` so the kernel RCZ is the
/// zone decision, not a post-hoc override over an already-computed zone.
pub(crate) fn kernel_diff_parts_override(
    task: &IncrTask,
    topology: &crate::topology::Topology,
) -> Option<KernelDiffParts> {
    if !KERNEL_ZONE_OVERRIDE_ENABLED {
        return None;
    }
    // mid_ext 是 CF 界(rle+1)语义——materialize_mid_ext 内部走
    // CF 的 materialize_zone/materialize_old_zone,与 HS 的章节闭包界不同域。
    // HS 恒回退权威 compute_zone + build_structure_parts(实测:长章节标题
    // 编辑的 Replace 值被 CF 界段截断——para 1..29 静默丢失)。
    if topology.zone != crate::topology::ZoneRule::ContainerClosure {
        return None;
    }
    let slot = &task.slot.as_ref()?.0;
    let slot_node = slot.node.load_full()?;
    let node = match &**slot_node {
        crate::cache::CacheNode::Edit { .. } => (*slot_node).clone(),
        crate::cache::CacheNode::Base { .. } => {
            // Phase A 修正：pass-through 后 slot 持有独立 Base，不再是 Edit。
            // CF kernel 仍需要一个 task 作用域的 Edit 描述符来复用
            // materialize_zone/old_zone/mid 的既有路径；该描述符不落槽、
            // 不共享、不物化 full，只作为本任务的 kernel 视图。
            let n_new = task.snap.line_len();
            let rls = task.ripple_line_start;
            let rle_e = (task.ripple_line_end + 1).min(n_new);
            let anchor = crate::incr::anchor_line(&task.snap_old, &task.edit_ranges);
            crate::cache::CacheNode::new_edit_full(
                &task.snap_old,
                task.snap.clone(),
                (rls, rle_e),
                task.edit_ranges.clone(),
                task.delta,
                anchor,
            )
        }
    };
    // 代际门(cache_fix_823):P5 守卫透传会把旧代 Edit 存进新代 slot
    // (结构同一——继承设计)——该节点的 zone/old_snap 属旧代,域与 task
    // 不匹配;代际不符一律回退权威 compute_zone(结构同一时 diff 无补丁,
    // 正确性不变,只损一次缓存命中)。
    if !Arc::ptr_eq(node.basis(), &task.snap) {
        return None;
    }
    let node_old = node.old_snap()?;
    if !Arc::ptr_eq(&node_old, &task.snap_old) {
        return None;
    }
    // override 恒 CF-only，HS 恒回退权威
    // compute_zone + build_structure_parts。
    let (kzs, kce) = node.materialize_zone();
    let (kzso, kceo) = node.materialize_old_zone()?;

    // cache_fix_824 S10:zs 就近可切块——内核 diff 的路径算术按"块对齐输入"
    // 构建;切块情形回退权威 compute_zone。P4：切块认证用局部 3 行探针，
    // 不再经 `old_block_index_of_own` 拖旧侧 full。
    {
        let zs_eff = kzs.min(kzso);
        // [S5 账本标注] 同上：口径受限的近似谓词（3 行窗），结果不得缓存。
        let probe_cut = |snap: &Snapshot, zs: usize, n: usize| -> bool {
            if zs == 0 || zs >= n {
                return false;
            }
            let probe = crate::cache::Segment::build(snap, zs.saturating_sub(1), (zs + 2).min(n));
            !probe.blocks.iter().any(|b| b.start as usize == zs)
        };
        if probe_cut(&task.snap_old, zs_eff, task.snap_old.line_len())
            || probe_cut(&task.snap, zs_eff, task.snap.line_len())
        {
            return None;
        }
    }
    // [§13-2] 尾对称预判（同一契约的"只判"形态）：ce 不是块起点（⇒ 不是前一块的块尾）
    // 即回退权威路径。顺序硬约束：物化侧的尾扩展（cache::tail_symmetry_fixpoint）必须
    // 已经落地——否则打字类编辑两侧 ce 都停在段落中部（infer.rs:6700-6711 可补尾短路），
    // kernel 会对每个段落内编辑 return None ⇒ RCZ 对打字全线失效（§13-1 依据）。
    {
        let tail_cut = |snap: &Snapshot, ce: usize, n: usize| -> bool {
            ce > 0 && ce < n && !crate::cache::Segment::build(
                snap, ce.saturating_sub(1), (ce + 2).min(n))
                .blocks.iter().any(|b| b.start as usize == ce)
        };
        if tail_cut(&task.snap_old, kceo, task.snap_old.line_len())
            || tail_cut(&task.snap, kce, task.snap.line_len())
        {
            return None;
        }
    }

    // Dual-coordinate support can legitimately start on different lines
    // (e.g. an edit anchored before ripple start). Align the starts so both
    // trees see the same prefix root; otherwise an unchanged root appears
    // old-only and is emitted as a false Remove.
    let zs = kzs.min(kzso);
    // cache_fix_824 §3:mid 化——materialize zone 已含容器闭包(S2),
    // 折叠产物即完整记录(无跨界——无 end-patch 需求);旧侧同律。
    let old = if kzso == zs {
        node.materialize_old_mid()?
    } else {
        Arc::new(crate::cache::Segment::build(&task.snap_old, zs, kceo))
    };
    let new = if kzs == zs {
        node.materialize_mid()?
    } else {
        Arc::new(crate::cache::Segment::build(&task.snap, zs, kce))
    };
    Some(KernelDiffParts {
        old: Some(old),
        new: Some(new),
        override_zone: true,
    })
}

// RCZ-authority switch. The randomized patch-equivalence gate is the
// authority: if it ever fails with this enabled, the gate must fail and this
// must be set back to false. Containment vs compute_zone is informational,
// because compute_zone intentionally over-approximates via edit_ranges.
const KERNEL_ZONE_OVERRIDE_ENABLED: bool = true;


// SectionClosure（HS 的 ripple 映射——§七）
// 输入 = ripple 区间(既成事实——ripple == diff——拓扑无关的行级扫描)
// 映射 = ripple 区间内标题变化 → 四类依赖传播(§5.5 边界依赖模型) →
//        受影响章节集合 → 区间并集 → zone(章节完整)
// 行级实现(标题 = 单行——不建块记录)——章节区间无容器感知
// (over-approximation——传播保守安全——§7.3 S ⊇ A 正确性不损)


/// 块级标题列表(缓存段块记录过滤——与树级 section_reorganize 同源——
/// 标题块 ∈ heading 候选——含 BQ 内 setext(HsHead1/2 块——块层已 resolve
/// 容器前缀——行级 header_lines 的 line_type 检测漏 BQ 内 setext)。
/// 消费面 = tests/unit/telemetry_cache.rs 的 HS 成本测试（cfg(test) include
/// 进 lib 测试）；生产热路径不调用。
pub(crate) fn block_header_lines(snap: &Snapshot,
                                 heading: &[crate::block::BlockType]) -> Vec<(usize, u8)> {
    let n = snap.line_len();
    let seg = crate::cache::structure_for_range(snap, 0, n);
    seg.blocks.iter()
        .filter(|b| heading.contains(&b.block_type))
        .map(|b| (b.start as usize, crate::block::header_level(b.block_type).unwrap_or(0)))
        .collect()
}


/// 行级章节区间(level 单调栈——无容器感知——over-approximation——
/// 终点 = 下一 level≤ 标题 或文档尾(修:原 usize::MAX 转 isize 溢出(-1)——比较错乱))。
/// pub(crate)（SDL 拓扑投影审计——双实现比对:树级 section_reorganize
/// (容器感知——精确)⊆ 行级(over-approximation)）。
pub(crate) fn line_section_intervals(heads: &[(usize, u8)], n_lines: usize) -> Vec<(usize, usize, u8)> {
    heads.iter().enumerate().map(|(i, &(s, lv))| {
        let next = heads[i + 1..].iter()
            .find(|&&(_, l)| l <= lv)
            .map(|&(e, _)| e)
            .unwrap_or(n_lines);
        (s, next, lv)
    }).collect()
}

/// 四类依赖传播的受影响集合(测试参照——unit/incr 的 S ⊇ A 门禁用)。
#[cfg(test)]
fn affected_sections(secs: &[(usize, usize, u8)], changed: &[(usize, u8)]) -> std::collections::HashSet<usize> {
    let mut affected = std::collections::HashSet::new();
    for &(pos, lv) in changed {
        for (i, &(s, e, _)) in secs.iter().enumerate() {
            if s <= pos && pos < e { affected.insert(i); }
        }
        if lv > 0 {
            for (i, &(_, e, _)) in secs.iter().enumerate() {
                if e == pos { affected.insert(i); }
            }
            for (i, &(s, _, l)) in secs.iter().enumerate() {
                if s >= pos {
                    affected.insert(i);
                    if l <= lv { break; }
                }
            }
        }
    }
    affected
}

/// 四类依赖传播(§5.5 边界依赖模型)——编辑标题(位置 pos、该侧 level lv)
/// 受影响章节:
///   ① 区间含 pos(编辑所在章节——终点依赖编辑标题)
///   ② 区间终点 = pos(前驱——终点邻接——标题删除/移动时终点延伸)
///   ③ 传播链:pos 之后的区间起点(到下一个 level ≤ lv 停——后续章节的
///     起点依赖编辑标题)
///   (④ 外层——区间含 pos 的更大章节——已含于 ①——覆盖所有含 pos 的
///   章节——含 ③ 链中标题的章节由 ①(链中标题变化时)覆盖)
/// v2 预实验的 S ⊇ A 方法(门禁测试——传播集合 ⊇ 实际变化)。
/// SectionClosure 完整映射(compute_zone 的 SectionClosure 分派——
/// 一次对比两侧——旧/新章节区间并集 → zone)。
/// 局部安全范围版：只扫描 ripple 邻域标题，并向前/后按章节边界
/// 展开；不再全量 header_lines(0,n)。
/// Phase B: 从 SecsCell 取标题行，避免 block_header_lines 全量块扫描。
fn secs_heading_lines(snap: &Snapshot) -> Vec<(usize, u8)> {
    let cell = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
    cell.secs_iter().map(|(s, _e, lv, _t)| (s, lv)).collect()
}

fn section_closure_zone(task: &IncrTask, line_map: &LineMap,
                        cs_o: usize, ce_o: usize,
                        cs_n: usize, ce_n: usize) -> (usize, usize, usize, usize) {
    let delta = task.delta;
    let rs = task.ripple_line_start;
    let re = task.ripple_line_end.max(rs);

    // 标题变化只在 ripple 区间检测；旧侧坐标用 LineMap 映射。
    // 827 阶段三:标题列表改块级(缓存段块记录过滤)——行级 header_lines
    // 漏引用块内 setext(块层已解析),S ⊇ A 缺口。
    let n_old = task.snap_old.line_len();
    let old_lo = line_map.to_old(rs).min(n_old.saturating_sub(1));
    let old_hi = line_map.to_old(re).min(n_old.saturating_sub(1)).max(old_lo);
    let mut old_heads: Vec<(usize, u8)> =
        secs_heading_lines(&task.snap_old)
            .into_iter()
            .filter(|&(p, _)| {
                let np = (p as isize + delta).max(0) as usize;
                np >= rs && np <= re
            })
            .collect();
    let new_heads: Vec<(usize, u8)> =
        secs_heading_lines(&task.snap)
            .into_iter()
            .filter(|&(p, _)| p >= rs && p <= re)
            .collect();

    let mut changed_o: Vec<(usize, u8)> = Vec::new();
    let mut changed_n: Vec<(usize, u8)> = Vec::new();
    for &(np, nlv) in &new_heads {
        let op = (np as isize - delta).max(0) as usize;
        match old_heads.iter().find(|&&(p, _)| p == op) {
            Some(&(_, olv)) => {
                let ot = task.snap_old.rope.line(op).to_string();
                let nt = task.snap.rope.line(np).to_string();
                if olv != nlv || ot != nt {
                    changed_o.push((op, olv));
                    changed_n.push((np, nlv));
                }
            }
            None => {
                changed_o.push((op, 0)); // 新增
                changed_n.push((np, nlv));
            }
        }
    }
    for &(op, olv) in &old_heads {
        let np = (op as isize + delta).max(0) as usize;
        if !new_heads.iter().any(|&(p, _)| p == np) {
            changed_o.push((op, olv));
            changed_n.push((np, 0)); // 删除
        }
    }

    // 受影响范围：每个变化标题的章节 + 前驱/后继传播。
    // 标题列表用块跳扫描物化（O(#block starts)），prev/next 二分；不再
    // 逐行 prev_heading / next_heading_le。
    let old_block_heads = if changed_o.is_empty() {
        Vec::new()
    } else {
        secs_heading_lines(&task.snap_old)
    };
    let new_block_heads = if changed_n.is_empty() {
        Vec::new()
    } else {
        secs_heading_lines(&task.snap)
    };

    let mut cs_o2 = cs_o;
    let mut ce_o2 = ce_o;
    for &(pos, lv) in &changed_o {
        let own_end = if lv > 0 {
            crate::infer::next_heading_le_in(&old_block_heads, pos, lv, n_old)
        } else {
            pos + 1
        };
        let (start, end) = match crate::infer::prev_heading_in(&old_block_heads, pos) {
            Some((ps, plv)) => {
                let containing_end = crate::infer::next_heading_le_in(&old_block_heads, ps, plv, n_old);
                (ps, containing_end.max(own_end))
            }
            None => (0, own_end),
        };
        cs_o2 = cs_o2.min(start);
        ce_o2 = ce_o2.max(end.max(pos + 1));
    }

    let mut cs_n2 = cs_n;
    let mut ce_n2 = ce_n;
    for &(pos, lv) in &changed_n {
        let own_end = if lv > 0 {
            crate::infer::next_heading_le_in(&new_block_heads, pos, lv, task.snap.line_len())
        } else {
            pos + 1
        };
        let (start, end) = match crate::infer::prev_heading_in(&new_block_heads, pos) {
            Some((ps, plv)) => {
                let containing_end = crate::infer::next_heading_le_in(&new_block_heads, ps, plv, task.snap.line_len());
                (ps, containing_end.max(own_end))
            }
            None => (0, own_end),
        };
        cs_n2 = cs_n2.min(start);
        ce_n2 = ce_n2.max(end.max(pos + 1));
    }

    (cs_o2, ce_o2, cs_n2, ce_n2)
}

/// 节列表覆盖之外的区间（节外空隙）——新侧空隙根重挂 / 规则 3 的 B1 补 REM 共用。
/// 语义与 adapt 内联版一致：节区间按闭区间处理，空隙 = 节列表的补集。
fn gap_intervals(secs: &[(usize, usize, u8)], n_lines: usize) -> Vec<(usize, usize)> {
    let mut gaps = Vec::new();
    let mut prev_end = 0usize;
    for &(ss, se, _) in secs {
        if prev_end < ss { gaps.push((prev_end, ss)); }
        prev_end = prev_end.max(se);
    }
    if prev_end < n_lines { gaps.push((prev_end, n_lines)); }
    gaps
}

/// align 决策——zone 内的共享容器层（依赖 spans，在 zone 的 ResolveView
/// （RowView）构建后调用）。
/// 输出对齐层与树构建参数（align==0: 从 zs 构建；align>0: 从 align 容器构建）。
struct AlignPlan {
    align: usize,
}

fn compute_align(task: &IncrTask, zone: &ZonePair,
                 o_spans: &dyn ResolveView, n_spans: &dyn ResolveView) -> AlignPlan {
    let ZonePair { cs_o, ce_o, cs_n, ce_n, .. } = *zone;
    let delta = task.delta;
    // Shared parent depth from zone ResolveView（RowView）。
    let align = {
        let md_o = (cs_o..ce_o.min(o_spans.zone_end())).map(|li| o_spans.close_depth_at(li)).min().unwrap_or(0);
        let md_n = (cs_n..ce_n.min(n_spans.zone_end())).map(|li| n_spans.close_depth_at(li)).min().unwrap_or(0);
        let mut a = md_o.min(md_n);
        for li in cs_o..ce_o.min(o_spans.zone_end()) {
            if o_spans.is_container_change(li) { a = a.min(o_spans.close_depth_at(li).saturating_sub(1)); }
        }
        for li in cs_n..ce_n.min(n_spans.zone_end()) {
            if n_spans.is_container_change(li) { a = a.min(n_spans.close_depth_at(li).saturating_sub(1)); }
        }
        for li in cs_o..ce_o.min(o_spans.zone_end()) {
            let nl = ((li as isize + delta).max(0) as usize).min(n_spans.zone_end().saturating_sub(1));
            if o_spans.is_loose_at(li)
               != n_spans.is_loose_at(nl) {
                a = a.min(o_spans.close_depth_at(li).saturating_sub(1));
            }
        }
        // Verify the container chain at cs matches between old and new.
        // align assumes depth-1..align containers are SHARED.  If cs sits at
        // a different container in old vs new (e.g. "- x" root BLT prepended
        // before a BQ: old chain [BQ], new chain [BLT]), they are siblings,
        // not a shared parent — align must drop to the divergence depth.
        let mut chain_ok = a;
        for d in 0..a {
            let ob = o_spans.node_type_at_depth(cs_o, d);
            let nb = n_spans.node_type_at_depth(cs_n, d);
            if ob != nb {
                chain_ok = d;
                break;
            }
        }
        a.min(chain_ok)
    };

    // 对齐容器由树按锚点行定位（对齐分支改动）；
    AlignPlan { align }
}


/// 增量事实的交付上下文（diff_patches_mode_with_ctx 的产出——适配层
/// （adapt）消费）。内容 = CF zone 路径上已经建好的产出物（段/树/坐标），
/// 适配层拿它做 AST 序 walk（节点身份）与值渲染，无需重建 zone 结构件。
/// 注意：本结构是"incr 侧单套 zone"的交付物——物化侧（cache）的
/// zone_both_sides 是另一轨（825 边界外，不并轨）。
pub(crate) struct ZoneContext {
    pub(crate) zone: ZonePair,
    /// 旧/新侧 zone 段（[zs,ce) 的结构件——records/blocks/view 容器）。
    pub(crate) o_seg: Arc<crate::cache::Segment>,
    pub(crate) n_seg: Arc<crate::cache::Segment>,
    /// 旧/新侧 CF 归属树（diff_level 的输入——树序；适配层 walk 时按
    /// AST 序换算：wrap + inline_roots 偏移，见 item_inline_roots/wrap 逻辑）。
    pub(crate) o_tree: crate::infer::AttrTreeNew,
    pub(crate) n_tree: crate::infer::AttrTreeNew,
    /// zone 首根之前的根计数（ZoneTransform::first_old/first_new）——
    /// patch 路径第 0 段 = off + 域内根序号，归一到 zone 树 roots 用。
    pub(crate) off_old: usize,
    pub(crate) off_new: usize,
    pub(crate) snap_old: Arc<Snapshot>,
    pub(crate) snap_new: Arc<Snapshot>,
}

/// 拓扑分派的增量入口（HS 的 diff 走 diff_patches_mode(task, &HEADINGSECTION_TOPOLOGY)
/// ——compute_zone 按 ZoneRule 分派(SectionClosure——ripple 映射)——
/// 建树按拓扑候选(章节壳)——diff_level 壳块递归)。
pub fn diff_patches_mode(task: &IncrTask, topology: &crate::topology::Topology) -> Vec<Patch> {
    diff_patches_mode_with_ctx(task, topology).0
}

/// 增量事实 + 交付上下文（阶段 1：行为零变化的纯抽出——主体照搬，
/// 只把 zone/段/树/快照装进 ZoneContext 一并返回）。
pub(crate) fn diff_patches_mode_with_ctx(
    task: &IncrTask,
    topology: &crate::topology::Topology,
) -> (Vec<Patch>, ZoneContext) {
    diff_patches_mode_with_ctx_g(task, topology, crate::topology::patch_granularity())
}

/// 显式粒度变体(测试/双粒度判官用——免全局开关的并行干扰;
/// 生产路径经 with_ctx 读 env 一次)。
pub(crate) fn diff_patches_mode_with_ctx_g(
    task: &IncrTask,
    topology: &crate::topology::Topology,
    g: crate::topology::Granularity,
) -> (Vec<Patch>, ZoneContext) {
    let n_old = task.snap_old.line_len();
    let _span_pz = crate::telemetry_span!("patch_zone",
        "rls" => task.ripple_line_start, "rle" => task.ripple_line_end,
        "delta" => task.delta, "edits" => task.edit_ranges.len());
    let line_map = LineMap::new(task);
    // ContainerClosure with an Edit slot: the kernel RCZ is the zone decision;
    // compute_zone is only a fallback when no kernel mids are available.
    // SectionClosure 恒回退权威 compute_zone(kernel override
    // 的 mid_ext 是 CF 界语义——与章节闭包界不同域)。
    let kernel_parts = if topology.zone == crate::topology::ZoneRule::ContainerClosure {
        kernel_diff_parts_override(task, topology)
    } else {
        None
    };
    let mut zone = if kernel_parts.is_some() {
        ZonePair {
            cs_o: 0,
            ce_o: 0,
            zs_o: 0,
            cs_n: 0,
            ce_n: 0,
            zs_n: 0,
        }
    } else {
        compute_zone(task, &line_map)
    };
    drop(_span_pz);
    let mut old_kernel: Option<Arc<crate::cache::Segment>> = None;
    let mut new_kernel: Option<Arc<crate::cache::Segment>> = None;
    if let Some(ref kp) = kernel_parts {
        let mut ok = true;
        if kp.override_zone {
            match (kp.old.as_ref(), kp.new.as_ref()) {
                (Some(old_mid), Some(new_mid)) => {
                    // Kernel RCZ is authoritative. Re-anchor cs from the ripple
                    // support start (mapped through LineMap) instead of trying
                    // to reuse compute_zone's cs, which may sit outside the
                    // kernel zone (compute_zone deliberately over-approximates).
                    let (zso, ceo) = (old_mid.start_line, old_mid.end_line);
                    let (zsn, cen) = (new_mid.start_line, new_mid.end_line);
                    // The kernel RCZ starts at a depth-0 block boundary. Anchor
                    // cs one line before it (compute_zone's cs-1 convention) so
                    // compute_align sees the pre-boundary depth and treats this
                    // as a root-level diff.
                    zone.zs_o = zso;
                    zone.ce_o = ceo;
                    zone.zs_n = zsn;
                    zone.ce_n = cen;
                    zone.cs_o = zso.saturating_sub(1);
                    zone.cs_n = zsn.saturating_sub(1);
                }
                _ => ok = false,
            }
        }
        if ok {
            old_kernel = kp.old.clone();
            new_kernel = kp.new.clone();
        }
    }
    let kernel_override = kernel_parts
        .as_ref()
        .map(|kp| kp.override_zone && kp.old.is_some() && kp.new.is_some())
        .unwrap_or(false)
        && old_kernel.is_some()
        && new_kernel.is_some();
    let ZonePair { cs_o, ce_o, zs_o, cs_n, ce_n, zs_n } = zone;

    // [A3-0] 根级偏移上提：`for_root` 与 `ctx_off_*` 的实参逐字相同
    // （`block_start_before(snap, zs)`），4 次执行 / 2 个唯一值。
    #[cfg(feature = "telemetry")]
    let _sp_rc_hoist = crate::telemetry_span!("root_count", "zs_o" => zs_o, "zs_n" => zs_n);
    // [A-1] 守卫：两侧 zone 锚相同（compute_zone 的 align_zs 取 min；kernel 覆写
    // 亦以同一 zs 建两侧段），且 α+1 严格落在三截拼接保证恒等的前缀 [0, rls) 内
    // ⇒ 新侧块起点 = 旧侧块起点（省一次新侧探针）。守卫不成立即原路计算。
    let reuse_new_block_start =
        zs_o == zs_n && zs_o.saturating_add(1) < task.ripple_line_start;
    let (root_off_o, root_off_n) = ZoneTransform::root_offsets(
        &task.snap_old, &task.snap, zs_o, zs_n, reuse_new_block_start);
    #[cfg(feature = "telemetry")] drop(_sp_rc_hoist);

    // zone 构建：记录（容器 + 块）+ view——组合原子（view arrays 单算）。
    // 内核命中侧直接用缓存 Segment 的 records/blocks/view；未命中侧走
    // build_structure_parts（完全兜底）。
    // 解析止于 rle 后,兜底侧需要跨界补尾——旧侧全量构建
    // 取真实 end(兜底路径,O(doc_old) 可接受),新侧 raw zone + 平移补尾
    // (禁解析 suffix)。anchor 单一来源(与 seam 描述符/物化共用)。
    let anchor = anchor_line(&task.snap_old, &task.edit_ranges);
    let _span_pb = crate::telemetry_span!("patch_build_old");
    let (o_seg, o_records, o_blocks, o_va) = if let Some(seg) = old_kernel.as_ref() {
        (
            seg.clone(),
            seg.records.clone(),
            seg.blocks.clone(),
            seg.view.clone(),
        )
    } else {
        // [cache_fix_824] 权威回退:zs 块对齐(diff 配对——case 190 实证) + ce
        // 经共享 fixpoint 闭包(截断 BQ 丢子树——case 281 实证;与内核 mid 同
        // 域语义)。typing 工作限由 compute_zone 保持(P0 契约——本路径仅
        // 无 slot 场景)。
        let ce_o_closed = crate::infer::zone_ce_closure_fl(
            &task.snap_old.frame_log.entries,
            &task.snap_old.lines,
            &task.snap_old.open_depth,
            task.snap_old.line_len(),
            cs_o,
            ce_o,
            anchor,
            crate::cache::old_block_index_of_own(&task.snap_old).as_deref().map(|v| v.as_slice()),
            ce_o,
        );
        let seg = crate::cache::Segment::build(&task.snap_old, zs_o, ce_o_closed);
        (
            std::sync::Arc::new(seg.clone()),
            seg.records,
            seg.blocks,
            seg.view,
        )
    };
    let o_zone = (o_seg.start_line as u32, o_seg.end_line as u32);
    let o_view = crate::infer::LineView::with_arrays(&o_records, &o_blocks,
        &task.snap_old.frame_log.entries, &task.snap_old.lines, &task.snap_old.rope,
        o_zone, o_va, &o_seg.arena);
    drop(_span_pb);
    let _span_pb2 = crate::telemetry_span!("patch_build_new");
    let (n_seg, n_records, n_blocks, n_va) = if let Some(seg) = new_kernel.as_ref() {
        (
            seg.clone(),
            seg.records.clone(),
            seg.blocks.clone(),
            seg.view.clone(),
        )
    } else {
        let ce_n_closed = crate::infer::zone_ce_closure_fl(
            &task.snap.frame_log.entries,
            &task.snap.lines,
            &task.snap.open_depth,
            task.snap.line_len(),
            cs_n,
            ce_n,
            anchor,
            crate::cache::old_block_index_of_own(&task.snap_old).as_deref().map(|v| v.as_slice()),
            ce_o,
        );
        let seg = crate::cache::Segment::build(&task.snap, zs_n, ce_n_closed);
        (
            std::sync::Arc::new(seg.clone()),
            seg.records,
            seg.blocks,
            seg.view,
        )
    };
    let n_zone = (n_seg.start_line as u32, n_seg.end_line as u32);
    let n_view = crate::infer::LineView::with_arrays(&n_records, &n_blocks,
        &task.snap.frame_log.entries, &task.snap.lines, &task.snap.rope,
        n_zone, n_va, &n_seg.arena);
    drop(_span_pb2);

    let plan = compute_align(task, &zone, &o_view, &n_view);
    // Kernel RCZ starts at a depth-0 boundary; root-level diff is the safe
    // path. 对齐容器由锚点行定位（与物化线/ce 闭包同源的 anchor 单一
    // 来源）；查找落空回退根级差分——慢但正确，绝不静默零事实
    // （patch gate 3000 case 1687）。
    let align = if kernel_override { 0 } else { plan.align };

    let _span_pt = crate::telemetry_span!("patch_tree");
    let (o_tree, n_tree, o_roots, n_roots, xform) = if align == 0 {
        // first_old/first_new = tree root[0]'s document index.  Trees start at
        // zs (depth-0), not cs (which may sit inside a container) — count from
        // zs so the offset matches the tree's first node.
        // [A3-0] 偏移已上提（root_off_o / root_off_n），此处只组装。
        let xform = ZoneTransform::for_root(root_off_o, root_off_n);
        // diff_level 直接消费 AttrTree——根索引数组 = tree.roots
        let o_attr = crate::infer::build_attribution_tree(
            &o_records, &o_blocks, &o_view, &o_seg.arena, o_zone, topology);
        let n_attr = crate::infer::build_attribution_tree(
            &n_records, &n_blocks, &n_view, &n_seg.arena, n_zone, topology);
        let (o_roots, n_roots) = (
            o_attr.roots.iter().map(|&r| r as usize).collect(),
            n_attr.roots.iter().map(|&r| r as usize).collect(),
        );
        (o_attr, n_attr, o_roots, n_roots, xform)
    } else {
        // 827-CF:对齐容器由锚点行定位（anchor = 首个编辑起点旧侧行——
        // 物化线/ce 闭包同源单一来源；新侧经行映射平移）。旧"回退行走"
        // 按深度跨容器边界，差分圈错容器、零事实（patch gate 3000
        // case 1687）。查找落空（树内无 depth==align-1 容器含锚点行）时
        // 回退根级差分——慢但正确（与物化线"守卫失败→重锚兜底"同律）。
        let a_o = anchor;
        let a_n = line_map.to_new(anchor);

        // align 场景:归属树全 zone 构建——根索引 = "区间含锚点行的
        // align 层容器节点"
        let o_attr = crate::infer::build_attribution_tree(
            &o_records, &o_blocks, &o_view, &o_seg.arena, o_zone, topology);
        let n_attr = crate::infer::build_attribution_tree(
            &n_records, &n_blocks, &n_view, &n_seg.arena, n_zone, topology);
        // [路径统一 阶段一] align 容器的文档路径改走**树轴**：与 patch 路径
        // 同源（ast_path_of = 父内孩子位置 + item 值 p 前缀），首段加文档根
        // 偏移 root_off_o。align_container_path 是 ci 轴（按深度全局
        // 容器计数器）+ d=0 从 0 扫块 + Listing 逐行扫，三套轴里最容易漂的
        // 一套（对账判官已证 ci 轴与父内序号轴在"同深度多父各带容器"时分叉）。
        // 定位失败（ast_path_of None）按同律回退根级差分——慢但正确，
        // 绝不静默零事实（与 patch gate 3000 case 1687 的处置一致）。
        let align_nodes = match (
            align_container_node(&o_attr, a_o, align),
            align_container_node(&n_attr, a_n, align),
        ) {
            (Some(o), Some(n)) => ast_path_of(&o_attr, &o_seg, &task.snap_old, o)
                .map(|mut comps| {
                    if let Some(c0) = comps.first_mut() {
                        *c0 += root_off_o;
                    }
                    (o, n, MD_PATH(comps))
                }),
            _ => None,
        };
        let (o_roots, n_roots, xform) = match align_nodes {
            Some((o, n, align_doc)) => {
                // Trees contain the align container itself.  Strip its own
                // index from the path to get the parent path, and pass the
                // index as the offset.
                let (parent_doc, ci_offset) = if align_doc.0.is_empty() {
                    (MD_PATH::root(), 0)
                } else {
                    let mut p = align_doc.0.clone();
                    let ci = p.pop().unwrap_or(0);
                    (MD_PATH(p), ci)
                };
                (
                    vec![o],
                    vec![n],
                    ZoneTransform::for_align(parent_doc.clone(), ci_offset),
                )
            }
            _ => (
                o_attr.roots.iter().map(|&r| r as usize).collect(),
                n_attr.roots.iter().map(|&r| r as usize).collect(),
                ZoneTransform::for_root(root_off_o, root_off_n),
            ),
        };
        (o_attr, n_attr, o_roots, n_roots, xform)
    };

    let ctx = DiffCtx { o_snap: &task.snap_old, o_spans: &o_view, n_snap: &task.snap,
        n_spans: &n_view,
        o_ze: o_zone.1 as usize, n_ze: n_zone.1 as usize };
    let (old_base, new_base, off_o, off_n) = xform.init_paths();
    let mut o_stack = old_base;
    let mut n_stack = new_base;
    let mut rem: Vec<(String, String, String)> = Vec::new();
    let mut add: Vec<(String, String, String)> = Vec::new();
    let mut repl: Vec<(String, String, String)> = Vec::new();
    let mut rattrs: Vec<Patch> = Vec::new();
    drop(_span_pt);
    let _span_pd = crate::telemetry_span!("patch_diff_level",
        "o_nodes" => o_tree.nodes.len(), "n_nodes" => n_tree.nodes.len(),
        "o_roots" => o_roots.len(), "n_roots" => n_roots.len());
    diff_level(&ctx, &o_tree, &o_roots, &n_tree, &n_roots,
        &mut o_stack, &mut n_stack, (off_o, off_n), false,
        &mut rem, &mut add, &mut repl, &mut rattrs, g);
    drop(_span_pd);
    // 纯插桩：diff_level 的产出计数（长度即规模——用于判定该 span 的成本是
    // 树规模驱动还是事实数驱动）。
    {
        let _sp_pd_counts = crate::telemetry_span!("pd_counts",
            "rem" => rem.len(), "add" => add.len(), "repl" => repl.len(), "rattrs" => rattrs.len());
    }
    // REM 桶内降序：apply 先删大 index 避免兄弟错位（diff_level 生成序为升序）。
    rem.reverse();
    let raw = rem.into_iter().chain(add).chain(repl);

    let mut patches: Vec<Patch> = raw.map(|(op, path, v)| match op.as_str() {
        "ADD" => Patch::Add { path, value: v },
        "REM" => Patch::Remove { path },
        _ => Patch::Replace { path, value: v },
    }).collect();
    // proj 面级补丁追加(排序:字段级独立于结构 arm——REPL 同级替换不
    // 冲突;REM 已先(降序),rattrs 恒在 REM/ADD 之后、与 REPL 同级)。
    patches.extend(rattrs);
    // ctx.off = 树首根的文档根序号(适配层 walk 的树→文档坐标),恒按
    // for_root 公式——align 分派的 xform.first_* 是对齐容器的序号,
    // 不是树首根序号(align=1 且对齐容器非文档首根时适配层 walk 全 None
    // ——case1687 [Hs] 暴露)。
    // [A3-0] 与 for_root 共用上提的同一对值（实参逐字相同，不再重算）。
    let ctx_off_o = root_off_o;
    let ctx_off_n = root_off_n;
    let c_ctx = ZoneContext {
        zone,
        o_seg,
        n_seg,
        o_tree,
        n_tree,
        off_old: ctx_off_o,
        off_new: ctx_off_n,
        snap_old: task.snap_old.clone(),
        snap_new: task.snap.clone(),
    };
    (patches, c_ctx)
}


// 适配层（L3——拓扑唯一入口）——incr_unify_826 §2


/// AST 序 walk 结果。
enum AdaptWalk {
    /// 树节点（目标树节点池索引）。
    Tree(usize),
    /// 槽位（inline/wrapper/tight-para）：宿主节点 + 槽位起的剩余段尾部。
    Slot { node: usize, tail: Vec<usize> },
}

/// 从 patch path（AST 序：0 段 = off+域内根序；item 层 = wrap+inline_roots 前缀
/// + 树 children 序；tight-para 槽位段无树节点）还原树节点身份。
/// 与 diff_level 同源换算（直接调用现成判定/函数——不复制规则）。
fn walk_ast_path(
    seg: &crate::cache::Segment,
    snap: &Snapshot,
    tree: &crate::infer::AttrTreeNew,
    path: &[usize],
    off: usize,
) -> Option<AdaptWalk> {
    // 827 阶段 1 P3 分项④：路径 walk（AST 序还原节点身份——零开销插桩）。
    let _span_adapt_walk = crate::telemetry_span!("adapt_walk");
    let root_idx = path.first()?.checked_sub(off)?;
    let va = seg.view.clone();
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), va, &seg.arena);
    let spans = &view as &dyn ResolveView;
    let mut cur = *tree.roots.get(root_idx)? as usize;
    let mut parent_item = false;
    for (k, &idx) in path[1..].iter().enumerate() {
        let node = &tree.nodes[cur];
        if node.is_item() {
            // ItemLeaf：AST ch 前缀 = wrap + inline_roots（现成函数）。
            let nd = snap.build_node(node.start as usize, node.end as usize,
                node.block_type, node.loose());
            let mut scratch = Vec::new();
            let plan = item_inline_plan(snap, &nd, spans, node.seg_depth as usize,
                list_frame_count(spans, node.start as usize, node.seg_depth as usize), &mut scratch);
            // 值 p 化:内容流前缀 = 值 p 占位(tag 不拆段,富文本在 p 内)。
            let prefix = value_p_prefix(&plan);
            if idx < prefix {
                return Some(AdaptWalk::Slot { node: cur, tail: path[1 + k..].to_vec() });
            }
            let rel = idx - prefix;
            // WC-13：rel 是**通道序**——0 槽孩子不占位，不能再按内部下标取。
            let Some((child, start)) = channel_to_internal(snap, tree, cur, rel, true) else {
                return Some(AdaptWalk::Slot { node: cur, tail: path[1 + k..].to_vec() });
            };
            if start != rel {
                // 落在孩子的多槽区间内部（tight 段落行内根平铺在父数组）⇒ 槽位层。
                return Some(AdaptWalk::Slot { node: cur, tail: path[1 + k..].to_vec() });
            }
            cur = child;
            parent_item = true;
            continue;
        }
        if parent_item && node.block_type == BlockType::Paragraph && !node.loose() {
            // tight-para 段落块：槽位段（无树 children）。
            return Some(AdaptWalk::Slot { node: cur, tail: path[1 + k..].to_vec() });
        }
        parent_item = false;
        // WC-13：idx 是**通道序**——跳过 0 槽孩子后取第 idx 个通道项；越界/落空 ⇒ 不可解。
        let (child, start) = channel_to_internal(snap, tree, cur, idx, false)?;
        if start != idx { return None; }
        cur = child;
    }
    Some(AdaptWalk::Tree(cur))
}

/// [路径统一 阶段一·改动2] **树轴路径原语**：行 → 路径分量（父内树孩子
/// 位置 + item 值 p 前缀）。ci 轴不存在于路径分量：ci 是
/// 按深度的全局容器计数器，与路径分量在"同深度多父各带容器"时分叉）。
/// 调用方需自备域段与文档根偏移（`path_str` 的首段）。
/// 判官：`path_to_root_matches_ast_path`（与 `ast_path_of` 逐值一致 +
/// 分量解析回的节点确实含该行）。
pub(crate) fn path_to_root(
    tree: &crate::infer::AttrTreeNew,
    seg: &crate::cache::Segment,
    snap: &Snapshot,
    line: usize,
) -> Option<Vec<usize>> {
    let idx = node_at_line(tree, line)?;
    ast_path_of(tree, seg, snap, idx)
}

/// 含 `line` 的最深节点（容器/块叶/item——树的 children 已含全孩子序）。
fn node_at_line(tree: &crate::infer::AttrTreeNew, line: usize) -> Option<usize> {
    fn walk(tree: &crate::infer::AttrTreeNew, idx: usize, line: usize, best: &mut Option<usize>) {
        let n = &tree.nodes[idx];
        if (n.start as usize) <= line && line < (n.end as usize) {
            *best = Some(idx);
            for &c in n.children_of(tree) {
                walk(tree, c as usize, line, best);
            }
        }
    }
    let mut best = None;
    for &r in &tree.roots {
        walk(tree, r as usize, line, &mut best);
    }
    best
}

/// AST 序路径（节点 → 目标树路径）：父链 + AST 换算（item 层
/// wrap + inline_roots + 树序；槽位尾巴由调用方追加）。
fn ast_path_of(
    tree: &crate::infer::AttrTreeNew,
    seg: &crate::cache::Segment,
    snap: &Snapshot,
    idx: usize,
) -> Option<Vec<usize>> {
    let mut parent = vec![usize::MAX; tree.nodes.len()];
    for (i, n) in tree.nodes.iter().enumerate() {
        for &c in n.children_of(tree) {
            parent[c as usize] = i;
        }
    }
    let va = seg.view.clone();
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), va, &seg.arena);
    let spans = &view as &dyn ResolveView;
    let mut comps = Vec::new();
    let mut cur = idx;
    loop {
        let p = parent[cur];
        let comp = if p == usize::MAX {
            // WC-13：0 槽（skip 型）节点**没有通道地址**——不产出 phantom 位。
            if channel_slots(snap, tree, cur, false) == 0 { return None; }
            tree.roots.iter().position(|&r| (r as usize) == cur)?
        } else {
            let pn = &tree.nodes[p];
            let kids = pn.children_of(tree);
            let internal = kids.iter().position(|&c| c as usize == cur)?;
            let p_item = pn.is_item();
            // WC-13：0 槽孩子不进通道 ⇒ 无地址（与发射侧的 `slots == 0` 门同源）。
            if channel_slots(snap, tree, cur, p_item) == 0 { return None; }
            // WC-13：pos = 前面兄弟的**通道槽位**前缀和（0 槽孩子不占位）。
            let mut pos = 0usize;
            for (i, &c) in kids.iter().enumerate() {
                if i == internal { break; }
                pos += channel_slots(snap, tree, c as usize, p_item);
            }
            if p_item {
                let nd = snap.build_node(pn.start as usize, pn.end as usize,
                    pn.block_type, pn.loose());
                let mut scratch = Vec::new();
                let plan = item_inline_plan(snap, &nd, spans, pn.seg_depth as usize,
                    list_frame_count(spans, pn.start as usize, pn.seg_depth as usize), &mut scratch);
                // 值 p 化:item 层流前缀 = 值 p 占位(tag 不拆段)。
                value_p_prefix(&plan) + pos
            } else {
                pos
            }
        };
        comps.push(comp);
        if p == usize::MAX { break; }
        cur = p;
    }
    comps.reverse();
    Some(comps)
}

/// 路径串（off_before 加在首段；tail = 槽位尾巴段——与节点路径拼接）。
fn path_str(comps: &[usize], off_before: usize, tail: &[usize]) -> String {
    let mut out = String::new();
    for (i, c) in comps.iter().enumerate() {
        if i == 0 {
            out.push_str(&format!("/ct/{}", off_before + c));
        } else {
            out.push_str(&format!("/ct/{c}"));
        }
    }
    for t in tail {
        out.push_str(&format!("/ct/{t}"));
    }
    out
}

/// 全文档 HS 树（仅标题删除/根级定位场景——O(n) 一次性，非热路径）。
/// 域构建（域 = 行区间 [ds, de)——段 + HS 树一次产出；军令状 H2：
/// 只建域内树，禁止全文档树）。
/// 827 优化（826 阶段 5——壳域缓存命中）：adapt 单次调用内的域构建缓存。
/// 键 = (快照, 域区间)；值 = (段, 域树)——同一快照同一域只 fold 一次。
/// 嵌套节父壳域被各子壳路径/壳渲染反复请求时只建一次；跨快照（旧/新侧）
/// 以快照指针区分，杜绝结构错配。
#[derive(Default)]
struct DomainCache {
    map: std::collections::HashMap<
        (usize, u32, u32),
        std::sync::Arc<(crate::cache::Segment, crate::infer::AttrTreeNew)>,
    >,
}

fn domain_parts(
    snap: &Snapshot,
    domain: (usize, usize),
    target: &crate::topology::Topology,
    cache: &mut DomainCache,
) -> Option<std::sync::Arc<(crate::cache::Segment, crate::infer::AttrTreeNew)>> {
    let (ds, de) = domain;
    if de <= ds {
        return None;
    }
    // 827 重写阶段 0/1:域密封——统一走 sealed_interval_of(单区间恒等;
    // 右界容器闭包 + 左界"开放容器"条件)。根级节/文档级空隙下恒等
    // (零回归);缓存键用密封后区间(同密封域同树)。
    let (ds, de) = crate::infer::sealed_interval_of(
        &snap.frame_log.entries, &snap.lines, &snap.open_depth, snap.line_len(), &[(ds, de)], ds);
    let key = (snap as *const Snapshot as usize, ds as u32, de as u32);
    if let Some(v) = cache.map.get(&key) {
        return Some(v.clone());
    }
    // 827 阶段 1 P3 分项②：域构建（段 + 域树——零开销插桩；缓存命中不打点）。
    let _span_adapt_parts = crate::telemetry_span!("adapt_parts",
        "ds" => ds, "de" => de, "n" => de - ds);
    let seg = crate::cache::Segment::build(snap, ds, de);
    let va = seg.view.clone();
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), va, &seg.arena);
    let tree = crate::infer::build_attribution_tree(
        &seg.records, &seg.blocks, &view, &seg.arena, (ds as u32, de as u32), target);
    let v = std::sync::Arc::new((seg, tree));
    cache.map.insert(key, v.clone());
    Some(v)
}


/// 目标节壳路径(827 重写阶段 1):密封域(sealed_interval_of 现成)+
/// 域树定位(locate_shell_in_domain)+ path_in_domain(ast_path_of)——
/// 取代 domain_root_path 的节列表父链(shell_at/outer_domain 同废)。
/// 域树自带容器父链与嵌套节结构,路径一次给全。
fn shell_path_in_tree(
    snap: &Snapshot,
    cell: &crate::projection::SecsCell,
    secs: &[(usize, usize, u8)],
    target: &crate::topology::Topology,
    sec_interval: (usize, usize),
    _heading: &[BlockType],
    cache: &mut DomainCache,
) -> Option<String> {
    let intervals: Vec<(usize, usize)> = secs.iter().map(|&(s, e, _)| (s, e)).collect();
    let (ds, de) = crate::infer::sealed_interval_of(
        &snap.frame_log.entries, &snap.lines, &snap.open_depth, snap.line_len(), &intervals, sec_interval.0);
    let part = domain_parts(snap, (ds, de), target, cache)?;
    let (seg, tree) = (&part.0, &part.1);
    let idx = locate_shell_in_domain(tree, sec_interval.0 as u32, sec_interval.1 as u32)?;
    let off = cell.root_count_before(seg.start_line);
    path_in_domain(snap, seg, tree, idx, off, &[])
}

/// 域前根计数（HS）：直读 `SecsCell.root_starts` 的二分前缀计数，
/// 与 `root_child_index_hs_from` 的测试区全量口径对拍。
fn hs_off_before(cell: &crate::projection::SecsCell, line: usize) -> usize {
    cell.root_count_before(line)
}

/// 同快照域内定位（结构导航：从域根按"区间包含"下行、剪枝——同一快照
/// 中 (start,end,bt,kind,item) 与树节点一一对应，是确定性导航，不是
/// 跨快照区间猜。军令状 H1：身份 = walk/节点引用；跨快照由 LineMap
/// 映射先行，域内同快照定位无歧义）。
fn locate_in_domain(
    tree: &crate::infer::AttrTreeNew,
    start: u32,
    end: u32,
    bt: BlockType,
    kind_some: bool,
    is_item: bool,
) -> Option<usize> {
    fn walk(tree: &crate::infer::AttrTreeNew, idx: usize,
            start: u32, end: u32, bt: BlockType, kind_some: bool, is_item: bool) -> Option<usize> {
        let n = &tree.nodes[idx];
        if n.start == start && n.end == end && n.block_type == bt
            && n.kind.is_some() == kind_some && n.is_item() == is_item {
            return Some(idx);
        }
        // 827 阶段二修正:剪枝只按起点(children 按 start 排序)——"n.end < end"
        // 对 item 不成立(item 区间止于自身行,块级子节点在其后)。
        if n.start > start { return None; }
        for &c in n.children_of(tree) {
            if let Some(f) = walk(tree, c as usize, start, end, bt, kind_some, is_item) {
                return Some(f);
            }
        }
        None
    }
    // "同签名多命中取最深"实测为**净负**（修 random#30 两处、引入
    // nest/inner_item_del、nest/outer_item_del、random#32/52/93 共 6 处）⇒ 回退。
    // 正确判别需要更强的信息（CF 路径深度/父链与域树深度的对应）。
    for &r in &tree.roots {
        if let Some(f) = walk(tree, r as usize, start, end, bt, kind_some, is_item) {
            return Some(f);
        }
    }
    None
}

/// [§13-6 修] 同签名多命中时取**最深**者（窄用途：嵌套容器区间完全相同——random#30 的
/// 内外 blockquote 因惰性续行同为 [0,4)；全局替换精确匹配实测会伤 6 例，故单列此函数供窄调用点）。
fn locate_in_domain_deepest(
    tree: &crate::infer::AttrTreeNew,
    start: u32,
    end: u32,
    bt: BlockType,
    kind_some: bool,
    is_item: bool,
) -> Option<usize> {
    fn walk(tree: &crate::infer::AttrTreeNew, idx: usize, start: u32, end: u32,
            bt: BlockType, kind_some: bool, is_item: bool, depth: usize,
            best: &mut Option<(usize, usize)>) {
        let n = &tree.nodes[idx];
        if n.start == start && n.end == end && n.block_type == bt
            && n.kind.is_some() == kind_some && n.is_item() == is_item {
            if best.map_or(true, |(_, d0)| depth > d0) { *best = Some((idx, depth)); }
        }
        if n.start > start { return; }
        for &c in n.children_of(tree) {
            walk(tree, c as usize, start, end, bt, kind_some, is_item, depth + 1, best);
        }
    }
    let mut best: Option<(usize, usize)> = None;
    for &r in &tree.roots {
        walk(tree, r as usize, start, end, bt, kind_some, is_item, 0, &mut best);
    }
    best.map(|(i, _)| i)
}

/// 壳节点定位（区间匹配 + kind=None + !is_item，不限定 block_type——
/// setext 标题行类型 = Paragraph 而壳块类型 = HsHead1，行级取不到；
/// 同一快照域内确定性导航，军令状 H1）。
fn locate_shell_in_domain(
    tree: &crate::infer::AttrTreeNew,
    start: u32,
    end: u32,
) -> Option<usize> {
    fn walk(tree: &crate::infer::AttrTreeNew, idx: usize, start: u32, end: u32) -> Option<usize> {
        let n = &tree.nodes[idx];
        if n.start == start && n.end == end && n.kind.is_none() && !n.is_item() {
            return Some(idx);
        }
        // 827 阶段二修正:剪枝只按起点(children 按 start 排序)——"n.end < end"
        // 剪枝对 item 不成立(item 节点区间止于自身行,块级子节点起点在
        // item 行之后——(4,5) 的 item 挂 (6,8) 的段落),把目标剪丢
        // (case1687 的 (6,9) 节壳定位 None)。
        if n.start > start {
            return None;
        }
        for &c in n.children_of(tree) {
            if let Some(f) = walk(tree, c as usize, start, end) {
                return Some(f);
            }
        }
        None
    }
    for &r in &tree.roots {
        if let Some(f) = walk(tree, r as usize, start, end) {
            return Some(f);
        }
    }
    None
}

/// 壳区间值渲染（壳 = 节区间 [s,e) 的节点——统一壳渲染入口；setext/
/// ATX 通吃，不依赖行级类型）。
fn render_shell_in_domain(
    snap: &Snapshot,
    domain: (usize, usize),
    target: &crate::topology::Topology,
    shell_interval: (usize, usize),
    cache: &mut DomainCache,
) -> Option<String> {
    // 827 阶段 1 P3 分项③：壳渲染（setext/ATX 通吃——零开销插桩）。
    let _span_adapt_shell = crate::telemetry_span!("adapt_shell_render");
    let part = domain_parts(snap, domain, target, cache)?;
    let (seg, tree) = (&part.0, &part.1);
    let idx = locate_shell_in_domain(&tree, shell_interval.0 as u32, shell_interval.1 as u32)?;
    let va = seg.view.clone();
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), va, &seg.arena);
    let mut buf = Vec::with_capacity(1024);
    crate::incr::render_tree(&mut buf, snap, &view, &tree, idx, target);
    Some(unsafe { String::from_utf8_unchecked(buf) })
}

/// WC-2（丙）：只渲染壳的**子数组**（\`[child,…]\`）——用于"旧侧没有该数组"时发一条
/// 整数组建键补丁（\`Add <壳路径>/ct\`），替代原先"整壳 REPL"的升级路径。
/// 与 render_shell_in_domain 同源（同一域、同一 render_node），只是跳过壳自身的 attrs。
fn render_children_array_in_domain(
    snap: &Snapshot,
    domain: (usize, usize),
    target: &crate::topology::Topology,
    shell_interval: (usize, usize),
    cache: &mut DomainCache,
) -> Option<String> {
    let part = domain_parts(snap, domain, target, cache)?;
    let (seg, tree) = (&part.0, &part.1);
    let idx = locate_shell_in_domain(&tree, shell_interval.0 as u32, shell_interval.1 as u32)?;
    let va = seg.view.clone();
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), va, &seg.arena);
    let mut buf = Vec::with_capacity(1024);
    {
        let mut bw = BufWriter::new(&mut buf);
        bw.write_slice(b"[");
        let mut nc = false;
        let mut scratch = crate::inline::InlineScratch::new();
        let mut ls = LineSupply::new(&snap.rope);
        for &c in tree.nodes[idx].children_of(&tree) {
            render_node(&mut bw, snap, &view, &tree, c as usize, &mut nc, &mut scratch, false, target, &(), &mut ls);
        }
        bw.write_slice(b"]");
        bw.commit();
    }
    Some(unsafe { String::from_utf8_unchecked(buf) })
}

/// WC-5：壳**自身面**的字段写（从 CF 事实值取——`lv`/`t`，值 = 裸 JSON 片段）。
/// 只处理壳的 attrs（`lv` 级别 / `t` 标题面）；出现未知字段差 → None
/// （调用方回退整壳 REPL，不猜）。`ct` 不是壳的面（HS 壳的 ct = 子通道，
/// CF 标题的 ct = 空内容面）——不进字段写，由子通道位置 diff 承担。
fn shell_attr_writes(shell_path: &str, old_v: &str, new_v: &str) -> Option<Vec<Patch>> {
    let o: serde_json::Value = serde_json::from_str(old_v).ok()?;
    let n: serde_json::Value = serde_json::from_str(new_v).ok()?;
    let (oo, no) = (o.as_object()?, n.as_object()?);
    // 非壳面字段出现差异（未知形态）→ 不字段化。
    for (k, v) in oo {
        if matches!(k.as_str(), "nt" | "ct" | "lv" | "t") { continue; }
        if no.get(k) != Some(v) { return None; }
    }
    for k in no.keys() {
        if !matches!(k.as_str(), "nt" | "ct" | "lv" | "t") { return None; }
    }
    let mut out = Vec::new();
    for k in ["lv", "t"] {
        let (a, b) = (oo.get(k), no.get(k));
        if a == b { continue; }
        let Some(b) = b else { return None };
        out.push(Patch::ReplaceAttr {
            path: shell_path.to_string(),
            attr: k.to_string(),
            value: serde_json::to_string(b).ok()?,
        });
    }
    // ct 差异且 lv/t 均无差异 → 形态未覆盖（标题内容面非空），回退。
    if oo.get("ct") != no.get("ct") && out.is_empty() { return None; }
    Some(out)
}

/// 壳孩子项：渲染值 + 跨快照配对身份（起始行/块类型/kind/item）。
struct ShellChild {
    start: usize,
    bt: BlockType,
    kind_some: bool,
    is_item: bool,
    value: String,
}

/// 收集壳的**孩子项**（渲染值 + 身份）。与 render_children_array_in_domain
/// 同域同壳，只是逐孩子返回而非拼整数组。
fn collect_shell_children(
    snap: &Snapshot,
    domain: (usize, usize),
    target: &crate::topology::Topology,
    shell_interval: (usize, usize),
    cache: &mut DomainCache,
) -> Option<Vec<ShellChild>> {
    let part = domain_parts(snap, domain, target, cache)?;
    let (seg, tree) = (&part.0, &part.1);
    let idx = locate_shell_in_domain(tree, shell_interval.0 as u32, shell_interval.1 as u32)?;
    let va = seg.view.clone();
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), va, &seg.arena);
    let mut out = Vec::new();
    for &c in tree.nodes[idx].children_of(tree) {
        // WC-13：只收"占通道位"的孩子——skip 孩子渲染值是空串，收进表会让
        // 下标偏移与"整数组 value 拼接"都出洞（[,X]）。
        if channel_slots(snap, tree, c as usize, false) == 0 { continue; }
        let cn = &tree.nodes[c as usize];
        let mut buf = Vec::with_capacity(256);
        {
            let mut bw = BufWriter::new(&mut buf);
            let mut nc = false;
            let mut scratch = crate::inline::InlineScratch::new();
            let mut ls = LineSupply::new(&snap.rope);
            render_node(&mut bw, snap, &view, tree, c as usize, &mut nc, &mut scratch, false, target, &(), &mut ls);
            bw.commit();
        }
        out.push(ShellChild {
            start: cn.start as usize,
            bt: cn.block_type,
            kind_some: cn.kind.is_some(),
            is_item: cn.is_item(),
            value: unsafe { String::from_utf8_unchecked(buf) },
        });
    }
    Some(out)
}

/// WC-5：壳的**子通道位置 diff**——取消整壳重发后，壳内容变化只发：
///   ① 旧侧多出的孩子 → REM（旧下标；全局排序通道转降序）
///   ② 新侧多出的孩子 → ADD（新下标升序；值 = 新侧域渲染的该孩子）
/// 配对 = **LineMap 映射先行**（旧孩子起始行 → 新侧行）+ 同块类型/kind/item
/// ——映射后同起点同型即同一节点（本文件"身份 = 映射 + 类型"的既定纪律）。
/// 不按渲染值配对：节壳的内部变化会导致渲染值不同，按值配对会在父级把整个
/// 子树当成"换了"（REM+ADD 父级壳），与子级位置 diff/事实级 op 双写
/// （实测 nested/hlevel：父级 ADD S11 与规则 3 的 ADD S12 双写 → 42 vs 41）。
/// `emitted` 里已有同 kind 同 path 的补丁（规则 2 重挂 / 规则 3 新壳 ADD /
/// 同节二次调用）→ 不重复发（幂等）。
/// 返回 `Some(true)` = **完整**（配对上的孩子渲染全同——本 diff 已覆盖该壳全部
/// 变化，调用方可标 affected 吞并列事实）；`Some(false)` = 有配对上的孩子内容变了
/// （未配对部分已发位置 op，但内容变化要留给 pass 2 的细粒度事实——调用方不得标
/// affected，否则事实被吞）；None = 渲染失败（调用方兜底）。
fn shell_children_position_ops(
    snap_o: &Snapshot,
    domain_o: (usize, usize),
    shell_o: (usize, usize),
    snap_n: &Snapshot,
    domain_n: (usize, usize),
    shell_n: (usize, usize),
    target: &crate::topology::Topology,
    cache: &mut DomainCache,
    shell_path: &str,
    line_map: &LineMap,
    emitted: &[Patch],
    out: &mut Vec<Patch>,
) -> Option<bool> {
    let co = collect_shell_children(snap_o, domain_o, target, shell_o, cache)?;
    let cn = collect_shell_children(snap_n, domain_n, target, shell_n, cache)?;
    let already = |kind: &str, path: &str| {
        emitted.iter().any(|p| match (kind, p) {
            ("ADD", Patch::Add { path: q, .. }) => q == path,
            ("REM", Patch::Remove { path: q }) => q == path,
            ("REPL", Patch::Replace { path: q, .. }) => q == path,
            _ => false,
        })
    };
    let mut matched_new = vec![false; cn.len()];
    let mut rem_idx: Vec<usize> = Vec::new();
    let mut complete = true;
    for (i, o) in co.iter().enumerate() {
        let t = line_map.to_new(o.start);
        let hit = cn.iter().enumerate().position(|(j, n)| {
            !matched_new[j] && n.start == t && n.bt == o.bt
                && n.kind_some == o.kind_some && n.is_item == o.is_item
        });
        match hit {
            Some(j) => {
                matched_new[j] = true;
                if cn[j].value != o.value { complete = false; }
            }
            None => rem_idx.push(i),
        }
    }
    let add_idx: Vec<usize> = (0..cn.len()).filter(|&j| !matched_new[j]).collect();
    if rem_idx.is_empty() && add_idx.is_empty() { return Some(complete); }
    // 整体建/删快路径（WC-2 丙语义的位置 diff 版）：未配对侧覆盖整个数组——
    //   旧侧全删（新侧空）→ 一条 `Remove <壳>/ct`；
    //   新侧全新（旧侧空）→ 一条 `Add <壳>/ct = <整数组>`；
    //   两侧非空且完全不同 → `Remove <壳>/ct` + `Add <壳>/ct = <整数组>`。
    let member = format!("{shell_path}/ct");
    // 同位 REPL 只在**全部未配对都是同位**（纯换值：rem_idx == add_idx，无删无插）
    // 且 diff 完整时用——此时排序通道里 REPL 单独应用（最后），不与 ADD 下标位移
    // 互相干扰。一旦还有 REM/ADD，同位项必须走 REM+ADD（apply 序 REM 降序 → ADD
    // 升序，ADD 下标用**删除后**坐标系）；否则前置 REM 会让 REPL 打到错位置
    // （实测 deep3/hdel：REM@0 + ADD@1..3 + REPL@0 → 目标 6 孩子只得 5，L1 被覆盖）。
    let mut reps: Vec<usize> = Vec::new();
    let rem_left: Vec<usize>;
    let add_left: Vec<usize>;
    if complete && !rem_idx.is_empty() && rem_idx == add_idx {
        reps = rem_idx.clone();
        rem_left = Vec::new();
        add_left = Vec::new();
    } else {
        rem_left = rem_idx.clone();
        add_left = add_idx.clone();
    }
    // 整体快路径（WC-2 丙语义的位置 diff 版）：无同位替换且未配对覆盖整个数组——
    //   旧侧全删 → 一条 `Remove <壳>/ct`；新侧全新 → 一条 `Add <壳>/ct = <整数组>`。
    if reps.is_empty() && rem_left.len() == co.len() && add_left.is_empty() && !co.is_empty() {
        if !already("REM", &member) { out.push(Patch::Remove { path: member }); }
        return Some(complete);
    }
    if reps.is_empty() && co.is_empty() && add_left.len() == cn.len() && !cn.is_empty() {
        if !already("ADD", &member) {
            let arr: Vec<&str> = cn.iter().map(|c| c.value.as_str()).collect();
            out.push(Patch::Add { path: member, value: format!("[{}]", arr.join(",")) });
        }
        return Some(complete);
    }
    // ADD 的下标是**删除后坐标系**（先 REM 后 ADD 的 apply 序）：在位项（配对 + 同位
    // 替换）与已插入项逐位占位，未配对项插入到当前位置——否则前置 REM 会让后续 ADD
    // 下标整体偏左（实测 deep3/hdel：REM 0 + ADD@1,2,3 → 目标 6 孩子只得 5）。
    let mut rep_set = vec![false; cn.len()];
    for &i in &reps { rep_set[i] = true; }
    let mut cur = 0usize;
    let mut add_pos: Vec<(usize, usize)> = Vec::new();
    for j in 0..cn.len() {
        if matched_new[j] || rep_set[j] { cur += 1; }
        else { add_pos.push((cur, j)); cur += 1; }
    }
    for &i in &reps {
        let path = format!("{shell_path}/ct/{i}");
        if already("REPL", &path) { continue; }
        out.push(Patch::Replace { path, value: cn[i].value.clone() });
    }
    for &i in rem_left.iter().rev() {
        let path = format!("{shell_path}/ct/{i}");
        if already("REM", &path) { continue; }
        out.push(Patch::Remove { path });
    }
    for &(pos, j) in &add_pos {
        let path = format!("{shell_path}/ct/{pos}");
        if already("ADD", &path) { continue; }
        out.push(Patch::Add { path, value: cn[j].value.clone() });
    }
    Some(complete)
}

/// WC-5：旧侧 CF 节点值渲染（对照新侧事实值取壳自身面）。
fn render_cf_node_value(
    snap: &Snapshot,
    seg: &crate::cache::Segment,
    tree: &crate::infer::AttrTreeNew,
    idx: usize,
) -> Option<String> {
    let va = seg.view.clone();
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), va, &seg.arena);
    let mut buf = Vec::with_capacity(256);
    crate::incr::render_tree(&mut buf, snap, &view, tree, idx, &crate::topology::CONTAINER_TOPOLOGY);
    Some(unsafe { String::from_utf8_unchecked(buf) })
}

/// 单节点值渲染（域内定位 + render_tree——军令状 D1：值渲染统一走
/// 节点所在域的渲染；域 = 节区间或节外区段）。
fn render_in_domain(
    snap: &Snapshot,
    domain: (usize, usize),
    target: &crate::topology::Topology,
    node_interval: (usize, usize),
    node_bt: BlockType,
    node_kind: Option<crate::block::FrameKind>,
    node_is_item: bool,
    cache: &mut DomainCache,
) -> Option<String> {
    let part = domain_parts(snap, domain, target, cache)?;
    let (seg, tree) = (&part.0, &part.1);
    let idx = locate_in_domain(&tree, node_interval.0 as u32, node_interval.1 as u32,
        node_bt, node_kind.is_some(), node_is_item)?;
    let va = seg.view.clone();
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), va, &seg.arena);
    let mut buf = Vec::with_capacity(1024);
    crate::incr::render_tree(&mut buf, snap, &view, &tree, idx, target);
    Some(unsafe { String::from_utf8_unchecked(buf) })
}

/// 域内路径（节点 → 目标域树的 AST 序路径；off_before 加首段；tail =
/// 槽位尾巴段——与节点路径拼接）。
fn path_in_domain(
    snap: &Snapshot,
    seg: &crate::cache::Segment,
    tree: &crate::infer::AttrTreeNew,
    idx: usize,
    off_before: usize,
    tail: &[usize],
) -> Option<String> {
    let comps = ast_path_of(tree, seg, snap, idx)?;
    Some(path_str(&comps, off_before, tail))
}

/// 归一化事实（827 P0）：CF 事实 → 双侧静态身份。
/// CF 事实坐标系：REM = 旧侧静态路径；ADD = 新侧静态路径；REPL = 新侧静态
/// 路径（旧侧配对身份被丢弃——diff_level 只带新侧路径）。目标拓扑交付需要
/// 双侧身份齐全：REPL 旧侧路径按层算术反推（apply-序不变量：配对旧节点在
/// REM 降序/ADD 升序全部落位后恰处 REPL 新侧路径——每层解 x − R(x) =
/// Pj − A(Pj)，R/A 为该层 REM/ADD 前缀索引计数，左式单调）。
/// mode-free 纯函数：输入只有 CF 事实流，不碰拓扑。
#[derive(Debug, Clone)]
struct NormFact {
    op: &'static str,      // "REM" | "ADD" | "REPL"
    old_path: Vec<usize>,  // 旧侧静态路径（REM/REPL 有效）
    new_path: Vec<usize>,  // 新侧静态路径（ADD/REPL 有效）
}

/// CF 事实流 → 归一化事实表。REPL 逐层解旧侧静态索引：
/// 设 REPL 新侧路径第 l 层索引 Pj，该层 REM 旧索引前缀计数 R(x)、
/// ADD 新索引前缀计数 A(y)，配对旧节点的该层静态索引 x 满足
/// x − R(x) = Pj − A(Pj)（左式 = 存活旧兄弟中位于其前者数）。
fn normalize_cf_facts(patches: &[Patch]) -> Vec<NormFact> {
    #[cfg(feature = "telemetry")]
    let _sp_norm = crate::telemetry_span!("norm_cf_facts");
    let rem_paths: Vec<Vec<usize>> = patches
        .iter()
        .filter_map(|p| match p {
            Patch::Remove { path } => Some(parse_path_like(path)),
            _ => None,
        })
        .collect();
    let add_paths: Vec<Vec<usize>> = patches
        .iter()
        .filter_map(|p| match p {
            Patch::Add { path, .. } => Some(parse_path_like(path)),
            _ => None,
        })
        .collect();
    patches
        .iter()
        .map(|p| match p {
            Patch::Remove { path } => NormFact {
                op: "REM",
                old_path: parse_path_like(path),
                new_path: Vec::new(),
            },
            Patch::Add { path, .. } => NormFact {
                op: "ADD",
                old_path: Vec::new(),
                new_path: parse_path_like(path),
            },
            // 面级粒度(ReplaceAttr)在归一/适配层 ≡ Replace(整块 REPL 语义
            // ——attr 变 = 块内容变,HS 升级规则按 REPL 消费;面信息只服务
            // CF 直接消费端)。
            Patch::Replace { path, .. } | Patch::ReplaceAttr { path, .. } => {
                let new_path = parse_path_like(path);
                let mut old_path = Vec::with_capacity(new_path.len());
                for (l, &pj) in new_path.iter().enumerate() {
                    let a = add_paths
                        .iter()
                        .filter(|ap| ap.len() > l && ap[l] <= pj)
                        .count();
                    let target = pj.saturating_sub(a);
                    let r_at = |x: usize| -> usize {
                        rem_paths
                            .iter()
                            .filter(|rp| rp.len() > l && rp[l] <= x)
                            .count()
                    };
                    let mut x = 0usize;
                    while (x as isize) - (r_at(x) as isize) < target as isize {
                        x += 1;
                    }
                    old_path.push(x);
                }
                NormFact {
                    op: "REPL",
                    old_path,
                    new_path,
                }
            }
        })
        .collect()
}

/// 适配层：CF 事实 → 目标拓扑补丁（incr_unify_826 §2.3）。
/// CF 目标：恒等（CF 无壳——叶/容器路径与值都已是目标形态）。
/// HS 目标：壳升级/域渲染/规范化——本实现第一版支持"根级节"文档
/// （嵌套节/容器内标题 = 阶段 5 扩展；范围见注释）。禁抄清单落实：
/// 身份 = walk（非按区间猜全文档）；域树 = 节区间构建（非全文档树）；
/// 路径 = AST 序换算（现成 wrap/item_inline_roots 逻辑）。
pub(crate) fn adapt(
    task: &IncrTask,
    patches: &[Patch],
    ctx: &ZoneContext,
    target: &crate::topology::Topology,
) -> Vec<Patch> {
    if target.zone == crate::topology::ZoneRule::ContainerClosure {
        return patches.to_vec();
    }
    let heading = target.heading_candidates;
    // A4 节区间归口(#31):secs 由节区间格供给(全量单源——与 selector 格
    // 同键同值)。[P1] 不再预先双侧拉取全量结构段：`shell_path_in_tree`
    // 已不需要全量段参数；真正的全量需求由 `section_cell` 按身份语义自取。
    let n_old = task.snap_old.line_len();
    let n_new = task.snap.line_len();
    let cell_old = task
        .snap_old
        .section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
    let cell_new = task
        .snap
        .section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
    let old_secs: Vec<(usize, usize, u8)> = cell_old
        .secs_iter()
        .map(|(s, e, lv, _t)| (s, e, lv))
        .collect();
    let new_secs: Vec<(usize, usize, u8)> = cell_new
        .secs_iter()
        .map(|(s, e, lv, _t)| (s, e, lv))
        .collect();
    // WC-5：节标题**行文本**（与 secs 同序）——pass 1.5 认领旧节时用标题面消歧：
    // 在节首插入整行会在 LineMap 的 edit_old 处产生歧义（新节与旧节起点同映射），
    // 单靠起点会把"新插入的节"错认成"旧节"（实测 exhaustive line_ins：
    // REM 越界 + ADD 吞并 → 往复失败）。
    let old_texts: Vec<String> = old_secs.iter()
        .map(|&(s, _, _)| task.snap_old.line_text(s).to_string())
        .collect();
    let new_texts: Vec<String> = new_secs.iter()
        .map(|&(s, _, _)| task.snap.line_text(s).to_string())
        .collect();
    // 无节文档：HS 树与 CF 树同构——恒等（军令状 H2：零全文档树）。
    if old_secs.is_empty() && new_secs.is_empty() {
        return patches.to_vec();
    }
    // 827 P0：事实归一化（CF 事实流 → 双侧静态身份——REPL 旧侧路径
    // 层算术反推）。后续步骤的三 pass 按序消费；本步骤先落数据层。
    let norm = normalize_cf_facts(patches);
    let _ = &norm; // 消费见后续修复步骤
    // 827 优化（826 阶段 5）：单次 adapt 调用内的域构建缓存——同快照同域
    // 只 fold 一次（嵌套节父壳域/壳渲染/域内渲染共用）。
    let mut dcache = DomainCache::default();

    // ── 受影响域（军令状 B2/B4：四类传播并集——section_closure_zone 直接调用）。
    // 输入用 CF zone 的最终 cs/ce 作扩展基（compute_zone 内部 stage2 值不外露；
    // 结果域只可能偏大不偏小——渲染域向上完整，正确性不损）。
    let line_map = LineMap::new(task);
    // 827 阶段 1 P3 分项①：域计算（四类传播并集——零开销插桩，
    // feature OFF 展开 ()）。
    let _span_adapt_domain = crate::telemetry_span!("adapt_domain");
    let (_do_s, _do_e, dn_s, dn_e) = section_closure_zone(
        task, &line_map, ctx.zone.cs_o, ctx.zone.ce_o, ctx.zone.cs_n, ctx.zone.ce_n);
    drop(_span_adapt_domain);
    let mut affected_o: Vec<(usize, usize)> = Vec::new();
    let mut affected_n: Vec<(usize, usize)> = Vec::new();
    // WC-5：只做了**壳自身面**字段写（未重发子通道 ct）的节——pass 1.5 不再
    // 整壳覆盖（避免与字段写重复），但 pass 2 **不吞并**（子内容按位置发）。
    let mut attrs_handled_n: Vec<(usize, usize)> = Vec::new();
    let mut shell_ops: Vec<Patch> = Vec::new();
    let mut any_heading = false;
    // 827 Bug E2 前置：壳 REM 覆盖的旧节区间（REPL 复用不可达判定用）。
    let mut shell_removed_o: Vec<(usize, usize)> = Vec::new();
    // [§13-6 修] 预收集：CF **ADD 事实**在新侧覆盖的节点区间。用途——无节侧重挂分支里，
    // 若幸存根的对应新根**已由某条 ADD 事实补出**，不得再重挂补一条（实测 random#93 重复补根）；
    // 只有未被任何 ADD 事实覆盖的幸存根才需要重挂补 ADD（实测 random#36 的 bq）。
    let fact_add_n: Vec<(usize, usize)> = patches
        .iter()
        .filter(|p| matches!(p, Patch::Add { .. }))
        .filter_map(|p| {
            let path = patch_path(p);
            let segs = parse_path_like(path);
            let w = walk_ast_path(&ctx.n_seg, &ctx.snap_new, &ctx.n_tree, &segs, ctx.off_new)?;
            let tidx = match w {
                AdaptWalk::Tree(i) => i,
                AdaptWalk::Slot { node, .. } => node,
            };
            let n = &ctx.n_tree.nodes[tidx];
            Some((n.start as usize, n.end as usize))
        })
        .collect();
    // 827 Bug B1 前置：REM 事实的旧侧节点身份（整块 REM 覆盖判定用——
    // 同快照内 (block_type, start, end) 三元组 = 节点身份；区间恒等不够：
    // bq 内容段可占满整个 bq 区间——须按类型区分）。
    let rem_old_intervals: Vec<(BlockType, usize, usize)> = patches
        .iter()
        .filter_map(|p| match p {
            Patch::Remove { path } => {
                let segs = parse_path_like(path);
                let w = walk_ast_path(&ctx.o_seg, &ctx.snap_old, &ctx.o_tree, &segs, ctx.off_old)?;
                let tidx = match w {
                    AdaptWalk::Tree(i) => i,
                    // 槽位事实 = inline 级 REM，绝不等于整块 REM——不参与
                    // 覆盖判定（否则槽位 REM 会把父块区间记进来误拦 B1）。
                    AdaptWalk::Slot { .. } => return None,
                };
                let nd = &ctx.o_tree.nodes[tidx];
                Some((nd.block_type, nd.start as usize, nd.end as usize))
            }
            _ => None,
        })
        .collect();

    // 827 重写阶段 1:密封域选择的区间输入(两侧各一份——REM 旧侧/其余新侧)。
    // WC-5：上提到 pass 1 之前——pass 1 的"旧壳收缩/扩张"位置 diff 也要用旧侧区间。
    let old_intervals: Vec<(usize, usize)> = old_secs.iter().map(|&(ss, se, _)| (ss, se)).collect();
    let new_intervals: Vec<(usize, usize)> = new_secs.iter().map(|&(ss, se, _)| (ss, se)).collect();

    // ── pass 1：标题事实 → 壳级操作（军令状 §2.3 三规则；无 pass 0 自造规则）──
    for (p, nf) in patches.iter().zip(norm.iter()) {
        let path = patch_path(p);
        let segs = parse_path_like(path);
        let (seg, snap, tree, off) = match p {
            Patch::Remove { .. } => (&ctx.o_seg, &ctx.snap_old, &ctx.o_tree, ctx.off_old),
            _ => (&ctx.n_seg, &ctx.snap_new, &ctx.n_tree, ctx.off_new),
        };
        let w = walk_ast_path(seg, snap, tree, &segs, off);
        let Some(w) = w else { continue };
        let AdaptWalk::Tree(tidx) = w else { continue };
        let nd = &tree.nodes[tidx];
        if !is_heading_bt(nd.block_type) || nd.kind.is_some() || nd.is_item() {
            continue;
        }
        any_heading = true;
        match p {
            Patch::Replace { .. } | Patch::ReplaceAttr { .. } => {
                // 规则 1：标题叶 REPL → 壳 REPL（值 = 壳区间渲染）。
                // ReplaceAttr(面级)在适配层 ≡ Replace——标题面值变按 REPL
                // 升级(壳渲染覆盖),面信息只服务 CF 直接消费端。
                // 变体（G3 用例"改 level"）：level 变化 = 树结构重组（节
                // 根级↔嵌套）——CF facts 表现为 REM+ADD 标题叶，走规则 2/3
                // 的 REM/ADD 分支处理（此处 old_lv==nlv 才走 REPL）。
                let s = nd.start as usize;
                let old_lv = old_secs.iter().find(|&&(ss, _, _)| ss == s).map(|&(_, _, lv)| lv);
                let new_entry = new_secs.iter().find(|&&(ss, _, _)| ss == s).copied();
                let Some((_, e, nlv)) = new_entry else { continue };
                if let Some(olv) = old_lv {
                    if olv != nlv {
                        continue; // level 变 = REM/ADD 形态，由对应分支处理。
                    }
                }
                // Phase C2 分类 2：标题自身字段级变化（ReplaceAttr）且
                // level 未变 -> 译到 sec 壳路径，保留 ReplaceAttr。
                // WC-5：只做字段写——不标 affected_n（子内容按位置发，pass 2 处理），
                // 只记 attrs_handled_n 供 pass 1.5 跳过。原实现标 affected_n 会把
                // 同节的细粒度子事实吞掉——实测 G3 flat/combo proj 丢更新（apply_eq=false）。
                if let Patch::ReplaceAttr { attr, value, .. } = p {
                    // [§13-14 修] 只有**解析出壳路径**才标记"已处理"并消费该事实；
                    // 解不出时不得静默丢弃（原实现无条件 push + continue ⇒ 该节的字段写丢失、
                    // 且 pass 1.5 会因 attrs_handled_n 跳过）——不消费即落到 pass 2（_ 臂）兜底。
                    if let Some(path) = shell_path_in_tree(&ctx.snap_new, &cell_new, &new_secs, target, (s, e), heading, &mut dcache) {
                        shell_ops.push(Patch::ReplaceAttr { path, attr: attr.clone(), value: value.clone() });
                        attrs_handled_n.push((s, e));
                        continue;
                    }
                }
                // WC-5：块粒度标题 REPL 同样只发**自身面**字段写（lv/t）——新侧值 =
                // 事实值（CF 节点渲染），旧侧值按归一化旧路径渲染；壳子通道 ct 不
                // 重发，子事实由 pass 2 按位置处理（同 ReplaceAttr 分支语义）。
                {
                    let Patch::Replace { value, .. } = p else { continue };
                    let old_cf = if nf.old_path.is_empty() {
                        None
                    } else {
                        match walk_ast_path(&ctx.o_seg, &ctx.snap_old, &ctx.o_tree, &nf.old_path, ctx.off_old) {
                            Some(AdaptWalk::Tree(i)) => render_cf_node_value(&ctx.snap_old, &ctx.o_seg, &ctx.o_tree, i),
                            _ => None,
                        }
                    };
                    let shell = shell_path_in_tree(&ctx.snap_new, &cell_new, &new_secs, target, (s, e), heading, &mut dcache);
                    let ws = match (&shell, old_cf.as_deref()) {
                        (Some(sh), Some(o)) => shell_attr_writes(sh, o, value),
                        _ => None,
                    };
                    if let (Some(_), Some(ws)) = (&shell, ws) {
                        shell_ops.extend(ws);
                        attrs_handled_n.push((s, e));
                        continue;
                    }
                }
                // 兜底：旧侧不可定位 / 未知字段差 → 整壳 REPL（原行为）。
                if let Some(v) = render_shell_in_domain(&ctx.snap_new, (s, e), target, (s, e), &mut dcache) {
                    if let Some(p) = shell_path_in_tree(&ctx.snap_new, &cell_new, &new_secs, target, (s, e), heading, &mut dcache) {
                        shell_ops.push(Patch::Replace { path: p, value: v });
                    }
                }
                affected_n.push((s, e));
            }
            Patch::Remove { .. } => {
                // 规则 2：标题叶 REM → 壳 REM + 幸存子节点重挂。
                // 827 阶段 0 修复（B3 判官实证——setext/combo 双写炸点）：
                //   有节侧（new_secs 非空）不重挂——幸存内容归相邻节、由
                //   传播域整壳 REPL（pass 1.5）覆盖；壳 REM + affected_o 照旧。
                //   无节侧（new_secs 空——HS 与 CF 同构）保留逐节点重挂。
                // 重挂 = LineMap 映射先行 → ctx 树定位 + 渲染（零新树）——
                // 军令状 H1：映射先行，域内同快照导航。
                // level 变（REM+ADD 同 s 行）：标题行仍在——内容随新壳 ADD
                // （规则 3）整体迁移，不重挂。
                let s = nd.start as usize;
                if let Some(&(_, e, _)) = old_secs.iter().find(|&&(ss, _, _)| ss == s) {
                    if let Some(po) = shell_path_in_tree(&ctx.snap_old, &cell_old, &old_secs, target, (s, e), heading, &mut dcache) {
                        shell_ops.push(Patch::Remove { path: po });
                    }
                    affected_o.push((s, e));
                    // 827 Bug E2 前置：壳 REM 覆盖的旧节区间（REPL 复用
                    // 不可达判定用）。
                    shell_removed_o.push((s, e));
                    if new_secs.iter().any(|&(ss, _, _)| ss == line_map.to_new(s)) {
                        continue; // level 变——内容随新壳，不重挂。
                    }
                    // [orphan-fix] 827 阶段 0 的「有节侧不重挂」只对**落进新节**的
                    // 幸存子节点成立（传播域整壳 REPL 覆盖）；落在新侧任何节之外的
                    // 幸存子节点必须重挂为节外空隙根，否则 P 帧结果 ≠ F 帧——
                    // 实测：删掉节标题行后正文变空隙根，只发壳 REM ⇒ 客户端镜像少
                    // 一个根，下一事务按服务端旧侧算出的 op 全部落到错的索引上。
                    // 逐节点判据在下面的 child 循环里（in_new_section）。
                    let Some(part_o) = domain_parts(&ctx.snap_old, (s, e), target, &mut dcache)
                        else { continue };
                    let (_seg_o, tree_o) = (&part_o.0, &part_o.1);
                    // 827 重写阶段 2:重挂源 = sec 节点自身(不是域根——容器
                    // 内节的密封域根 = 容器,其 children 含节的兄弟,重挂会
                    // 误带兄弟内容)。
                    let Some(shell_idx) = locate_shell_in_domain(&tree_o, s as u32, e as u32)
                        else { continue };
                    for &c in tree_o.nodes[shell_idx].children_of(&tree_o) {
                        let cn = &tree_o.nodes[c as usize];
                        let start_n = line_map.to_new(cn.start as usize);
                        let end_n = line_map.to_new(cn.end as usize);
                        let bts = cn.block_type;
                        // [orphan-fix] 有节侧：落进新节的幸存子节点交给传播域
                        // （pass 1.5/2）；节外空隙根在这里按 HS 根路径重挂——
                        // 路径 = 空隙起点前的 HS 根计数 + 域内根序（与规则 3 的
                        // B1 补 REM 同一公式），值 = 该根在新侧域树上的渲染。
                        if !new_secs.is_empty() {
                            let n_new = task.snap.line_len();
                            let in_new_section = new_secs.iter()
                                .any(|&(ss, se, _)| ss <= start_n && start_n <= se);
                            if in_new_section { continue; }
                            let Some((ga, gb)) = gap_intervals(&new_secs, n_new)
                                .into_iter().find(|&(a, b)| a <= start_n && start_n < b)
                                else { continue };
                            let Some(part_g) = domain_parts(&ctx.snap_new, (ga, gb), target, &mut dcache)
                                else { continue };
                            let (seg_g, tree_g) = (&part_g.0, &part_g.1);
                            let Some(k) = tree_g.roots.iter().position(|&r| {
                                let rn = &tree_g.nodes[r as usize];
                                (rn.start as usize) <= start_n && end_n <= (rn.end as usize)
                            }) else { continue };
                            // [§13-14 修] 偏移用**域段起点**（domain_parts 密封后的 seg_g.start_line），
                            // 不能用于未密封的 gap 起点 ga：域被 sealed_interval_of 向左扩时
                            // [ds, ga) 内的 HS 根会被多计 ⇒ 路径整体偏大。与 pass 2 的
                            // hs_off_before(cell, seg2.start_line)（:4965）同律。
                            let off_g = hs_off_before(&cell_new, seg_g.start_line);
                            let root_i = tree_g.roots[k] as usize;
                            let Some((pn, v)) = emit_cf_side(&ctx.snap_new, seg_g, tree_g,
                                off_g, root_i, &[], target) else { continue };
                            shell_ops.push(Patch::Add { path: pn, value: v });
                            affected_n.push((start_n, end_n));
                            continue;
                        }
                        // 无节侧重挂（new_secs 空——HS 与 CF 同构）：ctx 树定位 + 渲染（零新树）。
                        // 两版容错定位实验均**净负**：
                        // ① 整体替换精确匹配 → 引入 random#52/#93 各 2 处；
                        // ② 仅精确失败时 fallback → 同样引入 random#52/#93 各 2 处。
                        // ⇒ 判别不在"精确是否失败"，而在"**幸存根是否已被壳操作覆盖**"：
                        //   #52/#93 已被覆盖（多补一条 ADD 即重复），#36 未覆盖（缺 ADD）。
                        // 正确形态（覆盖性判别）需更多信息；此处保持精确匹配（4 违规基线）。
                        // [§13-6 修] 定位：**精确匹配为主**；失败时启用容错 fallback，但
                        // **只接受类型匹配**的候选（block_type + kind + is_item）——
                        // 不匹配 ⇒ 该幸存根在新侧**没有对应节点**（内容已被 REM 事实删除，实测
                        // random#52/#93）⇒ 不补 ADD；类型匹配 ⇒ 真幸存（实测 random#36）⇒ 补 ADD。
                        // 候选评分 = **跨度最小**（最具体）→ 重叠最大（原"重叠最大优先"会选中
                        // 外层更大节点：#52 span 0..2、#93 span 0..4 ⇒ 重复补根）。
                        let idx2 = match locate_in_domain(&ctx.n_tree,
                            start_n as u32, end_n as u32, bts, cn.kind.is_some(), cn.is_item()) {
                            Some(i) => i,
                            None => {
                                let mut best: Option<(usize, usize, usize)> = None; // (idx, span, overlap)
                                let mut stack: Vec<usize> =
                                    ctx.n_tree.roots.iter().map(|&r| r as usize).collect();
                                while let Some(ni) = stack.pop() {
                                    let n = &ctx.n_tree.nodes[ni];
                                    let (a, b) = (n.start as usize, n.end as usize);
                                    let ov = b.min(end_n).saturating_sub(a.max(start_n));
                                    if ov == 0 { continue; }
                                    let type_ok = n.block_type == bts
                                        && n.kind.is_some() == cn.kind.is_some()
                                        && n.is_item() == cn.is_item();
                                    if !type_ok { continue; }
                                    let span = b - a;
                                    let better = match best {
                                        None => true,
                                        Some((_, s0, o0)) =>
                                            (usize::MAX - span, ov) > (usize::MAX - s0, o0),
                                    };
                                    if better { best = Some((ni, span, ov)); }
                                    for &c in n.children_of(&ctx.n_tree) { stack.push(c as usize); }
                                }
                                match best { Some((ni, _, _)) => ni, None => continue }
                            }
                        };
                        // [§13-6 修] 该新根若已被某条 ADD 事实覆盖 ⇒ 不再重挂（否则重复补根：random#93）。
                        let (cs_, ce_) = (ctx.n_tree.nodes[idx2].start as usize,
                                          ctx.n_tree.nodes[idx2].end as usize);
                        if fact_add_n.iter().any(|&(a, b)| cs_ < b && a < ce_) { continue; }
                        let Some((pn, v)) = emit_cf_side(&ctx.snap_new, &ctx.n_seg,
                            &ctx.n_tree, ctx.off_new, idx2, &[], target) else { continue };
                        shell_ops.push(Patch::Add { path: pn, value: v });
                        // 827 Bug A：重挂壳值已含子内容——区间投 affected_n，pass2 吞并判定
                        // 跳过其内部 CF 事实（否则 pass2 无节侧直推造成子内容双写）。
                        // [§13-6 修] 投**实际定位节点**区间：fallback 下映射区间可能错位，
                        // 用它会连带吞掉同批其它根的 ADD（random#36：映射 [0,3) 吞掉 p("#>")）。
                        let (ns_, ne_) = (ctx.n_tree.nodes[idx2].start as usize,
                                          ctx.n_tree.nodes[idx2].end as usize);
                        affected_n.push((ns_, ne_));
                    }
                }
            }
            Patch::Add { .. } => {
                // 规则 3：标题叶 ADD → 新壳 ADD（父链路径）+ 旧壳（收缩）REPL。
                let s = nd.start as usize;
                if let Some(&(_, e, _)) = new_secs.iter().find(|&&(ss, _, _)| ss == s) {
                    if let Some(p) = shell_path_in_tree(&ctx.snap_new, &cell_new, &new_secs, target, (s, e), heading, &mut dcache) {
                        if let Some(v) = render_shell_in_domain(&ctx.snap_new, (s, e), target, (s, e), &mut dcache) {
                            shell_ops.push(Patch::Add { path: p, value: v });
                        }
                    }
                    affected_n.push((s, e));
                    // 827 Bug B1：新壳 ADD 值已含吞并内容——旧侧根级块
                    // （不在旧节内）凡 LineMap 映射后区间 ⊆ 新节、且未被
                    // 整块 REM 事实覆盖者 → 补 REM（旧侧根路径）；区间投
                    // affected_o（其内部 CF 事实随吞并跳过）。根路径 =
                    // hs_off_before(空隙起点) + 空隙域根序号（域起点的
                    // off，非节点行的 off——多根空隙不双计）。
                    {
                        let mut gaps: Vec<(usize, usize)> = Vec::new();
                        let mut prev_end = 0usize;
                        for &(ss, se, _) in &old_secs {
                            if prev_end < ss {
                                gaps.push((prev_end, ss));
                            }
                            prev_end = prev_end.max(se);
                        }
                        let n_old = task.snap_old.line_len();
                        if prev_end < n_old {
                            gaps.push((prev_end, n_old));
                        }
                        for (a, b) in gaps {
                            let Some(part_g) = domain_parts(&task.snap_old, (a, b), target, &mut dcache)
                                else { continue };
                            let (seg_g, tree_g) = (&part_g.0, &part_g.1);
                            // [§13-14 同族第三处] 根序号口径 = **域段起点**（domain_parts 可能向左
                            // 密封扩展 ⇒ seg_g.start_line < gap 起点 a），用 a 会把 [seg.start, a)
                            // 内的 HS 根漏计、路径偏大（实测 random#3/Hs：gap(4,5) 算得 idx=5 而
                            // JSON 只有 4 个根 ⇒ Remove /ct/5 不存在）。与 pass 1.5 的
                            // hs_off_before(cell, seg_g.start_line) 同律。
                            let off_g = hs_off_before(&cell_old, seg_g.start_line);
                            for (k, &r) in tree_g.roots.iter().enumerate() {
                                let cn = &tree_g.nodes[r as usize];
                                let (cs_, ce_) = (cn.start as usize, cn.end as usize);
                                let start_n = line_map.to_new(cs_);
                                let end_n = line_map.to_new(ce_);
                                if s <= start_n
                                    && end_n <= e
                                    && !rem_old_intervals.iter().any(|&(bt, rs, re)| {
                                        bt == cn.block_type && rs == cs_ && re == ce_
                                    })
                                {
                                    let idx = off_g + k;
                                    shell_ops.push(Patch::Remove { path: format!("/ct/{idx}") });
                                    affected_o.push((cs_, ce_));
                                }
                            }
                        }
                    }
                    // WC-5：旧壳收缩/扩张不再整壳重发——按位置发**子通道**增删
                    // （旧节 vs 新节的孩子数组前缀/后缀 diff）。跨快照行号用
                    // LineMap 映射（hins 插入后旧侧标题行 ≠ 新侧行号）。与 pass 1.5
                    // 的同类 diff 幂等（同 path 同 kind 去重）。
                    if let Some(&(os, oe, _)) = old_secs.iter().find(|&&(ss, se, _)| ss < s && s <= se) {
                        let osn = line_map.to_new(os);
                        if os != s && new_secs.iter().any(|&(ss, _, _)| ss == osn) {
                            if let Some(&(_, ne, _)) = new_secs.iter().find(|&&(ss, _, _)| ss == osn) {
                                let sealed = |snap: &Snapshot, ivs: &[(usize, usize)], line: usize| {
                                    crate::infer::sealed_interval_of(
                                        &snap.frame_log.entries, &snap.lines, &snap.open_depth,
                                        snap.line_len(), ivs, line)
                                };
                                let dso = sealed(&ctx.snap_old, &old_intervals, os);
                                let dsn = sealed(&ctx.snap_new, &new_intervals, osn);
                                let sh = shell_path_in_tree(&ctx.snap_new, &cell_new, &new_secs, target, (osn, ne), heading, &mut dcache);
                                let mut tmp: Vec<Patch> = Vec::new();
                                let pos = match &sh {
                                    Some(sh) => shell_children_position_ops(
                                        &ctx.snap_old, dso, (os, oe),
                                        &ctx.snap_new, dsn, (osn, ne),
                                        target, &mut dcache, sh, &line_map, &shell_ops, &mut tmp),
                                    None => None,
                                };
                                if let Some(complete) = pos {
                                    shell_ops.extend(tmp);
                                    // 完整才标 affected（吞并列事实）；不完整时内容变化
                                    // 留给 pass 2 的细粒度事实（否则被吞——实测 flat/cross）。
                                    // 完整同时标旧侧（stale REM 也要吞——见 pass 1.5 注释）。
                                    if complete {
                                        affected_n.push((osn, ne));
                                        affected_o.push((os, oe));
                                    }
                                } else if let Some(p2) = shell_path_in_tree(&ctx.snap_new, &cell_new, &new_secs, target, (osn, ne), heading, &mut dcache) {
                                    // 兜底：旧节不可定位/渲染失败 → 整壳 REPL（原行为）。
                                    if let Some(v) = render_shell_in_domain(&ctx.snap_new, (osn, ne), target, (osn, ne), &mut dcache) {
                                        shell_ops.push(Patch::Replace { path: p2, value: v });
                                        affected_n.push((osn, ne));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── pass 1.5：标题变化时，传播域（四类传播并集——section_closure_zone
    // 输出）内的节——WC-5：不再整壳 REPL，改发**子通道位置 diff**（军令状
    // B4 的"前驱/链节边界变化" = 孩子集合变化，前缀/后缀 diff 精确表达；
    // 同节壳自身面未变——标题变化的节已在 attrs_handled_n）。子事实仍由本
    // diff 吞并（affected_n）——这不是整壳重发，是精确位置集。
    // 无标题变化时传播域 = ripple 区间，不启用（内容编辑保持叶级）。
    if any_heading {
        for &(s, e, _) in &new_secs {
            if s < dn_e && dn_s < e
                && !affected_n.iter().any(|&(as_, ae)| as_ == s && ae == e)
                && !attrs_handled_n.iter().any(|&(as_, ae)| as_ == s && ae == e)
            {
                // 827 重写阶段 2:容器内节(树父 = 容器)不做壳操作——
                // 壳值不含容器父(REPL 后容器丢失),且会吞掉容器壳 ADD;
                // 其增删改由事实级壳操作覆盖(REM/ADD/REPL 节路径)。
                let (ds2, de2) = crate::infer::sealed_interval_of(
                    &ctx.snap_new.frame_log.entries, &ctx.snap_new.lines, &ctx.snap_new.open_depth,
                    ctx.snap_new.line_len(), &new_intervals, s);
                {
                    if let Some(part) = domain_parts(&ctx.snap_new, (ds2, de2), target, &mut dcache) {
                        if let Some(idx) = locate_shell_in_domain(&part.1, s as u32, e as u32) {
                            if matches!(crate::infer::parent_kind(&part.1, idx), Some(Some(_))) {
                                continue;
                            }
                        }
                    }
                }
                // 旧节认领（WC-5）：起点映射 + **标题面文本**一致优先；起点映射在
                // 插入点歧义（新节与旧节同映射）时退回"标题面 + 级别 + 区间映射重叠"。
                let old_sec = new_secs.iter().position(|&(ss, _, _)| ss == s).and_then(|k| {
                    let (nt, nlv) = (&new_texts[k], new_secs[k].2);
                    let by_start = old_secs.iter().enumerate()
                        .find(|(j, &(os, _, _))| {
                            line_map.to_new(os) == s && old_texts[*j].trim() == nt.trim()
                        })
                        .map(|(j, _)| j);
                    let by_face = old_secs.iter().enumerate()
                        .find(|(j, &(os, oe, olv))| {
                            old_texts[*j].trim() == nt.trim() && olv == nlv
                                && line_map.to_new(os) < e && s < line_map.to_new(oe)
                        })
                        .map(|(j, _)| j);
                    by_start.or(by_face).map(|j| old_secs[j])
                });
                let sh = shell_path_in_tree(&ctx.snap_new, &cell_new, &new_secs, target, (s, e), heading, &mut dcache);
                let mut tmp: Vec<Patch> = Vec::new();
                let pos: Option<bool> = match (old_sec, &sh) {
                    (Some((os, oe, _)), Some(sh)) => {
                        let dso = crate::infer::sealed_interval_of(
                            &ctx.snap_old.frame_log.entries, &ctx.snap_old.lines, &ctx.snap_old.open_depth,
                            ctx.snap_old.line_len(), &old_intervals, os);
                        shell_children_position_ops(
                            &ctx.snap_old, dso, (os, oe),
                            &ctx.snap_new, (ds2, de2), (s, e),
                            target, &mut dcache, sh, &line_map, &shell_ops, &mut tmp)
                    }
                    // 新节（旧侧无对应）：子树随规则 3 的新壳 ADD 携带——零 op。
                    (None, _) => Some(true),
                    _ => None,
                };
                if let Some(complete) = pos {
                    shell_ops.extend(tmp);
                    // 不完整（配对上的孩子内容变了）不标 affected——内容变化留给
                    // pass 2 的细粒度事实，否则被吞（实测 flat/cross）。完整则**两侧**
                    // 都标：旧侧 REM 事实也要吞（否则 stale REM 没了整壳 REPL 兜底——
                    // 实测 line_ins：新壳 ADD 直接位移，stale REM 打错位置）。
                    if complete {
                        affected_n.push((s, e));
                        if let Some((os, oe, _)) = old_sec { affected_o.push((os, oe)); }
                    }
                    continue;
                }
                // 兜底：旧节不可定位/渲染失败 → 整壳 REPL（原行为）。
                if let Some(v) = render_shell_in_domain(&ctx.snap_new, (s, e), target, (s, e), &mut dcache) {
                    if let Some(p) = shell_path_in_tree(&ctx.snap_new, &cell_new, &new_secs, target, (s, e), heading, &mut dcache) {
                        shell_ops.push(Patch::Replace { path: p, value: v });
                    }
                }
                affected_n.push((s, e));
            }
        }
    }

    // ── pass 2：普通事实——受影响区间内吞并；区间外重定位 + 值统一域渲染
    // （军令状 D1；槽位值 = inline 内容渲染，拓扑无关——沿用 CF 值，明示）。──
    let mut out: Vec<Patch> = shell_ops;
    // 827 Bug C：已升级为整壳 REPL 的新节区间（后续同节 ADD 随壳值携带）。
    let mut upgraded_n: Vec<(usize, usize)> = Vec::new();
    // 827 Bug E2：已升级为域根 ADD 的新侧根块区间（同根多 REPL 去重）。
    let mut upgraded_add_n: Vec<(usize, usize)> = Vec::new();
    for (p, nf) in patches.iter().zip(norm.iter()) {
        let path = patch_path(p);
        let segs = parse_path_like(path);
        let (seg, snap, tree, off, affected, secs, intervals, cell) = match p {
            Patch::Remove { .. } => (&ctx.o_seg, &ctx.snap_old, &ctx.o_tree, ctx.off_old,
                &affected_o, &old_secs, &old_intervals, &cell_old),
            _ => (&ctx.n_seg, &ctx.snap_new, &ctx.n_tree, ctx.off_new,
                &affected_n, &new_secs, &new_intervals, &cell_new),
        };
        // 无节侧：HS 与 CF 同构——路径/值原样；无壳操作，不存在吞并。
        // 827 Bug A 例外：重挂壳值已含子内容（rule 2 无节侧重挂投了
        // affected_n）——其内部的 ADD 事实必须吞并（否则双写）。
        // 827 Bug E2 例外：REPL 且旧侧有节——壳 REM 可能发生（复用
        // 不可达判定 + 域根升级需要主路径的吞并/E2 逻辑）。
        if secs.is_empty() {
            if matches!(p, Patch::Add { .. }) {
                if let Some(w) = walk_ast_path(&ctx.n_seg, &ctx.snap_new, &ctx.n_tree, &segs, ctx.off_new) {
                    let (tidx2, _) = match w {
                        AdaptWalk::Tree(i) => (i, ()),
                        AdaptWalk::Slot { node, .. } => (node, ()),
                    };
                    let nd2 = &ctx.n_tree.nodes[tidx2];
                    let (s2, e2) = (nd2.start as usize, nd2.end as usize);
                    if s2 < e2 && affected_n.iter().any(|&(as_, ae)| s2 < ae && as_ < e2) {
                        continue; // 重挂壳已携带此内容。
                    }
                    // 827 Bug E2b：ADD 复用不可达——新节点域根块（无节侧
                    // = 全文档空隙域）的旧侧映射区间与壳 REM 的旧节相交
                    // （CF 复用源随壳整体移除）→ 升级为域根整块 ADD（值 =
                    // 域根渲染），本条 ADD 吞并。同根多 ADD 经 upgraded_add_n
                    // 去重。
                    if !shell_removed_o.is_empty() {
                        let d2 = (0usize, snap.line_len());
                        if let Some(part_e) = domain_parts(snap, d2, target, &mut dcache) {
                            let tree_e = &part_e.1;
                            let root_hit = tree_e.roots.iter().enumerate().find(|&(_, &r)| {
                                let rn = &tree_e.nodes[r as usize];
                                (rn.start as usize) <= s2 && e2 <= (rn.end as usize)
                            });
                            if let Some((k2, &r2)) = root_hit {
                                let rn = &tree_e.nodes[r2 as usize];
                                let (rs_, re_) = (rn.start as usize, rn.end as usize);
                                let os_ = line_map.to_old(rs_);
                                let oe_ = line_map.to_old(re_);
                                // 复用源不可达判定(827 重写阶段 2,根级):
                                // 新侧根在旧侧映射区间的"重叠旧根"——
                                // ① 无重叠旧根(根整体迁移/合并)→ 不可达;
                                // ② 重叠旧根是已被壳 REM 的节 → 不可达;
                                // ③ 重叠旧根是幸存容器/块(仅子内容变化)
                                //    → 可达(事实级 op 打进幸存容器)。
                                // 区间包含/树祖先链在此都不可靠:节在容器内
                                // 时两者同区间,只有"根本身是否被删"能区分。
                                let mut unreachable = false;
                                if let Some(part_o2) = domain_parts(
                                    &ctx.snap_old, (0usize, ctx.snap_old.line_len()),
                                    target, &mut dcache,
                                ) {
                                    let tree_o2 = &part_o2.1;
                                    let overlap = tree_o2.roots.iter().find(|&&r| {
                                        let rn = &tree_o2.nodes[r as usize];
                                        (rn.start as usize) < oe_ && os_ < (rn.end as usize)
                                    });
                                    // [§13-6 修] 升级（域根整块 ADD）只在"旧根**确实被移除**"时成立：
                                    // ① 旧根必须**完整落在**映射区间内（部分重叠 ⇒ 是另一个幸存根）；
                                    // ② 旧根是节壳（heading 块）且其区间**被某个已 REM 的节区间包含**
                                    //    （原判据用"区间相等"，节有正文时恒不成立；且相等无法区分
                                    //     "节被删" 与 "节内子节点被删"）。
                                    // 反例（random#30/#36）：HS 下容器是**节外空隙根**、
                                    // 只有其子节点被 REM，旧根仍存活 ⇒ 误升级把容器根重插一份。
                                    match overlap {
                                        None => unreachable = true,
                                        Some(&r) => {
                                            let rn = &tree_o2.nodes[r as usize];
                                            let (rs3, re3) = (rn.start as usize, rn.end as usize);
                                            let fully = rs3 >= os_ && re3 <= oe_;
                                            let is_sec = rn.kind.is_none()
                                                && rn.blk_idx().is_none()
                                                && heading.contains(&rn.block_type);
                                            let removed = shell_removed_o
                                                .iter()
                                                .any(|&(ss, se)| ss <= rs3 && re3 <= se);
                                            unreachable = fully && is_sec && removed;
                                        }
                                    }
                                } else {
                                    unreachable = true;
                                }
                                if unreachable {
                                    if !upgraded_add_n.iter().any(|&(us, ue)| us == rs_ && ue == re_) {
                                        if let Some(v2) = render_in_domain(
                                            snap, d2, target, (rs_, re_),
                                            rn.block_type, rn.kind, rn.is_item(), &mut dcache,
                                        ) {
                                            out.push(Patch::Add {
                                                path: format!("/ct/{k2}"),
                                                value: v2,
                                            });
                                        }
                                        upgraded_add_n.push((rs_, re_));
                                    }
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
            if matches!(p, Patch::Replace { .. }) && !old_secs.is_empty() {
                // 落主路径（吞并判定 + E2 复用不可达升级）。
            } else {
                out.push(p.clone());
                continue;
            }
        }
        let Some(w) = walk_ast_path(seg, snap, tree, &segs, off) else { continue };
        let (tidx, tail): (usize, Vec<usize>) = match w {
            AdaptWalk::Tree(i) => (i, Vec::new()),
            AdaptWalk::Slot { node, tail } => (node, tail),
        };
        let nd = &tree.nodes[tidx];
        if is_heading_bt(nd.block_type) && nd.kind.is_none() && !nd.is_item() {
            continue; // 标题事实已由 pass 1 处理。
        }
        let (s, e) = (nd.start as usize, nd.end as usize);
        // 吞并判定（REM 旧侧映射新侧再查——段落“搬家”）：历史意图的映射
        // 若要接上须单独立项，此处不留死计算。
        // 827 阶段二修正:区间重叠只是必要条件——壳渲染只携带节**子树**
        // 内容;item 内节的区间含兄弟块(节子树仅标题),这些兄弟必须走
        // pass2。判据:存在一个重叠的受影响节,其子树含本节点区间。
        let mut carried = |snap: &Snapshot, intervals: &[(usize, usize)],
                       s_: usize, e_: usize| -> bool {
            for &(as_, ae_) in intervals {
                if !(s_ < ae_ && as_ < e_) {
                    continue;
                }
                if let Some(part) = domain_parts(snap, (as_, ae_), target, &mut dcache) {
                    let tree = &part.1;
                    if let Some(si) = locate_shell_in_domain(tree, as_ as u32, ae_ as u32) {
                        // 子树含 (s_, e_) 的节点 → 壳值已携带。
                        let mut stack = vec![si];
                        while let Some(ni) = stack.pop() {
                            let n = &tree.nodes[ni];
                            if n.start as usize == s_ && n.end as usize == e_ {
                                return true;
                            }
                            for &c in n.children_of(tree) {
                                stack.push(c as usize);
                            }
                        }
                    }
                }
            }
            false
        };
        let carried_old = matches!(p, Patch::Remove { .. })
            && carried(&ctx.snap_old, &affected_o, s, e);
        let carried_new = !matches!(p, Patch::Remove { .. })
            && carried(&ctx.snap_new, &affected_n, s, e);
        if carried_old || carried_new {
            continue; // 壳级吞并(壳值已携带)。
        }
        // 目标域：密封区间选择(827 重写阶段 1——sealed_interval_of 取代
        // shell_at/outer_domain;域树自带容器父链与嵌套节,路径一次给全)。
        let (ds2, de2) = crate::infer::sealed_interval_of(
            &snap.frame_log.entries, &snap.lines, &snap.open_depth, snap.line_len(), intervals, s);
        let Some(part2) = domain_parts(snap, (ds2, de2), target, &mut dcache) else { continue };
        let (seg2, tree2) = (&part2.0, &part2.1);
        let mut idx2 = match locate_in_domain(&tree2, s as u32, e as u32,
            nd.block_type, nd.kind.is_some(), nd.is_item()) {
            Some(i) => i,
            None => continue,
        };
        // [§13-6 修] 窄形态最深匹配：嵌套容器可共享同一区间（random#30：内外 blockquote 因惰性
        // 续行同为 [0,4)）⇒ 精确匹配落到**域根**、路径塌成 /ct/0。仅当"命中域根 **且** 事实路径
        // 含嵌套分量（≥2 段）"时改取最深精确匹配（全局取最深实测伤 6 例）。
        let mut used_deep = false;
        if segs.len() >= 2 && tree2.roots.iter().any(|&r| r as usize == idx2) {
            if let Some(deep) = locate_in_domain_deepest(&tree2, s as u32, e as u32,
                nd.block_type, nd.kind.is_some(), nd.is_item()) {
                if deep != idx2 { used_deep = true; }
                idx2 = deep;
            }
        }

        let off2 = hs_off_before(cell, seg2.start_line);
        let Some(pn) = path_in_domain(snap, seg2, tree2, idx2, off2, &tail) else { continue };
        match p {
            Patch::Add { .. } => {
                // WC-2（丙）：节内 ADD 到"旧侧无子节的节"——旧壳**没有** ct 数组
                // （渲染省略空数组），逐元素 ADD /ct/X/ct/0 不可寻址。原实现升级为
                // 整壳 REPL；这里发**一条**建数组补丁：
                //   Add <壳路径>/ct = [<新孩子…>]   （对象成员 set，值 = 整数组裸 JSON）
                // 同节后续 ADD 事实随该数组携带 → 跳过（upgraded_n 去重语义不变）。
                if let Some((s_new, e_new)) = crate::infer::innermost_interval(intervals, s) {
                    let old_sec = old_secs
                        .iter()
                        .find(|&&(os, _, _)| line_map.to_new(os) == s_new);
                    if let Some(&(os, oe, _)) = old_sec {
                        if let Some(part_o) =
                            domain_parts(&task.snap_old, (os, oe), target, &mut dcache)
                        {
                            let tree_o = &part_o.1;
                            let childless = tree_o.roots.len() == 1
                                && tree_o.nodes[tree_o.roots[0] as usize].children_of(tree_o).is_empty();
                            if childless {
                                if !upgraded_n.iter().any(|&(us, ue)| us == s_new && ue == e_new) {
                                    if let (Some(prefix), Some(v)) = (
                                        shell_path_in_tree(snap, cell, secs, target, (s_new, e_new), heading, &mut dcache),
                                        render_children_array_in_domain(snap, (ds2, de2), target, (s_new, e_new), &mut dcache),
                                    ) {
                                        out.push(Patch::Add {
                                            path: format!("{prefix}/ct"),
                                            value: v,
                                        });
                                    }
                                    upgraded_n.push((s_new, e_new));
                                }
                                continue;
                            }
                        }
                    }
                }
                // 值 = 节点域渲染（D1 统一入口）；槽位（tail 非空）值 = inline
                // 内容渲染，拓扑无关（不查壳表）——与 CF 值等价，明示沿用。
                let v = if tail.is_empty() {
                    // [§13-6 修] 窄-最深生效时，值必须按**同一节点下标**渲染：
                    // 区间渲染在同签名多命中（嵌套 bq 共享区间）时会取到最外层 ⇒ 路径用内层、
                    // 值用外层 ⇒ 多一层嵌套（random#30 实测）。其余情形保持原区间渲染路径不变。
                    let v_deep = if used_deep {
                        emit_cf_side(snap, seg2, tree2, off2, idx2, &[], target).map(|(_, v)| v)
                    } else { None };
                    v_deep.unwrap_or_else(||
                        render_in_domain(snap, (ds2, de2), target, (s, e),
                            nd.block_type, nd.kind, nd.is_item(), &mut dcache)
                            .unwrap_or_else(|| match p { Patch::Add { value, .. } => value.clone(), _ => unreachable!() }))
                } else {
                    match p { Patch::Add { value, .. } => value.clone(), _ => unreachable!() }
                };
                out.push(Patch::Add { path: pn, value: v });
            }
            Patch::Remove { .. } => {
                // 827 Bug F：节内最后一个子节点 REM 会残留空 ch 数组
                // （渲染规范省略空 ch——826 §2.3 规范化 REM 侧，与 Bug C
                // 的 ADD 侧同族）。升级为整壳 REPL（新壳渲染）；同节多
                // REM 经 upgraded_n 去重。
                if tail.is_empty() {
                    if let Some((os2, oe2)) = crate::infer::innermost_interval(intervals, s) {
                        if let Some(part_o2) =
                            domain_parts(&task.snap_old, (os2, oe2), target, &mut dcache)
                        {
                            let tree_o2 = &part_o2.1;
                            let is_only_child = tree_o2.roots.len() == 1 && {
                                let root = tree_o2.roots[0];
                                let ch = tree_o2.nodes[root as usize].children_of(tree_o2);
                                ch.len() == 1 && {
                                    let c0 = &tree_o2.nodes[ch[0] as usize];
                                    c0.start as usize == s && c0.end as usize == e
                                }
                            };
                            if is_only_child {
                                let s_new = line_map.to_new(os2);
                                if let Some(&(_, ne2, _)) =
                                    new_secs.iter().find(|&&(ss, _, _)| ss == s_new)
                                {
                                    // WC-5：发**子通道位置 diff**（旧节 vs 新节）——"唯一
                                    // 子节点被删"只是"壳孩子集合变化"的特例：原位后仍可能
                                    // 有新孩子（实测 bq 插入：旧 bq 被 [bl, bq] 取代——
                                    // `remove <壳>/ct` 清空数组后，后续 `ADD .../ct/1/...`
                                    // 越界 APPLY_FAIL）。diff 完整时标 affected 吞并列事实。
                                    let sh = shell_path_in_tree(
                                        &ctx.snap_new, &cell_new, &new_secs, target,
                                        (s_new, ne2), heading, &mut dcache,
                                    );
                                    let mut tmp: Vec<Patch> = Vec::new();
                                    let pos: Option<bool> = match &sh {
                                        Some(sh) => {
                                            let dso = crate::infer::sealed_interval_of(
                                                &ctx.snap_old.frame_log.entries, &ctx.snap_old.lines,
                                                &ctx.snap_old.open_depth, ctx.snap_old.line_len(),
                                                &old_intervals, os2);
                                            let dsn = crate::infer::sealed_interval_of(
                                                &ctx.snap_new.frame_log.entries, &ctx.snap_new.lines,
                                                &ctx.snap_new.open_depth, ctx.snap_new.line_len(),
                                                &new_intervals, s_new);
                                            shell_children_position_ops(
                                                &ctx.snap_old, dso, (os2, oe2),
                                                &ctx.snap_new, dsn, (s_new, ne2),
                                                target, &mut dcache, sh, &line_map, &out, &mut tmp)
                                        }
                                        None => None,
                                    };
                                    // 只有**完整** diff 才敢不发整壳 REPL：不完整意味着配对上的
                                    // 孩子内容变了，而这类事实（CF item 层节点在 HS 树里
                                    // locate_in_domain 可能落空——实测 bq DeleteStart）pass 2
                                    // 未必兜得住 → 退回整壳 REPL（旧行为，值随附子树）。
                                    if let Some(true) = pos {
                                        out.extend(tmp);
                                        upgraded_n.push((s_new, ne2));
                                        // 两侧都标：旧侧 REM 事实同样被本 diff 覆盖
                                        // （stale REM 无整壳 REPL 兜底时会打错位置）。
                                        affected_n.push((s_new, ne2));
                                        affected_o.push((os2, oe2));
                                        continue;
                                    }
                                    // 兜底：整壳 REPL（不完整/定位或渲染失败）。
                                    if let (Some(prefix), Some(v)) = (
                                        sh,
                                        render_shell_in_domain(&ctx.snap_new, (s_new, ne2), target, (s_new, ne2), &mut dcache),
                                    ) {
                                        if !upgraded_n.iter().any(|&(us, ue)| us == s_new && ue == ne2) {
                                            out.push(Patch::Replace { path: prefix, value: v });
                                        }
                                        upgraded_n.push((s_new, ne2));
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
                out.push(Patch::Remove { path: pn });
            }
            Patch::Replace { value, .. } | Patch::ReplaceAttr { value, .. } => {
                // 827 Bug E2：REPL 复用不可达——旧侧节点整体落在壳 REM 的
                // (ReplaceAttr 面级在适配层 ≡ Replace——同升级语义)
                // 旧节内（旧块随壳移除、不可复用），且新侧节点在全部新节
                // 外（根级新块）。升级为新侧域根块整体 ADD（值 = 域根渲
                // 染），REPL 本身吞并。同根多 REPL 经 upgraded_add_n 去重。
                if !nf.old_path.is_empty() {
                    if let Some(w_o) = walk_ast_path(
                        &ctx.o_seg, &ctx.snap_old, &ctx.o_tree, &nf.old_path, ctx.off_old,
                    ) {
                        if let AdaptWalk::Tree(t_o) = w_o {
                            let nd_o = &ctx.o_tree.nodes[t_o];
                            let (s_o, e_o) = (nd_o.start as usize, nd_o.end as usize);
                            if shell_removed_o.iter().any(|&(ss, se)| ss <= s_o && e_o <= se)
                                && crate::infer::innermost_interval(&new_intervals, s).is_none()
                            {
                                let d2 = crate::infer::sealed_interval_of(
                                    &snap.frame_log.entries, &snap.lines, &snap.open_depth, snap.line_len(),
                                    &new_intervals, s);
                                if let Some(part_e) = domain_parts(snap, d2, target, &mut dcache) {
                                    let tree_e = &part_e.1;
                                    let root_hit = tree_e.roots.iter().enumerate().find(|&(_, &r)| {
                                        let rn = &tree_e.nodes[r as usize];
                                        (rn.start as usize) <= s && e <= (rn.end as usize)
                                    });
                                    if let Some((k2, &r2)) = root_hit {
                                        let rn = &tree_e.nodes[r2 as usize];
                                        let (rs_, re_) = (rn.start as usize, rn.end as usize);
                                        if !upgraded_add_n.iter().any(|&(us, ue)| us == rs_ && ue == re_) {
                                            if let Some(v2) = render_in_domain(
                                                snap, d2, target, (rs_, re_),
                                                rn.block_type, rn.kind, rn.is_item(),
                                                &mut dcache,
                                            ) {
                                                let off2 = hs_off_before(cell, d2.0);
                                                out.push(Patch::Add {
                                                    path: format!("/ct/{}", off2 + k2),
                                                    value: v2,
                                                });
                                            }
                                            upgraded_add_n.push((rs_, re_));
                                        }
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
                // Phase C2 分类 1：普通非标题树节点 ReplaceAttr 只译路径，不整节点域渲染。
                // WC-9：**槽尾非空时同样必须保留字段写**——tight item 的值包 p 与 item
                // 同行（locate_in_domain 只能定位到那个 item），路径尾巴 `[0]` 正是
                // "值占第 0 槽"，`pn` 因此就是渲染树里那个 p 节点，attr 属于该节点。
                // 原实现在这里降级成整槽 REPL（值 = 裸行内串），客户端只能靠 K5 手工
                // 补 `{nt:"p",ct:…}`，而后端 F 帧是 `{"nt":"p","t":"","ct":…}` ⇒
                // `verify()` 报 `…/t undefined vs ""`（每点一次组件必现）。字段写与 F 帧
                // 逐值等价，无需任何补偿。
                if let Patch::ReplaceAttr { attr, value, .. } = p {
                    out.push(Patch::ReplaceAttr { path: pn, attr: attr.clone(), value: value.clone() });
                    continue;
                }
                let v = if tail.is_empty() {
                    render_in_domain(snap, (ds2, de2), target, (s, e),
                        nd.block_type, nd.kind, nd.is_item(), &mut dcache)
                        .unwrap_or_else(|| value.clone())
                } else {
                    value.clone()
                };
                out.push(Patch::Replace { path: pn, value: v });
            }
        }
    }
    // ── WC-5：成员整数组 ADD（`Add <壳>/ct = [...]`）覆盖其内的逐元素/更深 op。
    // 同一数组既"整体建"又"逐元素插"会双写（实测 h1_insert_mid：目标 1 个 M 得 2 个）；
    // 整体建的值来自新侧渲染（兄弟数组已含全部子树），故其下所有 op 冗余 → 丢弃。
    // 注意只针对**成员 ADD**：成员 REMOVE 不覆盖（它清空数组，后面的逐元素 ADD 是
    // 在不同坐标系上的重建，语义不同——由位置 diff 与 affected 标记保证一致）。
    {
        let member_adds: Vec<String> = out.iter().filter_map(|p| match p {
            Patch::Add { path, value } if path.ends_with("/ct") && value.starts_with('[') => {
                Some(path.clone())
            }
            _ => None,
        }).collect();
        if !member_adds.is_empty() {
            out.retain(|p| {
                let path = patch_path(p);
                !member_adds.iter().any(|m| {
                    path.len() > m.len() && path.starts_with(m.as_str())
                        && path.as_bytes()[m.len()] == b'/'
                })
            });
        }
    }
    // ── WC-5：跨 pass 去重——同一 (kind, path) 只保留首次出现。
    // 位置 diff（Site E / pass 1.5）与 pass 2 事实可能指向同一位置（hins 实测
    // 重复 REM：diff 清了整节子通道，pass 2 又发一条同路径 REM → apply 越界红）。
    {
        let mut seen: std::collections::HashSet<(u8, String)> = std::collections::HashSet::new();
        out.retain(|p| {
            let key = match p {
                Patch::Add { path, .. } => (0u8, path.clone()),
                Patch::Remove { path } => (1, path.clone()),
                Patch::Replace { path, .. } => (2, path.clone()),
                Patch::ReplaceAttr { path, attr, .. } => (3, format!("{path}#{attr}")),
            };
            seen.insert(key)
        });
    }

    // ── apply 安全序：REM（同父降序——先删大 index）→ ADD（升序）→ REPL ──
    let key = |p: &str| -> (Vec<usize>, usize) {
        let segs = parse_path_like(p);
        let (last, parents) = segs.split_last()
            .map(|(l, r)| (*l, r.to_vec())).unwrap_or((0, vec![]));
        (parents, last)
    };
    out.sort_by(|a, b| {
        let (ka, kb) = (key(patch_path(a)), key(patch_path(b)));
        match (patch_op(a), patch_op(b)) {
            ("REM", "REM") => kb.0.cmp(&ka.0).then(kb.1.cmp(&ka.1)),
            ("ADD", "ADD") | ("REPL", "REPL") => ka.0.cmp(&kb.0).then(ka.1.cmp(&kb.1)),
            ("REM", _) => std::cmp::Ordering::Less,
            (_, "REM") => std::cmp::Ordering::Greater,
            ("ADD", _) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Greater,
        }
    });
    out
}


// patch match attr —— claim 计算（N3/N4/C2）

//
// 语义（规格 D3/§3.2）：
//  - 只在 **最终** Vec<Patch> 上做事（adapt 之后、序列化之前）；
//  - 每条 op 挂**自己路径**的认领者：REM 查旧快照、ADD/REPL 查新快照；
//  - v1 不剪枝：候选 = by_doc 全集；只做 sens 短路（②）；
//  - claim 计算必须 total——Path worker 的 catch_unwind 会吞整帧（规格 F5）。

/// 路径 → 节点区间（只对最终补丁流做事）。
fn resolve_patch_interval(
    ctx: &ZoneContext,
    mode: crate::topology::ProjectionMode,
    p: &Patch,
    segs: &[usize],
    cache: &mut DomainCache,
) -> Option<(u32, u32)> {
    if segs.is_empty() {
        return None;
    }
    match mode {
        crate::topology::ProjectionMode::Hs => {
            let snap = match p {
                Patch::Remove { .. } => &ctx.snap_old,
                _ => &ctx.snap_new,
            };
            hs_resolve_interval(snap, segs, cache)
        }
        _ => cf_resolve_interval(ctx, p, segs),
    }
}

fn node_interval(tree: &crate::infer::AttrTreeNew, w: AdaptWalk) -> Option<(u32, u32)> {
    let i = match w {
        AdaptWalk::Tree(i) => i,
        // 槽位（inline/tight-para）归宿主块区间（规格 §3.2④ 边界语义）
        AdaptWalk::Slot { node, .. } => node,
    };
    let n = tree.nodes.get(i)?;
    Some((n.start, n.end))
}

/// CF 侧：REM 旧侧、其余新侧（与 adapt 侧选择同律）。
fn cf_resolve_interval(ctx: &ZoneContext, p: &Patch, segs: &[usize]) -> Option<(u32, u32)> {
    let (seg, snap, tree, off) = match p {
        Patch::Remove { .. } => (&ctx.o_seg, &ctx.snap_old, &ctx.o_tree, ctx.off_old),
        _ => (&ctx.n_seg, &ctx.snap_new, &ctx.n_tree, ctx.off_new),
    };
    let w = walk_ast_path(seg, snap, tree, segs, off)?;
    node_interval(tree, w)
}

/// HS 侧三步反解（规格 N4）：root_starts 定域 → domain_parts 取树 → hs_off_before + walk_ast_path。
fn hs_resolve_interval(snap: &Snapshot, segs: &[usize], cache: &mut DomainCache) -> Option<(u32, u32)> {
    let cell = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
    let line = *cell.root_starts.get(*segs.first()?)? as usize;
    let intervals: Vec<(usize, usize)> = cell.secs_iter().map(|(s, e, _, _)| (s, e)).collect();
    let sealed = crate::infer::sealed_interval_of(
        &snap.frame_log.entries,
        &snap.lines,
        &snap.open_depth,
        snap.line_len(),
        &intervals,
        line,
    );
    let part = domain_parts(snap, sealed, &crate::topology::HEADINGSECTION_TOPOLOGY, cache)?;
    let (seg, tree) = (&part.0, &part.1);
    let off = hs_off_before(&cell, seg.start_line);
    let w = walk_ast_path(seg, snap, tree, segs, off)?;
    node_interval(tree, w)
}

/// 与 patches 等长的认领数组（调用方保证 patches 非空）。
fn claim_ids(
    task: &IncrTask,
    patches: &[Patch],
    mode: crate::topology::ProjectionMode,
    reg: &Arc<parking_lot::Mutex<crate::r#match::MatchRegistry>>,
    ctx: &ZoneContext,
) -> Vec<crate::r#match::ClaimSet> {
    // 零分配空结果：serializer 视「claims 为空」= 全部无归属（规格「无 match 无额外代价」）。
    let defs = reg.lock().defs_for_doc(task.doc_id);
    if defs.is_empty() {
        return Vec::new();
    }
    let empty = || vec![crate::r#match::ClaimSet::new(); patches.len()];
    let frame = crate::r#match::frame_field_mask(patches);
    let active: Vec<Arc<crate::r#match::MatchDef>> = defs
        .into_iter()
        .filter(|d| !crate::r#match::sens_allows_skip(d, frame))
        .collect();
    if active.is_empty() {
        return empty();
    }
    // U6-1 解析提前：op 到区间，一次解析两用（触碰判定 + 挂载）。
    // REM 在旧坐标，经 LineMap.to_new 映射进新坐标参与 side1 触碰判定；ADD/REPL 已在新坐标。
    let mut cache = DomainCache::default();
    let line_map = LineMap::new(task);
    let mut ivs_new: Vec<(usize, usize)> = Vec::new();
    let mut per_op: Vec<Option<(usize, usize)>> = Vec::with_capacity(patches.len());
    for p in patches {
        let segs = parse_path_like(patch_path(p));
        let iv = resolve_patch_interval(ctx, mode, p, &segs, &mut cache)
            .map(|(s, e)| (s as usize, e as usize));
        if let Some((s, e)) = iv {
            match p {
                Patch::Remove { .. } => {
                    let s2 = line_map.to_new(s);
                    let e2 = line_map.to_new(e).max(s2);
                    ivs_new.push((s2, e2));
                }
                _ => ivs_new.push((s, e)),
            }
        }
        per_op.push(iv);
    }
    // U6-2 维护 scope：锚点几何平移 + 节尾每写从 section_cell 现取（节内追加天然覆盖）
    {
        let mut g = reg.lock();
        g.maintain_scopes(task.doc_id, (task.ripple_line_start, task.ripple_line_end), ctx.snap_new.line_len());
        g.refresh_scopes(task.doc_id, &ctx.snap_new);
    }
    // U6-3 求值：side0 永不求值（携带 nodes 即旧侧选择集）；side1 只对触碰 def 求值。
    let need_old = patches.iter().any(|p| matches!(p, Patch::Remove { .. }));
    let map_old = need_old.then(|| {
        let g = reg.lock();
        let sets: Vec<Vec<crate::block::BlockNode>> = active
            .iter()
            .map(|d| {
                g.states
                    .get(&(task.doc_id, d.def_id))
                    .and_then(|st| st.nodes.clone())
                    .unwrap_or_default()
            })
            .collect();
        crate::r#match::ClaimMap::from_sets(&active, sets)
    });
    let map_new = if !ivs_new.is_empty() {
        let mut g = reg.lock();
        let sets = g.eval_touched(task.doc_id, &active, &ctx.snap_new, &ivs_new);
        Some(crate::r#match::ClaimMap::from_sets(&active, sets))
    } else {
        None
    };
    // U6-4 挂载：直接消费已解析区间（原 match_resolve 二次解析消失）
    let mut out = Vec::with_capacity(patches.len());
    for (p, iv) in patches.iter().zip(per_op.iter()) {
        let map = match p {
            Patch::Remove { .. } => map_old.as_ref(),
            _ => map_new.as_ref(),
        };
        let mut claims = crate::r#match::ClaimSet::new();
        if let (Some(map), Some((s, e))) = (map, iv) {
            if let Some(v) = map.get(*s as u32, *e as u32) {
                claims = v.clone();
            }
        }
        out.push(claims);
    }
    out
}

/// panic 隔离（F5）：claim 失败降级空集，绝不吞掉整帧 P 补丁。
fn claim_ids_guarded(
    task: &IncrTask,
    patches: &[Patch],
    mode: crate::topology::ProjectionMode,
    reg: &Arc<parking_lot::Mutex<crate::r#match::MatchRegistry>>,
    ctx: &ZoneContext,
) -> Vec<crate::r#match::ClaimSet> {
    let n = patches.len();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        claim_ids(task, patches, mode, reg, ctx)
    }));
    match r {
        Ok(v) if v.len() == n => v,
        Ok(_) => vec![crate::r#match::ClaimSet::new(); n],
        Err(_) => {
            eprintln!("[match] claim_ids panicked — 降级空归属（本帧补丁照发）");
            vec![crate::r#match::ClaimSet::new(); n]
        }
    }
}

/// C2 入口：事实层恒 CF、拓扑只在适配层分派（与 incr_patches / hs_patches 同型），
/// 返回 (最终补丁, 与补丁等长的认领数组)。旧入口一字不改。
pub fn patches_and_claims(
    task: &IncrTask,
    mode: crate::topology::ProjectionMode,
    reg: &Arc<parking_lot::Mutex<crate::r#match::MatchRegistry>>,
) -> (Vec<Patch>, Vec<crate::r#match::ClaimSet>) {
    let (patches, ctx) = diff_patches_mode_with_ctx(task, &crate::topology::CONTAINER_TOPOLOGY);
    let topo = match mode {
        crate::topology::ProjectionMode::Hs => &crate::topology::HEADINGSECTION_TOPOLOGY,
        _ => &crate::topology::CONTAINER_TOPOLOGY,
    };
    let out = adapt(task, &patches, &ctx, topo);
    if out.is_empty() {
        return (out, Vec::new());
    }
    // 测量插桩：match_claim = match 计算总段（含求值 + 反解）。
    #[cfg(feature = "telemetry")]
    let _sp_match = crate::telemetry_span!("match_claim", "n" => out.len() as u64);
    let claims = claim_ids_guarded(task, &out, mode, reg, &ctx);
    debug_assert_eq!(claims.len(), out.len());
    (out, claims)
}

/// 无节侧（HS 与 CF 同构）——节点路径 + 值渲染直接走给定 CF 树（零新树，
/// 军令状 H2）。重挂场景专用（普通节点无节侧直接原样输出，不经过这里）。
fn emit_cf_side(
    snap: &Snapshot,
    seg: &crate::cache::Segment,
    tree: &crate::infer::AttrTreeNew,
    off: usize,
    idx: usize,
    tail: &[usize],
    target: &crate::topology::Topology,
) -> Option<(String, String)> {
    let comps = ast_path_of(tree, seg, snap, idx)?;
    let pn = path_str(&comps, off, tail);
    let va = seg.view.clone();
    let view = crate::infer::LineView::with_arrays(
        &seg.records, &seg.blocks, &snap.frame_log.entries, &snap.lines, &snap.rope,
        (seg.start_line as u32, seg.end_line as u32), va, &seg.arena);
    let mut buf = Vec::with_capacity(1024);
    crate::incr::render_tree(&mut buf, snap, &view, tree, idx, target);
    Some((pn, unsafe { String::from_utf8_unchecked(buf) }))
}

/// patch 的操作词（排序用）。
fn patch_op(p: &Patch) -> &'static str {
    match p {
        Patch::Add { .. } => "ADD",
        Patch::Remove { .. } => "REM",
        Patch::Replace { .. } | Patch::ReplaceAttr { .. } => "REPL",
    }
}

/// patch 的路径（排序用）。
fn patch_path(p: &Patch) -> &str {
    match p {
        Patch::Add { path, .. }
        | Patch::Remove { path }
        | Patch::Replace { path, .. }
        | Patch::ReplaceAttr { path, .. } => path,
    }
}
fn parse_path_like(path: &str) -> Vec<usize> {
    // WC-2（丙）：路径可能以**对象成员段**结尾（如 `/ct/0/ct` = 节点 0 的 ct 成员，
    // 承载"整数组建键"补丁）。成员段不是下标——遇到即停，返回其前的元素链；
    // 排序键/归一化只需要元素链，成员语义由应用器各自处理（原实现在此处把
    // 非数字段静默当 0，会把成员路径误判成更深一层的下标）。
    let mut out = Vec::new();
    for s in path.split("/ct/").skip(1) {
        match s.parse::<usize>() {
            Ok(i) => out.push(i),
            Err(_) => break,
        }
    }
    out
}

/// 827 表化:标题块判定查级别表(与 block::is_header 同源)。
fn is_heading_bt(bt: BlockType) -> bool {
    crate::block::header_level(bt).is_some()
}

/// HS 入口（阶段 3 对拍/将来切换）。
pub fn hs_patches(task: &IncrTask) -> Vec<Patch> {
    let (patches, ctx) = diff_patches_mode_with_ctx(task, &crate::topology::CONTAINER_TOPOLOGY);
    adapt(task, &patches, &ctx, &crate::topology::HEADINGSECTION_TOPOLOGY)
}


// 归属树 + render_tree_roots 是唯一渲染实现。


fn write_item_leaf(bw: &mut BufWriter<'_>, nc: &mut bool, snap: &Snapshot,
                   container_nt: crate::block::BlockType,
                   i: usize, item_end: usize, spans: &dyn ResolveView,
                   peel_layers: usize,
                   frame_depth: usize,
                   scratch: &mut crate::inline::InlineScratch) -> bool {
    #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("blk_item");
    use crate::effect::effect_for;
    let _t0 = if INLINE_DBG.with(|c| *c.borrow()) {
        Some(std::time::Instant::now())
    } else {
        None
    };
    let fk = crate::block::frame_kind(container_nt).unwrap();
    let eff = effect_for(fk);
    // item 的显示类型 = 容器类型（调用方在 auto_inject 的 prev_depth 深度取）。
    // 不能用 depth-1 重取——多层同开行（`- ``` ` 开 BLT+CF）会取到内层
    // （CF），item 变 c（F/P 分歧）。
    let item_type = container_nt;
    let item_nt = crate::effect::compact_type_name(item_type);
    let is_loose_li = spans.is_loose_at(i);
    // #255：item_end 前瞻（内容段续行并入——loose wrapper p 的 ct 覆盖
    // 打开行 + 续行——官方 item 段落单块）
    let node = snap.build_node(i, item_end, container_nt, is_loose_li);

    if *nc { bw.write_byte(b','); }
    *nc = true;

    bw.write_slice(b"{\"nt\":\"");
    bw.write_slice(item_nt.as_bytes());
    bw.write_slice(b"\",");

    let mut _cs: Vec<u8> = Vec::new();
    // 内容形态经 item_inline_plan 单源（零拷贝优先 + fallback 按层剥 +
    // apply 抑制判定——与 diff 偏移 item_inline_roots 完全同路径——
    // 消灭 oracle/渲染漂移）。
    let plan = item_inline_plan(snap, &node, spans, frame_depth, peel_layers, &mut _cs);
    let wl_tag = plan.tag;
    let wl_content = plan.content;
    let wi_ct_has = plan.content_has_inline;
    let is_content_block = plan.is_content_block;
    // 值包 p(外部形态定案):item attrs 只发 t(身份面)——Content(值文本)
    // 恒抑制:值 = 内容流首 p 块(wrapper 机制推广全部 item——tight/loose
    // 收敛;#301 直写与 loose wrapper 分支统一)。tag 不再 inline 拆段
    // (tag=身份——ck [x] 拆段消失,面语义阶段再归位)。
    {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("blk_item_attr");
        write_block_attrs_pre(bw, snap, &node, wl_tag.as_ref(), "", true, false);
    }

    // 值 p 判定:有文本值(!is_content_block && !is_empty_content)——
    // BlockContent(嵌套块条目)与空 item 无值 p。关壳约定:本函数只写
    // item 开头 + attrs + 内容流开(有值 p 时)+ p——不关壳——
    // render_node 按返回值补 children/关壳。
    // 值 p:有文本值 = 内容区间非空(非 BlockContent)。注:真空 item
    // (shape=EmptyContent)有续行并入区间时 content 非空——仍输出值 p 保
    // 内容不丢(scan 层并入语义为 pre-existing,CM 分歧另裁)。
    let has_value = !is_content_block && !wl_content.is_empty();
    if has_value {
        #[cfg(feature = "telemetry")]
        let _g = crate::telemetry::deep_span("blk_item_val");
        bw.write_slice(b"\"ct\":[");
        *nc = false; // fresh content stream, first element needs no comma
        let wtype = eff.loose_wrapper.unwrap_or("p");
        bw.write_slice(b"{\"nt\":\"");
        bw.write_slice(wtype.as_bytes());
        bw.write_slice(b"\",");
        // WC-1：同一个 nt 必须同一套键——值包 p 的 tag 面结构上恒空，也要写出来（空串），
        // 否则前端会看到两种 p 形状（值包无 t / 块叶有 t），"字段键恒在"假设被破坏。
        bw.write_slice(b"\"t\":\"\",");
        if wi_ct_has {
            // 富文本值:p 的 ct = 拆段数组(与普通段落 C1 同规则)
            bw.write_slice(b"\"ct\":[");
            write_inline_children_pre(bw, &mut false, snap, &node, "", wl_content.as_ref(), false, true, scratch);
            bw.write_slice(b"],");
        } else if !wl_content.is_empty() {
            bw.write_slice(b"\"ct\":\"");
            bw.write_str_escaped(wl_content.as_ref());
            bw.write_slice(b"\",");
        }
        let last = bw.peek_last();
        if last == Some(b',') { bw.pop_last(); }
        bw.write_byte(b'}');
        *nc = true; // value p emitted, next element needs comma
    }
    if let Some(t0) = _t0 {
        T_BLOCK.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
    }
    has_value
}

/// tight item 内段落块(#301——CM §5.2 tight 规则:item 内段落不包 p——
/// 官方裸文本)——inline 节点直写 item ch。无 inline 触发时单 txt 节点
/// (write_inline_children_pre 的 content 分支对无触发内容跳过——手动补)。
/// 与 P 侧渲染共用(F/P 一致)。
fn write_para_inline(bw: &mut BufWriter<'_>, nc: &mut bool, snap: &Snapshot,
                     node: &crate::block::BlockNode,
                     scratch: &mut crate::inline::InlineScratch) {
    let _t0 = if INLINE_DBG.with(|c| *c.borrow()) {
        Some(std::time::Instant::now())
    } else { None };
    let mut _cs = Vec::new();
    let (content, has_inline) = para_inline_plan(snap, node, &mut _cs);
    if content.is_empty() { return; }
    if has_inline {
        write_inline_children_pre(bw, nc, snap, node, "", content.as_ref(), false, true, scratch);
    } else {
        if *nc { bw.write_byte(b','); }
        *nc = true;
        bw.write_slice(b"{\"nt\":\"txt\",\"ct\":\"");
        bw.write_str_escaped(content.as_ref());
        bw.write_slice(b"\"}");
    }
    if let Some(t0) = _t0 {
        T_PARA_INLINE.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
    }
}


// diff_level 直接消费 AttrTree——align_container_node（align 场景——ps 行 →
// align 层容器索引——不是最深容器）


/// align 容器子树根(align 场景——树根 = **align 层容器**(共享容器层——
/// AttrNode.depth == align-1——文档 depth_at = AttrNode.depth + 1)——不是
/// 最深容器。
/// 原"最深容器"近似在 align 层 < 最深层时错误——BQ>BLT 场景
/// align=1(共享 BQ)但最深容器 = BLT——REPL /ct/0(整棵 BQ 替换)的 value
/// 只渲染 BLT——bl 替换 bq——roundtrip 分叉。align 容器层唯一(包含 line 的
/// depth==align-1 容器)。
fn align_container_node(tree: &crate::infer::AttrTreeNew, line: usize, align: usize) -> Option<usize> {
    let target_depth = align.saturating_sub(1);
    fn walk(tree: &crate::infer::AttrTreeNew, idx: usize, line: usize,
            target: usize, best: &mut Option<usize>) {
        let n = &tree.nodes[idx];
        if n.kind.is_some() && n.start as usize <= line && line < n.end as usize
            && n.depth as usize == target
        {
            *best = Some(idx);
            return;
        }
        for &c in n.children_of(tree) {
            walk(tree, c as usize, line, target, best);
        }
    }
    let mut best = None;
    for &r in &tree.roots {
        walk(tree, r as usize, line, target_depth, &mut best);
    }
    best
}


// render_tree——共享渲染遍历器(树驱动——归属树消费)
// 三分支(Shell 容器/ItemLeaf 段/Leaf 块)——渲染函数体复用
// (write_block_node/write_item_leaf/write_para_inline——函数体不动)。


/// 归属树渲染入口(单根)。输出到 buf。
/// 加 topology 参数——渲染分派查壳声明
/// (容器壳 shell_for / 章节壳 shell_by_block——拓扑无关渲染器)。
pub fn render_tree(buf: &mut Vec<u8>, snap: &Snapshot, spans: &dyn ResolveView,
                   tree: &crate::infer::AttrTreeNew, root: usize,
                   topology: &crate::topology::Topology) {
    render_tree_with(buf, snap, spans, tree, root, topology, &());
}

/// 单根渲染 + 归属注入（N5）。P 值路径走上面那个无归属入口（逐字节不变）。
pub fn render_tree_with(buf: &mut Vec<u8>, snap: &Snapshot, spans: &dyn ResolveView,
                        tree: &crate::infer::AttrTreeNew, root: usize,
                        topology: &crate::topology::Topology,
                        claims: &dyn ClaimLookup) {
    let mut scratch = crate::inline::InlineScratch::new();
    let mut ls = LineSupply::new(&snap.rope);
    render_tree_with_shared(buf, snap, spans, tree, root, topology, claims,
                            &mut scratch, &mut ls);
}

/// S1（性能专项）共享版单根渲染：`scratch`/`ls` 由调用方跨根持有——
/// `tree.roots` 按 start 升序、`LineSupply` 只进不退，故 ③ 的逐根循环
/// 共用一个 supply 即把每根「从行 0 重扫」降为「接着往前走」（总步数
/// O(总行数) 而非 Σ(根偏移)）。`BufWriter` 仍每根一个（存表需要每根
/// 独立字节）。旧 `render_tree_with` 退化为「建三重件→调本函数」，字节不变。
pub(crate) fn render_tree_with_shared<'r>(buf: &mut Vec<u8>, snap: &'r Snapshot, spans: &dyn ResolveView,
                        tree: &crate::infer::AttrTreeNew, root: usize,
                        topology: &crate::topology::Topology,
                        claims: &dyn ClaimLookup,
                        scratch: &mut crate::inline::InlineScratch,
                        ls: &mut LineSupply<'r>) {
    let mut bw = BufWriter::new(buf);
    let mut nc = false;
    render_node(&mut bw, snap, spans, tree, root, &mut nc, scratch, false, topology, claims, ls);
    bw.commit();
}

/// 多根渲染(全量 to_ast 形态——`[` + 根序列 + `]` 由调用方拼)。
pub fn render_tree_roots(buf: &mut Vec<u8>, snap: &Snapshot, spans: &dyn ResolveView,
                         tree: &crate::infer::AttrTreeNew,
                         topology: &crate::topology::Topology) {
    render_tree_roots_with(buf, snap, spans, tree, topology, &());
}

/// 多根渲染 + 归属注入（N5，F 帧入口）。无认领 ⇒ 与上面逐字节一致。
pub fn render_tree_roots_with(buf: &mut Vec<u8>, snap: &Snapshot, spans: &dyn ResolveView,
                              tree: &crate::infer::AttrTreeNew,
                              topology: &crate::topology::Topology,
                              claims: &dyn ClaimLookup) {
    let mut bw = BufWriter::new(buf);
    let mut nc = false;
    let mut scratch = crate::inline::InlineScratch::new();
    let mut ls = LineSupply::new(&snap.rope);
    for &r in &tree.roots {
        render_node(&mut bw, snap, spans, tree, r as usize, &mut nc, &mut scratch, false, topology, claims, &mut ls);
    }
    bw.commit();
}

/// 归属键（N5）：块级节点对象在收尾前追加 「,match:[…]」（空集省略）。
/// 无注册 / P 值路径传 &() ⇒ 一个字节都不写（F 帧逐字节不变由判官钉住）。
fn write_claims_key(bw: &mut BufWriter<'_>, claims: &dyn ClaimLookup, start: u32, end: u32) {
    let Some(tags) = claims.claims(start, end) else { return };
    if tags.is_empty() {
        return;
    }
    bw.write_slice(b",\"match\":[");
    for (i, t) in tags.iter().enumerate() {
        if i > 0 { bw.write_byte(b','); }
        bw.write_byte(b'"');
        bw.write_json_escaped(t);
        bw.write_byte(b'"');
    }
    bw.write_byte(b']');
}
/// 递归三分支渲染。
/// 逗号管理:容器壳内独立 nc(壳间兄弟逗号);ItemLeaf 内共享 nc
/// (write_item_leaf 已重置——children 首子无逗号);块共享 nc。
/// parent_item:父节点是 ItemLeaf(tight 段落直写的父判定)。
/// topology 参数——壳声明查表(容器 shell_for / 章节壳 shell_by_block——
/// 拓扑无关渲染器——共享形态 + 扩展形态两层分派)。
fn render_node<'r>(bw: &mut BufWriter<'_>, snap: &'r Snapshot, spans: &dyn ResolveView,
               tree: &crate::infer::AttrTreeNew, idx: usize,
               nc: &mut bool, scratch: &mut crate::inline::InlineScratch,
               parent_item: bool, topology: &crate::topology::Topology,
               claims: &dyn ClaimLookup, ls: &mut LineSupply<'r>) {
    let n = &tree.nodes[idx];
    if let Some(fk) = n.kind {
        // TableRow 记录节点(blk=None——PopPush 复用容器——行 Shell 是
        // 渲染单位)——透明:不写壳,children 直接上提到父数组(共享 nc——
        // TableRow 不产独立壳——每数据行一个 tr)
        if crate::infer::render_shape::is_table_row_transparent(n) {
            #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("rn_tr");
            let _t0 = if INLINE_DBG.with(|c| *c.borrow()) {
                Some(std::time::Instant::now())
            } else { None };
            for &c in tree.nodes[idx].children_of(tree) {
                render_node(bw, snap, spans, tree, c as usize, nc, scratch, parent_item, topology, claims, ls);
            }
            if let Some(t0) = _t0 {
                T_TR.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
            }
            return;
        }
        // ── 容器壳(含 TableRow 行 Shell——TableRow 的容器语义)──
        // 壳查 topology.shells (nt + attrs 规格——attrs 空 = 不写——
        // is_shell 语义——行为不变——引用同一 BLOCK_ATTRS)
        #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("rn_shell");
        let _shell_t0 = if INLINE_DBG.with(|c| *c.borrow()) {
            Some(std::time::Instant::now())
        } else { None };
        let shell = topology.shell_for(fk);
        if *nc { bw.write_byte(b','); }
        *nc = true;
        bw.write_slice(b"{\"nt\":\"");
        bw.write_slice(shell.nt.as_bytes());
        bw.write_slice(b"\",");
        if !shell.attrs.is_empty() {
            // attrs(非 shell 容器:CF/TableRow/表头——开壳时写)
            // 内容源块与节点字段同(container_block_type/start/end)。
            // content_end=0(容器无开行并入块——单行 Comment 即此)
            // 时回退节点 end——整容器区间作内容源(范围解析由
            // read_block_proj 侧完成,Comment 单行内容得以输出)。
            let ce = tree.nodes[idx].content_end as usize;
            let content_end = if ce > 0 { ce } else { tree.nodes[idx].end as usize };
            let tmp = {
                #[cfg(feature = "telemetry")] let _s_b = crate::telemetry_span!("rn_build");
                snap.build_node(tree.nodes[idx].start as usize,
                    content_end,
                    n.block_type, n.loose())
            };
            write_block_attrs(bw, snap, spans, &tmp, shell.attrs, false, false);
        }
        // 表头列(cols——可缺省 attr;键名避开内容流 "ct"(attrs Content
        // 与通道撞名——换名承载);值 = 表头行(容器开行)的 cells,与
        // 数据行 tr 的 ct 同 join 语义(trim + \t)。渲染输出层改动——
        // 归属树/读面/写面不动)。
        if fk == FrameKind::Table {
            // 表头行短——直接复制(壳渲染低频,零拷贝不值得)
            let hdr_line = snap.rope.line(tree.nodes[idx].start as usize).to_string();
            let hdr = crate::block::str_view(crate::block::strip_bq(hdr_line.as_bytes(), false));
            let hdr_trim = hdr.trim_end_matches(['\n', '\r']);
            let (_, ranges) = crate::block::cell_ranges_bytes(hdr_trim.as_bytes());
            if !ranges.is_empty() {
                let joined = ranges
                    .iter()
                    .map(|r| hdr_trim[r.clone()].trim().to_string())
                    .collect::<Vec<_>>()
                    .join("\t");
                bw.write_slice(b"\"cols\":\"");
                bw.write_str_escaped(&joined);
                bw.write_slice(b"\",");
            }
        }
        if let Some(t0) = _shell_t0 {
            // 壳自耗时（开壳 + attrs），children 递归不计入。
            T_SHELL.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
        }
        // ch:[] 移除（用户裁决——无 children 不写 ch）。
        // HtmlBlock:内容按自身规格(RawText)渲染 = 整块原文——attrs 的
        // ct(Raw 投影,tmp 区间 = 容器整块)已写毕;children(元素化 he
        // 帧——scan 内部组织)若再输出会产生第二个 ct 键(撞键——JSON
        // 双 ct,消费端取后键覆盖原文)。he 树保留为内部事实,不进通道。
        if crate::infer::render_shape::allows_children(fk) && !tree.nodes[idx].children_of(tree).is_empty() {
            bw.write_slice(b"\"ct\":[");
            let mut child_nc = false;
            for &c in tree.nodes[idx].children_of(tree) {
                render_node(bw, snap, spans, tree, c as usize, &mut child_nc, scratch, false, topology, claims, ls);
            }
            bw.write_slice(b"]");
            write_claims_key(bw, claims, n.start, n.end);
            bw.write_byte(b'}');
        } else {
            let last = bw.peek_last();
            if last == Some(b',') { bw.pop_last(); }
            write_claims_key(bw, claims, n.start, n.end);
            bw.write_byte(b'}');
        }
    } else if crate::infer::render_shape::is_item_branch(n) {
        // ── ItemLeaf 段(内容源块语义——seg_depth = 容器帧深度——
        // peel_layers = list_frame_count(seg_depth))──
        #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("rn_item");
        let peel_layers = list_frame_count(spans, tree.nodes[idx].start as usize, n.seg_depth as usize);
        // write_item_leaf 写 item 开头 + attrs + ch 开(有 inline 子时)+ 内容——
        // 返回 has_ch(inline 子存在)——树子节点(n.children)也进 ch——
        // 两者任一存在都写 ch(嵌套列表段:无 inline 但 children 非空——补 ch 开)
        let has_ch = write_item_leaf(bw, nc, snap, n.block_type, tree.nodes[idx].start as usize,
            tree.nodes[idx].end as usize, spans, peel_layers, n.seg_depth as usize, scratch);
        if has_ch || !tree.nodes[idx].children_of(tree).is_empty() {
            if !has_ch {
                bw.write_slice(b"\"ct\":[");
                *nc = false;
            }
            for &c in tree.nodes[idx].children_of(tree) {
                render_node(bw, snap, spans, tree, c as usize, nc, scratch, true, topology, claims, ls);
            }
            bw.write_slice(b"]");
            write_claims_key(bw, claims, n.start, n.end);
            bw.write_byte(b'}');
        } else {
            // 无子(ch:[] 移除)——去 attrs 尾逗号后关壳
            let last = bw.peek_last();
            if last == Some(b',') { bw.pop_last(); }
            write_claims_key(bw, claims, n.start, n.end);
            bw.write_byte(b'}');
        }
        // 弹壳语义——children 空时 write_item_leaf 重置的 false 会漏下一个
        // 兄弟的逗号
        *nc = true;
    } else {
        // ── 章节壳(块类型 ∈ shells_by_block——扩展形态)──
        // 章节壳 = 块节点(kind=None)+ 壳声明——nt + attrs(lv/t)+ ch 递归——
        // 与容器壳同构(渲染形态共享——壳声明数据化)
        if let Some(shell) = crate::infer::render_shape::section_shell(topology, n) {
            #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("rn_shell_sec");
            let _sec_t0 = if INLINE_DBG.with(|c| *c.borrow()) {
                Some(std::time::Instant::now())
            } else { None };
            if *nc { bw.write_byte(b','); }
            *nc = true;
            bw.write_slice(b"{\"nt\":\"");
            bw.write_slice(shell.nt.as_bytes());
            bw.write_slice(b"\",");
            let tmp = {
                #[cfg(feature = "telemetry")] let _s_b = crate::telemetry_span!("rn_build");
                snap.build_node(tree.nodes[idx].start as usize,
                    tree.nodes[idx].content_end as usize,
                    n.block_type, n.loose())
            };
            // 章节壳同律（D3）：Tag 面有行内 → 抑制 t 字符串并把数组写进同一键；
            // 壳的 ct 是子通道 → Content 字符串恒抑制（等价原 SECTION_ATTRS 过滤 ct）。
            let shell_line = ls.line_at(tmp.start_line);
            let (shell_tag, shell_tag_inline) = extract_tag_str_line(snap, &tmp, shell_line);
            write_block_attrs(bw, snap, spans, &tmp, shell.attrs, true, shell_tag_inline);
            if shell_tag_inline {
                let feed_key = inline_feed_attr_name(n.block_type, true);
                bw.write_slice(b"\"");
                bw.write_slice(feed_key.as_bytes());
                bw.write_slice(b"\":[");
                write_inline_children_pre(bw, &mut false, snap, &tmp, shell_tag.as_ref(), "", true, false, scratch);
                bw.write_slice(b"],");
            }
            if let Some(t0) = _sec_t0 {
                // 章节壳自耗时（开壳 + lv/t attrs），children 递归不计入。
                T_SEC_SHELL.with(|c| *c.borrow_mut() += t0.elapsed().as_secs_f64() * 1000.0);
            }
            if !tree.nodes[idx].children_of(tree).is_empty() {
                bw.write_slice(b"\"ct\":[");
                let mut child_nc = false;
                for &c in tree.nodes[idx].children_of(tree) {
                    render_node(bw, snap, spans, tree, c as usize, &mut child_nc, scratch, false, topology, claims, ls);
                }
                bw.write_slice(b"]");
                write_claims_key(bw, claims, n.start, n.end);
                bw.write_byte(b'}');
            } else {
                let last = bw.peek_last();
                if last == Some(b',') { bw.pop_last(); }
                write_claims_key(bw, claims, n.start, n.end);
                bw.write_byte(b'}');
            }
            return;
        }
        // ── 块 leaf ──
        // WC-13：本分支**保持原判定**（"渲染零变化"是硬不变量；若改成
        // channel_slots==0 会让 sdl_topology_content_conservation 的 doc-7 CF 渲染变空
        // ——TableRow 块叶是真实渲染元素）。diff 侧的 channel_slots 与它逐条对应
        // （同一 skip 集合），由漂移判官对拍。
        if crate::infer::render_shape::is_skip_leaf(n.block_type) {
            // 空 BQ 开行 Blank 块等——渲染跳过(skip 语义)
            #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("rn_skip");
            return;
        }
        #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("rn_leaf");
        let node = {
            #[cfg(feature = "telemetry")] let _s_b = crate::telemetry_span!("rn_build");
            snap.build_node(tree.nodes[idx].start as usize,
                tree.nodes[idx].end as usize,
                n.block_type, n.loose())
        };
        if crate::infer::render_shape::is_tight_para(parent_item, n) {
            // #301 tight 段落直写(特判——父 ItemLeaf 且不 loose——item 内
            // 多块场景的段落块不包 p——CM §5.2 tight 规则;值段 p 不受影响)
            write_para_inline(bw, nc, snap, &node, scratch);
        } else {
            write_block_node(bw, nc, snap, spans, &node, scratch, claims, ls);
        }
    }
}

/// MD 渲染（R0/L1a）。
///
/// full: 0..n 行游标，与 trinity::to_md 同构；不建树。
/// selected: AttrTree cover + 祖先容器前缀剥离 + 行游标。
/// 本模块只依赖 infer::render_shape / infer::md_line::emit_line，不依赖 JSON writer。
pub(crate) mod md_render {
    use crate::block::{BlockNode, FrameKind};
    use crate::infer::render_shape::{self, NodeShape};
    use crate::infer::AttrTreeNew;
    use crate::projection::{MdRenderError, Snapshot};
    use crate::topology::{ProjectionMode, Topology};

    /// Snapshot 版 BQ 深度（渲染用；底层实现在 infer::md_line）。
    fn line_depths(snap: &Snapshot) -> Vec<usize> {
        crate::infer::md_line::bq_depths(&snap.frame_log.entries, snap.lines.as_slice())
    }

    /// 全篇 canonical md：逐行 (line, PL, prev PL, BQ 深度) -> emit_line。
    pub(crate) fn render_all(snap: &Snapshot, _mode: ProjectionMode) -> String {
        let n = snap.line_len();
        if n == 0 {
            return String::new();
        }
        let depths = line_depths(snap);
        let mut out = String::with_capacity(snap.rope.byte_len() + 1);
        for i in 0..n {
            let line = snap.rope.line(i).to_string();
            let ft = snap.lines[i].node_type();
            let prev = if i > 0 { Some(snap.lines[i - 1].node_type()) } else { None };
            for s in crate::infer::md_line::emit_line(&line, ft, prev, depths[i], None) {
                out.push_str(&s);
            }
        }
        out
    }

    /// 选中 BlockNode（IDENT）的 md：按输入顺序拼接；默认允许重复/重叠。
    pub(crate) fn render_selected(
        snap: &Snapshot,
        mode: ProjectionMode,
        nodes: &[BlockNode],
    ) -> Result<String, MdRenderError> {
        if nodes.is_empty() {
            return Ok(String::new());
        }
        let cell = snap.tree_cell(mode.topology());
        let tree = &cell.tree;
        let depths = line_depths(snap);
        let mut out = String::new();
        for target in nodes {
            let (idx, path) = find_cover(tree, mode.topology(), target)?;
            render_subtree(snap, tree, mode.topology(), idx, &path, target, &depths, &mut out)?;
        }
        Ok(out)
    }

    /// 选择节点 -> AttrTree 节点。
    ///
    /// 规则（A7 初版）:
    /// - frame_ordinal == 255（item/leaf/section）：优先 section / item / leaf；
    /// - frame_ordinal < 255（同线壳）：按容器深度排序取第 fo 个；
    /// - 同优先级多候选 -> AmbiguousCover；无候选 -> NodeNotFound。
    fn find_cover(
        tree: &AttrTreeNew,
        topology: &Topology,
        target: &BlockNode,
    ) -> Result<(usize, Vec<usize>), MdRenderError> {
        let mut candidates: Vec<(usize, Vec<usize>)> = Vec::new();
        let mut path = Vec::new();
        for &root in &tree.roots {
            collect_candidates(tree, root as usize, &mut path, target, &mut candidates);
        }
        if candidates.is_empty() {
            return Err(MdRenderError::NodeNotFound);
        }
        if target.frame_ordinal == 255 {
            let mut ranked: Vec<(u8, usize, Vec<usize>)> = candidates
                .into_iter()
                .map(|(idx, p)| {
                    let shape = render_shape::shape(&tree.nodes[idx], topology, false);
                    let prio = match shape {
                        NodeShape::Section { .. } => 4,
                        NodeShape::Item => 3,
                        NodeShape::Leaf { .. } => 2,
                        NodeShape::Container { .. } => 1,
                        NodeShape::Transparent => 0,
                    };
                    (prio, idx, p)
                })
                .collect();
            ranked.sort_by_key(|x| std::cmp::Reverse(x.0));
            let top = ranked[0].0;
            let best: Vec<_> = ranked.into_iter().filter(|x| x.0 == top).collect();
            if best.len() != 1 {
                return Err(MdRenderError::AmbiguousCover);
            }
            return Ok((best[0].1, best[0].2.clone()));
        }
        // 同线壳：优先 Container；按 depth 升序，取第 frame_ordinal 个。
        // list item 形 target（identity ord0 壳）应先落 Item 节点：容器壳
        // 按 target 区间裁剪会渲染成空（projection checker 的 99% 家族）。
        if crate::effect::block_decl(target.node_type).is_list_item {
            let items: Vec<(usize, Vec<usize>)> = candidates
                .iter()
                .filter_map(|(idx, p)| {
                    match render_shape::shape(&tree.nodes[*idx], topology, false) {
                        NodeShape::Item => Some((*idx, p.clone())),
                        _ => None,
                    }
                })
                .collect();
            if items.len() == 1 {
                return Ok((items[0].0, items[0].1.clone()));
            }
        }
        let mut containers: Vec<(usize, Vec<usize>, u8)> = candidates
            .iter()
            .filter_map(|(idx, p)| {
                match render_shape::shape(&tree.nodes[*idx], topology, false) {
                    NodeShape::Container { .. } => Some((*idx, p.clone(), tree.nodes[*idx].depth)),
                    _ => None,
                }
            })
            .collect();
        containers.sort_by_key(|x| (x.2, x.0));
        let i = target.frame_ordinal as usize;
        if i < containers.len() {
            // 同 depth 多候选：无法唯一 -> Ambiguous。
            if containers.iter().filter(|x| x.2 == containers[i].2).count() > 1 {
                return Err(MdRenderError::AmbiguousCover);
            }
            return Ok((containers[i].0, containers[i].1.clone()));
        }
        // 有些 item 的 identity frame_ordinal 仍是 0（例如同节第二个 Bullet），
        // 但这一行没有同 start 的容器壳；退回 item/leaf 候选。
        let fallback: Vec<(usize, Vec<usize>)> = candidates
            .into_iter()
            .filter(|(idx, _)| {
                matches!(
                    render_shape::shape(&tree.nodes[*idx], topology, false),
                    NodeShape::Item | NodeShape::Leaf { .. }
                )
            })
            .collect();
        if fallback.len() == 1 {
            return Ok(fallback.into_iter().next().unwrap());
        }
        Err(if fallback.is_empty() {
            MdRenderError::NodeNotFound
        } else {
            MdRenderError::AmbiguousCover
        })
    }

    fn collect_candidates(
        tree: &AttrTreeNew,
        idx: usize,
        path: &mut Vec<usize>,
        target: &BlockNode,
        out: &mut Vec<(usize, Vec<usize>)>,
    ) {
        path.push(idx);
        let n = &tree.nodes[idx];
        if n.start == target.start_line as u32
            && n.block_type == target.node_type
        {
            out.push((idx, path.clone()));
        }
        for &c in n.children_of(tree) {
            collect_candidates(tree, c as usize, path, target, out);
        }
        path.pop();
    }

    fn is_list_kind(k: FrameKind) -> bool {
        matches!(k, FrameKind::BulletList | FrameKind::OrderedList)
    }

    /// 渲染以 selected_idx 为根的子树；祖先容器前缀剥掉，item 保留 parent list marker。
    fn render_subtree(
        snap: &Snapshot,
        tree: &AttrTreeNew,
        topology: &Topology,
        selected_idx: usize,
        path: &[usize],
        target: &BlockNode,
        depths: &[usize],
        out: &mut String,
    ) -> Result<(), MdRenderError> {
        let node = &tree.nodes[selected_idx];
        let start = target.start_line;
        let end = target.end_line;
        if start >= end || end > snap.line_len() {
            return Err(MdRenderError::SpanOutOfRange);
        }
        // 祖先容器（selected 之上的 path 节点）的 BQ 帧数：BQ 前缀在每条
        // 续行上都出现，必须剥；祖先 list 帧的 marker 只出现在各自开行，
        // 续行上并不重复——所以 list 剥离只对「本行同线壳」做。
        let strip_bq: usize = path
            .iter()
            .take(path.len().saturating_sub(1))
            .filter_map(|&i| match render_shape::shape(&tree.nodes[i], topology, false) {
                NodeShape::Container { fk, .. } if fk == FrameKind::BlockQuote => Some(()),
                _ => None,
            })
            .count();
        let start_shells = push_shell_kinds(snap, start);
        let start_list_shells = start_shells
            .iter()
            .filter(|k| is_list_kind(**k))
            .count();
        // 本行需要剥掉的「祖先 list marker 层」数：
        // - 壳选择（fo<255）：壳序号之前的同线 list 壳；
        // - item（fo=255 且 node_type 是列表项）：保留自己的 item marker，剥其余；
        // - 内容叶（其余 255）：该行 marker 都是祖先，全剥。
        let strip_lists = if target.frame_ordinal < 255 {
            start_shells
                .iter()
                .take(target.frame_ordinal as usize)
                .filter(|k| is_list_kind(**k))
                .count()
        } else if crate::effect::block_decl(target.node_type).is_list_item {
            start_list_shells.saturating_sub(1)
        } else {
            start_list_shells
        };
        for i in start..end {
            let line = snap.rope.line(i).to_string();
            let (layers, _) = crate::block::peel(&line);
            let mut cut = 0usize;
            let mut bq_stripped = 0usize;
            let mut n_bq = strip_bq;
            let mut n_list = if i == start { strip_lists } else { 0 };
            for layer in &layers {
                if layer.kind == FrameKind::BlockQuote && n_bq > 0 {
                    n_bq -= 1;
                    bq_stripped += 1;
                    cut = layer.end;
                    continue;
                }
                if is_list_kind(layer.kind) && n_list > 0 {
                    n_list -= 1;
                    cut = layer.end;
                    continue;
                }
                break;
            }
            let rest = &line[cut.min(line.len())..];
            let ft = snap.lines[i].node_type();
            let prev = if i > 0 { Some(snap.lines[i - 1].node_type()) } else { None };
            let d = depths[i].saturating_sub(bq_stripped);
            for s in crate::infer::md_line::emit_line(rest, ft, prev, d, None) {
                out.push_str(&s);
            }
        }
        Ok(())
    }

    /// 该行真开帧的 push 序列（frame_ordinal 的同源口径：PopPush 延续、
    /// body_mut 换态不计；kind 序 = 发射序）。
    fn push_shell_kinds(snap: &Snapshot, line: usize) -> Vec<FrameKind> {
        snap.frame_log
            .entries
            .iter()
            .filter(|&&e| {
                crate::block::entry_line(e) == line as u32
                    && !crate::block::entry_is_pop(e)
                    && !crate::block::entry_is_body_mut(e)
            })
            .map(|&e| crate::block::entry_frame(e).kind())
            .collect()
    }

}

#[cfg(test)]
mod md_render_tests {
    use crate::projection::State;
    use crate::topology::ProjectionMode;
    use crate::block::{BlockNode, BlockType};

    fn snap(text: &str) -> crate::projection::Snapshot {
        let mut st = State::new(text);
        st.snapshot()
    }

    fn node(s: &crate::projection::Snapshot, start: usize, end: usize, bt: BlockType, fo: u8) -> BlockNode {
        let mut n = s.build_node(start, end, bt, false);
        n.frame_ordinal = fo;
        n
    }

    #[test]
    fn empty_selection_is_empty() {
        let s = snap("");
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[]).unwrap(), "");
    }

    #[test]
    fn selected_same_line_shells() {
        let s = snap("- - foo\n");
        let outer = node(&s, 0, 1, BlockType::Bullet, 0);
        let inner = node(&s, 0, 1, BlockType::Bullet, 1);
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[outer]).unwrap(), "- - foo\n");
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[inner]).unwrap(), "- foo\n");
    }

    #[test]
    fn selected_bq_and_item() {
        let s = snap("> - bar\n");
        let bq = node(&s, 0, 1, BlockType::Blockquote, 0);
        let item = node(&s, 0, 1, BlockType::Bullet, 255);
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[bq]).unwrap(), "> - bar\n");
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[item]).unwrap(), "- bar\n");
    }

    #[test]
    fn selected_header_section_and_paragraph() {
        let s = snap("# A\n\npara one\npara two\n\n## B\n- a: 1\n- b: 2\n");
        let h1 = node(&s, 0, 8, BlockType::Header1, 255);
        let h2 = node(&s, 5, 8, BlockType::Header2, 255);
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[h1]).unwrap(),
            "# A\n\npara one\npara two\n\n## B\n- a: 1\n- b: 2\n");
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[h2]).unwrap(),
            "## B\n- a: 1\n- b: 2\n");
    }


    #[test]
    fn json_payload_bytes_pinned() {
        use sha2::{Digest, Sha256};
        fn hash(s: &crate::projection::Snapshot) -> String {
            let j = s.to_ast_object_mode(ProjectionMode::Cf);
            let i = j.find("\"ct\":[").unwrap() + 6;
            format!("{:x}", Sha256::digest(j[i..].as_bytes()))
        }
        let cases = [
            ("# A\n\npara one\npara two\n\n## B\n- a: 1\n- b: 2\n",
             "739a178c1200eec16a1c2b8ee993656fb6ece62dede7f5b29a2ce12bb21f5402"),
            ("- - foo\n",
             "720fa04fb43cdff1bce2a8f5410c751aed66353fe60c08bc03d33e2bac8fccb2"),
            ("> - bar\n",
             "bdc95165d60d45de250dbc9e20a24d9aa763c61fe6d8cfa58b6ea9fed80497c0"),
            ("line one\nline two\nline three\n",
             "3faa35897c12a620d27189b377dfdcc6fbaac806bce673803fcff4e907daa0ec"),
            ("- a\n\n  b\n\n- c\n",
             "fd8c44e50569198390452549f2522349409e5fa9184484b18a143df05fbfa8f5"),
        ];
        for (doc, want) in cases {
            assert_eq!(hash(&snap(doc)), want, "doc={doc:?}");
        }
    }

    #[test]
    fn selected_multi_line_and_loose() {
        let s = snap("line one\nline two\nline three\n");
        let p = node(&s, 0, 3, BlockType::Paragraph, 255);
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[p]).unwrap(),
            "line one\nline two\nline three\n");

        let l = snap("- a\n\n  b\n\n- c\n");
        let a = node(&l, 0, 1, BlockType::Bullet, 255);
        let c = node(&l, 4, 5, BlockType::Bullet, 255);
        assert_eq!(l.render_md_selected(ProjectionMode::Cf, &[a]).unwrap(), "- a\n");
        assert_eq!(l.render_md_selected(ProjectionMode::Cf, &[c]).unwrap(), "- c\n");
    }

    #[test]
    fn selected_multiple_roots_keep_order_and_errors() {
        let s = snap("- - foo\n");
        let outer = node(&s, 0, 1, BlockType::Bullet, 0);
        let inner = node(&s, 0, 1, BlockType::Bullet, 1);
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[outer, inner]).unwrap(),
            "- - foo\n- foo\n");
        let bad = node(&s, 3, 4, BlockType::Paragraph, 255);
        assert!(s.render_md_selected(ProjectionMode::Cf, &[bad]).is_err());
    }

    /// find_cover 的 tree→identity 迁移的 parity 基线：identity 产品
    /// （selector::all_nodes）与「AttrTree 去 Transparent 投影」的
    /// (start,end,type) 多集差异。当前两 walker 分区不一致（phantom 等），
    /// 差异数应随迁移逐步归零；数字变化须按本条钉住复核。
    #[test]
    fn find_cover_tree_identity_parity_baseline() {
        use std::collections::BTreeMap;
        let docs = [
            "- - foo\n",
            "> - bar\n",
            "> - x\n>   - y\n",
            "# A\n\npara one\n\n## B\n- a: 1\n- b: 2\n",
            "| a | b |\n| - | - |\n| 1 | 2 |\n",
            "- a\n\n  b\n\n- c\n",
        ];
        let topo = ProjectionMode::Cf.topology();
        for doc in docs {
            let s = snap(doc);
            let identity: BTreeMap<(usize, usize, u8), usize> = crate::selector::all_nodes(&s)
                .iter()
                .map(|n| (n.start_line, n.end_line, n.node_type as u8))
                .fold(BTreeMap::new(), |mut m, k| { *m.entry(k).or_default() += 1; m });
            let cell = s.tree_cell(topo);
            let tree = &cell.tree;
            let mut proj: BTreeMap<(usize, usize, u8), usize> = BTreeMap::new();
            for n in &tree.nodes {
                if matches!(
                    crate::infer::render_shape::shape(n, topo, false),
                    crate::infer::render_shape::NodeShape::Transparent
                ) {
                    continue;
                }
                *proj.entry((n.start as usize, n.end as usize, n.block_type as u8)).or_default() += 1;
            }
            let keys: std::collections::BTreeSet<_> =
                identity.keys().chain(proj.keys()).collect();
            let diff: Vec<_> = keys
                .iter()
                .map(|k| {
                    let d = identity.get(*k).copied().unwrap_or(0) as isize
                        - proj.get(*k).copied().unwrap_or(0) as isize;
                    (**k, d)
                })
                .filter(|(_, d)| *d != 0)
                .collect();
            // 钉住当前两 walker 的分区差异（tree→identity 迁移时逐条归零复核）。
            let expected: &[((usize, usize, u8), isize)] = match doc {
                "- - foo\n" => &[((0, 1, 6), -2)],
                "> - bar\n" => &[],
                "> - x\n>   - y\n" => &[((1, 2, 6), -1)],
                "# A\n\npara one\n\n## B\n- a: 1\n- b: 2\n" => &[
                    ((0, 1, 0), -1), ((0, 7, 0), 1),
                    ((4, 5, 1), -1), ((4, 7, 1), 1), ((5, 7, 6), -1),
                ],
                "| a | b |\n| - | - |\n| 1 | 2 |\n" => &[
                    ((0, 1, 12), 1), ((0, 3, 12), -1), ((2, 3, 12), -1),
                ],
                "- a\n\n  b\n\n- c\n" => &[
                    ((0, 1, 6), -1), ((0, 4, 6), 1), ((0, 5, 6), -1),
                ],
                _ => &[],
            };
            assert_eq!(diff, expected.to_vec(), "parity diff for {doc:?}");
        }
    }

    #[test]
    fn selected_contract_order_duplicates_trailing_newline() {        use crate::projection::MdRenderError;
        let s = snap("- a\n\n- b\n");
        let a = node(&s, 0, 1, BlockType::Bullet, 255);
        let b = node(&s, 2, 3, BlockType::Bullet, 255);
        // 顺序按输入；允许重复；每块尾 \n（最后一块也带）。
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[b.clone(), a.clone()]).unwrap(), "- b\n- a\n");
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[a.clone(), a.clone()]).unwrap(), "- a\n- a\n");
        // 布局行（块间空行）不输出。
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[a, b]).unwrap(), "- a\n- b\n");
        // 结构化错误：cover 无候选 / span 越界。
        let notfound = node(&s, 1, 2, BlockType::Paragraph, 255);
        assert!(matches!(s.render_md_selected(ProjectionMode::Cf, &[notfound]), Err(MdRenderError::NodeNotFound)));
        let oob = node(&s, 0, 99, BlockType::Bullet, 255);
        assert!(matches!(s.render_md_selected(ProjectionMode::Cf, &[oob]), Err(MdRenderError::SpanOutOfRange)));
        // 整篇保真入口保留布局行。
        assert_eq!(s.render_md_all(ProjectionMode::Cf), "- a\n\n- b\n");
    }

    #[test]
    fn selected_table_and_html() {
        let s = snap("| a | b |\n| - | - |\n| 1 | 2 |\n");
        let row = node(&s, 2, 3, BlockType::TableRow, 255);
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[row]).unwrap(), "| 1 | 2 |\n");

        let h = snap("<div>\nx\n</div>\n");
        let html = node(&h, 0, 3, BlockType::HtmlBlock, 255);
        assert_eq!(h.render_md_selected(ProjectionMode::Cf, &[html]).unwrap(), "<div>\nx\n</div>\n");
    }

    #[test]
    fn selected_code_fence() {
        let s = snap("~~~rust\nfn x() {}\n~~~\n");
        let fence = node(&s, 0, 3, BlockType::CodeFenceOpen, 255);
        assert_eq!(s.render_md_selected(ProjectionMode::Cf, &[fence]).unwrap(), "~~~rust\nfn x() {}\n~~~\n");
    }


}



// A4 对拍判官(#35):节区间格 == 裸建 section_intervals;树格 == 裸建
// build_attribution_tree(随机文档 × 拓扑)。派生格 = Segment 三件套的
// 纯函数,对拍即格层正确性。

/// WC-13 漂移判官：逐层断言 `Σ channel_slots(该层孩子) == 该层渲染出的 ct 元素数`。
/// 数据来源：内部归属树（`tree_cell`）vs `ast_ch`（= F 帧的 ct 数组）。
/// 另外打印每篇文档 × 模式的渲染哈希（RENDER_HASH），用于修复前后逐字节对比。
#[cfg(test)]
mod wc13_channel_tests {
    use super::channel_slots;
    use crate::projection::State;
    use crate::topology::{ProjectionMode, CONTAINER_TOPOLOGY, HEADINGSECTION_TOPOLOGY};
    use crate::infer::AttrTreeNew;
    use serde_json::Value;


    struct Drift { layer: String, slots: usize, json: usize, detail: String }

    fn walk(snap: &crate::projection::Snapshot, tree: &AttrTreeNew, kids: &[usize],
            elems: &[Value], parent_item: bool, path: &str,
            topo: &crate::topology::Topology, out: &mut Vec<Drift>) {
        let total: usize = kids.iter().map(|&c| channel_slots(snap, tree, c, parent_item)).sum();
        if total != elems.len() {
            let mut detail = String::new();
            for &c in kids {
                let n = &tree.nodes[c];
                detail.push_str(&format!("[idx={c} kind={:?} bt={:?} loose={} slots={} kids={}] ",
                    n.kind, n.block_type, n.loose(),
                    channel_slots(snap, tree, c, parent_item), n.children_of(tree).len()));
            }
            let here = serde_json::to_string(elems).unwrap_or_default();
            let here = &here[..here.len().min(260)];
            out.push(Drift { layer: format!("{path} (parent_item={parent_item})"),
                slots: total, json: elems.len(),
                detail: format!("{detail}| json_here={here}") });
        }
        let mut pos = 0usize;
        for &c in kids {
            let k = channel_slots(snap, tree, c, parent_item);
            if k == 0 { continue; }
            let n = &tree.nodes[c];
            let inner: Vec<usize> = n.children_of(tree).iter().map(|&x| x as usize).collect();
            let is_container = n.kind.is_some() && n.kind != Some(crate::block::FrameKind::HtmlBlock);
            let is_shell_leaf = topo.shell_by_block(n.block_type).is_some();
            if pos < elems.len() && (is_container || is_shell_leaf) {
                // 只对"ct 是数组"的元素递归（TableRow 行壳的 ct 是 cells 字符串——不递归）
                if let Some(sub) = elems[pos].get("ct").and_then(|x| x.as_array()).cloned() {
                    if !inner.is_empty() || !sub.is_empty() {
                        let child_path = format!("{path}->{c}/ct");
                        let pi = n.is_item();
                        walk(snap, tree, &inner, &sub, pi, &child_path, topo, out);
                    }
                } else if !inner.is_empty() {
                    // 行壳（TableRow）的 ct 是 cells 字符串、内部还有记录叶——本卡
                    // **未建模**的形态，只记录不判定（另开卡追）；渲染零变化不依赖它。

                }
            }
            pos += k;
        }
    }

    #[test]
    fn wc13_channel_slot_vs_render_no_drift() {
        let cm = std::fs::read_to_string("../commonmark.md")
            .or_else(|_| std::fs::read_to_string("commonmark.md")).unwrap_or_default();
        let mut docs: Vec<(&str, String)> = vec![
            ("empty_bq", ">\n".into()),
            ("bq_inner_blank", "> a\n>\n> b\n".into()),
            ("list_inner_blank", "- a\n\n- b\n".into()),
            ("section_blank", "# S\n\n\n\ntext\n".into()),
            ("top_blank", "a\n\n\n\nb\n".into()),
            ("tight_item_inline", "- a\n- b\n".into()),
            ("item_fence", "- ```\n  x\n  ```\n".into()),
            ("bq_setext", "> q\n> ===\n".into()),
            ("table", "| a | b |\n|---|---|\n| c | d |\n".into()),
            ("html_block", "<div>\nraw\n</div>\n".into()),
            ("mixed", "# H\n\n- a\n\n  > q\n\n- b\n".into()),
        ];
        if !cm.is_empty() { docs.push(("commonmark.md", cm)); }
        let mut drifts = Vec::new();
        for (name, doc) in &docs {
            for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
                let mut st = State::new(doc);
                let snap = st.snapshot();
                let topo = match mode {
                    ProjectionMode::Cf => &CONTAINER_TOPOLOGY,
                    ProjectionMode::Hs => &HEADINGSECTION_TOPOLOGY,
                    ProjectionMode::Bf => &CONTAINER_TOPOLOGY,
                };
                let frame: serde_json::Value = serde_json::from_str(&snap.to_ast_object_mode(mode))
                    .unwrap_or_else(|e| panic!("F 帧解析失败 {name} {mode:?}: {e}"));
                let json = frame["ct"].clone();

                let cell = snap.tree_cell(topo);
                let tree: &AttrTreeNew = &cell.tree;
                let roots: Vec<usize> = tree.roots.iter().map(|&r| r as usize).collect();
                let elems: Vec<Value> = json.as_array().cloned().unwrap_or_default();
                let root_path = format!("{name}/{mode:?}/root");
                walk(&snap, tree, &roots, &elems, false, &root_path, topo, &mut drifts);
            }
        }
        if !drifts.is_empty() {
            let mut msg = String::new();
            for d in &drifts {
                msg.push_str(&format!("\n  layer={} slots={} json={} {}",
                    d.layer, d.slots, d.json, d.detail));
            }
            panic!("WC-13 通道槽位漂移 {} 层:{}", drifts.len(), msg);
        }
    }

    /// WC-13 往返判官：`walk_ast_path(ast_path_of(node))` 必须解回**同一个节点**
    /// （Tree{node}，或该节点自身的槽位层 Slot{node}）——"生产者（树→路径）与消费者
    /// （路径→树）同一坐标系"的证据；覆盖含 skip 孩子的文档 × 两种拓扑。
    #[test]
    fn wc13_ast_path_roundtrip() {
        let cm = std::fs::read_to_string("../commonmark.md")
            .or_else(|_| std::fs::read_to_string("commonmark.md")).unwrap_or_default();
        let mut docs: Vec<(&str, String)> = vec![
            ("empty_bq", ">\n".into()),
            ("bq_inner_blank", "> a\n>\n> b\n".into()),
            ("list_inner_blank", "- a\n\n- b\n".into()),
            ("section_blank", "# S\n\n\n\ntext\n".into()),
            ("tight_item", "- a\n- b\n".into()),
            ("item_fence", "- ```\n  x\n  ```\n".into()),
            ("table", "| a | b |\n|---|---|\n| c | d |\n".into()),
            ("nested", "# H\n\n- a\n\n  > q\n\n- b\n".into()),
            ("top_blank", "a\n\n\n\nb\n".into()),
            ("bq_setext", "> q\n> ===\n".into()),
        ];
        // commonmark 前 400 行：把覆盖量抬到有统计意义的规模（ast_path_of 每次
        // 重建 LineView，整篇 9810 行会让判官变 O(n²) 级——取片段即可）。
        if !cm.is_empty() {
            let seg: String = cm.lines().take(400).collect::<Vec<_>>().join("\n");
            docs.push(("commonmark_400", seg));
        }
        let mut bad: Vec<String> = Vec::new();
        let mut checked = 0usize;
        for (name, doc) in &docs {
            for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
                let mut st = State::new(doc);
                let snap = st.snapshot();
                let n = snap.line_len();
                let seg = crate::cache::Segment::build(&snap, 0, n);
                let topo = match mode {
                    ProjectionMode::Cf => &CONTAINER_TOPOLOGY,
                    ProjectionMode::Hs => &HEADINGSECTION_TOPOLOGY,
                    ProjectionMode::Bf => &CONTAINER_TOPOLOGY,
                };
                let cell = snap.tree_cell(topo);
                let tree: &AttrTreeNew = &cell.tree;
                for idx in 0..tree.nodes.len() {
                    let Some(comps) = super::ast_path_of(tree, &seg, &snap, idx) else { continue };
                    let w = super::walk_ast_path(&seg, &snap, tree, &comps, 0);
                    match w {
                        Some(super::AdaptWalk::Tree(j)) if j == idx => { checked += 1; }
                        Some(super::AdaptWalk::Slot { node, .. }) if node == idx => { checked += 1; }
                        other => {
                            let desc = match other {
                                Some(super::AdaptWalk::Tree(j)) => format!("Tree({j})"),
                                Some(super::AdaptWalk::Slot { node, .. }) => format!("Slot({node})"),
                                None => "None".to_string(),
                            };
                            bad.push(format!("{name} {mode:?} idx={idx} path={comps:?} -> {desc}"));
                        }
                    }
                }
            }
        }
        assert!(bad.is_empty(), "WC-13 往返不一致 {} 例（前 8）: {:#?}", bad.len(), &bad[..bad.len().min(8)]);
        assert!(checked > 100, "往返判官覆盖过少: {checked}");
    }
}

#[cfg(test)]
mod a4_cell_equiv_tests {
    use crate::projection::State;
    use crate::topology::{CONTAINER_TOPOLOGY, HEADINGSECTION_TOPOLOGY};

    fn next_rand(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state
    }

    fn tokens() -> &'static [&'static str] {
        &[
            "plain paragraph\n",
            "some **bold** text\n",
            "# Heading\n",
            "## Subheading\n",
            "### Deep\n",
            "- item\n",
            "  - nested\n",
            "> quote\n",
            "> # inbq\n",
            "> foo\n> ===\n",
            "```rust\nlet x = 1;\n```\n",
            "\n",
            "foo\n===\n",
            "- a\n\n  cont\n",
            "| a | b |\n|---|---|\n| c | d |\n",
        ]
    }

    fn random_doc(rng: &mut u64, case: u64) -> String {
        let mut doc = String::new();
        let line_count = 8 + (next_rand(rng) % 14) as usize;
        for _ in 0..line_count {
            doc.push_str(tokens()[next_rand(rng) as usize % tokens().len()]);
        }
        let _ = case;
        doc
    }

    /// 节区间格 == 裸建 section_intervals(secs 四元 + header 容器截断
    /// bounds 双产物各自对拍——§9 新增对拍)。
    #[test]
    fn secs_cell_equals_bare_section_intervals() {
        let mut rng: u64 = 0xA4C0_FFEE_0001_0001;
        for case in 0..300u64 {
            let doc = random_doc(&mut rng, case);
            let mut state = State::new(&doc);
            let snap = state.snapshot();
            let n = snap.line_len();
            let cell = snap.section_cell(&HEADINGSECTION_TOPOLOGY);
            let seg = crate::cache::Segment::build(&snap, 0, n);
            let bare = crate::infer::section_intervals(
                &seg.blocks,
                &seg.records,
                &seg.arena,
                HEADINGSECTION_TOPOLOGY.heading_candidates,
                n,
            );
            let cell_secs: Vec<(usize, usize, u8, crate::block::BlockType)> =
                cell.secs_iter().collect();
            assert_eq!(
                cell_secs, bare,
                "case={case}: secs cell must equal bare full-caliber fold\n  doc={doc:?}"
            );
            for b in seg.blocks.iter() {
                if crate::block::is_header(b.block_type) && b.container_ref != u32::MAX {
                    let rec = &seg.records[b.container_ref as usize];
                    let bound = rec
                        .push_lines_of(&seg.arena)
                        .find(|l| *l > b.start)
                        .map(|l| l as usize)
                        .unwrap_or_else(|| rec.end_usize().unwrap_or(n));
                    let got = cell
                        .header_bound(b.start as usize, rec.frame.kind() as u8)
                        .expect("header bound present");
                    assert_eq!(
                        got, bound,
                        "case={case}: header bound @{} must equal full-caliber fold",
                        b.start
                    );
                }
            }
        }
    }

    /// 树格 == 裸建 build_attribution_tree(CF/HS 两拓扑,全量范围)。
    #[test]
    fn tree_cell_equals_bare_tree() {
        let mut rng: u64 = 0xA4C0_FFEE_0001_0002;
        for case in 0..100u64 {
            let doc = random_doc(&mut rng, case);
            let mut state = State::new(&doc);
            let snap = state.snapshot();
            let n = snap.line_len();
            for topo in [&CONTAINER_TOPOLOGY, &HEADINGSECTION_TOPOLOGY] {
                let cell = snap.tree_cell(topo);
                let seg = crate::cache::Segment::build(&snap, 0, n);
                let view = crate::infer::LineView::with_arrays(
                    &seg.records,
                    &seg.blocks,
                    &snap.frame_log.entries,
                    &snap.lines,
                    &snap.rope,
                    (0, n as u32),
                    seg.view.clone(),
                    &seg.arena,
                );
                let bare = crate::infer::build_attribution_tree(
                    &seg.records,
                    &seg.blocks,
                    &view,
                    &seg.arena,
                    (0, n as u32),
                    topo,
                );
                assert!(
                    cell.tree.structurally_eq(&bare),
                    "case={case} topo={}: tree must equal bare build",
                    topo.name
                );
                assert_eq!(
                    cell.tree.roots, bare.roots,
                    "case={case} topo={}: tree roots must equal bare build",
                    topo.name
                );
                assert!(
                    crate::cache::records_structurally_eq(
                        &cell.seg.records,
                        &cell.seg.arena,
                        &seg.records,
                        &seg.arena
                    ),
                    "case={case} topo={}: cell segment records must equal bare build",
                    topo.name
                );
                assert_eq!(
                    cell.seg.blocks, seg.blocks,
                    "case={case} topo={}: cell segment blocks must equal bare build",
                    topo.name
                );
            }
        }
    }
}

#[cfg(test)]
mod incr_tests {
    include!("../tests/unit/incr.rs");
}

#[cfg(test)]
mod a1_guard_tests {
    include!("../tests/unit/cell_a1_guard_equiv.rs");
}

#[cfg(test)]
mod diff_build_equiv_tests {
    include!("../tests/unit/diff_build_equiv.rs");
}
#[cfg(test)]
mod granularity_proj_tests {
    use super::*;
    use crate::projection::State;
    use std::sync::Arc;

    /// 构造单编辑任务(仿 p4b2 样板)。
    fn proj_task(old: &str, s: usize, e: usize, text: &str) -> IncrTask {
        let mut state = State::new(old);
        let snap_old = Arc::new(state.snapshot());
        state.edit(s, e, text);
        let snap = Arc::new(state.snapshot());
        let diff = state.ripple_diff.take().expect("ripple_diff");
        let rls = diff.ripple_line_start;
        let rle = diff.ripple_line_end;
        IncrTask {
            snap, doc_id: 0, snap_old, delta: diff.delta,
            edit_ranges: crate::cache::edit_actions_arc(vec![(s, e)]),
            ripple_line_start: rls, ripple_line_end: rle,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        }
    }

    /// 本地 apply(判官语义同 apply_patch_simple——路径导航 + 各 arm;
    /// ReplaceAttr = 定位对象元素 set 字段)。mod 自足(不依赖 p4b2 mod 私有项)。
    fn apply_local(root: &mut serde_json::Value, p: &Patch) -> bool {
        let (path, val) = match p {
            Patch::Add { path, value } => (path, Some(value)),
            Patch::Remove { path } => (path, None),
            Patch::Replace { path, value } => (path, Some(value)),
            Patch::ReplaceAttr { path, value, .. } => (path, Some(value)),
        };
        if path.is_empty() {
            return matches!(p, Patch::Replace { .. });
        }
        fn ch_arr(cur: &mut serde_json::Value, creating: bool) -> Option<&mut Vec<serde_json::Value>> {
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
        // WC-2（丙）：末段非数字 = **对象成员位**（如 `/ct/0/ct` = 给节点 0 写 ct 成员，
        // 值可为整数组——"建数组"补丁）。剥下成员名后走既有元素链导航，末尾按
        // 成员 set/remove 处理；数字末段维持原数组位语义。
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
        // 成员位：先解值，再对目标节点做成员 set（Add/Replace/ReplaceAttr）
        // 或成员 remove（Remove）。
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
                    Err(_e) => return false,
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
                    match serde_json::from_str(val.unwrap()) {
                        Ok(v) => {
                            arr[idx] = v;
                            true
                        }
                        Err(_e) => false,
                    }
                } else {
                    false
                }
            }
            Patch::ReplaceAttr { attr, .. } => {
                if idx < arr.len() {
                    if let Some(obj) = arr[idx].as_object_mut() {
                        match serde_json::from_str(val.unwrap()) {
                            Ok(v) => {
                                obj.insert(attr.clone(), v);
                                true
                            }
                            Err(_e) => false,
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }

    /// 应用补丁到 old full AST(timestamp 归一),与 new full 比较。
    fn proj_apply_ok(task: &IncrTask, patches: &[Patch]) -> bool {
        let old_root: serde_json::Value = serde_json::from_str(
            &task.snap_old.to_ast_object_mode(crate::topology::ProjectionMode::Cf)).unwrap();
        let new_root: serde_json::Value = serde_json::from_str(
            &task.snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf)).unwrap();
        let mut applied = old_root;
        for p in patches {
            if !apply_local(&mut applied, p) {
                return false;
            }
        }
        if let (Some(a), Some(n)) = (
            applied.as_object_mut().and_then(|o| o.get_mut("timestamp")),
            new_root.get("timestamp"),
        ) {
            *a = n.clone();
        }
        applied == new_root
    }

    #[test]
    fn granularity_proj_face_patches() {
        use crate::topology::Granularity;
        // 场景:(old, edit, new)——键变 / 值文本变 / 顶层段文本变
        let cases: &[(&str, (usize, usize, &str), &str)] = &[
            ("- k: v\n", (2, 3, "k2"), "- k2: v\n"),
            ("- k: v\n", (5, 6, "v2"), "- k: v2\n"),
            ("hello\n", (0, 5, "hey"), "hey\n"),
            ("- a\n", (3, 4, "ab"), "- ab\n"),
        ];
        for &(old, (s, e, txt), _new) in cases {
            let task = proj_task(old, s, e, txt);
            // block 粒度(显式):无 ReplaceAttr
            let (b_p, _) = diff_patches_mode_with_ctx_g(
                &task, &crate::topology::CONTAINER_TOPOLOGY, Granularity::Block);
            assert!(!b_p.iter().any(|p| matches!(p, Patch::ReplaceAttr { .. })),
                "block 粒度不应产 ReplaceAttr: {b_p:?}");
            assert!(proj_apply_ok(&task, &b_p), "block 应用等价失败: {b_p:?}");
            // proj 粒度(显式):单面差 → ReplaceAttr;应用等价
            let (p_p, _) = diff_patches_mode_with_ctx_g(
                &task, &crate::topology::CONTAINER_TOPOLOGY, Granularity::Proj);
            assert!(p_p.iter().any(|p| matches!(p, Patch::ReplaceAttr { .. })),
                "proj 粒度应产 ReplaceAttr: {p_p:?}");
            assert!(proj_apply_ok(&task, &p_p), "proj 应用等价失败: {p_p:?}");
        }
    }

    /// 表驱动字段级(§5.5 二期——容器 Leaf 接入):CF lang/code、IC code、
    /// RefDef url/title 的单字段编辑 → 具体字段的 ReplaceAttr + 应用等价。
    #[test]
    fn granularity_proj_leaf_container_fields() {
        use crate::topology::Granularity;
        // (old, 编辑区间, 期望 attr)——单字段变场景
        let cases: &[(&str, (usize, usize, &str), &str)] = &[
            // CF lang 变 → t
            ("```rust\nlet x = 1;\n```\n", (3, 7, "py"), "t"),
            // CF code 变 → cd
            ("```rust\nlet x = 1;\n```\n", (16, 17, "2"), "cd"),
            // IC 内容变 → cd
            ("    indented\n", (7, 8, "x"), "cd"),
        ];
        for &(old, (s, e, txt), want_attr) in cases {
            let task = proj_task(old, s, e, txt);
            let (p_p, _) = diff_patches_mode_with_ctx_g(
                &task, &crate::topology::CONTAINER_TOPOLOGY, Granularity::Proj);
            let has = p_p.iter().any(|p| {
                matches!(p, Patch::ReplaceAttr { attr, .. } if attr == want_attr)
            });
            assert!(has,
                "场景 {old:?} 编辑({s},{e},{txt:?}) 应产 ReplaceAttr attr={want_attr}: {p_p:?}");
            assert!(proj_apply_ok(&task, &p_p), "proj 应用等价失败: {p_p:?}");
        }
        // RefDef 不字段化(§11.4 裁决:block 层承载的 inline 引用定义,
        // u/ti 是 RefMap 键值非渲染 attr)——编辑回退整块 REPL:
        // 无 ReplaceAttr + 应用等价。
        // WC-4：清空 / 形态切换都应是**一条**字段写，且值是裸 JSON（字符串或数组），
        // 应用后等于 full。
        let wc4: &[(&str, (usize, usize, &str), &str, &str)] = &[
            // 围栏代码内容清空 ⇒ ReplaceAttr{attr:"cd", value:""}
            ("```rust\nlet x = 1;\n```\n", (8, 19, ""), "cd", "\"\""),
            // 标题文字清空 ⇒ ReplaceAttr{attr:"t", value:""}
            ("# Title\n", (2, 7, ""), "t", "\"\""),
            // 标题文字由纯文本变行内 ⇒ 值是**数组**（裸 JSON）
            ("# plain\n", (2, 7, "**b**"), "t", "["),
            // 反向：数组变回字符串 ⇒ 值是字符串
            ("# **b**\n", (2, 7, "plain"), "t", "\""),
            // 含标记标题改名 ⇒ 仍是**一条**字段写（数组整体替换，不下钻元素）
            ("# **b** c\n", (8, 9, "d"), "t", "["),
        ];
        for &(old, (s, e, txt), want_attr, want_prefix) in wc4 {
            let task = proj_task(old, s, e, txt);
            let (p_p, _) = diff_patches_mode_with_ctx_g(
                &task, &crate::topology::CONTAINER_TOPOLOGY, Granularity::Proj);
            assert_eq!(p_p.len(), 1, "手工用例应恰好一条 op（清空/改名）: {p_p:?}");
            let attrs: Vec<&Patch> = p_p
                .iter()
                .filter(|p| matches!(p, Patch::ReplaceAttr { attr, .. } if attr == want_attr))
                .collect();
            assert_eq!(attrs.len(), 1,
                "场景 {old:?} 编辑({s},{e},{txt:?}) 应恰好一条 ReplaceAttr attr={want_attr}: {p_p:?}");
            if let Patch::ReplaceAttr { value, .. } = attrs[0] {
                assert!(value.starts_with(want_prefix),
                    "值应是裸 JSON（前缀 {want_prefix:?}）: {value}");
                let _: serde_json::Value = serde_json::from_str(value)
                    .unwrap_or_else(|e| panic!("值不是合法 JSON: {value} ({e})"));
            }
            assert!(proj_apply_ok(&task, &p_p), "proj 应用等价失败: {p_p:?}");
        }
        for old in ["[foo]: /url \"ti\"\n\n[foo]\n"] {
            let task = proj_task(old, 7, 11, "x");
            let (p_p, _) = diff_patches_mode_with_ctx_g(
                &task, &crate::topology::CONTAINER_TOPOLOGY, Granularity::Proj);
            assert!(!p_p.iter().any(|p| matches!(p, Patch::ReplaceAttr { .. })),
                "refdef 编辑不应产 ReplaceAttr: {p_p:?}");
            assert!(proj_apply_ok(&task, &p_p), "proj 应用等价失败: {p_p:?}");
        }
    }
}


#[cfg(test)]
mod p4b2_kernel_zone_tests {
    use super::*;
    use crate::cache::{CacheNode, CacheSlot};
    use crate::projection::State;
    use crate::topology::{CONTAINER_TOPOLOGY, HEADINGSECTION_TOPOLOGY};

    fn next_rand(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
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

    fn tokens() -> &'static [&'static str] {
        &[
            // ── 单行 token(原 20 项——基础形态) ──
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
            // ── 多行块 token(unit/incr.rs 块语料方法论——单行随机无法生成
            //    setext/嵌套块级组合——穷举级覆盖) ──
            "a\n",
            "para\ncont\n",
            "foo\n===\n",
            "foo\n---\n",
            "> foo\n> ===\n",
            "> foo\n> ---\n",
            "- foo\n  ===\n",
            "- # h\n  text\n",
            "# h\ntext\n",
            "## h\n> q\n",
            "| a |\n| - |\n",
            "> - a\n> ===\n",
            "- a\n\n  cont\n",
            "1. a\n2. b\n",
            "---\n",
            "[a]: /u\n",
            "> # h\n> - x\n> ===\n",
        ]
    }

    #[test]
    fn kernel_zone_containment_probe_300() {
        let mut rng: u64 = 0x243F_6A88_85A3_08D3;
        let mut both_exact = 0usize;
        let mut new_only = 0usize;
        let mut old_only = 0usize;
        let mut neither = 0usize;
        let mut containment_failures = 0usize;

        for case in 0..300u64 {
            let mut doc = String::new();
            let line_count = (next_rand(&mut rng) % 10) as usize;
            for _ in 0..line_count {
                doc.push_str(tokens()[next_rand(&mut rng) as usize % tokens().len()]);
            }

            let mut state = State::new(&doc);
            let old_snap = Arc::new(state.snapshot());
            let (start, end, text) = random_edit(&mut rng, &doc, tokens());
            state.edit(start, end, &text);
            let new_snap = Arc::new(state.snapshot());
            let diff = state
                .ripple_diff
                .take()
                .expect("edit should produce ripple diff");

            let n_new = new_snap.line_len();
            let rls = diff.ripple_line_start;
            let rle_e = (diff.ripple_line_end + 1).min(n_new);
            // Production anchor: old-side line of the first edited byte.
            // IncrTask.edit_ranges must carry the OLD deletion span so
            // compute_zone sees removed old structure.
            let anchor_old = old_snap.line_of_byte(start);
            let incr_edit_range = (start, end);
            // P1 描述符(seam 同款):edit_ranges = IncrTask 同款旧侧跨度。
            let f = new_snap.line_of_byte(start.min(new_snap.byte_len().saturating_sub(1)));
            let edit = CacheNode::new_edit_full(
                &old_snap,
                new_snap.clone(),
                (rls, rle_e),
                crate::cache::edit_actions_arc(vec![incr_edit_range]),
                diff.delta,
                anchor_old,
            );

            let task = IncrTask {
                snap: new_snap.clone(),
                doc_id: 0,
                snap_old: old_snap.clone(),
                delta: diff.delta,
                edit_ranges: crate::cache::edit_actions_arc(vec![incr_edit_range]),
                ripple_line_start: rls,
                ripple_line_end: diff.ripple_line_end,
                slot: None, slot_old: None, reaper_tx: None, structural_change: false,
            };
            let line_map = LineMap::new(&task);
            let zone = compute_zone(&task, &line_map);

            let (kzs, kce) = edit.materialize_zone();
            let (kzso, kceo) = edit
                .materialize_old_zone()
                .expect("Edit node must have an old zone");

            let old_exact = (kzso, kceo) == (zone.zs_o, zone.ce_o);
            let new_exact = (kzs, kce) == (zone.zs_n, zone.ce_n);
            match (old_exact, new_exact) {
                (true, true) => both_exact += 1,
                (false, true) => new_only += 1,
                (true, false) => old_only += 1,
                (false, false) => neither += 1,
            }

            let contained = kzso <= zone.zs_o
                && kceo >= zone.ce_o
                && kzs <= zone.zs_n
                && kce >= zone.ce_n;
            if !contained {
                containment_failures += 1;
            }
        }

        let _sp = crate::telemetry_span!(
            "kernel_zone_containment",
            "n" => 300u64,
            "exact_both" => both_exact as u64,
            "new_only" => new_only as u64,
            "old_only" => old_only as u64,
            "neither" => neither as u64,
            "containment_failures" => containment_failures as u64
        );
        // Containment vs `compute_zone` is recorded for diagnosis only:
        // `compute_zone` deliberately over-approximates through edit_ranges
        // and zs alignment, so a kernel RCZ may be tighter yet still contain
        // every changed L1 object. The patch-equivalence gate is the actual
        // correctness authority for the kernel RCZ.
        //
        // 本探针**不是判官**，也不得被当作「两口径是否分叉」的仪器：
        //   ① 它只比较 (zs, ce) 两组锚，**从不读 cs**——而权威与物化的组装差异
        //      （stage2 `expand_zone_cs_blocks_fl`）正发生在 cs；
        //   ② 它的 containment 指标与「compute_zone 故意过近似」相冲，失败不代表 bug。
        //   ⇒ 想把「两口径是否分叉」变成事实，必须先让物化侧暴露 cs（属改生产的范围）。
        //   仪器的可见量：zs/ce（双侧）；不可见量：cs、组装次数、块级扩展。
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
        fn ch_arr<'a>(
            cur: &'a mut serde_json::Value,
            creating: bool,
        ) -> Option<&'a mut Vec<serde_json::Value>> {
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
        // WC-2（丙）：末段非数字 = **对象成员位**（如 `/ct/0/ct` = 给节点 0 写 ct 成员，
        // 值可为整数组——"建数组"补丁）。剥下成员名后走既有元素链导航，末尾按
        // 成员 set/remove 处理；数字末段维持原数组位语义。
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
                ["ct"] => {
                    // Root-envelope path: the final array lives on `cur`.
                    continue;
                }
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
        // 成员位：先解值，再对目标节点做成员 set（Add/Replace/ReplaceAttr）
        // 或成员 remove（Remove）。
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
                // Python 侧生产 apply 用 list.insert 语义:越界 index 追加
                // 到末尾(diff 的 Add 索引用 new 坐标,old 数组更短时越界
                // 是合法的"末尾追加")——测试工具与之一致。
                let parsed: serde_json::Value = match serde_json::from_str(val.unwrap()) {
                    Ok(v) => v,
                    Err(_e) => {
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
                // 面级:定位元素对象后 set 单字段(元素须为对象;path 与
                // REPL 同 = 元素内容流坐标)。
                if idx < arr.len() {
                    let el = &mut arr[idx];
                    if let Some(obj) = el.as_object_mut() {
                        match serde_json::from_str(val.unwrap()) {
                            Ok(v) => {
                                obj.insert(attr.clone(), v);
                                true
                            }
                            Err(_e) => false,
                        }
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }

    #[test]
    fn randomized_patch_equivalence_gate_300() {
        let mut rng: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut kernel_hits = 0usize;
        let mut cases = 0usize;

        // PATCH-GATE:36 tokens(20 单行 + 16 多行块)·3000 例(升级自 300——
        // 多行块语料覆盖 setext/嵌套块级组合)。
        for case in 0..3000u64 {
            let mut doc = String::new();
            let line_count = (next_rand(&mut rng) % 10) as usize;
            for _ in 0..line_count {
                doc.push_str(tokens()[next_rand(&mut rng) as usize % tokens().len()]);
            }

            let mut state = State::new(&doc);
            let old_snap = Arc::new(state.snapshot());
            let (start, end, text) = random_edit(&mut rng, &doc, tokens());
            state.edit(start, end, &text);
            let new_snap = Arc::new(state.snapshot());
            let diff = state
                .ripple_diff
                .take()
                .expect("edit should produce ripple diff");

            let n_new = new_snap.line_len();
            let rls = diff.ripple_line_start;
            let rle_e = (diff.ripple_line_end + 1).min(n_new);
            // Production anchor: old-side line of the first edited byte.
            // IncrTask.edit_ranges carry the OLD deletion span.
            let anchor_old = old_snap.line_of_byte(start);
            let incr_edit_range = (start, end);
            // P1 描述符(seam 同款)。
            let f = new_snap.line_of_byte(start.min(new_snap.byte_len().saturating_sub(1)));
            let edit = CacheNode::new_edit_full(
                &old_snap,
                new_snap.clone(),
                (rls, rle_e),
                crate::cache::edit_actions_arc(vec![incr_edit_range]),
                diff.delta,
                anchor_old,
            );
            let slot = Arc::new(CacheSlot::new());
            slot.node.store(Some(Arc::new(edit.clone())));

            let task_none = IncrTask {
                snap: new_snap.clone(),
                doc_id: 0,
                snap_old: old_snap.clone(),
                delta: diff.delta,
                edit_ranges: crate::cache::edit_actions_arc(vec![incr_edit_range]),
                ripple_line_start: rls,
                ripple_line_end: diff.ripple_line_end,
                slot: None, slot_old: None, reaper_tx: None, structural_change: false,
            };
            let task_slot = IncrTask {
                snap: new_snap.clone(),
                doc_id: 0,
                snap_old: old_snap.clone(),
                delta: diff.delta,
                edit_ranges: crate::cache::edit_actions_arc(vec![incr_edit_range]),
                ripple_line_start: rls,
                ripple_line_end: diff.ripple_line_end,
                slot: Some(crate::cache::CacheSlotHandle(slot.clone())), slot_old: None, reaper_tx: None, structural_change: false,
            };

            let patches_none = diff_patches_mode(&task_none, &CONTAINER_TOPOLOGY);
            let patches_slot = diff_patches_mode(&task_slot, &CONTAINER_TOPOLOGY);

            // Apply both patch streams to the OLD full AST and require the NEW
            // full AST. This is the real RCZ gate: kernel RCZ may be tighter
            // than compute_zone, but it must still contain every changed L1
            // object.
            let old_root: serde_json::Value = serde_json::from_str(
                &old_snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf),
            )
            .unwrap();
            let new_root: serde_json::Value = serde_json::from_str(
                &new_snap.to_ast_object_mode(crate::topology::ProjectionMode::Cf),
            )
            .unwrap();
            let mut applied = old_root.clone();
            for p in &patches_slot {
                if !apply_patch_simple(&mut applied, p) {
                    panic!("apply failed case={case} patch={p:?}");
                }
            }
            // Root envelopes carry a generation-time epoch-second timestamp;
            // patch application preserves the old timestamp. Normalize before
            // comparing so a second boundary cannot make the gate flaky.
            if let (Some(a), Some(n)) = (
                applied.as_object_mut().and_then(|o| o.get_mut("timestamp")),
                new_root.get("timestamp"),
            ) {
                *a = n.clone();
            }
            if applied != new_root {
                panic!(
                    "kernel patch does not reach full AST case={case} doc:\n{doc}\nfinal:\n{}",
                    state.rope.to_string()
                );
            }
            // 双粒度(proj 面级):同事实 diff 的 ReplaceAttr 输出必须同样达
            // full(应用等价——面级回退 REPL 安全由随机语料验证)。
            {
                let (patches_proj, _) = diff_patches_mode_with_ctx_g(
                    &task_slot, &CONTAINER_TOPOLOGY,
                    crate::topology::Granularity::Proj);
                let mut applied_p = old_root.clone();
                for p in &patches_proj {
                    if !apply_patch_simple(&mut applied_p, p) {
                        panic!("proj apply failed case={case} patch={p:?}");
                    }
                }
                if let (Some(a), Some(n)) = (
                    applied_p.as_object_mut().and_then(|o| o.get_mut("timestamp")),
                    new_root.get("timestamp"),
                ) {
                    *a = n.clone();
                }
                if applied_p != new_root {
                    panic!(
                        "proj kernel patch does not reach full AST case={case} doc:\n{doc}\nfinal:\n{}\nproj={patches_proj:?}\nblock={patches_slot:?}",
                        state.rope.to_string()
                    );
                }
            }
            if let Some(kp) = kernel_diff_parts_override(&task_slot, &CONTAINER_TOPOLOGY) {
                if kp.override_zone && kp.old.is_some() && kp.new.is_some() {
                    kernel_hits += 1;
                }
            }
            cases += 1;
        }

        let _sp = crate::telemetry_span!("patch_gate_cf", "n" => cases as u64, "kernel_hits" => kernel_hits as u64);
        assert_eq!(
            kernel_hits, cases,
            "every CF case with an Edit slot must use kernel RCZ"
        );

        // HS 恒回退权威 compute_zone(kernel override 只对 CF)。
        // 两场景:① 4 行 benign(旧假绿 case——CF 界恰好 ⊇ 章节);② 长章节
        // 标题编辑(CF 界 ⊊ 章节区间——修复前 slot 的 Replace 值被 CF 界段
        // 截断,para 1..29 静默丢失——实测反例)。
        for (name, pos, len, rep) in [
            ("benign", 6usize, 1usize, "X"),
            ("long-section", 0usize, 3usize, "# AA"),
        ] {
            let mut state = if name == "long-section" {
                let mut doc = String::from("# A\n\n");
                for i in 0..30 {
                    doc.push_str(&format!("para {i}\n\n"));
                }
                doc.push_str("# B\n");
                State::new(&doc)
            } else {
                State::new("# A\n\nBody\n# B\n")
            };
            let old_snap = Arc::new(state.snapshot());
            state.edit(pos, pos + len, rep);
            let new_snap = Arc::new(state.snapshot());
            let diff = state
                .ripple_diff
                .take()
                .expect("edit should produce ripple diff");
            let n_new = new_snap.line_len();
            let rls = diff.ripple_line_start;
            let rle_e = (diff.ripple_line_end + 1).min(n_new);
            // Production anchor/range (首编辑起点字节在旧快照上的行)。
            let anchor_old = old_snap.line_of_byte(pos);
            let incr_edit_range = (pos, pos + len);
            let edit = CacheNode::new_edit_full(
                &old_snap,
                new_snap.clone(),
                (rls, rle_e),
                crate::cache::edit_actions_arc(vec![incr_edit_range]),
                diff.delta,
                anchor_old,
            );
            let slot = Arc::new(CacheSlot::new());
            slot.node.store(Some(Arc::new(edit)));

            let task_none = IncrTask {
                snap: new_snap.clone(),
                doc_id: 0,
                snap_old: old_snap.clone(),
                delta: diff.delta,
                edit_ranges: crate::cache::edit_actions_arc(vec![incr_edit_range]),
                ripple_line_start: rls,
                ripple_line_end: diff.ripple_line_end,
                slot: None, slot_old: None, reaper_tx: None, structural_change: false,
            };
            let task_slot = IncrTask {
                snap: new_snap.clone(),
                doc_id: 0,
                snap_old: old_snap.clone(),
                delta: diff.delta,
                edit_ranges: crate::cache::edit_actions_arc(vec![incr_edit_range]),
                ripple_line_start: rls,
                ripple_line_end: diff.ripple_line_end,
                slot: Some(crate::cache::CacheSlotHandle(slot)), slot_old: None, reaper_tx: None, structural_change: false,
            };
            let patches_none = hs_patches(&task_none);
            let patches_slot = hs_patches(&task_slot);
            assert_eq!(
                patches_none, patches_slot,
                "{name}: HS output must not depend on slot"
            );
        }
    }
}


// 适配层判官（incr_unify_826 阶段 2/3 门）——lib 级对拍。
// 口径：apply 后 AST 逐字节相等（判官先行——不逐 patch 比对，形态分歧合法）。


#[cfg(test)]
mod unify_adapt_tests {
    use super::*;
    use crate::projection::State;
    use crate::topology::{CONTAINER_TOPOLOGY, HEADINGSECTION_TOPOLOGY, ProjectionMode};

    fn build_doc(sec_lines: usize, n_secs: usize) -> (String, Vec<(usize, usize)>, Vec<(usize, String)>) {
        let mut s = String::new();
        let mut paras: Vec<(usize, usize)> = Vec::new();
        let mut heads: Vec<(usize, String)> = Vec::new();
        for sec in 0..n_secs {
            let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
            let text = format!("# Section {sec}");
            s.push_str(&text);
            s.push_str("\n\n");
            heads.push((li, text));
            for i in 0..sec_lines {
                let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                let xyz = s.len() + format!("par {sec}-{i} ").len();
                s.push_str(&format!("par {sec}-{i} xyz\n\n"));
                paras.push((li, xyz));
            }
        }
        (s, paras, heads)
    }

    /// 场景构造（G3 用例矩阵扩展）：文档形态 × 编辑类型。
    /// 形态: "flat"(根级节) / "nested"(Top + ## Sub) / "setext"(=== 标题)。
    /// 编辑类型在原 kinds 上追加 "hlevel"(标题改 level：`# → ##`)。
    fn make_case(kind: &str, shape: &str) -> Option<(Arc<Snapshot>, Arc<Snapshot>, IncrTask)> {
        let (doc, paras, heads) = match shape {
            "nested" => {
                // "# Top" + 25 × ("## Sub {j}" + 40 段)
                let mut s = String::new();
                let mut paras = Vec::new();
                let mut heads = Vec::new();
                s.push_str("# Top\n\n");
                for j in 0..25usize {
                    let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                    let text = format!("## Sub {j}");
                    s.push_str(&text);
                    s.push_str("\n\n");
                    heads.push((li, text));
                    for i in 0..40usize {
                        let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                        let xyz = s.len() + format!("par s{j}-{i} ").len();
                        s.push_str(&format!("par s{j}-{i} xyz\n\n"));
                        paras.push((li, xyz));
                    }
                }
                (s, paras, heads)
            }
            "setext" => {
                // setext 标题: "Section {j}\n===\n\n" + 段——块级 HsHead1。
                let mut s = String::new();
                let mut paras = Vec::new();
                let mut heads = Vec::new();
                for j in 0..10usize {
                    let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                    let text = format!("Section {j}");
                    s.push_str(&text);
                    s.push_str("\n===\n\n");
                    heads.push((li, text));
                    for i in 0..20usize {
                        let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                        let xyz = s.len() + format!("par {j}-{i} ").len();
                        s.push_str(&format!("par {j}-{i} xyz\n\n"));
                        paras.push((li, xyz));
                    }
                }
                (s, paras, heads)
            }
            // B1 预实验形态：多行容器跨节域密封性验证。
            // ① fence_cross：fence 吞掉"节标题行"(CodeContent) → 树级无该节，
            //   根级节域 = [S0, S2)——fence 完整含于域内——壳值渲染不截。
            // ② bq_inner_head：BQ 内标题(容器内节) + 同级标题——节域天然密封
            //   (容器内节的边界 = 容器内下一同级/容器尾, 不切容器)。
            "fence_cross" => {
                let mut s = String::new();
                let mut paras = Vec::new();
                let mut heads = Vec::new();
                s.push_str("# Section 0\n\n");
                heads.push((0, "# Section 0".to_string()));
                for i in 0..8usize {
                    let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                    let xyz = s.len() + format!("par 0-{i} ").len();
                    s.push_str(&format!("par 0-{i} xyz\n\n"));
                    paras.push((li, xyz));
                }
                // fence 内含 "# Section 1"(CodeContent——不构成节边界)。
                s.push_str("```\ncode a\n# Section 1\ncode b\n```\n\n");
                heads.push(((s.bytes().filter(|&b| b == b'\n').count()) as usize,
                    "# Section 2".to_string()));
                s.push_str("# Section 2\n\n");
                for i in 0..8usize {
                    let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                    let xyz = s.len() + format!("par 2-{i} ").len();
                    s.push_str(&format!("par 2-{i} xyz\n\n"));
                    paras.push((li, xyz));
                }
                (s, paras, heads)
            }
            "bq_inner_head" => {
                // BQ 内标题(keep_atoms 容器内节)："> # Inner\n> q line" +
                // 根级节。标题编辑瞄准 BQ 内标题与根级标题两种。
                let mut s = String::new();
                let mut paras = Vec::new();
                let mut heads = Vec::new();
                s.push_str("# Top\n\n");
                heads.push((0, "# Top".to_string()));
                s.push_str("> quote line\n");
                let bq_head_li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                let bq_head_text = "> # Inner".to_string();
                s.push_str("> # Inner\n> inner text\n\n");
                heads.push((bq_head_li, bq_head_text));
                s.push_str("# After\n\n");
                heads.push(((s.bytes().filter(|&b| b == b'\n').count()) as usize,
                    "# After".to_string()));
                for i in 0..6usize {
                    let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                    let xyz = s.len() + format!("par a-{i} ").len();
                    s.push_str(&format!("par a-{i} xyz\n\n"));
                    paras.push((li, xyz));
                }
                (s, paras, heads)
            }
            // B2 预实验形态：tight item 多槽位(#301)——AST 序 walk 的
            // wrap/inline_roots 偏移段覆盖。
            "tight" => {
                let mut s = String::new();
                let mut paras = Vec::new();
                let mut heads = Vec::new();
                s.push_str("# Section 0\n\n");
                heads.push((0, "# Section 0".to_string()));
                s.push_str("- *em* and *strong* text\n- plain item\n\n");
                for i in 0..6usize {
                    let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                    let xyz = s.len() + format!("par 0-{i} ").len();
                    s.push_str(&format!("par 0-{i} xyz\n\n"));
                    paras.push((li, xyz));
                }
                s.push_str("# Section 1\n\n");
                heads.push(((s.bytes().filter(|&b| b == b'\n').count()) as usize,
                    "# Section 1".to_string()));
                for i in 0..6usize {
                    let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                    let xyz = s.len() + format!("par 1-{i} ").len();
                    s.push_str(&format!("par 1-{i} xyz\n\n"));
                    paras.push((li, xyz));
                }
                (s, paras, heads)
            }
            // B4 预实验形态：三层嵌套(# Top / ## Mid / ### Leaf)。
            "deep3" => {
                let mut s = String::new();
                let mut paras = Vec::new();
                let mut heads = Vec::new();
                s.push_str("# Top\n\n");
                heads.push((0, "# Top".to_string()));
                for m in 0..4usize {
                    let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                    let text = format!("## Mid {m}");
                    s.push_str(&text);
                    s.push_str("\n\n");
                    heads.push((li, text));
                    for l in 0..3usize {
                        let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                        let text = format!("### Leaf {m}-{l}");
                        s.push_str(&text);
                        s.push_str("\n\n");
                        heads.push((li, text));
                        for i in 0..4usize {
                            let li = (s.bytes().filter(|&b| b == b'\n').count()) as usize;
                            let xyz = s.len() + format!("par {m}-{l}-{i} ").len();
                            s.push_str(&format!("par {m}-{l}-{i} xyz\n\n"));
                            paras.push((li, xyz));
                        }
                    }
                }
                (s, paras, heads)
            }
            // B4 预实验形态：大文档(50 节 × 50 段 ≈ 5000 行)。
            "bigflat" => {
                build_doc(50, 50)
            }
            _ => {
                let (doc, paras, heads) = build_doc(20, 10);
                (doc, paras, heads)
            }
        };
        let target = paras.len() / 2;
        let (li, xyz) = paras[target];
        let (hli, htext) = heads.iter().rev().find(|(l, _)| *l <= li).cloned()
            .unwrap_or_else(|| (0usize, "# Section 0".to_string()));
        let hoff = doc.lines().take(hli).map(|l| l.len() + 1).sum::<usize>();
        let mut state = State::new(&doc);
        let old_snap = Arc::new(state.snapshot());
        let (start, end, text) = match kind {
            "inline" => (xyz, xyz + 3, "XYZ".to_string()),
            "struct" => {
                let s = paras.iter().find(|(l, _)| *l >= li).map(|(l, _)| *l).unwrap_or(0);
                let off = doc.lines().take(s).map(|l| l.len() + 1).sum::<usize>();
                (off, off, "> quote new\n".to_string())
            }
            "heading" => (hoff, hoff + htext.len(), format!("{htext}X")),
            "hlevel" => {
                // 标题改 level：`# X` → `## X`（`===\n` 形态改 `---` 无法单行
                // 替换——只测 ATX 标题的 level 变化）。
                if htext.starts_with('#') {
                    (hoff, hoff + htext.len(), format!("#{htext}"))
                } else {
                    return None; // setext 形态跳过（level 改走 heading 文本变）。
                }
            }
            "hdel" => {
                // setext 标题块 = 文本行 + "===\n\n"（5 字节）——整块删除；
                // ATX 单行。
                let extra = if shape == "setext" { htext.len() + 5 } else { htext.len() };
                (hoff, hoff + extra, String::new())
            }
            "hins" => {
                let s = paras.iter().find(|(l, _)| *l >= li).map(|(l, _)| *l).unwrap_or(0);
                let off = doc.lines().take(s).map(|l| l.len() + 1).sum::<usize>();
                (off, off, "# NewSect\n\n".to_string())
            }
            "combo" => {
                // B3 预实验：单笔编辑覆盖标题 + 相邻段(标题变 + 内容变)。
                let p_off = paras.iter().find(|(l, _)| *l >= li).map(|(_, o)| *o).unwrap_or(xyz);
                let seg = format!("{htext}X\n\npar combo changed");
                (hoff, p_off + 3, seg)
            }
            _ => {
                // cross：跨节删除（目标段到下一节第一段 xyz+3）。
                let nxt = paras[target + 20];
                (xyz, nxt.1 + 3, String::new())
            }
        };
        state.edit(start, end, &text);
        let new_snap = Arc::new(state.snapshot());
        let diff = state.ripple_diff.take()?;
        let task = IncrTask {
            snap: new_snap.clone(), doc_id: 0, snap_old: old_snap.clone(),
            delta: diff.delta,
            edit_ranges: crate::cache::edit_actions_arc(vec![(start, end)]),
            ripple_line_start: diff.ripple_line_start,
            ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        Some((old_snap, new_snap, task))
    }

    /// apply（判官语义——与 p4b2 的 apply_patch_simple 同律：路径段
    /// ["ct", idx] = 进数组取子；孤 ["ct"] = 取当前节点的 ch 字段标记）。
    fn apply_patch_judge(root: &mut serde_json::Value, p: &Patch) -> bool {
        let (path, val) = match p {
            Patch::Add { path, value } => (path, Some(value)),
            Patch::Remove { path } => (path, None),
            Patch::Replace { path, value } => (path, Some(value)),
            Patch::ReplaceAttr { path, value, .. } => (path, Some(value)),
        };
        if path.is_empty() {
            return matches!(p, Patch::Replace { .. });
        }
        fn ch_arr<'a>(
            cur: &'a mut serde_json::Value,
            creating: bool,
        ) -> Option<&'a mut Vec<serde_json::Value>> {
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
        // WC-2（丙）：末段非数字 = **对象成员位**（如 `/ct/0/ct` = 给节点 0 写 ct 成员，
        // 值可为整数组——"建数组"补丁）。剥下成员名后走既有元素链导航，末尾按
        // 成员 set/remove 处理；数字末段维持原数组位语义。
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
        // 成员位：先解值，再对目标节点做成员 set（Add/Replace/ReplaceAttr）
        // 或成员 remove（Remove）。
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
            Patch::Remove { .. } => {
                if idx < arr.len() { arr.remove(idx); true } else { false }
            }
            Patch::Add { value, .. } => match serde_json::from_str::<serde_json::Value>(value) {
                Ok(v) => { if idx <= arr.len() { arr.insert(idx, v); true } else { false } }
                Err(_) => false,
            },
            Patch::Replace { value, .. } => match serde_json::from_str::<serde_json::Value>(value) {
                Ok(v) => { if idx < arr.len() { arr[idx] = v; true } else { false } }
                Err(_) => false,
            },
            Patch::ReplaceAttr { attr, value, .. } => {
                match serde_json::from_str::<serde_json::Value>(value) {
                    Ok(v) => {
                        if idx < arr.len() {
                            if let Some(obj) = arr[idx].as_object_mut() {
                                obj.insert(attr.clone(), v);
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                    Err(_) => false,
                }
            }
        }
    }

    fn apply_all(root: &mut serde_json::Value, patches: &[Patch]) -> bool {
        patches.iter().all(|p| apply_patch_judge(root, p))
    }

    fn strip_ts(mut v: serde_json::Value) -> serde_json::Value {
        if let Some(o) = v.as_object_mut() { o.remove("timestamp"); }
        v
    }

    #[test]
    fn adapt_cf_identity_and_walk_ok() {
        for kind in ["inline", "struct", "heading", "hdel", "hins"] {
            let Some((_o, _n, task)) = make_case(kind, "flat") else { continue };
            let (patches, ctx) = diff_patches_mode_with_ctx(&task, &CONTAINER_TOPOLOGY);
            // walk 全 patch 零失败（阶段 2 门）。
            for p in &patches {
                let path = match p {
                    Patch::Add { path, .. }
                    | Patch::Remove { path }
                    | Patch::Replace { path, .. }
                    | Patch::ReplaceAttr { path, .. } => path,
                };
                let segs = parse_path_like(path);
                let (seg, snap, tree, off) = match p {
                    Patch::Remove { .. } => (&ctx.o_seg, &ctx.snap_old, &ctx.o_tree, ctx.off_old),
                    _ => (&ctx.n_seg, &ctx.snap_new, &ctx.n_tree, ctx.off_new),
                };
                assert!(walk_ast_path(seg, snap, tree, &segs, off).is_some(),
                    "{kind}: walk failed on {path}");
            }
            // 恒等：adapt(CF) ≡ 输入。
            let adapted = adapt(&task, &patches, &ctx, &CONTAINER_TOPOLOGY);
            assert_eq!(adapted, patches, "{kind}: adapt(CF) not identity");
        }
    }

    #[test]
    fn adapt_hs_roundtrip() {
        // G3 判官矩阵（军令状 §5 阶段 3）：形态 × 编辑类型。
        // 形态：flat（根级节）/ nested（嵌套节）/ setext（=== 标题）/
        // fence_cross（B1 多行容器跨节域）/ bq_inner_head（B1 容器内节）/
        // tight（B2 槽位）/ deep3+bigflat（B4 深度与规模）；
        // 类型：inline/struct/heading/hlevel（改 level）/hdel/hins/cross（flat 专属）/
        // combo（B3 标题+内容同笔）。
        let shapes: &[(&str, &[&str])] = &[
            ("flat", &["inline", "struct", "heading", "hlevel", "hdel", "hins", "cross", "combo"]),
            ("nested", &["inline", "struct", "heading", "hlevel", "hdel", "hins", "combo"]),
            ("setext", &["inline", "struct", "heading", "hdel", "hins", "combo"]),
            ("fence_cross", &["inline", "heading", "hins", "hdel"]),
            ("bq_inner_head", &["heading", "hins", "hdel"]),
            ("tight", &["inline", "heading", "hins", "combo"]),
            ("deep3", &["heading", "hdel", "hins", "combo"]),
            ("bigflat", &["heading", "hins", "cross", "combo"]),
        ];
        for &(shape, kinds) in shapes {
            for &kind in kinds {
                let Some((old_snap, new_snap, task)) = make_case(kind, shape) else { continue };
                let base = old_snap.to_ast_object_mode(ProjectionMode::Hs);
                let target = new_snap.to_ast_object_mode(ProjectionMode::Hs);
                let target_v = strip_ts(serde_json::from_str(&target).unwrap());

                // 生产适配路径。
                let hs = hs_patches(&task);
                let mut applied = strip_ts(serde_json::from_str(&base).unwrap());
                let ok = apply_all(&mut applied, &hs);
                assert!(ok, "{shape}/{kind}: adapt(HS) apply failed");

                assert_eq!(applied, target_v, "{shape}/{kind}: adapt(HS) != target");
            }
        }
        // HS-direct 对拍的对照组使命完成（缺陷已实证：
        // flat hins/cross、nested 全形态、setext hdel——适配层路径均正确）。
    }

    #[test]
    fn hs_proj_roundtrip_matrix() {
        let shapes: &[(&str, &[&str])] = &[
            ("flat", &["inline", "heading", "hlevel", "hdel", "hins"]),
            ("nested", &["inline", "heading", "hlevel", "hdel", "hins"]),
            ("setext", &["inline", "heading", "hdel", "hins"]),
        ];
        for &(shape, kinds) in shapes {
            for &kind in kinds {
                let Some((old_snap, new_snap, task)) = make_case(kind, shape) else { continue };
                let base = old_snap.to_ast_object_mode(ProjectionMode::Hs);
                let target = new_snap.to_ast_object_mode(ProjectionMode::Hs);
                let (facts, ctx) = diff_patches_mode_with_ctx_g(
                    &task, &CONTAINER_TOPOLOGY, crate::topology::Granularity::Proj,
                );
                let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);
                let mut applied = strip_ts(serde_json::from_str(&base).unwrap());
                assert!(apply_all(&mut applied, &hs), "{shape}/{kind}: proj apply failed");
                assert_eq!(
                    applied,
                    strip_ts(serde_json::from_str(&target).unwrap()),
                    "{shape}/{kind}: HS proj roundtrip"
                );
            }
        }
    }

    #[test]
    fn hs_proj_preserves_replace_attr_and_roundtrips() {
        let Some((old_snap, new_snap, task)) = make_case("inline", "flat") else {
            return;
        };
        let (facts, ctx) = diff_patches_mode_with_ctx_g(
            &task,
            &CONTAINER_TOPOLOGY,
            crate::topology::Granularity::Proj,
        );
        assert!(
            facts.iter().any(|p| matches!(p, Patch::ReplaceAttr { .. })),
            "CF proj must produce ReplaceAttr for paragraph content edit"
        );
        let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);
        assert!(
            hs.iter().any(|p| matches!(p, Patch::ReplaceAttr { .. })),
            "HS proj must preserve ReplaceAttr: {hs:?}"
        );
        let mut applied = serde_json::from_str::<serde_json::Value>(
            &old_snap.to_ast_object_mode(ProjectionMode::Hs),
        )
        .unwrap();
        let target = serde_json::from_str::<serde_json::Value>(
            &new_snap.to_ast_object_mode(ProjectionMode::Hs),
        )
        .unwrap();
        assert!(apply_all(&mut applied, &hs), "HS proj apply failed");
        assert_eq!(strip_ts(applied), strip_ts(target), "HS proj roundtrip");
    }

    /// WC-2（丙）：往**旧侧无子**的节里插入 ⇒ 发一条建数组补丁
    /// （`Add <壳路径>/ct` = 整数组），而不是逐元素 ADD，也不再整壳 REPL。
    #[test]
    fn hs_proj_childless_section_insert_creates_array() {
        let old = "# A\n\n## S\n";
        let (s, e) = (old.len(), old.len());
        let text = "\nbody\n";
        let mut state = State::new(old);
        let old_snap = Arc::new(state.snapshot());
        state.edit(s, e, text);
        let new_snap = Arc::new(state.snapshot());
        let diff = state.ripple_diff.take().expect("ripple_diff");
        let task = IncrTask {
            snap: new_snap.clone(), doc_id: 0, snap_old: old_snap.clone(), delta: diff.delta,
            edit_ranges: crate::cache::edit_actions_arc(vec![(s, e)]),
            ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        let (facts, ctx) = diff_patches_mode_with_ctx_g(
            &task, &CONTAINER_TOPOLOGY, crate::topology::Granularity::Proj,
        );
        let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);
        // ① 恰好一条"建数组"补丁：路径以 /ct 结尾（成员位），值是整数组
        let creates: Vec<&Patch> = hs
            .iter()
            .filter(|p| matches!(p, Patch::Add { path, .. } if path.ends_with("/ct")))
            .collect();
        assert_eq!(creates.len(), 1, "期望恰好一条建数组补丁，实得: {hs:?}");
        if let Patch::Add { path, value } = creates[0] {
            assert_eq!(path, "/ct/0/ct/0/ct", "建数组路径 = 壳路径 + /ct");
            let v: serde_json::Value = serde_json::from_str(value).expect("建数组值应是合法 JSON");
            assert!(v.is_array(), "值应是数组（整数组裸 JSON）: {value}");
            assert_eq!(v.as_array().unwrap().len(), 1, "应含一个新孩子: {value}");
        }
        // ② 不得再对 S 的壳做整壳 REPL
        assert!(
            !hs.iter().any(|p| matches!(p, Patch::Replace { path, .. } if path == "/ct/0/ct/0")),
            "不应再整壳 REPL: {hs:?}"
        );
        // ③ apply 后等于目标全量帧
        let base = old_snap.to_ast_object_mode(ProjectionMode::Hs);
        let target = new_snap.to_ast_object_mode(ProjectionMode::Hs);
        let mut applied = strip_ts(serde_json::from_str(&base).unwrap());
        assert!(apply_all(&mut applied, &hs), "HS apply 失败: {hs:?}");
        assert_eq!(applied, strip_ts(serde_json::from_str(&target).unwrap()), "HS 往返不等: {hs:?}");
    }

    /// WC-2（丙）对照用例：**已有孩子**的节再插一个 ⇒ 不得出现"建数组"补丁
    /// （数组已在，按元素插即可），且往返仍等值。
    #[test]
    fn hs_proj_existing_children_keeps_element_add() {
        let old = "# A\n\n## S\n\nbody1\n";
        let (s, e) = (old.len(), old.len());
        let text = "\nbody2\n";
        let mut state = State::new(old);
        let old_snap = Arc::new(state.snapshot());
        state.edit(s, e, text);
        let new_snap = Arc::new(state.snapshot());
        let diff = state.ripple_diff.take().expect("ripple_diff");
        let task = IncrTask {
            snap: new_snap.clone(), doc_id: 0, snap_old: old_snap.clone(), delta: diff.delta,
            edit_ranges: crate::cache::edit_actions_arc(vec![(s, e)]),
            ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        let (facts, ctx) = diff_patches_mode_with_ctx_g(
            &task, &CONTAINER_TOPOLOGY, crate::topology::Granularity::Proj,
        );
        let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);
        assert!(
            !hs.iter().any(|p| matches!(p, Patch::Add { path, .. } if path.ends_with("/ct"))),
            "数组已存在，不该再发建数组补丁: {hs:?}"
        );
        let base = old_snap.to_ast_object_mode(ProjectionMode::Hs);
        let target = new_snap.to_ast_object_mode(ProjectionMode::Hs);
        let mut applied = strip_ts(serde_json::from_str(&base).unwrap());
        assert!(apply_all(&mut applied, &hs), "HS apply 失败: {hs:?}");
        assert_eq!(applied, strip_ts(serde_json::from_str(&target).unwrap()), "HS 往返不等: {hs:?}");
    }

    /// WC-5 数量判官（位置寻址 = 取消整壳重发）：
    /// ① 纯改名（级别不变）——Block/Proj 双粒度都恰好 **1 条 op**，且是标题面字段写
    ///    `ReplaceAttr{attr:"t"}`（样本：单节改名 / 6 节文档改 A1a / 含标记改名）；
    /// ② 换父（级别变化）——op 集合恰为 `REM <旧位置>` + `ADD <新位置>`（样本：
    ///    `# A\n\np\n\n# B\n\nq\n` 的 B 升一级 → REM /ct/1 + ADD /ct/0/ct/1）；
    /// ③ G3 形态矩阵 × 双粒度——应用补丁后 == 目标全量帧（combo/hdel/hins/cross/
    ///    deep3/nested/tight/bq 的边界吸收、跨节删除与嵌套重挂等价判官）。
    #[test]
    fn wc5_op_count_samples() {
        use crate::topology::Granularity;
        fn snapshot_edit(old: &str, s: usize, e: usize, text: &str)
            -> (Arc<Snapshot>, Arc<Snapshot>, IncrTask) {
            let mut state = State::new(old);
            let old_snap = Arc::new(state.snapshot());
            state.edit(s, e, text);
            let new_snap = Arc::new(state.snapshot());
            let diff = state.ripple_diff.take().expect("ripple_diff");
            let task = IncrTask {
                snap: new_snap.clone(), doc_id: 0, snap_old: old_snap.clone(), delta: diff.delta,
                edit_ranges: crate::cache::edit_actions_arc(vec![(s, e)]),
                ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
                slot: None, slot_old: None, reaper_tx: None, structural_change: false,
            };
            (old_snap, new_snap, task)
        }
        let six = "# A1\n\np1\n\n# A1a\n\np2\n\n# A2\n\np3\n\n# A3\n\np4\n\n# A4\n\np5\n\n# A5\n\np6\n";
        let a1a = six.find("A1a").expect("A1a");
        fn apply_check(old_snap: &Arc<Snapshot>, new_snap: &Arc<Snapshot>, hs: &[Patch]) -> bool {
            let base_v = strip_ts(serde_json::from_str::<serde_json::Value>(
                &old_snap.to_ast_object_mode(ProjectionMode::Hs)).unwrap());
            let target_v = strip_ts(serde_json::from_str::<serde_json::Value>(
                &new_snap.to_ast_object_mode(ProjectionMode::Hs)).unwrap());
            let mut applied = base_v;
            apply_all(&mut applied, hs) && applied == target_v
        }
        // ① 纯改名（级别不变）：恰好 1 条 op，且是标题面字段写 t——
        //    "字段写不再连带整壳"的数量判官；Block/Proj 双粒度同形。
        for &(name, old, s, e, text, want_path) in &[
            ("单节改名", "# Alpha\n\nbody\n", 2, 7, "Alphax", "/ct/0"),
            ("6节改A1a", six, a1a, a1a + 3, "A1ax", "/ct/1"),
            ("标记改名", "# **b** c\n\nbody\n", 7, 8, "d", "/ct/0"),
        ] {
            let (old_snap, new_snap, task) = snapshot_edit(old, s, e, text);
            for g in [Granularity::Block, Granularity::Proj] {
                let (facts, ctx) = diff_patches_mode_with_ctx_g(&task, &CONTAINER_TOPOLOGY, g);
                let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);

                assert_eq!(hs.len(), 1,
                    "{name} g={g:?} 纯改名应恰好 1 条 op（取消整壳重发）: {hs:?}");
                match &hs[0] {
                    Patch::ReplaceAttr { path, attr, .. } => {
                        assert_eq!(path, want_path, "{name} g={g:?}");
                        assert_eq!(attr, "t", "{name} g={g:?}");
                    }
                    other => panic!("{name} g={g:?} 纯改名应是标题面字段写 t，实得 {other:?}"),
                }
                assert!(apply_check(&old_snap, &new_snap, &hs),
                    "{name} g={g:?} 应用不等价: {hs:?}");
            }
        }
        // ② 换父（级别变化）：op 集合 = 数组位置的 REM + ADD（不再整壳 REPL）。
        {
            let (old_snap, new_snap, task) = snapshot_edit("# A\n\np\n\n# B\n\nq\n", 8, 11, "## B");
            for g in [Granularity::Block, Granularity::Proj] {
                let (facts, ctx) = diff_patches_mode_with_ctx_g(&task, &CONTAINER_TOPOLOGY, g);
                let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);

                assert_eq!(hs.len(), 2, "换父应是数组位置增删两条: {hs:?}");
                assert!(matches!(&hs[0], Patch::Remove { path } if path == "/ct/1"),
                    "换父第 1 条应是旧位置 REM /ct/1: {hs:?}");
                assert!(matches!(&hs[1], Patch::Add { path, .. } if path == "/ct/0/ct/1"),
                    "换父第 2 条应是新位置 ADD /ct/0/ct/1: {hs:?}");
                assert!(apply_check(&old_snap, &new_snap, &hs), "换父应用不等价: {hs:?}");
            }
        }
        // ③ G3 形态矩阵 × 双粒度：应用后 == 目标全量帧（等价判官——覆盖 combo/hdel/
        //    hins/cross/deep3/nested/tight/bq 的边界吸收、跨节删除与嵌套重挂）。
        for &(shape, kind) in &[
            ("flat", "combo"), ("flat", "hdel"), ("flat", "hins"), ("flat", "cross"),
            ("setext", "combo"), ("nested", "combo"), ("nested", "hlevel"),
            ("deep3", "hdel"), ("tight", "hins"), ("bq_inner_head", "hdel"),
        ] {
            let Some((old_snap, new_snap, task)) = make_case(kind, shape) else { continue };
            for g in [Granularity::Block, Granularity::Proj] {
                let (facts, ctx) = diff_patches_mode_with_ctx_g(&task, &CONTAINER_TOPOLOGY, g);
                let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);
                let eq = apply_check(&old_snap, &new_snap, &hs);

                assert!(eq, "G3:{shape}/{kind} g={g:?} 应用 != 目标全量: {hs:?}");
            }
        }
        // ④ 两例 zone_oracle 回归（WC-5 修复的形态——曾有 op 冲突/越界）：
        //    ① 空节里插新节：只发**一条**建数组补丁（`Add <壳>/ct = [新孩子]`）+ 应用等价；
        //    ② bq 前插列表：唯一子节点被"替换"（不是删除）——逐元素 REM+ADD，
        //       不得再发 `remove <壳>/ct`（那会清空数组、令后续 ADD 越界）。
        for &(name, old, s, e, text, want_ops) in &[
            ("h1_insert_mid", "# A\n\n# B\n", 4, 4, "## M\n", 1usize),
            // [§13-4] I 层（implicit 内容键）后此例收敛为**单条 ADD**：新列表插到节首，
            // 原 bq 只需按位置位移，不必再发 REM+ADD 对（旧 3 条是位置身份时代的产物）。
            ("bq_ins_start", "# H\n\n> q\n> q2\n", 5, 5, "- ins\n", 1usize),
            // 节首插整行：LineMap 在插入点歧义——旧节必须按标题面认领（否则新节被错认成
            // 旧节、stale REM 无兜底打错位置）；目标是 1 条新壳 ADD。
            ("line_ins_at_head", "# H\n\n- a\n\n> q\n", 0, 0, "# X\n", 1usize),
        ] {
            let (old_snap, new_snap, task) = snapshot_edit(old, s, e, text);
            for g in [Granularity::Block, Granularity::Proj] {
                let (facts, ctx) = diff_patches_mode_with_ctx_g(&task, &CONTAINER_TOPOLOGY, g);
                let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);

                assert_eq!(hs.len(), want_ops, "{name} g={g:?} op 数: {hs:?}");
                assert!(apply_check(&old_snap, &new_snap, &hs),
                    "{name} g={g:?} 应用 != 目标全量: {hs:?}");
            }
        }
    }
    /// WC-9 判官：值槽写（`- value: on` → `off`）在 HS 下必须发**路径形态字段写**
    /// （attr=`ct`，指向渲染树里的值包 p 节点），不得降级成"整槽 REPL + 裸行内串"——
    /// 后者会让客户端只能靠 K5 手工补 `{nt:"p",ct:…}`（缺 `t:""`），与后端 F 帧分叉。
    /// 两种粒度都要 apply 后与目标 HS 全量帧逐值相等（apply_check）。
    #[test]
    fn wc9_value_slot_write_is_field_write() {
        use crate::topology::Granularity;
        let old = "# DEMO_UI\n\n## PAGE1\n\n### SWITCH1\n- value: on\n";
        let s = old.find("on").expect("on");
        let mut state = crate::projection::State::new(old);
        let old_snap = std::sync::Arc::new(state.snapshot());
        state.edit(s, s + 2, "off");
        let new_snap = std::sync::Arc::new(state.snapshot());
        let diff = state.ripple_diff.take().expect("ripple_diff");
        let task = IncrTask {
            snap: new_snap.clone(), doc_id: 0, snap_old: old_snap.clone(), delta: diff.delta,
            edit_ranges: crate::cache::edit_actions_arc(vec![(s, s + 2)]),
            ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        let apply_check = |hs: &[Patch]| -> bool {
            let mut base = strip_ts(serde_json::from_str::<serde_json::Value>(
                &old_snap.to_ast_object_mode(ProjectionMode::Hs)).unwrap());
            let target = strip_ts(serde_json::from_str::<serde_json::Value>(
                &new_snap.to_ast_object_mode(ProjectionMode::Hs)).unwrap());
            apply_all(&mut base, hs) && base == target
        };
        // Proj：恰好一条字段写，attr=值包 p 的 ct，值=新行内串，路径指向 p 节点。
        {
            let (facts, ctx) = diff_patches_mode_with_ctx_g(&task, &CONTAINER_TOPOLOGY, Granularity::Proj);
            let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);
            assert_eq!(hs.len(), 1, "值槽写 Proj 应恰好 1 条 op: {hs:?}");
            match &hs[0] {
                Patch::ReplaceAttr { path, attr, value } => {
                    assert_eq!(attr, "ct", "字段名应是值包 p 的 ct: {hs:?}");
                    assert_eq!(value, "\"off\"", "值应是新行内串: {hs:?}");
                    assert!(path.ends_with("/ct/0"), "路径应指向值包 p 节点: {hs:?}");
                }
                other => panic!("值槽写 Proj 必须是字段写（不得降级整槽 REPL，否则 K5 分叉）: {other:?}"),
            }
            assert!(apply_check(&hs), "Proj 应用后应等于目标 HS 帧: {hs:?}");
        }
        // Block：整块 REPL（值 = 完整投影，含 t:""），应用等价。
        {
            let (facts, ctx) = diff_patches_mode_with_ctx_g(&task, &CONTAINER_TOPOLOGY, Granularity::Block);
            let hs = adapt(&task, &facts, &ctx, &HEADINGSECTION_TOPOLOGY);
            assert_eq!(hs.len(), 1, "值槽写 Block 应恰好 1 条 op: {hs:?}");
            assert!(matches!(&hs[0], Patch::Replace { .. }), "Block 应是整块 REPL: {hs:?}");
            assert!(apply_check(&hs), "Block 应用后应等于目标 HS 帧: {hs:?}");
        }
    }

    #[test]
    fn secs_root_count_matches_full_walker() {
        for &(shape, kind) in &[
            ("fence_cross", "inline"), ("flat", "inline"), ("setext", "inline"),
            ("nested", "heading"), ("bq_inner_head", "heading"), ("tight", "inline"),
        ] {
            let Some((old_snap, _new_snap, _task)) = make_case(kind, shape) else { continue };
            let n = old_snap.line_len();
            let seg = crate::cache::structure_for_range(&old_snap, 0, n);
            let cell = old_snap.section_cell(&HEADINGSECTION_TOPOLOGY);
            for line in 0..=n {
                let got = cell.root_count_before(line);
                let want = root_child_index_hs_from(
                    &seg, line, HEADINGSECTION_TOPOLOGY.heading_candidates,
                );
                if got != want {
                    panic!("{shape}/{kind} line {line}: got {got} want {want}");
                }
            }
        }
    }

    /// patch match attr：claim 计算判官（spec S2/S5；独立 oracle 对拍）。
    mod match_claims_s2 {
        include!("../tests/unit/match_claims.rs");
    }
}
