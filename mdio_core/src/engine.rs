use std::sync::Arc;
use std::sync::OnceLock;
use crop::Rope;
use std::ops::Range;
use memchr;
use regex::Regex;
use serde::{Serialize, Deserialize};
use serde_json::Value as JsonValue;
use crate::store::ProjectStore;
use crate::block::BlockType;

use crate::inline;
use crate::op::get_bpe;

pub use crate::op::{TreeSummary, TreeResponse};

// Selector types
pub use crate::block::BlockNode;
pub use crate::selector::{
    CmpOp, Value, ByQuantifier,
    FilterExpr, TypeName, FilterMethod, SingleSlice, Step,
};

pub type MdioSnapshot = crate::projection::Snapshot;

use crate::projection::State;

impl MdioSnapshot {
    pub fn select_nodes(&self, steps: &[Step]) -> Vec<BlockNode> {
        crate::selector::select_nodes(self, steps)
    }

    pub fn to_string(&self) -> String { self.rope.to_string() }

    pub fn run_find(&self, steps: &[Step], query_configs: &[(String, bool, bool)], ctx_b: usize, ctx_a: usize) -> Vec<String> {
        crate::op::run_find(self, steps, query_configs, ctx_b, ctx_a)
    }

    pub fn run_tree(&self, steps: &[Step], max_depth: usize, detail_level: usize) -> String {
        crate::op::run_tree(self, steps, max_depth, detail_level)
    }

    pub fn heading_at_line(&self, line_no: usize) -> String {
        crate::op::reverse_pathfinding(self, line_no)
    }

}

pub struct MdioEngine {
    pub state: State,
}

impl MdioEngine {
    pub fn new(text: &str) -> Self {
        Self { state: State::new(text) }
    }

    pub fn from_state(state: State) -> Self {
        Self { state }
    }

    pub fn make_snapshot(&mut self) -> MdioSnapshot {
        self.state.snapshot()
    }

    pub fn ensure_ast(&mut self) {
        self.state.ensure_projected()
    }

    pub fn edit(&mut self, start_byte: usize, end_byte: usize, text: &str) {
        self.state.edit(start_byte, end_byte, text);
    }

    pub fn byte_len(&self) -> usize { self.state.rope.byte_len() }
    pub fn byte_of_line(&self, line: usize) -> usize { self.state.rope.byte_of_line(line) }
    pub fn line_of_byte(&self, byte: usize) -> usize {
        // char-boundary 防御（同 projection::snap_down_char_boundary）——
        // CRDT 回环偏移漂移时防 panic
        self.state.rope.line_of_byte(crate::projection::snap_down_char_boundary(&self.state.rope, byte))
    }
    pub fn line_len(&self) -> usize { self.state.rope.line_len() }
    pub fn byte_at(&self, idx: usize) -> u8 { self.state.rope.byte(idx) }
    pub fn byte_slice(&self, range: Range<usize>) -> crop::RopeSlice<'_> {
        let start = crate::projection::snap_down_char_boundary(&self.state.rope, range.start);
        let mut end = crate::projection::snap_down_char_boundary(&self.state.rope, range.end);
        if end < start { end = start; }
        self.state.rope.byte_slice(start..end)
    }
    pub fn rope_clone(&self) -> Rope { self.state.rope.clone() }

}

use crate::projection::sanitize_nul;

pub(crate) use crate::block::is_header;

pub(crate) use crate::selector::{NodeCategory, node_category};

pub(crate) use crate::block::{trim_start, header_level};

pub use crate::block::{BlockProj, InlineFeed, BlockProjSpec, BlockWriteTarget, block_proj_spec, BlockMask};


// Projection / Truncation



pub use crate::inline::{InlineSegmentRole, InlineSegment};

pub use crate::selector::SelectorIR;

pub use crate::op::ScaffoldInsert;

impl MdioSnapshot {
    pub fn build_scaffold(&self, steps: &[Step], operator: &str, payload: &str) -> Option<ScaffoldInsert> {
        crate::op::build_scaffold(self, steps, operator, payload, 0)
    }
}

// 辅助函数（可放在 engine.rs 顶部或同一模块）
pub use crate::selector::is_simple_selector;
pub(crate) use crate::selector::extract_head_parts;


// Tests


#[cfg(test)]
mod block_classify_tests {
    include!("../tests/unit/block_classify.rs");
}

#[cfg(test)]
mod framelog_rebuild_tests {
    include!("../tests/unit/framelog_rebuild.rs");
}

#[cfg(test)]
mod ripple_edge_tests {
    include!("../tests/unit/ripple_edge.rs");
}

#[cfg(test)]
mod inline_integration_tests {
    include!("../tests/unit/inline_integration.rs");
}

#[cfg(test)]
mod html_element_tests {
    include!("../tests/unit/html_element.rs");
}

#[cfg(test)]
mod html_integration_tests {
    include!("../tests/unit/html_integration.rs");
}

#[cfg(test)]
mod cont_flag_tests {
    include!("../tests/unit/cont_flag.rs");
}

#[cfg(test)]
mod replay_cost_probe_tests {
    include!("../tests/unit/replay_cost_probe.rs");
}

#[cfg(test)]
mod prefix_invariance_probe_tests {
    include!("../tests/unit/prefix_invariance_probe.rs");
}

#[cfg(test)]
mod boundary_shape_probe_tests {
    include!("../tests/unit/boundary_shape_probe.rs");
}

#[cfg(test)]
mod atomic_assembly_probe_tests {
    include!("../tests/unit/atomic_assembly_probe.rs");
}
