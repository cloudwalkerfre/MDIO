use std::time::{SystemTime, UNIX_EPOCH};
use std::sync::Arc;
use ahash::AHashMap as HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use parking_lot::{RwLock, Mutex};
use dashmap::DashMap;
use dashmap::DashSet;
use tokio::runtime::Runtime;
use smallvec::SmallVec;
use anyhow::{anyhow, Result};
use crossbeam::channel::{unbounded, Sender};
#[cfg(feature = "python")]
use pyo3::prelude::*;
use crate::protocol::*;
use crate::session::MdioSession;
use crate::vfs::MountMode;
use crate::engine::MdioSnapshot;
use crate::selector::{SelectorIR, Step};
use crate::hook::{HookIndex, HookEvent};
use crate::backend::{BackendConfig, Scheme};
use crate::incr::Patch;
use crate::projection::project_text;
use crate::block::BlockProj;
use crate::selector::select_nodes;
use crate::telemetry_span;
use crate::telemetry_guard;

#[path = "worktree.rs"]
pub mod worktree;
use worktree::{WtTree, WtNode, WorktreeEntry, Rule, Binding, path_matches};
use crate::backend::{Backend, FsBackend, MemoryBackend, WorktreeInfo};
use crate::backend::{WalEntry, WAL_MAGIC};
pub use crate::backend::EditRecord;

pub(crate) fn bincode_cfg() -> impl bincode::config::Config {
    bincode::config::standard()
        .with_little_endian()
        .with_variable_int_encoding()
}

// const SERVER_PEER_ID: u64 = 0;
pub type PacketBuffer = SmallVec<[u8; 128]>;

use crate::utils::normalize_vpath;


// Server Action (Return to Python)

#[derive(Debug, Clone)]
pub enum ServerAction {
    Broadcast { channel: String, data: PacketBuffer },
    Direct { data: PacketBuffer },
    DirectAndSub { channel: String, data: PacketBuffer },
    Unsubscribe { channel: String },
    FileAction { entry_id: String, op: crate::protocol::FileOp },
    None,
}


// Internal Structures

#[derive(Clone)]
struct ArenaMeta {
    storage_key: String,
    vfs_path: String,
    wt_uid: String,
}

struct FileArena {
    slots: RwLock<Vec<Option<ArenaMeta>>>,
    path_map: RwLock<HashMap<String, u32>>,
}

impl FileArena {
    fn new() -> Self {
        Self { slots: RwLock::new(Vec::new()), path_map: RwLock::new(HashMap::new()) }
    }

    fn get_or_alloc(&self, storage_key: String, wt_uid: &str, vfs_path: String) -> u32 {
        {
            let map = self.path_map.read();
            if let Some(&id) = map.get(&storage_key) { return id; }
        }
        let mut map = self.path_map.write();
        let mut slots = self.slots.write();
        if slots.is_empty() { slots.push(None); }
        if let Some(&id) = map.get(&storage_key) { return id; }

        let id = slots.len() as u32;
        slots.push(Some(ArenaMeta { storage_key: storage_key.clone(), vfs_path, wt_uid: wt_uid.to_string() }));
        map.insert(storage_key, id);
        id
    }

    pub fn get_id_only(&self, storage_key: &str) -> Option<u32> {
        let map = self.path_map.read();
        map.get(storage_key).copied()
    }

    fn get_meta(&self, id: u32) -> Option<ArenaMeta> {
        let slots = self.slots.read();
        slots.get(id as usize).cloned().flatten()
    }

    fn remove(&self, id: u32) {
        // Lock order must match get_or_alloc: map first, then slots.
        let mut map = self.path_map.write();
        map.retain(|_, v| *v != id);
        let mut slots = self.slots.write();
        if (id as usize) < slots.len() {
            slots[id as usize] = None;
        }
    }

    /// Remove all entries whose wt_uid matches.
    /// Kept for drop_worktree cleanup.
    fn remove_by_wt_uid(&self, target_wt_uid: &str) -> Vec<u32> {
        let mut removed_ids = Vec::new();
        let mut map = self.path_map.write();
        let mut slots = self.slots.write();
        map.retain(|_path, id| {
            if let Some(meta) = slots.get(*id as usize).and_then(|m| m.as_ref()) {
                if meta.wt_uid == target_wt_uid {
                    removed_ids.push(*id);
                    if (*id as usize) < slots.len() {
                        slots[*id as usize] = None;
                    }
                    return false;
                }
            }
            true
        });
        removed_ids
    }

    fn get_doc_ids_for_worktree(&self, target_wt_uid: &str) -> Vec<u32> {
        let map = self.path_map.read(); let slots = self.slots.read();
        map.iter().filter_map(|(_p, id)| slots.get(*id as usize).and_then(|m| m.as_ref()).filter(|meta| meta.wt_uid == target_wt_uid).map(|_| *id)).collect()
    }

    /// 按 storage_key 前缀收集 doc_id——目录递归删除前的 session 清理用。
    /// 前缀 = "目录路径 + /"(如 "{storage_root}docs/"),精确前缀匹配,不误伤同名兄弟目录。
    fn get_doc_ids_by_prefix(&self, prefix: &str) -> Vec<u32> {
        let map = self.path_map.read(); let slots = self.slots.read();
        map.iter().filter(|(p, _)| p.starts_with(prefix))
            .filter_map(|(_, id)| slots.get(*id as usize).and_then(|m| m.as_ref()).map(|_| *id))
            .collect()
    }
}


// Detect Task (async hook detection)


pub enum DetectTask {
    SelectorWrite {
        doc_id: u32,
        /// 该任务快照的文档写序序号；hook.active_from > snap_seq 时整个任务对该 hook 不生效。
        snap_seq: u64,
        write_steps: Vec<Step>,
        new_snap: Arc<MdioSnapshot>,
        ripple_line_start: usize,
        ripple_line_end: usize,
        ripple_frame_actions: Vec<u64>,
        /// 旧/新快照的 PL+FL 是否逐项相等。false 时 hook 可用 hit_range
        /// 做 T1 剪枝；true 时保守全量评估。
        structural_change: bool,
        /// 本次编辑的实际行区间（精确——!structural_change 时内容只改这里——
        /// hit_range 剪枝用它，比保守的 ripple 窄得多）。
        edit_line_start: usize,
        edit_line_end: usize,
        /// suppress 消化语义：检测照做（更新 cached/hit_range），只抑制
        /// HookEvent 投递与一次性 hook 的移除。
        suppress_events: bool,
        /// B2b(#46):任务槽锚——检测在途期间保活本代槽(半径 1 的任务
        /// pins 之一);收尾半钩子(B3)对它执行"最后持者取物"。
        slot: crate::cache::CacheSlotHandle,
        /// B3 接线:worker 收尾半钩子通道。
        reaper_tx: Option<crate::session::ReaperTx>,
    },
    CrdtWrite {
        doc_id: u32,
        /// 该任务快照的文档写序序号（同 SelectorWrite.snap_seq）。
        snap_seq: u64,
        new_snap: Arc<MdioSnapshot>,
        ripple_line_start: usize,
        ripple_line_end: usize,
        ripple_frame_actions: Vec<u64>,
        structural_change: bool,
        edit_line_start: usize,
        edit_line_end: usize,
        suppress_events: bool,
        slot: crate::cache::CacheSlotHandle,
        reaper_tx: Option<crate::session::ReaperTx>,
    },
}

use opendal::blocking::Operator as BlockingOperator;

/// Step 5: incremental AST patch task — incr_plan §6.2
pub use crate::incr::IncrTask;


// WtTree 实现


impl WtTree {
    pub fn new() -> Self {
        Self { nodes: HashMap::new(), dirty: true }
    }

    /// 确保树为最新。dirty 时从 worktrees.md 全量重建。
    pub fn ensure(&mut self, store: &ProjectStore) {
        if !self.dirty { return; }
        self.nodes.clear();
        self.build(store, "genesis", None, None);
        if let Some(genesis) = self.nodes.get_mut("genesis") {
            genesis.external_root = store.genesis_external_root.clone();
            genesis.bindings = store.genesis_bindings.clone();
        }
        self.dirty = false;
    }

    fn build(&mut self, store: &ProjectStore,
             name: &str, parent_uid: Option<&str>, entry: Option<&WorktreeEntry>) {
        // 根的身份是「没有父」，不是名字。子条目可以叫 genesis（项目目录 basename），
        // 把它误判成根会重复读同一份 worktrees.md，形成无限递归。
        let is_root = parent_uid.is_none();
        let uid = if is_root { "genesis".to_string() }
                  else { entry.as_ref().unwrap().uid.clone() };

        let parent_storage = parent_uid
            .and_then(|pu| self.nodes.get(pu).map(|n| n.storage_root.as_str()))
            .unwrap_or("");

        let storage_root = if is_root {
            store.genesis_root.clone()
        } else {
            format!("{}.mds/mounts/mnt_{}/", parent_storage, uid)
        };

        // 惰性恢复
        if !is_root && !self.nodes.contains_key(&uid) {
            store.ensure_worktree_storage(&storage_root);
        }

        // 读子条目
        let children_entries = store.read_worktree_children(&storage_root)
            .unwrap_or_default();

        let rule_text = entry.and_then(|e| e.rule.as_deref()).unwrap_or("");
        let rule = Rule::parse(rule_text);
        let bindings = {
            let bt = rule.bind_text();
            if bt.is_empty() { Vec::new() }
            else { Binding::parse(&bt).unwrap_or_default() }
        };

        let child_uids: Vec<String> = children_entries.iter()
            .map(|c| c.uid.clone()).collect();

        // 项目入口已统一为 path；旧条目无 external_root 时用 path 兜底。
        let external_root = entry
            .and_then(|e| e.external_root.clone())
            .or_else(|| entry.and_then(|e| e.path.clone()));
        // 布局恒定（core 侧不再有"默认值"这条兜底）；条目里的旧字段读进来只作留痕，不参与决策
        let doc_subdir = entry.and_then(|e| e.doc_subdir.clone());
        let wal_rel = entry.and_then(|e| e.wal_rel.clone());

        self.nodes.insert(uid.clone(), WtNode {
            uid: uid.clone(),
            name: name.to_string(),
            parent: parent_uid.map(|s| s.to_string()),
            children: child_uids,
            storage_root,
            bindings,
            rule,
            external_root,
            doc_subdir,
            wal_rel,
        });

        for c in &children_entries {
            self.build(store, &c.name, Some(&uid), Some(c));
        }
    }
}



// ProjectStore (Main Logic)

/// FJ 线程任务（FULL JSON 渲染——独立线程——不卡调用线程/P 帧）。
/// 任务投递时快照已 load（Arc 持有——渲染期间编辑产生新快照——版本隔离）。
pub struct FjTask {
    pub doc_id: u32,
    pub snap: Arc<MdioSnapshot>,
    pub slot: Arc<crate::cache::CacheSlot>,
    /// B3 接线:FJ worker 收尾半钩子通道。
    pub reaper_tx: Option<crate::session::ReaperTx>,
    /// C1 队列压缩标志:同 doc 后续投递置 true → 本代过期(回调 None,
    /// 不渲染)。
    pub cancelled: Arc<AtomicBool>,
    /// 完成回调（FJ 线程内调用）: Some(frame) = 'F' 帧（渲染完成且快照未过期）;
    /// None = 快照已过期（渲染期间编辑——调用方决定是否重投——前端刷新可丢/新订阅者重投）。
    #[cfg(feature = "python")]
    pub cb: Py<PyAny>,
}


// omp：进程级模式——模式真相在 topology::process_mode()（进程内唯一，
// 生产通道 = 进程 env 启动读一次）；per-doc 记账（827 阶段 3）已整体移除。


/// F/P 分派（omp 进程模式）：同一事实 diff，入口按进程模式选拓扑。
/// 实现 = incr::patches_for_mode（生产入口与测试 T-mode 同源）。
pub(crate) fn patches_for_mode(
    task: &IncrTask,
    mode: crate::topology::ProjectionMode,
) -> Vec<Patch> {
    crate::incr::patches_for_mode(task, mode)
}

pub struct ProjectStore {
    sessions: Arc<DashMap<u32, Arc<MdioSession>>>,
    arena: Arc<FileArena>,
    pub hook_index: Arc<Mutex<HookIndex>>,
    pub hook_docs: Arc<DashSet<u32>>,
    /// 文档快照序号时钟（与所有 session 共享）：写路径 assign、注册路径 watermark。
    pub detect_clock: Arc<crate::session::DetectClock>,
    /// patch match attr 归属注册表。
    pub match_registry: Arc<Mutex<crate::r#match::MatchRegistry>>,
    /// 注册表是否**非空**（defs ∪ wildcard 模板）。空 ⇒ Path/FJ 走实施前的老路径，
    /// 零额外代价（不碰 arena、不加锁、不分配 claims）——规格「无 match 无额外代价」。
    pub match_active: Arc<std::sync::atomic::AtomicBool>,
    pub detect_tx: Sender<DetectTask>,
    pub dispatch_tx: Sender<HookEvent>,
    #[cfg(feature = "python")]
    pub ffi_callback_ref: Arc<std::sync::Mutex<Option<Py<PyAny>>>>,
    /// AST Dispatch Thread 回调（Step 5 incremental patches）
    #[cfg(feature = "python")]
    pub ast_callback_ref: Arc<std::sync::Mutex<Option<Py<PyAny>>>>,
    /// Step 5 Path Worker channel sender (always present, path ops gated by inline)
    pub path_tx: crossbeam::channel::Sender<IncrTask>,
    /// FJ 线程任务通道（FULL JSON 渲染——独立线程——不卡调用线程/P 帧）
    pub fj_tx: crossbeam::channel::Sender<FjTask>,
    /// C1 队列压缩注册表:doc_id → 当前最新代任务标志(投递侧写入;
    /// worker 按任务自带标志判定过期)。
    pub(crate) fj_pending: Arc<DashMap<u32, Arc<AtomicBool>>>,
    // Flush 编排线程
    flush_shutdown: Arc<AtomicBool>,
    flush_thread: Option<std::thread::JoinHandle<()>>,
    // WT VFS
    pub(crate) vfs_op: Arc<BlockingOperator>,
    pub(crate) backends: Arc<RwLock<HashMap<String, Box<dyn Backend>>>>,
    pub(crate) tree: Arc<RwLock<WtTree>>,
    pub(crate) genesis_root: String,
    pub(crate) genesis_external_root: Option<String>,
    pub(crate) genesis_bindings: Vec<Binding>,
    _rt: Runtime,

    #[cfg(feature = "telemetry")]
    pub telemetry_active: Arc<AtomicBool>,

    #[cfg(feature = "telemetry")]
    pub telemetry_session: Arc<parking_lot::Mutex<Option<Arc<crate::telemetry::TelemetryInner>>>>,
}

impl ProjectStore {
    /// 三根分层：`doc_subdir`（如 "wt"）与 `wal_rel`（如 ".wal"）由 host 给，core 只转发、不认识名字。
    /// **没有默认值**：缺名字由 FsBackend 直接报错（绝不静默退回扁平）。
    /// 注：旧签名 `new(storage_root)`（= 扁平行）不再使用——它与「没有默认值」
    /// 自相矛盾（传 None,None 必被 FsBackend 拒），调用方一律显式给布局名。
    pub fn new_with_layout(
        storage_root: &str,
        doc_subdir: Option<String>,
        wal_rel: Option<String>,
    ) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        let _guard = rt.enter();

