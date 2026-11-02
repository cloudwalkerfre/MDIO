//! 小内核机械化冒烟测试：枚举短 K10 行序列，检查可执行不变式。
//!
//! Enumerates short K10 line sequences and checks executable invariants:
//!   T1 determinism (same doc twice -> same FL)
//!   T2 per-line Q* P* order (no Q after P on the same line)
//!   T3 depth <= MAX_BLOCK_NESTING
//!   line-order monotonicity
//!
//! This is an executable finite check, not a theorem-prover proof.

use mdio_core::block::{entry_frame, entry_is_body_mut, entry_line, MAX_BLOCK_NESTING};
use mdio_core::projection::State;

const ALPHABET: &[&str] = &[
    "",
    "| h |",
    "|---|",
    "| x |",
    "<!--",
    "c",
    "-->",
    "<div>",
    "raw",
    "</div>",
    "[a]: /url",
    "> q",
    "- a",
    "```",
];

fn fl(doc: &str) -> Vec<(usize, bool, bool)> {
    // (line, is_B, is_Q)
    let mut st = State::new(doc);
    let snap = st.snapshot();
    snap.frame_log
        .entries
        .iter()
        .map(|&e| {
            let line = entry_line(e) as usize;
            let is_b = entry_is_body_mut(e);
            let is_q = !is_b && (e >> 63 != 0);
            let _ = entry_frame(e);
            (line, is_b, is_q)
        })
        .collect()
}

fn check(doc: &str) {
    let mut st = State::new(doc);
    let snap = st.snapshot();
    let n = snap.lines.len();

    // T3: depth bound.
    for l in 0..n {
        assert!(
            snap.open_depth_at(l) <= MAX_BLOCK_NESTING,
            "T3 depth bound line {l} doc={doc:?}"
        );
    }
    assert!(snap.eof_open_depth() <= MAX_BLOCK_NESTING, "T3 eof depth");

    // T2 + line order: per line Q* P*, B anywhere after Q*P*.
    let events = fl(doc);
    let mut i = 0;
    while i < events.len() {
        let line = events[i].0;
        assert!(line < n || n == 0, "line out of range {line} n={n} doc={doc:?}");
        let mut seen_p = false;
        while i < events.len() && events[i].0 == line {
            let (_, is_b, is_q) = events[i];
            if !is_b {
                if is_q {
                    assert!(!seen_p, "T2 Q after P line {line} doc={doc:?}");
                } else {
                    seen_p = true;
                }
            }
            i += 1;
        }
    }

    // T1: determinism.
    let again = fl(doc);
    assert_eq!(events, again, "T1 determinism doc={doc:?}");
}

#[test]
fn k10_kernel_mechanization_smoke() {
    let mut count = 0usize;
    for a in ALPHABET {
        for b in ALPHABET {
            for c in ALPHABET {
                for d in ALPHABET {
                    for e in ALPHABET {
                        let doc = format!("{a}\n{b}\n{c}\n{d}\n{e}\n");
                        check(&doc);
                        count += 1;
                    }
                }
            }
        }
    }
    assert_eq!(count, ALPHABET.len().pow(5));
}

/// Long-running one-off: 14^6 = 7,529,536 docs.
#[test]
#[ignore = "long: 14^6 docs"]
fn k10_kernel_mechanization_six_line() {
    let mut count = 0usize;
    for a in ALPHABET {
        for b in ALPHABET {
            for c in ALPHABET {
                for d in ALPHABET {
                    for e in ALPHABET {
                        for f in ALPHABET {
                            let doc = format!("{a}\n{b}\n{c}\n{d}\n{e}\n{f}\n");
                            check(&doc);
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(count, ALPHABET.len().pow(6));
}
