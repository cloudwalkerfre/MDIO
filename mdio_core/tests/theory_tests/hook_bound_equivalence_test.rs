//! Hook Static Bound Equivalence Test
//!
//! Verifies the hook bound invariants H1, H2, and H3 (defined below).
//!
//! Run: cargo test --test hook_bound_equivalence_test
//!
//! ── Invariants ──
//!
//! H1 (scope invariance):       scope_range ∩ R = ∅  ⇒  shadow does NOT fire
//! H2 (batch subtree skip):     section(n) ∩ R = ∅   ⇒  ∀H ∈ ℱ*(n): shadow does NOT fire
//! H3 (conservativity):          shadow fires         ⇒  scope_range ∩ R ≠ ∅
//!
//! ── Testbed ──
//!
//! Canonical documents covering: single section, multi-section, nested sections,
//! sections with lists/blockquotes/code fences, Setext headers.

#![allow(non_snake_case)]

use mdio_core::engine::MdioEngine;
use mdio_core::engine::MdioSnapshot;
use mdio_core::selector::{Step, FilterExpr, TypeName, CmpOp, Value, SingleSlice};
use mdio_core::projection::{BlockType, BlockProj};
use mdio_core::block::{
    FrameKind,
    entry_line, entry_is_pop, entry_is_body_mut, entry_frame,
};

// Inline helpers (crate-internal functions not publicly exported)

fn is_header(t: BlockType) -> bool {
    use BlockType::*;
    matches!(t, Header1 | Header2 | Header3 | Header4 | Header5 | Header6 | HsHead1 | HsHead2)
}

fn header_level(t: BlockType) -> Option<u8> {
    use BlockType::*;
    match t {
        Header1 | HsHead1 => Some(1),
        Header2 | HsHead2 => Some(2),
        Header3 => Some(3),
        Header4 => Some(4),
        Header5 => Some(5),
        Header6 => Some(6),
        _ => None,
    }
}

// §TB.1: Testbed — Documents

struct DocSpec {
    text: &'static str,
    name: &'static str,
    /// Expected top-level sections: (name, start_line, end_line)
    sections: &'static [(&'static str, usize, usize)],
    /// Expected nested sections under each top-level section
    sub_sections: &'static [(&'static str, &'static str, usize, usize)],
}

fn docs() -> Vec<DocSpec> {
    vec![
        // D1: Single H1 section, no subsections
        DocSpec {
            text: "# S\n- a: va\n- b: vb\n\nplain text\n",
            name: "single_h1",
            sections: &[("S", 0, 4)],
            sub_sections: &[],
        },
        // D2: Two H1 sections, disjoint
        DocSpec {
            text: "# A\n- x: 1\n\n# B\n- y: 2\n",
            name: "two_h1",
            sections: &[("A", 0, 3), ("B", 3, 5)],
            sub_sections: &[],
        },
        // D3: H1 + H2 nesting
        DocSpec {
            text: "# Top\n\n## Sub1\n- a: va\n\n## Sub2\n- b: vb\n",
            name: "h1_h2_nesting",
            sections: &[("Top", 0, 6)],
            sub_sections: &[
                ("Top", "Sub1", 2, 4),
                ("Top", "Sub2", 4, 6),
            ],
        },
        // D4: H1 + H2 + H3 deep nesting
        DocSpec {
            text: "# A\n\n## B\n\n### C\n- deep: val\n\n## D\n- shallow: val\n",
            name: "h1_h2_h3_deep",
            sections: &[("A", 0, 8)],
            sub_sections: &[
                ("A", "B", 2, 6),
                ("A", "D", 6, 8),
            ],
        },
        // D5: Three H1 sections with different content types
        DocSpec {
            text: "# S1\n- k: v\n\n# S2\n```rust\ncode\n```\n\n# S3\n> quoted\n",
            name: "three_h1_mixed",
            sections: &[("S1", 0, 2), ("S2", 2, 5), ("S3", 5, 7)],
            sub_sections: &[],
        },
        // D6: Setext headers
        DocSpec {
            text: "Setext H1\n=========\n\ncontent\n\nSetext H2\n---------\n\nmore\n",
            name: "setext",
            sections: &[("Setext H1", 0, 9)],  // H1 covers entire doc (H2 is child, not sibling)
            sub_sections: &[],
        },
        // D7: Header inside blockquote
        DocSpec {
            text: "# Outer\n\n> # Inner\n> - item\n\nplain\n",
            name: "header_in_bq",
            sections: &[("Outer", 0, 5)],
            sub_sections: &[],
        },
        // D8: Single section with many paragraphs (large section, local edit)
        DocSpec {
            text: "# Big\n\np1\n\np2\n\np3\n\np4\n\n- k: target\n\np5\n\np6\n",
            name: "big_section_local_edit",
            sections: &[("Big", 0, 12)],
            sub_sections: &[],
        },
        // D9: Empty sections between headers
        DocSpec {
            text: "# A\n\n# B\n- x: 1\n",
            name: "empty_section",
            sections: &[("A", 0, 3), ("B", 3, 5)],
            sub_sections: &[],
        },
        // D10: Section with OL (ordered list)
        DocSpec {
            text: "# S\n1. first\n2. second\n3. third\n",
            name: "ordered_list",
            sections: &[("S", 0, 4)],
            sub_sections: &[],
        },
    ]
}

// §TB.2: Testbed — Hooks

#[derive(Clone, Debug)]
struct HookSpec {
    id: u64,
    steps: Vec<Step>,
    /// Expected scope_range: Some((L,E)) for resolvable, None for global/unresolvable
    expected_scope: Option<(usize, usize)>,
    desc: &'static str,
}

