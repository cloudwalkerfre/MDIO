//! MDIO Selector — render 剪枝的块跳形态(行块统一——判定在行,消费在块)
//!
//! 选择器实现(唯一基准 py 契约 292):
//! 消费 scan 产物链(PL/FL——不重新解析)+ infer 现成设施——零自造判定:
//!   块起点   = BlockRec.start（seg.blocks 升序 + 区间过滤——V3 起不再
//!              逐行判定/重建 view arrays）
//!   块类型   = BlockRec.block_type（BQ 内容/伪表回退/setext 全由
//!              build_block_records 的 context_resolve 单一来源）
//!   块尾     = ContainerRec 直读(record.end 含 closing 语义——item 用
//!              item_subtree_end)+ 标题章节区间 min 容器截断 + §6 懒缓存
//!   枚举类别 = node_category(None, i, t)(flat 模式——side_effect + Skip 表)
//!   推进     = 直接子块跳 i=end;后代标题穿入 i+=1(章节内容独立枚举)
//!   匹配     = peel 族(match_node_name——find_colon_or_bracket_tag 官方/
//!              tag_eq 快路径)
//! 行级 resolved 缺口 → zone 重建路径;
//!   当前无消费方,未接(不假装实现)。

use crate::telemetry_span;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use crate::block::{BlockProj, BlockNode, BlockType};
use crate::projection::Snapshot;
// FrameSpans 不作为生产视图——node_category 的 spans 参数 &dyn 化（op 升级后删桥）。
use crate::block::FrameKind;

// ── 节点类别判定（D6 契约归属：自 infer 迁入——Comment 例外语义）──

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NodeCategory { Block, Skip }

/// fdc_pass2 内部成本桶(探查记账——首读 pass2 14.5µs 的细分归因;
/// feature OFF 时无测量调用点写入,字段恒 0,零开销)。桶语义:
///   li  = line_identity(行身份链推导)
///   idn = identity_nodes 整体(含内部 block_end_resolve 与匹配物化——
///         与 bre/mn/bn 嵌套,分析时不得与子桶相加)
///   bre = 推进线 block_end_resolve(每发射块一次)
///   bpn = 二分跳 partition_point(嵌套块组跳过)
///   mn  = match_node_name(目标名匹配——Drill 读每候选一次)
///   bn  = build_node(匹配命中的节点物化——行几何测量)
#[derive(Default)]
struct P2Breakdown {
    li_ns: u64,
    idn_ns: u64,
    bre_ns: u64,
    bpn_ns: u64,
    mn_ns: u64,
    bn_ns: u64,
    /// 探查：identity_nodes 内部的 block_end_resolve（内容路径每块一次——
    /// 旧 bre 桶只统计外层推进调用，导致 ~440ns/块未归因）。
    bre_inner_ns: u64,
    bre_inner_calls: u64,
    iter: u64,
    skip: u64,
    emit: u64,
    /// 探查:访问行序列(≤16 记录)+ 结束时 next_line——用数据反推
    /// pass2 的推进形状(计数形状与代码推演不符时的裁决依据)。
    l_seq: Vec<usize>,
    nl_end: i64,
    /// 探查:输入清单形状——blocks 列表长度/段范围/首末块起点/推进线
    /// block_end_resolve 调用次数(pass2 谜底裁决用)。
    blocks_n: i64,
    seg_lo: i64,
    seg_hi: i64,
    bs_first: i64,
    bs_last: i64,
    bre_calls: i64,
    /// 探查:完整块起点序列(裁决用——不再猜清单内容,直接吐)。
    bs_seq: Vec<usize>,
    q_start: i64,
    q_end: i64,
}

/// node_category(spans, line, t)——selector 的节点枚举类别。
/// side_effect 容器（Comment/RefDef）route away from block tree——
/// Comment 例外（selector 与 walk 不一致修复——TestPjComment 暴露）:
///   spans 可用（nested 模式——逐行推进 i+1）: 内容行（Comment 帧在栈且
///     无 Push）必须 Skip（壳内——不产节点——防与壳节点重复）——开行
///     （is_block_start——有 Push——壳节点）与单行配对（无帧——leaf）
///     → Block（与 walk 一致——AST 产 cm 壳/leaf）
///   spans 不可用（flat 模式——node_end 推进 i=end——开行 Block 后直接跳
///     块尾——内容行不被访问）→ 访问到的 Comment 行都产节点 → Block
/// RefDef 同样不一致（AST 产 rd——side_effect=PopulateRefMap）——
/// 无测试暴露——记录（不扩域——保持现有 Skip）。
pub(crate) fn node_category(spans: Option<&dyn crate::effect::ResolveView>, line: usize, t: BlockType) -> NodeCategory {
    #[cfg(feature = "telemetry")] let _s_nci = crate::telemetry_span!("sel_ncat_impl");
    // FrameKind containers with side_effect route away from block tree
    if let Some(kind) = crate::block::frame_kind(t) {
        if crate::effect::effect_for(kind).side_effect.is_some() {
            let comment_block = if kind == FrameKind::Comment {
                spans.map_or(true, |s| {
                    let d = s.close_depth_at(line);
                    let frame_in = d > 0
                        && (0..d).any(|k| s.node_type_at_depth(line, k) == BlockType::Comment);
                    // 无帧（单行配对 leaf）或开行（有 Push——壳节点）
                    !frame_in || s.is_block_start(line)
                })
            } else {
                false
            };
            if !comment_block {
                return NodeCategory::Skip;
            }
        }
    }
    if matches!(t,
        BlockType::CodeFenceClose | BlockType::CodeContent
        | BlockType::Blank | BlockType::ThematicBreak
        | BlockType::HtmlContent
        | BlockType::TableSeparator
    ) {
        return NodeCategory::Skip;
    }
    NodeCategory::Block
}

pub struct BlockBoundaryCache {
    intervals: Vec<ScanInterval>,
    /// 行 → item 子树尾(C1 折叠 subtree_end 的批量物化——一次前向
    /// open_at_depth pass O(entries)——之后 O(1)——C5 解药:
    /// item_subtree_end 逐调用 O(entries) 重扫 → 批量列)。
    cell_ends: Option<Vec<u32>>,
    // #43:structure/structure_full 字段不存在——全量结构缓存归位到快照级
    // 段 memo(projection::Snapshot.segment_memo,全量单值);merged 局部段
    // 只供本次查询不共享(merged_zone_chain_probe 实证语义保留)。
}

/// kind 位(B1 折叠物化——FrameKind repr(u8) 序数直接移位)。
#[inline]
const fn kind_bit(kind: crate::block::FrameKind) -> u16 { 1u16 << (kind as usize) }

/// C1 折叠 subtree_end 的批量物化（C5 解药——与 item_subtree_end 逐值等价）：
/// 前向一次 O(entries)——每深度维护当前开 item 行，pop 时结算、行尾补 n。
fn build_cell_ends(entries: &[u64], n: usize) -> Vec<u32> {
    let mut ends = vec![n as u32; n.max(1)];
    // 深度 ≤ MAX_BLOCK_NESTING（scan 层界）——单行复合（BQ+BLT 双 push）
    // 也可达深度 2 > n——open 按深度上限开
    let mut open: Vec<Option<usize>> = vec![None; n + crate::block::MAX_BLOCK_NESTING + 1];
    let mut depth: usize = 0;
    for &e in entries {
        if crate::block::entry_is_body_mut(e) { continue; }
        if crate::block::entry_is_pop(e) {
            if depth > 0 {
                if let Some(l) = open[depth].take() {
                    let li = crate::block::entry_line(e) as usize;
                    // 首个闭合生效（复合行同线多帧——外层容器 pop 不得覆盖
                    // 内层 item 已结算的子树尾）
                    if ends[l] == n as u32 { ends[l] = li as u32; }
                }
                depth -= 1;
            }
        } else {
            depth += 1;
            let li = crate::block::entry_line(e) as usize;
            if depth < open.len() && li < n { open[depth] = Some(li); }
        }
    }
    ends
}

/// 已扫描区间(扫描顺序推进——天然连续——通常 1-2 个)。
/// start_line = 实际扫描起点(Blank 根 = 0;块 = start+1——起点语义绑定
/// 块列表粒度)。
/// V3 §6.4：扩展为"第一遍+第二遍产物"的完整缓存条目——frame_ordinal
/// 入键（同线壳链 children 视图随父序号变化）、children_spec 行号级
/// 构造参数（命中时 infer::build_node 重建字节面）。
#[derive(Clone)]
pub struct ScanInterval {
    pub start_line: u32,
    pub end_line: u32,
    /// 父节点类型 + 本次 target——推进剪枝的结果与两者都相关：
    /// 同一个行区间对不同的 parent_type/target_name 可能产出不同的
    /// 可达块列表（H1 target 只留 H1、下一 target 必须重扫）。旧缓存
    /// 只按 (start,end) 命中，第一次查询会把后续目标可达到的块全部
    /// 剪掉（setext H1→H2 连续 Drill 实证）。
    pub parent_type: BlockType,
    /// 父壳链序号（V3 新增）——同线壳链父的后缀视图随 frame_ordinal
    /// 变化——§4.7 "boundaries 缺 ordinal 不能复用"的障碍消解点。
    pub frame_ordinal: u8,
    pub target_name: Option<String>,
    /// (行, 块尾, 块类型)——node_end 结果补录
    pub boundaries: Vec<(u32, u32, BlockType)>,
    /// 第二遍 children 的行号级构造参数（V3 新增——命中时 build_node
    /// 重建——字节区间/几何随当前快照现算）。
    pub children_spec: Vec<ChildNodeSpec>,
}

/// 第二遍产物的行号级构造参数（V3 §6.4）——不含字节区间/几何：
/// 命中缓存时用 infer::build_node（纯函数）重建 BlockNode——
/// 字节面随当前快照现算；生命周期 = 快照（block_cache 随快照新建）。
#[derive(Clone, Copy)]
pub struct ChildNodeSpec {
    pub start_line: u32,
    pub end_line: u32,
    pub node_type: BlockType,
    pub loose: bool,
    pub frame_ordinal: u8,
}

impl BlockBoundaryCache {
    pub fn new() -> Self {
        BlockBoundaryCache { intervals: Vec::new(), cell_ends: None }
    }
    /// 行 → item 子树尾（C1 折叠 subtree_end 的批量物化——一次前向
    /// open_at_depth pass O(entries)——之后 O(1)——C5 解药：替代
    /// item_subtree_end 的逐调用 O(entries) 重扫）。
    pub fn cell_end(&mut self, entries: &[u64], li: usize, n: usize) -> usize {
        let ends = self.cell_ends.get_or_insert_with(|| build_cell_ends(entries, n));
        ends.get(li).copied().unwrap_or(n as u32) as usize
    }
    /// 补录区间(§6.2——selector 跳块层扫描——只增不改——超出 16 丢最旧)。
    /// 缓存键 = (start, end, parent_type, frame_ordinal, target)——推进剪枝
    /// 结果随 target 变化，漏掉 target 维度会串味（不同 target 命中同一
    /// 份被剪过的列表）；frame_ordinal 区分同线壳链父的后缀视图。
    /// V3 §6.4：children_spec 一并补录——同快照重复查询完整复用两遍产物。
    pub fn record(&mut self, start_line: usize, end_line: usize,
                  parent_type: BlockType, frame_ordinal: u8, target_name: Option<&str>,
                  boundaries: Vec<(u32, u32, BlockType)>,
                  children_spec: Vec<ChildNodeSpec>) {
        if self.intervals.len() >= 16 {
            self.intervals.remove(0);
        }
        self.intervals.push(ScanInterval {
            start_line: start_line as u32, end_line: end_line as u32,
            parent_type, frame_ordinal,
            target_name: target_name.map(str::to_string),
            boundaries, children_spec,
        });
    }
    /// 区间查询(严格相等——五元组命中——同一次扫描的块列表才能安全复用
    /// ——不同 target/父壳序号重新扫+补录)。
    pub fn lookup(&self, start_line: usize, end_line: usize,
                  parent_type: BlockType, frame_ordinal: u8, target_name: Option<&str>)
        -> Option<&ScanInterval> {
        self.intervals.iter().rev().find(|iv|
            start_line == iv.start_line as usize && end_line == iv.end_line as usize
                && iv.parent_type == parent_type
                && iv.frame_ordinal == frame_ordinal
                && iv.target_name.as_deref() == target_name)
    }
}

/// 行号级 memo 有效性闸门:
/// 快照的 memo **有数据时必然新鲜**——
///   1. 本快照 merge_shared_cache 构建(selector 扫描产物,随快照新建);
///   2. 内容编辑守卫通过后继承(validate_content_overlay——结构已验证
///      未变才挂载,全有效);
/// 唯一过期源是无条件继承——故 Edit 节点也无需
/// 挡(其 memo 只能来自上述两条新鲜路径)。曾有一刀切 memo_state_ok
/// (Edit → 全失效)与区间感知版(memo_interval_ok)——实测均误挡
/// 本快照构建的 structure(高频结构编辑下 fdc_struct_build 8279 次 =
/// 237ms,65% 冷):同一快照第 2 次 fdc 的 structure 是刚构建的,却被
/// "Edit + 区间跨 zone" 挡掉重新构建。保留本函数作为未来继承扩展
/// (结构编辑按 zone 前继承)的闸门位置——当前恒 true。
fn memo_interval_ok(_snap: &Snapshot, _start: usize, _end: usize) -> bool {
    true
}

/// §6.4 共享查询：短锁取命中 ScanInterval 的 clone（boundaries +
/// children_spec 均为行号级小数据——命中成本 ~0.1-0.3µs）。
fn shared_interval_lookup(snap: &Snapshot, start_line: usize, end_line: usize,
                          parent_type: BlockType, frame_ordinal: u8,
                          target_name: Option<&str>)
    -> Option<ScanInterval> {
    if !memo_interval_ok(snap, start_line, end_line) {
        return None;
    }
    shared_block_cache(snap)
        .and_then(|m| m.lock().unwrap()
            .lookup(start_line, end_line, parent_type, frame_ordinal, target_name)
            .cloned())
}

// Selector Vocabulary


#[derive(Debug, Clone, PartialEq)]
pub enum CmpOp { Eq, Ne, Ge, Le }

#[derive(Debug, Clone)]
pub enum Value { String(String), Number(f64) }

#[derive(Debug, Clone)]
pub enum ByQuantifier {
    Exists,
    Empty,
    Any(Box<FilterExpr>),
    All(Box<FilterExpr>),
}

