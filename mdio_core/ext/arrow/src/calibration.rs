//! μ-driven mechanism calibration for the arrow facade.
//!
//! Framework, not a bug list. Documents are enumerated with the μ state
//! machine; a registry of independent checkers validates the mechanisms the
//! arrow tables are built on. Every checker declares its oracle layer:
//!
//! - IndependentTree:    render-tree walker (can catch core identity)
//! - IndependentGeometry: read face vs geometry write range (can catch peel)
//! - MuModel:            μ events vs production frame log (can catch scanner)
//! - SelfConsistency:    facade-internal relations (edges/desc/sections)
//! - Roundtrip:          canonical render == source (emit/render layer)
//!
//! Findings carry a normalized structural signature; witnesses are
//! delta-minimized per (checker, signature). Enabled with
//! `--features calibration`; does not touch DuckDB at all.

use std::collections::{HashMap, HashSet};
use std::fmt;

use mdio_core::block::{block_proj_spec, entry_frame, entry_is_body_mut, entry_line, BlockNode, BlockProj, BlockType};
use mdio_core::projection::{project_text, State};
use mdio_core::topology::ProjectionMode;
use mdio_core::trinity::exhaustive_v2::{
    enumerate_docs, enumerate_where, extend_doc, line_classes, mu_doc, mu_events_for, Doc, EventOp,
    MuState, StateKey,
};
use mdio_core::trinity::mechanism::tree_roots;

use mdio_core::arrow::facade::{self, Face, CONTENT_LEAF_FRAME_ORDINAL};

// ── registry types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckerName {
    NonWf,
    Edges,
    Roots,
    Ordinal,
    Span,
    Face,
    FlEvents,
    Desc,
    Sections,
    RenderAll,
    Projection,
    Cmark,
}

impl CheckerName {
    pub const ALL: [CheckerName; 12] = [
        CheckerName::NonWf,
        CheckerName::Edges,
        CheckerName::Roots,
        CheckerName::Ordinal,
        CheckerName::Span,
        CheckerName::Face,
        CheckerName::FlEvents,
        CheckerName::Desc,
        CheckerName::Sections,
        CheckerName::RenderAll,
        CheckerName::Projection,
        CheckerName::Cmark,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            CheckerName::NonWf => "non_wf",
            CheckerName::Edges => "edges",
            CheckerName::Roots => "roots",
            CheckerName::Ordinal => "ordinal",
            CheckerName::Span => "span",
            CheckerName::Face => "face",
            CheckerName::FlEvents => "fl",
            CheckerName::Desc => "desc",
            CheckerName::Sections => "sections",
            CheckerName::RenderAll => "render_all",
            CheckerName::Projection => "projection",
            CheckerName::Cmark => "cmark",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleKind {
    IndependentTree,
    IndependentGeometry,
    MuModel,
    SelfConsistency,
    Roundtrip,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub signature: &'static str,
    pub detail: String,
}

pub struct CheckCtx<'a> {
    pub doc: &'a Doc,
    pub snap: &'a mdio_core::projection::Snapshot,
    pub data: Option<&'a facade::FacadeData>,
    pub budget: &'a Budget,
}

#[derive(Debug, Default, Clone)]
pub struct Budget {
    /// Per-checker document cap; missing entry = unlimited. The cmark checker
    /// is expensive (one python + two subprocesses per doc) and should always
    /// get an explicit cap.
    pub per_checker: HashMap<CheckerName, usize>,
}

impl Budget {
    pub fn limit(&self, name: CheckerName) -> Option<usize> {
        match self.per_checker.get(&name) {
            Some(v) => Some(*v),
            // The external cmark checker costs a python + two subprocesses per
            // doc; it must be opted into explicitly.
            None if name == CheckerName::Cmark => Some(0),
            None => None,
        }
    }
}

pub trait Checker {
    fn name(&self) -> CheckerName;
    fn oracle(&self) -> OracleKind;
    fn requires_facade(&self) -> bool;
    /// Independent-layer checkers (tree/mu/geometry) run first; a doc failing
    /// any of them skips the SelfConsistency/Roundtrip layers.
    fn independent_layer(&self) -> bool;
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding>;
}

// ── checkers ───────────────────────────────────────────────────────────────

struct NonWfChecker;
impl Checker for NonWfChecker {
    fn name(&self) -> CheckerName { CheckerName::NonWf }
    fn oracle(&self) -> OracleKind { OracleKind::SelfConsistency }
    fn requires_facade(&self) -> bool { false }
    fn independent_layer(&self) -> bool { true }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        if let Err(e) = facade::build(ctx.snap) {
            let msg = format!("{e}");
            let signature = if msg.contains("phantom") {
                "non_wf/phantom_child"
            } else if msg.contains("duplicate") {
                "non_wf/duplicate_key"
            } else {
                "non_wf/build_error"
            };
            vec![Finding { signature, detail: format!("facade::build rejected: {msg}") }]
        } else {
            Vec::new()
        }
    }
}

struct EdgesChecker;
impl Checker for EdgesChecker {
    fn name(&self) -> CheckerName { CheckerName::Edges }
    fn oracle(&self) -> OracleKind { OracleKind::SelfConsistency }
    fn requires_facade(&self) -> bool { true }
    fn independent_layer(&self) -> bool { false }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let data = ctx.data.expect("requires facade");
        let n = data.node_rows.len() as u32;
        for e in &data.edges {
            if e.parent >= n || e.child >= n {
                return vec![Finding {
                    signature: "edges/out_of_range",
                    detail: format!("edge endpoint out of range: {e:?}"),
                }];
            }
        }
        Vec::new()
    }
}

struct RootsChecker;
impl Checker for RootsChecker {
    fn name(&self) -> CheckerName { CheckerName::Roots }
    fn oracle(&self) -> OracleKind { OracleKind::IndependentTree }
    fn requires_facade(&self) -> bool { true }
    fn independent_layer(&self) -> bool { true }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let Some(data) = ctx.data else {
            return vec![Finding {
                signature: "roots/missing_facade",
                detail: format!("facade data absent for {:?}", ctx.doc.text),
            }];
        };
        let mut ref_roots = HashSet::new();
        let child_set: HashSet<u32> = data.edges.iter().map(|e| e.child).collect();
        for (start, end, block_type) in tree_roots(ctx.snap) {
            // identity emits one node per PopPush item; tree roots are whole
            // list containers. Accept the identity node set whose
            // [start_line, end_line) union covers the tree root interval.
            let mut spans: Vec<(u32, u32, u32)> = data
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, x)| {
                    x.node_type == block_type
                        && x.start_line as u32 >= start
                        && x.end_line as u32 <= end
                })
                .map(|(i, x)| (x.start_line as u32, x.end_line as u32, i as u32))
                .collect();
            spans.sort();
            let mut cur = start;
            for (s, e, _i) in &spans {
                if *s <= cur && *e > cur {
                    cur = *e;
                }
            }
            if cur < end {
                return vec![Finding {
                    signature: "roots/missing_shell",
                    detail: format!("tree root ({start}, {end}, {}) not covered by identity: {spans:?}", block_type as u8),
                }];
            }
            for (_s, _e, i) in spans {
                if !child_set.contains(&i) {
                    ref_roots.insert(i);
                }
            }
        }
        let actual: HashSet<u32> = data.roots.iter().map(|r| r.node).collect();
        if actual == ref_roots {
            return Vec::new();
        }
        let extra: Vec<u32> = actual.difference(&ref_roots).copied().collect();
        let extra_leaf = extra.iter().all(|i| {
            data.node_rows
                .get(*i as usize)
                .map(|r| r.frame_ordinal == CONTENT_LEAF_FRAME_ORDINAL)
                .unwrap_or(false)
        });
        let signature = if extra_leaf { "roots/extra_leaf" } else { "roots/mismatch" };
        vec![Finding {
            signature,
            detail: format!("roots mismatch: tree={ref_roots:?} identity={actual:?}"),
        }]
    }
}

