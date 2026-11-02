
// mdio_core/src/snap.rs — Semantic Time Subsystem (SNAP)

//
// Content-addressable checkpoint primitives for MDIO file versions:
//   state_hash = SHA256(file_content)          — per-file version identity
//   snap_hash  = SHA256(parent | author | message | at | file_states)
//
// Storage layout:
//   /.mds/snap.md            — snap registry with branch HEAD pointers
//   /.mds/ckpt/YYYY-MM.md    — content store, ## state_hash per version
//

use sha2::{Sha256, Digest};
use serde::Serialize;
use ahash::AHashMap as HashMap;


// Hashing utilities — per-field hasher.update() avoids String alloc


/// SHA256 hex digest of file content.  Returns 64 lower-case hex chars.
/// Used as both the ckpt ## section name and the per-file fingerprint.
///
/// Test vector:
///   SHA256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
pub fn state_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// SHA256 snap identity hash.
///
/// Fields are hashed individually via `hasher.update()` (zero intermediate
/// String allocation).  `file_states` is a pre-built, sorted string of
/// `path\0state_hash` pairs joined by `\0`.
pub fn snap_hash(
    parent: &str,
    author: &str,
    message: &str,
    at: &str,
    file_states: &str,
) -> String {
    let safe_author = author.replace("|", "\\|");
    let safe_message = message.replace("|", "\\|");
    let mut hasher = Sha256::new();
    hasher.update(parent.as_bytes());
    hasher.update(b"|");
    hasher.update(safe_author.as_bytes());
    hasher.update(b"|");
    hasher.update(safe_message.as_bytes());
    hasher.update(b"|");
    hasher.update(at.as_bytes());
    hasher.update(b"|");
    hasher.update(file_states.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Build the file_states string — sorted `path:state_hash` pairs joined by \0.
/// Format: `path1:hash1\0path2:hash2\0...`.
/// Mirrored in Python snap_ops.py for hash consistency.
pub fn build_file_states(notes: &HashMap<String, String>) -> String {
    let mut paths: Vec<&String> = notes.keys().collect();
    paths.sort();
    let pairs: Vec<String> = paths.iter()
        .map(|p| format!("{}:{}", p, notes[*p]))
        .collect();
    pairs.join("\0")
}


// Diff — gix::diff::blob::UnifiedDiff + diffy apply


use std::fmt::Write as _;

use gix::diff::blob::{
    diff_with_slider_heuristics, Algorithm, InternedInput, UnifiedDiff,
};
use gix::diff::blob::unified_diff::{
    ConsumeHunk, HunkHeader, DiffLineKind, ContextSize,
};

/// Collector that renders unified diff lines into a String.
struct DiffCollector(String);

impl ConsumeHunk for DiffCollector {
    type Out = String;

    fn consume_hunk(&mut self, header: HunkHeader, lines: &[(DiffLineKind, &[u8])]) -> std::io::Result<()> {
        let _ = writeln!(self.0,
            "@@ -{},{} +{},{} @@",
            header.before_hunk_start, header.before_hunk_len,
            header.after_hunk_start, header.after_hunk_len);

        for (kind, line) in lines {
            let prefix = match kind {
                DiffLineKind::Context => " ",
                DiffLineKind::Add => "+",
                DiffLineKind::Remove => "-",
            };
            self.0.push_str(prefix);
            let s = String::from_utf8_lossy(line);
            // Strip trailing \n — diffy's parser stores lines without it
            self.0.push_str(s.strip_suffix("\n").unwrap_or(&s));
            self.0.push('\n');
        }
        Ok(())
    }

    fn finish(self) -> Self::Out {
        self.0
    }
}

/// Produce a unified diff with 3 lines of context via gix::diff::blob.
/// Returns empty string when texts are identical.
pub fn snap_diff(text_a: &str, text_b: &str) -> String {
    let input = InternedInput::new(text_a.as_bytes(), text_b.as_bytes());
    let diff = diff_with_slider_heuristics(Algorithm::Histogram, &input);
    let collector = DiffCollector(String::new());
    let unified = UnifiedDiff::new(&diff, &input, collector, ContextSize::symmetrical(3));
    unified.consume().unwrap_or_default()
}

/// Apply a unified diff to `before` text, returning the reconstructed result.
/// Uses diffy for patch parsing and application.
pub fn snap_apply(before: &str, diff_text: &str) -> Result<String, String> {
    if diff_text.is_empty() {
        return Ok(before.to_string());
    }
    let patch = diffy::Patch::from_str(diff_text)
        .map_err(|e| format!("diffy parse: {e}"))?;
    diffy::apply(before, &patch)
        .map_err(|e| format!("diffy apply: {e}"))
}


// SnapMeta — parsed representation of one snap entry


/// Parsed metadata for a single snap checkpoint.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SnapMeta {
    pub hash: String,
    pub parent: String,
    pub author: String,
    pub message: String,
    pub at: String,
    pub tag: String,
    /// VFS path → state_hash for every file captured by this snap.
    pub notes: HashMap<String, String>,
}

/// Parse a single `### <hash>` block from snap.md.
///
/// Expected format (matches SNAP.md spec):
/// ```markdown
/// ### <snap_hash>
/// - parent: <parent_hash>
/// - author: <name>
/// - message: <text>
/// - at: <iso8601>
/// - tag: <optional>
/// - notes:
///   - /path/file.md: <state_hash>
/// ```
pub fn parse_snap_meta(md_text: &str) -> Option<SnapMeta> {
    let mut meta = SnapMeta::default();
    let mut in_notes = false;

    for line in md_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() { continue; }

        // ### heading → snap hash (must be 64-char hex)
        if trimmed.starts_with("### ") {
            let candidate = trimmed[4..].trim();
            if candidate.len() == 64 && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
                meta.hash = candidate.to_string();
            }
            continue;
        }

        // #### notes → enter notes block (h4 heading)
        if trimmed == "#### notes" {
            in_notes = true;
            continue;
        }

        if in_notes {
            // Flat bullet: - /path/file.md: <state_hash>
            if let Some(inner) = trimmed.strip_prefix("- ") {
                if let Some(colon_pos) = inner.rfind(':') {
                    let p = inner[..colon_pos].trim();
                    let h = inner[colon_pos + 1..].trim();
                    if !p.is_empty() && !h.is_empty() {
                        meta.notes.insert(p.to_string(), h.to_string());
                    }
                }
            }
            continue;
        }

        // Scalar fields
        if let Some(val) = trimmed.strip_prefix("- parent:") {
            meta.parent = val.trim().to_string();
        } else if let Some(val) = trimmed.strip_prefix("- author:") {
            meta.author = val.trim().to_string();
        } else if let Some(val) = trimmed.strip_prefix("- message:") {
            meta.message = val.trim().to_string();
        } else if let Some(val) = trimmed.strip_prefix("- at:") {
            meta.at = val.trim().to_string();
        } else if let Some(val) = trimmed.strip_prefix("- tag:") {
            meta.tag = val.trim().to_string();
        }
    }

    if meta.hash.is_empty() { None } else { Some(meta) }
}