#[derive(Debug, Clone)]
pub enum FilterExpr {
    Cmp { proj: BlockProj, op: CmpOp, value: Value },
    Regex { proj: BlockProj, regex: Regex },
    And(Vec<FilterExpr>),
    Or(Vec<FilterExpr>),
    Not(Box<FilterExpr>),
    TypeCheck(TypeName),
    Method { proj: BlockProj, method: FilterMethod },
    By { proj: BlockProj, steps: Vec<Step>, quantifier: ByQuantifier },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeName {
    Header(u8),
    Bullet,
    OrderedList,
    Code,
    IndtCode,
    Paragraph,
    BlockQuote,
    Table,
    Comment,
    Html,
}

#[derive(Debug, Clone)]
pub enum FilterMethod {
    Has(String),
    Eq(Value),
    Re(Regex),
    Empty,
}

#[derive(Debug, Clone)]
pub enum SingleSlice {
    Index(isize),
    Range(Option<isize>, Option<isize>, Option<isize>),
}

#[derive(Debug, Clone)]
pub enum Step {
    Drill(String),
    Wildcard,
    RecursiveDrill(String),
    RecursiveWildcard,
    Filter(FilterExpr),
    Slice(Vec<SingleSlice>),
    /// 融合步骤: 遍历 + 过滤 + 切片，单遍扫描(py 侧 fuse_steps 产出)
    TraversalFilter {
        traversal: Box<Step>,
        filters: Vec<FilterExpr>,
        limit: usize,
    },
}


// Selector Utilities


pub fn is_simple_selector(steps: &[Step]) -> bool {
    steps.iter().all(|s| matches!(s, Step::Drill(_)))
}

pub fn extract_head_parts(steps: &[Step]) -> Vec<String> {
    steps.iter()
        .filter_map(|s| if let Step::Drill(name) = s { Some(name.clone()) } else { None })
        .collect()
}


// Selector IR


#[derive(Debug, Deserialize)]
pub struct SelectorIR {
    #[serde(default)]
    pub steps: Vec<Step>,
}


// FilterExpr::from_json_array


impl FilterExpr {
    pub fn from_json_array(arr: &[JsonValue]) -> Option<Self> {
        let tag = arr.first()?.as_str()?;
        match tag {
            "TYPECHECK" => {
                let tn = parse_type_name(arr.get(1)?.as_str()?)?;
                Some(FilterExpr::TypeCheck(tn))
            }
            "CMP" => {
                let proj = parse_proj(arr.get(1)?.as_str()?)?;
                let op = parse_op(arr.get(2)?.as_str()?)?;
                let v = parse_value(arr.get(3)?)?;
                Some(FilterExpr::Cmp { proj, op, value: v })
            }
            "REGEX" => {
                let proj = parse_proj(arr.get(1)?.as_str()?)?;
                let raw = arr.get(2)?.as_str()?.trim_matches(|c| c == '"' || c == '\'');
                let regex = Regex::new(raw).ok()?;
                Some(FilterExpr::Regex { proj, regex })
            }
            "AND" | "OR" => {
                let mut subs = Vec::new();
                for sub in arr.get(1)?.as_array()? {
                    if let Some(sa) = sub.as_array() {
                        if let Some(f) = FilterExpr::from_json_array(sa) { subs.push(f); }
                    }
                }
                if tag == "AND" { Some(FilterExpr::And(subs)) } else { Some(FilterExpr::Or(subs)) }
            }
            "NOT" => {
                let sub = arr.get(1)?.as_array()?;
                Some(FilterExpr::Not(Box::new(FilterExpr::from_json_array(sub)?)))
            }
            "HAS" => {
                let proj = parse_proj(arr.get(1)?.as_str()?)?;
                let text = arr.get(2)?.as_str()?.to_string();
                Some(FilterExpr::Method { proj, method: FilterMethod::Has(text) })
            }
            "EQ" => {
                let proj = parse_proj(arr.get(1)?.as_str()?)?;
                let v = parse_value(arr.get(2)?)?;
                Some(FilterExpr::Method { proj, method: FilterMethod::Eq(v) })
            }
            "RE" => {
                let proj = parse_proj(arr.get(1)?.as_str()?)?;
                let raw = arr.get(2)?.as_str()?.trim_matches(|c| c == '"' || c == '\'');
                let regex = Regex::new(raw).ok()?;
                Some(FilterExpr::Method { proj, method: FilterMethod::Re(regex) })
            }
            "EMPTY" => {
                let proj = parse_proj(arr.get(1)?.as_str()?)?;
                Some(FilterExpr::Method { proj, method: FilterMethod::Empty })
            }
            "BY" | "BY_EMPTY" | "BY_ANY" | "BY_ALL" => {
                let proj = parse_proj(arr.get(1)?.as_str()?)?;
                let inner = arr.get(2)?.as_array()?;
                let inner_filter = FilterExpr::from_json_array(inner)?;
                let steps = vec![Step::RecursiveWildcard, Step::Filter(inner_filter)];
                let quantifier = match tag {
                    "BY" => ByQuantifier::Exists,
                    "BY_EMPTY" => ByQuantifier::Empty,
                    "BY_ANY" => {
                        let pred_arr = arr.get(3)?.as_array()?;
                        ByQuantifier::Any(Box::new(FilterExpr::from_json_array(pred_arr)?))
                    }
                    "BY_ALL" => {
                        let pred_arr = arr.get(3)?.as_array()?;
                        ByQuantifier::All(Box::new(FilterExpr::from_json_array(pred_arr)?))
                    }
                    _ => return None,
                };
                Some(FilterExpr::By { proj, steps, quantifier })
            }
            _ => None,
        }
    }
}


// Step Deserialize (hand-written)


use serde::de;

impl<'de> Deserialize<'de> for Step {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where D: de::Deserializer<'de> {
        let arr: Vec<JsonValue> = Vec::deserialize(deserializer)?;
        if arr.is_empty() {
            return Err(de::Error::custom("empty step array"));
        }
        let tag = arr[0].as_str().ok_or(de::Error::custom("step tag not a string"))?;
        match tag {
            "Drill" => {
                let name = arr.get(1).and_then(|v| v.as_str()).unwrap_or("");
                Ok(Step::Drill(name.to_string()))
            }
            "Wildcard" => Ok(Step::Wildcard),
            "RecursiveWildcard" => Ok(Step::RecursiveWildcard),
            "RecursiveDrill" => {
                let name = arr.get(1).and_then(|v| v.as_str()).unwrap_or("");
                Ok(Step::RecursiveDrill(name.to_string()))
            }
            "Slice" => {
                let mut specs = Vec::new();
                if arr.len() > 1 {
                    let second = &arr[1];
                    if let Some(idx) = second.as_i64() {
                        specs.push(SingleSlice::Index(idx as isize));
                    } else if let Some(items) = second.as_array() {
                        for item in items {
                            if let Some(idx) = item.as_i64() {
                                specs.push(SingleSlice::Index(idx as isize));
                            } else if let Some(range_arr) = item.as_array() {
                                if range_arr.first().and_then(|v| v.as_str()) == Some("RANGE") {
                                    let start = range_arr.get(1).and_then(|v| v.as_i64()).map(|n| n as isize);
                                    let end = range_arr.get(2).and_then(|v| v.as_i64()).map(|n| n as isize);
                                    let step = range_arr.get(3).and_then(|v| v.as_i64()).map(|n| n as isize);
                                    specs.push(SingleSlice::Range(start, end, step));
                                }
                            }
                        }
                    }
                }
                Ok(Step::Slice(specs))
            }
            "TraversalFilter" => {
                let traversal_arr = arr.get(1).and_then(|v| v.as_array())
                    .ok_or(de::Error::custom("TraversalFilter missing traversal"))?;
                let traversal: Step = serde_json::from_value(serde_json::Value::Array(traversal_arr.clone()))
                    .map_err(|e| de::Error::custom(format!("TraversalFilter bad traversal: {}", e)))?;
                let filters_arr = arr.get(2).and_then(|v| v.as_array())
                    .ok_or(de::Error::custom("TraversalFilter missing filters"))?;
                let mut filters = Vec::new();
                for fa in filters_arr {
                    if let Some(fa_arr) = fa.as_array() {
                        if let Some(fe) = FilterExpr::from_json_array(fa_arr) {
                            filters.push(fe);
                        }
                    }
                }
                let limit = arr.get(3).and_then(|v| v.as_u64()).unwrap_or(1) as usize;
                Ok(Step::TraversalFilter { traversal: Box::new(traversal), filters, limit })
            }
            _ => {
                FilterExpr::from_json_array(&arr)
                    .map(Step::Filter)
                    .ok_or_else(|| de::Error::custom(format!("unknown step tag: {}", tag)))
            }
        }
    }
}


// Parse helpers

// 只接受规范类型名("bullet"/"ol"/...)——用户别名("b"/"x"/"codeblock")由
// py 侧 TYPE_ANNOT 解析后到达(契约同步:TypeName 变体 ↔ mdio_ir.py)。

pub fn parse_type_name(s: &str) -> Option<TypeName> {
    match s {
        "h" => Some(TypeName::Header(0)),
        "h1" => Some(TypeName::Header(1)),
        "h2" => Some(TypeName::Header(2)),
        "h3" => Some(TypeName::Header(3)),
        "h4" => Some(TypeName::Header(4)),
        "h5" => Some(TypeName::Header(5)),
        "h6" => Some(TypeName::Header(6)),
        "bullet" => Some(TypeName::Bullet),
        "ol" => Some(TypeName::OrderedList),
        "code" => Some(TypeName::Code),
        "indtcode" => Some(TypeName::IndtCode),
        "paragraph" => Some(TypeName::Paragraph),
        "bq" => Some(TypeName::BlockQuote),
        "tb" => Some(TypeName::Table),
        "comment" => Some(TypeName::Comment),
        "html" => Some(TypeName::Html),
        _ => None,
    }
}

pub fn parse_proj(s: &str) -> Option<BlockProj> {
    match s {
        ".tag" | ".t" => Some(BlockProj::Tag),
        ".content" | ".ct" => Some(BlockProj::Content),
        ".raw" | ".r" => Some(BlockProj::Raw),
        ".container" | ".ctn" => Some(BlockProj::Container),
        ".mark" | ".m" => Some(BlockProj::Mark),
        _ => None,
    }
}

fn parse_op(s: &str) -> Option<CmpOp> {
    match s { "==" | "=" => Some(CmpOp::Eq), "!=" => Some(CmpOp::Ne), ">=" => Some(CmpOp::Ge), "<=" => Some(CmpOp::Le), _ => None }
}

fn parse_value(v: &JsonValue) -> Option<Value> {
    if v.is_string() { Some(Value::String(v.as_str()?.trim_matches(|c| c == '"' || c == '\'').to_string())) }
    else if v.is_number() { Some(Value::Number(v.as_f64()?)) }
    else { None }
}


// 类型匹配——表驱动(effect 声明——与渲染面 compact_type_name 同源)


pub fn check_node_type(node: &BlockNode, tn: &TypeName) -> bool {
    type_match(node.node_type, tn)
}

fn type_match(t: BlockType, tn: &TypeName) -> bool {
    use crate::block::frame_kind;
    use crate::effect::content_mode_for;
    use crate::effect::ContentMode;
    match tn {
        TypeName::Header(0) => crate::block::is_header(t),
        TypeName::Header(level) => crate::block::header_level(t) == Some(*level),
        TypeName::Bullet => {
            matches!(frame_kind(t),
                Some(crate::block::FrameKind::BulletList))
        }
        TypeName::OrderedList => {
            matches!(frame_kind(t),
                Some(crate::block::FrameKind::OrderedList))
        }
        TypeName::Code => {
            content_mode_for(t) == ContentMode::Code
        }
        TypeName::IndtCode => {
            crate::effect::compact_type_name(t) == "ic"
        }
        TypeName::Paragraph => crate::effect::compact_type_name(t) == "p",
        TypeName::BlockQuote => {
            frame_kind(t) == Some(crate::block::FrameKind::BlockQuote)
        }
        TypeName::Table => {
            matches!(frame_kind(t),
                Some(crate::block::FrameKind::Table | crate::block::FrameKind::TableRow))
        }
        TypeName::Comment => {
            frame_kind(t) == Some(crate::block::FrameKind::Comment)
        }
        TypeName::Html => {
            matches!(frame_kind(t),
                Some(crate::block::FrameKind::HtmlBlock
                    | crate::block::FrameKind::HtmlElement))
        }
    }
}


// 匹配/过滤/切片(与遍历形态无关——行级/节点级纯函数)


/// 行级块名匹配(peel 族——不物化 BlockNode——直接行文本)。
/// setext(段落+下划线)靠后(与 h1 等价后统一——未决问题)。
pub fn match_node_name(snap: &Snapshot, line_idx: usize, t: BlockType, target: &str) -> bool {
    #[cfg(feature = "telemetry")] let _s = crate::telemetry_span!("sel_match");
    Snapshot::with_line_str(snap.line_text(line_idx), |s: &str| {
        if crate::block::is_header(t) {
            // multi-`>` 开行标题（`> > # H9`）：全层剥 BQ 后提取——
            // extract_heading_text 内部 strip_blockquote_prefix 只剥一层，
            // 残留 `>` 使 `#` 检测失效（name drill `$..H9` 挂——F1 实证）。
            let full = crate::block::str_view(crate::block::strip_bq(s.as_bytes(), false));
            crate::block::extract_heading_text(full) == target
        } else if t == BlockType::Bullet || t == BlockType::Checklist {
            // 官方 kv 标签提取(find_colon_or_bracket_tag——与 node_tag 投影
            // 同源——纯字符规则(2026-08-08 用户裁定)/checkbox 形态/inline
            // 跳过/bracket key——Drill 匹配与投影 tag 语义统一)
            // BQ 内列表 item——全层剥 BQ 前缀再剥 marker（原只剥一层：
            // `> > - deep: d` 残留 `> -` 使 tag 提取成 `> - deep`——
            // multi-`>` item 的 name drill 挂——F1 实证）。
            let full = crate::block::str_view(crate::block::strip_bq(s.as_bytes(), false));
            if t == BlockType::Checklist {
                // P7 查漏：checklist 的 tag 源是 find_ckl_tag（colon 优先、
                // 无 colon 回 checkbox）；strip_list_marker 已越过 `[ ] `，
                // 再用 find_colon_or_bracket_tag(mark_end=6) 会把相对索引
                // 叠在 checkbox 之后（chk_1 drill 挂）。
                crate::block::extract_kv_tag(t, full) == target
            } else {
                let trimmed = crate::block::strip_list_marker(full, t);
                let mark_end = trimmed.as_ptr() as usize - full.as_ptr() as usize;
                let range = crate::block::find_colon_or_bracket_tag(full.as_bytes(), mark_end);
                range.start < range.end && &full[range] == target
            }
        } else if t == BlockType::CodeFenceOpen {
            crate::block::extract_fence_lang(s) == target
        } else if t == BlockType::HtmlBlock || t == BlockType::HtmlElement {
            crate::block::extract_html_tagname(s) == target
        } else {
            false
        }
    })
}

pub fn slice_nodes(nodes: Vec<BlockNode>, specs: &[SingleSlice]) -> Vec<BlockNode> {
    let len = nodes.len() as isize;
    if len == 0 { return vec![]; }
    let mut result = Vec::new();
    for spec in specs {
        match spec {
            SingleSlice::Index(idx) => {
                let real_idx = if *idx < 0 { len + idx } else { *idx };
                if real_idx >= 0 && real_idx < len { result.push(nodes[real_idx as usize].clone()); }
            }
            SingleSlice::Range(start_opt, end_opt, step_opt) => {
                let step = step_opt.unwrap_or(1);
                if step == 0 { continue; }
                let mut start = start_opt.unwrap_or(if step > 0 { 0 } else { len - 1 });
                if start < 0 { start += len; }
                let start = if start < 0 { if step > 0 { 0 } else { -1 } } else if start >= len { if step > 0 { len } else { len - 1 } } else { start };
                let mut end = end_opt.unwrap_or(if step > 0 { len } else { -len - 1 });
                if end < 0 && end_opt.is_some() { end += len; }
                let end = if end < 0 { if step > 0 { 0 } else { -1 } } else if end >= len { if step > 0 { len } else { len - 1 } } else { end };
                if step > 0 {
                    let mut i = start;
                    while i < end {
                        if i >= 0 && i < len { result.push(nodes[i as usize].clone()); }
                        i += step;
                    }
                } else {
                    let mut i = start;
                    while i > end {
                        if i >= 0 && i < len { result.push(nodes[i as usize].clone()); }
                        i += step;
                    }
                }
            }
        }
    }
    result
}

pub fn eval_method(snap: &Snapshot, node: &BlockNode, proj: &BlockProj, method: &FilterMethod) -> bool {
    let s = snap.read_block_proj(node, *proj);
    match method {
        FilterMethod::Has(text) => s.contains(text.as_str()),
        FilterMethod::Eq(val) => compare(&s, &CmpOp::Eq, val),
        FilterMethod::Re(regex) => regex.is_match(&s),
        FilterMethod::Empty => s.is_empty(),
    }
}

pub fn eval_filter(snap: &Snapshot, node: &BlockNode, expr: &FilterExpr) -> bool {
    match expr {
        FilterExpr::Cmp { proj, op, value } => {
            // .tag == 快路径(tag_eq——geometry 精确——strip_bq 全剥基准)
            if *proj == BlockProj::Tag && *op == CmpOp::Eq {
                if let Value::String(s) = value {
                    return snap.tag_eq(node, s);
                }
            }
            compare(&snap.read_block_proj(node, *proj), op, value)
        }
        FilterExpr::Regex { proj, regex } => regex.is_match(&snap.read_block_proj(node, *proj)),
        FilterExpr::And(exprs) => exprs.iter().all(|e| eval_filter(snap, node, e)),
        FilterExpr::Or(exprs) => exprs.iter().any(|e| eval_filter(snap, node, e)),
        FilterExpr::Not(expr) => !eval_filter(snap, node, expr),
        FilterExpr::TypeCheck(tn) => check_node_type(node, tn),
        FilterExpr::Method { proj, method } => eval_method(snap, node, proj, method),
        FilterExpr::By { proj, steps, quantifier } => eval_by(snap, node, proj, steps, quantifier),
    }
}

/// .by[] 证据过滤——内容投影做子文档重扫(纯函数——不建树)。
pub fn eval_by(snap: &Snapshot, node: &BlockNode, proj: &BlockProj, steps: &[Step], quantifier: &ByQuantifier) -> bool {
    let text = snap.read_block_proj(node, *proj);
    match quantifier {
        ByQuantifier::Exists => {
            if text.is_empty() { return false; }
            let inner_snap = crate::projection::project_text(&text);
            !select_nodes(&inner_snap, steps).is_empty()
        }
        ByQuantifier::Empty => {
            if text.is_empty() { return true; }
            let inner_snap = crate::projection::project_text(&text);
            select_nodes(&inner_snap, steps).is_empty()
        }
        ByQuantifier::Any(pred) => {
            if text.is_empty() { return false; }
            let inner_snap = crate::projection::project_text(&text);
            let evidence = select_nodes(&inner_snap, steps);
            evidence.iter().any(|n| eval_filter(&inner_snap, n, pred))
        }
        ByQuantifier::All(pred) => {
            if text.is_empty() { return false; }
            let inner_snap = crate::projection::project_text(&text);
            let evidence = select_nodes(&inner_snap, steps);
            if evidence.is_empty() { return false; }
            evidence.iter().all(|n| eval_filter(&inner_snap, n, pred))
        }
    }
}

