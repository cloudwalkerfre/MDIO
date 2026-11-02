// @ts-check
/**
 * uiMd —— 层4 服务：声明的翻译官。
 *
 * 声明格式固定（三大块：## mods 登记 / ## match 归属表（wt 级，S1 回迁）/ ## app - appview 谁认领 wt；
 * 可见性从 app 私有区 `### visibility` 读；条目 = bullet，tag=名字、content=位置）。
 * match 下**直列条目**（`- 名字: 路径::选择器`）—— **没有子节、没有桶**。
 * 语义仍是「**这条 header 路径的章归谁画**」：`sec`（以 header 标记的章节，h1–h6 · 任意位置 · 任意深度）
 * 由**自定义件整章渲染**，这是从 sec 诞生起的默认设定；「命中即封闭」由**节点**决定（`isSectionShell(nt)`），不由条目属性决定。
 * 所以提取 = **固定形状走法**：找标题节 → 穿容器拿 bullet 列表 → tag/content 直接 map。
 * 与引擎 :tag 读同义（docs §2.2：有 :tag 结尾默认 content 投影）——JS 侧的 content 面
 * （contentValueOf/contentFaceOf）是「引擎 content 投影」的等价物；需要权威值时以 :tag 读为准（e2e B2/E3 已互证）。
 * 不解析任何文本、不需要后端新接口：声明树 = 打开 /.mds/ui.md 得到的 F 帧交付树。
 * 认领解析：IR 在声明装载时**一次性**编译成 { name, ir }；判定在渲染路径上按**路径**算（`chainWalk`（app 注入）：能力 `apply`
 * 一族的链上求值）——Drill/链式/同层过滤都在那里按祖先链判，因此不需要 fold 与整树扫描（那套已随 P3 删除）。
 * 选择器动态、按条目编译。
 *
 * 纪律（判官 R1/R5）：无静态 import；声明处理路径无 split/正则切条目；不碰全局对象。
 * 节点形状（契约 §4.1）：bullet 的 t = key（名字），ct = 值文本；节节点的 t = 节名。
 */

/**
 * 认知地图（认知统一，读本文件先读这张图）：
 *   树 = 交付 JSON（节点带 nt/t/ct/cd 等键，MST 物化）。
 *   原语只有两个，没有第三个：
 *     ① 遍历（问「在哪 / 是谁」）= apply —— 关系动词（Drill / Wildcard / Recursive 系 / TraversalFilter）
 *        与同层动词（Filter / TYPECHECK / AND / OR / NOT / Slice），本仓库唯一遍历语义；
 *     ② 面（问「它是什么」）= 四张 —— tag 面（textOf，富标题摊平）、content 面（contentFaceOf /
 *        contentValueOf，T4 per-type 三档取法）、field 面（readField）、type 面（isSection / isContainer / nt）。
 *   上层全是组合：sectionOf（声明节查询 = apply 的具名组合）、bulletsOf（apply 组合 + 形态诊断）、
 *     gates / claimMatches（apply 组合 + 策略）、entryOf（tag 面 + content 面）。上层不引入新原语。
 *   引擎契约表四张（引擎定义、本文件逐字留底一份）：TOPOLOGY、TYPE_SPELLINGS、
 *     CONTENT_BY_NT / CONTENT_UNREADABLE、STEP_TAGS。
 */


/**
 * 拓扑常量表 —— 前端**唯一一份**拓扑事实（照 mdio_core/src/topology.rs 物化）。
 * 表里**刻意不写拓扑模式名**：源码规则明令前端不得出现"拓扑模式"相关词汇（规则名"前端无拓扑概念"），
 * 所以这里只留事实（哪些壳穿透、哪个壳是节），模式名不进代码。
 * 四个消费点都读它：穿透外壳 · 类型简写 · 什么算一节 · 壳名表；标准渲染件按契约（库不许 import
 * 应用代码）同源自备一份，规则必须一致。
 * 与引擎的对应：透明壳 = 引擎里"属性为空"的那三只裸壳；节壳 = 章节拓扑下八种标题统一落成的 h。
 */
/**
 * ── 导出的两栏账（波 4 · C3）──
 * **生产面**（app / store / 组件 —— 直接或经读面）：
 *   · 表与拓扑：TOPOLOGY · TYPE_SPELLINGS
 *   · 判定：isCandidate · claimMatches（谓词）· chainWalk（app 注入的递降）
 *   · 面 / 文本：inlineTextOf · contentValueOf · contentFaceOf（内部 kidsOf/descendants/readField 经 makeRead 转交）
 *   · 编译 / 装载：extractDecl · loadDecl · compileMatchTable · compileVisTable · modCompOf
 *   · 路径：pathMatches    · 读面：makeRead（组件唯一读侧入口）
 *   · **经读面暴露给组件**：apply · isTransparentShell · asArray（+ 内部 kidsOf/descendants/readField）
 *     —— 组件**只许** `read.*` 拿它们；**不许** `import ... from "ui_core/uiMd"` 直接用（越层）。
 * **判据面**（仓内生产代码零调用，只有判据/测试在读）：positionalStepOf
 *   —— 不是死码，但**不许被生产新增调用**；要挪用它先挪栏（由 check_no_new_machinery 守）。
 */
export const TOPOLOGY = {
  /**
   * 选择器求值时**穿透**的外壳 = 引擎的**容器壳**（`mdio_core/src/effect.rs:164-178`：`bl`→item `b`、`od`→item `ol`）。
   *  修：`bq` 曾被列在这里 ⇒ `bq` 永远不是候选、`[bq]` 选不中它 —— 而 manual §0 把 `bq`
   * 列为**八种真类型之一**（`[bq] [blockquote]` 可选、可读写），引擎 `BLOCK_TYPE_META` 里 `blockquote` 的
   * `is_skip` 也是 `false`（`effect.rs:330`）。对拍实测：`$ /doc_small.md::..*[bq] -pj ct` 引擎 1 条、前端 0 条。
   */
  transparent: ["bl", "od"],
  /**
   * **注入叶的扣减规则（实测原文）**：每个 item 扣 **1 个内容叶**
   *   —— **不是**「取 item 的首个 `p` 孩子」（那条第 9 处就不成立）：实测有 **9 处首孩子非 p、6 处多 p**；
   *   `ol` 同样要扣。出处：`effect.rs:159-180`（容器→item 叶 b/ol）· `:139` + `:773-775`（LIST_INJECT / is_list_item）。
   */
  /** "一节"的身份：八种标题统一落成 h（**只有这一个**；读到非规范的 sec 即报诊断）。 */
  sectionShell: "h",
  /**
   * 类型名 → 候选壳名值域。**逐条抄自引擎的权威表**，不再自造分组：
   *   · `mdio_core/src/effect.rs:317-365` BLOCK_TYPE_META：debug_name（head/bullet/code/blockquote/ordered_list/
   *     paragraph/comment/table/checklist/indtcode/html/thematic_break/refdef/hs_sep）与 compact_name（h/b/c/bq/ol/p/cm/tr/x/ic/hb/hr/…）；
   *   · `manual.v37.md:30-41` 的过滤器简写（blt/codeblock/orderlist/blockquote/table/paragraph/head1-6…）。
   *  修（对拍实测）：表把 `ol` 映射到 `["ol","bl","od"]`（⇒ `[ol]` 会命中 **bullet 列表壳**）、
   * 把 checklist(`x`) 并进 bullet、把 indtcode(`ic`) 并进 code、并把 `table`/`blockquote`/`blt`/`orderlist`/`headN` 等
   * 引擎/手册里有的名字**漏掉** ⇒ 引擎若发那些名字，`alias[name] || [name]` 会退化成"拿类型名比 nt" ⇒ **静默永不命中**。
   */
  aliases: {
    head: ["h"], h: ["h"],
    bullet: ["b"], b: ["b"], blt: ["b"],
    code: ["c", "ic"], c: ["c", "ic"], codeblock: ["c", "ic"],       // T2：code → {c, ic}（实测 /MDS/mdio_help.md 引擎 ..[code] = 39 = c+ic）
    indtcode: ["ic"], ic: ["ic"],
    blockquote: ["bq"], bq: ["bq"],
    ordered_list: ["ol"], ol: ["ol"], o: ["ol"], orderlist: ["ol"],
    paragraph: ["p"], p: ["p"],
    comment: ["cm"], cm: ["cm"],
    table: ["tb", "tr"], tb: ["tb", "tr"], tr: ["tr"],        // T2：tb/table → {tb, tr}；`tr` 是**表外拼写**（声明侧已被 P0② 红灯拦下）
    checklist: ["b"], x: ["b"],                             // T2：交付树无 nt=x —— checklist 落成 b（勾选态写在 t 上）
    html: ["hb", "he"], hb: ["hb"], he: ["he"],            // T2：html → {hb, he}（全库零实例，仅表对齐）
    thematic_break: ["hr"], hr: ["hr"],
    refdef: ["rd"], rd: ["rd"],
  },
};
export function isTransparentShell(nt) { return TOPOLOGY.transparent.indexOf(nt) >= 0; }
/** 「什么算一节」= 拓扑表那一格，**没有别的入口**（读到非规范的 sec 即报诊断；判据 check_section_source 守）。 */
export function isSectionShell(nt) { return nt === TOPOLOGY.sectionShell; }

