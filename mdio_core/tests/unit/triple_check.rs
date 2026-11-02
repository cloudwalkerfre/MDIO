#[test]
fn triple_nested() {
    let doc = "- outer\n  1. inner\n     - deep\n";
    let mut state = State::new(doc);
    let snap = state.snapshot();
    let mut fold = crate::infer::RecordFold::new(&snap.frame_log.entries, &snap.lines);
    crate::infer::walk_zone(&snap.frame_log.entries, (0, snap.line_len() as u32), &mut fold);
    let (recs, arena) = fold.finish();
    let blocks = crate::infer::build_block_records(&snap.frame_log.entries, &snap.lines, &snap.rope, &recs, &arena, (0, snap.line_len() as u32));
    let spans = crate::infer::LineView::new(&recs, &blocks, &snap.frame_log.entries, &snap.lines, &snap.rope, (0, snap.line_len() as u32), &arena);
    for li in 0..snap.line_len() {
        let d = spans.close_depth_at(li);
        let nts: Vec<String> = (0..d).map(|di| format!("{:?}", spans.node_type_at_depth(li, di))).collect();

    }

}