struct OrdinalChecker;
impl Checker for OrdinalChecker {
    fn name(&self) -> CheckerName { CheckerName::Ordinal }
    fn oracle(&self) -> OracleKind { OracleKind::MuModel }
    fn requires_facade(&self) -> bool { true }
    fn independent_layer(&self) -> bool { true }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let data = ctx.data.expect("requires facade");
        let mut pushes: HashMap<usize, usize> = HashMap::new();
        for ev in &ctx.doc.events {
            if ev.op == EventOp::P {
                *pushes.entry(ev.line).or_default() += 1;
            }
        }
        for row in &data.node_rows {
            if row.frame_ordinal == CONTENT_LEAF_FRAME_ORDINAL { continue; }
            let p = pushes.get(&(row.start_line as usize)).copied().unwrap_or(0);
            if (row.frame_ordinal as usize) >= p {
                return vec![Finding {
                    signature: "ordinal/out_of_order",
                    detail: format!(
                        "node {} at line {} has frame_ordinal {} but μ opens {p} frames",
                        row.node, row.start_line, row.frame_ordinal
                    ),
                }];
            }
        }
        Vec::new()
    }
}

struct SpanChecker;
impl Checker for SpanChecker {
    fn name(&self) -> CheckerName { CheckerName::Span }
    fn oracle(&self) -> OracleKind { OracleKind::MuModel }
    fn requires_facade(&self) -> bool { true }
    fn independent_layer(&self) -> bool { true }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let data = ctx.data.expect("requires facade");
        for (i, node) in data.nodes.iter().enumerate() {
            let range = node.byte_range.clone();
            if range.end > ctx.doc.text.len() {
                return vec![Finding { signature: "span/range_out", detail: format!("node {i} range {range:?} beyond text") }];
            }
            let text_slice = &ctx.doc.text[range.clone()];
            let raw = ctx.snap.byte_slice(range.clone()).to_string();
            if raw != text_slice {
                return vec![Finding { signature: "span/raw_mismatch", detail: format!("node {i} raw mismatch: {raw:?}") }];
            }
            match facade::read_face(ctx.snap, node, Face::Raw) {
                Some(f) if f == text_slice => {}
                other => return vec![Finding { signature: "span/face_mismatch", detail: format!("node {i} read_face raw = {other:?}") }],
            }
        }
        Vec::new()
    }
}