/** 节判定与文本（HS 树：节 = 标题节点，标题文本在 tag）。 */
function isSection(node) { return !!node && isSectionShell(node.nt); }
function textOf(node) { return node ? inlineTextOf(node.t) : ""; }
/** 数组读取：真数组与 MST 可观察数组（Array.isArray 对后者为 false）都吃。 */
export function asArray(v) {
  if (Array.isArray(v)) return v;
  if (v && typeof v === "object" && typeof v.length === "number") return Array.prototype.slice.call(v);
  return [];
}
/** 行内 run 数组 → 文本（**富标题/富 tag**：与引擎 `.tag` 逐字相同 —— 逐 run 取文本、无分隔符拼接）。
 *  取值顺序按交付树实际形状：`ct`(string) → `ct`(数组，递归) → `cd`(string，行内 code/数学) → `t`(string)。
 *  为什么必须有它：`h.t` 可以是 run 数组（实测 40 篇样本 310/1101），只吃 string ⇒ **Drill 永不命中**（P0③）。
 *  未覆盖：`im`（图片，core 是 `{s,alt,ti}`）在标题里取不到文本 —— 语料无实例，如实单列。 */
export function inlineTextOf(v) {
  if (typeof v === "string") return v;
  if (Array.isArray(v)) { let out = ""; for (const c of v) out += inlineTextOf(c); return out; }
  if (v && typeof v === "object") {
    if (typeof v.length === "number") return inlineTextOf(asArray(v));   // MST 可观察数组不是真数组
    if (typeof v.ct === "string") return v.ct;
    if (v.ct && typeof v.ct === "object") return inlineTextOf(v.ct);
    if (typeof v.cd === "string") return v.cd;
    if (typeof v.t === "string") return v.t;
  }
  return "";
}

/** 投影字段读取：节点上只带 tag/content 两面（raw/mark/container 是 -pj 读投影、不上节点）；读不到记 missing。 */
function readField(node, field, missing) {
  if (field === ".tag" || field === ".t") { if ("t" in node) return inlineTextOf(node.t); if ("tag" in node) return node.tag; }
  else if (field === ".content" || field === ".ct") { const v = contentFaceOf(node); if (v != null) return v; }   // per-type 面映射（T4）
  missing.add(field);
  return null;
}

/** per-type 面映射（T4）：同一个「面」在不同 nt 上落在不同键。
 *  出处：`effect.rs:484-512` BLOCK_ATTRS（面 → 键）· `topology.rs:227-238` shells（壳无 attrs）· `block.rs:4092-4125` can_read。
 *  取法三档：**直读键**（c/ic → cd）· **重建**（tb = cols + 各行）· **孩子文本**（列表项 b/ol/x 的内容在注入叶）。
 *  `h`/`bq` 的 content 在引擎侧是**整块 raw 正文**（实测 `..[h.t.eq(\"METRICS\")] -pj content` = 该节全部子项的 md 原文）⇒ 树面重建不了，返回 null（不可读），单列为已知差。 */
const CONTENT_BY_NT = { c: "cd", ic: "cd", cd: "cd" };  // 直读：代码内容在 cd（client.js:258 的引擎契约）· 新交付树 inline code 的 nt 直接是 "cd"
const CONTENT_UNREADABLE = ["h", "bq"];                 // 引擎给「整块 raw 正文」，树面重建不了
export function contentValueOf(node) {
  if (!node) return null;
  const nt = node.nt;
  if (CONTENT_UNREADABLE.indexOf(nt) >= 0) return null;
  const key = CONTENT_BY_NT[nt];
  if (key) return typeof node[key] === "string" ? node[key] : null;
  if (nt === "tb") {                                      // 表：表头在 cols、行在 tr.ct（实测形状）
    return {
      head: typeof node.cols === "string" ? node.cols : "",
      rows: asArray(node.ct).map((r) => (typeof r.ct === "string" ? r.ct : flatTextOf(r))),
    };
  }
  if (typeof node.ct === "string") return node.ct;        // 叶子：p/cm/hr/tr/行内 run
  if (typeof node.content === "string") return node.content;
  if (asArray(node.ct).length) return flatTextOf(node);   // 列表项：内容在注入叶（实测 月活用户 → 12345）
  return null;
}
/**
 * 交给组件的**读面** —— 只做一件事：把本文件里**既有**的函数与表**按原名转交**给交付件
 *   （组件不许 import 本文件：分层 + 判官 R1 ⇒ 只能这样递过去）。
 *
 * 设计口径：**不发明新概念**。这里出现的每个名字都是本文件里已有的导出，语义不变：
 *   · 树面：`kidsOf` `descendants` `asArray`
 *   · 值面：`textOf`（名字/标签）· `readField`（按面取结构化值）· `contentValueOf`/`contentFaceOf`
 *   · 求值：`apply`（能力本体）· `walkPrims`（交付面）· `pathMatches`
 *   · 表：`TOPOLOGY` `TYPE_SPELLINGS`（表就是表，不另起分组名）
 *   · 判定/位置**不在这里**：判定在 `Node.get gates`（背后是 store 的 `gates(node)`），链在 `Node.get chain`，
 *     而 `ui` 本来就在组件的 props 里（`NodeView`）⇒ 重复转交就是发明第二套入口。
 * 应用级恒定（NodeView 只造一次）⇒ 放进 props 不影响 memo 的四样。
 */
export function makeRead() {
  return Object.freeze({
    kidsOf, descendants, asArray,
    textOf, readField, contentValueOf, contentFaceOf,
    apply, walkPrims, pathMatches,
    claimMatches,   // 判定谓词（认领/可见性共用）：**能力面**，交给 app 的判定策略用（B2）
    isSectionShell, isTransparentShell,
    TOPOLOGY, TYPE_SPELLINGS,
  });
}

/** 读面的唯一出生点（单源）：NodeView 与装配点都 import 它，不再各造一份。 */
export const READ = makeRead();

/** 摊平写法（判据与过滤用）：就是上面的值按「表头 + 逐行」拼接；**两处同源，别各写一遍**。 */
export function contentFaceOf(node) {
  const v = contentValueOf(node);
  if (typeof v === "string") return v;
  if (!v) return null;
  return [v.head].concat(v.rows).filter((s) => s !== "").join("\n");
}

/** Step 词表（丐版允许的六种；`Slice` 由 positionalStepOf 单独硬拒并给改法）。 */
const STEP_TAGS = ["Drill", "Wildcard", "RecursiveDrill", "RecursiveWildcard", "Filter", "TraversalFilter", "Slice"];
/** TYPECHECK 合法拼写 = T2 的 35 条（逐字抄 `mdio_py/.../mdio_ir.py:131-161`；canonical 集 = `selector.rs:534-554` 的 16 个）。
 *  **实测**：bullet 的树面 nt 集**只有 `b`** —— 交付树里不存在 `nt=x` 的节点，
 *  checklist 落成 `nt=b` 且勾选态写在 `t` 上（`t="x"` = 已勾选）；**5 篇含 2251 个 item 零例外**：引擎 `..*[x]` == `..*[bullet]` == 树 `b` 节点数。
 *  `canonical` 形如 `hN` 时树面 nt 都是 `h`，必须**再按节点 `lv` 过滤**才等于引擎的 `Header(N)`（`selector.rs:591 header_level(t)`）。
 *  为什么必须编译期红灯：**表外拼写在引擎侧静默不过滤**（`..[zzz]` 返回全量），而前端 `alias[name] || [name]` 恒不命中 ⇒ 两条路方向相反。 */
export const TYPE_SPELLINGS = ["b", "blt", "bullet", "x", "checklist", "o", "ol", "orderlist", "ordered_list", "c", "code", "codeblock", "indtcode", "p", "paragraph", "h", "head", "h1", "head1", "h2", "head2", "h3", "head3", "h4", "head4", "h5", "head5", "h6", "head6", "bq", "blockquote", "tb", "table", "comment", "html"];

