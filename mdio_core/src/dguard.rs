//! domain 子系统就地守卫（声明账守卫 + 接线账审计；零依赖；跑在 --lib 门）。
//!
//! 定位：结构性约束（让声明模块路径不可命名）是主判官；本文件是机械底线，覆盖
//! 「路径仍可命名时的手写直引」这一类最常见违规。编译期只有 8KB 窗口 smoke，
//! 窗口外的盲区由本文件的全文件扫描覆盖。
//!
//! 裁定：严格原文扫描，不剥注释。代价是注释里出现禁令字样也会红——这是有意的：
//! 剥注释会整类漏掉「被注释掉的旧引用」这类文本，假阴性比假阳性危险。
//!
//! 覆盖不到的残余（须与结构性约束合读）：
//! - use ... as 重命名：重命名后正文只剩别名路径，文本看不见原路径；
//! - 宏展开产生的路径：token 流展开后的结果在源码文本里不存在；
//! - 字符串拼接构造的路径名：concat! 或运行期拼串，文本层永远看不见。

use crate::domain::{DOMAINS, DOMAIN_COUNT, DomainId, Slot, Status};

/// I3/I5 通用禁令（三个独立推导侧文件只用这一组）。
const FORBIDDEN_COMMON: &[&str] = &[
    "crate::domain",
    "DomainId",
    "DomainDecl",
    "DOMAINS",
    "DomainValue",
];

/// I3：缓存层生产区额外不得命名格层类型（格封印不外泄）。
const FORBIDDEN_CACHE: &[&str] = &["CellKey", "CellValue", "CellPolicy", "CellKind"];

/// 判官缺口白名单（节点尾两套实现缺对拍判官；容器尾两套同理）。
/// 纪律：judge 为空的行必须在此登记，否则登记完整性判官会红——缺口可见，不靠注释。
const JUDGE_GAP: &[&str] = &["node_end_nav", "node_end_rec", "cont_tail_build"];

fn hit(src: &str, needles: &[&str]) -> Vec<String> {
    needles
        .iter()
        .filter(|n| src.contains(**n))
        .map(|n| n.to_string())
        .collect()
}

/// I3/I5 机械底线：严格原文扫描（含注释）。
#[test]
fn domain_source_text_guard() {
    let cache = include_str!("cache.rs");
    let trinity = include_str!("trinity.rs");

    let mut cache_needles: Vec<&str> = FORBIDDEN_COMMON.to_vec();
    cache_needles.extend_from_slice(FORBIDDEN_CACHE);

    let mut bad: Vec<String> = Vec::new();
    let hits = hit(cache, &cache_needles);
    if !hits.is_empty() {
        bad.push(format!("cache.rs: {hits:?}"));
    }
    for (name, src) in [("trinity.rs", trinity)] {
        let hits = hit(src, FORBIDDEN_COMMON);
        if !hits.is_empty() {
            bad.push(format!("{name}: {hits:?}"));
        }
    }
    assert!(
        bad.is_empty(),
        "I3/I5 源码文本守卫命中（声明模块不得被生产/规范/还原/形式侧命名）: {bad:?}"
    );
}

/// 登记完整性：表长与枚举一一对应，每行必填项非空、原子位不越界。
#[test]
fn domain_registry_completeness() {
    assert_eq!(DOMAINS.len(), DOMAIN_COUNT, "表长必须等于枚举变体数");
    let known_bits = crate::domain::A_ROPE | crate::domain::A_PL | crate::domain::A_FL;
    for d in DOMAINS.iter() {
        assert!(!d.name.is_empty(), "域必须有名字");
        assert!(d.atoms != 0, "域必须声明依赖原子: {}", d.name);
        assert_eq!(d.atoms & !known_bits, 0, "原子掩码出现未知位: {}", d.name);
        assert!(
            d.judge.is_some() || JUDGE_GAP.contains(&d.name),
            "判官必须写明，或在 JUDGE_GAP 白名单里登记为已知缺口: {}",
            d.name
        );
        if d.status == Status::PendingMerge {
            assert!(d.caliber.is_some(), "待合并行必须写明口径名: {}", d.name);
        }
    }
}

/// 并行可见性：两套并行必须是两个不同 id、口径不同、判官不同、接线出口不同。
#[test]
fn domain_parallel_pairs_are_distinct() {
    let pairs: [(DomainId, DomainId); 4] = [
        (DomainId::ZoneAuth, DomainId::ZoneMat),
        (DomainId::RootMerge, DomainId::RootSplit),
        (DomainId::NodeEndNav, DomainId::NodeEndRec),
        (DomainId::ContTailView, DomainId::ContTailBuild),
    ];
    for (a, b) in pairs {
        assert_ne!(a, b, "两套并行不得共用同一个域 id");
        let (da, db) = (&DOMAINS[a as usize], &DOMAINS[b as usize]);
        assert_ne!(da.caliber, db.caliber, "两套并行口径名必须不同: {} vs {}", da.name, db.name);
        if da.judge.is_none() && db.judge.is_none() {
            // 两侧都无判官 = 已知缺口对；必须两侧都在缺口白名单里。
            assert!(
                JUDGE_GAP.contains(&da.name) && JUDGE_GAP.contains(&db.name),
                "两侧都无判官的并行对必须登记为已知缺口: {} vs {}",
                da.name,
                db.name
            );
        } else {
            assert_ne!(da.judge, db.judge, "两套并行判官名必须不同: {} vs {}", da.name, db.name);
        }
        assert_ne!(da.outlet, db.outlet, "两套并行接线出口必须不同: {} vs {}", da.name, db.name);
    }
}

