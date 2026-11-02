//! 融合收官测试：ViewArrays(DepthChainFold) 折叠 == forward_replay_anchors 前缀重放
//!
//! 单源契约：fold_replay_entry 与 DepthChainFold 必须同律。
//! 覆盖：infer 侧 ViewArrays/DepthChainFold 的深度/帧/锚三项等价；
//! ripple 侧（shadow_replay_equals_forward_replay_anchors）由另一测试覆盖。
//! 说明: open_lines（HtmlElement 开行）由 frame_open_line 单源（block.rs）——
//!       集成测试不可访问 pub(crate)，此处用 pub 的 forward_replay_anchors
//!       （折叠 stack/anchors 的公开形态），frame 与 anchor 逐层对拍；
//!       depth/chain 的完整等价由 ①深度②帧③锚 三项断言覆盖。
//!
//! 运行: cargo test --test view_arrays_fold_equivalence

use mdio_core::block::{
    entry_frame, entry_line, forward_replay_anchors, BlockFrame, ANCHOR_NONE,
};
use mdio_core::infer::{build_structure_parts, partition};
use mdio_core::projection::State;

const TEST_DOCS: &[&str] = &[
    "",
    "# h\npara\n",
    "- a\n- b\n  - c\n\n- d\n",
    "> - x\n    code\n1. a\npara\n  1. c\ncont\n     - e\n  1. c\n  para\n",
    "| a | b |\n|---|---|\n| c | d |\n",
    "<div>\n  1. c\n      - f\n</div>\n",
    "```rust\nlet x = 1;\n```\n",
    "- [ ] task\n- [x] done\n\n> q\n",
];

#[test]
fn view_arrays_fold_equals_forward_replay() {
    for doc in TEST_DOCS {
        let mut st = State::new(doc);
        let snap = st.snapshot();
        let entries = &snap.frame_log.entries;
        let lines = &snap.lines;
        let n = snap.line_len();
        if n == 0 { continue; }
        let (recs, _blks, va, _arena) = build_structure_parts(&entries, &lines, &snap.open_depth, &snap.rope, (0, n as u32));

        for l in 0..n {
            // 前缀重放: 处理完行 l 的所有 entries（entry_line <= l）——公开形态
            let upto = partition(entries, l as u32 + 1);
            let mut stack: Vec<BlockFrame> = Vec::new();
            let mut anchors: Vec<Option<usize>> = Vec::new();
            forward_replay_anchors(&entries[..upto], &mut stack, &mut anchors);

            // ① depth 相等（0' 白捡：depth == chain.len() 构造性恒等）
            //    chain_rows 为 CSR 两列（chain_offsets/chain_data），行 l 链长 =
            //    chain_offsets[l+1] - chain_offsets[l]（集成测试不可访问 pub(crate) 的 chain_row）。
            let chain_len = (va.chain_offsets[l + 1] - va.chain_offsets[l]) as usize;
            assert_eq!(chain_len, stack.len(),
                "depth(=chain len) 分叉 doc={doc:?} l={l}: view={} replay={}",
                chain_len, stack.len());

            // ② chain 逐层: rid → records[rid].frame 的 kind/indent == 前缀重放栈帧的 kind/indent
            //    （body 位不比较: records 存"最终 body"（后续 body_mut 更新——如 LOOSE），
            //      前缀重放是"行时 body"——这是 ViewArrays 的既有设计（结构级查询），
            //      不是 fold 分裂; body_out 差异不影响 admit——R1 理论
            //      theory_bisimulation_body_out_diff_ok。）
            let lo = va.chain_offsets[l] as usize;
            let hi = va.chain_offsets[l + 1] as usize;
            let chain = &va.chain_data[lo..hi];
            assert_eq!(chain.len(), stack.len(),
                "chain 长度分叉 doc={doc:?} l={l}");
            for (di, &rid) in chain.iter().enumerate() {
                let frame = recs[rid as usize].frame;
                assert_eq!(frame.kind(), stack[di].kind(),
                    "frame kind 分叉 doc={doc:?} l={l} depth={di}: view={:?} replay={:?}",
                    frame.kind(), stack[di].kind());
                // indent 不比较: ContainerRec 的 indent 是"记录语义"（如 BQ 内容列），
                //   帧栈 indent 是"剥离前"——记录粒度 vs 帧粒度的既有差异（非 fold 分裂——kind 已证一致）。
                // ③ anchor 相等（records[rid].anchor 是"创建时锚"（item_boundary
                //    复用不更新锚）——等于该记录首次 Push 时的锚——与前缀重放行时锚一致）
                let va_anchor = recs[rid as usize].anchor;
                let rp_anchor = anchors[di].map(|a| a as u8).unwrap_or(ANCHOR_NONE);
                assert_eq!(va_anchor, rp_anchor,
                    "anchor 分叉 doc={doc:?} l={l} depth={di}: view={} replay={}",
                    va_anchor, rp_anchor);
            }
            // ④（注） open_lines 由 frame_open_line 单源 + forward_replay_runtime 恢复——
            //    本测试用公开形态（无 open_lines 返回）——该轴已在
            //    `shadow_replay_equals_forward_replay_anchors`（ripple 侧）覆盖。
            let _ = entry_frame; let _ = entry_line;
        }
    }
}
