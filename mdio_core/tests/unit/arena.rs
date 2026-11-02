// FileArena / remove_by_wt_uid 测试

use super::super::FileArena;

#[test]
fn remove_by_wt_uid_clears_all() {
    let arena = FileArena::new();
    let id1 = arena.get_or_alloc("p1".into(), "genesis", "/a.md".into());
    let id2 = arena.get_or_alloc("p2".into(), "genesis", "/b.md".into());
    let id3 = arena.get_or_alloc("p3".into(), "other", "/c.md".into());
    assert!(arena.get_meta(id1).is_some());
    assert!(arena.get_meta(id2).is_some());
    assert!(arena.get_meta(id3).is_some());

    let removed = arena.remove_by_wt_uid("genesis");
    assert_eq!(removed.len(), 2);
    assert!(removed.contains(&id1));
    assert!(removed.contains(&id2));
    assert!(arena.get_meta(id1).is_none());
    assert!(arena.get_meta(id2).is_none());
    assert!(arena.get_meta(id3).is_some());
}

#[test]
fn remove_by_wt_uid_other_untouched() {
    let arena = FileArena::new();
    let id1 = arena.get_or_alloc("p1".into(), "one", "/a.md".into());
    let id2 = arena.get_or_alloc("p2".into(), "two", "/b.md".into());
    let removed = arena.remove_by_wt_uid("one");
    assert_eq!(removed, vec![id1]);
    assert!(arena.get_meta(id2).is_some());
}

#[test]
fn remove_by_wt_uid_empty_when_no_match() {
    let arena = FileArena::new();
    arena.get_or_alloc("p1".into(), "genesis", "/a.md".into());
    let removed = arena.remove_by_wt_uid("nonexistent");
    assert!(removed.is_empty());
}
