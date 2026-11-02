// std —— mdio 的标准 MD render lib（mods 第一条默认装；声明里是 ### node 下的登记条目 - std）。
// 排版：真实元素（h1-h6/p/ul/ol/li/blockquote/pre/table）+ 语义类名（md-inline-code/md-admonition），
// 样式由宿主全局样式表提供（mdio_ui/demo/src/ui.css 的 .mdio-std 段，token 移植自旧壳 index.css）。
//
// **逐节点**（node 桶的契约：件只画当前节点自己的壳）：
//   · 块级孩子（p / bl / c / bq / 子节 …）交回框架 renderNode —— 每个孩子一个 memo 边界，
//     并且孩子自己会再走一遍认领（区域件才有机会命中它）；本库不再自己递归画整棵子树；
//   · 行内孩子（txt / st / em / cd …）就地渲染：行内语境由**父级**决定、节点极轻，不值一个组件实例；
//   · 节（h/sec）：<div class="mdio-std"> 包住**标题 + 全部孩子**（容器样式是给子树用的，必须包住正文；
//     与「每节一个 .mdio-std、子节各自再一个」同形）。
// 契约仍是普通 React 件：{ node, snap, exec, renderNode }（不 import mobx/MST）。
import { createElement as h, Fragment, createContext, useContext, useState, useEffect, useMemo, useRef, useCallback } from "react";
// 样式：**CSS 作为 mod**——本包另有 `theme.js` 导出 `sheet`（CSSStyleSheet），
// 由客户端装载器 adopt（实测 62 条规则进 adoptedStyleSheets，pages-live 11/11）。
// 这里保留真 .css 导入只为让构建产出资产（便于将来做 <link> 或预加载）；**不再自注入 <style>**。
import "./std/appearance.css";

const UNSAFE_URL_RE = /^(javascript:|data:|vbscript:)/i;

// ── 子节点身份键（React key）──
// 官方规则（MobX react-optimizations）：「Don't use array indexes or any value that might change in the future as key」。
// 为什么在库内也要做：本库内部递归 h(Block,…)/h(Inline,…) 的兄弟列表同样是可增删的（插入一个块/行内节点），
// 用下标 key 时**其后所有兄弟的 key 不变而 node 变了** ⇒ props 全变 ⇒ React 逐个重渲（实测：框架层的同类问题
// 在 200 节文档里插入一节是 202 实例重渲 / 233 次渲染，换身份键后降到 2 实例 / 33 次，↓86%）。
// 这里用**节点实例**发号（WeakMap）：MST 补丁不换未变节点的实例 ⇒ 存活项身份稳定；节点被替换时旧号随实例回收。
// 纯 JS、零依赖（本库不 import mobx/MST —— 契约要求）。
const __keyOf = new WeakMap();
let __keySeq = 0;
function keyOf(node) {
  if (node == null || (typeof node !== "object" && typeof node !== "function")) return "s";
  let k = __keyOf.get(node);
  if (k === undefined) { __keySeq += 1; k = __keySeq; __keyOf.set(node, k); }
  return k;
}
// ── 投影归一（新架构 HS 的形状约定，全部收敛在这一层；渲染分支只调这三个取数函数）──
// 约定（实测 4 篇文档全量节点）：
//   root: ct=[h]            节壳: t=标题, ct=节体      列表: bl/od > b/ol（key 在 t, 值在 ct[0]=p）
//   段落: p.ct 可能是字符串，也可能是 txt/cd 等子节点   表格: tr.ct = tab 分隔字符串（或单元格节点数组）
const kids = (node) => (node && Array.isArray(node.kids) ? node.kids : (node && Array.isArray(node.ct) ? node.ct : []));   // 优先读节点上的 getter（缓存点），非 MST 入参退回 ct
/** 文本摊平：**走读面的内容面**（`read.contentValueOf`）。
 *   不是 `read.textOf` —— 那是**名字面**（框架的 inlineTextOf：只摊平 `.t`，即标题文本 / bullet 的 tag），
 *  对 `p` / `txt` / `cd` / `tr` 这类**行内与叶节点**，名字面恒为空 ⇒ 值会整片丢掉
 *  （实测：未认领的 bullet 只剩 `值:`，值不在 DOM 里）。
 *  规则仍只有一处（读面）；参数序 = (read, node)。 */
const txtOf = (read, node) => { const v = read.contentValueOf(node); return typeof v === "string" ? v : ""; };
/** 读 `.t` 面（名字/标签/代码语言/序号）用的空 missing 账：readField 只在读不到时往里记一笔。 */
const noFace = new Set();
const tOf = (read, node) => read.readField(node, ".t", noFace) || "";
/** 列表项（b/ol/x）：key 在 t（可为空串 = 无 key）、值在 ct[0]。任务清单的 t 形如 "[x] …"，不算 key。 */
const itemOf = (read, node) => {
  const key = tOf(read, node);
  const isTask = key.startsWith("[");
  return { key, isTask, hasKey: !!key && !isTask, body: kids(node) };
};
/** 表格单元格取文本：**走读面**（同一处规则）；吃「tab 分隔字符串」「单元格节点数组」两种交付形态。 */
function mkCells(read, v) {
  const raw = Array.isArray(v) ? v.map((c) => { const s = read.contentValueOf(c); return typeof s === "string" ? s : ""; }) : [String(v == null ? "" : v)];
  return raw.flatMap((s) => String(s).split("\t")).filter((c) => c.length > 0);
}

