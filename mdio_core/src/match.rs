//! 匹配引擎（match.rs）—— **hook match + patch match 的共享实现**。
//!
//! 装的是「卡点之前」的一切：前缀分组 · scope 计算/维护（组级）· 剪枝谓词 ·
//! 跨写结果缓存 · 求值共享（整链）。
//!
//! **卡点之后**的两条尾巴不在这里：
//!   * hook 尾巴（hash 对拍 → 事件 → FFI → Python 动作）——hook.rs；
//!   * match 尾巴（sens 短路 → ClaimMap → 逐 op 反解 → P 帧序列化）——incr.rs。
//!
//! 层位：消费层，与 hook.rs 平级。**不进 infer**（无状态原子层）；
//! **不进 cache**（消费算法不归核）。


use crate::block::{BlockNode, BlockProj};
use crate::incr::Patch;
use crate::projection::Snapshot;
use crate::selector::{normalize_steps, select_nodes, select_nodes_from, FilterExpr, Step};
use smallvec::SmallVec;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[cfg(test)]
thread_local! {
    /// 判官插桩：eval_shared 内部的选择器求值次数（线程本地，不受并行测试污染）。
    static EVAL_SEL_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn bump_eval_sel_calls(n: usize) {
    EVAL_SEL_CALLS.with(|c| c.set(c.get() + n));
}
#[cfg(test)]
fn eval_sel_calls() -> usize {
    EVAL_SEL_CALLS.with(|c| c.get())
}


// 1 前缀分组（自 hook.rs 平移——原文搬，零行为变化）


/// Drill 路径键（零成本 newtype，供 HashMap 键安全）。
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub struct DrillKey(pub String);

/// 前导 Drill 前缀链（遇非 Drill 步即断；无前缀 ⇒ 返回空链 `[[]]`）。
pub fn prefix_paths(steps: &[Step]) -> Vec<Vec<DrillKey>> {
    let mut out = Vec::new();
    let mut cur: Vec<DrillKey> = Vec::new();
    for step in steps {
        match step {
            Step::Drill(name) => {
                cur.push(DrillKey(name.clone()));
                out.push(cur.clone());
            }
            Step::Wildcard | Step::RecursiveWildcard | Step::RecursiveDrill(_)
            | Step::Filter(_) | Step::Slice(_) | Step::TraversalFilter { .. } => {
                if !cur.is_empty() && out.last() != Some(&cur) { out.push(cur.clone()); }
                break;
            }
        }
    }
    if out.is_empty() { out.push(vec![]); }
    out
}


// 2 scope 计算（自 hook.rs 平移——原文搬）


/// 前缀锚定的节区间：最浅可解析 Drill 前缀的锚点行 → 覆盖它的最深章节区间。
/// 无 Drill 前缀（全局）或不可解析 ⇒ None（D5 门据此拒绝注册）。
pub fn compute_scope_range(
    steps: &[Step],
    snap: &Snapshot,
) -> Option<(usize, usize)> {
    let prefixes = prefix_paths(steps);
    for prefix in prefixes.iter() {
        if prefix.is_empty() { continue; }
        let prefix_steps: Vec<Step> = prefix.iter()
            .map(|dk| Step::Drill(dk.0.clone()))
            .collect();
        let anchors = snap.select_nodes(&prefix_steps);
        if let Some(anchor) = anchors.first() {
            let start = anchor.start_line;
            // 身份投影单源律：块级节区间格供给。
            let cell = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
            let end = cell
                .secs_iter()
                .rev()
                .find(|&(s, e, _, _)| s <= start && start < e)
                .map(|(_, e, _, _)| e)
                .unwrap_or(snap.line_len());
            return Some((start, end));
        }
    }
    None
}


// 3 组级 scope 缓存语义 + 剪枝谓词（泛化件：hook 与 match 同权）


/// 组级 scope 缓存条目（P28 语义，自 hook.rs 平移类型）。
/// 有效条件 = 行数相同 && 非结构变化。
#[derive(Debug, Clone)]
pub struct CachedScope {
    pub range: Option<(usize, usize)>,
    pub line_count: u32,
    /// 该前缀链的每级锚点行（hook 的 T1 守卫用；match 暂不用）。
    pub anchor_lines: Vec<usize>,
}

/// 组级 scope 缓存：键 = (doc_id, 前缀链)。
#[derive(Default)]
pub struct ScopeCache {
    map: HashMap<(u32, Vec<String>), CachedScope>,
}

impl ScopeCache {
    pub fn new() -> Self { Self::default() }

    pub fn get(&self, doc_id: u32, key: &[String]) -> Option<&CachedScope> {
        self.map.get(&(doc_id, key.to_vec()))
    }

    pub fn put(&mut self, doc_id: u32, key: Vec<String>, v: CachedScope) {
        self.map.insert((doc_id, key), v);
    }

    pub fn clear_doc(&mut self, doc_id: u32) {
        self.map.retain(|(d, _), _| *d != doc_id);
    }

    /// 缓存有效判据（照抄 hook.rs detect 的 cache_valid）：行数一致 && 非结构变化。
    pub fn is_valid(c: &CachedScope, new_line_count: usize, structural_change: bool) -> bool {
        c.line_count == new_line_count as u32 && !structural_change
    }
}


// 4 求值共享（整链）


/// 步骤序列的规范键（分组 / 缓存用；Step 无 Hash/Eq）。
fn seq_key(steps: &[Step]) -> String {
    format!("{steps:?}")
}

/// 求值共享（整链）：按**完整 steps 序列**建 trie——
///   * 同序列：整链求值一次，全部订阅者共享同一节点集；
///   * 异序列：共享公共前缀（trie 同层节点只算一次）。
/// 求值沿用 `run_steps` 折叠律（`select_nodes_from` 与 `select_nodes` 同一实现），
/// 不复制 Step 语义。返回与 `seqs` 等长、逐序列对齐的节点集。
pub fn eval_shared(seqs: &[Vec<Step>], snap: &Snapshot) -> Vec<Vec<BlockNode>> {
    struct TrieNode {
        children: HashMap<String, (usize, Step)>,
        set: Option<Vec<BlockNode>>,
    }
    impl Default for TrieNode {
        fn default() -> Self { Self { children: HashMap::new(), set: None } }
    }

    // 唯一序列 → trie 叶节点
    let mut nodes: Vec<TrieNode> = vec![TrieNode::default()];
    let mut leaf_of_key: HashMap<String, usize> = HashMap::new();
    for steps in seqs {
        let key = seq_key(steps);
        if leaf_of_key.contains_key(&key) { continue; }
        let mut cur = 0usize;
        for st in steps {
            let tok = format!("{st:?}");
            let next = match nodes[cur].children.get(&tok) {
                Some(&(id, _)) => id,
                None => {
                    let id = nodes.len();
                    nodes.push(TrieNode::default());
                    nodes[cur].children.insert(tok, (id, st.clone()));
                    id
                }
            };
            cur = next;
        }
        leaf_of_key.insert(key, cur);
    }

    // 自根向叶逐层求值（根 = 文档根节点集；每步 = 对父集合续跑一步）。
    #[cfg(test)]
    let mut sel_calls = 0usize;
    nodes[0].set = Some(select_nodes(snap, &[]));
    #[cfg(test)]
    { sel_calls += 1; }
    let mut order: Vec<usize> = vec![0usize];
    let mut head = 0usize;
    while head < order.len() {
        let n = order[head];
        head += 1;
        let children: Vec<(usize, Step)> = nodes[n].children.values().cloned().collect();
        for (child, step) in children {
            let parent_set = nodes[n].set.clone().unwrap_or_default();
            let set = select_nodes_from(snap, &parent_set, std::slice::from_ref(&step));
            #[cfg(test)]
            { sel_calls += 1; }
            nodes[child].set = Some(set);
            order.push(child);
        }
    }
    #[cfg(test)]
    bump_eval_sel_calls(sel_calls);

    seqs.iter()
        .map(|s| {
            let leaf = leaf_of_key[&seq_key(s)];
            nodes[leaf].set.clone().unwrap_or_default()
        })
        .collect()
}


// match 表（订阅注册表 · match 面）
//
// 语义（D1–D6）：注册单位 = 地址(file_pattern) + 标签(tag)；file 段在注册期展开成
// doc_id 集，表按 doc_id 索引；不做配对（op 挂自己路径的认领者——REM 查旧快照、
// ADD/REPL 查新快照，见 incr::claim_ids）；优先级 = 注册序（升序，末位最高）；
// 拒绝无 Drill 前缀的注册（scope_range == None，D5）。求值一律走引擎 eval_shared。


/// 归属值 = tag（mod 名）按注册序升序。
pub type ClaimSet = SmallVec<[Arc<str>; 2]>;


// 字段掩码


/// 字段掩码。当前线路字段只有 `t` / `ct`（incr.rs:1612/1619，面级字段写 attr）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FieldMask(pub u8);

impl FieldMask {
    pub const T: u8 = 1 << 0;
    pub const CT: u8 = 1 << 1;

    #[inline]
    pub const fn from_bits(b: u8) -> Self {
        FieldMask(b)
    }
    /// wire 字段名 → 位；未知字段 ⇒ None（保守：不参与短路）。
    pub fn from_attr(attr: &str) -> Option<FieldMask> {
        match attr {
            "t" => Some(FieldMask(Self::T)),
            "ct" => Some(FieldMask(Self::CT)),
            _ => None,
        }
    }
    #[inline]
    pub fn set(&mut self, bits: u8) {
        self.0 |= bits;
    }
    #[inline]
    pub fn union(self, o: FieldMask) -> FieldMask {
        FieldMask(self.0 | o.0)
    }
    #[inline]
    pub fn intersects(self, o: FieldMask) -> bool {
        self.0 & o.0 != 0
    }
    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}


// sens 静态推导（注册期一次；在 normalize_steps 之后 —— 与执行形态一致）


fn proj_mask(proj: BlockProj, mask: &mut FieldMask, structural: &mut bool) {
    match proj {
        // 内容 / 标签面 → 线路字段 `t`（tag 面按渲染形态出值，WC-4）
        BlockProj::Content | BlockProj::Tag => mask.set(FieldMask::T),
        // 子通道 → 线路字段 `ct`
        BlockProj::Container => mask.set(FieldMask::CT),
        // Raw / Mark 无线路字段可写 ⇒ 保守标结构敏感（拒绝短路）
        BlockProj::Raw | BlockProj::Mark => {
            mask.set(FieldMask::T | FieldMask::CT);
            *structural = true;
        }
    }
}

fn filter_sens(f: &FilterExpr, mask: &mut FieldMask, structural: &mut bool) {
    match f {
        FilterExpr::Cmp { proj, .. }
        | FilterExpr::Regex { proj, .. }
        | FilterExpr::Method { proj, .. }
        | FilterExpr::By { proj, .. } => proj_mask(*proj, mask, structural),
        FilterExpr::TypeCheck(_) => *structural = true,
        FilterExpr::And(v) | FilterExpr::Or(v) => {
            for s in v {
                filter_sens(s, mask, structural);
            }
        }
        FilterExpr::Not(b) => filter_sens(b, mask, structural),
    }
}

/// IR → (敏感字段掩码, 结构敏感)。**Drill 只置 `t`（标题文本改写会打断 Drill 链），
/// 不置结构位**——否则 D5 放行的任何注册都永不短路（规格 N1 R3）。
/// 结构位只给位置/结构算子与 Raw/Mark 投影。
pub fn derive_sens(steps: &[Step]) -> (FieldMask, bool) {
    let steps = normalize_steps(steps);
    let mut mask = FieldMask::default();
    let mut structural = false;
    for s in &steps {
        match s {
            Step::Drill(_) => mask.set(FieldMask::T),
            Step::Wildcard | Step::RecursiveWildcard | Step::RecursiveDrill(_) | Step::Slice(_) => {
                structural = true
            }
            Step::Filter(f) => filter_sens(f, &mut mask, &mut structural),
            Step::TraversalFilter { traversal, filters, limit } => {
                match traversal.as_ref() {
                    Step::Drill(_) => mask.set(FieldMask::T),
                    _ => structural = true,
                }
                for f in filters {
                    filter_sens(f, &mut mask, &mut structural);
                }
                if *limit > 0 {
                    structural = true;
                }
            }
        }
    }
    (mask, structural)
}

/// 本帧「全字段写」⇒ 返回字段掩码；否则 None（不短路）。
/// 字段写 = **仅 ReplaceAttr**（成员位 `…/ct` 的 Add/Remove/Replace 算结构写）。
pub fn frame_field_mask(patches: &[Patch]) -> Option<FieldMask> {
    if patches.is_empty() {
        return None;
    }
    let mut mask = FieldMask::default();
    for p in patches {
        match p {
            Patch::ReplaceAttr { attr, .. } => {
                let m = FieldMask::from_attr(attr)?;
                mask = mask.union(m);
            }
            _ => return None,
        }
    }
    Some(mask)
}

/// ② 短路判据：本帧全字段写、该条不结构敏感、且字段不与 sens 相交 ⇒ 整条跳过。
pub fn sens_allows_skip(def: &MatchDef, frame: Option<FieldMask>) -> bool {
    match frame {
        None => false,
        Some(m) => !def.struct_sensitive && !m.intersects(def.sens),
    }
}


// 注册表


/// 一次 `register_match` = 一个模板（C5 补登记要 pattern/IR/tag）。
#[derive(Debug, Clone)]
pub struct RegMeta {
    pub reg_id: u64,
    pub wt_uid: Arc<str>,
    /// 核认 glob 形态（核永不看到 `.*`；归一在 mdio_py 声明层）。
    pub pattern: String,
    pub steps: Arc<[Step]>,
    pub tag: Arc<str>,
    pub sens: FieldMask,
    pub struct_sensitive: bool,
    pub wildcard: bool,
}

/// 一个 (注册, 文档) 一条定义。
#[derive(Debug, Clone)]
pub struct MatchDef {
    pub def_id: u64,
    pub reg_id: u64,
    pub wt_uid: Arc<str>,
    pub doc_id: u32,
    pub steps: Arc<[Step]>,
    pub tag: Arc<str>,
    /// 注册/补登记时 `compute_scope_range` 的结果（D5 门保证 Some）。
    pub scope: (usize, usize),
    /// 当前不读取。
    pub scope_line_count: u32,
    pub sens: FieldMask,
    pub struct_sensitive: bool,
}

/// U6：per-(doc, def) 的携带状态。nodes = Some(当前代新快照的选择集)；
/// 未触碰代行号可能陈旧，但认领只在触碰后发生（触碰则重算）⇒ 无害。
#[derive(Clone, Default)]
pub struct DefState {
    pub scope: (usize, usize),
    pub lc: u32,
    pub nodes: Option<Vec<BlockNode>>,
    /// 注册时 scope 解析失败的全文档回退（仅测试/防御面；生产 D5 拒绝 None）——
    /// refresh 对此态不做节区间收窄，保持 (0, line_len)。
    pub whole_doc: bool,
}

#[derive(Default)]
pub struct MatchRegistry {
    /// doc_id → [def_id]（按注册序；同 HookIndex.all_hooks）。
    pub by_doc: HashMap<u32, Vec<u64>>,
    /// def_id → def。
    pub defs: HashMap<u64, Arc<MatchDef>>,
    /// reg_id → 模板。
    pub regs: HashMap<u64, RegMeta>,
    /// 通配注册（reg_id 列表），供新文档补登记。
    pub wildcard: Vec<u64>,
    /// (reg_id, doc_id) 已登记索引 ⇒ needs_fanout O(1)。
    pub applied: HashSet<(u64, u32)>,
    /// U6：per-(doc, def) 携带状态——几何维护的 scope + 携带节点集。
    pub states: HashMap<(u32, u64), DefState>,
    next_id: u64,
}

impl MatchRegistry {
    pub fn new() -> Self {
        Self { next_id: 1, ..Default::default() }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// 建模板；返回 `reg_id`（恒非 0）。sens 在此静态推出。
    pub fn register_reg(
        &mut self,
        wt_uid: &str,
        pattern: &str,
        steps: Vec<Step>,
        tag: &str,
        wildcard: bool,
    ) -> u64 {
        let (sens, struct_sensitive) = derive_sens(&steps);
        let reg_id = self.alloc_id();
        let meta = RegMeta {
            reg_id,
            wt_uid: Arc::from(wt_uid),
            pattern: pattern.to_string(),
            steps: Arc::from(steps),
            tag: Arc::from(tag),
            sens,
            struct_sensitive,
            wildcard,
        };
        if wildcard {
            self.wildcard.push(reg_id);
        }
        self.regs.insert(reg_id, meta);
        reg_id
    }

    /// 为某文档登记该注册的一条 def（返回 def_id；reg 不存在 ⇒ None）。
    pub fn add_def(
        &mut self,
        reg_id: u64,
        doc_id: u32,
        scope: (usize, usize),
        scope_line_count: u32,
    ) -> Option<u64> {
        let meta = self.regs.get(&reg_id)?.clone();
        if self.applied.contains(&(reg_id, doc_id)) {
            return None;
        }
        let def_id = self.alloc_id();
        let def = MatchDef {
            def_id,
            reg_id,
            wt_uid: meta.wt_uid.clone(),
            doc_id,
            steps: meta.steps.clone(),
            tag: meta.tag.clone(),
            scope,
            scope_line_count,
            sens: meta.sens,
            struct_sensitive: meta.struct_sensitive,
        };
        self.defs.insert(def_id, Arc::new(def));
        self.by_doc.entry(doc_id).or_default().push(def_id);
        self.applied.insert((reg_id, doc_id));
        self.states.insert((doc_id, def_id), DefState { scope, lc: scope_line_count, nodes: None, whole_doc: false });
        Some(def_id)
    }

    /// 通配注册在该文档上是否还有未登记的模板（O(#wildcard)）。
    pub fn needs_fanout(&self, doc_id: u32, wt_uid: &str) -> bool {
        self.wildcard.iter().any(|rid| {
            !self.applied.contains(&(*rid, doc_id))
                && self.regs.get(rid).map_or(false, |m| &*m.wt_uid == wt_uid)
        })
    }

    /// C5 补登记：对该文档、该 worktree 的未登记通配模板逐条算 scope；
    /// `None` ⇒ **不登记也不标记**（前缀解析出来后仍可补上——正确性优先）。
    /// 返回本次新登记条数。调用方必须在 `task.snap` 可用处调用（Path/FJ worker）。
    pub fn ensure_doc(&mut self, doc_id: u32, wt_uid: &str, snap: &Snapshot) -> usize {
        let pending: Vec<u64> = self
            .wildcard
            .iter()
            .copied()
            .filter(|rid| {
                !self.applied.contains(&(*rid, doc_id))
                    && self.regs.get(rid).map_or(false, |m| &*m.wt_uid == wt_uid)
            })
            .collect();
        let mut n = 0usize;
        for reg_id in pending {
            let Some(meta) = self.regs.get(&reg_id) else { continue };
            let Some(scope) = crate::hook::compute_scope_range(&meta.steps, snap) else {
                continue;
            };
            let line_count = snap.line_len() as u32;
            let steps = meta.steps.clone();
            if let Some(def_id) = self.add_def(reg_id, doc_id, scope, line_count) {
                self.init_state(doc_id, def_id, scope, line_count, &steps, snap, false);
                n += 1;
            }
        }
        n
    }

    /// U6：状态初值（prefill 转正）——注册时把选择集算好带进状态（不是缓存）。
    /// 这次求值在 compute_scope_range 刚暖过的快照上只花十几 µs。
    pub fn init_state(
        &mut self,
        doc_id: u32,
        def_id: u64,
        scope: (usize, usize),
        lc: u32,
        steps: &[Step],
        snap: &Snapshot,
        whole_doc: bool,
    ) {
        let set = eval_shared(&[steps.to_vec()], snap);
        self.states.insert(
            (doc_id, def_id),
            DefState { scope, lc, nodes: Some(set.into_iter().next().unwrap_or_default()), whole_doc },
        );
    }

    /// U6：每写刷新——锚点行保持（注册初值 + 几何 δ 平移），**节尾从当前快照的
    /// section_cell 现取**。节内追加（节尾增长）由此天然覆盖；兄弟节互不相交。
    /// 代价 = O(#defs × log #secs) + section_cell 一次构建（每快照 OnceLock，HS adapt 已暖）。
    pub fn refresh_scopes(&mut self, doc_id: u32, snap: &Snapshot) {
        let cell = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
        for ((d, _), st) in self.states.iter_mut() {
            if *d != doc_id {
                continue;
            }
            if st.whole_doc {
                st.scope = (0, snap.line_len());
                st.lc = snap.line_len() as u32;
                continue;
            }
            let l = st.scope.0;
            let e = cell
                .secs_iter()
                .rev()
                .find(|&(s, e, _, _)| s <= l && l < e)
                .map(|(_, e, _, _)| e)
                .unwrap_or(snap.line_len());
            st.scope = (l, e.min(snap.line_len()));
            st.lc = snap.line_len() as u32;
        }
    }

    /// 每写维护——几何 δ 平移 scope；nodes 不动。
    pub fn maintain_scopes(&mut self, doc_id: u32, ripple: (usize, usize), new_lc: usize) {
        for ((d, _), st) in self.states.iter_mut() {
            if *d != doc_id {
                continue;
            }
            let delta = new_lc as isize - st.lc as isize;
            let (l, e) = st.scope;
            if l >= ripple.1 {
                st.scope = (
                    (l as isize + delta).max(0) as usize,
                    ((e as isize + delta).max(0) as usize).min(new_lc),
                );
            } else if e <= ripple.0 && e == st.lc as usize && delta > 0 {
                st.scope = (l, (e as isize + delta) as usize);
            }
            st.lc = new_lc as u32;
        }
    }

    /// U6：op 区间集是否触碰该 def 的 scope（严格半开相交；状态缺失 ⇒ 保守触碰）。
    /// scope 的节尾由 refresh_scopes 每写现取 ⇒ 节内追加（节尾增长）天然被覆盖，
    /// 兄弟节的写不相交 ⇒ 不误评。
    fn def_touched(st: Option<&DefState>, ivs: &[(usize, usize)]) -> bool {
        let Some(st) = st else { return true };
        let (l, e) = st.scope;
        ivs.iter().any(|&(a, b)| a < e && l < b)
    }

    /// U6：patch 驱动求值——只对触碰 def 求值（整链共享保留）；未触碰 ⇒ 空集
    /// （本帧零认领、零 eval）。求值后更新携带 nodes + 回推 scope（首锚点行 +
    /// section_cell 最深层节区间——与 compute_scope_range 同义，零二次 eval）。
    pub fn eval_touched(
        &mut self,
        doc_id: u32,
        defs: &[Arc<MatchDef>],
        snap: &Snapshot,
        op_ivs: &[(usize, usize)],
    ) -> Vec<Vec<BlockNode>> {
        let mut out: Vec<Vec<BlockNode>> = Vec::with_capacity(defs.len());
        let mut todo: Vec<usize> = Vec::new();
        for (i, d) in defs.iter().enumerate() {
            out.push(Vec::new());
            if Self::def_touched(self.states.get(&(doc_id, d.def_id)), op_ivs) {
                todo.push(i);
            }
        }
        if !todo.is_empty() {
            let seqs: Vec<Vec<Step>> = todo.iter().map(|&i| defs[i].steps.to_vec()).collect();
            let sets = eval_shared(&seqs, snap);
            for (k, &i) in todo.iter().enumerate() {
                out[i] = sets[k].clone();
                // nodes 更新；scope 由 refresh_scopes 每写现取（不在此回推）
                if let Some(st) = self.states.get_mut(&(doc_id, defs[i].def_id)) {
                    // park 判官：选择集 ⊆ 注册 scope——
                    // debug-only，防「同名兄弟插入锚点节之外」类 scope 剪枝 gap 在将来
                    // 复用核侧线时静默漏算；whole_doc（测试兜底）豁免。
                    if !st.whole_doc {
                        for n in &sets[k] {
                            debug_assert!(
                                st.scope.0 <= n.start_line && n.end_line <= st.scope.1,
                                "match 选择集越出 scope：def={} scope={:?} node=[{}, {})",
                                defs[i].def_id, st.scope, n.start_line, n.end_line
                            );
                        }
                    }
                    st.nodes = Some(sets[k].clone());
                }
            }
        }
        out
    }

    /// 该文档的全部 def（**按注册序**；调用方持锁取快照后立即释放）。
    pub fn defs_for_doc(&self, doc_id: u32) -> Vec<Arc<MatchDef>> {
        self.by_doc
            .get(&doc_id)
            .map(|ids| {
                ids.iter()
                    .filter_map(|id| self.defs.get(id).cloned())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn remove_def(&mut self, def_id: u64) {
        if let Some(def) = self.defs.remove(&def_id) {
            if let Some(list) = self.by_doc.get_mut(&def.doc_id) {
                list.retain(|id| *id != def_id);
                if list.is_empty() {
                    self.by_doc.remove(&def.doc_id);
                }
            }
            self.applied.remove(&(def.reg_id, def.doc_id));
            self.states.remove(&(def.doc_id, def_id));
        }
    }

    /// 注销一次注册（其全部 doc-def + 模板 + wildcard + applied 项）。
    pub fn remove_reg(&mut self, reg_id: u64) -> usize {
        let ids: Vec<u64> = self
            .defs
            .values()
            .filter(|d| d.reg_id == reg_id)
            .map(|d| d.def_id)
            .collect();
        for id in &ids {
            self.remove_def(*id);
        }
        self.regs.remove(&reg_id);
        self.wildcard.retain(|r| *r != reg_id);
        ids.len()
    }

    /// 清某文档上属于该 worktree 的 def（同 HookIndex.clear_doc）。
    pub fn clear_doc(&mut self, doc_id: u32, wt_uid: &str) {
        let ids: Vec<u64> = self
            .by_doc
            .get(&doc_id)
            .map(|v| {
                v.iter()
                    .filter(|id| {
                        self.defs.get(id).map_or(false, |d| &*d.wt_uid == wt_uid)
                    })
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        for id in ids {
            self.remove_def(id);
        }
    }

    /// 清该 worktree 的全部 def/模板/wildcard/applied（wt 卸载）。
    pub fn clear_worktree(&mut self, wt_uid: &str) {
        let ids: Vec<u64> = self
            .defs
            .values()
            .filter(|d| &*d.wt_uid == wt_uid)
            .map(|d| d.def_id)
            .collect();
        let regs: Vec<u64> = self
            .regs
            .values()
            .filter(|r| &*r.wt_uid == wt_uid)
            .map(|r| r.reg_id)
            .collect();
        for id in ids {
            self.remove_def(id);
        }
        for r in regs {
            self.regs.remove(&r);
            self.wildcard.retain(|w| *w != r);
        }
    }

    pub fn has_doc(&self, doc_id: u32) -> bool {
        self.by_doc.contains_key(&doc_id)
    }

    /// 注册表是否非空（defs ∪ wildcard 模板）。空 ⇒ 调用方走**零额外代价**老路径。
    pub fn is_active(&self) -> bool {
        !self.defs.is_empty() || !self.wildcard.is_empty()
    }

    pub fn count_for_worktree(&self, wt_uid: &str) -> usize {
        self.defs.values().filter(|d| &*d.wt_uid == wt_uid).count()
    }

    pub fn count_defs(&self) -> usize {
        self.defs.len()
    }
}

/// 同 tag 重复 ⇒ 保留**最高优先级位次**（list 已按注册序升序）。
pub fn dedupe_keep_last(v: &mut ClaimSet) {
    let mut i = 0;
    while i < v.len() {
        if v[i + 1..].iter().any(|t| *t == v[i]) {
            v.remove(i);
        } else {
            i += 1;
        }
    }
}


// F 侧归属查找（N5）


/// 区间 → 认领集（F 帧渲染期 O(1) 查询）。
#[derive(Default)]
pub struct ClaimMap {
    map: HashMap<(u32, u32), ClaimSet>,
}

impl ClaimMap {
    pub fn empty() -> Self {
        Self::default()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn get(&self, start: u32, end: u32) -> Option<&ClaimSet> {
        self.map.get(&(start, end))
    }
    /// 该文档全体 def 的归属映射（引擎**整链共享**求值：同序列一次、异序列共享前缀）。
    ///
    /// 分组/求值由匹配引擎 eval_shared 承担（trie：同序列整链一次，
    /// 异序列共享公共前缀），此处只做「区间 → 注册序 tag」组装。
    pub fn build(defs: &[Arc<MatchDef>], snap: &Snapshot) -> Self {
        if defs.is_empty() { return Self::default(); }
        let seqs: Vec<Vec<Step>> = defs.iter().map(|d| d.steps.to_vec()).collect();
        let sets = eval_shared(&seqs, snap);
        Self::from_sets(defs, sets)
    }

    /// 由引擎给出的逐 def 节点集组装归属映射（求值已在引擎侧完成——
    /// 剪枝 / 跨写缓存 / 整链共享的产出）。sets 必须与 defs 等长同序。
    pub fn from_sets(defs: &[Arc<MatchDef>], sets: Vec<Vec<BlockNode>>) -> Self {
        debug_assert_eq!(defs.len(), sets.len());
        // (def 下标, tag)：trie 迭代序不是注册序，收尾按注册序排序再载入。
        let mut map: HashMap<(u32, u32), Vec<(usize, Arc<str>)>> = HashMap::new();
        for (i, nodes) in sets.into_iter().enumerate() {
            for n in nodes {
                map.entry((n.start_line as u32, n.end_line as u32))
                    .or_default()
                    .push((i, defs[i].tag.clone()));
            }
        }
        let mut out: HashMap<(u32, u32), ClaimSet> = HashMap::with_capacity(map.len());
        for (k, mut v) in map {
            v.sort_by_key(|(idx, _)| *idx); // 注册序（D4：升序，末位最高）
            let mut cs: ClaimSet = v.into_iter().map(|(_, t)| t).collect();
            dedupe_keep_last(&mut cs);
            out.insert(k, cs);
        }
        Self { map: out }
    }
}

impl crate::incr::ClaimLookup for ClaimMap {
    fn claims(&self, start: u32, end: u32) -> Option<&[Arc<str>]> {
        self.map.get(&(start, end)).map(|v| v.as_slice())
    }
    fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}



// match 表 / 尾巴辅助 判官


// 单元判官


#[cfg(test)]
mod tests {
    use super::*;
    use crate::selector::{CmpOp, FilterMethod, SingleSlice, TypeName, Value};

    #[test]
    fn sens_table_matches_spec() {
        // Drill → {t}，不置结构位（R3）
        let (m, s) = derive_sens(&[Step::Drill("A".into())]);
        assert_eq!(m, FieldMask(FieldMask::T));
        assert!(!s, "Drill must not be structural");
        // Drill + Content 过滤 → 仍 {t}
        let (m, s) = derive_sens(&[
            Step::Drill("A".into()),
            Step::Filter(FilterExpr::Cmp {
                proj: BlockProj::Content,
                op: CmpOp::Eq,
                value: Value::String("x".into()),
            }),
        ]);
        assert_eq!(m, FieldMask(FieldMask::T));
        assert!(!s);
        // Container 投影 → {ct}
        let (m, s) = derive_sens(&[
            Step::Drill("A".into()),
            Step::Filter(FilterExpr::Method { proj: BlockProj::Container, method: FilterMethod::Empty }),
        ]);
        assert_eq!(m, FieldMask(FieldMask::T | FieldMask::CT));
        assert!(!s);
        // 位置/结构算子
        for st in [
            Step::Wildcard,
            Step::RecursiveWildcard,
            Step::RecursiveDrill("A".into()),
            Step::Slice(vec![SingleSlice::Index(2)]),
            Step::Filter(FilterExpr::TypeCheck(TypeName::Paragraph)),
        ] {
            let (_, s) = derive_sens(&[Step::Drill("A".into()), st]);
            assert!(s, "variant must be structural");
        }
        // TraversalFilter: limit=0 不结构，limit>0 结构
        let (_, s0) = derive_sens(&[Step::Drill("A".into()), Step::TraversalFilter {
            traversal: Box::new(Step::Drill("B".into())),
            filters: vec![],
            limit: 0,
        }]);
        assert!(!s0);
        let (_, s1) = derive_sens(&[Step::Drill("A".into()), Step::TraversalFilter {
            traversal: Box::new(Step::Drill("B".into())),
            filters: vec![],
            limit: 2,
        }]);
        assert!(s1);
        // Raw/Mark → 结构敏感
        let (_, sr) = derive_sens(&[
            Step::Drill("A".into()),
            Step::Filter(FilterExpr::Cmp {
                proj: BlockProj::Raw,
                op: CmpOp::Eq,
                value: Value::String("x".into()),
            }),
        ]);
        assert!(sr);
        // And/Or/Not 取并集
        let (m, s) = derive_sens(&[
            Step::Drill("A".into()),
            Step::Filter(FilterExpr::Or(vec![
                FilterExpr::Cmp { proj: BlockProj::Content, op: CmpOp::Eq, value: Value::String("x".into()) },
                FilterExpr::Not(Box::new(FilterExpr::TypeCheck(TypeName::Bullet))),
            ])),
        ]);
        assert_eq!(m, FieldMask(FieldMask::T));
        assert!(s, "TypeCheck inside Not must mark structural");
    }

    #[test]
    fn frame_field_mask_only_replace_attr() {
        let ra_t = Patch::ReplaceAttr { path: "/ct/0/t".into(), attr: "t".into(), value: "\"x\"".into() };
        let ra_ct = Patch::ReplaceAttr { path: "/ct/0/ct/0".into(), attr: "ct".into(), value: "[1]".into() };
        assert_eq!(frame_field_mask(&[ra_t.clone(), ra_ct.clone()]), Some(FieldMask(FieldMask::T | FieldMask::CT)));
        // 成员位 Add / 结构 op ⇒ None（不短路）
        assert_eq!(frame_field_mask(&[Patch::Add { path: "/ct/0/ct".into(), value: "[]".into() }]), None);
        assert_eq!(frame_field_mask(&[ra_t.clone(), Patch::Remove { path: "/ct/1".into() }]), None);
        // 未知 attr ⇒ None
        assert_eq!(frame_field_mask(&[Patch::ReplaceAttr { path: "/x".into(), attr: "zz".into(), value: "1".into() }]), None);
        assert_eq!(frame_field_mask(&[]), None);
    }

    #[test]
    fn sens_skip_predicate() {
        let def = MatchDef {
            def_id: 1, reg_id: 1, wt_uid: Arc::from("genesis"), doc_id: 0,
            steps: Arc::from(vec![Step::Drill("A".into())]),
            tag: Arc::from("m"), scope: (0, 0), scope_line_count: 0,
            sens: FieldMask(FieldMask::T), struct_sensitive: false,
        };
        // 纯 ct 写 ⇒ sens {t} 不含 ct ⇒ 跳过
        assert!(sens_allows_skip(&def, Some(FieldMask(FieldMask::CT))));
        // t 写 ⇒ 不跳
        assert!(!sens_allows_skip(&def, Some(FieldMask(FieldMask::T))));
        // 非纯字段写帧 ⇒ 不跳
        assert!(!sens_allows_skip(&def, None));
        // 结构敏感 ⇒ 永不跳
        let mut d2 = def.clone();
        d2.struct_sensitive = true;
        assert!(!sens_allows_skip(&d2, Some(FieldMask(FieldMask::CT))));
    }

    #[test]
    fn dedupe_keeps_highest_priority_position() {
        let mut v: ClaimSet = SmallVec::new();
        v.push(Arc::from("a")); v.push(Arc::from("b")); v.push(Arc::from("a"));
        dedupe_keep_last(&mut v);
        let got: Vec<&str> = v.iter().map(|s| &**s).collect();
        assert_eq!(got, vec!["b", "a"]);
    }

    #[test]
    fn registry_lifecycle_and_wildcard_index() {
        let mut r = MatchRegistry::new();
        let reg = r.register_reg("genesis", "**", vec![Step::Drill("A".into())], "modA", true);
        assert!(reg != 0);
        assert!(r.needs_fanout(7, "genesis"));
        assert!(!r.needs_fanout(7, "other"));       // wt 不匹配
        assert_eq!(r.add_def(reg, 7, (0, 3), 10).is_some(), true);
        assert_eq!(r.add_def(reg, 7, (0, 3), 10), None);   // 幂等（applied）
        assert!(!r.needs_fanout(7, "genesis"));
        assert_eq!(r.count_for_worktree("genesis"), 1);
        assert_eq!(r.defs_for_doc(7).len(), 1);
        assert_eq!(&*r.defs_for_doc(7)[0].tag, "modA");
        // 注销后零残留
        assert_eq!(r.remove_reg(reg), 1);
        assert_eq!(r.count_defs(), 0);
        assert!(r.defs_for_doc(7).is_empty());
        assert!(!r.has_doc(7));
        assert!(!r.needs_fanout(7, "genesis"));
        assert_eq!(r.count_for_worktree("genesis"), 0);
    }

    #[test]
    fn ensure_doc_fanout_gates_by_scope_and_wt() {
        use crate::projection::State;
        let snap = State::new("# A\n\ntext\n").snapshot();
        let mut r = MatchRegistry::new();
        let reg_hit = r.register_reg("genesis", "**", vec![Step::Drill("A".into())], "hit", true);
        let _reg_miss = r.register_reg("genesis", "**", vec![Step::Drill("NOPE".into())], "miss", true);
        let _reg_other = r.register_reg("wt2", "**", vec![Step::Drill("A".into())], "other", true);
        assert_eq!(r.ensure_doc(9, "genesis", &snap), 1, "only resolvable prefix registers");
        let defs = r.defs_for_doc(9);
        assert_eq!(defs.len(), 1);
        assert_eq!(&*defs[0].tag, "hit");
        assert!(r.needs_fanout(9, "genesis"), "unresolved template stays pending (retry later)");
        // 二次调用不重复登记（已登记 + 未解析者仍 pending，但不会再加 def）
        assert_eq!(r.ensure_doc(9, "genesis", &snap), 0);
        assert_eq!(r.defs_for_doc(9).len(), 1);
        // clear_doc / clear_worktree
        r.clear_doc(9, "genesis");
        assert_eq!(r.defs_for_doc(9).len(), 0);
        assert_eq!(r.ensure_doc(9, "genesis", &snap), 1);
        r.clear_worktree("genesis");
        assert_eq!(r.count_defs(), 0);
        assert_eq!(r.defs_for_doc(9).len(), 0);
        assert!(r.regs.values().all(|m| &*m.wt_uid == "wt2"));
        // 空 IR（无 Drill）⇒ 无 scope ⇒ 永不登记
        let mut r2 = MatchRegistry::new();
        let reg_empty = r2.register_reg("genesis", "**", vec![], "e", true);
        assert_eq!(r2.ensure_doc(1, "genesis", &snap), 0);
        assert!(r2.defs_for_doc(1).is_empty());
        let _ = reg_empty;
    }

    #[test]
    fn grouped_claim_map_shares_prefix_evaluation() {
        use crate::projection::State;
        use std::collections::HashMap;
        let snap = State::new("# A\n\nx one\n\n# B\n\ny two\n").snapshot();
        let mut r = MatchRegistry::new();
        for tag in ["m1", "m2", "m3"] {
            let reg = r.register_reg(
                "g",
                "**",
                vec![Step::Drill("A".into()), Step::Wildcard],
                tag,
                false,
            );
            r.add_def(reg, 0, (0, usize::MAX), 0);
        }
        let defs = r.defs_for_doc(0);
        // 整链共享：同序列 3 条 ⇒ root + A + Wildcard = 3 次（未共享 ≥ 9）
        EVAL_SEL_CALLS.with(|c| c.set(0));
        let grouped = ClaimMap::build(&defs, &snap);
        assert_eq!(eval_sel_calls(), 3, "同序列整链共享：root + 步数");
        // 等价性：逐条独立求值必须与分组逐区间一致
        let mut per_def: HashMap<(u32, u32), ClaimSet> = HashMap::new();
        for d in &defs {
            for n in crate::selector::select_nodes(&snap, &d.steps) {
                per_def
                    .entry((n.start_line as u32, n.end_line as u32))
                    .or_default()
                    .push(d.tag.clone());
            }
        }
        for v in per_def.values_mut() {
            dedupe_keep_last(v);
        }
        assert_eq!(grouped.map, per_def, "分组求值必须与逐条求值一致");
        // 不同前缀不串组：A 组与 B 组各自区间都在
        let mut r2 = MatchRegistry::new();
        let ra = r2.register_reg("g", "**", vec![Step::Drill("A".into()), Step::Wildcard], "a", false);
        let rb = r2.register_reg("g", "**", vec![Step::Drill("B".into()), Step::Wildcard], "b", false);
        r2.add_def(ra, 0, (0, usize::MAX), 0);
        r2.add_def(rb, 0, (0, usize::MAX), 0);
        EVAL_SEL_CALLS.with(|c| c.set(0));
        let map2 = ClaimMap::build(&r2.defs_for_doc(0), &snap);
        assert!(map2.len() >= 2, "A/B 两组各自区间都在");
        // 异序列共享公共前缀：root + B 组 = root + A + Wildcard + B + Wildcard = 5
        assert_eq!(eval_sel_calls(), 5, "A/B 异序列共享 root");
    }

    #[test]
    fn claim_map_intervals_and_order() {
        use crate::projection::State;
        let snap = State::new("# A\n\ntext\n\n# B\n\nx\n").snapshot();
        let mut r = MatchRegistry::new();
        let r1 = r.register_reg("genesis", "**", vec![Step::Drill("A".into())], "m1", false);
        let r2 = r.register_reg("genesis", "**", vec![Step::Drill("B".into())], "m2", false);
        let r3 = r.register_reg("genesis", "**", vec![Step::Drill("A".into())], "m3", false);
        r.add_def(r1, 3, (0, 0), 0);
        r.add_def(r2, 3, (0, 0), 0);
        r.add_def(r3, 3, (0, 0), 0);
        let defs = r.defs_for_doc(3);
        let map = ClaimMap::build(&defs, &snap);
        // A 的区间由 m1+m3 认领（注册序），B 由 m2
        let a: Vec<&str> = map.map.values().filter(|v| v.iter().any(|t| &**t == "m1")).flat_map(|v| v.iter().map(|s| &**s)).collect();
        assert!(a.contains(&"m3"), "A interval carries both tags: {:?}", a);
        assert!(map.len() >= 2, "A and B intervals, got {}", map.len());
    }
}



// 引擎判官


// 判官


#[cfg(test)]
mod engine_tests {
    use super::*;
    use crate::projection::State;

    fn snap(text: &str) -> Snapshot { State::new(text).snapshot() }
    fn doc() -> Snapshot { snap("# A\n\nx one\n\n## B\n\ny two\n\n# C\n\nz three\n") }
    fn drill(n: &str) -> Step { Step::Drill(n.into()) }

    // ── 1 前缀分组 ──
    #[test]
    fn prefix_paths_semantics() {
        let a = prefix_paths(&[drill("A"), drill("B")]);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0][0].0, "A");
        assert_eq!(a[1][1].0, "B");
        // 非 Drill 步断链
        let w = prefix_paths(&[drill("A"), Step::Wildcard]);
        assert_eq!(w.len(), 1);
        // 无前缀 ⇒ 空链（D5 门据此拒绝）
        assert_eq!(prefix_paths(&[Step::Wildcard]), vec![vec![]]);
    }

    // ── 2 scope 计算 ──
    #[test]
    fn compute_scope_is_section_of_anchor() {
        let s = doc();
        let (l, e) = compute_scope_range(&[drill("A")], &s).expect("A resolvable");
        assert_eq!(l, 0);
        // A 的章节延伸到 C 之前
        assert!(e > 0 && e <= s.line_len(), "e={e}");
        // 不可解析 ⇒ None；无前缀 ⇒ None
        assert!(compute_scope_range(&[drill("NoSuch")], &s).is_none());
        assert!(compute_scope_range(&[Step::Wildcard], &s).is_none());
    }

    // ── 4 求值共享（整链）：oracle 等价 + 共享计数 ──
    #[test]
    fn eval_shared_matches_oracle() {
        let s = doc();
        let seqs = vec![
            vec![drill("A")],
            vec![drill("A"), drill("B")],
            vec![drill("C")],
            vec![drill("A"), Step::Wildcard],
        ];
        let got = eval_shared(&seqs, &s);
        for (i, sq) in seqs.iter().enumerate() {
            let want = select_nodes(&s, sq);
            let g: Vec<(usize, usize)> = got[i].iter().map(|n| (n.start_line, n.end_line)).collect();
            let w: Vec<(usize, usize)> = want.iter().map(|n| (n.start_line, n.end_line)).collect();
            assert_eq!(g, w, "seq {i} must equal oracle");
        }
    }

    #[test]
    fn eval_shared_merges_identical_and_shared_prefix() {
        let s = doc();
        // 3 条同序列 ⇒ trie = root + A + B + Wildcard = 4 次求值（非 3×整链）
        EVAL_SEL_CALLS.with(|c| c.set(0));
        let seq = vec![drill("A"), drill("B")];
        let got = eval_shared(&[seq.clone(), seq.clone(), seq.clone()], &s);
        assert_eq!(eval_sel_calls(), 3, "root + A + B");
        assert_eq!(got[0].len(), got[1].len());
        assert_eq!(got[1].len(), got[2].len());
        // 异序列共享前缀：单独跑 [A] 与 [A,B] 合计 = root+A+B = 3
        EVAL_SEL_CALLS.with(|c| c.set(0));
        let _ = eval_shared(&[vec![drill("A")], vec![drill("A"), drill("B")]], &s);
        assert_eq!(eval_sel_calls(), 3, "shared prefix evaluated once");
    }

    // ── U6：patch 驱动短路（触碰跳过 / 几何维护 / 零求值断言）──
    #[test]
    fn u6_touch_skip_and_maintain() {
        let mut state = State::new("# A\n\nx one\n\n# B\n\ny two\n");
        let snap0 = Arc::new(state.snapshot());
        let steps = vec![Step::Drill("A".into())];
        let mut r = MatchRegistry::new();
        let rid = r.register_reg("g", "**", steps.clone(), "m", false);
        let scope = crate::hook::compute_scope_range(&steps, &snap0).expect("scope");
        let lc0 = snap0.line_len() as u32;
        let did = r.add_def(rid, 0, scope, lc0).unwrap();
        r.init_state(0, did, scope, lc0, &steps, &snap0, false);
        // 写：在 B 节内改（结构写，不触碰 A 节 scope）
        let b_byte = state.rope.to_string().find("y two").unwrap();
        state.edit(b_byte, b_byte, "y three");
        let snap1 = Arc::new(state.snapshot());
        let diff = state.ripple_diff.take().expect("ripple");
        r.maintain_scopes(0, (diff.ripple_line_start, diff.ripple_line_end), snap1.line_len());
        let st_scope0 = r.states.get(&(0, did)).unwrap().scope.0;
        let st_scope1 = r.states.get(&(0, did)).unwrap().scope.1;
        assert!(st_scope0 >= scope.0, "scope 起点不得前移");
        // 不触碰：op 区间在 B 节 ⇒ 零求值、空集
        let b_iv = vec![(4usize, 6usize)];
        EVAL_SEL_CALLS.with(|c| c.set(0));
        let defs = r.defs_for_doc(0);
        let sets = r.eval_touched(0, &defs, &snap1, &b_iv);
        assert_eq!(eval_sel_calls(), 0, "U6：不触碰 ⇒ 零求值");
        assert!(sets[0].is_empty(), "U6：不触碰 ⇒ 空集");
        // 触碰：op 区间落在 A 节 ⇒ 求值 + nodes 更新 + scope 回推
        let a_iv = vec![(0usize, 2usize)];
        EVAL_SEL_CALLS.with(|c| c.set(0));
        let sets2 = r.eval_touched(0, &defs, &snap1, &a_iv);
        assert!(eval_sel_calls() > 0, "U6：触碰 ⇒ 求值");
        assert!(!sets2[0].is_empty(), "U6：触碰 ⇒ 非空选择集");
        // ④ 尾增长：在 A 节内追加一行（A 的节尾增长）→ refresh 后 scope 覆盖 ⇒ 触碰
        let a_body = state.rope.to_string().find("x one").unwrap();
        state.edit(a_body, a_body, "x zero\n");
        let snap2 = Arc::new(state.snapshot());
        let diff2 = state.ripple_diff.take().expect("ripple2");
        r.maintain_scopes(0, (diff2.ripple_line_start, diff2.ripple_line_end), snap2.line_len());
        r.refresh_scopes(0, &snap2);
        let stg = r.states.get(&(0, did)).unwrap();
        assert!(stg.scope.1 > st_scope1, "U6：节内追加 ⇒ scope 尾增长");
        let grow_iv = vec![(3usize, 4usize)];
        EVAL_SEL_CALLS.with(|c| c.set(0));
        let sets3 = r.eval_touched(0, &defs, &snap2, &grow_iv);
        assert!(eval_sel_calls() > 0, "U6：节内追加 ⇒ 触碰求值");
        let st2 = r.states.get(&(0, did)).unwrap();
        let iv_a: Vec<(usize, usize)> = st2.nodes.as_ref().unwrap().iter().map(|n| (n.start_line, n.end_line)).collect();
        let iv_b: Vec<(usize, usize)> = sets3[0].iter().map(|n| (n.start_line, n.end_line)).collect();
        assert_eq!(iv_a, iv_b, "U6：携带 nodes 更新");
        assert_eq!(st2.scope.0, sets2[0][0].start_line, "U6：scope 回推 = 首锚点行");
    }
}




