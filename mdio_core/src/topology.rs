//! 拓扑声明(拓扑 = 数据结构——三端:
//! 候选声明(建树端)/壳声明(渲染端)/zone 声明(增量端)。
//! 第三拓扑 = 填一份数据——共享算法内部枚举分派(闭集合——罕见扩展加变体)。
//! 第一性约束:新增拓扑成本 = 只写一个
//! 构建器(填数据)——共享层零拓扑专属语义。

use crate::block::{BlockType, FrameKind};
use crate::effect::{BLOCK_ATTRS, TAG_ONLY_ATTRS, BlockAttrSpec};

/// 候选声明(建树端)——类型 → 区间来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateRule {
    /// 容器记录区间(已物化——ContainerRecord——expand 建容器节点)。
    /// 透明穿透排除是**运行时属性**(Table 未确认/rd 撤销/eff.is_transparent
    /// ——shell_transparent)——不进声明——expand 查表后仍查运行时透明。
    ContainerRecord,
    /// 列表分段(共享——push_lines 分组——BLT/OL → ItemLeaf 段、
    /// TableRow → 行 Shell 段)。
    ItemSegments,
    /// 标题区间(派生——level 单调栈——HeadingSection)。
    HeadingSection,
    /// 块记录区间(已物化——通用来源——块节点)。
    BlockRange,
}

/// 壳声明(渲染端)——薄两字段。attrs 空 = 不写 attrs(现状 is_shell 语义——
/// BQ/BLT/OL);非空 = 按规格写(现状非 shell 容器的开壳 attrs 语义)。
#[derive(Debug, Clone, Copy)]
pub struct ShellDecl {
    pub nt: &'static str,
    pub attrs: &'static [BlockAttrSpec],
}

/// zone 声明(增量端)——ripple 映射规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneRule {
    /// 容器闭包(包含依赖——现状——ripple → 容器完整)。
    ContainerClosure,
    /// 章节闭包(边界依赖——四类)。
    SectionClosure,
}

/// 拓扑 = 一个数据结构(候选 + 壳 + zone)。
pub struct Topology {
    pub name: &'static str,
    /// 容器帧候选(ContainerRecord/ItemSegments——键 = FrameKind)。
    /// 键不能用 BlockType——Table 帧与 TableRow 帧的
    /// container_block_type 都 = TableRow——键冲突(Table 帧被误判为
    /// ItemSegments 分段)——FrameKind 10 种无冲突。
    pub candidates: &'static [(FrameKind, CandidateRule)],
    /// 标题候选(HeadingSection——键 = 块类型——标题非帧——
    /// 单独字段避免与容器帧混合)。
    pub heading_candidates: &'static [BlockType],
    /// 块候选白名单(BlockRange——键 = 块类型——attach 只建
    /// 声明块——空 = 全建(CF/HS 现状——行为不变);非空 = 白名单
    /// (第三拓扑演示——泛化验证))。
    pub block_candidates: &'static [BlockType],
    /// 壳声明按 FrameKind 索引(与 CONTAINER_EFFECTS 同索引——10 项)。
    pub shells: &'static [ShellDecl],
    /// 块壳声明(章节壳等——块类型 → 壳——渲染端块分支先查此表——
    /// 命中 = 壳渲染(nt + attrs + ch 递归)而非 leaf——§五 两层分派的
    /// 扩展形态)。
    pub shells_by_block: &'static [(BlockType, ShellDecl)],
    pub zone: ZoneRule,
}

impl Topology {
    /// 容器帧是否携带指定候选规则(同一帧可多条规则——容器帧 + 分段)。
    pub fn has_rule(&self, fk: FrameKind, rule: CandidateRule) -> bool {
        self.candidates.iter().any(|&(f, r)| f == fk && r == rule)
    }

    /// 壳声明查表(按 FrameKind——O(1) 索引)。
    pub fn shell_for(&self, fk: FrameKind) -> &'static ShellDecl {
        &self.shells[fk as usize]
    }

    /// 块壳声明查表(章节壳——块类型 → 壳——None = leaf 渲染)。
    pub fn shell_by_block(&self, bt: BlockType) -> Option<&'static ShellDecl> {
        self.shells_by_block.iter()
            .find(|&&(t, _)| t == bt)
            .map(|&(_, ref s)| s)
    }
}

