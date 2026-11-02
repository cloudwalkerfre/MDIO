use ahash::AHashMap as HashMap;
use arc_swap::ArcSwap;
use crossbeam::channel::Sender;
use parking_lot::Mutex;
use smallvec::SmallVec;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::telemetry_span;

use crate::engine::{MdioEngine, MdioSnapshot};
use crate::op::WriteEdit;
use crate::selector::Step;
use crate::store::DetectTask;
use crate::store::EditRecord;
use crate::backend::WalEntry;
use crate::store::IncrTask;
use cola::{Deletion, Insertion, Replica, ReplicaId};
use crossbeam::queue::SegQueue;
use dashmap::DashSet;
use serde::{Deserialize, Serialize};

// Cola 作者推荐的 Bincode 配置: Little Endian + VarInt
pub(crate) fn bincode_config() -> impl bincode::config::Config {
    bincode::config::standard()
        .with_little_endian()
        .with_variable_int_encoding()
}

/// ── 血统世代号（epoch）────────────────────────────────────────────────────
/// **文档血统的身份**：它是"这份副本从哪条 base run 长出来"的编号。
///   · 无 checkpoint 新建会话（既有文本被登记成本副本 id 名下的**新 base run**）⇒ 取新号；
///   · 从 checkpoint 恢复（继承别人的 run 结构）⇒ 沿用 checkpoint 里的号。
/// 客户端持有它，据此判断"手上这份副本还是不是服务端当前血统的后代"——不等就整篇
/// 重取，而不是继续往一条对不上的血统上集成（后者会 **integrate None 永久静默**）。
/// 进程基址取随机值：重启后世代号不与上一世代重合（重合 = 客户端误判"没变"）。
static LINEAGE_SEQ: AtomicU64 = AtomicU64::new(0);
static LINEAGE_BASE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

pub fn next_lineage() -> u64 {
    let base = *LINEAGE_BASE.get_or_init(|| rand::random::<u64>() | 1);
    base.wrapping_add(LINEAGE_SEQ.fetch_add(1, Ordering::Relaxed))
}


// Wire Protocol & CRDT

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum WireOp {
    Insert { op: Insertion, content: String },
    Delete { op: Deletion },
}

// 使用 (ReplicaId, range_start, range_end) 作为插入的唯一标识
type InsertionKey = (ReplicaId, usize, usize);
fn make_insertion_key(ins: &Insertion) -> InsertionKey {
    let range = ins.text().temporal_range();
    (ins.inserted_by(), range.start, range.end)
}


// 事务与状态网关

pub struct WriteState {
    pub engine: MdioEngine,
    pub replica: Option<Replica>,
    pub pending_inserts: HashMap<InsertionKey, String>,
    /// Auto-recorded by TransactionContext::edit() — drained in execute_transaction.
    pub edit_spans: Vec<(usize, usize, String)>,
}

pub struct TransactionContext<'a> {
    pub state: &'a mut WriteState,
    pub wire_ops: &'a mut SmallVec<[WireOp; 4]>,
    pub tx_min_dirty_line: usize,
}

impl<'a> TransactionContext<'a> {
    /// Unified edit entry: delegates to engine + auto-records for WAL.
    #[inline]
    fn edit(&mut self, start: usize, end: usize, text: &str) {
        self.state.engine.edit(start, end, text);
        self.state.edit_spans.push((start, end, text.to_string()));
    }

    pub fn snapshot(&mut self) -> MdioSnapshot {
        self.state.engine.make_snapshot()
    }