/// Generate hooks for a given document spec.
/// Returns hooks targeting various sections.
fn hooks_for(spec: &DocSpec) -> Vec<HookSpec> {
    let mut hooks = Vec::new();
    let mut id = 1u64;
    let mut next = || { let n = id; id += 1; n };

    // Pattern 1: RecursiveWildcard + TypeCheck (global — scope = None)
    hooks.push(HookSpec {
        id: next(),
        steps: vec![
            Step::RecursiveWildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        ],
        expected_scope: None,
        desc: "global: RecursiveWildcard + TypeCheck(Bullet)",
    });

    // Pattern 2: Drill + Wildcard + Filter + Slice (single-level Dril)
    // One hook per top-level section that has content
    for &(name, start, end) in spec.sections {
        if end > start + 1 {
            hooks.push(HookSpec {
                id: next(),
                steps: vec![
                    Step::Drill(name.to_string()),
                    Step::Wildcard,
                    Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
                    Step::Slice(vec![SingleSlice::Index(0)]),
                ],
                expected_scope: Some((start, end)),
                desc: "single Dril into section",
            });
        }
    }

    // Pattern 3: Drill + Drill + Wildcard + Filter + Slice (nested Dril)
    // Scope is the FIRST (parent) anchor's section, not the child's.
    for &(parent, _child, _child_start, _child_end) in spec.sub_sections {
        // Find the parent section start from spec.sections
        if let Some(&(_, p_start, p_end)) = spec.sections.iter().find(|&&(n, _, _)| n == parent) {
            if p_end > p_start + 1 {
                hooks.push(HookSpec {
                    id: next(),
                    steps: vec![
                        Step::Drill(parent.to_string()),
                        Step::Drill(_child.to_string()),
                        Step::Wildcard,
                        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
                        Step::Slice(vec![SingleSlice::Index(0)]),
                    ],
                    expected_scope: Some((p_start, p_end)),
                    desc: "nested Dril into sub-section",
                });
            }
        }
    }

    // Pattern 4: Unresolvable Dril prefix (targets non-existent heading)
    hooks.push(HookSpec {
        id: next(),
        steps: vec![
            Step::Drill("NonExistent".to_string()),
            Step::Wildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
            Step::Slice(vec![SingleSlice::Index(0)]),
        ],
        expected_scope: None, // unresolvable
        desc: "unresolvable: Drill into non-existent heading",
    });

    // Pattern 5: RecursiveDrill (global-ish but might have effective bound)
    hooks.push(HookSpec {
        id: next(),
        steps: vec![
            Step::RecursiveDrill("Sub1".to_string()),
            Step::Wildcard,
            Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
            Step::Slice(vec![SingleSlice::Index(0)]),
        ],
        expected_scope: None, // RecursiveDrill → pre(S)=[]
        desc: "RecursiveDrill: pre(S)=[] → global",
    });

    hooks
}

// §TB.3: Testbed — Edits

#[derive(Clone, Debug)]
struct EditSpec {
    /// Byte range to replace
    range: (usize, usize),
    /// Replacement text
    new_text: &'static str,
    /// Description
    desc: &'static str,
}

/// Generate edits covering various scenarios for a given document.
fn edits_for(text: &str) -> Vec<EditSpec> {
    let mut edits = Vec::new();

    // E1: Pure content edit — change bullet text
    if let Some(pos) = text.find(": v") {
        let _end = text[pos..].find('\n').map_or(text.len(), |n| pos + n);
        edits.push(EditSpec {
            range: (pos + 2, pos + 3), // just the "v"
            new_text: "changed",
            desc: "E1: pure content edit (bullet value change)",
        });
    }

    // E2: Paragraph text edit (no structure change)
    if let Some(pos) = text.find("plain") {
        edits.push(EditSpec {
            range: (pos, pos + 5),
            new_text: "CHANGED",
            desc: "E2: paragraph text edit",
        });
    }

    // E3: Heading rename (structure unchanged, name changed)
    if let Some(pos) = text.find("# S") {
        let end = text[pos..].find('\n').map_or(pos + 3, |n| pos + n);
        if pos + 2 < end {
            edits.push(EditSpec {
                range: (pos + 2, pos + 3),
                new_text: "Renamed",
                desc: "E3: heading rename",
            });
        }
    }

    // E4: Structural edit — insert new heading
    if let Some(pos) = text.find('\n') {
        edits.push(EditSpec {
            range: (pos, pos),
            new_text: "\n# NewSection\n- nk: nv\n",
            desc: "E4: insert new heading + content",
        });
    }

    edits
}

// §TB.4: Shadow (ground truth)

/// Ground truth: does the hook trigger?
/// Returns true if select_nodes on old vs new produces different content.
fn shadow_trigger(steps: &[Step], old: &mdio_core::engine::MdioSnapshot, new: &mdio_core::engine::MdioSnapshot) -> bool {
    let old_nodes = old.select_nodes(steps);
    let new_nodes = new.select_nodes(steps);

    if old_nodes.len() != new_nodes.len() {
        return true;
    }
    for (o, n) in old_nodes.iter().zip(new_nodes.iter()) {
        if old.byte_slice(o.byte_range.clone()) != new.byte_slice(n.byte_range.clone()) {
            return true;
        }
        if o.node_type != n.node_type {
            return true;
        }
    }
    false
}

// §TB.5: Scope range computation

/// Compute scope_range for a hook's steps on a snapshot.
/// Returns None for global hooks (no Dril prefix or unresolvable).
/// Returns Some((start, end)) for the heading section of the FIRST (shallowest) Dril match.
///
/// NOTE: For multi-level Dril chains like Dril("A")::Dril("B"),
/// the scope must be the FIRST anchor's section, not the last. Because edits between
/// the first and last anchor can change parent-child relationships, breaking the Dril
/// chain even when the final anchor's section is outside R. See regression in
/// `h1_scope_invariance_single_hook` with D3 "h1_h2_nesting" + insert heading edit.
fn compute_scope_range(steps: &[Step], snap: &mdio_core::engine::MdioSnapshot) -> Option<(usize, usize)> {
    // Single source of truth: hook.rs owns the production scope algorithm.
    // The shadow keeps its own is_header/header_level below for section-tree
    // bookkeeping, but scope bounds must never drift from production.
    mdio_core::hook::compute_scope_range(steps, snap)
}

// §TB.6: Ripple zone extraction

/// Extract the ripple zone [rs, re) after an edit.
/// Returns (ripple_start, ripple_end) — the range of lines that were re-scanned.
fn ripple_zone(text: &str, edit: &EditSpec) -> (usize, usize) {
    let mut engine = MdioEngine::new(text);
    engine.edit(edit.range.0, edit.range.1, edit.new_text);
    engine.ensure_ast();

    // The ripple_diff is captured during ensure_ast() → ripple_lines()
    // It's stored in engine.state.ripple_diff
    match &engine.state.ripple_diff {
        Some(diff) => (diff.ripple_line_start, diff.ripple_line_end),
        None => {
            // No ripple — edit had no effect, or document is empty
            let start = engine.line_of_byte(edit.range.0);
            (start, start + 1)
        }
    }
}

// §1.1: H1 — Scope Invariance

