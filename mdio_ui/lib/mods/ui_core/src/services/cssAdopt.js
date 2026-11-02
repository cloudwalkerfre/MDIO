// @ts-check
/**
 * cssAdopt —— **CSS 作为 mod** 的采纳/回收（框架面）。从 `demo/src/entry.client.ts` 搬来（S4）。
 *
 * mod 导出 `sheet`（浏览器原生 `CSSStyleSheet`）⇒ 装载时 adopt。
 * 为什么不用 <link>：`rc.css()` 只给静态入口图的 chunk 出 link，**运行时装载的 mod 拿不到**；
 * 为什么不用自注入 <style>：写进 head 后**不可移除、无作用域**。adopt 的 sheet 可卸载、可按子树作用域。
 * 形态约束（实测）：平台原生的 **CSS module script**（import attributes 形如 `with { type: "css" }`，目标为 .css）
 * 被 Vite 8/Rolldown 拒绝（`[MISSING_EXPORT]`），所以 CSS mod 是 **JS 模块 + `?inline` 文本 + `new CSSStyleSheet()`**。
 */
/**
 * 日志口：**由装配点注入**（`setCssAdoptLog`）。
 * 为什么不直接 import logBus：`services/*.js` 之间的 import 被判据 R1 禁止（只放行供应商包）——
 * 这条纪律的意图是"services 是叶子"，所以框架件不抓装配点的总线，改由装配点把口子递进来。
 * @type {(text: string) => void}
 */
import { scopeCssText, namespaceCssKeyframes } from "@esmap/guard";

let log = () => {};
/*** @param {(text: string) => void} fn */
export function setCssAdoptLog(fn) { log = typeof fn === "function" ? fn : () => {}; }

/** 声明驱动的那张表（`pruneCssMods` 按声明名剪枝）。 @type {Map<string, CSSStyleSheet>} */
const adoptedSheets = new Map();
/** **惰性采纳**（§13.7 第 3 项）：由 mod 在**用到时**才带来的样式（如 KaTeX 的 369KB 文本），
 *  不进"声明驱动"的那张表 ⇒ `pruneCssMods` 按声明名剪枝时**不会误摘**它。 @type {Map<string, CSSStyleSheet>} */
const lazySheets = new Map();

/** **额外样式落点**（B 路线）：app 内容住进影子树后，框架已采纳的 sheet 也必须在那边生效。
 *  登记一个根时把**当前全部** sheet 同步过去；此后每次采纳/摘除都镜像到所有已登记根。
 *  只镜像「落在 document 上的」那些（`target` 明确指向某根的 sheet 不参与，避免二次搬运）。
 *  @type {Set<Document | ShadowRoot>} */
const mirrorRoots = new Set();
function mirrorPut(root, sheet) {
  try {
    if (!root || !("adoptedStyleSheets" in root) || !sheet) return;
    if (root.adoptedStyleSheets.indexOf(sheet) >= 0) return;
    root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheet];
  } catch (e) { /* 跨 document 的表会抛 NotAllowedError ⇒ 跳过这一根 */ }
}
function mirrorDrop(sheet) {
  for (const root of mirrorRoots) {
    try { if ("adoptedStyleSheets" in root) root.adoptedStyleSheets = root.adoptedStyleSheets.filter((s) => s !== sheet); } catch (e) { /* ignore */ }
  }
}
function mirrorAll(sheet) { for (const root of mirrorRoots) mirrorPut(root, sheet); }

/** 登记一个「额外落点」（影子根）：把当前所有已采纳 sheet 同步进去；返回同步条数。
 *  @param {Document | ShadowRoot} root @returns {number} */
export function registerStyleRoot(root) {
  if (!root || !("adoptedStyleSheets" in root)) return 0;
  mirrorRoots.add(root);
  let n = 0;
  for (const s of document.adoptedStyleSheets) { mirrorPut(root, s); n += 1; }
  for (const s of adoptedSheets.values()) mirrorPut(root, s);
  for (const s of lazySheets.values()) mirrorPut(root, s);
  log("css-mod: 登记样式落点（影子根）· 同步 " + n + " 张");
  return n;
}

