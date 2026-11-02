use super::*;

// --- Known SHA256 vectors (from O1) ---

#[test]
fn test_state_hash_empty_string() {
    let h = state_hash("");
    assert_eq!(h, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
}

#[test]
fn test_state_hash_hello() {
    let h = state_hash("hello");
    assert_eq!(h, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
}

// --- Deterministism ---

#[test]
fn test_state_hash_deterministic() {
    assert_eq!(state_hash("abc"), state_hash("abc"));
    assert_ne!(state_hash("a"), state_hash("b"));
}

#[test]
fn test_snap_hash_deterministic() {
    let a = snap_hash("p", "a", "m", "2026-01-01", "/f.md:h");
    let b = snap_hash("p", "a", "m", "2026-01-01", "/f.md:h");
    assert_eq!(a, b);
    assert_eq!(a.len(), 64);
}

// --- Field sensitivity (from O3) ---

#[test]
fn test_snap_hash_field_sensitive() {
    let base = snap_hash("p", "a", "m", "t", "f");
    assert_ne!(base, snap_hash("p2", "a", "m", "t", "f"));
    assert_ne!(base, snap_hash("p", "a2", "m", "t", "f"));
    assert_ne!(base, snap_hash("p", "a", "m2", "t", "f"));
    assert_ne!(base, snap_hash("p", "a", "m", "t2", "f"));
    assert_ne!(base, snap_hash("p", "a", "m", "t", "f2"));
}

// --- Diff ---

#[test]
fn test_snap_diff_identical() {
    let d = snap_diff("a\nb\nc\n", "a\nb\nc\n");
    assert_eq!(d, "");
}

#[test]
fn test_snap_diff_changed() {
    let d = snap_diff("a\nb\nc\n", "a\nx\nc\n");
    assert!(d.contains('-') || d.contains('+'));
    assert!(d.contains("@@"));
}

// --- Parse ---

#[test]
fn test_parse_snap_meta_full() {
    let md = "\
### aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
- parent: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
- author: alice
- message: initial snap
- at: 2025-01-15T10:30:00Z
- tag: v1.0
#### notes
- /docs/readme.md: def789
- /src/main.md: ghi012
";
    let meta = parse_snap_meta(md).unwrap();
    assert_eq!(meta.hash, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert_eq!(meta.parent, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    assert_eq!(meta.author, "alice");
    assert_eq!(meta.message, "initial snap");
    assert_eq!(meta.at, "2025-01-15T10:30:00Z");
    assert_eq!(meta.tag, "v1.0");
    assert_eq!(meta.notes.get("/docs/readme.md").unwrap(), "def789");
    assert_eq!(meta.notes.get("/src/main.md").unwrap(), "ghi012");
}

#[test]
fn test_parse_snap_meta_minimal() {
    let h = "a".repeat(64);
    let md = format!("### {}\n", h);
    let meta = parse_snap_meta(&md).unwrap();
    assert_eq!(meta.hash, h);
    assert!(meta.parent.is_empty());
    assert!(meta.notes.is_empty());
}

#[test]
fn test_parse_snap_meta_no_header() {
    assert!(parse_snap_meta("just some text\n- parent: x").is_none());
}

#[test]
fn test_parse_all_snaps() {
    let content = "\
# TIME

## main
- HEAD: 2222222222222222222222222222222222222222222222222222222222222222

### 1111111111111111111111111111111111111111111111111111111111111111
- parent: genesis
- author: alice
- message: first
- at: 2026-01-01T00:00:00Z
#### notes
- /doc.md: state1

### 2222222222222222222222222222222222222222222222222222222222222222
- parent: 1111111111111111111111111111111111111111111111111111111111111111
- author: bob
- message: second
- at: 2026-01-02T00:00:00Z
#### notes
- /doc.md: state2
";
    let (entries, heads) = parse_all_snaps(content);
    assert_eq!(entries.len(), 2);
    assert_eq!(heads.get("main").unwrap(), "2222222222222222222222222222222222222222222222222222222222222222");
    assert_eq!(entries[0].hash, "1111111111111111111111111111111111111111111111111111111111111111");
    assert_eq!(entries[1].hash, "2222222222222222222222222222222222222222222222222222222222222222");
}

#[test]
fn test_read_branch_head() {
    let content = "# TIME\n\n## main\n- HEAD: abc123\n\n## feature\n- HEAD: xyz789\n";
    assert_eq!(read_branch_head(content, "main"), Some("abc123".to_string()));
    assert_eq!(read_branch_head(content, "feature"), Some("xyz789".to_string()));
    assert_eq!(read_branch_head(content, "none"), None);
}

#[test]
fn test_build_file_states_sort_order() {
    let mut notes = HashMap::new();
    notes.insert("/b.md".to_string(), "h2".to_string());
    notes.insert("/a.md".to_string(), "h1".to_string());
    let fs = build_file_states(&notes);
    assert_eq!(fs, "/a.md:h1\0/b.md:h2");
}

#[test]
fn test_ckpt_file_name() {
    assert_eq!(ckpt_file_name(2026, 5), "2026-05.md");
    assert_eq!(ckpt_file_name(2026, 12), "2026-12.md");
}

#[test]
fn test_format_ckpt_section() {
    let section = format_ckpt_section("abc123", "/notes/design.md", "2026-05-04T12:00:00Z", "# Title\n- item: value\n");
    assert!(section.contains("## abc123"));
    assert!(section.contains("- path: /notes/design.md"));
    assert!(section.contains("- at: 2026-05-04T12:00:00Z"));
    assert!(section.contains("- content: |"));
    assert!(section.contains("    # Title"));
    assert!(section.contains("    - item: value"));
}

#[test]
fn test_parse_ckpt_section_roundtrip() {
    let original = "# Title\n## Section\n- key: value";
    let section = format_ckpt_section("abc123", "/doc.md", "2026-01-01T00:00:00Z", original);
    let (path, at, content) = parse_ckpt_section(&section, "abc123").unwrap();
    assert_eq!(path, "/doc.md");
    assert_eq!(at, "2026-01-01T00:00:00Z");
    assert_eq!(content, original);
}

#[test]
fn test_parse_ckpt_section_legacy_fallback() {
    let legacy = "## abc123\nraw content\nmore lines\n";
    let (path, at, content) = parse_ckpt_section(legacy, "abc123").unwrap();
    assert!(path.is_empty());
    assert!(at.is_empty());
    assert_eq!(content, "raw content\nmore lines");
}

#[test]
fn test_find_common_ancestor() {
    let entries = vec![
        SnapMeta { hash: "a".into(), parent: "b".into(), ..Default::default() },
        SnapMeta { hash: "b".into(), parent: "c".into(), ..Default::default() },
        SnapMeta { hash: "c".into(), parent: "".into(), ..Default::default() },
        SnapMeta { hash: "x".into(), parent: "b".into(), ..Default::default() },
    ];
    assert_eq!(find_common_ancestor(&entries, "a", "x"), Some("b".into()));
    assert_eq!(find_common_ancestor(&entries, "a", "b"), Some("b".into()));
    let entries2 = vec![
        SnapMeta { hash: "a".into(), parent: "".into(), ..Default::default() },
        SnapMeta { hash: "x".into(), parent: "".into(), ..Default::default() },
    ];
    assert_eq!(find_common_ancestor(&entries2, "a", "x"), None);
}

#[test]
fn test_find_common_ancestor_first_snap() {
    let entries = vec![
        SnapMeta { hash: "genesis".into(), parent: "".into(), ..Default::default() },
        SnapMeta { hash: "child".into(), parent: "genesis".into(), ..Default::default() },
    ];
    assert_eq!(find_common_ancestor(&entries, "genesis", "child"), Some("genesis".into()));
}

// --- Diff apply roundtrip ---

#[test]
fn test_snap_apply_roundtrip() {
    let cases = vec![
        ("a\nb\nc\n", "a\nb\nc\n"),
        ("a\nb\nc\n", "a\nx\nc\n"),
        ("line 1\nline 2\n", "line 1\nline 2\nline 3\n"),
        ("", "hello\n"),
        ("hello\n", ""),
    ];
    for (before, after) in cases {
        let diff = snap_diff(before, after);
        let reconstructed = snap_apply(before, &diff).unwrap();
        assert_eq!(reconstructed, after, "roundtrip failed for {:?} -> {:?}", before, after);
    }
}

#[test]
fn test_snap_apply_empty_diff() {
    let result = snap_apply("hello", "").unwrap();
    assert_eq!(result, "hello");
}

#[test]
fn test_snap_diff_apply_markdown() {
    let before = "# Title\n\n- key: val\n\ntext\n";
    let after = "# Title\n\n- key: newval\n- k2: v2\n\nmodified\n";
    let diff = snap_diff(before, after);
    let reconstructed = snap_apply(before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}

// --- Delta ckpt format ---

#[test]
fn test_format_ckpt_section_delta() {
    let section = format_ckpt_section_delta(
        "def456", "/doc.md", "2026-01-01T00:00:00Z",
        "abc123", "@@ -1,1 +1,1 @@\n-old\n+new\n",
    );
    assert!(section.contains("## def456"));
    assert!(section.contains("- delta_from: abc123"));
    assert!(section.contains("- patch: |"));
}

#[test]
fn test_parse_ckpt_entry_full() {
    let section = format_ckpt_section("abc123", "/doc.md", "2026-01-01T00:00:00Z", "content here");
    let entry = parse_ckpt_entry(&section, "abc123").unwrap();
    match entry {
        CkptEntry::Full { path, content, .. } => {
            assert_eq!(path, "/doc.md");
            assert_eq!(content, "content here");
        }
        _ => panic!("expected Full"),
    }
}

#[test]
fn test_parse_ckpt_entry_delta() {
    let section = format_ckpt_section_delta(
        "def456", "/doc.md", "2026-01-01T00:00:00Z",
        "abc123", "@@ -1,1 +1,1 @@\n-old\n+new\n",
    );
    let entry = parse_ckpt_entry(&section, "def456").unwrap();
    match entry {
        CkptEntry::Delta { path, delta_from, patch, .. } => {
            assert_eq!(path, "/doc.md");
            assert_eq!(delta_from, "abc123");
            assert!(patch.contains("@@"));
        }
        _ => panic!("expected Delta"),
    }
}

#[test]
fn test_ckpt_section_roundtrip_preserves_trailing_newline() {
    let content = "line 1\nline 2\n";
    let section = format_ckpt_section("abc123", "/f.md", "2026-01-01T00:00:00Z", content);
    let result = parse_ckpt_entry(&section, "abc123").unwrap();
    match result {
        CkptEntry::Full { content: c, .. } => assert_eq!(c, content),
        _ => panic!("expected Full"),
    }
}

// --- Hash verification ---

#[test]
fn test_reconstruct_ckpt_full_verifies_hash() {
    let content = "hello world\n";
    let h = state_hash(content);
    let entry = CkptEntry::Full { path: "/f.md".into(), at: "t".into(), content: content.into() };
    let result = reconstruct_ckpt_content(&entry, &h, &|_| None, 10).unwrap();
    assert_eq!(result, content);
}

#[test]
fn test_reconstruct_ckpt_full_detects_corruption() {
    let content = "hello world\n";
    let h = state_hash(content);
    let entry = CkptEntry::Full { path: "/f.md".into(), at: "t".into(), content: "corrupted".into() };
    let result = reconstruct_ckpt_content(&entry, &h, &|_| None, 10);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("integrity"));
}

#[test]
fn test_reconstruct_ckpt_delta_chain() {
    let parent = "line 1\nline 2\nline 3\n";
    let child = "line 1\nCHANGED\nline 3\n";
    let parent_h = state_hash(parent);
    let child_h = state_hash(child);
    let diff = snap_diff(parent, child);

    let parent_entry = CkptEntry::Full { path: "/f.md".into(), at: "t1".into(), content: parent.into() };
    let child_entry = CkptEntry::Delta { path: "/f.md".into(), at: "t2".into(), delta_from: parent_h.clone(), patch: diff };

    let resolve = |h: &str| -> Option<CkptEntry> {
        if h == parent_h { Some(parent_entry.clone()) } else { None }
    };

    let result = reconstruct_ckpt_content(&child_entry, &child_h, &resolve, 10).unwrap();
    assert_eq!(result, child);
}

#[test]
fn test_reconstruct_ckpt_delta_wrong_hash() {
    let parent = "line 1\nline 2\n";
    let child = "line 1\nCHANGED\n";
    let parent_h = state_hash(parent);
    let diff = snap_diff(parent, child);
    let wrong_h = state_hash("something else");

    let parent_entry = CkptEntry::Full { path: "/f.md".into(), at: "t1".into(), content: parent.into() };
    let child_entry = CkptEntry::Delta { path: "/f.md".into(), at: "t2".into(), delta_from: parent_h.clone(), patch: diff };

    let resolve = |h: &str| -> Option<CkptEntry> {
        if h == parent_h { Some(parent_entry.clone()) } else { None }
    };

    let result = reconstruct_ckpt_content(&child_entry, &wrong_h, &resolve, 10);
    assert!(result.is_err());
}

#[test]
fn test_reconstruct_ckpt_max_depth() {
    let entry = CkptEntry::Delta {
        path: "/f.md".into(), at: "t".into(),
        delta_from: "p".into(), patch: "".into(),
    };
    let result = reconstruct_ckpt_content(&entry, "any", &|_| None, 0);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("too deep"));
}

// --- Delta strategy ---

#[test]
fn test_delta_strategy_tag_forces_base() {
    match delta_strategy(10, 100, 0, true) {
        StorageKind::Base => {}
        _ => panic!("tagged should be base"),
    }
}

#[test]
fn test_delta_strategy_large_diff_forces_base() {
    match delta_strategy(90, 100, 0, false) {
        StorageKind::Base => {}
        _ => panic!("large diff should be base"),
    }
}

#[test]
fn test_delta_strategy_deep_chain_forces_base() {
    match delta_strategy(10, 100, MAX_DELTA_DEPTH, false) {
        StorageKind::Base => {}
        _ => panic!("deep chain should be base"),
    }
}

#[test]
fn test_delta_strategy_small_diff_shallow_chain_is_delta() {
    match delta_strategy(10, 100, 5, false) {
        StorageKind::Delta { .. } => {}
        _ => panic!("small diff, shallow chain should be delta"),
    }
}

#[test]
fn test_delta_strategy_empty_file() {
    match delta_strategy(0, 0, 0, false) {
        StorageKind::Base => {}
        _ => {}
    }
}

#[test]
fn test_delta_strategy_at_max_depth_minus_one() {
    match delta_strategy(10, 100, MAX_DELTA_DEPTH - 1, false) {
        StorageKind::Delta { .. } => {}
        _ => panic!("should allow delta at depth {}-1", MAX_DELTA_DEPTH),
    }
}

// --- Chain depth ---

#[test]
fn test_compute_chain_depth_full_is_zero() {
    let full = CkptEntry::Full { path: "/f.md".into(), at: "t".into(), content: "hi".into() };
    let resolve = |h: &str| if h == "abc" { Some(full.clone()) } else { None };
    assert_eq!(compute_chain_depth("abc", &resolve, 10), 0);
}

#[test]
fn test_compute_chain_depth_three_levels() {
    let base = CkptEntry::Full { path: "/f.md".into(), at: "t".into(), content: "base".into() };
    let d1 = CkptEntry::Delta { path: "/f.md".into(), at: "t".into(), delta_from: "base".into(), patch: "p1".into() };
    let d2 = CkptEntry::Delta { path: "/f.md".into(), at: "t".into(), delta_from: "d1".into(), patch: "p2".into() };
    let resolve = |h: &str| match h {
        "base" => Some(base.clone()), "d1" => Some(d1.clone()), "d2" => Some(d2.clone()), _ => None,
    };
    assert_eq!(compute_chain_depth("d2", &resolve, 10), 2);
}

#[test]
fn test_compute_chain_depth_cycle_detected() {
    let d1 = CkptEntry::Delta { path: "/f.md".into(), at: "t".into(), delta_from: "d2".into(), patch: "p1".into() };
    let d2 = CkptEntry::Delta { path: "/f.md".into(), at: "t".into(), delta_from: "d1".into(), patch: "p2".into() };
    let resolve = |h: &str| {
        match h { "d1" => Some(d1.clone()), "d2" => Some(d2.clone()), _ => None }
    };
    let depth = compute_chain_depth("d1", &resolve, 100);
    assert!(depth < 100);
}

#[test]
fn test_compute_chain_depth_not_found() {
    let resolve = |_: &str| -> Option<CkptEntry> { None };
    assert_eq!(compute_chain_depth("missing", &resolve, 10), 0);
}

// --- Diff apply edge cases ---

#[test]
fn test_snap_apply_multi_hunk() {
    let before = "a\nb\nc\nd\ne\nf\ng\nh\n";
    let after = "a\nB\nc\nD\ne\nf\nG\nH\n";
    let diff = snap_diff(before, after);
    let reconstructed = snap_apply(before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}

#[test]
fn test_snap_apply_indented() {
    let before = "  keep\n  remove\n  keep\n";
    let after = "  keep\n  insert\n  keep\n";
    let diff = snap_diff(before, after);
    let reconstructed = snap_apply(before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}

#[test]
fn test_snap_apply_unicode() {
    let before = "ä½ å¥½\nä¸çŒ\n";
    let after = "ä½ å¥½\nä¿®æ”¹çš„ä¸–ç•Œ\n";
    let diff = snap_diff(before, after);
    let reconstructed = snap_apply(before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}

#[test]
fn test_snap_apply_large_file() {
    let before = "line\n".repeat(100);
    let mut after = "line\n".repeat(100);
    after.replace_range(50 * 5..50 * 5 + 5, "CHANGED\n");
    let diff = snap_diff(&before, &after);
    let reconstructed = snap_apply(&before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}

#[test]
fn test_snap_apply_empty_to_empty() {
    let diff = snap_diff("", "");
    assert_eq!(diff, "");
    assert_eq!(snap_apply("", &diff).unwrap(), "");
}

// --- Delta ckpt format edge cases ---

#[test]
fn test_format_ckpt_section_delta_empty_patch() {
    let section = format_ckpt_section_delta(
        "abc", "/f.md", "2026-01-01T00:00:00Z", "parent", "",
    );
    assert!(section.contains("## abc"));
    assert!(section.contains("- delta_from: parent"));
}

#[test]
fn test_parse_ckpt_entry_not_found() {
    let ckpt = "## abc\n- path: /f.md\n- content: |\n    text\n";
    assert!(parse_ckpt_entry(ckpt, "nonexistent").is_none());
}

#[test]
fn test_parse_ckpt_entry_empty_ckpt() {
    assert!(parse_ckpt_entry("", "abc").is_none());
}

#[test]
fn test_ckpt_section_roundtrip_no_trailing_newline() {
    let content = "line 1\nline 2";
    let section = format_ckpt_section("abc", "/f.md", "2026-01-01T00:00:00Z", content);
    let result = parse_ckpt_entry(&section, "abc").unwrap();
    match result {
        CkptEntry::Full { content: c, .. } => assert_eq!(c, content),
        _ => panic!("expected Full"),
    }
}

#[test]
fn test_ckpt_section_roundtrip_single_line() {
    let content = "just one line";
    let section = format_ckpt_section("abc", "/f.md", "2026-01-01T00:00:00Z", content);
    let result = parse_ckpt_entry(&section, "abc").unwrap();
    match result {
        CkptEntry::Full { content: c, .. } => assert_eq!(c, content),
        _ => panic!("expected Full"),
    }
}

#[test]
fn test_ckpt_section_roundtrip_multi_newline_end() {
    let content = "line 1\nline 2\n\n";
    let section = format_ckpt_section("abc", "/f.md", "2026-01-01T00:00:00Z", content);
    let result = parse_ckpt_entry(&section, "abc").unwrap();
    match result {
        CkptEntry::Full { content: c, .. } => assert_eq!(c, content),
        _ => panic!("expected Full"),
    }
}

// --- Delta reconstruction deep tests ---

#[test]
fn test_reconstruct_delta_three_levels() {
    let v1 = "line 1\nline 2\nline 3\n";
    let v2 = "line 1\nCHANGED\nline 3\n";
    let v3 = "line 1\nCHANGED\nline 3\nNEW\n";
    let v4 = "PREFIX\nline 1\nCHANGED\nline 3\nNEW\n";

    let h1 = state_hash(v1);
    let h2 = state_hash(v2);
    let h3 = state_hash(v3);
    let h4 = state_hash(v4);

    let diff12 = snap_diff(v1, v2);
    let diff23 = snap_diff(v2, v3);
    let diff34 = snap_diff(v3, v4);

    let e1 = CkptEntry::Full { path: "/f.md".into(), at: "t1".into(), content: v1.into() };
    let e2 = CkptEntry::Delta { path: "/f.md".into(), at: "t2".into(), delta_from: h1.clone(), patch: diff12 };
    let e3 = CkptEntry::Delta { path: "/f.md".into(), at: "t3".into(), delta_from: h2.clone(), patch: diff23 };
    let e4 = CkptEntry::Delta { path: "/f.md".into(), at: "t4".into(), delta_from: h3.clone(), patch: diff34 };

    let resolve = |h: &str| match h {
        hh if hh == h1 => Some(e1.clone()),
        hh if hh == h2 => Some(e2.clone()),
        hh if hh == h3 => Some(e3.clone()),
        hh if hh == h4 => Some(e4.clone()),
        _ => None,
    };

    assert_eq!(reconstruct_ckpt_content(&e4, &h4, &resolve, 10).unwrap(), v4);
    assert_eq!(reconstruct_ckpt_content(&e3, &h3, &resolve, 10).unwrap(), v3);
    assert_eq!(reconstruct_ckpt_content(&e2, &h2, &resolve, 10).unwrap(), v2);
}

#[test]
fn test_reconstruct_cycle_detection() {
    let bad_entry = CkptEntry::Delta {
        path: "/f.md".into(), at: "t".into(),
        delta_from: "cycle_target".into(), patch: "p".into(),
    };
    let cycle_target = CkptEntry::Delta {
        path: "/f.md".into(), at: "t".into(),
        delta_from: "bad_entry".into(), patch: "p2".into(),
    };
    let resolve = |h: &str| match h {
        "bad_entry" => Some(bad_entry.clone()),
        "cycle_target" => Some(cycle_target.clone()),
        _ => None,
    };
    let result = reconstruct_ckpt_content(&bad_entry, "bad_entry_hash", &resolve, 10);
    assert!(result.is_err());
}

#[test]
fn test_reconstruct_delta_broken_patch() {
    let parent = "line 1\nline 2\n";
    let parent_h = state_hash(parent);
    let parent_entry = CkptEntry::Full { path: "/f.md".into(), at: "t".into(), content: parent.into() };
    let corrupted_entry = CkptEntry::Delta {
        path: "/f.md".into(), at: "t".into(),
        delta_from: parent_h.clone(),
        patch: "this is not a valid unified diff".into(),
    };
    let resolve = |h: &str| if h == parent_h { Some(parent_entry.clone()) } else { None };
    let result = reconstruct_ckpt_content(&corrupted_entry, "child_hash", &resolve, 10);
    assert!(result.is_err());
}

#[test]
fn test_reconstruct_delta_parent_not_found() {
    let entry = CkptEntry::Delta {
        path: "/f.md".into(), at: "t".into(),
        delta_from: "missing_parent".into(), patch: "@@ -1,1 +1,1 @@\n-a\n+b\n".into(),
    };
    let resolve = |_: &str| -> Option<CkptEntry> { None };
    let result = reconstruct_ckpt_content(&entry, "child_hash", &resolve, 10);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

// --- Parse ckpt legacy format roundtrip ---

#[test]
fn test_parse_ckpt_entry_legacy_returns_full() {
    let legacy = "## abc123\nraw content\nmore lines\n";
    let entry = parse_ckpt_entry(legacy, "abc123").unwrap();
    match entry {
        CkptEntry::Full { content, .. } => assert_eq!(content, "raw content\nmore lines"),
        _ => panic!("legacy should be Full"),
    }
}

// --- Snap diff edge cases ---

#[test]
fn test_snap_diff_single_line_to_multi() {
    let before = "single\n";
    let after = "line 1\nline 2\n";
    let diff = snap_diff(before, after);
    assert!(!diff.is_empty());
    let reconstructed = snap_apply(before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}

#[test]
fn test_snap_diff_delete_all() {
    let before = "many\nlines\nhere\n";
    let after = "";
    let diff = snap_diff(before, after);
    assert!(!diff.is_empty());
    let reconstructed = snap_apply(before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}

#[test]
fn test_snap_diff_only_whitespace_change() {
    let before = "line1\nline2\n";
    let after = "line1 \nline2\n";
    let diff = snap_diff(before, after);
    let reconstructed = snap_apply(before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}

#[test]
fn test_snap_diff_same_start_end() {
    let before = "start\nline 2\nline 3\nline 4\nend\n";
    let after = "start\nCHANGED A\nCHANGED B\nline 4\nend\n";
    let diff = snap_diff(before, after);
    let reconstructed = snap_apply(before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}

#[test]
fn test_snap_diff_frontmatter() {
    let before = "---\ntitle: old\n---\n\ntext\n";
    let after = "---\ntitle: new\ntags: added\n---\n\nmodified text\n";
    let diff = snap_diff(before, after);
    let reconstructed = snap_apply(before, &diff).unwrap();
    assert_eq!(reconstructed, after);
}