// **表的唯一 owner 就是本文件的这些常量**（数据 + 出处 + 实测一起在这）。
// `src/tables/{t1_nt,t2_names,t3_values,t4_faces}.json` 不再使用：
//   它们无人 import（不是第二份实现），且知识主体（出处/实测/修法）本来就在本文件的注释里 ⇒ 只把那几张表**仅存**的实测数字
//   （注入叶「每 item 扣 1」的 9/6 计数、bullet 的 `x`→`b` 2251 例）搬进注释后删除。
//    连带影响：`.tmp/p0` 那套读 `src/tables/*` 的对拍脚本（`check_tables.mjs` / `check_tables_two_copies.mjs`）会因此报缺件 —— 那是**判据侧的旧装置**，不是运行时依赖。
// 运行时形态就是上面这些既有常量（`TOPOLOGY` · `TYPE_SPELLINGS` · `CONTENT_BY_NT`/`CONTENT_UNREADABLE`）——
// **不另起分组名**：发明的中间层只会多一套说法。
// 同步由判据保：`.tmp/p0/check_tables_in_repo.mjs`（JSON ↔ 字面量）· `.tmp/apply-tables/check_tables.mjs`（JSON ↔ 引擎）。
// 另一份**逐字节相同的拷贝**在 `.tmp/apply-tables/`（对拍脚本按名读它）——相等由 `.tmp/p0/check_tables_two_copies.mjs` 守，改一边就红。
/** 丐版契约红灯：未知 step / 未知 filter tag / 表外拼写（一次遍历，递归进 filter 内层）。 */
function irProblems(ir, out) {
  for (const st of ir || []) {
    if (!Array.isArray(st)) { out.push("step 不是数组"); continue; }
    const t = st[0];
    if (t === "TraversalFilter") {
      // **约定（A3/B5）**：uimd 的 sel 不许 tf/slice —— 这里在**装载期**报出来（不静默）。
      if (Number(st[3] || 0) > 0) out.push("uimd 的 sel 不许 TraversalFilter(limit>0) —— 推荐精确 tag 名命中");
      if (Array.isArray(st[1])) irProblems([st[1]], out); else out.push("TraversalFilter 缺内层 traversal");
      if (Array.isArray(st[2])) irProblems(st[2], out); else out.push("TraversalFilter 的 filters 不是数组");
      continue;
    }
    if (t === "Slice") { out.push("uimd 的 sel 不许 slice —— 推荐精确 tag 名命中"); continue; }
    if (t === "AND" || t === "OR") { irProblems(Array.isArray(st[1]) ? st[1] : [], out); continue; }
    if (t === "NOT") { irProblems([st[1]], out); continue; }
    if (t === "TYPECHECK") {
      if (typeof st[1] !== "string" || TYPE_SPELLINGS.indexOf(st[1]) < 0) out.push("表外拼写：" + String(st[1]));
      continue;
    }
    if (STEP_TAGS.indexOf(t) >= 0 || LOCAL_FILTER_TAGS.indexOf(t) >= 0) continue;
    out.push("未知 tag：" + String(t));
  }
  return out;
}

/** 行内 run 的 nt：引擎 `node_category` 把它们排除在候选之外（`selector.rs:106-113` 的 Skip 集 + 行内不开节点）。 */
const INLINE_NT = ["txt", "tx", "sb", "lb", "cd", "em", "st", "dl", "lk", "al", "mt", "dm", "im", "rf", "hi", "entity", "text", "math_inline", "math_block", "mb"];
/** 壳（列表外壳）：引擎的 `..` 不含它们；`bq` 是「壳 + 内容叶提升」⇒ **是**候选（实测三条算式里 bq 都在）。 */
const SHELL_NT = ["bl", "od"];
/**
 * 候选面（T1 · 引擎 `node_category` 的**树面等价**）：root / 壳(`bl`,`od`) / 行内 run / `hr` / **注入叶** 都不是候选。
 * 注入叶 = 列表项（`b`/`ol`/`x`）的**第一个**孩子（引擎侧 `LIST_INJECT = [ItemLeaf]` ⇒ 每 item 注入 1 个；
 * 投影到树面就是「每 item 扣 1 个内容叶」）。
 *
 * 实测（3 篇逐 nt 精确对上）：`/dashboard.md` 61 → **29** · `/MDS/mdio_help.md` 1113 → **446** ·
 * `/demos/antd/README.md` 289 → **107**（与引擎 `..` 的命中数逐个相等）。
 */
export function isCandidate(node, parent) {
  if (!node || typeof node !== "object") return false;
  const nt = node.nt;
  if (!nt || nt === "root") return false;
  if (SHELL_NT.indexOf(nt) >= 0 || nt === "hr") return false;
  if (INLINE_NT.indexOf(nt) >= 0) return false;
  if (parent && (parent.nt === "b" || parent.nt === "ol" || parent.nt === "x") && asArray(parent.ct)[0] === node) return false;
  return true;
}

/** 带「面字段」的 filter tag（st[1] 是面名）：字段校验 / 求值 / 白名单共用一处。 */
const FIELD_TAGS = ["CMP", "REGEX", "HAS", "EQ", "RE", "EMPTY"];
/** 与元素位置无关的 filter tag（可就地判定；位置类由 positionalStepOf 另行硬拒）。 */
const LOCAL_FILTER_TAGS = ["TYPECHECK", "AND", "OR", "NOT"].concat(FIELD_TAGS);

function filterOf(step, missing) {
  const t = step[0];
  if (t === "TYPECHECK") {
    const name = step[1];
    return (n) => {
      // 手册两个写法都要吃：`[h1]-[h6]` 与 `[head1]-[head6]`（manual.v37.md:34）。
      const lv = /^(?:h|head)([1-6])$/.exec(name);
      if (lv) return isSectionShell(n.nt) && (n.lv || 1) === Number(lv[1]);
      const alias = TOPOLOGY.aliases;   // 唯一一份（值域见拓扑表里的注释）
      return (alias[name] || [name]).indexOf(n.nt) >= 0;
    };
  }
  if (t === "CMP") {
    const [, field, op, v] = step;
    return (n) => {
      const a = readField(n, field, missing);
      if (a == null) return false;
      if (op === "==" || op === "=") return a === v;
      if (op === "!=") return a !== v;
      const x = Number(a), y = Number(v);
      return !Number.isNaN(x) && !Number.isNaN(y) && (op === ">=" ? x >= y : (op === "<=" ? x <= y : false));
    };
  }
  if (t === "REGEX") {
    const [, field, re] = step;
    return (n) => {
      const a = readField(n, field, missing);
      return a != null && new RegExp(re).test(String(a));
    };
  }
  // ── 方法四式（HAS/EQ/RE/EMPTY）：与 CMP/REGEX 同形、只差算子 ──
  // 语义逐字对齐引擎：selector.rs:708-716 eval_method + :769-781 compare；
  // **不可读面 = 空串**（T3 semantics: unreadable → empty_string）⇒ 四式恒有定义，不再「未知 tag 恒 false」。
  // 注：`.tag` 遇富标题（t 是行内 run 数组）时读到非字符串 ⇒ 现按空串处理，摊平见 P0③。
  if (t === "HAS" || t === "EQ" || t === "RE" || t === "EMPTY") {
    const field = step[1], arg = step[2];
    return (n) => {
      const v = readField(n, field, missing);
      const s = typeof v === "string" ? v : "";
      if (t === "HAS") return s.indexOf(String(arg)) >= 0;                // Value::String ⇒ contains
      if (t === "EMPTY") return s === "";                                 // s.is_empty()
      if (t === "RE") { try { return new RegExp(String(arg)).test(s); } catch (e) { return false; } }
      // EQ：RHS 是字符串 ⇒ 逐字节相等；RHS 是数字 ⇒ parse::<f64>() 再比（parse 失败即 false）
      if (typeof arg === "number") { const x = Number(s); return s !== "" && !Number.isNaN(x) && x === arg; }
      return s === String(arg);
    };
  }
  if (t === "AND") return (n) => step[1].every((s) => filterOf(s, missing)(n));
  if (t === "OR") return (n) => step[1].some((s) => filterOf(s, missing)(n));
  if (t === "NOT") return (n) => !filterOf(step[1], missing)(n);
  return () => false;
}

// 引擎遍历语义：列表容器（bl/od/bq）对选择器**透明**——Wildcard/Drill 的「子节点」
// 指容器内的条目，容器本身不出现在遍历结果里（引擎 node_category 的 Skip 路由）。
const CONTAINER = new Set(TOPOLOGY.transparent);   // 与拓扑表同源（唯一一份）

/** 遍历：容器（bl/od/bq）对选择器透明 —— `看板.*[b]` 不该把容器也算命中（引擎 node_category 的 Skip 路由）。 */
function kidsOf(list) {
  const out = [];
  for (const n of list || []) {
    for (const c of asArray(n.ct)) {
      if (CONTAINER.has(c.nt)) { for (const item of asArray(c.ct)) out.push(item); }
      else out.push(c);
    }
  }
  return out;
}