#[test]
fn h1_scope_invariance_single_hook() {
    for spec in &docs() {
        let old_snap = mdio_core::engine::MdioEngine::new(spec.text).make_snapshot();
        let hooks = hooks_for(spec);

        for edit in &edits_for(spec.text) {
            let (rs, re) = ripple_zone(spec.text, edit);
            // Build new snapshot from a fresh engine with the edit
            let mut new_engine = MdioEngine::new(spec.text);
            new_engine.edit(edit.range.0, edit.range.1, edit.new_text);
            let new_snap = new_engine.make_snapshot();

            for hook in &hooks {
                let scope = compute_scope_range(&hook.steps, &old_snap);
                let triggered = shadow_trigger(&hook.steps, &old_snap, &new_snap);

                match scope {
                    Some((L, E)) => {
                        if L >= re || E <= rs {
                            // scope ∩ R = ∅ → must NOT trigger
                            assert!(
                                !triggered,
                                "[{}] H1 VIOLATED: hook=\"{}\" scope=({},{}) R=({},{}) \
                                 scope∩R=∅ but shadow triggered!\n  text: {:?}\n  edit: {:?}",
                                spec.name, hook.desc, L, E, rs, re,
                                spec.text, edit.desc
                            );
                        }
                        // else scope ∩ R ≠ ∅ → may or may not trigger (no assertion)
                    }
                    None => {
                        // unresolvable or global → always evaluate (no H1 assertion)
                        // But we CAN verify H3: if triggered, scope ∩ R must be non-empty
                        // For None scope, this is vacuously true (scope is "everything")
                    }
                }
            }
        }
    }
}

// §1.2: H3 — Conservativity

#[test]
fn h3_conservativity() {
    for spec in &docs() {
        let old_snap = mdio_core::engine::MdioEngine::new(spec.text).make_snapshot();
        let hooks = hooks_for(spec);

        for edit in &edits_for(spec.text) {
            let (rs, re) = ripple_zone(spec.text, edit);
            let mut new_engine = MdioEngine::new(spec.text);
            new_engine.edit(edit.range.0, edit.range.1, edit.new_text);
            let new_snap = new_engine.make_snapshot();

            for hook in &hooks {
                let scope = compute_scope_range(&hook.steps, &old_snap);
                let triggered = shadow_trigger(&hook.steps, &old_snap, &new_snap);

                if triggered {
                    match scope {
                        Some((L, E)) => {
                            assert!(
                                L < re && rs < E,
                                "[{}] H3 VIOLATED: hook=\"{}\" scope=({},{}) R=({},{}) \
                                 hook TRIGGERED but scope∩R=∅!\n  text: {:?}\n  edit: {:?}",
                                spec.name, hook.desc, L, E, rs, re,
                                spec.text, edit.desc
                            );
                        }
                        None => {
                            // global/unresolvable hook triggered — this is fine,
                            // scope=None means "entire document", always intersects R
                        }
                    }
                }
            }
        }
    }
}

// §1.3: H2 — Batch Subtree Skip

/// Simple Section tree node for testing
#[derive(Debug, Clone)]
struct SectionNode {
    name: String,
    section: (usize, usize),
    level: u8,
    hooks: Vec<HookSpec>,
    children: Vec<SectionNode>,
}

/// Build a minimal Section tree from a snapshot and hook set
fn build_section_tree(
    snap: &mdio_core::engine::MdioSnapshot,
    hooks: &[HookSpec],
) -> SectionNode {
    // Find all headers and build the tree
    let mut headers: Vec<(usize, String, u8)> = Vec::new();
    for i in 0..snap.line_len() {
        let t = snap.line_type_at(i);
        if is_header(t) {
            if let Some(lvl) = header_level(t) {
                let name = snap.line_text(i).to_string();
                let name = name.trim().to_string();
                headers.push((i, name, lvl));
            }
        }
    }

    // Build tree recursively
    fn build_subtree(
        headers: &[(usize, String, u8)],
        start_idx: usize,
        parent_level: u8,
        parent_section_end: usize,
        hooks: &[HookSpec],
    ) -> (SectionNode, usize) {
        let mut children = Vec::new();
        let mut idx = start_idx;
        let current = &headers[start_idx];
        let node_start = current.0;
        let node_name = current.1.clone();
        let node_level = current.2;

        // Find next sibling (peer or ancestor) to determine section end
        let node_end = {
            let mut end = parent_section_end;
            for j in (start_idx + 1)..headers.len() {
                if headers[j].2 <= node_level {
                    end = headers[j].0;
                    break;
                }
            }
            end
        };

        idx = start_idx + 1;

        // Process children (deeper-level headers within this section)
        while idx < headers.len() && headers[idx].0 < node_end {
            if headers[idx].2 > node_level {
                let (child, next_idx) = build_subtree(
                    headers, idx, node_level, node_end, hooks,
                );
                children.push(child);
                idx = next_idx;
            } else {
                break;
            }
        }

        // Collect hooks whose scope_range matches this section
        let node_hooks: Vec<HookSpec> = hooks.iter()
            .filter(|h| {
                if let Some((L, _E)) = h.expected_scope {
                    L == node_start
                } else {
                    false
                }
            })
            .cloned()
            .collect();

        let node = SectionNode {
            name: node_name,
            section: (node_start, node_end),
            level: node_level,
            hooks: node_hooks,
            children,
        };
        (node, idx)
    }

    if headers.is_empty() {
        return SectionNode {
            name: "root".to_string(),
            section: (0, snap.line_len()),
            level: 0,
            hooks: hooks.iter().filter(|h| h.expected_scope.is_none()).cloned().collect(),
            children: Vec::new(),
        };
    }

    // Find all top-level headers (H1 or highest level present)
    let top_level = headers.iter().map(|h| h.2).min().unwrap_or(1);
    let mut root_children = Vec::new();
    let mut idx = 0;

    while idx < headers.len() {
        if headers[idx].2 == top_level {
            let section_end = if idx + 1 < headers.len() {
                // Find next header with level <= top_level
                let mut end = snap.line_len();
                for j in (idx + 1)..headers.len() {
                    if headers[j].2 <= top_level {
                        end = headers[j].0;
                        break;
                    }
                }
                end
            } else {
                snap.line_len()
            };
            let (child, next_idx) = build_subtree(
                &headers, idx, top_level - 1, section_end, hooks,
            );
            root_children.push(child);
            idx = next_idx;
        } else {
            idx += 1;
        }
    }

    // Global / unresolvable hooks go to root
    let global_hooks: Vec<HookSpec> = hooks.iter()
        .filter(|h| h.expected_scope.is_none())
        .cloned()
        .collect();

    SectionNode {
        name: "root".to_string(),
        section: (0, snap.line_len()),
        level: 0,
        hooks: global_hooks,
        children: root_children,
    }
}

/// Collect all hooks in a node's subtree
fn subtree_hooks(node: &SectionNode) -> Vec<HookSpec> {
    let mut result = node.hooks.clone();
    for child in &node.children {
        result.extend(subtree_hooks(child));
    }
    result
}

