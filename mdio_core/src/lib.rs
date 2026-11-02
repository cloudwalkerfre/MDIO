// MDIO Core Library
//
// High-performance markdown parser with incremental parsing support
// and collaborative editing support via Cola CRDT

pub mod protocol;
pub mod arrow;
pub(crate) mod cache;
pub(crate) mod domain;
pub mod block;
pub mod projection;
pub mod infer;
pub mod selector;
pub mod op;
pub mod engine;
pub mod session;
pub mod utils;
#[cfg(test)]
#[path = "../tests/support/testhooks.rs"]
pub mod testhooks;
pub mod store;
pub mod vfs;
pub mod hook;
pub mod r#match;
pub mod snap;
pub mod backend;
pub mod inline;
pub mod effect;
pub mod topology;
pub mod incr;
// trinity：三边循环判官（唯一 verify 模块）。
// 纯框架部分无条件编译；obs 绑定部分经 feature = trinity-obs 门控。
pub mod trinity;
#[cfg(feature = "telemetry")]
pub mod telemetry;




// telemetry_span! Macro (feature-gated, exported)

// NOTE: Defined here (not in telemetry.rs) because #[macro_export] + #[cfg]
// does not compose well inside submodule files. Placing at crate root ensures
// the cfg branch is correctly resolved regardless of feature flags.

/// Create a telemetry span. Expands to `()` when feature OFF.
#[cfg(feature = "telemetry")]
#[macro_export]
macro_rules! telemetry_span {
    ($name:expr) => {
        $crate::telemetry::span($name)
    };
    ($name:expr, $($k:expr => $v:expr),*) => {
        $crate::telemetry::span($name)$(.kv($k, $v))*
    };
}

#[cfg(not(feature = "telemetry"))]
#[macro_export]
macro_rules! telemetry_span {
    ($name:expr) => {{ () }};
    ($name:expr, $($k:expr => $v:expr),*) => {{ () }};
}


// telemetry_guard! Macro — ensure_session for Store entry points


/// Ensure the telemetry session is active for the current thread.
/// Expands to `()` (noop) when feature OFF.
#[cfg(feature = "telemetry")]
#[macro_export]
macro_rules! telemetry_guard {
    ($store:expr) => {
        $crate::telemetry::ensure_session(&*$store.telemetry_active, &*$store.telemetry_session)
    };
}

#[cfg(not(feature = "telemetry"))]
#[macro_export]
macro_rules! telemetry_guard {
    ($store:expr) => {{ () }};
}

pub use engine::{MdioEngine, MdioSnapshot};
pub use selector::{SelectorIR, is_simple_selector};
pub use projection::{BlockType, BlockNode, BlockProj};
pub use block::{PackedLineInfo, cell_ranges_bytes};
pub use cache::{EditActions, edit_actions_arc};

/// 全局性能测试串行锁——所有时序断言测试共用（并行跑全量时防抖动）。
#[cfg(test)]
pub(crate) static PERF_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// S1 源码文本守卫与登记审计（只读判官，不接线）。
#[cfg(test)]
mod dguard;


// PyO3 Python Binding


#[cfg(feature = "python")]
mod python_bindings {
    use super::*;
    use crate::engine::MdioSnapshot;
    use crate::store::{ProjectStore, ServerAction};
    use pyo3::prelude::*;
    use pyo3::types::PyBytes;
    use pyo3::Bound;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use ahash::AHashMap as HashMap;

    #[pyclass]
    pub struct PyServerAction {
        pub inner: ServerAction,
    }

    #[pymethods]
    impl PyServerAction {
        #[getter]
        fn is_broadcast(&self) -> bool { matches!(self.inner, ServerAction::Broadcast { .. }) }

        #[getter]
        fn is_direct(&self) -> bool { matches!(self.inner, ServerAction::Direct { .. }) }

        #[getter]
        fn is_subscribe(&self) -> bool { matches!(self.inner, ServerAction::DirectAndSub { .. }) }

        #[getter]
        fn is_unsubscribe(&self) -> bool { matches!(self.inner, ServerAction::Unsubscribe { .. }) }

        #[getter]
        fn channel(&self) -> Option<String> {
            match &self.inner {
                ServerAction::Broadcast { channel, .. } => Some(channel.clone()),
                ServerAction::DirectAndSub { channel, .. } => Some(channel.clone()),
                ServerAction::Unsubscribe { channel } => Some(channel.clone()),
                _ => None
            }
        }