function descendants(list) {
  const out = [];
  const walk = (nodes) => {
    for (const n of nodes || []) {
      if (CONTAINER.has(n.nt)) { walk(asArray(n.ct)); continue; }
      out.push(n);
      walk(asArray(n.ct));
    }
  };
  walk(list);
  return out;
}

/**
 * **能力本体 · `apply(list, steps)`** —— IR 在交付 JSON 上的求值。**纯函数**（不改入参、不读全局）。
 *
 * 分解（设计目的，别丢）：一次**关系步**（Drill / Wildcard / Recursive*）= **下降一层**
 *   = React 的 `ct` map = IR 向右推进一步：
 *     `apply(list, [s0, s1, …]) ≡ s0(list).flatMap(x => apply(x.ct, [s1, …]))`
 *   ⇒ 纯函数 + 可递归 + **可停在任意前缀**，而**前缀 = 已下降层数 = 渲染已递归到的深度**。
 *   ⇒ `Slice`（对**当前层的列表**取下标/切片）与 `TraversalFilter`（先按内层 traversal 下降、
 *      再在那一层过滤/限量）就落在这条分解里 —— 它们是「层级列表上的操作」，
 *      **不是某个渲染框架的特供**（这就是它能进通用能力的根本）。
 *
 * 动词分两类（词表由引擎定义，本函数只 apply、不编译）：
 *   · **关系步**：Drill · Wildcard · RecursiveDrill · RecursiveWildcard · TraversalFilter —— 下降一层（容器透明）
 *   · **同层步**：Filter / TYPECHECK / AND / OR / NOT（`LOCAL_FILTER_TAGS`）· Slice —— 作用在**当前列表**上
 * 返回 `{ hits, missing }`；不认识的动词 → `{ hits: [], missing, gate }`（交给调用方，不猜、不静默）。
 */
export function apply(list, steps) {
  const missing = new Set();
  let cur = Array.isArray(list) ? list : (list && typeof list.length === "number" ? Array.from(list) : []);
  for (const st of steps || []) {
    if (!Array.isArray(st)) return { hits: [], missing, gate: "step-not-array" };
    const t = st[0];
    if (t === "Drill") cur = kidsOf(cur).filter((n) => textOf(n) === st[1]);
    else if (t === "Wildcard") cur = kidsOf(cur);
    else if (t === "RecursiveWildcard") cur = descendants(kidsOf(cur));
    else if (t === "RecursiveDrill") cur = descendants(kidsOf(cur)).filter((n) => isSection(n) && textOf(n) === st[1]);
    else if (t === "TraversalFilter") {
      const inner = apply(cur, [st[1]]);
      for (const k of inner.missing) missing.add(k);
      if (inner.gate) return { hits: [], missing, gate: inner.gate };
      const f = filterOf(st[2].length === 1 ? st[2][0] : ["AND", st[2]], missing);
      const hit = inner.hits.filter((n) => f(n));
      const Ln = Number(st[3] || 0);
      cur = Ln > 0 ? (hit.length >= Ln ? [hit[Ln - 1]] : []) : hit;
    } else if (t === "Slice") {
      cur = sliceList(cur, st[1]);
    } else if (LOCAL_FILTER_TAGS.indexOf(t) >= 0) {
      cur = cur.filter((n) => filterOf(st, missing)(n));
    } else {
      return { hits: [], missing, gate: t };
    }
  }
  return { hits: cur, missing };
}
/**
 * **交付面 · 链上求值原语**（跟读单 A2）：**「递降」是 app 对能力 `apply` 的应用**，框架只给原语与口子。
 *   给这些就够：能力 `apply` · 过滤器构造 `filterOf` · 名字面 `textOf` · 节判定 `isSection` ·
 *   容器判定 `isContainer`（`TOPOLOGY.transparent` 的唯一一份）· 同层步词表 `localFilterTags`。
 *   为什么连 `filterOf` 一起给：不给就得让 app 再写一遍过滤 —— 那就是第二套。
 */
export const walkPrims = {
  apply,
  filterOf,
  textOf,
  isSection,
  isContainer: (nt) => CONTAINER.has(nt),
  localFilterTags: LOCAL_FILTER_TAGS,
};

/** `Slice` = **对当前层的列表**取下标/切片（`[n]` / `[a:b]` / `RANGE`；负数自尾部数）。 */
function sliceList(cur, spec0) {
  const spec = Array.isArray(spec0) ? spec0 : [spec0];
  const out = [];
  for (const it of spec) {
    if (Array.isArray(it) && it[0] === "RANGE") {
      const a = it[1], b = it[2];
      out.push(...cur.slice(a == null ? 0 : a, b == null ? cur.length : b));
    } else if (typeof it === "number") {
      const k = it < 0 ? cur.length + it : it;
      if (cur[k]) out.push(cur[k]);
    }
  }
  return out;
}

/**
 * **求值谓词 · 链上求值**（归属定案）：**谓词留框架**（认领与可见性共用同一处命中：路径门控 + 候选面 + 链上位移），
 *   **策略归 app**（谁赢 / 可见规则在 `ui_std/src/claimEval.js`，经 `setClaimEval` 注入，见 `services/claimEval.js`）。
 *   即：本文件是**能力与谓词**，app 是**策略与用法** —— 跟读单 B2「认领与可见性归 `basic_render`」的落地形态。
 *   · **关系步**（Drill · Wildcard · RecursiveDrill · RecursiveWildcard · TraversalFilter）= 沿链**下降**（容器透明的 gap 规则与引擎一致）；
 *   · **同层步**（Filter/TYPECHECK/AND/OR/NOT）= **交给能力 `apply`**（同一套原语，不另写一遍过滤）；
 *   · `opts.containers === true` ⇒ 「**容器跟孩子走**」：本节点是容器时，任一祖先是落点也算（可见性用；认领不用）。
 * 返回 `true/false`；**空 IR ⇒ `null`**（「没有选择器就没有判定」，是语义不是错）；
 * **畸形 step / `Slice` / `TraversalFilter(limit>0)` / 内层不是 traversal / 未知动词 ⇒ 抛错** ——
 *   A3 收口：装载期已**硬拒不参战**（`irProblems` + `positionalStepOf`），这里再响亮一次，**绝不静默**；
 *   正常声明永远不会走到（活声明的两条选择器都是「按名下钻」形态）。
 */



/** **content 面的文本摊平**（内部实现：contentValueOf 三档取法里的「重建/注入叶」共用它；对外只认
 *  contentFaceOf / contentValueOf / readField(".content")）。
 *  字符串直取；内联子节点数组则按「cd 先于 ct」拼文本。
 *   保留手写递归的理由（apply 化专项）：cd/ct 字符串是**终止节点**（贡献一次文本后不下钻），
 *  apply 的动词（Drill / Wildcard / Recursive 系 / Filter / Slice）是「遍历/选择」语义，表达不了「遇 cd 停钻」的投影规则——
 *  硬套 RecursiveWildcard 会把终止节点的子孙文本也并进来（语义变）。 */
function flatTextOf(node) {
  if (typeof node.ct === "string") return node.ct;
  if (typeof node.content === "string") return node.content;
  if (asArray(node.ct).length) {
    const parts = [];
    const walk = (n) => {
      // cd 先于 ct：MST 模型会给每个节点注入 ct="" 默认值，code span 的内容在 cd——
      // ct 先判会把空串当文本吃走，cd 永远读不到（实测：std 认领 sel 变空）。
      if (typeof n.cd === "string" && n.cd !== "") { parts.push(n.cd); return; }
      if (typeof n.ct === "string") { parts.push(n.ct); return; }
      if (typeof n.t === "string") parts.push(n.t);
      for (const c of asArray(n.ct)) walk(c);
    };
    for (const c of node.ct) walk(c);
    return parts.join("");
  }
  return "";
}

/** bullet 节点 → 条目（tag 面 + content 面的组合）：名字 = tag（富标题摊平），内容 = content 文本（trim）。 */
function entryOf(node) {
  const name = textOf(node).trim();
  const spec = flatTextOf(node).trim();
  return { name, spec, node };
}

/** 位置 "path::SEL" → { path, sel }（第一个 :: 切开；不用 split，按 indexOf 找一次）。 */
function splitAddr(spec) {
  const s = String(spec == null ? "" : spec);
  const i = s.indexOf("::");
  if (i < 0) return { path: s, sel: "" };
  return { path: s.slice(0, i), sel: s.slice(i + 2) };
}

/** 交付树里找标题为 name 的节（HS 树：标题节 t = 节名）。
 *
 * **修（实测事故）**：声明文件正常以 **`# UI` 一级标题开头**，此时 `## mods` / `## match`
 * 是**孙节**；只看直接子节 ⇒ 整份声明解析成 0 条（实测 `decl mods=0 match=0` ⇒ 没有 std mod
 * ⇒ 文档面空白 ⇒ SOP 的 S3/S8 红）。这里补**一层下探**（只一层：避免误抓正文里的同名小节）。
 */