struct FaceChecker;
impl Checker for FaceChecker {
    fn name(&self) -> CheckerName { CheckerName::Face }
    fn oracle(&self) -> OracleKind { OracleKind::IndependentGeometry }
    fn requires_facade(&self) -> bool { true }
    fn independent_layer(&self) -> bool { true }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let Some(data) = ctx.data else {
            return vec![Finding {
                signature: "face/missing_facade",
                detail: "facade data absent despite requires_facade guard".to_string(),
            }];
        };
        for (i, node) in data.nodes.iter().enumerate() {
            if !block_proj_spec(node.node_type).can_write.contains(BlockProj::Tag) {
                continue;
            }
            let Some(read_tag) = facade::read_face(ctx.snap, node, Face::Tag) else { continue };
            // The write-range projection gives the tag span *before* the
            // write; the read face must be exactly that span's text.
            match ctx.snap.proj_write_range(&node.byte_range, node.node_type, "tag") {
                Ok(range) if range.start <= range.end && range.end <= ctx.doc.text.len() => {
                    let geometry_slice = ctx.snap.byte_slice(range.clone()).to_string();
                    if read_tag != geometry_slice {
                        return vec![Finding {
                            signature: "face/tag_range_mismatch",
                            detail: format!("node {i} tag read {read_tag:?} != range slice {geometry_slice:?}"),
                        }];
                    }
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
        // Mark face: read_face(Mark) == rope[line_start + peel_width ..
        // + mark_text_end] (the display mark, without the mandatory space).
        // Geometry comes from the identity node itself (chain-aware after S1),
        // not re-derived with a peel-all build_node.
        for (i, node) in data.nodes.iter().enumerate() {
            if !block_proj_spec(node.node_type).can_read.contains(BlockProj::Mark) {
                continue;
            }
            let Some(geo) = node.geometry.as_ref() else { continue };
            let peel_width = node.peel_width;
            let line_start = ctx.snap.byte_of_line(node.start_line);
            let start = line_start + peel_width;
            let end = start + geo.mark_text_end;
            if end > ctx.doc.text.len() {
                continue;
            }
            let expected = ctx.snap.byte_slice(start..end).to_string();
            if let Some(read_mark) = facade::read_face(ctx.snap, node, Face::Mark) {
                if read_mark != expected {
                    return vec![Finding {
                        signature: "face/mark_range_mismatch",
                        detail: format!("node {i} mark read {read_mark:?} != geometry slice {expected:?}"),
                    }];
                }
            }
        }
        // Content face (single-line, non-BQ nodes): read_face(Content) ==
        // rope[proj_write_range(node, "content")] trimmed of trailing newline.
        // BQ content write range covers the prefixed line, so it is excluded.
        for (i, node) in data.nodes.iter().enumerate() {
            if node.node_type == BlockType::Blockquote {
                continue;
            }
            if !block_proj_spec(node.node_type).can_read.contains(BlockProj::Content) {
                continue;
            }
            if node.end_line.saturating_sub(node.start_line) > 1 {
                continue;
            }
            let Ok((range, _text)) = ctx.snap.compute_write(&node.byte_range, node.node_type, "content", "") else {
                continue;
            };
            if range.start > range.end || range.end > ctx.doc.text.len() {
                continue;
            }
            // identity 节点是首行级：其读面不含子块。key-like 的 ct 写槽现在
            // 会跨非列表子块（树 item 节点的跨度），超出 identity 节点范围时
            // 该面归树 item，不在 identity 面校验（服务端读面用的是树节点）。
            if range.end > node.byte_range.end {
                continue;
            }
            // When the content range covers the whole line, the container
            // prefix is inside it; strip the node's own peel width (chain-aware)
            // only in that case.
            let peel = if range.start == node.byte_range.start {
                node.peel_width.min(range.len())
            } else {
                0
            };
            let expected = ctx
                .snap
                .byte_slice((range.start + peel)..range.end)
                .to_string()
                .trim_end_matches(['\n', '\r'])
                .to_string();
            if let Some(read_content) = facade::read_face(ctx.snap, node, Face::Content) {
                if read_content != expected {
                    return vec![Finding {
                        signature: "face/content_range_mismatch",
                        detail: format!("node {i} content read {read_content:?} != range slice {expected:?}"),
                    }];
                }
            }
        }
        Vec::new()
    }
}

struct FlEventsChecker;
impl Checker for FlEventsChecker {
    fn name(&self) -> CheckerName { CheckerName::FlEvents }
    fn oracle(&self) -> OracleKind { OracleKind::MuModel }
    fn requires_facade(&self) -> bool { false }
    fn independent_layer(&self) -> bool { true }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let Some(mu) = mu_events_for(&ctx.doc.text) else {
            return vec![Finding { signature: "fl/mu_unparseable", detail: "μ cannot parse".to_string() }];
        };
        let mut st = State::new(&ctx.doc.text);
        let snap = st.snapshot();
        let fl: Vec<(EventOp, mdio_core::block::BlockFrame)> = snap
            .frame_log
            .entries
            .iter()
            .map(|&e| {
                let f = entry_frame(e);
                if entry_is_body_mut(e) { (EventOp::B, f) }
                else if (e >> 63) != 0 { (EventOp::Q, f) }
                else { (EventOp::P, f) }
            })
            .collect();
        if fl != mu {
            let sig = if let Some(i) = (0..fl.len().min(mu.len())).find(|&i| fl[i] != mu[i]) {
                // Divergence inside the common prefix: report which side
                // produced the unexpected event.
                match (fl[i].0, mu[i].0) {
                    (EventOp::Q, _) => "fl/diverges_extra_q",
                    (_, EventOp::Q) => "fl/diverges_extra_q_mu",
                    (EventOp::B, _) => "fl/diverges_extra_b",
                    (_, EventOp::B) => "fl/diverges_extra_b_mu",
                    _ => "fl/diverges",
                }
            } else if fl.len() > mu.len() {
                let extra = &fl[mu.len()..];
                if extra.iter().all(|(op, _)| *op == EventOp::Q) { "fl/missing_q" }
                else if extra.iter().all(|(op, _)| *op == EventOp::B) { "fl/missing_b" }
                else { "fl/missing_tail" }
            } else {
                "fl/extra_mu_tail"
            };
            return vec![Finding {
                signature: sig,
                detail: format!("fl={fl:?} mu={mu:?}"),
            }];
        }
        Vec::new()
    }
}

struct DescChecker;
impl Checker for DescChecker {
    fn name(&self) -> CheckerName { CheckerName::Desc }
    fn oracle(&self) -> OracleKind { OracleKind::SelfConsistency }
    fn requires_facade(&self) -> bool { true }
    fn independent_layer(&self) -> bool { false }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let Some(data) = ctx.data else {
            return vec![Finding {
                signature: "desc/missing_facade",
                detail: format!("facade absent for doc {:?}", ctx.doc.text),
            }];
        };
        let n = data.node_rows.len();
        let got = match facade::descendants(&data.edges, n) {
            Ok(d) => d,
            Err(e) => return vec![Finding { signature: "desc/error", detail: format!("{e}") }],
        };
        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); n];
        for e in &data.edges {
            if (e.parent as usize) < n { adj[e.parent as usize].push(e.child); }
        }
        let mut expect: HashSet<(u32, u32, u32)> = HashSet::new();
        for a in 0..n {
            let mut q = std::collections::VecDeque::new();
            let mut depth = HashMap::new();
            q.push_back(a as u32);
            depth.insert(a as u32, 0u32);
            while let Some(x) = q.pop_front() {
                let d = depth[&x];
                for &c in &adj[x as usize] {
                    if !depth.contains_key(&c) {
                        depth.insert(c, d + 1);
                        expect.insert((a as u32, c, d + 1));
                        q.push_back(c);
                    }
                }
            }
        }
        let got_set: HashSet<(u32, u32, u32)> = got.iter().map(|g| (g.ancestor, g.descendant, g.depth)).collect();
        if got_set != expect {
            return vec![Finding {
                signature: "desc/count_mismatch",
                detail: format!("descendants closure mismatch: got {} expected {}", got_set.len(), expect.len()),
            }];
        }
        Vec::new()
    }
}

struct SectionsChecker;
impl Checker for SectionsChecker {
    fn name(&self) -> CheckerName { CheckerName::Sections }
    fn oracle(&self) -> OracleKind { OracleKind::SelfConsistency }
    fn requires_facade(&self) -> bool { true }
    fn independent_layer(&self) -> bool { false }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let data = ctx.data.expect("requires facade");
        for m in &data.members {
            let Some(node) = data.node_rows.get(m.node as usize) else {
                return vec![Finding { signature: "sections/member_range", detail: format!("member node {} out of range", m.node) }];
            };
            let Some(sec) = data.sections.iter().find(|s| s.section == m.section) else {
                return vec![Finding { signature: "sections/unknown_section", detail: format!("member {} section {}", m.node, m.section) }];
            };
            if (node.start_line as u32) < sec.start || (node.end_line as u32) > sec.end {
                return vec![Finding {
                    signature: "sections/member_outside",
                    detail: format!("member {} span [{},{}] outside [{},{}]", m.node, node.start_line, node.end_line, sec.start, sec.end),
                }];
            }
        }
        for s in &data.sections {
            if (s.header_node as usize) >= data.node_rows.len() {
                return vec![Finding { signature: "sections/header_range", detail: format!("section {} header_node {} out of range", s.section, s.header_node) }];
            }
        }
        Vec::new()
    }
}

struct RenderAllChecker;
impl Checker for RenderAllChecker {
    fn name(&self) -> CheckerName { CheckerName::RenderAll }
    fn oracle(&self) -> OracleKind { OracleKind::Roundtrip }
    fn requires_facade(&self) -> bool { false }
    fn independent_layer(&self) -> bool { false }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let rendered = ctx.snap.render_md_all(ProjectionMode::Hs);
        if rendered != ctx.doc.text {
            // Lazy BQ continuation: the source may omit a `>` prefix that the
            // canonical render re-emits. Accept lines that only gained one
            // leading `> ` (canonical re-prefixation), not an emit bug.
            if lazy_prefix_only(&rendered, &ctx.doc.text) {
                return Vec::new();
            }
            let pos = rendered
                .bytes()
                .zip(ctx.doc.text.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| rendered.len().min(ctx.doc.text.len()));
            let line = ctx.doc.text[..pos.min(ctx.doc.text.len())]
                .bytes()
                .filter(|&b| b == b'\n')
                .count();
            return vec![Finding {
                signature: "render_all/divergence",
                detail: format!("canonical render diverges at byte {pos} line {line}: got {rendered:?} want {:?}", ctx.doc.text),
            }];
        }
        Vec::new()
    }
}

