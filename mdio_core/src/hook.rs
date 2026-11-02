
// HookIndex — PathIndex fusion

//
// Synchronous HookIndex. Shadow Worker thread removed entirely.
// Lifecycle management via synchronous HookIndex ops.
// Python callback scheduling via retained FFI Dispatcher thread.
//
// Detection is scope-grouped and uses selector::select_nodes as the single
// source of truth for hook targets. Pruning layers:
//   1. all_hooks      — per-doc hook enumeration (skip documents with no hooks)
//   2. Spatial pruning — skip hook groups whose scope_range misses the ripple zone

use ahash::AHashMap as HashMap;
use std::hash::{Hash, Hasher};

use ahash::AHasher;

use crate::engine::MdioSnapshot;
use crate::selector::Step;
use crate::block::{
    BlockType, BlockNode,
    FrameKind,
    entry_line, entry_is_pop, entry_is_body_mut, entry_frame,
};


// Types


pub type HookId = u64;

#[derive(Debug, Clone)]
pub struct HookEvent {
    pub hook_id: HookId,
    pub cb_code: String,
    pub ctx_val: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct HookCache {
    pub combined_hash: u64,
}

// 组级 scope 缓存语义与 Drill 键已平移至匹配引擎（match.rs）；
// 此处再导出保持既有调用面（hook 内部 + matcher/测试）不变。
pub use crate::r#match::{compute_scope_range, prefix_paths, CachedScope, DrillKey};

#[derive(Debug, Clone)]
pub struct HookDef {
    pub hook_id: HookId,
    pub wt_uid: String,
    pub doc_id: u32,
    /// 注册生效水位：snap_seq < active_from 的任务快照早于本 hook 生效，必须完全跳过。
    pub active_from: u64,
    pub steps: Vec<Step>,
    pub is_always: bool,
    pub cb_code: String,
    pub ctx_val: serde_json::Value,
    pub cached: Option<HookCache>,
    pub hit_range: Option<(usize, usize)>,
    /// hit_range 对应的文档行数，用于跨编辑 δ 偏移。
    pub hit_line_count: u32,
    /// 完整 Dril 前缀链的最后一个锚点行（若可解析）。
    /// 用于“编辑在目标之后”的剪枝守卫：祖先锚点被重命名时不能跳。
    pub full_prefix_anchor_line: Option<usize>,
    pub registered_prefixes: Vec<Vec<DrillKey>>,
    /// 静态界：pre(S) 锚点 header 的 section 区间 [L, E)。
    /// None = 全局（无 Dril 前缀）或不可解。
    pub scope_range: Option<(usize, usize)>,
    /// scope_range 对应的文档行数，用于跨编辑 δ 偏移。
    pub scope_line_count: u32,
}


// HookIndex


pub struct HookIndex {
    all_hooks: HashMap<u32, Vec<HookId>>,
    hooks: HashMap<HookId, HookDef>,
    group_scope_cache: HashMap<(u32, Vec<String>), CachedScope>,
}

impl HookIndex {
    pub fn new() -> Self {
        Self {
            all_hooks: HashMap::new(),
            hooks: HashMap::new(),
            group_scope_cache: HashMap::new(),
        }
    }


    // Registration


    pub fn register(
        &mut self, hook_id: HookId, wt_uid: &str, doc_id: u32,
        steps: &[Step], is_always: bool, cb_code: &str,
        ctx_val: serde_json::Value, active_from: u64, snap: &MdioSnapshot,
    ) {
        let prefixes = prefix_paths(steps);
        self.all_hooks.entry(doc_id).or_default().push(hook_id);

        let nodes = crate::selector::select_nodes(snap, steps);
        let cached = if nodes.is_empty() { None } else {
            Some(HookCache { combined_hash: combined_hash_from_nodes(&nodes, snap) })
        };
        let hit_range = if nodes.is_empty() { None } else {
            let min_l = nodes.iter().map(|n| n.start_line).min().unwrap_or(0);
            let max_l = nodes.iter().map(|n| n.end_line.saturating_sub(1)).max().unwrap_or(0);
            Some((min_l, max_l))
        };

        let scope_range = compute_scope_range(steps, snap);
        let scope_line_count = snap.line_len() as u32;
        // 完整前缀链的最后一个锚点行：与 hit_range 同版本，供 T1 跨编辑守卫。
        let full_prefix_anchor_line = prefixes.last()
            .filter(|p| !p.is_empty())
            .and_then(|p| {
                let full_steps: Vec<Step> = p.iter()
                    .map(|dk| Step::Drill(dk.0.clone()))
                    .collect();
                snap.select_nodes(&full_steps).first().map(|n| n.start_line)
            });

        // P28：注册时缓存每个非空前缀的组级 scope 与锚点行。
        for p in &prefixes {
            if p.is_empty() { continue; }
            let key: Vec<String> = p.iter().map(|d| d.0.clone()).collect();
            let steps: Vec<Step> = p.iter().map(|dk| Step::Drill(dk.0.clone())).collect();
            let range = compute_scope_range(&steps, snap);
            let (_, anchor_lines) = resolve_prefix_anchor_lines(p, snap);
            self.group_scope_cache.insert((doc_id, key), CachedScope {
                range,
                line_count: snap.line_len() as u32,
                anchor_lines,
            });
        }

        self.hooks.insert(hook_id, HookDef {
            hook_id, wt_uid: wt_uid.to_string(), doc_id, active_from, steps: steps.to_vec(), is_always,
            cb_code: cb_code.to_string(), ctx_val, cached, hit_range,
            hit_line_count: scope_line_count,
            full_prefix_anchor_line,
            registered_prefixes: prefixes,
            scope_range, scope_line_count,
        });
    }


    // Removal