/** **doc 值**（B4）：无 `::` 且形如 `/…md` ⇒ 这个 key 的值是**另一篇 md 的地址**（不是当前树里的选择器）。 */
const MD_ADDR = /^\/.*\.md$/;
export function docAddrOf(spec) {
  const s = String(spec == null ? "" : spec).trim();
  return (s.indexOf("::") < 0 && MD_ADDR.test(s)) ? s : null;
}

/** **app 私有区 `### match` → 认领条目**（形状与 `extractDecl` 的 `## match` 一致：`{ name, path, sel }`；
 *  doc 值 → `{ name, doc, merge: true }`，由装配点**展开一层**（读那篇 md 的 `### match` 并并入）。 */
export function matchEntriesOf(section, errors) {
  const out = [];
  if (!section) return out;
  for (const it of bulletsOf(section, "match", errors || [])) {
    const e = entryOf(it.node);
    if (!e.name) continue;
    const doc = docAddrOf(e.spec);
    if (doc) { out.push({ name: e.name, doc: doc, path: null, sel: null, merge: true }); continue; }
    const a = splitAddr(e.spec);
    out.push({ name: e.name, path: a.path, sel: a.sel });
  }
  return out;
}

/** **app 私有区 `### visibility` → `{ include, exclude }`**（与 `extractDecl` 的 `## visibility` 同形）。 */
export function visEntriesOf(section, errors) {
  const vis = { include: [], exclude: [] };
  if (!section) return null;
  for (const it of bulletsOf(section, "visibility", errors || [])) {
    const e = entryOf(it.node);
    if (e.name !== "include" && e.name !== "exclude") continue;
    const docV = docAddrOf(e.spec);
    if (docV) { (errors || []).push({ name: e.name, at: "app visibility · " + e.name, msg: "visibility 不支持 doc 值（" + docV + "）：可见性是本档的表态，不并入别的表" }); continue; }
    vis[e.name].push(splitAddr(e.spec));
  }
  return (vis.include.length || vis.exclude.length) ? vis : null;
}

/** 表 md 引用收集（纯，实施 Plan 步 4 从装配点收编）：
 *  三来源（顶层 ## match / app 私有区 ### match / 各 mod 私有区 ### match）的 doc 值，去重返回；
 *  条目解析错误经 errors 输出（调用方账）。 */
export function collectTableRefs(decl, appSection, modSectionOf, errors) {
  const refs = [];
  const push = (entries) => { for (const e of entries || []) if (e && e.doc) refs.push(String(e.doc)); };
  {
    const m0 = appSection ? sectionOf(appSection, "match") : null;
    push(matchEntriesOf(m0, errors || []));
  }
  for (const m of (decl && decl.mods) || []) {
    const s1 = typeof modSectionOf === "function" ? modSectionOf(m.name) : null;
    const m1 = s1 ? sectionOf(s1, "match") : null;
    push(matchEntriesOf(m1, errors || []));
  }
  push((decl && decl.match) || []);
  return Array.from(new Set(refs));
}

/** 取节（声明节查询 = apply 的具名组合：Drill 直子节 + 仅 lv1 标题层一层下探）。
 *  mod 私有区的取节入口（范式 §63：mod 标题下的小节由该 mod 自己读）。 */
export function sectionOf(tree, name) {
  // ① 直子节：Drill 按名下钻（apply 的容器透明 = 引擎遍历语义），补节判定（与旧语义一致：只认节不认条目）。
  const direct = apply([tree], [["Drill", name]]).hits.find((n) => isSection(n));
  if (direct) return direct;
  // ② **只下探「文档标题」那一层**（`# UI` 把 `## mods` 包成孙节的写作习惯；为此加的下探）。
  // **不再下探进 mod/app 私有区**（`## <MOD>`）：否则顶层 `## match` 的解析会钻进 app 私有区读到它的 `### match`，
  // 于是**同一条错被报两遍**、`decl.match` 还混进 app 的表（C7-③ 的分类计数把它照了出来）。
  const tops = apply([tree], [["Wildcard"]]).hits.filter((n) => isSection(n) && (n.lv || 1) === 1);
  const deep = apply(tops, [["Drill", name]]).hits.find((n) => isSection(n));
  return deep || null;
}

/**
 * 节下的条目列表：穿透 `bl`/`od`/`bq` 容器（引擎遍历语义），**条目直接列在本节下**。
 *
 * **子节一律不认**（没有桶）：`### node` / `### section` / `sec` ⇒ 大声报错并跳过该子节。
 * 语义没有变：一条 match 条目仍然是「**这条 header 路径的章 ⇒ 由我的件整章渲染**」；
 * 变的是**写法**（不再用一个子节名去声明"我这条是区域级"——那件事本来就由节点决定：`isSectionShell(nt)`）。
 *
 * 返回 [{ node, at }]：at = "## <节> · 第 N 条"（错误定位用）。
 */
function bulletsOf(section, sectionName, errors) {
  const out = [];
  let seen = 0;   // 条目序位：错误要能报到"哪一节、第几条"（research/2026-09-14-synthesis 轮2 第3条：只丢坏的那一节并报出位置）
  const at = (what) => "## " + sectionName + " · 第 " + seen + " 条" + (what ? "（" + what + "）" : "");
  // 遍历收进 apply：Wildcard 一步取齐节下条目（容器透明 = 引擎遍历语义，kidsOf 的 Skip 路由）。
  for (const c of apply([section], [["Wildcard"]]).hits) {
    if (isSection(c)) {
      // **没有桶**：条目直接列在本节下，没有子节。
      // 留着 `### node`/`### section` 的旧声明 ⇒ **大声报错**（不静默当没看见），并跳过该子节。
      seen += 1;
      errors.push({ kind: "schema", name: textOf(c) || "(子节)", at: at("子节"), msg: "声明形态不认：" + sectionName + " 下不再有子节（桶已删）—— 把条目直接列在 " + sectionName + " 下；已跳过该子节" });
      continue;
    }
    if (c.nt === "b" || c.nt === "x") { seen += 1; out.push({ node: c, at: at(null) }); continue; }
    seen += 1;
    errors.push({ kind: "schema", name: textOf(c) || ("(" + (c.nt || "?") + ")"), at: at(null), msg: "声明形态不认：" + sectionName + " 下出现非 bullet 条目（nt=" + (c.nt || "?") + "），已跳过" });
  }
  return out;
}

// `bucketOfName`（子节名 → 桶）不存在：桶是实现期发明的中间概念（只有「认领 = 路径谓词」一条）。
// 「节用自定义件渲染」是 **`sec` 的原始设定**，与桶无关 —— 它由节点决定（`isSectionShell(nt)`），不由条目属性决定。

/**
 * 声明交付树 → 三张原始表 + 形态诊断（固定形状走法，零文本解析、零 IR）。
 *
 * match 条目 = `- 名字: 路径::选择器`（**没有桶**；子节是旧写法 ⇒ 红灯）。
 */
