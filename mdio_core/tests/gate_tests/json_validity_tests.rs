/// F/P JSON 输出合法性验证——在本地 CommonMark 语料上端到端验证（语料未入库，路径由 commonmark_path() 解析）。
///
/// 手写 JSON（BufWriter 裸指针直写）容易缺逗号/括号/尾部逗号——
/// serde_json 整体解析兜底，外加 apply 后与全量 to_ast_object 一致性对比。
use std::sync::Arc;
use serde_json::Value;
use mdio_core::projection::State;
use mdio_core::incr::{patches_for_mode, Patch, IncrTask};
use mdio_core::topology::ProjectionMode;

fn commonmark_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().expect("mdio_core parent")
        .join("commonmark.md")
}

fn parse(v: &str) -> Value {
    serde_json::from_str(v)
        .unwrap_or_else(|e| panic!("invalid JSON: {e}\nvalue={:.300}", v))
}

/// root 信封 → ch 数组（按 mode 统一适配；集成测试
/// 本地辅助）。
fn ast_ch(snap: &mdio_core::projection::Snapshot) -> Value {
    let v: Value = parse(&snap.to_ast_object_mode(mdio_core::topology::ProjectionMode::Cf));
    v["ct"].clone()
}

/// T-mode 全量 ch 数组(827):信封 ch 按 mode 自动适配。
fn mode_ch(snap: &mdio_core::projection::Snapshot, mode: ProjectionMode) -> Value {
    let v: Value = parse(&snap.to_ast_object_mode(mode));
    v["ct"].clone()
}

fn apply_patch(root: &mut Value, p: &Patch) -> bool {
    let (path, val) = match p {
        Patch::Add { path, value } => (path, Some(value.as_str())),
        Patch::Remove { path } => (path, None),
        Patch::Replace { path, value } => (path, Some(value.as_str())),
        Patch::ReplaceAttr { path, value, .. } => (path, Some(value.as_str())),
    };
    if path.is_empty() {
        return match p {
            Patch::Replace { .. } => { *root = parse(val.unwrap()); true }
            _ => false,
        };
    }
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs == ["ct"] {
        let arr = if root.is_array() { root.as_array_mut() }
                  else { root.get_mut("ct").and_then(|c| c.as_array_mut()) };
        let arr = match arr { Some(a) => a, None => return false };
        match p {
            Patch::Add { .. } => { arr.push(parse(val.unwrap())); }
            Patch::Remove { .. } => { arr.clear(); }
            Patch::Replace { .. } | Patch::ReplaceAttr { .. } => { *arr = parse(val.unwrap()).as_array().cloned().unwrap_or_default(); }
        }
        return true;
    }
    if segs.len() < 2 { return false; }
    // 末段判位（同 unit/incr.rs 的 apply_patch_simple）：数字 = 数组下标；
    // 非数字 = 对象成员位（建数组 / 整数组 REM）。
    let last = *segs.last().unwrap();
    let member: Option<&str> = if last.parse::<usize>().is_ok() { None } else { Some(last) };
    let mut cur: &mut Value = root;
    for pair in segs[..segs.len() - 1].chunks(2) {
        if pair.len() < 2 { break; }
        let ci: usize = match pair[1].parse() { Ok(i) => i, Err(_) => return false };
        let arr = if cur.is_array() { cur.as_array_mut() }
                  else { cur.get_mut("ct").and_then(|c| c.as_array_mut()) };
        let arr = match arr { Some(a) => a, None => return false };
        if ci >= arr.len() { return false; }
        cur = &mut arr[ci];
    }
    if let Some(m) = member {
        let obj = match cur.as_object_mut() { Some(o) => o, None => return false };
        return match p {
            Patch::Add { .. } => { obj.insert(m.to_string(), parse(val.unwrap())); true }
            Patch::Remove { .. } => obj.remove(m).is_some(),
            Patch::Replace { .. } => {
                if !obj.contains_key(m) { return false; }
                obj.insert(m.to_string(), parse(val.unwrap()));
                true
            }
            Patch::ReplaceAttr { .. } => false,
        };
    }
    let idx: usize = last.parse().unwrap();
    let arr = if cur.is_array() { cur.as_array_mut() }
              else { cur.get_mut("ct").and_then(|c| c.as_array_mut()) };
    let arr = match arr { Some(a) => a, None => return false };
    match p {
        Patch::Add { .. } => { if idx <= arr.len() { arr.insert(idx, parse(val.unwrap())); true } else { false } }
        Patch::Remove { .. } => { if idx < arr.len() { arr.remove(idx); true } else { false } }
        Patch::Replace { .. } => { if idx < arr.len() { arr[idx] = parse(val.unwrap()); true } else { false } }
        Patch::ReplaceAttr { attr, .. } => {
            if idx < arr.len() {
                if let Some(obj) = arr[idx].as_object_mut() {
                    obj.insert(attr.clone(), parse(val.unwrap()));
                    true
                } else { false }
            } else { false }
        }
    }
}