pub fn compare(node_val: &str, op: &CmpOp, value: &Value) -> bool {
    match (op, value) {
        (CmpOp::Eq, Value::String(s)) => node_val == s,
        (CmpOp::Ne, Value::String(s)) => node_val != s,
        (CmpOp::Eq, Value::Number(n)) => node_val.parse::<f64>().map(|v| v == *n).unwrap_or(false),
        (CmpOp::Ne, Value::Number(n)) => node_val.parse::<f64>().map(|v| v != *n).unwrap_or(false),
        (CmpOp::Ge, Value::Number(n)) => node_val.parse::<f64>().map(|v| v >= *n).unwrap_or(false),
        (CmpOp::Le, Value::Number(n)) => node_val.parse::<f64>().map(|v| v <= *n).unwrap_or(false),
        (CmpOp::Ge, Value::String(s)) => node_val.as_bytes() >= s.as_bytes(),
        (CmpOp::Le, Value::String(s)) => node_val.as_bytes() <= s.as_bytes(),
        _ => false,
    }
}

pub fn exists(snap: &Snapshot, steps: &[Step]) -> bool {
    !select_nodes(snap, steps).is_empty()
}

pub fn head_exists(snap: &Snapshot, steps: &[Step]) -> bool {
    if steps.is_empty() { return false; }
    let nodes = select_nodes(snap, steps);
    !nodes.is_empty() && nodes.iter().all(|n| crate::block::is_header(n.node_type))
}

pub fn select_ranges(snap: &Snapshot, steps: &[Step]) -> Vec<std::ops::Range<usize>> {
    select_nodes(snap, steps).into_iter().map(|n| n.byte_range).collect()
}

pub fn select_for_write(snap: &Snapshot, steps: &[Step]) -> Vec<BlockNode> {
    let _span = telemetry_span!("select_for_write", "step_count" => steps.len());
    select_nodes(snap, steps)
}


// iter 形态遍历(行级现算——零物化——找到就停)


/// 共享 block_cache 的锁句柄（懒初始化）；None = 旧快照路径用局部缓存。
fn shared_block_cache(snap: &Snapshot) -> Option<&std::sync::Mutex<BlockBoundaryCache>> {
    snap.block_cache.as_ref().map(|bc| {
        bc.get_or_init(|| std::sync::Mutex::new(BlockBoundaryCache::new()))
    })
}

/// 短锁读取共享 cell_ends；重扫描在锁外执行（sel 对齐——Mutex 不再
/// 覆盖全量 build_structure_parts）。P3:命中前检查节点状态(区间感知)。
fn shared_cell_ends(snap: &Snapshot, start: usize, end: usize) -> Option<Vec<u32>> {
    if !memo_interval_ok(snap, start, end) {
        return None;
    }
    shared_block_cache(snap)
        .map(|m| m.lock().unwrap().cell_ends.clone())
        .flatten()
}

/// 结构段来源(#43):内核 prefix/mid/suffix 路由优先;未命中 → 全量
/// 查询(0,n)走段格总入口 structure_for_range——Edit 链存活即三域拼接
/// (零解析,旧 full 白吃 + mid + δ 平移),链断则内部裸建兜底 + 回填 memo
/// (行为一致);其余 None(调用方局部构建,不共享)。
pub(crate) fn shared_structure_for_range(snap: &Snapshot, start: usize, end: usize)
    -> Option<std::sync::Arc<crate::cache::Segment>>
{
    if let Some(kernel) = crate::cache::kernel_structure_parts_for_range(snap, start, end) {
        return Some(kernel);
    }
    if !memo_interval_ok(snap, start, end) {
        return None;
    }
    if start == 0 && end == snap.line_len() {
        // 每代第一次 (0,n) 查询不裸建整文档——
        // 链存活走拼接、链断回落裸建(该入口自兜底,总返回全量段)。
        return Some(crate::cache::structure_for_range(snap, 0, end));
    }
    None
}

/// 短锁回写：cell_ends 只填空位（并发重复构建无害），直接子区间照常补录。
/// (全量结构归快照级段 memo,局部段不共享。)
fn merge_shared_cache(snap: &Snapshot, local: &mut BlockBoundaryCache, record_intervals: bool) {
    let Some(m) = shared_block_cache(snap) else { return };
    let mut g = m.lock().unwrap();
    if g.cell_ends.is_none() && local.cell_ends.is_some() {
        g.cell_ends = local.cell_ends.take();
    }
    if record_intervals {
        for iv in std::mem::take(&mut local.intervals) {
            g.intervals.push(iv);
            if g.intervals.len() > 16 { g.intervals.remove(0); }
        }
    }
}

/// 直接子遍历核心(render 剪枝的块跳形态——两遍):
///   第一遍:build_structure_parts 的 ContainerRec 结算块尾 +
///     逐行块起点判定(LineView 原语)
///   第二遍:块列表驱动——块尾(records 或 node_end 基础尾;标题 = min
///     (章节区间, 容器截断))→ node_category → 匹配 → build_node
/// §6 懒缓存:扫描前 lookup(同区间命中跳过);扫描后 record(直接子)。
/// 嵌套/内容行在块尾区间内——推进不可达(枚举终端——测试 33/40)。
pub fn find_direct_children(snap: &Snapshot, parent: &BlockNode, target_name: Option<&str>) -> Vec<BlockNode> {
    #[cfg(feature = "telemetry")] let _span = crate::telemetry_span!("sel_fdc");
    // V3.3：命中路径前置——同快照重复查询直接复用 children_spec（build_node
    // 重建），**不 clone structure/cell_ends**（大文档每次 fdc clone ~60KB ≈
    // 30µs——命中路径不需要它们）。未命中才取共享缓存走全量扫描。
    let parent_is_shell = is_shell_node(parent);
    let start = if parent.node_type == BlockType::Blank { parent.start_line }
        else if parent_is_shell { parent.start_line }
        else { parent.start_line + 1 };
    let end = traversal_end(snap, parent);
    if let Some(iv) = shared_interval_lookup(
        snap, start, end, parent.node_type, parent.frame_ordinal, target_name)
    {
        #[cfg(feature = "telemetry")] let _sp_hit = crate::telemetry_span!("fdc_cache_hit");
        #[cfg(feature = "telemetry")] let _sp_rb = crate::telemetry_span!("fdc_hit_rebuild", "spec_count" => iv.children_spec.len());
        let rebuilt: Vec<BlockNode> = iv.children_spec.iter().map(|s| {
            let mut node = snap.build_node(s.start_line as usize, s.end_line as usize, s.node_type, s.loose);
            node.frame_ordinal = s.frame_ordinal;
            node
        }).collect();
        #[cfg(feature = "telemetry")] drop(_sp_rb);
        return rebuilt;
    }
    // 重扫描在锁外：shared_cell_ends 只短锁取缓存列，结束后短锁回写。
    // 探查 span:fdc 序厅(边界缓存取列 + 段供给 + 内核路由件)——首读
    // sel_fdc 未归因面的细分归因。
    #[cfg(feature = "telemetry")]
    let _sp_pro = crate::telemetry_span!("fdc_prologue");
    let mut local = BlockBoundaryCache::new();
    local.cell_ends = shared_cell_ends(snap, start, end);
    let seg = shared_structure_for_range(snap, start, end);
    let kernel_starts = crate::cache::kernel_block_starts(snap);
    let merged_zone = crate::cache::kernel_merged_zone(snap, start, end);
    #[cfg(feature = "telemetry")] drop(_sp_pro);
    // [去重] start/end/parent_is_shell 由外层算好传入——内层不重复计算
    // （traversal_end → 列表项内容叶走 item_subtree_end，是 O(entries) 的前向扫；
    // 文档根上每查询白扫一遍，且两处必须永远一致）。
    let children = find_direct_children_inner(
        snap, &mut local, parent, target_name, seg,
        kernel_starts.as_ref().map(|v| v.as_slice()), merged_zone,
        (start, end, parent_is_shell), None);
    merge_shared_cache(snap, &mut local, true);
    children
}

/// 行级"真开"push 掩码——PopPush 延续不计（同 kind 同行 pop+push =
/// 壳延续非新开——矩阵 b 行实测 entries：pop BQ + push BQ + push BLT——
/// BQ 是延续、BLT 是真开——行身份 = Bullet 而非第二个 BQ 壳）。
/// 与树的容器延续合并同律（is_poppush_kindonly 语义）。
/// 开行身份链（修点 13——任意 FrameKind 通用层序列投影，禁手写分类）：
///   shells  = 该行真开帧 push 序列的类型投影（PopPush 延续不计；
///             content_type_of 覆盖全部 10 个 FrameKind）
///   content = blocks_new 块记录 block_type（折叠族全层剥——reclassify
///             正解——`> > # H9` → Header1；块起点不在本行 → None）。
/// 枚举时按序每帧发一节点（frame_ordinal 0..N-1），最后发内容叶。
fn line_identity(entries: &[u64], blocks_new: &[crate::infer::BlockRec], line: usize)
    -> (Vec<BlockType>, Option<BlockType>)
{
    let mut popped: u16 = 0;
    let mut shells: Vec<BlockType> = Vec::new();
    let mut ei = crate::infer::partition(entries, line as u32);
    while ei < entries.len() && crate::block::entry_line(entries[ei]) as usize == line {
        let e = entries[ei];
        if !crate::block::entry_is_body_mut(e) {
            let k = crate::block::entry_frame(e).kind();
            let bit = kind_bit(k);
            if crate::block::entry_is_pop(e) {
                // PopPush 配对与 infer/effect 同源：必须同 line 紧邻同 kind
                // Push（kindonly——LOOSE 差异仍是同一列表/表的延续）。
                if crate::infer::is_poppush_pair(entries, ei, false) {
                    popped |= bit;
                }
            } else if popped & bit != 0 {
                popped &= !bit; // PopPush
                // BLT/OL 的 PopPush = 新 item 壳（枚举必须独立节点）；
                // TableRow 的 PopPush 仍走 content leaf：其逐行边界在
                // BlockRec 而非 ContainerRec.end，壳化会让每行吞整表。
                // BQ 的同线 pop+push 不是延续：blank 已关旧 BQ，push 是
                // 新根（`> - x\n\n> x` 的第二个 BQ），必须发独立壳。
                let shell_type = crate::effect::container_block_type(k);
                if crate::effect::block_decl(shell_type).is_list_item {
                    shells.push(shell_type);
                } else if !crate::effect::decl_for(k).has_pop_push {
                    shells.push(shell_type);
                }
            } else {
                shells.push(crate::effect::container_block_type(k));
            }
        }
        ei += 1;
    }
    let content = {
        let idx = blocks_new.partition_point(|b| (b.start as usize) <= line);
        if idx == 0 {
            None
        } else {
            let b = &blocks_new[idx - 1];
            if b.start as usize == line { Some(b.block_type) } else { None }
        }
    };
    (shells, content)
}


/// 内容叶身份的 frame_ordinal 哨兵：它占用的不是壳链序号，后续遍历
/// 不能把同 frame_kind 的内容叶误判成同线壳（BQ+list 行 `> - a` 的
/// Bullet 内容叶被当壳时，`a.*[bullet]` 会返回 a 自身并漏掉嵌套 b）。
const CONTENT_LEAF_FRAME_ORDINAL: u8 = u8::MAX;

#[inline]
fn is_shell_node(parent: &BlockNode) -> bool {
    parent.frame_ordinal != CONTENT_LEAF_FRAME_ORDINAL
        && (crate::block::frame_kind(parent.node_type).is_some() || parent.frame_ordinal > 0)
}

#[inline]
fn content_leaf(parent: &BlockNode) -> bool {
    parent.frame_ordinal == CONTENT_LEAF_FRAME_ORDINAL
}

/// 内容叶的投影区间（块记录）不包含嵌套子项，但遍历区间必须覆盖
/// item 子树；BQ+list 的 `a..b` 依赖这条差异。
fn traversal_end(snap: &Snapshot, parent: &BlockNode) -> usize {
    if content_leaf(parent) && crate::effect::block_decl(parent.node_type).is_list_item {
        crate::infer::item_subtree_end(
            &snap.frame_log.entries,
            &snap.open_depth,
            parent.start_line,
            snap.line_len(),
        )
    } else {
        parent.end_line
    }
}

/// 实际会发射的壳链长度（identity_nodes 的 shell_end 截断逻辑单源）。
/// 全部用表列推导：
/// - content 是 item 时，同线壳链截到最后一个非列表壳（BQ 承载、item 由
///   块记录内容身份发射——不吞嵌套子树）。
/// - 泛化壳（同 frame_kind 的 content_type_of）与解析后的具体 content
///   类型重复时压掉最后一个壳（Checklist→Bullet 等）。
/// - 伪表/PopPush 壳（解析后退化为 Paragraph）压掉壳，内容身份接管。
fn line_shell_end(shells: &[BlockType], content: Option<BlockType>) -> usize {
    let mut base = shells.len();
    let Some(ct) = content else { return base };
    // EXPERIMENT(b): do not truncate the shell chain before a same-line item;
    // emit BQ+BQ+BLT shells so identity keys match the tree walker's shells.
    let _ = crate::effect::block_decl(ct).is_list_item;
    let Some(&last) = shells.last() else { return base };
    let generic_shell_duplicates_content = crate::block::frame_kind(ct).is_some_and(|k| {
        ct != crate::effect::container_block_type(k)
            && crate::effect::container_block_type(k) == last
            && crate::block::frame_kind(last) == Some(k)
    });
    let pop_push_shell_fell_back_to_paragraph = ct == BlockType::Paragraph
        && crate::block::frame_kind(last)
            .is_some_and(|k| crate::effect::decl_for(k).has_pop_push)
        && !crate::effect::block_decl(last).is_list_item;
    if generic_shell_duplicates_content || pop_push_shell_fell_back_to_paragraph {
        base = base.saturating_sub(1);
    }
    base
}