    fn apply_one(
        &mut self,
        raw_range: std::ops::Range<usize>,
        operator: &str,
        payload: &str,
        cumulative_offset: isize,
    ) -> isize {
        let _span = telemetry_span!("apply_one", "op" => operator);
        let start = (raw_range.start as isize + cumulative_offset) as usize;
        let end = (raw_range.end as isize + cumulative_offset) as usize;

        // [fix] 钳制上限取 engine 与 replica 长度的较小者：两者若因上游不一致
        // 而错位，越界会落进 cola 的 panic（`inserted/deleted` 的边界检查）。
        // 正常路径下二者恒等（见 from_checkpoint 的同源修复），此处只作兜底。
        let len = self.state.engine.byte_len();
        let cap = self.state.replica.as_ref().map_or(len, |r| r.len().min(len));
        debug_assert_eq!(cap, len, "engine/replica 长度错位: engine={len} replica={cap}");
        let start = start.min(cap);
        let end = end.min(cap).max(start);

        let current_line = self.state.engine.line_of_byte(start);
        self.tx_min_dirty_line = self.tx_min_dirty_line.min(current_line);

        match operator {
            "/>" => {
                // 引号剥壳不存在（补正 P0-1 + G3 实证）：Rust 背板不判义——
                // 载荷引号一律原样写入（G3 抓出合法引文 "List...tab." 被误剥；
                // LLM 包裹剥壳是调用方语义，由 py interpreter 层承担）。
                let final_payload = payload;

                let mut trailing_n: usize = 0;
                let mut pos = end;
                while pos > start {
                    pos -= 1;
                    if self.state.engine.byte_at(pos) == b'\n' {
                        trailing_n += 1;
                    } else {
                        break;
                    }
                }
                let effective_end = end - trailing_n;
                let has_trailing_nl = trailing_n > 0
                    || (end < self.state.engine.byte_len()
                        && self.state.engine.byte_at(end) == b'\n');
                let clean_payload = if has_trailing_nl {
                    final_payload.trim_end_matches('\n')
                } else {
                    final_payload
                };

                if effective_end > start {
                    self.edit(start, effective_end, "");
                    if let Some(rep) = self.state.replica.as_mut() {
                        self.wire_ops.push(WireOp::Delete {
                            op: rep.deleted(start..effective_end),
                        });
                    }
                }
                if !clean_payload.is_empty() {
                    self.edit(start, start, clean_payload);
                    if let Some(rep) = self.state.replica.as_mut() {
                        self.wire_ops.push(WireOp::Insert {
                            op: rep.inserted(start, clean_payload.len()),
                            content: clean_payload.to_string(),
                        });
                    }
                }
                (clean_payload.len() as isize) - ((effective_end - start) as isize)
            }
            "+>" => {
                // 引号剥壳不存在（同 /> 分支——Rust 背板不判义）。
                let final_payload = payload;
                self.edit(end, end, final_payload);
                if let Some(rep) = self.state.replica.as_mut() {
                    self.wire_ops.push(WireOp::Insert {
                        op: rep.inserted(end, final_payload.len()),
                        content: final_payload.to_string(),
                    });
                }
                final_payload.len() as isize
            }
            "->" => {
                if end > start {
                    self.edit(start, end, "");
                    if let Some(rep) = self.state.replica.as_mut() {
                        self.wire_ops.push(WireOp::Delete {
                            op: rep.deleted(start..end),
                        });
                    }
                    -((end - start) as isize)
                } else {
                    0
                }
            }
            "~>" => {
                let mut delta = 0isize;
                match crate::op::parse_patch_blocks(payload) {
                    Ok(patches) => {
                        let mut dynamic_end = end;
                        for patch in patches {
                            let old_str = patch.old;
                            let new_str = patch.new;
                            if old_str.is_empty() {
                                continue;
                            }

                            let target_node_text =
                                self.state.engine.byte_slice(start..dynamic_end).to_string();
                            let mut matched_pos = target_node_text.find(&old_str);
                            let mut final_old_str = old_str.as_str();
                            let mut final_new_str = new_str.as_str();

                            if matched_pos.is_none() {
                                let old_trim = old_str.trim_matches(|c| c == '\n' || c == '\r');
                                let new_trim = new_str.trim_matches(|c| c == '\n' || c == '\r');
                                matched_pos = target_node_text.find(old_trim);
                                if matched_pos.is_some() {
                                    final_old_str = old_trim;
                                    final_new_str = new_trim;
                                }
                            }

                            if let Some(pos) = matched_pos {
                                let abs_start = start + pos;
                                let abs_end = abs_start + final_old_str.len();
                                if abs_end > abs_start {
                                    self.edit(abs_start, abs_end, "");
                                    if let Some(rep) = self.state.replica.as_mut() {
                                        self.wire_ops.push(WireOp::Delete {
                                            op: rep.deleted(abs_start..abs_end),
                                        });
                                    }
                                }
                                if !final_new_str.is_empty() {
                                    self.edit(abs_start, abs_start, final_new_str);
                                    if let Some(rep) = self.state.replica.as_mut() {
                                        self.wire_ops.push(WireOp::Insert {
                                            op: rep.inserted(abs_start, final_new_str.len()),
                                            content: final_new_str.to_string(),
                                        });
                                    }
                                }
                                let diff =
                                    (final_new_str.len() as isize) - (final_old_str.len() as isize);
                                delta += diff;
                                dynamic_end = (dynamic_end as isize + diff) as usize;
                            } else {
                                eprintln!(
                                    "⚠️ [MDIO Patch Warning] Match failed! '{}' not found.",
                                    final_old_str
                                );
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("⚠️ [MDIO Patch Warning] {}", e.message);
                    }
                }
                delta
            }
            _ => 0,
        }
    }

    pub fn apply_mdio_op(
        &mut self,
        mut ranges: Vec<std::ops::Range<usize>>,
        operator: &str,
        payload: &str,
    ) {
        // 稳定排序：多个零宽插入同点时保持调用方给出的顺序（同点插入顺序可观察）。
        ranges.sort_by_key(|r| r.start);
        let mut cumulative_offset: isize = 0;
        for r in ranges {
            cumulative_offset += self.apply_one(r, operator, payload, cumulative_offset);
        }
    }

    pub fn apply_mdio_op_multi(&mut self, mut edits: Vec<WriteEdit>, operator: &str) {
        let _span = telemetry_span!("apply_crdt", "edits" => edits.len(), "op" => operator);
        if edits.len() > 1 {
            // 稳定排序（不是 sort_unstable_by_key）：同 start 的零宽插入保持
            // plan 产出的原顺序；插入顺序影响最终文本。
            edits.sort_by_key(|e| e.range.start);
        }
        let mut cumulative_offset: isize = 0;
        for e in edits {
            cumulative_offset += self.apply_one(e.range, operator, &e.text, cumulative_offset);
        }
    }

    /// `plan_patch` 预计算好的精确编辑：按生成顺序直接 delete/insert。
    /// range 已经是应用时刻的当前坐标，不再做排序或累积偏移。
    pub fn apply_patch_edits(&mut self, edits: Vec<WriteEdit>) {
        let _span = telemetry_span!("apply_patch_edits", "edits" => edits.len());
        for e in edits {
            let len = self.state.engine.byte_len();
            let cap = self.state.replica.as_ref().map_or(len, |r| r.len().min(len));
            let start = e.range.start.min(cap);
            let end = e.range.end.min(cap).max(start);
            let current_line = self.state.engine.line_of_byte(start);
            self.tx_min_dirty_line = self.tx_min_dirty_line.min(current_line);

            if end > start {
                self.edit(start, end, "");
                if let Some(rep) = self.state.replica.as_mut() {
                    self.wire_ops.push(WireOp::Delete {
                        op: rep.deleted(start..end),
                    });
                }
            }
            if !e.text.is_empty() {
                self.edit(start, start, &e.text);
                if let Some(rep) = self.state.replica.as_mut() {
                    self.wire_ops.push(WireOp::Insert {
                        op: rep.inserted(start, e.text.len()),
                        content: e.text.clone(),
                    });
                }
            }
        }
    }
}


// Session


/// 文档快照序号时钟（进程内单调）。
///
/// - 写路径：`execute_transaction` 在 doc write_guard 内 `assign()`，把序号随 DetectTask 入队；
/// - 注册路径：`ProjectStore::register_hook` 在同一把 write_guard 内 `watermark()`，
///   作为 hook 的 `active_from`：序号小于它的任务快照早于 hook 生效，必须跳过。
pub struct DetectClock {
    next: AtomicU64,
}

impl DetectClock {
    pub fn new() -> Self {
        Self { next: AtomicU64::new(0) }
    }

    /// 分配下一个任务序号（从 1 开始；调用方必须持有该文档的 write_guard）。
    pub fn assign(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// 下一个将被分配的序号 = 此刻注册的 hook 的生效水位（调用方必须持有 write_guard）。
    pub fn watermark(&self) -> u64 {
        self.next.load(Ordering::SeqCst) + 1
    }
}

pub struct MdioSession {
    pub doc_id: u32,
    /// 本会话所持副本的血统世代号（见 next_lineage）。checkpoint 里带着它，
    /// 客户端凭它判断自己那份副本是否还属于服务端当前血统。
    pub lineage: u64,
    pub publisher: ArcSwap<MdioSnapshot>,
    pub write_guard: Mutex<WriteState>,
    pub detect_tx: Option<crossbeam::channel::Sender<DetectTask>>,
    /// 文档快照序号时钟（与 store 共享；写路径 assign、注册路径 watermark）。
    pub detect_clock: Arc<DetectClock>,
    /// Step 5: incremental AST patch channel
    pub path_tx: Option<crossbeam::channel::Sender<IncrTask>>,
    pub hook_docs: Arc<DashSet<u32>>,
    pub dirty: AtomicBool,
    pub storage_key: String,
    pub wal_queue: Option<Arc<SegQueue<(String, Vec<WalEntry>)>>>,
    /// B2a(#1):表值 Weak——表不保活,淘汰 = 引用归零(需求驱动);
    /// 保活半径恒 1 = {latest_published} ∪ {任务 pins}。
    pub(crate) cache_slots: Arc<dashmap::DashMap<u64, std::sync::Weak<crate::cache::CacheSlot>>>,
    /// B2a(#8):恒强引用锚(最新发布版描述符)——publisher 快照的 slot 恒可
    /// upgrade,旧代无任务 pin 即随最后 Arc 归零。
    pub(crate) latest_published: ArcSwap<Arc<crate::cache::CacheSlot>>,
    next_slot_id: AtomicU64,
    /// 限量释放的回收通道(写线程 O(1) 转移;回收线程付 O(doc) drop)。
    /// B3 起触发点 = 带锁两钩子(写线程换代 + worker 收尾)。
    full_reaper: Option<ReaperTx>,
}

/// 组装限量的释放载体:full/render 缓存是 O(doc) 对象,
/// 写线程同步 drop 要付 O(doc) 释放(6MB ≈ 20 万次 free,4.3ms 实测——
/// seam_diag_budget 归因),交给回收线程 drop(写线程 O(1) swap+send)。
/// pub(B3 接线):三任务结构的 reaper_tx 字段需可名化——外部 crate
/// (bins/集成测试)以 `None` 构造这些公开字段,字段类型必须可名。
pub struct ReleasedMem {
    full: Option<Arc<crate::cache::Segment>>,
    render: Option<Arc<crate::cache::RootCache>>,
}

/// 只允许 session/pool 构造的 sender 类型：外部任务字段只能 None。
#[derive(Clone)]
pub struct ReaperTx(std::sync::mpsc::Sender<ReleasedMem>);

impl ReaperTx {
    pub(crate) fn from_sender(tx: std::sync::mpsc::Sender<ReleasedMem>) -> Self {
        Self(tx)
    }
    fn send(&self, bag: ReleasedMem) {
        let _ = self.0.send(bag);
    }
}

mod shard_reaper {
    use super::{ReaperTx, ReleasedMem};
    use std::sync::OnceLock;

    const SHARDS: usize = 2;
    static POOL: OnceLock<Vec<ReaperTx>> = OnceLock::new();

    pub(crate) fn tx_for(doc_id: u32) -> ReaperTx {
        let pool = POOL.get_or_init(|| {
            let mut txs = Vec::with_capacity(SHARDS);
            for shard in 0..SHARDS {
                let (tx, rx) = std::sync::mpsc::channel::<ReleasedMem>();
                let res = std::thread::Builder::new()
                    .name(format!("mdio-shard-reaper-{}", shard))
                    .spawn(move || loop {
                        match rx.recv() {
                            Ok(bag) => {
                                let _ = std::panic::catch_unwind(
                                    std::panic::AssertUnwindSafe(|| drop(bag)),
                                );
                            }
                            Err(_) => break,
                        }
                    });
                if let Err(e) = res {
                    eprintln!("[reaper] shard {} spawn failed: {}", shard, e);
                }
                txs.push(ReaperTx::from_sender(tx));
            }
            txs
        });
        pool[(doc_id as usize) % SHARDS].clone()
    }
}

/// B3 带锁"最后持者取物"协议(§5.3,两个半钩子共用):overlay 锁下判
/// strong_count == `holders`(执行体已知在持者数——写线程换代半钩子 =
/// 旧锚 + 本执行体克隆 = 2;worker 收尾半钩子 = 任务自持 = 1)则
/// swap-out {full, render} 送 reaper。锁内判 count 消除"双互见"漏取
/// 竞态;残留早取窗口(判后新持者取到空槽)后果 = 一次重建,与现状无槽
/// 同价,正确性无损。mid 例外:OnceLock 单构建语义,不进取物清单。
pub(crate) fn reaper_take_if_last(
    slot: &Arc<crate::cache::CacheSlot>,
    tx: &ReaperTx,
    holders: usize,
) {
    let took = if Arc::strong_count(slot) != holders {
        false
    } else {
        let _ov = slot.overlay.lock();
        let node = slot.node.load_full();
        if let Some(n) = node {
            let mut bag = ReleasedMem {
                full: None,
                render: None,
            };
            match &**n {
                crate::cache::CacheNode::Edit { full, render, .. } => {
                    bag.full = full.swap(None);
                    bag.render = render.swap(None);
                }
                crate::cache::CacheNode::Base { full, render, .. } => {
                    bag.full = full.swap(None);
                    bag.render = render.swap(None);
                }
            }
            let bag_state = match (bag.full.is_some(), bag.render.is_some()) {
                (true, true) => "both",
                (true, false) => "full",
                (false, true) => "render",
                (false, false) => "none",
            };
            tx.send(bag);
            // 探查 kv:took = 协议执行;bag = 实际移走什么(full/render/
            // both/none)——读方只依赖 full,render-only 取物不影响
            // 旧侧命中;机制断言按 full 口径,不把 render 取物混进
            // "取物→重建"链。
            #[cfg(feature = "telemetry")]
            {
                let _sp = crate::telemetry_span!("reaper_take",
                    "took" => true, "moved" => bag_state != "none",
                    "bag" => bag_state,
                    "holders" => holders as u64, "slot_id" => slot.id);
            }
            return;
        }
        false
    };
    // 探查 kv:取物与否 + 槽号——与下一轮首读 old_full_of 的昂贵轮按
    // 代际配对,裁决"取物 → 首读重建"因果链(机制查证,feature OFF 零开销)。
    #[cfg(feature = "telemetry")]
    {
        let _sp = crate::telemetry_span!("reaper_take",
            "took" => took, "moved" => false, "bag" => "none",
            "holders" => holders as u64, "slot_id" => slot.id);
    }
}

/// 行段逐值比较——三截拼接下 PL 差异的完整
/// 判定域(前缀恒等/tail 保留)。缺失/越界 → false → 保守 structural。
fn lines_segment_eq(
    a: &[crate::block::PackedLineInfo],
    b: &[crate::block::PackedLineInfo],
    rls: usize,
    rle: usize,
) -> bool {
    match (
        a.get(rls..=rle.min(a.len().saturating_sub(1))),
        b.get(rls..=rle.min(b.len().saturating_sub(1))),
    ) {
        (Some(sa), Some(sb)) => sa == sb,
        _ => false,
    }
}

impl MdioSession {
    pub fn new(
        doc_id: u32,
        initial_text: &str,
        peer_id: u64,
        storage_key: String,
        wal_queue: Option<Arc<SegQueue<(String, Vec<WalEntry>)>>>,
        detect_tx: Option<crossbeam::channel::Sender<DetectTask>>,
        path_tx: Option<crossbeam::channel::Sender<IncrTask>>,
        hook_docs: Arc<DashSet<u32>>,
        detect_clock: Arc<DetectClock>,
    ) -> Self {
        let mut engine = MdioEngine::new(initial_text);
        let mut snapshot = engine.make_snapshot();
        // 827-A1:初始快照补槽——写侧唯一缺口(会话首编辑的旧侧沿链不可达、
        // 每次全量自建 750µs 且无缓存)。槽随会话缓存表保活;基代节点留
        // 首读 ensure_node 惰性建(一次 1.07ms/会话)。id 1 归初始代,
        // next_slot_id 从 2 起(避免与首编辑代冲突)。
        let cache_slots: Arc<dashmap::DashMap<u64, std::sync::Weak<crate::cache::CacheSlot>>> =
            Arc::new(dashmap::DashMap::new());
        let next_slot_id = AtomicU64::new(2);
        let init_slot = Arc::new(crate::cache::CacheSlot::new_with_id(1));
        snapshot.set_cache_slot(Arc::downgrade(&init_slot));
        cache_slots.insert(1, Arc::downgrade(&init_slot));
        let latest_published = ArcSwap::from_pointee(init_slot);
        let replica = Some(Replica::new(peer_id as ReplicaId, engine.byte_len()));

        let reaper_tx = shard_reaper::tx_for(doc_id);
        Self {
            doc_id,
            lineage: next_lineage(),
            publisher: ArcSwap::from_pointee(snapshot),
            write_guard: Mutex::new(WriteState {
                engine,
                replica,
                pending_inserts: HashMap::new(),
                edit_spans: Vec::new(),
            }),
            detect_tx,
            path_tx,
            hook_docs,
            detect_clock,
            dirty: AtomicBool::new(false),
            storage_key,
            wal_queue,
            cache_slots,
            latest_published,
            next_slot_id,
            full_reaper: Some(reaper_tx),
        }
    }

    /// Build a normal, fully writable session from a handoff-copied Snapshot.
    /// Mirrors new(): initial CacheSlot, cache_slots, latest_published, replica,
    /// reaper, dirty=false. Does not scan.
    pub fn from_snapshot(
        doc_id: u32,
        mut snapshot: MdioSnapshot,
        peer_id: u64,
        storage_key: String,
        wal_queue: Option<Arc<SegQueue<(String, Vec<WalEntry>)>>>,
        detect_tx: Option<crossbeam::channel::Sender<DetectTask>>,
        path_tx: Option<crossbeam::channel::Sender<IncrTask>>,
        hook_docs: Arc<DashSet<u32>>,
        detect_clock: Arc<DetectClock>,
    ) -> Self {
        let engine = MdioEngine::from_state(crate::projection::State::from_snapshot(&snapshot));
        let cache_slots: Arc<dashmap::DashMap<u64, std::sync::Weak<crate::cache::CacheSlot>>> =
            Arc::new(dashmap::DashMap::new());
        let next_slot_id = AtomicU64::new(2);
        let init_slot = Arc::new(crate::cache::CacheSlot::new_with_id(1));
        snapshot.set_cache_slot(Arc::downgrade(&init_slot));
        cache_slots.insert(1, Arc::downgrade(&init_slot));
        let latest_published = ArcSwap::from_pointee(init_slot);
        let replica = Some(Replica::new(peer_id as ReplicaId, engine.byte_len()));

        let reaper_tx = shard_reaper::tx_for(doc_id);

        Self {
            doc_id,
            lineage: next_lineage(),
            publisher: ArcSwap::from_pointee(snapshot),
            write_guard: Mutex::new(WriteState {
                engine,
                replica,
                pending_inserts: HashMap::new(),
                edit_spans: Vec::new(),
            }),
            detect_tx,
            path_tx,
            hook_docs,
            detect_clock,
            dirty: AtomicBool::new(false),
            storage_key,
            wal_queue,
            cache_slots,
            latest_published,
            next_slot_id,
            full_reaper: Some(reaper_tx),
        }
    }

    pub fn from_checkpoint(
        doc_id: u32,
        checkpoint: &[u8],
        peer_id: u64,
        current_text: &str,
        storage_key: String,
        wal_queue: Option<Arc<SegQueue<(String, Vec<WalEntry>)>>>,
        detect_tx: Option<crossbeam::channel::Sender<DetectTask>>,
        path_tx: Option<crossbeam::channel::Sender<IncrTask>>,
        hook_docs: Arc<DashSet<u32>>,
        detect_clock: Arc<DetectClock>,
    ) -> Result<Self, String> {
        // checkpoint 载荷 = (text, EncodedReplica, lineage)。lineage 是血统身份，
        // 与副本一起传递：从 checkpoint 恢复 = **继承**那条血统，不新开一代。
        let (dump, _): ((String, cola::EncodedReplica, u64), usize) =
            bincode::serde::decode_from_slice(checkpoint, bincode_config())
            .map_err(|e| format!("Invalid checkpoint data: {e}"))?;
        let (stored_text, protocol, lineage) = dump;

        let replica = Replica::decode(peer_id as ReplicaId, &protocol)
            .map_err(|e| format!("CRDT decode fail: {e}"))?;

        // [fix] engine 必须与 replica **同源**：replica 由 checkpoint 内的
        // `stored_text` 编码而来。调用方 `current_text` 可能与之不同——iroh 远程
        // 挂载路径的上游 `write_locator(+>, auto_scaffold)` 会把空文档 payload
        // 归一化（补尾换行 / 列表前缀），于是 current_text 比 stored_text 长；
        // 若采用它，engine/replica 长度即错位，之后任何落在文档末尾的写入都会
        // 在 cola 的边界检查里 panic（`rep.inserted(offset > len)`）。
        // 故以 stored_text 为准（判官 `from_checkpoint_engine_takes_stored_text_when_caller_differs`）。
        let engine_text: &str = if stored_text == current_text {
            current_text
        } else {
            &stored_text
        };
        let mut engine = MdioEngine::new(engine_text);
        let mut snapshot = engine.make_snapshot();
        // 827-A1:同 new()——初始快照补槽(缺口修复,id 1 归初始代)。
        let cache_slots: Arc<dashmap::DashMap<u64, std::sync::Weak<crate::cache::CacheSlot>>> =
            Arc::new(dashmap::DashMap::new());
        let next_slot_id = AtomicU64::new(2);
        let init_slot = Arc::new(crate::cache::CacheSlot::new_with_id(1));
        snapshot.set_cache_slot(Arc::downgrade(&init_slot));
        cache_slots.insert(1, Arc::downgrade(&init_slot));
        let latest_published = ArcSwap::from_pointee(init_slot);

        let reaper_tx = shard_reaper::tx_for(doc_id);
        Ok(Self {
            doc_id,
            lineage,
            publisher: ArcSwap::from_pointee(snapshot),
            write_guard: Mutex::new(WriteState {
                engine,
                replica: Some(replica),
                pending_inserts: HashMap::new(),
                edit_spans: Vec::new(),
            }),
            detect_tx,
            path_tx,
            hook_docs,
            detect_clock,
            dirty: AtomicBool::new(false),
            storage_key,
            wal_queue,
            cache_slots,
            latest_published,
            next_slot_id,
            full_reaper: Some(reaper_tx),
        })
    }

    pub fn apply_update(&self, update: &[u8]) {
        if update.is_empty() {
            return;
        }
        let _span =
            telemetry_span!("crdt_apply_update", "doc_id" => self.doc_id, "bytes" => update.len());

        let (ops, _): (SmallVec<[WireOp; 2]>, usize) =
            match bincode::serde::decode_from_slice(update, bincode_config()) {
            Ok(res) => res,
            Err(_) => return,
        };

        self.execute_transaction(None, false, |ctx| {
            if ctx.state.replica.is_none() {
                return;
            }

            for op in ops {
                match op {
                    WireOp::Insert {
                        op: ins_op,
                        content,
                    } => {
                        let key = make_insertion_key(&ins_op);
                        ctx.state
                            .pending_inserts
                            .insert(key.clone(), content.clone());

                        let offset_opt = ctx
                            .state
                            .replica
                            .as_mut()
                            .unwrap()
                            .integrate_insertion(&ins_op);
                        if let Some(offset) = offset_opt {
                            let dirty_l = ctx.state.engine.line_of_byte(offset);
                            ctx.tx_min_dirty_line = ctx.tx_min_dirty_line.min(dirty_l);

                            ctx.edit(offset, offset, &content);
                            ctx.state.pending_inserts.remove(&key);
                        }
                    }
                    WireOp::Delete { op: del_op } => {
                        let ranges = ctx
                            .state
                            .replica
                            .as_mut()
                            .unwrap()
                            .integrate_deletion(&del_op);
                        for range in ranges.into_iter().rev() {
                            let dirty_l = ctx.state.engine.line_of_byte(range.start);
                            ctx.tx_min_dirty_line = ctx.tx_min_dirty_line.min(dirty_l);

                            ctx.edit(range.start, range.end, "");
                        }
                    }
                }
            }

            // CRDT 积压释放
            loop {
                let mut processed = 0;
                let back_ins = ctx
                    .state
                    .replica
                    .as_mut()
                    .unwrap()
                    .backlogged_insertions()
                    .map(|(b_txt, b_off)| (b_txt.clone(), b_off))
                    .collect::<Vec<_>>();
                for (text, offset) in back_ins {
                    let r = text.temporal_range();
                    let key = (text.inserted_by(), r.start, r.end);
                    if let Some(content) = ctx.state.pending_inserts.remove(&key) {
                        let dirty_l = ctx.state.engine.line_of_byte(offset);
                        ctx.tx_min_dirty_line = ctx.tx_min_dirty_line.min(dirty_l);

                        ctx.state.engine.edit(offset, offset, &content);
                        // incr_plan §6.3: dirty_lines removed
                        processed += 1;
                    }
                }

                let back_del = ctx
                    .state
                    .replica
                    .as_mut()
                    .unwrap()
                    .backlogged_deletions()
                    .collect::<Vec<_>>();
                for ranges in back_del {
                    for range in ranges.into_iter().rev() {
                        let dirty_l = ctx.state.engine.line_of_byte(range.start);
                        ctx.tx_min_dirty_line = ctx.tx_min_dirty_line.min(dirty_l);

                        ctx.edit(range.start, range.end, "");
                        processed += 1;
                    }
                }
                if processed == 0 {
                    break;
                }
            }
        });
    }

    pub fn get_checkpoint(&self) -> Vec<u8> {
        let _span = telemetry_span!("get_checkpoint");
        let state = self.write_guard.lock();
        let rope = state.engine.rope_clone();
        if let Some(rep) = &state.replica {
            let dump = (rope.to_string(), rep.encode(), self.lineage);
            return bincode::serde::encode_to_vec(&dump, bincode_config()).unwrap();
        }
        Vec::new()
    }

    pub fn get_engine_text(&self) -> String {
        self.publisher.load().to_string()
    }

    /// B3 接线:FJ 投递取回收通道 clone(免查 sessions 表竞态)。
    pub(crate) fn reaper_tx(&self) -> Option<ReaperTx> {
        self.full_reaper.clone()
    }

    /// When `suppress_hooks` is true, hook detection is skipped — used by snap restore.
    pub fn execute_transaction<F, R>(
        &self,
        write_steps: Option<&[Step]>,
        suppress_hooks: bool,
        f: F,
    ) -> (R, Vec<u8>)
    where
        F: FnOnce(&mut TransactionContext) -> R,
    {
        let _span = telemetry_span!("execute_txn", "doc_id" => self.doc_id);
        let _span_lock = telemetry_span!("txn_acquire_lock");
        let mut w_state = self.write_guard.lock();
        drop(_span_lock);
        let mut wire_ops = SmallVec::new();

        // ── incr_plan §6.2: capture pre-edit snapshot before ripple ──
        let snap_old = self.publisher.load();

        let mut ctx = TransactionContext {
            state: &mut w_state,
            wire_ops: &mut wire_ops,
            tx_min_dirty_line: usize::MAX,
        };

        let _span_closure = telemetry_span!("txn_closure");
        let result = f(&mut ctx);
        drop(_span_closure);
        let final_dirty_line = ctx.tx_min_dirty_line;
        drop(ctx);

        let _span_snapshot =
            telemetry_span!("snapshot_arc", "line_count" => w_state.engine.line_len());
        let new_snap = w_state.engine.make_snapshot();
        drop(_span_snapshot);
        let mut snap_arc = Arc::new(new_snap);
        let diff = w_state.engine.state.ripple_diff.take();
        // hook T1 剪枝信号：PL+FL 不变 ⟹ 纯内容编辑（无结构变化）。
        // 结构变化时 hook 保守全量评估。
        // 局部化——三截拼接构造推:lines[0,rls) 恒等(回退
        // 重扫值=旧值——文本未变、栈同)、tail 旧值保留 ⟹ 行级差异只在
        // [rls,rle];entries:前缀 I1 恒等、ripple 段=actions、尾段 δ 平移
        // (δ≠0 行标签不可比 → ripple_unchanged 恒 false → 保守)。O(doc)
        // 全量逐项比 → O(zone)。
        // [F22 修复] 接缝判定的两个**区间局部**信号：同时服务分类与 diag tag
        // （同一计算不重复）。旧 diag tag 直接比整列 PL 与整个 FrameLog entries，
        // δ=0 时恒等 ⇒ 每次写都做 O(doc) memcmp（~160KB），是纯测量成本却计进
        // `seam_classify`。由接缝三截构造引理：δ=0 时「全量相等 ⟺ 区间相等」
        // （前缀恒等 + tail 不平移），故局部信号与旧 tag 语义等价而只需 O(zone)。
        let (pl_zone_eq, fl_zone_eq) = match diff.as_ref() {
            Some(d) if d.delta == 0 => (
                lines_segment_eq(
                    &snap_arc.lines, &snap_old.lines,
                    d.ripple_line_start, d.ripple_line_end,
                ),
                crate::infer::ripple_unchanged(
                    &snap_old.frame_log.entries,
                    &d.ripple_frame_actions,
                    d.ripple_line_start,
                    d.ripple_line_end,
                ),
            ),
            // δ≠0：行标签不可比（原注释同律，保守判不等）
            _ => (false, false),
        };
        let structural_change = match diff.as_ref() {
            Some(d) => {
                snap_arc.lines.len() != snap_old.lines.len()
                    || snap_arc.frame_log.entries.len()
                        != snap_old.frame_log.entries.len()
                    || !pl_zone_eq
                    || !fl_zone_eq
            }
            None => {
                !crate::block::entries_eq_ignoring_consume(
                    &snap_old.frame_log.entries, &snap_arc.frame_log.entries)
                    || snap_old.lines.as_ref() != snap_arc.lines.as_ref()
            }
        };
        // 测量探针:seam 分类计数(内容/结构编辑——write 路径归因用)
        let _sp_sc = telemetry_span!(
            "seam_classify",
            "structural" => structural_change,
            "delta" => diff.as_ref().map_or(0, |d| d.delta),
            "pl_zone_eq" => pl_zone_eq,
            "fl_zone_eq" => fl_zone_eq
        );

        // ── Unified edit recording: drain once, split to WAL ──
        let edits: Vec<(usize, usize, String)> = w_state.edit_spans.drain(..).collect();
        // WAL recording
        // 单编辑 W_N = [start, start+len(text)); 携带 (start, new_end) 而非旧删除跨度 (start, old_end).
        let edit_ranges: Vec<(usize, usize)> =
            edits.iter().map(|(s, _e, t)| (*s, *s + t.len())).collect();
        // IncrTask 需要旧侧删除跨度 (start, old_end)：否则删除跨行时
        // compute_zone 只看到删除点，漏掉被删的旧结构。
        let incr_edit_ranges: Vec<(usize, usize)> =
            edits.iter().map(|(s, e, _)| (*s, *e)).collect();

        // ── Additive cache seam: stage a CacheSlot for the newly published snapshot. ──
        let rls = diff
            .as_ref()
            .map(|d| d.ripple_line_start)
            .unwrap_or(final_dirty_line);
        let delta = diff.as_ref().map_or(0, |d| d.delta);
        let new_n = snap_arc.line_len();
        let rle_e = diff.as_ref().map_or(rls.saturating_add(1), |d| {
            (d.ripple_line_end + 1).min(new_n)
        });
        // anchor 单一来源(与 compute_zone 共用 anchor_line):首编辑起点
        // 字节的旧侧行 + 钳制防御;空表回退 0(生产 Edit 必带编辑)。
        // 窄化载荷一次转换(4GB 上限断言落点),三消费点共享同一 Arc。
        let incr_er = crate::cache::edit_actions_arc(incr_edit_ranges.clone());
        let anchor = crate::incr::anchor_line(&snap_old, &incr_er);
        // 审查修订(开放项 5):edit_lines 不另存——物化 stage1 用 edit_ranges
        // (旧侧删除跨度,IncrTask 同款)字节精确映射。
        let slot_id = self.next_slot_id.fetch_add(1, Ordering::Relaxed);
        let new_slot = Arc::new(crate::cache::CacheSlot::new_with_id(slot_id));
        // P4b.1: attach the Weak before `CacheNode::new_edit` clones `snap_arc`
        // (after the clone, `Arc::get_mut` would fail for structural edits).
        if let Some(snap_ref) = Arc::get_mut(&mut snap_arc) {
            snap_ref.set_cache_slot(Arc::downgrade(&new_slot));
        }
        // The Weak back-pointer is the only old-node lookup. Map keys are
        // generation ids (never pointer addresses), so a stale pointer fallback
        // would be unsafe and has been removed.
        let old_node = snap_old
            .cache_slot
            .upgrade()
            .and_then(|slot| slot.node.load_full())
            .map(|node| (*node).clone());
        // Edit 无条件挂载——old_snap 走 Weak(保活边界 =
        // slot 保留 32 代),不再依赖 old_node 存在。kernel 从第一个结构
        // 编辑起可达(旧行为:无读方 → old_node None → Edit 不挂 → 写流
        // kernel override 零参与)。
        if structural_change {
            let edit_node = crate::cache::CacheNode::new_edit_full(
                &snap_old,
                snap_arc.clone(),
                (rls, rle_e),
                incr_er.clone(),
                delta,
                anchor,
            );
            new_slot.node.store(Some(Arc::new(edit_node)));        } else {
            // P5 pure-content seam: keep the old node as a pass-through
            // candidate. B1(#10):守卫裁决移写线程——发布即定型(现状首个
            // 读者裁决 → 本处直接复用 validate_content_overlay);通过 →
            // node 透传旧 Base + overlay 保留(字节面归属);拒绝 → 提升
            // Edit、overlay 释放。附带收益(828 §7.1):publish 前 Arc::get_mut
            // 可用——守卫内的 memo 继承(block_cache)按 &mut 传入生效,
            // 新快照继承旧快照的 selector 缓存壳(δ=0 行号键有效)。
            let mut overlay = crate::cache::ContentOverlay::new(
                crate::cache::edit_actions_arc(edit_ranges.clone()),
                crate::cache::edit_actions_arc(incr_edit_ranges.clone()),
                old_node,
                (rls, rle_e),
                delta,
                anchor,
            );
            let base_bytes = snap_old.cache_slot.upgrade().and_then(|old_slot| {
                let ov = old_slot.overlay.lock().clone()?;
                ov.root_render_cache.get().cloned()
            });
            if let Some(ov) = Arc::get_mut(&mut overlay) {
                ov.set_base_render_cache(base_bytes);
            }
            let resolved = crate::cache::validate_content_overlay(&new_slot, &overlay, &mut snap_arc);
            let passthrough = resolved.as_ref().map(|(_, p)| *p).unwrap_or(false);
            if passthrough {
                // 守卫通过:新 slot 已拥有独立新 Edit/Base;overlay 保留(字节面
                // 归属契约),overlay.base 用新节点,释放旧 node 引用。
                if let Some(ov) = Arc::get_mut(&mut overlay) {
                    ov.base = resolved.as_ref().map(|(n, _)| n.clone());
                }
                *new_slot.overlay.lock() = Some(overlay);
            }
            // 守卫拒绝:validate 已提升 Edit 并清空 overlay——无事可做。
        }

        let _span_publish = telemetry_span!("publish_snapshot");
        self.publisher.store(snap_arc.clone());
        drop(_span_publish);
        // B2b(#11):旧侧槽锚捕获——必须在 latest_published.store 新锚之前
        // (旧 latest 恒强引用在持,upgrade 必成 O(1));否则表改 Weak 后旧代
        // 槽随前任务收尾归零,adapt seg_old / block_header_lines 旧侧 /
        // old_block_index_of 每代 3-4 次 779µs 自建。
        let old_slot_handle = snap_old
            .cache_slot
            .upgrade()
            .map(crate::cache::CacheSlotHandle);
        // B3 写线程换代半钩子(§5.3 顺序约束):判旧 latest 槽必须先于
        // latest_published.store 新锚——旧锚 + 本执行体克隆 = 2 持者,
        // ==2 即无任务 pins → swap-out {full, render} 送 reaper(O(doc)
        // drop 写线程不付)。
        if let Some(ref tx) = self.full_reaper {
            let old_latest = self.latest_published.load_full();
            reaper_take_if_last(&*old_latest, tx, 2);
        }
        // B2a:表 Weak 登记(表不保活)+ latest 恒强锚换代——保留半径恒 1
        // ({latest} ∪ 任务 pins);FIFO/32 预算统计释放块删除(编辑轴
        // 代数不再决定存续权)。
        self.cache_slots.insert(slot_id, Arc::downgrade(&new_slot));
        self.latest_published.store(Arc::new(new_slot.clone()));

        // Async hook detection —— 必须在 drop(w_state) 之前发送：
        // 锁内发送 ⟹ 同 doc 任务入队顺序 = 事务提交顺序（FIFO 保证的
        // 第三环）；unbounded channel send 不阻塞，锁持有增量 = 任务
        // 构造的常数成本。suppress 写也进检测（消化 cached），只抑制投递。
        if self.hook_docs.contains(&self.doc_id) {
            if let Some(ref tx) = self.detect_tx {
                let _span = telemetry_span!("hook_detect_send", "doc_id" => self.doc_id);
                let (rls, rle) = match &diff {
                    Some(d) => (d.ripple_line_start, d.ripple_line_end),
                    None => {
                        let l = final_dirty_line;
                        (l, l.saturating_add(1))
                    }
                };
                let rfa = diff
                    .as_ref()
                    .map_or(vec![], |d| d.ripple_frame_actions.clone());
                // 精确编辑行区间：起点用旧快照（编辑前行号）、终点用新快照
                // （编辑后行号）+1。!structural_change 时行数不变（PL+FL 逐项
                // 相等 ⟹ 行号系统一致），两快照行号可混用；内容编辑只改这里。
                let els = edit_ranges
                    .iter()
                    .map(|(s, _)| snap_old.line_of_byte(*s))
                    .min()
                    .unwrap_or(rls);
                let ele = edit_ranges
                    .iter()
                    .map(|(s, e)| snap_arc.line_of_byte((*e).min(snap_arc.byte_len())))
                    .max()
                    .map(|l| l.saturating_add(1))
                    .unwrap_or(rle);
                // 仍在 write_guard 内：分配本快照的写序序号（注册水位比较用）。
                let snap_seq = self.detect_clock.assign();
                let task = match write_steps {
                    Some(steps) => DetectTask::SelectorWrite {
                        doc_id: self.doc_id,
                        snap_seq,
                        write_steps: steps.to_vec(),
                        new_snap: snap_arc.clone(),
                        ripple_line_start: rls,
                        ripple_line_end: rle,
                        ripple_frame_actions: rfa,
                        structural_change,
                        edit_line_start: els,
                        edit_line_end: ele,
                        suppress_events: suppress_hooks,
                        slot: crate::cache::CacheSlotHandle(new_slot.clone()),
                        reaper_tx: self.full_reaper.clone(),
                    },
                    None => DetectTask::CrdtWrite {
                        doc_id: self.doc_id,
                        snap_seq,
                        new_snap: snap_arc.clone(),
                        ripple_line_start: rls,
                        ripple_line_end: rle,
                        ripple_frame_actions: rfa,
                        structural_change,
                        edit_line_start: els,
                        edit_line_end: ele,
                        suppress_events: suppress_hooks,
                        slot: crate::cache::CacheSlotHandle(new_slot.clone()),
                        reaper_tx: self.full_reaper.clone(),
                    },
                };
                let _ = tx.send(task);
            }
        }

        drop(w_state);

        // Mark dirty and push to WAL
        self.dirty.store(true, Ordering::Relaxed);
        if let Some(ref wq) = self.wal_queue {
            let entries: Vec<WalEntry> = edits
                .into_iter()
                .map(|(start, end, text)| WalEntry::Edit(EditRecord { start, end, text }))
                .collect();
            if !entries.is_empty() {
                let _span = telemetry_span!("wal_push", "record_count" => entries.len());
                wq.push((self.storage_key.clone(), entries));
            }
        }

        // Step 5: incremental AST patch — incr_plan §6.2 three-signal path
        let has_content_change = !edit_ranges.is_empty() || diff.is_some();
        if diff.is_some() || has_content_change {
            let (d, rls, rle) = match &diff {
                Some(d) => (
                    d.delta,
                    d.ripple_line_start,
                    d.ripple_line_end,
                ),
                None => {
                    // No frame change — ripple lines from edit ranges only
                    let rls = edit_ranges
                        .iter()
                        .map(|(s, _)| snap_old.line_of_byte(*s))
                        .min()
                        .unwrap_or(0);
                    let rle = edit_ranges
                        .iter()
                        .map(|(s, e)| {
                        let end = if *e > *s { *e - 1 } else { *s };
                        snap_old.line_of_byte(end.min(snap_old.byte_len().saturating_sub(1)))
                        })
                        .max()
                        .unwrap_or(0);
                    (0isize, rls, rle)
                }
            };
            if let Some(ref tx) = self.path_tx {
                let _ = tx.send(IncrTask {
                    snap: snap_arc.clone(),
                    doc_id: self.doc_id,
                    snap_old: snap_old.clone(),
                    delta: d,
                    edit_ranges: incr_er.clone(),
                    ripple_line_start: rls,
                    ripple_line_end: rle,
                    slot: Some(crate::cache::CacheSlotHandle(new_slot.clone())),
                    slot_old: old_slot_handle.clone(),
                    reaper_tx: self.full_reaper.clone(),
                    structural_change,
                });
            }
        }

        let op_bytes = if wire_ops.is_empty() {
            Vec::new()
        } else {
            let _span = telemetry_span!("bincode_encode", "op_count" => wire_ops.len());
            bincode::serde::encode_to_vec(&wire_ops, bincode_config()).unwrap()
        };
        (result, op_bytes)
    }
}

#[cfg(test)]
mod cache_seam_tests {
    use super::*;

    fn test_session(text: &str) -> MdioSession {
        MdioSession::new(
            1,
            text,
            1,
            "test".to_string(),
            None,
            None,
            None,
            Arc::new(DashSet::new()),
            Arc::new(DetectClock::new()),
        )
    }

    /// [fix] checkpoint 的 stored_text 与调用方 current_text 不同（iroh 挂载路径
    /// 上游接骨归一化会补一个尾换行）时，engine 必须取 stored_text——否则
    /// engine/replica 长度错位，末尾写入会在 cola 边界 panic。
    #[test]
    fn from_checkpoint_engine_takes_stored_text_when_caller_differs() {
        let stored = "abc".to_string();
        let rep = Replica::new(7, stored.len());
        let encoded = rep.encode();
        let ckpt = bincode::serde::encode_to_vec(&(stored.clone(), encoded, 7u64), bincode_config())
            .expect("encode checkpoint");
        // 调用方文本多一个尾换行（模拟 write_locator 接骨归一化后的结果）
        let session = MdioSession::from_checkpoint(
            1, &ckpt, 9, "abc\n", "k".to_string(), None, None, None,
            Arc::new(DashSet::new()),
            Arc::new(DetectClock::new()),
        )
        .expect("checkpoint load");
        assert_eq!(session.get_engine_text(), "abc", "engine 必须取 checkpoint 的 stored_text");
        // 文档末尾写入：修复前这里会 panic（rep.inserted(4) 越界，replica 长 3）
        session.execute_transaction(None, true, |ctx| {
            ctx.apply_mdio_op_multi(
                vec![crate::op::WriteEdit { range: 3..3, text: "X".to_string() }],
                "+>",
            );
        });
        assert_eq!(session.get_engine_text(), "abcX");
    }

    #[test]
    fn structural_edit_creates_edit_node() {
        let session = test_session("hello\n");
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0);
            ctx.state.engine.edit(start, start, "# ");
        });

        // 827-A1:槽表含初始代(id 1)+ 编辑代——共 2;取挂 Edit 节点的代。
        assert_eq!(session.cache_slots.len(), 2);
        let slot = session
            .cache_slots
            .iter()
            .find_map(|s| {
                s.value()
                    .upgrade()
                    .filter(|v| v.node.load_full().is_some())
            })
            .expect("edit generation must mount a node");
        // 结构编辑无条件挂 Edit(不再依赖 old_node 存在)——
        // 冷启动(无旧读方)也从第一个结构编辑起挂 Edit。
        assert!(matches!(
            &**slot.node.load_full().expect("Edit must mount"),
            crate::cache::CacheNode::Edit { .. }
        ));
        assert!(slot.overlay.lock().is_none());

        // P4b.1 seam: the published snapshot's Weak upgrades to the same
        // CacheSlot Arc that `cache_slots` holds; the write thread built only
        // the Edit descriptor (lazy materialization belongs to readers).
        let published = session.publisher.load();
        let upgraded = published
            .cache_slot
            .upgrade()
            .expect("published snapshot Weak must upgrade");
        let map_weak = session
            .cache_slots
            .get(&upgraded.id)
            .map(|w| w.upgrade())
            .flatten()
            .expect("published snapshot has a slot");
        assert!(Arc::ptr_eq(&upgraded, &map_weak));
        assert!(matches!(
            &**upgraded.node.load_full().unwrap(),
            crate::cache::CacheNode::Edit { .. }
        ));
        assert!(upgraded.overlay.lock().is_none());
    }

