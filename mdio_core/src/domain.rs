//! 结构知识声明模块（S2 后：声明表 + 已接线访问器）。
//!
//! domain 子系统：声明账 + 接线账。本模块不引入运行时行为，
//! 只回答「哪些结构事实是可登记的概念、它们各自依赖什么、在哪一相位可知、用哪一档物化、
//! 由哪条律担保、当前是否已接线」。
//!
//! 边界：
//! - 只登记格档（Cell）四域加四对并行实现与投影三格，共 11 行；列档（承诺列 / 深度列）延后。
//! - 声明表只被 dguard/测试读取；域实例（DomainId/TopoDep/CellEntranceDecl）在
//!   projection 等生产路径已接线（S2）；头注「只登记，不接线」已过期。
//! - 两套并行各自一行、各自 caliber、各自 judge；第一阶段不裁决谁对谁错。
//! - 原子检查走常量泛型（内联 const 在单态化时求值）加模块级 const（编译期无条件求值）；
//!   不使用对象 trait 的关联常量（会杀死 dyn 兼容性）。
//! - 登记顺序即拓扑序：任何域的每个依赖下标严格小于本域下标（DAG 无环的 const 可判形式）。

// ── 域枚举（由宏同时生成变体与计数，避免「表长 == 变体数」的手工同步）──

macro_rules! domains {
    ($($id:ident),* $(,)?) => {
        #[repr(u16)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum DomainId { $($id),* }
        /// 枚举变体数（表长断言与依赖环判据共用）。
        pub(crate) const DOMAIN_COUNT: usize = [$(stringify!($id)),*].len();
    };
}

// 顺序即拓扑序：任何域的依赖下标必须严格小于本域下标（见 deps_topo_ordered）。
// 并行的两行各自口径、可相邻；但 ContTailBuild 以 ContTailView 为阅读顺序依赖
//（deps 已登记），故两行并非互不依赖；格档身份域（Tree/Sec/Span）无依赖，置于末尾。
domains! {
    ZoneAuth,
    ZoneMat,
    RootMerge,
    RootSplit,
    NodeEndNav,
    NodeEndRec,
    ContTailView,
    ContTailBuild,
    Tree,
    Sec,
    Span
}

// ── 原子 ──

/// 三个输入原子：行值列、帧事件日志、原文字节。
pub(crate) const A_ROPE: u8 = 1;
pub(crate) const A_PL: u8 = 2;
pub(crate) const A_FL: u8 = 4;

// ── 域属性 ──

/// 构造方式（四种，由它机械推出可知相位：Probe→Line、Replay/Build→Post、Closure→Zone）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Construct { Probe, Replay, Build, Closure }

/// 坐标轴（一个域只有一套坐标；出现第二套即「分裂」，按两个域实例登记）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CoordKind { Scalar, Range, Ordinal, Path, Bytes, Key }

