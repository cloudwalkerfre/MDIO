#!/usr/bin/env node
// port-theme-hostcss —— 把两个「宿主主题」转成 mdio 皮肤（与 port-theme-github/mdmdt 同一套口径）。
//
// 输入（vendor/）：
//   · macOS 27 Themes（christoarmani/typora-theme-macos27，MIT）→ macos27.css：单文件，暗色在
//     `@media (prefers-color-scheme: dark)` 里（:root 变量 + #write/body 覆盖）。
//   · Bloom · mist（webkubor/typora-Bloom-theme，MIT）→ bloom-mist.css + bloom-mist-dark.css：
//     built 的两份完整主题（浅色 / 深色各自成篇），结构同源 ⇒ 暗色按声明差分输出，不复制整份。
//
// 变换（与 github/mdmdt 一致）：
//   1. 只留内容规则：`#write` → `.mdio-doc`，裸内容标签 → `.mdio-doc <tag>`；Typora 编辑器外壳
//      （#typora-sidebar / .cm-s-* / #md-* / outline / file-tree / dropdown…）整族丢弃。
//   2. token 从 `:root` 上移到**门控元素**（.br-shell），变量统一改名（--mac-* / --bl-*）避免与宿主撞名；
//      暗色 token 双写成 `body.dark`（document 面）与 `:host(.dark)`（影子根面）。
//   3. 画布：上游 `body` 的底色/渐变移到宿主窗格 `.br-doc`（理由同 github：.mdio-doc 上色会得到
//      随内容高度截断的色块），`.mdio-doc` 保持透明；正文排版（font/color/line-height/letter-spacing）落到 `.mdio-doc`。
//   4. Bloom 随包的 MiSans 字体不搬（字体许可未随仓附）：@font-face 整块删除，字体栈替换为系统 CJK 栈。
//   5. 布局声明（max-width / margin / padding）从基座规则里剥掉 —— 列宽与留白归宿主 app。
//
// 用法：node scripts/port-theme-hostcss.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const themes = path.join(here, "..", "src", "themes");
const vendor = path.join(themes, "vendor");