/// 投影模式(拓扑分派的 JSON 表达——root 信封的 mode 字段——§5.6)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionMode {
    /// 容器拓扑(现状默认映射模型)。
    Cf,
    /// 章节拓扑。
    Hs,
    /// 块平铺拓扑(泛化验证演示——第三拓扑)。
    Bf,
}

impl ProjectionMode {
    /// mode 字段的 JSON 值(与 Topology.name 一致)。
    pub fn name(self) -> &'static str {
        match self {
            ProjectionMode::Cf => "cf",
            ProjectionMode::Hs => "hs",
            ProjectionMode::Bf => "bf",
        }
    }

    /// 模式 → 拓扑声明(ProjectionMode 入口)。
    pub fn topology(self) -> &'static Topology {
        match self {
            ProjectionMode::Cf => &CONTAINER_TOPOLOGY,
            ProjectionMode::Hs => &HEADINGSECTION_TOPOLOGY,
            ProjectionMode::Bf => &BLOCKFLAT_TOPOLOGY,
        }
    }
}

// ── omp S3:进程级 ProcessMode（One Mode Per Process）──────────────────────
//
// 语义：
//   生命周期 = 进程启动（首次读取时从环境变量定一次）→ 进程结束；进程内唯一。
//   生产通道唯一 = 进程环境变量 MDIO_PROJ_MODE（"cf"→Cf，**默认 Hs**；
//   Bf 不经 env，仅 set_mode 可传）——宿主（mdio_py/_env.py）启动时按**回退链**注入：
//   `$MDIO_HOME/.env` → `<runtime_root>/.env` → 打包样板 `mdio_py/env.example`
//   （安装态首跑据此落地一份 `$MDIO_HOME/.env`；`MDIO_ENV_DEBUG=1` 可看命中账）；
//   Rust 侧只读标准环境变量，零 .env 解析。
//   切换 = 重启进程（客户端重连收 F 全量帧自愈——827 既有机制）。
//   测试通道 = set_mode（仅测试/诊断；生产代码不得调用）。
//
// 形态：OnceLock（env 惰性初始化一次，无顺序契约）+ ArcSwap（读路径无锁、
// 测试写原子替换）。依赖零新增（arc-swap 已在 mdio_core/Cargo.toml）。

use arc_swap::ArcSwap;
use std::sync::OnceLock;

static MODE: OnceLock<ArcSwap<ProjectionMode>> = OnceLock::new();

/// env 解析（唯一规则）：`MDIO_PROJ_MODE="cf"` → Cf，**否则 Hs（默认）**（Bf 不经 env）。
/// 默认 Hs：MCP／console-script 启动链不读 `.env`，若默认 Cf 会让后端静默落 Cf
/// （同链上无人报错）；要 Cf 请显式 `MDIO_PROJ_MODE=cf`（A/B 仍可用）。
fn mode_from_env() -> ProjectionMode {
    match std::env::var("MDIO_PROJ_MODE").as_deref() {
        Ok("cf") => ProjectionMode::Cf,
        _ => ProjectionMode::Hs,
    }
}

fn mode_slot() -> &'static ArcSwap<ProjectionMode> {
    MODE.get_or_init(|| ArcSwap::from_pointee(mode_from_env()))
}

/// 进程唯一模式读入口。
/// 首次调用惰性从进程环境变量初始化：MDIO_PROJ_MODE="cf" → Cf，否则 Hs（默认）
/// （Bf 不经 env，只能经 set_mode）。生产唯一通道 = 启动环境变量；
/// 生产切换 = 重启进程（客户端重连收 F 全量帧自愈）。
pub fn process_mode() -> ProjectionMode {
    **mode_slot().load()
}

/// 测试/诊断专用（生产切换 = 重启进程；生产代码不得调用）。
/// 调用后同进程所有 doc 立即跟随新模式；不保证与已发出帧的拓扑一致——
/// 活跃客户端持有旧拓扑帧时，后续 F/P 帧会与其不一致（真实产品场景必须重启）。
/// 测试用它模拟"重启后的新进程模式"，配"重连收 F 帧自愈"断言与生产语义对应。
pub fn set_mode(mode: ProjectionMode) {
    mode_slot().store(std::sync::Arc::new(mode));
}