/// 把一行的身份链物化为 BlockNode：shells[start_idx..] 按序发壳（frame_ordinal
/// 标全局链序号），随后发内容叶（blocks_new）。无 shell 时回退 t。
fn identity_nodes(
    snap: &Snapshot,
    bc: &mut BlockBoundaryCache,
    blocks_new: &[crate::infer::BlockRec],
    secs: &[(usize, usize, u8, BlockType)],
    settled: &std::collections::HashMap<(usize, u8), usize>,
    l: usize,
    vk: Option<u8>,
    t: BlockType,
    shells: &[BlockType],
    start_idx: usize,
    content: Option<BlockType>,
    target_name: Option<&str>,
    boundaries: &mut Vec<(u32, u32, BlockType)>,
    p2: &mut P2Breakdown,
) -> Vec<BlockNode> {
    let mut out = Vec::new();
    // BQ 同线的列表 item：壳链只发 BQ，item 节点由内容身份（blocks_new）
    // 独立发射（区间 = 块记录——不吞嵌套子树）。
    let content_is_list = content.map_or(false, |c| crate::effect::block_decl(c).is_list_item);
    let shell_end = line_shell_end(shells, content);
    let same_line_list_chain = shells.iter()
        .filter(|s| crate::effect::block_decl(**s).is_list_item)
        .count() >= 2;
    for (i, &st) in shells.iter().enumerate().take(shell_end).skip(start_idx) {
        // 普通单 item 壳 = 完整 item 子树（父区间包含嵌套子项）；同线多
        // item 链或 BQ 同线 item 用块记录区间（每个 marker 独立节点）。
        let be = if crate::effect::block_decl(st).is_list_item && !same_line_list_chain {
            block_end_resolve(snap, bc, blocks_new, secs, l, st, vk, settled)
        } else if crate::effect::block_decl(st).is_list_item {
            blocks_new.iter()
                .find(|b| b.start as usize == l && b.block_type == st)
                .map(|b| b.end as usize)
                .unwrap_or_else(|| {
                    block_end_resolve(snap, bc, blocks_new, secs, l, st, vk, settled)
                })
        } else {
            block_end_resolve(snap, bc, blocks_new, secs, l, st, vk, settled)
        };
        boundaries.push((l as u32, be as u32, st));
        if node_category(None, l, st) == NodeCategory::Block {
            #[cfg(feature = "telemetry")]
            let t_mn = crate::telemetry::ticks_ns();
            let m = match target_name {
                Some(n) => match_node_name(snap, l, st, n),
                None => true,
            };
            #[cfg(feature = "telemetry")]
            { p2.mn_ns += crate::telemetry::ticks_ns() - t_mn; }
            if m {
                #[cfg(feature = "telemetry")]
                let t_bn = crate::telemetry::ticks_ns();
                let mut node = snap.build_node(l, be, st, false);
                node.frame_ordinal = i as u8;
                #[cfg(feature = "telemetry")]
                { p2.bn_ns += crate::telemetry::ticks_ns() - t_bn; }
                out.push(node);
            }
        }
    }
    if let Some(ct) = content.filter(|&c| {
        c != BlockType::Blockquote
            && (!shells.contains(&c)
                || (content_is_list && shells.contains(&BlockType::Blockquote)))
    }) {
        let be = if crate::effect::block_decl(ct).is_list_item {
            blocks_new.iter()
                .find(|b| b.start as usize == l && b.block_type == ct)
                .map(|b| b.end as usize)
                .unwrap_or_else(|| {
                    block_end_resolve(snap, bc, blocks_new, secs, l, ct, vk, settled)
                })
        } else {
            #[cfg(feature = "telemetry")]
            let t_bi = crate::telemetry::ticks_ns();
            let r = block_end_resolve(snap, bc, blocks_new, secs, l, ct, vk, settled);
            #[cfg(feature = "telemetry")]
            { p2.bre_inner_ns += crate::telemetry::ticks_ns() - t_bi; p2.bre_inner_calls += 1; }
            r
        };
        boundaries.push((l as u32, be as u32, ct));
        if node_category(None, l, ct) == NodeCategory::Block {
            #[cfg(feature = "telemetry")]
            let t_mn = crate::telemetry::ticks_ns();
            let m = match target_name {
                Some(n) => match_node_name(snap, l, ct, n),
                None => true,
            };
            #[cfg(feature = "telemetry")]
            { p2.mn_ns += crate::telemetry::ticks_ns() - t_mn; }
            if m {
                #[cfg(feature = "telemetry")]
                let t_bn = crate::telemetry::ticks_ns();
                let mut node = snap.build_node(l, be, ct, false);
                node.frame_ordinal = CONTENT_LEAF_FRAME_ORDINAL;
                #[cfg(feature = "telemetry")]
                { p2.bn_ns += crate::telemetry::ticks_ns() - t_bn; }
                out.push(node);
            }
        }
    }
    if out.is_empty() && shells.is_empty() && content.is_none() {
        let be = block_end_resolve(snap, bc, blocks_new, secs, l, t, vk, settled);
        boundaries.push((l as u32, be as u32, t));
        if node_category(None, l, t) == NodeCategory::Block {
            #[cfg(feature = "telemetry")]
            let t_mn = crate::telemetry::ticks_ns();
            let m = match target_name {
                Some(n) => match_node_name(snap, l, t, n),
                None => true,
            };
            #[cfg(feature = "telemetry")]
            { p2.mn_ns += crate::telemetry::ticks_ns() - t_mn; }
            if m {
                #[cfg(feature = "telemetry")]
                let t_bn = crate::telemetry::ticks_ns();
                let mut node = snap.build_node(l, be, t, false);
                node.frame_ordinal = CONTENT_LEAF_FRAME_ORDINAL;
                #[cfg(feature = "telemetry")]
                { p2.bn_ns += crate::telemetry::ticks_ns() - t_bn; }
                out.push(node);
            }
        }
    }
    out
}
fn find_direct_children_inner(snap: &Snapshot, bc: &mut BlockBoundaryCache,
                              parent: &BlockNode, target_name: Option<&str>,
                              seg_in: Option<std::sync::Arc<crate::cache::Segment>>,
                              kernel_starts: Option<&[u32]>,
                              fallback_zone: Option<(usize, usize)>,
                              // [去重] (start, end, parent_is_shell)——外层已算，
                              // 内层不再重复（traversal_end 对列表项内容叶是 O(entries)）。
                              span: (usize, usize, bool),
                              pre: Option<&PreScan>) -> Vec<BlockNode> {
    // 修点 13：父是壳 → 同线链按 frame_ordinal 取后缀；内容叶
    // （CONTENT_LEAF_FRAME_ORDINAL）与普通块父从 start+1 开始。
    // V3 §6.4：五元组（区间+父类型+壳链序号+target）命中直接复用
    // 第二遍产物——children_spec（行号级）经 infer::build_node 纯函数
    // 重建（字节面现算）；frame_ordinal 入键消解 §4.7 "boundaries 缺
    // ordinal 不能复用"的障碍；生命周期 = 快照（block_cache 随快照）。
    // V3.3：命中路径已前置到 find_direct_children 入口（跳过 structure
    // clone）——此处只走未命中。
    let (start, end, parent_is_shell) = span;
    // 第一遍:游标 + 判定（+ C3b 块记录物化）
    let mut owned_scan: Option<(Vec<(usize, BlockType, Option<u8>)>,
        std::collections::HashMap<(usize, u8), usize>,
        std::sync::Arc<crate::cache::Segment>,
        Vec<(usize, usize, u8, BlockType)>)> = None;
    let (blocks, settled, seg, secs): (
        &[(usize, BlockType, Option<u8>)],
        &std::collections::HashMap<(usize, u8), usize>,
        std::sync::Arc<crate::cache::Segment>,
        &[(usize, usize, u8, BlockType)],
    ) = match pre {
        Some(ctx) => {
            let lo = ctx.blocks.partition_point(|b| b.0 < start);
            let hi = ctx.blocks.partition_point(|b| b.0 < end);
            (&ctx.blocks[lo..hi], &ctx.settled, ctx.seg.clone(), &ctx.secs)
        }
        None => {
            owned_scan = Some(scan_blocks(snap, bc, seg_in, start, end, kernel_starts, fallback_zone));
            let s = owned_scan.as_ref().unwrap();
            (s.0.as_slice(), &s.1, s.2.clone(), s.3.as_slice())
        }
    };
    let blocks_new = &seg.blocks;
    // 第二遍:推进线(块尾跳过/标题穿入——终端语义)+ 匹配 + 物化
    #[cfg(feature = "telemetry")] let _sp_p2 = crate::telemetry_span!("fdc_pass2");
    let mut children = Vec::new();
    let mut boundaries = Vec::new();
    let mut next_line = start;
    // 探查桶:pass2 内部细分(§P 探查记账——li/idn/bre/bpn + 计数)。
    let mut p2b = P2Breakdown::default();
    #[cfg(feature = "telemetry")]
    {
        p2b.blocks_n = blocks.len() as i64;
        p2b.seg_lo = seg.start_line as i64;
        p2b.seg_hi = seg.end_line as i64;
        p2b.bs_first = blocks.first().map(|b| b.0 as i64).unwrap_or(-1);
        p2b.bs_last = blocks.last().map(|b| b.0 as i64).unwrap_or(-1);
        p2b.bs_seq = blocks.iter().map(|b| b.0).collect();
        p2b.q_start = start as i64;
        p2b.q_end = end as i64;
    }
    // V4.1 二分跳：blocks 升序 + next_line 严格递增（block_end_resolve 全
    // 分支 > l）⟹ 第一个起点 ≥ next_line 的块即下一可达候选——嵌套块组
    // O(log N) 跳过（语义与线性 continue 逐位等价——pass2_binary_skip_equiv
    // 契约测试守护——children 集合/顺序不变——下游 idx/slice/filter 不受影响）。
    let mut bi = 0usize;
    while bi < blocks.len() {
        #[cfg(feature = "telemetry")]
        { p2b.iter += 1; }
        let (l, t, vk) = blocks[bi];
        #[cfg(feature = "telemetry")]
        { if p2b.l_seq.len() < 16 { p2b.l_seq.push(l); } }
        if l < next_line {
            #[cfg(feature = "telemetry")]
            { p2b.skip += 1; }
            #[cfg(feature = "telemetry")]
            let t_bp = crate::telemetry::ticks_ns();
            bi = blocks.partition_point(|&(b, _, _)| b < next_line);
            #[cfg(feature = "telemetry")]
            { p2b.bpn_ns += crate::telemetry::ticks_ns() - t_bp; }
            continue;
        }  // 上一块区间内——嵌套/内容——不可达（二分跳过整组）
        let at_parent_line = l == parent.start_line;
        #[cfg(feature = "telemetry")]
        let t_li = crate::telemetry::ticks_ns();
        let (shells, content) = line_identity(&snap.frame_log.entries, blocks_new, l);
        #[cfg(feature = "telemetry")]
        { p2b.li_ns += crate::telemetry::ticks_ns() - t_li; }
        let shell_end = line_shell_end(&shells, content);
        let start_idx = if at_parent_line && parent_is_shell {
            (parent.frame_ordinal as usize).saturating_add(1).min(shell_end)
        } else {
            0
        };
        // 同线父壳只发链后缀：剩余壳；链尾父壳（start_idx == shell_end）
        // 才发内容叶。
        let content_to_emit = if at_parent_line && parent_is_shell && start_idx < shell_end {
            None
        } else {
            content
        };
        #[cfg(feature = "telemetry")]
        let t_idn = crate::telemetry::ticks_ns();
        let emitted = identity_nodes(snap, bc, blocks_new, &secs, &settled,
            l, vk, t, &shells, start_idx, content_to_emit, target_name, &mut boundaries,
            &mut p2b);
        #[cfg(feature = "telemetry")]
        { p2b.idn_ns += crate::telemetry::ticks_ns() - t_idn; }
        let emitted_empty = emitted.is_empty();
        children.extend(emitted);
        // 直接子推进 = 首个身份块尾（块尾跳过——嵌套/内容行不可达）；
        // 无可发射身份（全 Skip/过滤掉）逐行推进。
        let first_identity = if start_idx < shell_end {
            shells[start_idx]
        } else {
            content.unwrap_or(t)
        };
        if emitted_empty {
            next_line = l + 1;
        } else {
            #[cfg(feature = "telemetry")]
            { p2b.bre_calls += 1; }
            #[cfg(feature = "telemetry")]
            let t_bre = crate::telemetry::ticks_ns();
            next_line = block_end_resolve(snap, bc, blocks_new, &secs,
                l, first_identity, vk, &settled);
            #[cfg(feature = "telemetry")]
            { p2b.bre_ns += crate::telemetry::ticks_ns() - t_bre; }
        }
        bi += 1;
    }
    #[cfg(feature = "telemetry")]
    {
        p2b.emit = children.len() as u64;
        p2b.nl_end = next_line as i64;
        let l_seq: Vec<String> = p2b.l_seq.iter().map(|l| l.to_string()).collect();
        crate::telemetry_span!("fdc_p2_breakdown",
            "li_ns" => p2b.li_ns, "idn_ns" => p2b.idn_ns,
            "bre_ns" => p2b.bre_ns, "bpn_ns" => p2b.bpn_ns,
            "mn_ns" => p2b.mn_ns, "bn_ns" => p2b.bn_ns,
            "bre_inner_ns" => p2b.bre_inner_ns, "bre_inner_calls" => p2b.bre_inner_calls,
            "iter" => p2b.iter, "skip" => p2b.skip, "emit" => p2b.emit,
            "l_seq" => l_seq.join(","), "nl_end" => p2b.nl_end,
            "blocks_n" => p2b.blocks_n, "seg_lo" => p2b.seg_lo, "seg_hi" => p2b.seg_hi,
            "bs_first" => p2b.bs_first, "bs_last" => p2b.bs_last,
            "bre_calls" => p2b.bre_calls,
            "bs_seq" => p2b.bs_seq.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(","),
            "q_start" => p2b.q_start, "q_end" => p2b.q_end,
            "start" => start, "end" => end);
    }
    #[cfg(feature = "telemetry")] drop(_sp_p2);
    // V3 §6.4：第二遍产物以行号级 spec 补录（boundaries 审计保留）——
    // 同快照重复查询命中后 build_node 重建。
    let children_spec: Vec<ChildNodeSpec> = children.iter().map(|n| ChildNodeSpec {
        start_line: n.start_line as u32,
        end_line: n.end_line as u32,
        node_type: n.node_type,
        loose: n.loose,
        frame_ordinal: n.frame_ordinal,
    }).collect();
    bc.record(start, end, parent.node_type, parent.frame_ordinal, target_name,
        boundaries, children_spec);
    children
}

/// 后代遍历(同直接子——推进差异:穿入律——region 内有块起点才 i+1 穿入,
/// 块起点/region 都出自单一拓扑事实——header 章节与嵌套 item 同律;
/// 空 region 块跳——枚举终端)。
/// 不参与 §6 懒缓存(递归集合与直接子不同——模式混淆实证)。
pub fn find_descendants(snap: &Snapshot, parent: &BlockNode, target_name: Option<&str>) -> Vec<BlockNode> {
    #[cfg(feature = "telemetry")] let _span = crate::telemetry_span!("sel_fdesc");
    let parent_is_shell = is_shell_node(parent);
    let start = if parent.node_type == BlockType::Blank { parent.start_line }
        else if parent_is_shell { parent.start_line }
        else { parent.start_line + 1 };
    let end = traversal_end(snap, parent);
    let mut local = BlockBoundaryCache::new();
    local.cell_ends = shared_cell_ends(snap, start, end);
    let seg = shared_structure_for_range(snap, start, end);
    let kernel_starts = crate::cache::kernel_block_starts(snap);
    let merged_zone = crate::cache::kernel_merged_zone(snap, start, end);
    let descendants = find_descendants_inner(
        snap, &mut local, parent, target_name, seg,
        kernel_starts.as_ref().map(|v| v.as_slice()), merged_zone);
    merge_shared_cache(snap, &mut local, false);
    descendants
}

/// 全节点平坦枚举:块行平铺 + `identity_nodes`(引擎唯一的节点物化点)。
/// 外部只读消费(Arrow 物化等)必须走这里——不复制任何节点语义。
pub trait NodeSink {
    fn emit(&mut self, node: &BlockNode);
}

pub fn all_nodes(snap: &Snapshot) -> Vec<BlockNode> {
    all_nodes_with_sink(snap, None)
}

pub fn all_nodes_with_sink(snap: &Snapshot, mut sink: Option<&mut dyn NodeSink>) -> Vec<BlockNode> {
    if snap.line_len() == 0 { return Vec::new(); }
    let mut bc = BlockBoundaryCache::new();
    bc.cell_ends = shared_cell_ends(snap, 0, snap.line_len());
    let seg = shared_structure_for_range(snap, 0, snap.line_len());
    let kernel_starts = crate::cache::kernel_block_starts(snap);
    let merged_zone = crate::cache::kernel_merged_zone(snap, 0, snap.line_len());
    let (blocks, settled, seg2, secs) = scan_blocks(
        snap, &mut bc, seg, 0, snap.line_len(),
        kernel_starts.as_ref().map(|v| v.as_slice()), merged_zone,
    );
    let blocks_new = &seg2.blocks;
    let mut out: Vec<BlockNode> = Vec::new();
    for &(l, t, vk) in &blocks {
        let (shells, content) = line_identity(&snap.frame_log.entries, blocks_new, l);
        let mut p2 = P2Breakdown::default();
        let batch = identity_nodes(
            snap, &mut bc, blocks_new, &secs, &settled,
            l, vk, t, &shells, 0, content, None,
            &mut Vec::new(), &mut p2,
        );
        if let Some(s) = sink.as_mut() {
            for n in &batch {
                s.emit(n);
            }
        }
        out.extend(batch);
    }
    out
}

/// 批量边轮子：一次全量 scan 上下文，供 all_direct_edges 逐父复用，
/// 避免每个父节点重复扫描全部 records。仅新入口使用；既有 selector 路径
/// 传 None，行为不变。
struct PreScan {
    blocks: Vec<(usize, BlockType, Option<u8>)>,
    settled: std::collections::HashMap<(usize, u8), usize>,
    seg: std::sync::Arc<crate::cache::Segment>,
    secs: Vec<(usize, usize, u8, BlockType)>,
}

fn build_pre_scan(snap: &Snapshot) -> (PreScan, BlockBoundaryCache) {
    let n = snap.line_len();
    let mut bc = BlockBoundaryCache::new();
    bc.cell_ends = shared_cell_ends(snap, 0, n);
    let seg = shared_structure_for_range(snap, 0, n);
    let kernel_starts = crate::cache::kernel_block_starts(snap);
    let merged_zone = crate::cache::kernel_merged_zone(snap, 0, n);
    let (blocks, settled, seg2, secs) = scan_blocks(
        snap, &mut bc, seg, 0, n,
        kernel_starts.as_ref().map(|v| v.as_slice()), merged_zone,
    );
    (PreScan { blocks, settled, seg: seg2, secs }, bc)
}

fn node_key(n: &BlockNode) -> (usize, usize, u8, u8) {
    (n.start_line, n.end_line, n.node_type as u8, n.frame_ordinal)
}

/// DIRECT 批量结果：parent/child 均为 all_nodes 下标，pos 为父内发射序号。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectEdge {
    pub parent: u32,
    pub child: u32,
    pub pos: u32,
}