/** 注销落点（根被拆掉时调用）。 @param {Document | ShadowRoot} root */
export function unregisterStyleRoot(root) { mirrorRoots.delete(root); }

/** @param {string} name @param {any} ns mod 命名空间（读 ns.sheet） */
export function adoptCssMod(name, ns) {
  const sheet = ns && ns.sheet;
  if (typeof CSSStyleSheet === "undefined" || !(sheet instanceof CSSStyleSheet)) return;
  const prevSheet = adoptedSheets.get(name);
  if (prevSheet === sheet) return;
  // **sheet 变了就换**（审查 C7）：dev 下 HMR/重建可能给出同名但新的 sheet 实例；
  // 在 `has(name)` 时直接 return ⇒ 页面会一直用旧样式，直到整页刷新。
  if (prevSheet) document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== prevSheet);
  adoptedSheets.set(name, sheet);
  document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
  mirrorAll(sheet);
  log("css-mod: adopted " + name + " · rules=" + sheet.cssRules.length);
}

/**
 * **回收**（§13.7）：声明里已不再出现的 CSS mod ⇒ 把它的 sheet 从 `adoptedStyleSheets` 摘掉。
 * 为什么必须有它：`<style>` 自注入写进 head 后不可回收（只能靠幂等 id 防重复），
 * 而 adopt 的 sheet 可以 remove —— 这是"样式也随声明装载/卸载"的关键一步。
 * @param {Set<string>} activeNames 声明里还活着的 CSS mod 名
 */
export function pruneCssMods(activeNames) {
  for (const [name, sheet] of [...adoptedSheets]) {
    if (activeNames.has(name)) continue;
    adoptedSheets.delete(name);
    document.adoptedStyleSheets = document.adoptedStyleSheets.filter((s) => s !== sheet);
    mirrorDrop(sheet);
    log("css-mod: unadopted " + name);
  }
}

/** 采纳的**落点**：默认 `document`；传 `ShadowRoot` 即落到那个 shadow root（原生隔离，样式不外泄）。
 *  @param {string} name @param {CSSStyleSheet} sheet @param {Document | ShadowRoot} [target] @returns {boolean} */
function adoptLazySheet(name, sheet, target) {
  const host = target || document;
  if (!host || !("adoptedStyleSheets" in host)) return false;
  const cur = lazySheets.get(name);
  if (cur === sheet) return true;
  if (cur) host.adoptedStyleSheets = host.adoptedStyleSheets.filter((s) => s !== cur);
  lazySheets.set(name, sheet);
  host.adoptedStyleSheets = [...host.adoptedStyleSheets, sheet];
  if (host === document) mirrorAll(sheet);
  log("css-mod: adopted(lazy) " + name + " · rules=" + sheet.cssRules.length);
  return true;
}

/** mod 侧入口：`globalThis.__mdioAdoptSheet("katex", sheet[, target])` ⇒ 采纳但不参与声明剪枝。
 *  @param {string} name @param {any} ns @param {Document | ShadowRoot} [target] @returns {boolean} */
export function adoptCssModLazy(name, ns, target) {
  const sheet = ns && ns.sheet;
  if (typeof CSSStyleSheet === "undefined" || !(sheet instanceof CSSStyleSheet)) return false;
  return adoptLazySheet(name, sheet, target);
}

/** **作用域采纳**（§13.7 第 2 项）：把一组规则**限制在子树**内 —— 用 `@scope (<selector>) { … }` 包一层再采纳。
 *  为什么是 `@scope` 而不是"给每条选择器加前缀"：不必解析/重写 CSS 文本，浏览器按子树原生命中。
 *  另一条路：`target` 传 `ShadowRoot` ⇒ 落到该 shadow root 的 `adoptedStyleSheets`（原生隔离）。
 *  @param {string} name @param {string} cssText @param {string} selector @param {Document | ShadowRoot} [target] @returns {boolean} */