/// True when `got` equals `want` except that some lines gained one leading
/// `> ` (canonical re-prefixation of lazy BQ continuation).
fn lazy_prefix_only(got: &str, want: &str) -> bool {
    let g: Vec<&str> = got.lines().collect();
    let w: Vec<&str> = want.lines().collect();
    g.len() == w.len()
        && g.iter().zip(w.iter()).all(|(a, b)| {
            a == b || a.strip_prefix("> ").map(|rest| rest == *b).unwrap_or(false)
        })
}

struct ProjectionChecker;
impl Checker for ProjectionChecker {
    fn name(&self) -> CheckerName { CheckerName::Projection }
    fn oracle(&self) -> OracleKind { OracleKind::Roundtrip }
    fn requires_facade(&self) -> bool { true }
    fn independent_layer(&self) -> bool { false }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        let data = ctx.data.expect("requires facade");
        let all = ctx.snap.render_md_all(ProjectionMode::Hs);
        // identity emits shells (ord<255) plus content leaves (ord==255) that
        // overlap: a shell renders the whole line and its leaves duplicate the
        // content. For the parity oracle select, per start_line, the outermost
        // shell (min ordinal) and drop leaves fully covered by those shells,
        // preserving the original node order.
        let mut min_ordinal: HashMap<usize, u8> = HashMap::new();
        for n in data.nodes.iter().filter(|n| n.frame_ordinal != CONTENT_LEAF_FRAME_ORDINAL) {
            let entry = min_ordinal.entry(n.start_line).or_insert(u8::MAX);
            if n.frame_ordinal < *entry {
                *entry = n.frame_ordinal;
            }
        }
        let mut selected: Vec<BlockNode> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for n in &data.nodes {
            if n.frame_ordinal != CONTENT_LEAF_FRAME_ORDINAL {
                if min_ordinal.get(&n.start_line) != Some(&n.frame_ordinal) {
                    continue;
                }
                // A shell whose line lies inside an already-selected outer
                // shell is rendered by that shell; drop it.
                let inside = spans
                    .iter()
                    .any(|(s, e)| n.start_line >= *s && n.start_line < *e);
                if inside {
                    continue;
                }
                spans.push((n.start_line, n.end_line));
                selected.push(n.clone());
                continue;
            }
            let covered = if mdio_core::effect::block_decl(n.node_type).is_list_item {
                spans.iter().any(|(s, e)| n.start_line >= *s && n.start_line <= *e)
            } else {
                spans.iter().any(|(s, e)| n.start_line >= *s && n.end_line <= *e)
            };
            if !covered {
                selected.push(n.clone());
            }
        }
        // Layout blank lines are not part of selected output (R0 contract),
        // while render_all keeps them; collapse blank runs on both sides.
        fn collapse_blank_runs(s: &str) -> String {
            let mut out = s.to_string();
            loop {
                let next = out.replace("\n\n", "\n");
                if next == out {
                    return out;
                }
                out = next;
            }
        }
        match ctx.snap.render_md_selected(ProjectionMode::Hs, &selected) {
            Ok(sel)
                if collapse_blank_runs(sel.trim_matches('\n'))
                    == collapse_blank_runs(all.trim_matches('\n')) =>
            {
                Vec::new()
            }
            Ok(sel) => {
                let sig = if ctx.doc.text.trim().is_empty() { "projection/blank_envelope" } else { "projection/identity_mismatch" };
                vec![Finding { signature: sig, detail: format!("identity selection != render_all: {sel:?}") }]
            }
            Err(e) => vec![Finding { signature: "projection/error", detail: format!("identity selection failed: {e}") }],
        }
    }
}

struct CmarkChecker;
impl Checker for CmarkChecker {
    fn name(&self) -> CheckerName { CheckerName::Cmark }
    fn oracle(&self) -> OracleKind { OracleKind::IndependentTree }
    fn requires_facade(&self) -> bool { false }
    fn independent_layer(&self) -> bool { true }
    fn check(&self, ctx: &CheckCtx<'_>) -> Vec<Finding> {
        match cmark_sig_ok(&ctx.doc.text) {
            Ok(true) => Vec::new(),
            Ok(false) => vec![Finding { signature: "e2/shell_diff", detail: "mdio TreeSig != cmark TreeSig".to_string() }],
            Err(e) => vec![Finding { signature: "e2/run_error", detail: e }],
        }
    }
}

/// External cmark E2: one python + two subprocesses per document. Keep the
/// budget explicit.
fn cmark_sig_ok(text: &str) -> Result<bool, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    const PY: &str = "import sys,pathlib; sys.path.insert(0,'/tmp'); import e2_sig; e2_sig.CF_DUMP=pathlib.Path(sys.argv[1]); print(e2_sig.sig_equal(sys.stdin.read())[0])";
    const BIN: &str = "/tmp/cf_dump_fix/target/release/mdio-cf-dump";
    let mut child = Command::new("python3")
        .args(["-c", PY, BIN])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cmark spawn: {e}"))?;
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(text.as_bytes())
        .map_err(|e| format!("cmark write: {e}"))?;
    let out = child.wait_with_output().map_err(|e| format!("cmark wait: {e}"))?;
    if !out.status.success() {
        return Err(format!("cmark exit: {} {}", out.status, String::from_utf8_lossy(&out.stderr)));
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    match s.as_str() {
        "True" => Ok(true),
        "False" => Ok(false),
        other => Err(format!("cmark unexpected output: {other}")),
    }
}

fn registry() -> Vec<Box<dyn Checker>> {
    vec![
        Box::new(NonWfChecker),
        Box::new(FlEventsChecker),
        Box::new(RootsChecker),
        Box::new(OrdinalChecker),
        Box::new(SpanChecker),
        Box::new(FaceChecker),
        Box::new(CmarkChecker),
        Box::new(EdgesChecker),
        Box::new(DescChecker),
        Box::new(SectionsChecker),
        Box::new(RenderAllChecker),
        Box::new(ProjectionChecker),
    ]
}

// ── report ─────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone, Copy)]
pub struct Cell {
    pub run: usize,
    pub pass: usize,
    pub fail: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone)]
pub struct Exemplar {
    pub checker: CheckerName,
    pub signature: &'static str,
    pub minimal_text: String,
    pub detail: String,
}

