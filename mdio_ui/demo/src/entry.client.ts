// @ts-check
/**
 * entry.client —— 水合装配点（原 main.jsx 的搬迁；§1.4 / 附录 B1·B3）。
 *
 * 全项目唯一知道各层类型的地方：boot 步骤序列（client → uiMd）+ 唯一自写装载线（md 文本线）
 * + createRoot。层间只传参、不 import；SSR 侧不跑本文件（无浏览器、无 wasm）。
 */
import { createElement as h, useEffect, useSyncExternalStore } from "react";
import { observer } from "mobx-react-lite";   // 入口件**自动 observer**：开发者不必自己包（少一个概念）
import { createRoot } from "react-dom/client";

// 主题：默认暗色（旧壳同款 .dark 机制）；DevView 头部可切换
if (!document.body.classList.contains("dark") && !document.body.classList.contains("light")) document.body.classList.add("dark");
// 模块 specifier 一律用**逻辑名**（esmx 官方协议：消费方永远看不到物理路径，重命名源文件不再破坏调用方）。
import { registerStep, start, subscribe, getState, fail as failBoot, reset as resetBoot } from "ui_core/boot";
import { connectClient } from "ui_core/client";
import { loadDecl, compileMatchTable, compileVisTable, modCompOf, compileModMatch, sectionOf, matchEntriesOf, visEntriesOf, READ, walkPrims, setClaimWalk, cachedCompileSel } from "ui_core/uiMd";
import { setClaimEval } from "ui_core/claimEval";   // 认领/可见性的策略口子（默认 std，host 直装；app 可覆盖）
import { createClaimEval as stdClaimEval, createChainWalk as stdChainWalk } from "ui_std/claimEval";   // 默认策略（host 在装载后直装）
import { UiDecl } from "ui_core/uiStore";
import { appRouter } from "ui_core/appRouter";   // 应用路由能力（共享单例）：入口件与节点件拿到同一份
import { DevStore } from "ui_devview/devStore";   // 壳态店类型（拼进总 store 的 dev 子树）
import { Wts } from "ui_core/client";   // 数据顶层类型（总 store 的 wts 子树）
import { types, onSnapshot, getType } from "mobx-state-tree";
import { observable } from "mobx";   // volatile box（app 槽挂活实例；项目 client.js 同款写法）
import { adoptInto, scopeCssByAttr, globalGuard } from "ui_core/domScope";   // 隔离原语（guard 的属性前缀法 / 影子根 / sheet 采纳 / 全局守卫）
import { defineAppView } from "ui_appview/appView";   // appview 容器件（mdio_ui 自带，独立 mod）
import { loadMd, loadScopedMd } from "ui_core/mdLine";
import { createMdScope } from "ui_core/mdScope";
import { createModLoader } from "ui_core/modLoader";
import DevView from "ui_devview/devview";   // 外壳（库包）：宿主把渲染体（comps 的 MDRoot / app 件）以 App prop 注入
import { makeRenderNode } from "ui_core/DocPane";   // B1 真搬：框架只出原语工厂；对外组件归 std/comps
import { MDRoot } from "ui_std/comps";            // 「渲染 md 根节点的组件」——兜底屏（开发面无 appview）也用它
import BasicRender from "ui_std/basic_render";   // 系统默认 app：dev 面无 av 时直接挂它（app 化）
import { createWorker } from "ui_wasm/worker";   // wasm web 适配器：worker 工厂注入 client（R1 零 import 纪律）
import { ENGINE_VERSION } from "ui_wasm/version"; // 引擎版本戳：boot 对拍基线（网关侧字段 = 后端配合项）
import "./app/ui.css";                      // 应用设计系统（外壳搬家后由装配点引入；check-layers 规则 11 仍盯着这个路径）
import { log } from "ui_core/logBus";   // 管线日志的数据源（服务层；装配点把 monitor 出口接过来）
import { monitorSink, report, watch as monitorWatch } from "ui_core/monitor";   // WS 侧监控（贴着 client.js）
// 框架面（S4 装配收口）：位置语法 / CSS 采纳 / 首帧等待 已搬进 core，装配点不再自带实现。
import { uiModuleOf, parseSpec, resolveUiSpec, wtChain } from "ui_core/locator";
import { adoptCssMod, pruneCssMods, setCssAdoptLog, registerStyleRoot, unregisterStyleRoot, mirrorStyleLinks, startDynamicStyleMirror } from "ui_core/cssAdopt";
import { waitSnapshot } from "ui_core/snapshot";
// READ 单源（ui_core/uiMd 出口）：装配点只 import，不再 makeRead
let devStore: any = null;   // S2：壳态店实例（client 步建；DevView 经 prop 拿）
// The shell/app lifecycle bridge is only for an embedding host iframe.
// A top-level mdio page has parent === self and must stay completely unaffected.
const BRIDGE = typeof window !== "undefined" && window.parent !== window

// Parent bridge: shell boot lifecycle (booting/ready/failed). This is an event,
// not a heartbeat. The client starts one boot deadline per booting event, and a
// successfully booted genesis/BASIC_RENDER app remains visible.
function postShell(state: "booting" | "ready" | "failed", error?: any) {
  if (!BRIDGE) return
  try {
    window.parent.postMessage({
      type: "mdio-host:shell",
      state: state,
      error: error ? String(error.message || error) : "",
    }, "*")
  } catch (e) { /* no parent */ }
}
subscribe((state, error) => {
  // The initial subscription snapshot can be "booting" before start(); only emit
  // transitions after the store starts emitting (ready/failed), plus switchTo's
  // explicit booting marker below.
  if (state === "booting") return
  postShell(state, error)
});
monitorSink(log);        // **装配点接线**：监控出口 → devview 的日志面板（框架件不依赖面板）
setCssAdoptLog(report);  // cssAdopt 的日志口注入（services 之间不许互相 import，判据 R1）⇒ 走监控出口

// 网关地址 = 同源相对（壳宿主代理），不写死主机；boot 不感知任何地址。
// 步骤 3：连接**连带 wt**（/ws/gateway/{wt}）——连接上下文由 URL 确定，服务端按需建 scope。
// 命令里的 wt 前缀仍是显式覆盖；HTTP / hook 无连接时才用显式参数。
/** 当前 wt：'?wt=<uid>'（缺省 genesis）。壳宿主把该 wt 的链内联在 #mdio-wt-chain（只读端点取来）。
 *  **可变**：换树要在**同一页面内**重跑启动序列（见文件末 `__mdioSwitchWt`），
 *  所以 WT 与 GATEWAY_WS 由 `setWt()` 维护，不再是一次性常量。 */
