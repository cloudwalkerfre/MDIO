// @ts-check
import { types, getParent, hasParent, isAlive, walk, getRoot, flow } from "mobx-state-tree";
import { observable, untracked } from "mobx";
import { pathMatches, modCompOf, extractDecl, sectionOf, loadDecl, collectTableRefs, compileMatchTable, compileVisTable, compileModMatch, visEntriesOf, matchEntriesOf } from "../services/uiMd.js";
import { getClaimEval } from "../services/claimEval.js";   // 认领/可见性的**策略**由 app 提供（B2）

/**
 * UiDecl —— 声明侧 MST（与树侧的文档店同范式）。**放视图层**：它持有 doc（树引用）与 mods（视图模块），
 * 并 import 服务层的判定（服务层内部互不 import，判官 R1；视图侧 import 服务是允许的）。
 *   · 快照只装纯数据：claims（声明序）+ vis（可见性）+ errors（诊断账）；
 *   · doc（文档店实例）与 mods（模块命名空间：函数/实例）放 volatile，且用 **observable.box** 承载 ——
 *     MST 的 volatile 不是 observable：直接存普通字段会让 observer 追踪不到（实测踩过）；
 *   · **判定只有一条路**（P3 之后）：claimAt/visible 逐个条目调 `uiMd.claimMatches`
 *     =「候选（isCandidate）或壳位」∧「`chainWalk(ir, 祖先链)`（app 注入）命中」；命中集与扫描不存在。
 *   · 表序 = 优先级从低到高（编译期排好），所以 claimAt **从尾往前扫、第一个命中即胜者**
 *     —— 与"声明序后写覆盖"逐字一致。
 * 组件选择（router）在视图层：doc/compTable.jsx 的 compFor(ui, node, preClaim)。
 */
/** 可见性条目的**形状**（F6：不再是 `types.frozen()` 随手对象 —— 错写即类型错）。
 *  形状来源：`uiMd.compileVisTable` 的产物 `{ name, path, sel, ir, docLevel? }`；
 *  消费点两处：`activeVis`（path 门控）与 `visible()`（`claimMatches` 逐条判定）。 */
const VisEntry = types.model("VisEntry", {
  name: types.optional(types.string, ""),        // "include" | "exclude"
  sel: types.optional(types.string, ""),
  ir: types.maybeNull(types.frozen()),           // 节点级条目才有；文档级条目 = null
  path: types.maybeNull(types.string),           // 只在该文档生效；null = 任意文档
  docLevel: types.optional(types.boolean, false),// true = 只有 path（文档级 include/exclude）
});
const Claim = types.model("Claim", {
  name: types.string,
  // path 必须显式声明：MST 对未声明字段**静默丢弃**（与 client.js 的节点模型丢 h 是同一类坑）。
  // 丢了它，扫描时 entry.path 恒 undefined ⇒ path 门控形同不存在（实测：/b.md 也照样命中）。
  path: types.maybeNull(types.string),   // 收 null 也收 undefined：编译产物显式写 null（maybe 只收 undefined，实测把活应用打崩过）
  ir: types.frozen(),          // 引擎编译的 steps：自由形状、不可变
  // **没有 `bucket` 字段**：桶是实现期发明的中间概念。判定只有一条路（`claimMatches` → `chainWalk`（app 注入）），
  // 「命中即封闭」由**节点**决定（`NodeView` 的 `isSectionShell(node.nt)`），不由条目属性决定。
});


// 两个模块级缓存（`chainMemo` 祖先链 · `hitMemo` 认领对象驻留）**不存在**：
//   P3 的原则就是「删缓存不改语义」——它们是命中集/扫描时代的遗留，只影响速度、不影响判定。
//   判据：金标准 78/78（逐节点胜者不变）+ 两个页面套件（真渲染不变）。