        // WT VFS：VFS Operator（MemoryLayer。FS backend 需 Backend.load()）
        let root_scheme = BackendConfig::detect_scheme(storage_root);
        let vfs_op = Arc::new(BackendConfig::resolve(Scheme::Memory).create_operator("/")?);
        let genesis_root = "/".to_string();
        let genesis_external_root = if storage_root != "/mem" && !storage_root.starts_with("/mem/") {
            Some(storage_root.split(|c| c == ',' || c == ';').next().unwrap_or(storage_root).trim().to_string())
        } else {
            None
        };
        // 从 storage_root 字符串解析 genesis bindings（各段 → RO 保护等）
        let root_path = genesis_external_root.as_deref().unwrap_or("/");
        let genesis_bindings = {
            let parts: Vec<&str> = storage_root.split(|c| c == ',' || c == ';')
                .map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
            let mut bs = Vec::new();
            for part in &parts[1..] {
                let segs: Vec<&str> = part.split(':').map(|s| s.trim()).collect();
                if segs.len() >= 2 && !segs[1].is_empty() && segs[1] != "RO" && segs[1] != "RW" {
                    let alias = segs[1].trim_start_matches('/');
                    let mode = if segs.len() > 2 && segs[2].eq_ignore_ascii_case("RO") { MountMode::RO } else { MountMode::RW };
                    if alias != ".mdio" && alias != ".mds" && !alias.is_empty() {
                        let target = segs[0].trim_end_matches('/');
                        let vfs_target = if target.starts_with(root_path) {
                            let rel = &target[root_path.len()..];
                            if rel.is_empty() { "/".to_string() } else { rel.to_string() }
                        } else {
                            target.to_string()
                        };
                        bs.push(Binding { my_path: format!("/{}", alias), target_path: vfs_target, mode });
                    }
                }
            }
            bs
        };
        {
            let op = &vfs_op;
            op.create_dir("/.mds/").ok();
            if !op.exists("/.mds/env.md").unwrap_or(false) {
                let _ = op.write("/.mds/env.md", "# SYSTEM_ENV\n# LOCAL_ENV\n".as_bytes());
            }
            if !op.exists("/.mds/hook.md").unwrap_or(false) {
                let _ = op.write("/.mds/hook.md", "# HOOKS\n".as_bytes());
            }
            if !op.exists("/.mds/proc.md").unwrap_or(false) {
                let _ = op.write("/.mds/proc.md", "# PROCESS_TABLE\n".as_bytes());
            }
            if !op.exists("/.mds/llm_config.md").unwrap_or(false) {
                let _ = op.write("/.mds/llm_config.md", "# LLM_CONFIG\n".as_bytes());
            }
            if !op.exists("/.mds/worktrees.md").unwrap_or(false) {
                let _ = op.write("/.mds/worktrees.md", "# WORKTREES\n".as_bytes());
            }
            op.create_dir("/.mds/ckpt/").ok();
            if !op.exists("/.mds/snap.md").unwrap_or(false) {
                let _ = op.write("/.mds/snap.md", "# TIME\n".as_bytes());
            }
            op.create_dir("/.mds/cache/").ok();
            if !op.exists("/.mds/cache/find.md").unwrap_or(false) {
                let _ = op.write("/.mds/cache/find.md", "# FIND_CACHE\n- tokenizer: v1\n".as_bytes());
            }
        }
        // 创建 genesis Backend
        // `None` 分支＝**无 L3 外部根**（`/mem` 等纯内存根）：挂 L7 的 Memory scheme 后端
        // （算子仍由 `BackendConfig::resolve(Scheme::Memory).create_operator` 产出，磁盘面为零）。
        // 老兜法是造一个指向 `/` 的 `FsBackend`——它会读/建真实根，且布局名必填之后直接构造失败。
        let genesis_backend: Box<dyn Backend> = match &genesis_external_root {
            Some(ext_path) => Box::new(FsBackend::new_with_docs(
                ext_path, &genesis_root, doc_subdir.as_deref(), wal_rel.as_deref())?),
            None => Box::new(MemoryBackend::new("/")?),
        };
        let mut backends_map: HashMap<String, Box<dyn Backend>> = HashMap::new();
        backends_map.insert("genesis".to_string(), genesis_backend);
        let backends: Arc<RwLock<HashMap<String, Box<dyn Backend>>>> = Arc::new(RwLock::new(backends_map));

        let hook_index = Arc::new(Mutex::new(HookIndex::new()));
        let hook_docs = Arc::new(DashSet::new());
        let detect_clock = Arc::new(crate::session::DetectClock::new());
        // patch match attr：注册表与 arena 必须先于 Path worker spawn(529) 建——
        // 注册表同 hook_index 先例；arena 上移供 C5 补登记取 doc→wt 映射
        // （原声明在 695，晚于 spawn；519-695 区间无其它 arena 引用，零行为变化）。
        let match_registry = Arc::new(Mutex::new(crate::r#match::MatchRegistry::new()));
        let match_active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let arena = Arc::new(FileArena::new());
        let (dispatch_tx, dispatch_rx) = unbounded::<HookEvent>();
        let (detect_tx, detect_rx) = unbounded::<DetectTask>();
        #[cfg(feature = "python")]
        let ffi_callback_ref = Arc::new(std::sync::Mutex::new(None::<Py<PyAny>>));

        // AST callback + Dispatch Thread (Step 5)
        #[cfg(feature = "python")]
        let ast_callback_ref = Arc::new(std::sync::Mutex::new(None::<Py<PyAny>>));

        // Telemetry session handle for core-compute worker threads (Detect /
        // Path / FJ).  Worker threads have no Python-side session activation,
        // so their selector/incr/render cost is invisible to telemetry unless
        // they ensure_session per task.  Arc so the spawn closures can capture
        // a clone while ProjectStore is still under construction.
        #[cfg(feature = "telemetry")]
        let tel_active = Arc::new(AtomicBool::new(false));
        #[cfg(feature = "telemetry")]
        let tel_session = Arc::new(parking_lot::Mutex::new(None::<Arc<crate::telemetry::TelemetryInner>>));

        // Detect Worker Thread
        let detect_hook_index = hook_index.clone();
        let detect_dispatch_tx = dispatch_tx.clone();
        #[cfg(feature = "telemetry")]
        let detect_tel_active = tel_active.clone();
        #[cfg(feature = "telemetry")]
        let detect_tel_session = tel_session.clone();
        std::thread::spawn(move || {
            loop {
                let task = match detect_rx.recv() {
                    Ok(t) => t,
                    Err(_) => break,
                };
                let run_one = || {
                    // 补盲：worker 线程激活 session——detect 的锚点/scope/evaluate
                    // 查询（select_nodes 全链）进入 telemetry。
                    #[cfg(feature = "telemetry")]
                    let _tel = crate::telemetry::ensure_session(&*detect_tel_active, &*detect_tel_session);
                    let mut hi = detect_hook_index.lock();
                    let events = match task {
                        DetectTask::SelectorWrite { doc_id, snap_seq, write_steps, new_snap, ripple_line_start, ripple_line_end, ripple_frame_actions, structural_change, edit_line_start, edit_line_end, suppress_events, slot, reaper_tx } => {
                            let ev = hi.detect_selector_write_ex(doc_id, &write_steps, &new_snap, ripple_line_start, ripple_line_end, &ripple_frame_actions, structural_change, edit_line_start, edit_line_end, suppress_events, snap_seq);
                            // B3 worker 收尾半钩子(Detect):任务自持 = 1 时取物。
                            if let Some(tx) = &reaper_tx {
                                crate::session::reaper_take_if_last(&slot.0, tx, 1);
                            }
                            ev
                        }
                        DetectTask::CrdtWrite { doc_id, snap_seq, new_snap, ripple_line_start, ripple_line_end, ripple_frame_actions, structural_change, edit_line_start, edit_line_end, suppress_events, slot, reaper_tx } => {
                            let ev = hi.detect_crdt_write_ex(doc_id, &new_snap, ripple_line_start, ripple_line_end, &ripple_frame_actions, structural_change, edit_line_start, edit_line_end, suppress_events, snap_seq);
                            if let Some(tx) = &reaper_tx {
                                crate::session::reaper_take_if_last(&slot.0, tx, 1);
                            }
                            ev
                        }
                    };
                    drop(hi);
                    for event in events {
                        let _ = detect_dispatch_tx.send(event);
                    }
                };
                // 单任务 panic 隔离：坏任务跳过，检测循环不中断（消化任务量增大后
                // hook 系统不失明的可靠运行前提）。
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_one)).is_err() {
                    continue;
                }
            }
        });

        // Path Worker + AST Dispatch Thread (Step 5 incremental patches)
        let (path_tx, path_rx) = unbounded::<IncrTask>();
        #[cfg(feature = "python")]
        {
            let ast_cb_ref = ast_callback_ref.clone();
            // patch match attr（C1）：注册表 + arena（C5 的 doc→wt）进闭包。
            let path_match_registry = match_registry.clone();
            let path_match_active = match_active.clone();
            let path_arena = arena.clone();
            // omp 读点①：Path Worker（P 增量）——按进程模式选拓扑（process_mode）。
            #[cfg(feature = "telemetry")]
            let path_tel_active = tel_active.clone();
            #[cfg(feature = "telemetry")]
            let path_tel_session = tel_session.clone();
            std::thread::spawn(move || {
                for task in &path_rx {
                    // ── 单任务 panic 隔离（同 Detect Worker 的写法）────────────────
                    // 没有它，一次 panic 就让整条补丁流水线**永久静默停摆**：发送端是
                    // `let _ = tx.send(..)`，receiver 死了没人知道。实测（注入一次 panic）：
                    // 之后每次编辑的补丁都不再到达，界面拿陈旧状态继续写回 —— 点两次
                    // 「加入购物车」只减一次（第二次按陈旧的 11 又写回 10）。
                    let run_one = || {
                    // 注入点（测试用）：MDIO_TEST_PANIC=path 时在第 n 个任务上炸一次。
                    #[cfg(test)]
                    {
                        if crate::testhooks::panic_armed("path") {
                            panic!("MDIO_TEST_PANIC=path 注入的测试 panic");
                        }
                    }
                    // 补盲：Path worker 激活 session——incr_patches 全链
                    // （build_structure_parts/attribution_tree/diff/render）进入 telemetry。
                    #[cfg(feature = "telemetry")]
                    let _tel = crate::telemetry::ensure_session(&*path_tel_active, &*path_tel_session);
                    // 探查根 span:Path 任务全链(补丁计算+序列化+回调)一棵
                    // 子树——与主线程 op 窗口的并发跨度区分开。
                    #[cfg(feature = "telemetry")]
                    let _sp_path = crate::telemetry_span!("path_render_total", "doc_id" => task.doc_id);
                    // patch match attr：**无注册 ⇒ 零额外代价**——直接走实施前老入口
                    // （不碰 arena、不加锁、不分配 claims；开关是一次 relaxed load）。
                    let (patches, claims): (Vec<Patch>, Vec<crate::r#match::ClaimSet>) =
                        if path_match_active.load(Ordering::Relaxed) {
                            // C5：首见 doc_id ⇒ 通配模板补登记（scope 从 task.snap 算；
                            // 逐文档过 D5 门——None 不登记也不标记，前缀出现后可补）。
                            let wt_uid = path_arena
                                .get_meta(task.doc_id)
                                .map(|m| m.wt_uid)
                                .unwrap_or_default();
                            {
                                let mut r = path_match_registry.lock();
                                if r.needs_fanout(task.doc_id, &wt_uid) {
                                    r.ensure_doc(task.doc_id, &wt_uid, &task.snap);
                                }
                            }
                            // C2/C3：claim 与补丁同源（事实层恒 CF、拓扑只在 adapt 分派）。
                            crate::incr::patches_and_claims(
                                &task,
                                crate::topology::process_mode(),
                                &path_match_registry,
                            )
                        } else {
                            (crate::incr::patches_for_mode(
                                &task, crate::topology::process_mode(),
                            ), Vec::new())
                        };
                    // [S1 账本外化] **每代一行跨代总账**：把 Rust 侧 ledger（5 kind ×
                    // 查询/命中/构建）接进 telemetry——worker 线程已有 ensure_session，
                    // 故 gateway 导出可见（写线程的 span 在 gateway 模式下不被采集，
                    // 见 docs/TELEMETRY.md §10.1 thread_local 语义；故此处只报 worker 面）。
                    // kv `kinds` = "tree;secs;rtbl;rblk;span" 的 `q/h/b`，顺序固定。
                    #[cfg(feature = "telemetry")]
                    {
                        use crate::projection::CellKind;
                        const KS: [CellKind; 5] = [
                            CellKind::Tree, CellKind::Secs, CellKind::RootTable,
                            CellKind::RootBlocksTable, CellKind::BlockSpans,
                        ];
                        // 每 kind 一条（短名 + 数值 kv）。**读法（docs/TELEMETRY.md §10.3）**：
                        // trace 模式的 kv 进事件 TLV 池，只在 `report_json_trace` 的
                        // args 可见；`export_phase_json` 携带 kv 的只有 **bench 模式**
                        // 的 `src="events"` 行 ⇒ 用本探针时 session 应建为 bench
                        // （否则只看到 5×代数 的行数，看不到 q/h/b）。
                        const KN: [&str; 5] = ["tree", "secs", "rtbl", "rblk", "span"];
                        for (i, k) in KS.iter().enumerate() {
                            let (q, h, b) = task.snap.cell_consumption.cell_stats(*k);
                            let _sp = crate::telemetry_span!("cell_ledger",
                                "kind" => KN[i], "q" => q, "h" => h, "b" => b,
                                "n" => task.snap.line_len() as u64);
                        }
                    }
                    // B3 worker 收尾半钩子(Path):释放任务锚前同协议——新侧/
                    // 旧侧槽各自"最后持者取物"(latest 恒锚在持 → count>1
                    // 自动跳过);补丁空与否都必须执行。
                    if let Some(tx) = &task.reaper_tx {
                        if let Some(s) = &task.slot {
                            crate::session::reaper_take_if_last(&s.0, tx, 1);
                        }
                        if let Some(s) = &task.slot_old {
                            crate::session::reaper_take_if_last(&s.0, tx, 1);
                        }
                    }
                    if patches.is_empty() { return; }   // 闭包内 return = 跳过本任务（原为 continue）

                    let mut pkt = Vec::with_capacity(512);
                    // 827 阶段 1 P1：帧序列化段测量（零开销插桩——feature OFF
                    // 展开 ()——只覆盖 patch→JSON 序列化，不含回调）。
                    let _span_pframe = telemetry_span!("pframe_serialize",
                        "n" => patches.len() as u64);
                    // [F23 修复] 走现成的 `BufWriter` 裸指针直写（渲染路径同款轮子）：
                    // 旧写法每个 patch 做 2 次 `path.replace` + 1 次 `format!`
                    // = 2–3 次 String 分配 + 一次整串拷贝；输出逐字节不变
                    // （`write_json_escaped` 与 replace 链等价）。
                    write_patch_frame(&mut pkt, &patches, &claims);
                    drop(_span_pframe);
                    // eprintln!("[PPATCH] doc={} patches={}", task.doc_id,
                    //     String::from_utf8_lossy(&pkt));

                    // 解释器不可用(纯 rust 测试/关闭中)→ 静默跳过;
                    // 业务路径(解释器存活)行为与 attach 逐值等价。
                    pyo3::Python::try_attach(|py| {
                        // 锁内不要执行 Python 回调：回调可能 call_soon_threadsafe，
                        // 与另一线程的 register_ast_callback 形成 Rust mutex/GIL 死锁。
                        let cb = ast_cb_ref
                            .lock()
                            .unwrap()
                            .as_ref()
                            .map(|cb| cb.clone_ref(py));
                        if let Some(cb) = cb {
                            let mut payload = Vec::with_capacity(1 + pkt.len());
                            payload.push(b'P');
                            payload.extend_from_slice(&pkt);
                            let frame = crate::store::ProjectStore::build_ast_frame(task.doc_id, &payload);
                            if let Err(e) = cb.call1(py, (frame,)) {
                                eprintln!("[AST Dispatch] callback error: {}", e);
                            }
                        }
                    });
                    };   // ← run_one 闭包结束（闭包内 return 即「跳过本任务」）
                    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(run_one)).is_err() {
                        // 拦住必须留痕：被跳过的任务意味着这篇文档的补丁可能缺失 ——
                        // 只「不崩」而不记账，等于把「静默死亡」换成「静默陈旧」。
                        #[cfg(test)]
                        crate::testhooks::note_contained_panic(task.doc_id);
                        eprintln!("[Path Worker] 任务 panic 已隔离 doc={}（该文档补丁可能缺失）", task.doc_id);
                    }
                }
            });
        }