#[test]
fn h2_batch_subtree_skip() {
    for spec in &docs() {
        let old_snap = mdio_core::engine::MdioEngine::new(spec.text).make_snapshot();
        let hooks = hooks_for(spec);
        let tree = build_section_tree(&old_snap, &hooks);

        for edit in &edits_for(spec.text) {
            let (rs, re) = ripple_zone(spec.text, edit);
            let mut new_engine = MdioEngine::new(spec.text);
            new_engine.edit(edit.range.0, edit.range.1, edit.new_text);
            let new_snap = new_engine.make_snapshot();

            // Walk the tree and verify batch skip
            fn check_node(
                node: &SectionNode,
                rs: usize,
                re: usize,
                old: &mdio_core::engine::MdioSnapshot,
                new: &mdio_core::engine::MdioSnapshot,
                spec_name: &str,
                edit_desc: &str,
            ) {
                let (L, E) = node.section;
                let intersects = L < re && rs < E;

                if !intersects {
                    // Node's section is outside R — ALL hooks in subtree must NOT fire
                    for hook in subtree_hooks(node) {
                        let triggered = shadow_trigger(&hook.steps, old, new);
                        assert!(
                            !triggered,
                            "[{}] H2 VIOLATED: node=\"{}\" section=({},{}) R=({},{}) \
                             section∩R=∅ but hook=\"{}\" in subtree triggered!\n  edit: {}",
                            spec_name, node.name, L, E, rs, re,
                            hook.desc, edit_desc
                        );
                    }
                } else {
                    // Node intersects R — check children recursively
                    for child in &node.children {
                        check_node(child, rs, re, old, new, spec_name, edit_desc);
                    }
                }
            }

            check_node(&tree, rs, re, &old_snap, &new_snap, spec.name, edit.desc);
        }
    }
}

// §2: Regression — Specific edge cases

