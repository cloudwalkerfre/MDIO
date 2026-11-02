//! MDIO Backend — L2 (virtual key) → L1 (storage locator) translation.
//!
//! VFS 层通过 BackendConfig 的三个方法完成路径翻译，
//! 不关心底层是 Fs、Memory、Redis 还是 S3。
//!
//! ```text
//! VFS key ("docs/a.md") ──→ storage_key(storage_root, key) ──→ L1 locator
//!                                          │
//!                                          ├── Fs:     "{storage_root}/{key}"
//!                                          ├── Memory: "{storage_root}/{key}"
//!                                          └── Redis:  "{storage_root}:{key}"
//! ```
//!
//! external_root：offload 的回邮地址。load 时从那里把数据吸进 Operator，
//! 记下这个地址，offload 时按它写回去。VFS 运行时全程不碰、不感知。

use anyhow::{anyhow, Result};
use opendal::blocking::Operator as BlockingOperator;
use opendal::services::{Fs, Memory};
use opendal::Operator;
#[cfg(feature = "persistence")]
use ahash::AHashMap as HashMap;
#[cfg(feature = "persistence")]
use parking_lot::Mutex;
#[cfg(feature = "persistence")]
use dashmap::DashMap;
#[cfg(feature = "persistence")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::path::Path;
use crossbeam::queue::SegQueue;
use sha2::{Sha256, Digest};
use serde::{Serialize, Deserialize};
use crate::store::bincode_cfg;


// Scheme


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scheme {
    Fs,
    Memory,
    Remote,
}