        // FFI Dispatcher Thread
        #[cfg(feature = "python")]
        {
            let dispatcher_ref = ffi_callback_ref.clone();
            std::thread::spawn(move || {
                while let Ok(task) = dispatch_rx.recv() {
                    // 解释器不可用(纯 rust 测试/关闭中)→ 静默跳过(同 AST Dispatch)。
                    pyo3::Python::try_attach(|py| {
                        // 同上：先 clone 后放锁，绝不在 Rust mutex 内进入 Python。
                        let cb = dispatcher_ref
                            .lock()
                            .unwrap()
                            .as_ref()
                            .map(|cb| cb.clone_ref(py));
                        if let Some(cb) = cb {
                            match pythonize::pythonize(py, &task.ctx_val) {
                                Ok(ctx_dict) => {
                                    if let Err(e) = cb.call1(py, (task.cb_code.clone(), ctx_dict)) {
                                        eprintln!("🚨 [Rust FFI Flash] 呼叫 Python 回调时崩溃!");
                                        e.print(py);
                                    }
                                },
                                Err(e) => eprintln!("🚨 [Rust FFI] Pythonize 失败: {}", e),
                            }
                        }
                    });
                }
            });
        }

        let sessions: Arc<DashMap<u32, Arc<MdioSession>>> = Arc::new(DashMap::new());
        // arena 已在 spawn 之前建（见上）；此处不再重复声明。

        // FJ 线程（FULL JSON 渲染——独立线程——不卡调用线程/P 帧）
        // 同构 AST Dispatch Thread（std::thread::spawn 常驻 + 通道）——渲染业务逻辑照旧
        // （全量 [0,n) 构建 + to_ast_object_mode——与 get_ast 同一调用——输出逐字节一致）
        let (fj_tx, fj_rx) = unbounded::<FjTask>();
        #[cfg(feature = "python")]
        {
            let fj_sessions = sessions.clone();
            // patch match attr（N5）：F 帧归属（注册表 + arena 取 doc→wt；补登记同 C5）。
            let fj_match_registry = match_registry.clone();
            let fj_match_active = match_active.clone();
            let fj_arena = arena.clone();
            // omp 读点②：FJ worker（F 全量）——按进程模式选拓扑（process_mode）。
            #[cfg(feature = "telemetry")]
            let fj_tel_active = tel_active.clone();
            #[cfg(feature = "telemetry")]
            let fj_tel_session = tel_session.clone();
            std::thread::spawn(move || {
                for task in &fj_rx {
                    // 补盲：FJ worker 激活 session——全量 AST 渲染
                    // （build_structure_parts/attribution_tree/render）进入 telemetry。
                    #[cfg(feature = "telemetry")]
                    let _tel = crate::telemetry::ensure_session(&*fj_tel_active, &*fj_tel_session);
                    // C1 队列压缩:过期代(cancelled)→ 回调 None 且**不渲染**
                    // (旧代任务从未渲染——压缩语义的硬断言面)。
                    if task.cancelled.load(Ordering::Relaxed) {
                        // 解释器不可用 → 静默跳过(同 AST Dispatch/FFI)。
                        pyo3::Python::try_attach(|py| {
                            if let Err(e) = task.cb.call1(py, (None::<Vec<u8>>,)) {
                                eprintln!("[FJ] stale callback error: {}", e);
                            }
                        });
                        if let Some(tx) = &task.reaper_tx {
                            crate::session::reaper_take_if_last(&task.slot, tx, 1);
                        }
                        continue;
                    }
                    // 上层失明——渲染/复用/建树全在渲染层
                    // （to_ast_object_mode 内部经 render_roots_to）；
                    // store 不再直触 cache slot。
                    // omp：F 与 P 必须同模式——进程内模式唯一，F/P 恒同源。
                    // 探查根 span：FJ 整任务(渲染+回调+reaper)一棵子树——
                    // trace 时序归因把它与读窗口的并发跨度区分开。
                    #[cfg(feature = "telemetry")]
                    let _sp_fj = crate::telemetry_span!("fj_render_total", "doc_id" => task.doc_id);
                    // 无注册 ⇒ 老入口（零额外代价）；有注册才建归属映射。
                    let json = if fj_match_active.load(Ordering::Relaxed) {
                        let wt = fj_arena
                            .get_meta(task.doc_id)
                            .map(|m| m.wt_uid)
                            .unwrap_or_default();
                        let defs = {
                            let mut r = fj_match_registry.lock();
                            if r.needs_fanout(task.doc_id, &wt) {
                                r.ensure_doc(task.doc_id, &wt, &task.snap);
                            }
                            r.defs_for_doc(task.doc_id)
                        };
                        let f_claims = crate::r#match::ClaimMap::build(&defs, &task.snap);
                        task.snap.to_ast_object_mode_with_claims(
                            crate::topology::process_mode(),
                            &f_claims,
                        )
                    } else {
                        task.snap.to_ast_object_mode(crate::topology::process_mode())
                    };
                    // 帧时序版本检查：渲染期间快照是否已更新（编辑 V2——publisher.store 新快照）
                    // 过期 → 回调 None（前端已有 P 增量流——旧全量无意义；调用方决定是否重投）
                    let stale = fj_sessions.get(&task.doc_id)
                        .map(|s| !std::sync::Arc::ptr_eq(&*s.value().publisher.load(), &task.snap))
                        .unwrap_or(true);
                    // 解释器不可用 → 静默跳过(同族统一)。
                    pyo3::Python::try_attach(|py| {
                        let frame = if stale { None } else {
                            let mut payload = Vec::with_capacity(1 + json.len());
                            payload.push(b'F');
                            payload.extend_from_slice(json.as_bytes());
                            Some(crate::store::ProjectStore::build_ast_frame(task.doc_id, &payload))
                        };
                        if let Err(e) = task.cb.call1(py, (frame,)) {
                            eprintln!("[FJ] callback error: {}", e);
                        }
                    });
                    // B3 worker 收尾半钩子(FJ):释放任务锚前同协议(latest
                    // 恒锚在持 → count>1 自动跳过)。
                    if let Some(tx) = &task.reaper_tx {
                        crate::session::reaper_take_if_last(&task.slot, tx, 1);
                    }
                }
            });
        }

        // WT VFS：初始化 WtTree
        let tree = {
            let mut t = WtTree::new();
            t.nodes.insert("genesis".to_string(), WtNode {
                uid: "genesis".to_string(),
                name: "genesis".to_string(),
                parent: None,
                children: Vec::new(),
                storage_root: genesis_root.clone(),
                bindings: genesis_bindings.clone(),
                rule: Rule::default(),
                external_root: genesis_external_root.clone(),
                doc_subdir: None,
                wal_rel: None,
            });
            t.dirty = true;
            Arc::new(RwLock::new(t))
        };

        // Flush 编排线程（仅 persistence）
        let flush_shutdown = Arc::new(AtomicBool::new(false));
        let persistent = cfg!(feature = "persistence") && {
            BackendConfig::resolve(root_scheme).persistent
        };
        let flush_thread = if persistent {
            let fs = flush_shutdown.clone();
            let vfs_op_flush = vfs_op.clone();
            let sessions_flush = sessions.clone();
            let arena_flush = arena.clone();
            let tree_flush = tree.clone();
            let backends_flush = backends.clone();
            Some(std::thread::spawn(move || {
                // 独立 knob:定期 flush 间隔与 WAL 合并间隔语义不同,不再跟随
                // MDIO_WAL_INTERVAL_MS(测试把后者拉到 10ms 时,flush 编排每
                // 10ms flush 一次,把未结算 edits 冲进 base + trim——合并写
                // 保帧的前提被破坏)。缺省 100ms 与旧行为一致;生产无人设置
                // MDIO_WAL_INTERVAL_MS(仅测试用),零业务影响。
                let interval_ms: u64 = std::env::var("MDIO_FLUSH_INTERVAL_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
                loop {
                    // 小步长轮询:shutdown 不等待 interval 到期(Drop 的 join
                    // 及时返回——长 interval 下旧单次 sleep 会让进程关闭挂起)。
                    // flush 周期不变(waited 累计到 interval 才 flush)。
                    let step = interval_ms.min(100).max(1);
                    let mut waited = 0u64;
                    while waited < interval_ms && !fs.load(Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(step));
                        waited += step;
                    }
                    if fs.load(Ordering::Relaxed) {
                        // final pass then break
                        for entry in sessions_flush.iter() {
                            if entry.value().dirty.load(Ordering::Relaxed) {
                                flush_one(*entry.key(), entry.value(), &arena_flush, &vfs_op_flush, &tree_flush, &backends_flush);
                            }
                        }
                        break;
                    }
                    for entry in sessions_flush.iter() {
                        if entry.value().dirty.load(Ordering::Relaxed) {
                            flush_one(*entry.key(), entry.value(), &arena_flush, &vfs_op_flush, &tree_flush, &backends_flush);
                        }
                    }
                }
            }))
        } else {
            None
        };

        // Backend.load：将 genesis 磁盘文件泵入 VFS mem
        if let Some(ref ext_path) = genesis_external_root {
            let be = backends.read();
            if let Some(b) = be.get("genesis") { b.load(&vfs_op, ext_path, &genesis_root).ok(); }
        }