        #[getter]
        fn data<'py>(&self, py: Python<'py>) -> Option<Bound<'py, PyBytes>> {
            match &self.inner {
                ServerAction::Broadcast { data, .. } => Some(PyBytes::new(py, data)),
                ServerAction::Direct { data } => Some(PyBytes::new(py, data)),
                ServerAction::DirectAndSub { data, .. } => Some(PyBytes::new(py, data)),
                _ => None
            }
        }

        #[getter]
        fn is_file_action(&self) -> bool { matches!(self.inner, ServerAction::FileAction { .. }) }

        #[getter]
        fn file_action_entry_id(&self) -> Option<String> {
            match &self.inner {
                ServerAction::FileAction { entry_id, .. } => Some(entry_id.clone()),
                _ => None,
            }
        }

        #[getter]
        fn file_action_op_json(&self) -> Option<String> {
            match &self.inner {
                ServerAction::FileAction { op, .. } => {
                    serde_json::to_string(op).ok()
                }
                _ => None,
            }
        }
    }

    #[pyclass(name = "ProjectStore")]
    struct PyProjectStore {
        inner: Arc<ProjectStore>,
    }

    // ── mem 通用迭代器(map/未来 foreach 共享;全部复用既有件)──
    /// 对临时文本的 selector 命中做倒序遍历:next 取片段(面=sel 面),
    /// commit 以标准写 op(/>)写回(位置稳定靠倒序),end 导出新文本。
    #[pyclass]
    pub struct MemIter {
        st: crate::session::WriteState,
        /// 初始快照:倒序写时当前节点之前的内容从不变——行号/字节位置对当前文本仍有效,
        /// next/commit 全程复用(避免每步 make_snapshot 触发 ripple,10MB×n 次的性能根因)。
        init_snap: crate::engine::MdioSnapshot,
        hits: Vec<crate::block::BlockNode>,
        cursor: usize,
        cur: Option<crate::block::BlockNode>,
        sel: Vec<crate::selector::Step>,
        /// 读面(f 的输入;next 用)——写面由 commit 参数给(-op 自带)
        read_proj: Option<String>,
    }

    /// commit 期间 state 的占位:detach 要求 owned 数据,state 需 take/放回。
    fn empty_write_state() -> crate::session::WriteState {
        crate::session::WriteState {
            engine: crate::engine::MdioEngine::new(""),
            replica: None,
            pending_inserts: Default::default(),
            edit_spans: Vec::new(),
        }
    }

    #[pymethods]
    impl MemIter {
        /// 取下一个命中片段(倒序;面 = begin 时 sel 的面)。返回 None = 结束。
        /// detach:快照 clone(Clone 便宜:rope/Arc)+ 节点重建 + 投影读全在无 GIL 段。
        fn next(&mut self, py: Python<'_>) -> PyResult<Option<String>> {
            if self.cursor == 0 {
                return Ok(None);
            }
            self.cursor -= 1;
            let base = self.hits[self.cursor].clone();
            let snap = self.init_snap.clone();
            let surface = self.read_proj.as_deref()
                .and_then(crate::block::BlockProj::from_proj_str)
                .unwrap_or(crate::block::BlockProj::Content);
            let (text, node) = py.detach(move || {
                let node = snap.build_node(base.start_line, base.end_line, base.node_type, base.loose);
                let face = snap.node_proj();
                let text = face.read_block_proj(&node, surface);
                (text, node)
            });
            self.cur = Some(node);
            Ok(Some(text))
        }

        /// 对当前命中执行标准写 op(operator + 写面由 -op 表达式给,如 '/> -pj raw';与手写写 op 同形);
        /// payload = 内容(-> 时忽略);边界/换行归写面规范。
        /// detach:state take 出(dummy 占位)→ 无 GIL 执行 plan+apply → 放回。
        fn commit(&mut self, py: Python<'_>, payload: &str, operator: &str, write_proj: Option<&str>) -> PyResult<()> {
            let Some(base) = self.cur.take() else {
                return Err(pyo3::exceptions::PyRuntimeError::new_err("MemIter: commit 前未 next"));
            };
            let st = std::mem::replace(&mut self.st, empty_write_state());
            let snap = self.init_snap.clone();
            let sel = self.sel.clone();
            let proj = write_proj.map(|s| s.to_string());
            let payload_s = payload.to_string();
            let op_s = operator.to_string();
            let (st, res) = py.detach(move || {
                let cur = snap.build_node(base.start_line, base.end_line, base.node_type, base.loose);
                // 与 interpreter 的 WriteOp 语义一致:-> 时 proj = None(整节点区间;删除语义)
                // 写面来自 -op(-pj);-> 时 proj=None(整节点区间)
                let proj_eff = if op_s == "->" { None } else { proj.as_deref() };
                let req = crate::op::WriteRequest {
                    steps: &sel,
                    operator: &op_s,
                    text: payload_s,
                    proj: proj_eff,
                    auto_scaffold: true,
                    auto_level: false,
                };
                let mut st = st;
                let plan = match crate::op::plan_write_with_nodes(&snap, req, &[cur], 0) {
                    Ok(p) => p,
                    Err(e) => return (st, Err(e.message)),
                };
                let mut wire: smallvec::SmallVec<[crate::session::WireOp; 4]> = smallvec::SmallVec::new();
                {
                    let mut txn = crate::session::TransactionContext {
                        state: &mut st,
                        wire_ops: &mut wire,
                        tx_min_dirty_line: usize::MAX,
                    };
                    match plan {
                        crate::op::WritePlan::Apply { operator, edits } => txn.apply_mdio_op_multi(edits, &operator),
                        crate::op::WritePlan::PatchApply { edits } => txn.apply_patch_edits(edits),
                        crate::op::WritePlan::MissingTarget => {}
                    }
                }
                (st, Ok(()))
            });
            self.st = st;
            res.map_err(|m| pyo3::exceptions::PyRuntimeError::new_err(m))
        }

        /// 导出处理后的 mem 文本(map 的数据流输出)。
        fn end(&self, py: Python<'_>) -> PyResult<String> {
            let rope = self.st.engine.rope_clone();
            Ok(py.detach(move || rope.to_string()))
        }
    }

    #[pymethods]
    impl PyProjectStore {
        /// storage_root = 挂载配置串；布局恒定（文档 `<根>/wt`、运行态 `<根>/.wal`），
        /// doc_subdir / wal_rel 参数保留兼容，值被忽略。
        #[new]
        #[pyo3(signature = (storage_root, doc_subdir=None, wal_rel=None))]
        fn new(storage_root: &str, doc_subdir: Option<String>, wal_rel: Option<String>) -> PyResult<Self> {
            let store = ProjectStore::new_with_layout(storage_root, doc_subdir, wal_rel)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("VFS Parse Error: {}", e)))?;
            Ok(Self { inner: Arc::new(store) })
        }

        #[pyo3(signature = (parent_uid, name, rule=""))]
        fn derive_worktree(&self, parent_uid: &str, name: &str, rule: &str) -> PyResult<String> {
            self.inner.derive_worktree_with_rule(parent_uid, name, rule)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
        }

        fn drop_worktree(&self, uids: Vec<String>) -> PyResult<()> {
            self.inner.drop_worktree(&uids)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
        }

        #[pyo3(signature = (parent_uid, name, external_root, scheme="fs", rule="", doc_subdir=None, wal_rel=None))]
        fn register_worktree(&self, parent_uid: &str, name: &str, external_root: &str, scheme: &str, rule: &str,
                             doc_subdir: Option<String>, wal_rel: Option<String>) -> PyResult<String> {
            self.inner.register_worktree_at_with_layout(parent_uid, name, external_root, scheme, rule, doc_subdir, wal_rel)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
        }

        /// 直接调用方需要 resolve_path 返回 doc_id。
        #[pyo3(signature = (wt_uid, vfs_path, intent_write=false))]
        fn resolve_path(&self, wt_uid: &str, vfs_path: &str, intent_write: bool) -> PyResult<u32> {
            self.inner.resolve_path(wt_uid, vfs_path, intent_write)
                .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))
        }



        #[pyo3(signature = (wt_uid, vfs_path))]
        fn export_snapshot(&self, py: Python, wt_uid: &str, vfs_path: &str) -> PyResult<(String, Vec<u8>)> {
            let uid = wt_uid.to_string();
            let path = vfs_path.to_string();
            py.detach(move || {
                let doc_id = self.inner.resolve_path(&uid, &path, false)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                let sess = self.inner.open_session(doc_id)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                let snapshot = sess.publisher.load();
                let text = snapshot.rope.to_string();
                let blob = crate::arrow::blob::encode(&snapshot.handoff_copy());
                Ok((text, blob))
            })
        }


        #[pyo3(signature = (source, wt_uid, paths, dest_prefix="/__handoff/"))]
        fn handoff_from(
            &self,
            py: Python,
            source: pyo3::PyRef<'_, PyProjectStore>,
            wt_uid: &str,
            paths: Vec<String>,
            dest_prefix: &str,
        ) -> PyResult<Vec<(String, Vec<u8>)>> {
            let source_store = source.inner.clone();
            let dest_store = self.inner.clone();
            let wt = wt_uid.to_string();
            let prefix = dest_prefix.to_string();
            py.detach(move || {
                let mut out = Vec::with_capacity(paths.len());
                for (i, path) in paths.iter().enumerate() {
                    let doc_id = source_store.resolve_path(&wt, path, false)
                        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                    let sess = source_store.open_session(doc_id)
                        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                    let snapshot = sess.publisher.load();
                    let text = snapshot.rope.to_string();
                    let copy = snapshot.handoff_copy();
                    let name = std::path::Path::new(path).file_name()
                        .map(|x| x.to_string_lossy().into_owned())
                        .unwrap_or_else(|| format!("doc_{i}.md"));
                    let dest = format!("{prefix}{i}_{name}");
                    dest_store.put_snapshot_doc("genesis", &dest, &text, copy.clone(), rand::random::<u64>())
                        .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                    out.push((dest, crate::arrow::blob::encode(&copy)));
                }
                Ok(out)
            })
        }
        #[pyo3(signature = (wt_uid, vfs_path, text, blob))]
        fn put_snapshot_doc(&self, py: Python, wt_uid: &str, vfs_path: &str, text: &str, blob: &[u8]) -> PyResult<u32> {
            let uid = wt_uid.to_string();
            let path = vfs_path.to_string();
            let text = text.to_string();
            let snapshot = crate::arrow::blob::decode(blob)
                .map_err(pyo3::exceptions::PyValueError::new_err)?;
            py.detach(move || {
                self.inner.put_snapshot_doc(&uid, &path, &text, snapshot, rand::random::<u64>())
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))
            })
        }
        #[pyo3(signature = (wt_uid, vfs_path))]
        fn open_snapshot(&self, py: Python, wt_uid: &str, vfs_path: &str) -> PyResult<PySnapshot> {
            let uid = wt_uid.to_string();
            let path = vfs_path.to_string();
            py.detach(move || {
                let doc_id = self.inner.resolve_path(&uid, &path, false)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                let sess = self.inner.open_session(doc_id)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                let snapshot = sess.publisher.load();
                let snapshot: Arc<MdioSnapshot> = Arc::clone(&snapshot);
                Ok(PySnapshot { inner: snapshot })
            })
        }

        fn bindings_info(&self, uid: &str) -> PyResult<Vec<(String, String, String)>> {
            let tree = self.inner.tree.read();
            let node = match tree.nodes.get(uid) {
                Some(n) => n,
                None => return Ok(Vec::new()),
            };
            Ok(node.bindings.iter().map(|b| {
                let mode_str = match b.mode { crate::vfs::MountMode::RW => "RW", _ => "RO" };
                (b.my_path.clone(), b.target_path.clone(), mode_str.to_string())
            }).collect())
        }

        fn rm(&self, py: Python, wt_uid: &str, vfs_path: &str, recursive: bool, force: bool, verbose: bool) -> PyResult<Option<Vec<String>>> {
            let uid = wt_uid.to_string();
            py.detach(move || { self.inner.remove_path(&uid, vfs_path, recursive, force, verbose).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string())) })
        }

        /// reload:以磁盘为准同步 worktree 内存镜像。返回被清理的 storage_key 列表(+dirty 警告行)。
        fn reload(&self, py: Python, wt_uid: &str) -> PyResult<Vec<String>> {
            let uid = wt_uid.to_string();
            py.detach(move || { self.inner.reload(&uid).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string())) })
        }

        fn mk(&self, py: Python, wt_uid: &str, vfs_path: &str, parents: bool, verbose: bool) -> PyResult<Option<Vec<String>>> {
            let uid = wt_uid.to_string();
            py.detach(move || { self.inner.create_path(&uid, vfs_path, parents, verbose).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string())) })
        }

        fn path_exists(&self, py: Python, wt_uid: &str, vfs_path: &str) -> PyResult<bool> {
            let uid = wt_uid.to_string();
            py.detach(move || {
                self.inner
                    .path_exists(&uid, vfs_path)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))
            })
        }

        fn head_exists(&self, py: Python, wt_uid: &str, vfs_path: &str, ir_dict: &Bound<'_, pyo3::types::PyAny>) -> PyResult<bool> {
            let ir: SelectorIR = pythonize::depythonize(ir_dict)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let uid = wt_uid.to_string();
            py.detach(move || {
                self.inner
                    .head_exists(&uid, vfs_path, ir)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))
            })
        }

        fn get_file_metadata(&self, py: Python, wt_uid: &str, vfs_path: &str) -> PyResult<(u64, u64, bool)> {
            let uid = wt_uid.to_string();
            py.detach(move || {
                self.inner.get_file_metadata(&uid, vfs_path)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))
            })
        }

        fn is_dir(&self, py: Python, wt_uid: &str, vfs_path: &str) -> PyResult<bool> {
            let uid = wt_uid.to_string();
            py.detach(move || {
                self.inner.is_dir(&uid, vfs_path)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))
            })
        }

        // 核心接口：接收原始字节，返回行动指令
        #[pyo3(signature = (packet, conn_wt=None))]
        fn handle_packet<'py>(&self, py: Python<'py>, packet: &[u8], conn_wt: Option<String>) -> PyResult<PyServerAction> {
            // [写线程插桩] WS 帧总入口（0x04 写 / 0x01 树读 / …）——与 worker
            // 线程各自 ensure_session 对称：缺了它，帧内所有 span（apply_crdt /
            // execute_txn / guard_validate / seam_classify）在 gateway 导出里整条缺失
            // （telemetry 会话 thread_local，docs/TELEMETRY.md §10.1）。
            let _tel_guard = telemetry_guard!(self.inner);
            let store = self.inner.clone();
            let packet_vec = packet.to_vec();

            // 释放 GIL，独立线程中运行 Rust 
            let action = py.detach(move || { store.handle_packet(&packet_vec, conn_wt.as_deref()) });
            Ok(PyServerAction { inner: action })
        }

        // 测试/调试接口，当前无任何调用方。
        // 与 write_locator 不同：该函数直接接收内部 doc_id（绕过 VFS 路径解析），
        // 返回原始 CRDT wire bytes（而非 write_locator 的 (doc_id, DN_SYNC 协议包) 元组）。
        // 如需启用 Hybrid Mode，请先对齐 write_locator 的返回格式并接入广播路径。
        fn apply_edit_selector<'py>(&self, py: Python<'py>, doc_id: u32, ir_dict: &Bound<'py, pyo3::types::PyAny>, op: &str, text: &str) -> PyResult<Vec<u8>> {
            // [写线程插桩] 与 `write_locator` 同款：把 telemetry 会话装配到**本线程**。
            // 缺了它，WS/CRDT 写路径（`execute_txn` / `guard_validate` / `seam_classify`）
            // 在 gateway 导出里整条缺失——telemetry 会话是 thread_local（docs/TELEMETRY.md
            // §10.1），worker 线程各自 ensure_session，写入口不单独建。
            let _tel_guard = telemetry_guard!(self.inner);
            // let ir_clone = ir_json.to_string();
            let op_clone = op.to_string();
            let text_clone = text.to_string();
            let ir: SelectorIR = pythonize::depythonize(ir_dict)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            
            let crdt_updates = py.detach(move || -> Result<Vec<u8>, pyo3::PyErr> {
                let session = self.inner.open_session(doc_id).map_err(|e|
                    pyo3::exceptions::PyValueError::new_err(format!("Failed to open session : {}", e))
                )?;
                let (_, w_bytes) = session.execute_transaction(Some(&ir.steps), false, |ctx| {
                    let ranges = crate::selector::select_ranges(&ctx.snapshot(), &ir.steps);
                    ctx.apply_mdio_op(ranges, &op_clone, &text_clone);
                });

                Ok::<_, pyo3::PyErr>(w_bytes)
            })?;
            Ok(crdt_updates)
        }

        /// 读操作 — 投影驱动，默认 raw。
        #[pyo3(signature = (wt_uid, vfs_path, ir_dict, proj=None))]
        fn read_locator<'py>(&self, py: Python<'py>, wt_uid: &str, vfs_path: &str, ir_dict: &Bound<'py, pyo3::types::PyAny>, proj: Option<&str>) -> PyResult<Vec<String>> {
            let _guard = telemetry_guard!(self.inner);
            let _span_total = telemetry_span!("rl_total");
            // 边界防御:空路径在 debug build 触发 opendal debug_assert
            // (空 → normalize "/" + FILE mode 矛盾)——release 下 resolve_path
            // 返回 File not found——此处统一显式报错(不碰核心逻辑)。
            if vfs_path.is_empty() {
                return Err(pyo3::exceptions::PyIOError::new_err("File not found: empty path"));
            }
            #[cfg(feature = "telemetry")] let _sp_depy = crate::telemetry_span!("rl_depy");
            let ir: SelectorIR = pythonize::depythonize(ir_dict)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            #[cfg(feature = "telemetry")] drop(_sp_depy);
            let uid = wt_uid.to_string();

            py.detach(move || -> PyResult<Vec<String>> {
                let doc_id = self.inner.resolve_path(&uid, vfs_path, false).map_err(|e|
                    pyo3::exceptions::PyIOError::new_err(e.to_string())
                )?;
                let session = self.inner.open_session(doc_id).map_err(|e|
                    pyo3::exceptions::PyIOError::new_err(e.to_string())
                )?;

                let snapshot = session.publisher.load();

                match crate::op::read_projection(&snapshot, &ir.steps, proj) {
                    Ok(results) => {
                        #[cfg(feature = "telemetry")] let _sp_ret = crate::telemetry_span!("rl_ret");
                        return Ok(results);
                    }
                    Err(_) => return Ok(Vec::new()),
                }

                // [TIME]
                // // 3.  [E5] 核心选择定位：(它在 10 万行级别到底多快？)
                // let t_sel_start = std::time::Instant::now();
                // let ranges = snapshot.select(&steps);
                // let t_sel = t_sel_start.elapsed();
                // // 4. [杂项] 字符串真实扣取转换时间
                // let t_slice_start = std::time::Instant::now();
                // let res: Vec<String> = ranges.into_iter()
                //     .map(|r| snapshot.rope.byte_slice(r).to_string())
                //     .collect();
                // let t_slice = t_slice_start.elapsed();
                // println!(
                //     "[ TRACE Rust Read] depythonize: {}μs | E4 (VFS/Session Lock): {}μs | E5 (AST Search): {}μs | Rope Slice Array: {}μs",
                //     t_depythonize.as_micros(),
                //     t_vfs.as_micros(),
                //     t_sel.as_micros(),
                //     t_slice.as_micros()
                // );
                
                // Ok(res)
                
                // Ok(snapshot.select(&steps).into_iter()
                //     .map(|r| snapshot.rope.byte_slice(r).to_string())
                //     .collect()
                // )




                // //  ================= 打点开始 =================
                // let start_time = std::time::Instant::now();
                
                // // 核心寻址：执行 AST 选择器算法，找出行号边界
                // let ranges = snapshot.select(&steps);
                
                // // 核心切片：根据算出的边界，从 B-Tree (Rope) 中扣出连续的字符串
                // let mut results = Vec::with_capacity(ranges.len());
                // for r in ranges {
                //     results.push(snapshot.rope.byte_slice(r).to_string());
                // }
                
                // let elapsed = start_time.elapsed();
                // // 因为这段会在后台频发，我们只在找到数据时，且耗时超过 1 微秒时打印，防刷屏
                // if !results.is_empty() {
                //     println!("🦀 [Rust Core Perf] Select + Rope Slice ({} nodes): {:.3} ms ({} ns)", 
                //         results.len(),
                //         elapsed.as_secs_f64() * 1000.0,
                //         elapsed.as_nanos()
                //     );
                // }
                // //  ================= 打点结束 =================
                // Ok(results)                
            })
        }

        // Returns (doc_id, dn_sync_packet): doc_id for channel routing, packet for broadcast.
        // Empty packet (len 0) means no CRDT ops were produced — caller checks truthiness as before.
        #[pyo3(signature = (wt_uid, vfs_path, ir_dict, op, text, auto_scaffold=true, god=false, auto_level=false, suppress_hooks=false, proj=None))]
        fn write_locator(&self, py: Python, wt_uid: &str, vfs_path: &str, ir_dict: &Bound<'_, PyAny>, op: &str, text: &str, auto_scaffold: bool, god: bool, auto_level: bool, suppress_hooks: bool, proj: Option<&str>) -> PyResult<(u32, Vec<u8>)> {
            let _guard = telemetry_guard!(self.inner);
            let _span_total = telemetry_span!("wl_total");
            // 边界防御(同 read_locator——空路径 debug_assert 防)
            if vfs_path.is_empty() {
                return Err(pyo3::exceptions::PyIOError::new_err("File not found: empty path"));
            }
            #[cfg(feature = "telemetry")] let _sp_depy = crate::telemetry_span!("wl_depy");
            let ir: SelectorIR = pythonize::depythonize(ir_dict)?;
            #[cfg(feature = "telemetry")] drop(_sp_depy);
            let is_write_op = op == "+>" || op == "/>";
            let effective_scaffold = auto_scaffold || (god && is_write_op);
            let uid = wt_uid.to_string();

            if (god || auto_scaffold) && is_write_op {
                self.inner.create_path(&uid, vfs_path, true, false).ok();
            }

            let (doc_id, packet) = py.detach(move || -> Result<(u32, Vec<u8>), pyo3::PyErr> {
            #[cfg(feature = "telemetry")] let _sp_vfs = crate::telemetry_span!("wl_vfs");
            let doc_id = self.inner.resolve_path(&uid, vfs_path, true)?;
            let session = self.inner.open_session(doc_id)?;
            // 本帧 op 所属的血统号（随载荷下行，接收端据此判断自己那份副本还对不对得上）
            let lineage = session.lineage;
            #[cfg(feature = "telemetry")] drop(_sp_vfs);

            // ── 一次 publisher.load + 一次 select_for_write ──
            // 结果保留，传入事务闭包复用；若 CRDT 并发导致快照变更，
            // 闭包内通过 Arc::ptr_eq 检测并回退到重新 select。
            let check_snap = session.publisher.load();
            let mut drill_depth = 0usize;
            #[cfg(feature = "telemetry")] let _sp_msel = crate::telemetry_span!("wl_main_select");
            let nodes = crate::selector::select_nodes_with_depth(&check_snap, &ir.steps, &mut drill_depth);
            #[cfg(feature = "telemetry")] drop(_sp_msel);

            // Pre-check：目标不存在 + 简单选择器 + 非接骨 → 报错
            if nodes.is_empty() && !effective_scaffold && is_simple_selector(&ir.steps) {
                let msg = format!(
                    "目标节点不存在，且未开启接骨模式 (op: {}, path: {})",
                    op, vfs_path
                );
                return Err(pyo3::exceptions::PyValueError::new_err(msg));
            }

            let op_owned = op.to_string();
            let mut text_owned = text.to_string();
            let proj_owned = proj.map(|s| s.to_string());

            // 尾换行归一化已下沉到 plan 层（op::finalize_payload——范围已知处
            // 决策：范围行尾才补 \n；行中范围（comment 标记间）不补——
            // FFI 层 range 盲的 blanket 追加会污染行中结构）。

            let steps = ir.steps;
            let (result, mut w_bytes) = session.execute_transaction(
                Some(&steps), suppress_hooks,
                |ctx| -> Result<(), String> {
                    let req = crate::op::WriteRequest {
                        steps: &steps,
                        operator: &op_owned,
                        text: text_owned,
                        proj: proj_owned.as_deref(),
                        auto_scaffold: effective_scaffold,
                        auto_level,
                    };

                    // 检测 CRDT 并发：若 publisher 快照未变，复用外部 nodes
                    #[cfg(feature = "telemetry")] let _sp_pe = crate::telemetry_span!("wl_ptr_eq");
                    let current_pub = session.publisher.load();
                    let ptr_eq = std::sync::Arc::ptr_eq(&check_snap, &current_pub);
                    #[cfg(feature = "telemetry")] drop(_sp_pe);
                    let plan = if nodes.is_empty() {
                        // ── 接骨：特例，必须新建全量 Snapshot ──
                        // 无并发（ptr_eq）时复用 check_snap（读路径
                        // 快照——与主 select 同源同 block_cache——锚点 select
                        // 命中主 select 的 fdc 缓存——省 ~90µs/写）；有并发时
                        // State 已更新——必须新建（保守）。
                        #[cfg(feature = "telemetry")] let _sp_bsc = crate::telemetry_span!("wl_branch_scaffold");
                        let snap_owned;
                        let snap: &crate::projection::Snapshot = if ptr_eq {
                            &check_snap
                        } else {
                            #[cfg(feature = "telemetry")] let _sp_cs = crate::telemetry_span!("wl_ctx_snap");
                            snap_owned = ctx.snapshot();
                            #[cfg(feature = "telemetry")] drop(_sp_cs);
                            &snap_owned
                        };
                        #[cfg(feature = "telemetry")] let _sp_plan = crate::telemetry_span!("wl_plan");
                        let p = crate::op::plan_write_with_nodes(snap, req, &nodes, drill_depth);
                        #[cfg(feature = "telemetry")] drop(_sp_plan);
                        #[cfg(feature = "telemetry")] drop(_sp_bsc);
                        p
                    } else if ptr_eq {
                        // ── 正常写入，无并发 → 复用 publisher snapshot，零 Rope clone ──
                        #[cfg(feature = "telemetry")] let _sp_bfast = crate::telemetry_span!("wl_branch_fast");
                        #[cfg(feature = "telemetry")] let _sp_plan = crate::telemetry_span!("wl_plan");
                        let p = crate::op::plan_write_with_nodes(&check_snap, req, &nodes, drill_depth);
                        #[cfg(feature = "telemetry")] drop(_sp_plan);
                        #[cfg(feature = "telemetry")] drop(_sp_bfast);
                        p
                    } else {
                        // ── CRDT 并发 → 必须重建 ──
                        #[cfg(feature = "telemetry")] let _sp_brs = crate::telemetry_span!("wl_branch_reselect");
                        #[cfg(feature = "telemetry")] let _sp_cs = crate::telemetry_span!("wl_ctx_snap");
                        let snap = ctx.snapshot();
                        #[cfg(feature = "telemetry")] drop(_sp_cs);
                        #[cfg(feature = "telemetry")] let _sp_rs = crate::telemetry_span!("wl_reselect");
                        let fresh_nodes =
                            crate::selector::select_for_write(&snap, req.steps);
                        #[cfg(feature = "telemetry")] drop(_sp_rs);
                        #[cfg(feature = "telemetry")] let _sp_plan = crate::telemetry_span!("wl_plan");
                        let p = crate::op::plan_write_with_nodes(&snap, req, &fresh_nodes, drill_depth);
                        #[cfg(feature = "telemetry")] drop(_sp_plan);
                        #[cfg(feature = "telemetry")] drop(_sp_brs);
                        p
                    }.map_err(|e| e.message)?;

                    #[cfg(feature = "telemetry")] let _sp_apply = crate::telemetry_span!("wl_apply");
                    match plan {
                        crate::op::WritePlan::Apply { operator, edits } => {
                            ctx.apply_mdio_op_multi(edits, &operator);
                        }
                        crate::op::WritePlan::PatchApply { edits } => {
                            ctx.apply_patch_edits(edits);
                        }
                        crate::op::WritePlan::MissingTarget => {}
                    }
                    #[cfg(feature = "telemetry")] drop(_sp_apply);
                    Ok(())
                },
            );
            if let Err(msg) = result {
                return Err(pyo3::exceptions::PyValueError::new_err(msg));
            }

            // 原地插入 DN_SYNC header，零额外分配：
            //   [tag][len][peer][doc] + [lineage: u64 LE] + bincode(Vec<WireOp>)
            // 血统号必须随帧下行：客户端凭它判断"我这份副本还是不是这条血统的后代"，
            // 不是就整篇重取（换代），而不是把 op 集成进陌生的血统里（永久静默）。
            if !w_bytes.is_empty() {
                let body_len = w_bytes.len() + 8;
                w_bytes.resize(body_len + crate::protocol::HEADER_SIZE, 0);
                // 先把 bincode 载荷右移 8（HEADER 位稍后写头），再写血统号——顺序不能反。
                w_bytes.copy_within(0..body_len - 8, crate::protocol::HEADER_SIZE + 8);
                w_bytes[crate::protocol::HEADER_SIZE..crate::protocol::HEADER_SIZE + 8]
                    .copy_from_slice(&lineage.to_le_bytes());
                w_bytes[0] = crate::protocol::DN_SYNC;
                w_bytes[1..5].copy_from_slice(&(body_len as u32).to_le_bytes());
                w_bytes[5..9].copy_from_slice(&0u32.to_le_bytes()); // peer = 0 (server)
                w_bytes[9..13].copy_from_slice(&doc_id.to_le_bytes());
            }
            let packet = w_bytes;
            Ok((doc_id, packet))
            })?;
            Ok((doc_id, packet))
        }

        /// ($$ 原语使用)，没有 File Lock 和 Session 开销 — 投影驱动，默认 raw。
        #[pyo3(signature = (text, ir_dict, proj=None))]
        fn query_memory<'py>(&self, py: Python<'py>, text: &str, ir_dict: &Bound<'py, pyo3::types::PyAny>, proj: Option<&str>) -> PyResult<Vec<String>> {
            let _guard = telemetry_guard!(self.inner);
            let _span = telemetry_span!("query_memory");
            let text_c = text.to_string();
            let ir: SelectorIR = pythonize::depythonize(ir_dict)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;

            py.detach(move || -> PyResult<Vec<String>> {
                let snapshot = crate::projection::project_text(&text_c);

                match crate::op::read_projection(&snapshot, &ir.steps, proj) {
                    Ok(results) => return Ok(results),
                    Err(_) => return Ok(Vec::new()),
                }
            })
        }

        /// map 算子用:文本 → 瞬态快照 → selector 命中节点的 (字节区间, 投影面值)。
        /// 与 query_memory 同链(project_text + selector),区间 + 该面文本一起回(map 的片段形态)。
        fn query_memory_ranges<'py>(&self, py: Python<'py>, text: &str, ir_dict: &Bound<'py, pyo3::types::PyAny>, proj: Option<&str>) -> PyResult<Vec<(usize, usize, String)>> {
            let _guard = telemetry_guard!(self.inner);
            let _span = telemetry_span!("query_memory_ranges");
            let text_c = text.to_string();
            let ir: SelectorIR = pythonize::depythonize(ir_dict)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let p = proj.unwrap_or("raw").to_string();

            py.detach(move || -> PyResult<Vec<(usize, usize, String)>> {
                let snapshot = crate::projection::project_text(&text_c);
                let surface = crate::block::BlockProj::from_proj_str(&p).unwrap_or(crate::block::BlockProj::Raw);
                let nodes = crate::selector::select_nodes(&snapshot, &ir.steps);
                let face = snapshot.node_proj();
                Ok(nodes.into_iter()
                    .map(|n| (n.byte_range.start, n.byte_range.end, face.read_block_proj(&n, surface)))
                    .collect())
            })
        }

        /// map 算子用:mem 通用迭代器开始(一次 State + select,倒序游标)。
        #[pyo3(signature = (text, ir_dict, proj=None))]
        fn mem_iter_begin<'py>(&self, py: Python<'py>, text: &str, ir_dict: &Bound<'py, pyo3::types::PyAny>, proj: Option<&str>) -> PyResult<Py<MemIter>> {
            let _guard = telemetry_guard!(self.inner);
            let ir: SelectorIR = pythonize::depythonize(ir_dict)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let text_c = text.to_string();
            let steps = ir.steps;
            let steps_sel = steps.clone();
            let proj_s = proj.map(|s| s.to_string());
            // detach:全 owned 数据进、owned 结果出(无借用跨 GIL 边界)
            let (st, init_snap, hits) = py.detach(move || {
                let mut st = crate::session::WriteState {
                    engine: crate::engine::MdioEngine::new(&text_c),
                    replica: None,
                    pending_inserts: Default::default(),
                    edit_spans: Vec::new(),
                };
                let snap = st.engine.make_snapshot();
                let hits = crate::selector::select_nodes(&snap, &steps_sel);
                (st, snap, hits)
            });
            let iter = MemIter {
                st,
                init_snap,
                cursor: hits.len(),  // 倒序:从末尾开始(写后段不影响前段位置)
                hits,
                cur: None,
                sel: steps,
                read_proj: proj_s,
            };
            Py::new(py, iter)
        }

        /// 读操作辅助，使用 false
        fn get_text<'py>(&self, py: Python<'py>, wt_uid: &str, vfs_path: &str) -> PyResult<String> {
            let _guard = telemetry_guard!(self.inner);
            let _span = telemetry_span!("get_text");
            let p_c = vfs_path.to_string();
            let uid = wt_uid.to_string();
            py.detach(move || -> PyResult<String> {
                if let Ok(doc_id) = self.inner.resolve_path(&uid, &p_c, false) {
                    if let Ok(sess) = self.inner.open_session(doc_id) {
                        return Ok(sess.get_engine_text()); // RCU
                    }
                }
                Ok(String::new())
            })
        }

        /// 返回文档的完整 JSON AST（含 inline 节点）。测试专用。
        /// root 信封契约（mode=cf）：md 名（从 vfs_path 提取）+ timestamp（信封自动）。
        fn to_ast_json<'py>(&self, py: Python<'py>, wt_uid: &str, vfs_path: &str) -> PyResult<String> {
            let _guard = telemetry_guard!(self.inner);
            let _span = telemetry_span!("ast_to_json");
            let p_c = vfs_path.to_string();
            let uid = wt_uid.to_string();
            py.detach(move || -> PyResult<String> {
                if let Ok(doc_id) = self.inner.resolve_path(&uid, &p_c, false) {
                    if let Ok(sess) = self.inner.open_session(doc_id) {
                        let snapshot = sess.publisher.load();
                        let md_name = std::path::Path::new(&p_c)
                            .file_name().map(|f| f.to_string_lossy().into_owned());
                        // omp 读点④：Python FFI——按进程模式选拓扑。
                        let mode = crate::topology::process_mode();
                        return Ok(snapshot.to_ast_object_envelope(mode, md_name.as_deref()));
                    }
                }
                Ok(String::new())
            })
        }

        /// 直接解析文本为 JSON AST（不经过 VFS）。测试专用。
        /// root 信封契约（mode=cf）。
        fn parse_to_ast<'py>(&self, py: Python<'py>, text: &str) -> PyResult<String> {
            let _guard = telemetry_guard!(self.inner);
            let _span = telemetry_span!("ast_parse");
            let text_c = text.to_string();
            py.detach(move || -> PyResult<String> {
                let snapshot = crate::projection::project_text(&text_c);
                // omp 读点⑤：无 doc 场景（测试专用入口）——进程模式。
                Ok(snapshot.to_ast_object_mode(crate::topology::process_mode()))
            })
        }

        /// 直接解析文本为嵌套 JSON AST。测试专用。
        /// root 信封契约（mode=cf）。
        fn parse_to_ast_nested<'py>(&self, py: Python<'py>, text: &str) -> PyResult<String> {
            let _guard = telemetry_guard!(self.inner);
            let _span = telemetry_span!("ast_parse_nested");
            let text_c = text.to_string();
            py.detach(move || -> PyResult<String> {
                let snapshot = crate::projection::project_text(&text_c);
                // omp 读点⑥：无 doc 场景（测试专用入口）——进程模式。
                Ok(snapshot.to_ast_object_mode(crate::topology::process_mode()))
            })
        }

        /// Debug: 纯结构嵌套树（无 inline 渲染）。验证 block 层嵌套用。
        fn debug_block_tree<'py>(&self, py: Python<'py>, text: &str) -> PyResult<String> {
            let _guard = telemetry_guard!(self.inner);
            let _span = telemetry_span!("ast_debug_tree");
            let text_c = text.to_string();
            py.detach(move || -> PyResult<String> {
                let snapshot = crate::projection::project_text(&text_c);
                Ok(snapshot.debug_block_tree())
            })
        }

        /// 暴漏给用户的通配扫描
        #[pyo3(signature = (wt_uid, pattern, include_hidden=false))]
        fn expand_glob(&self, py: Python<'_>, wt_uid: &str, pattern: &str, include_hidden: bool) -> PyResult<Vec<String>> {
            let uid = wt_uid.to_string();
            py.detach(move || -> PyResult<Vec<String>> {
                Ok(self.inner.expand_glob(&uid, pattern, include_hidden))
            })
        }

        /// 独立主动落盘通道（供后端框架自己做节流 / 退出钩子 使用）
        fn force_flush_file(&self, py: Python<'_>, doc_id: u32) -> PyResult<()> {
            py.detach(move || -> PyResult<()> {
                self.inner.flush_doc_to_vfs(doc_id).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))
            })
        }

        /// Alias for force_flush_file — legacy test compatibility.
        fn flush_doc_to_vfs(&self, py: Python<'_>, doc_id: u32) -> PyResult<()> {
            py.detach(move || -> PyResult<()> {
                self.inner.flush_doc_to_vfs(doc_id).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))
            })
        }

        fn register_hook_dispatcher(&self, callback: Py<PyAny>) -> PyResult<()> {
            // 把 Python 传来的闭包指针，牢牢锁进最底层的 Rust 共享变量里
            *self.inner.ffi_callback_ref.lock().unwrap() = Some(callback);
            Ok(())
        }

        /// AST Dispatch 回调注册（Step 5 incremental patches）。
        fn register_ast_callback(&self, callback: Py<PyAny>) -> PyResult<()> {
            *self.inner.ast_callback_ref.lock().unwrap() = Some(callback);
            Ok(())
        }

        fn register_hook(&self, wt_uid: &str, vfs_path: &str, ir_dict: &Bound<'_, pyo3::types::PyAny>, cb_code: &str, ctx_dict: &Bound<'_, pyo3::types::PyAny>, is_always: bool) -> PyResult<u64> {
            let doc_id = self.inner.resolve_path(wt_uid, vfs_path, false).map_err(|e|
                pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
            let ir: SelectorIR = pythonize::depythonize(ir_dict).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let ctx_val: serde_json::Value = pythonize::depythonize(ctx_dict).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let hook_id = self.inner.register_hook(wt_uid, doc_id, ir, cb_code, ctx_val, is_always);
            if hook_id == 0 {
                return Err(pyo3::exceptions::PyIOError::new_err(format!(
                    "hook register failed (session open): wt={} path={}", wt_uid, vfs_path)));
            }
            Ok(hook_id)
        }
        fn remove_hook(&self, hook_id: u64) -> PyResult<()> { self.inner.remove_hook(hook_id); Ok(()) }
        fn clear_worktree_hooks(&self, uid: &str) -> PyResult<()> {
            self.inner.hook_index.lock().clear_worktree(uid); Ok(())
        }
        fn hook_count_for_worktree(&self, uid: &str) -> usize {
            self.inner.hook_index.lock().count_for_worktree(uid)
        }
        fn clear_doc_hooks(&self, wt_uid: &str, vfs_path: &str) -> PyResult<()> {
            if let Ok(doc_id) = self.inner.resolve_path(wt_uid, vfs_path, false) { self.inner.clear_doc_hooks(doc_id, wt_uid); }
            Ok(())
        }

        // ── patch match attr：归属注册 / 注销（spec N6；形状同 register_hook 族）──

        /// 注册归属：`file_pattern` 已由 mdio_py 声明层归一（核永不看到 `.*`）。
        /// 返回 reg_id；核侧失败 ⇒ 抛 IOError（不静默成功）。
        fn register_match(&self, wt_uid: &str, file_pattern: &str,
                          ir_dict: &Bound<'_, pyo3::types::PyAny>, tag: &str) -> PyResult<u64> {
            let ir: SelectorIR = pythonize::depythonize(ir_dict)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let reg_id = self.inner.register_match(wt_uid, file_pattern, ir, tag);
            if reg_id == 0 {
                return Err(pyo3::exceptions::PyIOError::new_err(format!(
                    "match register failed: wt={} pattern={} tag={}", wt_uid, file_pattern, tag)));
            }
            Ok(reg_id)
        }
        fn remove_match(&self, reg_id: u64) -> usize { self.inner.remove_match(reg_id) }
        /// 清某文档的归属（吞「文档已不在」——同 clear_doc_hooks）。
        fn clear_doc_matches(&self, wt_uid: &str, vfs_path: &str) -> PyResult<()> {
            if let Ok(doc_id) = self.inner.resolve_path(wt_uid, vfs_path, false) {
                self.inner.clear_doc_matches(doc_id, wt_uid);
            }
            Ok(())
        }
        fn clear_worktree_matches(&self, uid: &str) -> PyResult<()> {
            self.inner.clear_worktree_matches(uid); Ok(())
        }
        fn match_count_for_worktree(&self, uid: &str) -> usize {
            self.inner.match_count_for_worktree(uid)
        }

        fn run_find<'py>(&self, py: Python<'py>, wt_uid: &str, vfs_path: &str, ir_dict: &Bound<'py, pyo3::types::PyAny>, query_configs: Vec<(String, bool, bool)>, ctx_b: usize, ctx_a: usize) -> PyResult<Vec<String>> {
            let _guard = telemetry_guard!(self.inner);
            let _span = telemetry_span!("run_find", "queries" => query_configs.len());
            let ir: SelectorIR = pythonize::depythonize(ir_dict)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let uid = wt_uid.to_string();
            py.detach(move || -> PyResult<Vec<String>> {
                let doc_id = self.inner.resolve_path(&uid, vfs_path, false).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                let session = self.inner.open_session(doc_id).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;

                let snapshot = session.publisher.load(); // RCU 白嫖
                Ok(snapshot.run_find(&ir.steps, &query_configs, ctx_b, ctx_a))
            })
        }

        fn run_tree<'py>(&self, py: Python<'py>, wt_uid: &str, vfs_path: &str, ir_dict: &Bound<'py, pyo3::types::PyAny>, depth: usize, detail: usize) -> PyResult<String> {
            let ir: SelectorIR = pythonize::depythonize(ir_dict)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let uid = wt_uid.to_string();
            py.detach(move || -> PyResult<String> {
                let doc_id = self.inner.resolve_path(&uid, vfs_path, false).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                let session = self.inner.open_session(doc_id).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                let snapshot = session.publisher.load();
                Ok(snapshot.run_tree(&ir.steps, depth, detail))
            })
        }

        fn heading_at_line<'py>(&self, py: Python<'py>, wt_uid: &str, vfs_path: &str, line_no: usize) -> PyResult<String> {
            let uid = wt_uid.to_string();
            py.detach(move || -> PyResult<String> {
                let doc_id = self.inner.resolve_path(&uid, vfs_path, false)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                let session = self.inner.open_session(doc_id)
                    .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
                let snapshot = session.publisher.load();
                Ok(snapshot.heading_at_line(line_no))
            })
        }

        /// AST 通道：获取 named-property AST JSON（全量）。
        fn get_ast(&self, py: Python<'_>, doc_id: u32) -> PyResult<Option<String>> {
            py.detach(move || Ok(self.inner.get_ast(doc_id)))
        }

        /// omp 测试通道：进程级模式切换（仅测试/诊断用）。
        /// 生产唯一通道 = 进程环境变量 MDIO_PROJ_MODE（默认 Hs，显式 "cf" 才 Cf）；生产切换 = 重启进程。
        /// 非法模式字符串报错（显式优于静默回落）。
        #[pyo3(signature = (mode))]
        fn set_mode(&self, mode: &str) -> PyResult<()> {
            let m = match mode {
                "cf" => crate::topology::ProjectionMode::Cf,
                "hs" => crate::topology::ProjectionMode::Hs,
                "bf" => crate::topology::ProjectionMode::Bf,
                _ => {
                    return Err(pyo3::exceptions::PyValueError::new_err(format!(
                        "unknown mode: {mode} (expect cf/hs/bf)"
                    )))
                }
            };
            crate::topology::set_mode(m);
            Ok(())
        }

        /// AST 通道：构建 DN_AST 帧（含 Net header + type byte + JSON payload）。
        fn get_ast_packet(&self, py: Python<'_>, doc_id: u32) -> PyResult<Option<Vec<u8>>> {
            py.detach(move || Ok(self.inner.get_ast_packet(doc_id)))
        }

        /// AST 通道（异步）：FULL JSON 渲染投递 FJ 线程——调用线程立即返回——
        /// 不阻塞调用线程/P 帧。完成回调（FJ 线程内调用）: cb(frame: Option<bytes>)——
        /// Some = 'F' 帧（渲染完成且快照未过期）; None = 快照已过期（调用方决定是否重投）。
        /// 返回 False = doc_id 无 session（投递失败）。
        #[cfg(feature = "python")]
        fn get_ast_packet_async<'py>(&self, py: Python<'py>, doc_id: u32, cb: Py<PyAny>) -> PyResult<bool> {
            py.detach(move || Ok(self.inner.submit_ast_packet(doc_id, cb)))
        }

        /// 获取前端所需的 VFS 文件树
        fn get_vfs_tree<'py>(&self, py: Python<'py>, wt_uid: &str, include_hidden: bool) -> PyResult<Vec<u8>> {
            let uid = wt_uid.to_string();
            py.detach(move || -> PyResult<Vec<u8>> {
                self.inner.refresh_tree_json(&uid, include_hidden).map_err(|e|
                    pyo3::exceptions::PyIOError::new_err(e.to_string()))
            })
        }

        /// 纯解码 checkpoint → 返回文本内容。不修改 session。供 iroh_mount 使用。
        #[staticmethod]
        fn decode_checkpoint_text(checkpoint_bytes: &[u8]) -> PyResult<String> {
            crate::store::ProjectStore::decode_checkpoint_text(checkpoint_bytes)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
        }

        /// 测试用：强制重置 doc 的 session，写入新内容，返回 doc_id。
        fn reset_doc(&self, py: Python<'_>, wt_uid: &str, vfs_path: &str, content: &str) -> PyResult<u32> {
            let uid = wt_uid.to_string();
            let vfs_path = vfs_path.to_string();
            let ct = content.to_string();
            py.detach(move || self.inner.reset_doc(&uid, &vfs_path, &ct)
                .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string())))
        }

        /// 反查 doc_id 归属的 WT uid。供 Python dispatch 路由用。
        fn doc_wt_uid(&self, doc_id: u32) -> Option<String> {
            self.inner.doc_wt_uid(doc_id)
        }

        /// 该文档当前血统号（epoch）。无会话 ⇒ None。网关据此判断要不要给订阅者推换代。
        fn doc_lineage(&self, py: Python<'_>, doc_id: u32) -> Option<u64> {
            py.detach(move || self.inner.doc_lineage(doc_id))
        }

        /// 构造一帧 DN_OPEN（**服务端主动推送换代**用）：载荷是该文档当前 checkpoint
        /// （含血统号）。订阅者收到即整篇重取——不需要等下一次写入才"发现"血统变了。
        fn build_dn_open(&self, py: Python<'_>, doc_id: u32, peer: u32) -> Option<Vec<u8>> {
            py.detach(move || self.inner.build_dn_open(doc_id, peer).map(|p| p.to_vec()))
        }

        /// 从 checkpoint（bincode: (text, cola::EncodedReplica)）恢复 session。
        /// 替换已有 session，使两侧共享 CRDT state，跨副本 merge 才能工作。
        fn load_checkpoint(&self, py: Python<'_>, doc_id: u32, checkpoint_bytes: &[u8]) -> PyResult<()> {
            let store = self.inner.clone();
            let ckpt = checkpoint_bytes.to_vec();
            py.detach(move || {
                store.load_checkpoint(doc_id, &ckpt)
                    .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
            })
        }

        fn flush_worktree_sessions(&self, py: Python<'_>, uid: &str) -> PyResult<()> {
            let u = uid.to_string();
            py.detach(move || -> PyResult<()> { self.inner.flush_worktree(&u); Ok(()) })
        }
        fn offload_worktree(&self, py: Python<'_>, uid: &str) -> PyResult<()> {
            let u = uid.to_string();
            py.detach(move || -> PyResult<()> {
                let t = self.inner.tree.read(); let ds = t.collect_descendants(&u); drop(t);
                for d in &ds { self.inner.flush_worktree(d); }
                let be = self.inner.backends.read();
                for d in &ds { let t = self.inner.tree.read(); if let Some(n) = t.nodes.get(d) { if let Some(ref ep) = n.external_root { if let Some(b) = be.get(d) { b.offload(&self.inner.vfs_op, ep, &n.storage_root).ok(); } } } drop(t); }
                drop(be);
                self.inner.drop_worktree(&ds).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
            })
        }
        fn tree_ensure(&self) -> PyResult<()> { self.inner.tree.write().ensure(&self.inner); Ok(()) }
        fn restore_project_worktrees(&self) -> PyResult<()> {
            self.inner.restore_project_worktrees().map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
        }
        fn list_worktrees(&self) -> Vec<(String, String, String, Option<String>)> {
            self.inner.list_worktrees().into_iter().map(|w|
                (w.uid, w.name, w.storage_root, w.external_root)).collect()
        }
        fn register_hook_by_uid(&self, uid: &str, vfs_path: &str, ir_dict: &Bound<'_, pyo3::types::PyAny>, cb_code: &str, ctx_dict: &Bound<'_, pyo3::types::PyAny>, is_always: bool) -> PyResult<u64> {
            let doc_id = self.inner.resolve_path(uid, vfs_path, false).map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
            let ir: SelectorIR = pythonize::depythonize(ir_dict).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let ctx_val: serde_json::Value = pythonize::depythonize(ctx_dict).map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let hook_id = self.inner.register_hook(uid, doc_id, ir, cb_code, ctx_val, is_always);
            if hook_id == 0 {
                return Err(pyo3::exceptions::PyIOError::new_err(format!(
                    "hook register failed (session open): uid={} path={}", uid, vfs_path)));
            }
            Ok(hook_id)
        }
        fn has_worktree(&self, uid: &str) -> bool { self.inner.has_worktree(uid) }
        fn rule_remote_entries(&self, uid: &str) -> Vec<(String, String)> {
            self.inner.rule_remote_entries(uid)
        }
        fn rule_add_remote(&self, uid: &str, vfs_path: &str, entry_id: &str) -> PyResult<()> {
            self.inner.rule_add_remote(uid, vfs_path, entry_id)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
        }
        fn rule_remove_remote(&self, uid: &str, vfs_path: &str) -> PyResult<()> {
            self.inner.rule_remove_remote(uid, vfs_path)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
        }
        fn parent_uid(&self, uid: &str) -> Option<String> {
            self.inner.tree.read().parent_of(uid).map(|s| s.to_string())
        }

        #[cfg(feature = "telemetry")]
        #[pyo3(signature = (name, mode, max_events=None, max_meta_pool=None,
            bench_samples_per_phase=None, max_layer_samples=None))]
        fn telemetry_session_create(
            &self,
            name: &str,
            mode: &str,
            max_events: Option<usize>,
            max_meta_pool: Option<usize>,
            bench_samples_per_phase: Option<usize>,
            max_layer_samples: Option<usize>,
        ) -> PyResult<String> {
            let tm = match mode {
                "bench" => crate::telemetry::TelemetryMode::Bench,
                _ => crate::telemetry::TelemetryMode::Trace,
            };
            let mut cfg = crate::telemetry::TelemetryConfig::from_env();
            if let Some(v) = max_events { cfg.max_events = v; }
            if let Some(v) = max_meta_pool { cfg.max_meta_pool = v; }
            if let Some(v) = bench_samples_per_phase {
                cfg.max_bench_samples_per_phase = v;
            }
            if let Some(v) = max_layer_samples { cfg.max_layer_samples = v; }
            let inner = Arc::new(crate::telemetry::TelemetryInner::new_with_config(
                name.to_string(),
                tm,
                cfg,
            ));
            *self.inner.telemetry_session.lock() = Some(inner.clone());
            self.inner.telemetry_active.store(true, Ordering::Relaxed);
            Ok(name.to_string())
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_span_enter(&self, name: &str) -> PyResult<()> {
            let guard = self.inner.telemetry_session.lock();
            if let Some(ref sess) = *guard {
                crate::telemetry::py_span_enter(sess, name);
            }
            Ok(())
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_span_exit(&self, name: &str, meta: Option<&Bound<'_, pyo3::types::PyDict>>) -> PyResult<()> {
            let guard = self.inner.telemetry_session.lock();
            if let Some(ref sess) = *guard {
                let mut vec: Vec<(String, String)> = Vec::new();
                if let Some(d) = meta {
                    for (k, v) in d.iter() {
                        let Ok(k) = k.extract::<String>() else { continue };
                        let v = if let Ok(v) = v.extract::<String>() {
                            v
                        } else if let Ok(v) = v.extract::<i64>() {
                            v.to_string()
                        } else if let Ok(v) = v.extract::<f64>() {
                            v.to_string()
                        } else if let Ok(v) = v.extract::<bool>() {
                            v.to_string()
                        } else {
                            continue
                        };
                        vec.push((k, v));
                    }
                }
                crate::telemetry::py_span_exit(sess, name, vec);
            }
            Ok(())
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_session_destroy(&self) -> PyResult<()> {
            // 真关闭: 释放 store 持有的 session + 运行时开关
            *self.inner.telemetry_session.lock() = None;
            self.inner.telemetry_active.store(false, Ordering::Relaxed);
            Ok(())
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_report(&self) -> PyResult<String> {
            let guard = self.inner.telemetry_session.lock();
            match *guard {
                Some(ref sess) => {
                    let term = crate::telemetry::report_all(sess, ".telemetry");
                    Ok(term)
                }
                None => Ok("No active telemetry session.".into()),
            }
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_report_trace_json(&self) -> PyResult<String> {
            let guard = self.inner.telemetry_session.lock();
            match *guard {
                Some(ref sess) => Ok(crate::telemetry::report_json_trace(sess)),
                None => Ok("{\"traceEvents\":[]}".into()),
            }
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_session_mode(&self) -> PyResult<String> {
            let guard = self.inner.telemetry_session.lock();
            match *guard {
                Some(ref sess) => Ok(match sess.mode {
                    crate::telemetry::TelemetryMode::Trace => "trace".into(),
                    crate::telemetry::TelemetryMode::Bench => "bench".into(),
                }),
                None => Ok(String::new()),
            }
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_merge_js_data(&self, json_str: &str) -> PyResult<()> {
            let guard = self.inner.telemetry_session.lock();
            if let Some(ref sess) = *guard {
                crate::telemetry::merge_js_data(sess, json_str);
            }
            Ok(())
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_get_layer_stats(&self) -> PyResult<String> {
            let guard = self.inner.telemetry_session.lock();
            match *guard {
                Some(ref sess) => Ok(crate::telemetry::export_layer_stats_json(sess)),
                None => Ok("[]".into()),
            }
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_export_phase_json(&self) -> PyResult<String> {
            let guard = self.inner.telemetry_session.lock();
            match *guard {
                Some(ref sess) => Ok(crate::telemetry::export_phase_json(sess)),
                None => Ok("[]".into()),
            }
        }

        #[cfg(feature = "telemetry")]
        fn telemetry_session_active(&self) -> PyResult<bool> {
            Ok(self.inner.telemetry_session.lock().is_some())
        }
    }

    // ── SNAP FFI: crypto / diff / parse (single source of truth) ──

    #[pyfunction]
    fn snap_state_hash(content: &str) -> String {
        crate::snap::state_hash(content)
    }

    #[pyfunction]
    fn snap_hash_ffi(parent: &str, author: &str, message: &str, at: &str, file_states: &str) -> String {
        crate::snap::snap_hash(parent, author, message, at, file_states)
    }

    #[pyfunction]
    fn snap_diff_ffi(text_a: &str, text_b: &str) -> String {
        crate::snap::snap_diff(text_a, text_b)
    }

    #[pyfunction]
    fn snap_parse_all(snap_md_text: &str) -> (String, String) {
        let (entries, heads) = crate::snap::parse_all_snaps(snap_md_text);
        let entries_json = serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string());
        let heads_json = serde_json::to_string(&heads).unwrap_or_else(|_| "{}".to_string());
        (entries_json, heads_json)
    }

    #[pyfunction]
    fn snap_parse_ckpt(ckpt_text: &str, state_hash: &str) -> Option<String> {
        crate::snap::parse_ckpt_entry(ckpt_text, state_hash)
            .and_then(|e| match e {
                crate::snap::CkptEntry::Full { content, .. } => Some(content),
                crate::snap::CkptEntry::Delta { .. } => None,
            })
    }

    /// Apply a unified diff patch to reconstruct content.
    #[pyfunction]
    fn snap_apply_ffi(before: &str, diff_text: &str) -> Option<String> {
        crate::snap::snap_apply(before, diff_text).ok()
    }

    /// Parse a ckpt section and return the full entry info as JSON.
    /// Returns (path, at, kind, content_or_delta_from, body) as JSON tuple.
    #[pyfunction]
    fn snap_parse_ckpt_full(ckpt_text: &str, state_hash: &str) -> Option<String> {
        let entry = crate::snap::parse_ckpt_entry(ckpt_text, state_hash)?;
        let json = match entry {
            crate::snap::CkptEntry::Full { path, at, content } => {
                serde_json::json!(["full", path, at, content])
            }
            crate::snap::CkptEntry::Delta { path, at, delta_from, patch } => {
                serde_json::json!(["delta", path, at, delta_from, patch])
            }
        };
        Some(json.to_string())
    }

    /// 将 FileOp JSON 编码为 bincode 字节（供 Python 测试构造 UP_FILE 帧）。
    #[pyfunction]
    fn encode_file_op_ffi(op_json: &str) -> Option<Vec<u8>> {
        let op: crate::protocol::FileOp = serde_json::from_str(op_json).ok()?;
        bincode::serde::encode_to_vec(op, crate::store::bincode_cfg()).ok()
    }

    #[pyclass(frozen)]
    pub struct PySnapshot {
        pub inner: Arc<MdioSnapshot>,
    }

    #[pymethods]
    impl PySnapshot {
        fn node_count(&self) -> usize {
            crate::selector::all_nodes(&self.inner).len()
        }

        fn render_all(&self) -> String {
            self.inner.render_md_all(crate::topology::ProjectionMode::Hs)
        }

        fn render_selected(&self, ids: Vec<i64>) -> PyResult<String> {
            let all = crate::selector::all_nodes(&self.inner);
            let nodes = ids.iter()
                .map(|id| {
                    if *id < 0 {
                        return Err(pyo3::exceptions::PyValueError::new_err("negative node id"));
                    }
                    all.get(*id as usize)
                        .cloned()
                        .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("node id out of range"))
                })
                .collect::<PyResult<Vec<_>>>()?;
            self.inner.render_md_selected(crate::topology::ProjectionMode::Hs, &nodes)
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(format!("ARROW_MD_RENDER: {e}")))
        }

        fn facade(&self) -> PyResult<PyFacade> {
            let state = crate::arrow::state::FacadeState::new(self.inner.clone())
                .map_err(|e| pyo3::exceptions::PyRuntimeError::new_err(e.to_string()))?;
            Ok(PyFacade { inner: state })
        }
    }

    #[pyclass(frozen)]
    pub struct PyFacade {
        pub inner: Arc<crate::arrow::state::FacadeState>,
    }

    #[pymethods]
    impl PyFacade {
        fn node_count(&self) -> usize {
            self.inner.nodes.len()
        }

        fn render_all(&self) -> String {
            self.inner.render_all()
        }

        fn render_selected(&self, ids: Vec<i64>) -> PyResult<String> {
            self.inner.render_selected(&ids)
                .map_err(pyo3::exceptions::PyRuntimeError::new_err)
        }

        fn face(&self, name: &str) -> PyResult<Vec<String>> {
            let face = crate::arrow::facade::Face::from_name(name)
                .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("unknown face"))?;
            Ok(self.inner.face_values(face))
        }
    }

    #[pymodule(gil_used = false)]
    fn mdio_core(m: &Bound<'_, PyModule>) -> PyResult<()> {
        m.add_class::<PyProjectStore>()?;
        m.add_class::<PyServerAction>()?;
        m.add_class::<PySnapshot>()?;
        m.add_class::<PyFacade>()?;
        m.add("__version__", env!("CARGO_PKG_VERSION"))?;
        m.add_function(wrap_pyfunction!(snap_state_hash, m)?)?;
        m.add_function(wrap_pyfunction!(snap_hash_ffi, m)?)?;
        m.add_function(wrap_pyfunction!(snap_diff_ffi, m)?)?;
        m.add_function(wrap_pyfunction!(snap_parse_all, m)?)?;
        m.add_function(wrap_pyfunction!(snap_parse_ckpt, m)?)?;
        m.add_function(wrap_pyfunction!(snap_apply_ffi, m)?)?;
        m.add_function(wrap_pyfunction!(snap_parse_ckpt_full, m)?)?;
        m.add_function(wrap_pyfunction!(encode_file_op_ffi, m)?)?;
        Ok(())
    }

    /// S0 禁止删除清单 keep-alive：pyo3 导出面（跨进程契约面）。
    /// 只能写在本模块内——PyProjectStore 与 8 个 #[pyfunction] 条目均为模块私有
    /// （跨模块不可命名，实测 E0603）；Python 侧另有 dir() 快照判官
    /// mdio_py/tests/test_pyo3_api_surface.py。删任一被导出名 ⇒ 本测试编译红。
    #[cfg(all(test, feature = "python"))]
    mod keepalive {
        #[test]
        fn pyo3_surface() {
            // 8 个 #[pyfunction]（Rust 侧零调用，Python 侧才是消费者）
            let _ = super::snap_state_hash;
            let _ = super::snap_hash_ffi;
            let _ = super::snap_diff_ffi;
            let _ = super::snap_parse_all;
            let _ = super::snap_parse_ckpt;
            let _ = super::snap_apply_ffi;
            let _ = super::snap_parse_ckpt_full;
            let _ = super::encode_file_op_ffi;
            // ProjectStore（pymethods）：含三个全仓零引用导出，最易被误删
            let _ = super::PyProjectStore::apply_edit_selector;
            let _ = super::PyProjectStore::force_flush_file;
            let _ = super::PyProjectStore::handle_packet;
            let _ = super::PyProjectStore::write_locator;
            let _ = super::PyProjectStore::to_ast_json;
            // PyServerAction（帧侧 getter，9 个全覆盖——[小修] 原先只钉 2 个）
            let _ = super::PyServerAction::is_broadcast;
            let _ = super::PyServerAction::is_direct;
            let _ = super::PyServerAction::is_subscribe;
            let _ = super::PyServerAction::is_unsubscribe;
            let _ = super::PyServerAction::channel;
            let _ = super::PyServerAction::data;
            let _ = super::PyServerAction::is_file_action;
            let _ = super::PyServerAction::file_action_entry_id;
            let _ = super::PyServerAction::file_action_op_json;
        }
    }
}