function slugify(text) { return (text || "").toLowerCase().replace(/[^\w\s-]/g, "").replace(/\s+/g, "-").replace(/-+/g, "-").replace(/^-|-$/g, ""); }


// ── 基础 md 富件（**hooks 版**）──
// 依赖前提（三层，缺一就会崩在 dispatcher 上）：
//   ① 本库声明 uses: ["ui_base"] ⇒ 构建时 react/mobx 被外部化（产物不自带副本）；
//   ② 平台 provides ⇒ esmx 在页面 import map 生成裸 specifier 映射；
//   ③ 宿主用 shim 覆盖该映射 ⇒ 裸 react 解析到**宿主应用同一个实例**（= renderer 那一份）。

/** 宿主主题信号：body.dark（宿主 ui.css 的明暗开关）。变化时让 shiki/mermaid 重渲。 */
function useHostDark() {
  const [dark, setDark] = useState(() => (typeof document !== "undefined" ? document.body.classList.contains("dark") : true));
  useEffect(() => {
    if (typeof document === "undefined") return undefined;
    const obs = new MutationObserver(() => setDark(document.body.classList.contains("dark")));
    obs.observe(document.body, { attributes: true, attributeFilter: ["class"] });
    return () => obs.disconnect();
  }, []);
  return dark;
}

// ── Shiki：代码高亮（懒加载 + 跨实例缓存 + 双主题）──
// 为什么懒加载：shiki 含语法引擎（wasm/grammars），只为可见代码块付费；不可见时保持纯 <pre> 文本（零闪烁）。
let __shikiP = null;
const __shikiCache = new Map();      // key = code\u0000lang\u0000dark → html
const __shikiLangs = new Set();      // 已加载语言
const SHIKI_THEMES = { light: "vitesse-light", dark: "vitesse-dark" };
const SHIKI_KNOWN = ["rust", "bash", "shell", "sh", "javascript", "js", "jsx", "typescript", "ts", "tsx", "python", "json", "yaml", "yml", "markdown", "md", "html", "css", "sql", "go", "diff", "toml", "text"];
function shikiHighlighter() {
  if (!__shikiP) {
    __shikiP = import("shiki").then((m) => m.createHighlighter({ themes: [SHIKI_THEMES.light, SHIKI_THEMES.dark], langs: [] }));
  }
  return __shikiP;
}