        Ok(Self {
            sessions, arena, hook_index, hook_docs, detect_clock, match_registry, match_active, detect_tx, dispatch_tx,
            #[cfg(feature = "python")] ffi_callback_ref,
            #[cfg(feature = "python")] ast_callback_ref,
            path_tx, fj_tx,
            fj_pending: Arc::new(DashMap::new()),
            flush_shutdown, flush_thread,
            vfs_op, backends, tree,
            genesis_root, genesis_external_root,
            genesis_bindings,
            _rt: rt,
            #[cfg(feature = "telemetry")]
            telemetry_active: tel_active,
            #[cfg(feature = "telemetry")]
            telemetry_session: tel_session,
        })
    }

    // ── WT VFS helpers─────────────────────────────────────

    /// 确保 storage_root 目录存在并初始化 local 文件（幂等），不生成新 uid。
    ///
    /// 约束（实测）：**只许写 `vfs_op`（Memory）**。父树的**磁盘**上出现过
    /// 子 wt 文档的第二份副本（`<父>/.mds/mounts/mnt_<uid>/dashboard.md` 等，且与真身不同 ⇒ 陈旧），
    /// 那是旧机制遗留、不是本函数所为；但这类"父树里的子 wt 文档副本"一律算缺陷——
    /// 它让同一份真相拥有两个地址。任何往 fs_op 写子 wt 文档的路径都应视为 bug。
    pub(crate) fn ensure_worktree_storage(&self, storage_root: &str) {
        let op = &self.vfs_op;
        // 边界防御:create_dir("/") 在 opendal memory 写入空 key——任何
        // list("/") 迭代空 key 触发 debug_assert(FILE + "/" 矛盾)——根目录
        // 恒存在——无意义调用且埋雷——跳过。
        if !storage_root.is_empty() && storage_root != "/" {
            let _ = op.create_dir(storage_root);
        }
        let mdio = format!("{}.mds/", storage_root);
        let _ = op.create_dir(&mdio);
        if !op.exists(&format!("{}env.md", mdio)).unwrap_or(false) {
            let _ = op.write(&format!("{}env.md", mdio), "# LOCAL_ENV\n".as_bytes());
        }
        if !op.exists(&format!("{}hook.md", mdio)).unwrap_or(false) {
            let _ = op.write(&format!("{}hook.md", mdio), "# HOOKS\n".as_bytes());
        }
        if !op.exists(&format!("{}proc.md", mdio)).unwrap_or(false) {
            let _ = op.write(&format!("{}proc.md", mdio), "# PROCESS_TABLE\n".as_bytes());
        }
        if !op.exists(&format!("{}worktrees.md", mdio)).unwrap_or(false) {
            let _ = op.write(&format!("{}worktrees.md", mdio), "# WORKTREES\n".as_bytes());
        }
        let _ = op.create_dir(&format!("{}ckpt/", mdio));
        if !op.exists(&format!("{}snap.md", mdio)).unwrap_or(false) {
            let _ = op.write(&format!("{}snap.md", mdio), "# TIME\n".as_bytes());
        }
        let _ = op.create_dir(&format!("{}cache/", mdio));
        if !op.exists(&format!("{}cache/find.md", mdio)).unwrap_or(false) {
            let _ = op.write(&format!("{}cache/find.md", mdio), "# FIND_CACHE\n- tokenizer: v1\n".as_bytes());
        }
    }

    /// 从指定 storage_root 读取并解析 worktrees.md，使用 MDIO selector。
    pub(crate) fn read_worktree_children(&self, storage_root: &str) -> Result<Vec<WorktreeEntry>> {
        let path = format!("{}.mds/worktrees.md", storage_root);
        let content = match self.vfs_op.read(&path) {
            Ok(data) => String::from_utf8(data.to_vec()).unwrap_or_default(),
            Err(_) => return Ok(Vec::new()),
        };
        if content.is_empty() {
            return Ok(Vec::new());
        }

        let snap = project_text(&content);
        let steps = vec![Step::Drill("WORKTREES".to_string()), Step::Wildcard];
        let nodes = select_nodes(&snap, &steps);

        let mut entries = Vec::new();
        for node in nodes {
            let name = snap.read_block_proj(&node, BlockProj::Tag).trim().to_string();
            let body = snap.read_block_proj(&node, BlockProj::Content);
            let mut entry = WorktreeEntry {
                name,
                uid: String::new(),
                rule: None,
                path: None,
                external_root: None,
                doc_subdir: None,
                wal_rel: None,
            };
            for line in body.lines() {
                let trimmed = line.trim();
                if let Some(val) = trimmed.strip_prefix("- uid: ") {
                    entry.uid = val.to_string();
                } else if let Some(val) = trimmed.strip_prefix("- rule: ") {
                    entry.rule = Some(val.to_string());
                } else if let Some(val) = trimmed.strip_prefix("- path: ") {
                    entry.path = Some(val.to_string());
                } else if let Some(val) = trimmed.strip_prefix("- external_root: ") {
                    entry.external_root = Some(val.to_string());
                } else if let Some(val) = trimmed.strip_prefix("- doc_subdir: ") {
                    entry.doc_subdir = Some(val.to_string());
                } else if let Some(val) = trimmed.strip_prefix("- wal_rel: ") {
                    entry.wal_rel = Some(val.to_string());
                }
            }
            if !entry.name.is_empty() {
                entries.push(entry);
            }
        }
        Ok(entries)
    }

    /// BFS 顺序返回所有 WT 信息（parent-first）。
    pub fn list_worktrees(&self) -> Vec<WorktreeInfo> {
        self.tree.read().list_worktrees()
    }

    /// 检查 WtTree 中是否已注册指定 uid 的 WT。
    pub fn has_worktree(&self, uid: &str) -> bool {
        self.tree.read().nodes.contains_key(uid)
    }

    pub fn rule_remote_entries(&self, wt_uid: &str) -> Vec<(String, String)> {
        let tree = self.tree.read();
        tree.nodes.get(wt_uid)
            .map(|n| n.rule.remote_entries())
            .unwrap_or_default()
    }

    pub fn rule_add_remote(&self, wt_uid: &str, vfs_path: &str, entry_id: &str) -> Result<()> {
        let mut tree = self.tree.write();
        let node = tree.nodes.get_mut(wt_uid)
            .ok_or_else(|| anyhow!("WT not found: {}", wt_uid))?;
        node.rule.add_remote(vfs_path, entry_id)?;
        node.bindings = Binding::parse(&node.rule.bind_text())?;
        tree.dirty = true;
        Ok(())
    }

    pub fn rule_remove_remote(&self, wt_uid: &str, vfs_path: &str) -> Result<()> {
        let mut tree = self.tree.write();
        let node = tree.nodes.get_mut(wt_uid)
            .ok_or_else(|| anyhow!("WT not found: {}", wt_uid))?;
        node.rule.remove_remote(vfs_path);
        node.bindings = Binding::parse(&node.rule.bind_text())?;
        tree.dirty = true;
        Ok(())
    }

    /// 标脏 WT 树:下次 tree_ensure 从 worktrees.md 全量重建。
    /// 外部条目治愈(回填 external_root)后调用,使本次即可建后端。
    pub fn tree_invalidate(&self) {
        self.tree.write().dirty = true;
    }

    /// B6: After tree_ensure discovers project WTs, create their Backends and load files.
    pub fn restore_project_worktrees(&self) -> Result<()> {
        let _guard = self._rt.enter();
        let projects: Vec<(String, String, String, Option<String>, Option<String>)> = {
            let t = self.tree.read();
            t.list_worktrees().into_iter()
                .filter(|w| w.uid != "genesis")
                .filter_map(|w| w.external_root.as_ref().map(|ep| (
                    w.uid.clone(), w.storage_root.clone(), ep.clone(),
                    w.doc_subdir.clone(), w.wal_rel.clone())))
                .collect()
        };
        {
            let t = self.tree.read();
            for w in t.list_worktrees().into_iter().filter(|w| w.uid != "genesis") {
                if w.external_root.is_none() {
                    eprintln!("⚠️ [restore] worktree {} ({}) skipped: missing external_root", w.uid, w.name);
                }
            }
        }
        for (uid, storage_root, ext_path, doc_subdir, wal_rel) in projects {
            if self.backends.read().contains_key(&uid) { continue; }
            let r = (|| -> Result<()> {
                let backend = FsBackend::new_with_docs(&ext_path, &storage_root,
                    doc_subdir.as_deref(), wal_rel.as_deref())?;
                backend.load(&self.vfs_op, &ext_path, storage_root.trim_end_matches('/'))?;
                self.backends.write().insert(uid.clone(), Box::new(backend));
                Ok(())
            })();
            if let Err(e) = r {
                eprintln!("⚠️ [restore] worktree {} skipped: {:#}", uid, e);
            }
        }
        Ok(())
    }

    /// 生成新的 8 位 base62 uid（微秒时间戳编码）。
    pub(crate) fn new_name(&self) -> String {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        const BASE62: &[u8; 62] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let mut n = ts as u64;
        let mut buf = [0u8; 16];
        let mut i = 16;
        while n > 0 && i > 0 {
            i -= 1;
            buf[i] = BASE62[(n % 62) as usize];
            n /= 62;
        }
        while i > 0 { i -= 1; buf[i] = b'0'; }
        // 取最后 8 位（最低有效位变化最快）
        String::from_utf8_lossy(&buf[8..16]).to_string()
    }

    /// WT VFS：创建非 project WT（bfork / cfork 统一）。返回 uid。
    pub fn derive_worktree(&self, parent_uid: &str, name: &str) -> Result<String> {
        self.derive_worktree_with_rule(parent_uid, name, "")
    }

    pub fn derive_worktree_with_rule(&self, parent_uid: &str, name: &str, rule_text: &str) -> Result<String> {
        let _guard = self._rt.enter();
        let uid = self.new_name();
        let mut tree = self.tree.write();
        let parent = tree.nodes.get(parent_uid)
            .ok_or_else(|| anyhow!("parent not found: {}", parent_uid))?;
        let storage_root = format!("{}.mds/mounts/mnt_{}/", parent.storage_root, uid);
        let rule = Rule::parse(rule_text);
        let bindings = {
            let bt = rule.bind_text();
            if bt.is_empty() { Vec::new() }
            else { Binding::parse(&bt)? }
        };
        let mut parent_node = tree.nodes.get_mut(parent_uid).unwrap();
        parent_node.children.push(uid.clone());
        tree.nodes.insert(uid.clone(), WtNode {
            uid: uid.clone(),
            name: name.to_string(),
            parent: Some(parent_uid.to_string()),
            children: Vec::new(),
            storage_root: storage_root.clone(),
            bindings,
            rule,
            external_root: None,
            doc_subdir: None,
            wal_rel: None,
        });
        drop(tree);
        self.ensure_worktree_storage(&storage_root);
        Ok(uid)
    }

    /// 家族契约校验：`external_root` 必须是该存储家族认得的**根**。
    ///
    /// 放在注册函数最前：拒绝必须零副作用——早于 `new_name()`/`tree.write()`，只做纯字符串
    /// 判定（不碰树、不建目录、不起线程）。
    ///
    /// - `fs`（默认／空串）：本地路径即可（不强制绝对；生产侧 host 传 realpath）——现状不变。
    /// - `memory`：**拒绝**——家族不可持久化：`WtNode` 与 `worktrees.md` 都没有 scheme 字段，
    ///   重启时 `restore_project_worktrees` 会无条件按 `FsBackend` 重建；需要非持久 wt 时走
    ///   `derive_worktree`（`external_root: None`，本来就不建后端）。
    /// - 服务家族（`remote` 及未来 S3/Redis）：**拒绝**——服务未接入（`Cargo.toml` 只编
    ///   `services-fs`/`services-memory`，`create_operator` 里 `Scheme::Remote` 现落 Memory 占位桩），
    ///   拒绝以免“看着成功、其实没存”。
    /// - 票据形态的根（任何 scheme）：**拒绝**并指路——票据表示对端会话，不是存储位置；
    ///   远端子树请用 `rule_add_remote(wt_uid, vfs_path, entry_id)`（与 `worktree mount` 同路）。
    fn contract_check_external_root(external_root: &str, scheme: &str) -> Result<()> {
        let root = external_root.trim();
        if root.is_empty() {
            return Err(anyhow!("external_root 不能为空（本地根缺失）"));
        }
        let ticket_like = root.contains("://") || root.starts_with("iroh:") || root.starts_with("iroh-share:");
        // 先判家族、再判根形态：否则服务型根（s3://…）会被“票据”分支拦下并给出错误指路。
        match scheme.trim().to_ascii_lowercase().as_str() {
            "" | "fs" => {
                if ticket_like {
                    return Err(anyhow!(
                        "fs 家族的 external_root 必须是本地路径，收到 {:?}：票据（iroh://…）表示对端会话、\
                         不是存储位置 ⇒ 要挂对端子树请用 rule_add_remote(wt_uid, vfs_path, entry_id)\
                         （与 worktree mount 同路）；服务型根（s3://…）属尚未接入的服务家族。",
                        root
                    ));
                }
                Ok(())
            }
            "memory" => Err(anyhow!(
                "scheme \"memory\" 暂不支持：家族不可持久化（WtNode/worktrees.md 无 scheme 字段，\
                 重启会被 restore_project_worktrees 重建为 FsBackend）；需要非持久 wt 请用 derive_worktree"
            )),
            other => Err(anyhow!(
                "scheme {:?} 暂不支持：S3/Redis 类服务家族尚未接入（只编了 services-fs/services-memory），\
                 拒绝以免静默落到 Memory 算子；服务家族接入属独立立项",
                other
            )),
        }
    }

    /// WT VFS：创建 project WT（从外部路径）。返回 uid。
    /// `scheme`：存储家族选择器（`fs` 默认；`memory` 与服务家族当前显式拒绝，见上）。
    /// `rule_text`：可选子侧挂载（父 VFS 路径段，与 bfork 同语法；空串＝无绑定）。
    /// 注：旧便利链 `register_worktree_at`/`_with_rule`/`_with_scheme` 不存在——它们一律传
    /// `None,None`，与「布局名必填、无扁平兜底」自相矛盾（传 None 必被 FsBackend 拒）。
    ///
    /// 带三根分层布局的注册（host 逐 wt 给 `doc_subdir`/`wal_rel`；**必填**、无默认）。
    pub fn register_worktree_at_with_layout(&self, parent_uid: &str, name: &str, external_root: &str,
                                            scheme: &str, rule_text: &str,
                                            doc_subdir: Option<String>, wal_rel: Option<String>) -> Result<String> {
        Self::contract_check_external_root(external_root, scheme)?;
        let _guard = self._rt.enter();
        let uid = self.new_name();
        // S1b（注册原子化）：**所有可失败步都在改树之前**——规则解析、后端构造均先做；
        // 读父只借读锁（构造后端是阻塞 I/O，不能握着树写锁）。
        let (storage_root, rule, bindings) = {
            let tree = self.tree.read();
            let parent = tree.nodes.get(parent_uid)
                .ok_or_else(|| anyhow!("parent not found: {}", parent_uid))?;
            let storage_root = format!("{}.mds/mounts/mnt_{}/", parent.storage_root, uid);
            let rule = Rule::parse(rule_text);
            let bindings = {
                let bt = rule.bind_text();
                if bt.is_empty() { Vec::new() }
                else { Binding::parse(&bt)? }
            };
            (storage_root, rule, bindings)
        };
        let backend = FsBackend::new_with_docs(external_root, &storage_root,
            doc_subdir.as_deref(), wal_rel.as_deref())?;
        // `load` 内部逐文件 `let _ =` ⇒ 事实上不会失败；仍放在改树之前。
        backend.load(&self.vfs_op, external_root, &storage_root.trim_end_matches('/'))?;
        // 改树：写锁内**重校验父仍在**——放锁期间父可能被 `drop_worktree` 删掉（TOCTOU）。
        // 此后不再有可失败步（`ensure_worktree_storage` 无返回值、`backends.insert` 不会失败）
        // ⇒ 失败一律发生在改树之前，树上不会出现“有节点、无后端”的半成品。
        {
            let mut tree = self.tree.write();
            if !tree.nodes.contains_key(parent_uid) {
                return Err(anyhow!("parent not found: {}", parent_uid));
            }
            tree.nodes.get_mut(parent_uid).unwrap().children.push(uid.clone());
            tree.nodes.insert(uid.clone(), WtNode {
                uid: uid.clone(),
                name: name.to_string(),
                parent: Some(parent_uid.to_string()),
                children: Vec::new(),
                storage_root: storage_root.clone(),
                bindings,
                rule,
                external_root: Some(external_root.to_string()),
                doc_subdir: doc_subdir.clone(),
                wal_rel: wal_rel.clone(),
            });
        }
        self.ensure_worktree_storage(&storage_root);
        self.backends.write().insert(uid.clone(), Box::new(backend));
        Ok(uid)
    }

    /// WT VFS：删除 WT 及其所有子孙。自动收集子孙，调用方只需传顶层 uid。
    pub fn drop_worktree(&self, uids: &[String]) -> Result<()> {
        if uids.contains(&"genesis".to_string()) { return Err(anyhow!("cannot drop genesis")); }

        // B1 fix: expand to include all descendants
        let all_uids = {
            let t = self.tree.read();
            let mut expanded: Vec<String> = Vec::new();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for uid in uids {
                for d in t.collect_descendants(uid) {
                    if seen.insert(d.clone()) { expanded.push(d); }
                }
            }
            expanded
        };

        for uid in &all_uids {
            let doc_ids = self.arena.get_doc_ids_for_worktree(uid);
            for doc_id in &doc_ids { let _ = self.flush_doc_to_vfs(*doc_id); }
            let be = self.backends.read(); let t = self.tree.read();
            if let Some(node) = t.nodes.get(uid) {
                if let Some(proj) = node.responsible_project(&t) {
                    if let Some(backend) = be.get(&proj.uid) {
                        for doc_id in &doc_ids {
                            if let Some(meta) = self.arena.get_meta(*doc_id) {
                                backend.delete_wal(&meta.storage_key).ok();
                            }
                        }
                    }
                }
            }
            drop(t); drop(be);
            // B2 fix: clear hooks for this uid
            self.hook_index.lock().clear_worktree(uid);
            // patch match attr（C1）：wt 卸载同步清归属注册表（defs/regs/wildcard/applied）。
            self.match_registry.lock().clear_worktree(uid);
            self.refresh_match_active();
            self.arena.remove_by_wt_uid(uid);
            for doc_id in &doc_ids { self.sessions.remove(doc_id); }
        }
        let mut tree = self.tree.write();
        for uid in all_uids.iter().rev() {
            if let Some(node) = tree.nodes.remove(uid) {
                if let Some(parent_uid) = &node.parent {
                    if let Some(parent) = tree.nodes.get_mut(parent_uid) { parent.children.retain(|c| c != uid); }
                }
                let _ = self.vfs_op.remove_all(&node.storage_root);
            }
        }
        Ok(())
    }




    pub fn resolve_path(&self, wt_uid: &str, vfs_path: &str, intent_write: bool) -> std::io::Result<u32> {
        let _guard = self._rt.enter();
        let _guard = telemetry_guard!(self);
        let _span = telemetry_span!("vfs_resolve", "wt_uid" => wt_uid, "path" => vfs_path);
        let vfs_path = normalize_vpath(vfs_path);
        let tree = self.tree.read();
        let node = tree.nodes.get(wt_uid)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("WT not found: {}", wt_uid)))?;
        let (storage_key, mode, wt_uid) = node.resolve(&vfs_path, &tree);
        drop(tree);

        if intent_write && mode == MountMode::RO {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Target is RO"));
        }

        if let Some(doc_id) = self.arena.get_id_only(&storage_key) {
            if self.sessions.contains_key(&doc_id) {
                return Ok(doc_id);
            }
        }

        if !self.vfs_op.exists(&storage_key).unwrap_or(false) {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "File not found"));
        }

        Ok(self.arena.get_or_alloc(storage_key, &wt_uid, vfs_path))
    }

    pub fn path_exists(&self, wt_uid: &str, vfs_path: &str) -> Result<bool> {
        let vfs_path = normalize_vpath(vfs_path);
        let tree = self.tree.read();
        let node = tree.nodes.get(wt_uid)
            .ok_or_else(|| anyhow!("WT not found: {}", wt_uid))?;
        let (storage_key, _, _) = node.resolve(&vfs_path, &tree);
        drop(tree);
        Ok(self.vfs_op.exists(&storage_key).unwrap_or(false)
            || self.vfs_op.exists(&format!("{}/", storage_key.trim_end_matches('/'))).unwrap_or(false))
    }

    pub fn head_exists(&self, wt_uid: &str, vfs_path: &str, ir: SelectorIR) -> Result<bool> {
        let doc_id = self.resolve_path(wt_uid, vfs_path, false)?;
        let session = self.open_session(doc_id)?;
        let snapshot = session.publisher.load();
        Ok(crate::selector::head_exists(&snapshot, &ir.steps))
    }

    /// AP §3-④-1:后端归属解析(与 flush_one 同款)——仅在挂载了真实 external_root 的
    /// project 上返回 Some;'/mem' 挂载(genesis external_root=None)下 responsible_project=None → 不镜像。
    /// 注意:闭包在 tree/backends 读锁内执行(与 flush_one 一致);仅用于文件系统操作的低频接入点。
    fn with_responsible_backend<T>(&self, wt_uid: &str, f: impl FnOnce(&dyn Backend) -> T) -> Option<T> {
        let t = self.tree.read();
        let b = self.backends.read();
        t.nodes.get(wt_uid)
            .and_then(|n| n.responsible_project(&t))
            .and_then(|proj| b.get(&proj.uid))
            .map(|bx| f(bx.as_ref()))
    }

    pub fn create_path(&self, wt_uid: &str, vfs_path: &str, parents: bool, verbose: bool) -> Result<Option<Vec<String>>> {
        let orig_path = vfs_path.to_string();
        let vfs_path = normalize_vpath(vfs_path);
        if vfs_path.is_empty() { return Err(anyhow!("empty path")); }
        let tree = self.tree.read();
        let node = tree.nodes.get(wt_uid).ok_or_else(|| anyhow!("WT not found: {}", wt_uid))?;
        let (storage_key, mode, _) = node.resolve(&vfs_path, &tree);
        drop(tree);
        if mode == MountMode::RO { return Err(anyhow!("Permission denied: read-only")); }
        if self.vfs_op.exists(&storage_key).unwrap_or(false) { return Ok(None); }
        let mut records = if verbose { Some(Vec::new()) } else { None };
        let is_file = vfs_path.ends_with(".md");
        if parents && !is_file {
            let parent = storage_key.trim_end_matches('/').rsplitn(2, '/').nth(1).unwrap_or("");
            if !parent.is_empty() && !self.vfs_op.exists(&format!("{}/", parent)).unwrap_or(false) {
                self.vfs_op.create_dir(&format!("{}/", parent))?;
                if let Some(ref mut r) = records { r.push(format!("created directory: /{}", parent.trim_start_matches('/'))); }
            }
        } else if !parents {
            let parent = storage_key.trim_end_matches('/').rsplitn(2, '/').nth(1).unwrap_or("");
            let parent_dir = format!("{}/", parent);
            if !parent.is_empty() && !self.vfs_op.exists(&parent_dir).unwrap_or(false) {
                return Err(anyhow!("Parent directory does not exist, use -f to create"));
            }
        }
        if is_file {
            self.vfs_op.write(&storage_key, Vec::<u8>::new())?;
            // AP §3-④-2 写透镜像(容忍失败——mem 保留 + offload 对账兜底):
            // flush 空串 = 磁盘空文件 + 自动建全父目录链(opendal write 语义);
            // 其 FlushMarker 恰好充当后续内容 edits 的审定点(C1)。
            self.with_responsible_backend(wt_uid, |be| { let _ = be.flush(&storage_key, ""); });
            if let Some(ref mut r) = records { r.push(format!("created file: {}", orig_path)); }
        } else {
            self.vfs_op.create_dir(&format!("{}/", storage_key.trim_end_matches('/')))?;
            // 目录镜像(容忍失败):磁盘 mkdir -p;load 从磁盘 list 恢复目录(含空目录)。
            self.with_responsible_backend(wt_uid, |be| { let _ = be.ensure_dir(&storage_key); });
            if let Some(ref mut r) = records { r.push(format!("created directory: {}", orig_path)); }
        }
        Ok(records)
    }

    pub fn remove_path(&self, wt_uid: &str, vfs_path: &str, recursive: bool, force: bool, verbose: bool) -> Result<Option<Vec<String>>> {
        let orig_path = vfs_path.to_string();
        let vfs_path = normalize_vpath(vfs_path);
        let tree = self.tree.read();
        let node = tree.nodes.get(wt_uid).ok_or_else(|| anyhow!("WT not found: {}", wt_uid))?;
        let (storage_key, mode, _) = node.resolve(&vfs_path, &tree);
        drop(tree);
        if mode == MountMode::RO { return Err(anyhow!("Permission denied: read-only")); }
        if !self.vfs_op.exists(&storage_key).unwrap_or(false)
            && !self.vfs_op.exists(&format!("{}/", storage_key.trim_end_matches('/'))).unwrap_or(false) {
            if force { return Ok(None); }
            return Err(anyhow!("Path not found"));
        }
        let mut records = if verbose { Some(Vec::new()) } else { None };
        // normalize_vpath 会去除尾部 /，导致目录 stat 失败；用 / 结尾的路径再试
        let storage_dir = format!("{}/", storage_key.trim_end_matches('/'));
        if recursive && (self.vfs_op.stat(&storage_key).map(|m| m.is_dir()).unwrap_or(false)
                        || self.vfs_op.stat(&storage_dir).map(|m| m.is_dir()).unwrap_or(false)) {
            // G7(AP §3-④-3):目录内所有已开 session 先关——否则 dirty 在 100ms 内被 flush 线程写回复活。
            for doc_id in self.arena.get_doc_ids_by_prefix(&storage_dir) { self.sessions.remove(&doc_id); }
            // C2(AP §1.2):磁盘先行、失败即报错、mem 不动——否则下次 load 泵回 = 删除复活。
            let mut mirror_err: Option<anyhow::Error> = None;
            self.with_responsible_backend(wt_uid, |be| {
                if let Err(e) = be.remove(&storage_key, true) { mirror_err = Some(e); }
            });
            if let Some(e) = mirror_err { return Err(e); }
            self.vfs_op.remove_all(&storage_dir)?;
            if let Some(ref mut r) = records { r.push(format!("removed directory: {}", orig_path)); }
        } else {
            // 单文件:同样先关 session(res_doc 同款),再 C2 磁盘先行。
            if let Some(doc_id) = self.arena.get_id_only(&storage_key) { self.sessions.remove(&doc_id); }
            let mut mirror_err: Option<anyhow::Error> = None;
            self.with_responsible_backend(wt_uid, |be| {
                if let Err(e) = be.remove(&storage_key, false) { mirror_err = Some(e); }
            });
            if let Some(e) = mirror_err { return Err(e); }
            // WAL 删除(先例:drop_worktree store.rs:885);失败容忍——孤儿残留属已知类别(G5)。
            self.with_responsible_backend(wt_uid, |be| { let _ = be.delete_wal(&storage_key); });
            self.vfs_op.delete(&storage_key)?;
            if let Some(ref mut r) = records { r.push(format!("removed file: {}", orig_path)); }
        }
        Ok(records)
    }

    /// reload:以磁盘为准同步 worktree 的内存镜像(清理磁盘已消失的镜像 + 重泵磁盘现存文件)。
    /// 与 offload 反向对称(offload=以 mem 为准对账到磁盘);仅支持有 external_root 的
    /// WT(genesis/项目 WT),bfork child 无 external_root → 拒绝(视图含 remap 路径会跨 WT)。
    /// 语义:以磁盘为准,内存中未落盘的编辑丢弃(随返回警告列出 dirty doc)。
    pub fn reload(&self, wt_uid: &str) -> Result<Vec<String>> {
        let _guard = self._rt.enter();
        let (ext_root, storage_root) = {
            let t = self.tree.read();
            let node = t.nodes.get(wt_uid).ok_or_else(|| anyhow!("WT not found: {}", wt_uid))?;
            let ext = node.external_root.clone()
                .ok_or_else(|| anyhow!("reload requires external_root (genesis/project WT); bfork child unsupported"))?;
            (ext, node.storage_root.clone())
        };
        let mut paths = Vec::new();
        let mut visited: HashMap<String, bool> = HashMap::new();
        {
            let t = self.tree.read();
            self.collect_vfs_paths(wt_uid, false, &t, &mut paths, &mut visited);
        }
        let mut keys: Vec<String> = Vec::new();
        {
            let t = self.tree.read();
            let node = t.nodes.get(wt_uid).unwrap();
            for p in &paths {
                let (sk, _mode, owner) = node.resolve(p, &t);
                if owner != wt_uid { continue; }
                keys.push(sk);
            }
        }
        let mut dropped_dirty: Vec<String> = Vec::new();
        for doc_id in self.arena.get_doc_ids_for_worktree(wt_uid) {
            if let Some(sess) = self.sessions.get(&doc_id) {
                if sess.dirty.load(Ordering::Relaxed) {
                    if let Some(meta) = self.arena.get_meta(doc_id) { dropped_dirty.push(meta.vfs_path.clone()); }
                }
            }
            self.sessions.remove(&doc_id);
        }
        let removed = self.with_responsible_backend(wt_uid, |be| be.reload(&self.vfs_op, &ext_root, &storage_root, &keys))
            .ok_or_else(|| anyhow!("reload: no backend for {}", wt_uid))??;
        let mut out = removed;
        if !dropped_dirty.is_empty() {
            out.push(format!("dropped {} dirty (unsaved) doc(s): {}", dropped_dirty.len(), dropped_dirty.join(", ")));
        }
        Ok(out)
    }

    /// Insert a handoff-copied snapshot as a normal writable document session.
    /// Writes the VFS text first (create_path only creates an empty file), resolves
    /// doc_id/arena, replaces any old session, and inserts a from_snapshot session.
    pub fn put_snapshot_doc(
        &self,
        wt_uid: &str,
        vfs_path: &str,
        text: &str,
        snapshot: crate::engine::MdioSnapshot,
        peer_id: u64,
    ) -> std::io::Result<u32> {
        self.create_path(wt_uid, vfs_path, true, false)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let doc_id = self.resolve_path(wt_uid, vfs_path, false)?;
        let storage_key = self.arena.get_meta(doc_id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "doc arena meta missing"))?
            .storage_key
            .clone();
        self.vfs_op.write(&storage_key, text.as_bytes().to_vec())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        let session = MdioSession::from_snapshot(
            doc_id,
            snapshot,
            peer_id,
            storage_key,
            None,
            Some(self.detect_tx.clone()),
            Some(self.path_tx.clone()),
            self.hook_docs.clone(),
            self.detect_clock.clone(),
        );
        self.sessions.insert(doc_id, Arc::new(session));
        Ok(doc_id)
    }

    pub fn open_session(&self, doc_id: u32) -> std::io::Result<Arc<MdioSession>> {
        let _guard = self._rt.enter();
        let _guard = telemetry_guard!(self);
        let _span = telemetry_span!("session_open", "doc_id" => doc_id);
        if let Some(s) = self.sessions.get(&doc_id) { return Ok(s.clone()); }

        let meta = self.arena.get_meta(doc_id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Doc ID Missing"))?;

        let file_bytes = self.vfs_op.read(&meta.storage_key)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
            .to_vec();

        let mut text = String::from_utf8(file_bytes).unwrap_or_default();

        // Crash recovery: 通过 Backend::replay_wal 恢复
        let be = self.backends.read();
        let t = self.tree.read();
        let project_uid = t.nodes.get(&meta.wt_uid)
            .and_then(|n| n.responsible_project(&t))
            .map(|n| n.uid.clone()).unwrap_or_else(|| "genesis".to_string());
        drop(t);
        if let Some(backend) = be.get(&project_uid) {
            if let Some(recovered) = backend.replay_wal(&meta.storage_key, &text) {
                let _ = self.vfs_op.write(&meta.storage_key, recovered.clone().into_bytes());
                text = recovered;
            }
        }

        let wal_q = be.get(&project_uid).and_then(|b| b.wal_queue());
        drop(be);

        let session = Arc::new(MdioSession::new(
            doc_id, &text, rand::random(),
            meta.storage_key.clone(),
            wal_q,
            Some(self.detect_tx.clone()),
            Some(self.path_tx.clone()),
            self.hook_docs.clone(),
            self.detect_clock.clone(),
        ));

        let is_sys = meta.vfs_path.starts_with("/.mds/");
        if is_sys {
            let mut w = session.write_guard.lock();
            let snap = w.engine.make_snapshot();
            session.publisher.store(Arc::new(snap));
        }

        self.sessions.insert(doc_id, session.clone());
        Ok(session)
    }

    /// 从 checkpoint（bincode: (text, cola::EncodedReplica, lineage)）恢复 session。
    /// 替换 DashMap 中已有的 session（若存在），使两侧共享 CRDT state。
    /// current_text 从现有 session engine 读取（VFS 写入是异步的，engine 才是最新）。
    /// 纯解码 checkpoint → 返回文本内容。不修改任何 session。
    pub fn decode_checkpoint_text(checkpoint_bytes: &[u8]) -> Result<String> {
        let (dump, _): ((String, cola::EncodedReplica, u64), usize) =
            bincode::serde::decode_from_slice(checkpoint_bytes, crate::session::bincode_config())
            .map_err(|e| anyhow!("Invalid checkpoint: {}", e))?;
        Ok(dump.0)
    }

    pub fn load_checkpoint(&self, doc_id: u32, checkpoint_bytes: &[u8]) -> Result<()> {
        let _guard = telemetry_guard!(self);
        let _span = telemetry_span!("load_checkpoint", "doc_id" => doc_id);
        let _guard = self._rt.enter();

        // 从现有 session 读最新文本（VFS 可能尚未 flush）
        let current_text = if let Some(old_session) = self.sessions.get(&doc_id) {
            old_session.get_engine_text()
        } else {
            // fallback: VFS read
            let meta = self.arena.get_meta(doc_id)
                .ok_or_else(|| anyhow!("Doc ID Missing: {}", doc_id))?;
            let file_bytes = self.vfs_op.read(&meta.storage_key)
                .map_err(|e| anyhow!("VFS read error: {}", e))?
                .to_vec();
            String::from_utf8(file_bytes).unwrap_or_default()
        };

        let meta = self.arena.get_meta(doc_id)
            .ok_or_else(|| anyhow!("Doc ID Missing: {}", doc_id))?;

        let be = self.backends.read();
        let t = self.tree.read();
        let project_uid = t.nodes.get(&meta.wt_uid)
            .and_then(|n| n.responsible_project(&t))
            .map(|n| n.uid.clone()).unwrap_or_else(|| "genesis".to_string());
        drop(t);
        let wal_q = be.get(&project_uid).and_then(|b| b.wal_queue());
        drop(be);

        let peer_id: u64 = rand::random();

        let session = Arc::new(MdioSession::from_checkpoint(
            doc_id, checkpoint_bytes, peer_id, &current_text,
            meta.storage_key.clone(),
            wal_q,
            Some(self.detect_tx.clone()),
            Some(self.path_tx.clone()),
            self.hook_docs.clone(),
            self.detect_clock.clone(),
        ).map_err(|e| anyhow!("Checkpoint load failed: {}", e))?);

        self.sessions.insert(doc_id, session);
        Ok(())
    }

    pub fn get_file_metadata(&self, wt_uid: &str, vfs_path: &str) -> Result<(u64, u64, bool)> {
        let vfs_path = normalize_vpath(vfs_path);
        let tree = self.tree.read();
        let node = tree.nodes.get(wt_uid)
            .ok_or_else(|| anyhow!("WT not found: {}", wt_uid))?;
        let (storage_key, mode, _) = node.resolve(&vfs_path, &tree);
        drop(tree);
        let meta = self.vfs_op.stat(&storage_key)
            .or_else(|_| self.vfs_op.stat(&format!("{}/", storage_key.trim_end_matches('/'))))?;
        let size = meta.content_length();
        let modified = meta.last_modified()
            .and_then(|t| {
                let sys_time: std::time::SystemTime = t.into();
                sys_time.duration_since(std::time::UNIX_EPOCH).ok()
            })
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok((size, modified, mode == MountMode::RO))
    }

    pub fn is_dir(&self, wt_uid: &str, vfs_path: &str) -> Result<bool> {
        let tree = self.tree.read();
        let node = tree.nodes.get(wt_uid)
            .ok_or_else(|| anyhow!("WT not found: {}", wt_uid))?;
        let (storage_key, _, _) = node.resolve(vfs_path, &tree);
        drop(tree);
        let meta = self.vfs_op.stat(&storage_key)
            .or_else(|_| self.vfs_op.stat(&format!("{}/", storage_key.trim_end_matches('/'))))?;
        Ok(meta.is_dir())
    }

    pub fn flush_worktree(&self, wt_uid: &str) {
        for doc_id in self.arena.get_doc_ids_for_worktree(wt_uid) { self.flush_doc_to_vfs(doc_id).ok(); }
    }

    pub fn flush_doc_to_vfs(&self, doc_id: u32) -> std::io::Result<()> {
        let _guard = self._rt.enter();
        let _guard = telemetry_guard!(self);
        let _span = telemetry_span!("doc_flush", "doc_id" => doc_id);
        if let Some(sess) = self.sessions.get(&doc_id) {
            if let Some(meta) = self.arena.get_meta(doc_id) {
                self.vfs_op.write(&meta.storage_key, sess.get_engine_text().into_bytes())
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
            }
        }
        Ok(())
    }

    /// 反查 doc_id 归属的 WT uid。供 Python dispatch 路由用。
    pub fn doc_wt_uid(&self, doc_id: u32) -> Option<String> {
        self.arena.get_meta(doc_id).map(|m| m.wt_uid.clone())
    }


    // WebSockets / Net 协议

    /// `MDIO_ADMIT_GATE`：默认 "1"（门在岗）；"0" ⇒ 跳过门（负对照用）。OnceLock 缓存一次读。
    fn admit_gate_on() -> bool {
        static GATE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *GATE.get_or_init(|| std::env::var("MDIO_ADMIT_GATE").map(|v| v != "0").unwrap_or(true))
    }

    /// 连接 wt 视图下的可达判定（**归属 ∨ 挂载可达**）——R1 把 Python 侧 `_reachable_doc_ids` 的
    /// 判定搬进协议层：`ArenaMeta.wt_uid` 给**归属**（arena 元数据查表，O(1)）；`WtNode.children`
    /// 的后代闭包 + 各后代 `storage_root`（形如 `…/.mds/mounts/mnt_<uid>/`）给**挂载可达**
    /// （`WtNode::resolve` 产出的 storage_key = storage_root + "/" + vfs_path）。
    /// doc 无元数据 ⇒ 与旧 Python 版「不在可达集」同判 = 拒绝；conn_wt 不在树上 ⇒ 判不了 ⇒ 接受
    /// （沿用安全侧退化）。
    fn doc_reachable_from(&self, conn_wt: &str, doc_id: u32) -> bool {
        let (verdict, why, dump) = self.doc_reachable_eval(conn_wt, doc_id);
        if Self::admit_debug_on() { Self::admit_debug_print(conn_wt, doc_id, verdict, &why, &dump); }
        verdict
    }

    /// `MDIO_ADMIT_DEBUG`：默认关；= "1" ⇒ 每个**新的** (conn_wt, doc_id) 判定打一行全量快照（T6 钉实测）。
    fn admit_debug_on() -> bool {
        static DBG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *DBG.get_or_init(|| std::env::var("MDIO_ADMIT_DEBUG").map(|v| v == "1").unwrap_or(false))
    }

    /// `MDIO_ADMIT_DEBUG_TAG`：本次调试的名字（可空）。行里带上它、去重表按它分区
    /// ⇒ 多人同时开调试时，各自的首行不会被对方的同名对吞掉（T6 实测踩到过：浏览器复现被吞）。
    fn admit_debug_tag() -> &'static str {
        static TAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        TAG.get_or_init(|| std::env::var("MDIO_ADMIT_DEBUG_TAG").unwrap_or_default())
    }

    /// **每次判定各打一行，不去重**。去重版本（原按 conn_wt+doc_id 只打首行）会在并发复现时
    /// 把「第一条」让给别人的同名对，实测把 T6 的浏览器复现整整吞掉一轮 ⇒ 诊断件宁可多打。
    fn admit_debug_print(conn_wt: &str, doc_id: u32, verdict: bool, why: &str, dump: &str) {
        println!("[ADMIT_DBG] tag={} conn_wt={} doc={} verdict={} why={} | {}",
                 Self::admit_debug_tag(), conn_wt, doc_id,
                 if verdict { "ACCEPT" } else { "REJECT" }, why, dump);
    }

    /// `MDIO_ADMIT_DEBUG=1` 时打一行 `collect_vfs_paths` 的**纯遍历**耗时（条数 + 是否含隐藏项 + 微秒）。
    /// **计时点在树读锁内**（该遍历需要 &WtTree）⇒ 量到的就是「持读锁跑这次遍历」的成本，不含打印。
    fn vfs_paths_debug(wt_uid: &str, include_hidden: bool, n: usize, dt: std::time::Duration) {
        println!("[VFS_PATHS_DBG] tag={} wt={} hidden={} n={} us={}",
                 Self::admit_debug_tag(), wt_uid, if include_hidden { 1 } else { 0 }, n, dt.as_micros());
    }

    /// 判定本体：返回 (结论, 命中的子判据, 快照文本)。快照在**树读锁在手时**就地构建，
    /// 返回后锁已释放再由上层打印 ⇒ 调试路径不重入 tree 锁、不在持锁时做 IO。
    /// 连接 wt 视图下的可达判定 —— 反演法。
    ///
    /// 与旧 Python 门 \`_reachable_doc_ids\`（按 conn_wt 视图的 id 集）逐格等价：
    /// T7 在 32 格（真实盘面 + 合成 + 边界）模拟器上验到原 24 格 **0 差异**、全部 32 格仅 V1 一格差异。
    ///
    /// 直接支：\`storage_key\` 去掉 \`conn_wt.storage_root\` 前缀得本地路径 p ⇒ \`resolve(p)\` 必须**逐字回到同一个
    ///         key**（同一性）且该 key 真实存在（活值 ⇒ delete/recreate 天然正确）。
    /// 绑定支：对每条 binding 取 \`keyT = resolve(b.my_path)\`；SK 落在 keyT 之下 ⇒ 反演本地路径
    ///         \`local = b.my_path + SK[keyT.len()..]\` ⇒ 再验同一性 + 存在（多级链闭合：keyT 已递归到最终 owner）。
    ///
    /// 为什么不用 \`meta.wt_uid\`、也不用子树前缀：\`ArenaMeta.wt_uid\` 由**首个枚举到该 key 的视图**冻结
    /// （\`get_or_alloc\` 命中即早退），与"哪个连接该看得到"无关 ⇒ 任何基于它的归属判据都带铸造
    /// 顺序脆弱性（挂载连接全拒就是这条病的现场）。也不用枚举：\`collect_vfs_paths\` 每包约 30 次 list + 228 次
    /// resolve（根连接），而树版本不随文档增删变 ⇒ 缓存必然陈旧，陈旧即误拒，而误拒会被客户端当作"撕掉该 doc
    /// 全部本地会话"。
    ///
    /// 已知与旧门的唯一分歧（V1）：\`walk_local\` 会跳过"与所在目录同名的文件"（
    /// \`if n == dn { continue }\`），枚举门漏计而本判据看到真实文件即放行 —— 放行的是 conn_wt **自己**命名空间
    /// 里的文件、不是跨树泄露，故**刻意不补**同款守卫（那等于把枚举的假阴性搬进来）。
    fn doc_reachable_eval(&self, conn_wt: &str, doc_id: u32) -> (bool, String, String) {
        let Some(meta) = self.arena.get_meta(doc_id) else {
            return (false, "ⓒmeta缺失⇒拒".to_string(),
                    format!("meta=<none> conn_wt={} conn_node=<未查（meta 缺失即拒）>", conn_wt));
        };
        let head = format!("meta: wt_uid={} storage_key={} vfs_path={}",
                           meta.wt_uid, meta.storage_key, meta.vfs_path);
        // A1（红队必改项）：判定前确保树最新 —— gateway 的 mount/rule 流程不调 tree_ensure，
        // 否则"刚登记的挂载"在下面的未知分支被整拒 ⇒ 0xFF ⇒ 客户端撕会话。
        // 必须在取 tree 读锁**之前**调（ensure 要写锁）。
        self.tree.write().ensure(self);
        let tree = self.tree.read();
        let Some(node) = tree.nodes.get(conn_wt) else {
            return (false, "A7conn_wt 不在树上⇒拒（旧门对未知 wt 得空集 ⇒ 拒）".to_string(),
                    format!("{} | conn_node=<none>", head));
        };
        let node_dump = format!(
            "conn_node<{}>: storage_root={} external_root={} bindings={} children={:?}",
            conn_wt, node.storage_root,
            node.external_root.as_deref().unwrap_or("<none>"), node.bindings.len(), node.children);
        let sk = meta.storage_key.as_str();
        // is_file：**逐字镜像** walk_local 的 push 条件（`if is_dir { recurse } else { push }`）⇒ `!is_dir()`。
        // 不用 is_file()：opendal 0.55 的 EntryMode 有 Unknown（且是 default）⇒ is_file() 与 is_dir() 同时为假，
        // 而 walk_local 会把 Unknown 当文档 push ⇒ 只有 `!is_dir()` 才是它的补集（T7/T10 核过 vendored 源码）。
        // stat 必须打 `self.vfs_op`（Memory，与 walk_local 同一个 operator）⇒ delete/recreate 的活值性质同源。
        let is_file = match self.vfs_op.stat(sk) {
            Ok(m) => !m.is_dir(),
            Err(_) => false,   // NotFound ⇒ 拒（fail-closed）
        };
        // 直接支
        let sr = node.storage_root.trim_end_matches('/');
        let p: String = if sr.is_empty() {
            sk.to_string()
        } else if let Some(rest) = sk.strip_prefix(&format!("{}/", sr)) {
            format!("/{}", rest)
        } else {
            String::new()
        };
        if !p.is_empty() {
            let (k, _m, _o) = node.resolve(&p, &tree);
            if k == sk && is_file {
                return (true, "④反演·直接支命中".to_string(), format!("{} | {}", head, node_dump));
            }
        }
        // 绑定支
        for b in &node.bindings {
            let (key_t, _m, _o) = node.resolve(&b.my_path, &tree);
            let kt = key_t.trim_end_matches('/');
            let rel = if sk == kt { "" }
                      else if let Some(r) = sk.strip_prefix(&format!("{}/", kt)) { r }
                      else { continue };
            let local = if rel.is_empty() { b.my_path.clone() }
                        else { format!("{}/{}", b.my_path.trim_end_matches('/'), rel) };
            let (k2, _m2, _o2) = node.resolve(&local, &tree);
            if k2 == sk && is_file {
                return (true, "④反演·绑定支命中".to_string(), format!("{} | {}", head, node_dump));
            }
        }
        (false, "④反演两支均不命中⇒拒".to_string(), format!("{} | {}", head, node_dump))
    }

    pub fn handle_packet(&self, packet: &[u8], conn_wt: Option<&str>) -> ServerAction {
        if packet.len() < HEADER_SIZE { return ServerAction::None; }
        let tag = packet[0];
        let doc_id = u32::from_le_bytes(packet[9..13].try_into().unwrap());

        // ── 协议层准入（R1）──────────────────────────────────────────────
        // 门从 Python 搬到这里：本函数**每包本来就被调用一次** ⇒ 零额外 FFI。
        // 只判 UP_OPEN(0x02) / UP_SYNC(0x04)；conn_wt = None ⇒ 无连接上下文，不设门。
        // 拒绝 ⇒ DnErr(0xFF)（引擎 DnErr 分支本就完整：「拒绝必须协议可见」）。
        if Self::admit_gate_on() {
            if let Some(cw) = conn_wt {
                if (tag == UP_OPEN || tag == UP_SYNC) && !self.doc_reachable_from(cw, doc_id) {
                    let peer = u32::from_le_bytes(packet[5..9].try_into().unwrap());
                    let reason = format!("doc={} 不在连接 wt={} 的可达集", doc_id, cw);
                    return ServerAction::Direct { data: self.build_pkg(0xFF, peer, doc_id, reason.as_bytes()) };
                }
            }
        }

        #[cfg(feature = "telemetry")]
        let t_enter = std::time::Instant::now();

        let result = match tag {
            UP_TREE => {
                let json_bytes = self.refresh_tree_json("genesis", false).unwrap_or(vec![]);
                let resp = self.build_pkg(DN_TREE, 0, 0, &json_bytes);
                ServerAction::DirectAndSub { channel: "sys_events".to_string(), data: resp }
            },
            UP_OPEN => {
                // 帧头 peer = 请求者（客户端 adapter id）。**必须回填**：原实现写死 0，
                // 客户端只能按 pending_mode_mask 的 HashMap 遍历序猜"这条回复是谁要的"
                // ——两视图下净效果恰好等价，一旦某 adapter 还挂着 pending（上次零帧）
                // 或视图数 >2，回复就会被错误的 peer 吃掉而新会话永不建立。
                // 回填后 DnOpen 与 UpOpen 一一对应。见
                // research/2026-09-21-lineage-single-source-model.md §11.3 I1c。
                let req_peer = u32::from_le_bytes(packet[5..9].try_into().unwrap());
                match self.open_session(doc_id) {
                    Ok(sess) => {
                        let snap = sess.get_checkpoint();
                        let resp = self.build_pkg(DN_OPEN, req_peer, doc_id, &snap);
                        ServerAction::DirectAndSub { channel: format!("doc_{}", doc_id), data: resp }
                    },
                    Err(e) => {
                        // 打不开 = 文档不存在（不存在/路径失效）。同样不能静默：客户端会一直等
                        // 那帧永远不来的 DnOpen（零帧），而它自己并不知道原因。回 DN_ERR，
                        // 由客户端把本地会话收掉并把原因报给宿主。
                        eprintln!("[Rust] Open Error: doc={} peer={} {:?} ⇒ 回 DN_ERR(open_failed)", doc_id, req_peer, e);
                        ServerAction::Direct {
                            data: self.build_pkg(DN_ERR, req_peer, doc_id, b"open_failed"),
                        }
                    }
                }
            },
            UP_SYNC => {
                // 上行载荷与服务端下行同一约定：[lineage: u64 LE][bincode(Vec<WireOp>)]。
                // 血统不等 ⇒ 这笔编辑锚在一条**已死**的血统上（服务端 reload/__reset__ 换过代），
                // 集成进去只会进 pending 永久停车、且**毫无痕迹**。所以：不集成，
                // 直接把当前 checkpoint 回推给发送者（客户端 DnOpen 收到即整篇重取）。
                let req_peer = u32::from_le_bytes(packet[5..9].try_into().unwrap());
                let (op_lineage, op_bytes) = Self::split_lineage(&packet[HEADER_SIZE..]);
                let cur_lineage = self.sessions.get(&doc_id).map(|s| s.lineage);
                let Some(cur) = cur_lineage else {
                    // **没有这个 doc 的 session**（被 rm / reload 摘掉，或客户端拿着陈旧 doc_id）。
                    // 这里原先返回 None = 帧被默默丢掉：客户端本地已经应用了这笔编辑，不回话
                    // 就成了"看起来存上了、其实没存"。DN_ERR 直到今天只有常量没有生产者——
                    // 它的第一个真实语义就是 doc_gone。
                    eprintln!("[Rust] UP_SYNC 无此文档: doc={} peer={} ⇒ 回 DN_ERR(doc_gone)", doc_id, req_peer);
                    return ServerAction::Direct {
                        data: self.build_pkg(DN_ERR, req_peer, doc_id, b"doc_gone"),
                    };
                };
                if let Some(l) = op_lineage {
                    if l != cur {
                        eprintln!("[Rust] UP_SYNC 血统不等: doc={} op={} 当前={} peer={} ⇒ 不集成，回推 checkpoint",
                            doc_id, l, cur, req_peer);
                        return match self.build_dn_open(doc_id, req_peer) {
                            Some(pkt) => ServerAction::Direct { data: pkt },
                            None => ServerAction::None,
                        };
                    }
                }
                if let Some(sess) = self.sessions.get(&doc_id) {
                    sess.apply_update(op_bytes);
                    // 广播给其余订阅者时，载荷前面缀上**本会话当前血统号**（session::next_lineage）。
                    // 接收端凭它判断自己那份副本是否还是这条血统的后代：不是就整篇重取，
                    // 而不是把 op 集成进一条对不上的血统里（那会 integrate None 且永久静默）。
                    // 帧头 peer 归零：广播路径的 peer 只用于发送端回声过滤，服务端已按
                    // source_peer_id 排除来源连接（crdt_scope.broadcast）。
                    let payload = Self::stamp_lineage(cur, op_bytes);
                    let broadcast_pkg = self.build_pkg(DN_SYNC, 0, doc_id, &payload);
                    ServerAction::Broadcast { channel: format!("doc_{}", doc_id), data: broadcast_pkg }
                } else { ServerAction::None }
            },
            UP_CLOSE => ServerAction::Unsubscribe { channel: format!("doc_{}", doc_id) },
            DN_SYNC => {
                // 服务器端同步（来自 write_locator 的广播）
                if let Some(sess) = self.sessions.get(&doc_id) {
                    let op_bytes = &packet[HEADER_SIZE..];
                    sess.apply_update(op_bytes);
                }
                ServerAction::None
            },
            UP_FILE | DN_FILE => {
                let pipe = u32::from_le_bytes(packet[9..13].try_into().unwrap());
                let entry_id = format!("{:08x}", pipe);
                let op_bytes = &packet[HEADER_SIZE..];
                if let Ok((op, _)) = bincode::serde::decode_from_slice::<crate::protocol::FileOp, _>(
                    op_bytes, bincode_cfg(),
                ) {
                    ServerAction::FileAction { entry_id, op }
                } else {
                    ServerAction::None
                }
            },
            #[cfg(feature = "telemetry")]
            crate::protocol::TELEMETRY_PROBE => {
                let pkg = PacketBuffer::from_slice(packet);
                let mut return_pkg = pkg;
                return_pkg[0] = crate::protocol::TELEMETRY_RETURN;
                ServerAction::Direct { data: return_pkg }
            },
            _ => ServerAction::None
        };

        // ── telemetry stamp ──
        #[cfg(feature = "telemetry")]
        if let Some(ref sess) = *self.telemetry_session.lock() {
            let dur = t_enter.elapsed().as_nanos() as u64;
            let bucket = crate::telemetry::SampleBucket::from_tag(tag);
            sess.add_layer_sample("RS_proc", dur, bucket, tag, doc_id);
        }

        result
    }

    /// DN_SYNC 载荷 = [lineage: u64 LE][bincode(Vec<WireOp>)]。
    /// 服务端发出的每个 CRDT 帧都带血统号——接收端据此判断"我这份副本还对得上吗"，
    /// 对不上就整篇重取（换代），而不是把 op 集成进一条陌生的血统里。
    pub(crate) fn stamp_lineage(lineage: u64, ops: &[u8]) -> Vec<u8> {
        let mut p = Vec::with_capacity(8 + ops.len());
        p.extend_from_slice(&lineage.to_le_bytes());
        p.extend_from_slice(ops);
        p
    }

    /// 上行载荷解前缀：[lineage: u64 LE][bincode(Vec<WireOp>)] → (Some(血统号), 纯 op 字节)。
    ///
    /// 判别**靠格式自证**：去掉 8 字节前缀后必须能在同一 bincode 配置下 decode 成
    /// Vec<WireOp>，**且恰好吃光整个后缀**。判不出前缀就按旧格式原样返回 (None, body)，
    /// 交给 apply_update 自己去解码/报错——这样旧客户端（无前缀）不会把那 8 个字节误读
    /// 成血统号，从而不会拿一个随机数去比对血统、无谓触发换代。
    ///
    /// 「恰好吃光」这一条不是洁癖：只判"decode 成功"时，一段全零尾巴会被解成**空 op 表**
    /// 并消耗 1 字节就返回成功，于是任何 ≥8 字节的旧载荷都会被误判成带号
    /// （判官 lineage_wire::legacy_payload_is_not_misread_as_lineage 抓的就是这个）。
    pub(crate) fn split_lineage(body: &[u8]) -> (Option<u64>, &[u8]) {
        if body.len() > 8 {
            let l = u64::from_le_bytes(body[0..8].try_into().unwrap());
            let tail = &body[8..];
            if let Ok((_, used)) =
                bincode::serde::decode_from_slice::<Vec<crate::session::WireOp>, _>(tail, bincode_cfg())
            {
                if used == tail.len() {
                    return (Some(l), tail);
                }
            }
        }
        (None, body)
    }

    /// 构造一帧 DN_OPEN（供服务端**主动**推送换代：血统变了就推一次，让订阅者整篇重取）。
    /// 文档不存在/会话打不开 ⇒ None。
    pub fn build_dn_open(&self, doc_id: u32, peer: u32) -> Option<PacketBuffer> {
        match self.open_session(doc_id) {
            Ok(sess) => {
                let snap = sess.get_checkpoint();
                Some(self.build_pkg(DN_OPEN, peer, doc_id, &snap))
            }
            Err(_) => None,
        }
    }

    /// 该文档当前血统号（无会话 ⇒ None）。网关用它判断"要不要给订阅者推换代"。
    pub fn doc_lineage(&self, doc_id: u32) -> Option<u64> {
        self.sessions.get(&doc_id).map(|s| s.lineage)
    }

    pub(crate) fn build_pkg(&self, tag: u8, peer: u32, doc: u32, payload: &[u8]) -> PacketBuffer {
        let len = payload.len() as u32;
        let mut buf = PacketBuffer::with_capacity(HEADER_SIZE + payload.len());
        buf.push(tag);
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&peer.to_le_bytes());
        buf.extend_from_slice(&doc.to_le_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    // ── FFI 跨语言查询接口 ──────────────────────────────────

    pub fn refresh_tree_json(&self, wt_uid: &str, include_hidden: bool) -> std::io::Result<Vec<u8>> {
        let _guard = self._rt.enter();
        let tree = self.tree.read();
        // 通过 visible_paths 获取完整虚拟视图
        let mut paths = Vec::new();
        let mut visited: HashMap<String, bool> = HashMap::new();
        // 诊断计时（默认关 ⇒ 连 Instant::now 都不取）。计时点在树读锁内：量的是「持锁跑这次遍历」。
        let t_paths = if Self::admit_debug_on() { Some(std::time::Instant::now()) } else { None };
        self.collect_vfs_paths(wt_uid, include_hidden, &tree, &mut paths, &mut visited);
        if let Some(t) = t_paths { Self::vfs_paths_debug(wt_uid, include_hidden, paths.len(), t.elapsed()); }
        drop(tree);
        paths.sort(); paths.dedup();
        let mut nodes_with_id = Vec::new();
        let tree = self.tree.read();
        for vfs_path in &paths {
            let node = match tree.nodes.get(wt_uid) { Some(n) => n, None => continue };
            let (storage_key, _, _) = node.resolve(vfs_path, &tree);
            let id = self.arena.get_or_alloc(storage_key.clone(), wt_uid, vfs_path.clone());
            let hash = self.vfs_op.read(&storage_key)
                .ok()
                .and_then(|d| String::from_utf8(d.to_vec()).ok())
                .map(|s| crate::snap::state_hash(&s))
                .unwrap_or_default();
            nodes_with_id.push(serde_json::json!({ "path": vfs_path, "id": id, "hash": hash }));
        }
        drop(tree);
        Ok(serde_json::to_vec(&nodes_with_id).unwrap_or(vec![]))
    }


    // Hook API (synchronous, via HookIndex)

    pub fn register_hook(&self, wt_uid: &str, doc_id: u32, ir: SelectorIR, cb_code: &str, ctx_val: serde_json::Value, is_always: bool) -> u64 {
        let _span = telemetry_span!("hook_register", "doc_id" => doc_id, "step_count" => ir.steps.len());
        // hook_id 0 = 未注册（哨兵）。以前这里静默跳过却照返回一个随机 id，Python 侧据此
        // 打印 "OK" 并把条目写进 hook.md ⇒ 「报成功没做事」。现在失败必须可观测：
        // 日志 + 哨兵值，由 pyo3 包装层（lib.rs register_hook / register_hook_by_uid）转成异常。
        let mut hook_id = rand::random::<u64>();
        if hook_id == 0 { hook_id = 1; }
        match self.open_session(doc_id) {
            Ok(sess) => {
                // 注册临界区：与写路径共用该 doc 的 write_guard，保证水位切割干净：
                // 注册前入队任务 snap_seq < active_from（全部跳过），注册后写 snap_seq >= active_from。
                // 锁序：write_guard → hook_index（worker 只持 hook_index，无环）。
                let _wg = sess.write_guard.lock();
                let active_from = self.detect_clock.watermark();
                let snap = sess.publisher.load();
                self.hook_index.lock().register(hook_id, wt_uid, doc_id, &ir.steps, is_always, cb_code, ctx_val, active_from, &snap);
                self.hook_docs.insert(doc_id);
                hook_id
            }
            Err(e) => {
                eprintln!("[hook] register FAILED (not registered) wt={} doc={}: {}", wt_uid, doc_id, e);
                0
            }
        }
    }
    pub fn remove_hook(&self, hook_id: u64) {
        let mut hi = self.hook_index.lock();
        if let Some(def) = hi.remove(hook_id) { if !hi.has_doc(def.doc_id) { self.hook_docs.remove(&def.doc_id); } }
    }
    pub fn clear_doc_hooks(&self, doc_id: u32, wt_uid: &str) {
        let mut hi = self.hook_index.lock();
        hi.clear_doc(doc_id, wt_uid);
        if !hi.has_doc(doc_id) { self.hook_docs.remove(&doc_id); }
    }

    // ── patch match attr：归属注册 / 注销（spec N2 / N6）────────────────────

    /// 注册一条归属（地址 + 标签）。
    ///
    /// - `file_pattern` 已由 mdio_py 声明层归一成核认 glob（核永不看到 `.*`）；
    /// - D5：`prefix_paths` 全空（无 Drill 前缀）⇒ 返回 `0` 哨兵 + 日志；
    /// - 逐 doc 过同一门：`compute_scope_range` 为 None ⇒ 跳过该文档（永不命中）；
    /// - 通配（pattern 含通配符）⇒ 记模板，新文档出现时由 C5 补登记；
    ///   非通配且无任何 doc 解析出 scope ⇒ 撤销模板并返回 `0`（失败可观测）。
    pub fn register_match(
        &self,
        wt_uid: &str,
        file_pattern: &str,
        ir: SelectorIR,
        tag: &str,
    ) -> u64 {
        // D5 门（全局）：无 Drill 前缀 ⇒ 每写全量求值（55 µs → 2.0 ms），直接拒。
        let prefixes = crate::hook::prefix_paths(&ir.steps);
        if prefixes.iter().all(|p| p.is_empty()) {
            eprintln!(
                "[match] register REJECTED (no Drill prefix): wt={} pattern={} tag={}",
                wt_uid, file_pattern, tag
            );
            return 0;
        }
        let wildcard = file_pattern.contains('*') || file_pattern.is_empty();
        let paths = self.expand_glob(wt_uid, file_pattern, false);
        let mut docs: Vec<u32> = Vec::new();
        for p in &paths {
            if let Ok(id) = self.resolve_path(wt_uid, p, false) {
                if !docs.contains(&id) {
                    docs.push(id);
                }
            }
        }
        let reg_id = self.match_registry.lock().register_reg(
            wt_uid,
            file_pattern,
            ir.steps.clone(),
            tag,
            wildcard,
        );
        let mut n_docs = 0usize;
        for doc_id in docs {
            let Ok(sess) = self.open_session(doc_id) else { continue };
            let snap = sess.publisher.load();
            let Some(scope) = crate::hook::compute_scope_range(&ir.steps, &snap) else {
                continue;
            };
            let lc = snap.line_len() as u32;
            let mut reg = self.match_registry.lock();
            if let Some(def_id) = reg.add_def(reg_id, doc_id, scope, lc) {
                // U6：状态初值（prefill 转正）——注册在「写前快照」上求值，
                // 该代第一次写的旧侧认领直接吃携带 nodes。
                reg.init_state(doc_id, def_id, scope, lc, &ir.steps, &snap, false);
                n_docs += 1;
            }
        }
        if n_docs > 0 || wildcard {
            self.match_active.store(true, Ordering::Relaxed);
        }
        if n_docs == 0 && !wildcard {
            // 具体地址但没有任何文档解析出 scope ⇒ 失败（与 register_hook 的 0 哨兵同律）。
            self.match_registry.lock().remove_reg(reg_id);
            eprintln!(
                "[match] register FAILED (no doc resolved scope): wt={} pattern={} tag={}",
                wt_uid, file_pattern, tag
            );
            return 0;
        }
        reg_id
    }

    /// 注销一次注册（其全部 doc-def + 模板 + wildcard/applied 项）。返回清掉的 def 数。
    pub fn remove_match(&self, reg_id: u64) -> usize {
        let n = self.match_registry.lock().remove_reg(reg_id);
        self.refresh_match_active();
        n
    }

    /// 清某文档上属于该 worktree 的归属（同 `clear_doc_hooks` 语义）。
    pub fn clear_doc_matches(&self, doc_id: u32, wt_uid: &str) {
        self.match_registry.lock().clear_doc(doc_id, wt_uid);
        self.refresh_match_active();
    }

    /// 清该 worktree 的全部归属（wt 卸载；同 `clear_worktree_hooks`）。
    pub fn clear_worktree_matches(&self, wt_uid: &str) {
        self.match_registry.lock().clear_worktree(wt_uid);
        self.refresh_match_active();
    }

    /// 注册表空否 → 开关（空 ⇒ Path/FJ 回老路径；判官可直读）。
    fn refresh_match_active(&self) {
        let active = self.match_registry.lock().is_active();
        self.match_active.store(active, Ordering::Relaxed);
    }

    /// 归属开关当前值（无注册 ⇒ false ⇒ 零额外代价路径）。
    pub fn match_is_active(&self) -> bool {
        self.match_active.load(Ordering::Relaxed)
    }

    /// 该 worktree 的归属 def 数（生命周期判官用）。
    pub fn match_count_for_worktree(&self, wt_uid: &str) -> usize {
        self.match_registry.lock().count_for_worktree(wt_uid)
    }

    // ── AST 通道 API (crdt_ast_patch_plan §2.2) ──

    /// 获取文档的 named-property AST JSON。热路径通过 DashMap 命中已有 session。
    /// root 契约：输出 root 信封
    /// (`{nt:root, mode, ch:[...]}`——mode = 拓扑分派的 JSON 表达——
    /// 虚拟根——patch path /ch/i = root.ch[i]——应用器进入 root.ch 一步)。
    pub fn get_ast(&self, doc_id: u32) -> Option<String> {
        // 上层失明——渲染/复用/建树全在渲染层
        // （to_ast_object_mode 内部经 render_roots_to）；store 不再直触 cache slot。
        // omp 读点③：get_ast——按进程模式选拓扑（与 Path/FJ 同源）。
        let mode = crate::topology::process_mode();
        // patch match attr（N5）：get_ast 与 FJ 同源——每块级节点带归属
        // （无注册 ⇒ ClaimMap 空 ⇒ 输出与实施前逐字节一致）。
        let reg = self.match_registry.clone();
        let arena = self.arena.clone();
        let reg_active = self.match_active.clone();
        let render = |snap: Arc<MdioSnapshot>| -> String {
            if !reg_active.load(Ordering::Relaxed) {
                return snap.to_ast_object_mode(mode); // 无注册：零额外代价老路径
            }
            let wt = arena.get_meta(doc_id).map(|m| m.wt_uid).unwrap_or_default();
            let defs = {
                let mut r = reg.lock();
                if r.needs_fanout(doc_id, &wt) {
                    r.ensure_doc(doc_id, &wt, &snap);
                }
                r.defs_for_doc(doc_id)
            };
            let claims = crate::r#match::ClaimMap::build(&defs, &snap);
            snap.to_ast_object_mode_with_claims(mode, &claims)
        };
        if let Some(sess) = self.sessions.get(&doc_id) {
            let snap = std::sync::Arc::clone(&*sess.value().publisher.load());
            return Some(render(snap));
        }
        let sess = self.open_session(doc_id).ok()?;
        let snap = std::sync::Arc::clone(&*sess.publisher.load());
        Some(render(snap))
    }

    /// C1 队列压缩核心(同 doc 只留最新代——旧代置 cancelled,返回新代
    /// 标志;过期回调 None 由 FJ worker 按标志交付,旧代任务从未渲染)。
    #[cfg(feature = "python")]
    pub(crate) fn compress_fj_pending(
        pending: &dashmap::DashMap<u32, Arc<AtomicBool>>,
        doc_id: u32,
    ) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        if let Some(old) = pending.insert(doc_id, flag.clone()) {
            old.store(true, Ordering::Relaxed);
        }
        flag
    }

    /// 投递 FJ 任务（FULL JSON 异步渲染——调用线程立即返回——不阻塞）。
    /// 完成回调（FJ 线程内调用）: cb(frame: Option<Vec<u8>>)——Some = 'F' 帧
    /// （渲染完成且快照未过期）; None = 快照已过期（渲染期间编辑——调用方决定：
    /// 前端刷新可丢弃（已有 P 增量流）/新订阅者可重投最新）。
    /// 返回 false = doc_id 无 session（投递失败——调用方处理）。
    #[cfg(feature = "python")]
    pub fn submit_ast_packet(&self, doc_id: u32, cb: Py<PyAny>) -> bool {
        let Some(sess) = self.sessions.get(&doc_id) else { return false; };
        // C1(#48):前置队列压缩——同 doc 只留最新代(旧代任务置 cancelled:
        // worker 回调 None 且不渲染);不触 cache。
        let cancelled = Self::compress_fj_pending(&self.fj_pending, doc_id);
        let snap = std::sync::Arc::clone(&*sess.value().publisher.load());
        // #47:FjTask.slot = 锚定(强 Arc——任务在途期间保活本代槽;
        // 收尾半钩子 B3 对它执行"最后持者取物")。
        let slot = snap
            .cache_slot
            .upgrade()
            .unwrap_or_else(|| Arc::new(crate::cache::CacheSlot::new()));
        self.fj_tx.send(FjTask { doc_id, snap, slot, reaper_tx: sess.reaper_tx(), cancelled, cb }).is_ok()
    }

    /// 构建 DN_AST 帧（Net header 13B + type byte + AST JSON payload）。
    /// Step 2-4 始终发送全量（'F'）。增量（'P'）在 Step 5 实现。
    pub fn get_ast_packet(&self, doc_id: u32) -> Option<Vec<u8>> {
        let ast_json = self.get_ast(doc_id)?;
        let mut payload = Vec::with_capacity(1 + ast_json.len());
        payload.push(b'F');
        payload.extend_from_slice(ast_json.as_bytes());
        let pkg = self.build_pkg(DN_AST, 0, doc_id, &payload);
        Some(pkg.to_vec())
    }

    /// 注册 AST 回调（供 Path Worker / AST Dispatch Thread 用）。Step 5。
    #[cfg(feature = "python")]
    pub fn register_ast_callback(&self, cb: Py<PyAny>) {
        *self.ast_callback_ref.lock().unwrap() = Some(cb);
    }

    /// Build a DN_AST frame from payload (without needing self).
    /// Used by the Path Worker thread for 'P' frame emission.
    pub fn build_ast_frame(doc_id: u32, payload: &[u8]) -> Vec<u8> {
        let header_size = crate::protocol::HEADER_SIZE;
        let mut buf = Vec::with_capacity(header_size + payload.len());
        buf.push(crate::protocol::DN_AST);
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&0u32.to_le_bytes()); // peer
        buf.extend_from_slice(&doc_id.to_le_bytes()); // doc
        buf.extend_from_slice(payload);
        buf
    }

    /// 测试用：强制关闭 doc 的 session 并用新内容重建，获得全新的 CRDT state。
    /// 不重启 gateway，不影响其他 doc。
    pub fn reset_doc(&self, wt_uid: &str, vfs_path: &str, new_content: &str) -> std::io::Result<u32> {
        let _guard = self._rt.enter();
        let doc_id = self.resolve_path(wt_uid, vfs_path, true)?;
        self.sessions.remove(&doc_id);
        let meta = self.arena.get_meta(doc_id)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Doc ID Missing"))?;
        // C3(AP §1.2):delete_wal → flush(new) → mem 写 → open_session。
        // 删除旧 WAL 必须先于 flush:若旧 WAL 无 marker,追加 marker 后 trim 不触发,
        // 旧 edits 会被 open_session 的 replay 重放到新内容上(污染)。
        let mut mirror_err: Option<anyhow::Error> = None;
        self.with_responsible_backend(wt_uid, |be| {
            if let Err(e) = be.delete_wal(&meta.storage_key) { mirror_err = Some(e); }
        });
        if let Some(e) = mirror_err { return Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())); }
        self.with_responsible_backend(wt_uid, |be| {
            if let Err(e) = be.flush(&meta.storage_key, new_content) { mirror_err = Some(e); }
        });
        if let Some(e) = mirror_err { return Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())); }
        self.vfs_op.write(&meta.storage_key, new_content.as_bytes().to_vec())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        self.open_session(doc_id)?;
        Ok(doc_id)
    }

    pub fn expand_glob(&self, wt_uid: &str, pattern: &str, include_hidden: bool) -> Vec<String> {
        let _guard = telemetry_guard!(self);
        let _guard = self._rt.enter();
        use globset::{GlobBuilder, GlobMatcher};
        let matcher: GlobMatcher = match GlobBuilder::new(pattern).literal_separator(true).empty_alternates(true).build() {
            Ok(g) => g.compile_matcher(), Err(_) => return Vec::new(),
        };
        let tree = self.tree.read();
        let mut paths = Vec::new();
        let mut visited: HashMap<String, bool> = HashMap::new();
        self.collect_vfs_paths(wt_uid, include_hidden, &tree, &mut paths, &mut visited);
        drop(tree);
        paths.retain(|p| matcher.is_match(p));
        paths
    }

    fn collect_vfs_paths(&self, wt_uid: &str, include_hidden: bool, tree: &WtTree,
        paths: &mut Vec<String>, visited: &mut HashMap<String, bool>,
    ) {
        if visited.contains_key(wt_uid) { return; }
        visited.insert(wt_uid.to_string(), true);
        let node = match tree.nodes.get(wt_uid) { Some(n) => n, None => return };
        // 绑定前缀排除仅对"绑定=跨 WT remap"的子树生效(内容归 parent 所有,不列本地):
        // 有 parent 的 WT(bfork 子 WT)才需要排除;genesis 无 parent,其自绑定
        // (MDIO_VFS_MOUNTS 子挂载=本地子目录 RO 保护,见 resolve() 同款特判)
        // 内容就在本地存储,必须照常列出,否则 ls/find 看不到该子树。
        let binding_prefixes: Vec<&str> = if node.parent.is_some() {
            node.bindings.iter().map(|b| b.my_path.as_str()).collect()
        } else { Vec::new() };
        Self::walk_local(&self.vfs_op, &node.storage_root, "", include_hidden, &binding_prefixes, paths);
        for b in &node.bindings {
            let parent_uid = match &node.parent { Some(p) => p, None => continue };
            let mut pp = Vec::new(); let mut pv: HashMap<String, bool> = HashMap::new();
            self.collect_vfs_paths(parent_uid, include_hidden, tree, &mut pp, &mut pv);
            let tp = b.target_path.trim_end_matches('/');
            let mp = b.my_path.trim_end_matches('/');
            for vfs_path in &pp {
                if path_matches(vfs_path, tp) {
                    let rest = &vfs_path[tp.len()..];
                    paths.push(if rest.is_empty() { mp.to_string() } else { format!("{}{}", mp, rest) });
                }
            }
        }
    }

    fn walk_local(op: &BlockingOperator, base: &str, rel: &str, ih: bool,
        prefixes: &[&str], results: &mut Vec<String>,
    ) {
        let dir = if rel.is_empty() { base.to_string() } else { format!("{}/{}", base.trim_end_matches('/'), rel) };
        let entries = match op.list(&format!("{}/", dir.trim_end_matches('/'))) { Ok(e) => e, Err(_) => return, };
        let dn = dir.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        for entry in entries {
            let n = entry.name(); let n = n.strip_suffix('/').unwrap_or(n);
            if n == "/" || n == dn { continue; }
            if !ih && n.starts_with('.') { continue; }
            let fr = if rel.is_empty() { n.to_string() } else { format!("{}/{}", rel, n) };
            if entry.metadata().is_dir() {
                Self::walk_local(op, base, &fr, ih, prefixes, results);
            } else {
                let vfs_path = format!("/{}", fr);
                if !prefixes.iter().any(|p| path_matches(&vfs_path, p)) { results.push(vfs_path); }
            }
        }
    }
}