#[derive(Debug, Default, Clone)]
pub struct Report {
    pub docs: usize,
    pub cells: HashMap<CheckerName, Cell>,
    pub signature_counts: HashMap<(CheckerName, &'static str), usize>,
    pub exemplars: Vec<Exemplar>,
    /// Full-depth failing document texts (capped); A-bucket prefixes for the
    /// sampler. Not serialized verbatim, only its length.
    pub fail_prefixes: Vec<String>,
    pub full_lines: usize,
    pub sampled_lines: Vec<usize>,
    pub sampled_docs: usize,
    pub estimated_total: usize,
    pub sample: HashMap<&'static str, usize>,
    pub sample_cells: HashMap<CheckerName, Cell>,
    pub sample_signature_counts: HashMap<(CheckerName, &'static str), usize>,
    /// Sampled documents retained for locus diagnostics (not serialized).
    pub sample_docs: Vec<Doc>,
}

const MAX_FAIL_PREFIXES: usize = 2000;

impl Report {
    pub fn to_json(&self) -> String {
        let cells: serde_json::Map<String, serde_json::Value> = self
            .cells
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    serde_json::json!({"run": v.run, "pass": v.pass, "fail": v.fail, "skipped": v.skipped}),
                )
            })
            .collect();
        let sigs: serde_json::Map<String, serde_json::Value> = self
            .signature_counts
            .iter()
            .map(|((k, s), v)| (format!("{}:{}", k.as_str(), s), serde_json::json!(v)))
            .collect();
        let exemplars: Vec<serde_json::Value> = self
            .exemplars
            .iter()
            .map(|e| serde_json::json!({
                "checker": e.checker.as_str(),
                "signature": e.signature,
                "minimal_text": e.minimal_text,
                "detail": e.detail,
            }))
            .collect();
        let sample_cells: serde_json::Map<String, serde_json::Value> = self
            .sample_cells
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    serde_json::json!({"run": v.run, "pass": v.pass, "fail": v.fail, "skipped": v.skipped}),
                )
            })
            .collect();
        let sample_sigs: serde_json::Map<String, serde_json::Value> = self
            .sample_signature_counts
            .iter()
            .map(|((k, s), v)| (format!("{}:{}", k.as_str(), s), serde_json::json!(v)))
            .collect();
        serde_json::json!({
            "docs": self.docs,
            "cells": cells,
            "signature_counts": sigs,
            "exemplars": exemplars,
            "fail_prefixes": self.fail_prefixes.len(),
            "sample": {
                "full_lines": self.full_lines,
                "sampled_lines": self.sampled_lines,
                "sampled_docs": self.sampled_docs,
                "estimated_total": self.estimated_total,
                "buckets": self.sample,
                "cells": sample_cells,
                "signature_counts": sample_sigs,
            },
        })
        .to_string()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "[calib] docs={}", self.docs)?;
        for name in CheckerName::ALL {
            let c = self.cells.get(&name).copied().unwrap_or_default();
            writeln!(f, "  {:<10} run={} pass={} fail={} skipped={}", name.as_str(), c.run, c.pass, c.fail, c.skipped)?;
        }
        for e in &self.exemplars {
            writeln!(f, "  [{}:{}] {:?} :: {}", e.checker.as_str(), e.signature, e.minimal_text, e.detail)?;
        }
        if self.sampled_docs > 0 {
            writeln!(f, "[sample] full_lines={} sampled_lines={:?} sampled_docs={} estimated_total={} buckets={:?}",
                self.full_lines, self.sampled_lines, self.sampled_docs, self.estimated_total, self.sample)?;
            for name in CheckerName::ALL {
                let c = self.sample_cells.get(&name).copied().unwrap_or_default();
                writeln!(f, "  sample {:<10} run={} pass={} fail={} skipped={}", name.as_str(), c.run, c.pass, c.fail, c.skipped)?;
            }
        }
        Ok(())
    }
}

const MAX_EXEMPLARS_PER_KEY: usize = 3;

pub fn calibrate(max_lines: usize, max_docs: Option<usize>) -> Report {
    calibrate_with_budget(Budget::default(), max_lines, max_docs)
}

pub fn calibrate_with_budget(budget: Budget, max_lines: usize, max_docs: Option<usize>) -> Report {
    let enumeration = enumerate_docs(max_lines, max_docs);
    let checkers = registry();
    let mut report = Report { docs: enumeration.docs.len(), ..Report::default() };

    for doc in &enumeration.docs {
        let snap = project_text(&doc.text);
        let data = facade::build(&snap).ok();
        let ctx = CheckCtx { doc, snap: &snap, data: data.as_ref(), budget: &budget };

        let mut independent_failed = false;
        for ch in &checkers {
            if !ch.independent_layer() { continue; }
            if budget_over(&budget, &report, ch.name()) {
                report.cells.entry(ch.name()).or_default().skipped += 1;
                continue;
            }
            let cell = report.cells.entry(ch.name()).or_default();
            if ch.requires_facade() && data.is_none() {
                cell.skipped += 1;
                continue;
            }
            cell.run += 1;
            let findings = ch.check(&ctx);
            if findings.is_empty() {
                cell.pass += 1;
            } else {
                cell.fail += 1;
                independent_failed = true;
                for f in &findings {
                    *report.signature_counts.entry((ch.name(), f.signature)).or_default() += 1;
                }
                push_exemplars(&mut report, ch.name(), &doc.text, findings);
                if report.fail_prefixes.len() < MAX_FAIL_PREFIXES {
                    report.fail_prefixes.push(doc.text.clone());
                }
            }
        }

        for ch in &checkers {
            if ch.independent_layer() { continue; }
            if budget_over(&budget, &report, ch.name()) {
                report.cells.entry(ch.name()).or_default().skipped += 1;
                continue;
            }
            let cell = report.cells.entry(ch.name()).or_default();
            if ch.requires_facade() && data.is_none() {
                cell.skipped += 1;
                continue;
            }
            if independent_failed {
                cell.skipped += 1;
                continue;
            }
            cell.run += 1;
            let findings = ch.check(&ctx);
            if findings.is_empty() {
                cell.pass += 1;
            } else {
                cell.fail += 1;
                for f in &findings {
                    *report.signature_counts.entry((ch.name(), f.signature)).or_default() += 1;
                }
                push_exemplars(&mut report, ch.name(), &doc.text, findings);
            }
        }
    }

    minimize_exemplars(&mut report);
    cmark_on_exemplars(&mut report);
    report
}