export function extractDecl(tree) {
  const mods = [];
  const match = [];

  // 「## app」= 谁认领 wt（appview 槽位键；## router 键已迁入此）。
  const app = { appview: "" };
  const errors = [];
  const warns = [];
  if (tree) {
    const secMods = sectionOf(tree, "mods");
    const secMatch = sectionOf(tree, "match");
    const secVis = sectionOf(tree, "visibility");
    const secRouter = sectionOf(tree, "router");
    const secApp = sectionOf(tree, "app");
    if (secMods) for (const it of bulletsOf(secMods, "mods", errors)) {
      const e = entryOf(it.node);
      if (!e.name) continue;
      // mods 的 path 形式：`- name: /doc.md::<位置>`——只有**以 / 开头**的值才按 path 切；
      // `md:/…`、`ui:…`、`http:…` 是位置本身（md: 的源文档路径由装载线用 exec 读，不在这里切）。
      // **写 wt app 的简化形式**（DX）：`- NAME: /myapp.md`（只有文档路径、没有 `::`）
      //   ⇒ 自动展开成「该文档里的 ```jsx@app 块」。第三方开发者因此**不必学 mdio 选择器**。
      //   约定：一个 app 文档里只放一块 ```jsx@app；缺块/多块由装载线记诊断（不在这里猜）。
      if (e.spec.startsWith("/") && e.spec.indexOf("::") < 0 && /[.]md$/.test(String(e.spec).trim())) {
        const p = String(e.spec).trim();
        mods.push({ name: e.name, path: null, spec: ".md::" + p + "::..*_c[c.t==\"jsx@app\"]", node: e.node });
        continue;
      }
      if (e.spec.startsWith("/") && e.spec.indexOf("::") > 0) {
        const a = splitAddr(e.spec);
        mods.push({ name: e.name, path: a.path, spec: a.sel, node: e.node });
        continue;
      }
      mods.push({ name: e.name, path: null, spec: e.spec, node: e.node });
    }
    // **顶层 `## match` 回 wt 级**：认领表 = wt 级归属表，来源 = 本节。
    if (secMatch) for (const it of matchEntriesOf(secMatch, errors)) match.push(it);
    // 顶层 `## visibility` 不是三大块之一；可见性仍从 app 私有区 `### visibility` 读。
    if (secVis) errors.push({ kind: "schema", at: "## visibility", msg: "顶层 `## visibility` 仍退役：可见性从 app 私有区 `## <APP> ### visibility` 读（## match 已于 2026-10-11 回 wt 级）" });
    // ## router 键已迁入 ## app；写即红灯。
    if (secRouter) errors.push({ kind: "schema", at: "## router", msg: "`## router` 节已退役（2026-10-11）：appview 键迁入 `## app`（值要写 mods 里的名字）" });
    // ## app 节：键集 = appview（与旧 ## router 同款校验）。
    if (secApp) for (const it of bulletsOf(secApp, "app", errors)) {
      const e = entryOf(it.node);
      if (!e.name) continue;
      if (e.name !== "appview") { errors.push({ kind: "schema", name: e.name, at: "## app · " + e.name, msg: "app 节不认的键：" + e.name + "（只认 appview）" }); continue; }
      if (!e.spec) { errors.push({ kind: "schema", name: "appview", at: "## app · appview", msg: "appview 的值为空（要写 mods 里的名字）" }); continue; }
      app.appview = e.spec;
    }
  }
  return { mods, match, app, errors, warns };   // 三大块：mods 登记 · match 归属表（wt 级）· app 谁认领 wt
}

/**
 * 装载声明（mods 循环）。deps = { declTree, load }；load = loader.js 的 load(name, spec, deps)。
 * 返回 { decl, mods, errors }。
 */
export async function loadDecl(deps) {
  const d = deps || {};
  // decl 优先(投影 getter 已算好);declTree 直传时此处同步投影(兼容旧调用)。
  const decl = d.decl != null ? d.decl : (d.declTree ? extractDecl(d.declTree) : null);
  if (!decl) {
    return { decl: { mods: [], match: [], app: { appview: "" }, errors: [], warns: [] }, mods: new Map(), errors: [{ name: "(decl)", msg: "没有声明交付树" }] };
  }
  // 「std 隐式兜底」不存在：声明里写什么就装什么 —— 谁要用 std 就在自己的 `## mods` 里写
  //   `- std: ui_std/std`（三个 wt 的声明已迁到显式写法）。没有兜底、没有暗补：装载面 = 声明面。
  const errors = (decl.errors || []).slice();   // 形态诊断（未认形态）与装载失败同一本账
  const mods = new Map();
  for (const m of decl.mods) {
    if (!m.spec) { errors.push({ kind: "schema", name: m.name, at: "## mods · " + m.name, msg: "位置为空（节点形状：" + JSON.stringify(m.node).slice(0, 160) + "）" }); continue; }
    if (!d.load) { errors.push({ kind: "wiring", name: m.name, at: "## mods · " + m.name, msg: "未注入 load（装配点该给 load 却没给）" }); continue; }
    try {
      mods.set(m.name, await d.load(m.name, m.spec));
    } catch (e) {
      errors.push({ kind: "wiring", name: m.name, at: "## mods · " + m.name, msg: "取件失败：" + String((e && e.message) || e).slice(0, 120) });
    }
  }
  // 值的红灯（app 的 appview）：值必须是 mods 里的名字，且那个件必须能画
  //（判据与 compFor / renderClaims 同源：modCompOf 只认 default/Component 函数）。不满足 ⇒ 记诊断；装配点同时回退 DocPane。
  {
    const av = decl.app && decl.app.appview;
    if (av) {
      if (!decl.mods.some((m) => m.name === av)) {
        errors.push({ kind: "wiring", name: "appview", at: "## app · appview", msg: "值不在 ## mods 里：" + av + "（appview 的值必须是 mods 里的名字）" });
      } else {
        const rec = mods.get(av);
        if (rec && !modCompOf(rec)) errors.push({ kind: "wiring", name: "appview", at: "## app · appview", msg: "被指名的件不能画（没有 default/Component 函数）：" + av });
      }
    }
  }
  return { decl, mods, errors };
}

/**
 * 装载记录 → **渲染组件**（"什么算这个 mod 能画一个节点"的唯一判据）。
 *
 * 为什么必须单独一条：mods 里有两类件 —— **渲染件**（导出 `default`/`Component` 函数的组件）与
 * **非渲染件**（CSS/副作用 mod：只导出 `sheet` 之类）。两类都能进 `## mods` 与 `## match`
 * （§13.6「声明即装载」；match 对样式件是**作用域**语义），但只有前者能占"谁画这个节点"的槽。
 * 若让非渲染件占住该槽：框架按"认领即自画"不再补画节的正文与子节，而它又交不出组件
 * ⇒ **整篇文档只剩标题**（实测：genesis 树的 `- THEME: ::..` 把 `std` 顶掉，
 * `/.mds/ui.md` 与 `/demos/README.md` 都只渲染出标题，bullet 与子节全丢）。
 * 消费点两处、同源：`uiStore.renderClaims`（认领表）与 `compTable.compFor`（router）。
 * 判据：`scripts/check-doc-render.mjs`（活体：文档面渲染出的 li 数 = 交付树里的 bullet 数）。
 */
export function modCompOf(rec) {
  const ns = rec && (rec.ns || rec);
  const C = ns && (ns.default || ns.Component);
  // 「能画」= **合法的 React 元素类型**：
  //   · 函数（普通组件 / 类组件 / 未包装的原件）；
  //   · 带 `$$typeof` 的对象（memo / forwardRef / lazy）—— `mobx-react-lite` 的 `observer(...)`
  //     返回的正是 memo 对象 ⇒ 旧判据（只收函数）会把「入口件必须自己 observer」这条计划要求判成**不能画**（实测踩到）。
  // 判据仍然只有这一处：compFor / renderClaims / appview 取值校验都问它。
  if (typeof C === "function") return C;
  if (C && typeof C === "object" && C.$$typeof) return C;
  return null;
}

// ══ 认领解析（渲染内：编译一次，渲染时逐节点解析，无每帧重建的表） ══

const NODE_FIELDS = [".tag", ".t", ".content", ".ct"];   // 节点上实有的字段；其余（raw/mark/container）是 -pj 读投影、不上节点

// 编译只在**声明装载**时发生（不是热路径），不按 sel 文本缓存。
//   缓存那点收益换不来一层机制；IR 只依赖 sel 文本、纯本地、无副作用 ⇒ 直接调即可。

/** 收集 IR 过滤字段（编译期校验：只放行节点实有字段；其余大声拒——读投影不是节点字段）。 */
function fieldsOf(ir, out) {
  for (const st of ir || []) {
    if (!Array.isArray(st)) continue;
    if (FIELD_TAGS.indexOf(st[0]) >= 0 && typeof st[1] === "string") out.push(st[1]);
    else if (st[0] === "AND" || st[0] === "OR") fieldsOf(st[1], out);
    else if (st[0] === "NOT") fieldsOf([st[1]], out);
    else if (st[0] === "TraversalFilter") { fieldsOf([st[1]], out); fieldsOf(st[2] || [], out); }
  }
  return out;
}

/**
 * 「登记条目」（全类型）：sel 只有一步 \`..\`（递归全部）⇒ 具体度最低 ⇒ 排在 node 组末尾。
 * 这就是设计稿里那句「**std 恒为末位**（写了也排最后）」的落地形态 ——
 * 登记条目参战的范围是"任意文档、所有节点"，任何写清谓词的条目都比它具体，理应先赢。
 */
function isBlanketIR(ir) {
  return Array.isArray(ir) && ir.length === 1 && Array.isArray(ir[0]) && ir[0][0] === "RecursiveWildcard";
}

/**
 * 判定语言收窄（方案 §5.2）：位置/序号类步骤**在路径上判不出来**——"同一层里第 n 个"问的是
 * "前面已经匹配到几个"，与深度无关，走到某个节点时手里没有这个信息。所以**编译期硬拒**并给改法。
 * 只圈声明面：命令行地址里的 `:key`（等价于 limit=1）不受影响（那些地址不走这条编译路径）。
 */
export function positionalStepOf(ir) {
  for (const st of ir || []) {
    if (!Array.isArray(st)) continue;
    if (st[0] === "Slice") return "Slice";
    if (st[0] === "TraversalFilter" && Number(st[3] || 0) > 0) return "TraversalFilter(limit=" + Number(st[3]) + ")";
  }
  return null;
}