/// Parse ALL snap entries AND branch heads from snap.md in a single pass.
/// Returns (entries, branch_heads) — H1's efficient single-pass design.
pub fn parse_all_snaps(snap_md_content: &str) -> (Vec<SnapMeta>, HashMap<String, String>) {
    let mut entries: Vec<SnapMeta> = Vec::new();
    let mut branch_heads: HashMap<String, String> = HashMap::new();
    let mut current_branch: Option<String> = None;
    let mut block_buf = String::new();
    let mut in_block = false;

    for line in snap_md_content.lines() {
        let trimmed = line.trim();

        // Branch header: ## branch_name (but not ### snap_hash)
        if let Some(branch) = trimmed.strip_prefix("## ") {
            if branch.starts_with("# ") { continue; }  // "### x" → strip_prefix("## ") → "# x"
            // Flush pending block
            if in_block && !block_buf.is_empty() {
                if let Some(meta) = parse_snap_meta(&block_buf) {
                    entries.push(meta);
                }
                block_buf.clear();
                in_block = false;
            }
            // Check for inline HEAD: "## main  - HEAD: abc"
            let (branch_name, head_part) = if let Some((b, rest)) = branch.split_once("- HEAD:") {
                (b.trim().to_string(), Some(rest.trim().to_string()))
            } else {
                (branch.trim().to_string(), None)
            };
            if let Some(h) = head_part {
                branch_heads.insert(branch_name.clone(), h);
            }
            current_branch = Some(branch_name);
            continue;
        }

        // Branch HEAD line: - HEAD: <hash>
        if let Some(val) = trimmed.strip_prefix("- HEAD:") {
            if let Some(ref branch) = current_branch {
                branch_heads.insert(branch.clone(), val.trim().to_string());
            }
            continue;
        }

        // Start of a snap entry block
        if trimmed.starts_with("### ") {
            if in_block && !block_buf.is_empty() {
                if let Some(meta) = parse_snap_meta(&block_buf) {
                    entries.push(meta);
                }
            }
            block_buf.clear();
            block_buf.push_str(trimmed);
            block_buf.push('\n');
            in_block = true;
            continue;
        }

        if in_block {
            block_buf.push_str(line);
            block_buf.push('\n');
        }
    }

    // Flush final block
    if in_block && !block_buf.is_empty() {
        if let Some(meta) = parse_snap_meta(&block_buf) {
            entries.push(meta);
        }
    }

    (entries, branch_heads)
}

