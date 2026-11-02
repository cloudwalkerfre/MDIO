//! Arrow core: pure Rust facade/state/blob, independent of DuckDB and PyO3.
//!
//! Single-file module: no arrow_core directory.

pub mod facade {
//! Pure arrow facade semantics: IDENT nodes -> DIRECT/roots/desc/sections/PJ/span.
//!
//! This module is intentionally independent of DuckDB. It is the single source
//! for the B-facing tables; lib.rs only adapts these values to DuckDB vectors.
use std::collections::{HashMap, HashSet, VecDeque};

use crate::block::{
    block_proj_spec, header_level, BlockNode, BlockProj, BlockType,
};
use crate::effect::block_decl;
use crate::projection::Snapshot;
use crate::selector::{all_direct_edges_checked_with_nodes, all_nodes, find_direct_children};

pub const SCHEMA_VERSION: u32 = 2;
pub const DESC_LIMIT: usize = 1_000_000;
pub const CONTENT_LEAF_FRAME_ORDINAL: u8 = u8::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacadeError {
    NonWf(String),
    DescTooBig { rows: usize, limit: usize },
}

impl std::fmt::Display for FacadeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FacadeError::NonWf(m) => write!(f, "MDIO_FACADE_NON_WF: {m}"),
            FacadeError::DescTooBig { rows, limit } => {
                write!(f, "ARROW_DESC_TOO_BIG: {rows} rows > limit {limit}")
            }
        }
    }
}

impl std::error::Error for FacadeError {}

pub type Result<T> = std::result::Result<T, FacadeError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Raw,
    Mark,
    Tag,
    Content,
    Container,
}