let WT = new URLSearchParams(location.search).get("wt") || "genesis";
// 面板 app 槽：代码宿主 wt = URL 参数（覆盖）+ 默认解析（按 ui.md 的 appview 身份匹配 devStore.panels 槽配置）。数据面仍是当前页 wt。
let LOG_WT = (new URLSearchParams(location.search).get("logwt") || "").trim();
let LEFT_WT = (new URLSearchParams(location.search).get("leftwt") || "").trim();
let FOOT_WT = (new URLSearchParams(location.search).get("footwt") || "").trim();
let GATEWAY_WS = "";
const wsUrlOf = (uid: string) =>
  (location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/ws/gateway/" + encodeURIComponent(uid);
const setWt = (uid: string) => { WT = uid; GATEWAY_WS = wsUrlOf(uid); };
setWt(WT);
// **根树 uid：开机算一次**（页面生命周期内不变）。
// 不能每次渲染重算：换树是就地重跑（不重载 HTML），内联的 `#mdio-wt-chain` 仍是本次载入时的旧值
// ⇒ 重算会退化成子 wt 自己 ⇒ 外壳去读子 wt 那份不存在的 /.mds/worktrees.md ⇒ 切换清单塌成一条、标题退化成 uid。
// 打包态静态 index 没有宿主内联链（只有 dev 宿主才注入 #mdio-wt-chain）⇒ 根 wt 约定恒为 genesis；
// 兜底也绝不能返回当前 wt，否则在子 wt 页面里树会以自己为根。
const ROOT_WT = (() => {
  const hasChain = typeof document !== "undefined" && !!document.getElementById("mdio-wt-chain");
  if (!hasChain) return "genesis";
  const c = wtChain(WT);
  return c.length ? c[c.length - 1] : "genesis";
})();
// Appview 模式：?app=/文档 → 产品单页；与 devview 同一条认领链路（uiMd 步照跑），std 渲染全文档
let APP_DOC = new URLSearchParams(location.search).get("app");   // 换树时要能改（见 switchWt 的 entry 感知）
/** 文档指针归一写点（壳唯一）：产品面写 ?app=（保留 hash）；dev 面忽略。
 *  触发源 = app 经 appRouter.setDocPointer 显式请求（/observe/方案.md 原则 2：壳段只由壳写，app 只写页内段）；
 *  null/空串 ⇒ 保留字段值空（产品面刷新停画廊的现状语义）。 */
function writeDocPointer(path: string | null) {
  if (APP_DOC === null) return;   // dev 面忽略（P3）
  APP_DOC = String(path == null ? "" : path);
  try { history.replaceState(null, "", "/?wt=" + encodeURIComponent(WT) + "&app=" + encodeURIComponent(APP_DOC) + String(location.hash || "")); } catch (e) { /* 无 history 环境忽略 */ }
}
// **`app-name` 属性覆盖**（`mdio:app-change` 驱动）：非空时压过声明里的 `## app - appview:`；清空则回声明值。
let APP_NAME: string | null = null;
// **位置语法与链式解析**已搬进框架（S4 装配收口）：`ui_core/services/locator.js`
//   —— `uiModuleOf`（uid → esmx 模块名）· `parseSpec`（规范形 <wt>.<scope>::<file>[::<sel>]）·
//     `uiPathOf`（出现旧写法即报错）· `wtChain(currentWt)`（读宿主内联的 #mdio-wt-chain）· `resolveUiSpec`（沿链取件 + 资产路由回退）。
//   搬走的理由：位置串是声明与渲染件之间的公共语言，解析它不属于"装配"；装配点只该"读 ?wt/?app → 装载 → 挂 app"。

// （位置语法的四个函数见上方指针：core/services/locator.js）


/** 装配产物（层3 → 层4 → 层5）。 */
let client: any = null;
let ui: any = null;   // 声明店实例 = root.decl 子树（总 store 建好后指过去）
// ── 总 store（对应 mdio_ui）：runtime 拼装的一棵主树 = wts / decl / dev / router / app 五子树 ──
//   · app 本性 late（动态加载、可切换）⇒ 槽位 types.late，载完 app mod 后赋 AppStoreType（见 uiMd 步）；
//   · tools = env（只装工具；client/exec/wt/write 等属性每次 boot 后挂，env 属性可写）。
const tools: any = { read: READ, appRouter: appRouter, exec: null, client: null, getMainRoot: () => root };   // env = 只装工具；getMainRoot = app 店独立成树后的取数桥（函数，符合「只许函数与服务」纪律）
const UiStore = types.model("UiStore", {
  wts: Wts,
  decl: UiDecl,   // 声明店子树（计划目标架构键名：decl）
  dev: DevStore,
  // router 子树不存在:空槽无读者,app 自建 createRouterStore 与主树无关
}).volatile(() => ({
  appBox: observable.box(null, { deep: false }),   // app 槽：volatile box 挂活实例（不进快照；modsBox 同款写法）
})).views((self) => ({
  get app() { return self.appBox.get(); },
})).actions((self) => ({
  /** app 槽（volatile box · 修订）：uiMd 步 create 真店后挂进来；类型 per-create —— late 的 create 缓存死局整体消失。 */
  setApp(store) { self.appBox.set(store || null); },
}));
let root: any = null;   // 总 store 实例：甲1 —— client 步先建（app=null），换 wt 不重建
let switchPreserveApp = false;   // 本次 boot 由 switchTo 触发；app 可用 reloadOnWtSwitch=false 保留店

/**
 * **mod 装载器**（实现搬进 `ui_core/services/modLoader.js`，本处只做注入）：
 * 框架件不许 import 框架件（判据 R1）⇒ client / WT / 位置语法 / md 装载线 / 样式采纳都从这里递进去。
 * `getClient`、`getWt` **必须是函数**：换树时二者都会变（见 switchWt）。
 */
const modLoader = createModLoader({
  getClient: () => client,
  getWt: () => WT,
  loadMd,
  loadScopedMd,
  createScope: createMdScope,
  parseSpec,
  uiModuleOf,
  resolveUiSpec,
  wtChain,
  adoptCssMod,
  log,
});
// 同名解构：装配点其余地方（applyDecl / 热重载判据）用法保持不变
const { loadMod, modSrcPaths } = modLoader;

let info: { treeCount: number; conn: string; entries: any[]; allEntries: any[]; logPanel: any; leftPanel: any; footPanel: any } = { treeCount: 0, conn: "", entries: [], allEntries: [], logPanel: null, leftPanel: null, footPanel: null };

// ── Step1 · MST 观察端(registerWatch)─────────────────────────────────────────────
// onSnapshot 挂声明树与各 md 件的代码块节点 ⇒ 树变即触发(替代帧订阅;对照期回调只记日志)。
// 代际 watchGen:换树/新一轮装载时作废在飞的挂载,防旧轮晚到的 onSnapshot 泄漏。
let watchDisposers: Array<() => void> = [];
let watchGen = 0;
async function registerWatch(self: any) {
  const gen = ++watchGen;
  for (const d of watchDisposers) { try { d(); } catch (e) { /* ignore */ } }
  watchDisposers = [];
  const declTree = self.declTree;
  if (declTree) {
    watchDisposers.push(onSnapshot(declTree, () => (ui as any).reload("hot")));
  }
  // 表 md 树观察:「观察谁」由店派生(tableMdDocs),装配只挂载。
  for (const t of (self.tableMdDocs || [])) {
    if (t.docId == null) continue;
    const tr = client.md(t.docId);
    if (tr) watchDisposers.push(onSnapshot(tr, () => { log("watch: 表 md 触发 " + t.path); (ui as any).reload("hot"); }));
  }
  // md 件代码块观察:「观察谁」由店派生(mdModSpecs 三要素),装配做异步定位(waitSnapshot/compileSel)与挂载。
  const specs = (self.mdModSpecs || []);
  await Promise.all(specs.map(async (sp: any) => {
    try {
      const ent = (info.allEntries || []).find((x: any) => x.path === sp.doc);
      if (!ent || ent.id == null) { log("watch: 找不到源文档 " + sp.doc); return; }
      let tree = client.md(ent.id);
      if (!tree) { await waitSnapshot(client, ent.id, 8000).catch(() => null); tree = client.md(ent.id); }
      if (gen !== watchGen) { log("watch: 代际作废 " + sp.name); return; }   // 换新轮已作废本轮
      if (!tree) { log("watch: 树未落店 " + sp.name); return; }
      const ir = await compileSel(sp.sel).catch(() => null);
      if (gen !== watchGen) { log("watch: 代际作废(编译后) " + sp.name); return; }
      if (!ir) { log("watch: sel 编译失败 " + sp.name + " sel=" + sp.sel); return; }
      const hits = READ.apply([tree], ir);
      const node = hits && hits.hits && hits.hits[0];
      if (node) {
        watchDisposers.push(onSnapshot(node, () => (ui as any).reload("mod-src")));
        log("watch: 已挂 mod-src " + sp.name + "（sel=" + sp.sel + " hit=" + (node.nt || "?") + "）");
      } else {
        log("watch: 未定位到代码块 " + sp.name + "（sel=" + sp.sel + " hits=" + ((hits && hits.hits && hits.hits.length) || 0) + "）");
      }
    } catch (e) { log("watch: 代码块观察挂载失败 " + sp.name + "：" + String((e && (e as any).message) || e).slice(0, 100)); }
  }));
  log("watch: 挂载完成 hot=" + (declTree ? 1 : 0) + " mod-src=" + specs.length + " 实际=" + watchDisposers.length);
}
function disposeWatch() {
  watchGen += 1;
  for (const d of watchDisposers) { try { d(); } catch (e) { /* ignore */ } }
  watchDisposers = [];
}
const infoListeners = new Set<() => void>();
const infoSubscribe = (cb: () => void) => { infoListeners.add(cb); return () => infoListeners.delete(cb); };
const infoSnapshot = () => info;
const setInfo = (patch: Partial<typeof info>) => {
  info = Object.assign({}, info, patch);
  for (const cb of Array.from(infoListeners)) { try { cb(); } catch (e) { /* ignore */ } }
};

// **连接树帧**（`onTree` 收到的第一条 = 网关按**连接 wt** 取的树，`crdt.py:256-260` 的补发）⇒
// 它就是"网关实际挂载的那棵树"的权威指纹。URL 请求的树（HTTP 取的那份）与它不一致 ⇒ 这个 wt 没被挂载。
let connTreeIds: Set<number> | null = null;   // 只用于播报"连接树帧 N 条"（= 网关按连接 wt 补发的那份）

// `waitSnapshot`（等某文档首个全量帧）已搬进框架：core/services/snapshot.js



registerStep("client", async () => {
  // 甲1：client 步先建主树（app 槽 late、app=null）；换 wt **不重建**（第 2/5 步）——root 只建一次，
  // 每次 boot 重跑只做指针与树册同步（第 5 步 :137「换 WS、把目标树快照 ensure 进册子」的落点）。
  if (!root) {
    root = UiStore.create(
      { wts: { wts: {}, currentWT: WT }, decl: { claims: [], vis: [], errors: [], route: "" }, dev: {}, app: null },
      tools
    );
    ui = root.decl;
    devStore = root.dev;
  } else {
    root.wts.ensure(WT);   // 目标树进册子（appRouter 路径不经 DevStore.switchWT 也照常）
    root.wts.setWT(WT);    // 指针跟连接走（幂等）
  }

  // S4：app 级同页换树通道 —— 壳把自己的 switchWt 注入 appRouter（app 经 appRouter.switchTo(uid) 换树）。
  appRouter.setTreeSwitcher(switchWt);
  // 指针归一入口（基线）：壳注入、app 显式请求。**壳显式请求的落地 = 写指针 + 走既有 `app-doc`
  // 通道**（DevView 监听元素属性 → openByPath → open(e)）——两件基线件接起来，不新增 appRouter API。
  appRouter.setDocPointerSink((p: any) => {
    writeDocPointer(p);
    if (p == null || p === "") return;
    try {
      const el: any = document.getElementById("app");
      // 先清后设:app-doc 同值 setAttribute 不触发 attributeChangedCallback ⇒ 同页再请求会静默丢失
      // (实测:Monaco 打开 stats 后属性仍是 fields,点导航"字段清单"写同值 ⇒ DevView 收不到请求 ⇒ 指针不动)
      if (el) { try { el.removeAttribute("app-doc"); } catch (e2) { /* ignore */ } el.setAttribute("app-doc", String(p)); }
    } catch (e) { /* 通道缺席忽略 */ }
  });
  client = await connectClient(GATEWAY_WS, {
    uid: WT,                       // 工作树店的 uid（?wt=<uid>，缺省 genesis）
    wts: root.wts,   // 容器进 client（第 1 步：构造器 o.wts.ensure 后取用；缺容器退单棵）
    env: tools,                    // 工具袋随树进 root（client.js:426）
    createWorker,                  // wasm web 适配器注入（ui_wasm/worker）；client 缺席回退旧裸路径
    onConnectionState: (s: string) => { setInfo({ conn: s }); },   // 播报由 monitor.watch 负责（WS 侧监控住在 core）
    onTree: (entries: any) => {   // 只记第一条（= 网关按连接 wt 补发的那条），不参与渲染
      if (connTreeIds) return;
      const arr = Array.isArray(entries) ? entries : [];
      connTreeIds = new Set(arr.map((e: any) => Number(e && e.id)).filter((n: number) => !Number.isNaN(n)));
      log("boot: 连接树帧 " + connTreeIds.size + " 条（= 网关挂载的那棵树）");
    },
  });
  log("boot: wasm 适配器引擎版本 = " + ENGINE_VERSION + "（与 lib/wasm/Cargo.toml 同源；网关侧对拍待后端暴露版本字段）");
  // **WS 侧监控接上**（业主口径）：`boot: conn` / `tree: entries` 两行由 monitor 播报，
  // 装配点不再自己打；monitor 包住上面两个回调（先播报、再转发），不夺权。
  if (client.wt && typeof client.wt === "function") client.wt().setLoader(client);   // 装载器：需求入口（wt.observe）要用它（店不 import 服务）   // 装载器：需求入口（wt.observe）要用它（店不 import 服务）
  // 工具后挂（每次 boot 更新；env 属性可写 —— client/exec 是稳定工具）
  tools.client = client;
  // exec 统一记录：app 经 env.exec 的调用不进日志；这里补上（成功/失败都记，与 footer 的 CLI→/CLI← 互补）
  tools.exec = (code: string) => {
    const t0 = (typeof performance !== "undefined" && performance.now) ? performance.now() : Date.now();
    log("EXEC→ " + String(code).slice(0, 140));
    return client.exec(code, { wt_uid: WT }).then((raw: any) => {
      const ms = Math.round(((typeof performance !== "undefined" && performance.now) ? performance.now() : Date.now()) - t0);
      log("EXEC← " + ms + "ms · " + String(raw == null ? "" : raw).slice(0, 200));
      return raw;
    }, (e: any) => {
      log("EXEC✗ " + String((e && e.message) || e).slice(0, 140));
      throw e;
    });
  };
  // 工具随店走(env 范式):renderNode 也后挂进 tools ⇒ app 件 getEnv(app).renderNode()、节点件 getEnv(node).renderNode()
  tools.renderNode = makeRenderNode(ui, tools.exec);
  client.switchTo = switchTo;   // 连接层换树能力：DevStore.switchWT 经 env.client.switchTo 调（第 5 步）
  monitorWatch(client);
  // 补丁帧只用来判「某条全局认领是否零生效」（见 maybeReportDeadSecClaims）；判定与失效都归 MST 自己。
  // 挂在这里（装配点）而不是 DevView：DevView 管"视图在画什么"，命中集的失效是**声明侧**的事。
  // 补丁帧只需一件事：判「某条 section 认领是否零生效」（**不再有 bumpPatch/structRev** —— 结构变化归 MST 自己）。
  client.subscribe((ev: any) => { if (ev && ev.type === "patch") maybeReportDeadSecClaims(); });
  // 文件树（含 /.mds，**按当前 wt 作用域取**）——devview 文件树与 uiMd 的 docId 解析共用这一份。
  // 排掉 /.mds/mounts/**：那是别的 wt 挂进来的树，不属于当前 wt 的文档面（fileview 只显示当前 wt 之下)。
  const ft = await client.fullTree(WT);
  const raw = Array.isArray(ft) ? ft : [];
  const list = raw.filter((e: any) => String(e && e.path || "").indexOf("/.mds/mounts/") !== 0);
  // **文件面写进工作树店**（波 4 · 业主口径：wt 套一个 md map + entries）⇒ 店是唯一来源，这里读回来喂 DevView
  client.wt().putEntries(list.map((e: any) => ({ path: String(e && e.path || ""), docId: Number(e && e.id) })));   // 写边界：通道用 id，本层用 docId
  const files = client.wt().files;   // ★ **读店上的 computed view**（不再手搓 map；专项里 DevView 直接读它）
  setInfo({ treeCount: files.length, entries: files, allEntries: files });
  // 壳态已纯字段化（DevStore 无投影、无 md 锚）⇒ demo_ui.md 不再 observe、不再写回（收口）。
  log("tree: entries " + list.length + "（wt=" + WT + " 作用域，已排其它 wt 的挂载树）");
  log("boot: CRDT connect " + GATEWAY_WS);
});

// **mod 装载线已搬进框架**：`ui_core/src/services/modLoader.js`。
// 为什么搬：装载（md 代码块 / ui 件 / npm 件 / http 件）是**声明 → 取件 → 模块**的公共语义，与宿主无关。
// 该文件**零 import**（判据 R1）：位置语法（locator）、md 装载线（mdLine）、样式采纳（cssAdopt）
// 都由本装配点在下面注入进去 —— 与 `boot.js` / `cssAdopt.js` 同一套接线纪律。

// sel 编译绑定：引擎编译 IR（已实测），**按 sel 文本缓存**——声明热重载每轮都会重编同一批选择器，
// 这是热路径（b3de2941「施工图 B3」删过这份缓存，理由「不是热路径」不成立：实测一次重载 46 次 sel compile）。
const compileSel = cachedCompileSel(async (sel: string) => {
  const raw = String(await client.exec("sel compile " + JSON.stringify(sel), { wt_uid: WT }));
  const o = JSON.parse(raw);
  if (!o || !Array.isArray(o.steps)) throw new Error("sel compile 响应缺 steps");
  return o.steps;
});

/** 已报过「零生效 sec 认领」的轮次键（声明轮次:文档路径）：同一轮只报一次。 */
const deadSecReported = new Set<string>();
/** 已排过延迟重试的轮次键（静态档只来一帧，见下）。 */
const deadSecRetry = new Set<string>();
/**
 * **契约 6 的「sec 零命中硬警告」**（标准模板研究 §2；补）：全局 sec 认领（无 path）
 * 在某档**一个节点都没接走**时打硬警告 —— 选择器拼错/桶位置写错，会完全静默。
 * 判据来源：**离线数**（`ui.claimHitsNow()` —— walk 一遍当前档的树，用同一处判定 `gates` 数每个条目接走多少节点）。
 * 为什么离线（波 3 收尾）：渲染路径不许有写（视图里写状态 MST 会报错），而且渲染期只覆盖"已渲染的可见节点"；
 * 走一遍树才是「这档里它到底有没有命中节点」的准确口径。
 * 时机：帧到达后判；**树还没到**（`ui.doc` 为空）⇒ 排一次延迟重试（同一 key 只排一次）。
 * 只在 `(声明轮次, 文档路径)` 上判一次 ⇒ 不会每帧刷屏。
 */
function maybeReportDeadSecClaims() {
  try {
    const key = (ui as any).declRev + ":" + (ui as any).route;
    if (deadSecReported.has(key)) return;
    if (!(ui as any).doc) {
      // **树还没到**（静态档只来一帧的实测教训）：此时 walk 数不出东西 ⇒ 就此返回会永远等不到第二次机会。
      // 补一次延迟重试（同一 key 只排一次）。
      if (!deadSecRetry.has(key)) {
        deadSecRetry.add(key);
        setTimeout(() => { deadSecRetry.delete(key); maybeReportDeadSecClaims(); }, 1500);
      }
      return;
    }
    const counts: Record<string, number> = (ui as any).claimHitsNow ? (ui as any).claimHitsNow() : {};
    deadSecReported.add(key);
    // 「全局条目」= 没写 path 门控的那些（任何文档都适用）⇒ 它们在**本档**一个节点都没接走 = 选择器拼错。
    // （没有桶：按 bucket === "section" 筛会漏掉守卫。）
    const globalEntries = ((ui as any).claims || []).filter((c: any) => !c.path);
    const dead = globalEntries.filter((c: any) => !counts[c.name]);
    for (const c of dead) log("decl ⚠ " + c.name + "：全局认领（无 path）在本档**零生效**（无节点被它接走）⇒ 检查选择器拼写与路径（契约 6：零命中硬警告）");
    if (dead.length) log("decl ⚠ 零生效 " + dead.length + " 条（逐条见上）· 本档 " + (ui as any).route);
  } catch (e) { /* 诊断不许影响装配 */ }
}

const DECL_PATH = "/.mds/ui.md";
let declDocId: number | null = null;
// 热重载判据的两段事实（md 件源文档路径集合 / 源文本快照）已随装载器搬进框架，见下方 modLoader。

/** 装载/重装声明（boot 与**热重载**共用）。why 只进日志，用于区分来源。 */
/**
 * **诊断分类计数（C7-③）**：`schema` = 声明自己的形状/字段不对；`wiring` = 指向的东西不存在/连不上；
 * `未标` = 新增报错点忘了标 `kind` ⇒ **显式露出**，不静默归桶（这就是分类账的意义）。
 */
function diagCounts(errs: any[]): string {
  let s = 0, w = 0, u = 0;
  for (const e of errs || []) { const t = e && e.kind; if (t === "schema") s += 1; else if (t === "wiring") w += 1; else u += 1; }
  return "schema " + s + " · wiring " + w + (u ? " · **未标 " + u + "**" : "");
}

// ── Step4 · 重装装配面(impl 能力袋):店编排流程(ui_core 的 reload flow),这里供能力 ──

/** 表 md 展开(会话依赖):open+waitSnapshot+树上 sectionOf,一层合并,返回认领条目。 */
async function expandRefs(refs: string[], errors: any[]): Promise<any[]> {
  const rawEntries: any[] = ((ui as any).decl.match || []).slice();
  const claimEntries: any[] = [];
  const seenDoc = new Set<string>();
  for (const e of rawEntries) {
    if (!e || !e.doc) { claimEntries.push(e); continue; }
    if (seenDoc.has(e.doc)) { log("decl ⚠ 表 md 重复引用（只并一次）：" + e.doc); continue; }
    seenDoc.add(e.doc);
    const ent = (info.allEntries || []).find((x: any) => x.path === e.doc);
    if (!ent || ent.id == null) { const msg = "doc 值指向的文档不在文件树里：" + e.doc; errors.push({ kind: "wiring", name: e.name, at: "## match · " + e.name, msg }); continue; }
    try {
      client.open(ent.id);
      const t2 = await waitSnapshot(client, ent.id, 8000);
      const inner: any[] = matchEntriesOf(sectionOf(t2, "match"), errors) as any;
      const nested = inner.filter((x: any) => x && x.doc);
      if (nested.length) { const msg = "只展开一层：被引用的 " + e.doc + " 里又出现 doc 值（" + nested.map((n: any) => n.doc).join(",") + "），已忽略"; errors.push({ kind: "wiring", name: e.name, at: "## match · " + e.name, msg }); log("claim ✗ [wiring] " + e.name + "：" + msg); }
      const flat = inner.filter((x: any) => x && !x.doc);
      for (const x of flat) claimEntries.push(x);
      log("decl: 并入表 md " + e.doc + " 的 " + flat.length + " 条（一层 · 引用名 " + e.name + "）");
    } catch (err) { const msg = "读被引用的表失败：" + String((err && (err as any).message) || err).slice(0, 80); errors.push({ kind: "wiring", name: e.name, at: "## match · " + e.name, msg }); log("claim ✗ [wiring] " + e.name + "：" + msg); }
  }
  if (refs.length) log("decl: 表 md 引用 " + refs.length + " 篇：" + refs.join(","));
  return claimEntries;
}

/** md 件源文档 open(代码块观察的树要靠帧更新;逐篇 open 只是只读订阅)。 */
function openSrcDocs() {
  try {
    const srcIds: number[] = [];
    for (const p of modLoader.modSrcPaths) {
      const ent = info.allEntries.find((x: any) => x.path === p);
      if (ent && ent.id != null && !srcIds.includes(ent.id)) srcIds.push(ent.id);
    }
    for (const id of srcIds) client.open(id);
    if (srcIds.length) log("decl: md 件源文档已开 " + srcIds.length + " 篇（改件 ⇒ 自动重装）");
  } catch (e) { log("decl: md 件源文档订阅失败（不阻塞 boot）：" + String((e && (e as any).message) || e).slice(0, 100)); }
}

/** 失败件 → 错误组件(错误要看得见;不强制重试,修好后下一帧自然重装)。 */
function injectErrorComps(bag: any) {
  for (const e of bag.errors || []) {
    if (String((e as any).msg).indexOf("取件失败") >= 0) {
      bag.mods.set(String((e as any).name), {
        name: String((e as any).name), via: "err", url: "err:" + String((e as any).name),
        ns: { default: modErrorComp(String((e as any).name), String((e as any).msg)) },
      });
    }
  }
}

/** modMatch 编译日志。 */
function logModMatch(name: string, t: any) {
  log("[modMatch] " + name + " rows=" + t.table.length + " errs=" + t.errors.length + " 表=" + t.table.map((r: any) => r.name + "@" + (r.path == null ? "*" : r.path)).join(", ") + " " + JSON.stringify(t.errors.slice(0, 2)));
  for (const er of t.errors) log("modMatch ✗ " + name + "：" + er.msg);
}

/** 诊断输出(纯函数:输入 = 店 + 局部 bags,输出 = log)。 */
function printDiag(uiStore: any, decl: any, declErrors: any[], claimBag: any, visBag: any, why: string) {
  log("boot: decl mods=" + decl.mods.length + " 诊断=" + declErrors.length + " 条（" + diagCounts(declErrors) + "）");
  {
    const fetchE = declErrors.filter((e: any) => String(e.msg).indexOf("取件失败") >= 0);
    const otherE = declErrors.filter((e: any) => String(e.msg).indexOf("取件失败") < 0);
    const allDiag: any[] = declErrors.concat(claimBag.errors || [], visBag.errors || []);
    log("boot: 诊断最终 " + allDiag.length + " 条（" + diagCounts(allDiag) + "）· 按账：decl " + declErrors.length + " / claim " + (claimBag.errors || []).length + " / vis " + (visBag.errors || []).length);
    if (fetchE.length) {
      log("decl ✗ " + fetchE.length + " 件 ui 未就绪（本 wt 链上无对应模块），明细：");
      for (const e of fetchE) log("decl ✗ [" + (e.kind || "未标") + "] " + e.name + "：" + e.msg);
    }
    for (const e of otherE.slice(0, 12)) log("decl ✗ [" + (e.kind || "未标") + "] " + e.name + "：" + e.msg);
    for (const e of (claimBag.errors || []).slice(0, 12)) log("claim ✗ [" + (e.kind || "未标") + "] " + e.name + "：" + e.msg);
    for (const e of (visBag.errors || []).slice(0, 12)) log("vis ✗ [" + (e.kind || "未标") + "] " + e.name + "：" + e.msg);
  }
  log("boot: claims " + claimBag.table.length + " 条 / vis " + visBag.table.length + " 条（判定走 claimMatches → chainWalk（app 注入））· 诊断合计 " + (declErrors.length + (claimBag.errors || []).length + (visBag.errors || []).length) + " 条（" + diagCounts(declErrors.concat(claimBag.errors || [], visBag.errors || [])) + "）· 按账：decl " + declErrors.length + " / claim " + (claimBag.errors || []).length + " / vis " + (visBag.errors || []).length);
  const avName = (decl.app && decl.app.appview) || "";
  log("boot: router appview=" + (avName || "(none)") + (avName ? "（该件接手 appview；文档体由它自己调 renderNode()）" : "（无槽位 ⇒ 缺省 MDBody = comps 的 MDRoot）"));
  if (visBag.table.length) log("boot: vis " + visBag.table.length + " 条（include/exclude 与 match 同管线）");
  const laClaimGlobal = claimBag.table.filter((t: any) => t.path == null).length;
  const laVisDoc = visBag.table.filter((e: any) => e && e.docLevel).length;
  const laVisNode = visBag.table.length - laVisDoc;
  log("层账 · claim：条目 " + claimBag.table.length + " 条（全局 " + laClaimGlobal + " · 带 path 门控 " + (claimBag.table.length - laClaimGlobal) + "）");
  const nonRender = [...new Set(claimBag.table.filter((t: any) => !modCompOf(uiStore.modOf(t.name))).map((t: any) => String(t.name)))];
  if (nonRender.length) log("层账 · 非渲染条目 " + nonRender.length + " 条不参与「谁画」（mod 无组件，match 对它只作作用域）：" + nonRender.join(",") + " —— 样式由装载器页级采纳（§13.6）");
  log("层账 · vis ：文档级 " + laVisDoc + " 条 · 节点级 " + laVisNode + " 条（vis 不分 sec/node：header 就是 type 为 header 的节点，隐藏任一节点 ⇒ 其子树不渲染，区域效果由树结构得到；写了 include 而 0 命中 ⇒ 整篇不可见）");
  log("boot: claims " + claimBag.table.length + " 条" + (claimBag.errors.length ? " / 诊断 " + claimBag.errors.length + " 条：" + claimBag.errors.map((e: any) => e.name + ":" + e.msg.slice(0, 50)).join("；") : ""));
  log("boot: 查表序（低→高 · 声明序）" + claimBag.table.map((t: any) => t.name).join(" → "));
  for (const e of claimBag.errors.slice(0, 12)) log("claim ✗ [" + (e.kind || "未标") + "] " + e.name + "：" + e.msg);
  for (const w of (claimBag.warns || [])) log("claim ⚠ " + w.name + "：" + w.msg);
  const all = Array.isArray(info.allEntries) ? info.allEntries : [];
  setInfo({ entries: all });
  const scope = uiStore.docScope;
  if (scope.include.length || scope.exclude.length) {
    const visible = all.filter((e: any) => uiStore.docVisible(e.path));
    log("decl(" + why + "): AppView 可见文档 " + visible.length + "/" + all.length + " 篇（docScope，只作用于产品面）");
  }
}

// **热重载观察端**（Step1 改造）：declWatch 帧订阅不存在，重装由 registerWatch 的 onSnapshot
// （声明树 + 各 md 件代码块节点）触发；重装本体 = UiDecl.reload（flow，装配点经 setReloadImpl 注入 applyDecl）。

registerStep("uiMd", async () => {
  const entry = info.allEntries.find((e: any) => e.path === DECL_PATH);
  if (!entry) throw new Error("文件树里没有 " + DECL_PATH);
  client.open(entry.id);
  const tree = await waitSnapshot(client, entry.id, 8000);
  (ui as any).setDeclDocId(entry.id);   // 声明文档 id 进店(decl 投影的树;先于重装)
  (ui as any).setAppName(APP_NAME || "");   // app 名(URL ?app= 覆盖)进店
  // Step4 · 装配面能力袋注入店:reload(flow)编排八步,这里供能力(店不 import 装配面)。
  (ui as any).setReloadImpl({
    loadMod,
    compileSel,
    expandRefs,
    openSrcDocs,
    resetSources: () => modLoader.resetSources(),
    beginScope: (decl: any) => modLoader.beginScope(decl),
    pruneCssMods,
    injectErrorComps,
    logModMatch,
    printDiag,
    logLine: (t: string) => log(t),
    registerWatch: (self: any) => registerWatch(self),
  });
  await (ui as any).reload("boot");   // 首装 = 店的 flow action(八步)
  // app 店（volatile box）：类型 per-create —— 载完 app mod 就地 create 真店挂进 root.app；
  // env = 共享工具面 tools。换 wt / 声明重载每次重建（同类型），box 替换即交付跟随。
  {
    const av = APP_NAME || ((ui as any).decl && (ui as any).decl.app && (ui as any).decl.app.appview) || "";
    const rec = av ? (ui as any).modOf(av) : null;
    const ns2 = rec && rec.ns;
    if (root) {
      const prevApp = root.app;
      const keepApp = !!switchPreserveApp
        && !!prevApp
        && ns2 && ns2.Store
        && ns2.reloadOnWtSwitch === false
        && getType(prevApp) === ns2.Store;
      switchPreserveApp = false;
      if (keepApp) {
        log("app store: keep on wt switch · " + av);
        try {
          if (typeof ns2.onWtSwitch === "function") ns2.onWtSwitch(prevApp, tools);
        } catch (e) {
          log("app store: onWtSwitch failed ⇒ recreate · " + String((e && e.message) || e).slice(0, 120));
          root.setApp(null);
          root.setApp(ns2.Store.create({}, tools));
        }
      } else {
        root.setApp(null);   // 先清（换 app 时旧店退场）
        if (ns2 && ns2.Store) root.setApp(ns2.Store.create({}, tools));
      }
    }
  }
  // **认领策略归属（拍板）**：口子 ui_core · 默认策略 ui_std · host 在装载后直装（首渲前）。
  //   appEntryOf 里 app 自提供策略时才覆盖；缺席 ⇒ std 常驻。
  setClaimEval(stdClaimEval(READ));
  setClaimWalk(stdChainWalk(walkPrims));
  declDocId = entry.id;
  // **基准必须在订阅之前定**（改）：`prime()` 只把"当前生效的声明文本"记成热重载基准。
  //   旧写法是 `void prime()`（不阻塞）+ 紧接着 subscribe ⇒ **竞态**：首帧若先到，基准还是 null ⇒
  //   `reloadDecl` 跳过内容比对 ⇒ 白白重装一次 ⇒ `declRev` 变 ⇒ **整篇文档第二次渲染**（业主问的"为何渲染两次"）。
  //   现在：先 await 基准，再订阅。基准本身仍**不阻塞 boot**（失败只记一行，页照常渲染）。
  // **热重载两种**：① 改 /.mds/ui.md（声明；含组件自己写回，如 swap 演示）
  //                ② 改**任一 md 件的源文档**（代码也是数据 ⇒ 在 Monaco 里改 jsx@page 块里的组件代码应当立刻生效）。
  // Step1c · 帧订阅不存在:重装由 registerWatch 的 onSnapshot 触发(MST 观察,数据写天然不触发)。
  log("decl: MST 观察已挂(改 ui.md 或任一 md 件源文档 ⇒ 自动重装)");
});

// ── 面板 app 槽（左 = MONACO_APP / 右 = LOG_APP；槽位化）────────────────
// 口径：槽只解决「组件代码从哪棵树取」；数据面 = 当前页面 wt
// （store 的 env 用页面 tools：client / getMainRoot / dev 指针全是页面的）。
/** 极小的 ui.md 读取器：只拿 ## mods 与 ## app - appview。
 *  为什么直读文本：槽位不另造第二份声明店（UiDecl 的八步装配属于当前页那棵树）；这里只要“装哪个件”。 */
function parsePanelDecl(text: string) {
  const out = { appview: "", mods: [] as Array<{ name: string; spec: string }> };
  let section = "";
  for (const raw of String(text || "").split(/\r?\n/)) {
    const line = raw.trim();
    const h2 = /^##\s+(.+)$/.exec(line);
    if (h2) { section = h2[1].trim().toLowerCase(); continue; }
    const row = /^[-*]\s+([A-Za-z0-9_.-]+)\s*:\s*(.+)$/.exec(line);
    if (!row) continue;
    if (section === "mods") out.mods.push({ name: row[1], spec: row[2].trim() });
    else if (section === "app" && row[1].toLowerCase() === "appview") out.appview = row[2].trim();
  }
  return out;
}
/** 每个代码宿主 wt 一个 modLoader（deps 的 getWt 绑到该 wt；md 代码块经 HTTP exec 带 wt_uid 取）。 */
const panelLoaderCache = new Map<string, any>();
function panelModLoaderOf(codeWt: string) {
  let L = panelLoaderCache.get(codeWt);
  if (!L) {
    L = createModLoader({ getClient: () => client, getWt: () => codeWt, loadMd, loadScopedMd, createScope: createMdScope, parseSpec, uiModuleOf, resolveUiSpec, wtChain, adoptCssMod, log });
    panelLoaderCache.set(codeWt, L);
  }
  return L;
}
/** 装一个面板 app：只从 codeWt 取「声明 + 组件」；store 用页面 tools。 */
async function loadPanelApp(codeWt: string, appviewName?: string) {
  if (!codeWt || !client) return null;
  const text = String(await client.readDoc(codeWt, "/.mds/ui.md") || "");
  const d = parsePanelDecl(text);
  const appview = String(appviewName || d.appview || "");
  if (!appview) throw new Error("代码 wt 的 /.mds/ui.md 里没有 ## app - appview");
  const L = panelModLoaderOf(codeWt);
  try { L.beginScope({ mods: d.mods }); } catch (e) { /* scope 建不起仍试直装 */ }
  const loaded = new Map<string, any>();
  for (const m of d.mods) {
    try { loaded.set(m.name, await L.loadMod(m.name, m.spec)); }
    catch (e: any) { log("panel mod 装载失败 " + m.name + "：" + String((e && e.message) || e).slice(0, 120)); }
  }
  const rec = loaded.get(appview);
  if (!rec) throw new Error("appview 件没装上：" + appview);
  const App = modCompOf(rec);
  if (!App) throw new Error("appview 件不能画：" + appview);
  const ns: any = rec && rec.ns;
  const store = ns && ns.Store ? ns.Store.create({}, tools) : null;
  log("panel app: " + codeWt + " → " + appview + " · mods=" + d.mods.length + " · 实装=" + loaded.size);
  return { App, store, codeWt };
}
/** 槽位配置（槽位编排归 devview/壳）：默认值在 DevStore.panels；装配点只读。URL 参数仍可覆盖。 */
function panelCfg() {
  const p = devStore && devStore.panels;
  return {
    left: String((p && p.left && p.left.appview) || "MONACO_APP"),
    log: String((p && p.log && p.log.appview) || "LOG_APP"),
    foot: String((p && p.foot && p.foot.appview) || "CONSOLE_APP"),
  };
}

/** 槽位默认解析（方案 1：用 ui.md 的 appview 身份，不依赖 README 约定）：
 *  URL 没给 &leftwt=/&logwt=/&footwt= 时，从网关运行时 wt 列表里找 ## app - appview: 等于槽配置值的树。
 *  为什么要扫：wt 注册是 runtime 的（uid 每次重启会变），不能把 uid 写死。 */
async function resolvePanelDefaults() {
  const cfg = panelCfg();
  const missing = () => (!LEFT_WT && !!cfg.left) || (!LOG_WT && !!cfg.log) || (!FOOT_WT && !!cfg.foot);
  if (!missing()) return;
  const res = await fetch("/control/status");
  if (!res.ok) throw new Error("control/status " + res.status);
  const j: any = await res.json();
  const uids: string[] = Array.isArray(j && j.wts) ? j.wts.filter((x: any) => typeof x === "string") : [];
  for (const uid of uids) {
    if (!missing()) break;
    try {
      const text = String(await client.readDoc(uid, "/.mds/ui.md") || "");
      const av = parsePanelDecl(text).appview;
      if (!av) continue;
      if (av === cfg.left && !LEFT_WT) LEFT_WT = uid;
      else if (av === cfg.log && !LOG_WT) LOG_WT = uid;
      else if (av === cfg.foot && !FOOT_WT) FOOT_WT = uid;
    } catch (e) { /* 无声明/未挂载：跳过 */ }
  }
  log("panel defaults: left=" + (LEFT_WT || "-") + " · log=" + (LOG_WT || "-") + " · foot=" + (FOOT_WT || "-") + "（by appview " + cfg.left + "/" + cfg.log + "/" + cfg.foot + " · scan " + uids.length + " wts）");
}

registerStep("panelApps", async () => {
  try { await resolvePanelDefaults(); } catch (e: any) { log("panel defaults 解析失败：" + String((e && e.message) || e).slice(0, 120)); }
  if (!LEFT_WT && !LOG_WT && !FOOT_WT) return;
  const cfg = panelCfg();
  if (LEFT_WT) {
    try {
      const p = await loadPanelApp(LEFT_WT, cfg.left);
      setInfo({ leftPanel: p });
    } catch (e: any) {
      log("panel app 装载失败（left " + LEFT_WT + "）：" + String((e && e.message) || e).slice(0, 160));
      setInfo({ leftPanel: null });
    }
  }
  if (LOG_WT) {
    try {
      const p = await loadPanelApp(LOG_WT, cfg.log);
      setInfo({ logPanel: p });
    } catch (e: any) {
      log("panel app 装载失败（log " + LOG_WT + "）：" + String((e && e.message) || e).slice(0, 160));
      setInfo({ logPanel: null });
    }
  }
  if (FOOT_WT) {
    try {
      const p = await loadPanelApp(FOOT_WT, cfg.foot);
      setInfo({ footPanel: p });
    } catch (e: any) {
      log("panel app 装载失败（foot " + FOOT_WT + "）：" + String((e && e.message) || e).slice(0, 160));
      setInfo({ footPanel: null });
    }
  }
});

const bootingStyle = {
  display: "flex", alignItems: "center", justifyContent: "center",
  minHeight: "100vh", fontFamily: "ui-monospace, Menlo, monospace", fontSize: 14, opacity: 0.7,
};

/**
 * 渲染入口（app 槽位）：声明里 `## app` / `- appview: NAME` 指名的件 = appview 这块地的主人（排他）。
 *   装配点交给它 `{ ui, exec, appRouter, wt }`（4 键）：read/renderNode 由 ui_std 自产，app 从 ui_std import。
 *   **漏配大声报账**（P2-2 · 决定 3）：**产品面**（`?app=` 存在）没有可用 app ⇒ 出漏配屏 + `report`；
 *   **开发面**才退回明文 `MDBody`（comps 的 `MDRoot`）（那是开发者视图的正当缺省，不是漏配）。
 * 身份稳定：包装器按 (ui, name, 件函数) 缓存 —— DevView 的 MemoDocApp 用 memo 比较 props，
 *   每次渲染新建函数会让 memo 永远失效（与 NodeView.memoOf 同一条纪律）。
 * 判据：带槽位的树产品面出现该件的 DOM 标记；不带槽位的树 DOM 与今天逐字节一致。
 */
/** 漏配屏（P2-2）：产品面没有可用 app ⇒ 大声报账，**不静默兜底**（业主决定 3）。
 *  同一 (槽位名, 原因) 只报一次，避免每次渲染刷屏。 */
const misconfigReported = new Set<string>();
/** 「app 没给认领策略」每个 app 只报一次（与漏配屏同一款去重）。 */
const noClaimEvalReported = new Set<string>();
function Misconfig(p: any) {
  const name = String((p && p.name) || "(未指定)");
  const reason = String((p && p.reason) || "");
  const key = name + "|" + reason;
  if (!misconfigReported.has(key)) {
    misconfigReported.add(key);
    try { report("AV_MISCONFIG: appview 漏配 · " + key); } catch (e) { /* 监控未就绪也不吞错 */ }
    try { console.error("[mdio] appview 漏配：", key); } catch (e) { /* ignore */ }
  }
  return h("div", { className: "dv-eb", role: "alert", "data-av-misconfig": key },
    h("div", { className: "dv-eb-ttl" }, "appview 漏配（产品面没有可用的 app）"),
    h("div", { className: "dv-eb-msg" }, "槽位 = " + name + " · 原因 = " + reason),
    h("div", { className: "dv-eb-msg" }, "修法：在 `/.mds/ui.md` 的 `## mods` 登记该件，并让 `## app - appview:` 指向它；或清掉 `app-name` 属性。"));
}

/** 装载失败的件 ⇒ 错误卡（错误看得见：认领到的节点在 ErrorBoundary 边界内渲染错误卡；错误明细逐条进日志）。
 *  框架不替 app 做决定 —— 不强制重试；修好后下一帧按判据自然重装恢复。 */
function modErrorComp(name: string, msg: string) {
  return function ModErrorComp() {
    return h("div", { "data-mod-error": name, style: { border: "1px solid #f5222d44", borderRadius: 12, background: "#1c1e26", color: "#ff7875", padding: "16pt", fontSize: "13px", lineHeight: 1.6, fontFamily: "system-ui" } },
      h("div", { style: { fontWeight: 650, marginBottom: "6pt" } }, "件装载失败 · " + name),
      h("div", { style: { whiteSpace: "pre-wrap", wordBreak: "break-word", opacity: 0.85 } }, msg));
  };
}

/** `App` prop 要的是**组件**，不是元素 —— 直接返回 `h(Misconfig, …)` 会触发 React #130（实测）。 */
function makeMisconfigApp(name: string, reason: string) {
  return function MisconfigApp() { return h(Misconfig, { name: name, reason: reason }); };
}

const appWrapCache = new WeakMap<any, Map<string, { C: any; W: any }>>();
function appEntryOf(uiRef: any): any {
  // `app-name` 属性（mdio:app-change）优先；没写就用声明里的 `## app - appview:`。
  const av = APP_NAME || ((uiRef as any).decl && (uiRef as any).decl.app && (uiRef as any).decl.app.appview) || "";
  if (!av) return APP_DOC != null ? makeMisconfigApp("", "声明里没有 `## app - appview:`，也没有 app-name 属性") : MDBody;
  const rec = uiRef && uiRef.modOf ? uiRef.modOf(av) : null;
  const C = modCompOf(rec);
  // 这里**不许再写一遍「能不能画」**：`modCompOf` 已经是唯一判据（且它认 memo/forwardRef/lazy ——
  // `observer(...)` 返回的正是 memo 对象）。我只认「有没有拿到件」，合法性由 loadDecl 记诊断。
  if (!C) return APP_DOC != null ? makeMisconfigApp(av, "mods 里找不到该件（或它不是可画组件）") : MDBody;
  // **「件 ≠ app」报账**（P2-2 补丁 · 实测：把 `PAPER` 当 app 挂 ⇒ 不报错但渲染为空）：
  //   判据 = 槽位名出现在 `## match` 的条目里 ⇒ 它被声明为「认领某片文档的件」，而 app 是由 `## app - appview:` 选的。
  //   只报账、**不阻断**（特殊用法可能存在）。
  try {
    const d2: any = (uiRef as any).decl;
    const asClaim = !!(d2 && Array.isArray(d2.match) && d2.match.some((x: any) => x && x.name === av));
    const key2 = "claim-as-app|" + av;
    if (asClaim && !misconfigReported.has(key2)) {
      misconfigReported.add(key2);
      try { report("AV_MISCONFIG: appview 槽位指向 `## match` 里的件（不是 app）· " + av); } catch (e) { /* ignore */ }
      try { console.warn("[mdio] appview 槽位指向件而非 app：", av); } catch (e) { /* ignore */ }
    }
  } catch (e) { /* 诊断失败不影响挂载 */ }
  // **app 契约新增（可选导出）`createClaimEval(read)`**（跟读单 B2「认领与可见性归 basic_render」，落地）：
  //   宿主在这里把 app 的策略**装进框架口子**，且必须赶在**首次渲染之前**（`Node.get gates` 是 MST computed，
  //   装晚了早先算出的结果会留在缓存里）。缺席 ⇒ 口子留空（无人认领 ⇒ 未认领节点由声明里的默认渲染器兜），
  //   并按「不静默」原则**报一行账**（每个 app 只报一次）。
  try {
    const ns: any = rec && (rec as any).ns;
    const make = ns && ns.createClaimEval;
    if (typeof make === "function") setClaimEval(make(READ));
    else {
      // std 默认常驻（host 在装载后已直装）——app 不覆盖就不动；只报一行信息（语义反转）。
      if (!noClaimEvalReported.has(av)) {
        noClaimEvalReported.add(av);
        try { report("AV_STD_CLAIMEVAL: app " + av + " 未自提供策略 ⇒ 用 std 默认（host 已装）"); } catch (e) { /* 监控未就绪不吞错 */ }
      }
    }
    // **A2 真搬**：「链上递降」也由 app 提供 —— `createChainWalk(walkPrims)` 装进框架口子（`uiMd.setClaimWalk`）。
    //   缺席 ⇒ 口子留空，**判定会抛错**（不静默、不兜底）。
    const makeWalk = ns && (ns as any).createChainWalk;
    if (typeof makeWalk === "function") setClaimWalk(makeWalk(walkPrims));
    // 缺席 ⇒ std 默认常驻（host 已装），不动。
  } catch (e) { /* 装载期诊断不阻断挂载 */ }
  let m = appWrapCache.get(uiRef);
  if (!m) { m = new Map(); appWrapCache.set(uiRef, m); }
  const hit = m.get(av);
  if (hit && hit.C === C) return hit.W;
  // 入口件的 props 契约（**第三方开发者只认这几个**）——**4 键（瘦身）**：
  //   · `ui`         —— 声明店（MST UiDecl：claims / mods / vis + `route`）
  //   · `exec`       —— 命令通道（经 wasm 到网关，一次真实往返）
  //   · `appRouter`  —— 页内路由能力（hash 视图：page / push / subscribe / href + setRoute / switchTo）
  //   · `wt`         —— 当前工作树店（`client.wt()` 原样传入：`uid`/`by`/`entries` + `files`/`byPath`/`phaseOf` 视图；需求入口 `wt.observe(path)`）
  //   决策点乙定案：appview 只收 { store, renderNode } 两个 prop ——
  //   · store = app 自己的店节点（root.app）；ui/wt 经 envOf(store).getMainRoot() 桥（修订二）、exec/read/appRouter 经 envOf(store)（4 键退休）；
  //   · renderNode = 框架交付面（文档体原语，运行时用 ui/exec 拼好）—— 不在退休的 4 键里。
  // **自动 observer**：入口件由框架包一层，读 store 相关 observable 自动重渲。
  const W = observer(function AppEntry(p: any) {
    // 契约 { app }：app = 总店 app 槽挂的 app 店；工具（renderNode/read/exec…）一律 getEnv(app) 取（env 范式）
    return h(C, { app: root ? root.app : null });
  });
  m.set(av, { C: C, W: W });
  return W;
}
/** 缺省渲染入口：模块级恒定身份（旧写法是内联箭头，每次渲染换身份 ⇒ MemoDocApp 的 memo 失效）。 */
function MDBody(p: any) {   // B1 基线契约：兜底屏 = comps 的 MDRoot（明文文档体；框架不再持有组件）
  return h(MDRoot, { renderNode: makeRenderNode(p && p.ui, p && p.exec) });
}

function BootScreen() {
  const s = useSyncExternalStore(subscribe, getState);
  // info（文件树列表等）变了也要重渲：声明按 docScope 过滤后的可见列表必须落到 UI 上
  // （只有 boot 状态订阅 ⇒ setInfo 更新了数据但界面不刷新，实测踩过）
  useSyncExternalStore(infoSubscribe, infoSnapshot);
  /**
   * **启动超时看门狗**：装载期的步骤若"既不抛错也不返回"（宿主/网关没起来、产物缺失），
   * 页面会永远停在装载态 —— 用户既看不到原因、也没有重试入口（实测踩过：`正在装载…` 挂半天）。
   * 超时后走**与步骤失败同一条**路径（同一个错误卡 + 刷新按钮）⇒ 有原因、有出口。
   * 注意：hook 必须**无条件**调用，所以这条 useEffect 放在早退之前。
   */
  useEffect(() => {
    if (s.state !== "booting") return;
    // 阈值回到 **10s**（业主指示）：先前之所以被"误杀"，真凶是 mdLine 在运行期从 esm.sh 取
    // @babel/standalone（~10s 起且常失败）—— 改用 /vendor/ 本地直出 ⇒ 换树首屏 1.5s。
    // 因而 10s 不再短于任何合法工作，可作为"真的没进展"的兜底；各步自身超时（连接 10s/worker 15s/首帧 8s）依旧。
    const t = setTimeout(() => {
      failBoot("bootTimeout", "装载超过 10 秒仍未完成：宿主或网关可能未就绪，或某个 mod 的产物缺失（控制台搜「boot: 步骤失败」看具体步骤）");
    }, 10000);
    return () => clearTimeout(t);
  }, [s.state]);
  if (s.state === "booting") {
    return h("div", { style: Object.assign({}, bootingStyle, { flexDirection: "column" }) },
      h("style", null, "@keyframes mdiospin{to{transform:rotate(360deg)}}"),
      h("div", { style: { width: 26, height: 26, border: "2px solid #ffffff22", borderTopColor: "#7aa2f7", borderRadius: "50%", animation: "mdiospin .9s linear infinite" } }),
      h("div", { style: { marginTop: 12, opacity: 0.8, fontSize: 13 } }, "正在装载…"),
      h("div", { style: { marginTop: 6, opacity: 0.4, fontSize: 11 } }, "超过 10 秒仍未完成会给出原因与重试入口"),
    );
  }
  if (s.state === "failed") {
    // **结构化失败卡**（重做）：把消息拆成「原因 / 该做什么」两段，命令单独成框、可一键复制；
    // 所有信息都来自 failBoot 的原始消息（不新造字段）：只认 `换树（在同一终端跑）：` 这个我们自己写下的分隔符。
    const msg = String((s.error && s.error.message) || "");
    const step = String((s.error && s.error.step) || "");
    const MARK = "换树（在同一终端跑）：";
    const mi = msg.indexOf(MARK);
    const detail = (mi >= 0 ? msg.slice(0, mi) : msg).trim();
    const cmd = mi >= 0 ? msg.slice(mi + MARK.length).trim() : "";
    const pick = () => { try { const r = document.createRange(); r.selectNodeContents(document.getElementById("mdio-cmd") as Node); const sel = window.getSelection(); if (sel) { sel.removeAllRanges(); sel.addRange(r); } } catch (e) { /* 选不中就手动拖 */ } };
    const copy = () => { pick(); try { void navigator.clipboard.writeText(cmd); } catch (e) { /* 无剪贴板权限时已选中，手动复制 */ } };
    /** **失败屏的退路（业主指示）**：除「刷新」外给一条**回得去**的路 ——
     *   有同源上一页就先回上一页（`document.referrer`，且不是本页）；否则回 **genesis**（`?wt=genesis`，丢掉 `app` 与 hash）。
     *   为什么不是 `history.back()`：直达/新标签进来的历史里可能没有上一页，而失败树本身没有价值。 */
    const goBack = () => {
      try {
        const ref = String(document.referrer || "");
        if (ref && ref.indexOf(location.origin) === 0 && ref !== location.href) { location.href = ref; return; }
      } catch (e) { /* 无 referrer ⇒ 退 genesis */ }
      try {
        const u = new URL(location.href);
        u.searchParams.set("wt", "genesis");
        u.searchParams.delete("app");
        u.hash = "";
        location.href = u.toString();
      } catch (e) { location.href = "/?wt=genesis"; }
    };
    return h("div", { style: { minHeight: "100vh", display: "flex", alignItems: "center", justifyContent: "center", background: "#15161c", padding: 24 } },
      h("div", { style: { width: 560, maxWidth: "100%", border: "1px solid #f5222d44", borderRadius: 12, background: "#1c1e26", color: "#e6e6e6", boxShadow: "0 8px 32px #00000066", overflow: "hidden" } },
        h("div", { style: { display: "flex", alignItems: "center", gap: 8, padding: "12px 16px", background: "#2a1d20", borderBottom: "1px solid #f5222d33" } },
          h("span", { style: { width: 20, height: 20, borderRadius: "50%", background: "#f5222d33", color: "#ff7875", display: "inline-flex", alignItems: "center", justifyContent: "center", fontSize: 12, flex: "0 0 auto" } }, "!"),
          h("b", { style: { fontSize: 14 } }, step === "wtGate" ? "这棵树没被网关挂载" : "启动失败：" + step),
        ),
        h("div", { style: { padding: "14px 16px", fontSize: 13, lineHeight: 1.7, whiteSpace: "pre-wrap", wordBreak: "break-word", color: "#c9ccd6" } }, detail),
        cmd
          ? h("div", { style: { margin: "0 16px 14px", border: "1px solid #3a3f4b", borderRadius: 8, background: "#111318" } },
              h("div", { style: { display: "flex", alignItems: "center", justifyContent: "space-between", padding: "6px 10px", borderBottom: "1px solid #2a2e38", fontSize: 11, color: "#8b93a7" } },
                h("span", null, "在同一终端运行"),
                h("button", { onClick: copy, style: { border: "1px solid #3a3f4b", borderRadius: 5, background: "#1c1f27", color: "#c9ccd6", fontSize: 11, padding: "2px 8px", cursor: "pointer" } }, "复制"),
              ),
              h("code", { id: "mdio-cmd", onClick: pick, title: "点击全选", style: { display: "block", padding: "10px 12px", fontFamily: "ui-monospace, Menlo, monospace", fontSize: 12, color: "#9ece6a", whiteSpace: "pre-wrap", wordBreak: "break-all", cursor: "text", userSelect: "all" } }, cmd),
            )
          : null,
        h("div", { style: { display: "flex", alignItems: "center", gap: 8, padding: "10px 16px", borderTop: "1px solid #2a2e38", fontSize: 11, color: "#6b7280" } },
          h("span", null, String((s.error && s.error.at) || "")),
          h("span", { style: { flex: 1 } }),
          h("button", { onClick: goBack, title: "返回 genesis（有同源上一页就先回上一页）", style: { padding: "4px 14px", borderRadius: 6, border: "1px solid #3a3f4b", background: "#1c1f27", color: "#c9ccd6", cursor: "pointer" } }, "返回"),
          h("button", { onClick: () => location.reload(), style: { padding: "4px 14px", borderRadius: 6, border: "1px solid #3a3f4b", background: "#232733", color: "#e6e6e6", cursor: "pointer" } }, "刷新"),
        ),
      ),
    );
  }

  // **面 = 壳自己的状态**（`?app=` 只是它在 URL 上的投影）：
  //   开发面 = 看 mods（md 里的 js / 作为数据源的 md）—— 外壳整块（三栏 + Monaco + 日志 + footer），文档面显示**明文文档**；
  //   产品面 = 正经 app —— **入口件就是整页的根**（全视野归 appview），外壳整块隐藏（display:none，**不卸载** ⇒ Monaco 会话不断、不重载）。
  //   开发者模式 = 纯 state（DevView 本地）；当前 route 经壳 setRoute 投影到 ?route=。
  if (client && (ui as any).decl) {
    // **一棵树**：?app= 只是"产品面初始态"（隐藏三栏 + footer），不再走另一棵树
    // ⇒ 开发者模式开关是就地显隐，不重载、不重连、Monaco 会话不断（用 location.href 换路由 ⇒ 整页重载）。
    // AppView（?app=）的初始文档也过 docScope：指向被排除的文档时退到第一篇可见档
    const initialDoc = (() => {
      if (APP_DOC == null || APP_DOC === "") return null;   // 有面、无入口文档 ⇒ 交给 app 自己的页（P2）
      try {
        const p = "/" + String(APP_DOC).replace(/^\/+/, "");
        if (ui.docVisible(p)) return p;
        const vis = (info.entries || []).filter((e: any) => ui.docVisible(e.path));
        const fb = (vis.find((e: any) => String(e.path).endsWith(".md")) || {}).path || null;
        if (fb) log("AppView: " + p + " 不在 docScope 内 ⇒ 退到 " + fb);
        return fb;
      } catch (e) { return APP_DOC; }
    })();
    return h(DevView, {
      client,
      entries: info.entries,
      ui,                     // 声明 store（claims + mods）——组件选择在 doc/compTable.jsx 的 compFor(ui, node)
      // **devview 包裹 appview**：`#app` 就在 workspace 里、被 devview 包围，但**不属于 devview**（它只是借了这一格）。
      // 开发者模式控制的是 **workspace 三栏 + footer 的可见性**，**header 永远显示**。
      App: appEntryOf(ui),    // appview：`## app` 的 appview 指名的件；缺省 = 明文 MDBody（comps 的 MDRoot）
      // 左格 app（S4 收口）：组件来自 &leftwt= 指定的代码宿主 wt；店用页面 tools（数据面=当前页 wt）。
      // 没有 leftwt ⇒ 左槽为空占位。
      leftApp: info.leftPanel ? info.leftPanel.App : null,
      leftStore: info.leftPanel ? info.leftPanel.store : null,
      // footer 槽 app（footer app 化）：组件来自 &footwt= 或按 CONSOLE_APP 解析；店用页面 tools
      footApp: info.footPanel ? info.footPanel.App : null,
      footStore: info.footPanel ? info.footPanel.store : null,
      // S1 右格 log app：组件来自 &logwt= 指定的代码宿主 wt；店用页面 tools（数据面=当前页 wt）
      logApp: info.logPanel ? info.logPanel.App : null,
      logStore: info.logPanel ? info.logPanel.store : null,
      wt: WT,                 // 当前 wt（?wt=<uid>）：console 与交付给认领组件的 exec 都用它
      // 链的语义是 **[自己, 父, …, 根]**（locator.js「子覆父」）⇒ **根在末尾**。
      // 取 [0] 拿到的是自己：wt 切换清单会去读本树那份不存在的 `/.mds/worktrees.md`，
      // 解析出 0 条 ⇒ 下拉退化成「标题 = uid」的一项（实测：子 wt 里点开只剩自己的 uid，切无可切）。
      rootWt: ROOT_WT,        // **根树 uid** 由装配点给（开机算一次；见上方常量处的成因注）
      onSwitchWt: switchWt,   // 换树（不刷新）：外壳调它；缺席时外壳退回整页导航（别的宿主实现仍可用）
      devStore: devStore,        // S2：壳态店（DevView 的 Header/主题效应读它）
      conn: info.conn,           // 连接状态（事件派生 → Header 派生 prop，修订 F）
      bootError: (s.state === "failed" && s.error && s.error.message) ? String(s.error.message) : null,   // boot 失败原因（修订 F）
      appRouter,              // 页内段能力（共享单例）：外壳订阅它、入口件用它拼真链接
      onDocPointer: writeDocPointer,   // 归一写点收敛到模块级 writeDocPointer（app 侧入口 = appRouter.setDocPointer）
      onSyncFace: (wantProduct: boolean, doc: string) => {
        APP_DOC = wantProduct ? String(doc || APP_DOC || "/README.md") : null;
        try {
          history.replaceState(null, "", "/?wt=" + encodeURIComponent(WT) + (APP_DOC !== null ? "&app=" + encodeURIComponent(APP_DOC) : "") + String(location.hash || ""));
        } catch (e) { /* 无 history 环境忽略 */ }
      },
      initialDoc,             // ?app=/x.md ⇒ 直接开这篇（docScope 外则退到可见档）
    });
  }
  return h("div", { style: bootingStyle }, "demo ready（等待声明装载）");
}

// ── app 那一格（`#app`）的**边界 sheet**：DOM/CSS 隔离原语的第一次真实使用 ──
//   · `@scope (#app) { … }`：这份样式只在 app 的地里生效（app 的样式世界边界）；
//   · `:scope { isolation: isolate }`：给 app 新建一个**层叠上下文** —— 里面 z-index / mix-blend / filter 不再与外壳互相影响。
//   更强的一档（影子根 = CSS/JS 硬边界）由 `ui_core/domScope` 的 `ensureIsolatedRoot` 提供，按需接。
//   · `@esmap/guard` 的**属性前缀法**（`[data-esmap-scope="mdio-app"]`，由 ui_appview 的元素给 `#app` 打上）；
(() => {
  try {
    const s = new CSSStyleSheet();
    s.replaceSync(scopeCssByAttr(":root { isolation: isolate; }", "mdio-app"));
    adoptInto(document, s);
  } catch (e) { /* 老浏览器没有 CSSStyleSheet 构造器 ⇒ 跳过（不影响功能） */ }
})();
// ── appview 容器（独立 mod `ui_appview`）：注册元素 + **驱动在这里** ──────────────────
//   元素负责「影子根 + 挂载点 + 生命周期事件」；驱动负责：登记样式落点（影子模式）、把层落点注入边界。
//   事件能到 document 是因为元素派发带了 `bubbles: true`（只写 composed 到不了 —— 实测）。
defineAppView();
const appMirrorHandles = new WeakMap<Element, { stop: () => void }>();   // ④ 的句柄：每个 appview 各一份（unmount 时按元素还原/摘根）
/** appview 那个 app 的**文档路径**：从声明里找 `## app - appview:` 指名的 mod，再取它的文档。
 *  两种行形状都认：`- NAME: path::sel`（path 字段）与 `- NAME: /x.md`（简化形式 ⇒ spec 里有 `.md::/x.md::`）。 */
function appDocOf(): string {
  const name = APP_NAME || ((ui as any).decl && (ui as any).decl.app && (ui as any).decl.app.appview) || "";
  if (!name) return "";
  const row = (((ui as any).decl && (ui as any).decl.mods) || []).find((m: any) => m && m.name === name);
  if (!row) return "";
  if (row.path) return String(row.path);
  const s = String(row.spec || "");
  const i = s.indexOf(".md::");
  if (i < 0) return "";
  const rest = s.slice(i + 5);
  const j = rest.indexOf("::");
  return j < 0 ? rest : rest.slice(0, j);
}

/** appview 的**中心格**：`#app` 是唯一带 layer 落点与中心 css@app 的格。第二格出现后，这条是宿主分发的硬边界。 */
function isCenterAppview(el: Element | null | undefined): boolean { return !!el && el.id === "app"; }

document.addEventListener("mdio:app-ready", (ev) => {
  const el = (ev.target as unknown as (HTMLElement & { shadowRoot?: ShadowRoot | null }) | null) || null;
  const center = isCenterAppview(el);
  if (center && BRIDGE) {
    try {
      const av = APP_NAME || ((ui as any).decl && (ui as any).decl.app && (ui as any).decl.app.appview) || "";
      window.parent.postMessage({ type: "mdio-host:app", app: av, wt: WT }, "*");
    } catch (e) { /* no parent */ }
  }
  const d = ((ev as CustomEvent).detail || {}) as { shadowRoot?: ShadowRoot | null };
  if (!d.shadowRoot) {
    if (center) appRouter.setLayerRoot(null);   // 无影子：层的落点回 document.body（今天的行为）
    return;
  }
  const root = d.shadowRoot;
  registerStyleRoot(root);            // 框架已采纳的 sheet 镜像进影子根（含 ui_std 排版 / wt 的 CSS mod）
  // ④ 自动接管动态 `<style>`：复制进影子根 + 用 guard 把原件圈进 mdio-app（不再泄漏到开发壳）。
  //    多根版本：后加入的根由同一份观察者补副本；句柄按元素记，unmount 时只摘自己这一根。
  if (el) appMirrorHandles.set(el, startDynamicStyleMirror(root));
  // ③ `<link>`（esmx 联邦 CSS 走 head）在影子根里看不见 ⇒ 读成 sheet 采纳进去；联邦 CSS 可能晚到，故再补一次。
  mirrorStyleLinks(root);
  setTimeout(() => { mirrorStyleLinks(root); }, 1500);   // 联邦 CSS 可能晚到 ⇒ 补一次
  if (!center) {
    // 非中心 appview（如 devview 左格）：只做样式落点，不接管 layer 落点 / 中心 css@app / BRIDGE。
    log("appview: panel 样式落点已登记 · " + (el && el.id ? "#" + el.id : "(无 id)"));
    return;
  }
  appRouter.setLayerRoot(root);     // 面板落进边界内
  // **app 自己的样式**：app 文档里的 ```css@app 块 ⇒ 直接采纳进影子根（app 只关心自己那一格，样式跟着它走）。
  void (async () => {
    try {
      const cssDocPath = appDocOf();   // 局部名不用旧「文档路径」叫法（§1.1：前端自造的 `Doc*`/旧字段名 = 0；判据段 2 扫原文，连注释也算）
      if (!cssDocPath) return;
      const cssText = String(await client.exec('$ ' + cssDocPath + '::..*_c[c.t=="css@app"] -pj ct', { wt_uid: WT, active_doc: cssDocPath }) || "");
      if (!cssText.trim()) return;
      const s = new CSSStyleSheet();
      s.replaceSync(cssText);
      adoptInto(root, s);
      log("appview: css@app ⇒ 影子根 · " + cssText.length + " 字");
    } catch (e) { /* 没写 css@app 就算了：样式可以写在组件里 */ }
  })();
});
// **属性驱动**（`mdio:app-change`）：外部/ app 自己只写属性就能换 app —— 不必拼 URL、不必知道声明。
//   · `app-name`：覆盖声明里的 `## app - appview:`（setInfo 逼一次重渲，入口缓存按名字失效）；
//   · `app-doc`：由**壳**（DevView）消费 —— 开档是壳的活（它才持有 open()），宿主不插手。
document.addEventListener("mdio:app-change", (ev) => {
  const d = ((ev as CustomEvent).detail || {}) as { name?: string; value?: string | null };
  const v = d.value == null ? "" : String(d.value).trim();
  if (d.name !== "app-name") return;
  const next = v || null;
  if (next === APP_NAME) return;
  APP_NAME = next;
  log("appview: app-name ⇒ " + (next || "(回声明值)"));
  setInfo({ appName: next });   // 逼一次重渲：入口件按 app 名缓存
});

document.addEventListener("mdio:app-unmount", (ev) => {
  const el = ((ev.target as Element | null) || document.getElementById("app")) as (HTMLElement & { shadowRoot?: ShadowRoot | null }) | null;
  if (el) {
    const h = appMirrorHandles.get(el);
    if (h) { try { h.stop(); } catch (e) { /* ignore */ } appMirrorHandles.delete(el); }
    if (el.shadowRoot) unregisterStyleRoot(el.shadowRoot);
  }
  if (isCenterAppview(el)) appRouter.setLayerRoot(null);   // 只有中心格卸载才把层落点回 document.body
});

// **全局污染守卫**（`@esmap/guard` · boot 后取一次快照，之后每 2s 比差集）：谁往 window 上塞了新键就进日志一次。
//   静态判据能守住我们自己的代码；这条守的是**跑起来的第三方**（库/插件/被采纳的 mod）。
setTimeout(() => {
  try {
    globalGuard({ interval: 2000, onViolation: (k: string) => log("[guard] 新增全局：" + k) });
  } catch (e) { /* 守卫失败不影响主流程 */ }
}, 4000);

/**
 * **换树不刷新**：DevView 的 wt 切换改调这里，而不是 `location.href`。
 *
 * 做法 = 启动序列可重入：`setWt(uid)` 换掉 WT/WS 地址 ⇒ `reset()` 清掉 start() 的一次性守卫
 * ⇒ 再 `start()` 重跑两步（`client` 重连该 wt 的 WS + 取文件树；`uiMd` 重读声明并 `ui.load` 换表）
 * ⇒ React 因 boot 状态变化重渲染 ⇒ DevView 拿到新 client/新声明。全程**不导航**（URL 用 replaceState 同步）。
 *
 * 失败语义：新 wt 不可用（未挂载/无声明）时 step 抛错 ⇒ boot 状态 = failed ⇒ 页面显示**同一张错误卡**
 * （有原因、有刷新按钮），而不是白屏或半旧半新。
 */
// ── 每棵树"上次所在页"的记忆（宿主侧 · 跨 wt 状态管理的 URL 层） ──
const WT_MEM_KEY = "mdio_wt_route";
function wtMem(): Record<string, string> {
  try { return JSON.parse(sessionStorage.getItem(WT_MEM_KEY) || "{}") || {}; } catch (e) { return {}; }
}
function setWtMem(uid: string, route: string) {
  try { const m = wtMem(); if (route) m[uid] = route; else delete m[uid]; sessionStorage.setItem(WT_MEM_KEY, JSON.stringify(m)); } catch (e) { /* 无 sessionStorage 忽略 */ }
}
/** 连接层换树（第 5 步 :137-138 拆小）：换 WS + 目标树快照进册子 + 重跑两步。
 *  路由/页记忆是换 wt 的默认附带动作（URL 按该 wt 上次所在页恢复）；store 层指针由 DevStore.switchWT 先改。 */
async function switchTo(uid: string) {
  if (!uid || uid === WT) return true;
  // Explicit boot-start marker: shell keep-alive uses it to start one deadline.
  postShell("booting")
  // 形态与指针解耦：换树是指针动作，不改「开发面 / 产品面」壳状态（曾无条件 APP_DOC=null ⇒ 产品面切树跳回开发面）。
  const faceProduct = APP_DOC != null;
  setWt(uid);
  setWtMem(WT, String(location.hash || ""));     // 离开前记住本树所在页（该树的页只对那棵树有意义）
  const back = wtMem()[uid] || "";                 // 回来时优先回上次那页
  try { history.replaceState(null, "", "/?wt=" + encodeURIComponent(uid) + (faceProduct && APP_DOC ? "&app=" + encodeURIComponent(APP_DOC) : "") + back); } catch (e) { /* 无 history 环境忽略 */ }
  try {
    const c: any = client;
    if (c && typeof c.close === "function") c.close();
    else if (c && typeof c.dispose === "function") c.dispose();
  } catch (e) { /* 关旧连接失败不阻断换树 */ }
  try { disposeWatch(); } catch (e) { /* ignore */ }                    // Step1 · 旧轮 onSnapshot 作废
  try { (ui as any).bumpReloadGen(); } catch (e) { /* ignore */ }        // P5 · 作废在飞的 reload（旧 wt 的晚到投递不再覆盖新树）
  if (!resetBoot()) throw new Error("换树失败：启动序列正在运行中，请稍后重试");
  switchPreserveApp = true;
  try {
    await start();
  } finally {
    switchPreserveApp = false;
  }
  const st: any = getState();
  if (st && st.state === "failed") {
    throw new Error("换树失败：" + String((st.error && st.error.message) || "未知原因"));
  }
  return true;
}

/** 宿主层入口（appRouter / DevView 契约用）：与连接层同体 —— 旧函数体保留便于回退（计划 :138）。 */
async function switchWt(uid: string) {
  return switchTo(uid);
}
// 不挂全局（判据 R6a 禁 `window.__*` 探针）：走**外壳契约**把换树能力当 prop 注入 DevView。


function App() {
  useEffect(() => {
    start().then(
      () => log("boot: ready"),
      (e: any) => log("boot: FAILED " + String((e && e.message) || e).slice(0, 160)),
    );
  }, []);
  return h(BootScreen, null);
}

createRoot(document.getElementById("root") as HTMLElement).render(h(App, null));