/// Read the HEAD hash for a branch from snap.md content.
pub fn read_branch_head(snap_md_content: &str, branch: &str) -> Option<String> {
    let mut in_branch = false;
    for line in snap_md_content.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            // Handle inline HEAD format: "## main - HEAD: abc"
            let (branch_name, head_part) = if let Some((b, rest)) = heading.split_once("- HEAD:") {
                (b.trim(), Some(rest.trim()))
            } else {
                (heading.trim(), None)
            };
            in_branch = branch_name == branch;
            if in_branch {
                if let Some(h) = head_part { return Some(h.to_string()); }
            }
            continue;
        }
        if in_branch {
            if let Some(head) = trimmed.strip_prefix("- HEAD:") {
                return Some(head.trim().to_string());
            }
        }
    }
    None
}

/// Format a snap entry as Markdown for appending to snap.md.
pub fn format_snap_entry(meta: &SnapMeta) -> String {
    let mut out = format!("### {}\n", meta.hash);
    if !meta.parent.is_empty() {
        out.push_str(&format!("- parent: {}\n", meta.parent));
    }
    out.push_str(&format!("- author: {}\n", meta.author));
    out.push_str(&format!("- message: {}\n", meta.message));
    out.push_str(&format!("- at: {}\n", meta.at));
    if !meta.tag.is_empty() {
        out.push_str(&format!("- tag: {}\n", meta.tag));
    }
    if !meta.notes.is_empty() {
        out.push_str("#### notes\n");
        let mut paths: Vec<&String> = meta.notes.keys().collect();
        paths.sort();
        for p in paths {
            out.push_str(&format!("- {}: {}\n", p, meta.notes[p]));
        }
    }
    out
}

/// Return the ckpt filename for a year-month pair.
pub fn ckpt_file_name(year: u32, month: u32) -> String {
    format!("{:04}-{:02}.md", year, month)
}

/// Format a ckpt section:
/// ## state_hash
/// - path: /path/to/file.md
/// - at: ISO8601
/// - content: |
///     <indented file content>
pub fn format_ckpt_section(state_hash: &str, vfs_path: &str, at: &str, content: &str) -> String {
    format_ckpt_section_inner(state_hash, vfs_path, at, None, content)
}

