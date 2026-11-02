// @ts-check
import { memo, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { observer } from "mobx-react-lite";   // S2：主题效应与 Header 主题包装读壳态店（observable 追踪）
import { log } from "ui_core/logBus";   // 日志数据源在服务层（从 devkit 迁出）；DevView 只写日志
import { entriesFromAst, aboutFromAst, makeRow, withCurrent } from "./devkit/wtList.js";
import PerfBox from "./devkit/PerfBox.jsx";
import ErrorBoundary from "ui_core/ErrorBoundary";
import { createPortal } from "react-dom";   // 只用来把 app 子树投进 appview 元素的挂载点
import { perfMarkPatch, perfMarkRender, perfCountFullFrame, perfCountPatchFrame } from "./devkit/perfBus.js";
import Header from "./chrome/Header.jsx";
import Footer from "./chrome/Footer.jsx";
import Split from "./chrome/Split.jsx";
import "./DevView.css";   // 外壳骨架样式随件走；子组件各自 import 自己的 .css（从聚合的 chrome.css 拆开）

/**
 * DevView —— 开发者视图外壳（旧壳 DemoLayout 的块结构照搬，底层换新架构）：
 *   header（常驻：品牌/wt/当前文档/面板开关/连接徽章/开发者模式切换/主题）
 *   body  = 左槽 app（Monaco）| app（渲染面，**由宿主注入**）| 右槽 app（管线日志）
 *   footer（常驻状态条）
 * 槽位化：左右槽都不是壳内组件，而是 **appview 元素 + 宿主注入的外部 wt app**；
 *   壳只决定挂/卸（Monaco / Log 开关）与显隐（开发者模式 CSS，不卸载）；当前档指针归壳，槽 app 自持自己的档。
 *
 * ── **宿主契约（S2 冻结）** ──────────────────────────────────────────────
 * 外壳不认识 mdio 的任何内部模块：它**不 import demo**、不 import 框架；一切能力由 props 注入。
 *
 * @param {object}   p.client     通道（client）：`exec(cmd, opt)` · `readDoc(wt, path)` · `openDoc` · `closeDoc`
 *                                 · `subscribe(cb)` · 文档与会话全在它后面；外壳只调这些方法。
 * @param {object}   p.ui         声明 store（MST）：`setRoute(path)` + `setDocId(id)`（doc 由派生得出） · `declInfo`；外壳只写不读内部。
 * @param {Array}    p.entries    当前 wt 作用域内的**交付树条目**（文件树数据源，宿主从通道取）。
 * @param {Function} p.App        渲染面组件（宿主注入，通常 = 框架的 DocPane）；收到 `{ ui, exec }`。
 *                                 **外壳不硬编码渲染器** —— 换实现即换这个 prop。
 * @param {string}   p.wt         当前工作树 uid（默认 genesis）。
 * @param {string?}  p.initialDoc 初始文档路径（?route= 深链用）。
 * @param {Function?} p.onSwitchWt 换树（不刷新）：宿主注入则就地换树；缺席时退回整页导航。
 */

// （影子硬边界开关已收进 ui_appview：`#app` 的本体是那个独立 mod 的自定义元素，外壳只认识标签名。）

/** 文档面外框（memo）：props 稳定 ⇒ 不随 DevView 的重渲染而重渲染。 */
const MemoDocApp = memo(function MemoDocApp({ App, ui, exec }) {
  return <App ui={ui} exec={exec} />;
});

/** S2 主题效应：读壳态店 dark（observer 追踪）⇒ body 与 appview 宿主的主题类。无店时退本地值。 */
const ThemeEffect = observer(function ThemeEffect({ store }) {
  const dark = store ? !!store.dark : true;
  useEffect(() => {
    // 显式两态：只移除 dark 会让浅色处于无类可依的状态（实测 A3 判红：背景透明）。
    document.body.classList.toggle("dark", !!dark);
    document.body.classList.toggle("light", !dark);
    // 影子树样式也要按主题切换 —— 把同一主题类也打到 appview 宿主 ⇒ 影子样式表可用 :host(.dark) 命中。
    // 中心 + 左/右槽宿主都要（panel app 的 :host(.dark) 才不会失配）；footer 槽加入。
    const hosts = ["app", "app-left", "app-log", "app-foot"];
    for (const id of hosts) {
      const themeHost = typeof document === "undefined" ? null : document.getElementById(id);
      if (themeHost) { themeHost.classList.toggle("dark", !!dark); themeHost.classList.toggle("light", !dark); }
    }
  }, [dark]);
  return null;
});

/** Header 主题包装：壳态全读 store；rev 由宿主递。 */
const HeaderTheme = observer(function HeaderTheme({ store, rev, conn, bootError, onToggleDev }) {
  return <Header store={store} rev={rev} conn={conn} bootError={bootError} onToggleDev={onToggleDev} />;
});

const DevView = observer(function DevView({ client, entries, ui, App = null, leftApp = null, leftStore = null, logApp = null, logStore = null, footApp = null, footStore = null, wt = "", rootWt = null, appTag = "mdio-appview", initialDoc = null, onSwitchWt = null, onSyncFace = null, appRouter = null, onDocPointer = null, devStore = null, conn = null, bootError = null }) {
  // ── app 那一格：本体是 ui_appview 的自定义元素（独立 mod）。外壳只做两件事：
  //    ① 渲染标签名（prop 传入，默认 mdio-appview）；② 把 app 子树投进它的**挂载点**。
  //    影子根建立 / 样式落点登记 / 层落点注入都在元素与宿主驱动侧（见 entry.client 的 mdio:app-ready 监听）。
  const appHostRef = useRef(null);
  const [appMt, setAppMt] = useState(null);
  const openRef = useRef(null);   // 最新 openByPath（每次渲染刷新；属性驱动要拿当前闭包）
  useEffect(() => {
    const el = appHostRef.current;
    if (!el) return undefined;
    const pick = () => setAppMt(el.mountPoint || el);
    // **属性驱动换篇**：元素上的 `app-doc` 一改就开那一篇（app 或外部只写属性，不必拼 URL/页内段）。
    const onChange = (e) => {
      const d = (e && e.detail) || {};
      if (d && d.name === "app-doc" && d.value && typeof openRef.current === "function") openRef.current(String(d.value));
    };
    pick();
    el.addEventListener("mdio:app-ready", pick);
    el.addEventListener("mdio:app-change", onChange);
    return () => { el.removeEventListener("mdio:app-ready", pick); el.removeEventListener("mdio:app-change", onChange); };
  }, [appTag]);
  // ── 左槽 app（exp_monaco）：同一个 appview 元素契约；外壳只渲染标签名 + 投子树 ──
  const leftHostRef = useRef(null);
  const [leftMt, setLeftMt] = useState(null);
  useEffect(() => {
    const el = leftHostRef.current;
    if (!el) return undefined;
    const pick = () => setLeftMt(el.mountPoint || el);
    pick();
    el.addEventListener("mdio:app-ready", pick);
    return () => el.removeEventListener("mdio:app-ready", pick);
    // mona 关 ⇒ 整块左格 app 卸载；再开 ⇒ 元素是**新的** ⇒ 必须重新取挂载点，否则 portal 还投进旧的 detached 元素
  }, [appTag, devStore && devStore.mona]);
  // ── 右格 log app（槽化 S1）：同一 appview 元素契约；log 关 ⇒ 整块卸载，再开 ⇒ 重新取挂载点 ──
  const logHostRef = useRef(null);
  const [logMt, setLogMt] = useState(null);
  useEffect(() => {
    const el = logHostRef.current;
    if (!el) return undefined;
    const pick = () => setLogMt(el.mountPoint || el);
    pick();
    el.addEventListener("mdio:app-ready", pick);
    return () => el.removeEventListener("mdio:app-ready", pick);
  }, [appTag, devStore && devStore.log]);
  /** 宿主注入的左格件（大写变量才能当 JSX 组件用）。 */
  const LeftAppC = leftApp;
  /** 宿主注入的右格日志件（同上）。 */
  const LogAppC = logApp;
  /** 宿主注入的 footer 件（console app；chrome 只负责挂载点与高度）。 */
  const FootAppC = footApp;
  /** 元素标签名：外壳只认识这个名字，不认识 ui_appview 这个包（契约：能力经 props 注入）。 */
  const AppTag = appTag || "mdio-appview";
  const [rev, setRev] = useState(0);
  // 壳态（dark/devMode/mona/log/wtList/栏宽）全部收进 DevStore；本地 state 与 sessionStorage 一并删
  // 文档面（宿主注入的 App）用 memo 包住：`App`/`ui`/`exec` 都是稳定引用 ⇒ DevView 自己因帧/日志重渲染时，
  // 文档面不再跟着重渲染（业主问「DevView 为何伴随 app 更新」的根因就在这里）。
  // 定义放组件外：放里面每次渲染都会造出新组件类型，memo 直接失效。
  const patchCount = useRef(0);
  const stallRef = useRef(null);   // AST-STALL 计时器（旧壳：写后迟迟无 P-patch 的卡死检测）
  const execSeq = useRef(0);
  const pendingRenderAt = useRef(0);   // PJ 到达时刻（等 DOM 提交后补 render 段；不再用 rAF）
  const openDocIdRef = useRef(null);   // 帧订阅用：开档时同步写，帧到达时比对 docId
  const entryPathRef = useRef("");   // 当前文档路径：popstate 回放幂等比对（HEAD 契约复原）
  // 日志订阅下沉到 LogPanel：DevView 不自己订整份日志 ⇒ **每追加一条日志不会重渲染整个 DevView 树**
  // （含文档面）⇒ 实测每按键 4–6 次整树重渲染。日志面板是唯一消费者，就让它自己订。
  // 栏宽与 footer 高度、wt 清单：已收进 DevStore（devStore.leftW/logW/footH/wtList）
  /**
   * 切树过渡态：业主指出"切 wt 像整页刷新"且**没有过渡**、装载期只有一句裸文本。
   * 现状：`onPickWt` 直接 `location.href=` ⇒ 整页导航（白屏 + SSR 占位"正在装载…"）。
   * 这里先补齐**过渡**：置 switching ⇒ 当前内容 blur + spinner ⇒ 再切。
   * （真正的"不刷新换树"需要把客户端启动改成可重入：`WT` 是模块级常量、boot 步骤一次性、WS 按 wt 连接 ——
   *  过渡本身不依赖那些改造。）
   */
  const [switching, setSwitching] = useState(false);
  useEffect(() => {
    let live = true;
    (async () => {
      if (!client) return;
      // 全局注册表在**根树**上（rootWt 由装配点给）；子条目写在**父树自己的** /.mds/worktrees.md 里
      //   （B10 语义）⇒ 递归读各 WT 自己那份即得树；README 的 `# app` 节给展示名 / tag / desc。
      // 结构一律由引擎解析（`$ <doc> | tojson` 的 AST），这里只遍历；不在 JS 里重做 md 文本解析。
      // 读法必须走 HTTP 的 readDocAst（带 wt_uid、无连接上下文）：
      //   子 wt 里用 WS exec 读 `$ /.mds/worktrees.md` 会落到该 wt 自己那份 ⇒ 清单塌成一条。
      const rootUid = rootWt || wt;
      const rows = [];
      const seen = new Set();
      const readAst = async (uid, path) => {
        try { return await client.readDocAst(uid, path); } catch (e) { return null; }
      };
      const walk = async (uid, title, parent, depth) => {
        if (!uid || seen.has(uid) || depth > 8) return;
        seen.add(uid);
        const meta = aboutFromAst(await readAst(uid, "/README.md"));
        rows.push(makeRow({ title, uid, parent, depth, meta }));
        const children = entriesFromAst(await readAst(uid, "/.mds/worktrees.md"));
        for (const c of children) await walk(c.uid, c.title, uid, depth + 1);
      };
      try { await walk(rootUid, rootUid, "", 0); }
      catch (e) { log("wt 清单读取失败：" + String((e && e.message) || e).slice(0, 80)); }
      if (!live) return;
      if (devStore && typeof devStore.setList === "function") devStore.setList(withCurrent(rows, wt, rootUid));
      log("wt 清单: " + rows.length + " 棵（tag app " + rows.filter((r) => r.tagApp).length + " 棵）");
    })();
    return () => { live = false; };
  }, [client, wt, rootWt]);

  /**
   * 开发者模式 = **就地开关**（纯 state，不写 URL）：关掉只隐藏 workspace 三栏 + footer，
   * 组件全部**保持挂载**（Monaco 会话、文档 session、日志流都不断）⇒ 切回来是瞬时的，页面不重载。
   */
  const devMode = devStore ? !!devStore.devMode : true;   // 开发者模式 = store 持久（跨 wt 切换不丢）
  const toggleDevMode = () => {
    const next = !(devStore ? !!devStore.devMode : true);
    if (devStore && typeof devStore.toggleDev === "function") devStore.toggleDev();
    const doc = (entry && entry.path) || initialDoc || "/README.md";
    // 形态与指针解耦：能走宿主就交给宿主一处组合 URL（它才知道 wt）。
    if (typeof onSyncFace === "function") { try { onSyncFace(!next, doc); return; } catch (e) { /* 退回本地写法 */ } }
    try {
      const url = "/?wt=" + encodeURIComponent(wt) + (next ? "" : "&app=" + encodeURIComponent(doc));
      history.replaceState(null, "", url);
    } catch (e) { /* 无 history 环境忽略 */ }
  };


  // S2：主题类落点由 <ThemeEffect> 统一写（店驱动 + 本地回退），此处不再双写。

  /**
   * 写后无补丁的卡死检测（旧壳 AST-STALL）：**只对写命令**武装（`/>` 写入、`+>` 追加）——
   * 读命令（`$ addr`）本来就不产生 P 帧，之前无差别武装导致读一次就误报一次。
   */
  const armStall = (code) => {
    if (!/\/>|\+>/.test(String(code || ""))) return;
    const seq = (execSeq.current += 1);
    if (stallRef.current) clearTimeout(stallRef.current);
    stallRef.current = setTimeout(() => {
      stallRef.current = null;
      log("AST-STALL: exec#" + seq + " 后 3.0s 无 P-patch");
    }, 3000);
  };

  // 业务面：exec = 与框架/console 同一条 mdio exec 通路，绑定当前 wt（旧壳 MDIO_EXEC/MDIO_RESULT 词汇）
  const exec = useMemo(
    () => (client ? async (code) => {
      log("MDIO_EXEC: " + String(code).slice(0, 160));
      try {
        // active_doc 必须带：md 的**节点寻址**（如 "5" | /> 站点统计.面板._b:记录）要靠它定位当前文档，
        // 否则引擎报 [V003] 缺省寻址 —— 不接这条通路，组件只能退化成"读整篇 → 正则替换 → 写整篇"。
        const raw = await client.exec(code, { wt_uid: wt, active_doc: entryPath || "" });
        log("MDIO_RESULT: " + String(raw == null ? "" : raw).slice(0, 200));
        armStall(code);
        return raw;
      } catch (e) {
        log("COMP✗: exec FAILED: " + String((e && e.message) || e).slice(0, 120));
        throw e;
      }
    } : null),
    [client, wt],
  );
  // 有挂起的 PJ 才结算 ⇒ 量到的是「PJ 到达 → DOM 已更新」，而不是「等到下一帧」。
  useEffect(() => {
    const t0 = pendingRenderAt.current;
    if (!t0) return;
    pendingRenderAt.current = 0;
    const t1 = (typeof performance !== "undefined" && performance.now) ? performance.now() : Date.now();
    perfMarkRender();
    log("recv_to_render: " + Math.round(t1 - t0) + "ms");
  });

  // 帧 bus：订阅不随 openDocId 重建；open 先置 ref 再发 UP_OPEN
  // 日志口径照旧壳：P 帧带**内容摘要**（op + path）、并记**帧间隔 send_gap** 与 **recv→render 耗时**。
  useEffect(() => {
    if (!client) return undefined;
    let lastAt = 0;
    return client.subscribe((ev) => {
      if (ev.docId !== openDocIdRef.current) return;
      const now = (typeof performance !== "undefined" && performance.now) ? performance.now() : Date.now();
      const gap = lastAt ? Math.round(now - lastAt) : 0;
      lastAt = now;
      setRev(ev.rev);   // 帧计数只喂视图显示（Header/Footer）；声明 store 不再写一个没人读的 rev
      if (ev.type === "snapshot") {
        // doc 已由 docId 派生（帧落在主树 wts.wt.by ⇒ computed 自动更新）⇒ 不再手写 setDoc
        patchCount.current = 0;
        perfCountFullFrame();
        log("AST F-snapshot: " + (ev.bytes || 0) + "b（rev=" + ev.rev + " doc=" + ev.docId + "）");
        if (gap && gap < 1000) log("send_gap: " + gap + "ms");   // 突发内的帧间隔才有意义（旧 telemetry：帧发送间隔）
      } else {
        patchCount.current += 1;
        const ops = Array.isArray(ev.ops) ? ev.ops : [];
        perfCountPatchFrame();
        perfMarkPatch(ops.length, ev.bytes || 0, ev.wasmMs);
        // WASM 分层遥测：每个补丁取一次（请求是 postMessage，回来走 client 的 telemetry_data 分支）
        try { client.requestTelemetry(); } catch (e) { /* ignore */ }
        if (typeof ev.wasmMs === "number") {
          log("incr: " + Math.round(ev.wasmMs) + "ms wall=" + (ev.wasmWallIn || "?") + "→" + (ev.wasmWallOut || "?") + "（进wasm→PJ出wasm）");
          // SOP K4 判据要的行（口径已与 incr 同源：都是「信息进 wasm → PJ 出 wasm」；保留旧行名不改判据）
          log("patch_latency: " + Math.round(ev.wasmMs) + "ms（写→补丁 = 进wasm→PJ出wasm）");
        }
        // 摘要化：紧凑序列化（不再两空格缩进——缩进只是空白，却让每补丁的字符串化开销明显变大）；
        // 面板对长行本来就是"默认折叠、点击展开"，信息一条不少。
        let pretty = "";
        try { pretty = JSON.stringify(ops); } catch (e) { /* ignore */ }
        if (gap && gap < 1000) log("send_gap: " + gap + "ms");
        log("P_JSON: " + (ev.bytes || 0) + "b\n" + pretty);
        // render 段的终点改到**DOM 提交之后**（见下方 effect）：rAF 会把「等下一帧」算进来
        pendingRenderAt.current = now;
        if (stallRef.current) { clearTimeout(stallRef.current); stallRef.current = null; }   // 补丁回来了 ⇒ 解除停滞告警
      }
    });
  }, [client]);

  const open = (e) => {
    if (!client || !e || e.id == null) return;
    openDocIdRef.current = e.id;   // 帧过滤：开档时同步写（切档不退订，身份 = 引擎实例 × 文档）
    const p = e.path || "";
    entryPathRef.current = p;
    if (ui) { ui.setRoute(p); ui.setDocId(e.id); }   // route 单源 + 档身份进店 ⇒ entry/openDocId/model 由派生得出
    const md0 = client.md ? client.md(e.id) : null;
    if (md0) md0.setPath(p);        // 文档店 path 字段（波 4）
    client.ensure(e.id);            // 幂等「我要这篇」
    log("openFile: " + e.path + " docId=" + e.id);
    try {
      const di = ui && ui.declInfo;
      if (di && di.vis > 0) log("vis: 本档生效 " + di.visOn + "/" + di.vis + " 条（doc=" + di.route + "）");
    } catch (err) { /* 观测失败不影响开档 */ }
  };

  const list = Array.isArray(entries) ? entries : [];

  // **当前档身份 = 壳 DevStore 的统一读接口**（包 wt 文件表 + ui 声明店，store 内 computed 派生；DevView 不再 React 双份）
  const entry = devStore ? devStore.entry : null;
  const openDocId = devStore ? devStore.openDocId : null;
  const model = devStore ? devStore.model : null;
  const entryPath = entry ? entry.path : "";

  /**
   * **页内段 → 开档**：app 只在页内段表达「要哪一篇」（本 app 的词汇 = `#/doc/<urlencoded 全路径>`），
   * 壳负责两件事：① 用**既有的 open()** 开那篇（不碰形态、不重载、不动 `?wt`）② 回调宿主把篇名**归一进 `?app=`**。
   * 幂等要求：`appRouter.subscribe` 同时挂 hashchange 与 popstate（note §2⑨）⇒ 同一次导航可能回调两次，开同一篇是幂等的。
   */
  const openByPath = (path, opts) => {
    const want = String(path || "");
    const found = (Array.isArray(entries) ? entries : []).find((x) => String(x && x.path) === want);
    if (found) {
      open(found);
      // `syncUrl === false`：这次开档的指针来源就是 URL（popstate）⇒ 不再回写 URL，避免自己打自己。
      if (!opts || opts.syncUrl !== false) {
        if (typeof onDocPointer === "function") { try { onDocPointer(want); } catch (err) { /* 归一失败不影响开档 */ } }
      }
      return true;
    }
    // app 刚写出的文档（新建的 chat 会话等）可能还没进 boot 时的 entries：按需取一次树、
    // 顺带刷新工作树文件面（Monaco 的 files 也读它），再开这一篇。
    if (client && typeof client.fullTree === "function" && wt) {
      void (async () => {
        try {
          const ft = await client.fullTree(wt);
          const list = (Array.isArray(ft) ? ft : []).filter((x) => String(x && x.path || "").indexOf("/.mds/mounts/") !== 0);
          const hit = list.find((x) => String(x && x.path) === want);
          if (!hit) return;
          try {
            const cur = client.wt && client.wt();
            if (cur && typeof cur.putEntries === "function") {
              cur.putEntries(list.map((x) => ({ path: String(x && x.path || ""), docId: Number(x && x.id) })));
            }
          } catch (err) { /* 文件面刷新失败不挡开档 */ }
          open(hit);
          if (!opts || opts.syncUrl !== false) {
            if (typeof onDocPointer === "function") { try { onDocPointer(want); } catch (err) { /* 归一失败不影响开档 */ } }
          }
        } catch (err) { /* 取树失败：保持旧档 */ }
      })();
      return true;
    }
    return false;
  };
  // 属性驱动（`mdio:app-change` 的 `app-doc`）要拿**最新闭包**的 openByPath：无 deps ⇒ 每次提交后刷新一次。
  useEffect(() => { openRef.current = openByPath; });
  /**
   * **「指针永远在 URL」的另一半**（back/forward 必须让文档跟着 URL 走）：
   *   归一之后篇名住在 `?app=`，浏览器后退/前进改的是 `?app=` —— 没有任何回调会开档，
   *   URL 与正在渲染的文档就会脱钩。这里按 URL 说的开档；文档没变就什么都不做（幂等）。
   */
  useEffect(() => {
    if (typeof window === "undefined") return undefined;
    const onPop = () => {
      let want = "";
      try { want = new URLSearchParams(location.search).get("app") || ""; } catch (e) { want = ""; }
      if (!want) return;
      if (want === String(entryPathRef.current || "")) return;   // 已经是这篇 ⇒ 不动
      openByPath(want, { syncUrl: false });
    };
    window.addEventListener("popstate", onPop);
    return () => { window.removeEventListener("popstate", onPop); };
  }, [entries, onDocPointer]);
  // **onPage 订阅不存在**：壳不解读 app 的词表（/doc/）——
  //   hash = app 段自治空间；「开档归一 ?route=」与「回非文档页清指针」都由 app 经 appRouter.setRoute 显式请求。

  // 旧壳行为：boot 即开档（DemoLayout boot 里 openFile(档路径)）——否则 mona/app 两栏空着
  const autoOpened = useRef(false);
  useEffect(() => {
    if (autoOpened.current || openDocIdRef.current != null || list.length === 0) return;
    // 旧壳约定：README.md 是约定文档；优先根层 README，其次最浅路径的 README，最后根层任意 .md
    const md = list.filter((e) => String(e.path).endsWith(".md"));
    const depth = (p) => String(p).split("/").filter(Boolean).length;
    const readmes = md.filter((e) => /README\.md$/i.test(String(e.path))).sort((a, b) => depth(a.path) - depth(b.path));
    // ?app=/x.md 时优先开它（产品面直链）；否则按旧壳约定挑 README
    const want = String(initialDoc || "").replace(/^\/+/, "");
    const pick = (want ? md.find((e) => String(e.path).replace(/^\/+/, "") === want) : null)
      || md.find((e) => String(e.path) === "/README.md") || readmes[0] || md.filter((e) => depth(e.path) === 1)[0];
    if (pick) { autoOpened.current = true; open(pick); }
  }, [list]);
  // 宿主契约要求必传壳态店；缺失时给空壳（所有 hook 已跑完，早退合法）
  if (!devStore) return <div className="dv-root" />;
  return (
    <div className={"dv-root" + (devMode ? "" : " is-product")}>
      <ThemeEffect store={devStore} />
      {/* 第 1 行 header：横贯整页宽（wt 切换在最左） */}
      <HeaderTheme store={devStore} rev={rev} conn={conn} bootError={bootError} onToggleDev={toggleDevMode} />
      <div className="dv-body">
        {/* 第 2 行 workspace：左槽 app | app（中心）| 右槽 log app */}
        {/* Monaco 开关 = **整个左格 app** 的显隐（旧版 Monaco 面板开关语义；关=从布局摘掉，中心 app 占满） */}
        {devStore.mona && (
          <>
            <div className="dv-pane dv-left" style={{ width: devStore.leftW, flex: "0 0 auto" }}>
              <AppTag className="dv-left-body" id="app-left" ref={leftHostRef} />
              {leftMt && LeftAppC ? createPortal(
                <ErrorBoundary name="左格 app"><LeftAppC app={leftStore} /></ErrorBoundary>, leftMt) : null}
              {!LeftAppC ? <div className="dv-pane-empty">左槽 app 未装载（期望代码宿主 wt 的 ui.md appview = MONACO_APP；可用 &amp;leftwt=&lt;wt&gt; 覆盖）</div> : null}
            </div>
            <Split width={devStore.leftW} min={320} max={1200} onDelta={(w) => devStore.setLeftW(w)} />
          </>
        )}
        <div className="dv-pane dv-app">
          {/* `#app` = appview 那一格（被 devview 包围、不属于 devview）；见 DevView.css 的 contain/container 两条。 */}
          {/* `data-esmap-scope` = @esmap/guard 的属性前缀法作用域名（宿主用同一个字面量采纳 app 的 CSS）。 */}
          <AppTag className="dv-app-body" id="app" ref={appHostRef} />
            {appMt ? createPortal(<ErrorBoundary name="文档面">
            {openDocId == null ? (
              <div className="dv-pane-empty" />
            ) : (
              //  文档切换时**不许把渲染面整块卸载**：切到还没落店的文档会有一瞬 `model == null`，
              //    旧写法据此换成「等待首帧…」占位 ⇒ 入口件连同它的状态（左栏清单）被卸载重挂（审计实测：
              //    `[data-app=scripts]` removed 1 / added 1，左栏瞬时只剩当前篇）。现在占位与渲染面**并存**。
              <div className={"dv-doc-inner" + (App ? " is-app" : "")}>
                {App ? <MemoDocApp App={App} ui={ui} exec={exec} /> : <div className="dv-pane-empty">未提供渲染面（宿主需传 App prop）</div>}
                {!model ? <div className="dv-pane-empty">等待首帧…（docId={openDocId}）</div> : null}
              </div>
            )}
            </ErrorBoundary>, appMt) : null}
          {switching && (
            <div className="dv-switching" role="status" aria-live="polite">
              <div className="dv-spin" />
              <div className="dv-switching-t">正在切换到该 wt…</div>
            </div>
          )}
          {/* exec 的输入与结果都在 footer（pseudo console 本体），app 栏不放输出 */}
        </div>
        {devStore.log && (
          <>
            {/* 右栏（log）分隔条：拖动向右 ⇒ log 变窄（宽度取负号交给 Split 做方向反转） */}
            <Split width={-devStore.logW} min={-800} max={-240} onDelta={(w) => devStore.setLogW(-w)} />
            <div className="dv-pane dv-log-col" style={{ width: devStore.logW, flex: "0 0 auto" }}>
              {/* 右列自上而下：[Perf] 卡片 → [log] 面板 */}
              <ErrorBoundary name="日志">
                {/* 开发者模式关闭 ⇒ PerfBox 不渲染（此前只是隐藏却照旧每帧重渲染）；
                    日志 app 槽**保持挂载**（与 Monaco 同：切回瞬时，不重载）。 */}
                <PerfBox visible={devMode} />
                <div className="dv-log-body">
                  <AppTag className="dv-log-app" id="app-log" ref={logHostRef} />
                  {logMt && LogAppC ? createPortal(
                    <ErrorBoundary name="日志 app"><LogAppC app={logStore} devMode={devMode} /></ErrorBoundary>, logMt) : null}
                  {!LogAppC ? <div className="dv-pane-empty">日志 app 未装载（期望代码宿主 wt 的 ui.md appview = LOG_APP；可用 &amp;logwt=&lt;wt&gt; 覆盖）</div> : null}
                </div>
              </ErrorBoundary>
            </div>
          </>
        )}
      </div>
      <ErrorBoundary name="footer app">
        <Footer
          height={devStore.footH}
          onHeight={(h) => devStore.setFootH(h)}
          appTag={AppTag}
          app={FootAppC}
          store={footStore}
          placeholder="footer app 未装载（期望 exp_console 的 ui.md appview = CONSOLE_APP；可用 &amp;footwt=&lt;wt&gt; 覆盖）"
        />
      </ErrorBoundary>
    </div>
  );
});

export default DevView;