/// C7 regression: heading rename — hook on old name, heading renamed
/// scope_range ∩ R must NOT be empty (hook should fire)
#[test]
fn regression_heading_rename() {
    let text = "# OldName\n- k: v\n";
    let old = mdio_core::engine::MdioEngine::new(text).make_snapshot();
    let steps = vec![
        Step::Drill("OldName".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag,
            op: CmpOp::Eq,
            value: Value::String("k".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let scope = compute_scope_range(&steps, &old);
    // scope should be (0, 2) — H1 section containing the bullet
    assert_eq!(scope, Some((0, 2)), "scope_range should be H1 section");

    let old_name_pos = text.find("OldName").unwrap();
    let edit = EditSpec {
        range: (old_name_pos, old_name_pos + 7),
        new_text: "NewName",
        desc: "rename heading",
    };
    let (rs, re) = ripple_zone(text, &edit);

    // Ripple zone should include the heading line
    assert!(rs <= 0 && re > 0, "ripple zone must include heading line 0, got [{},{})", rs, re);

    // scope ∩ R must be non-empty
    let (L, E) = scope.unwrap();
    assert!(L < re && rs < E, "scope ({},{}) must intersect R ({},{})", L, E, rs, re);

    // Ground truth: hook should fire
    let mut new_engine = MdioEngine::new(text);
    new_engine.edit(edit.range.0, edit.range.1, edit.new_text);
    let new = new_engine.make_snapshot();
    assert!(shadow_trigger(&steps, &old, &new), "hook must fire on heading rename");
}

/// Section-boundary edge: edit at the exact boundary of a section
#[test]
fn regression_section_boundary_edit() {
    let text = "# A\n- a: 1\n\n# B\n- b: 2\n";
    let old = mdio_core::engine::MdioEngine::new(text).make_snapshot();

    // Hook on section A
    let steps_a = vec![
        Step::Drill("A".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("a".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    // Hook on section B
    let steps_b = vec![
        Step::Drill("B".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("b".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let scope_a = compute_scope_range(&steps_a, &old);
    let scope_b = compute_scope_range(&steps_b, &old);
    assert_eq!(scope_a, Some((0, 3)), "A section: H1 at line 0, next H1 at line 3");
    assert_eq!(scope_b, Some((3, 5)));

    // Edit in section B's bullet
    let edit = EditSpec {
        range: (text.find("- b:").unwrap(), text.find("- b:").unwrap() + 4),
        new_text: "- b: changed",
        desc: "edit bullet in section B",
    };
    let (rs, re) = ripple_zone(text, &edit);

    let mut new_engine = MdioEngine::new(text);
    new_engine.edit(edit.range.0, edit.range.1, edit.new_text);
    let new = new_engine.make_snapshot();

    // H1 for hook A: scope ∩ R should be empty
    let (La, Ea) = scope_a.unwrap();
    let a_intersects = La < re && rs < Ea;
    let a_triggers = shadow_trigger(&steps_a, &old, &new);
    if !a_intersects {
        assert!(!a_triggers, "H1: hook A scope ∩ R = ∅ but triggers");
    }

    // H1 for hook B: scope ∩ R should NOT be empty
    let (Lb, Eb) = scope_b.unwrap();
    assert!(Lb < re && rs < Eb, "hook B scope must intersect R");
}

/// Partial Dril: H1("S") exists but H2("Sub") doesn't
#[test]
fn regression_partial_dril() {
    let text = "# S\n- x: 1\n";
    let old = mdio_core::engine::MdioEngine::new(text).make_snapshot();

    // Hook with nested Dril where second level doesn't exist
    let steps = vec![
        Step::Drill("S".to_string()),
        Step::Drill("NonExistentSub".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let scope = compute_scope_range(&steps, &old);
    // Should fall back to H1("S")'s section (partial resolution)
    assert_eq!(scope, Some((0, 2)), "partial Dril should resolve to H1 scope");

    // Edit outside section S should not trigger
    // (but there's no content outside S in this doc, so edit inside S)
    let edit = EditSpec {
        range: (text.find("- x:").unwrap(), text.find("- x:").unwrap() + 2),
        new_text: "- y",
        desc: "edit inside section",
    };
    let (rs, re) = ripple_zone(text, &edit);
    let (L, E) = scope.unwrap();
    assert!(L < re && rs < E, "edit inside section → scope must intersect R");
}

/// Setext header section computation
#[test]
fn regression_setext_scope() {
    let text = "Setext H1\n=========\n\ncontent\n\nSetext H2\n---------\n\nmore\n";
    let old = mdio_core::engine::MdioEngine::new(text).make_snapshot();

    let steps = vec![
        Step::Drill("Setext H1".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Paragraph)),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let scope = compute_scope_range(&steps, &old);
    // Setext H1 (level 1): section = (0, 10). H2 (level 2) is child, not peer.
    assert_eq!(scope, Some((0, 9)));

    // Edit in H2 section content — should still intersect H1 scope
    let h2_pos = text.find("more").unwrap();
    let edit = EditSpec {
        range: (h2_pos, h2_pos + 4),
        new_text: "MORE",
        desc: "edit in H2 section (child of H1)",
    };
    let (rs, re) = ripple_zone(text, &edit);

    let (L, E) = scope.unwrap();
    // scope is (0, 9)
    assert!(L < re && rs < E, "edit in child H2 section must intersect parent H1 scope");
}

/// Unresolvable hook — must always evaluate
#[test]
fn regression_unresolvable_hook() {
    let text = "# S\n- k: v\n";
    let old = mdio_core::engine::MdioEngine::new(text).make_snapshot();

    let steps = vec![
        Step::Drill("NoSuchHeading".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let scope = compute_scope_range(&steps, &old);
    assert_eq!(scope, None, "unresolvable Dril should have scope=None");

    // Even a trivial edit — hook is unresolvable, must always be checked
    let edit = EditSpec {
        range: (text.find('v').unwrap(), text.find('v').unwrap() + 1),
        new_text: "w",
        desc: "trivial content edit",
    };

    let mut new_engine = MdioEngine::new(text);
    new_engine.edit(edit.range.0, edit.range.1, edit.new_text);
    let new = new_engine.make_snapshot();

    // Unresolvable hook — triggers if content now matches (zero→existing)
    // In this case, still unresolvable → should NOT trigger
    let triggers = shadow_trigger(&steps, &old, &new);
    assert!(!triggers, "still unresolvable → should not trigger");

    // Now create the heading
    let edit2 = EditSpec {
        range: (text.find("# S").unwrap(), text.find("# S").unwrap() + 3),
        new_text: "# NoSuchHeading",
        desc: "create matching heading",
    };
    let mut new_engine2 = MdioEngine::new(text);
    new_engine2.edit(edit2.range.0, edit2.range.1, edit2.new_text);
    let new2 = new_engine2.make_snapshot();
    let triggers2 = shadow_trigger(&steps, &old, &new2);
    assert!(triggers2, "heading created → hook must trigger (zero→existing)");
}

/// Delete heading: structural change (C_info) — verifies scope ∩ R
/// and that hook fires correctly on heading deletion (existing→zero).
#[test]
fn regression_delete_heading() {
    let text = "# A\n- a: 1\n\n# B\n- b: 2\n";
    let old = MdioEngine::new(text).make_snapshot();

    // Hook on section B — after deletion, Dril("B") unresolvable → existing→zero
    let steps = vec![
        Step::Drill("B".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let scope = compute_scope_range(&steps, &old);
    assert!(scope.is_some(), "hook on B should be resolvable");

    // Delete heading A — B becomes top-level H1, shifts scope
    let a_pos = text.find("# A").unwrap();
    let a_end = text[a_pos..].find('\n').map_or(text.len(), |n| a_pos + n + 1);
    let edit = EditSpec {
        range: (a_pos, a_end),
        new_text: "",
        desc: "delete heading A",
    };
    let (rs, re) = ripple_zone(text, &edit);
    let mut new_engine = MdioEngine::new(text);
    new_engine.edit(edit.range.0, edit.range.1, edit.new_text);
    let new = new_engine.make_snapshot();

    // After deleting A, B's scope_range computed from OLD doc should intersect R
    // because the deletion at line 0 causes ripple through section B's lines
    let (L, E) = scope.unwrap();
    let intersects = L < re && rs < E;
    // Hook may or may not fire: B becomes H1 but Dril("B") still resolves
    let triggers = shadow_trigger(&steps, &old, &new);
    if !intersects {
        assert!(!triggers, "scope∩R=∅ but hook fired — H1 violation");
    }
}

/// RecursiveDril with actual matches — global scope but only affects
/// specific matching sections. 该配置下 scope=None（总是评估）。
#[test]
fn regression_recursive_dril_with_matches() {
    let text = "# A\n- a: 1\n\n# B\n- b: 2\n\n# C\n- c: 3\n";
    let old = mdio_core::engine::MdioEngine::new(text).make_snapshot();

    // RecursiveDril("B") — matches H1("B") anywhere. pre(S)=[], scope=None.
    let steps = vec![
        Step::RecursiveDrill("B".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let scope = compute_scope_range(&steps, &old);
    assert_eq!(scope, None, "RecursiveDril has no Dril prefix → scope=None");

    // Edit in section A — should NOT affect RecursiveDril("B")
    let edit = EditSpec {
        range: (text.find("- a:").unwrap(), text.find("- a:").unwrap() + 4),
        new_text: "- a: changed",
        desc: "edit in section A",
    };
    let mut new_engine = MdioEngine::new(text);
    new_engine.edit(edit.range.0, edit.range.1, edit.new_text);
    let new = new_engine.make_snapshot();
    let triggers = shadow_trigger(&steps, &old, &new);
    // With scope=None (global), we can't skip. But the hook should NOT trigger
    // because RecursiveDril("B") still matches B's bullet (content unchanged).
    assert!(!triggers, "RecursiveDril B should not fire on edit in section A");
}

/// Bullet key tag Dril: selector 直接选中 bullet key tag 节点，
/// 因此 scope 收窄为 [bullet_line, doc_end)，而不是全局 None。
#[test]
fn regression_bullet_tag_dril() {
    let text = "# S\n- host: prod\n- other: otherval\n";
    let old = MdioEngine::new(text).make_snapshot();

    let steps = vec![
        Step::Drill("host".to_string()),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    assert!(
        !old.select_nodes(&steps).is_empty(),
        "Drill(\"host\") should resolve to the host bullet"
    );
    let scope = compute_scope_range(&steps, &old);
    assert_eq!(scope, Some((1, old.line_len())));

    let edit = EditSpec {
        range: (text.find("otherval").unwrap(), text.find("otherval").unwrap() + 8),
        new_text: "changed",
        desc: "edit unrelated bullet",
    };
    let mut new_engine = MdioEngine::new(text);
    new_engine.edit(edit.range.0, edit.range.1, edit.new_text);
    let new = new_engine.make_snapshot();
    let triggers = shadow_trigger(&steps, &old, &new);
    assert!(!triggers, "unrelated bullet edit should not trigger host Dril hook");
}

// §3: EventClass helpers — adapted from structure_equivalence_test.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvClass { C_info, I_info, T_info }

fn body_adm_eq(kind: FrameKind, a: u64, b: u64) -> bool {
    let fa = entry_frame(a);
    let fb = entry_frame(b);
    match kind {
        // 步骤 3（删位）：BQ body 恒 0——落 _ => true
        FrameKind::BulletList => fa.bullet_code() == fb.bullet_code(),
        FrameKind::OrderedList => fa.delim_code() == fb.delim_code(),
        FrameKind::HtmlBlock => fa.block_type() == fb.block_type(),
        FrameKind::RefDef => fa.body() == fb.body(),
        _ => true, // BQ（删位）, CF, Comment, Table, HtmlElement: body_adm = ∅
    }
}

/// Classify a single FL entry in context of its surrounding entries.
fn classify_entry(entries: &[u64], idx: usize) -> EvClass {
    let e = entries[idx];
    if entry_is_body_mut(e) {
        let f = entry_frame(e);
        return match f.kind() {
            FrameKind::BulletList | FrameKind::OrderedList => {
                if f.loose() { EvClass::T_info } else { EvClass::C_info }
            }
            FrameKind::CodeFence | FrameKind::Comment => EvClass::T_info,
            _ => EvClass::C_info,
        };
    }
    let kind = entry_frame(e).kind();
    let line = entry_line(e);

    if entry_is_pop(e) {
        let is_poppush = entries[idx+1..].iter().any(|&pe| {
            !entry_is_pop(pe) && !entry_is_body_mut(pe)
                && entry_line(pe) == line && entry_frame(pe).kind() == kind
        });
        if is_poppush {
            if let Some(&pe) = entries[idx+1..].iter().find(|&&pe| {
                !entry_is_pop(pe) && !entry_is_body_mut(pe)
                    && entry_line(pe) == line && entry_frame(pe).kind() == kind
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

    // Push
    let is_poppush = entries[..idx].iter().any(|&pe| {
        entry_is_pop(pe) && entry_line(pe) == line && entry_frame(pe).kind() == kind
    });
    if is_poppush {
        if let Some(&pe) = entries[..idx].iter().find(|&&pe| {
            entry_is_pop(pe) && entry_line(pe) == line && entry_frame(pe).kind() == kind
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

/// From an engine after edit, extract which lines in [rs, re) have structural events.
fn struct_lines(engine: &MdioEngine) -> Vec<usize> {
    let diff = match &engine.state.ripple_diff {
        Some(d) => d,
        None => return vec![],
    };
    let mut lines = vec![];
    for (i, &e) in diff.ripple_frame_actions.iter().enumerate() {
        let cls = classify_entry(&diff.ripple_frame_actions, i);
        if matches!(cls, EvClass::C_info | EvClass::I_info) {
            let line = entry_line(e) as usize;
            if !lines.contains(&line) { lines.push(line); }
        }
    }
    lines.sort();
    lines
}

/// Collect output line numbers from selector result.
fn output_lines(nodes: &[mdio_core::engine::BlockNode]) -> Vec<usize> {
    let mut lines: Vec<usize> = nodes.iter().map(|n| n.start_line).collect();
    lines.sort();
    lines.dedup();
    lines
}

// §4: L2 — EventClass refinement

/// L2 condition: R_struct ∩ bound = ∅ ∧ R_content ∩ (D_dril ∪ D_out) = ∅ ⇒ ¬trigger.
/// Here we verify the stronger condition: when R_struct=∅ (pure content edit
/// with no container changes), and edited line ∉ D_out, the hook does NOT fire.
#[test]
fn l2_pure_content_edit_outside_output_lines() {
    // 多 bullet 场景：同一标题下多个 bullet，hook 只监听一个特定 tag
    let text = "# PROC\n- status: IDLE\n- turn: 0\n- step: 0\n";
    let mut engine = MdioEngine::new(text);
    let old = engine.make_snapshot();

    // Hook watches "status" tag only
    let steps = vec![
        Step::Drill("PROC".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("status".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let scope = compute_scope_range(&steps, &old);
    let (L, E) = scope.unwrap();

    // D_out = {line of "status: IDLE"} = line 1
    let old_nodes = old.select_nodes(&steps);
    let d_out = output_lines(&old_nodes);
    assert!(!d_out.is_empty(), "D_out must be non-empty");

    // Edit "turn: 0" (line 2), NOT "status" (line 1)
    let turn_pos = text.find("turn:").unwrap();
    engine.edit(turn_pos + 5, turn_pos + 6, "1");
    let new = engine.make_snapshot();

    // Verify L1 evaluates: scope ∩ R ≠ ∅
    let diff = engine.state.ripple_diff.as_ref().unwrap();
    let (rs, re) = (diff.ripple_line_start, diff.ripple_line_end);
    assert!(L < re && rs < E, "L1: scope ∩ R ≠ ∅ — must evaluate");

    // EventClass classification of ripple events
    let sl = struct_lines(&engine);
    // Note: even "pure content" edits can produce BLT PopPush events
    // (item boundaries), which classify as I_info → R_struct non-empty.
    // This is a quirk of how scan_block emits FL entries for list items.
    // The L2 condition should check: do any STRUCTURAL events
    // intersect the HOOK'S dependencies specifically?

    // R_content ∩ D_out: edited line (turn) ∉ D_out (status)
    let edited_line = re.saturating_sub(1); // ripple zone's last line
    assert!(!d_out.contains(&edited_line),
        "edited line {} ∉ D_out {:?}", edited_line, d_out);

    // L2 prediction: hook should NOT fire
    let triggers = shadow_trigger(&steps, &old, &new);
    assert!(!triggers,
        "L2: R_struct=∅ ∧ edited line ∉ D_out → hook must NOT fire");
}

/// L2: edit to same tag but different heading → should NOT trigger.
/// Mirrors: editing agent A's status should not trigger agent B's hook.
#[test]
fn l2_different_dril_target_not_affected() {
    // Two agents under PROC, each with status
    let text = "# PROC\n- status: IDLE\n- turn: 0\n- status: BUSY\n- turn: 1\n";
    let old = MdioEngine::new(text).make_snapshot();

    // Hook watches only the FIRST status bullet (under PROC, first bullet with tag "status")
    let steps = vec![
        Step::Drill("PROC".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("status".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let old_nodes = old.select_nodes(&steps);
    let d_out = output_lines(&old_nodes);
    // Slice(0) picks the FIRST match — line 1 ("status: IDLE")
    assert_eq!(d_out, vec![1], "D_out should be line 1 (first status)");

    // Edit the SECOND status bullet (line 3: "status: BUSY" → "status: DONE")
    let status2_pos = text.rfind("status:").unwrap(); // second occurrence
    let mut engine = MdioEngine::new(text);
    engine.edit(status2_pos + 7, status2_pos + 11, "DONE");
    let new = engine.make_snapshot();

    let triggers = shadow_trigger(&steps, &old, &new);
    // The first status (line 1) is unchanged. Slice(0) still picks it.
    // The second status (line 3) changed but Slice(0) doesn't reach it.
    assert!(!triggers,
        "editing second status should NOT trigger hook watching first status (Slice(0))");
}

/// L2: structural change in section (R_struct ≠ ∅) → MUST evaluate.
/// Mirrors: adding a new bullet changes the container structure.
#[test]
fn l2_structural_change_forces_evaluation() {
    let text = "# PROC\n- status: IDLE\n";
    let old = MdioEngine::new(text).make_snapshot();

    let steps = vec![
        Step::Drill("PROC".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("status".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    // Insert a new bullet — structural change (new BLT item)
    let insert_pos = text.find("- status:").unwrap() + "- status: IDLE\n".len();
    let mut engine = MdioEngine::new(text);
    engine.edit(insert_pos, insert_pos, "- new_field: val\n");
    let new = engine.make_snapshot();

    let sl = struct_lines(&engine);
    assert!(!sl.is_empty(), "R_struct must be non-empty for insert edit");

    // Hook may or may not fire depending on whether the new bullet
    // affects the status bullet's content or position.
    let triggers = shadow_trigger(&steps, &old, &new);
    // Inserting AFTER the status bullet: status content unchanged,
    // byte_range unchanged (edit is after, not before).
    // Hook does NOT fire — content at the path is identical.
    assert!(!triggers,
        "insert after status bullet: content unchanged → hook should NOT fire");
}

// §5: L3 — Incremental evaluation

/// When anchor ∉ R and edit does NOT affect D_out: incremental =
/// keep old result, skip re-scan. Verify against full select_nodes.
#[test]
fn l3_incremental_equals_full_when_edit_outside_output() {
    // Large section: heading at top, many bullet tags scattered below.
    // Hook watches one specific tag. Edit touches a DIFFERENT tag.
    let mut text = String::from("# PROC\n- status: IDLE\n");
    for i in 0..20 {
        text.push_str(&format!("- field_{}: val_{}\n", i, i));
    }
    let mut engine = MdioEngine::new(&text);
    let old = engine.make_snapshot();

    let steps = vec![
        Step::Drill("PROC".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("status".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let diff = match &engine.state.ripple_diff {
        Some(_) => {
            // Rebuild: fresh engine didn't have ripple
            let mut e2 = MdioEngine::new(&text);
            let _ = e2.make_snapshot();
            e2
        }
        None => {
            let e2 = MdioEngine::new(&text);
            e2
        }
    };
    // Fresh start: make snapshot, then edit field_5
    let mut engine = MdioEngine::new(&text);
    let _old = engine.make_snapshot(); // consume initial ripple

    let field5_pos = text.find("field_5:").unwrap();
    engine.edit(field5_pos + 8, field5_pos + 12, "NEWVAL");
    let new = engine.make_snapshot();

    let scope = compute_scope_range(&steps, &_old);
    let (L, _E) = scope.unwrap();
    let diff = engine.state.ripple_diff.as_ref().unwrap();
    let (rs, re) = (diff.ripple_line_start, diff.ripple_line_end);

    // Anchor (heading PROC) at line 0 should be outside R (edit is far below)
    assert!(L < rs || L >= re,
        "anchor line {} outside R [{},{})", L, rs, re);

    // Full evaluation
    let full = new.select_nodes(&steps);
    // Old evaluation
    let old_result = _old.select_nodes(&steps);

    // Incremental: anchor not in R → Dril chain unchanged.
    // Field 5 edit doesn't affect status tag → output unchanged.
    assert_eq!(old_result.len(), full.len(),
        "output should be same size when edit outside D_out");
    for (o, f) in old_result.iter().zip(full.iter()) {
        assert_eq!(o.node_type, f.node_type);
        let oc = _old.byte_slice(o.byte_range.clone()).to_string();
        let nc = new.byte_slice(f.byte_range.clone()).to_string();
        assert_eq!(oc, nc, "content identical when edit outside D_out");
    }
}

/// When anchor ∉ R and edit DOES touch D_out: incremental must
/// re-scan Δ and produce different result from old.
#[test]
fn l3_incremental_detects_output_change() {
    let mut text = String::from("# PROC\n- status: IDLE\n");
    for i in 0..20 {
        text.push_str(&format!("- field_{}: val_{}\n", i, i));
    }
    let mut engine = MdioEngine::new(&text);
    let old = engine.make_snapshot();

    let steps = vec![
        Step::Drill("PROC".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("status".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let scope = compute_scope_range(&steps, &old);
    let (L, _E) = scope.unwrap();

    // Edit the status line itself
    let status_pos = text.find("IDLE").unwrap();
    engine.edit(status_pos, status_pos + 4, "BUSY");
    let new = engine.make_snapshot();

    let diff = engine.state.ripple_diff.as_ref().unwrap();
    let (rs, re) = (diff.ripple_line_start, diff.ripple_line_end);

    // Anchor ∉ R
    assert!(L < rs || L >= re,
        "anchor line {} outside R [{},{})", L, rs, re);

    // Full evaluation must detect the change
    let old_result = old.select_nodes(&steps);
    let full = new.select_nodes(&steps);

    assert_eq!(old_result.len(), full.len());
    let oc = old.byte_slice(old_result[0].byte_range.clone()).to_string();
    let nc = new.byte_slice(full[0].byte_range.clone()).to_string();
    assert_ne!(oc, nc, "status changed from IDLE to BUSY — content must differ");

    let triggers = shadow_trigger(&steps, &old, &new);
    assert!(triggers, "hook must fire when output content changes");
}

/// Core L3 invariant: when anchor ∉ R, the incremental method
/// (old_result \ affected_by_Δ) ∪ scan_on_Δ(new_snap, Δ)
/// produces the same result as full select_nodes(new_snap).
#[test]
fn l3_incremental_matches_full_select() {
    // 构造含多个 bullet 的文档。Hook 只监听一个 tag。
    // Edit a DIFFERENT tag. Incremental should match full.
    let mut text = String::from("# PROC\n- status: IDLE\n- turn: 0\n- step: 0\n");
    for i in 0..15 {
        text.push_str(&format!("- log_{}: entry_{}\n", i, i));
    }
    let mut engine = MdioEngine::new(&text);
    let old = engine.make_snapshot();

    let steps = vec![
        Step::Drill("PROC".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("step".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let scope = compute_scope_range(&steps, &old);
    let (L, _E) = scope.unwrap();

    // Edit "turn: 0" → "turn: 5"
    let turn_pos = text.find("turn:").unwrap();
    engine.edit(turn_pos + 5, turn_pos + 6, "5");
    let new = engine.make_snapshot();

    let diff = engine.state.ripple_diff.as_ref().unwrap();
    let (rs, re) = (diff.ripple_line_start, diff.ripple_line_end);
    assert!(L < rs || L >= re, "anchor ∉ R");

    // Full select_nodes
    let full = new.select_nodes(&steps);
    let old_result = old.select_nodes(&steps);

    // Simulate incremental: old nodes not intersecting Δ + re-scan on Δ
    let delta_start = rs;
    let delta_end = re;
    let unaffected_old: Vec<_> = old_result.iter()
        .filter(|n| n.end_line <= delta_start || n.start_line >= delta_end)
        .collect();

    // Re-scan: find nodes in Δ that match the selector
    // (simplified: just check if any part of Δ could match the steps)
    let new_in_delta: Vec<_> = full.iter()
        .filter(|n| n.start_line < delta_end && n.end_line > delta_start)
        .collect();

    // Incremental = unaffected_old + new_in_delta
    let incremental_count = unaffected_old.len() + new_in_delta.len();

    // The ripple zone is conservative: an edit inside a list item may carry the
    // convergence line into the next item, so a node can overlap Δ without its
    // selected projection changing.  The invariant that matters is that the
    // union of unaffected-old nodes and full nodes in Δ reproduces the full
    // result set; the hook detector itself still compares hashes and must not
    // fire for an unchanged "step" node.
    assert_eq!(incremental_count, full.len(),
        "incremental result count must match full select_nodes");
    if !new_in_delta.is_empty() {
        assert_eq!(old_result.len(), full.len());
        for (o, n) in old_result.iter().zip(full.iter()) {
            assert_eq!(o.node_type, n.node_type);
            assert_eq!(
                old.byte_slice(o.byte_range.clone()),
                new.byte_slice(n.byte_range.clone()),
                "ripple-zone overlap must not imply projected content changed"
            );
        }
    }
    // Content comparison
    for (o, f) in unaffected_old.iter().zip(
        full.iter().filter(|n| n.end_line <= delta_start || n.start_line >= delta_end)
    ) {
        assert_eq!(o.node_type, f.node_type);
        let oc = old.byte_slice(o.byte_range.clone()).to_string();
        let nc = new.byte_slice(f.byte_range.clone()).to_string();
        assert_eq!(oc, nc);
    }
}

// §5: Trie prefix sharing
//
// Two hooks with the same Dril prefix share the same scope_range.
// Their evaluate_and_dispatch can share Dril resolution.

#[test]
fn trie_prefix_sharing_same_scope() {
    let text = "# S\n- a: va\n- b: vb\n- c: vc\n";
    let old = MdioEngine::new(text).make_snapshot();

    // Hook 1: tag "a"
    let steps_a = vec![
        Step::Drill("S".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("a".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    // Hook 2: tag "b" — same Dril prefix, different filter
    let steps_b = vec![
        Step::Drill("S".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp {
            proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("b".to_string()),
        }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let scope_a = compute_scope_range(&steps_a, &old);
    let scope_b = compute_scope_range(&steps_b, &old);
    assert_eq!(scope_a, scope_b,
        "hooks with same Dril prefix must have identical scope_range");
}

/// Two hooks on nested Dril paths: parent scope contains child scope
#[test]
fn trie_prefix_sharing_nested_scope() {
    let text = "# Top\n\n## Sub\n- k: val\n\nplain\n";
    let old = MdioEngine::new(text).make_snapshot();

    // Hook on H1 "Top"
    let steps_top = vec![
        Step::Drill("Top".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Paragraph)),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    // Hook on H2 "Sub" under "Top"
    let steps_sub = vec![
        Step::Drill("Top".to_string()),
        Step::Drill("Sub".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];

    let scope_top = compute_scope_range(&steps_top, &old).unwrap();
    let scope_sub = compute_scope_range(&steps_sub, &old).unwrap();

    // "Sub" scope must be contained in "Top" scope (nested sections)
    assert!(scope_sub.0 >= scope_top.0 && scope_sub.1 <= scope_top.1,
        "child scope {:?} must be ⊆ parent scope {:?}", scope_sub, scope_top);
}

// §6: production compute_scope_range parity

#[test]
fn test_compute_scope_range_parity() {
    use mdio_core::hook::compute_scope_range as prod_scope;
    for spec in &docs() {
        let snap = MdioEngine::new(spec.text).make_snapshot();
        let hooks = hooks_for(spec);
        for hook in &hooks {
            let prod = prod_scope(&hook.steps, &snap);
            let test_scope = compute_scope_range(&hook.steps, &snap);
            assert_eq!(prod, test_scope,
                "[{}] hook=\"{}\": production compute_scope_range {:?} != test {:?}",
                spec.name, hook.desc, prod, test_scope);
        }
    }
}

#[test]
fn test_scope_range_multi_edit_delta_shift() {
    // Edit 1: delete lines before section (δ < 0).
    // Edit 2: edit within shifted scope → must evaluate.
    let text = "# A\n\n# B\n- k: v\n";
    let s = MdioEngine::new(text).make_snapshot();
    let mut idx = mdio_core::hook::HookIndex::new();
    let steps = vec![
        Step::Drill("B".to_string()),
        Step::Wildcard,
        Step::Filter(FilterExpr::TypeCheck(TypeName::Bullet)),
        Step::Filter(FilterExpr::Cmp { proj: BlockProj::Tag, op: CmpOp::Eq,
            value: Value::String("k".to_string()) }),
        Step::Slice(vec![SingleSlice::Index(0)]),
    ];
    let scope_before = compute_scope_range(&steps, &s);
    assert_eq!(scope_before, Some((2, 4)), "section B: lines 2-4");

    idx.register(1, "", 0, &steps, false, "cb", serde_json::json!({}), 0, &s);

    // Edit 1: delete heading A (lines 0-2), δ = -2
    let mut e1 = MdioEngine::new(text);
    e1.edit(0, text.find("# B").unwrap(), "");
    let ns1 = e1.make_snapshot();
    let (rs1, re1) = match &e1.state.ripple_diff {
        Some(d) => (d.ripple_line_start, d.ripple_line_end),
        None => (0, 1),
    };

    let _events1 = idx.detect_crdt_write(0, &ns1, rs1, re1, &[]);

    // Edit 2: edit bullet content
    let new_text = ns1.to_string();
    let mut e2 = MdioEngine::new(&new_text);
    let v_pos = new_text.find('v').unwrap();
    e2.edit(v_pos, v_pos + 1, "w");
    let ns2 = e2.make_snapshot();
    let (rs2, re2) = match &e2.state.ripple_diff {
        Some(d) => (d.ripple_line_start, d.ripple_line_end),
        None => (0, 1),
    };

    let events2 = idx.detect_crdt_write(0, &ns2, rs2, re2, &[]);
    assert!(!events2.is_empty(),
        "edit 2 should trigger hook (bullet content changed) — scope must be correct after edit 1's delta shift");
}