export function adoptScopedSheet(name, cssText, selector, target) {
  if (typeof CSSStyleSheet === "undefined" || !cssText || !selector) return false;
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync("@scope (" + selector + ") {\n" + cssText + "\n}");
    return adoptLazySheet(name, sheet, target);
  } catch (e) { log("css-mod: scoped 失败 " + name + " · " + String((e && e.message) || e).slice(0, 60)); return false; }
}

/** **样式表链接 → 影子根**（B 路线第③步）：esmx 的联邦 CSS 是 host 往 `<head>` 注入的 `<link>`，
 *  而影子根看不见 `<link>`（官方《联邦中的样式》）⇒ 把**同源** link 读成 `CSSStyleSheet` 采纳进那个根。
 *  · 幂等：**按根记表**（同一根同一 href 只采纳一次）；构造表不能跨根复用 ⇒ 每根各自 new 一张；
 *  · 跨源（`cssRules` 读不了）记**全局**跳过表，避免每根重复报同一行；还没有 `sheet` 的留到下轮；
 *  · **不用 fetch**：浏览器已经加载过这张表，直接从 CSSOM 读 `cssRules` 序列化（也避开了判据 R6b「数据经 client」）。
 *  @param {Document | ShadowRoot} root @returns {number} 本次镜像条数 */
const linkSeenByRoot = new WeakMap();
const unreadableLinks = new Set();
export function mirrorStyleLinks(root) {
  if (!root || !("adoptedStyleSheets" in root) || typeof document === "undefined") return 0;
  let seen = linkSeenByRoot.get(root);
  if (!seen) { seen = new Set(); linkSeenByRoot.set(root, seen); }
  const links = [...document.querySelectorAll('link[rel="stylesheet"][href]')];
  let n = 0;
  for (const l of links) {
    const href = String(l.href || "");
    if (!href || seen.has(href) || unreadableLinks.has(href)) continue;
    const css = l.sheet;
    if (!css) continue;                       // 还没加载完 ⇒ 下次调用再来
    let text = "";
    try { text = [...css.cssRules].map((r) => r.cssText).join("\n"); }
    catch (e) { unreadableLinks.add(href); log("css-mod: link 跨源跳过（cssRules 不可读）" + href.slice(0, 70)); continue; }
    if (!text) continue;
    try {
      const sheet = new CSSStyleSheet();
      sheet.replaceSync(text);
      if (root.adoptedStyleSheets.indexOf(sheet) < 0) {
        root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheet];
      }
      seen.add(href);
      n += 1;
      log("css-mod: link→影子根 " + href.slice(0, 70));
    } catch (e) { log("css-mod: link 采纳失败（跳过）" + href.slice(0, 70)); }
  }
  return n;
}

/** **属性前缀法采纳**（esmx 生态的 `@esmap/guard`）：把 CSS 文本按 `data-esmap-scope="<prefix>"` 重写再采纳 —— 
 *  正好补上「CSS 当 mod」缺的那块：全浏览器可用（`@scope` 要 Firefox 146+），且 `@keyframes` 会被改名（否则两条 app 的 `spin` 互相覆盖）。
 *  与 `adoptScopedSheet`（`@scope`）并存，谁合适用谁：属性法只挡 app→外；外面的样式仍会命中进来（这是它的已知边界）。
 *  @param {string} name @param {string} cssText @param {string} prefix @param {Document | ShadowRoot} [target] @returns {boolean} */
export function adoptCssScopedByAttr(name, cssText, prefix, target) {
  if (typeof CSSStyleSheet === "undefined" || !cssText || !prefix) return false;
  try {
    const scoped = namespaceCssKeyframes(scopeCssText(cssText, prefix), prefix);
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(scoped);
    return adoptLazySheet(name, sheet, target);
  } catch (e) { log("css-mod: attr-scoped 失败 " + name + " · " + String((e && e.message) || e).slice(0, 60)); return false; }
}

