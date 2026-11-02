// patch match attr —— P 帧序列化判官
//
// 判官：① claims 全空 ⇒ 与**实施前序列化的复刻**逐字节相同（本文件内置参考实现）；
//       ② match 键落位（四臂：Add/Replace/ReplaceAttr 在 value 后；Remove 紧跟 path）；
//       ③ 空集省略键；④ tag 转义与 path 同族。
use crate::incr::Patch;
use crate::r#match::ClaimSet;
use std::sync::Arc;

fn cs(tags: &[&str]) -> ClaimSet {
    let mut v = ClaimSet::new();
    for t in tags {
        v.push(Arc::from(*t));
    }
    v
}

/// 实施前序列化复刻（逐字节基准；与 git 历史的 store.rs:588-641 同文）。
fn reference_frame(patches: &[Patch]) -> Vec<u8> {
    let mut pkt = Vec::new();
    {
        let mut bw = crate::incr::BufWriter::new(&mut pkt);
        bw.write_byte(b'[');
        for (i, p) in patches.iter().enumerate() {
            if i > 0 {
                bw.write_byte(b',');
            }
            match p {
                Patch::Add { path, value } => {
                    bw.write_slice(b"{\"op\":\"add\",\"path\":\"");
                    bw.write_json_escaped(path);
                    bw.write_slice(b"\",\"value\":");
                    bw.write_slice(value.as_bytes());
                    bw.write_byte(b'}');
                }
                Patch::Remove { path } => {
                    bw.write_slice(b"{\"op\":\"remove\",\"path\":\"");
                    bw.write_json_escaped(path);
                    bw.write_slice(b"\"}");
                }
                Patch::Replace { path, value } => {
                    bw.write_slice(b"{\"op\":\"replace\",\"path\":\"");
                    bw.write_json_escaped(path);
                    bw.write_slice(b"\",\"value\":");
                    bw.write_slice(value.as_bytes());
                    bw.write_byte(b'}');
                }
                Patch::ReplaceAttr { path, attr, value } => {
                    bw.write_slice(b"{\"op\":\"replace\",\"path\":\"");
                    bw.write_json_escaped(path);
                    bw.write_byte(b'/');
                    bw.write_json_escaped(attr);
                    bw.write_slice(b"\",\"value\":");
                    bw.write_slice(value.as_bytes());
                    bw.write_byte(b'}');
                }
            }
        }
        bw.write_byte(b']');
        bw.commit();
    }
    pkt
}

fn corpus() -> Vec<Patch> {
    vec![
        Patch::Add { path: "/ct/0".into(), value: "{\"nt\":\"p\"}".into() },
        Patch::Add { path: "/ct/1/ct".into(), value: "[1,2]".into() },
        Patch::Remove { path: "/ct/2".into() },
        Patch::Replace { path: "/ct/3".into(), value: "\"模组\"".into() },
        Patch::ReplaceAttr { path: "/ct/4/ct/0".into(), attr: "t".into(), value: "\"a\\\"b\"".into() },
        Patch::ReplaceAttr { path: "/ct/5/ct/0".into(), attr: "ct".into(), value: "[{\"nt\":\"p\"}]".into() },
        Patch::Replace { path: "/ct/6".into(), value: String::new() },
        Patch::Add { path: "/".into(), value: "{}".into() },
    ]
}

#[test]
fn frame_bytes_identical_without_claims() {
    let patches = corpus();
    let claims = vec![ClaimSet::new(); patches.len()];
    let mut got = Vec::new();
    crate::store::write_patch_frame(&mut got, &patches, &claims);
    assert_eq!(
        got,
        reference_frame(&patches),
        "claims 全空时必须与实施前逐字节相同"
    );
}

#[test]
fn frame_match_key_placement() {
    let patches = vec![
        Patch::Add { path: "/ct/0".into(), value: "v".into() },
        Patch::Remove { path: "/ct/1".into() },
        Patch::Replace { path: "/ct/2".into(), value: "w".into() },
        Patch::ReplaceAttr { path: "/ct/3".into(), attr: "t".into(), value: "\"x\"".into() },
    ];
    let claims = vec![
        cs(&["m1"]),
        cs(&["m1", "m2"]),
        ClaimSet::new(), // 空集 ⇒ 省略键
        cs(&["m3"]),
    ];
    let mut got = Vec::new();
    crate::store::write_patch_frame(&mut got, &patches, &claims);
    let s = String::from_utf8(got).unwrap();
    assert_eq!(
        s,
        r#"[{"op":"add","path":"/ct/0","value":v,"match":["m1"]},{"op":"remove","path":"/ct/1","match":["m1","m2"]},{"op":"replace","path":"/ct/2","value":w},{"op":"replace","path":"/ct/3/t","value":"x","match":["m3"]}]"#
    );
}

#[test]
fn frame_match_key_escaping_and_empty_vec() {
    // tag 里的引号/反斜杠与 path 同族转义；中文原样透传。
    let patches = vec![Patch::Remove { path: "/ct/0".into() }];
    let claims = vec![cs(&["a\"b\\c", "模组"])];
    let mut got = Vec::new();
    crate::store::write_patch_frame(&mut got, &patches, &claims);
    let s = String::from_utf8(got).unwrap();
    // 语义判官：转义正确性 = 「帧是合法 JSON 且 tag 逐值相等」（免源码转义歧义）。
    let v: serde_json::Value = serde_json::from_str(&s).expect("P 帧必须是合法 JSON");
    let m = v[0]["match"].as_array().expect("match 数组");
    let got_tags: Vec<&str> = m.iter().map(|x| x.as_str().unwrap()).collect();
    assert_eq!(got_tags, vec!["a\"b\\c", "模组"]);
    // 边界（如实钉住）：本帧转义走共享原语 write_json_escaped——它只护引号/反斜杠/
    // 非 ASCII，不护控制字符（path 亦同族，且 path/tag 在生产里都是标识符形态）。
    assert!(!s.chars().any(|c| (c as u32) < 0x20), "帧内不得出现裸控制字符");
}

#[test]
fn frame_empty_patches_is_empty_array() {
    let mut got = Vec::new();
    crate::store::write_patch_frame(&mut got, &[], &[]);
    assert_eq!(String::from_utf8(got).unwrap(), "[]");
}