/// 批量 DIRECT 扫描结果，额外保留无法映射回 IDENT 的 phantom 子节点。
/// 正常文档 phantom_children 为空；非空表示 facade 应报 NON_WF。
#[derive(Debug, Clone, Default)]
pub struct DirectEdgesScan {
    pub edges: Vec<DirectEdge>,
    pub phantom_children: Vec<(u32, BlockNode)>,
}

/// 一趟全量 scan + 逐父窗口发射，产出 selector 语义的 DIRECT。
/// 与 find_direct_children 逐边相等（同一 inner 发射逻辑），不改变 selector 行为。
pub fn all_direct_edges(snap: &Snapshot) -> Vec<DirectEdge> {
    all_direct_edges_checked(snap).edges
}

pub fn all_direct_edges_checked(snap: &Snapshot) -> DirectEdgesScan {
    let nodes = all_nodes(snap);
    all_direct_edges_checked_with_nodes(snap, &nodes)
}

pub fn all_direct_edges_checked_with_nodes(snap: &Snapshot, nodes: &[BlockNode]) -> DirectEdgesScan {
    let n = nodes.len();
    if n == 0 { return DirectEdgesScan::default(); }
    let (pre, mut bc) = build_pre_scan(snap);
    let key_to_idx: std::collections::HashMap<_, usize> =
        nodes.iter().enumerate().map(|(i, x)| (node_key(x), i)).collect();
    let mut out: Vec<DirectEdge> = Vec::new();
    let mut phantom: Vec<(u32, BlockNode)> = Vec::new();
    for p in 0..n {
        let parent = &nodes[p];
        let parent_is_shell = is_shell_node(parent);
        let start = if parent.node_type == BlockType::Blank { parent.start_line }
            else if parent_is_shell { parent.start_line }
            else { parent.start_line + 1 };
        let end = traversal_end(snap, parent);
        let children = find_direct_children_inner(
            snap, &mut bc, parent, None, None, None, None,
            (start, end, parent_is_shell), Some(&pre),
        );
        for (pos, c) in children.iter().enumerate() {
            if let Some(&ci) = key_to_idx.get(&node_key(c)) {
                out.push(DirectEdge { parent: p as u32, child: ci as u32, pos: pos as u32 });
            } else {
                phantom.push((p as u32, c.clone()));
            }
        }
    }
    DirectEdgesScan { edges: out, phantom_children: phantom }
}

fn find_descendants_inner(snap: &Snapshot, bc: &mut BlockBoundaryCache,
                          parent: &BlockNode, target_name: Option<&str>,
                          seg_in: Option<std::sync::Arc<crate::cache::Segment>>,
                          kernel_starts: Option<&[u32]>,
                          fallback_zone: Option<(usize, usize)>) -> Vec<BlockNode> {
    // 修点 13（同 fdc）：父是壳 → 同线链按 frame_ordinal 取后缀；
    // 内容叶哨兵按普通块父从 start+1 开始。
    let parent_is_shell = is_shell_node(parent);
    let start = if parent.node_type == BlockType::Blank { parent.start_line }
        else if parent_is_shell { parent.start_line }
        else { parent.start_line + 1 };
    let end = traversal_end(snap, parent);
    let (blocks, settled, seg, secs) = scan_blocks(snap, bc, seg_in, start, end, kernel_starts, fallback_zone);
    let blocks_new = &seg.blocks;
    let mut descendants = Vec::new();
    let mut next_line = start;
    for &(l, t, vk) in &blocks {
        if l < next_line { continue; }  // 上一块区间内——不可达
        let at_parent_line = l == parent.start_line;
        let (shells, content) = line_identity(&snap.frame_log.entries, blocks_new, l);
        let shell_end = line_shell_end(&shells, content);
        let start_idx = if at_parent_line && parent_is_shell {
            (parent.frame_ordinal as usize).saturating_add(1).min(shell_end)
        } else {
            0
        };
        let content_to_emit = if at_parent_line && parent_is_shell && start_idx < shell_end {
            None
        } else {
            content
        };
        // 后代路径(RecursiveDrill)暂不导出桶——探查面在直接子(Direct
        // Drill);桶对象仍传递以兼容签名(feature OFF 零成本)。
        let mut p2b_desc = P2Breakdown::default();
        let emitted = identity_nodes(snap, bc, blocks_new, &secs, &settled,
            l, vk, t, &shells, start_idx, content_to_emit, target_name, &mut Vec::new(),
            &mut p2b_desc);
        let emitted_empty = emitted.is_empty();
        descendants.extend(emitted);
        if emitted_empty {
            next_line = l + 1;
            continue;
        }
        let first_identity = if start_idx < shell_end {
            shells[start_idx]
        } else {
            content.unwrap_or(t)
        };
        let be = block_end_resolve(snap, bc, blocks_new, &secs,
            l, first_identity, vk, &settled);
        // 穿入律：region 内有块起点才穿入（同线链已在 emitted 中一次性
        // 展开——此处只看后续行）。块起点列与 scan_blocks 同一来源（blocks）。
        let has_child = (l + 1..be).any(|i| {
            blocks.binary_search_by_key(&i, |&(s, _, _)| s).is_ok()
        });
        next_line = if has_child { l + 1 } else { be };
    }
    descendants
}

/// settled 双语义拆分哨兵位(§7.2):header 身份条目键 = (start, kind | FLAG),
/// 与 push_lines 容器区间条目(start, kind)键空间分离,互不覆盖。
const HEADER_BOUND_KEY_FLAG: u8 = 0x80;

/// 第一遍扫描(游标结算 + 逐行块起点判定):
///   settled = (起点, kind) → 游标块尾(Push 类块——按 t 的 frame_kind 匹配:
///     表头行 Push Table 帧但 t=TableRow——Table 栈项不匹配 TableRow 块——
///     leaf 单行语义保持)
///   blocks = (起点, 类型) 列表(推进序——含 Skip 类——第二遍分派)
fn scan_blocks(snap: &Snapshot, bc: &mut BlockBoundaryCache,
               seg_in: Option<std::sync::Arc<crate::cache::Segment>>,
               start: usize, end: usize, kernel_starts: Option<&[u32]>,
               fallback_zone: Option<(usize, usize)>)
    -> (Vec<(usize, BlockType, Option<u8>)>, std::collections::HashMap<(usize, u8), usize>,
        std::sync::Arc<crate::cache::Segment>, Vec<(usize, usize, u8, BlockType)>) {
    // selector 导航固定使用 HS 章节语义；若未来需要 cf/bf 模式，
    // 应把 Topology/heading_candidates 作为参数传入，而不是硬编码 HS。
    // V3：第一遍直接消费 infer 的 BlockRec（行→块映射是 infer 已解决的
    // 权威产物）——不再逐行判定/重建 view arrays——blocks 列表 = 块记录
    // 区间过滤 + container_ref 直读 vk（区间化在消费点自然达成）。
    // #43:块导航段来源 = 内核路由/快照级段 memo(调用方传入);未命中 →
    // 局部构建(merged 局部段只供本次查询,不共享——merged_zone 链实证)。
    let n_full = snap.line_len();
    let entries = &snap.frame_log.entries;
    #[cfg(feature = "telemetry")] let _sp_clone = crate::telemetry_span!("fdc_struct_clone");
    let seg: std::sync::Arc<crate::cache::Segment> = if let Some(s) = seg_in {
        #[cfg(feature = "telemetry")] drop(_sp_clone);
        s
    } else {
        #[cfg(feature = "telemetry")] drop(_sp_clone);
        #[cfg(feature = "telemetry")] let _sp_sb = crate::telemetry_span!("fdc_struct_build");
        // §6.3 merged-zone local build for cross-mid queries; otherwise the
        // full build remains the fallback(全量查询 memo-first,§7.1/#36b:
        // 先读快照级段 memo,未命中裸建并回填)。
        let build_zone = fallback_zone.unwrap_or((0, n_full));
        let built = if build_zone == (0, n_full) {
            if let Some(m) = snap.segment_memo.get() {
                m.clone()
            } else {
                let b = std::sync::Arc::new(crate::cache::with_build_purpose(
                    "selector_full", || crate::cache::Segment::build(snap, build_zone.0, build_zone.1)));
                let _ = snap.segment_memo.set(b.clone());
                b
            }
        } else {
            std::sync::Arc::new(crate::cache::with_build_purpose(
                "selector_local", || crate::cache::Segment::build(snap, build_zone.0, build_zone.1)))
        };
        #[cfg(feature = "telemetry")] drop(_sp_sb);
        built
    };
    let recs_full = &seg.records;
    let blocks_new = &seg.blocks;
    // selector 导航语义固定为 HS 章节（Drill 头部按标题章节组织）；
    // 如需随 ProjectionMode 切换，把 Topology.heading_candidates 作为
    // 参数传入 scan_blocks，而不是在这里按模式分支。
    // A2 归格(身份投影单源律):secs 与 settled header 子集一律派生格供给
    // (全量口径,双产物同一次全量 fold 产出)——根治形态 = 删除"局部段上
    // 算节区间"路径(局部/merged 段的截断身份不再进入身份产物)。块导航
    // (区间语义)仍由 bc.structure/局部段供给,禁动。
    #[cfg(feature = "telemetry")] let _sp_sec = crate::telemetry_span!("fdc_sec_intervals");
    let cell = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
    // 全量节区间格派生,按行过滤(与查询区间重叠面)。
    let secs: Vec<(usize, usize, u8, crate::block::BlockType)> = cell
        .secs_iter()
        .filter(|&(s, e, _, _)| s < end && e > start)
        .collect();
    #[cfg(feature = "telemetry")] drop(_sp_sec);
    // sel 对齐：容器边界直接消费 build_structure_parts 的 ContainerRec，
    // 不再用 BlockEndCursor 复刻 boundary_for。record.end 已含
    // end_includes_close/真实 Pop 结算；PopPush 续行的每个 push_line 也
    // 需要一条 (line, kind) 边界，与旧游标对 Push 半部的入栈语义一致。
    #[cfg(feature = "telemetry")] let _sp_set = crate::telemetry_span!("fdc_settled_loop");
    let mut settled_raw: Vec<(usize, u8, usize)> = Vec::new();
    for r in recs_full {
        let rec_end = r.end_usize().unwrap_or(end).min(end);
        for pl in r.push_lines_of(&seg.arena) {
            let pl = pl as usize;
            if pl >= start && pl < end {
                settled_raw.push((pl, r.frame.kind() as u8, rec_end));
            }
        }
    }
    #[cfg(feature = "telemetry")] drop(_sp_set);
    // 块列表 = BlockRec 区间过滤（二分定位起点——升序）+ header 的
    // 容器截断(身份语义——派生格 header_bounds 全量口径供给,禁回落
    // 查询尾截断:#41 兜底约束 = 同全量口径或显式 panic,缺失即不变量
    // 违反)。header 条目键加哨兵位(HEADER_BOUND_KEY_FLAG)——settled 双
    // 语义拆分(§7.2"两者独立来源"):push_lines 条目 = 容器区间语义
    // (截断合法,禁动)与 header 身份条目同键冲突时不得互相覆盖。
    let mut blocks: Vec<(usize, BlockType, Option<u8>)> = Vec::new();
    // PRC boundary index: when the Base kernel supplied its sorted block-start
    // column, binary-search it directly; otherwise fall back to BlockRec scan.
    let lo = if let Some(starts) = kernel_starts {
        starts.partition_point(|&s| (s as usize) < start)
    } else {
        blocks_new.partition_point(|b| (b.start as usize) < start)
    };
    for b in &blocks_new[lo..] {
        let bs = b.start as usize;
        if bs >= end { break; }
        let vk = if crate::block::is_header(b.block_type) && b.container_ref != u32::MAX {
            let r = &recs_full[b.container_ref as usize];
            let k = r.frame.kind() as u8;
            let bound = cell.header_bound(bs, k).unwrap_or_else(|| {
                panic!(
                    "cell header bound missing: header@{bs} kind={k} — identity product must be full-caliber"
                )
            });
            settled_raw.push((bs, k | HEADER_BOUND_KEY_FLAG, bound));
            Some(k)
        } else {
            None
        };
        blocks.push((bs, b.block_type, vk));
    }
    #[cfg(feature = "telemetry")] let _sp_sm = crate::telemetry_span!("fdc_settled_map");
    let mut settled: std::collections::HashMap<(usize, u8), usize> =
        std::collections::HashMap::with_capacity(settled_raw.len());
    for (s, k, e) in settled_raw {
        settled.entry((s, k)).or_insert(e);
    }
    #[cfg(feature = "telemetry")] drop(_sp_sm);
    (blocks, settled, seg, secs)
}

/// 块尾解析(第二遍——C3b 块记录直读——node_end 清零):
///   Section  → 章节区间(块记录派生) min 容器截断(ContainerRec)
///   Cell     → item 子树尾(深度跟踪——与块记录 item 块 end 语义不同型)
///   Container/Leaf → 块记录 end(node_end 四类语义已折叠进块记录——P5 对照)
fn block_end_resolve(snap: &Snapshot, bc: &mut BlockBoundaryCache,
                     blocks_new: &[crate::infer::BlockRec],
                     secs: &[(usize, usize, u8, BlockType)],
                     l: usize, t: BlockType, vk: Option<u8>,
                     settled: &std::collections::HashMap<(usize, u8), usize>) -> usize {
    let sec_end = secs.iter()
        .rev()
        .find(|&&(s, e, _, _)| s <= l && l < e)
        .map(|&(_, e, _, _)| e)
        .unwrap_or(0);
    if crate::block::is_header(t) {
        return match vk
            .and_then(|k| settled.get(&(l, k | HEADER_BOUND_KEY_FLAG)).copied())
        {
            Some(ce) => sec_end.min(ce),
            None => sec_end,
        };
    }
    if crate::effect::block_decl(t).is_list_item {
        // C5：批量物化列 O(1)（替代 item_subtree_end 逐调用重扫）
        return bc.cell_end(&snap.frame_log.entries, l, snap.line_len());
    }
    // Container 角色：ContainerRec 直读优先（settled = record.end 单源——
    // 与 infer 树 ContainerRec 同源），回退块记录二分。
    if let Some(k) = crate::block::frame_kind(t) {
        if let Some(ce) = settled.get(&(l, k as u8)).copied() {
            return ce;
        }
    }
    let idx = blocks_new.partition_point(|b| (b.start as usize) <= l);
    if idx > 0 {
        let b = &blocks_new[idx - 1];
        if (b.end as usize) > l {
            return b.end as usize;
        }
    }
    l + 1
}


// 融合步骤(遍历 + 过滤 + limit——py fuse_steps 产出)

