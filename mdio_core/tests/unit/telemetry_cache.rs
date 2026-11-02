// telemetry_cache — hs_materialize_cost_gate 的 telemetry 纪律专门测试
// (上轮升级:Instant 外部计时 + "dur_ns" kv 覆盖系统 cputicks 字段 →
// span 自身计时 + 仅 scenario 维度 kv)。include! 进 cache.rs 的
// tests 模块(外层已 use super::*——本文件不再重复导入)。
// 判官纪律(§12):断言触达生产同构驱动(drive_hs_* 与 gate 共用同一
// 路径,零逻辑克隆)、事后断言(导出后形状检查)、场景可达(真实三场景)。

/// 共享场景驱动(与 hs_materialize_cost_gate 同一生产同构路径):
/// 编辑 → CF 物化 → HS 权威路径分解(标题扫描 + 节区间闭包)→ 全量构建。
/// TELEMETRY §10 纪律:span 自身 cputicks 计时(导出行 dur_ns = 系统
/// 数值);禁 Instant 外部时钟;禁 "dur_ns" 作 kv 名(会以字符串覆盖
/// 系统计时字段,§10.5 时钟纪律)。
fn drive_hs_scenario(
    doc: &str,
    edit_pos: usize,
    edit_len: usize,
    repl: &str,
    tag: &'static str,
    min_heads: usize,
) -> (usize, usize) {
    let mut state = State::new(doc);
    let old_snap = Arc::new(state.snapshot());
    state.edit(edit_pos, edit_pos + edit_len, repl);
    let new_snap = Arc::new(state.snapshot());
    let diff = state.ripple_diff.take().expect("ripple diff");
    let n_new = new_snap.line_len();
    let rls = diff.ripple_line_start;
    let rle_e = (diff.ripple_line_end + 1).min(n_new);
    let anchor = old_snap.line_of_byte(edit_pos.min(old_snap.byte_len().saturating_sub(1)));
    let base = CacheNode::new_base(old_snap.clone());
    let edit = CacheNode::new_edit_full(
        &old_snap,
        new_snap.clone(),
        (rls, rle_e),
        crate::cache::edit_actions_arc(vec![(edit_pos, edit_pos + edit_len)]),
        diff.delta,
        anchor,
    );
    // 1. CF 物化(span 自身计时)
    {
        let _sp = crate::telemetry_span!("hs_cost_cf_materialize", "scenario" => tag);
        let _ = edit.materialize_zone();
        let _ = edit.materialize_mid();
    }
    // 2. HS 权威路径分解:标题扫描 + 节区间闭包
    let heads = {
        let _sp = crate::telemetry_span!("hs_cost_heading_scan", "scenario" => tag);
        crate::incr::block_header_lines(
            &new_snap,
            crate::topology::HEADINGSECTION_TOPOLOGY.heading_candidates,
        )
    };
    let secs = {
        let _sp = crate::telemetry_span!("hs_cost_section_intervals", "scenario" => tag);
        crate::incr::line_section_intervals(&heads, new_snap.line_len())
    };
    // 3. 全量构建
    {
        let _sp = crate::telemetry_span!("hs_cost_full_build", "scenario" => tag);
        let _ = Segment::build(&new_snap, 0, n_new);
    }
    assert!(
        heads.len() >= min_heads,
        "scenario {tag}: heads {} below floor {min_heads}",
        heads.len()
    );
    assert!(!secs.is_empty(), "scenario {tag}: section intervals empty");
    let _ = &base;
    (heads.len(), secs.len())
}

