//! exhaustive_v2 — μ state-graph explorer (BQ/BLT/OL/leaf).
//!
//! This is a test/feature-only module. It does not touch `exhaustive_gen`.
//! The v2 core is derived from μ transitions:
//!
//!   state --LineClass--> state'
//!     with action = T(state, line)
//!          events = flatten(action)
//!          text   = realize_line(state, line)
//!
//! A document is a path in this graph.

use crate::block::{BlockFrame, BlockType, FrameKind};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Marker {
    Bullet,
    Star,
    Plus,
    Ordered,
    OrderedParen,
}

fn marker_kind_body(m: Marker) -> (FrameKind, u8) {
    match m {
        Marker::Bullet => (FrameKind::BulletList, 0),
        Marker::Star => (FrameKind::BulletList, 16),
        Marker::Plus => (FrameKind::BulletList, 32),
        Marker::Ordered => (FrameKind::OrderedList, 0),
        Marker::OrderedParen => (FrameKind::OrderedList, 32),
    }
}

fn marker_base(m: Marker) -> u8 {
    match m {
        Marker::Bullet | Marker::Star | Marker::Plus => 2,
        Marker::Ordered | Marker::OrderedParen => 3,
    }
}

fn marker_text(m: Marker) -> &'static str {
    match m {
        Marker::Bullet => "- ",
        Marker::Star => "* ",
        Marker::Plus => "+ ",
        Marker::Ordered => "1. ",
        Marker::OrderedParen => "1) ",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineClass {
    pub blank: bool,
    pub bq: u8,
    pub marker: Option<Marker>,
    pub indent: u8,
    pub content_empty: bool,
    pub lazy: bool,
}

#[derive(Clone, Debug)]
pub struct MuState {
    pub stack: Vec<BlockFrame>,
    pub pending: Vec<(usize, FrameKind)>,
    pub prev: Option<BlockType>,
}

impl MuState {
    pub fn start() -> Self {
        MuState { stack: Vec::new(), pending: Vec::new(), prev: None }
    }

    pub fn key(&self) -> StateKey {
        StateKey {
            stack: self.stack.iter().map(|f| f.0).collect(),
            pending: self.pending.iter().map(|(d, k)| (*d, *k as u8)).collect(),
            prev: self.prev.map(|t| t as u8),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StateKey {
    pub stack: Vec<u16>,
    pub pending: Vec<(usize, u8)>,
    pub prev: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Continue,
    ContinueBlank,
    Pop,
    Push(Vec<BlockFrame>),
    PopPush(BlockFrame),
    BodyMut(BlockFrame),
    Composite(Vec<Action>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventOp {
    P,
    Q,
    B,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub line: usize,
    pub op: EventOp,
    pub frame: BlockFrame,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub from: StateKey,
    pub to: StateKey,
    pub line: LineClass,
    pub line_type: BlockType,
    pub text: String,
    pub action: Action,
    pub events: Vec<Event>,
}

/// Declared finite scope. Coverage is meaningful only relative to this.
#[derive(Clone, Debug)]
pub struct Scope {
    pub kinds: Vec<FrameKind>,
    pub max_kind_depth: usize,
    pub line_classes: usize,
}

impl Scope {
    pub fn first_slice() -> Self {
        Scope {
            kinds: vec![FrameKind::BlockQuote, FrameKind::BulletList, FrameKind::OrderedList],
            max_kind_depth: 3,
            line_classes: line_classes().len(),
        }
    }

    pub fn declared_kind_sequences(&self) -> usize {
        let mut total = 0usize;
        for d in 0..=self.max_kind_depth {
            total += self.kinds.len().pow(d as u32);
        }
        total
    }
}

/// One enumerated document: a concrete line path, not one BFS representative.
#[derive(Clone, Debug)]
pub struct Doc {
    pub text: String,
    pub events: Vec<Event>,
    pub line_types: Vec<BlockType>,
    pub states: Vec<StateKey>,
}

/// Document enumeration result. `capped` means the doc cap was hit; otherwise
/// every path up to `max_lines` was emitted.
#[derive(Clone, Debug)]
pub struct Enumeration {
    pub docs: Vec<Doc>,
    pub max_lines: usize,
    pub capped: bool,
}

pub struct Graph {
    pub start: StateKey,
    pub edges: Vec<Edge>,
    pub states: Vec<StateKey>,
    pub capped: bool,
    parent: HashMap<StateKey, (StateKey, usize)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub states: usize,
    pub edges: usize,
    pub p_events: usize,
    pub q_events: usize,
    pub b_events: usize,
    pub line_classes: usize,
    pub action_kinds: usize,
    pub action_bigrams: usize,
    pub line_bigrams: usize,
    pub target_pairs: usize,
    pub realized_pairs: usize,
    pub line_action_pairs: usize,
    pub declared_kind_sequences: usize,
    pub covered_kind_sequences: usize,
    pub declared_transition_pairs: usize,
    pub covered_transition_pairs: usize,
    pub unreachable_kind_sequences: usize,
    pub unreachable_transition_pairs: usize,
    pub capped: bool,
}

fn kind_seq(key: &StateKey) -> Vec<u8> {
    key.stack.iter().map(|raw| ((raw >> 12) & 0xF) as u8).collect()
}

fn line_type_for(state: &MuState, line: &LineClass) -> BlockType {
    if line.blank {
        return BlockType::Blank;
    }
    // Opening a new BQ makes the line the BQ opening line; a line already
    // inside the BQ suffix is classified by its own marker.
    if bq_depth(&state.stack) < line.bq as usize {
        return BlockType::Blockquote;
    }
    match line.marker {
        Some(Marker::Bullet | Marker::Star | Marker::Plus) => BlockType::Bullet,
        Some(Marker::Ordered | Marker::OrderedParen) => BlockType::OrderedListItem,
        None => BlockType::Paragraph,
    }
}

fn outer_list(state: &MuState) -> Option<BlockFrame> {
    state.stack.iter().copied().find(|f| list_kind(f.kind()))
}

fn action_frame(a: &Action) -> Option<BlockFrame> {
    match a {
        Action::Push(fs) => fs.last().copied(),
        Action::PopPush(f) => Some(*f),
        _ => None,
    }
}

fn action_key(a: &Action) -> u8 {
    match a {
        Action::Continue => 0,
        Action::ContinueBlank => 1,
        Action::Pop => 2,
        Action::Push(_) => 3,
        Action::PopPush(_) => 4,
        Action::BodyMut(_) => 5,
        Action::Composite(_) => 6,
    }
}

fn frame(kind: FrameKind, indent: u8, body: u8) -> BlockFrame {
    BlockFrame::new(kind, indent, body)
}

fn list_content_indent(line: &LineClass, m: Marker) -> u8 {
    let base = marker_base(m);
    // Inside a BQ, the list frame column is measured after the BQ prefix;
    // `>   - x` still opens a BulletList with indent=2.
    if line.bq > 0 {
        base
    } else {
        line.indent.saturating_add(base)
    }
}

fn open_frames(line: &LineClass) -> Vec<BlockFrame> {
    let mut frames = Vec::new();
    if let Some(m) = line.marker {
        let (kind, body) = marker_kind_body(m);
        frames.push(frame(kind, list_content_indent(line, m), body));
    }
    frames
}

fn list_kind(kind: FrameKind) -> bool {
    matches!(kind, FrameKind::BulletList | FrameKind::OrderedList)
}

fn list_marker_indent(f: BlockFrame) -> u8 {
    match f.kind() {
        FrameKind::BulletList => f.indent().saturating_sub(2),
        FrameKind::OrderedList => f.indent().saturating_sub(3),
        _ => f.indent(),
    }
}

fn bq_depth(stack: &[BlockFrame]) -> usize {
    stack.iter().filter(|f| f.kind() == FrameKind::BlockQuote).count()
}

fn top_kind(s: &MuState) -> Option<FrameKind> {
    s.stack.last().map(|f| f.kind())
}

fn with_b_pre(state: &MuState, action: Action) -> Action {
    // The loose bit propagates bottom-up: pick the deepest pending list frame
    // that is not yet loose. For a nested Push any kind counts; for a sibling
    // PopPush the new list kind must match (different kind replaces the list
    // and emits no B).
    let deepest_non_loose = |want_kind: Option<FrameKind>| {
        state
            .pending
            .iter()
            .rev()
            .find(|(d, _)| {
                state
                    .stack
                    .get(*d)
                    .map(|f| {
                        !f.loose() && want_kind.map_or(true, |k| f.kind() == k)
                    })
                    .unwrap_or(false)
            })
            .copied()
    };
    match action {
        Action::Push(_) => {
            let Some((depth, _)) = deepest_non_loose(None) else { return action };
            let f = state.stack[depth];
            let target = BlockFrame::new(f.kind(), f.indent(), f.body() | 8);
            Action::Composite(vec![Action::BodyMut(target), action])
        }
        Action::PopPush(new) => {
            let Some((depth, _)) = deepest_non_loose(Some(new.kind())) else { return action };
            let f = state.stack[depth];
            let target = BlockFrame::new(f.kind(), f.indent(), f.body() | 8);
            Action::Composite(vec![action, Action::BodyMut(target)])
        }
        _ => action,
    }
}

/// Pure model: one line transition. First slice only.
pub fn t(state: &MuState, line: &LineClass) -> Action {
    if line.blank {
        return match top_kind(state) {
            // A blank line closes the whole consecutive BQ suffix.
            Some(FrameKind::BlockQuote) => {
                Action::Composite(vec![Action::Pop; bq_depth(&state.stack)])
            }
            Some(k) if list_kind(k) && bq_depth(&state.stack) > 0 => {
                // Blank does not close here; production closes on the next
                // BQ-prefixed line (prev == Blank drives that transition).
                Action::ContinueBlank
            }
            Some(k) if list_kind(k) => Action::ContinueBlank,
            _ => Action::Continue,
        };
    }

    // A >=4-space marker outside any container is indented code, not a list
    // item. Inside an open list the same indent is a nested item.
    if state.stack.is_empty() && line.bq == 0 && line.marker.is_some() && line.indent >= 4 {
        return Action::Continue;
    }

    let bq_need = bq_depth(&state.stack);
    // A BQ-prefixed line after a loose blank (prev == Blank, list on top)
    // closes the pending list and the BQ suffix, then reopens from the line.
    if state.prev == Some(BlockType::Blank)
        && state.stack.last().map(|f| list_kind(f.kind())).unwrap_or(false)
        && line.bq > 0
    {
        let mut acts = vec![Action::Pop; state.stack.len()];
        acts.push(t(&MuState::start(), line));
        return Action::Composite(acts);
    }
    // A list on top hides lower BQ frames. If this line has fewer BQ markers
    // than the open BQ suffix, close the list and excess BQ layers first.
    if (line.bq as usize) < bq_need
        && line.marker.is_some()
        && state.stack.last().map(|f| list_kind(f.kind())).unwrap_or(false)
    {
        let keep = line.bq as usize;
        let mut base = state.clone();
        let mut acts = Vec::new();
        while bq_depth(&base.stack) > keep {
            base.stack.pop();
            acts.push(Action::Pop);
        }
        acts.push(t(&base, line));
        return Action::Composite(acts);
    }
    // A structural marker with fewer BQ prefixes than open BQ layers belongs
    // to the parent BQ: close the innermost BQ first.
    if (line.bq as usize) < bq_need
        && line.marker.is_some()
        && top_kind(state) == Some(FrameKind::BlockQuote)
    {
        let mut base = state.clone();
        base.stack.pop();
        return Action::Composite(vec![Action::Pop, t(&base, line)]);
    }
    // Line enters more BQ layers than the current suffix: close any non-BQ
    // frames, then open the BQ delta + line frames.
    if (line.bq as usize) > bq_need {
        let keep = bq_need;
        let mut acts = Vec::new();
        for _ in keep..state.stack.len() {
            acts.push(Action::Pop);
        }
        let mut frames = Vec::new();
        for _ in keep..line.bq as usize {
            frames.push(frame(FrameKind::BlockQuote, 0, 2));
        }
        frames.extend(open_frames(line));
        if frames.is_empty() {
            acts.push(Action::Continue);
        } else {
            acts.push(Action::Push(frames));
        }
        return Action::Composite(acts);
    }

    // A structural line at/before the BQ content column closes the BQ suffix.
    // A plain line or a marker deeper than that column is BQ lazy content.
    let top_is_list = state.stack.last().map(|f| list_kind(f.kind())).unwrap_or(false);
    if bq_need > 0
        && line.bq == 0
        && line.marker.is_some()
        && (!line.content_empty || top_is_list)
        && line.indent <= (2 * bq_need) as u8
    {
        let mut acts = vec![Action::Pop; state.stack.len()];
        // After closing the BQ suffix, a >=4-space marker at top level is
        // indented code, not a new list.
        let frames = if line.indent >= 4 { Vec::new() } else { open_frames(line) };
        if frames.is_empty() {
            acts.push(Action::Continue);
        } else {
            acts.push(Action::Push(frames));
        }
        return Action::Composite(acts);
    }

    if state.stack.is_empty() {
        let mut frames = Vec::new();
        for _ in 0..line.bq {
            frames.push(frame(FrameKind::BlockQuote, 0, 2));
        }
        frames.extend(open_frames(line));
        return if frames.is_empty() { Action::Continue } else { Action::Push(frames) };
    }

    let top = *state.stack.last().expect("nonempty");
    match top.kind() {
        FrameKind::BlockQuote => {
            if line.bq > 0 {
                // BQ delta was handled above; here the line is still inside
                // the open BQ suffix. A marker opens a list, otherwise the
                // line is BQ content (Continue).
                if let Some(m) = line.marker {
                    let ind = list_content_indent(line, m);
                    let (k, body) = marker_kind_body(m);
                    Action::Push(vec![frame(k, ind, body)])
                } else if line.content_empty && (line.bq as usize) < bq_need {
                    // `> ` under two BQs closes one BQ layer.
                    Action::Pop
                } else {
                    Action::Continue
                }
            } else {
                // BQ lazy continuation: a plain line or a marker past the
                // BQ content column does not pop the BQ.
                Action::Continue
            }
        }
        FrameKind::BulletList | FrameKind::OrderedList => {
            // A >=4-space marker beyond the list content column is indented
            // code at top level; close the list, do not open a new list.
            if line.bq == 0
                && line.marker.is_some()
                && line.indent >= 4
                && line.indent < top.indent()
            {
                let mut base = state.clone();
                base.stack.pop();
                return Action::Composite(vec![Action::Pop, t(&base, line)]);
            }
            // A marker shallower than this list's marker column closes this
            // list item; recurse on the parent state so two list levels can
            // close in one line (Q, Q, then Q/P or P).
            if line.marker.is_some() && line.indent < list_marker_indent(top) {
                let mut base = state.clone();
                base.stack.pop();
                let mut rest = t(&base, line);
                // Closing a loose list hands its loose bit to the replacement
                // list of the same kind, but only when no further list frame
                // is being closed on the way (multi-level dedent does not
                // propagate the bit).
                if top.loose() && base.stack.last().map(|f| !list_kind(f.kind())).unwrap_or(true) {
                    match &mut rest {
                        Action::Push(fs) => {
                            if let Some(f) = fs.last_mut() {
                                if f.kind() == top.kind() {
                                    *f = BlockFrame::new(f.kind(), f.indent(), f.body() | 8);
                                }
                            }
                        }
                        Action::PopPush(f) => {
                            if f.kind() == top.kind() {
                                *f = BlockFrame::new(f.kind(), f.indent(), f.body() | 8);
                            }
                        }
                        _ => {}
                    }
                }
                let mut acts = vec![Action::Pop];
                let top_depth = state.stack.len().saturating_sub(1);
                if let Some(&(depth, _)) = state.pending.iter().find(|(d, _)| *d == top_depth) {
                    if let (Some(old), Some(new)) =
                        (state.stack.get(depth).copied(), action_frame(&rest))
                    {
                        if !old.loose()
                            && old.kind() == new.kind()
                            && old.body() == new.body()
                        {
                            acts.push(rest);
                            acts.push(Action::BodyMut(BlockFrame::new(
                                new.kind(),
                                new.indent(),
                                new.body() | 8,
                            )));
                            return Action::Composite(acts);
                        }
                    }
                }
                acts.push(rest);
                return Action::Composite(acts);
            }
            let action = if let Some(m) = line.marker {
                let (kind, body) = marker_kind_body(m);
                // Inside an already-open list, a nested marker's content
                // column is measured from the marker's own line indent.
                let base = marker_base(m);
                let top_marker = list_marker_indent(top);
                // Sibling markers keep their nesting level.  A marker that
                // falls between the current marker column and content column
                // inside BQ closes/reopens at the BQ-relative content column;
                // at top level it stays absolute.
                let content_indent = if line.indent == top_marker {
                    line.indent.saturating_add(base)
                } else if line.indent < top.indent() && line.bq > 0 {
                    base
                } else {
                    line.indent.saturating_add(base)
                };
                let f = frame(kind, content_indent, if line.indent < top.indent() && top.loose() && kind == top.kind() { body | 8 } else { body });
                // Compare marker columns, not content columns: a bullet marker
                // has content offset 2, an ordered marker offset 3.
                if line.indent >= top.indent() {
                    // marker column at/inside the item content column => nested
                    Action::Push(vec![f])
                } else {
                    // sibling marker, or a shallower marker that closes this
                    // list and opens a new one on the same line
                    Action::PopPush(f)
                }
            } else if line.lazy && !state.pending.is_empty() && line.indent < top.indent() {
                // Lazy text outside the BQ/list after a loose blank closes
                // the pending list and any enclosing BQ suffix. Frame bodies
                // were already flipped by BodyMut, so plain Pop is exact.
                Action::Composite(vec![Action::Pop; state.stack.len()])
            } else if line.lazy || !line.content_empty || line.bq > 0 {
                // BQ-only line (`> `) is container content, it does not
                // close an open list.
                Action::Continue
            } else {
                Action::Pop
            };
            with_b_pre(state, action)
        }
        _ => Action::Continue,
    }
}

/// Apply one action. Events are the FL projection.
pub fn apply(state: &MuState, action: &Action, line_no: usize) -> (MuState, Vec<Event>) {
    let mut s = state.clone();
    let mut events = Vec::new();
    match action {
        Action::Continue => {
            s.prev = Some(BlockType::Paragraph);
        }
        Action::ContinueBlank => {
            if s.pending.is_empty() {
                // A loose blank flips every open list frame; record all of
                // them so later B_pre/B_post can pick the right depth.
                s.pending = s
                    .stack
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| list_kind(f.kind()))
                    .map(|(d, f)| (d, f.kind()))
                    .collect();
            }
            s.prev = Some(BlockType::Blank);
        }
        Action::Pop => {
            if let Some(f) = s.stack.pop() {
                events.push(Event { line: line_no, op: EventOp::Q, frame: f });
            }
            // Depths are stack indexes; entries at/above the popped top are
            // stale.
            s.pending.retain(|(d, _)| *d < s.stack.len());
            s.prev = Some(BlockType::Blank);
        }
        Action::Push(frames) => {
            for f in frames {
                s.stack.push(*f);
                events.push(Event { line: line_no, op: EventOp::P, frame: *f });
            }
            s.pending.clear();
            s.prev = Some(BlockType::Paragraph);
        }
        Action::PopPush(f) => {
            if let Some(old) = s.stack.pop() {
                events.push(Event { line: line_no, op: EventOp::Q, frame: old });
            }
            s.stack.push(*f);
            events.push(Event { line: line_no, op: EventOp::P, frame: *f });
            s.pending.clear();
            s.prev = Some(BlockType::Paragraph);
        }
        Action::BodyMut(f) => {
            if let Some(top) = s.stack.last_mut() {
                *top = BlockFrame::new(top.kind(), top.indent(), f.body());
            }
            events.push(Event { line: line_no, op: EventOp::B, frame: *f });
            s.prev = Some(BlockType::Paragraph);
        }
        Action::Composite(actions) => {
            for a in actions {
                let (next, ev) = apply(&s, a, line_no);
                s = next;
                events.extend(ev);
            }
        }
    }
    (s, events)
}

/// First-slice line alphabet. Small but enough for BQ/list/leaf graph.
pub fn line_classes() -> Vec<LineClass> {
    vec![
        LineClass { blank: true, bq: 0, marker: None, indent: 0, content_empty: true, lazy: false },
        LineClass { blank: false, bq: 1, marker: None, indent: 0, content_empty: false, lazy: false },
        LineClass { blank: false, bq: 0, marker: Some(Marker::Bullet), indent: 0, content_empty: false, lazy: false },
        LineClass { blank: false, bq: 0, marker: Some(Marker::Ordered), indent: 0, content_empty: false, lazy: false },
        LineClass { blank: false, bq: 1, marker: Some(Marker::Bullet), indent: 0, content_empty: false, lazy: false },
        LineClass { blank: false, bq: 0, marker: Some(Marker::Bullet), indent: 2, content_empty: false, lazy: false },
        LineClass { blank: false, bq: 2, marker: None, indent: 0, content_empty: false, lazy: false },
        LineClass { blank: false, bq: 1, marker: Some(Marker::Ordered), indent: 0, content_empty: false, lazy: false },
        LineClass { blank: false, bq: 2, marker: Some(Marker::Bullet), indent: 0, content_empty: false, lazy: false },
        LineClass { blank: false, bq: 0, marker: None, indent: 0, content_empty: false, lazy: true },
    ]
}

pub fn realize_line(_state: &MuState, line: &LineClass) -> Option<String> {
    if line.blank {
        return Some(String::new());
    }
    if line.lazy {
        return Some("p".to_string());
    }
    let mut s = String::new();
    for _ in 0..line.bq {
        s.push_str("> ");
    }
    for _ in 0..line.indent {
        s.push(' ');
    }
    if let Some(m) = line.marker {
        s.push_str(marker_text(m));
    }
    if !line.content_empty {
        s.push('x');
    }
    Some(s)
}

pub fn build_graph(max_states: usize) -> Graph {
    let start = MuState::start();
    let start_key = start.key();
    let mut visited: HashSet<StateKey> = HashSet::new();
    visited.insert(start_key.clone());
    let mut queue: VecDeque<MuState> = VecDeque::new();
    queue.push_back(start);
    let mut edges = Vec::new();
    let mut states = vec![start_key.clone()];
    let mut parent: HashMap<StateKey, (StateKey, usize)> = HashMap::new();
    let mut capped = false;

    while let Some(s) = queue.pop_front() {
        if states.len() >= max_states {
            capped = true;
            break;
        }
        let from = s.key();
        for (li, lc) in line_classes().into_iter().enumerate() {
            let Some(text) = realize_line(&s, &lc) else { continue };
            let action = t(&s, &lc);
            let (next, events) = apply(&s, &action, li);
            let to = next.key();
            let edge_idx = edges.len();
            edges.push(Edge {
                from: from.clone(),
                to: to.clone(),
                line: lc,
                line_type: line_type_for(&s, &lc),
                text,
                action,
                events,
            });
            if !visited.contains(&to) {
                if states.len() >= max_states {
                    capped = true;
                    continue;
                }
                visited.insert(to.clone());
                states.push(to.clone());
                parent.insert(to.clone(), (from.clone(), edge_idx));
                queue.push_back(next);
            }
        }
    }

    Graph { start: start_key, edges, states, capped, parent }
}

impl Graph {
    pub fn path_text(&self, key: &StateKey) -> String {
        let mut lines = Vec::new();
        let mut cur = key.clone();
        let mut guard = 0usize;
        while let Some((parent, edge_idx)) = self.parent.get(&cur) {
            guard += 1;
            if guard > self.edges.len() + 1 {
                break;
            }
            lines.push(self.edges[*edge_idx].text.clone());
            cur = parent.clone();
            if cur == self.start {
                break;
            }
        }
        lines.reverse();
        let mut out = lines.join("\n");
        if !out.is_empty() {
            out.push('\n');
        }
        out
    }

    pub fn path_events(&self, key: &StateKey) -> Vec<Event> {
        let mut segments: Vec<Vec<Event>> = Vec::new();
        let mut cur = key.clone();
        let mut guard = 0usize;
        while let Some((parent, edge_idx)) = self.parent.get(&cur) {
            guard += 1;
            if guard > self.edges.len() + 1 {
                break;
            }
            segments.push(self.edges[*edge_idx].events.clone());
            cur = parent.clone();
            if cur == self.start {
                break;
            }
        }
        segments.reverse();
        let mut out = Vec::new();
        for seg in segments {
            out.extend(seg);
        }
        out
    }

    pub fn path_line_types(&self, key: &StateKey) -> Vec<BlockType> {
        self.path_edge_indices(key)
            .into_iter()
            .map(|i| self.edges[i].line_type)
            .collect()
    }

    pub fn path_edge_indices(&self, key: &StateKey) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = key.clone();
        let mut guard = 0usize;
        while let Some((parent, edge_idx)) = self.parent.get(&cur) {
            guard += 1;
            if guard > self.edges.len() + 1 {
                break;
            }
            out.push(*edge_idx);
            cur = parent.clone();
            if cur == self.start {
                break;
            }
        }
        out.reverse();
        out
    }

    pub fn coverage(&self) -> Coverage {
        let mut c = Coverage {
            states: self.states.len(),
            edges: self.edges.len(),
            ..Coverage::default()
        };
        c.capped = self.capped;
        let mut lc = HashSet::new();
        let mut ak = HashSet::new();
        let mut ab = HashSet::new();
        let mut lb = HashSet::new();
        let mut lap = HashSet::new();
        for e in &self.edges {
            lc.insert(e.line);
            ak.insert(action_key(&e.action));
            lap.insert((e.line, action_key(&e.action)));
            for ev in &e.events {
                match ev.op {
                    EventOp::P => c.p_events += 1,
                    EventOp::Q => c.q_events += 1,
                    EventOp::B => c.b_events += 1,
                }
            }
        }
        for key in &self.states {
            let path = self.path_edge_indices(key);
            for w in path.windows(2) {
                ab.insert((action_key(&self.edges[w[0]].action), action_key(&self.edges[w[1]].action)));
                lb.insert((self.edges[w[0]].line, self.edges[w[1]].line));
            }
        }
        c.line_classes = lc.len();
        c.action_kinds = ak.len();
        c.action_bigrams = ab.len();
        c.line_bigrams = lb.len();
        c.target_pairs = c.states * c.line_classes;
        c.realized_pairs = c.edges;
        c.line_action_pairs = lap.len();
        c
    }

    /// Coverage against a pre-declared kind-sequence scope, not against the
    /// BFS-discovered state set (which would make the ratio tautological).
    pub fn scope_coverage(&self, scope: &Scope) -> Coverage {
        let mut c = self.coverage();
        let declared_seq = scope.declared_kind_sequences();
        let mut covered_seq = HashSet::new();
        for key in &self.states {
            covered_seq.insert(kind_seq(key));
        }
        let mut covered_trans = HashSet::new();
        for e in &self.edges {
            covered_trans.insert((kind_seq(&e.from), e.line));
        }
        c.declared_kind_sequences = declared_seq;
        c.covered_kind_sequences = covered_seq.len();
        c.declared_transition_pairs = declared_seq.saturating_mul(scope.line_classes);
        c.covered_transition_pairs = covered_trans.len();
        c.unreachable_kind_sequences = declared_seq.saturating_sub(c.covered_kind_sequences);
        c.unreachable_transition_pairs =
            c.declared_transition_pairs.saturating_sub(c.covered_transition_pairs);
        c
    }
}

/// True document enumeration: DFS over line choices, emitting every concrete
/// path up to `max_lines` (not one BFS representative per state).
pub fn enumerate_docs(max_lines: usize, max_docs: Option<usize>) -> Enumeration {
    let classes = line_classes();
    let mut docs: Vec<Doc> = Vec::new();
    let mut capped = false;
    let start = MuState::start();
    let mut text = String::new();
    let mut events: Vec<Event> = Vec::new();
    let mut line_types: Vec<BlockType> = Vec::new();
    let mut states = vec![start.key()];

    fn dfs(
        state: &MuState,
        depth: usize,
        max_lines: usize,
        max_docs: Option<usize>,
        classes: &[LineClass],
        text: &mut String,
        events: &mut Vec<Event>,
        line_types: &mut Vec<BlockType>,
        states: &mut Vec<StateKey>,
        docs: &mut Vec<Doc>,
        capped: &mut bool,
    ) {
        if depth > 0 {
            if let Some(limit) = max_docs {
                if docs.len() >= limit {
                    *capped = true;
                    return;
                }
            }
            docs.push(Doc {
                text: text.clone(),
                events: events.clone(),
                line_types: line_types.clone(),
                states: states.clone(),
            });
        }
        if depth == max_lines {
            return;
        }
        for lc in classes {
            if let Some(limit) = max_docs {
                if docs.len() >= limit {
                    *capped = true;
                    return;
                }
            }
            let Some(realized) = realize_line(state, lc) else {
                continue;
            };
            let action = t(state, lc);
            let (next, ev) = apply(state, &action, depth);
            let text_mark = text.len();
            let ev_mark = events.len();
            let lt_mark = line_types.len();
            let st_mark = states.len();

            text.push_str(&realized);
            text.push('\n');
            events.extend(ev);
            line_types.push(line_type_for(state, lc));
            states.push(next.key());
            dfs(
                &next,
                depth + 1,
                max_lines,
                max_docs,
                classes,
                text,
                events,
                line_types,
                states,
                docs,
                capped,
            );

            text.truncate(text_mark);
            events.truncate(ev_mark);
            line_types.truncate(lt_mark);
            states.truncate(st_mark);
        }
    }

    dfs(
        &start,
        0,
        max_lines,
        max_docs,
        &classes,
        &mut text,
        &mut events,
        &mut line_types,
        &mut states,
        &mut docs,
        &mut capped,
    );
    Enumeration { docs, max_lines, capped }
}

/// Prefix-gated enumeration: emits only leaves at `max_lines`, and descends
/// into a child only when `gate(state)` is true at depth `max_lines - 1`.
/// This prunes subtrees for sampling without re-running the full depth.
pub fn enumerate_where<F: Fn(&MuState, usize) -> bool>(max_lines: usize, gate: F) -> Enumeration {
    let classes = line_classes();
    let mut docs: Vec<Doc> = Vec::new();
    let start = MuState::start();
    let mut text = String::new();
    let mut events: Vec<Event> = Vec::new();
    let mut line_types: Vec<BlockType> = Vec::new();
    let mut states = vec![start.key()];

    fn dfs<F: Fn(&MuState, usize) -> bool>(
        state: &MuState,
        depth: usize,
        max_lines: usize,
        gate: &F,
        classes: &[LineClass],
        text: &mut String,
        events: &mut Vec<Event>,
        line_types: &mut Vec<BlockType>,
        states: &mut Vec<StateKey>,
        docs: &mut Vec<Doc>,
    ) {
        if depth == max_lines {
            docs.push(Doc {
                text: text.clone(),
                events: events.clone(),
                line_types: line_types.clone(),
                states: states.clone(),
            });
            return;
        }
        // The gate is consulted at every depth: staged samplers prune each
        // level with that level's key set instead of walking the full prefix
        // space up to max_lines - 1.
        if depth > 0 && !gate(state, depth) {
            return;
        }
        for lc in classes {
            let Some(realized) = realize_line(state, lc) else {
                continue;
            };
            let action = t(state, lc);
            let (next, ev) = apply(state, &action, depth);
            let text_mark = text.len();
            let ev_mark = events.len();
            let lt_mark = line_types.len();
            let st_mark = states.len();

            text.push_str(&realized);
            text.push('\n');
            events.extend(ev);
            line_types.push(line_type_for(state, lc));
            states.push(next.key());
            dfs(
                &next,
                depth + 1,
                max_lines,
                gate,
                classes,
                text,
                events,
                line_types,
                states,
                docs,
            );

            text.truncate(text_mark);
            events.truncate(ev_mark);
            line_types.truncate(lt_mark);
            states.truncate(st_mark);
        }
    }

    dfs(
        &start,
        0,
        max_lines,
        &gate,
        &classes,
        &mut text,
        &mut events,
        &mut line_types,
        &mut states,
        &mut docs,
    );
    Enumeration { docs, max_lines, capped: false }
}

/// Rebuild a μ Doc from an already-generated text: parses each line back into
/// its LineClass, replays T/apply, and reconstructs events/line_types/states.
/// Used by out-of-tree checkers to minimize a witness without re-enumerating.
pub fn mu_doc(text: &str) -> Option<Doc> {
    let (state, events, line_types, states) = mu_doc_state(text)?;
    Some(Doc {
        text: text.to_string(),
        events,
        line_types,
        states,
    })
}

/// Replay state from text without materializing the Doc.
pub fn mu_doc_state(
    text: &str,
) -> Option<(MuState, Vec<Event>, Vec<BlockType>, Vec<StateKey>)> {
    let mut raw_lines: Vec<&str> = text.split('\n').collect();
    if raw_lines.last() == Some(&"") {
        raw_lines.pop();
    }
    let mut state = MuState::start();
    let mut events = Vec::new();
    let mut line_types = Vec::new();
    let mut states = vec![state.key()];
    for (i, line) in raw_lines.iter().enumerate() {
        let lc = lineclass_of(line)?;
        let action = t(&state, &lc);
        let (next, ev) = apply(&state, &action, i);
        events.extend(ev);
        line_types.push(line_type_for(&state, &lc));
        states.push(next.key());
        state = next;
    }
    Some((state, events, line_types, states))
}

/// Append one concrete line class to a generated document and return the
/// extended μ Doc. Used by samplers to force a final line class without a
/// full re-enumeration.
pub fn extend_doc(text: &str, lc: &LineClass) -> Option<Doc> {
    let (state, mut events, mut line_types, mut states) = mu_doc_state(text)?;
    let realized = realize_line(&state, lc)?;
    let action = t(&state, lc);
    let (next, ev) = apply(&state, &action, line_types.len());
    let mut out_text = text.to_string();
    if !out_text.is_empty() && !out_text.ends_with('\n') {
        out_text.push('\n');
    }
    out_text.push_str(&realized);
    out_text.push('\n');
    events.extend(ev);
    line_types.push(line_type_for(&state, lc));
    states.push(next.key());
    Some(Doc {
        text: out_text,
        events,
        line_types,
        states,
    })
}

/// μ as an analysis assistant: search the line-choice tree in DFS order and
/// return the first (shortest-depth) document matching the predicate.  Used to
/// localize a bug to a minimal state transition instead of a long corpus.
pub fn find_witness<F>(max_lines: usize, mut pred: F) -> Option<Doc>
where
    F: FnMut(&Doc) -> bool,
{
    let classes = line_classes();
    let start = MuState::start();
    let mut text = String::new();
    let mut events: Vec<Event> = Vec::new();
    let mut line_types: Vec<BlockType> = Vec::new();
    let mut states = vec![start.key()];

    fn dfs<F>(
        state: &MuState,
        depth: usize,
        max_lines: usize,
        classes: &[LineClass],
        text: &mut String,
        events: &mut Vec<Event>,
        line_types: &mut Vec<BlockType>,
        states: &mut Vec<StateKey>,
        pred: &mut F,
    ) -> Option<Doc>
    where
        F: FnMut(&Doc) -> bool,
    {
        if depth > 0 {
            let doc = Doc {
                text: text.clone(),
                events: events.clone(),
                line_types: line_types.clone(),
                states: states.clone(),
            };
            if pred(&doc) {
                return Some(doc);
            }
        }
        if depth == max_lines {
            return None;
        }
        for lc in classes {
            let Some(realized) = realize_line(state, lc) else {
                continue;
            };
            let action = t(state, lc);
            let (next, ev) = apply(state, &action, depth);
            let text_mark = text.len();
            let ev_mark = events.len();
            let lt_mark = line_types.len();
            let st_mark = states.len();

            text.push_str(&realized);
            text.push('\n');
            events.extend(ev);
            line_types.push(line_type_for(state, lc));
            states.push(next.key());
            if let Some(found) = dfs(
                &next, depth + 1, max_lines, classes, text, events, line_types, states, pred,
            ) {
                return Some(found);
            }
            text.truncate(text_mark);
            events.truncate(ev_mark);
            line_types.truncate(lt_mark);
            states.truncate(st_mark);
        }
        None
    }

    dfs(
        &start, 0, max_lines, &classes, &mut text, &mut events, &mut line_types, &mut states,
        &mut pred,
    )
}

/// Parse one first-slice line into its LineClass.  Complex lines (tables,
/// fences, HTML, tags) return None.
pub fn lineclass_of(line: &str) -> Option<LineClass> {
    if line.trim().is_empty() {
        return Some(LineClass {
            blank: true, bq: 0, marker: None, indent: 0, content_empty: true, lazy: false,
        });
    }
    let mut rest = line;
    let mut bq = 0u8;
    while rest.starts_with('>') {
        bq += 1;
        rest = &rest[1..];
        if rest.starts_with(' ') || rest.starts_with('\t') {
            rest = &rest[1..];
        } else {
            break;
        }
    }
    let indent = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let after = &rest[indent..];
    let (marker, content) = if let Some(r) = after.strip_prefix("- ")
        .or_else(|| after.strip_prefix("* "))
        .or_else(|| after.strip_prefix("+ "))
    {
        (Some(Marker::Bullet), r)
    } else if after.starts_with("1. ") {
        (Some(Marker::Ordered), &after[3..])
    } else if after.starts_with("1) ") {
        (Some(Marker::OrderedParen), &after[3..])
    } else {
        (None, after)
    };
    Some(LineClass {
        blank: false,
        bq,
        marker,
        indent: indent as u8,
        content_empty: content.is_empty(),
        // Plain text is a potential lazy continuation; the list/BQ branches
        // decide whether it actually continues the container.
        lazy: marker.is_none() && bq == 0 && !content.is_empty(),
    })
}

/// Run the μ transition path over a document's first-slice lines.
pub fn mu_events_for(text: &str) -> Option<Vec<(EventOp, BlockFrame)>> {
    let mut raw_lines: Vec<&str> = text.split('\n').collect();
    if raw_lines.last() == Some(&"") {
        raw_lines.pop();
    }
    let mut state = MuState::start();
    let mut out = Vec::new();
    for (i, line) in raw_lines.iter().enumerate() {
        let lc = lineclass_of(line)?;
        let action = t(&state, &lc);
        let (next, ev) = apply(&state, &action, i);
        out.extend(ev.into_iter().map(|e| (e.op, e.frame)));
        state = next;
    }
    Some(out)
}

/// Line classes deliberately outside the initial coverage. Each entry is a
/// declared gap, not a silent omission: restoring one requires its scanner
/// behavior to be modeled and edge-checked first.
pub const EXCLUDED_FIRST_SLICE: &[(&str, &str)] = &[
    ("star_plus_top_level", "needs marker-body-specific list replacement rules"),
    ("ordered_nested_under_bq_pending", "scanner may close BQ or insert BulletList"),
    ("bq_nested_indented_bullet", "BQ-relative marker/content column not yet modeled"),
    ("empty_item", "empty marker after BQ/list has context-dependent closure"),
    ("bq_container_only_nested", "container-only BQ line under two BQs reopens/closes"),
];

/// Module boundary: v2 does not fork/duplicate the generator;
/// it pins the existing public surface as a regression oracle.
pub mod legacy {
    pub use crate::trinity::exhaustive_gen::{all_levels, nested_docs, templates};
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mu_path_events(lines: &[LineClass]) -> Vec<(EventOp, BlockFrame)> {
        let mut state = MuState::start();
        let mut out = Vec::new();
        for (i, lc) in lines.iter().enumerate() {
            let action = t(&state, lc);
            let (next, ev) = apply(&state, &action, i);
            out.extend(ev.into_iter().map(|e| (e.op, e.frame)));
            state = next;
        }
        out
    }

    #[test]
    fn scope_coverage_has_external_denominator() {
        let g = build_graph(500);
        let scope = Scope::first_slice();
        let c = g.scope_coverage(&scope);
        assert_eq!(c.declared_kind_sequences, 1 + 3 + 9 + 27);
        assert!(c.covered_kind_sequences <= c.declared_kind_sequences);
        assert_eq!(c.declared_transition_pairs, c.declared_kind_sequences * scope.line_classes);
        assert!(c.covered_transition_pairs <= c.declared_transition_pairs);
        assert!(c.covered_kind_sequences > 0);
        assert_eq!(
            c.unreachable_kind_sequences,
            c.declared_kind_sequences - c.covered_kind_sequences
        );
        assert_eq!(
            c.unreachable_transition_pairs,
            c.declared_transition_pairs - c.covered_transition_pairs
        );
    }

    #[test]
    fn excluded_first_slice_is_declared() {
        assert!(EXCLUDED_FIRST_SLICE.len() >= 3);
        for (name, reason) in EXCLUDED_FIRST_SLICE {
            assert!(!name.is_empty());
            assert!(!reason.is_empty());
        }
    }

    #[test]
    fn graph_e1_status_smoke() {
        use crate::projection::State;
        use crate::trinity::entries::edge1_status;
        use crate::trinity::obs::RealBundle;

        let g = build_graph(200);
        let mut checked = 0usize;
        for key in g.states.iter().skip(1) {
            let text = g.path_text(key);
            let mut st = State::new(&text);
            let snap = st.snapshot();
            let bundle = RealBundle::from_snapshot(&snap);
            let report = edge1_status(&bundle);
            assert!(
                report.bits.e1,
                "E1 red for {text:?} events={:?} tree={}",
                actual_events(&text),
                snap.debug_block_tree()
            );
            checked += 1;
        }
        assert!(checked > 5, "checked={checked}");
    }

    #[test]
    fn graph_e3_status_smoke() {
        use crate::projection::State;
        use crate::trinity::entries::edge3_status;
        use crate::trinity::obs::RealBundle;

        let g = build_graph(200);
        let mut checked = 0usize;
        for key in g.states.iter().skip(1) {
            let text = g.path_text(key);
            if text.trim().is_empty() {
                continue;
            }
            let mut st = State::new(&text);
            let snap = st.snapshot();
            let bundle = RealBundle::from_snapshot(&snap);
            let golden = g.path_line_types(key);
            let actual = crate::trinity::obs::row_decisions(&bundle.lines);
            assert_eq!(edge3_status(&bundle, &golden), Some(true), "E3 red for {text:?} golden={golden:?} actual={actual:?}");
            checked += 1;
        }
        assert!(checked > 5, "checked={checked}");
    }

    #[test]
    fn export_v2_docs_for_external_cmark() {
        let Ok(dir) = std::env::var("V2_EXPORT_DIR") else {
            return;
        };
        std::fs::create_dir_all(&dir).expect("create export dir");
        let e = enumerate_docs(4, None);
        let mut n = 0usize;
        for d in e.docs {
            if d.text.trim().is_empty() {
                continue;
            }
            std::fs::write(format!("{dir}/doc_{n:04}.md"), d.text).expect("write doc");
            n += 1;
        }
        std::fs::write(
            format!("{dir}/MANIFEST"),
            format!("{n}\nmax_lines={}\ncapped={}\n", e.max_lines, e.capped),
        )
        .expect("write manifest");
        assert!(n > 0);
    }

    #[test]
    fn enumerate_docs_is_exhaustive_up_to_depth() {
        let l = line_classes().len();
        let e = enumerate_docs(3, None);
        assert!(!e.capped);
        assert_eq!(e.docs.len(), l + l * l + l * l * l);
        let mut seen = HashSet::new();
        for d in &e.docs {
            assert!(!d.text.is_empty());
            assert!(seen.insert(d.text.clone()));
        }
    }

    #[test]
    fn enumerate_docs_respects_cap() {
        let e = enumerate_docs(8, Some(10));
        assert!(e.capped);
        assert_eq!(e.docs.len(), 10);
    }

    #[test]
    fn enumerate_where_gates_prefix() {
        let l = line_classes().len();
        assert_eq!(enumerate_where(3, |_, _| true).docs.len(), l * l * l);
        assert_eq!(enumerate_where(3, |_, _| false).docs.len(), 0);
    }

    #[test]
    fn mu_find_witness_localizes_blank_bq_close() {
        // Search the line tree for the first doc whose event stream contains
        // Q(BulletList) followed by Q(BlockQuote) — the blank boundary close.
        let found = find_witness(3, |d| {
            d.events.windows(2).any(|w| {
                w[0].op == EventOp::Q
                    && w[0].frame.kind() == FrameKind::BulletList
                    && w[1].op == EventOp::Q
                    && w[1].frame.kind() == FrameKind::BlockQuote
            })
        });
        let doc = found.expect("witness");
        assert_eq!(doc.text, "\n> - x\n- x\n");
    }

    #[test]
    fn mu_lineclass_of_and_events_match_state() {
        let lc = lineclass_of("> > - x").expect("line class");
        assert_eq!(lc.bq, 2);
        assert_eq!(lc.marker, Some(Marker::Bullet));
        let lc2 = lineclass_of("> 1. x").expect("line class");
        assert_eq!(lc2.bq, 1);
        assert_eq!(lc2.marker, Some(Marker::Ordered));

        let mu = mu_events_for("> - x\n\n> x\n").expect("mu path");
        let actual = actual_events("> - x\n\n> x\n");
        assert_eq!(mu, actual, "μ path should reproduce the fixed production");
    }

    #[test]
    fn mu_reproduces_blank_bq_gap_but_needs_oracle() {
        let lines = vec![
            LineClass { blank: false, bq: 1, marker: Some(Marker::Bullet), indent: 0, content_empty: false, lazy: false },
            LineClass { blank: true, bq: 0, marker: None, indent: 0, content_empty: true, lazy: false },
            LineClass { blank: false, bq: 1, marker: None, indent: 0, content_empty: false, lazy: false },
        ];
        let model = mu_path_events(&lines);
        let actual = actual_events("> - x\n\n> x\n");
        let expected = vec![
            (EventOp::P, frame(FrameKind::BlockQuote, 0, 2)),
            (EventOp::P, frame(FrameKind::BulletList, 2, 0)),
            (EventOp::Q, frame(FrameKind::BulletList, 2, 0)),
            (EventOp::Q, frame(FrameKind::BlockQuote, 0, 2)),
            (EventOp::P, frame(FrameKind::BlockQuote, 0, 2)),
        ];
        assert_eq!(actual, expected, "production blank-boundary close is fixed");
        assert_eq!(model, actual, "mu T follows the fixed production transition");
    }

    #[test]
    fn mu_reproduces_bq_depth_gap_but_needs_oracle() {
        let lines = vec![
            LineClass { blank: false, bq: 2, marker: Some(Marker::Bullet), indent: 0, content_empty: false, lazy: false },
            LineClass { blank: false, bq: 1, marker: Some(Marker::Bullet), indent: 0, content_empty: false, lazy: false },
        ];
        let model = mu_path_events(&lines);
        let actual = actual_events("> > - x\n> - x\n");
        let expected = vec![
            (EventOp::P, frame(FrameKind::BlockQuote, 0, 2)),
            (EventOp::P, frame(FrameKind::BlockQuote, 0, 2)),
            (EventOp::P, frame(FrameKind::BulletList, 2, 0)),
            (EventOp::Q, frame(FrameKind::BulletList, 2, 0)),
            (EventOp::Q, frame(FrameKind::BlockQuote, 0, 2)),
            (EventOp::P, frame(FrameKind::BulletList, 2, 0)),
        ];
        assert_eq!(actual, expected, "production BQ-depth close is fixed");
        assert_eq!(model, actual, "mu T follows the fixed production transition");
    }

    #[test]
    fn marker_body_codes() {
        assert_eq!(marker_kind_body(Marker::Bullet), (FrameKind::BulletList, 0));
        assert_eq!(marker_kind_body(Marker::Star), (FrameKind::BulletList, 16));
        assert_eq!(marker_kind_body(Marker::Plus), (FrameKind::BulletList, 32));
        assert_eq!(marker_kind_body(Marker::Ordered), (FrameKind::OrderedList, 0));
        assert_eq!(marker_kind_body(Marker::OrderedParen), (FrameKind::OrderedList, 32));
    }

    #[test]
    fn ordered_nested_dot_matches_state() {
        let s = MuState {
            stack: vec![frame(FrameKind::OrderedList, 3, 0)],
            pending: Vec::new(),
            prev: None,
        };
        let lc = LineClass {
            blank: false,
            bq: 0,
            marker: Some(Marker::Ordered),
            indent: 3,
            content_empty: false,
            lazy: false,
        };
        let a = t(&s, &lc);
        let (_n, ev) = apply(&s, &a, 0);
        let expected: Vec<_> = ev.iter().map(|e| (e.op, e.frame)).collect();
        let actual = actual_events("1. x\n   1. x\n");
        assert_eq!(expected, actual[1..]);
    }

    #[test]
    fn graph_reaches_bq_and_list() {
        let g = build_graph(2000);
        assert!(g.states.len() > 5, "states={}", g.states.len());
        assert!(g.edges.len() > 10, "edges={}", g.edges.len());

        // A path ending in BQ text should contain "> x".
        let bq = g
            .states
            .iter()
            .find(|k| k.stack.len() == 1 && k.stack[0] == frame(FrameKind::BlockQuote, 0, 2).0)
            .expect("BQ state");
        assert!(g.path_text(bq).contains("> x"));
    }

    #[test]
    fn list_sibling_is_poppush() {
        let s = MuState {
            stack: vec![frame(FrameKind::BulletList, 2, 0)],
            pending: Vec::new(),
            prev: None,
        };
        let lc = LineClass {
            blank: false,
            bq: 0,
            marker: Some(Marker::Bullet),
            indent: 0,
            content_empty: false,
            lazy: false,
        };
        let a = t(&s, &lc);
        assert!(matches!(a, Action::PopPush(_)), "got {a:?}");
        let (_n, ev) = apply(&s, &a, 0);
        assert_eq!(ev.iter().map(|e| e.op).collect::<Vec<_>>(), vec![EventOp::Q, EventOp::P]);
    }

    #[test]
    fn blank_top_bq_pops() {
        let s = MuState {
            stack: vec![frame(FrameKind::BlockQuote, 0, 2)],
            pending: Vec::new(),
            prev: None,
        };
        let lc = LineClass { blank: true, bq: 0, marker: None, indent: 0, content_empty: true, lazy: false };
        let a = t(&s, &lc);
        let (_n, ev) = apply(&s, &a, 0);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].op, EventOp::Q);
    }

    #[test]
    fn blank_top_list_continues() {
        let s = MuState {
            stack: vec![frame(FrameKind::BulletList, 2, 0)],
            pending: Vec::new(),
            prev: None,
        };
        let lc = LineClass { blank: true, bq: 0, marker: None, indent: 0, content_empty: true, lazy: false };
        assert_eq!(t(&s, &lc), Action::ContinueBlank);
    }

    fn actual_events(text: &str) -> Vec<(EventOp, BlockFrame)> {
        let mut st = crate::projection::State::new(text);
        let snap = st.snapshot();
        snap.frame_log
            .entries
            .iter()
            .map(|&e| {
                let frame = crate::block::entry_frame(e);
                if crate::block::entry_is_body_mut(e) {
                    (EventOp::B, frame)
                } else if (e >> 63) != 0 {
                    (EventOp::Q, frame)
                } else {
                    (EventOp::P, frame)
                }
            })
            .collect()
    }

    #[test]
    fn graph_paths_match_state_fl() {
        let g = build_graph(5000);
        let mut checked = 0usize;
        for key in g.states.iter().skip(1) {
            let text = g.path_text(key);
            let expected = mu_events_for(&text).expect("μ parses its own path text");
            let actual = actual_events(&text);
            assert_eq!(
                expected, actual,
                "path mismatch\nstate={key:?}\ntext={text:?}\nexpected={expected:?}\nactual={actual:?}"
            );
            checked += 1;
        }
        assert!(checked > 5, "checked={checked}");
    }

    #[test]
    fn coverage_ledger_counts_graph() {
        let g = build_graph(500);
        let c = g.coverage();
        assert_eq!(c.states, g.states.len());
        assert_eq!(c.edges, g.edges.len());
        assert!(c.edges > c.states, "edges={} states={}", c.edges, c.states);
        assert!(c.p_events > 0);
        assert!(c.q_events > 0);
        assert!(c.b_events > 0, "b_events={}", c.b_events);
        assert!(c.line_classes > 3, "line_classes={}", c.line_classes);
        assert!(c.action_kinds > 1, "action_kinds={}", c.action_kinds);
        assert!(c.action_bigrams > 0, "action_bigrams={}", c.action_bigrams);
        assert!(c.line_bigrams > 0, "line_bigrams={}", c.line_bigrams);
        assert_eq!(c.target_pairs, c.states * c.line_classes);
        assert_eq!(c.realized_pairs, c.edges);
        assert!(c.line_action_pairs > 0);
        if !c.capped {
            assert_eq!(c.realized_pairs, c.target_pairs, "uncapped graph must realize every state×line pair");
        }
    }

    #[test]
    fn graph_edges_match_state_fl() {
        let g = build_graph(1000);
        let mut checked = 0usize;
        for e in &g.edges {
            let mut text = g.path_text(&e.from);
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&e.text);
            text.push('\n');
            let expected = mu_events_for(&text).expect("μ parses its own path text");
            let actual = actual_events(&text);
            assert_eq!(
                expected, actual,
                "edge mismatch\nfrom={:?}\nline={:?}\ntext={text:?}\nexpected={expected:?}\nactual={actual:?}",
                e.from, e.line
            );
            checked += 1;
        }
        assert!(checked > 10, "checked={checked}");
    }

    mod legacy_regression {
        use super::legacy;

        #[test]
        fn legacy_counts_unchanged() {
            assert_eq!(legacy::templates().len(), 27);
            let (l1, l2, l3) = legacy::all_levels();
            assert_eq!((l1.len(), l2.len(), l3.len()), (27, 729, 27));

            let strict = legacy::nested_docs(2, usize::MAX);
            let deep = legacy::nested_docs(5, 1200);
            assert_eq!(strict.len(), 459);
            assert_eq!(deep.len(), 3051);
            assert_eq!(deep.len() - strict.len(), 2592);
        }
    }
}