// 物化下沉(轻判先行): TypeCheck 在候选 spec.node_type 上判(与物化后
// check_node_type 同源——build_node 的 node_type 直取 spec);
// CMP(tag,==,String) 用 line_tag_eq 行级判(line_geometry 现算——
// 与物化后 tag_eq 的 geometry 切片逐位同构——同一函数族);
// 通过轻判的候选才 build_node(重过滤——content 类/Or/Not/By 等——
// 在物化节点上判)。
// 等价性: filters 全合取语义(iter().all)——轻子句前置剔除 = 数学等价;
// spec 路径物化与 find_direct_children 缓存命中 rebuild 同构(同 spec);
// 未命中/递归遍历/无轻判子句 → 现状全量路径(无损下界)。
/// limit==0 全量收集;limit>0 收集到第 limit 个匹配——输出该节点。
fn apply_traversal_filter(snap: &Snapshot, ctx: &[BlockNode], traversal: &Step,
                          filters: &[FilterExpr], limit: usize,
                          direct: impl Fn(&Snapshot, &BlockNode, Option<&str>) -> Vec<BlockNode>,
                          desc: impl Fn(&Snapshot, &BlockNode, Option<&str>) -> Vec<BlockNode>) -> Vec<BlockNode> {
    let (type_filters, post_filters): (Vec<&FilterExpr>, Vec<&FilterExpr>) =
        filters.iter().partition(|f| matches!(f, FilterExpr::TypeCheck(_)));
    // 可轻判的 post 子句: 顶层 CMP(.tag,==,String)——行级比较零物化。
    // (Or/Not/By 包裹或非 Eq/非 Tag 的比较留在重过滤——物化后判,保守保语义。)
    let (light_posts, heavy_posts): (Vec<&FilterExpr>, Vec<&FilterExpr>) =
        post_filters.iter().partition(|f| matches!(f,
            FilterExpr::Cmp { proj: crate::block::BlockProj::Tag,
                              op: CmpOp::Eq,
                              value: Value::String(_) }));
    let mut matches: Vec<BlockNode> = Vec::new();
    let unlimited = limit == 0;
    for c in ctx {
        // ── 轻判路径: direct 类遍历 + 缓存命中(children_spec 行号级——零物化) ──
        // direct 类(Wildcard/Drill)才有 §6 懒缓存——desc 类走现状全量
        let is_direct = matches!(traversal, Step::Drill(_) | Step::Wildcard);
        if is_direct {
            let target: Option<&str> = match traversal {
                Step::Drill(n) => Some(n.as_str()),
                _ => None,
            };
            if let Some(specs) = find_direct_children_specs(snap, c, target) {
                for spec in &specs {
                    if !unlimited && matches.len() >= limit { break; }
                    // 轻判 1: TypeCheck —— spec.node_type 与物化后同源
                    let type_ok = type_filters.iter().all(|f| {
                        if let FilterExpr::TypeCheck(tn) = f {
                            type_match(spec.node_type, tn)
                        } else { true }
                    });
                    if !type_ok { continue; }
                    // 轻判 2: tag== 行级比较(与 tag_eq geometry 切片同构)
                    let light_ok = light_posts.iter().all(|f| {
                        if let FilterExpr::Cmp { value: Value::String(target), .. } = f {
                            line_tag_eq(snap, spec.start_line as usize, spec.node_type, target)
                        } else { true }
                    });
                    if !light_ok { continue; }
                    // 通过轻判才物化(与缓存命中 rebuild 同构——同 spec 同节点)
                    let mut node = snap.build_node(spec.start_line as usize, spec.end_line as usize,
                                                   spec.node_type, spec.loose);
                    node.frame_ordinal = spec.frame_ordinal;
                    // 重过滤(content 类/Or/Not/By/非 Eq tag 等)在物化节点上
                    let heavy_ok = heavy_posts.iter().all(|f| eval_filter(snap, &node, f));
                    if heavy_ok { matches.push(node); }
                }
                if !unlimited && matches.len() >= limit { break; }
                continue;
            }
        }
        // ── 现状路径: 全量遍历 + 物化后过滤(缓存未命中 / 递归遍历) ──
        let mut cands: Vec<BlockNode> = match traversal {
            Step::Wildcard => direct(snap, c, None),
            Step::Drill(name) => direct(snap, c, Some(name)),
            Step::RecursiveWildcard => desc(snap, c, None),
            Step::RecursiveDrill(name) => desc(snap, c, Some(name)),
            _ => Vec::new(),
        };
        for cand in cands.drain(..) {
            if !unlimited && matches.len() >= limit { break; }
            let type_ok = type_filters.iter().all(|f| {
                if let FilterExpr::TypeCheck(tn) = f {
                    check_node_type(&cand, tn)
                } else { true }
            });
            if !type_ok { continue; }
            let post_ok = post_filters.iter().all(|f| eval_filter(snap, &cand, f));
            if post_ok { matches.push(cand); }
        }
        if !unlimited && matches.len() >= limit { break; }
    }
    if unlimited { matches } else if matches.len() >= limit { vec![matches[limit - 1].clone()] } else { vec![] }
}

/// 行级 tag 相等比较(Filter tag== 轻判——零物化)。
/// 与物化后 tag_eq(infer::NodeProj::tag_eq——geometry 坐标切片)逐位同构:
/// strip_bq 全剥 → line_geometry(现算) → tag_range 切片比较——同一函数族,
/// 无二次词法(不重新实现 tag 提取)。line_geometry 失败/无 tag → false。
fn line_tag_eq(snap: &Snapshot, line_idx: usize, t: crate::block::BlockType,
               target: &str) -> bool {
    Snapshot::with_line_str(snap.line_text(line_idx), |s: &str| {
        let stripped = std::str::from_utf8(crate::block::strip_bq(s.as_bytes(), false))
            .unwrap_or(s);
        if let Some(geo) = crate::block::line_geometry(t, stripped.as_bytes()) {
            if geo.tag_range.start < geo.tag_range.end {
                let end = geo.tag_range.end.min(stripped.len());
                return &stripped[geo.tag_range.start..end] == target;
            }
        }
        false
    })
}

/// direct 类遍历的零物化候选:缓存命中返回 children_spec(行号级小数据);
/// 未命中返回 None(调用方回退全量 find_direct_children)。
/// 命中判定与 find_direct_children 入口同构(五元组 + memo 闸门)。
fn find_direct_children_specs(snap: &Snapshot, parent: &BlockNode,
                              target_name: Option<&str>) -> Option<Vec<ChildNodeSpec>> {
    let parent_is_shell = is_shell_node(parent);
    let start = if parent.node_type == BlockType::Blank { parent.start_line }
        else if parent_is_shell { parent.start_line }
        else { parent.start_line + 1 };
    let end = traversal_end(snap, parent);
    if !memo_interval_ok(snap, start, end) {
        return None;
    }
    shared_interval_lookup(snap, start, end, parent.node_type, parent.frame_ordinal,
                           target_name).map(|iv| iv.children_spec)
}

/// 执行前 IR 归一(段融合——与 py fuse_steps 同规则):
///   末段 [遍历, Filter*] → TraversalFilter(limit=0);
///   [遍历, Filter*, Slice(Index>=0)] → TraversalFilter(limit=n+1)。
/// 幂等(TraversalFilter 不再融合);递归处理前缀(多个可融合段)。
/// 作用: 未 fuse 的直调路径(hook 注册等)获得与 exec(fuse 后)一致的
/// 执行形态——apply_traversal_filter 的轻判/单遍收益自动触达。
/// 等价性: py fuse_steps 等价证明(45 测试)同规则。
pub fn normalize_steps(steps: &[Step]) -> Vec<Step> {
    fn is_trav(s: &Step) -> bool {
        matches!(s, Step::Drill(_) | Step::Wildcard
                   | Step::RecursiveDrill(_) | Step::RecursiveWildcard)
    }
    fn fuse_impl(steps: &[Step]) -> Vec<Step> {
        if steps.is_empty() { return vec![]; }
        let mut i = steps.len();
        while i > 0 {
            i -= 1;
            match &steps[i] {
                Step::Slice(specs) if specs.len() == 1 => {
                    if let SingleSlice::Index(n) = specs[0] {
                        if n >= 0 {
                            let mut j = i;
                            while j > 0 && matches!(steps[j - 1], Step::Filter(_)) { j -= 1; }
                            if j > 0 && is_trav(&steps[j - 1]) {
                                let mut pre = fuse_impl(&steps[..j - 1]);
                                let traversal = steps[j - 1].clone();
                                let filters: Vec<FilterExpr> = steps[j..i].iter()
                                    .filter_map(|s| if let Step::Filter(f) = s { Some(f.clone()) } else { None })
                                    .collect();
                                pre.push(Step::TraversalFilter {
                                    traversal: Box::new(traversal),
                                    filters,
                                    limit: (n as usize) + 1,
                                });
                                return pre;
                            }
                        }
                    }
                }
                // 融合仅当 Filter 组到尾(i+1==len)——若其后有不可融合步骤
                // (Slice RANGE/多段等),融合会丢弃它们——保留原步骤序(执行等价,
                // 不 fuse 只是不优化)。
                Step::Filter(_) if i + 1 == steps.len() => {
                    let mut j = i + 1;
                    while j > 0 && matches!(steps[j - 1], Step::Filter(_)) { j -= 1; }
                    if j > 0 && is_trav(&steps[j - 1]) {
                        let mut pre = fuse_impl(&steps[..j - 1]);
                        let traversal = steps[j - 1].clone();
                        let filters: Vec<FilterExpr> = steps[j..i + 1].iter()
                            .filter_map(|s| if let Step::Filter(f) = s { Some(f.clone()) } else { None })
                            .collect();
                        pre.push(Step::TraversalFilter {
                            traversal: Box::new(traversal),
                            filters,
                            limit: 0,
                        });
                        return pre;
                    }
                }
                _ => {}
            }
        }
        steps.to_vec()
    }
    fuse_impl(steps)
}

fn root_node(snap: &Snapshot) -> BlockNode {
    // 空文档(0 行):byte_of_line(0) panic——返回 dummy(byte_range 空——
    // 与旧 select_nodes 的空 steps 形态一致)
    if snap.line_len() == 0 {
        return BlockNode {
            start_line: 0, end_line: 0, byte_range: 0..0,
            node_type: BlockType::Blank, loose: false, geometry: None, peel_width: 0,
            frame_ordinal: 0,
        };
    }
    snap.build_node(0, snap.line_len(), BlockType::Blank, false)
}

/// 步骤链执行(集合传递——每步输出 = 下步输入——find 闭包参数化:
/// 行级快路径 find_direct_children/find_descendants 与物化路径
/// find_*_v(RowView 列直读)共用——逻辑零改动)。
/// 同一 item 的壳(ord<255)与内容叶(255)现在都发射（与 tree walker 对齐）。
/// 查询语义仍以叶为 item：一组 (start,end,type) 里若存在叶，压掉壳。
fn dedup_item_shell_leaf(nodes: Vec<BlockNode>) -> Vec<BlockNode> {
    let mut leaf_keys: std::collections::HashSet<(usize, u8)> = std::collections::HashSet::new();
    for n in &nodes {
        if n.frame_ordinal == CONTENT_LEAF_FRAME_ORDINAL
            && crate::effect::block_decl(n.node_type).is_list_item
        {
            leaf_keys.insert((n.start_line, n.node_type as u8));
        }
    }
    if leaf_keys.is_empty() {
        return nodes;
    }
    nodes
        .into_iter()
        .filter(|n| {
            !(n.frame_ordinal != CONTENT_LEAF_FRAME_ORDINAL
                && crate::effect::block_decl(n.node_type).is_list_item
                && leaf_keys.contains(&(n.start_line, n.node_type as u8)))
        })
        .collect()
}

fn run_steps(
    snap: &Snapshot,
    steps: &[Step],
    track_depth: Option<&mut usize>,
    direct: impl Fn(&Snapshot, &BlockNode, Option<&str>) -> Vec<BlockNode>,
    desc: impl Fn(&Snapshot, &BlockNode, Option<&str>) -> Vec<BlockNode>,
) -> Vec<BlockNode> {
    run_steps_seeded(snap, steps, None, track_depth, direct, desc)
}

/// 播种版（patch match attr 合并）：`seed = Some(..)` 时从给定节点集起跑，
/// 其余与 run_steps **同一实现**（同一 fast-path 选择——不复制任何 Step 语义）。
fn run_steps_seeded(
    snap: &Snapshot,
    steps: &[Step],
    seed: Option<&[BlockNode]>,
    mut track_depth: Option<&mut usize>,
    direct: impl Fn(&Snapshot, &BlockNode, Option<&str>) -> Vec<BlockNode>,
    desc: impl Fn(&Snapshot, &BlockNode, Option<&str>) -> Vec<BlockNode>,
) -> Vec<BlockNode> {
    #[cfg(feature = "telemetry")] let _span = crate::telemetry_span!("sel_total", "steps" => steps.len());
    if steps.is_empty() {
        return match seed {
            Some(s) => s.to_vec(), // 播种续跑：空步 = 恒等（空集继续是空集）
            None => vec![root_node(snap)], // 原 select_nodes 语义
        };
    }
    if snap.line_len() == 0 { return vec![]; }
    // 播种语义精确：Some(空集) 就是空集（前缀未解析 ⇒ 结果为空——**不得回退 root**，
    // 否则会从根扫全文档）；只有 None（原 select_nodes）才从 root 起跑。
    let mut ctx: Vec<BlockNode> = match seed {
        Some(s) => s.to_vec(),
        None => vec![root_node(snap)],
    };
    for step in steps {
        let next: Vec<BlockNode> = match step {
            Step::Drill(name) => {
                let mut res = Vec::new();
                for c in &ctx { res.extend(direct(snap, c, Some(name))); }
                res
            }
            Step::Wildcard => {
                let mut res = Vec::new();
                for c in &ctx { res.extend(direct(snap, c, None)); }
                res
            }
            Step::RecursiveDrill(name) => {
                let mut res = Vec::new();
                for c in &ctx { res.extend(desc(snap, c, Some(name))); }
                res
            }
            Step::RecursiveWildcard => {
                let mut res = Vec::new();
                for c in &ctx { res.extend(desc(snap, c, None)); }
                res
            }
            Step::Filter(expr) => ctx.into_iter().filter(|c| eval_filter(snap, c, expr)).collect(),
            Step::Slice(specs) => slice_nodes(ctx, specs),
            Step::TraversalFilter { traversal, filters, limit } =>
                apply_traversal_filter(snap, &ctx, traversal, filters, *limit, &direct, &desc),
        };
        ctx = dedup_item_shell_leaf(next);
        if let Some(ref mut d) = track_depth {
            if matches!(step, Step::Drill(_)) {
                if !ctx.is_empty() { **d += 1; } else { track_depth = None; }
            } else { track_depth = None; }
        }
        if ctx.is_empty() { break; }
    }
    ctx
}

fn select_nodes_impl(snap: &Snapshot, steps: &[Step], track_depth: Option<&mut usize>) -> Vec<BlockNode> {
    // 执行前 IR 归一(段融合——py fuse_steps 同规则): 未 fuse 的直调路径
    // (hook 注册/写 locator 等)获得与 exec 一致的 TF 执行形态——
    // apply_traversal_filter 轻判触达。语义等价(同 fuse 规则——45 pass)。
    let normalized = normalize_steps(steps);
    run_steps(snap, &normalized, track_depth, find_direct_children, find_descendants)
}


// zone 重建路径(设计书第四节——触发条件 1 无实例:RefDef 约定渲染时
// 处理(当不存在)——#542/A0.5 不是 selector 的事;setext 靠后。
// 触发条件 2(归属/children 序)与 3(契约不满足)当前无消费方——
// 物化入口(父区间定界 → [zs,ze) 区间构建 → RowView 列直读——
// diff_patches_mode 形态)在消费方出现时接——不假装实现)。


pub fn select_nodes(snap: &Snapshot, steps: &[Step]) -> Vec<BlockNode> {
    select_nodes_impl(snap, steps, None)
}

/// patch match attr 合并：从给定节点集继续步骤链。
/// 语义 = run_steps 集合折叠：select_nodes([A,B]) ≡ select_nodes_from(select_nodes([A]),[B])。
/// （hook 的 scan_once 教训——手写步骤链漏变体；这里与 select_nodes 同实现、同 fast-path。）
pub fn select_nodes_from(snap: &Snapshot, start: &[BlockNode], steps: &[Step]) -> Vec<BlockNode> {
    let normalized = normalize_steps(steps);
    run_steps_seeded(snap, &normalized, Some(start), None, find_direct_children, find_descendants)
}

/// 记录连续前导 Drill 步骤中解析到非空节点集的数量(接骨用——
/// 最深的已存在标题深度)。
pub fn select_nodes_with_depth(snap: &Snapshot, steps: &[Step], drill_depth: &mut usize) -> Vec<BlockNode> {
    select_nodes_impl(snap, steps, Some(drill_depth))
}