// mod 侧全局入口（与声明剪枝无关的那条路）：模块加载即安装。
(globalThis).__mdioAdoptSheet = (name, sheet, target) => adoptCssModLazy(String(name), { sheet }, target);
(globalThis).__mdioAdoptScopedSheet = (name, cssText, selector) => adoptScopedSheet(String(name), String(cssText), String(selector));
/** **动态 `<style>` → 影子根**（B 路线第④步）：CSS-in-JS / 库会往 head 塞 `<style>`，影子根看不见它。
 *  **两件事一起做**：① 给每个影子根一份副本（属性前缀穿不过影子边界，不复制就够不着 app 内容）；
 *  ② 用 `@esmap/guard` 的 `scopeCssText` 把**原件**改写成只对 `[data-esmap-scope="mdio-app"]` 生效（中心 `#app` 自带该属性）。
 *  **多根语义**：一份 head 观察者 + 多个根。后加入的根补拿已接管样式的副本（用保存的原文）；
 *  原件只按 guard 前缀改写**一次**（每根各自改写会双重前缀）；最后一个根卸载时才 stop 观察者并还原原件。
 *  · 默认**不动**：启动前就存在的 `<style>`（外壳/预置）、`data-vite-dev-id`（Vite）、`type=importmap`；`opts.exclude` 可再加（按根生效）；
 *  @param {Document | ShadowRoot} root @param {{exclude?: (el: Element) => boolean, restoreOnStop?: boolean}} [opts] */