#[allow(dead_code)]
impl Face {
    pub const ALL: [Face; 5] = [
        Face::Raw,
        Face::Mark,
        Face::Tag,
        Face::Content,
        Face::Container,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Face::Raw => "raw",
            Face::Mark => "mark",
            Face::Tag => "tag",
            Face::Content => "content",
            Face::Container => "container",
        }
    }

    pub fn bit(self) -> u8 {
        match self {
            Face::Raw => 1,
            Face::Mark => 2,
            Face::Tag => 4,
            Face::Content => 8,
            Face::Container => 16,
        }
    }

    pub fn proj(self) -> BlockProj {
        match self {
            Face::Raw => BlockProj::Raw,
            Face::Mark => BlockProj::Mark,
            Face::Tag => BlockProj::Tag,
            Face::Content => BlockProj::Content,
            Face::Container => BlockProj::Container,
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "raw" => Some(Face::Raw),
            "mark" => Some(Face::Mark),
            "tag" => Some(Face::Tag),
            "content" => Some(Face::Content),
            "container" => Some(Face::Container),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeRow {
    pub node: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub byte_start: u32,
    pub byte_end: u32,
    pub frame_ordinal: u8,
    pub role: &'static str,
    pub type_name: &'static str,
    pub level: i32,
    pub section: Option<u32>,
    pub can_read: u8,
    pub can_write: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeRow {
    pub parent: u32,
    pub child: u32,
    pub pos: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootRow {
    pub node: u32,
    pub pos: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescRow {
    pub ancestor: u32,
    pub descendant: u32,
    pub depth: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionRow {
    pub section: u32,
    pub parent_section: Option<u32>,
    pub level: u32,
    pub start: u32,
    pub end: u32,
    pub depth: u32,
    pub in_list: bool,
    pub header_node: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberRow {
    pub node: u32,
    pub section: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanRow {
    pub node: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub byte_start: u32,
    pub byte_end: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaRow {
    pub topology: &'static str,
    pub schema_version: u32,
}

#[derive(Debug, Clone)]
pub struct FacadeBase {
    pub nodes: Vec<BlockNode>,
    pub node_rows: Vec<NodeRow>,
    pub sections: Vec<SectionRow>,
    pub members: Vec<MemberRow>,
    pub spans: Vec<SpanRow>,
}

#[derive(Debug, Clone)]
pub struct FacadeRelations {
    pub edges: Vec<EdgeRow>,
    pub roots: Vec<RootRow>,
}

#[derive(Debug, Clone)]
pub struct FacadeData {
    pub nodes: Vec<BlockNode>,
    pub node_rows: Vec<NodeRow>,
    pub edges: Vec<EdgeRow>,
    pub roots: Vec<RootRow>,
    pub sections: Vec<SectionRow>,
    pub members: Vec<MemberRow>,
    pub spans: Vec<SpanRow>,
    #[allow(dead_code)]
    pub meta: MetaRow,
}

pub fn role_of(n: &BlockNode) -> &'static str {
    if header_level(n.node_type).is_some() {
        "header"
    } else if n.frame_ordinal == CONTENT_LEAF_FRAME_ORDINAL {
        if block_decl(n.node_type).is_list_item {
            "item_leaf"
        } else {
            "content"
        }
    } else {
        "shell"
    }
}

pub fn type_name(t: BlockType) -> &'static str {
    use BlockType::*;
    match t {
        Header1 | HsHead1 | Header2 | HsHead2 | Header3 | Header4 | Header5 | Header6 => "head",
        Bullet | Checklist => "bullet",
        OrderedListItem => "ol",
        CodeFenceOpen | CodeFenceClose | CodeContent => "code",
        IndentedCode => "indtcode",
        Paragraph => "p",
        Blockquote => "bq",
        TableRow | TableSeparator => "tb",
        Comment => "comment",
        HtmlBlock | HtmlElement | HtmlContent => "html",
        ThematicBreak => "hr",
        HsSep => "hssep",
        Blank => "blank",
        RefDef => "refdef",
    }
}

pub fn can_read_mask(t: BlockType) -> u8 {
    let spec = block_proj_spec(t);
    Face::ALL
        .iter()
        .filter(|f| spec.can_read.contains(f.proj()))
        .fold(0u8, |m, f| m | f.bit())
}

pub fn can_write_mask(t: BlockType) -> u8 {
    let spec = block_proj_spec(t);
    Face::ALL
        .iter()
        .filter(|f| spec.can_write.contains(f.proj()))
        .fold(0u8, |m, f| m | f.bit())
}

#[allow(dead_code)]
pub fn read_face(snap: &Snapshot, node: &BlockNode, face: Face) -> Option<String> {
    let spec = block_proj_spec(node.node_type);
    if !spec.can_read.contains(face.proj()) {
        return None;
    }
    Some(snap.read_block_proj(node, face.proj()))
}

pub fn node_key(n: &BlockNode) -> (usize, usize, u8, u8) {
    (n.start_line, n.end_line, n.node_type as u8, n.frame_ordinal)
}

fn node_index_map(nodes: &[BlockNode]) -> Result<HashMap<(usize, usize, u8, u8), u32>> {
    let mut map = HashMap::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        let key = node_key(n);
        if let Some(prev) = map.insert(key, i as u32) {
            return Err(FacadeError::NonWf(format!(
                "duplicate node identity key {key:?}: indices {prev} and {i}"
            )));
        }
    }
    Ok(map)
}

pub fn check_wf(nodes: &[BlockNode], snap: &Snapshot) -> Result<()> {
    if nodes.is_empty() {
        return Ok(());
    }
    let _ = node_index_map(nodes)?;
    let line_len = snap.line_len();
    for (i, n) in nodes.iter().enumerate() {
        if n.start_line > n.end_line {
            return Err(FacadeError::NonWf(format!(
                "node {i} has start_line {} > end_line {}",
                n.start_line, n.end_line
            )));
        }
        if n.end_line > line_len {
            return Err(FacadeError::NonWf(format!(
                "node {i} ends at line {} beyond line_len {line_len}",
                n.end_line
            )));
        }
        if n.byte_range.start > n.byte_range.end {
            return Err(FacadeError::NonWf(format!(
                "node {i} has reversed byte range {:?}",
                n.byte_range
            )));
        }
    }
    Ok(())
}

fn roots_of(snap: &Snapshot, nodes: &[BlockNode], index: &HashMap<(usize, usize, u8, u8), u32>) -> Result<Vec<RootRow>> {
    let root = BlockNode {
        start_line: 0,
        end_line: snap.line_len(),
        byte_range: 0..0,
        node_type: BlockType::Blank,
        loose: false,
        geometry: None,
        peel_width: 0,
        frame_ordinal: 0,
    };
    let children = find_direct_children(snap, &root, None);
    let mut out = Vec::with_capacity(children.len());
    for (pos, child) in children.iter().enumerate() {
        let Some(&node) = index.get(&node_key(child)) else {
            return Err(FacadeError::NonWf(format!(
                "root emits phantom child {:?}",
                node_key(child)
            )));
        };
        out.push(RootRow { node, pos: pos as u32 });
    }
    let _ = nodes;
    Ok(out)
}

fn edges_of(snap: &Snapshot, nodes: &[BlockNode], _index: &HashMap<(usize, usize, u8, u8), u32>) -> Result<Vec<EdgeRow>> {
    let scan = all_direct_edges_checked_with_nodes(snap, nodes);
    if let Some((parent, child)) = scan.phantom_children.first() {
        return Err(FacadeError::NonWf(format!(
            "parent {parent} emits phantom child {:?}",
            node_key(child)
        )));
    }
    Ok(scan.edges.into_iter().map(|e| EdgeRow { parent: e.parent, child: e.child, pos: e.pos }).collect())
}

fn sections_of(
    nodes: &[BlockNode],
    secs_meta: Vec<(u32, u32, u8, u8, u8, bool)>,
) -> Result<Vec<SectionRow>> {
    let mut by_start: HashMap<u32, Vec<(usize, u8)>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        if let Some(lv) = header_level(n.node_type) {
            by_start.entry(n.start_line as u32).or_default().push((i, lv));
        }
    }
    let mut rows = Vec::with_capacity(secs_meta.len());
    for (start, end, level, _t, depth, in_list) in secs_meta {
        let Some(header) = by_start.get(&start).and_then(|v| {
            v.iter()
                .find(|(_, lv)| *lv == level)
                .map(|(i, _)| *i)
                .or_else(|| v.first().map(|(i, _)| *i))
        }) else {
            return Err(FacadeError::NonWf(format!(
                "section at line {start} has no header node"
            )));
        };
        if end <= start {
            return Err(FacadeError::NonWf(format!(
                "section at line {start} has invalid interval [{start},{end})"
            )));
        }
        rows.push(SectionRow {
            section: header as u32,
            parent_section: None,
            level: level as u32,
            start,
            end,
            depth: depth as u32,
            in_list,
            header_node: header as u32,
        });
    }
    rows.sort_by_key(|r| (r.start, r.level));
    let mut stack: Vec<usize> = Vec::new();
    for i in 0..rows.len() {
        let start = rows[i].start;
        while let Some(&top) = stack.last() {
            if rows[top].end <= start || rows[top].level >= rows[i].level {
                stack.pop();
            } else {
                break;
            }
        }
        if let Some(&top) = stack.last() {
            if start >= rows[top].end {
                return Err(FacadeError::NonWf(format!(
                    "non-laminar sections: {} starts at {start} after parent end {}",
                    rows[i].section, rows[top].end
                )));
            }
            rows[i].parent_section = Some(rows[top].section);
        }
        stack.push(i);
    }
    Ok(rows)
}

fn assign_sections(
    nodes: &[BlockNode],
    sections: &[SectionRow],
) -> (Vec<Option<u32>>, Vec<MemberRow>) {
    let mut order: Vec<usize> = (0..nodes.len()).collect();
    order.sort_by_key(|&i| nodes[i].start_line);
    let mut active: Vec<usize> = Vec::new();
    let mut j = 0usize;
    let mut per_node = vec![None; nodes.len()];
    let mut members = Vec::new();
    for i in order {
        let pos = nodes[i].start_line as u32;
        while j < sections.len() && sections[j].start <= pos {
            active.push(j);
            j += 1;
        }
        active.retain(|&k| sections[k].end > pos);
        if let Some(&deep) = active.last() {
            per_node[i] = Some(sections[deep].section);
        }
        for &k in &active {
            members.push(MemberRow {
                node: i as u32,
                section: sections[k].section,
            });
        }
    }
    (per_node, members)
}

pub fn build(snap: &Snapshot) -> Result<FacadeData> {
    build_from_nodes(snap, all_nodes(snap))
}

pub fn build_base(snap: &Snapshot, nodes: Vec<BlockNode>) -> Result<FacadeBase> {
    check_wf(&nodes, snap)?;
    let sections = sections_of(&nodes, snap.hs_sections_meta())?;
    let (section_per_node, members) = assign_sections(&nodes, &sections);
    let mut node_rows = Vec::with_capacity(nodes.len());
    let mut spans = Vec::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        let level = header_level(n.node_type).unwrap_or(0) as i32;
        node_rows.push(NodeRow {
            node: i as u32,
            start_line: n.start_line as u32,
            end_line: n.end_line as u32,
            byte_start: n.byte_range.start as u32,
            byte_end: n.byte_range.end as u32,
            frame_ordinal: n.frame_ordinal,
            role: role_of(n),
            type_name: type_name(n.node_type),
            level,
            section: section_per_node[i],
            can_read: can_read_mask(n.node_type),
            can_write: can_write_mask(n.node_type),
        });
        spans.push(SpanRow {
            node: i as u32,
            start_line: n.start_line as u32,
            end_line: n.end_line as u32,
            byte_start: n.byte_range.start as u32,
            byte_end: n.byte_range.end as u32,
        });
    }
    Ok(FacadeBase { nodes, node_rows, sections, members, spans })
}

pub fn build_relations(snap: &Snapshot, nodes: &[BlockNode]) -> Result<FacadeRelations> {
    let index = node_index_map(nodes)?;
    let edges = edges_of(snap, nodes, &index)?;
    // roots = nodes with no incoming edge (the edge table is already
    // tree-shaped).
    let roots = roots_from_edges(nodes, &edges);
    Ok(FacadeRelations { edges, roots })
}

fn roots_from_edges(nodes: &[BlockNode], edges: &[EdgeRow]) -> Vec<RootRow> {
    let mut has_parent = vec![false; nodes.len()];
    for e in edges {
        has_parent[e.child as usize] = true;
    }
    let mut out = Vec::new();
    for (i, _) in nodes.iter().enumerate() {
        if !has_parent[i] {
            out.push(RootRow { node: i as u32, pos: out.len() as u32 });
        }
    }
    out
}

pub fn build_from_nodes(snap: &Snapshot, nodes: Vec<BlockNode>) -> Result<FacadeData> {
    let base = build_base(snap, nodes.clone())?;
    let rel = build_relations(snap, &nodes)?;
    Ok(FacadeData {
        nodes,
        node_rows: base.node_rows,
        edges: rel.edges,
        roots: rel.roots,
        sections: base.sections,
        members: base.members,
        spans: base.spans,
        meta: MetaRow {
            topology: "hs",
            schema_version: SCHEMA_VERSION,
        },
    })
}

pub fn descendants(edges: &[EdgeRow], node_count: usize) -> Result<Vec<DescRow>> {
    let mut adj: Vec<Vec<u32>> = vec![Vec::new(); node_count];
    for e in edges {
        adj[e.parent as usize].push(e.child);
    }
    let mut out = Vec::new();
    for ancestor in 0..node_count {
        let mut q: VecDeque<(u32, u32)> = VecDeque::new();
        // Diamond edges (shell + leaf both children) reach a node at several
        // depths; descendants must keep one row per (ancestor, descendant)
        // with the minimal depth.
        let mut best: HashMap<u32, u32> = HashMap::new();
        for &child in &adj[ancestor] {
            if best.insert(child, 1).is_none() {
                q.push_back((child, 1));
            }
        }
        while let Some((node, depth)) = q.pop_front() {
            if best.get(&node) != Some(&depth) {
                continue;
            }
            out.push(DescRow {
                ancestor: ancestor as u32,
                descendant: node,
                depth,
            });
            if out.len() > DESC_LIMIT {
                return Err(FacadeError::DescTooBig {
                    rows: out.len(),
                    limit: DESC_LIMIT,
                });
            }
            if depth as usize >= node_count {
                return Err(FacadeError::NonWf(
                    "descendant closure depth exceeded node count (cycle?)".to_string(),
                ));
            }
            for &child in &adj[node as usize] {
                if best.get(&child).map_or(true, |&d| depth + 1 < d) {
                    best.insert(child, depth + 1);
                    q.push_back((child, depth + 1));
                }
            }
        }
    }
    Ok(out)
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::project_text;

    fn build_text(text: &str) -> Result<FacadeData> {
        let snap = project_text(text);
        build(&snap)
    }

    #[test]
    fn same_line_nodes_keep_frame_ordinal() {
        let f = build_text("- - foo\n").expect("same-line doc is WF");
        let same: Vec<_> = f
            .node_rows
            .iter()
            .filter(|n| n.start_line == 0 && n.type_name == "bullet")
            .collect();
        assert!(same.len() >= 2, "expected two same-line bullets, got {same:?}");
        let mut ords: Vec<_> = same.iter().map(|n| n.frame_ordinal).collect();
        ords.sort_unstable();
        ords.dedup();
        assert!(ords.len() >= 2, "same-line nodes must differ by frame_ordinal: {same:?}");
        assert!(!f.roots.is_empty());
        for e in &f.edges {
            assert!((e.parent as usize) < f.nodes.len());
            assert!((e.child as usize) < f.nodes.len());
        }
    }

    #[test]
    fn multiline_node_has_range() {
        let f = build_text("> a\n> b\n").expect("bq doc is WF");
        assert!(
            f.nodes.iter().any(|n| n.end_line > n.start_line),
            "expected a multi-line node: {:?}",
            f.nodes
                .iter()
                .map(|n| (n.start_line, n.end_line, n.node_type))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn sections_are_derived_from_meta() {
        let f = build_text("# A\n\ntext\n\n## B\n\nmore\n").expect("header doc is WF");
        assert_eq!(f.sections.len(), 2, "sections: {:?}", f.sections);
        assert!(f.sections[1].parent_section.is_some(), "nested section parent: {:?}", f.sections);
        assert!(f.members.iter().any(|m| m.section == f.sections[0].section));
    }

    #[test]
    fn descendants_closure_has_depth() {
        let f = build_text("- a\n  - b\n    - c\n").expect("nested list is WF");
        let d = descendants(&f.edges, f.nodes.len()).expect("desc closure");
        assert!(d.iter().any(|r| r.depth >= 2), "expected depth >= 2: {d:?}");
    }

    #[test]
    fn phantom_now_matches_tree() {
        // The BQ nested bullet emits the BLT shell (ord 2) and the item leaf
        // (255); no phantom child remains.
        let f = build_text("> > - item\n").expect("identity now matches tree");
        let ords: Vec<u8> = f.nodes.iter().map(|n| n.frame_ordinal).collect();
        assert!(ords.contains(&2), "missing BLT shell ord 2: {ords:?}");
        assert!(ords.contains(&255), "missing item leaf: {ords:?}");
        assert_eq!(f.roots.len(), 1);
    }

    #[test]
    fn normalize_dedupes_raw_before_strip() {
        // Dedupe is exact-raw first; a and a\\n are different chunks and both survive strip.
        let out = normalize_rows(vec!["a\n".to_string(), "a\n".to_string(), "a".to_string()], true);
        assert_eq!(out, "aa");
    }
}

#[allow(dead_code)]
pub fn normalize_rows(chunks: impl IntoIterator<Item = String>, dedupe: bool) -> String {
    let mut out = String::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw in chunks {
        if dedupe && !seen.insert(raw.clone()) {
            continue;
        }
        let stripped = raw.trim_end_matches(['\n', '\r']);
        if stripped.is_empty() {
            continue;
        }
        out.push_str(stripped);
    }
    out
}

#[cfg(test)]
mod corpus_tests {
    use super::*;
    use crate::projection::project_text;

    fn build_text(text: &str) -> Result<FacadeData> {
        let snap = project_text(text);
        build(&snap)
    }

    #[test]
    fn corpus_is_wf_or_rejects_phantom() {
        let cases: [(&str, &str, bool); 7] = [
            ("same_line", include_str!("../tests/corpus/same_line.md"), true),
            ("multi_line", include_str!("../tests/corpus/multi_line.md"), true),
            ("sections", include_str!("../tests/corpus/sections.md"), true),
            ("nested", include_str!("../tests/corpus/nested.md"), true),
            ("phantom", include_str!("../tests/corpus/phantom.md"), true),
            ("same_line_bq", include_str!("../tests/corpus/same_line_bq.md"), true),
            ("crlf", include_str!("../tests/corpus/crlf.md"), true),
        ];
        for (name, text, ok) in cases {
            let got = build_text(text);
            if ok {
                let f = got.unwrap_or_else(|e| panic!("{name}: unexpected NON_WF: {e}"));
                assert_eq!(f.roots.len() >= 1, true, "{name}: no roots");
                assert!(!f.spans.is_empty(), "{name}: no spans");
            } else {
                assert!(matches!(got, Err(FacadeError::NonWf(_))), "{name}: expected NON_WF, got {got:?}");
            }
        }
    }
}


}

pub mod state {
//! Lazy facade state shared by PyO3 and DuckDB adapters.
//! Pure Rust: no DuckDB, no PyO3.

use crate::arrow::facade::{
    build_base, build_relations, check_wf, descendants, DescRow, EdgeRow, FacadeBase,
    FacadeError, FacadeRelations, SectionRow,
};
use crate::block::{BlockNode, BlockProj};
use crate::infer::FaceSpans;
use crate::projection::Snapshot;
use crate::selector::all_nodes;
use std::sync::{Arc, OnceLock};

#[derive(Clone, Copy, Debug)]
pub enum FaceSrc { Chunk(u32), Fallback, Content }

#[derive(Clone, Copy, Debug)]
pub struct FaceRef { pub src: FaceSrc, pub off: u32, pub len: u32 }

#[derive(Default)]
pub struct FaceColumn { pub refs: Vec<FaceRef>, pub fallback: Vec<u8> }

pub struct ContentCol { pub refs: Vec<FaceRef>, pub bytes: Vec<u8>, pub values: Vec<String> }

pub struct FacadeState {
    pub snap: Arc<Snapshot>,
    pub nodes: Arc<Vec<BlockNode>>,
    chunks: Vec<(usize, usize)>,
    base: OnceLock<Result<Arc<FacadeBase>, String>>,
    relations: OnceLock<Result<Arc<FacadeRelations>, String>>,
    desc: OnceLock<Result<Arc<Vec<DescRow>>, String>>,
    spans: OnceLock<Arc<FaceSpans>>,
    raw: OnceLock<Arc<FaceColumn>>,
    mark: OnceLock<Arc<FaceColumn>>,
    tag: OnceLock<Arc<FaceColumn>>,
    content: OnceLock<Arc<ContentCol>>,
    container: OnceLock<Arc<FaceColumn>>,
}

impl FacadeState {
    pub fn new(snap: Arc<Snapshot>) -> Result<Arc<Self>, FacadeError> {
        let nodes = all_nodes(&snap);
        check_wf(&nodes, &snap)?;
        let chunks = build_chunks(&snap);
        Ok(Arc::new(Self {
            snap,
            nodes: Arc::new(nodes),
            chunks,
            base: OnceLock::new(),
            relations: OnceLock::new(),
            desc: OnceLock::new(),
            spans: OnceLock::new(),
            raw: OnceLock::new(),
            mark: OnceLock::new(),
            tag: OnceLock::new(),
            content: OnceLock::new(),
            container: OnceLock::new(),
        }))
    }

    pub fn base(&self) -> Result<Arc<FacadeBase>, String> {
        self.base
            .get_or_init(|| {
                build_base(&self.snap, self.nodes.as_ref().clone())
                    .map(Arc::new)
                    .map_err(|e| e.to_string())
            })
            .clone()
    }

    pub fn relations(&self) -> Result<Arc<FacadeRelations>, String> {
        self.relations
            .get_or_init(|| {
                build_relations(&self.snap, &self.nodes)
                    .map(Arc::new)
                    .map_err(|e| e.to_string())
            })
            .clone()
    }

    pub fn descendants(&self) -> Result<Arc<Vec<DescRow>>, String> {
        let rel = self.relations()?;
        self.desc
            .get_or_init(|| {
                descendants(&rel.edges, self.nodes.len())
                    .map(Arc::new)
                    .map_err(|e| e.to_string())
            })
            .clone()
    }

    pub fn spans(&self) -> &FaceSpans {
        self.spans
            .get_or_init(|| {
                Arc::new(crate::infer::read_block_proj_batch_spans(
                    &self.snap, &self.nodes, None,
                ))
            })
            .as_ref()
    }

    pub fn raw(&self) -> &FaceColumn {
        self.raw
            .get_or_init(|| Arc::new(build_raw_column(&self.snap, &self.nodes, &self.chunks)))
            .as_ref()
    }

    pub fn mark(&self) -> &FaceColumn {
        self.mark
            .get_or_init(|| {
                Arc::new(build_span_column(
                    &self.snap, &self.nodes, &self.chunks, &self.spans().mark, BlockProj::Mark,
                ))
            })
            .as_ref()
    }

    pub fn tag(&self) -> &FaceColumn {
        self.tag
            .get_or_init(|| {
                Arc::new(build_span_column(
                    &self.snap, &self.nodes, &self.chunks, &self.spans().tag, BlockProj::Tag,
                ))
            })
            .as_ref()
    }

    pub fn content(&self) -> &ContentCol {
        self.content
            .get_or_init(|| Arc::new(build_content_col(&self.snap, &self.nodes)))
            .as_ref()
    }

    pub fn container(&self) -> &FaceColumn {
        self.container
            .get_or_init(|| {
                let content = self.content();
                let spans = crate::infer::read_block_proj_batch_spans(
                    &self.snap, &self.nodes, Some(&content.values),
                );
                Arc::new(build_span_column(
                    &self.snap, &self.nodes, &self.chunks, &spans.container,
                    BlockProj::Container,
                ))
            })
            .as_ref()
    }


    /// Face values, built through the corresponding lazy OnceLock.
    pub fn face_values(&self, face: crate::arrow::facade::Face) -> Vec<String> {
        use crate::arrow::facade::Face;
        let (refs, fallback, content): (&[FaceRef], &[u8], Option<&[u8]>) = match face {
            Face::Raw => { let c = self.raw(); (&c.refs, &c.fallback, None) }
            Face::Mark => { let c = self.mark(); (&c.refs, &c.fallback, None) }
            Face::Tag => { let c = self.tag(); (&c.refs, &c.fallback, None) }
            Face::Content => { let c = self.content(); (&c.refs, &[], Some(&c.bytes)) }
            Face::Container => { let c = self.container(); (&c.refs, &c.fallback, None) }
        };
        let chunk_ptrs: Vec<*const u8> = self.snap.rope.chunks().map(|c| c.as_ptr()).collect();
        refs.iter().map(|r| {
            let bytes: &[u8] = match r.src {
                // SAFETY: chunk_ptrs was collected from this Snapshot's rope and the
                // ref offsets/lengths come from the same rope's block spans.
                FaceSrc::Chunk(ci) => unsafe {
                    std::slice::from_raw_parts(chunk_ptrs[ci as usize].add(r.off as usize), r.len as usize)
                },
                FaceSrc::Fallback => &fallback[r.off as usize..(r.off + r.len) as usize],
                FaceSrc::Content => &content.expect("content bytes")[r.off as usize..(r.off + r.len) as usize],
            };
            std::str::from_utf8(bytes).expect("face utf8").to_string()
        }).collect()
    }

    pub fn render_all(&self) -> String {
        self.snap.render_md_all(crate::topology::ProjectionMode::Hs)
    }

    pub fn render_selected(&self, ids: &[i64]) -> Result<String, String> {
        if ids == [-1] {
            return Ok(self.render_all());
        }
        let mut nodes = Vec::with_capacity(ids.len());
        for id in ids {
            if *id < 0 {
                return Err(format!("ARROW_INVALID_RESULT: node id {id} is negative (only synthetic root -1 is allowed)"));
            }
            let node = self.nodes.get(*id as usize)
                .ok_or_else(|| format!("ARROW_INVALID_RESULT: node id {id} out of range"))?;
            nodes.push(node.clone());
        }
        self.snap.render_md_selected(crate::topology::ProjectionMode::Hs, &nodes)
            .map_err(|e| format!("ARROW_MD_RENDER: {e}"))
    }
}


#[cfg(test)]
mod lazy_tests {
    use super::*;
    use crate::projection::State;

    #[test]
    fn facade_stays_lazy_until_each_slot_is_requested() {
        let mut st = State::new("# T\n\npara\n\n- a\n- b\n");
        let snap = std::sync::Arc::new(st.snapshot());
        let f = FacadeState::new(snap).unwrap();
        assert!(f.base.get().is_none());
        assert!(f.relations.get().is_none());
        assert!(f.raw.get().is_none());
        let _ = f.raw();
        assert!(f.raw.get().is_some());
        assert!(f.base.get().is_none());
        assert!(f.relations.get().is_none());
        let _ = f.relations();
        assert!(f.relations.get().is_some());
        let _ = f.base();
        assert!(f.base.get().is_some());
    }
}

fn build_chunks(snap: &Snapshot) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut acc = 0usize;
    for ch in snap.rope.chunks() {
        let n = ch.len();
        out.push((acc, acc + n));
        acc += n;
    }
    out
}

fn push_face_value(
    col: &mut FaceColumn, chunks: &[(usize, usize)], snap: &Snapshot, off: usize, len: usize,
) {
    let ci = chunks.partition_point(|&(_, e)| e <= off);
    if ci < chunks.len() && off >= chunks[ci].0 && off + len <= chunks[ci].1 {
        col.refs.push(FaceRef {
            src: FaceSrc::Chunk(ci as u32),
            off: (off - chunks[ci].0) as u32,
            len: len as u32,
        });
    } else {
        let fb = col.fallback.len();
        let bytes = snap.rope.byte_slice(off..off + len).to_string();
        col.fallback.extend_from_slice(bytes.as_bytes());
        col.refs.push(FaceRef { src: FaceSrc::Fallback, off: fb as u32, len: len as u32 });
    }
}

fn build_raw_column(
    snap: &Snapshot, nodes: &[BlockNode], chunks: &[(usize, usize)],
) -> FaceColumn {
    let mut col = FaceColumn { refs: Vec::with_capacity(nodes.len()), fallback: Vec::new() };
    for nd in nodes {
        push_face_value(
            &mut col, chunks, snap, nd.byte_range.start,
            nd.byte_range.end.saturating_sub(nd.byte_range.start),
        );
    }
    col
}

fn build_span_column(
    snap: &Snapshot, nodes: &[BlockNode], chunks: &[(usize, usize)],
    spans: &[Option<(u32, u32)>], proj: BlockProj,
) -> FaceColumn {
    let mut col = FaceColumn { refs: Vec::with_capacity(nodes.len()), fallback: Vec::new() };
    for (i, nd) in nodes.iter().enumerate() {
        match spans[i] {
            Some((off, len)) => push_face_value(&mut col, chunks, snap, off as usize, len as usize),
            None => {
                let v = snap.read_block_proj(nd, proj);
                let fb = col.fallback.len();
                col.fallback.extend_from_slice(v.as_bytes());
                col.refs.push(FaceRef {
                    src: FaceSrc::Fallback, off: fb as u32, len: v.len() as u32,
                });
            }
        }
    }
    col
}

fn build_content_col(snap: &Snapshot, nodes: &[BlockNode]) -> ContentCol {
    let values = crate::infer::read_block_proj_batch(snap, nodes, BlockProj::Content);
    let mut bytes = Vec::new();
    let mut refs = Vec::with_capacity(values.len());
    for v in &values {
        let off = bytes.len();
        bytes.extend_from_slice(v.as_bytes());
        refs.push(FaceRef { src: FaceSrc::Content, off: off as u32, len: v.len() as u32 });
    }
    ContentCol { refs, bytes, values }
}
}

pub mod blob {
//! Handoff snapshot blob: internal A/B transport for the five Snapshot columns.
use crate::block::{FrameLog, PackedLineInfo};
use crate::projection::{Snapshot, SnapshotConsumption};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, OnceLock, Weak};

const MAGIC: &[u8; 8] = b"MDIOHND1";
const VERSION: u32 = 1;

pub fn encode(s: &Snapshot) -> Vec<u8> {
    let text = s.rope.to_string();
    let lines: Vec<u8> = s.lines.iter().map(|x| x.0).collect();
    let prev: Vec<u8> = s.prev_col.iter().map(|x| x.0).collect();
    let depth: Vec<u8> = s.open_depth.iter().copied().collect();
    let entries = &s.frame_log.entries;
    let mut out = Vec::with_capacity(32 + text.len() + lines.len() + prev.len() + depth.len() + entries.len() * 8);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    put_bytes(&mut out, text.as_bytes());
    put_bytes(&mut out, &lines);
    put_bytes(&mut out, &prev);
    put_bytes(&mut out, &depth);
    out.extend_from_slice(&(entries.len() as u64).to_le_bytes());
    for e in entries { out.extend_from_slice(&e.to_le_bytes()); }
    out
}

fn put_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
}

fn take_u32(b: &[u8], off: &mut usize) -> Result<u32, String> {
    let end = off.checked_add(4).ok_or("blob overflow")?;
    let v = b.get(*off..end).ok_or("blob truncated")?;
    *off = end;
    Ok(u32::from_le_bytes(v.try_into().map_err(|_| "blob u32")?))
}

fn take_u64(b: &[u8], off: &mut usize) -> Result<u64, String> {
    let end = off.checked_add(8).ok_or("blob overflow")?;
    let v = b.get(*off..end).ok_or("blob truncated")?;
    *off = end;
    Ok(u64::from_le_bytes(v.try_into().map_err(|_| "blob u64")?))
}

fn take_bytes(b: &[u8], off: &mut usize) -> Result<Vec<u8>, String> {
    let n = take_u64(b, off)? as usize;
    let end = off.checked_add(n).ok_or("blob overflow")?;
    let v = b.get(*off..end).ok_or("blob truncated")?.to_vec();
    *off = end;
    Ok(v)
}

pub fn decode(b: &[u8]) -> Result<Snapshot, String> {
    if b.get(0..8) != Some(MAGIC) { return Err("ARROW_SNAPSHOT_BLOB: bad magic".into()); }
    let mut off = 8usize;
    let version = take_u32(b, &mut off)?;
    if version != VERSION { return Err(format!("ARROW_SNAPSHOT_BLOB: unsupported version {version}")); }
    let text = String::from_utf8(take_bytes(b, &mut off)?).map_err(|e| format!("ARROW_SNAPSHOT_BLOB: text {e}"))?;
    let lines = take_bytes(b, &mut off)?.into_iter().map(PackedLineInfo).collect::<Vec<_>>();
    let prev_col = take_bytes(b, &mut off)?.into_iter().map(PackedLineInfo).collect::<Vec<_>>();
    let open_depth = take_bytes(b, &mut off)?;
    let n = take_u64(b, &mut off)? as usize;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n { entries.push(take_u64(b, &mut off)?); }
    if off != b.len() { return Err("ARROW_SNAPSHOT_BLOB: trailing bytes".into()); }
    Ok(Snapshot {
        rope: crop::Rope::from(&*text),
        lines: Arc::new(lines),
        prev_col: Arc::new(prev_col),
        open_depth: Arc::new(open_depth),
        frame_log: Arc::new(FrameLog { entries }),
        block_cache: Some(Arc::new(OnceLock::new())),
        cache_slot: Weak::new(),
        prev_snapshot: Weak::new(),
        cell_consumption: Arc::new(SnapshotConsumption {
            tree_full: AtomicU64::new(0),
            secs: AtomicU64::new(0),
            cell_query: Default::default(),
            cell_hit: Default::default(),
            cell_built: Default::default(),
        }),
        cells: Some(Arc::new(OnceLock::new())),
        segment_memo: OnceLock::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::State;

    #[test]
    fn blob_roundtrip() {
        let mut st = State::new("# T\n\npara\n\n- a\n- b\n");
        let s = st.snapshot();
        let b = encode(&s);
        let d = decode(&b).unwrap();
        assert_eq!(s.rope.to_string(), d.rope.to_string());
        assert_eq!(&*s.lines, &*d.lines);
        assert_eq!(&*s.prev_col, &*d.prev_col);
        assert_eq!(&*s.open_depth, &*d.open_depth);
        assert_eq!(s.frame_log.entries, d.frame_log.entries);
    }
}
}