    pub fn remove(&mut self, hook_id: HookId) -> Option<HookDef> {
        let def = self.hooks.remove(&hook_id)?;
        if let Some(list) = self.all_hooks.get_mut(&def.doc_id) {
            list.retain(|id| *id != hook_id);
            if list.is_empty() { self.all_hooks.remove(&def.doc_id); }
        }
        Some(def)
    }

    pub fn clear_doc(&mut self, doc_id: u32, wt_uid: &str) {
        let to_remove: Vec<HookId> = self.all_hooks.get(&doc_id)
            .map(|v| v.iter().filter(|id| {
                self.hooks.get(id).map(|h| h.wt_uid == wt_uid).unwrap_or(false)
            }).copied().collect()).unwrap_or_default();
        for id in to_remove { self.remove(id); }
    }
    pub fn clear_worktree(&mut self, wt_uid: &str) {
        let to_remove: Vec<HookId> = self.hooks.values()
            .filter(|h| h.wt_uid == wt_uid).map(|h| h.hook_id).collect();
        for id in to_remove { self.remove(id); }
    }

    pub fn count_for_worktree(&self, wt_uid: &str) -> usize {
        self.hooks.values().filter(|h| h.wt_uid == wt_uid).count()
    }

    pub fn has_doc(&self, doc_id: u32) -> bool {
        self.all_hooks.contains_key(&doc_id)
    }


    // Detection — unified for all edit sources


    pub fn detect_crdt_write(&mut self, doc_id: u32, new_snap: &MdioSnapshot, ripple_start: usize, ripple_end: usize, ripple_frame_actions: &[u64]) -> Vec<HookEvent> {
        // 兼容入口（测试/旧调用）：水位取 u64::MAX，永不做注册前跳过。
        self.detect(doc_id, new_snap, ripple_start, ripple_end, ripple_frame_actions, true, 0, 0, false, u64::MAX)
    }
    pub fn detect_crdt_write_ex(&mut self, doc_id: u32, new_snap: &MdioSnapshot, ripple_start: usize, ripple_end: usize, ripple_frame_actions: &[u64], structural_change: bool, edit_start: usize, edit_end: usize, suppress_events: bool, snap_seq: u64) -> Vec<HookEvent> {
        self.detect(doc_id, new_snap, ripple_start, ripple_end, ripple_frame_actions, structural_change, edit_start, edit_end, suppress_events, snap_seq)
    }
    pub fn detect_selector_write(&mut self, _doc_id: u32, _write_steps: &[Step], new_snap: &MdioSnapshot, ripple_start: usize, ripple_end: usize, ripple_frame_actions: &[u64]) -> Vec<HookEvent> {
        self.detect(_doc_id, new_snap, ripple_start, ripple_end, ripple_frame_actions, true, 0, 0, false, u64::MAX)
    }
    pub fn detect_selector_write_ex(&mut self, _doc_id: u32, _write_steps: &[Step], new_snap: &MdioSnapshot, ripple_start: usize, ripple_end: usize, ripple_frame_actions: &[u64], structural_change: bool, edit_start: usize, edit_end: usize, suppress_events: bool, snap_seq: u64) -> Vec<HookEvent> {
        self.detect(_doc_id, new_snap, ripple_start, ripple_end, ripple_frame_actions, structural_change, edit_start, edit_end, suppress_events, snap_seq)
    }