#[cfg(test)]
mod cell_ends_tests {
    /// Setext 标题必须对 selector 可见（Drill 定位 + TypeCheck 分类）。
    /// hook compute_scope_range 依赖此语义计算章节 scope。
    #[test]
    fn setext_head_is_selectable() {
        use super::{Step, BlockType};
        use crate::projection::State;
        let mut st = State::new("Setext H1\n=========\n\ncontent\n\nSetext H2\n---------\n\nmore\n");
        let snap = st.snapshot();
        let nodes = super::select_nodes(&snap, &[Step::Drill("Setext H1".into())]);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_type, BlockType::HsHead1);
        assert_eq!((nodes[0].start_line, nodes[0].end_line), (0, 9));
        // H2 位于 H1 章节内部——直接子语义下不是 root 的直接子；
        // 从 H1 出发钻取必须可达（同时覆盖缓存键含 target 的路径：
        // 第一个 H1 查询不能污染第二个 target 的重扫）。
        let h2 = super::select_nodes(&snap, &[
            Step::Drill("Setext H1".into()),
            Step::Drill("Setext H2".into()),
        ]);
        assert_eq!(h2.len(), 1);
        assert_eq!(h2[0].node_type, BlockType::HsHead2);
    }

    /// 同线身份壳链截断（P7c）：BQ+list、泛化壳、伪表/PopPush 三类
    /// 全部由表列推导——不再手写 BQ/TableRow kind。
    #[test]
    fn line_shell_end_derived_filters() {
        use super::BlockType::*;
        // BQ 承载的 item：BQ 与 item 壳都发（与 tree walker 对齐）；item 叶(255)
        // 仍独立发，查询层 dedup_item_shell_leaf 把壳压掉。
        assert_eq!(super::line_shell_end(&[Blockquote, Bullet], Some(Bullet)), 2);
        // 列表壳链在后（BQ 在 item 之下）：BQ 与 item 壳都发。
        assert_eq!(super::line_shell_end(&[Bullet, Blockquote], Some(Bullet)), 2);
        // 纯列表 item：保留唯一 item 壳。
        assert_eq!(super::line_shell_end(&[Bullet], Some(Bullet)), 1);
        // 泛化壳（BLT 的 content_type_of=Bullet）与具体 content（Checklist）重复。
        assert_eq!(super::line_shell_end(&[Bullet], Some(Checklist)), 0);
        // 伪表：TableRow 壳 + 内容回退 Paragraph → 只发内容身份。
        assert_eq!(super::line_shell_end(&[TableRow], Some(Paragraph)), 0);
        // RefDef 不是 PopPush 壳——不能误压（约束位非 TableRow 手写）。
        assert_eq!(super::line_shell_end(&[RefDef], Some(Paragraph)), 1);
        // 普通内容与壳同型不压（Bullet 壳 + Bullet 内容走 BQ 分支外的单 item）。
        assert_eq!(super::line_shell_end(&[Blockquote, CodeFenceOpen], Some(CodeFenceOpen)), 2);
    }

    /// BQ+list 行的内容叶不能带壳身份（查漏回归）：`a` 的 Bullet 内容叶
    /// 是普通块父，children 从下一行开始；否则 `a.*[bullet]` 返回 a 自身。
    #[test]
    fn bq_list_content_leaf_is_not_shell() {
        use super::{Step, FilterExpr, TypeName};
        use crate::projection::State;
        let mut st = State::new("# S\n> - a: 1\n>   - b: 2\n> - c: 3\n");
        let snap = st.snapshot();
        let a = super::select_nodes(&snap, &[Step::Drill("S".into()), Step::Drill("a".into())]);
        assert_eq!(a.len(), 1, "drill a 应命中内容叶");
        let nested_bullets = super::select_nodes(&snap, &[
            Step::Drill("S".into()),
            Step::Drill("a".into()),
            Step::Wildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        ]);
        assert_eq!(nested_bullets.len(), 1, "a 的直接子 bullet 应只有 b: {:?}", nested_bullets.iter().map(|n| (n.start_line, n.end_line, n.frame_ordinal)).collect::<Vec<_>>());
        assert_eq!(nested_bullets[0].start_line, 2);
        let b = super::select_nodes(&snap, &[
            Step::Drill("S".into()),
            Step::Drill("a".into()),
            Step::RecursiveDrill("b".into()),
        ]);
        assert_eq!(b.len(), 1, "a..b 应命中嵌套 b: {:?}", b.iter().map(|n| (n.start_line, n.end_line)).collect::<Vec<_>>());
    }

    /// 普通 bullet 壳的 cell_end 不吞兄弟 item（投影 raw/content 契约）。
    #[test]
    fn plain_bullet_ranges_do_not_swallow_siblings() {
        use super::{Step, FilterExpr, TypeName};
        use crate::projection::State;
        let mut st = State::new("# Top\n- port: 5432\n- just text\n- [x] done\n");
        let snap = st.snapshot();
        let nodes = super::select_nodes(&snap, &[
            Step::RecursiveWildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        ]);
        assert_eq!(nodes.len(), 3);
        for (i, n) in nodes.iter().enumerate() {
            assert_eq!((n.start_line, n.end_line), (i + 1, i + 2), "bullet {i}");
        }
    }

    #[test]
    fn bq_heading_stays_inside_outer_section() {
        use super::{Step, FilterExpr, TypeName};
        use crate::projection::State;
        let mut st = State::new("# S\n> # H4\n> - B3: x\n");
        let snap = st.snapshot();
        let s = super::select_nodes(&snap, &[Step::Drill("S".into())]);
        assert_eq!(s.len(), 1);
        // H4 在 BQ 内，是 BQ 的局部章节；它只截断同容器深度的标题，
        // 不能截断 root 直系的 S section。
        assert_eq!((s[0].start_line, s[0].end_line), (0, 3));
        let bq_under_s = super::select_nodes(&snap, &[
            Step::Drill("S".into()),
            Step::RecursiveWildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::BlockQuote)),
        ]);
        assert_eq!(bq_under_s.len(), 1, "BQ 应仍在 S 章节内: {bq_under_s:?}");
        assert_eq!((bq_under_s[0].start_line, bq_under_s[0].end_line), (1, 3));
    }

    #[test]
    fn selector_blocks_direct_children_contract() {
        use super::{BlockType, Step, FilterExpr, TypeName};
        use crate::projection::State;
        let mut st = State::new("# Doc\n## Blocks\n> old blockquote\n> line two\n\n1. task one\n");
        let snap = st.snapshot();
        let nodes = super::select_nodes(&snap, &[
            Step::Drill("Doc".into()),
            Step::Drill("Blocks".into()),
            Step::Wildcard,
        ]);
        let kinds: Vec<BlockType> = nodes.iter().map(|n| n.node_type).collect();
        assert_eq!(kinds, vec![BlockType::Blockquote, BlockType::Paragraph, BlockType::OrderedListItem]);
        let bqs = super::select_nodes(&snap, &[
            Step::Drill("Doc".into()),
            Step::Drill("Blocks".into()),
            Step::Wildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::BlockQuote)),
        ]);
        assert_eq!(bqs.len(), 1);
        let bq_children = super::find_direct_children(&snap, &bqs[0], None);
        assert_eq!(bq_children.iter().map(|n| n.node_type).collect::<Vec<_>>(), vec![BlockType::Paragraph]);
    }

    /// C5 守卫：批量 cell_ends 与逐调用 item_subtree_end 逐值等价
    /// （含嵌套/兄弟/PopPush 延续/未闭合帧四类形态）。
    #[test]
    fn cell_ends_batch_equiv() {
        use crate::projection::State;
        let docs = [
            "# T\n- a\n- b\n- c",
            "# T\n- a\n    - b\n        - c\n- d",
            "# T\n> - a: 1\n>   # H\n>   - b: 2",
            "# T\n1. a\n    - b\n2. c",
            "# T\n- users:\n    - admin\n- logging:\n    - level",
            "# T\n> # H4\n> - B3: x",
            "# T\n> bq_text\n> - item1: x\n>   - item2: y",
        ];
        for doc in docs {
            let mut st = State::new(doc);
            let snap = st.snapshot();
            let n = snap.line_len();
            let ends = super::build_cell_ends(&snap.frame_log.entries, n);
            for li in 0..n {
                let t = snap.line_type_at(li);
                if crate::effect::block_decl(t).is_list_item {
                    let expect = crate::infer::item_subtree_end(&snap.frame_log.entries, &snap.open_depth, li, n);
                    assert_eq!(ends[li] as usize, expect,
                        "cell_ends[{li}] != item_subtree_end({li}) for doc {doc:?}");
                }
            }
        }
    }

    /// V3 §6.4 契约：同快照两次 select（第二次 fdc 全命中 children_spec
    /// 缓存）结果逐字段一致——命中重建（build_node）与未命中物化等价。
    #[test]
    fn v3_cache_roundtrip_equiv() {
        use super::{BlockType, Step};        use crate::projection::State;
        let docs = [
            "# S\n\n## A\n- a: 1\n- b: 2\n\npara\n",
            "# S\n> - a\n> - b\n\n# S2\n",
            "- - foo\n- bar\n",
            "> # H4\n> - B3: x\n",
            "# T\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
            "# S\n1. a\n    - b\n2. c\n",
            "> bq_text\n> - item1: x\n>   - item2: y\n",
            "```\ncode\n```\n\npara\n",
            "- a: 1\n- b: 2\n",
        ];
        let node_eq = |a: &super::BlockNode, b: &super::BlockNode| {
            a.start_line == b.start_line
                && a.end_line == b.end_line
                && a.byte_range == b.byte_range
                && a.node_type == b.node_type
                && a.loose == b.loose
                && a.frame_ordinal == b.frame_ordinal
                && a.peel_width == b.peel_width
                && format!("{:?}", a.geometry) == format!("{:?}", b.geometry)
        };
        for doc in docs {
            let mut st = State::new(doc);
            let snap = st.snapshot();
            // 深链 + wildcard + filter + slice 混合形态
            let steps = vec![
                Step::Wildcard,
                Step::Filter(super::FilterExpr::TypeCheck(super::TypeName::Bullet)),
            ];
            let a = super::select_nodes(&snap, &steps);
            let b = super::select_nodes(&snap, &steps);
            assert_eq!(a.len(), b.len(), "len mismatch for {doc:?}");
            for (x, y) in a.iter().zip(b.iter()) {
                assert!(node_eq(x, y),
                    "node mismatch for {doc:?}: {:?} vs {:?}", x, y);
            }
            // 同名 drill 链（target 维度命中）
            let steps2 = vec![Step::Drill("S".into())];
            let a2 = super::select_nodes(&snap, &steps2);
            let b2 = super::select_nodes(&snap, &steps2);
            assert_eq!(a2.len(), b2.len(), "drill len mismatch for {doc:?}");
            for (x, y) in a2.iter().zip(b2.iter()) {
                assert!(node_eq(x, y), "drill node mismatch for {doc:?}: {:?} vs {:?}", x, y);
            }
        }
    }

    /// pass2 二分跳等价性（临时验证）：blocks 升序 + next_line 严格递增 ⟹
    /// partition_point 跳过与线性 continue 的处理序列逐位一致。
    #[test]
    fn pass2_binary_skip_equiv() {
        // 确定性推进 f(l) = l + (l % 4) + 1——严格 > l（模拟 block_end_resolve）
        let f = |l: usize| l + (l % 4) + 1;
        for trial in 0..300usize {
            let n = 30 + (trial * 37) % 70;
            let mut blocks: Vec<usize> = Vec::new();
            let mut s = 0usize;
            for _ in 0..n {
                blocks.push(s);
                s += 1 + (trial * 13 + s) % 3;
            }
            // 线性（现有实现形态）
            let mut nl = 0usize;
            let mut seq_lin = Vec::new();
            for &l in &blocks {
                if l < nl { continue; }
                seq_lin.push(l);
                nl = f(l);
            }
            // 二分跳（候选实现形态）
            let mut i = 0usize;
            let mut nl = 0usize;
            let mut seq_bin = Vec::new();
            while i < blocks.len() {
                let l = blocks[i];
                if l < nl {
                    i = blocks.partition_point(|&b| b < nl);
                    continue;
                }
                seq_bin.push(l);
                nl = f(l);
                i += 1;
            }
            assert_eq!(seq_lin, seq_bin, "trial {trial}: lin={seq_lin:?} bin={seq_bin:?}");
        }
    }

    /// PRC P4b.1 selector equivalence gate: a Base kernel slot must produce
    /// exactly the same `select_nodes` results as the same Snapshot whose Weak
    /// is dangling; an Edit slot must also equal the no-slot fallback.
    #[test]
    fn prc_kernel_selector_equiv_base_and_edit() {
        use super::{BlockNode, FilterExpr, Step, TypeName};
        use crate::cache::CacheSlot;
        use crate::projection::State;
        use std::sync::Arc;

        fn node_eq(a: &BlockNode, b: &BlockNode) -> bool {
            a.start_line == b.start_line
                && a.end_line == b.end_line
                && a.byte_range == b.byte_range
                && a.node_type == b.node_type
                && a.loose == b.loose
                && a.frame_ordinal == b.frame_ordinal
                && a.peel_width == b.peel_width
                && format!("{:?}", a.geometry) == format!("{:?}", b.geometry)
        }

        fn assert_same_nodes(a: &[BlockNode], b: &[BlockNode], doc: &str, steps_desc: &str) {
            assert_eq!(a.len(), b.len(), "len mismatch for {doc:?} / {steps_desc}");
            for (x, y) in a.iter().zip(b.iter()) {
                assert!(
                    node_eq(x, y),
                    "node mismatch for {doc:?} / {steps_desc}: {x:?} vs {y:?}"
                );
            }
        }

        let docs = [
            "# S\n\n## A\n- a: 1\n- b: 2\n\npara\n",
            "# S\n> - a\n> - b\n\n# S2\n",
            "- - foo\n- bar\n",
            "> # H4\n> - B3: x\n",
            "# T\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
            "# S\n1. a\n    - b\n2. c\n",
            "> bq_text\n> - item1: x\n>   - item2: y\n",
            "```\ncode\n```\n\npara\n",
            "- a: 1\n- b: 2\n",
        ];
        let step_sets: Vec<(&str, Vec<Step>)> = vec![
            ("root wildcard", vec![Step::Wildcard]),
            ("recursive wildcard", vec![Step::RecursiveWildcard]),
            (
                "bullet filter",
                vec![
                    Step::Wildcard,
                    Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
                ],
            ),
            ("drill S", vec![Step::Drill("S".into())]),
            ("drill S wildcard", vec![Step::Drill("S".into()), Step::Wildcard]),
        ];

        for doc in docs {
            for (steps_desc, steps) in &step_sets {
                // Fresh snapshot pair per step set: keeps each kernel slot cold
                // and avoids the shared block_cache interval hit masking the
                // kernel path (the clone shares block_cache with its dangling
                // twin, which is the intended fallback comparison).
                let mut st = State::new(doc);
                let snap = st.snapshot();
                let mut with_slot = snap.clone();
                let slot = Arc::new(CacheSlot::new());
                with_slot.set_cache_slot(Arc::downgrade(&slot));
                let kernel_nodes = super::select_nodes(&with_slot, steps);
                let fallback_nodes = super::select_nodes(&snap, steps);
                assert_same_nodes(&kernel_nodes, &fallback_nodes, doc, steps_desc);
                assert!(
                    slot.node.load().is_some(),
                    "Base slot must be lazily built for {doc:?} / {steps_desc}"
                );
            }
        }

        // Edit-node snapshot: kernel routing falls back (no direct Base
        // ancestor / cross-mid), so the result must equal the no-slot
        // fallback result.
        let old_text = "# Title\n\nPara one\n\nAfter paragraph.\n";
        let start = old_text.find("Para one").unwrap();
        let end = start + "Para one".len();
        let mut st = State::new(old_text);
        let old_snap = Arc::new(st.snapshot());
        st.edit(start, end, "- item\n");
        let mut new_snap = st.snapshot();
        let diff = st.ripple_diff.take().expect("edit should produce ripple diff");
        let n = new_snap.line_len();
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(n);
        let dangling = new_snap.clone();

        let slot = Arc::new(CacheSlot::new());
        let mut new_snap_arc = Arc::new(new_snap);
        if let Some(s) = Arc::get_mut(&mut new_snap_arc) {
            s.set_cache_slot(Arc::downgrade(&slot));
        }
        let edit = crate::cache::CacheNode::new_edit(
            &old_snap,
            new_snap_arc.clone(),
            (rls, rle_e),
            diff.delta,
            rls,
        );
        slot.node.store(Some(Arc::new(edit)));

        for (steps_desc, steps) in &step_sets {
            let edit_nodes = super::select_nodes(&*new_snap_arc, steps);
            let fallback_nodes = super::select_nodes(&dangling, steps);
            assert_same_nodes(&edit_nodes, &fallback_nodes, old_text, steps_desc);
        }
    }

}


// A2 selector 归格 gate(判官先行——§9 统一断言:"标题集合相等" +
// "header 块尾 == 全量口径")。身份单源律:身份投影只从全量单源读;
// 局部段算节区间(现状缺陷面)在此判官下必须红,归格后绿。

#[cfg(test)]
mod a2_selector_cell_gate {
    use super::*;
    use std::sync::Arc;
    use crate::cache::{CacheNode, CacheSlot};
    use crate::projection::State;
    use crate::topology::HEADINGSECTION_TOPOLOGY;