/// 拓扑依赖：Free（消费点传实参）或 Pinned（键里取常量）。
#[derive(Clone, Copy)]
pub(crate) enum TopoDep { Free, Pinned(&'static crate::topology::Topology) }

/// 物化槽五档：Column / InSegment / Cell / SlotCache / PerCall。
/// 本表当前用到 Cell 与 PerCall 两档；其余变体留作后续登记与穷尽分派。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot { Column, InSegment, Cell, SlotCache, PerCall }

/// 状态位。注意：没有 DeletionCandidate —— 「不进表」的值
/// 不能出现在表字段里；无接线出口的概念在进表之前就被挡下（见 dguard）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status { Active, PendingWire, PendingMerge }

/// 生命周期状态（登记第七项）。只对释放方式不由机制固定的档必填；格档随快照生死，为 None。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifecycle { PerCall, WithSnapshot, SessionScoped, NodeScoped }

/// 一条域声明 = 七项登记加四项记账。
pub(crate) struct DomainDecl {
    /// 域（实例）名；同名异义必须拆名，不得合并。
    pub name: &'static str,
    /// 依赖原子位掩码（A_ROPE | A_PL | A_FL）。
    pub atoms: u8,
    /// 构造依赖（DAG 的边；下标必须严格小于本域下标）。
    pub deps: &'static [DomainId],
    /// 构造方式 → 可知相位。
    pub construct: Construct,
    /// 坐标轴。
    pub coord: CoordKind,
    /// 拓扑依赖。
    pub topo: TopoDep,
    /// 物化槽。
    pub slot: Slot,
    /// 是否身份投影产物（派生位：identity ⇒ 必须格档，见模块级断言）。
    pub identity: bool,
    /// 口径名：两套并行各自登记（键必须区分口径，否则先查者占槽、后查者命中）。
    pub caliber: Option<&'static str>,
    /// 状态位。
    pub status: Status,
    /// 接线出口：真正执行该域的断言或生产入口符号（与 judge 分工——judge 证明律）。
    pub outlet: Option<&'static str>,
    /// 生命周期；格档为 None（随快照生死，由机制推导）。
    pub lifecycle: Option<Lifecycle>,
    /// 自有判官名；无则由根律继承。
    pub judge: Option<&'static str>,
    /// 律名引用（强链接：LawId 枚举，编译期锁定，改名即红）——domain 接触面之一。
    pub law: crate::trinity::LawId,
}

// ── 声明表 ──
//
// 七个格入口的角色标注（S2 换元前的分工说明；不改 Status 枚举，不新增状态值）：
//   生产五个：
//     1. projection.rs::Snapshot::root_table_cell       —— 根（合并口径 / 拆分口径共用载荷，按 caliber 分键）
//     2. projection.rs::Snapshot::block_spans_cell      —— 跨度（Build 臂）
//     3. projection.rs::Snapshot::section_cell          —— 章节（拓扑实参传入，Free）
//     4. projection.rs::Snapshot::tree_cell             —— 归属树（拓扑实参传入，Free）
//     5. projection.rs::Snapshot::block_spans_cell_if_ready —— 跨度（UseIfReady 臂；生产调用点 = incr.rs 的
//        站点声明，先查后建、含继承腿，未命中由调用方走自陈回退）
//   仅 telemetry 一个：
//     6. projection.rs::Snapshot::peek_root_table_cell  —— 根表只读探测（Peek 臂，不建、无副作用）；
//        唯一调用点在 incr.rs 的 telemetry span 块内，故默认 feature 下不可达——属于「feature 门控的生产调用点」
//        这一类（S0 禁止删除清单类别之一），不是死代码。
//   诊断样例一个：
//     7. projection.rs::Snapshot::peek_block_spans_cell —— 跨度 Peek 臂样例位（doc 自陈样例/诊断位，
//        当前无生产调用）；其存在性由 tests/unit/keepalive.rs 的 const 断言钉住。
//
// 说明：以上七项是「入口」而非「域」——入口与域是多对一（同一域可有 Build/UseIfReady/Peek 三臂）。

pub(crate) static DOMAINS: [DomainDecl; DOMAIN_COUNT] = [
    DomainDecl {
        name: "zone_auth",
        atoms: A_FL | A_PL,
        deps: &[],
        construct: Construct::Closure,
        coord: CoordKind::Range,
        topo: TopoDep::Free,
        slot: Slot::PerCall,
        identity: false,
        caliber: Some("authoritative"),
        status: Status::Active,
        outlet: Some("incr::compute_zone"),
        judge: Some("zone_closure_equivalence"),
        law: crate::trinity::LawId::RZone,
        lifecycle: Some(Lifecycle::PerCall),
    },
    DomainDecl {
        name: "zone_mat",
        atoms: A_FL | A_PL,
        deps: &[DomainId::ZoneAuth],
        construct: Construct::Closure,
        coord: CoordKind::Range,
        topo: TopoDep::Free,
        slot: Slot::PerCall,
        identity: false,
        caliber: Some("materialized"),
        status: Status::Active,
        outlet: Some("cache::zone_both_sides"),
        judge: Some("zone_oracle_equivalence"),
        law: crate::trinity::LawId::RZone,
        lifecycle: Some(Lifecycle::PerCall),
    },
    DomainDecl {
        name: "root_merge",
        atoms: A_PL | A_FL | A_ROPE,
        deps: &[],
        construct: Construct::Build,
        coord: CoordKind::Ordinal,
        topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY),
        slot: Slot::Cell,
        identity: true,
        caliber: Some("merge"),
        status: Status::Active,
        outlet: Some("cache::kernel_root_table"),
        judge: Some("root_blocks_before_identity_matches_cf_tree_roots"),
        law: crate::trinity::LawId::RRootOrd,
        lifecycle: None,
    },
    DomainDecl {
        name: "root_split",
        atoms: A_PL | A_FL | A_ROPE,
        deps: &[],
        construct: Construct::Build,
        coord: CoordKind::Ordinal,
        topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY),
        slot: Slot::Cell,
        identity: true,
        caliber: Some("split"),
        status: Status::PendingMerge,
        outlet: Some("projection::try_update_secs_from_edit"),
        judge: Some("secs_cell_update_structural_matches_full"),
        law: crate::trinity::LawId::RSecs,
        lifecycle: None,
    },
    DomainDecl {
        name: "node_end_nav",
        atoms: A_PL | A_FL,
        deps: &[],
        construct: Construct::Build,
        coord: CoordKind::Range,
        topo: TopoDep::Free,
        slot: Slot::PerCall,
        identity: false,
        caliber: Some("navigation"),
        status: Status::Active,
        outlet: Some("infer::get_node_end_from_lines"),
        judge: None,
        law: crate::trinity::LawId::RTreeFold,
        lifecycle: Some(Lifecycle::PerCall),
    },
    DomainDecl {
        name: "node_end_rec",
        atoms: A_PL | A_FL | A_ROPE,
        deps: &[DomainId::NodeEndNav],
        construct: Construct::Build,
        coord: CoordKind::Range,
        topo: TopoDep::Free,
        slot: Slot::PerCall,
        identity: false,
        caliber: Some("record_direct"),
        status: Status::Active,
        // A4：record_direct 无独立出口符号，不挂悬空 outlet。
        outlet: None,
        judge: None,
        law: crate::trinity::LawId::RTreeFold,
        lifecycle: Some(Lifecycle::PerCall),
    },
    DomainDecl {
        name: "cont_tail_view",
        atoms: A_PL | A_FL,
        deps: &[],
        construct: Construct::Build,
        coord: CoordKind::Range,
        topo: TopoDep::Free,
        slot: Slot::PerCall,
        identity: false,
        caliber: Some("line_view"),
        status: Status::Active,
        outlet: Some("infer::LineView::container_end_for"),
        judge: Some("container_end_for_dual_caliber"),
        law: crate::trinity::LawId::RTreeFold,
        lifecycle: Some(Lifecycle::PerCall),
    },
    DomainDecl {
        name: "cont_tail_build",
        atoms: A_PL | A_FL,
        deps: &[DomainId::ContTailView],
        construct: Construct::Probe,
        coord: CoordKind::Range,
        topo: TopoDep::Free,
        slot: Slot::PerCall,
        identity: false,
        caliber: Some("build_phase"),
        status: Status::Active,
        outlet: Some("infer::BuildView::container_end_for"),
        judge: None,
        law: crate::trinity::LawId::RTreeFold,
        lifecycle: Some(Lifecycle::PerCall),
    },
    DomainDecl {
        name: "tree",
        atoms: A_PL | A_FL | A_ROPE,
        deps: &[],
        construct: Construct::Build,
        coord: CoordKind::Range,
        topo: TopoDep::Free,
        slot: Slot::Cell,
        identity: true,
        caliber: Some("full"),
        status: Status::Active,
        outlet: Some("projection::Snapshot::tree_cell"),
        judge: Some("tree_fold_block_nesting_oracle"),
        law: crate::trinity::LawId::RTreeFold,
        lifecycle: None,
    },
    DomainDecl {
        name: "sec",
        atoms: A_PL | A_FL,
        deps: &[],
        construct: Construct::Build,
        coord: CoordKind::Range,
        topo: TopoDep::Free,
        slot: Slot::Cell,
        identity: true,
        caliber: Some("full"),
        status: Status::Active,
        outlet: Some("projection::Snapshot::section_cell"),
        judge: Some("secs_cell_update_structural_matches_full"),
        law: crate::trinity::LawId::RSecs,
        lifecycle: None,
    },
    DomainDecl {
        name: "span",
        atoms: A_PL | A_FL | A_ROPE,
        deps: &[],
        construct: Construct::Build,
        coord: CoordKind::Range,
        topo: TopoDep::Pinned(&crate::topology::CONTAINER_TOPOLOGY),
        slot: Slot::Cell,
        identity: true,
        caliber: Some("full"),
        status: Status::Active,
        outlet: Some("projection::Snapshot::block_spans_cell"),
        judge: Some("assembled_full_equals_full_build"),
        law: crate::trinity::LawId::RSplice,
        lifecycle: None,
    },
];