/// 三场景全集驱动(与 gate 的 ①②③ 完全同构):
/// ① 长章节改标题(10×2000 行)② 章内内容编辑(2000 均匀节)
/// ③ 随机文档(3000 行 token 池)。
fn drive_hs_all_scenarios() {
    // ① 长章节改标题:改第 5 章标题。
    let mut doc1 = String::new();
    for s in 0..10 {
        doc1.push_str(&format!("# Long Section {s}\n\n"));
        for i in 0..2000 {
            doc1.push_str(&format!("line {s}-{i} text\n"));
        }
        doc1.push('\n');
    }
    let h5_pos = doc1.find("# Long Section 5").unwrap();
    drive_hs_scenario(
        &doc1,
        h5_pos + 2,
        "Long Section 5".len(),
        "Renamed Section Five",
        "s1_long_section_heading",
        10,
    );
    // ② 章内内容编辑:2000 个均匀小节,改中段正文。
    let mut doc2 = String::new();
    for i in 0..2000 {
        if i % 20 == 0 {
            doc2.push_str(&format!("# Section {i}\n\n"));
        } else {
            doc2.push_str(&format!("paragraph {i}\n\n"));
        }
    }
    let p_pos = doc2.find("paragraph 1005").unwrap();
    drive_hs_scenario(&doc2, p_pos, 5, "text!", "s2_uniform_many_sections", 100);
    // ③ 随机文档:结构与编辑点都随机。
    fn next_rand(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state
    }
    let tokens: &[&str] = &[
        "plain paragraph\n",
        "some **bold** text\n",
        "# Heading\n",
        "## Sub\n",
        "- item\n",
        "  - nested\n",
        "> quote\n",
        "```rust\nlet x = 1;\n```\n",
        "\n",
        "foo\n===\n",
        "- a\n\n  cont\n",
        "| a | b |\n|---|---|\n| c | d |\n",
    ];
    let mut rng: u64 = 0xD2A0_2026_0806_0003;
    let mut doc3 = String::new();
    for _ in 0..3000 {
        doc3.push_str(tokens[next_rand(&mut rng) as usize % tokens.len()]);
    }
    let p3 = (next_rand(&mut rng) as usize) % doc3.len();
    drive_hs_scenario(
        &doc3,
        p3,
        (next_rand(&mut rng) as usize % 20).min(doc3.len() - p3),
        "- x\n",
        "s3_random_doc",
        1,
    );
}

/// 无 feature 编译护栏:telemetry 类型只在 feature 下存在,会话块
/// 全部 cfg 剔除;无 feature 时各测试退化为"驱动路径正确性"单测
/// (span 宏展开 (),行为零变)。

/// S1+S2:导出行形状 = 系统计时数值型——Instant+dur_ns kv 反模式回归
/// 即红(dur_ns 变字符串);裸语句 span(dur≈0)即红(下界)。
#[test]
fn hs_cost_export_system_measured_shape() {
    #[cfg(feature = "telemetry")]
    let sess = std::sync::Arc::new(crate::telemetry::TelemetryInner::new(
        "hs_cost_shape".into(),
        crate::telemetry::TelemetryMode::Bench,
    ));
    #[cfg(feature = "telemetry")]
    let _guard = crate::telemetry::set_session(&sess);
    drive_hs_all_scenarios();
    #[cfg(feature = "telemetry")]
    {
        let json = crate::telemetry::export_phase_json(&sess);
        let rows: serde_json::Value = serde_json::from_str(&json).expect("export json");
        let mut events_checked = 0usize;
        for row in rows.as_array().expect("export rows") {
            let name = row["name"].as_str().unwrap_or("");
            if !name.starts_with("hs_cost_") {
                continue;
            }
            let d = row["dur_ns"]
                .as_u64()
                .unwrap_or_else(|| panic!("dur_ns must be system-measured number: {row}"));
            assert!(
                (1..=30_000_000_000u64).contains(&d),
                "system-measured dur_ns out of bounds: {d}"
            );
            if row["src"] == "events" {
                assert!(
                    row["scenario"].is_string() && !row["scenario"].as_str().unwrap_or("").is_empty(),
                    "scenario dimension missing on events row: {row}"
                );
                events_checked += 1;
            }
        }
        assert!(
            events_checked >= 12,
            "expected >=12 dimension rows (3 scenarios x 4 phases), got {events_checked}"
        );
    }
}