// **没有分桶**：表序 = **声明序**（`claimAt` 从尾往前扫 ⇒ 后写覆盖）。
// 为什么不需要分组：只有「认领 = 路径谓词」一条；「谁更具体」是**声明的写法**（兜底条目写在前面），
// 不是框架的排序魔法。三份 wt 声明已按等价序重排（node 组的条目在前、section 组的条目在后）。

/**
 * 这条 IR 会不会读**内容文本**（.content / .ct）。命中集时代的用途是「内容档要不要换钥匙」。
 *  命中集与扫描不存在 ⇒ 生产侧**已无消费者**（demo 也不再传 usesContent）；留作词汇/将来复用。
 * 为什么这是个有价值的判据（实测）：
 * 而声明里绝大多数条目只按类型/标签/位置选（\`::..\`、\`::DASHBOARD.引用\`），**不受内容文本影响**。
 * 只要没有条目读内容，"改一个 bullet 的值"就不该触发重扫（数据写 = 纯内容变化）。
 */
// `irUsesContent`（这条 IR 读不读内容面）不存在：命中集与扫描没了，它就没有消费者。

/**
 * 选择器编译缓存（按 sel 文本）：IR 是**纯函数**。b3de2941「施工图 B3」删过这份缓存，
 * 理由是「编译只发生在声明装载时，不是热路径」——但声明热重载（改 ui.md / 改 md 件）每轮都会
 * 重编同一批选择器，恰恰是热路径（实测：一次声明重载 46 次 sel compile，占大头耗时）。
 * 缓存 promise：同 sel 并发只发一次；失败即删（下次重试）。
 */
export function cachedCompileSel(compileSel) {
  const cache = new Map();
  return async (sel) => {
    if (cache.has(sel)) return cache.get(sel);
    const p = Promise.resolve(compileSel(sel));
    cache.set(sel, p);
    try { return await p; } catch (e) { cache.delete(sel); throw e; }
  };
}

/** 声明装载时**一次性**编译：每条 match → { name, path, ir }（表序 = 声明序）；编译/字段问题进 errors。 */
export async function compileMatchTable(decl, compileSel, modPaths, entries) {
  const table = [];
  const errors = [];
  const warns = [];
  // **S1**：条目来源**必须传入**（= 顶层 `## match` 解析出来的那批，wt 级归属表）；缺 ⇒ 空表。
  // **缺席是正常态**（没写 `## match` ⇒ 空表，整篇走默认渲染器）：不报灯。
  //   只有「## visibility 旧写法」和「条目本身写错」才报（前者在 extractDecl，后者在下面逐条校验）。
  for (const m of entries || []) {
    const own = m.path && String(m.path).trim();
    const fromMod = modPaths && typeof modPaths.get === "function" ? modPaths.get(m.name) : null;
    const path = own || fromMod || null;   // path 参与扫描门控（与 vis 同语义；null = 任意文档）
    if (m.doc) {
      errors.push({ kind: "schema", name: m.name, at: "## match · " + m.name, msg: "认领表不接受 doc 值（" + m.doc + "）：认领 = 某个 mod 画某个节点；doc 值只作「并入另一张表」的引用，应由装配点先展开" });
      continue;
    }
    if (!m.sel) {
      // **没有选择器就没有判定**：IR 为空 ⇒ `chainWalk` 返回 null ⇒ 永不命中。
      // 没有桶，也就没有「文档级表态」这个**半截机制**（它当年 push 了一条 `ir: []` + `bucket: "doc"`，
      //   而空 IR 判不出任何节点 ⇒ 从来没生效过）。要整篇表态就写 `::..`（全类型通配）——一条真实的 IR。
      errors.push({ kind: "schema", name: m.name, at: "## match · " + m.name, msg: "match 条目缺选择器（IR 为空 ⇒ 永不命中）：整篇表态写 `::..`；某一段写 `文件段::标题路径`。已跳过该条" });
      continue;
    }
    try {
      const ir = await compileSel(m.sel);
      if (!Array.isArray(ir)) { errors.push({ kind: "schema", name: m.name, at: "## match · " + m.name, msg: "sel 编译结果不是 Step[]" }); continue; }
      const probs = irProblems(ir, []);
      if (probs.length) { errors.push({ kind: "schema", name: m.name, at: "## match · " + m.name, msg: "不合丐版契约（红灯）：" + probs.join("；") }); continue; }
      const used = fieldsOf(ir, []);
      const badF = used.filter((f) => NODE_FIELDS.indexOf(f) < 0);
      if (badF.length) { errors.push({ kind: "schema", name: m.name, at: "## match · " + m.name, msg: "IR 用到的字段不上节点（raw/mark/container 是 -pj 读投影）：" + badF.join(",") }); continue; }
      // 语言收窄：位置/序号类写法硬拒（不回退到全树扫描）
      const pos = positionalStepOf(ir);
      if (pos) { errors.push({ kind: "schema", name: m.name, at: "## match · " + m.name, msg: "位置/序号类写法在路径上判不出来（" + pos + "）⇒ 不参战。改法：给目标节点起名字/标题/标识，用「按名下钻」或「按字段筛选」表达；工具地址不受此限" }); continue; }
      // （判定走 claimMatches → chainWalk（app 注入））
      table.push({ name: m.name, path, ir });   // 判定走 claimMatches → chainWalk（app 注入）
    } catch (e) {
      errors.push({ kind: "schema", name: m.name, at: "## match · " + m.name, msg: "sel 编译失败（不回填）：" + String((e && e.message) || e).slice(0, 120) });
    }
  }
  return { table, errors, warns };   // 表序 = 声明序
}

/**
 * 声明装载时**一次性**编译 visibility 表：与 compileMatchTable **同一套**编译、同一套字段校验，
 * 只有产物用途不同（match 决定「谁画」，vis 决定「画不画」）。
 * 地址形状本来就同源（extractDecl 里两者都走 splitAddr）⇒ 不引第二套机制。
 */
export async function compileVisTable(decl, compileSel, entries) {
  const table = [];
  const errors = [];
  // **B2**：同 `compileMatchTable` —— 条目来源可传入（app 私有区 `### visibility`），不传则顶层 `## visibility`。
  const vis = entries;
  // **缺席是正常态**（app 没写 `### visibility` ⇒ 空表 = 全可见）：不报灯。
  if (!vis) return { table, errors };          // 无 visibility 节 ⇒ 空表（渲染面零变化）
  for (const name of ["include", "exclude"]) {
    for (const a of vis[name] || []) {
      const sel = String((a && a.sel) || "").trim();
      const p0 = String((a && a.path) || "").trim();
      // 两种 vis：① 节点级 = path::SEL（编译 IR，渲染面用）② **文档级** = 只有 path（sel 空）⇒ 归 docScope
      if (!sel) {
        if (!p0) { errors.push({ kind: "schema", name: "visibility/" + name, msg: "path 与 sel 都为空，无法判定作用域" }); continue; }
        table.push({ name, path: p0, sel: "", ir: null, docLevel: true });
        continue;
      }
      try {
        const ir = await compileSel(sel);
        if (!Array.isArray(ir)) { errors.push({ kind: "schema", name: "visibility/" + name, msg: "sel 编译结果不是 Step[]" }); continue; }
        const probs = irProblems(ir, []);
        if (probs.length) { errors.push({ kind: "schema", name: "visibility/" + name, msg: "不合丐版契约（红灯）：" + probs.join("；") }); continue; }
        const badF = fieldsOf(ir, []).filter((f) => NODE_FIELDS.indexOf(f) < 0);
        if (badF.length) { errors.push({ kind: "schema", name: "visibility/" + name, msg: "IR 用到的字段不上节点：" + badF.join(",") }); continue; }
        const posV = positionalStepOf(ir);
        if (posV) { errors.push({ kind: "schema", name: "visibility/" + name, msg: "位置/序号类写法在路径上判不出来（" + posV + "）⇒ 该条不参战（改法与 match 同）" }); continue; }
        table.push({ name, path: a.path, sel, ir });
      } catch (e) {
        errors.push({ kind: "schema", name: "visibility/" + name, msg: "sel 编译失败：" + String((e && e.message) || e).slice(0, 120) });
      }
    }
  }
  return { table, errors };
}

/**
 * match trait 的编译入口（「mods 认领 node 的固定范式」§63/§64）：
 *   取节 = mod 标题树下的 `match` 小节；条目语法与 compileMatchTable 一致（`名字: path::sel`），
 *   `_HIDDEN` 是保留宏（命中 = 显式不画）；sel 必填（整篇表态写 `::..`）；
 *   path 允许省略（null = 该 mod 自己管；本表不参与 path 门控 —— 门控由调用方保证）。
 * 产物形状与 compileMatchTable 相同：{ name, path, ir }（表序 = 声明序）。
 */