    pub fn detect(
        &mut self, doc_id: u32, new_snap: &MdioSnapshot,
        ripple_start: usize, ripple_end: usize,
        _ripple_frame_actions: &[u64], // 事件路径暂不消费该信号
        structural_change: bool,
        edit_start: usize, edit_end: usize,
        suppress_events: bool,
        snap_seq: u64,
    ) -> Vec<HookEvent> {
        #[cfg(feature = "telemetry")] let _s_det = crate::telemetry_span!("hook_detect_work", "doc_id" => doc_id);
        let mut hook_ids: Vec<HookId> = self.all_hooks.get(&doc_id)
            .map(|v| v.clone()).unwrap_or_default();
        // 注册水位过滤：快照早于 hook 生效的任务对该 hook 完全不可见（不评估、不投递、
        // 也不走 maintain_* 剪枝维护）。过滤必须在分组/剪枝之前完成。
        hook_ids.retain(|id| self.hooks.get(id).map_or(false, |h| h.active_from <= snap_seq));
        if hook_ids.is_empty() { return vec![]; }

        let new_N = new_snap.line_len();
        let mut events = Vec::new();

        // Group hooks by second-to-last registered prefix so hooks that share
        // a section only pay the scope-range computation once.  The actual
        // hook target is always evaluated by select_nodes (the single selector
        // implementation); there is no per-hook step re-expansion here.
        let mut groups: HashMap<Vec<DrillKey>, Vec<HookId>> = HashMap::new();
        for &hook_id in &hook_ids {
            let hook = match self.hooks.get(&hook_id) { Some(h) => h, None => continue };
            let prefixes = &hook.registered_prefixes;
            let key = if prefixes.len() >= 2 {
                prefixes[prefixes.len() - 2].clone()
            } else {
                prefixes.first().cloned().unwrap_or(vec![])
            };
            groups.entry(key).or_default().push(hook_id);
        }

        for (prefix, group_hook_ids) in groups {
            #[cfg(feature = "telemetry")] let _s_grp = crate::telemetry_span!("hook_detect_group");
            if prefix.is_empty() {
                // Global hooks: no shared Dril resolution.
                for &hook_id in &group_hook_ids {
                    let can_skip = match self.hooks.get(&hook_id) {
                        Some(h) => if !structural_change {
                            // 纯内容编辑：编辑行外 ⟹ 目标 hash 不变（同组级路径）
                            edit_range_allows_skip(h, edit_start, edit_end)
                        } else {
                            ripple_start < ripple_end
                                && hit_range_allows_skip(
                                    h, ripple_start, ripple_end, new_N,
                                    structural_change, true, &[])
                        },
                        None => false,
                    };
                    if can_skip {
                        if let Some(h) = self.hooks.get_mut(&hook_id) {
                            maintain_hit_range(h, ripple_start, ripple_end, new_N);
                        }
                        continue;
                    }
                    let in_ripple = match self.hooks.get(&hook_id) {
                        Some(h) => {
                            if ripple_start >= ripple_end { true }
                            else {
                                match h.scope_range {
                                    None => true,
                                    Some((L, E)) => {
                                        let eff_E = if E == h.scope_line_count as usize && new_N > h.scope_line_count as usize {
                                            (E as isize + (new_N as isize - h.scope_line_count as isize)) as usize
                                        } else { E };
                                        if L >= ripple_end || eff_E <= ripple_start {
                                            if let Some(hook) = self.hooks.get_mut(&hook_id) {
                                                maintain_scope_range(hook, ripple_start, ripple_end, new_N);
                                            }
                                            false
                                        } else {
                                            true
                                        }
                                    }
                                }
                            }
                        },
                        None => false,
                    };
                    if in_ripple {
                        if let Some(event) = self.evaluate_and_dispatch(hook_id, new_snap, suppress_events, snap_seq) {
                            events.push(event);
                        }
                    }
                }
                continue;
            }

            // Non-empty prefix: resolve anchor lines first.
            // 缓存只用于“剪枝”；缓存失效时本次必须保守评估全组，
            // 但可以同时重算并更新缓存供后续编辑使用。
            let prefix_steps: Vec<Step> = prefix.iter()
                .map(|dk| Step::Drill(dk.0.clone()))
                .collect();
            let cache_key: Vec<String> = prefix.iter().map(|d| d.0.clone()).collect();
            let cached = self.group_scope_cache.get(&(doc_id, cache_key.clone())).cloned();
            // !structural_change（PL+FL 逐项相等）⟹ 行结构/标题位置全局不变
            // ⟹ 锚点行号与 scope 必然不变 ⟹ 缓存必然有效——无需 resolve 重算。
            // （resolve 仅用于验证缓存；!structural_change 时无需重算。）
            let cache_valid = cached.as_ref().map_or(false, |c| {
                c.line_count == new_N as u32 && !structural_change
            });
            let (anchors_complete, prefix_anchors) = if cache_valid {
                (true, cached.as_ref().unwrap().anchor_lines.clone())
            } else {
                #[cfg(feature = "telemetry")] let _s_an = crate::telemetry_span!("hd_anchor");
                let r = resolve_prefix_anchor_lines(&prefix, new_snap);
                #[cfg(feature = "telemetry")] drop(_s_an);
                r
            };
            let group_scope = if cache_valid {
                cached.as_ref().and_then(|c| c.range)
            } else {
                #[cfg(feature = "telemetry")] let _s_sc = crate::telemetry_span!("hd_scope");
                let range = compute_scope_range(&prefix_steps, new_snap);
                #[cfg(feature = "telemetry")] drop(_s_sc);
                self.group_scope_cache.insert((doc_id, cache_key), CachedScope {
                    range,
                    line_count: new_N as u32,
                    anchor_lines: prefix_anchors.clone(),
                });
                None
            };

            // Degenerate or empty ripple zone → conservative.
            if ripple_start < ripple_end {
                if let Some((L, E)) = group_scope {
                    if L >= ripple_end || E <= ripple_start {
                        for &hook_id in &group_hook_ids {
                            if let Some(h) = self.hooks.get_mut(&hook_id) {
                                maintain_scope_range(h, ripple_start, ripple_end, new_N);
                            }
                        }
                        continue;
                    }
                }
            }

            // Scope ∩ R ≠ ∅ — evaluate each hook with the full selector chain.
            // A hand-rolled step chain is incomplete: any Step variant it
            // doesn't cover stops the chain early, and fused TraversalFilter
            // steps fall through and hash the section root itself (two
            // same-prefix hooks both fire). select_nodes is the single
            // implementation for every Step variant.
            for &hook_id in &group_hook_ids {
                // !structural_change（纯内容编辑）⟹ 只有编辑行字节变 ⟹
                // hit_range 不含编辑行 ⟹ 目标 hash 必然不变 ⟹ 无需 evaluate。
                // 编辑行判断比 ripple（解析传播范围）精确得多——内容编辑的
                // ripple 会覆盖后续结构稳定行（heading+item 两行 conv 收敛），
                // 造成目标被保守 evaluate。
                // 守卫（dea13b8c 缺口修复——c5_always_anchor_shift/
                // heading_rename_anchor_guard 实证）：标题/锚点行文本编辑不
                // 改 PL/FL（structural_change=false）但漂移 Drill 锚点
                // （锚点行 ≠ hit_range 行——hit_range 在 ripple 内不更新），
                // edit_range 精确剪枝会漏触发。锚点命中判定：
                //   1) full_prefix_anchor_line（注册时完整链末级锚点——
                //      覆盖深一层锚点：组按倒数第二前缀分组，A2 级锚点
                //      不在组缓存里——heading_rename 实证）
                //   2) 组缓存 anchor_lines（最近一次 resolve——覆盖跨编辑
                //      锚点漂移——c5 实证）
                // 组 scope 不可作守卫（同前缀组共享大 scope——s2 实测 200
                // hook 全保守 1.18s 超时）；锚点行粒度既正确又保持剪枝性能。
                let edit_hits_anchor = cached.as_ref().map_or(false, |c| {
                    c.anchor_lines.iter().any(|&a| edit_start <= a && a < edit_end)
                });
                let can_skip = if !structural_change && cache_valid {
                    self.hooks.get(&hook_id).map_or(false, |h| {
                        let anchor_hit = h.full_prefix_anchor_line
                            .map_or(false, |a| edit_start <= a && a < edit_end)
                            || edit_hits_anchor;
                        !anchor_hit && edit_range_allows_skip(h, edit_start, edit_end)
                    })
                } else if !structural_change {
                    false
                } else {
                    ripple_start < ripple_end
                        && self.hooks.get(&hook_id).map_or(false, |h| {
                            hit_range_allows_skip(
                                h, ripple_start, ripple_end, new_N,
                                structural_change, anchors_complete, &prefix_anchors)
                        })
                };
                if can_skip {
                    if let Some(h) = self.hooks.get_mut(&hook_id) {
                        maintain_hit_range(h, ripple_start, ripple_end, new_N);
                    }
                    continue;
                }
                if let Some(event) = self.evaluate_and_dispatch(hook_id, new_snap, suppress_events, snap_seq) {
                    events.push(event);
                }
            }
        }

        events
    }


