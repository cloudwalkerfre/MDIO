//! 确定性 ripple 等价 fuzz：edit + ripple 必须与全量重扫逐字节一致
//! （lines + FrameLog），FL 分叉时再对 AST 全量比对。
//! 固定种子：覆盖 anchor 分叉、HtmlElement tail 平移、多行替换 splice 守卫三类形态。
use mdio_core::projection::State;

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
        POOL[(self.next() as usize) % POOL.len()]
    }
}

const POOL: &[&str] = &[
    "a", "para", "cont", "  para", "    code",
    "- a", "  - b", "   - c", "    - d", "     - e", "      - f", "       - g",
    "1. a", " 1. b", "  1. c", "   1. d",
    "* a", "+ a", "- [ ] t", "- [x] d",
    "> q", "> - x", ">", "> 1. y",
    "# h", "## h", "### h", "---", "====", "```", "```x", "~~~",
    "<div>", "</div>", "<table>", "</table>", "<x y=\"z\">",
    "| a | b |", "|---|---|", "| c | d |",
    "", "  ", "    ",
];

fn state_diff(a: &State, b: &State) -> Vec<String> {
    let mut out = Vec::new();
    for i in 0..a.lines.len().max(b.lines.len()) {
        let x = a.lines.get(i).map(|p| p.0);
        let y = b.lines.get(i).map(|p| p.0);
        if x != y {
            out.push(format!("line[{i}] ripple={x:?} rebuild={y:?}"));
        }
    }
    // 承诺列逐行比对。
    for i in 0..a.prev_col.len().max(b.prev_col.len()) {
        let x = a.prev_col.get(i).map(|p| p.0);
        let y = b.prev_col.get(i).map(|p| p.0);
        if x != y {
            out.push(format!("prev_col[{i}] ripple={x:?} rebuild={y:?}"));
        }
    }
    let fa = &a.frame_log.entries;
    let fb = &b.frame_log.entries;
    for i in 0..fa.len().max(fb.len()) {
        let x = fa.get(i).copied();
        let y = fb.get(i).copied();
        if x != y {
            out.push(format!("FL[{i}] ripple={x:?} rebuild={y:?}"));
        }
    }
    out
}

#[test]
fn ripple_equals_full_rescan_deterministic_fuzz() {
    let mut rng = Rng(0x9E3779B97F4A7C15);
    for trial in 0..60_000u64 {
        let n = 1 + (rng.next() as usize) % 12;
        let mut doc = String::new();
        for _ in 0..n {
            doc.push_str(rng.tok());
            doc.push('\n');
        }
        let doc_len = doc.len();
        let start = (rng.next() as usize) % doc_len;
        let span = 1 + (rng.next() as usize) % 8;
        let end = (start + span).min(doc_len);
        let mut text = String::new();
        let parts = 1 + (rng.next() as usize) % 3;
        for _ in 0..parts {
            text.push_str(rng.tok());
            text.push('\n');
        }
        let mut ripple = State::new(&doc);
        ripple.edit(start, end, &text);
        if (rng.next() & 3) == 0 {
            let d2 = ripple.rope.to_string();
            if d2.is_empty() {
                continue;
            }
            let s2 = (rng.next() as usize) % d2.len();
            let e2 = (s2 + 1 + (rng.next() as usize) % 6).min(d2.len());
            ripple.edit(s2, e2, rng.tok());
        }
        ripple.ensure_projected();
        let mut rebuild = State::new(&ripple.rope.to_string());
        let diffs = state_diff(&ripple, &rebuild);
        if !diffs.is_empty() {
            let ra = ripple.snapshot().to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
            let rb = rebuild.snapshot().to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf);
            panic!(
                "trial {trial}: ripple != rebuild\ndoc={doc:?} start={start} end={end} text={text:?}\n{diffs:?}\nripple AST={ra}\nrebuild AST={rb}"
            );
        }
    }
}