// ── 句柄层：把声明过的原子变成类型事实（常量泛型，不用关联常量）──

#[derive(Clone, Copy)]
pub(crate) struct PlView<'a>(pub &'a [crate::block::PackedLineInfo]);
#[derive(Clone, Copy)]
pub(crate) struct FlView<'a>(pub &'a crate::block::FrameLog);
#[derive(Clone, Copy)]
pub(crate) struct RopeView<'a>(pub &'a crop::Rope);

/// 构造上下文：只填声明过的视图。读未声明原子在单态化时编译失败（E0080）。
pub(crate) struct DCtx<'a, const ATOMS: u8> {
    pl: Option<PlView<'a>>,
    fl: Option<FlView<'a>>,
    rope: Option<RopeView<'a>>,
}

impl<'a, const ATOMS: u8> DCtx<'a, ATOMS> {
    pub(crate) fn pl_view(&self) -> PlView<'a> {
        const { assert!(ATOMS & A_PL != 0, "域声明里没有 PL 原子") }
        self.pl.expect("PL 视图未填")
    }
    pub(crate) fn fl_view(&self) -> FlView<'a> {
        const { assert!(ATOMS & A_FL != 0, "域声明里没有 FL 原子") }
        self.fl.expect("FL 视图未填")
    }
    pub(crate) fn rope_view(&self) -> RopeView<'a> {
        const { assert!(ATOMS & A_ROPE != 0, "域声明里没有 ROPE 原子") }
        self.rope.expect("ROPE 视图未填")
    }
}