/// Format a delta ckpt section — stores a patch from parent instead of full content.
/// ## state_hash
/// - path: /path/to/file.md
/// - at: ISO8601
/// - delta_from: parent_state_hash
/// - patch: |
///     <indented unified diff>
pub fn format_ckpt_section_delta(
    state_hash: &str, vfs_path: &str, at: &str,
    delta_from: &str, patch: &str,
) -> String {
    format_ckpt_section_inner(state_hash, vfs_path, at, Some(delta_from), patch)
}

fn format_ckpt_section_inner(
    state_hash: &str, vfs_path: &str, at: &str,
    delta_from: Option<&str>, content_or_patch: &str,
) -> String {
    let mut out = format!("## {}\n", state_hash);
    out.push_str(&format!("- path: {}\n", vfs_path));
    out.push_str(&format!("- at: {}\n", at));
    if let Some(parent) = delta_from {
        out.push_str(&format!("- delta_from: {}\n", parent));
        out.push_str("- patch: |\n");
    } else {
        out.push_str("- content: |\n");
    }
    // split('\n') preserves trailing empty segments (unlike lines())
    for segment in content_or_patch.split('\n') {
        out.push_str(&format!("    {}\n", segment));
    }
    // If content ends with \n, split('\n') produced one extra empty segment at end — correct.
    // If content doesn't end with \n, last segment is the final line — also correct.
    out
}

/// Result of parsing a ckpt section.
#[derive(Debug, Clone)]
pub enum CkptEntry {
    /// Full content (base or legacy format).
    Full { path: String, at: String, content: String },
    /// Delta — needs parent content to reconstruct.
    Delta { path: String, at: String, delta_from: String, patch: String },
}

/// Parse a ckpt section back into a CkptEntry.
/// Returns None if the section header is not found or content is malformed.
pub fn parse_ckpt_entry(ckpt_text: &str, target_hash: &str) -> Option<CkptEntry> {
    let target = format!("## {}", target_hash);
    let mut in_target = false;
    let mut path = String::new();
    let mut at = String::new();
    let mut delta_from: Option<String> = None;
    let mut in_body = false;
    let mut is_patch = false;
    let mut body_lines: Vec<String> = Vec::new();

    for line in ckpt_text.lines() {
        let trimmed = line.trim();

        // Section boundary: next ## <64-hex> (only on UNINDENTED lines)
        if in_target && !line.starts_with(' ') && trimmed.starts_with("## ") && trimmed != target {
            let candidate = trimmed.strip_prefix("## ").unwrap();
            if candidate.len() == 64 && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
                break;
            }
        }

        if !in_target {
            if trimmed == target {
                in_target = true;
            }
            continue;
        }

        if in_body {
            if let Some(unindented) = line.strip_prefix("    ") {
                body_lines.push(unindented.to_string());
            } else {
                // Non-indented line outside content — end of YAML | literal block
                in_body = false;
            }
            continue;
        }

        if let Some(val) = trimmed.strip_prefix("- path:") {
            path = val.trim().to_string();
        } else if let Some(val) = trimmed.strip_prefix("- at:") {
            at = val.trim().to_string();
        } else if let Some(val) = trimmed.strip_prefix("- delta_from:") {
            delta_from = Some(val.trim().to_string());
        } else if trimmed == "- content: |" {
            in_body = true;
        } else if trimmed == "- patch: |" {
            in_body = true;
            is_patch = true;
        }
    }

    if !in_target {
        return None;
    }
    let parsed_body = !body_lines.is_empty();
    if !parsed_body {
        // Fallback: try legacy format (raw content after ## hash, no metadata)
        return parse_ckpt_section_legacy(ckpt_text, target_hash)
            .map(|(p, a, c)| CkptEntry::Full { path: p, at: a, content: c });
    }

    // No longer strip trailing empty lines.
    // format_ckpt_section_inner uses split('\n') which preserves trailing newlines
    // as empty segments. join("\n") correctly reconstructs the original content
    // with its trailing \n intact.

    let body = body_lines.join("\n");

    if let Some(parent) = delta_from {
        Some(CkptEntry::Delta { path, at, delta_from: parent, patch: body })
    } else {
        Some(CkptEntry::Full { path, at, content: body })
    }
}