const DYN_APP_SCOPE = "mdio-app";
const dynMirror = {
  active: false,
  roots: new Map(),     // root → { exclude }
  counts: new Map(),    // root → 该根已采纳的副本数
  seen: null,           // WeakSet<Element>：本会话已处理
  pre: null,            // Set<Element>：启动前已存在（不动）
  reverted: null,       // Map<Element, string>：原件 → 原文（还原用）
  items: [],            // Array<{ el, orig }>：已接管样式（后加入的根补副本）
  mo: null,
  restoreOnStop: true,
};
function dynAdoptInto(root, text, rec) {
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(text);
    if (root.adoptedStyleSheets.indexOf(sheet) >= 0) return false;
    root.adoptedStyleSheets = [...root.adoptedStyleSheets, sheet];
    dynMirror.counts.set(root, (dynMirror.counts.get(root) || 0) + 1);
    return true;
  } catch (e) { return false; }   // 单根失败不影响其它根
}
function dynTake(el) {
  const d = dynMirror;
  try {
    if (!d.active || !el || d.seen.has(el) || el.tagName !== "STYLE") return;
    if (d.pre.has(el)) return;                                     // 启动前就有的 = 外壳/预置，不动
    if (el.getAttribute("type") === "importmap") return;
    if (el.dataset && el.dataset.viteDevId != null) return;        // Vite 注入的外壳样式
    const text = String(el.textContent || "");
    if (!text.trim()) return;
    // **单根时保持旧语义**：该根 exclude ⇒ 整张不处理（不复制、不记录、不圈选）。
    // 多根时 exclude 只能按根生效（副本可跳过），原件仍会被圈选一次 —— 全局改写没法按根分裂，这一点写在这里。
    if (d.roots.size === 1) {
      const only = d.roots.values().next().value;
      if (only && typeof only.exclude === "function" && only.exclude(el)) return;
    }
    d.seen.add(el);
    d.items.push({ el, orig: text });
    // ① 每个根各补一份副本（构造表不能跨根复用）
    for (const [root, rec] of d.roots) {
      if (typeof rec.exclude === "function" && rec.exclude(el)) continue;
      dynAdoptInto(root, text, rec);
    }
    // ② 把原件圈进 app（guard 的属性前缀法）—— **只改一次**；库自带样式只复制不改写
    // 修（实测：Monaco **光标不可见**）：**库自带、且「编辑器住在哪棵树就要在哪生效」的运行期样式**
    //   一律**不许**被圈进 app scope。典型 = Monaco 运行期注入的 `<style class="monaco-colors">`（定义
    //   `--vscode-editorCursor-foreground` 等主题变量）：dev 面的 devkit Monaco **既不在 `#app` 子树、也不在影子根**，
    //   一旦被前缀化，主题变量取不到 ⇒ `.cursor` 背景算成 `rgba(0,0,0,0)`（能用鼠标聚焦、但看不见光标）。
    //   处置：这种样式**只做①给影子树补副本，跳过②改写原件** —— 原件留在 document，供 scope 外的编辑器使用。
    const libraryOwned = String(el.className || "").split(/\s+/).indexOf("monaco-colors") >= 0
      || text.indexOf("--vscode-editorCursor-foreground") >= 0;
    if (libraryOwned) return;
    try {
      d.reverted.set(el, text);
      el.textContent = namespaceCssKeyframes(scopeCssText(text, DYN_APP_SCOPE), DYN_APP_SCOPE);
    } catch (e) { /* 改不动 ⇒ 只保留副本 */ }
  } catch (e) { /* 解析失败 ⇒ 跳过这一张 */ }
}
function dynStart(rec) {
  const d = dynMirror;
  d.active = true;
  d.seen = new WeakSet();
  d.pre = new Set([...document.head.querySelectorAll("style")]);
  d.reverted = new Map();
  d.items = [];
  d.counts = new Map();
  d.restoreOnStop = !rec || rec.restoreOnStop !== false;
  for (const el of document.head.querySelectorAll("style")) dynTake(el);
  d.mo = new MutationObserver((ms) => { for (const m of ms) for (const nd of m.addedNodes) if (nd && nd.nodeType === 1) dynTake(nd); });
  d.mo.observe(document.head, { childList: true });
  log("css-mod: 动态样式已接管（" + d.roots.size + " 个影子根 · 原件圈进 " + DYN_APP_SCOPE + " · 启动前 " + d.pre.size + " 张不动）");
}
function dynRemoveRoot(root) {
  const d = dynMirror;
  if (!d.roots.has(root)) return;
  d.roots.delete(root);
  d.counts.delete(root);
  if (d.roots.size > 0) return;                                   // 还有根要这些副本 ⇒ 观察者与原件都不动
  d.active = false;
  try { if (d.mo) d.mo.disconnect(); } catch (e) { /* ignore */ }
  d.mo = null;
  if (d.restoreOnStop && d.reverted) {
    for (const [el, text] of d.reverted) { try { el.textContent = text; } catch (e) { /* ignore */ } }
  }
  d.reverted = null; d.seen = null; d.pre = null; d.items = [];
  log("css-mod: 动态样式镜像收摊（最后一个影子根已卸载）");
}
export function startDynamicStyleMirror(root, opts) {
  if (!root || !("adoptedStyleSheets" in root) || typeof document === "undefined") return { stop: () => {}, count: () => 0 };
  const o = opts || {};
  const d = dynMirror;
  const rec = { exclude: o.exclude };
  if (!d.roots.has(root)) {
    d.roots.set(root, rec);
    if (!d.active) dynStart(rec);
    else {
      // 后加入的根：补拿已接管样式的副本（原件已被改写，副本用保存的原文）
      for (const it of d.items) {
        if (typeof rec.exclude === "function" && rec.exclude(it.el)) continue;
        dynAdoptInto(root, it.orig, rec);
      }
    }
  }
  return {
    stop: () => dynRemoveRoot(root),
    count: () => d.counts.get(root) || 0,
  };
}

(globalThis).__mdioAdoptScopedByAttr = (name, cssText, prefix) => adoptCssScopedByAttr(String(name), String(cssText), String(prefix));
(globalThis).__mdioMirrorLinks = (root) => mirrorStyleLinks(root || document);   // ③ 的入口（供宿主/诊断调用）
(globalThis).__mdioStartStyleMirror = (root, exclude) => startDynamicStyleMirror(root, { exclude: exclude });   // ④ 的入口（默认不启动，谁需要谁开）
