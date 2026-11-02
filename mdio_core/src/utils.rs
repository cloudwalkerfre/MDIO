//! Utility functions for benchmarking, testing, and path manipulation.

use std::time::{Duration, Instant};
use std::fs;

// ── Benchmark / testing ──

/// Load a test file from disk
pub fn load_test_file(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|_| "".to_string())
}

/// Measure execution time of a function
///
/// # Arguments
/// * `f` - A function to measure
///
/// # Returns
/// A tuple of (Duration, Result)
pub fn measure<F, R>(f: F) -> (Duration, R)
where
    F: FnOnce() -> R,
{
    let start = Instant::now();
    let r = f();
    (start.elapsed(), r)
}

/// Format duration for display
pub fn format_duration(d: Duration) -> String {
    if d.as_secs() > 0 {
        format!("{:.3}s", d.as_secs_f64())
    } else if d.as_millis() > 0 {
        format!("{:.3}ms", d.as_micros() as f64 / 1000.0)
    } else if d.as_micros() > 0 {
        format!("{:.3}μs", d.as_micros() as f64)
    } else {
        format!("{}ns", d.as_nanos())
    }
}

// ── Path helpers ──

/// Normalize a virtual path: remove `..` / `.` segments, ensure `/` prefix.
pub fn normalize_vpath(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && p != &".").collect();
    let mut result = String::new();
    for part in parts {
        if part == ".." {
            if result != "/" {
                let last = result.rfind('/').unwrap_or(0);
                result.truncate(last);
                if result.is_empty() { result.push('/'); }
            }
        } else {
            if !result.is_empty() && result != "/" { result.push('/'); }
            result.push_str(part);
        }
    }
    if result.is_empty() { result.push('/'); }
    if !result.starts_with('/') { result.insert(0, '/'); }
    result
}

/// Check whether `child` is a true subpath of `parent`.
/// Prevents e.g. `/data_backup` from being misidentified as a child of `/data`.
pub fn is_subpath(parent: &str, child: &str) -> bool {
    let p = parent.trim_end_matches('/');
    let c = child.trim_end_matches('/');
    if p == c { return true; }
    if c.starts_with(p) {
        if c.as_bytes().get(p.len()) == Some(&b'/') { return true; }
    }
    false
}

/// Strip `parent_dir` from `child_path`, returning the remainder.
/// Returns `Some("")` when the two paths are equal.
pub fn get_relative_subpath<'a>(parent_dir: &str, child_path: &'a str) -> Option<&'a str> {
    let p = parent_dir.trim_end_matches('/');
    let c = child_path.trim_end_matches('/');
    if p == c { return Some(""); }
    if c.starts_with(p) {
        let p_len = p.len();
        if p_len == 0 { return Some(c.trim_start_matches('/')); }
        if c.as_bytes().get(p_len) == Some(&b'/') { return Some(&c[p_len + 1..]); }
    }
    None
}

#[cfg(test)]
mod tests {
    include!("../tests/unit/utils.rs");
}