    // Core evaluation


    fn evaluate_and_dispatch(
        &mut self, hook_id: HookId, new_snap: &MdioSnapshot,
        suppress: bool, snap_seq: u64,
    ) -> Option<HookEvent> {
        #[cfg(feature = "telemetry")] let _s_ev = crate::telemetry_span!("hd_eval");
        let hook = self.hooks.get_mut(&hook_id)?;
        if hook.active_from > snap_seq {
            return None;
        }
        #[cfg(feature = "telemetry")] let _s_q = crate::telemetry_span!("hd_query");
        let nodes = crate::selector::select_nodes(new_snap, &hook.steps);
        #[cfg(feature = "telemetry")] drop(_s_q);

        let new_cache = if nodes.is_empty() { None } else {
            #[cfg(feature = "telemetry")] let _s_h = crate::telemetry_span!("hd_hash");
            let h = Some(HookCache { combined_hash: combined_hash_from_nodes(&nodes, new_snap) });
            #[cfg(feature = "telemetry")] drop(_s_h);
            h
        };

        let old_cache = hook.cached.clone();
        let triggered = match &old_cache {
            None => !nodes.is_empty(),
            Some(old) => new_cache.is_none()
                || old.combined_hash != new_cache.as_ref().unwrap().combined_hash,
        };

        // suppress 消化：只消化内容变化；存在性变化两个方向冻结
        // （None→Some 保持 None、Some→None 保持 Some——出现/消失留给
        // 非 suppress 评估触发）。非 suppress 照常整值更新。
        if suppress {
            if old_cache.is_some() && new_cache.is_some() {
                hook.cached = new_cache;
            }
        } else {
            hook.cached = new_cache;
        }
        hook.hit_range = if nodes.is_empty() { None } else {
            let min_l = nodes.iter().map(|n| n.start_line).min().unwrap_or(0);
            let max_l = nodes.iter().map(|n| n.end_line.saturating_sub(1)).max().unwrap_or(0);
            Some((min_l, max_l))
        };
        hook.hit_line_count = new_snap.line_len() as u32;

        // suppress：检测与消化已完成，投递被抑制；一次性 hook 不 remove。
        if suppress {
            return None;
        }

        let is_always = hook.is_always;
        let cb_code = if triggered { hook.cb_code.clone() } else { Default::default() };
        let ctx_val = if triggered { hook.ctx_val.clone() } else { Default::default() };

        if triggered {
            if !is_always { self.remove(hook_id); }
            Some(HookEvent { hook_id, cb_code, ctx_val })
        } else { None }
    }
}


// DrillKey / prefix_paths 已平移至匹配引擎（match.rs），此处再导出。



// Algorithm: write_path


pub fn write_path(steps: &[Step]) -> Vec<DrillKey> {
    let mut path = Vec::new();
    for step in steps {
        match step {
            Step::Drill(name) => path.push(DrillKey(name.clone())),
            _ => break,
        }
    }
    path
}


// Algorithm: combined_hash


pub fn combined_hash_from_nodes(
    nodes: &[BlockNode], snap: &MdioSnapshot,
) -> u64 {
    let mut h = AHasher::default();
    for node in nodes {
        snap.read_block_proj(node, crate::block::BlockProj::Content).hash(&mut h);
        snap.read_block_proj(node, crate::block::BlockProj::Tag).hash(&mut h);
        (node.node_type as u8).hash(&mut h);
    }
    h.finish()
}


// Algorithm: maintain_scope_range（hit 级维护——hook 私有，留在此处）


/// hit_range 在当前新文档行数下的有效区间。
/// 与 maintain_scope_range 同律：目标完全在 ripple 之后 → 整体平移 δ；
/// 目标在旧文档尾部且文档增长 → 扩展 end。
fn effective_hit_range(
    hook: &HookDef,
    rs: usize,
    re: usize,
    new_line_count: usize,
) -> Option<(usize, usize)> {
    let (hL, hE) = hook.hit_range?;
    if new_line_count == hook.hit_line_count as usize {
        return Some((hL, hE));
    }
    let delta = new_line_count as isize - hook.hit_line_count as isize;
    if hL >= re {
        Some((
            ((hL as isize + delta).max(0)) as usize,
            ((hE as isize + delta).max(0)) as usize,
        ))
    } else if hE < rs && hE == hook.hit_line_count as usize && delta > 0 {
        Some((hL, (hE as isize + delta) as usize))
    } else {
        Some((hL, hE))
    }
}

/// 纯内容编辑（structural_change == false）的精确剪枝谓词：
/// PL+FL 逐项相等 ⟹ 行结构/类型/归属全局不变，rope 只改编辑行字节 ⟹
/// 目标 hash（Content+Tag 字节区间纯函数）只可能因编辑行而变。
/// hit_range 不含编辑行 ⟹ 目标必然未变 ⟹ 无需 evaluate。
/// 比 ripple 判断精确：内容编辑的 ripple 会覆盖后续"结构稳定两行"
/// （heading+item conv 收敛），造成目标被保守 evaluate。
fn edit_range_allows_skip(hook: &HookDef, es: usize, ee: usize) -> bool {
    let Some((hL, hE)) = hook.hit_range else { return false };
    // hit_range 是闭区间 [hL, hE]（含尾行——单行节点 (8,8) = 行 8）——
    // "完全在编辑前"必须严格小于（hE < es），否则编辑行 == hE 时误剪。
    hE < es || hL >= ee
}

/// T1 剪枝谓词。仅当编辑未改变 PL/FL（structural_change == false）时
/// 允许跳过：
///   - hit 完全在 ripple 之前：目标内容必不受后续编辑影响；
///   - hit 完全在 ripple 之后：仅当 ripple 未触碰任何组前缀锚点时安全
///     （否则编辑可能改写了 hook 的 Drill 路径祖先）。
/// anchors_complete=false 表示某个前缀锚点在新文档已不可解析，保守不跳。
fn hit_range_allows_skip(
    hook: &HookDef,
    rs: usize,
    re: usize,
    new_line_count: usize,
    structural_change: bool,
    anchors_complete: bool,
    anchor_lines: &[usize],
) -> bool {
    if structural_change {
        return false;
    }
    let Some((hL, hE)) = effective_hit_range(hook, rs, re, new_line_count) else {
        return false;
    };
    if hE < rs {
        return true;
    }
    if hL >= re {
        if !anchors_complete {
            return false;
        }
        // 完整前缀链的最后一个锚点如果落在 ripple 内（例如标题被重命名），
        // 目标虽然在其后，但 Drill 路径可能已断裂——保守评估。
        if let Some(a) = hook.full_prefix_anchor_line {
            let delta = new_line_count as isize - hook.hit_line_count as isize;
            let eff_a = if a >= re { ((a as isize + delta).max(0)) as usize } else { a };
            if eff_a >= rs && eff_a < re {
                return false;
            }
        }
        return !anchor_lines.iter().any(|&a| a >= rs && a < re);
    }
    false
}

/// 跳过后维护 hit_range（与 maintain_scope_range 同款 δ 规则）。
fn maintain_hit_range(
    hook: &mut HookDef,
    rs: usize,
    re: usize,
    new_line_count: usize,
) {
    if let Some((hL, hE)) = hook.hit_range {
        let delta = new_line_count as isize - hook.hit_line_count as isize;
        if hL >= re {
            let new_L = ((hL as isize + delta).max(0)) as usize;
            let new_E = ((hE as isize + delta).max(0)) as usize;
            hook.hit_range = Some((new_L, new_E.min(new_line_count)));
        } else if hE < rs && hE == hook.hit_line_count as usize && delta > 0 {
            hook.hit_range = Some((hL, (hE as isize + delta) as usize));
        }
        hook.hit_line_count = new_line_count as u32;
    }
}

/// 解析组前缀的每一级锚点行。锚点集合用于 T1 的“编辑在目标之后”安全
/// 守卫；anchors_complete=false 表示链中途断了。
fn resolve_prefix_anchor_lines(
    prefix: &[DrillKey],
    snap: &MdioSnapshot,
) -> (bool, Vec<usize>) {
    let mut anchors = Vec::with_capacity(prefix.len());
    for k in 1..=prefix.len() {
        let steps: Vec<Step> = prefix[..k]
            .iter()
            .map(|dk| Step::Drill(dk.0.clone()))
            .collect();
        let nodes = snap.select_nodes(&steps);
        if let Some(n) = nodes.first() {
            anchors.push(n.start_line);
        } else {
            return (false, anchors);
        }
    }
    (true, anchors)
}

/// Update scope_range after a skip. Shifts by δ when scope is entirely
/// after the ripple zone. Expands scope end when scope was at old doc end
/// and document grew (to catch zero→existing transitions from appended content).
fn maintain_scope_range(
    hook: &mut HookDef,
    rs: usize,
    re: usize,
    new_line_count: usize,
) {
    if let Some((L, E)) = hook.scope_range {
        let delta = new_line_count as isize - hook.scope_line_count as isize;
        if L >= re {
            let new_L = ((L as isize + delta).max(0)) as usize;
            let new_E = ((E as isize + delta).max(0)) as usize;
            hook.scope_range = Some((new_L, new_E.min(new_line_count)));
        } else if E <= rs && E == hook.scope_line_count as usize && delta > 0 {
            // scope was at old doc end, document grew → expand scope end
            hook.scope_range = Some((L, (E as isize + delta) as usize));
        }
        hook.scope_line_count = new_line_count as u32;
    }
}


// Algorithm: EventClass — classify FL entries for structural vs content changes


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvClass { C_info, I_info, T_info }

/// body_adm bits equality — used for PopPush classification.
/// B1-4：单源到 effect::same_container（结构身份 = 表推导的 adm 位）。
/// 语义差异（有意保留）：Table 体差异（confirmed/tentative）PopPush 对——
/// 旧 `_ => true` 判 I_info，same_container 判 C_info；唯一消费点
/// （scope 更新）把 C_info | I_info 同权处理，可观察行为不变。
/// 若未来消费方区分 C/I，需先裁决 Table 身份语义。
fn body_adm_eq(kind: FrameKind, a: u64, b: u64) -> bool {
    let fa = entry_frame(a);
    let fb = entry_frame(b);
    debug_assert!(fa.kind() == kind && fb.kind() == kind,
        "body_adm_eq 只应比较同 kind 的 PopPush 对");
    crate::effect::same_container(fa, fb)
}

/// Classify a single FL entry in its surrounding context.
pub fn classify_entry(entries: &[u64], idx: usize) -> EvClass {
    let e = entries[idx];
    if entry_is_body_mut(e) {
        let f = entry_frame(e);
        return if crate::effect::is_list_container(f.kind()) {
            if f.loose() { EvClass::T_info } else { EvClass::C_info }
        } else if matches!(f.kind(), FrameKind::CodeFence | FrameKind::Comment) {
            EvClass::T_info
        } else {
            EvClass::C_info
        };
    }
    let kind = entry_frame(e).kind();

    if entry_is_pop(e) {
        let is_poppush = entries[idx+1..].iter().any(|&pe| {
            crate::effect::is_poppush_kindonly(e, Some(pe))
        });
        if is_poppush {
            if let Some(&pe) = entries[idx+1..].iter().find(|&&pe| {
                crate::effect::is_poppush_kindonly(e, Some(pe))
            }) {
                return if body_adm_eq(kind, e, pe) { EvClass::I_info } else { EvClass::C_info };
            }
            return EvClass::C_info;
        }
        return match kind {
            FrameKind::CodeFence | FrameKind::Comment => EvClass::T_info,
            _ => EvClass::C_info,
        };
    }

    // Push entry
    let is_poppush = entries[..idx].iter().any(|&pe| {
        crate::effect::is_poppush_kindonly(pe, Some(e))
    });
    if is_poppush {
        if let Some(&pe) = entries[..idx].iter().find(|&&pe| {
            crate::effect::is_poppush_kindonly(pe, Some(e))
        }) {
            return if body_adm_eq(kind, pe, e) { EvClass::I_info } else { EvClass::C_info };
        }
        return EvClass::C_info;
    }
    match kind {
        FrameKind::CodeFence | FrameKind::Comment => EvClass::T_info,
        _ => EvClass::C_info,
    }
}


// hook 换源 gate(S⊇A 断言,预言机 = 块级节区间
// "目标 ⊆ 章节区间")。现状行级 scope 在反例集/随机对照下必须红
// (偏小 34.8% + None 19.2%——漏评估面),换块级节区间格后绿。

#[cfg(test)]
mod a3_hook_scope_gate {
    use super::*;
    use crate::block::{BlockProj, BlockType};
    use crate::projection::State;
    use crate::topology::HEADINGSECTION_TOPOLOGY;