/// S3:维度归因完整性——3 tags x 4 phases 全矩阵、无多余维度行
/// (kv 丢失/错配即红)。
#[test]
fn hs_cost_scenario_dimension_matrix() {
    #[cfg(feature = "telemetry")]
    let sess = std::sync::Arc::new(crate::telemetry::TelemetryInner::new(
        "hs_cost_matrix".into(),
        crate::telemetry::TelemetryMode::Bench,
    ));
    #[cfg(feature = "telemetry")]
    let _guard = crate::telemetry::set_session(&sess);
    drive_hs_all_scenarios();
    #[cfg(feature = "telemetry")]
    {
        let json = crate::telemetry::export_phase_json(&sess);
        let rows: serde_json::Value = serde_json::from_str(&json).expect("export json");
        let mut got: Vec<(String, String)> = Vec::new();
        for row in rows.as_array().expect("export rows") {
            let name = row["name"].as_str().unwrap_or("");
            if name.starts_with("hs_cost_") && row["src"] == "events" {
                got.push((
                    row["scenario"].as_str().unwrap_or("").to_string(),
                    name.to_string(),
                ));
            }
        }
        let mut want: Vec<(String, String)> = Vec::new();
        for tag in [
            "s1_long_section_heading",
            "s2_uniform_many_sections",
            "s3_random_doc",
        ] {
            for phase in [
                "hs_cost_cf_materialize",
                "hs_cost_heading_scan",
                "hs_cost_section_intervals",
                "hs_cost_full_build",
            ] {
                want.push((tag.to_string(), phase.to_string()));
            }
        }
        got.sort();
        want.sort();
        assert_eq!(got, want, "scenario x phase matrix mismatch");
    }
}

/// S4+S5a:bench 模式 kv-span 双族行形态(events 带维度 / acc 无维度)
/// + 双写计数不变式(每 kv-span 恰好 acc+events 各一次——第三方计时
/// 混入即破)。
#[test]
fn hs_cost_bench_dual_family_rows() {
    #[cfg(feature = "telemetry")]
    let sess = std::sync::Arc::new(crate::telemetry::TelemetryInner::new(
        "hs_cost_family".into(),
        crate::telemetry::TelemetryMode::Bench,
    ));
    #[cfg(feature = "telemetry")]
    let _guard = crate::telemetry::set_session(&sess);
    drive_hs_all_scenarios();
    #[cfg(feature = "telemetry")]
    {
        let json = crate::telemetry::export_phase_json(&sess);
        let rows: serde_json::Value = serde_json::from_str(&json).expect("export json");
        let mut events = 0usize;
        let mut acc = 0usize;
        for row in rows.as_array().expect("export rows") {
            let name = row["name"].as_str().unwrap_or("");
            if !name.starts_with("hs_cost_") {
                continue;
            }
            match row["src"].as_str().unwrap_or("") {
                "events" => {
                    assert!(
                        row["scenario"].is_string(),
                        "events row must carry scenario: {row}"
                    );
                    events += 1;
                }
                "acc" => {
                    assert!(
                        row.get("scenario").is_none(),
                        "acc row must be dimension-free (pooled distribution): {row}"
                    );
                    acc += 1;
                }
                other => panic!("unexpected src family: {other}"),
            }
        }
        assert!(events > 0 && acc > 0, "both families must exist");
        assert_eq!(
            events, acc,
            "double-write invariant: acc rows must equal events rows per kv-span"
        );
    }
}

/// S6:trace 模式调用树形态——外层 span 包住驱动后,内层 hs_cost_* 的
/// parent 链可重建(Chrome trace args)。
#[test]
fn hs_cost_trace_tree_preserved() {
    #[cfg(feature = "telemetry")]
    let sess = std::sync::Arc::new(crate::telemetry::TelemetryInner::new(
        "hs_cost_trace".into(),
        crate::telemetry::TelemetryMode::Trace,
    ));
    #[cfg(feature = "telemetry")]
    let _guard = crate::telemetry::set_session(&sess);
    #[cfg(feature = "telemetry")]
    let _outer = crate::telemetry_span!("hs_gate_run");
    drive_hs_all_scenarios();
    #[cfg(feature = "telemetry")]
    {
        let json = crate::telemetry::report_json_trace(&sess);
        assert!(
            json.contains("hs_cost_heading_scan"),
            "trace must contain instrumented phases: {json}"
        );
        assert!(
            json.contains(r#""parent":"hs_gate_run""#),
            "trace must preserve parent tree: {json}"
        );
    }
}