/* ── 通用工具 ─────────────────────────────────────────────── */
const stripComments = (s) => s.replace(/\/\*[\s\S]*?\*\//g, "");
const stripBraced = (css, re) => {
  // 删除匹配 `re` 起始的整块（括号配平），返回 { rest, blocks }
  const blocks = [];
  let out = "";
  let i = 0;
  while (i < css.length) {
    const m = re.exec(css.slice(i));
    if (!m) { out += css.slice(i); break; }
    out += css.slice(i, i + m.index);
    let j = i + m.index + m[0].length;
    let depth = 1;
    while (j < css.length && depth > 0) {
      if (css[j] === "{") depth += 1;
      else if (css[j] === "}") depth -= 1;
      j += 1;
    }
    blocks.push(css.slice(i + m.index, j));
    i = j;
  }
  return { rest: out, blocks };
};
const parseRules = (css) => {
  const out = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m;
  while ((m = re.exec(css))) out.push({ sel: m[1].trim().replace(/\s+/g, " "), body: m[2].trim().replace(/\s+/g, " ") });
  return out;
};
const renameVars = (s, prefix) => s.replace(/--([\w-]+)/g, (_, n) => `--${prefix}-${n}`);


/** 按逗号拆选择器列表，但尊重括号（Typora 主题里大量 :is(a, b)、:not(...)）。 */
function splitSelectors(sel) {
  const out = [];
  let cur = "";
  let depth = 0;
  let quote = "";
  for (let i = 0; i < sel.length; i += 1) {
    const ch = sel[i];
    if (quote) { cur += ch; if (ch === quote && sel[i - 1] !== "\\") quote = ""; continue; }
    if (ch === '"' || ch === "'") { quote = ch; cur += ch; continue; }
    if (ch === "(") depth += 1;
    else if (ch === ")") depth = Math.max(0, depth - 1);
    if (ch === "," && depth === 0) { const p = cur.trim(); if (p) out.push(p); cur = ""; continue; }
    cur += ch;
  }
  const last = cur.trim();
  if (last) out.push(last);
  return out;
}

/** 按 `;` 拆声明，但尊重引号与括号（Bloom 的 token 里有含分号的 svg data URI）。 */
function splitDecls(body) {
  const out = [];
  let cur = "";
  let depth = 0;
  let quote = "";
  for (let i = 0; i < body.length; i += 1) {
    const ch = body[i];
    if (quote) {
      cur += ch;
      if (ch === quote && body[i - 1] !== "\\") quote = "";
      continue;
    }
    if (ch === '"' || ch === "'") { quote = ch; cur += ch; continue; }
    if (ch === "(") depth += 1;
    else if (ch === ")") depth = Math.max(0, depth - 1);
    if (ch === ";" && depth === 0) { const d = cur.trim(); if (d) out.push(d); cur = ""; continue; }
    cur += ch;
  }
  const last = cur.trim();
  if (last) out.push(last);
  return out;
}

const TAGS = "h[1-6]|p|ul|ol|li|blockquote|pre|code|table|thead|tbody|tr|th|td|hr|img|a|strong|em|del|mark|u|sup|sub|kbd|figure|figcaption|input";
const CHROME = /#typora|\.typora|#md-|\.cm-|#file|\.file-|\.outline|\.dropdown|\.modal|\.popover|\.tooltip|\.window-|\.on-focus|\.mac-|\.mac-seamless|\.sidebar|#w-|#zoom|#md-|\.search|\.toolbar|\.footer|\.megamenu|\.quick-|\.auto-suggest|\.task-list-item|\.md-toc|\.md-meta-block|\.md-htmlblock|\.md-math/;
const MAP = [
  // Typora 的代码块是 `<pre class="md-fences">` 本体；mdio 是 `<div class="md-codeblock"><pre><code>`。
  // 所以先吃掉 `pre.` / `pre[mdtype=fences]` 这类宿主元素前缀，再统一落成 .md-codeblock（div），否则规则永远不命中。
  [/pre\.md-fences/g, ".md-codeblock"],
  [/pre\[mdtype=["']fences["']\]/g, ".md-codeblock"],
  [/\.md-fences/g, ".md-codeblock"],
  [/\.mermaid/g, ".md-mermaid"],
  [/\.md-alert-caution/g, ".md-admonition--danger"],
  [/\.md-alert-important/g, ".md-admonition--info"],
  [/\.md-alert-note/g, ".md-admonition--note"],
  [/\.md-alert-tip/g, ".md-admonition--tip"],
  [/\.md-alert-warning/g, ".md-admonition--warning"],
  [/\.md-alert\b/g, ".md-admonition"],
];
function mapSelector(sel) {
  let s = sel;
  if (CHROME.test(s)) return null;
  if (/::(before|after)/.test(s) && !/::(selection|marker)/.test(s)) return null;
  for (const [re, to] of MAP) s = s.replace(re, to);
  s = s.replace(/#write\s*>\s*/g, ".mdio-doc > ").replace(/#write/g, ".mdio-doc");
  s = s.replace(new RegExp(`(^|,\\s*)(?=${TAGS}\\b)`, "g"), "$1.mdio-doc ");
  if (!/\.mdio-doc|::(selection|marker)/.test(s)) {
    // 纯类选择器只在映射到我们的扩展类时救回（否则 Typora 的纯类规则会被整批带进来）
    if (!/^\.md-(codeblock|mermaid|admonition|image|math)/.test(s.trim())) return null;
    s = ".mdio-doc " + s;
  }
  return s;
}

/** 把一套主题 CSS（浅或暗）变成 mdio 皮肤所需的四类产物。 */
function normalize(src, { prefix, light, fontFix }) {
  let s = stripComments(src);
  s = s.replace(/@(charset|import|namespace)[^;]*;/g, "");   // 顶层 at-语句会污染第一个选择器
  s = stripBraced(s, /@font-face\s*\{/).rest;
  if (fontFix) s = s.replace(/"MiSans"/g, '-apple-system, "PingFang SC", "Microsoft YaHei", sans-serif');
  s = renameVars(s, prefix);
  const rules = parseRules(s).map((r) => ({ ...r, raw: r.sel }));
  const out = { tokens: [], base: [], canvas: [], content: [], other: [] };
  // 变量可能声明在任意选择器里（不止 :root）⇒ 全局收集，避免「用到但没声明」的悬空 token
  for (const r of rules) {
    const vars = splitDecls(r.body).map((d) => d.trim()).filter((d) => d.startsWith("--"));
    if (vars.length) out.tokens.push(vars.join("; "));
  }
  for (const r of rules) {
    if (r.raw === ":root" || /^:root\b/.test(r.raw)) continue;   // 变量已在上一轮全局收集
    if (r.raw === "body" || r.raw === "html") {
      // 宿主页样式：底色/渐变 → 画布；文字排版 → 基座；布局丢弃
      const decls = splitDecls(r.body);
      const bg = decls.filter((d) => /^(background|background-color)/.test(d));
      const typo = decls.filter((d) => /^(font|color|line-height|letter-spacing|text-rendering|font-feature)/.test(d));
      if (bg.length) out.canvas.push(bg.join("; "));
      if (typo.length) out.base.push({ sel: ".mdio-doc", body: typo.join("; "), typo: true });
      continue;
    }
    // 选择器列表逐个映射：混着 Typora 外壳（#typora-* 等）的逗号列表只丢那几支，不连坐整条规则
    const mappedParts = [...new Set(splitSelectors(r.sel).map((x) => mapSelector(x.trim())).filter(Boolean))];
    if (!mappedParts.length) continue;
    if (mappedParts.length === 1 && mappedParts[0] === ".mdio-doc") {
      const decls = splitDecls(r.body);
      const typo = decls.filter((d) => /^(font|color|line-height|letter-spacing|text-align|text-rendering|font-feature)/.test(d));
      const bg = decls.filter((d) => /^(background|background-color)/.test(d));
      if (typo.length) out.base.push({ sel: ".mdio-doc", body: typo.join("; "), typo: true });
      if (bg.length) out.canvas.push(bg.join("; "));
      continue;
    }
    out.content.push({ sel: mappedParts.join(", "), body: r.body });
  }
  return out;
}

const dedupe = (rules) => {
  const seen = new Set();
  const out = [];
  for (const r of rules) {
    const k = r.sel;
    if (seen.has(k)) { // 合并同选择器声明
      const prev = out.find((x) => x.sel === k);
      prev.body = prev.body + "; " + r.body;
    } else { seen.add(k); out.push({ ...r }); }
  }
  return out;
};

function emit(name, label, opts) {
  const { lightSrc, darkSrc, header, lightFile, darkFile, extra } = opts;
  const L = normalize(lightSrc, { prefix: opts.prefix, fontFix: opts.fontFix });
  const D = darkSrc ? normalize(darkSrc, { prefix: opts.prefix, fontFix: opts.fontFix }) : null;
  const gate = `[data-md-theme="${name}"]`;
  const lContent = dedupe(L.content);
  const lBase = dedupe(L.base.filter((r) => r.typo || r.sel === "body" || r.sel === "html").map((r) => r.typo ? r : { sel: ".mdio-doc", body: splitDecls(r.body).map((d) => d.trim()).filter((d) => /^(font|color|line-height|letter-spacing)/.test(d)).join("; ") })).filter((r) => r.body);
  const lines = [header];

  // 亮色 token
  const dedupeTokens = (list) => {
    const m = new Map();
    for (const d of list.flatMap((b) => splitDecls(b).map((x) => x.trim()).filter(Boolean))) {
      const i = d.indexOf(":");
      m.set(d.slice(0, i).trim(), d);
    }
    return [...m.values()];
  };
  const lTokens = dedupeTokens(L.tokens);
  lines.push(`\n/* 亮色 token（上游 :root） */\n${gate} {\n  ${lTokens.join(";\n  ")};\n}`);
  // 画布
  if (L.canvas.length) lines.push(`\n/* 画布：上游 body 底色/渐变 → 宿主窗格 .br-doc */\n${gate} .br-doc { ${L.canvas.join("; ")}; }\n${gate} .mdio-doc { background-color: transparent; }`);
  // 正文基座
  if (lBase.length) lines.push(`\n/* 正文基座 */\n${gate} .mdio-doc { ${lBase.map((r) => r.body).join("; ")}; }`);
  // 亮色内容规则
  const gateSel = (sel) => splitSelectors(sel).map((x) => `${gate} ${x.trim()}`).join(",\n");
  lines.push(`\n/* 亮色规则（门控） */\n${lContent.map((r) => `${gateSel(r.sel)} { ${r.body} }`).join("\n")}`);

  // 暗色
  if (D) {
    const dTokens = dedupeTokens(D.tokens);
    if (dTokens.length) lines.push(`\n/* 暗色 token：document 面 body.dark / 影子根面 :host(.dark) */\nbody.dark ${gate} {\n  ${dTokens.join(";\n  ")};\n}\n:host(.dark) ${gate} {\n  ${dTokens.join(";\n  ")};\n}`);
    if (D.canvas.length) lines.push(`body.dark ${gate} .br-doc { ${D.canvas.join("; ")}; }\n:host(.dark) ${gate} .br-doc { ${D.canvas.join("; ")}; }`);
    // 暗色内容：同选择器差分；新选择器整条输出
    const lightBySel = new Map(lContent.map((r) => [r.sel, r.body]));
    const changes = [];
    for (const r of dedupe(D.content)) {
      const lb = lightBySel.get(r.sel);
      if (!lb) { changes.push({ sel: r.sel, body: r.body }); continue; }
      const parse = (b) => new Map(splitDecls(b).map((d) => { const i = d.indexOf(":"); return [d.slice(0, i).trim(), d.slice(i + 1).trim()]; }));
      const a = parse(lb), b = parse(r.body);
      const diff = [...b.entries()].filter(([k, v]) => a.get(k) !== v).map(([k, v]) => `${k}: ${v}`);
      if (diff.length) changes.push({ sel: r.sel, body: diff.join("; ") });
    }
    const darkSel = (sel) => splitSelectors(sel).map((x) => `body.dark ${gate} ${x.trim()}`).join(",\n") + ",\n" + splitSelectors(sel).map((x) => `:host(.dark) ${gate} ${x.trim()}`).join(",\n");
    lines.push(`\n/* 暗色差分 */\n${changes.map((r) => `${darkSel(r.sel)} { ${r.body} }`).join("\n")}`);
  }
  if (extra) lines.push("\n" + extra.trim());
  const file = path.join(themes, `${name}.css`);
  fs.writeFileSync(file, lines.join("\n") + "\n");
  console.log(`[port-theme-hostcss] ${name}: light rules=${lContent.length} dark=${D ? D.content.length : 0} tokens=${lTokens.length}/${D ? D.tokens.flatMap((b) => splitDecls(b).filter(Boolean)).length : 0} -> ${path.relative(process.cwd(), file)}`);
}

/* ── macOS 27 ─────────────────────────────────────────────── */
{
  let s = stripComments(fs.readFileSync(path.join(vendor, "macos27.css"), "utf8"));
  const dark = stripBraced(s, /@media\s*\(prefers-color-scheme:\s*dark\)\s*\{/);
  let lightAll = dark.rest;
  // 其余 @media（640px 响应式 / print / contrast / reduced-motion）整块丢弃
  lightAll = stripBraced(lightAll, /@media[^{]*\{/).rest;
  const lightSrc = lightAll;
  const darkSrc = dark.blocks[0] ? dark.blocks[0].replace(/^@media[^{]*\{/, "").replace(/\}\s*$/, "") : "";
  emit("macos27", "macOS 27", {
    prefix: "mac",
    lightSrc,
    darkSrc,
    lightFile: "vendor/macos27.css",
    darkFile: "vendor/macos27.css",
    header: `/* macos27 —— macOS 27 Themes 移植（由 scripts/port-theme-hostcss.mjs 生成，勿手改）。
 *
 * 上游：christoarmani/typora-theme-macos27 的 macos27.css（MIT，© 2026 christoarmani），许可见 vendor/macos27-LICENSE.txt。
 * 变换：@media(prefers-color-scheme:dark) 块 → 容器暗色状态；#write → .mdio-doc；正文基座与画布分离；
 *       丢弃 Typora 编辑器外壳与 print/响应式/prefers-* 媒体块；token 改名 --mac-* 并挂在门控元素上。
 * 上游宣称：全部文本对 ≥4.5:1、控件/焦点 ≥3:1（WCAG 2.2 AA）；暗色跟随系统。 */`,
    extra: `/* 补上游漏声明：macOS 27 的 samp 规则用了 var(--m-font-mono) 但从未声明 ⇒ 这里给系统等宽栈。 */
[data-md-theme="macos27"] { --mac-m-font-mono: "SF Mono", Menlo, Monaco, "JetBrains Mono", ui-monospace, monospace; }
/* ── 外壳皮肤（整壳替换）：接管左栏文档树 / 面包屑 / 切换器的视觉；结构尺寸与交互仍归 app。 */
[data-md-theme="macos27"].br-shell { background: var(--mac-m-bg); color: var(--mac-m-text); }
[data-md-theme="macos27"] .br-side { background: var(--mac-m-surface); border-right-color: var(--mac-m-border); }
[data-md-theme="macos27"] .br-eyebrow,
[data-md-theme="macos27"] .br-count,
[data-md-theme="macos27"] .br-folder-count,
[data-md-theme="macos27"] .br-folder,
[data-md-theme="macos27"] .br-item,
[data-md-theme="macos27"] .br-side-empty,
[data-md-theme="macos27"] .br-crumb-label,
[data-md-theme="macos27"] .br-crumb-sep,
[data-md-theme="macos27"] .br-theme { color: var(--mac-m-text-2); }
[data-md-theme="macos27"] .br-folder-name { color: var(--mac-m-text); }
[data-md-theme="macos27"] .br-caret { border-left-color: var(--mac-m-text-2); }
[data-md-theme="macos27"] .br-folder:hover,
[data-md-theme="macos27"] .br-item:hover { background: var(--mac-m-hover-bg); color: var(--mac-m-text); }
[data-md-theme="macos27"] .br-item.is-active { background: var(--mac-m-active-bg); color: var(--mac-m-on-accent); }
[data-md-theme="macos27"] .br-item.is-active::before { background: var(--mac-m-accent-fill); }
[data-md-theme="macos27"] .br-file-glyph { border-color: var(--mac-m-control-border); }
[data-md-theme="macos27"] .br-file-glyph::after { border-color: var(--mac-m-control-border); background: var(--mac-m-surface); }
[data-md-theme="macos27"] .br-item:hover .br-file-glyph::after { background: var(--mac-m-hover-bg); }
[data-md-theme="macos27"] .br-item.is-active .br-file-glyph,
[data-md-theme="macos27"] .br-item.is-active .br-file-glyph::after { border-color: var(--mac-m-accent-fill); }
[data-md-theme="macos27"] .br-item.is-active .br-file-glyph::after { background: var(--mac-m-active-bg); }
[data-md-theme="macos27"] .br-crumb { background: var(--mac-m-bg); border-bottom-color: var(--mac-m-border); }
[data-md-theme="macos27"] .br-crumb-path { color: var(--mac-m-link); }
[data-md-theme="macos27"] .br-theme select { color: var(--mac-m-select-text); background: var(--mac-m-select-bg); border-color: var(--mac-m-border); }

/* 节包裹层：floor 的 .mdio-std 是 15px/1.65，会把 macOS 27 的正文口径（16px/1.65/-0.005em）压回去。 */
[data-md-theme="macos27"] .mdio-doc .mdio-std { font-size: 16px; line-height: 1.65; letter-spacing: -0.005em; }

/* ── mdio 扩展元素（macOS 27 皮肤：沿用本皮肤的「系统表面 + 10px 圆角 + 强调色」口径，不另起风格）── */
[data-md-theme="macos27"] .mdio-doc .md-bullet-key {
  color: var(--mac-m-text);
  background: var(--mac-m-surface-2);
  border: 1px solid var(--mac-m-border);
  border-radius: var(--mac-m-radius-s);
  padding: 0 0.4em;
  font-weight: 600;
}
[data-md-theme="macos27"] .mdio-doc .md-claim-badge {
  color: var(--mac-m-on-accent);
  background: var(--mac-m-accent-fill);
  border-radius: var(--mac-m-radius-s);
  padding: 1px 8px;
  font-size: 0.78em;
  font-weight: 600;
}
[data-md-theme="macos27"] .mdio-doc .md-col-grip::before { background-color: var(--mac-m-border-strong); }
[data-md-theme="macos27"] .mdio-doc th:hover .md-col-grip::before { background-color: var(--mac-m-accent-fill); }
[data-md-theme="macos27"] .mdio-doc .md-math { color: var(--mac-m-text); }
[data-md-theme="macos27"] .mdio-doc .md-mermaid {
  background: var(--mac-m-surface);
  border: 1px solid var(--mac-m-border);
  border-radius: var(--mac-m-radius-m);
  padding: 1rem;
}
[data-md-theme="macos27"] .mdio-doc .md-html { color: var(--mac-m-text); }
[data-md-theme="macos27"] .mdio-doc .md-math--raw { color: var(--mac-m-text-2); font-family: var(--mac-m-font-mono); }
[data-md-theme="macos27"] .mdio-doc .md-admonition-title {
  color: var(--mac-m-text-2);
  font-size: 0.82em;
  font-weight: 700;
  letter-spacing: 0.02em;
  text-transform: uppercase;
}
[data-md-theme="macos27"] .mdio-doc .md-image-zoom { border: 1px solid var(--mac-m-border); border-radius: var(--mac-m-radius-m); }
[data-md-theme="macos27"] .mdio-doc .md-html-raw { color: var(--mac-m-text); }
[data-md-theme="macos27"] .mdio-doc .md-image { border-radius: var(--mac-m-radius-m); overflow: hidden; }
[data-md-theme="macos27"] .mdio-doc .md-image-zoom { border: 1px solid var(--mac-m-border); border-radius: var(--mac-m-radius-m); }
/* 外壳残余件：选区 / 焦点环 / 滚动条 / 加载环（滚动条伪元素里 var() 不解析 ⇒ 字面色）。 */
[data-md-theme="macos27"] .br-shell ::selection { background: var(--mac-m-select-bg); color: var(--mac-m-text); }
[data-md-theme="macos27"] .br-item:focus-visible,
[data-md-theme="macos27"] .br-folder:focus-visible,
[data-md-theme="macos27"] .br-theme select:focus-visible { outline-color: var(--mac-m-accent-fill); }
[data-md-theme="macos27"] .br-spin-ring { border-top-color: var(--mac-m-accent-fill); }
[data-md-theme="macos27"] .br-tree::-webkit-scrollbar-thumb,
[data-md-theme="macos27"] .br-doc::-webkit-scrollbar-thumb { background-color: #e1e1e6; }
body.dark [data-md-theme="macos27"] .br-tree::-webkit-scrollbar-thumb,
body.dark [data-md-theme="macos27"] .br-doc::-webkit-scrollbar-thumb,
:host(.dark) [data-md-theme="macos27"] .br-tree::-webkit-scrollbar-thumb,
:host(.dark) [data-md-theme="macos27"] .br-doc::-webkit-scrollbar-thumb { background-color: #424245; }
`,
  });
}

/* ── Bloom · mist ─────────────────────────────────────────── */
{
  const lightSrc = fs.readFileSync(path.join(vendor, "bloom-mist.css"), "utf8");
  const darkSrc = fs.readFileSync(path.join(vendor, "bloom-mist-dark.css"), "utf8");
  emit("bloom", "Bloom · mist", {
    prefix: "bl",
    fontFix: true,
    lightSrc: stripBraced(stripComments(lightSrc), /@media[^{]*\{/).rest,
    darkSrc: stripBraced(stripComments(darkSrc), /@media[^{]*\{/).rest,
    lightFile: "vendor/bloom-mist.css",
    darkFile: "vendor/bloom-mist-dark.css",
    header: `/* bloom —— Bloom · mist（Typora 主题）移植（由 scripts/port-theme-hostcss.mjs 生成，勿手改）。
 *
 * 上游：webkubor/typora-Bloom-theme 的 built/bloom-mist.css + bloom-mist-dark.css（MIT，© 2025 webkubor），
 *       许可见 vendor/bloom-LICENSE.txt。Bloom 共 12 组配色 × 明暗（莫兰迪设计语言，源码用 OKLCH）。
 * 变换：浅/暗两份 built 主题 → 浅色全量 + 暗色声明差分；#write → .mdio-doc；token 改名 --bl-* 挂门控元素；
 *       画布（上游 body 的柔光渐变）移到 .br-doc；丢 Typora 外壳与 print/响应式媒体块。
 * 字体：上游随包 MiSans（字体许可未随仓附）**不搬**，字体栈替换为系统 CJK 栈（PingFang/微软雅黑/Noto）。
 * 已知裁剪：Bloom 的 memo/卡片等 Typora 专用内容类没有对应 DOM，未移植。 */`,
    extra: `/* 代码块顶部留白修正：Bloom 上游给 .md-fences 预留 3.2em 顶部空间画「Mac 窗口三圆点」，
 * 那是 ::before 装饰；mdio 引擎不产出该伪元素 ⇒ 顶部会出现一条无意义空白，压回与左右一致的 1.4em。 */
[data-md-theme="bloom"] .mdio-doc .md-codeblock { padding-top: 1.4em !important; }

/* 补上游漏声明：Bloom 暗色 code 规则用了 var(--code-inline-text) 但浅/暗两份都没声明该变量。
 * 取值按 Bloom 自己的墨色阶补：浅色取正文墨色一档更深、暗色取暖白一档更亮。 */
[data-md-theme="bloom"] { --bl-code-inline-text: #2f2b26; }
body.dark [data-md-theme="bloom"] { --bl-code-inline-text: #ece6dc; }
:host(.dark) [data-md-theme="bloom"] { --bl-code-inline-text: #ece6dc; }
/* ── 外壳皮肤（整壳替换）：Bloom 上游本就有侧栏/文件树规则（--side-bar-bg-color 等），这里映射到 app 外壳。 */
[data-md-theme="bloom"].br-shell { background: var(--bl-bg); color: var(--bl-text); }
[data-md-theme="bloom"] .br-side { background: var(--bl-side-bar-bg-color); border-right-color: var(--bl-border); }
[data-md-theme="bloom"] .br-eyebrow,
[data-md-theme="bloom"] .br-count,
[data-md-theme="bloom"] .br-folder-count,
[data-md-theme="bloom"] .br-folder,
[data-md-theme="bloom"] .br-item,
[data-md-theme="bloom"] .br-side-empty,
[data-md-theme="bloom"] .br-crumb-label,
[data-md-theme="bloom"] .br-crumb-sep,
[data-md-theme="bloom"] .br-theme { color: var(--bl-muted); }
[data-md-theme="bloom"] .br-folder-name { color: var(--bl-text); }
[data-md-theme="bloom"] .br-caret { border-left-color: var(--bl-muted); }
[data-md-theme="bloom"] .br-folder:hover,
[data-md-theme="bloom"] .br-item:hover { background: var(--bl-item-hover-bg-color); color: var(--bl-item-hover-text-color); }
[data-md-theme="bloom"] .br-item.is-active { background: var(--bl-accent-soft); color: var(--bl-accent); }
[data-md-theme="bloom"] .br-item.is-active::before { background: var(--bl-accent); }
[data-md-theme="bloom"] .br-file-glyph { border-color: var(--bl-muted); }
[data-md-theme="bloom"] .br-file-glyph::after { border-color: var(--bl-muted); background: var(--bl-side-bar-bg-color); }
[data-md-theme="bloom"] .br-item:hover .br-file-glyph::after { background: var(--bl-item-hover-bg-color); }
[data-md-theme="bloom"] .br-item.is-active .br-file-glyph,
[data-md-theme="bloom"] .br-item.is-active .br-file-glyph::after { border-color: var(--bl-accent); }
[data-md-theme="bloom"] .br-item.is-active .br-file-glyph::after { background: var(--bl-accent-soft); }
[data-md-theme="bloom"] .br-crumb { background: var(--bl-bg); border-bottom-color: var(--bl-border); }
[data-md-theme="bloom"] .br-crumb-path { color: var(--bl-accent); }
[data-md-theme="bloom"] .br-theme select { color: var(--bl-text); background: var(--bl-surface); border-color: var(--bl-border); }

/* 节包裹层：Bloom 正文口径 16px/1.8（floor 的 15px/1.65 会压回去）。 */
[data-md-theme="bloom"] .mdio-doc .mdio-std { font-size: 16px; line-height: 1.8; letter-spacing: normal; }

/* ── mdio 扩展元素（Bloom 皮肤：沿用「大圆角 + accent 半透明 + 无重边框」口径）── */
[data-md-theme="bloom"] .mdio-doc .md-bullet-key {
  color: var(--bl-accent);
  background: rgba(var(--bl-accent-rgb), 0.14);
  border: 1px solid rgba(var(--bl-accent-rgb), 0.28);
  border-radius: 8px;
  padding: 0 0.42em;
  font-weight: 700;
}
[data-md-theme="bloom"] .mdio-doc .md-claim-badge {
  color: var(--bl-accent);
  background: rgba(var(--bl-accent-rgb), 0.16);
  border-radius: 999px;
  padding: 1px 10px;
  font-size: 0.78em;
  font-weight: 700;
}
[data-md-theme="bloom"] .mdio-doc .md-col-grip::before { background-color: rgba(var(--bl-accent-rgb), 0.35); }
[data-md-theme="bloom"] .mdio-doc th:hover .md-col-grip::before { background-color: var(--bl-accent); }
[data-md-theme="bloom"] .mdio-doc .md-math { color: var(--bl-text); }
[data-md-theme="bloom"] .mdio-doc .md-math--raw { color: var(--bl-muted); font-family: var(--bl-font-mono); }
[data-md-theme="bloom"] .mdio-doc .md-html,
[data-md-theme="bloom"] .mdio-doc .md-html-raw { color: var(--bl-text); }
[data-md-theme="bloom"] .mdio-doc .md-image { border-radius: 14px; overflow: hidden; }
[data-md-theme="bloom"] .mdio-doc .md-image-zoom { border-radius: 14px; }
/* 首个块顶距：浅色由 floor 的 .mdio-std:first-child 规则压平，暗色 h1 规则特异性更高会顶开 ⇒ 明暗都压平。 */
[data-md-theme="bloom"] .mdio-doc > .mdio-std:first-child > :first-child,
body.dark [data-md-theme="bloom"] .mdio-doc > .mdio-std:first-child > :first-child,
:host(.dark) [data-md-theme="bloom"] .mdio-doc > .mdio-std:first-child > :first-child { margin-top: 0 !important; }
/* 暗色代码块：上游给 mac 窗口灯预留 3.3em 顶距，mdio 没有窗口灯 ⇒ 压到 1.4em；
 * 暗色规则带 :host(.dark) 前缀、特异性更高，所以这条必须同样带暗色前缀并放在其后。 */
body.dark [data-md-theme="bloom"] .mdio-doc .md-codeblock,
:host(.dark) [data-md-theme="bloom"] .mdio-doc .md-codeblock { padding-top: 1.4em !important; }
/* 外壳残余件：选区 / 焦点环 / 滚动条 / 加载环（滚动条伪元素里 var() 不解析 ⇒ 字面色）。 */
[data-md-theme="bloom"] .br-shell ::selection { background: var(--bl-selection); }
[data-md-theme="bloom"] .br-item:focus-visible,
[data-md-theme="bloom"] .br-folder:focus-visible,
[data-md-theme="bloom"] .br-theme select:focus-visible { outline-color: var(--bl-accent); }
[data-md-theme="bloom"] .br-spin-ring { border-top-color: var(--bl-accent); }
[data-md-theme="bloom"] .br-tree::-webkit-scrollbar-thumb,
[data-md-theme="bloom"] .br-doc::-webkit-scrollbar-thumb { background-color: rgba(25, 35, 42, 0.16); }
body.dark [data-md-theme="bloom"] .br-tree::-webkit-scrollbar-thumb,
body.dark [data-md-theme="bloom"] .br-doc::-webkit-scrollbar-thumb,
:host(.dark) [data-md-theme="bloom"] .br-tree::-webkit-scrollbar-thumb,
:host(.dark) [data-md-theme="bloom"] .br-doc::-webkit-scrollbar-thumb { background-color: rgba(236, 243, 248, 0.2); }
`,
  });
}