    /// 全量口径节区间(身份单源:全量段 fold)。
    fn full_secs(snap: &MdioSnapshot) -> Vec<(usize, usize, u8, BlockType)> {
        let n = snap.line_len();
        let seg = crate::cache::Segment::build(snap, 0, n);
        crate::infer::section_intervals(
            &seg.blocks,
            &seg.records,
            &seg.arena,
            HEADINGSECTION_TOPOLOGY.heading_candidates,
            n,
        )
    }

    /// 覆盖某行的最深章节(deepest_section 语义:最近 start 且覆盖)。
    fn containing_section(secs: &[(usize, usize, u8, BlockType)], line: usize) -> Option<(usize, usize)> {
        secs.iter()
            .rev()
            .find(|&&(s, e, _, _)| s <= line && line < e)
            .map(|&(s, e, _, _)| (s, e))
    }

    /// 文档内全部可 Drill 标题的 (steps, 预言机章节区间) 列表——tag 与
    /// selector 的 Drill 匹配同源(match_node_name:strip_bq 全层剥 +
    /// extract_heading_text 单行提取),保证生产锚点可解析。
    fn heading_queries(snap: &MdioSnapshot) -> Vec<(Vec<Step>, (usize, usize))> {
        let n = snap.line_len();
        let seg = crate::cache::Segment::build(snap, 0, n);
        let secs = crate::infer::section_intervals(
            &seg.blocks,
            &seg.records,
            &seg.arena,
            HEADINGSECTION_TOPOLOGY.heading_candidates,
            n,
        );
        let mut out = Vec::new();
        for &(s, e, lv, t) in &secs {
            let tag = MdioSnapshot::with_line_str(snap.line_text(s), |ls| {
                let full =
                    std::str::from_utf8(crate::block::strip_bq(ls.as_bytes(), false)).unwrap_or(ls);
                crate::block::extract_heading_text(full).to_string()
            });
            if tag.is_empty() {
                continue;
            }
            out.push((vec![Step::Drill(tag.clone())], (s, e)));
            let _ = lv;
        }
        out
    }