// ── patch 粒度开关(omp 同款:One Granularity Per Process)──────────────
// 语义与 MDIO_PROJ_MODE 同族:进程启动时 env 定一次,生产切换 = 重启;
// 测试经 set_granularity。diff 输出层(diff_level REPL 决策点)消费——
// Block = 现状(整块 REPL 原子);Proj = 面级分诊(ReplaceAttr)。
// 默认 Block:不设 env 时行为与改造前逐字节一致。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    /// 现状:REPL 整块原子
    Block,
    /// 面级:同型 REPL 前按几何切分,恰一单面差 → ReplaceAttr
    Proj,
}

static GRANULARITY: OnceLock<ArcSwap<Granularity>> = OnceLock::new();

/// env 解析(唯一规则):MDIO_PATCH_GRANULARITY="proj" → Proj,否则 Block。
fn granularity_from_env() -> Granularity {
    match std::env::var("MDIO_PATCH_GRANULARITY").as_deref() {
        Ok("proj") => Granularity::Proj,
        _ => Granularity::Block,
    }
}

fn granularity_slot() -> &'static ArcSwap<Granularity> {
    GRANULARITY.get_or_init(|| ArcSwap::from_pointee(granularity_from_env()))
}

/// 进程唯一粒度读入口(env 惰性初始化一次)。
pub fn patch_granularity() -> Granularity {
    **granularity_slot().load()
}

/// 测试/诊断专用(生产切换 = 重启进程;语义同 set_mode)。
pub fn set_granularity(g: Granularity) {
    granularity_slot().store(std::sync::Arc::new(g));
}

/// 容器拓扑(现状行为声明化——回归验证:声明化后逐字节不变)。
/// 候选:全部容器帧(ContainerRecord——expand 建节点)+ 列表分段
/// (ItemSegments——BLT/OL/TableRow 的 push_lines 分组)。
pub static CONTAINER_TOPOLOGY: Topology = Topology {
    name: "cf",
    candidates: &[
        (FrameKind::BlockQuote, CandidateRule::ContainerRecord),
        (FrameKind::BulletList, CandidateRule::ContainerRecord),
        (FrameKind::OrderedList, CandidateRule::ContainerRecord),
        (FrameKind::CodeFence, CandidateRule::ContainerRecord),
        (FrameKind::Table, CandidateRule::ContainerRecord),
        (FrameKind::TableRow, CandidateRule::ContainerRecord),
        (FrameKind::Comment, CandidateRule::ContainerRecord),
        (FrameKind::HtmlBlock, CandidateRule::ContainerRecord),
        (FrameKind::HtmlElement, CandidateRule::ContainerRecord),
        (FrameKind::RefDef, CandidateRule::ContainerRecord),
        (FrameKind::BulletList, CandidateRule::ItemSegments),
        (FrameKind::OrderedList, CandidateRule::ItemSegments),
        (FrameKind::TableRow, CandidateRule::ItemSegments),
    ],
    heading_candidates: &[],
    block_candidates: &[],
    // 壳声明按 FrameKind 索引(10 项——与 CONTAINER_EFFECTS 对齐)。
    // attrs:非 shell 容器 = BLOCK_ATTRS[container_block_type](现状开壳
    // attrs 语义——write_block_attrs 的 BLOCK_ATTRS[t] 引用同一数组);
    // is_shell 容器(BQ/BLT/OL)= 空(不写 attrs)。
    shells: &[
        ShellDecl { nt: "bq", attrs: &[] },
        ShellDecl { nt: "bl", attrs: &[] },
        ShellDecl { nt: "od", attrs: &[] },
        ShellDecl { nt: "c", attrs: BLOCK_ATTRS[BlockType::CodeFenceOpen as usize] },
        ShellDecl { nt: "hb", attrs: BLOCK_ATTRS[BlockType::HtmlBlock as usize] },
        // tb 壳不带 Content 的 ct attr——表头由 cols 承载，
        // ct 键唯一留给 children 通道（HtmlBlock 同款撞键类，6213-6217 先例）。
        ShellDecl { nt: "tb", attrs: TAG_ONLY_ATTRS },
        ShellDecl { nt: "cm", attrs: BLOCK_ATTRS[BlockType::Comment as usize] },
        ShellDecl { nt: "he", attrs: BLOCK_ATTRS[BlockType::HtmlElement as usize] },
        ShellDecl { nt: "rd", attrs: BLOCK_ATTRS[BlockType::RefDef as usize] },
        ShellDecl { nt: "tr", attrs: BLOCK_ATTRS[BlockType::TableRow as usize] },
    ],
    shells_by_block: &[],
    zone: ZoneRule::ContainerClosure,
};