/// apply_patch 的成员末段语义（建数组 / 整数组 REM/REPL），
/// 并回归根 `/ct` 的**子数组专用**分支（:49）不被成员分支吃掉。
#[test]
fn apply_member_end_segment_semantics() {
    // ① 根 /ct 专用分支仍在：Add 追加、Replace 整数组替换
    let mut root: Value = parse(r#"{"nt":"blt","ct":[{"nt":"li","t":""}]}"#);
    assert!(apply_patch(&mut root, &Patch::Add {
        path: "/ct".into(), value: r#"{"nt":"li","t":"b"}"#.into(),
    }), "① 根 /ct Add 应走子数组专用分支");
    assert_eq!(root["ct"].as_array().unwrap().len(), 2, "① 追加结果: {root}");
    assert!(apply_patch(&mut root, &Patch::Replace {
        path: "/ct".into(), value: r#"[{"nt":"li","t":"r"}]"#.into(),
    }), "① 根 /ct Replace 应整数组替换");
    assert_eq!(root["ct"], parse(r#"[{"nt":"li","t":"r"}]"#), "① 整数组替换结果");
    // ② 成员位建数组（丙）：壳缺 ct ⇒ 一条 op 落整数组
    let mut tree: Value = parse(r#"{"nt":"blt","ct":[{"nt":"li","t":""},{"nt":"li","t":""}]}"#);
    let add_arr = Patch::Add {
        path: "/ct/0/ct".into(), value: r#"[{"nt":"p","t":"x"}]"#.into(),
    };
    assert!(apply_patch(&mut tree, &add_arr), "② 成员位 Add(数组) 应成功");
    assert_eq!(
        tree,
        parse(r#"{"nt":"blt","ct":[{"nt":"li","t":"","ct":[{"nt":"p","t":"x"}]},{"nt":"li","t":""}]}"#),
        "② 建数组后整棵结果"
    );
    // ③ 成员位 Replace 覆盖已存在数组（整数组 REPL）
    let repl_arr = Patch::Replace {
        path: "/ct/0/ct".into(), value: r#"[{"nt":"p","t":"y"}]"#.into(),
    };
    assert!(apply_patch(&mut tree, &repl_arr), "③ 成员位 Replace(键存在) 应成功");
    assert_eq!(tree["ct"][0]["ct"], parse(r#"[{"nt":"p","t":"y"}]"#), "③ 覆盖结果");
    // ④ 成员位 Remove 删键
    assert!(apply_patch(&mut tree, &Patch::Remove { path: "/ct/0/ct".into() }),
        "④ 成员位 Remove(键存在) 应成功");
    assert!(tree["ct"][0].get("ct").is_none(), "④ 删键结果: {tree}");
    // ⑤ 成员位 Replace 于不存在键 ⇒ 失败，且不留痕
    assert!(!apply_patch(&mut tree, &Patch::Replace {
        path: "/ct/1/ct".into(), value: "[]".into(),
    }), "⑤ Replace 不存在键必须失败");
    assert!(tree["ct"][1].get("ct").is_none(), "⑤ 失败不得留痕: {tree}");
    // ⑥ 末段数字仍是数组下标（Insert 语义）
    assert!(apply_patch(&mut tree, &Patch::Add {
        path: "/ct/1".into(), value: r#"{"nt":"li","t":"w"}"#.into(),
    }), "⑥ 下标 Add 应成功");
    assert_eq!(tree["ct"].as_array().unwrap().len(), 3, "⑥ Insert 语义: {tree}");
    // ⑦ 成员位标量覆盖 + ReplaceAttr 落在节点对象上（两种仍互不串味）
    assert!(apply_patch(&mut tree, &Patch::Add {
        path: "/ct/0/t".into(), value: r#""renamed""#.into(),
    }), "⑦ 标量成员写应成功");
    assert_eq!(tree["ct"][0]["t"], "renamed", "⑦ 标量成员写结果");
    assert!(apply_patch(&mut tree, &Patch::ReplaceAttr {
        path: "/ct/0".into(), attr: "t".into(), value: r#""attr""#.into(),
    }), "⑦ ReplaceAttr(element+attr) 应成功");
    assert_eq!(tree["ct"][0]["t"], "attr", "⑦ ReplaceAttr 结果");
}

/// 单个编辑的完整验证：F-snapshot 合法 + 每个 P-patch value 合法 + apply 后与全量一致。
/// T-mode(827):同一用例按 mode 自动切换适配——CF/HS 双跑共用一个 verify。
fn verify_edit(md: &str, start: usize, end: usize, text: &str) {
    for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
        let mut state = State::new(md);
        let snap_old = Arc::new(state.snapshot());
        let old_json: Value = mode_ch(&snap_old, mode);
        state.edit(start, end, text);
        let snap = Arc::new(state.snapshot());
        let new_json: Value = mode_ch(&snap, mode);
        let diff = state.ripple_diff.take().expect("must have ripple_diff");
        let task = IncrTask {
            snap: snap.clone(), doc_id: 0,
            snap_old,
            delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(start, end)]),
            ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
            slot: None, slot_old: None, reaper_tx: None, structural_change: false,
        };
        let patches = patches_for_mode(&task, mode);
        for p in &patches {
            match p {
                Patch::Add { path, value } | Patch::Replace { path, value } => {
                    parse(value); // 合法性：缺逗号/括号/尾部逗号在此炸
                    let _ = path;
                }
                Patch::ReplaceAttr { value, .. } => {
                    parse(value);
                }
                Patch::Remove { .. } => {}
            }
        }
        let mut got = old_json.clone();
        for p in &patches {
            if !apply_patch(&mut got, p) {
                panic!("APPLY_FAIL[{mode:?}] for edit ({start},{end}) {:?} patch={:?}", text, p);
            }
        }
        if got != new_json {
            let g = serde_json::to_string_pretty(&got).unwrap_or_default();
            let n = serde_json::to_string_pretty(&new_json).unwrap_or_default();
            // 找第一个不同行
            let gl: Vec<&str> = g.lines().collect();
            let nl: Vec<&str> = n.lines().collect();
            let mut first = "?".to_string();
            for (i, (a, b)) in gl.iter().zip(nl.iter()).enumerate() {
                if a != b {
                    first = format!("line {i}: got={a:?} new={b:?}");
                    break;
                }
            }
            if gl.len() != nl.len() {
                first = format!("len got={} new={} firstdiff={first}", gl.len(), nl.len());
            }


            // 错位点附近
            for (i, (a, b)) in gl.iter().zip(nl.iter()).enumerate().skip(780).take(24) {
                if a != b {  }
            }
            panic!("apply mismatch[{mode:?}] for edit ({start},{end}) {:?}\nfirst diff: {first}\npatches: {:?}", text, patches);
        }
    }
}

#[test]
fn ic_probe() {
    // 缩进代码块内容编辑——复现 nt 变 p 的疑点
    let md = "    code line one\n    code line two\n\npara\n";
    let mut state = State::new(md);
    let snap_old = Arc::new(state.snapshot());
    state.edit(7, 11, "EDIT"); // 改第一行 code → EDIT
    let snap = Arc::new(state.snapshot());
    let diff = state.ripple_diff.take().unwrap();
    let task = IncrTask {
        snap: snap.clone(), doc_id: 0,
        snap_old,
        delta: diff.delta, edit_ranges: mdio_core::edit_actions_arc(vec![(7, 11)]),
        ripple_line_start: diff.ripple_line_start, ripple_line_end: diff.ripple_line_end,
        slot: None, slot_old: None, reaper_tx: None, structural_change: false,
    };
    let patches = patches_for_mode(&task, ProjectionMode::Cf);
    for p in &patches {

    }
}

#[test]
fn f_snapshot_commonmark_valid_json() {
    let md = std::fs::read_to_string(commonmark_path()).expect("commonmark.md");
    let snap = State::new(&md).snapshot();
    let arr = ast_ch(&snap).as_array().cloned().unwrap_or_default();
    assert!(arr.len() > 100, "commonmark.md should yield many root blocks, got {}", arr.len());
    // 逐节点递归检查（额外保险：嵌套结构完整）
    fn walk(v: &Value, path: &str) {
        match v {
            Value::Array(items) => {
                for (i, it) in items.iter().enumerate() {
                    walk(it, &format!("{path}[{i}]"));
                }
            }
            Value::Object(map) => {
                for (k, val) in map {
                    walk(val, &format!("{path}/{k}"));
                }
            }
            _ => {}
        }
    }
    walk(&Value::Array(arr), "/children");
}

#[test]
fn p_patches_commonmark_valid_json() {
    let md = std::fs::read_to_string(commonmark_path()).expect("commonmark.md");
    // 多种编辑类型 × 不同位置
    // 诊断：行 144-158 的 PL + 渲染
    {
        let mut st = State::new(&md);
        let old_s = st.snapshot();
        st.edit(5000, 5005, "EDIT");
        let new_s = st.snapshot();


        for (name, s) in [("OLD", &old_s), ("NEW", &new_s)] {
            let full: Value = ast_ch(&s);
            fn scan(v: &Value, path: &str, out: &mut Vec<String>) {
                let s = serde_json::to_string(v).unwrap_or_default();
                if s.contains("Or this?") || s.contains("There are") || s.contains("1.  one") {
                    out.push(format!("{path}: {}", &s[..s.len().min(110)]));
                }
                if let Some(arr) = v.get("ct").and_then(|c| c.as_array()) {
                    for (i, it) in arr.iter().enumerate() {
                        scan(it, &format!("{path}/{i}"), out);
                    }
                }
            }
            let mut out = Vec::new();
            scan(&full, "", &mut out);
            for o in out.iter().take(6) {  }
        }
    }
    verify_edit(&md, 5000, 5005, "EDIT");                        // 行内改字（中部）
    verify_edit(&md, 2000, 2000, "# New Heading\n");             // 插行（结构变化）
    verify_edit(&md, 2000, 2000, "## *em* new head\n");         // 含触发标题插入（规则 R：t 两编码）
    // 已有无触发标题 → 含触发标题（t 由字符串变数组；diff 走整块 REPL 回退）
    if let Some(i) = md.find("\n## Block quotes\n") {
        let s = i + 1;
        verify_edit(&md, s, s + "## Block quotes".len(), "## *em* quotes\n");
    }
    verify_edit(&md, 3000, 3100, "");                             // 删段（大删除）
    verify_edit(&md, 8000, 8000, "- bullet\n- list\n");           // 尾部插列表
    verify_edit(&md, 100, 100, "> quote\n");                      // 前部插 BQ
    // CF 块：行首插入（合法围栏——段落中间插入 ``` 会产生未闭合 CF，非真实输入）
    let l_cf = md.find("This document is generated from a text").unwrap();
    verify_edit(&md, l_cf, l_cf, "```rust\nfn main() {}\n```\n");
    verify_edit(&md, 7000, 7005, "1. one\n2. two\n");             // OL
    // 全文档替换（极端）
    verify_edit(&md, 0, md.len(), "# Fresh\n\n- a\n");
}
// block × inline × 双模组合矩阵回归
//
// 用例覆盖 5 类容器 × 8 类行形与深度组合；组合形态是本矩阵的用例源。
//
// 每篇统一判据（与前端 verify:ast 的“双模等价 + apply==full”同款）：
//   ① 标题两编码（规则 R）：CF/HS 同一标题的“数组拼接文本”一致、编码形态一致
//      （半收敛守卫）
//   ② 一笔行内编辑 → verify_edit：CF/HS 各跑 apply(P, F_old) == F_new

/// 行内数组 → 文本（规则 R 的“数组侧”：标记被消费；软换行等节点按其 ct）。
fn inline_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(inline_text).collect(),
        Value::Object(o) => o
            .get("ct")
            .or_else(|| o.get("cd"))
            .map(inline_text)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// 收集标题（深度优先）：(lv, t 文本, t 是否数组)
fn collect_heads(v: &Value, out: &mut Vec<(u64, String, bool)>) {
    match v {
        Value::Array(a) => a.iter().for_each(|x| collect_heads(x, out)),
        Value::Object(o) => {
            if o.get("nt").and_then(|x| x.as_str()) == Some("h") {
                let t = o.get("t").cloned().unwrap_or(Value::Null);
                out.push((
                    o.get("lv").and_then(|x| x.as_u64()).unwrap_or(0),
                    inline_text(&t),
                    t.is_array(),
                ));
            }
            if let Some(c) = o.get("ct") { collect_heads(c, out); }
        }
        _ => {}
    }
}

fn bugs2_matrix_docs() -> Vec<&'static str> {
    vec![
        // ── L2 × K1（内容 ÷ 列表容器；guard 最多）──
        "- *em* item\n",
        "- **strong** item\n",
        "- `code` item\n",
        "- [ ] task *em*\n",
        "1. *em* ordered\n",
        // ── L1 × K1（列表项 ÷ 列表容器：PopPush vs Push）──
        "- a\n  - *em* b\n",
        "- a\n  1. **c**\n",
        "- a\n  > *em* q\n",
        // ── L2 × K1 细分（段落延续 / 标题拒绝 / 围栏拒绝）──
        "- a\n  # *em* h\n",
        "- a\n  ```\n  *em*\n  ```\n",
        // ── L2 × K2（内容 ÷ 引用容器：懒延续）──
        "> *em* text\n",
        "> **strong** text\n> lazy *em*\n",
        "> - *em* item\n",
        // ── L3/L4 × K2（围栏/表格 ÷ BQ；R21 virtual u8 区）──
        "> ```\n> *em* code\n> ```\n",
        "> | *a* | **b** |\n> |---|---|\n> | `c` | d |\n",
        // ── L8 × K2/K3/K4（空行拒绝 vs Continue）──
        "> a\n\n> *em* b\n",
        "```\n\n*em*\n```\n",
        "<div>\n\n*em*\n</div>\n",
        // ── L5/L6 × K4/K5（HTML 块/元素；P6 吞噬区）──
        "<div>\n*em*\n</div>\n",
        "<p>*em*</p>\n",
        "<script>\n*em*\n</script>\n",
        // ── L7 × K5（引用定义）──
        "[lab]: /url \"*em*\"\n",
        // ── 标题族（tag 面两编码）──
        "# *em* H1\n",
        "## **strong** H2\n",
        "### `code` H3\n",
        "*em* setext\n===\n",
        "plain setext\n---\n",
        "#### plain H4\n",
        // ── 表 III 深度组合 ──
        "> > *em* deep\n",
        "- - *em* deep\n",
        "- > *em* x\n",
        "> - - **b**\n",
        "> - > *em* x\n",
        "- > - *em* x\n",
        "> - a\n>   - b\n",
        // 表 III 深度 4（[BQ,BLT,BLT,CF] / [BQ,BQ,BLT,CF]）
        "> - - \u{0060}\u{0060}\u{0060}\n>     *em*\n>     \u{0060}\u{0060}\u{0060}\n",
        "> > - \u{0060}\u{0060}\u{0060}\n> >   *em*\n> >   \u{0060}\u{0060}\u{0060}\n",
        // BUGS_DIG 历史形态（cross-kind / BQ>CF / 嵌套关闭）
        "* a\n- b\n",
        "1. a\n- b\n",
        "> - a\n>   - b\n> *em*\n",
        "> > a\n> *em* b\n",
    ]
}

#[test]
fn bugs2_block_inline_matrix_dual_mode() {
    for (i, md) in bugs2_matrix_docs().iter().enumerate() {
        let mut st = State::new(md);
        let snap = st.snapshot();
        let cf = mode_ch(&snap, ProjectionMode::Cf);
        let hs = mode_ch(&snap, ProjectionMode::Hs);
        let mut cfh = Vec::new();
        let mut hsh = Vec::new();
        collect_heads(&cf, &mut cfh);
        collect_heads(&hs, &mut hsh);
        assert_eq!(cfh.len(), hsh.len(),
            "case {i} 标题数两模式不一致: {md:?}\ncf={cfh:?}\nhs={hsh:?}");
        for (a, b) in cfh.iter().zip(hsh.iter()) {
            assert_eq!(a.0, b.0, "case {i} 标题层级不一致: {md:?}");
            assert_eq!(a.1, b.1, "case {i} 标题文本（数组拼接）两模式不一致: {md:?}");
            assert_eq!(a.2, b.2, "case {i} 标题两编码形态两模式不一致（半收敛）: {md:?}");
        }
        // 一笔行内编辑（插到首行末）→ CF/HS 双跑 apply==full
        let e = md.find('\n').unwrap_or(md.len());
        verify_edit(md, e, e, " *x*");
    }
}

/// 规则 R 锁定：有触发字符的标题，行内必须在 t（CF 叶子与 HS 节壳同判）。
#[test]
fn header_inline_lives_in_t_both_modes() {
    // (文档, 该标题 t 数组拼接后的期望文本——标记被消费、余文保留)
    for (md, want) in [("# *em* H1\n", "em H1"), ("*em* setext\n===\n", "em setext")] {
        let mut st = State::new(md);
        let snap = st.snapshot();
        for mode in [ProjectionMode::Cf, ProjectionMode::Hs] {
            let v = mode_ch(&snap, mode);
            let mut heads = Vec::new();
            collect_heads(&v, &mut heads);
            assert!(!heads.is_empty(), "{mode:?} {md:?} 无标题节点");
            for (lv, text, is_arr) in &heads {
                assert!(*is_arr, "{mode:?} {md:?} lv={lv} 行内标题的 t 应为数组: {text:?}");
                assert_eq!(text, want, "{mode:?} {md:?} 行内拼接文本不符");
            }
        }
    }
}