export const UiDecl = types
  .model("UiDecl", {
    claims: types.array(Claim),           // 声明序，快照可见
    // visibility 表：与 claims **同管线**（同一 IR、同一扫描点），只是消费方向相反（画不画 vs 谁画）
    vis: types.array(VisEntry),           // 形状见 VisEntry（F6：给了类型）
    errors: types.array(types.frozen()),  // 诊断账，快照可见
    // （rev 字段不存在：全代码库零读点，每补丁白写 13–24 微秒；帧计数由 client._rev 经事件 ev.rev 提供，
    //   工作区视图读的是 ev.rev 而不是这里的字段）
    route: types.optional(types.string, ""),   // 当前 route（原 mdPath 正名）：claims/vis/mods 的 path 段按它门控
    docId: types.maybe(types.number),            // 当前档（纯 JSON 数字；doc 由它从主树派生 —— 收口）
    declDocId: types.maybe(types.number),        // 声明文档（ui.md）的 id：decl 投影的树（与 docId 的活动档是两回事）
    appName: types.optional(types.string, ""),    // app 名(URL ?app= 覆盖,装配面 boot 注入一次;空则用声明 appview)
    // **只剩一个帧计数**：`declRev`（声明换新；渲染路径的影响集订阅的就是它）。
    // 三档读数 `layoutRev`/`contentRev`/`structRev` 不存在：`structRev` 的读者本来是"祖先链缓存"，
    // 缓存删了它就成了**零读者计数器**（审计：全仓只有两处自增、没有一处读）——
    // 结构变化本来由 MST 自己的可观察性负责（`DocPane` 读 `root.ct`、`NodeView` 读 `getSnapshot(node)`）。
    declRev: types.optional(types.number, 0),
  })
  .volatile(() => ({
    // deep:false —— 官方实践：**不透明值**（MST 节点实例、模块命名空间/组件函数）不要深度转换。
    // 默认 deep:true 会把装载记录与命名空间也 observable 化（实测：ns.default 被包成 ObservableValue@…），
    // 组件身份/通知行为随之失真（渲染数与订阅语义都测不准）。
    modsBox: observable.box(new Map(), { deep: false }),  // 名字 → 模块命名空间（含组件函数）
    modMatchBox: observable.box(new Map(), { deep: false }),  // mod 私有 match trait 表（名字 → {name,path,ir}[]；范式 §63）
    // 每个 store 实例一个普通对象（多实例不串味）
    storeToken: {},
    // ── Step2 · 重装 = store 的 flow action ──
    reloadImpl: null,          // 执行器（装配点注入：applyDecl 的包装；零 import 纪律，店不 import 装配面）
    reloading: false,          // 单飞：重装进行中只记一条待办
    pendingWhy: null,
    // **换树竞态代际**（P5）：换树（boot 重跑）时作废在飞的 reload——
    // 旧 wt 的在飞异步读（表 md 展开/装载）晚到时，其投递会覆盖新树的声明。gen 对不上即弃轮。
    reloadGen: 0,
    // **`_claimHits` 不存在**：命中计数不在**渲染路径**上累加（那是"视图里写状态"，
    //   MST 会报错），诊断走**离线数**：`claimHitsNow()` 在声明换新 / 帧到达时 walk 一遍当前档的树。
    // （route 放 MST 属性而不是 volatile —— 注意：旧注释「volatile 里的 box 追不到」**已被实测推翻**（NOTE §8.4 /
  //   check_volatile_box.mjs：volatile 里的 box **能**被 computed 追踪，改了就重跑）。真正的理由是语义归属：
    //   route 属于**店状态**（该进快照、该被内容键覆盖），不该藏在 volatile 里。）
  }))
  .views((self) => ({
    /** 当前档：由 docId 从主树的 wts.wt.by 现取（无参 getter = computed；换 wt 自动跟随）。 */
    get doc() {
      const w = getRoot(self).wts && getRoot(self).wts.wt;
      if (!w || self.docId == null) return null;
      return w.by.get(String(self.docId)) || null;
    },
    /** ★ 声明结构 = 声明文档树(ui.md)的投影(getter = computed:树不变不重算,树变自动失效)。
     *  不复制、不进快照——树是唯一真源,这里只是它的派生视图。 */
    get decl() {
      const w = getRoot(self).wts && getRoot(self).wts.wt;
      const t = w && self.declDocId != null ? w.by.get(String(self.declDocId)) : null;
      return extractDecl(t || null);
    },
    /** 声明树本身(decl 投影的源)——树唯一入口,替代 applyDecl 的 tree 参数与现场 sectionOf。 */
    get declTree() {
      const w = getRoot(self).wts && getRoot(self).wts.wt;
      return w && self.declDocId != null ? w.by.get(String(self.declDocId)) : null;
    },
    /** app 私有区节(## <APPNAME>)。app 名 = URL 覆盖 || 声明 appview。 */
    get appSection() {
      return sectionOf(self.declTree, self.appName || self.decl.app.appview);
    },
    /** app 私有区的 ### visibility 节。 */
    get appVisSection() {
      const s = self.appSection;
      return s ? sectionOf(s, "visibility") : null;
    },
    /** mod 私有节(## <MODNAME>;带参 view:不缓存但可追踪,重装 flow 每轮只读几次,无伤)。 */
    modSectionOf(name) { return sectionOf(self.declTree, name); },
    /**
     * **「观察谁」派生化(P3)**：md 件的 {name, doc, sel} 三要素由**店**从声明派生(纯字符串解析),
     * 装配面只做异步定位(waitSnapshot/compileSel)与挂载(onSnapshot)。解析规则与 registerWatch 原样：
     * ".md::" 共 5 字符(doc 从 5 起到下一个 "::",sel 在其后)。依赖 decl(重装换新)⇒ 自动失效重算。
     */
    get mdModSpecs() {
      const mods = (self.decl && self.decl.mods) || [];
      const out = [];
      for (const m of mods) {
        const s = String((m && m.spec) || "");
        if (s.indexOf(".md::") !== 0) continue;
        const i = s.indexOf("::", 5);
        out.push({ name: String((m && m.name) || ""), doc: s.slice(5, i), sel: s.slice(i + 2) });
      }
      return out;
    },
    /**
     * **表 md 观察目标(P3 派生)**：三来源 doc 值(顶层 match / app 私有 match / mods 私有 match)去重,
     * 映射 {path, docId}(店在主树上,经 wts 的 byPath 拿 docId)。展开(waitSnapshot/树上 sectionOf)仍归装配。
     */
    get tableMdDocs() {
      const decl = self.decl || {};
      const refs = [];
      const pushDoc = (d) => { if (d && refs.indexOf(d) < 0) refs.push(d); };
      for (const e of decl.match || []) pushDoc(e && e.doc);
      const as = self.appSection;
      if (as) {
        const m0 = sectionOf(as, "match");
        for (const e of matchEntriesOf(m0 || null, [])) pushDoc(e && e.doc);
      }
      for (const m of decl.mods || []) {
        const s1 = self.modSectionOf(m && m.name);
        const m1 = s1 ? sectionOf(s1, "match") : null;
        for (const e of matchEntriesOf(m1, [])) pushDoc(e && e.doc);
      }
      const w = getRoot(self).wts && getRoot(self).wts.wt;
      const byPath = w && w.byPath;
      const out = [];
      for (const p of refs) {
        const id = byPath ? byPath[p] : null;
        out.push({ path: p, docId: id == null ? null : Number(id) });
      }
      return out;
    },
    /**
     * **离线数命中**（判「零生效声明」用）：walk 当前档的树，用**同一个判定**（`gates`）数每个条目接走多少节点。
     * 为什么离线：① 渲染路径不许有写（视图有副作用 = MST 报错，实测）；② 渲染期只覆盖"已渲染的可见节点"，
     * 而这条诊断要的是「这档里它到底有没有命中节点」——走一遍树才是准确口径。
     * 调用点：`entry.client` 的 `maybeReportDeadSecClaims`（声明换新 / 帧到达时，按 `declRev:route` 去重）。
     */
    claimHitsNow() {
      const out = Object.create(null);
      const root = self.doc;
      if (!root) return out;
      untracked(() => {
        walk(root, (n) => {
          const c = self.gates(n).claim;                 // 一处 hit（与渲染同一条判定）
          if (c && c.name) out[c.name] = (out[c.name] || 0) + 1;
        });
      });
      return out;
    },
    get mods() { return self.modsBox.get(); },

    /**
     * **条目表**：claims 与 vis 各自一张（编译期产物，过 path 门控后按当前文档生效）。
     * P3 之后**没有命中集**：判定在渲染路径上逐节点按路径算（claimMatches）。
     * 内容版本 snap（NodeView 里 getSnapshot(node)）仍是「给件的内容键」。
     */
    get renderClaims() {
      const mods = self.modsBox.get();
      return self.claims.filter((c) => !!modCompOf(mods.get(c.name)));
    },
    get activeClaims() {
      const p = self.route;
      const list = self.renderClaims;
      if (!p) return list;
      return list.filter((c) => c.path == null || pathMatches(c.path, p));
    },
    get activeVis() {
      const p = self.route;
      if (!p) return self.vis;
      return self.vis.filter((e) => !e || e.path == null || pathMatches(e.path, p));
    },
    /**
     * 命中判定：**从表尾往前扫，第一个命中即胜者**（= 声明序「后写覆盖」）。
     * 表序就是**声明序**（`compileMatchTable` 不再排序，没有 `byBucket`）。
     * 「命中即封闭」不在这一层：它由 `NodeView` 按**节点**决定（`isSectionShell(node.nt)`）。
     */
    /**
     * **命中就这一处**（波 3 收口）：`claim`（`## match`）与 `visible`（`## visibility` 的 include/exclude）
     * 是**同一件事**（都是「条目表 + path 门控 + 一条祖先链上的 hit」）⇒ 合并成一次判定：
     *   · **一条链**（`Node.get chain()`：一条链，两个判定共用）；
     *   · **一处 hit**（唯一实现就是服务层 `claimMatches` → app 注入的 `chainWalk`）；
     *   · 差别只在**是否继承容器**：认领 = 精确、vis = 容器跟孩子走。**今天这个差异由调用点传 `opts.containers` 决定**
     *     （认领 `false`、vis `true`，见下面两处）；方案 §2.1 曾设想「差异进条目」，**R6.5 未采纳**（口径分裂已记进
     *     `/前端IR对仗/复盘_方案v6对代码.md`）—— 注释「不在调用点」与代码不符；此处为准。
     * 消费点：认领 ⇒ `compFor` 选件；可见 ⇒ 画/不画。业务仍留在消费点。
     */
    gates(node) {
      if (!node || typeof node !== "object") return { claim: null, visible: true };
      // **一条链**：链的所有者是 `Node.get chain()`（无参 getter = MST computed）。
      //   非 MST 入参（判据里的普通对象）取不到 `chain` ⇒ 退成单元素链，**不碰 MST**（`check_store_boot` 的既有约定）。
      const chain = Array.isArray(node.chain) ? node.chain : [node];
      const parent = chain.length > 1 ? chain[chain.length - 2] : null;
      // **策略归 app**（跟读单 B2「认领与可见性归 basic_render」的落地）：
      //   框架只问口子要结果；没装 app 的策略 ⇒ 无认领（未认领照旧由声明里的默认渲染器兜），**不猜、不暗补**。
      const f = getClaimEval();
      if (!f) return { claim: null, visible: true };
      const r = f(self, node, chain, parent);
      return r && typeof r === "object" ? { claim: r.claim || null, visible: r.visible !== false } : { claim: null, visible: true };
    },
    // `noteClaimHit` **不存在**：渲染路径不写状态；诊断走 `claimHitsNow()`（离线 walk）。
    /** 文档级可见范围（vis 里 sel 空、只有 path 的条目）：include/exclude 文档路径。 */
    get docScope() {
      const inc = [], exc = [];
      for (const e of self.vis) {
        if (!e || !e.docLevel) continue;
        if (e.name === "include") inc.push(e.path);
        else if (e.name === "exclude") exc.push(e.path);
      }
      return { include: inc, exclude: exc };
    },
    /** 文档路径是否可见（文件树/开档用）：exclude 命中即隐藏；有 include 时只留命中。 */
    docVisible(path) {
      const p = String(path == null ? "" : path);
      const sc = self.docScope;
      for (const pat of sc.exclude) if (pathMatches(pat, p)) return false;
      if (sc.include.length) return sc.include.some((pat) => pathMatches(pat, p));
      return true;
    },
    /** 观测：本档 path 门控的生效条数（claims / vis 分开数），供日志用。 */
    get declInfo() {
      const p = self.route;
      const hit = (arr) => (arr || []).filter((e) => e && e.path != null && pathMatches(e.path, p)).length;
      return { route: p, claims: self.claims.length, claimsOn: hit(self.claims), vis: self.vis.length, visOn: hit(self.vis) };
    },
    /** 组件名 → 模块命名空间（视图层取 default 当组件）。 */
    modOf(name) { return (name && self.modsBox.get().get(name)) || null; },
    /** mod 私有 match trait 表（S2：宿主装载期编译、经 props 注入给件；范式 §63/§64）。
     *  **path 门控**与 claims/vis 同一处：在谓词 `claimMatches` 里按「节点所属文档的 doc_path 段」判；
     *  这里**不再按页面 route 预过滤**（多篇同页时 route 表达不了；私有表与公开表同一语义）。 */
    modMatchOf(name) {
      return (name && self.modMatchBox.get().get(name)) || null;
    },
    /**
     * 可见性判据（渲染面唯一读取点）：**exclude 优先**；有 include 时只有命中者可见；
     * 两表皆空 ⇒ 全可见（无 visibility 节时行为零变化）。
     */
    /** 薄包装：可见性（与认领同一次判定，见 `gates`）。 */

  }))
  // **写操作必须落在 `.actions` 里**：MST 对受保护对象的修改只认 action。
  //    这里曾被一次清理误删 `.actions((self) => ({`，结果 `load()` 里的 `claims.replace` 抛
  //    「Cannot modify Claim[]@/claims, the object is protected」⇒ **整个应用起不来**。
  //    检查：启动后必须能 load（人工跑一次应用启动即可；仓库里没有同名脚本）。
  .actions((self) => ({
    /** 换 route：命中判定与祖先链都从头算（route 是 path 门控的键）。 */
    setRoute(path) { self.route = String(path == null ? "" : path); },
    /** 换档（只记 docId 纯字段；doc 由派生得出 —— 收口）。 */
    setDocId(id) { self.docId = typeof id === "number" ? id : null; },
    /** 声明文档 id（decl 投影的树）。 */
    setDeclDocId(id) { self.declDocId = typeof id === "number" ? id : null; },
    /** app 名（URL 覆盖,装配面 boot 注入一次）。 */
    setAppName(name) { self.appName = String(name == null ? "" : name); },
    // **`bumpPatch` 不存在**（性能审计）：它的唯一效果是 `structRev += 1`，而 `structRev` **零读者**
    //（读者本来是"祖先链缓存"，缓存删了它就悬空）。结构变化由 MST 自己的可观察性负责：
    // `DocPane` 读 `root.ct`、`NodeView` 读 `getSnapshot(node)` ⇒ 增删只重渲受影响的那一支。
    // 这也消掉了每个补丁帧的 `ops` 遍历 + 一次 store 写。
    /** 当前 route（claims/vis 的 path 门控用）。 */

    /**
     * 装载声明。**claims 的顺序 = 优先级从低到高**（claimAt 从尾往前扫、第一个命中即胜者）——
     * 这个顺序由 uiMd.compileMatchTable 排好（node 登记 → node 带谓词 → sec）；手工喂表必须照这个序。
     */
    load({ claims, vis, errors, mods, modMatch }) {
      self.claims.replace(claims || []);
      self.vis.replace(vis || []);
      self.errors.replace(errors || []);
      self.modsBox.set(mods instanceof Map ? mods : new Map(Object.entries(mods || {})));
      self.modMatchBox.set(modMatch instanceof Map ? modMatch : new Map());
      self.declRev += 1;                  // 声明换新 ⇒ 认领判定必然重算（条目表换了）；命中计数改离线数（无状态可重置）
    },
    /** 注入装配面能力袋（impl 对象：loadMod/compileSel/expandRefs/openSrcDocs/pruneCssMods/injectErrorComps/printDiag/registerWatch/logModMatch）。
     *  店只编排流程；装载/编译/收集是 uiMd 纯函数（店直接调），会话与样式能力经 impl。 */
    setReloadImpl(impl) { self.reloadImpl = impl && typeof impl === "object" ? impl : null; },
    /** 换树作废在飞 reload（装配面 switchTo 时调；见 reload flow 的代际校验）。 */
    bumpReloadGen() { self.reloadGen += 1; },
    /** ★ 重装 = store 的 flow action（观察端 onSnapshot 触发）。八步编排 + 单飞 trailing。 */
    reload: flow(function* (why) {
      if (self.reloading) { self.pendingWhy = why; return; }
      self.reloading = true;
      self.reloadGen += 1;
      const gen = self.reloadGen;
      const alive = () => gen === self.reloadGen;   // 换树 bumpReloadGen 则在飞轮作废（旧树数据不再投递）
      try {
        const impl = self.reloadImpl;
        if (!impl) return;
        impl.resetSources();   // 上一轮的源文档登记作废,本轮装载重新收集
        const decl = self.decl;
        // ①.5 换本轮 wt/app 的局部 md scope（decl.mods 的 .md:: 项 = @app/<NAME>）
        if (typeof impl.beginScope === "function") impl.beginScope(decl);
        // ① 投影(computed 已缓存) → ② 装载(loadMod 注入;loadMdMod 登记源文档路径)
        const bag = yield loadDecl({ decl, load: impl.loadMod });
        if (!alive()) { impl.logLine("decl: 重装代际作废(换树) why=" + why); return; }
        // ③ 表 md 收集(uiMd 纯,带容错:树异常不阻塞重装) → ④ 展开(装配:open+waitSnapshot+树上 sectionOf)
        let refs = [];
        try {
          refs = collectTableRefs(decl, self.appSection, (n) => self.modSectionOf(n), bag.errors);
        } catch (e) {
          impl.logLine("decl: 表 md 引用收集失败（不阻塞）：" + String((e && e.message) || e).slice(0, 100));
        }
        const claimEntries = yield impl.expandRefs(refs, bag.errors);
        if (!alive()) { impl.logLine("decl: 重装代际作废(换树) why=" + why); return; }
        impl.openSrcDocs();
        impl.logLine("decl: 认领表来自顶层 ## match（" + claimEntries.length + " 条）");
        // ⑤ 编译(引擎 compileSel 注入)
        const modPaths = new Map((decl.mods || []).filter((m) => m.path).map((m) => [m.name, m.path]));
        const claimBag = yield compileMatchTable(decl, impl.compileSel, modPaths, claimEntries);
        if (!alive()) { impl.logLine("decl: 重装代际作废(换树) why=" + why); return; }
        const appVisEntries = visEntriesOf(self.appVisSection, bag.errors);
        const visBag = yield compileVisTable(decl, impl.compileSel, appVisEntries);
        if (!alive()) { impl.logLine("decl: 重装代际作废(换树) why=" + why); return; }
        const modMatchMap = new Map();
        try {
          for (const m of decl.mods) {
            const sec = self.modSectionOf(m.name);
            if (!sec) continue;
            const t = yield compileModMatch(sec, impl.compileSel);
            impl.logModMatch(m.name, t);
            if (t.table.length) modMatchMap.set(m.name, t.table);
          }
        } catch (e) {
          impl.logLine("modMatch 编译失败（不阻塞 boot）：" + String((e && e.message) || e).slice(0, 100));
        }
        // ⑥ 样式回收 + 错误组件
        if (!alive()) { impl.logLine("decl: 重装代际作废(换树) why=" + why); return; }
        impl.pruneCssMods(new Set(bag.mods.keys()));
        impl.injectErrorComps(bag);
        // ⑦ 一次投递(唯一写店点)
        self.load({
          claims: claimBag.table,
          vis: visBag.table,
          errors: (bag.errors || []).concat(claimBag.errors || [], visBag.errors || []),
          mods: bag.mods,
          modMatch: modMatchMap,
        });
        // ⑧ 诊断输出(装配纯函数) → ⑨ 观察换新(装配挂载)
        impl.printDiag(self, decl, bag.errors, claimBag, visBag, why);
        impl.registerWatch(self);
      } finally {
        self.reloading = false;
        const w = self.pendingWhy; self.pendingWhy = null;
        if (w) yield self.reload(w);
      }
    }),
  }));

export function createUi() { return UiDecl.create({ claims: [], vis: [], errors: [], route: "" }); }