    /// 全量口径节区间 oracle(身份单源:全量段 fold——不经 cache、不经 selector)。
    fn oracle_secs(snap: &Snapshot) -> Vec<(usize, usize, u8, crate::block::BlockType)> {
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

    /// 标题集合 + 块尾口径(全量 fold——"header 块尾 == 全量口径"的预言机)。
    fn oracle_headings(snap: &Snapshot) -> Vec<(usize, usize, u8)> {
        let mut v: Vec<(usize, usize, u8)> = oracle_secs(snap)
            .into_iter()
            .map(|(s, e, lv, _)| (s, e, lv))
            .collect();
        v.sort();
        v
    }

    /// 生产路径标题集合:root Wildcard → 逐节点直接子遍历(selector 导航
    /// 固定 HS 章节语义——区间查询逐级收缩,局部段截断缺陷经此面暴露)。
    fn production_headings(snap: &Snapshot) -> Vec<(usize, usize, u8)> {
        let mut out: Vec<(usize, usize, u8)> = Vec::new();
        let mut seen: std::collections::HashSet<(usize, usize, u8)> =
            std::collections::HashSet::new();
        let mut frontier: Vec<crate::block::BlockNode> =
            super::select_nodes(snap, &[Step::Wildcard]);
        let mut guard = 0usize;
        loop {
            if frontier.is_empty() || guard > 256 {
                break;
            }
            guard += 1;
            let mut next: Vec<crate::block::BlockNode> = Vec::new();
            for n in frontier {
                let key = (n.start_line, n.end_line, n.node_type as u8);
                if seen.insert(key) {
                    if crate::block::is_header(n.node_type) {
                        out.push((
                            n.start_line,
                            n.end_line,
                            crate::block::header_level(n.node_type).unwrap_or(0),
                        ));
                    }
                }
                next.extend(super::find_direct_children(snap, &n, None));
            }
            frontier = next;
        }
        out.sort();
        out.dedup();
        out
    }

    /// 生产形态 Edit 快照(seam 同款描述符 + 槽挂 Edit 节点——kernel 路由激活)。
    /// 返回 (旧快照, 新快照, 槽保活)——槽必须随夹具存活,否则 Weak 悬空
    /// 退化为无槽路径(判官测错对象)。
    fn edit_snap(
        base: &str,
        edit: (usize, usize, &str),
    ) -> (Arc<Snapshot>, Arc<Snapshot>, Arc<CacheSlot>) {
        let mut state = State::new(base);
        let old_snap = Arc::new(state.snapshot());
        let (s, e, t) = edit;
        state.edit(s, e, t);
        let mut new_snap = Arc::new(state.snapshot());
        let diff = state.ripple_diff.take().expect("ripple");
        let n = new_snap.line_len();
        let rls = diff.ripple_line_start;
        let rle_e = (diff.ripple_line_end + 1).min(n);
        let anchor = old_snap.line_of_byte(s.min(old_snap.byte_len().saturating_sub(1)));
        let slot = Arc::new(CacheSlot::new());
        // P4b.1 同序:先挂 Weak 再 new_edit_full(clone 后 get_mut 必败)。
        if let Some(sn) = Arc::get_mut(&mut new_snap) {
            sn.set_cache_slot(Arc::downgrade(&slot));
        }
        let node = CacheNode::new_edit_full(
            &old_snap,
            new_snap.clone(),
            (rls, rle_e),
            crate::cache::edit_actions_arc(vec![(s, e)]),
            diff.delta,
            anchor,
        );
        slot.node.store(Some(Arc::new(node)));
        (old_snap, new_snap, slot)
    }

    /// §9 反例集(强制材料):BQ 内 ATX 标题、BQ 内 setext 标题、跨容器 BQ、
    /// 嵌套节。文档带前后段落衬垫使 mid zone 保持局部(小文档 zone 闭包
    /// 吞全文档 → merged=全量 → 无截断面);中段编辑后逐级遍历暴露
    /// sec_n 截断与容器边界重分类的身份缺陷面。
    fn padded_doc(mid: &str, edit_pos: usize, ins: &str) -> (String, usize) {
        let mut doc = String::new();
        for i in 0..12 {
            doc.push_str(&format!("para head {i}\n\n"));
        }
        doc.push_str(mid);
        for i in 0..12 {
            doc.push_str(&format!("para tail {i}\n\n"));
        }
        let pos = edit_pos + doc.find(mid).unwrap_or(0);
        let _ = ins;
        (doc, pos)
    }

    #[test]
    fn a2_heading_set_equals_full_caliber_fixed() {
        // (中段结构, 编辑插入文本) 四反例形。
        let mids: Vec<(&str, &str)> = vec![
            // BQ 内 ATX 标题(容器内标题 + 容器跨界截断)。
            (
                "> # InBq\n> bq body\n> bq body\n> bq body\n> bq body\n\n## S2\n\npara\n\n",
                "## Mid\n\n",
            ),
            // BQ 内 setext 标题(下划线在段界外 → 截断重分类丢标题)。
            (
                "> foo\n> ===\n> more bq\n> more bq\n> more bq\n> more bq\n\n## S2\n\npara\n\n",
                "## Mid\n\n",
            ),
            // 跨容器 BQ(容器跨界 + 容器内标题)。
            (
                "> bq line\n> bq line\n> - item\n> # InBq\n> tail\n> tail\n\n## S2\n\npara\n\n",
                "- mid\n",
            ),
            // 嵌套节(层级结构 + 中段编辑)。
            (
                "# A\n\n## B\n\ntext b\n\n# C\n\n## D\n\ntext d\n\n# E\n\npara\n\n",
                "## Mid\n\n",
            ),
        ];
        for (i, (mid, ins)) in mids.iter().enumerate() {
            // 编辑插在中段结构的中部(衬垫之后的 mid 区间内)。
            let (doc, pos) = padded_doc(mid, mid.len() / 2, ins);
            let (_old, new_snap, _slot) = edit_snap(&doc, (pos, pos, ins));
            let prod = production_headings(&new_snap);
            let oracle = oracle_headings(&new_snap);
            assert_eq!(
                prod, oracle,
                "fixed case {i}: 标题集合与块尾必须同全量口径(身份单源)\n  prod  ={prod:?}\n  oracle={oracle:?}"
            );
        }
    }

    /// A2 主 gate(判官先行,生产同构直驱):scan_blocks 的身份产物
    /// (secs + settled header 子集)必须同全量口径。参数与生产
    /// find_direct_children(861 行)同构:shared_structure_for_range +
    /// kernel_merged_zone + scan_blocks——场景可达性由构造保证。
    /// 断言:①标题集合相等(secs 三元组 == 全量 oracle 的查询重叠面);
    /// ②header 块尾 == 全量口径(settled 中 header 条目 == 全量 fold 的
    /// 容器截断 bound,非查询尾截断)。
    fn full_fold(
        snap: &Snapshot,
    ) -> (
        Vec<(usize, usize, u8, crate::block::BlockType)>,
        std::collections::HashMap<(usize, u8), usize>,
    ) {
        let n = snap.line_len();
        let seg = crate::cache::Segment::build(snap, 0, n);
        let secs = crate::infer::section_intervals(
            &seg.blocks,
            &seg.records,
            &seg.arena,
            HEADINGSECTION_TOPOLOGY.heading_candidates,
            n,
        );
        let mut hb = std::collections::HashMap::new();
        for b in seg.blocks.iter() {
            if crate::block::is_header(b.block_type) && b.container_ref != u32::MAX {
                let rec = &seg.records[b.container_ref as usize];
                let bound = rec
                    .push_lines_of(&seg.arena)
                    .find(|l| *l > b.start)
                    .map(|l| l as usize)
                    .unwrap_or_else(|| rec.end_usize().unwrap_or(n));
                hb.insert((b.start as usize, rec.frame.kind() as u8), bound);
            }
        }
        (secs, hb)
    }

    fn check_query(snap: &Snapshot, s: usize, e: usize, desc: &str) {
        let (o_secs, o_hb) = full_fold(snap);
        let fb = crate::cache::kernel_merged_zone(snap, s, e);
        let mut bc = BlockBoundaryCache::new();
        let seg_in = super::shared_structure_for_range(snap, s, e);
        let kernel_starts = crate::cache::kernel_block_starts(snap);
        let (_blocks, settled, _seg, secs) = super::scan_blocks(
            snap,
            &mut bc,
            seg_in,
            s,
            e,
            kernel_starts.as_ref().map(|v| v.as_slice()),
            fb,
        );
        // ① 标题集合相等:secs 三元组 == 全量 oracle 的查询重叠面。
        let mut prod_secs: Vec<(usize, usize, u8, crate::block::BlockType)> = secs
            .iter()
            .filter(|&&(ss, ee, _, _)| ss < e && ee > s)
            .copied()
            .collect();
        prod_secs.sort_by_key(|t| (t.0, t.1, t.2));
        let mut want_secs: Vec<(usize, usize, u8, crate::block::BlockType)> = o_secs
            .iter()
            .filter(|&&(ss, ee, _, _)| ss < e && ee > s)
            .copied()
            .collect();
        want_secs.sort_by_key(|t| (t.0, t.1, t.2));
        assert_eq!(
            prod_secs, want_secs,
            "{desc}: secs 标题集合/块尾必须同全量口径\n  zone   ={fb:?} query=[{s},{e})\n  prod  ={prod_secs:?}\n  oracle={want_secs:?}"
        );
        // ② header 块尾 == 全量口径:settled 中 header 条目(哨兵位键)必为
        // 全量容器截断。
        for ((hs, hk), he) in settled.iter() {
            if *hk & HEADER_BOUND_KEY_FLAG != 0 {
                let rk = *hk & !HEADER_BOUND_KEY_FLAG;
                if let Some(&want) = o_hb.get(&(*hs, rk)) {
                    assert_eq!(
                        *he, want,
                        "{desc}: settled header bound {hs}/{rk} = {he} 必须 == 全量口径 {want}(禁查询尾截断)"
                    );
                }
            }
        }
    }

    /// 生产同构直驱 gate:固定反例(§9 强制材料)+ 各 oracle 章节区间查询。
    #[test]
    fn a2_secs_product_equals_full_caliber_fixed() {
        fn pad(n: usize) -> String {
            let mut s = String::new();
            for i in 0..n {
                s.push_str(&format!("para pad {i}\n\n"));
            }
            s
        }
        let mids: Vec<&str> = vec![
            "> # InBq\n> bq body\n> bq body\n> bq body\n> bq body\n\n## S2\n\npara\n\n",
            "> foo\n> ===\n> more bq\n> more bq\n> more bq\n> more bq\n\n## S2\n\npara\n\n",
            "> bq line\n> bq line\n> - item\n> # InBq\n> tail\n> tail\n\n## S2\n\npara\n\n",
            "# A\n\n## B\n\ntext b\n\n# C\n\n## D\n\ntext d\n\n# E\n\npara\n\n",
        ];
        for (i, mid) in mids.iter().enumerate() {
            let doc = format!("{}{}{}", pad(8), mid, pad(8));
            let pos = doc.find("para pad 7").unwrap() + 8 + mid.len() / 2;
            let (_old, new_snap, _slot) = edit_snap(&doc, (pos, pos, "## Mid\n\n"));
            let (o_secs, _) = full_fold(&new_snap);
            for (k, &(s, e, _, _)) in o_secs.iter().enumerate() {
                check_query(&new_snap, s, e, &format!("fixed {i} sec {k}"));
            }
            check_query(&new_snap, 0, new_snap.line_len(), &format!("fixed {i} full"));
        }
    }

    /// 生产同构直驱 gate:300 长文档 × 随机结构编辑 × 全章节区间查询。
    #[test]
    fn a2_secs_product_equals_full_caliber_random() {
        fn next_rand(state: &mut u64) -> u64 {
            *state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *state
        }
        let tokens: &[&str] = &[
            "plain paragraph\n",
            "some **bold** text\n",
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
        let mut rng: u64 = 0x2A2F_4B11_DEAD_0001;
        for case in 0..300u64 {
            let mut doc = String::new();
            let head_pad = 8 + (next_rand(&mut rng) % 10) as usize;
            for i in 0..head_pad {
                doc.push_str(&format!("para head {case}-{i}\n\n"));
            }
            let line_count = 12 + (next_rand(&mut rng) % 16) as usize;
            for _ in 0..line_count {
                doc.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
            }
            let tail_pad = 8 + (next_rand(&mut rng) % 10) as usize;
            for i in 0..tail_pad {
                doc.push_str(&format!("para tail {case}-{i}\n\n"));
            }
            let len = doc.len();
            let mid = len / 2;
            let pos = mid + (next_rand(&mut rng) as usize % (len / 8).max(1)).min(len - mid);
            let ins = if next_rand(&mut rng) % 2 == 0 { "## Mid\n\n" } else { "- x\n" };
            let (_old, new_snap, _slot) = edit_snap(&doc, (pos, pos, ins));
            let (o_secs, _) = full_fold(&new_snap);
            for (k, &(s, e, _, _)) in o_secs.iter().enumerate() {
                check_query(&new_snap, s, e, &format!("case {case} sec {k}"));
            }
            check_query(&new_snap, 0, new_snap.line_len(), &format!("case {case} full"));
        }
    }

    /// 随机对照:300 长文档(40-90 行,前后衬垫保证 mid zone 局部)× 随机
    /// 结构编辑——生产遍历标题集合 == 全量 oracle。
    #[test]
    fn a2_heading_set_equals_full_caliber_random() {
        fn next_rand(state: &mut u64) -> u64 {
            *state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            *state
        }
        let tokens: &[&str] = &[
            "plain paragraph\n",
            "some **bold** text\n",
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
            "| a | b |\n|---|---|\n| c | d |\n",
        ];
        let mut rng: u64 = 0x2A2F_4B11_DEAD_0001;
        let mut mismatches = 0usize;
        for case in 0..300u64 {
            let mut doc = String::new();
            // 前衬垫(保证编辑前的 zone 边界远离文档头)。
            let head_pad = 8 + (next_rand(&mut rng) % 10) as usize;
            for i in 0..head_pad {
                doc.push_str(&format!("para head {case}-{i}\n\n"));
            }
            let line_count = 10 + (next_rand(&mut rng) % 16) as usize;
            for _ in 0..line_count {
                doc.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
            }
            // 后衬垫(保证 merged 尾远离文档尾)。
            let tail_pad = 8 + (next_rand(&mut rng) % 10) as usize;
            for i in 0..tail_pad {
                doc.push_str(&format!("para tail {case}-{i}\n\n"));
            }
            let len = doc.len();
            let mid = len / 2;
            let pos = mid + (next_rand(&mut rng) as usize % (len / 8).max(1)).min(len - mid);
            let ins = if next_rand(&mut rng) % 2 == 0 { "## Mid\n\n" } else { "- x\n" };
            let (_old, new_snap, _slot) = edit_snap(&doc, (pos, pos, ins));
            let prod = production_headings(&new_snap);
            let oracle = oracle_headings(&new_snap);
            if prod != oracle {
                mismatches += 1;
                if mismatches <= 3 {
                    eprintln!(
                        "[A2-GATE] mismatch case={case} edit@{pos}\n  doc   ={doc:?}\n  prod  ={prod:?}\n  oracle={oracle:?}"
                    );
                }
            }
        }
        assert_eq!(
            mismatches, 0,
            "production heading set must equal full-caliber oracle: {mismatches}/300 mismatches"
        );

    }
}
#[cfg(test)]
mod light_opt_tests {
    use super::*;
    use crate::projection::State;

    fn snap_of(text: &str) -> Snapshot {
        let mut state = State::new(text);
        state.snapshot()
    }

    fn doc() -> String {
        let mut s = String::from("# S\n");
        for i in 0..40 { s.push_str(&format!("- k{i}: v{i}\n")); }
        for i in 0..20 { s.push_str(&format!("- empty{i}:\n")); }
        for i in 0..10 { s.push_str(&format!("- [x] done{i}: yes\n")); }
        s
    }

    fn tag_eq_cmp(target: &str) -> FilterExpr {
        FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq, value: Value::String(target.into()) }
    }

    fn sig(nodes: &[BlockNode]) -> Vec<(usize, u8)> {
        nodes.iter().map(|n| (n.start_line, n.node_type as u8)).collect()
    }

    /// 未 fuse 裸步骤(直调路径形态)与 fuse 后形态输出一致——
    /// normalize_steps 的等价性(同 py fuse 规则)。
    #[test]
    fn normalize_equiv_bare_vs_fused() {
        let snap = snap_of(&doc());
        // 裸形态: [Drill, Wildcard, Filter*, Slice] (hook 注册/直调)
        let bare = vec![
            Step::Drill("S".into()),
            Step::Wildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
            Step::Filter(tag_eq_cmp("k5")),
            Step::Slice(vec![SingleSlice::Index(0)]),
        ];
        // fuse 后形态(同 py fuse_steps 输出)
        let fused = vec![
            Step::Drill("S".into()),
            Step::TraversalFilter {
                traversal: Box::new(Step::Wildcard),
                filters: vec![
                    FilterExpr::TypeCheck(TypeName::Bullet),
                    tag_eq_cmp("k5"),
                ],
                limit: 1,
            },
        ];
        let a = sig(&select_nodes(&snap, &bare));
        let b = sig(&select_nodes(&snap, &fused));
        assert_eq!(a, b, "裸步骤(经 normalize)与 fuse 后输出应一致");
        // normalize 幂等
        let n1 = normalize_steps(&bare);
        let n2 = normalize_steps(&n1);
        assert_eq!(n1.len(), n2.len(), "normalize 幂等");
    }

    /// 轻判路径等价: 同快照首查(fresh)与二次(轻判)输出一致。
    #[test]
    fn light_path_fresh_vs_cached() {
        let snap = snap_of(&doc());
        let steps = vec![Step::Drill("S".into()), Step::TraversalFilter {
            traversal: Box::new(Step::Wildcard),
            filters: vec![FilterExpr::TypeCheck(TypeName::Bullet), tag_eq_cmp("k5")],
            limit: 0,
        }];
        let fresh = sig(&select_nodes(&snap, &steps));
        let cached = sig(&select_nodes(&snap, &steps));
        assert_eq!(fresh, cached);
    }
}