    /// B2a 生命周期判官(表 Weak 淘汰 + latest 恒锚):连续结构编辑且每轮
    /// 读方物化——旧代槽随最后 Arc 归零(表不保活),唯一恒活 = latest 锚
    /// (与任务 pins;本测试无任务通道 → pins 为空)。
    #[test]
    fn b2a_weak_table_drops_old_generations_anchor_keeps_latest() {
        let session = test_session("# S\n\nbody\n");
        let mut seen_ids: Vec<u64> = Vec::new();
        for i in 0..6 {
            session.execute_transaction(None, true, |ctx| {
                let len = ctx.state.engine.byte_len();
                ctx.state.engine.edit(len, len, &format!("- item {i}\n"));
            });
            let published = session.publisher.load();
            let slot = published.cache_slot.upgrade().expect("published slot alive");
            seen_ids.push(slot.id);
            let node = crate::cache::ensure_node(&slot, published.clone());
            let _ = node.materialize_full();
        }
        // 表值 Weak:最新代恒活(latest 锚),旧代随上一轮循环的局部 Arc
        // 释放归零(表内 Weak 升级失败)。
        let latest = session.publisher.load();
        let latest_slot = latest.cache_slot.upgrade().expect("latest anchored");
        assert_eq!(latest_slot.id, *seen_ids.last().unwrap());
        for old_id in seen_ids.iter().take(seen_ids.len() - 1) {
            let dangling = session
                .cache_slots
                .get(old_id)
                .and_then(|w| w.upgrade());
            assert!(
                dangling.is_none(),
                "old generation {old_id} must die with its last Arc (table Weak)"
            );
        }
        // latest 锚 == 表内同 Arc 的升级。
        let anchored = session.latest_published.load_full();
        assert_eq!(anchored.id, *seen_ids.last().unwrap());
        assert!(Arc::ptr_eq(&*anchored, &latest_slot));
    }