#[inline]
fn short_hash(h: &str) -> &str {
    let end = h.len().min(12);
    &h[..end]
}

/// Reconstruct full content from a CkptEntry, optionally resolving delta chains.
///
/// `resolve_entry` is called to look up the parent's CkptEntry when a delta is encountered.
/// `max_depth` prevents infinite recursion.
/// After reconstruction, `state_hash(result) == target_hash` is verified.
pub fn reconstruct_ckpt_content(
    entry: &CkptEntry,
    target_hash: &str,
    resolve_entry: &dyn Fn(&str) -> Option<CkptEntry>,
    max_depth: u32,
) -> Result<String, String> {
    reconstruct_ckpt_content_impl(entry, target_hash, resolve_entry, max_depth, &mut Vec::new())
}

fn reconstruct_ckpt_content_impl(
    entry: &CkptEntry,
    target_hash: &str,
    resolve_entry: &dyn Fn(&str) -> Option<CkptEntry>,
    max_depth: u32,
    visited: &mut Vec<String>,
) -> Result<String, String> {
    match entry {
        CkptEntry::Full { content, .. } => {
            let actual = state_hash(content);
            if actual != target_hash {
                return Err(format!(
                    "ckpt integrity check failed: expected {}, got {}",
                    short_hash(target_hash), short_hash(&actual)
                ));
            }
            Ok(content.clone())
        }
        CkptEntry::Delta { delta_from, patch, .. } => {
            if visited.contains(delta_from) {
                return Err(format!(
                    "delta chain cycle detected at {}", short_hash(delta_from)
                ));
            }
            if max_depth == 0 {
                return Err("delta chain too deep".to_string());
            }
            visited.push(delta_from.to_string());
            let parent_entry = resolve_entry(delta_from)
                .ok_or_else(|| format!("delta_from {} not found", short_hash(delta_from)))?;
            let parent_content = reconstruct_ckpt_content_impl(
                &parent_entry, delta_from, resolve_entry, max_depth - 1, visited,
            )?;
            let reconstructed = snap_apply(&parent_content, patch)?;
            let actual = state_hash(&reconstructed);
            if actual != target_hash {
                return Err(format!(
                    "delta reconstruction hash mismatch: expected {}, got {}",
                    short_hash(target_hash), short_hash(&actual)
                ));
            }
            Ok(reconstructed)
        }
    }
}

/// Convenience: parse + reconstruct in one call.
/// Kept for backward compat — returns (path, at, content).
pub fn parse_ckpt_section(ckpt_text: &str, target_hash: &str) -> Option<(String, String, String)> {
    let entry = parse_ckpt_entry(ckpt_text, target_hash)?;
    match entry {
        CkptEntry::Full { path, at, content } => Some((path, at, content)),
        CkptEntry::Delta { path, at, delta_from, patch: _ } => {
            // For direct parse (no resolve_entry callback), return the patch text
            // Caller must handle delta resolution separately
            Some((path, at, format!("<delta from {}>", short_hash(&delta_from))))
        }
    }
}

