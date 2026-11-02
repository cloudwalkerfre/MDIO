// g1_g2_ic_content —— 读侧内容门：G1 覆盖行 5/6/18 的原文列，
// G2 覆盖 X3 字面输入；断言内容投影的键型与列位。
use mdio_core::projection::State;
use mdio_core::topology::ProjectionMode;
use serde_json::Value;

/// G1 的自包含夹具（行 5/6/18 的原文列）
const G1_DOC: &str = "- x:\n  >     - 只要今天的\n  >     - 排完说最该先修哪个\n- y:\n  >     - mdio_success: $ /logs/today.md::ERRORS\n- z:\n  >     - E_CONN_REFUSED: 42\n";
/// G2 的 X3 字面输入（BQ col4 in list）
const X3_DOC: &str = "- a\n  - b\n    >     - x\n";

fn envelope(text: &str) -> Value {
    let mut st = State::new(text);
    let snap = st.snapshot();
    let s = snap.to_ast_object_mode(ProjectionMode::Cf);
    serde_json::from_str(&s).expect("envelope json")
}

fn ic_cds(v: &Value, out: &mut Vec<String>) {
    if let Some(arr) = v.get("ct").and_then(|c| c.as_array()) {
        for child in arr {
            if child.get("nt").and_then(|n| n.as_str()) == Some("ic") {
                if let Some(cd) = child.get("cd").and_then(|c| c.as_str()) {
                    out.push(cd.to_string());
                }
            }
            ic_cds(child, out);
        }
    }
}

#[test]
fn g1_three_ic_nodes_self_contained() {
    let json = envelope(G1_DOC);
    let mut cds = Vec::new();
    ic_cds(&json, &mut cds);
    println!("[G1] ic 节点数 = {}", cds.len());
    for (i, c) in cds.iter().enumerate() {
        println!("[G1]   ic[{i}].cd = {c:?}");
    }
    let wants = [
        "- 只要今天的\n- 排完说最该先修哪个",
        "- mdio_success: $ /logs/today.md::ERRORS",
        "- E_CONN_REFUSED: 42",
    ];
    let missing: Vec<&str> = wants
        .iter()
        .copied()
        .filter(|w| !cds.iter().any(|c| c == w))
        .collect();
    assert!(
        missing.is_empty(),
        "G1 红：以下 cd 期望值未出现：{:?}；实测 ic cd = {:?}",
        missing,
        cds
    );
}

#[test]
fn g2_x3_bq_in_list_ic_content() {
    let json = envelope(X3_DOC);
    let mut cds = Vec::new();
    ic_cds(&json, &mut cds);
    println!("[G2] X3 doc = {X3_DOC:?}");
    for (i, c) in cds.iter().enumerate() {
        println!("[G2]   ic[{i}].cd = {c:?}");
    }
    assert!(
        cds.iter().any(|c| c == "- x"),
        "G2 红：X3 的 ic cd 应为 \"- x\"，实测 ic cd = {:?}",
        cds
    );
}