/// S1 零接线回执：声明模块在库里不可命名（生产侧只允许经本测试文件引用）。
/// 真正的零引用判据是源码扫描门加编译期 8KB 窗口 smoke，本测试只作回执。
#[test]
fn domain_decl_module_is_not_wired_yet() {
    for d in DOMAINS.iter() {
        assert!(
            matches!(d.status, Status::Active | Status::PendingWire | Status::PendingMerge),
            "状态位只有三值: {}",
            d.name
        );
    }
}


// S2 判官：域 → 旧桶映射 + 入口级账本（附录 G.8-2 / G.7-1）


/// 全部域实例（判官清单）。长度与 DOMAIN_COUNT 对齐——加域漏登记本清单即红。
const ALL_DOMAINS: &[DomainId] = &[
    DomainId::ZoneAuth,
    DomainId::ZoneMat,
    DomainId::RootMerge,
    DomainId::RootSplit,
    DomainId::NodeEndNav,
    DomainId::NodeEndRec,
    DomainId::ContTailView,
    DomainId::ContTailBuild,
    DomainId::Tree,
    DomainId::Sec,
    DomainId::Span,
];

/// [S2/G.8-2] 域 → 旧桶映射：**满射 + 全性，不是双射**。
/// ① 每个 slot==Cell 的域恰好映到一个桶；② 非格档域映到 None；
/// ③ 五个桶每个都至少被一个域映到；④ **不得**要求「格实例 ↔ 桶」一一对应——
/// Tree/Sec 是 Free 拓扑（cf/hs 两实例共桶，G.6 显式折叠）。
#[test]
fn domain_bucket_map_is_surjective_and_total() {
    use crate::projection::{domain_bucket, CellKind, SnapshotConsumption};

    assert_eq!(ALL_DOMAINS.len(), DOMAIN_COUNT, "新域未进判官清单");

    const FIVE: [CellKind; 5] = [
        CellKind::Tree,
        CellKind::Secs,
        CellKind::RootTable,
        CellKind::RootBlocksTable,
        CellKind::BlockSpans,
    ];
    let mut covered = [false; 5];

    for &d in ALL_DOMAINS {
        let decl = &DOMAINS[d as usize];
        match (decl.slot, domain_bucket(d)) {
            (Slot::Cell, Some(b)) => covered[SnapshotConsumption::kind_idx(b)] = true,
            (Slot::Cell, None) => panic!("格档域必须有桶: {}", decl.name),
            (_, Some(b)) => panic!("非格档域不得有桶: {} -> {:?}", decl.name, b),
            (_, None) => {}
        }
    }
    for (i, b) in FIVE.iter().enumerate() {
        assert!(covered[i], "桶 {b:?} 没有被任何域映到（映射非满射）");
    }

    // 非双射的显式面：Free 拓扑域多实例共桶（cf/hs 折叠进同一个桶）。
    for d in [DomainId::Tree, DomainId::Sec] {
        assert!(
            matches!(DOMAINS[d as usize].topo, crate::domain::TopoDep::Free),
            "{} 应为 Free 拓扑（多实例共桶是 G.6 的显式折叠）",
            DOMAINS[d as usize].name
        );
        assert!(domain_bucket(d).is_some(), "Free 格档域仍必须有桶");
    }
    // 两套并行根口径不得共桶（否则域→桶映射失去隔离意义）。
    assert_ne!(domain_bucket(DomainId::RootMerge), domain_bucket(DomainId::RootSplit));
}

/// [S2/G.8-1] prewarm-critical 集合只含实测取证的域，且必须是格档域。
#[test]
fn prewarm_critical_domains_are_cell_slots() {
    use crate::projection::{domain_bucket, PREWARM_CRITICAL};

    assert!(
        PREWARM_CRITICAL.contains(&DomainId::RootMerge),
        "A-0′ 10× 实测的域必须在 prewarm-critical 里"
    );
    for &d in PREWARM_CRITICAL {
        assert!((d as usize) < DOMAIN_COUNT, "prewarm-critical 出现未知域判别值");
        let decl = &DOMAINS[d as usize];
        assert!(
            matches!(decl.slot, Slot::Cell),
            "prewarm-critical 只对格档域有意义: {}",
            decl.name
        );
        assert!(domain_bucket(d).is_some(), "prewarm-critical 域必须有桶: {}", decl.name);
    }
    // G.8-1：未取证的域不得先冻结进生产常量（RootSplit / Sec 待两条 p50）。
    for d in [DomainId::RootSplit, DomainId::Sec] {
        assert!(
            !PREWARM_CRITICAL.contains(&d),
            "{} 尚未有 p50 实测，不得预先冻结进 PREWARM_CRITICAL",
            DOMAINS[d as usize].name
        );
    }
}

