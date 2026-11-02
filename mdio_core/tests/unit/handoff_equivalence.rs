// Handoff: snapshot -> put_snapshot_doc -> open_session/get_engine_text.
// Included from store::tests; uses only public store/session APIs.
use crate::projection::State;
use crate::store::ProjectStore;

#[test]
fn put_snapshot_doc_keeps_text_and_no_dirty() {
    let text = "# T\n\npara\n\n- a\n- b\n";
    let mut state = State::new(text);
    let snap = state.snapshot();
    let copy = snap.handoff_copy();

    let store = ProjectStore::new_with_layout("/mem", None, None).unwrap();
    let doc_id = store
        .put_snapshot_doc("genesis", "/__handoff_probe.md", text, copy, 7)
        .unwrap();
    let sess = store.open_session(doc_id).unwrap();
    assert_eq!(sess.get_engine_text(), text);
    assert!(!sess.dirty.load(std::sync::atomic::Ordering::Relaxed));
}
