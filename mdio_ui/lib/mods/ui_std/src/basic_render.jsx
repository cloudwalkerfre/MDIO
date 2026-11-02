// @ts-check
/**
 * std/basic_render —— BASIC_RENDER（系统默认 app，正经 app 化）
 *
 * 两个角色：
 *   · 正经 app：## mods 登记 + ## app - appview: BASIC_RENDER 指名装载（genesis 正是这么用）；
 *   · 系统 fallback app：dev 面无 av 时由宿主直接挂它（entry.client 的 MDBody）。
 *
 * 组成（三件套）：
 *   · store：MST 模具（type 工厂 + create(snapshot, env)）；files = 文件树 computed —— **数据即路由表**；
 *   · 导航：左栏嵌套文件树点选 = appRouter.setRoute（当前 route）+ push hash（app 内部页）；
 *   · 状态：切档 pending ⇒ 模糊 + 转圈（帧落地即清）；兜底：renderNode() 渲染当前文档。
 * 已知边界（如实）：route 单源在 ui；左栏点击经壳 setRoute 开档 + 投影 URL只记 hash、
 *   文档体仍跟壳的工作区；产品面完整生效（点选 = 切指针 + 页内段）。
 * 样式：暖羊皮纸编辑台（亮/暗双主题 tokens 在 css 的 :host 与 :host(.dark)）。
 *   正文排版（字号/间距/配色/shiki）：`themes/parchment.css` —— **本件静态 import 自带**
 *   （构建期 chunk CSS → manifest → 宿主 <link> → 影子根镜像，零声明；见 entry.node.ts:108-138）。
 *   规则以祖先属性 `[data-md-theme="parchment"]` 门控，属性由本件写在 `.br-shell` 上：
 *   这样将来多套皮肤只需都 import、切换只改属性，而挂载本 app 的树**不必**在 `## mods` 里声明任何东西。
 *   （修正：用 `- THEME:` 声明级 sheet 承载皮肤是错通道 —— 那会让每棵装 app 的树重复声明。）
 * 状态：当前皮肤是 **app 店** 的字段（`Store.theme`，切换走 `setTheme`），不写 localStorage/hash ——
 *   boot 重建店回默认、声明热重载不重建店（选择保留），这两条生命周期正是把状态放店里的理由。
 */
import { createElement as h, useEffect, useMemo, useState } from "react";
import { observer } from "mobx-react-lite";
import { types, getEnv } from "mobx-state-tree";
import { MDRoot } from "./comps.js";
import "./basic_render.css";
import "./themes/fonts.css";      // 自托管字体（JBMono，base64 内联；由 scripts/gen-theme-fonts.mjs 生成）
import "./themes/parchment.css";
import "./themes/github.css";     // 由 scripts/port-theme-github.mjs 从 vendored 上游生成
import "./themes/mdmdt.css";      // 由 scripts/port-theme-mdmdt.mjs 从 vendored 上游生成
import "./themes/macos27.css";    // 由 scripts/port-theme-hostcss.mjs 从 vendored 上游生成（macOS 27 Themes）
import "./themes/bloom.css";      // 由 scripts/port-theme-hostcss.mjs 从 vendored 上游生成（Bloom · mist）
import "./themes/atelier.css";    // 自有原创皮肤「手作纸」（非移植）

export { createClaimEval, createChainWalk } from "./claimEval.js";

/** **皮肤登记表**：id 必须与 CSS 里的 `[data-md-theme="<id>"]` 门控一致。
 *  `plain` 故意不 import 任何皮肤 CSS —— 它表示「跟随 std 的 appearance floor」，
 *  既是切换的对照组，也是将来某套皮肤出问题时的兜底项。
 *  加一套皮肤 = ①写 `themes/<id>.css`（规则前缀 `[data-md-theme="<id>"]`）②本文件加一行静态 import
 *  ③本表加一条。**不要**把它写成 `## mods` 里的 sheet mod —— 那是树级覆盖通道，会让每棵树重复声明。
 *  现有皮肤：parchment（自有）· github（MIT）· mdmdt（Apache-2.0）· macos27（MIT）· bloom（MIT）
 *  · atelier（自有原创；取法自近三年三类高质量 md 样式，见 themes/NOTICE.txt）。
 *  **皮肤是整壳的**：除正文（`.mdio-doc`）外，皮肤还可以接管外壳视觉 —— `.br-side`（左栏文档树）、
 *  `.br-item/.br-folder`（树行）、`.br-crumb`（面包屑）、`.br-theme`（切换器）与 `.br-shell`（整壳底色）。
 *  契约：皮肤只改**视觉**（颜色/底色/边框/字体/圆角），**结构尺寸与交互仍归 app**（侧栏宽度、树缩进
 *  paddingLeft、折叠、点击、拖拽列宽、加载态不动）；外壳规则同样以 `[data-md-theme]` 门控，且要保证
 *  明暗两态下都从本皮肤的 token 取色（用 `var(--<theme>-*)`，别写死颜色）。 */