/// [S2/G.7-1] 入口级计数器：**Σ入口 q == Σ桶 q**——窗口内每一次桶级查询都必须
/// 经七个格入口之一（不许有绕过入口的键级直调）。七入口各驱动一次。
#[test]
fn cell_entry_ledger_attributes_every_bucket_query() {
    use crate::projection::{cell_entry_reset, cell_entry_totals, CellEntry, CellKind, State};

    cell_entry_reset();
    let mut st = State::new("# H

para one

para two
");
    let snap = st.snapshot();
    let topo_c = &crate::topology::CONTAINER_TOPOLOGY;
    let topo_h = &crate::topology::HEADINGSECTION_TOPOLOGY;

    // span 桶三入口按 peek → if_ready → build 的顺序（G.2 顺序约束）。
    let _ = snap.peek_block_spans_cell();
    let _ = snap.block_spans_cell_if_ready();
    let _ = snap.block_spans_cell();
    let _ = snap.tree_cell(topo_c);
    let _ = snap.section_cell(topo_h);
    let _ = snap.root_table_cell_merge();
    let _ = snap.root_table_cell_split();
    let _ = snap.peek_root_table_cell(false);
    let _ = snap.peek_root_table_cell(true);

    let totals = cell_entry_totals();
    assert_eq!(totals.len(), crate::projection::CELL_ENTRY_COUNT);
    for e in [
        CellEntry::Tree,
        CellEntry::Section,
        CellEntry::RootTable,
        CellEntry::BlockSpans,
        CellEntry::PeekRootTable,
        CellEntry::SpansIfReady,
        CellEntry::PeekSpans,
    ] {
        assert!(totals[e as usize].0 >= 1, "入口 {e:?} 未计入入口账（分派表漏行？）");
    }

    const FIVE: [CellKind; 5] = [
        CellKind::Tree,
        CellKind::Secs,
        CellKind::RootTable,
        CellKind::RootBlocksTable,
        CellKind::BlockSpans,
    ];
    let bucket_q: u64 = FIVE.iter().map(|k| snap.cell_consumption.cell_stats(*k).0).sum();
    let entry_q: u64 = totals.iter().map(|t| t.0).sum();
    assert_eq!(entry_q, bucket_q, "Σ入口 q 必须等于 Σ桶 q（有查询绕过了七个格入口）");
    let bucket_h: u64 = FIVE.iter().map(|k| snap.cell_consumption.cell_stats(*k).1).sum();
    let entry_h: u64 = totals.iter().map(|t| t.1).sum();
    assert_eq!(entry_h, bucket_h, "Σ入口 h 必须等于 Σ桶 h");
    let bucket_b: u64 = FIVE.iter().map(|k| snap.cell_consumption.cell_stats(*k).2).sum();
    let entry_b: u64 = totals.iter().map(|t| t.2).sum();
    assert_eq!(entry_b, bucket_b, "Σ入口 b 必须等于 Σ桶 b");
}

/// [S2/G.3-4 + G.7-1] 待接线（PendingWire）入口的 q 只能在**入口级**证明：
/// 诊断 Peek 位（peek_block_spans_cell）在只跑生产入口的窗口里 q == 0，
/// 而同一 span 桶的桶级 q 已被 Build / UseIfReady 抬高——入口级不可省。
#[test]
fn cell_entry_ledger_proves_pending_wire_q_is_zero() {
    use crate::projection::{cell_entry_reset, cell_entry_stats, CellEntry, CellKind, State};

    cell_entry_reset();
    let mut st = State::new("# H

para one

para two
");
    let snap = st.snapshot();
    // 生产窗口：六个生产入口（不含诊断 Peek 位）。
    let _ = snap.block_spans_cell_if_ready();
    let _ = snap.block_spans_cell();
    let _ = snap.tree_cell(&crate::topology::CONTAINER_TOPOLOGY);
    let _ = snap.section_cell(&crate::topology::HEADINGSECTION_TOPOLOGY);
    let _ = snap.root_table_cell_merge();
    let _ = snap.root_table_cell_split();
    let _ = snap.peek_root_table_cell(false);

    assert_eq!(
        cell_entry_stats(CellEntry::PeekSpans).0,
        0,
        "PendingWire 入口（诊断 Peek 位）在无调用窗口必须 q == 0"
    );
    let span_q = snap.cell_consumption.cell_stats(CellKind::BlockSpans).0;
    assert!(
        span_q >= 2,
        "span 桶 q 已被同桶其他入口抬高（实得 {span_q}）——入口级计数不可省"
    );
}