/// P3: the external cmark oracle runs only on minimized witnesses — one
/// invocation per unique (signature, minimal_text), decoupled from document
/// count. Per-doc cmark stays disabled via its default budget of 0.
fn cmark_on_exemplars(report: &mut Report) {
    let mut seen: HashSet<(&'static str, String)> = HashSet::new();
    let cell = report.cells.entry(CheckerName::Cmark).or_default();
    let mut extra: Vec<Exemplar> = Vec::new();
    for e in &report.exemplars {
        if !seen.insert((e.signature, e.minimal_text.clone())) {
            continue;
        }
        cell.run += 1;
        match cmark_sig_ok(&e.minimal_text) {
            Ok(true) => cell.pass += 1,
            Ok(false) => {
                cell.fail += 1;
                *report
                    .signature_counts
                    .entry((CheckerName::Cmark, "e2/shell_diff"))
                    .or_default() += 1;
            }
            Err(msg) => {
                cell.fail += 1;
                *report
                    .signature_counts
                    .entry((CheckerName::Cmark, "e2/run_error"))
                    .or_default() += 1;
                extra.push(Exemplar {
                    checker: CheckerName::Cmark,
                    signature: "e2/run_error",
                    minimal_text: e.minimal_text.clone(),
                    detail: msg,
                });
            }
        }
    }
    report.exemplars.extend(extra);
}

/// Sampler plan: full sweep at `full_max_lines`, gated sample at
/// `sampled_max_lines` (usually full + 1). Sampling only falsifies — the
/// report must label sample cells as such.
#[derive(Debug, Clone, Copy)]
pub struct SweepPlan {
    pub full_max_lines: usize,
    pub sampled_max_lines: usize,
}

/// Run the checker registry over an arbitrary doc slice; used by the sampler.
fn run_sample_docs(
    docs: &[Doc],
) -> (HashMap<CheckerName, Cell>, HashMap<(CheckerName, &'static str), usize>) {
    let checkers = registry();
    let budget = Budget::default();
    let mut cells: HashMap<CheckerName, Cell> = HashMap::new();
    let mut sigs: HashMap<(CheckerName, &'static str), usize> = HashMap::new();
    for doc in docs {
        let snap = project_text(&doc.text);
        let data = facade::build(&snap).ok();
        let ctx = CheckCtx { doc, snap: &snap, data: data.as_ref(), budget: &budget };
        for ch in &checkers {
            let cell = cells.entry(ch.name()).or_default();
            if budget
                .limit(ch.name())
                .map(|lim| cell.run >= lim)
                .unwrap_or(false)
            {
                cell.skipped += 1;
                continue;
            }
            if ch.requires_facade() && data.is_none() {
                cell.skipped += 1;
                continue;
            }
            cell.run += 1;
            let findings = ch.check(&ctx);
            if findings.is_empty() {
                cell.pass += 1;
            } else {
                cell.fail += 1;
                for f in &findings {
                    *sigs.entry((ch.name(), f.signature)).or_default() += 1;
                }
            }
        }
    }
    (cells, sigs)
}

/// Three-bucket prefix sampler.
///
/// - A: full-depth prefixes with an independent-layer failure signature.
/// - B: full-depth prefixes whose μ final state has pending loose frames.
/// - C: every A/B prefix extended by every line class (breadth backstop).
pub fn calibrate_sampled(plan: SweepPlan) -> Report {
    let mut report = calibrate(plan.full_max_lines, None);
    report.full_lines = plan.full_max_lines;
    let l = line_classes().len();
    let mut estimated_total = 0usize;
    for d in (plan.full_max_lines + 1)..=plan.sampled_max_lines {
        estimated_total += (0..d).fold(1usize, |acc, _| acc * l);
    }
    report.estimated_total = estimated_total;

    // A: failing prefixes (capped so the sample stays interactive).
    let mut gate_keys: HashSet<StateKey> = HashSet::new();
    let mut texts: Vec<String> = Vec::new();
    let mut a_cnt = 0usize;
    for t in &report.fail_prefixes {
        if a_cnt >= 100 {
            break;
        }
        if let Some(d) = mu_doc(t) {
            if let Some(k) = d.states.last() {
                if gate_keys.insert(k.clone()) {
                    a_cnt += 1;
                    texts.push(t.clone());
                }
            }
        }
    }

    // B: pending loose prefixes (capped).
    let full_docs = enumerate_docs(plan.full_max_lines, None);
    let mut b_cnt = 0usize;
    for d in &full_docs.docs {
        if b_cnt >= 100 {
            break;
        }
        if let Some(k) = d.states.last() {
            if !k.pending.is_empty() && gate_keys.insert(k.clone()) {
                b_cnt += 1;
                texts.push(d.text.clone());
            }
        }
    }

    // Chained staged sampling: each extra depth is gated by that depth's key
    // set, so the DFS prunes at every level instead of walking the full
    // prefix space up to max_lines - 1.
    let mut keys_by_depth: Vec<Option<HashSet<StateKey>>> =
        vec![None; plan.sampled_max_lines + 1];
    keys_by_depth[plan.full_max_lines] = Some(gate_keys);
    let mut combined: Vec<Doc> = Vec::new();
    let mut sampled_lines: Vec<usize> = Vec::new();
    for d in (plan.full_max_lines + 1)..=plan.sampled_max_lines {
        let gate = |s: &MuState, depth: usize| match keys_by_depth.get(depth) {
            Some(Some(set)) => set.contains(&s.key()),
            _ => true,
        };
        let sampled = enumerate_where(d, gate);
        let mut next_keys: HashSet<StateKey> = HashSet::new();
        let per_depth_cap = 1000usize;
        let mut pushed = 0usize;
        for doc in sampled.docs {
            if let Some(k) = doc.states.last() {
                next_keys.insert(k.clone());
            }
            combined.push(doc);
            pushed += 1;
            if pushed >= per_depth_cap {
                break;
            }
        }
        keys_by_depth[d] = Some(next_keys.into_iter().take(1000).collect());
        sampled_lines.push(d);
    }

    // C: breadth backstop — extend sampled prefixes by every line class.
    let classes = line_classes();
    let mut synth = 0usize;
    'outer: for t in texts.iter().take(10) {
        for lc in &classes {
            if synth >= 500 {
                break 'outer;
            }
            if let Some(d) = extend_doc(t, lc) {
                combined.push(d);
                synth += 1;
            }
        }
    }

    let (sample_cells, sample_sigs) = run_sample_docs(&combined);
    report.sampled_lines = sampled_lines;
    report.sampled_docs = combined.len();
    report.sample = [("A", a_cnt), ("B", b_cnt), ("C", synth)]
        .into_iter()
        .collect();
    report.sample_cells = sample_cells;
    report.sample_signature_counts = sample_sigs;
    report.sample_docs = combined;
    report
}

fn budget_over(budget: &Budget, report: &Report, name: CheckerName) -> bool {
    budget
        .limit(name)
        .map(|lim| report.cells.get(&name).map(|c| c.run).unwrap_or(0) >= lim)
        .unwrap_or(false)
}

fn push_exemplars(report: &mut Report, checker: CheckerName, text: &str, findings: Vec<Finding>) {
    for f in findings {
        let count = report.exemplars.iter().filter(|e| e.checker == checker && e.signature == f.signature).count();
        if count >= MAX_EXEMPLARS_PER_KEY {
            continue;
        }
        report.exemplars.push(Exemplar {
            checker,
            signature: f.signature,
            minimal_text: text.to_string(),
            detail: f.detail,
        });
    }
}

fn minimize_exemplars(report: &mut Report) {
    for e in report.exemplars.iter_mut() {
        let checker = e.checker;
        let signature = e.signature;
        e.minimal_text = minimize(e.minimal_text.clone(), &|t| {
            let Some(doc) = mu_doc(t) else { return false };
            let snap = project_text(t);
            let data = facade::build(&snap).ok();
            let budget = Budget::default();
            let ctx = CheckCtx { doc: &doc, snap: &snap, data: data.as_ref(), budget: &budget };
            run_checker(checker, &ctx).iter().any(|f| f.signature == signature)
        });
    }
}

fn run_checker(name: CheckerName, ctx: &CheckCtx<'_>) -> Vec<Finding> {
    registry()
        .into_iter()
        .find(|ch| ch.name() == name)
        .map(|ch| ch.check(ctx))
        .unwrap_or_default()
}

fn first_diff(
    a: &[(EventOp, mdio_core::block::BlockFrame)],
    b: &[(EventOp, mdio_core::block::BlockFrame)],
) -> Option<usize> {
    (0..a.len().min(b.len())).find(|&i| a[i] != b[i])
}

fn minimize(mut text: String, pred: &impl Fn(&str) -> bool) -> String {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    loop {
        let mut reduced = false;
        for i in 0..lines.len() {
            let cand: Vec<String> = lines.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, l)| l.clone()).collect();
            let ct = cand.join("\n") + "\n";
            if ct.trim().is_empty() { continue; }
            if pred(&ct) {
                lines = cand;
                reduced = true;
                break;
            }
        }
        if !reduced { break; }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    let _ = &mut text;
    out
}

// ── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth3_full_report() {
        let r = calibrate(3, None);
        println!("{r}");
        assert!(r.docs > 0);
    }

    #[test]
    fn depth3_ordinal_span_face_clean() {
        let r = calibrate(3, None);
        for name in [CheckerName::Ordinal, CheckerName::Span] {
            let c = r.cells.get(&name).copied().unwrap_or_default();
            assert_eq!(c.fail, 0, "{}: {:?}", name.as_str(), r.exemplars);
        }
        // Face now covers mark/content. Mark findings (identity frame vs
        // geometry slice) are recorded, not fixed; tag/content must stay 0.
        let face = r.cells.get(&CheckerName::Face).copied().unwrap_or_default();
        let tag_fails = r
            .signature_counts
            .get(&(CheckerName::Face, "face/tag_range_mismatch"))
            .copied()
            .unwrap_or(0);
        let content_fails = r
            .signature_counts
            .get(&(CheckerName::Face, "face/content_range_mismatch"))
            .copied()
            .unwrap_or(0);
        assert_eq!(tag_fails, 0, "tag face drifted");
        assert_eq!(content_fails, 0, "content face drifted");
        assert!(face.fail >= tag_fails + content_fails, "face fail undercount");
    }

    #[test]
    fn depth3_fl_model_gaps_pinned() {
        // fl compares production frame log against μ. After μ learned to
        // classify plain text as a lazy-continuation candidate, the blank-list
        // gap family is closed; pin 0 so model regressions are caught here.
        let r = calibrate(3, None);
        let c = r.cells.get(&CheckerName::FlEvents).copied().unwrap_or_default();
        assert_eq!(c.fail, 0, "fl gaps drifted: {:?}", r.exemplars);
        for ((n, sig), v) in &r.signature_counts {
            if *n == CheckerName::FlEvents {
                println!("fl {sig}: {v}");
            }
        }
    }

    #[test]
    #[ignore = "depth-5 full sweep; run explicitly"]
    fn depth5_full_report() {
        let r = calibrate(5, None);
        println!("{r}");
        for ((n, sig), v) in &r.signature_counts {
            if *n == CheckerName::FlEvents {
                println!("fl {sig}: {v}");
            }
        }
    }

    #[test]
    fn sampled_depth6_fl_zero() {
        let r = calibrate_sampled(SweepPlan { full_max_lines: 5, sampled_max_lines: 6 });
        let a = r.sample.get("A").copied().unwrap_or(0);
        let b = r.sample.get("B").copied().unwrap_or(0);
        let c = r.sample.get("C").copied().unwrap_or(0);
        if !r.fail_prefixes.is_empty() {
            assert!(a > 0, "A bucket empty despite failing prefixes");
        }
        assert!(b > 0, "B bucket empty");
        assert!(c > 0, "C bucket empty");
        assert!(r.sampled_docs > 0);
        assert_eq!(r.sample_cells.get(&CheckerName::FlEvents).map(|x| x.fail).unwrap_or(0), 0,
            "sampled fl gaps: {:?}", r.sample_signature_counts);
        assert_eq!(r.sample_cells.get(&CheckerName::RenderAll).map(|x| x.fail).unwrap_or(0), 0,
            "sampled render_all gaps: {:?}", r.sample_signature_counts);
        assert_eq!(r.sample_cells.get(&CheckerName::Desc).map(|x| x.fail).unwrap_or(0), 0,
            "sampled desc gaps: {:?}", r.sample_signature_counts);
    }

    #[test]
    #[ignore = "diagnostic dump; run explicitly"]
    fn face_witness_dump() {
        let text = "> x\n> > - x\n";
        let snap = project_text(text);
        let data = facade::build(&snap).unwrap();
        for (i, n) in data.nodes.iter().enumerate() {
            println!("node {i}: {:?} ord={} peel={} geo={:?} mark={:?}",
                n.node_type, n.frame_ordinal, n.peel_width, n.geometry,
                facade::read_face(&snap, n, Face::Mark));
        }
        println!("roots={:?}", data.roots);
        println!("edges={:?}", data.edges);

        let text2 = "- x\n- x\n";
        let snap2 = project_text(text2);
        let data2 = facade::build(&snap2).unwrap();
        println!("=== {} ===", text2.replace('\n', "|"));
        for (i, n) in data2.nodes.iter().enumerate() {
            println!("node {i}: {:?} ord={} lines=({},{})",
                n.node_type, n.frame_ordinal, n.start_line, n.end_line);
        }
        println!("roots={:?}", data2.roots);
        println!("tree_roots={:?}", tree_roots(&snap2));

        let text3 = "> - x\n\n> x\n";
        let snap3 = project_text(text3);
        let data3 = facade::build(&snap3).unwrap();
        println!("=== {} ===", text3.replace('\n', "|"));
        for (i, n) in data3.nodes.iter().enumerate() {
            println!("node {i}: {:?} ord={} lines=({},{})",
                n.node_type, n.frame_ordinal, n.start_line, n.end_line);
        }
        println!("roots={:?}", data3.roots);
        println!("tree_roots={:?}", tree_roots(&snap3));
        for e in &snap3.frame_log.entries {
            println!("  fl line={} pop={} bm={} frame={:?}",
                entry_line(*e), (e >> 63) != 0, entry_is_body_mut(*e), entry_frame(*e));
        }

        let text4 = "> x\n> - x\n";
        let snap4 = project_text(text4);
        let data4 = facade::build(&snap4).unwrap();
        println!("=== {} ===", text4.replace('\n', "|"));
        for (i, n) in data4.nodes.iter().enumerate() {
            println!("node {i}: {:?} ord={} lines=({},{}) sel={:?}",
                n.node_type, n.frame_ordinal, n.start_line, n.end_line,
                snap4.render_md_selected(ProjectionMode::Hs, std::slice::from_ref(n)));
        }
        println!("all={:?}", snap4.render_md_all(ProjectionMode::Hs));

        let text5 = "> > - x\n";
        let snap5 = project_text(text5);
        let nodes5 = mdio_core::selector::all_nodes(&snap5);
        println!("=== {} ===", text5.replace('\n', "|"));
        for (i, n) in nodes5.iter().enumerate() {
            println!("ident {i}: {:?} ord={} lines=({},{})",
                n.node_type, n.frame_ordinal, n.start_line, n.end_line);
        }
        let scan5 = mdio_core::selector::all_direct_edges_checked_with_nodes(&snap5, &nodes5);
        println!("edges={:?}", scan5.edges);
        println!("phantoms={:?}", scan5.phantom_children);
        println!("tree={}", snap5.debug_block_tree());
    }

    #[test]
    #[ignore = "sample render locus dump; run explicitly"]
    fn render_all_sample_loci() {
        let r = calibrate_sampled(SweepPlan { full_max_lines: 5, sampled_max_lines: 6 });
        let mut shown = 0usize;
        for d in &r.sample_docs {
            let snap = project_text(&d.text);
            let rendered = snap.render_md_all(ProjectionMode::Hs);
            if rendered != d.text && !lazy_prefix_only(&rendered, &d.text) {
                let pos = rendered
                    .bytes()
                    .zip(d.text.bytes())
                    .position(|(a, b)| a != b)
                    .unwrap_or_else(|| rendered.len().min(d.text.len()));
                let line = d.text[..pos.min(d.text.len())].bytes().filter(|&b| b == b'\n').count();
                println!("locus byte={pos} line={line}\n  want {:?}\n  got  {:?}", d.text, rendered);
                shown += 1;
                if shown >= 12 {
                    break;
                }
            }
        }
    }

    #[test]
    #[ignore = "chained 6/7/8 sampling sweep; run explicitly"]
    fn sampled_depth68_report() {
        for depth in [6usize, 7, 8] {
            let r = calibrate_sampled(SweepPlan { full_max_lines: 5, sampled_max_lines: depth });
            println!("=== sampled to depth {depth} ===");
            println!("{r}");
        }
    }

    #[test]
    #[ignore = "depth-6 full sweep; run explicitly"]
    fn depth6_full_report() {
        let r = calibrate(6, None);
        println!("{r}");
    }

    #[test]
    #[ignore = "depth-6 capped sample; run explicitly"]
    fn depth6_sample100k_report() {
        let r = calibrate(6, Some(100_000));
        println!("{r}");
    }

    #[test]
    #[ignore = "depth-5 baseline dump; run explicitly"]
    fn depth5_baseline_dump() {
        let r = calibrate(5, None);
        let js = r.to_json();
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/baseline_depth5.json");
        std::fs::write(path, &js).expect("write baseline_depth5.json");
        println!("baseline dumped: {path} ({} bytes)", js.len());
    }

    #[test]
    #[ignore = "depth-4 full sweep; run explicitly"]
    fn depth4_full_report() {
        let r = calibrate(4, None);
        println!("{r}");
        for ((n, sig), v) in &r.signature_counts {
            if *n == CheckerName::FlEvents {
                println!("fl {sig}: {v}");
            }
        }
    }

    #[test]
    fn depth3_signature_counts_consistent() {
        let r = calibrate(3, None);
        // Every checker's fail count must be exactly the sum of its signature
        // counts; roots must have folded into the three known signatures.
        for name in CheckerName::ALL {
            let fail = r.cells.get(&name).map(|c| c.fail).unwrap_or(0);
            let sig_sum: usize = r
                .signature_counts
                .iter()
                .filter(|((n, _), _)| *n == name)
                .map(|(_, v)| v)
                .sum();
            assert_eq!(fail, sig_sum, "{} fail vs signature sum", name.as_str());
        }
        let roots_fail = r.cells.get(&CheckerName::Roots).map(|c| c.fail).unwrap_or(0);
        assert_eq!(roots_fail, 0, "roots must be clean after granularity alignment");
    }

    #[test]
    fn depth3_json_report_nonempty() {
        let r = calibrate(3, None);
        let js = r.to_json();
        assert!(js.contains("\"docs\":1110"), "{js}");
        assert!(js.contains("\"roots\""), "{js}");
        assert!(!js.contains("roots/"), "{js}");
    }

    #[test]
    fn depth3_cmark_witness_driven() {
        // P3: cmark runs once per unique (signature, minimal_text) exemplar,
        // never per document. depth3 has 1110 docs but cmark.run must be the
        // witness count, a small number.
        let r = calibrate(3, None);
        let c = r.cells.get(&CheckerName::Cmark).copied().unwrap_or_default();
        let expected: usize = r
            .exemplars
            .iter()
            .map(|e| (e.signature, e.minimal_text.clone()))
            .collect::<HashSet<_>>()
            .len();
        assert_eq!(c.run, expected, "cmark must run once per unique witness");
        assert!(c.run < r.docs / 10, "cmark run={} should be decoupled from docs={}", c.run, r.docs);
        let sig_sum: usize = r
            .signature_counts
            .iter()
            .filter(|((n, _), _)| *n == CheckerName::Cmark)
            .map(|(_, v)| v)
            .sum();
        assert_eq!(c.fail, sig_sum, "cmark fail vs signature sum");
        println!("cmark cell: {c:?}");
    }

    #[test]
    fn fault_injection_edges_out_of_range() {
        // A facade with an edge pointing past the node table must fail.
        let fake = facade::FacadeData {
            nodes: Vec::new(),
            node_rows: Vec::new(),
            edges: vec![facade::EdgeRow { parent: 0, child: 9, pos: 0 }],
            roots: Vec::new(),
            sections: Vec::new(),
            members: Vec::new(),
            spans: Vec::new(),
            meta: facade::MetaRow { topology: "hs", schema_version: 2 },
        };
        let text = "> x\n".to_string();
        let doc = mu_doc(&text).unwrap();
        let snap = project_text(&text);
        let budget = Budget::default();
        let ctx = CheckCtx { doc: &doc, snap: &snap, data: Some(&fake), budget: &budget };
        let f = EdgesChecker.check(&ctx);
        assert!(!f.is_empty(), "edges checker must flag out-of-range edge");
    }
}