/** 代码块：先出纯文本，可见后再高亮（Shiki）；失败/未知名 ⇒ 保持纯文本。 */
function MDCode({ code, lang }) {
  const dark = useHostDark();
  const hostRef = useRef(null);
  const langKey = (String(lang || "").trim().split(/\s+/)[0] || "text").toLowerCase();
  const langCls = "md-code lang-" + langKey.replace(/[^a-z0-9+#._-]/gi, "");
  useEffect(() => {
    const el = hostRef.current;
    if (!el || typeof IntersectionObserver === "undefined") return undefined;
    let alive = true;
    const key = code + "\u0000" + langKey + "\u0000" + (dark ? "d" : "l");
    const paint = (html) => { if (alive && hostRef.current) hostRef.current.innerHTML = html; };
    const cached = __shikiCache.get(key);
    if (cached) { paint(cached); return undefined; }
    const run = async () => {
      try {
        const hl = await shikiHighlighter();
        const use = SHIKI_KNOWN.includes(langKey) ? langKey : "text";
        const alias = use === "sh" ? "shell" : (use === "js" ? "javascript" : (use === "ts" ? "typescript" : use));
        if (!__shikiLangs.has(alias)) { await hl.loadLanguage(alias); __shikiLangs.add(alias); }
        const html = hl.codeToHtml(code, { lang: alias, themes: SHIKI_THEMES, defaultColor: false });
        __shikiCache.set(key, html);
        paint(html);
      } catch (e) { /* 降级：保留纯文本 */ }
    };
    const io = new IntersectionObserver((entries) => {
      if (entries.some((x) => x.isIntersecting)) { io.disconnect(); run(); }
    }, { rootMargin: "400px" });
    io.observe(el);
    return () => { alive = false; io.disconnect(); };
  }, [code, langKey, dark]);
  return h("div", { className: "md-codeblock", "data-lang": lang || undefined, ref: hostRef },
    h("pre", null, h("code", { className: langCls }, code)));
}

// ── Mermaid：图表（懒加载 + 主题跟随 + 失败降级为源码块）──
let __mermaidP = null;
let __mermaidSeq = 0;
function mermaidApi(dark) {
  if (!__mermaidP) {
    __mermaidP = import("mermaid").then((m) => {
      const api = m.default || m;
      api.initialize({ startOnLoad: false, securityLevel: "strict" });
      return api;
    });
  }
  return __mermaidP.then((api) => {
    try { api.initialize({ startOnLoad: false, securityLevel: "strict", theme: dark ? "dark" : "default" }); } catch (e) { /* 忽略 */ }
    return api;
  });
}

/** mermaid 代码块 → 图；渲染失败 ⇒ 退回源码块（内容不丢）。 */
function MDMermaid({ code }) {
  const dark = useHostDark();
  const [svg, setSvg] = useState(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let alive = true;
    setSvg(null); setFailed(false);
    (async () => {
      try {
        const api = await mermaidApi(dark);
        __mermaidSeq += 1;
        const out = await api.render("mdio-mermaid-" + __mermaidSeq, code);
        if (alive) setSvg(out && out.svg ? out.svg : String(out));
      } catch (e) { if (alive) setFailed(true); }
    })();
    return () => { alive = false; };
  }, [code, dark]);
  if (failed) return h("pre", { className: "md-mermaid md-mermaid--err", "data-lang": "mermaid" }, h("code", null, code));
  if (svg == null) return h("pre", { className: "md-mermaid md-mermaid--pending", "data-lang": "mermaid" }, h("code", null, code));
  return h("div", { className: "md-mermaid", dangerouslySetInnerHTML: { __html: svg } });
}

/** 一次性注入 KaTeX 样式（字体已 base64 内联在 katex-css.js ⇒ 宿主零参与、无资源 404）。 */
function ensureKatexCss(css) {
  if (typeof document === "undefined") return;
  if (document.getElementById("mdio-katex-css")) return;
  const st = document.createElement("style");
  st.id = "mdio-katex-css";
  st.textContent = css;
  document.head.appendChild(st);
}

// 排版样式由 CSS mod（`theme.js` 的 `sheet`）经 `adoptedStyleSheets` 投递，
// 可卸载、可作用域，且不写 head。`ensureStdCss()` 与其调用点一并删除。

/** 公式：KaTeX 排版。首帧先出**原文**（不阻塞、不闪烁空白），样式与引擎**懒加载**后替换为排版结果。 */
function MDMath({ node, display, read }) {
  const latex = read.readField(node, ".content", noFace) || "";   // 面读（T4：行内 run 的内容面）
  const [html, setHtml] = useState(null);
  useEffect(() => {
    let alive = true;
    setHtml(null);
    (async () => {
      try {
        const [katexMod, cssMod] = await Promise.all([import("katex"), import("./katex.js")]);
        // CSS mod 形态：宿主提供采纳入口就**采纳**（可卸载、不进 head）；否则回落自注入（老宿主 / 无 CSSStyleSheet）。
        const adopt = globalThis.__mdioAdoptSheet;   // 宿主注入的采纳入口（entry.client 里挂）
        if (!(adopt && cssMod.sheet && adopt("katex", cssMod.sheet))) ensureKatexCss(cssMod.css);
        const katex = katexMod.default || katexMod;
        const out = katex.renderToString(latex, { throwOnError: false, displayMode: !!display, output: "html", strict: false });
        if (alive) setHtml(out);
      } catch (e) { /* 降级：保留原文（下面那条分支） */ }
    })();
    return () => { alive = false; };
  }, [latex, display]);
  if (html == null) {
    return display
      ? h("div", { className: "md-math md-math--raw" }, latex)
      : h("span", { className: "md-math md-math--raw" }, latex);
  }
  return h(display ? "div" : "span", { className: "md-math", dangerouslySetInnerHTML: { __html: html } });
}

/** 图片：加载态 + 失败兜底 + 点击放大（Esc/点击关闭，附下载）。w/h = HTML <img> 自带尺寸（可选）。 */
function MDImage({ node, w, h: eh, align }) {
  // core 的 IMAGE_ATTRS = { s, alt, ti }（effect.rs:1932-1936）⇒ 读 s/alt/ti。
  // **投影名 `al` 的兜底不存在**：两个名字读同一个值 = 两套真话；`h` 只留给链接节点（`lk`）。
  const src = typeof node.s === "string" ? node.s : "";
  const alt = typeof node.alt === "string" ? node.alt : "";
  const [loaded, setLoaded] = useState(false);
  const [failed, setFailed] = useState(false);
  const [zoomed, setZoomed] = useState(false);
  useEffect(() => { setLoaded(false); setFailed(false); }, [src]);
  if (failed) return h("span", { className: "md-image-error", title: src }, "🖼 图片加载失败" + (alt ? "：" + alt : ""));
  return h(Fragment, null,
    h("img", {
      src, alt, loading: "lazy", decoding: "async",
      width: w || undefined, height: eh || undefined, align: align || undefined,
      className: "md-image" + (loaded ? " is-loaded" : ""),
      style: { maxWidth: "100%", borderRadius: 8, cursor: loaded ? "zoom-in" : "default", opacity: loaded ? 1 : 0.35 },
      onLoad: () => setLoaded(true), onError: () => setFailed(true),
      onClick: () => { if (loaded) setZoomed(true); },
    }),
    zoomed ? h("div", { className: "md-image-zoom", onClick: () => setZoomed(false) },
      h("img", { src, alt }),
      h("a", { className: "md-image-act", href: src, download: true, target: "_blank", rel: "noopener noreferrer", onClick: (e) => e.stopPropagation() }, "下载"),
      h("button", { className: "md-image-act", onClick: (e) => { e.stopPropagation(); setZoomed(false); } }, "关闭")) : null);
}

// ── markdown 内嵌 HTML：DOMPurify 清洗 → DOM → React ────────────────────────────
// 背景：引擎把 HTML 原文放在节点上（hb: {t, ct}；行内 HTML 落成 txt / 单元格字符串），"解不解释"是渲染器策略。
// 本件只做三件事：① 用 DOMPurify 把原文清洗成 DOM（浏览器解析 + 显式白名单；不用它的默认宽松配置）
// ② 拿回**已清洗 DOM** ③ 机械映射成 React（img→MDImage、table→现有 class）。安全判断全在清洗器里，这里不做。
// 诊断开关：`?html=off` ⇒ 原文透传（对照现状 / 出问题时的即时回退；默认关）。
const HTML_OFF = (() => { try { return new URLSearchParams(location.search).get("html") === "off"; } catch (e) { return false; } })();
const HTML_TAGS = ["img", "br", "table", "thead", "tbody", "tfoot", "tr", "th", "td", "caption", "a",
  "b", "strong", "i", "em", "u", "s", "del", "code", "pre", "kbd", "sub", "sup", "small", "mark", "span",
  "div", "p", "blockquote", "ul", "ol", "li", "hr", "h1", "h2", "h3", "h4", "h5", "h6", "details",
  "summary", "figure", "figcaption", "center", "dl", "dt", "dd"];
const HTML_ATTRS = ["src", "alt", "title", "width", "height", "href", "colspan", "rowspan", "align",
  "start", "open", "cite"];
const HTML_ATTR_MAP = { colspan: "colSpan", rowspan: "rowSpan", readonly: "readOnly", tabindex: "tabIndex", class: "className", for: "htmlFor" };
// 布尔属性：DOM 属性是 boolean，React 收到 "" 会写成 `.open = ""`（布尔化为 false）⇒ 必须转 true（HTML 语义 = 存在即真）。
const HTML_BOOLEAN_ATTRS = new Set(["open", "hidden", "disabled", "required", "multiple", "selected", "checked", "readonly", "autofocus"]);
// React 19 对 table 结构里的空白文本节点会硬报错（"whitespace text nodes cannot be a child of <table>"），
// 而浏览器解析出来的 DOM 里恰好有这些格式化空白 ⇒ 映射时丢掉（只丢空白，不丢内容）。
const HTML_TABLE_PARENTS = new Set(["table", "thead", "tbody", "tfoot", "tr", "colgroup"]);
// 空元素（void）不能带 children：React 19 对 `<br children=[]>` 会硬报错（#137 = void element with children）。
const HTML_VOID = new Set(["br", "hr", "area", "base", "col", "embed", "input", "link", "meta", "source", "track", "wbr"]);
const HTML_LIMIT = 256 * 1024;

/** DOMPurify：懒加载（首次遇到内嵌 HTML 才拉；与 shiki/mermaid/katex 同模式）。 */
let __dpP = null;
function domPurifyMod() {
  if (!__dpP) {
    __dpP = import("dompurify").then((m) => { const D = m.default || m; return D && D.isSupported ? D : null; }).catch(() => null);
  }
  return __dpP;
}

/** DOMPurify：显式白名单（不吃默认的宽松配置：默认允许 CSS/style 属性与 data-*）。 */
async function sanitizePurify(raw) {
  const D = await domPurifyMod();
  if (!D) return null;
  try {
    return D.sanitize(raw, {
      RETURN_DOM_FRAGMENT: true, ALLOWED_TAGS: HTML_TAGS, ALLOWED_ATTR: HTML_ATTRS,
      ALLOW_DATA_ATTR: false, KEEP_CONTENT: true,
    });
  } catch (e) { return null; }
}

/** 清洗 → 已清洗 DOM；off/失败/超限 ⇒ null（调用点必须回退原文，绝不渲染未清洗内容）。 */
async function sanitizeToDom(raw) {
  if (HTML_OFF) return null;
  if (typeof raw !== "string" || !raw.trim() || raw.length > HTML_LIMIT) return null;
  return await sanitizePurify(raw);
}

/** 已清洗 DOM → React 子树：纯机械映射（没有安全判断；img 复用 MDImage 的加载/失败/放大 UX）。 */
function domToReact(node, keyBase) {
  const out = [];
  const kids = node.childNodes || [];
  const parentTag = node.tagName ? node.tagName.toLowerCase() : "";
  for (let i = 0; i < kids.length; i += 1) {
    const n = kids[i];
    if (n.nodeType === 3) {                                     // 文本：React 自动转义
      if (HTML_TABLE_PARENTS.has(parentTag) && !n.nodeValue.trim()) continue;
      out.push(n.nodeValue);
      continue;
    }
    if (n.nodeType !== 1) continue;                              // 注释/其他：不画
    const tag = n.tagName.toLowerCase();
    const key = keyBase + ":" + i + ":" + tag;
    if (tag === "img") {
      out.push(h(MDImage, {
        key, w: Number(n.getAttribute("width")) || 0, h: Number(n.getAttribute("height")) || 0,
        align: n.getAttribute("align") || "",
        node: { s: n.getAttribute("src") || "", alt: n.getAttribute("alt") || "", ti: n.getAttribute("title") || "" },
      }));
      continue;
    }
    const props = { key };
    for (const a of Array.from(n.attributes)) {
      const name = a.name;
      props[HTML_ATTR_MAP[name] || name] = HTML_BOOLEAN_ATTRS.has(name) ? true : a.value;
    }
    if (HTML_VOID.has(tag)) { out.push(h(tag, props)); continue; }
    const inner = domToReact(n, key);
    if (tag === "table") out.push(h("div", { key, className: "md-table-wrap" }, h("table", { className: "md-table" }, inner)));
    else out.push(h(tag, props, inner));
  }
  return out;
}

/** 块级 HTML（hb/he）：首帧/失败显示原文；清洗完成后换成受控元素树。 */
function MDHtml({ node }) {
  const raw = typeof node.ct === "string" ? node.ct : "";
  const [kids, setKids] = useState(null);
  useEffect(() => {
    let alive = true;
    setKids(null);
    sanitizeToDom(raw).then((dom) => { if (alive && dom) setKids(domToReact(dom, "hb")); }).catch(() => { /* 回退原文 */ });
    return () => { alive = false; };
  }, [raw]);
  if (HTML_OFF) return h("div", null, raw);                    // 诊断：原文透传（对照现状 / 回退）
  if (kids) return h("div", { className: "md-html", "data-html": "dp" }, kids);
  return h("div", { className: "md-html-raw" }, raw);          // 首帧 / 清洗器不可用
}

/** 表格单元格里的行内 HTML（单元格是纯字符串）：同一管线，输出无外层块。 */
function MDFrag({ text }) {
  const [kids, setKids] = useState(null);
  useEffect(() => {
    let alive = true;
    setKids(null);
    if (typeof text !== "string" || text.indexOf("<") < 0) return () => { alive = false; };
    sanitizeToDom(text).then((dom) => { if (alive && dom) setKids(domToReact(dom, "fr")); }).catch(() => { /* 回退原文 */ });
    return () => { alive = false; };
  }, [text]);
  if (kids) return h(Fragment, null, kids);
  return text;
}

/** 单行（tr）：单元格由 tab 分隔的 ct 拆出 —— tb 的表体与「tr 单独派发」共用这一处（P0⑤）。 */
function MDRow({ node, read }) {
  return h("tr", null, mkCells(read, node.ct).map((c, j) => h("td", { key: "c" + j }, h(MDFrag, { text: c }))));
}

/** 表格：语义元素 + 列宽拖拽（相邻两列此消彼长，含最小宽守卫）。 */
function MDTable({ node, snap, read }) {
  // 该加的 memo 就加：表头/单元格是从交付树**派生**出来的（拆 tab、拼文本），
  // 用框架交下来的内容版本 snap 当键（MST 每节点缓存 + structural sharing ⇒ 未变即同一对象）。
  const header = useMemo(() => {
    const head = (read.contentValueOf(node) || {}).head || "";
    return head.length > 0 ? head.split("\t").filter((c) => c.length > 0) : mkCells(read, node.ct);
  }, [snap, node, read]);
  const rows = useMemo(() => kids(node).filter((c) => c && c.nt === "tr").map((r) => ({ k: keyOf(r), cells: mkCells(read, r.ct) })), [snap, node, read]);
  const ref = useRef(null);
  const drag = useRef(null);
  const [widths, setWidths] = useState(null);
  const onDown = useCallback((i) => (e) => {
    const ths = ref.current ? ref.current.querySelectorAll("thead th") : null;
    if (!ths || !ths[i] || !ths[i + 1]) return;
    e.preventDefault();
    try { e.currentTarget.setPointerCapture(e.pointerId); } catch (err) { /* 合成事件无捕获 */ }
    drag.current = { i, x: e.clientX, w: Array.from(ths, (t) => t.getBoundingClientRect().width) };
    setWidths(drag.current.w);
  }, []);
  const onMove = useCallback((e) => {
    const d = drag.current;
    if (!d) return;
    const pair = d.w[d.i] + d.w[d.i + 1];
    const min = Math.min(48, Math.floor(pair / 2));
    const val = Math.max(min, Math.min(pair - min, Math.round(d.w[d.i] + e.clientX - d.x)));
    const next = d.w.slice();
    next[d.i] = val; next[d.i + 1] = pair - val;
    setWidths(next);
  }, []);
  const onUp = useCallback(() => { drag.current = null; }, []);
  return h("div", { className: "md-table-wrap" },
    h("table", { ref, className: "md-table", onPointerMove: onMove, onPointerUp: onUp, onPointerCancel: onUp },
      widths ? h("colgroup", null, widths.map((w, i) => h("col", { key: "w" + i, style: { width: Math.round(w) + "px" } }))) : null,
      header.length > 0 ? h("thead", null, h("tr", null, header.map((c, i) => h("th", { key: "h" + i },
        h(MDFrag, { text: c }),
        i < header.length - 1 ? h("span", { className: "md-col-grip", title: "拖动调整列宽", onPointerDown: onDown(i) }) : null)))) : null,
      h("tbody", null, rows.map((r) => h("tr", { key: r.k }, r.cells.map((c, j) => h("td", { key: "c" + j }, h(MDFrag, { text: c }))))))));
}
// ── 内联（旧 RenderInline 移植） ──
/** 软换行（sb）在引用块里按**换行**渲染（2026-10-31）：mdio 的 bq 是**逐行**语义（引擎 content 面就是「逐行剥离 `> `」），
 *  引用/聊天记录里每行都是独立一行；而在普通段落里仍保持 CommonMark 的「空格连接」（否则按 80 列折行的正文会变成一行一句）。
 *  用上下文而不是改 Inline 的默认值：框架 renderNode 逐节点派发时上下文照样穿透，不影响 mod 认领与 memo 边界。
 *  对照：硬换行 lb 在任何语境都出 `<br>`（见下）。 */
const SOFTBREAK_BR = createContext(false);
function Inline({ node, read }) {
  const k = node && node.nt;
  const softBreakBr = useContext(SOFTBREAK_BR);
  const txt = (n) => { const v = read.contentValueOf(n); return typeof v === "string" ? v : ""; };   // 内容面（不是名字面；见 txtOf 的注释）
  switch (k) {
    case "txt": return node.center ? h("span", { style: { display: "block", textAlign: "center", width: "100%" } }, node.ct ?? "") : (node.ct ?? "");
    case "st": return h("strong", null, kids(node).map((c, i) => h(Inline, { key: keyOf(c), node: c, read })));
    case "em": return h("em", null, kids(node).map((c, i) => h(Inline, { key: keyOf(c), node: c, read })));
    case "dl": return h("del", null, kids(node).map((c, i) => h(Inline, { key: keyOf(c), node: c, read })));
    case "highlight": return h("mark", null, kids(node).map((c, i) => h(Inline, { key: keyOf(c), node: c, read })));
    case "insert": return h("ins", null, kids(node).map((c, i) => h(Inline, { key: keyOf(c), node: c, read })));
    case "subscript": return h("sub", null, kids(node).map((c, i) => h(Inline, { key: keyOf(c), node: c, read })));
    case "superscript": return h("sup", null, kids(node).map((c, i) => h(Inline, { key: keyOf(c), node: c, read })));
    case "cd": return h("code", { className: "md-inline-code" }, read.readField(node, ".content", noFace) || txt(node));
    // p 在行内语境（bullet 的值、标题数组里的段落）：展开子节点，保留行内标记（cd→code、st→strong…）
    case "p": return kids(node).length ? h("span", null, kids(node).map((c, i) => h(Inline, { key: keyOf(c), node: c, read }))) : txt(node);
    case "lk": { const href = (node.h || "").trim(); const safe = !UNSAFE_URL_RE.test(href) ? href : ""; const target = safe && !href.startsWith("#") ? "_blank" : undefined; return h("a", { href: safe || undefined, title: node.ti ?? undefined, target, rel: target === "_blank" ? "noopener noreferrer" : undefined }, kids(node).map((c, i) => h(Inline, { key: keyOf(c), node: c, read }))); }
    case "im": return h(MDImage, { node });   // 加载态 + 失败兜底 + 点击放大（见 MDImage）
    case "lb": return h("br", null);
    case "sb": return softBreakBr ? h("br", null) : " ";
    case "al": { const href = (node.h || "").trim(); const safe = !UNSAFE_URL_RE.test(href) ? href : ""; return h("a", { href: safe || undefined, target: safe && !href.startsWith("#") ? "_blank" : undefined, rel: "noopener noreferrer" }, node.ct ?? ""); }
    case "math_inline": case "mt": return h(MDMath, { node, display: false, read });   // KaTeX 排版（懒加载；失败降级原文）
    case "math_block": case "mb": return h(MDMath, { node, display: true, read });
    case "emoji": return node.name ?? "";
    case "checkbox": case "checkbox_input": return h("input", { type: "checkbox", disabled: true, checked: !!node.checked, style: { marginRight: 5 } });
    case "reference": case "footnote_reference": return h("sup", { style: { fontSize: 11, cursor: "pointer" } }, "[" + (node.id ?? "") + "]");
    default: return h("span", null, txt(node));
  }
}

// ── 块级（旧 RenderNode 移植） ──
const BLOCK_CHILD = new Set(["p", "h", "b", "x", "ol", "bl", "od", "c", "ic", "bq", "tb", "hb", "hr", "cm", "he"]);
/** 行内 run 的 nt：**掉到块派发**时交给 Inline（不再出裸 `<div>` —— P0⑤ 的 `tx` 等，实测 tx 108 实例）。
 *  与 BLOCK_CHILD 互补；两者都不在 ⇒ 仍走 default（未知类型兜底）。 */
const INLINE_RUN = new Set(["txt", "tx", "sb", "lb", "cd", "em", "st", "dl", "lk", "al", "mt", "dm", "im", "rf", "hi", "entity", "text", "math_inline", "math_block", "mb"]);
const ADMONITION = { note: "ℹ", tip: "💡", warning: "⚠️", danger: "🚫", info: "ℹ" };

/**
 * 块级壳：**只画当前节点**。结构孩子交回框架 renderNode（逐节点 ⇒ 每个孩子一个 memo 边界 + 一次认领机会）；
 * 行内孩子就地走 Inline（行内语境由调用点决定，见 BLOCK_CHILD 那条分支）。
 */
function Block({ node, snap, renderNode, read, modMatch, use, exec, appRouter }) {
  const txt = (n) => { const v = read.contentValueOf(n); return typeof v === "string" ? v : ""; };   // 内容面
  const nt = node.nt;
  const ch = kids(node);
  // match trait（std 转正）：结构孩子的「换件/不画」由 std 自己的表回答；
  // 命中但取不到件 ⇒ 就地按默认块画（不交回框架，避免回环）；未命中 ⇒ 照旧 renderNode。
  const matcher = modMatch ? modMatch("std") : null;
  const renderChild = (c) => {
    const hit = matcher ? matcher(c) : null;
    if (hit === "_HIDDEN") return null;
    if (hit) {
      const C = use ? use(hit) : null;
      if (C) return h(C, { node: c, renderNode, read, exec, snap, modMatch, use, appRouter });
      return h(Block, { node: c, snap, renderNode, read, modMatch, use, exec, appRouter });
    }
    return renderNode(c);
  };
  if (INLINE_RUN.has(nt)) return h(Inline, { node, read });     // 行内 run 掉到块派发 ⇒ 交 Inline（P0⑤）
  switch (nt) {
    case "h": {
      const level = Math.min(node.lv || 1, 6);
      const Tag = "h" + level;
      // 标题只在 t：字符串＝源文本；行内数组＝带标记的标题（实测全库 355 处，如 setext）。
      // 绝不能退到 txt(node)——它会先递归 ct（那是**节体**），标题会变成整节正文。
      // 富标题：`t` 是 **run 数组** ⇒ 这一处读的是**孩子通道**（与 `kids(node)` 读 `ct` 同一条规矩），
      //   不是面值读取 ⇒ 判据 check_no_direct_face 只数面值读（`typeof node.t === "string"` 那种）。
      const inlineTitle = Array.isArray(node.t) ? node.t : null;
      const title = inlineTitle ? inlineTitle.map((c) => txtOf(read, c)).join("") : tOf(read, node);
      return h(Tag, { id: slugify(title) || undefined },
        inlineTitle ? inlineTitle.map((c, i) => h(Inline, { key: keyOf(c), node: c, read })) : title);
    }
    case "b": case "ol": {
      const it = itemOf(read, node);
      const tRaw = tOf(read, node);
      // 任务项两种形态：旧投影 t="[x] …"；新投影把勾选标记单独放 t（实测 /all_types.md 的 `- [x] checklist` → t="x"）
      const isChecklist = /^\[[ xX]\]/.test(tRaw) || tRaw === "x" || tRaw === " ";
      const checked = /\[[xX]\]/.test(tRaw) || tRaw === "x";
      // key/值的分隔：值侧自带前导空格（p.ct = " v"）时不重复加
      // ol 的 t 是**源序号**（实测 "1"/"4"/"6"/"7"），不是 key：用 <li value> 保留源编号，绝不加 ": " 前缀
      const isOrdered = nt === "ol";
      const num = isOrdered && /^\d+$/.test(tOf(read, node)) ? Number(tOf(read, node)) : undefined;
      const keyNode = (!isOrdered && !isChecklist && it.hasKey) ? h("span", { className: "md-bullet-key" }, it.key + ":" + (ch.length && !/^\s/.test(String(txt(ch[0]) || "")) ? " " : "")) : null;
      // p 是「值的载体」：bullet 是行内语境，p 走 Inline 展开（不另起块）
      const content = ch.length ? ch.map((c, i) => (c.nt === "p" ? h(Inline, { key: keyOf(c), node: c, read }) : BLOCK_CHILD.has(c.nt) ? renderChild(c) : h(Inline, { key: keyOf(c), node: c, read }))) : txt(node);
      if (isChecklist) return h("li", { style: { display: "flex", alignItems: "flex-start", gap: 6, listStyle: "none" } }, h("input", { type: "checkbox", disabled: true, checked, style: { marginTop: "0.35em" } }), h("span", { style: checked ? { textDecoration: "line-through", opacity: 0.5 } : undefined }, content));   // 旧壳 RenderNode:137-147
      return h("li", num === undefined ? null : { value: num }, keyNode, content);
    }
    case "x": {
      const checked = !!(node.checked || tOf(read, node).includes("[x]"));
      const xi = itemOf(read, node);
      const xKey = xi.hasKey ? h("span", { className: "md-bullet-key" }, xi.key + ": ") : null;
      return h("li", { style: { display: "flex", alignItems: "flex-start", gap: 6, listStyle: "none" } }, h("input", { type: "checkbox", disabled: true, checked, style: { marginTop: "0.35em" } }), xKey, h("span", { style: checked ? { textDecoration: "line-through", opacity: 0.5 } : undefined }, ch.length ? ch.map((c, i) => h(Inline, { key: keyOf(c), node: c, read })) : txt(node)));
    }
    case "bl": case "od": {
      const Tag = nt === "od" ? "ol" : "ul";
      const firstNum = nt === "od" && ch.length && /^\d+$/.test(String(ch[0].t == null ? "" : ch[0].t)) ? Number(ch[0].t) : undefined;
      return h(Tag, firstNum === undefined ? null : { start: firstNum }, ch.map((c) => renderChild(c)));
    }
    case "c": case "ic": {
      const inner = ch.find((c) => c.nt === "c" || c.nt === "ic");
      const src = inner || node;                    // 内容叶优先（引擎把 fence 内容放在 c/ic 上）
      const lang = tOf(read, src);
      const code = read.readField(src, ".content", noFace) || "";
      if (lang === "mermaid") return h(MDMermaid, { code });
      return h(MDCode, { code, lang });
    }
    case "p": return h("p", null, ch.length ? ch.map((c, i) => h(Inline, { key: keyOf(c), node: c, read })) : txt(node));
    case "tb": return h(MDTable, { node, snap, read });   // 表头在 cols（tab 拼接）；行用 tr 身份键；列宽可拖（见 MDTable）
    // tr 单独派发（promote.tb 落地后行会被单独认领）：出一行语义表格，无件则表塌（P0⑤）
    case "tr": return h("div", { className: "md-table-wrap" }, h("table", { className: "md-table" }, h("tbody", null, h(MDRow, { node, read }))));
    // 注释：引擎侧 Comment 是 side_effect 容器（selector.rs:75-115 路由离开块树、node_category 判 Skip）
    // ⇒ 与 MD 惯例一致：**不渲染**（掉 default 会出裸 `<div>`，见 P0⑤）
    case "cm": return null;
    case "bq": {
      // bq 是**逐行**语义：只把 bq 自己的段落（直接子 p）按换行渲染（sb → <br>）。
      // 列表/代码/表格等子块交给它们各自的 render comp（软换行仍按各自规则，例如列表项内折行仍用空格连接）；
      // 嵌套 bq 由它自己的 bq case 再开一次作用域。子节点仍走框架 renderNode（保留 mod 认领/memo 边界）。
      const inner = ch.length
        ? ch.map((c) => (c.nt === "p" ? h(SOFTBREAK_BR.Provider, { key: keyOf(c), value: true }, renderChild(c)) : renderChild(c)))
        : txt(node);
      return h("blockquote", { cite: node.cite }, inner);
    }
    // 块级 HTML（hb）：原文在 ct —— 走上面"DOMPurify 清洗 → DOM → React"管线（诊断回退：?html=off）。he 目前引擎不发，留着对称。
    case "hb": case "he": return h(MDHtml, { node });
    case "hr": return h("hr", null);
    case "admonition": {
      const admKind = node.kind || "note";
      return h("div", { className: "md-admonition md-admonition--" + admKind }, h("div", { className: "md-admonition-title" }, (ADMONITION[admKind] || "ℹ") + " " + (node.title || (admKind.charAt(0).toUpperCase() + admKind.slice(1)))), ch.map((c) => renderChild(c)));
    }
    case "vmr_container": return h("div", { className: "md-admonition md-admonition--note" }, node.name ? h("div", { className: "md-admonition-title" }, node.name) : null, ch.map((c) => renderChild(c)));
    default: return h("div", null, typeof node.ct === "string" ? node.ct : ch.map((c) => renderChild(c)));
  }
}

// 「什么算一节」**不再手抄**：由框架经**读面**下发（`read.isSectionShell`，就是 `uiMd` 里那个同名函数）。
// 为什么不能 import：`ui_std` 的 `uses` 只有 `ui_base`（分层不许 import `ui_core`），判官 R1 也禁 services 互 import
// ⇒ 读面（props `read`）是**唯一**的合法通道。判决：`.tmp/p0/check_section_source.mjs`（本文件不许再有本地谓词）。

/**
 * 默认导出 = **每个节点**的壳（框架逐节点调用本件；不再是"一次调用画整篇"）。
 * 节：\`.mdio-std\` 容器**只包这一格**（标题）；**孩子由框架补画**（`NodeView` 的 `SectionKids`）。
 * 其余节点：直接出元素，孩子由 Block 按语境决定就地渲染还是交回框架。
 *
 *  **修（业主实测「渲染两次」的根因之一）**：节分支还 `kids(node).map(renderNode)`
 * 画了一遍孩子 ⇒ 与框架的补画**重复**：每个**未被认领**的节，其子树都会被画两遍。
 * 排版作用域由**文档体**那层 `.mdio-doc.mdio-std`（`DocPane` 已包）负责 ⇒ 这里不必再包住子树。
 */
export default function Std({ node, snap, renderNode, read, modMatch, use, exec, appRouter }) {
  // ensureStdCss() 不存在：样式由 CSS mod 采纳（见文件头注释）
  // std 转正：本节点先问自己的 trait 表；命中且取得到件 ⇒ 交件；
  // `_HIDDEN` ⇒ 不画；命中但取不到件 ⇒ 落回 Block（避免回环）。
  const matcher = modMatch ? modMatch("std") : null;
  const hit = matcher ? matcher(node) : null;
  if (hit === "_HIDDEN") return null;
  if (hit) {
    const C = use ? use(hit) : null;
    if (C) return h(C, { node, snap, renderNode, read, exec, modMatch, use, appRouter });
  }
  if (!read.isSectionShell(node.nt)) return h(Block, { node, snap, renderNode, read, modMatch, use, exec, appRouter });
  return h("div", { className: "mdio-std" }, h(Block, { node, snap, renderNode, read, modMatch, use, exec, appRouter }));
}