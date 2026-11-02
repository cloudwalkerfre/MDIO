// 前缀不变性契约测试 —— v3(cached_stack 跨事务条件存活)的承重前提。
//
// 不变量 I1:edit+ripple 后,新 FrameLog 的三截拼接保证
//   new_entries[..prefix_len(rls)] == old_entries[..prefix_len(rls)]
// (行号逐位相同——前缀不平移;rls = ripple_line_start,回退后)。
//
// v3 的整个正确性归纳在此不变量上(缓存栈 = 前缀重放 ⟹ 跨事务有效)。
// 本测试在今天(未实现 v3)就必须通过——它是构造性质(三截拼接)的直接展开,
// 不是对新功能的期望。随机 2000 例:文档 × 编辑(插/删/换)。

use super::*;
use crate::block::entry_line;

fn next_rand(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state
}

fn tokens() -> &'static [&'static str] {
    &[
        "plain paragraph\n",
        "some **bold** text\n",
        "line one\n",
        "# Heading\n",
        "## Subheading\n",
        "### Deep\n",
        "- item\n",
        "- [ ] task\n",
        "1. first\n",
        "  - nested\n",
        "> quote\n",
        "> > nested\n",
        "```rust\nlet x = 1;\n```\n",
        "~~~\ncode\n~~~\n",
        "| a | b |\n|---|---|\n| c | d |\n",
        "[ref]: /url\n",
        "[id]: https://example.com \"t\"\n",
        "\n",
        "    indented code\n",
        "\tcode\n",
        "foo\n===\n",
        "foo\n---\n",
        "> foo\n> ===\n",
    ]
}

#[test]
fn prefix_invariance_fuzz() {
    let mut rng: u64 = 0xC0FF_EE42_4242_4242;
    let mut checked = 0usize;
    for _case in 0..2000u64 {
        let mut doc = String::new();
        let line_count = (next_rand(&mut rng) % 12) as usize + 1;
        for _ in 0..line_count {
            doc.push_str(tokens()[next_rand(&mut rng) as usize % tokens().len()]);
        }
        let mut st = State::new(&doc);
        let old_entries = st.frame_log.entries.clone();

        // 随机 1-2 次编辑(同事务聚合,覆盖 min_dirty 聚合路径)
        let n_edits = 1 + (next_rand(&mut rng) % 2) as usize;
        for _ in 0..n_edits {
            let len = st.rope.byte_len();
            if len == 0 {
                let t = tokens()[next_rand(&mut rng) as usize % tokens().len()];
                st.edit(0, 0, t);
                continue;
            }
            let op = next_rand(&mut rng) % 3;
            match op {
                0 => {
                    let pos = next_rand(&mut rng) as usize % (len + 1);
                    let pos = crate::projection::snap_down_char_boundary(&st.rope, pos);
                    let t = tokens()[next_rand(&mut rng) as usize % tokens().len()];
                    st.edit(pos, pos, t);
                }
                1 => {
                    let start = next_rand(&mut rng) as usize % len;
                    let start = crate::projection::snap_down_char_boundary(&st.rope, start);
                    let mut end = (start + 1 + next_rand(&mut rng) as usize % (len - start)).min(len);
                    end = crate::projection::snap_down_char_boundary(&st.rope, end).max(start);
                    st.edit(start, end, "");
                }
                _ => {
                    let start = next_rand(&mut rng) as usize % len;
                    let start = crate::projection::snap_down_char_boundary(&st.rope, start);
                    let mut end = (start + 1 + next_rand(&mut rng) as usize % (len - start)).min(len);
                    end = crate::projection::snap_down_char_boundary(&st.rope, end).max(start);
                    let t = tokens()[next_rand(&mut rng) as usize % tokens().len()];
                    st.edit(start, end, t);
                }
            }
        }
        let _ = st.snapshot();
        let Some(diff) = st.ripple_diff.take() else { continue };
        let rls = diff.ripple_line_start;

        // I1:rls 之前(不含)的 entries 逐位不变(含行号——前缀不平移)。
        let p_old = old_entries.partition_point(|e| entry_line(*e) < rls as u32);
        let new_entries = &st.frame_log.entries;
        let p_new = new_entries.partition_point(|e| entry_line(*e) < rls as u32);
        assert_eq!(
            p_old, p_new,
            "前缀条目数分叉: rls={rls} old_p={p_old} new_p={p_new}\ndoc={doc:?}"
        );
        assert_eq!(
            &old_entries[..p_old],
            &new_entries[..p_new],
            "前缀 entries 分叉: rls={rls}\ndoc={doc:?}\nfinal={:?}",
            st.rope.to_string()
        );
        checked += 1;
    }
    assert!(checked >= 1500, "样本不足: {checked}");
    eprintln!("[PREFIX-I1] checked={checked} —— 前缀不变性 2000 例全成立");
}