const THEMES = [
  { id: "parchment", label: "羊皮纸" },
  { id: "github", label: "GitHub" },
  { id: "mdmdt", label: "Mdmdt" },
  { id: "macos27", label: "macOS 27" },
  { id: "bloom", label: "Bloom" },
  { id: "atelier", label: "手作纸" },
  { id: "plain", label: "库默认" },
];
const DEFAULT_THEME = "parchment";

export const Store = types
  .model("BasicRenderStore", {
    /** 当前皮肤（纯 UI 状态）：boot 重建店时回默认；声明热重载只跑 ui.reload、不重建店 ⇒ 选择保留。 */
    theme: types.optional(types.string, ""),
  })
  .views((self) => ({
    /** 生效皮肤 = 显式选择 || 默认。将来若要「每个 wt 的默认皮肤」，在这一层插
     *  `getMainRoot().decl.app.theme`（与下面 files 同一条 env 桥）——那需要先给 `## app` 扩键集。 */
    get activeTheme() { return self.theme || DEFAULT_THEME; },
    /** 文件树（数据即路由表）：app 店独立成树（volatile box 挂载 ⇒ getRoot 到不了主树），经 env 的 getMainRoot 桥读主树数据（修订）。 */
    get files() {
      const env = getEnv(self) || {};
      const r = typeof env.getMainRoot === "function" ? env.getMainRoot() : null;
      const w = (r && r.wts && r.wts.wt) || null;
      const raw = (w && w.files) || [];
      return raw
        .filter((f) => { const p = String(f && f.path); return p.indexOf("/.mdio/") !== 0 && p.indexOf("/.mds/") !== 0; })
        .slice()
        .sort((a, b) => String(a && a.path).localeCompare(String(b && b.path)));
    },
  }))
  .actions((self) => ({
    /** 换皮肤：只认登记表里的 id（非法值清回默认，声明面永远不出现脏值）。 */
    setTheme(v) {
      const s = String(v == null ? "" : v);
      self.theme = THEMES.some((t) => t.id === s) ? s : "";
    },
  }));


const norm = (p) => String(p == null ? "" : p).replace(/^\/+/, "");

/** 左栏树的排序口径：**目录先于文件**；同级按名称自然排序（数字按数值、中文按本地化、忽略大小写）。
 *  为什么需要：扫描返回的 files 顺序是文件系统顺序，不是字典序；不排序时左栏看起来是「乱的」。
 *  EDIT_APP 的 buildTree 用同一口径（basic_edit.jsx），改这里时那边同步。 */
const TREE_COLLATOR = new Intl.Collator("zh-Hans-CN", { numeric: true, sensitivity: "base" });
function cmpTreeNode(a, b) {
  if (a.dir !== b.dir) return a.dir ? -1 : 1;
  const byName = TREE_COLLATOR.compare(a.name, b.name);
  if (byName !== 0) return byName;
  return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;   // 同名再按码位，保证稳定
}

/** 扁平路径 → 嵌套树（目录聚组、文件名叶子；建完统一排序，见 cmpTreeNode）。 */
function buildTree(files) {
  const root = { name: "/", dir: true, path: "/", kids: [], map: new Map() };
  for (const f of files) {
    const p = String((f && f.path) || "");
    const parts = p.split("/").filter((s) => s !== "");
    let node = root;
    let acc = "";
    for (let i = 0; i < parts.length; i++) {
      const seg = parts[i];
      acc = acc + "/" + seg;
      let child = node.map.get(seg);
      if (!child) {
        child = { name: seg, dir: i < parts.length - 1, path: acc, kids: [], map: new Map() };
        node.map.set(seg, child);
        node.kids.push(child);
      }
      node = child;
    }
  }
  const sortKids = (node) => {
    if (!node.dir) return;
    node.kids.sort(cmpTreeNode);
    for (const k of node.kids) sortKids(k);
  };
  sortKids(root);
  return root;
}
function countFiles(node) {
  if (!node.dir) return 1;
  let n = 0;
  for (const k of node.kids) n += countFiles(k);
  return n;
}