    /// 定向等价门:局部化 structural_change == 全量逐项比。
    /// 随机文档 × 随机编辑(结构/内容混合),两式结果必须一致。
    #[test]
    fn localized_seam_classify_equals_full_compare() {
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
                "- item\n",
                "- [ ] task\n",
                "1. first\n",
                "  - nested\n",
                "> quote\n",
                "```rust\nlet x = 1;\n```\n",
                "\n",
                "    indented code\n",
                "[id]: https://example.com \"t\"\n",
                "| a | b |\n|---|---|\n| c | d |\n",
            ]
        }
        let mut rng: u64 = 0x2531_0124_ABCD_0001;
        let mut checked = 0usize;
        for case in 0..200u64 {
            let mut doc = String::new();
            let line_count = 3 + (next_rand(&mut rng) % 12) as usize;
            for _ in 0..line_count {
                doc.push_str(tokens()[next_rand(&mut rng) as usize % tokens().len()]);
            }
            let mut state = crate::projection::State::new(&doc);
            let old_snap = Arc::new(state.snapshot());
            let len = doc.len();
            let (start, end, text) = if len == 0 || next_rand(&mut rng) % 2 == 0 {
                let pos = next_rand(&mut rng) as usize % (len + 1);
                let tok = tokens()[next_rand(&mut rng) as usize % tokens().len()];
                (pos, pos, tok.to_string())
            } else {
                let start = next_rand(&mut rng) as usize % len;
                let end = start + 1 + next_rand(&mut rng) as usize % (len - start);
                (start, end, String::new())
            };
            state.edit(start, end, &text);
            let new_snap = Arc::new(state.snapshot());
            let diff = state.ripple_diff.take();
            // 全量。
            let full = !crate::block::entries_eq_ignoring_consume(
                &old_snap.frame_log.entries, &new_snap.frame_log.entries)
                || old_snap.lines.as_ref() != new_snap.lines.as_ref();
            // 局部化(新实现——seam 同式)。
            let localized = match diff.as_ref() {
                Some(d) => {
                    let rls = d.ripple_line_start;
                    let rle = d.ripple_line_end;
                    new_snap.lines.len() != old_snap.lines.len()
                        || new_snap.frame_log.entries.len()
                            != old_snap.frame_log.entries.len()
                        || !lines_segment_eq(&new_snap.lines, &old_snap.lines, rls, rle)
                        || !crate::infer::ripple_unchanged(
                            &old_snap.frame_log.entries,
                            &d.ripple_frame_actions,
                            rls,
                            rle,
                        )
                }
                None => {
                    !crate::block::entries_eq_ignoring_consume(
                        &old_snap.frame_log.entries, &new_snap.frame_log.entries)
                        || old_snap.lines.as_ref() != new_snap.lines.as_ref()
                }
            };
            checked += 1;
            assert_eq!(
                full, localized,
                "case={case}: full={full} localized={localized} doc={doc:?} edit=({start},{end})→{text:?}"
            );
        }
    }

    #[test]
    fn seam_edit_node_materializes_covering_zone() {
        // 端到端(seam → 第一读者):事务发布后,读者 ensure_node 物化 Edit,
        // zone 覆盖编辑行且 mid 段包含变化块。
        let session = test_session("# Title\n\npara one\n\n- a\n- b\n");
        // 预热:首个事务冷启动(无旧节点 → 读者建 Base)。
        session.execute_transaction(None, true, |ctx| {
            let len = ctx.state.engine.byte_len();
            ctx.state.engine.edit(len, len, "\n");
        });
        {
            let s0 = session.publisher.load();
            let slot0 = s0.cache_slot.upgrade().expect("slot0");
            let _ = crate::cache::ensure_node(&slot0, s0.clone());
        }
        // 第二个事务:有旧节点 → seam 创建 Edit 链。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(2);
            ctx.state.engine.edit(start, start, "- new\n");
        });
        let published = session.publisher.load();
        let slot = published
            .cache_slot
            .upgrade()
            .expect("published snapshot Weak must upgrade");
        let node = crate::cache::ensure_node(&slot, published.clone());
        assert!(
            matches!(&*node, crate::cache::CacheNode::Edit { .. }),
            "structural edit with a warm base must produce an Edit node"
        );
        let (zs, ce) = node.materialize_zone();
        assert!(zs <= 2 && 2 < ce, "zone {zs}..{ce} must cover the edited line");
        let mid = node.materialize_mid().expect("Edit node has a mid segment");
        assert_eq!((mid.start_line, mid.end_line), (zs, ce));
        assert!(
            mid.blocks
                .iter()
                .any(|b| (b.start as usize) <= 2 && 2 < (b.end as usize)),
            "mid must contain the edited block: {:?}",
            mid.blocks
                .iter()
                .map(|b| (b.start, b.end))
                .collect::<Vec<_>>()
        );
    }

    /// B2a 生命周期判官:无任务/无读者下连续编辑——publisher 快照的 slot
    /// 恒可 upgrade(latest 锚),表内 Weak 不保活旧代(radius-1);地图条目
    /// 只持死 Weak(淘汰零代码——B3 半钩子补地图清理点)。
    #[test]
    fn b2a_published_slot_always_upgrades_via_anchor() {
        let session = test_session("a\n");
        for _ in 0..40 {
            session.execute_transaction(None, true, |ctx| {
                let len = ctx.state.engine.byte_len();
                ctx.state.engine.edit(len, len, "x");
            });
        }
        let published = session.publisher.load();
        let slot = published
            .cache_slot
            .upgrade()
            .expect("latest anchored: publisher slot must upgrade");
        assert_eq!(session.latest_published.load_full().id, slot.id);
        // 表 Weak:仅 latest 锚活;旧代 Weak 全部悬空。
        let alive: Vec<u64> = session
            .cache_slots
            .iter()
            .filter(|e| e.value().strong_count() > 0)
            .map(|e| *e.key())
            .collect();
        assert_eq!(alive, vec![slot.id], "radius-1: only latest alive without task pins");
    }

    #[test]
    fn content_edit_with_warm_base_passes_guard_at_publish() {
        let session = test_session("hello world\n");

        // First content edit + first reader: cold Base for the new snapshot.
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0) + 6;
            let end = start + 5;
            ctx.state.engine.edit(start, end, "there");
        });
        let snap1 = session.publisher.load();
        let slot1 = snap1
            .cache_slot
            .upgrade()
            .expect("published snapshot has slot");
        assert!(slot1.node.load_full().is_none());
        let node1 = crate::cache::ensure_node(&slot1, snap1.clone());
        assert!(matches!(&*node1, crate::cache::CacheNode::Base { .. }));

        // Second pure-content edit: 守卫在发布时定型——透传旧 Base +
        // overlay 保留(字节面归属);读者拿到的即发布产物。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0) + 6;
            let end = start + 5;
            ctx.state.engine.edit(start, end, "hello");
        });
        let snap2 = session.publisher.load();
        let slot2 = snap2
            .cache_slot
            .upgrade()
            .expect("published snapshot has slot");
        let published = slot2
            .node
            .load_full()
            .expect("发布即定型:守卫裁决必在写线程内完成");
        assert!(matches!(&**published, crate::cache::CacheNode::Base { .. }));
        assert!(slot2.overlay.lock().is_some(), "透传后 overlay 保留(字节面归属)");

        let node2 = crate::cache::ensure_node(&slot2, snap2.clone());
        assert!(matches!(&*node2, crate::cache::CacheNode::Base { .. }));
        assert!(
            !Arc::ptr_eq(&node1, &node2),
            "guard must create an independent new node"
        );
    }

    /// B1 契约更新:冷启动内容编辑(无旧节点——守卫无 base 可透传)发布
    /// 后 node/overlay 皆空,读者 new_base 冷建;Weak 契约不变。
    #[test]
    fn content_edit_cold_start_publishes_empty_slot() {
        let session = test_session("hello\n");
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0) + 1;
            ctx.state.engine.edit(start, start, "x");
        });

        // 827-A1:槽表含初始代 + 编辑代——共 2。
        assert_eq!(session.cache_slots.len(), 2);

        // P4b.1 seam: same Weak-upgrade contract for content edits.
        let published = session.publisher.load();
        let upgraded = published
            .cache_slot
            .upgrade()
            .expect("published snapshot Weak must upgrade");
        let map_weak = session
            .cache_slots
            .get(&upgraded.id)
            .map(|w| w.upgrade())
            .flatten()
            .expect("published snapshot has a slot");
        assert!(Arc::ptr_eq(&upgraded, &map_weak));
        // B1:冷启动无 base 可透传——发布后 node/overlay 皆空,读者冷建。
        assert!(upgraded.node.load_full().is_none());
        assert!(upgraded.overlay.lock().is_none());
        let node = crate::cache::ensure_node(&upgraded, published.clone());
        assert!(matches!(&*node, crate::cache::CacheNode::Base { .. }));
    }

    /// B1 守卫等价判官(判官先行,§9):纯内容编辑后守卫裁决在写线程内
    /// 定型(发布即定型)——发布后槽状态 == 首个读者裁决结果(透传 Base
    /// 或提升 Edit);字节面 == 无槽冷路径。现状(裁决延迟到读者)下必红。
    #[test]
    fn b1_guard_resolves_passthrough_at_publish() {
        let session = test_session("hello world\n");
        // 暖基:首次内容编辑 + 首读建 Base。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0) + 6;
            ctx.state.engine.edit(start, start + 5, "there");
        });
        let snap1 = session.publisher.load();
        let slot1 = snap1.cache_slot.upgrade().expect("slot1");
        let node1 = crate::cache::ensure_node(&slot1, snap1.clone());
        assert!(matches!(&*node1, crate::cache::CacheNode::Base { .. }));

        // 第二次纯内容编辑:守卫必须在发布时定型(透传 Base)。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0) + 6;
            ctx.state.engine.edit(start, start + 5, "hello");
        });
        let snap2 = session.publisher.load();
        let slot2 = snap2.cache_slot.upgrade().expect("slot2");
        let published_node = slot2
            .node
            .load_full()
            .expect("发布即定型:守卫裁决必在写线程内完成");
        assert!(
            matches!(&**published_node, crate::cache::CacheNode::Base { .. }),
            "守卫通过必须于发布时透传独立 Base"
        );
        assert!(slot2.overlay.lock().is_some(), "透传后 overlay 保留(字节面归属)");
        // 字节面 == 无槽冷路径。
        let cached = snap2.to_ast_object_mode(crate::topology::ProjectionMode::Cf);
        let cold = crate::projection::State::new(&snap2.rope.to_string())
            .snapshot()
            .to_ast_object_mode(crate::topology::ProjectionMode::Cf);
        let cv: serde_json::Value = serde_json::from_str(&cached).unwrap();
        let ov: serde_json::Value = serde_json::from_str(&cold).unwrap();
        assert_eq!(cv["ct"], ov["ct"], "发布即定型字节 == 无槽冷路径");
    }

    /// B1 继承判官(判官先行——修复前红,修复后绿):828 §7.1"守卫移写
    /// 线程附带 memo 继承成功率提升(publish 前 Arc::get_mut 可用)"的
    /// 兑现面——发布即定型路径下,守卫通过的纯内容编辑发布时,新快照
    /// 必须继承旧快照的 selector 缓存壳(block_cache 壳 Arc 同指针;
    /// 旧壳内容含旧快照上已暖的查询键,δ=0 下行号键有效)。修复前调用
    /// 点 clone 传参致 get_mut 恒失败 → 新壳 → 本判官红。
    #[test]
    fn b1_pure_content_publish_inherits_block_cache_shell() {
        let session = test_session("hello world\n");
        // 暖基:一次内容编辑 + 首读建 Base(守卫透传需要 overlay.base)。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0) + 6;
            ctx.state.engine.edit(start, start + 5, "there");
        });
        let snap1 = session.publisher.load();
        let slot1 = snap1.cache_slot.upgrade().expect("slot1");
        let _ = crate::cache::ensure_node(&slot1, snap1.clone());

        // 第二次纯内容编辑:发布时守卫通过(透传)→ 继承必须发生。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0) + 6;
            ctx.state.engine.edit(start, start + 5, "hello");
        });
        let snap2 = session.publisher.load();
        assert!(
            std::sync::Arc::ptr_eq(
                snap1.block_cache.as_ref().expect("shell 1"),
                snap2.block_cache.as_ref().expect("shell 2"),
            ),
            "纯内容编辑发布后,新快照必须继承旧快照的 selector 缓存壳"
        );
    }
    /// A 档继承判官：守卫通过的
    /// 纯内容编辑发布时，新快照的 Base 节点必须**共享**旧节点的三张行号级
    /// 派生索引（root_table / root_blocks_table / block_spans）。依据：透传
    /// ⟹ δ=0 + [rls,rle] 结构恒等 + 区间外前缀不变性 ⇒ 三张表逐值相同；
    /// 源可以是 Base 或 Edit（结构编辑后首代是 Edit，其索引由该次 Path 物化）。
    /// 实测背景：漏接使内容编辑形状下 rcb_n = 188.4µs@1500 节（Path 的 91.2%）。
    #[test]
    fn passthrough_inherits_a3_derived_indices() {
        let session = test_session("# H\n\npara a\n\npara b\n\n");
        // 暖基：一次内容编辑 + 首读建 Base（守卫透传需要 overlay.base）。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(2) + 5;
            ctx.state.engine.edit(start, start + 1, "x");
        });
        let snap1 = session.publisher.load();
        let slot1 = snap1.cache_slot.upgrade().expect("slot1");
        let _ = crate::cache::ensure_node(&slot1, snap1.clone());
        // 物化三张表（模拟"上一代已建"）。
        let rt1 = crate::cache::kernel_root_table(&snap1).expect("root_table");
        let rbt1 = crate::cache::kernel_root_blocks_table(&snap1).expect("root_blocks_table");
        let bs1 = crate::cache::kernel_block_spans_full(&snap1).expect("block_spans");

        // 第二次纯内容编辑：发布时守卫通过（透传）→ 三张表必须共享同一 Arc。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(2) + 5;
            ctx.state.engine.edit(start, start + 1, "y");
        });
        let snap2 = session.publisher.load();
        let rt2 = crate::cache::kernel_root_table(&snap2).expect("root_table2");
        let rbt2 = crate::cache::kernel_root_blocks_table(&snap2).expect("root_blocks_table2");
        let bs2 = crate::cache::kernel_block_spans_full(&snap2).expect("block_spans2");
        assert!(std::sync::Arc::ptr_eq(&rt1, &rt2), "root_table 应跨透传共享");
        assert!(std::sync::Arc::ptr_eq(&rbt1, &rbt2), "root_blocks_table 应跨透传共享");
        assert!(std::sync::Arc::ptr_eq(&bs1, &bs2), "block_spans 应跨透传共享");
    }

    /// B1 oidx 继承判官(判官先行——修复前红,修复后绿):守卫通过的
    /// 纯内容编辑发布时,新快照必须继承旧快照的旧块索引 Arc(δ=0
    /// 全局结构同一 → 块索引逐字节相同;判据 = 既有守卫裁决,无新
    /// 判断)。前提 = 旧索引已初始化(链首/结构后未初始化 → 跳过,
    /// 首次消费裸建)。修复前守卫分支无索引继承 → 新快照未初始化 →
    /// 本判官红。
    #[test]
    fn b1_pure_content_publish_inherits_old_block_index() {
        let session = test_session("hello world\n");
        // 暖基:一次内容编辑 + 首读建 Base。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0) + 6;
            ctx.state.engine.edit(start, start + 5, "there");
        });
        let snap1 = session.publisher.load();
        let slot1 = snap1.cache_slot.upgrade().expect("slot1");
        let _ = crate::cache::ensure_node(&slot1, snap1.clone());
        // 暖旧索引(继承的前提 = 旧索引已初始化)。
        let idx1 = crate::cache::old_block_index_of_own(&snap1).expect("idx1");

        // 第二次纯内容编辑:守卫通过 → 索引继承必须发生。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(0) + 6;
            ctx.state.engine.edit(start, start + 5, "hello");
        });
        let snap2 = session.publisher.load();
        // [归位 S2] 断言对象 = 格入口（同源：BlockSpans 格）。
        let idx2 = crate::cache::old_block_index_of_own(&snap2).expect("inherited index");
        assert!(
            std::sync::Arc::ptr_eq(&idx1, &idx2),
            "守卫通过必须继承旧快照的旧块跨度格 Arc"
        );
    }

    /// B1 oidx 结构轮护栏(现状即绿——防继承修复误伤):结构编辑
    /// (structural_change=true)走提升 Edit 分支,新快照不得继承旧索引
    /// (键失效,首次消费必须裸建)。
    #[test]
    fn b1_structural_publish_does_not_inherit_old_block_index() {
        let session = test_session("# S\n\nbody\n");
        session.execute_transaction(None, true, |ctx| {
            let len = ctx.state.engine.byte_len();
            ctx.state.engine.edit(len, len, "- item\n");
        });
        let snap1 = session.publisher.load();
        let slot1 = snap1.cache_slot.upgrade().expect("slot1");
        let _ = crate::cache::ensure_node(&slot1, snap1.clone());
        let _ = crate::cache::old_block_index_of_own(&snap1);
        // 结构编辑:追加一行 → structural_change=true → 提升 Edit。
        session.execute_transaction(None, true, |ctx| {
            let len = ctx.state.engine.byte_len();
            ctx.state.engine.edit(len, len, "- item2\n");
        });
        let snap2 = session.publisher.load();
        // [归位 S2] 断言"不共享"（结构轮格重建，Arc 不同）。
        let idx1 = crate::cache::old_block_index_of_own(&snap1).expect("idx1");
        let idx2 = crate::cache::old_block_index_of_own(&snap2).expect("idx2");
        assert!(
            !std::sync::Arc::ptr_eq(&idx1, &idx2),
            "结构轮不得共享旧块跨度格(键失效,首次消费重建)"
        );
    }

    /// B1 守卫等价判官(拒绝面):setext level 改写——行级信号纯内容
    /// (PL/FL 逐项相等)但守卫 zone 内块级重分类 → 发布时提升 Edit、
    /// overlay 释放。现状(裁决延迟)下必红。
    #[test]
    fn b1_guard_promotes_at_publish_on_setext_level_change() {
        let session = test_session("# S\n\nfoo\n===\n\npara\n");
        // 暖基:一次编辑 + 首读。
        session.execute_transaction(None, true, |ctx| {
            let len = ctx.state.engine.byte_len();
            ctx.edit(len, len, "\n");
        });
        let snap1 = session.publisher.load();
        let slot1 = snap1.cache_slot.upgrade().expect("slot1");
        let _ = crate::cache::ensure_node(&slot1, snap1.clone());
        // setext level 改写:"===" → "---"(HsSep 不变,块级 HsHead1→HsHead2)。
        let pos = session.get_engine_text().find("===").unwrap();
        let _span = crate::telemetry_span!("b1_judge_promote", "pos" => pos as u64);
        session.execute_transaction(None, true, |ctx| {
            ctx.edit(pos, pos + 3, "---");
        });
        let snap2 = session.publisher.load();
        let slot2 = snap2.cache_slot.upgrade().expect("slot2");
        let published_node = slot2
            .node
            .load_full()
            .expect("发布即定型:守卫裁决必在写线程内完成");
        let node_kind = match &**published_node {
            crate::cache::CacheNode::Edit { .. } => "Edit",
            crate::cache::CacheNode::Base { .. } => "Base",
        };
        assert!(
            matches!(&**published_node, crate::cache::CacheNode::Edit { .. }),
            "守卫拒绝必须于发布时提升 Edit, got {node_kind}"
        );
        assert!(slot2.overlay.lock().is_none(), "提升后 overlay 释放");
    }

    /// B2b 任务锚基线(#11/#27/#46):投递的 IncrTask 带新侧 + 旧侧槽锚
    /// (旧锚在 latest.store 之前捕获——表 Weak 后旧代槽随前任务收尾
    /// 归零,无锚则 adapt 旧侧每代自建);DetectTask 带本代槽锚。
    #[test]
    fn b2b_tasks_carry_slot_anchors() {
        use crate::store::{DetectTask, IncrTask};
        let (path_tx, path_rx) = crossbeam::channel::unbounded::<IncrTask>();
        let (detect_tx, detect_rx) = crossbeam::channel::unbounded::<DetectTask>();
        let hook_docs: Arc<DashSet<u32>> = Arc::new(DashSet::new());
        hook_docs.insert(1u32);
        let session = MdioSession::new(
            1,
            "# S\n\nbody\n",
            1,
            "test".to_string(),
            None,
            Some(detect_tx),
            Some(path_tx),
            hook_docs,
            Arc::new(DetectClock::new()),
        );
        session.execute_transaction(None, false, |ctx| {
            let start = ctx.state.engine.byte_of_line(2);
            ctx.state.engine.edit(start, start, "- new\n");
        });
        // IncrTask:新侧 + 旧侧锚齐备,新侧 = latest。
        let task = path_rx.recv().expect("incr task delivered");
        let new_anchor = task.slot.as_ref().expect("new-side anchor").0.clone();
        let old_anchor = task.slot_old.as_ref().expect("old-side anchor").0.clone();
        assert_eq!(new_anchor.id, session.latest_published.load_full().id);
        assert!(old_anchor.id < new_anchor.id, "old anchor is a prior generation");
        // DetectTask:本代槽锚(latest)。
        let dtask = detect_rx.recv().expect("detect task delivered");
        let dslot = match &dtask {
            DetectTask::SelectorWrite { slot, .. } | DetectTask::CrdtWrite { slot, .. } => {
                slot.0.clone()
            }
        };
        assert_eq!(dslot.id, new_anchor.id, "DetectTask carries the current slot anchor");
    }

    /// B3 释放路径判官(§9 强制级夹具:队列深度>1 + 积压期内存含未取
    /// 大件 + 文档头编辑 zone=全量):连续结构编辑且每轮物化 full,换代
    /// 半钩子对旧 latest(仅剩旧锚 + 执行体克隆 = 2 持者)取物——旧代
    /// full/render 送 reaper,最新代完好;mid 例外(OnceLock)不受影响。
    #[test]
    fn b3_writer_half_hook_releases_old_latest_full() {
        let session = test_session("# S\n\nbody\n");
        let mut latest_ids: Vec<u64> = Vec::new();
        for i in 0..4 {
            session.execute_transaction(None, true, |ctx| {
                let len = ctx.state.engine.byte_len();
                ctx.state.engine.edit(len, len, &format!("- item {i}\n"));
            });
            let published = session.publisher.load();
            let slot = published.cache_slot.upgrade().expect("latest anchored");
            latest_ids.push(slot.id);
            let node = crate::cache::ensure_node(&slot, published.clone());
            let _ = node.materialize_full();
        }
        // 每一轮换代:半钩子已对"上一轮 latest"取物——旧代槽随最后 Arc
        // 归零(表 Weak + 无任务 pins)。最新代 full 完好:
        let latest = session.publisher.load();
        let lslot = latest.cache_slot.upgrade().expect("latest anchored");
        let lnode = lslot.node.load_full().expect("latest has node");
        match &**lnode {
            crate::cache::CacheNode::Edit { full, .. } => {
                assert!(full.load_full().is_some(), "latest full intact after half-hook");
            }
            _ => panic!("latest must be Edit"),
        }
        // 文档头编辑(zone=全量,mid 退化):结构正确性不受半钩子影响。
        let _ = latest;
        session.execute_transaction(None, true, |ctx| {
            ctx.state.engine.edit(0, 0, "# Head\n\n");
        });
        let head_snap = session.publisher.load();
        let head_slot = head_snap.cache_slot.upgrade().expect("latest anchored");
        let head_node = crate::cache::ensure_node(&head_slot, head_snap.clone());
        let full = head_node.materialize_full().expect("full assembles after head edit");
        let oracle = crate::cache::Segment::build(&head_snap, 0, head_snap.line_len());
        assert!(
            crate::cache::records_structurally_eq(
                &full.records, &full.arena, &oracle.records, &oracle.arena
            ),
            "head-edit full == oracle"
        );
        assert_eq!(full.blocks, oracle.blocks, "head-edit blocks == oracle");
    }

    /// B3 释放路径判官(强制级:链中段死亡——链长 6 只保最近 2 代):
    /// 中段旧代槽随 Weak 归零,链尾组装断链 → 裸建入槽单构建,产物与
    /// 全量 oracle 逐字段相等(正确性不变)。
    #[test]
    fn b3_chain_mid_death_bare_builds_equals_oracle() {
        let session = test_session("# S\n\nbody\n");
        let mut mid_slots: Vec<std::sync::Arc<crate::cache::CacheSlot>> = Vec::new();
        for i in 0..6 {
            session.execute_transaction(None, true, |ctx| {
                let len = ctx.state.engine.byte_len();
                ctx.state.engine.edit(len, len, &format!("## S{i}\n\npara\n"));
            });
            let published = session.publisher.load();
            let slot = published.cache_slot.upgrade().expect("latest anchored");
            if i < 2 {
                mid_slots.push(slot.clone());
            }
            let node = crate::cache::ensure_node(&slot, published.clone());
            let _ = node.materialize_full();
        }
        // 释放中段 pin(只留 latest)——中段槽归零。
        drop(mid_slots);
        let latest = session.publisher.load();
        let lslot = latest.cache_slot.upgrade().expect("latest anchored");
        let lnode = crate::cache::ensure_node(&lslot, latest.clone());
        // 中段死亡 → 组装断链 → 裸建入槽;产物 == 全量 oracle。
        let full = lnode.materialize_full().expect("bare-build into slot");
        let oracle = crate::cache::Segment::build(&latest, 0, latest.line_len());
        assert!(
            crate::cache::records_structurally_eq(
                &full.records, &full.arena, &oracle.records, &oracle.arena
            ),
            "mid-death full == oracle"
        );
        assert_eq!(full.blocks, oracle.blocks, "mid-death blocks == oracle");
    }

    /// B3 并发压力判官(§9 推荐级):换代半钩子 × worker 收尾半钩子并发
    /// 交火、双持者同时释放——锁内 strong_count 判据竞态面:任何交错下
    /// 取物至多一次(全满 bag 恰好一封),且取后读方重建一次无损。
    #[test]
    fn b3_concurrent_take_races_take_at_most_once() {
        use crate::cache::{CacheNode, CacheSlot};
        use crate::projection::State;
        let mut state = State::new("# S\n\nbody\n");
        let snap = std::sync::Arc::new(state.snapshot());
        let slot = std::sync::Arc::new(CacheSlot::new());
        let node = CacheNode::new_base_eager(snap.clone());
        slot.node.store(Some(std::sync::Arc::new(node)));
        let (tx, rx) = std::sync::mpsc::channel::<ReleasedMem>();
        let tx = super::ReaperTx::from_sender(tx);
        // 并发交火:8 线程各持克隆,同时执行"最后持者"——仅当持者数恰为
        // 自持 + 豁免时取物;其余空手而归。
        let mut handles = Vec::new();
        for _ in 0..8 {
            let slot = slot.clone();
            let tx = tx.clone();
            handles.push(std::thread::spawn(move || {
                // 各自持克隆 = 持者数 ≥ 2 → 无人可取;随后统一释放到
                // 仅剩自己 → 恰好一线程取物成功。
                let mine = slot.clone();
                drop(slot);
                // 此时持者 = mine + 其它线程的 mine(竞态面):count 判据
                // 保证至多一线程观察到自持唯一。
                let _ = super::reaper_take_if_last(&mine, &tx, 1);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // 至多一封 bag;槽内 full 至多被取一次。
        let bags: Vec<ReleasedMem> = rx.try_iter().collect();
        assert!(bags.len() <= 1, "at most one take wins the race");
        let taken = bags.iter().any(|b| b.full.is_some());
        // 确定性收尾:所有克隆释放后自持唯一 → 取物执行(若竞态未取);
        // 规范不变量 = 两次阶段合计恰一次 full 移交。
        let (tx2, rx2) = std::sync::mpsc::channel::<ReleasedMem>();
        let tx2 = super::ReaperTx::from_sender(tx2);
        let _ = super::reaper_take_if_last(&slot, &tx2, 1);
        let second: Vec<ReleasedMem> = rx2.try_iter().collect();
        assert_eq!(second.len(), 1, "cleanup take runs with sole holder");
        assert!(
            taken ^ second[0].full.is_some(),
            "exactly one full handoff across race + cleanup"
        );
        // 最终态:full 已空(取物完成),读方重建一次无损。
        match &**slot.node.load_full().unwrap() {
            crate::cache::CacheNode::Base { full, .. } => {
                assert!(full.load_full().is_none(), "full released exactly once");
            }
            _ => unreachable!(),
        }
    }

    /// B3 reaper 线程端到端(无 seam 观测):换代半钩子 swap 出的 full
    /// 经 mpsc 送达会话 reaper 线程并被 drop。观测 = 段 Arc 持者数:
    /// 测试持克隆(1)+ 袋内 Arc(1)= 2;reaper 消费后回落 1(有界轮询)。
    /// 生产零改动——reaper 线程体(recv + drop)不需要测试缝。
    #[test]
    fn b3_reaper_thread_drains_and_drops_bags() {
        let session = test_session("# S\n\nbody\n");
        // 初始代 Base:物化 full 并持克隆(观测锚);槽/节点克隆随后必须
        // 释放——半钩子的 holders==2 判据按槽强计数,多余槽克隆会跳取。
        let snap0 = session.publisher.load();
        let slot0 = snap0.cache_slot.upgrade().expect("init anchored");
        let _node0 = crate::cache::ensure_node(&slot0, snap0.clone());
        let held = crate::cache::structure_for_range(&snap0, 0, snap0.line_len());
        drop((snap0, slot0, _node0));
        // 换代(结构编辑,无任务 pins):写线程半钩子取物 → bag 入 reaper。
        session.execute_transaction(None, true, |ctx| {
            let len = ctx.state.engine.byte_len();
            ctx.state.engine.edit(len, len, "- item\n");
        });
        // 轮询:reaper 线程 drop 袋内 Arc 后,held 是唯一持者。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while Arc::strong_count(&held) > 1 {
            assert!(
                std::time::Instant::now() < deadline,
                "reaper thread must drain the bag within 5s"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            Arc::strong_count(&held),
            1,
            "bag dropped by the reaper thread;held clone is the sole holder"
        );
    }

    #[test]
    fn phase_b_prev_snapshot_only_on_guard_pass() {
        let session = test_session("# S\n\nhello\n");
        // 暖基：一次纯内容编辑 + 首读，形成可透传 Base。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(2) + 6;
            ctx.state.engine.edit(start, start, "x");
        });
        let snap1 = session.publisher.load();
        let slot1 = snap1.cache_slot.upgrade().expect("slot1");
        let _ = crate::cache::ensure_node(&slot1, snap1.clone());

        // 第二次纯内容编辑：守卫通过，应挂前驱。
        session.execute_transaction(None, true, |ctx| {
            let start = ctx.state.engine.byte_of_line(2) + 7;
            ctx.state.engine.edit(start, start, "y");
        });
        let snap2 = session.publisher.load();
        assert!(
            snap2.prev_snapshot.upgrade().is_some(),
            "guard-pass must link direct predecessor"
        );

        // 结构编辑（行数不变，heading -> list）：不得挂前驱。
        let pos = session.get_engine_text().find("# S").unwrap();
        session.execute_transaction(None, true, |ctx| {
            ctx.state.engine.edit(pos, pos + 1, "-");
        });
        let snap3 = session.publisher.load();
        assert!(
            snap3.prev_snapshot.upgrade().is_none(),
            "structural edit must not inherit cells"
        );
    }

}