/// Legacy fallback: content stored directly after ## hash without metadata.
fn parse_ckpt_section_legacy(ckpt_text: &str, target_hash: &str) -> Option<(String, String, String)> {
    let target = format!("## {}", target_hash);
    let mut in_target = false;
    let mut body_lines: Vec<String> = Vec::new();

    for line in ckpt_text.lines() {
        let trimmed = line.trim();
        if !in_target {
            if trimmed == target {
                in_target = true;
            }
            continue;
        }
        if trimmed.starts_with("## ") {
            let candidate = trimmed.strip_prefix("## ").unwrap();
            if candidate.len() == 64 && candidate.chars().all(|c| c.is_ascii_hexdigit()) {
                break;
            }
        }
        body_lines.push(line.to_string());
    }

    if !in_target || body_lines.is_empty() {
        return None;
    }

    // Strip trailing empty lines
    while body_lines.last().map_or(false, |l| l.trim().is_empty()) {
        body_lines.pop();
    }

    let content = body_lines.join("\n");
    Some((String::new(), String::new(), content))
}


// Delta strategy — automatic base/delta decision


/// Maximum delta chain depth before forcing a base (full content) entry.
pub const MAX_DELTA_DEPTH: u32 = 20;

/// Storage decision for a ckpt entry.
pub enum StorageKind {
    /// Store full content (base).
    Base,
    /// Store delta from a parent state_hash.
    Delta { delta_from: String },
}

/// Decide whether to store a delta or a full base entry.
///
/// Rules:
/// 1. Tagged snapshots always store as base.
/// 2. If the delta is >75% of the full content size, store as base.
/// 3. If the delta chain is too deep (>= MAX_DELTA_DEPTH), store as base.
pub fn delta_strategy(
    delta_size: usize,
    full_size: usize,
    chain_depth: u32,
    is_tagged: bool,
) -> StorageKind {
    if is_tagged {
        return StorageKind::Base;
    }
    if full_size > 0 && delta_size > full_size * 3 / 4 {
        return StorageKind::Base;
    }
    if chain_depth >= MAX_DELTA_DEPTH {
        return StorageKind::Base;
    }
    // caller must fill in delta_from after computing the snap hash
    StorageKind::Delta { delta_from: String::new() }
}

/// Compute how deep a delta chain is by walking delta_from links.
/// Returns 0 for a base entry (full content), depth from there for deltas.
pub fn compute_chain_depth(
    target_hash: &str,
    resolve_entry: &dyn Fn(&str) -> Option<CkptEntry>,
    max_depth: u32,
) -> u32 {
    let mut depth = 0u32;
    let mut cur = target_hash.to_string();
    let mut visited: Vec<String> = Vec::new();
    loop {
        if depth >= max_depth { return depth; }
        match resolve_entry(&cur) {
            Some(CkptEntry::Full { .. }) => return depth,
            Some(CkptEntry::Delta { delta_from, .. }) => {
                if visited.contains(&delta_from) { return depth; } // cycle
                visited.push(cur.clone());
                cur = delta_from;
                depth += 1;
            }
            None => return depth,
        }
    }
}


// DAG traversal — 跨分支 diff


/// Find the common ancestor of two snap hashes by walking parent chains.
/// Each snap has a single parent — this is a tree, not a general DAG.
/// Returns the common ancestor hash, or None if branches have no intersection.
pub fn find_common_ancestor(
    entries: &[SnapMeta],
    id_a: &str,
    id_b: &str,
) -> Option<String> {
    // Build hash → SnapMeta index
    let index: HashMap<&str, &SnapMeta> = entries.iter()
        .map(|e| (e.hash.as_str(), e))
        .collect();

    // Walk chain A, collect ancestors in a set
    let mut ancestors_a: HashMap<String, usize> = HashMap::new();
    let mut cur = id_a;
    let mut depth = 0;
    while let Some(entry) = index.get(cur) {
        ancestors_a.insert(cur.to_string(), depth);
        depth += 1;
        if entry.parent.is_empty() { break; }
        cur = &entry.parent;
    }

    // Walk chain B, find first intersection
    cur = id_b;
    while let Some(entry) = index.get(cur) {
        if ancestors_a.contains_key(cur) {
            return Some(cur.to_string());
        }
        if entry.parent.is_empty() { break; }
        cur = &entry.parent;
    }

    None
}


// Tests


#[cfg(test)]
mod tests {
    include!("../tests/unit/snap.rs");
}