/// 章节拓扑(第二拓扑)。
/// 候选:容器帧**同 CONTAINER_TOPOLOGY**(内容完整——§四.6 同一份文档任何
/// 拓扑下内容提取一致——§5.5 的"排除 Comment/HtmlBlock/HtmlElement/RefDef/
/// TableRow"作废:排除会丢内容——违背第一性约束)+ 标题候选
/// (heading_candidates——Header1-6/HsHead1-2——level 栈构建章节壳)。
/// 壳:容器壳(共享形态——同 CF)+ 章节壳(nt=h——attrs 复用 BLOCK_ATTRS[Header*]
/// ——shells_by_block;Tag 面行内与 CF 同律写在 t)。
pub static HEADINGSECTION_TOPOLOGY: Topology = Topology {
    name: "hs",
    candidates: CONTAINER_TOPOLOGY.candidates,
    heading_candidates: &[
        BlockType::Header1, BlockType::Header2, BlockType::Header3,
        BlockType::Header4, BlockType::Header5, BlockType::Header6,
        BlockType::HsHead1, BlockType::HsHead2,
    ],
    block_candidates: &[],
    shells: CONTAINER_TOPOLOGY.shells,
    // 章节壳:全部标题类型 → h（D5：与 CF 同名同形态）+ attrs 复用块表
    // （D4：M5 惯例，其余 7 条壳皆如此）；内容在章节壳 ct 的块。
    shells_by_block: &[
        (BlockType::Header1, ShellDecl { nt: "h", attrs: BLOCK_ATTRS[BlockType::Header1 as usize] }),
        (BlockType::Header2, ShellDecl { nt: "h", attrs: BLOCK_ATTRS[BlockType::Header2 as usize] }),
        (BlockType::Header3, ShellDecl { nt: "h", attrs: BLOCK_ATTRS[BlockType::Header3 as usize] }),
        (BlockType::Header4, ShellDecl { nt: "h", attrs: BLOCK_ATTRS[BlockType::Header4 as usize] }),
        (BlockType::Header5, ShellDecl { nt: "h", attrs: BLOCK_ATTRS[BlockType::Header5 as usize] }),
        (BlockType::Header6, ShellDecl { nt: "h", attrs: BLOCK_ATTRS[BlockType::Header6 as usize] }),
        (BlockType::HsHead1, ShellDecl { nt: "h", attrs: BLOCK_ATTRS[BlockType::HsHead1 as usize] }),
        (BlockType::HsHead2, ShellDecl { nt: "h", attrs: BLOCK_ATTRS[BlockType::HsHead2 as usize] }),
    ],
    zone: ZoneRule::SectionClosure,
};

/// 第三拓扑(泛化验证——承诺验证:
/// "第三拓扑 = 填一个数据结构"——共享算法(建树/渲染/zone)零改动)。
/// 块平铺拓扑:容器全部透明(ContainerRecord 空——expand 全透明——
/// children 上提——块挂根)+ 块候选白名单(block_candidates——attach
/// 只建声明块类型——渲染跳过语义保留)。演示:同一文档 → 块序列输出。
pub static BLOCKFLAT_TOPOLOGY: Topology = Topology {
    name: "bf",
    candidates: &[],
    heading_candidates: &[],
    block_candidates: &[
        BlockType::Paragraph,
        BlockType::Header1, BlockType::Header2, BlockType::Header3,
        BlockType::Header4, BlockType::Header5, BlockType::Header6,
        BlockType::Bullet, BlockType::OrderedListItem,
        BlockType::ThematicBreak, BlockType::Comment,
        BlockType::HtmlBlock, BlockType::RefDef,
    ],
    // 容器全透明——壳占位(不渲染——shell_for 索引完整 10 项)
    shells: &[
        ShellDecl { nt: "", attrs: &[] },
        ShellDecl { nt: "", attrs: &[] },
        ShellDecl { nt: "", attrs: &[] },
        ShellDecl { nt: "", attrs: &[] },
        ShellDecl { nt: "", attrs: &[] },
        ShellDecl { nt: "", attrs: &[] },
        ShellDecl { nt: "", attrs: &[] },
        ShellDecl { nt: "", attrs: &[] },
        ShellDecl { nt: "", attrs: &[] },
        ShellDecl { nt: "", attrs: &[] },
    ],
    shells_by_block: &[],
    zone: ZoneRule::ContainerClosure,
};