    /// S⊇A 断言:生产 scope 必须存在、起点 = 锚标题行、终点 ≥ 预言机章节尾。
    fn assert_superset(
        doc_desc: &str,
        steps: &[Step],
        snap: &MdioSnapshot,
        secs: &[(usize, usize, u8, BlockType)],
    ) {
        let prod = compute_scope_range(steps, snap);
        let Some((ps, pe)) = prod else {
            // 锚点存在(块级可解析)而 scope 为 None = 漏评估面(现状 None 19.2%)。
            panic!("{doc_desc}: compute_scope_range returned None for resolvable prefix {steps:?}");
        };
        let oe = containing_section(secs, ps)
            .map(|(_, e)| e)
            .unwrap_or(snap.line_len());
        assert!(
            pe >= oe,
            "{doc_desc}: scope end {pe} must cover oracle section end {oe} (S⊇A violated) steps={steps:?}"
        );
    }

    /// 反例集(强制材料):BQ 内 ATX 标题、BQ 内 setext 标题、跨容器 BQ、
    /// 嵌套节——行级 PL 扫描在容器内标题/setext 下漏评估或错边界。
    #[test]
    fn a3_scope_superset_fixed_counterexamples() {
        let docs: &[(&str, &str)] = &[
            // BQ 内 ATX:行级 line_type_at 看不到容器内标题——行级版盲扫。
            ("bq-atx", "# Top\n\n> # InBq\n> bq body\n> bq body\n\n## S2\n\npara\n"),
            // BQ 内 setext:下划线行不是 header 行类型——行级版盲扫。
            ("bq-setext", "# Top\n\n> foo\n> ===\n> more\n> more\n\n## S2\n\npara\n"),
            // 跨容器 BQ:容器跨界 + 容器内标题。
            ("cross-bq", "# Top\n\n> bq\n> bq\n> - item\n> # InBq\n> tail\n\n## S2\n\npara\n"),
            // 嵌套节:层级结构。
            ("nested", "# A\n\n## B\n\ntext\n\n# C\n\n## D\n\ntext\n"),
        ];
        for (name, doc) in docs {
            let mut state = State::new(doc);
            let snap = state.snapshot();
            let secs = full_secs(&snap);
            for (steps, _oracle) in heading_queries(&snap) {
                assert_superset(name, &steps, &snap, &secs);
            }
        }
    }