/// 持久化运行时开关(MDIO_PERSIST)——唯一 truth source。
/// 缺省 false(.env 未设置/未读到 = 不落盘,测试场景天然不污染);
/// 显式值接受 "1"/"true"(大小写不敏感)。
fn persist_enabled() -> bool {
    std::env::var("MDIO_PERSIST")
        .map(|v| v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}


// BackendConfig


#[derive(Debug, Clone)]
pub struct BackendConfig {
    pub scheme: Scheme,
    /// 定期 flush + 截断 WAL 的间隔（秒）。仅 persistent=true 时生效。
    pub checkpoint_interval_secs: u64,
    /// 仅远程后端有用，checkpoint 窗口内合并写入（毫秒）。预留字段。
    pub coalesce_ms: u64,
    /// false → 不写 WAL，不启 worker，纯内存运行。
    /// 默认由 `cfg!(feature = "persistence")` 决定。
    pub persistent: bool,
}

impl BackendConfig {
    // ── L2 → L1 翻译 ──

    /// L2 key → L1 存储定位符。Backend 自己决定拼法。
    ///
    /// Fs:      `format!("{}/{}", storage_root.trim_end_matches('/'), key.trim_start_matches('/'))`
    /// Memory:  `format!("{}/{}", storage_root, key)`
    pub fn storage_key(&self, storage_root: &str, key: &str) -> String {
        match self.scheme {
            Scheme::Fs => {
                format!(
                    "{}/{}",
                    storage_root.trim_end_matches('/'),
                    key.trim_start_matches('/')
                )
            }
            Scheme::Memory => {
                format!("{}/{}", storage_root, key)
            }
            Scheme::Remote => {
                format!("remote://{}/{}", storage_root, key)
            }
        }
    }

    /// child_root 是否在 parent_root 范围内（derive_child 安全边界）。
    ///
    /// Fs:      `is_subpath(parent, child)`
    /// Memory:  永远 true
    pub fn contains(&self, parent_storage_root: &str, child_storage_root: &str) -> bool {
        match self.scheme {
            Scheme::Fs => {
                let p = parent_storage_root.trim_end_matches('/');
                let c = child_storage_root.trim_end_matches('/');
                if p == c {
                    return true;
                }
                if c.starts_with(p) {
                    if c.as_bytes().get(p.len()) == Some(&b'/') {
                        return true;
                    }
                }
                false
            }
            Scheme::Memory => true,
            Scheme::Remote => true,
        }
    }

    // ── Operator 创建 ──

    /// 创建 BlockingOperator。从 vfs.rs 迁入，按 Scheme 分派。
    pub fn create_operator(&self, storage_root: &str) -> Result<BlockingOperator> {
        let async_op = match self.scheme {
            Scheme::Memory | Scheme::Remote => {
                Operator::new(Memory::default())?.finish()
            }
            Scheme::Fs => {
                let builder = Fs::default().root(storage_root);
                Operator::new(builder)?.finish()
            }
        };
        Ok(BlockingOperator::new(async_op)?)
    }

    // ── 默认配置 ──

    /// 内置默认表。persistent 由 feature flag 控制。
    #[allow(unused_variables)]
    pub fn defaults() -> Vec<BackendConfig> {
        let persistent = cfg!(feature = "persistence");
        vec![
            BackendConfig {
                scheme: Scheme::Fs,
                checkpoint_interval_secs: 300,
                coalesce_ms: 0,
                persistent,
            },
            BackendConfig {
                scheme: Scheme::Memory,
                checkpoint_interval_secs: 0,
                coalesce_ms: 0,
                persistent: false,
            },
        ]
    }

    /// 按 Scheme 查配置。
    pub fn resolve(scheme: Scheme) -> BackendConfig {
        let persistent = persist_enabled();
        match scheme {
            Scheme::Fs => BackendConfig {
                scheme: Scheme::Fs,
                checkpoint_interval_secs: std::env::var("MDIO_CHECKPOINT_FS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(300),
                coalesce_ms: 0,
                persistent,
            },
            Scheme::Memory => BackendConfig {
                scheme: Scheme::Memory,
                checkpoint_interval_secs: 0,
                coalesce_ms: 0,
                persistent: false,
            },
            Scheme::Remote => BackendConfig {
                scheme: Scheme::Remote,
                checkpoint_interval_secs: 0,
                coalesce_ms: 0,
                persistent: false,
            },
        }
    }

    /// 从 storage_root 字符串检测 Scheme。
    pub fn detect_scheme(src: &str) -> Scheme {
        if src == "/mem" || src.starts_with("/mem/") {
            Scheme::Memory
        } else if src == "remote" || src.starts_with("remote://") {
            Scheme::Remote
        } else {
            Scheme::Fs
        }
    }
}


// Tests


#[cfg(test)]
mod tests {
    include!("../tests/unit/backend_tests.rs");
    mod wal_tests {
        use crate::store::bincode_cfg;
        include!("../tests/unit/wal.rs");
    }
}


fn fs_list_recursive(op: &BlockingOperator, path: &str) -> Result<Vec<opendal::Entry>> {
    use opendal::options::ListOptions;
    let opts = ListOptions { recursive: true, ..Default::default() };
    op.list_options(path, opts).map_err(|e| anyhow!("List Err: {}", e))
}

/// 引擎只管理 .md 文件(含隐藏 .md,如 .notes.md):load 只泵入、offload 只回收 .md。
fn is_md_file(path: &str) -> bool { path.ends_with(".md") }


// WAL Types


pub(crate) const WAL_MAGIC: u32 = 0x4D44494F; // "MDIO"

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EditRecord {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum WalEntry {
    Edit(EditRecord),
    FlushMarker,
}

fn sha256_hex(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}



// Backend trait


/// 持久化 Backend 抽象(契约层,对一切未来 Backend 生效)。每个 project WT 持有一个 Backend 实例。
/// 2 个必需方法(load/offload)+ 6 个带默认实现(replay_wal/flush/delete_wal/wal_queue/ensure_dir/remove)。
/// WAL 细节(append/trim/flush_marker/create_wal_operator)全部是 FsBackend 私有。
///
/// **新增 Backend 的实现清单(契约)**:
///   - Fs 类(有本地/远程文件语义):全量实现 load/offload/flush/ensure_dir/remove(可含 delete_wal/wal_queue);
///   - 对象存储类:load/offload/flush/ensure_dir/remove 必须实现;WAL 可选;默认实现=“该 BE 不提供”。
/// 注:load 的 external_root 参数当前实现未用(FsBackend::new 已用它建 fs_op)——签名保留
/// 是为 trait 对称(offload 用同签名)+ 未来多后端(Redis/S3)需要。
pub(crate) trait Backend: Send + Sync {
    fn load(&self, vfs_op: &BlockingOperator, external_root: &str, storage_root: &str) -> Result<()>;
    fn offload(&self, vfs_op: &BlockingOperator, external_root: &str, storage_root: &str) -> Result<()>;
    fn replay_wal(&self, storage_key: &str, vfs_text: &str) -> Option<String> { None }
    fn flush(&self, storage_key: &str, content: &str) -> Result<()> { Ok(()) }
    fn delete_wal(&self, storage_key: &str) -> Result<()> { Ok(()) }
    fn wal_queue(&self) -> Option<Arc<SegQueue<(String, Vec<WalEntry>)>>> { None }
    /// 持久化目录确保(递归 mkdir -p 语义)。默认空实现 = 该 BE 不提供目录镜像。
    fn ensure_dir(&self, storage_key: &str) -> Result<()> { Ok(()) }
    /// 持久化删除:文件 → 删单个文件;recursive=true 且为目录 → 递归删除整树。
    /// 默认空实现 = 该 BE 不提供删除镜像。
    fn remove(&self, storage_key: &str, recursive: bool) -> Result<()> { Ok(()) }
    /// 磁盘镜像同步(以磁盘为准):清理 vfs_op 中"磁盘已消失"的本 WT 镜像,再重泵磁盘现存文件。
    /// 与 offload 对称——offload 以 mem 为准对账到磁盘,reload 以磁盘为准对账到 mem。
    /// keys = store 层传入的该 WT 当前 mem storage_keys(精确归属,含 owner 过滤后的)。
    /// 返回被清理(磁盘已消失)的 storage_key 列表。默认实现 = 该 BE 不提供。
    fn reload(&self, vfs_op: &BlockingOperator, external_root: &str, storage_root: &str, keys: &[String]) -> Result<Vec<String>> {
        Err(anyhow!("reload not supported by this backend"))
    }
}

/// WT 信息 — 供 Python bootstrap 遍历所有 WT。
#[derive(Debug, Clone)]
pub(crate) struct WorktreeInfo {
    pub uid: String,
    pub name: String,
    pub storage_root: String,
    pub external_root: Option<String>,
    /// 三根分层布局（None = 旧扁平行）
    pub doc_subdir: Option<String>,
    pub wal_rel: Option<String>,
}

pub(crate) struct FsBackend {
    fs_op: BlockingOperator,
    /// 运行态根（WAL 用）：三根分层时 = external_root（**文档根之外**）；否则与 fs_op 同一根。
    wal_op: BlockingOperator,
    storage_root: String,
    /// WAL 目录（相对 external_root）：genesis 用 .wal，project WT 用 .mds/.wal。
    wal_dir: String,
    /// 运行时持久化开关(MDIO_PERSIST)——构造时一次判定。
    persistent: bool,
    #[cfg(feature = "persistence")]
    wal_queue: Arc<SegQueue<(String, Vec<WalEntry>)>>,
    #[cfg(feature = "persistence")]
    wal_mutexes: Arc<DashMap<String, Mutex<()>>>,
    #[cfg(feature = "persistence")]
    wal_shutdown: Arc<AtomicBool>,
    #[cfg(feature = "persistence")]
    wal_thread: Option<std::thread::JoinHandle<()>>,
}

impl FsBackend {
    /// 三根分层：**文档根** = `external_root/<doc_subdir>`；**运行态根** = external_root；WAL 目录 = `wal_rel`。
    /// 两个名字**由 host 给**（core 不认识 wt/plugin/.wal 这些层级名）；**没有默认值、没有兜底**：
    /// 缺名字即报错，绝不静默退回"文档就在 external_root 下"。
    pub fn new_with_docs(
        external_root: &str,
        storage_root: &str,
        doc_subdir: Option<&str>,
        wal_rel: Option<&str>,
    ) -> Result<Self> {
        let doc_subdir = doc_subdir.map(|d| d.trim_matches('/')).filter(|d| !d.is_empty())
            .ok_or_else(|| anyhow!("layout: doc_subdir is required (no flat fallback)"))?;
        let wal_rel = wal_rel.map(|w| w.trim_matches('/')).filter(|w| !w.is_empty())
            .ok_or_else(|| anyhow!("layout: wal_rel is required (no flat fallback)"))?;
        let doc_root = format!("{}/{}", external_root.trim_end_matches('/'), doc_subdir);
        let fs_op = BackendConfig::resolve(Scheme::Fs).create_operator(&doc_root)?;
        // WAL 必须在**文档根之外**：否则泵入 VFS 要按名字跳过它，offload/拷贝还会带上运行态。
        let wal_op = BackendConfig::resolve(Scheme::Fs).create_operator(external_root)?;
        let persistent = persist_enabled();
        let wal_dir = wal_rel.to_string();
        #[cfg(feature = "persistence")]
        {
            let _ = wal_op.create_dir(&format!("/{}/", wal_dir));
            let wal_queue = Arc::new(SegQueue::new());
            let wal_mutexes = Arc::new(DashMap::new());
            let wal_shutdown = Arc::new(AtomicBool::new(false));
            let wal_thread = if persistent {
                let wq = wal_queue.clone();
                let wm = wal_mutexes.clone();
                let ws = wal_shutdown.clone();
                let op = wal_op.clone();
                let wd = wal_dir.clone();
                Some(std::thread::spawn(move || {
                    Self::wal_append_worker(wq, wm, ws, op, wd);
                }))
            } else {
                None // MDIO_PERSIST 未开启:不建 WAL worker(队列字段保留恒建,空转无害)
            };
            Ok(Self { fs_op, wal_op, storage_root: storage_root.to_string(), wal_dir, persistent, wal_queue, wal_mutexes, wal_shutdown, wal_thread })
        }
        #[cfg(not(feature = "persistence"))]
        {
            Ok(Self { fs_op, wal_op, storage_root: storage_root.to_string(), wal_dir, persistent })
        }
    }

    /// genesis / 测试兼容：静态 WAL 路径仍指根下 .wal。
    pub fn wal_path(storage_key: &str) -> String {
        format!(".wal/{}.log", sha256_hex(storage_key))
    }

    fn wal_path_for(&self, storage_key: &str) -> String {
        format!("{}/{}.log", self.wal_dir, sha256_hex(storage_key))
    }

    // ── WAL 私有方法 ──

/// 单条 WalEntry 的帧编码(len + magic + crc + payload)。
#[cfg(feature = "persistence")]
fn encode_wal_entry(entry: &WalEntry) -> Vec<u8> {
    let payload = bincode::serde::encode_to_vec(entry, bincode_cfg()).unwrap_or_default();
    let len = payload.len() as u32;
    let mut crc = crc32fast::Hasher::new(); crc.update(&payload); let crc = crc.finalize();
    let mut buf = Vec::with_capacity(12 + payload.len());
    buf.extend_from_slice(&len.to_le_bytes()); buf.extend_from_slice(&WAL_MAGIC.to_le_bytes());
    buf.extend_from_slice(&crc.to_le_bytes()); buf.extend_from_slice(&payload);
    buf
}

/// 读-合并-写:旧 WAL + 本批 entries → 原子替换(tmp + rename)。
/// 覆盖写会抹掉已落盘的 edits 与 FlushMarker(双应用根因),合并写保住它们。
#[cfg(feature = "persistence")]
fn wal_merge_append(op: &BlockingOperator, wal_key: &str, entries: &[WalEntry]) -> Result<()> {
    let existing = op.read(wal_key).unwrap_or_default();
    let mut merged = existing.to_vec();
    for e in entries { merged.extend_from_slice(&Self::encode_wal_entry(e)); }
    let tk = format!("{}.tmp", wal_key);
    op.write(&tk, merged)?; op.rename(&tk, wal_key)?;
    Ok(())
}

/// **结算即清**——规则：只清已结算的；清这个动作就发生在结算之后。
///
/// 批内出现 `FlushMarker` 时由 worker 调用：最后一个 `FlushMarker` 之前（含它自己）的帧
/// 都已随 `flush` 写进 base，直接丢弃；**其后若没有 edit ⇒ 整个 .log 删除**（这正是历史
/// 积攒的那批「尾帧是 marker」文件，实测 349 个里 340 个属于此类）；其后若还有 edit
/// （下一代的未结算改动）⇒ 只留那一段，且**不再带 marker**——它的 base 已更新，replay
/// 必须从新 base 起全量应用这些 edit（`replay_wal_impl`：无 marker ⇒ 全量应用）。
#[cfg(feature = "persistence")]
fn wal_settle_drop(op: &BlockingOperator, wal_key: &str) -> Result<()> {
    let data = match op.read(wal_key) { Ok(d) => d.to_vec(), Err(_) => return Ok(()), };
    if data.is_empty() { return Ok(()); }
    let mut cut: Option<usize> = None;
    let mut p = 0usize;
    while p + 12 <= data.len() {
        let l = u32::from_le_bytes([data[p],data[p+1],data[p+2],data[p+3]]) as usize;
        if u32::from_le_bytes([data[p+4],data[p+5],data[p+6],data[p+7]]) != WAL_MAGIC { break; }
        let ee = p + 12 + l; if ee > data.len() { break; }
        if let Ok((d,_)) = bincode::serde::decode_from_slice::<WalEntry,_>(&data[p+12..ee], bincode_cfg()) {
            if matches!(d, WalEntry::FlushMarker) { cut = Some(ee); }
        }
        p = ee;
    }
    // 无结算点：一律不动（未结算的 edit 是崩溃恢复的唯一副本）。
    let Some(cut) = cut else { return Ok(()) };
    let tail = data[cut..].to_vec();
    if tail.is_empty() {
        op.delete(wal_key).map_err(|e| anyhow!("wal_settle_drop: {}", e))
    } else {
        let tk = format!("{}.tmp", wal_key);
        op.write(&tk, tail)?; op.rename(&tk, wal_key)?;
        Ok(())
    }
}

    fn replay_wal_impl(&self, storage_key: &str, base_text: &str) -> Option<String> {
        use crate::engine::MdioEngine;
        let wk = self.wal_path_for(storage_key); let wb = self.wal_op.read(&wk).ok()?.to_vec();
        if wb.is_empty() { return Some(base_text.to_string()); }
        let has_fm = { let mut p=0; let mut f=false; while p+12<=wb.len() { let l=u32::from_le_bytes([wb[p],wb[p+1],wb[p+2],wb[p+3]]) as usize; if u32::from_le_bytes([wb[p+4],wb[p+5],wb[p+6],wb[p+7]])!=WAL_MAGIC{break} let ee=p+12+l; if ee>wb.len(){break} if let Ok((e,_))=bincode::serde::decode_from_slice::<WalEntry,_>(&wb[p+12..ee],bincode_cfg()){if matches!(e,WalEntry::FlushMarker){f=true;break}} p=ee; } f };
        let mut e=MdioEngine::new(base_text); let mut p=0; let mut sf=false;
        while p+12<=wb.len() { let l=u32::from_le_bytes([wb[p],wb[p+1],wb[p+2],wb[p+3]]) as usize; if u32::from_le_bytes([wb[p+4],wb[p+5],wb[p+6],wb[p+7]])!=WAL_MAGIC{break} let ee=p+12+l; if ee>wb.len(){break} if let Ok((entry,_))=bincode::serde::decode_from_slice::<WalEntry,_>(&wb[p+12..ee],bincode_cfg()){ match entry { WalEntry::FlushMarker=>{sf=true} WalEntry::Edit(rec)=>{if !has_fm||sf{let s=rec.start.min(e.byte_len());let en=rec.end.min(e.byte_len()).max(s);if s<en{e.edit(s,en,"");} if !rec.text.is_empty(){e.edit(s,s,&rec.text);}}}} } p=ee; }
        Some(e.rope_clone().to_string())
    }

    /// WAL append worker — FsBackend 内部线程，drain 自己的 wal_queue。
    /// FlushMarker 与 edits 共用队列:结算顺序由队列天然保证(方案 d),
    /// 不再依赖锁的先后;append 为读-合并-写(保住旧 edits 与 marker)。
    #[cfg(feature = "persistence")]
    fn wal_append_worker(
        queue: Arc<SegQueue<(String, Vec<WalEntry>)>>,
        mutexes: Arc<DashMap<String, Mutex<()>>>,
        shutdown: Arc<AtomicBool>,
        op: BlockingOperator,
        wal_dir: String,
    ) {
        let interval_ms: u64 = std::env::var("MDIO_WAL_INTERVAL_MS").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
            if shutdown.load(Ordering::Relaxed) && queue.is_empty() { break; }
            let mut batches: HashMap<String, Vec<WalEntry>> = HashMap::new();
            while let Some((sk, entries)) = queue.pop() { batches.entry(sk).or_default().extend(entries); }
            for (sk, entries) in &batches {
                if entries.is_empty() { continue; }
                let wk = format!("{}/{}.log", wal_dir, sha256_hex(sk));
                let lock = mutexes.entry(sk.clone()).or_insert_with(|| Mutex::new(()));
                let _g = lock.lock();
                let _ = Self::wal_merge_append(&op, &wk, entries);
                // 本批含结算点 ⇒ **结算之后立刻清已结算的**（清完若其后无 edit 则整个 .log 删除）
                if entries.iter().any(|e| matches!(e, WalEntry::FlushMarker)) {
                    let _ = Self::wal_settle_drop(&op, &wk);
                }
            }
        }
    }
}

#[cfg(feature = "persistence")]
impl Drop for FsBackend {
    fn drop(&mut self) {
        self.wal_shutdown.store(true, Ordering::Relaxed);
        if let Some(h) = self.wal_thread.take() { let _ = h.join(); }
    }
}

/// **L7 的 Memory scheme 后端**（`Scheme::Memory`）——给 `/mem` 这类**无 L3 外部根**的 store 用。
///
/// 为什么要它：`backends` 必须为每个 wt 放一个 `Backend`，而若只有 `FsBackend` 一个实现，
/// 就只能「造一个指向 `/` 的 `FsBackend`」——那会让 Fs 算子去碰**真实根**
/// （`load` 列 `/`、构造时 `create_dir("/.wal/")`），既违背「`/mem` 是纯内存」，
/// 也在布局名必填之后**直接构造失败**（`ProjectStore("/mem")` 与依赖它的入口一起挂）。
///
/// 范式对齐（不新发明轮子）：
///   · 算子仍由既有 `BackendConfig::resolve(Scheme::Memory).create_operator(root)` 产出
///     ——与 `ProjectStore.vfs_op` 同一个轮子；
///   · 只补 `Backend` trait 的空实现，其余（`replay_wal`/`flush`/`delete_wal`/`wal_queue`/
///     `ensure_dir`/`remove`）继承默认值；`reload` 另给 Ok(空)——「磁盘→VFS 重同步」对无盘后端
///     无意义，不该报默认的 Err。
///
/// 语义：`/mem` 的文档面 I/O 全部由 `ProjectStore.vfs_op`（全局 MemoryLayer）承担；本后端**不持有
/// 第二份内存文档面**（两个 Memory 算子是各自独立的命名空间，写进去没人读）⇒ 落盘面为空。
pub(crate) struct MemoryBackend {
    cfg: BackendConfig,
    /// 由 `cfg.create_operator(root)` 产出的 memlayer 算子：维持「后端 = config + 算子」的既有范式
    /// （今天不参与读写，故前缀下划线）。
    _op: BlockingOperator,
}

impl MemoryBackend {
    pub(crate) fn new(root: &str) -> Result<Self> {
        let cfg = BackendConfig::resolve(Scheme::Memory);
        let _op = cfg.create_operator(root)?;
        Ok(Self { cfg, _op })
    }

    /// 配置面（测试/诊断：断言 scheme 与 persistent 来自既有表，而不是各处硬写）。
    #[allow(dead_code)]
    pub(crate) fn cfg(&self) -> &BackendConfig { &self.cfg }
}

impl Backend for MemoryBackend {
    /// 无 L3 外部根：没有磁盘可载入（文档面在 `ProjectStore.vfs_op` 里）。
    fn load(&self, _vfs_op: &BlockingOperator, _external_root: &str, _storage_root: &str) -> Result<()> { Ok(()) }
    /// 同上：没有磁盘可卸载。
    fn offload(&self, _vfs_op: &BlockingOperator, _external_root: &str, _storage_root: &str) -> Result<()> { Ok(()) }
    /// 「磁盘→VFS 重同步」对无盘后端无意义：返回「没有 key 被清理」。
    fn reload(&self, _vfs_op: &BlockingOperator, _external_root: &str, _storage_root: &str, _keys: &[String]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

impl Backend for FsBackend {
    fn load(&self, vfs_op: &BlockingOperator, external_root: &str, storage_root: &str) -> Result<()> {
        let entries = fs_list_recursive(&self.fs_op, "/").unwrap_or_default();
        for entry in entries { let path = entry.path();
            // 引擎私有区(WAL)不泵入 VFS
            if path == ".wal" || path.starts_with(".wal/")
                || path == ".mds/.wal" || path.starts_with(".mds/.wal/") { continue; }
            // 边界防御:FS list 含根条目(path="/")——vd 构造出 "//"——
            // create_dir("//") 在 memory 后端写入空 key——后续 list 迭代
            // 根目录恒存在——跳过。
            if path.is_empty() || path == "/" { continue; }
            if entry.metadata().is_dir() { let vd = format!("{}/{}/", storage_root.trim_end_matches('/'), path.trim_end_matches('/')); let _ = vfs_op.create_dir(&vd); } else if is_md_file(&path) { if let Ok(data) = self.fs_op.read(&format!("/{}", path)) { let key = format!("{}/{}", storage_root.trim_end_matches('/'), path); let _ = vfs_op.write(&key, data); } } }
        Ok(())
    }

    fn offload(&self, vfs_op: &BlockingOperator, external_root: &str, storage_root: &str) -> Result<()> {
        let sr = storage_root.trim_end_matches('/');
        let mut files: Vec<String> = Vec::new();
        offload_collect(vfs_op, sr, "", &mut files);
        for rel in &files {
            let storage_key = if rel.is_empty() { sr.to_string() } else { format!("{}/{}", sr, rel) };
            if let Ok(data) = vfs_op.read(&storage_key) {
                if let Some(parent) = Path::new(rel).parent() {
                    if parent != Path::new("") { self.fs_op.create_dir(&format!("/{}", parent.display())).ok(); }
                }
                self.fs_op.write(&format!("/{}", rel), data)?;
            }
        }
        let mut disk_files: Vec<String> = Vec::new();
        offload_collect(&self.fs_op, "", "", &mut disk_files);
        for rel in &disk_files {
            // 引擎只回收 .md:非 .md(含 .wal 日志)一律不动,防止误删
            if !is_md_file(rel) { continue; }
            let storage_key = if rel.is_empty() { sr.to_string() } else { format!("{}/{}", sr, rel) };
            if !vfs_op.exists(&storage_key).unwrap_or(false) {
                self.fs_op.delete(&format!("/{}", rel)).ok();
            }
        }
        Ok(())
    }

    fn replay_wal(&self, storage_key: &str, vfs_text: &str) -> Option<String> {
        self.replay_wal_impl(storage_key, vfs_text)
    }

    fn flush(&self, storage_key: &str, content: &str) -> Result<()> {
        #[cfg(feature = "persistence")]
        {
            if !self.persistent { return Ok(()); } // 运行时开关(MDIO_PERSIST)未开启:纯空转
            // strip 自己的 storage_root → 磁盘相对路径
            let rel = storage_key.strip_prefix(self.storage_root.as_str()).unwrap_or(storage_key);
            let rel = rel.trim_matches('/');
            // 确保父目录存在(与 offload 同款):系统区 /.mds/* 首次写盘时父目录可能未建,
            // opendal Fs 写不自动建父目录，缺失即静默失败，须显式 ensure_dir。
            if let Some(parent) = Path::new(rel).parent() {
                if parent != Path::new("") { let _ = self.fs_op.create_dir(&format!("/{}/", parent.display())); }
            }
            let _ = self.fs_op.write(&format!("/{}", rel), content.as_bytes().to_vec());
            // 结算点入队:与 edits 共用队列,顺序由队列保证(方案 d)。
            // base 数据文件已同步写盘且先于结算,崩溃矩阵成立。
            self.wal_queue.push((storage_key.to_string(), vec![WalEntry::FlushMarker]));
        }
        #[cfg(not(feature = "persistence"))]
        { let _ = (storage_key, content); }
        Ok(())
    }

    fn delete_wal(&self, storage_key: &str) -> Result<()> {
        let wk = self.wal_path_for(storage_key);
        self.wal_op.delete(&wk).map_err(|e| anyhow!("delete_wal: {}", e))
    }

    /// 持久化目录确保（mkdir -p 语义——opendal create_dir 恒递归）。
    /// 注意：opendal create_dir 的 validate_path(DIR) 要求路径以 `/` 结尾。
    fn ensure_dir(&self, storage_key: &str) -> Result<()> {
        if !self.persistent { return Ok(()); }
        let rel = storage_key.strip_prefix(self.storage_root.as_str()).unwrap_or(storage_key);
        let rel = rel.trim_matches('/');
        if rel.is_empty() { return Ok(()); }
        self.fs_op.create_dir(&format!("/{}/", rel))
            .map_err(|e| anyhow!("ensure_dir: {}", e))
    }

    /// 持久化删除(纯 OpenDAL,失败可见)。
    /// 文件 → delete(NotFound→Ok 幂等);目录(recursive)→ 复用 fs_list_recursive 收集全树,
    /// 按"文件先、目录后(先深后浅)"显式删除——与 FlatLister 后序产出同构,保证目录删时已空;
    /// 不直接使用 opendal remove_all(其 delete_try_stream 尽力而为、失败吞掉,静默残留=删除复活)。
    fn remove(&self, storage_key: &str, recursive: bool) -> Result<()> {
        if !self.persistent { return Ok(()); }
        let rel = storage_key.strip_prefix(self.storage_root.as_str()).unwrap_or(storage_key);
        let rel = rel.trim_matches('/');
        if rel.is_empty() { return Ok(()); } // 防御:根目录不可删
        if !recursive {
            let p = format!("/{}", rel);
            let r = self.fs_op.delete(&p).map_err(|e| anyhow!("remove file: {}", e))?;
            // 文档全生命周期清除:连带其 WAL(NotFound → Ok 幂等)
            let _ = self.delete_wal(storage_key);
            return Ok(r);
        }
        // 递归删除整树:先文件、后目录(先深后浅)
        let base = rel.trim_end_matches('/');
        let entries: Vec<opendal::Entry> = fs_list_recursive(&self.fs_op, &format!("/{}", base)).unwrap_or_default();
        let mut files: Vec<String> = Vec::new();
        let mut dirs: Vec<String> = Vec::new();
        for e in &entries {
            let p = e.path().trim_end_matches('/').to_string();
            if e.metadata().is_dir() { dirs.push(p); }
            else if is_md_file(&p) { files.push(p); }
        }
        for f in &files {
            self.fs_op.delete(&format!("/{}", f)).map_err(|e| anyhow!("remove file: {}", e))?;
            let sk = format!("{}{}", self.storage_root, f);
            let _ = self.delete_wal(&sk);
        }
        dirs.sort_by_key(|d| std::cmp::Reverse(d.len())); // 先深后浅
        for d in &dirs {
            if let Err(e) = self.fs_op.delete(&format!("/{}", d)) {
                eprintln!("⚠️ [remove] dir kept (not empty): {}: {:#}", d, e);
            }
        }
        // 根目录本身(entries 含 base 时已删;此处幂等兜底)
        if let Err(e) = self.fs_op.delete(&format!("/{}", base)) {
            eprintln!("⚠️ [remove] base kept: {}: {:#}", base, e);
        }
        Ok(())
    }

    fn reload(&self, vfs_op: &BlockingOperator, external_root: &str, storage_root: &str, keys: &[String]) -> Result<Vec<String>> {
        if !self.persistent { return Ok(Vec::new()); }
        // 1. 磁盘现存文件集(相对路径;只认 .md,与 load/offload 口径一致)
        let mut disk: Vec<String> = Vec::new();
        for e in fs_list_recursive(&self.fs_op, "/").unwrap_or_default() {
            let p = e.path().trim_end_matches('/').to_string();
            if is_md_file(&p) { disk.push(p); }
        }
        // 2. 清理:keys 中磁盘已消失的镜像(逐 key delete——remove_all 尽力而为会静默残留)
        let sr = storage_root.trim_end_matches('/');
        let mut removed: Vec<String> = Vec::new();
        for k in keys {
            let rel = k.strip_prefix(sr).unwrap_or(k.as_str()).trim_matches('/');
            if rel.is_empty() { continue; }
            if !disk.iter().any(|d| d == rel) {
                let _ = vfs_op.delete(k);
                let _ = self.delete_wal(k);
                removed.push(k.clone());
            }
        }
        // 3. **丢弃未落盘编辑**（help 对 reload 的承诺）：重泵前，把**所有现存文件**的 WAL 一并清掉。
        //    少了这一步，读路径会重放旧 WAL ⇒ 磁盘改动被旧内容盖住。实测：
        //    exp_proj 的 /.mds/ui.md 改盘 + reload 后仍读到旧文本（该 doc 早年经管道写过 ⇒ 有 WAL），
        //    而 exp_proj2 同一条 reload 立即生效（无 WAL）——不对称的根因就在这里。
        for d in &disk {
            let k = format!("{}/{}", sr, d);
            let _ = self.delete_wal(&k);
        }
        // 4. 重泵磁盘现存文件(复用 load 的泵入语义)
        self.load(vfs_op, external_root, storage_root)?;
        Ok(removed)
    }

    #[cfg(feature = "persistence")]
    fn wal_queue(&self) -> Option<Arc<SegQueue<(String, Vec<WalEntry>)>>> {
        Some(self.wal_queue.clone())
    }
}

fn offload_collect(op: &BlockingOperator, base: &str, rel: &str, results: &mut Vec<String>) {
    let dir = if rel.is_empty() { base.to_string() } else { format!("{}/{}", base, rel) };
    let entries = match op.list(&format!("{}/", dir)) { Ok(e) => e, Err(_) => return, };
    let dir_name = dir.rsplit('/').next().unwrap_or("");
    for entry in entries {
        let name = entry.name().strip_suffix('/').unwrap_or(entry.name());
        if name == "/" || name == dir_name { continue; }
        let full_rel = if rel.is_empty() { name.to_string() } else { format!("{}/{}", rel, name) };
        if entry.metadata().is_dir() { offload_collect(op, base, &full_rel, results); }
        else { results.push(full_rel); }
    }
}


// Legacy test helpers (kept for test compatibility)


/// WAL replay helper used by tests; production replay runs on the async worker thread.
#[cfg(test)]
fn replay_wal_entries(wire: &[u8], base: &str) -> Result<String, String> {
    use crate::engine::MdioEngine;
    if wire.is_empty() { return Ok(base.to_string()); }
    // 检查是否存在 FlushMarker，记录第一个 FlushMarker 之后的位置
    let mut has_fm = false;
    let mut first_fm_end = 0;
    {
        let mut pos = 0;
        while pos + 12 <= wire.len() {
            let len = u32::from_le_bytes([wire[pos], wire[pos+1], wire[pos+2], wire[pos+3]]) as usize;
            let magic = u32::from_le_bytes([wire[pos+4], wire[pos+5], wire[pos+6], wire[pos+7]]);
            if magic != WAL_MAGIC { break; }
            let end = pos + 12 + len;
            if end > wire.len() { break; }
            if let Ok((entry, _)) = bincode::serde::decode_from_slice::<WalEntry, _>(&wire[pos+12..end], bincode_cfg()) {
                if matches!(entry, WalEntry::FlushMarker) { has_fm = true; first_fm_end = end; break; }
            }
            pos = end;
        }
    }
    // 从 first_fm_end（或 0）开始 replay 所有 Edit
    let mut engine = MdioEngine::new(base);
    let mut pos = if has_fm { first_fm_end } else { 0 };
    while pos + 12 <= wire.len() {
        let len = u32::from_le_bytes([wire[pos], wire[pos+1], wire[pos+2], wire[pos+3]]) as usize;
        let magic = u32::from_le_bytes([wire[pos+4], wire[pos+5], wire[pos+6], wire[pos+7]]);
        if magic != WAL_MAGIC { break; }
        let end = pos + 12 + len;
        if end > wire.len() { break; }
        if let Ok((entry, _)) = bincode::serde::decode_from_slice::<WalEntry, _>(&wire[pos+12..end], bincode_cfg()) {
            if let WalEntry::Edit(rec) = entry {
                engine.edit(rec.start, rec.end, &rec.text);
            }
        }
        pos = end;
    }
    Ok(engine.make_snapshot().to_string())
}