const BasicRenderApp = observer(function BasicRenderApp({ app }) {
  // 契约 { app }：app = 总店 app 槽挂的 app 店；工具（renderNode/read/exec/appRouter）一律 getEnv(app) 取（env 范式）
  const env = app ? (getEnv(app) || {}) : {};
  const mainRoot = typeof env.getMainRoot === "function" ? env.getMainRoot() : null;
  const ui = mainRoot && mainRoot.decl;
  const appRouter = env.appRouter;
  const files = (app && app.files) || [];
  const tree = useMemo(() => buildTree(files).kids, [files]);
  // 目录默认收起（业主定）：openDirs 记的是「已展开的目录」，初始空集 ⇒ 全部收起；
  // 用户点开/收起只改这个集合，排序、折叠互不影响。
  const [openDirs, setOpenDirs] = useState(() => new Set());
  const [pending, setPending] = useState(null);   // 目标档（判定「到位没」）
  const [spin, setSpin] = useState(false);        // 是否显示模糊+转圈（延迟出现，快切不闪）
  const curPath = norm(ui && typeof ui.route === "string" ? ui.route : "");
  const theme = app ? app.activeTheme : DEFAULT_THEME;   // 皮肤：店值（boot 回默认；声明热重载不重建店 ⇒ 选择保留）

  // 切档 pending：目标档就位（curPath 跟上）即清；dev 面指针被忽略 ⇒ 兜底超时清除，避免长转。
  // 加载态策略（调整）：**延迟 150ms 才显示**，而不是给最短显示时长。
  //   原先的最短显示 260ms 会让「内容已切换」之后还多糊 200ms（实测 crumb 852ms 就换了、loading 到 1059ms 才摘）
  //   ⇒ 点击观感变慢。现在快切（本地帧往返仅几 ms）完全不闪，真慢才出转圈。
  useEffect(() => {
    if (!pending) { setSpin(false); return undefined; }
    const show = setTimeout(() => setSpin(true), 150);
    const giveUp = setTimeout(() => { setPending(null); setSpin(false); }, 1200);   // 通道缺席/未生效时收手
    return () => { clearTimeout(show); clearTimeout(giveUp); };
  }, [pending]);
  useEffect(() => {
    if (!pending) return;
    if (norm(curPath) === norm(pending)) { setPending(null); setSpin(false); }
  }, [curPath, pending]);

  const open = (path) => {
    const p = String(path || "");
    if (norm(p) === norm(curPath)) return;
    setPending(p);
    try {
      // 文档即 route：经基线「指针归一入口」向壳显式请求（appRouter.setDocPointer —— 壳注入的 handler 负责开档），不 push hash。
      if (appRouter && typeof appRouter.setDocPointer === "function") appRouter.setDocPointer(p);
    } catch (e) { /* 导航失败不影响渲染 */ }
  };
  const toggleDir = (path) => {
    setOpenDirs((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path); else next.add(path);
      return next;
    });
  };

  const rows = [];
  const walk = (nodes, depth) => {
    for (const n of nodes) {
      const pad = 10 + depth * 14;
      if (n.dir) {
        const isOpen = openDirs.has(n.path);
        rows.push(h("li", {
          key: "d:" + n.path, role: "button", tabIndex: 0, className: "br-folder" + (isOpen ? "" : " is-closed"),
          style: { paddingLeft: pad + "px" },
          onClick: () => toggleDir(n.path),
          onKeyDown: (ev) => { if (ev && (ev.key === "Enter" || ev.key === " ")) toggleDir(n.path); },
        },
          h("span", { className: "br-caret" + (isOpen ? " is-open" : ""), "aria-hidden": "true" }),
          h("span", { className: "br-folder-name" }, n.name),
          h("span", { className: "br-folder-count" }, String(countFiles(n)))));
        if (isOpen) walk(n.kids, depth + 1);
      } else {
        const active = norm(n.path) === norm(curPath);
        rows.push(h("li", {
          key: "f:" + n.path, role: "button", tabIndex: 0, className: "br-item" + (active ? " is-active" : ""),
          style: { paddingLeft: pad + "px" }, title: n.path,
          onClick: () => open(n.path),
          onKeyDown: (ev) => { if (ev && (ev.key === "Enter" || ev.key === " ")) open(n.path); },
        },
          h("span", { className: "br-file-glyph", "aria-hidden": "true" }),
          h("span", { className: "br-item-path" }, n.name)));
      }
    }
  };
  walk(tree, 0);

  return h("div", { className: "br-shell", "data-md-theme": theme },
    h("aside", { className: "br-side" },
      h("div", { className: "br-side-head" },
        h("span", { className: "br-eyebrow" }, "文档"),
        h("span", { className: "br-count" }, String(files.length))),
      files.length
        ? h("ul", { className: "br-tree" }, rows)
        : h("div", { className: "br-side-empty" }, "没有文档")),
    h("main", { className: "br-main" + (spin ? " is-loading" : "") },
      h("header", { className: "br-crumb" },
        h("div", { className: "br-col" },
          h("span", { className: "br-crumb-label" }, "当前位置"),
          h("span", { className: "br-crumb-sep" }, "/"),
          h("span", { className: "br-crumb-path" }, norm(curPath) || "—"),
          THEMES.length > 1 && app
            ? h("label", { className: "br-theme" },
              h("span", null, "皮肤"),
              h("select", {
                value: theme,
                onChange: (e) => app.setTheme(e && e.target ? e.target.value : ""),
              }, THEMES.map((t) => h("option", { key: t.id, value: t.id }, t.label))))
            : null)),
      h("div", { className: "br-doc" },
        h("div", { className: "br-col" }, h(MDRoot, { renderNode: env.renderNode }))),
      spin && pending ? h("div", { className: "br-spin", "data-spin": norm(pending) },
        h("span", { className: "br-spin-ring" }),
        h("span", { className: "br-spin-label" }, norm(pending))) : null));
});

export default BasicRenderApp;
