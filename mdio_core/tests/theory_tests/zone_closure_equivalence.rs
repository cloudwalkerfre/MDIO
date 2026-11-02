//! zone closure 独立实现守卫。
//!
//! 本测试的“扫描参考实现”完全本地独立编写，不调用 incr 生产函数，
//! 只与 infer 记录版 oracle 对拍。
//!
//! 覆盖：空 zone、EOF 空 zone、容器内锚定、shell PopPush 同 kind 链、
//! 跨 kind PopPush、未关闭容器、fence/table/BQ 组合。

use mdio_core::block::{entry_frame, entry_is_body_mut, entry_is_pop, entry_line, BlockType, FrameKind};
use mdio_core::effect::{end_for_rule, is_poppush_kindonly};
use mdio_core::infer::{container_log_end, partition};
use mdio_core::projection::{Snapshot, State};

/// 独立实现：单次前向括号匹配。
/// 注意：本函数刻意不复用 mdio_core::incr / infer::expand_zone_to_containers。
fn expand_zone_scan_reference(snap: &Snapshot, cs: usize, ce: usize) -> (usize, usize) {
    let entries = &snap.frame_log.entries;
    let lines = &snap.lines;
    let n = snap.line_len();
    let mut cs = cs.min(n);
    let mut ce = ce.min(n);

    // 先锚定 cs 到容器外（depth 0），否则 cs 落在容器内时，
    // 扫描区间不含容器的 Push，ce 无法扩到容器尾。
    cs = snap.snap_to_depth0(cs);

    let ei_start = partition(entries, cs as u32);
    let ei_end = partition(entries, ce as u32);
    let mut stack: Vec<(FrameKind, usize)> = Vec::new();
    let mut min_push = usize::MAX;
    let mut i = ei_start;
    while i < ei_end.min(entries.len()) {
        let e = entries[i];
        if entry_is_body_mut(e) {
            i += 1;
            continue;
        }
        let li = entry_line(e) as usize;
        let kind = entry_frame(e).kind();
        if entry_is_pop(e) {
            let is_poppush = is_poppush_kindonly(e, entries.get(i + 1).copied());
            if is_poppush {
                // 容器延续（PopPush 不关闭）：弹栈更新锚，
                // Push 半部复用旧 start 入栈。
                if let Some((pk, pl)) = stack.pop() {
                    if pk == kind {
                        min_push = min_push.min(pl);
                        stack.push((pk, pl));
                    }
                }
                i += 2;
                continue;
            }
            // 真实 Pop：弹栈结算 end。
            if let Some((pk, pl)) = stack.pop() {
                if pk == kind {
                    min_push = min_push.min(pl);
                    let pop_pl = lines.get(li).map(|p| p.node_type()).unwrap_or(BlockType::Blank);
                    let end = end_for_rule(kind, li, pop_pl);
                    if end > ce {
                        ce = end.min(n);
                    }
                }
            }
            i += 1;
        } else {
            stack.push((kind, li));
            i += 1;
        }
    }

    // 未关闭容器：用 container_log_end 补结算。
    for &(kind, pl) in &stack {
        if let Some(end) = container_log_end(entries, lines, pl as u32, kind) {
            if end > ce {
                ce = end.min(n);
            }
        } else {
            ce = n;
        }
    }

    if min_push != usize::MAX && min_push < cs {
        cs = min_push;
    }
    cs = snap.snap_to_depth0(cs);
    (cs, ce)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn tok(&mut self) -> &'static str {
        TOKENS[(self.next() as usize) % TOKENS.len()]
    }
}

const TOKENS: &[&str] = &[
    "a", "para", "  para", "    code",
    "- a", "  - b", "   - c", "    - d", "     - e", "      - f",
    "1. a", " 1. b", "  1. c", "   1. d",
    "* a", "+ a", "- [ ] t", "- [x] d",
    "> q", "> - x", ">", "> 1. y",
    "# h", "## h", "### h", "---", "====", "```", "```x", "~~~",
    "<div>", "</div>", "<x y=\"z\">",
    "| a | b |", "|---|---|", "| c | d |", "| e |", "|---|",
    "[a]: /url", "  title",
    "", "  ", "    ",
];

#[test]
fn expand_zone_containers_record_equals_scan() {
    let mut rng = Rng(0x5EED5EED);
    let mut zones = 0u64;
    for _ in 0..20_000 {
        let n_lines = 1 + (rng.next() as usize) % 10;
        let mut doc = String::new();
        for _ in 0..n_lines {
            doc.push_str(rng.tok());
            doc.push('\n');
        }
        let mut st = State::new(&doc);
        let snap = st.snapshot();
        let n = snap.line_len();
        let entries = &snap.frame_log.entries;
        let lines = &snap.lines;

        let mut fold = mdio_core::infer::RecordFold::new(entries, lines);
        mdio_core::infer::walk_zone(entries, (0, n as u32), &mut fold);
        let (recs, arena) = fold.finish();
        let depths = snap.open_depth.clone();

        for cs in 0..=n {
            for ce in cs..=n {
                let scan = expand_zone_scan_reference(&snap, cs, ce);
                let record = mdio_core::infer::expand_zone_to_containers(&recs, &arena, &depths, n, cs, ce);
                let fl = mdio_core::infer::expand_zone_to_containers_fl(entries, lines, &depths, n, cs, ce);
                zones += 1;
                assert_eq!(
                    scan, record,
                    "scan vs record divergence\ndoc={doc:?}\ncs={cs} ce={ce}\nscan={scan:?}\nrecord={record:?}"
                );
                assert_eq!(
                    scan, fl,
                    "scan vs fl divergence\ndoc={doc:?}\ncs={cs} ce={ce}\nscan={scan:?}\nfl={fl:?}"
                );
            }
        }
    }

}