    /// 随机对照:300 文档 × 全部可 Drill 标题——S⊇A 全绿。
    #[test]
    fn a3_scope_superset_random() {
        fn next_rand(state: &mut u64) -> u64 {
            *state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *state
        }
        let tokens: &[&str] = &[
            "plain paragraph\n",
            "# H\n",
            "## S\n",
            "### D\n",
            "> # inbq\n",
            "> foo\n> ===\n",
            "> bq line\n",
            "- a\n",
            "  - nested\n",
            "```rust\nlet x = 1;\n```\n",
            "\n",
            "foo\n===\n",
            "- a\n\n  cont\n",
        ];
        let mut rng: u64 = 0x9B3C_71F2_DEAD_0002;
        let mut checked = 0usize;
        for case in 0..300u64 {
            let mut doc = String::new();
            let line_count = 4 + (next_rand(&mut rng) % 12) as usize;
            for _ in 0..line_count {
                doc.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
            }
            let mut state = State::new(&doc);
            let snap = state.snapshot();
            let secs = full_secs(&snap);
            for (steps, _oracle) in heading_queries(&snap) {
                assert_superset(&format!("case={case} doc={doc:?}"), &steps, &snap, &secs);
                checked += 1;
            }
        }
        assert!(checked > 0, "random corpus must produce heading queries");
    }
}


// suppress digest 语义测试


#[cfg(test)]
mod suppress_digest_tests {
    use super::*;
    use crate::engine::MdioEngine;
    use crate::selector::{CmpOp, FilterExpr, SingleSlice, Step, TypeName, Value};
    use crate::block::BlockProj;

    fn kv_steps(key: &str) -> Vec<Step> {
        vec![
            Step::Drill("S".into()),
            Step::Wildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
            Step::Filter(FilterExpr::Cmp {
                proj: BlockProj::Tag,
                op: CmpOp::Eq,
                value: Value::String(key.to_string()),
            }),
            Step::Slice(vec![SingleSlice::Index(0)]),
        ]
    }

    fn ripple(engine: &MdioEngine) -> (usize, usize) {
        match &engine.state.ripple_diff {
            Some(d) => (d.ripple_line_start, d.ripple_line_end),
            None => (0, engine.line_len()),
        }
    }

    /// suppress 笔：不产事件、不 remove、消化内容变化；
    /// 随后非 suppress 笔只触发纯增量。
    #[test]
    fn suppress_digests_content_without_event_or_remove() {
        let text = "# S\n- k: v\n";
        let mut engine = MdioEngine::new(text);
        let old = engine.make_snapshot();
        let steps = kv_steps("k");
        let mut idx = HookIndex::new();
        idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &old);

        let v = text.find('v').unwrap();
        engine.edit(v, v + 1, "vv");
        let new = engine.make_snapshot();
        let (rs, re) = ripple(&engine);
        let events = idx.detect_selector_write_ex(
            0, &steps, &new, rs, re, &[], false, 1, 2, true, u64::MAX);
        assert!(events.is_empty(), "suppress 笔不得产事件: {:?}", events);
        assert_eq!(idx.count_for_worktree(""), 1, "一次性 hook 不得被 suppress 误杀");

        // 第二笔非 suppress：v -> vv 的增量在 suppress 笔已消化，
        // 此处 vv -> vvv 只触发一次。
        let vv = engine.byte_len() - 2; // 行尾 "vv\n" 的第一个 v
        engine.edit(vv, vv + 1, "vvv");
        let new2 = engine.make_snapshot();
        let (rs, re) = ripple(&engine);
        let events = idx.detect_selector_write_ex(
            0, &steps, &new2, rs, re, &[], false, 1, 2, false, u64::MAX);
        assert_eq!(events.len(), 1, "非 suppress 笔应触发纯增量");
        assert_eq!(idx.count_for_worktree(""), 0, "一次性 hook 触发后移除");
    }

    /// 存在性冻结：目标被流中删除时 cached 保持 Some，
    /// 消失事件留给非 suppress 评估。
    #[test]
    fn suppress_freezes_disappearance() {
        let text = "# S\n- k: v\n";
        let mut engine = MdioEngine::new(text);
        let old = engine.make_snapshot();
        let steps = kv_steps("k");
        let mut idx = HookIndex::new();
        idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &old);

        let bullet = text.find("- k:").unwrap();
        let end = bullet + "- k: v\n".len();
        engine.edit(bullet, end, "");
        let new = engine.make_snapshot();
        let (rs, re) = ripple(&engine);
        let events = idx.detect_selector_write_ex(
            0, &steps, &new, rs, re, &[], true, bullet, bullet, true, u64::MAX);
        assert!(events.is_empty(), "suppress 笔不得产消失事件");