export async function compileModMatch(modTree, compileSel) {
  const table = [];
  const errors = [];
  const sec = modTree ? sectionOf(modTree, "match") : null;
  if (!sec) return { table, errors };   // 不写小节 = 不用 trait（范式承诺：用不用随意）
  for (const it of bulletsOf(sec, "match", errors)) {
    const e = entryOf(it.node);
    if (!e.name) continue;
    const docV = docAddrOf(e.spec);
    if (docV) { table.push({ name: e.name, doc: docV, ir: null }); continue; }   // **B4**：框架只交付地址，内容由该 mod 自己取（用它手里的 exec/read）
    const hasAddr = String(e.spec || "").indexOf("::") >= 0;
    const a = hasAddr ? splitAddr(e.spec) : { path: null, sel: String(e.spec || "").trim() };   // 无 :: ⇒ 整串即选择器（mod 自己的篇，path 不写）
    if (!a.sel) { errors.push({ kind: "schema", name: e.name, at: "mod match · " + e.name, msg: "match trait 条目缺选择器（sel 必填；整篇表态写 ::..）—— 已跳过" }); continue; }
    try {
      const ir = await compileSel(a.sel);
      if (!Array.isArray(ir)) { errors.push({ kind: "schema", name: e.name, at: "mod match · " + e.name, msg: "sel 编译结果不是 Step[]" }); continue; }
      const probs = irProblems(ir, []);
      if (probs.length) { errors.push({ kind: "schema", name: e.name, at: "mod match · " + e.name, msg: "不合丐版契约（红灯）：" + probs.join("；") }); continue; }
      const badF = fieldsOf(ir, []).filter((f) => NODE_FIELDS.indexOf(f) < 0);
      if (badF.length) { errors.push({ kind: "schema", name: e.name, at: "mod match · " + e.name, msg: "IR 用到的字段不上节点：" + badF.join(",") }); continue; }
      const pos = positionalStepOf(ir);
      if (pos) { errors.push({ kind: "schema", name: e.name, at: "mod match · " + e.name, msg: "位置/序号类写法在路径上判不出来（" + pos + "）⇒ 该条不参战" }); continue; }
      table.push({ name: e.name, path: a.path || null, ir });
    } catch (err) {
      errors.push({ kind: "schema", name: e.name, at: "mod match · " + e.name, msg: "sel 编译失败（不回填）：" + String((err && err.message) || err).slice(0, 120) });
    }
  }
  return { table, errors };
}

/**
 * match trait 的判定入口（mod 渲染时逐节点问）：表尾往前扫，第一个命中即回名字（含宏 `_HIDDEN`）；无 → null。
 * chain 取 node.chain（MST computed，与 gates 同法）；非 MST 入参退成 [node]（check 口径同 claimMatches）。
 */
export function matchOf(node, table) {
  if (!node || typeof node !== "object" || !Array.isArray(table)) return null;
  const chain = Array.isArray(node.chain) ? node.chain : [node];
  const parent = chain.length > 1 ? chain[chain.length - 2] : null;
  for (let i = table.length - 1; i >= 0; i -= 1) {
    const t = table[i];
    if (!t || !t.name) continue;
    if (claimMatches(t, node, chain, parent)) return t.name;
  }
  return null;
}

/**
 * 地址里的 path 段与当前文档是否匹配（**声明说了算**：不匹配的条目当场不参与求值）。
 * 三种写法（旧壳 ui.md 里都出现过，取并集）：
 *   · 空 / '*' / '.*'        ⇒ 任意文档（std 的 `- std: \`.*::..\`` 就是它）
 *   · '/demos/xbt/'          ⇒ 目录前缀（末尾斜杠）
 *   · '/dashboard.md'        ⇒ 精确路径；含 glob 字符（* ?）时按 glob 转正则
 * 一律先规范化：去空白、补前导 '/'、去重复斜杠。
 */
export function pathMatches(pattern, mdPath) {
  const p = String(pattern == null ? "" : pattern).trim().replace(/\/+/g, "/");
  if (!p || p === "*" || p === ".*") return true;
  const norm = (s) => { const t = String(s == null ? "" : s).trim().replace(/\/+/g, "/"); return t.startsWith("/") ? t : "/" + t; };
  const doc = norm(mdPath);
  if (p.endsWith("/")) return doc.startsWith(p) || doc === p.slice(0, -1);          // 目录前缀
  if (p.indexOf("*") < 0 && p.indexOf("?") < 0) return doc === norm(p);             // 精确
  // glob：转义除 * ? 之外的正则元字符，* → [^/]*（按段匹配），? → [^/]
  // 注意：逐字符用 for…of 遍历（判官 R5a 禁本文件出现切分调用；它防的是"按文本切声明条目"，注释里写了同名字样也会被文本扫描命中）。
  let re = "^";
  for (const ch of norm(p)) {
    if (ch === "*") re += "[^/]*";
    else if (ch === "?") re += "[^/]";
    else re += /[.*+?^${}()|[\]\\]/.test(ch) ? "\\" + ch : ch;
  }
  return new RegExp(re + "$").test(doc);
}

/** **节点链的 doc_path 段**（path 三段 `wt::doc_path::sel` 的中段）：链首是店（root 节点），店自带 `path`。
 *  文档身份由店在诞生时带上（open / F 帧落地），节点沿 `chain` 向上继承 —— 谓词只读这一段，不查文件表。
 *  无文档身份（脱离树/纯对象）⇒ 返回空串，调用方按「不门控」处理（与旧 route 为空时等价）。 */
export function docPathOf(chain) {
  const root = chain && chain[0];
  return root && typeof root.path === "string" ? root.path : "";
}

/**
 * **判定谓词（P3 生产入口）**：一条路回答「这条目命中这个节点吗」（认领与可见性共用它）。
 *   = 「候选（或壳位）」∧「**doc_path 段命中**」∧「IR 在该节点的祖先链上命中」。
 *   · doc_path 段：条目写了 `path` 就只作用于**节点所属的那篇**（不是页面当前 route）；节点无身份 ⇒ 不门控；
 *   · 候选面：`isCandidate`（T1 树面等价）——非候选（注入叶/行内/hr/root）永不被认领；
 *   · 壳位：`bl`/`od` 不是候选，但要看**谁在问**：
 *       - **可见性**（`opts.containers === true`）：容器跟孩子走 —— 区域被盖住时容器也得盖住，否则会渲染出空壳；
 *       - **认领**（默认 false）：只认「落点就是本节点」。**容器继承认领会让一个区域画出两份** ——
 *         真页面实测：`/stats.md` 的卡片出现两次（`h 面板` 一次 + 区域里那个 `bl` 又一次）。
 *   · 位置：`chain` = 祖先链（生产由 `Node.get chain` 给，`ir ⟺ mst path` 的那条链）。
 */
let claimWalk = null;
/** **递降口子**（跟读单 A2）：「链上求值」由 **app** 提供 —— `std/basic_render` 的 `createChainWalk(walkPrims)`；
 *  框架只留**口子 + 谓词**（本函数）；没装 ⇒ **抛错**（不静默、不兜底）。装点：宿主在挂 app 时 `setClaimWalk(...)`。 */
export function setClaimWalk(fn) { claimWalk = typeof fn === "function" ? fn : null; }
export function getClaimWalk() { return claimWalk; }

export function claimMatches(entry, node, chain, parent, opts) {
  if (!entry || !node || typeof node !== "object") return false;
  if (!isCandidate(node, parent) && !CONTAINER.has(node.nt)) return false;
  // **path 门控（doc_path 段）**：条目写了 path ⇒ 只作用于「节点所属的那篇」；节点无文档身份 ⇒ 不门控
  // （与 route 为空时「全部条目参战」的语义一致）。多篇同页时每篇各命中自己的条目。
  if (entry.path != null) {
    const docPath = docPathOf(chain);
    if (docPath !== "" && !pathMatches(entry.path, docPath)) return false;
  }
  // **默认关容器继承**（认领口径）；要开（vis）就显式传 `{ containers: true }`。
  // **A2 真搬**：链上递降由 app 注入（`createChainWalk`），框架这里只有谓词与口子。
  if (!claimWalk) throw new Error("claimMatches：链上递降（chainWalk）尚未由 app 注入 —— 跟读单 A2：递降归 app（`createChainWalk(walkPrims)`），框架不兜底");
  return claimWalk(entry.ir, chain, opts || { containers: false }) === true;
}

/** 共享的"字段缺失"账：同一批判据里复用，避免每次判据 new 一个 Set（只记录字段名，不随节点增长）。 */
const EMPTY_MISSING = new Set();