// patch match attr —— P 帧序列化（C4）


/// 非空认领集 → `,"match":["tag",…]`（键序 = 对象收尾前；空集省略键）。
fn write_match_key(bw: &mut crate::incr::BufWriter<'_>, m: Option<&crate::r#match::ClaimSet>) {
    let Some(m) = m else { return };
    if m.is_empty() {
        return;
    }
    bw.write_slice(b",\"match\":[");
    for (i, tag) in m.iter().enumerate() {
        if i > 0 {
            bw.write_byte(b',');
        }
        bw.write_byte(b'"');
        bw.write_json_escaped(tag);
        bw.write_byte(b'"');
    }
    bw.write_byte(b']');
}

/// P 帧补丁序列化唯一入口（实施前逐字节等价；claims 全空 ⇒ 输出与实施前一致）。
/// `claims` 与 `patches` 等长；非空 claim 追加 `,"match":[…]`
/// （Add/Replace/ReplaceAttr 在 value 之后；Remove 无 value ⇒ 紧跟 path）。
pub(crate) fn write_patch_frame(
    pkt: &mut Vec<u8>,
    patches: &[crate::incr::Patch],
    claims: &[crate::r#match::ClaimSet],
) {
    debug_assert!(
        claims.is_empty() || claims.len() == patches.len(),
        "claims must be empty (no registration) or parallel to patches"
    );
    let has_claims = !claims.is_empty();
    let mut bw = crate::incr::BufWriter::new(pkt);
    bw.write_byte(b'[');
    for (i, p) in patches.iter().enumerate() {
        if i > 0 {
            bw.write_byte(b',');
        }
        let m = if has_claims { claims.get(i).filter(|c| !c.is_empty()) } else { None };
        match p {
            crate::incr::Patch::Add { path, value } => {
                bw.write_slice(b"{\"op\":\"add\",\"path\":\"");
                bw.write_json_escaped(path);
                bw.write_slice(b"\",\"value\":");
                bw.write_slice(value.as_bytes());
                write_match_key(&mut bw, m);
                bw.write_byte(b'}');
            }
            crate::incr::Patch::Remove { path } => {
                bw.write_slice(b"{\"op\":\"remove\",\"path\":\"");
                bw.write_json_escaped(path);
                bw.write_slice(b"\"");
                write_match_key(&mut bw, m);
                bw.write_byte(b'}');
            }
            crate::incr::Patch::Replace { path, value } => {
                bw.write_slice(b"{\"op\":\"replace\",\"path\":\"");
                bw.write_json_escaped(path);
                bw.write_slice(b"\",\"value\":");
                bw.write_slice(value.as_bytes());
                write_match_key(&mut bw, m);
                bw.write_byte(b'}');
            }
            crate::incr::Patch::ReplaceAttr { path, attr, value } => {
                // WC-3 线路统一：字段写 = 标准 6902 路径形态
                // （patch_6902_plan §2.2）——动词用 replace（"字段键
                // 恒在"保证目标位置合法），"改哪个字段"由 path 末段
                // 承载。自定义动词 replace_attr 不再上线；消费端
                // 无需再做 path+attr 折叠。value 仍是已序列化的裸
                // JSON（数组值可直接承载，见 §7.2 的编码契约）。
                bw.write_slice(b"{\"op\":\"replace\",\"path\":\"");
                bw.write_json_escaped(path);
                bw.write_byte(b'/');
                bw.write_json_escaped(attr);
                bw.write_slice(b"\",\"value\":");
                bw.write_slice(value.as_bytes());
                write_match_key(&mut bw, m);
                bw.write_byte(b'}');
            }
        }
    }
    bw.write_byte(b']');
    bw.commit();
}


// flush_one — simplified (calls backend.flush trait method)


fn flush_one(
    doc_id: u32, session: &Arc<MdioSession>,
    arena: &Arc<FileArena>,
    vfs_op: &Arc<BlockingOperator>,
    tree: &Arc<RwLock<WtTree>>, backends: &Arc<RwLock<HashMap<String, Box<dyn Backend>>>>,
) {
    let meta = match arena.get_meta(doc_id) { Some(m) => m, None => return };
    let sk = meta.storage_key.clone();
    let content = session.publisher.load_full().to_string();
    let _ = vfs_op.write(&sk, content.clone().into_bytes());
    let t = tree.read(); let b = backends.read();
    if let Some(backend) = t.nodes.get(&meta.wt_uid)
        .and_then(|n| n.responsible_project(&t))
        .and_then(|proj| b.get(&proj.uid))
    {
        let _ = backend.flush(&sk, &content);
    }
    drop(t); drop(b);
    session.dirty.store(false, Ordering::Relaxed);
}



// ProjectStore 生命周期终结器

impl Drop for ProjectStore {
    fn drop(&mut self) {
        self.flush_shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.flush_thread.take() {
            let _ = handle.join();
        }
    }
}


// Tests

#[cfg(all(test, feature = "python"))]
mod c1_fj_compress_tests {
    use super::*;

    /// C1 队列压缩判官(§9 断言形态硬要求③:可达场景 = 真实生产机制——
    /// submit_ast_packet 的压缩核心):同 doc 连续投递 → 旧代标志置
    /// cancelled(worker 据此回调 None 且不渲染)、新代标志存活;
    /// 跨 doc 互不影响。声明边界(项目先例 = kernel_block_starts 对
    /// Edit 返回 None 的形态):FJ worker 消费分支(store.rs
    /// 597-608,5 行:cancelled → 回调 None + reaper 移交 + 不渲染)无
    /// 自动化端到端断言,正确性由亲读保证——分支为胶水,压缩语义
    /// (本判官断言面)与 reaper 移交(session b3 家族断言面)各自已被
    /// 独立覆盖,不为此加 python 测试基建。
    #[test]
    fn c1_compress_marks_old_cancelled_keeps_latest() {
        let pending: dashmap::DashMap<u32, Arc<AtomicBool>> = dashmap::DashMap::new();
        let f1 = super::ProjectStore::compress_fj_pending(&pending, 7);
        assert!(!f1.load(Ordering::Relaxed), "first submission alive");
        let f2 = super::ProjectStore::compress_fj_pending(&pending, 7);
        assert!(f1.load(Ordering::Relaxed), "old generation cancelled on resubmit");
        assert!(!f2.load(Ordering::Relaxed), "latest generation alive");
        // 同 doc 第三次:只取消最近一代。
        let f3 = super::ProjectStore::compress_fj_pending(&pending, 7);
        assert!(f2.load(Ordering::Relaxed), "previous generation cancelled");
        assert!(!f3.load(Ordering::Relaxed), "latest alive");
        // 跨 doc 互不影响。
        let f4 = super::ProjectStore::compress_fj_pending(&pending, 8);
        assert!(!f4.load(Ordering::Relaxed));
        assert!(!f3.load(Ordering::Relaxed), "other-doc submit leaves doc 7 latest alive");
    }
}

#[cfg(test)]
mod tests {
    /// S0 禁删清单 keep-alive 出口断言（删被保资产即编译红）。
    mod keepalive_tests {
        include!("../tests/unit/keepalive.rs");
    }
    mod arena_tests {
        include!("../tests/unit/arena.rs");
    }
    /// omp 判官：进程级模式（切换三值 + F/P 分派）——process_mode.rs（S2/S3 重写）。
    mod process_mode_tests {
        include!("../tests/unit/process_mode.rs");
    }
    #[cfg(feature = "persistence")]
    mod persist_tests {
        include!("../tests/unit/persist.rs");
    }
    /// patch match attr：P 帧序列化判官（spec S3/C4）。
    mod match_frame_tests {
        include!("../tests/unit/match_frame.rs");
    }
    /// patch match attr：注册/注销生命周期判官（spec S1）。
    mod match_lifecycle_tests {
        include!("../tests/unit/match_lifecycle.rs");
    }
    /// patch match attr：F 帧归属判官（spec S6/N5）。
    mod match_f_tests {
        include!("../tests/unit/match_f.rs");
    }
    /// 血统线格式判官：上行/下行载荷的 [lineage][bincode] 约定。
    mod lineage_wire_tests {
        include!("../tests/unit/lineage_wire.rs");
    }
    /// 注册入口的家族契约校验（S1）：票据/服务家族/未知取值一律拒绝，`fs` 放行。
    mod register_contract_tests {
        include!("../tests/unit/register_contract.rs");
    }
    mod handoff_equivalence_tests {
        include!("../tests/unit/handoff_equivalence.rs");
    }
}