        // 同一快照非 suppress 强制评估：消失必须触发一次（冻结证明）。
        let events = idx.detect_selector_write_ex(
            0, &steps, &new, rs, re, &[], true, bullet, bullet, false, u64::MAX);
        assert_eq!(events.len(), 1, "消失事件留给非 suppress 评估");
        assert_eq!(idx.count_for_worktree(""), 1, "is_always hook 不移除");
    }

    /// 存在性冻结：目标在流中出现时 cached 保持 None，
    /// 出现事件留给非 suppress 评估。
    #[test]
    fn suppress_freezes_appearance() {
        let text = "# S\n- a: va\n";
        let mut engine = MdioEngine::new(text);
        let old = engine.make_snapshot();
        let steps = kv_steps("nope");
        let mut idx = HookIndex::new();
        idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 0, &old);

        let at = text.len();
        engine.edit(at, at, "- nope: x\n");
        let new = engine.make_snapshot();
        let (rs, re) = ripple(&engine);
        let events = idx.detect_selector_write_ex(
            0, &steps, &new, rs, re, &[], true, at, at, true, u64::MAX);
        assert!(events.is_empty(), "suppress 笔不得产出现事件");

        // 同一快照非 suppress 强制评估：出现必须触发一次（冻结证明）。
        let events = idx.detect_selector_write_ex(
            0, &steps, &new, rs, re, &[], true, at, at, false, u64::MAX);
        assert_eq!(events.len(), 1, "出现事件留给非 suppress 评估");
    }

    /// 注册水位：注册前入队的旧快照（目标不存在）不得触发、不得改 cache；
    /// 注册后第一个任务（snap_seq == active_from）才评估。
    #[test]
    fn stale_snapshot_before_registration_is_skipped() {
        let text = "# S\n- k: v\n";
        let mut engine = MdioEngine::new(text);
        let registered = engine.make_snapshot();
        let steps = kv_steps("k");
        let mut idx = HookIndex::new();
        idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 3, &registered);

        // 旧快照：k 还不存在（模拟注册前入队的任务），snap_seq=2 < active_from=3。
        let mut stale_engine = MdioEngine::new("# S\n- a: 1\n");
        let stale = stale_engine.make_snapshot();
        let events = idx.detect_selector_write_ex(
            0, &steps, &stale, 0, stale.line_len(), &[], true, 0, stale.line_len(), false, 2);
        assert!(events.is_empty(), "注册前快照不得触发: {:?}", events);

        // 注册后的真实变化：v -> vv，snap_seq=3 == active_from ⇒ 恰好一次。
        let mut engine2 = MdioEngine::new(text);
        let v = text.find('v').unwrap();
        engine2.edit(v, v + 1, "vv");
        let new = engine2.make_snapshot();
        let (rs, re) = ripple(&engine2);
        let events = idx.detect_selector_write_ex(
            0, &steps, &new, rs, re, &[], true, rs, re, false, 3);
        assert_eq!(events.len(), 1, "注册后首个任务必须评估");
    }

    /// 注册水位：旧快照连 suppress 消化也不能做——否则会把旧状态写进 cached，
    /// 让后续真任务的判定失真。
    #[test]
    fn stale_suppress_does_not_digest_cache() {
        let text = "# S\n- k: v\n";
        let mut engine = MdioEngine::new(text);
        let registered = engine.make_snapshot();
        let steps = kv_steps("k");
        let mut idx = HookIndex::new();
        idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 3, &registered);

        // 旧 suppress 任务（snap_seq=2）：内容已变成 vv，但快照早于注册 ⇒ 必须完全跳过。
        let mut engine2 = MdioEngine::new(text);
        let v = text.find('v').unwrap();
        engine2.edit(v, v + 1, "vv");
        let changed = engine2.make_snapshot();
        let (rs, re) = ripple(&engine2);
        let events = idx.detect_selector_write_ex(
            0, &steps, &changed, rs, re, &[], true, rs, re, true, 2);
        assert!(events.is_empty(), "旧 suppress 不得投递");

        // 同一个 changed 快照以 snap_seq=3 非 suppress 再来：若前面消化了 cache，
        // 这里会不触发；正确实现应触发一次。
        let events = idx.detect_selector_write_ex(
            0, &steps, &changed, rs, re, &[], true, rs, re, false, 3);
        assert_eq!(events.len(), 1, "旧 suppress 不得污染 cached");
    }

    /// 注册后第一个任务（snap_seq == active_from）必须评估；早一个序号则不评估。
    #[test]
    fn first_post_registration_seq_is_evaluated() {
        let text = "# S\n- k: v\n";
        let mut engine = MdioEngine::new(text);
        let registered = engine.make_snapshot();
        let steps = kv_steps("k");
        let mut idx = HookIndex::new();
        idx.register(1, "", 0, &steps, true, "cb", serde_json::json!({}), 3, &registered);

        let mut engine2 = MdioEngine::new(text);
        let v = text.find('v').unwrap();
        engine2.edit(v, v + 1, "vv");
        let changed = engine2.make_snapshot();
        let (rs, re) = ripple(&engine2);

        let early = idx.detect_selector_write_ex(
            0, &steps, &changed, rs, re, &[], true, rs, re, false, 2);
        assert!(early.is_empty(), "snap_seq 早于水位不得评估");
        let first = idx.detect_selector_write_ex(
            0, &steps, &changed, rs, re, &[], true, rs, re, false, 3);
        assert_eq!(first.len(), 1, "snap_seq == active_from 必须评估");
    }
}