// ── 编译期内生检查（模块级常量：编译期无条件求值，check 期即报）──

/// 登记顺序必须构成拓扑序：任何域的每个依赖下标严格小于本域下标。
/// 这等价于「依赖 DAG 无环」的自然数标号判据，且不需要在 const 里做递归 DFS。
/// 负例探针（留档）：把任一行的 deps 指向下标更大的域，即报
/// `E0080 evaluation panicked: 域依赖 DAG 有环或登记顺序非拓扑序`；
/// 自环（deps 含自身）同样被 `>= i` 抓住。
pub(crate) const fn deps_topo_ordered(d: &[DomainDecl]) -> bool {
    let mut i = 0usize;
    while i < d.len() {
        let mut k = 0usize;
        let mut ok = true;
        while k < d[i].deps.len() {
            if d[i].deps[k] as usize >= i {
                ok = false;
                break;
            }
            k += 1;
        }
        if !ok {
            return false;
        }
        i += 1;
    }
    true
}

/// 表长 == 枚举变体数（加域忘改表在编译期即红，不留到运行期越界）。
const _: () = assert!(DOMAINS.len() == DOMAIN_COUNT, "表长不等于枚举变体数");

/// 依赖 DAG 无环（以「登记顺序即拓扑序」判定，见 deps_topo_ordered）。
const _: () = assert!(
    deps_topo_ordered(&DOMAINS),
    "域依赖 DAG 有环或登记顺序非拓扑序"
);

/// 逐行不变式：身份投影必须格档；待接线必须有接线出口；格档不得重复登记生命周期。
const _: () = {
    let mut i = 0usize;
    while i < DOMAINS.len() {
        if DOMAINS[i].identity {
            assert!(
                matches!(DOMAINS[i].slot, Slot::Cell),
                "身份投影产物必须进格（计划 §7.1 第 1 条）"
            );
        }
        if matches!(DOMAINS[i].status, Status::PendingWire) {
            assert!(
                DOMAINS[i].outlet.is_some(),
                "待接线域必须有显式断言出口，否则是删除候选"
            );
        }
        if matches!(DOMAINS[i].slot, Slot::Cell) {
            assert!(
                DOMAINS[i].lifecycle.is_none(),
                "格档生命周期由机制推导（随快照生死），不得另行登记"
            );
        }
        i += 1;
    }
};

#[cfg(test)]
mod domain_trinity_probe_tests {
    include!("../tests/unit/domain_trinity_probe.rs");
}

#[cfg(test)]
mod direction_domain_probe_tests {
    include!("../tests/unit/direction_domain_probe.rs");
}

#[cfg(test)]
mod domain_step2_preprobe_tests {
    include!("../tests/unit/domain_step2_preprobe.rs");
}
