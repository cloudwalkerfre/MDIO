#!/usr/bin/env node
// port-theme-mdmdt —— 把 vendored 的 Typora 主题 Mdmdt 转成我们的主题包。
//
// 上游：`src/themes/vendor/mdmdt-light.css` / `mdmdt-dark.css`（Apache-2.0，© cayxc，许可证随包在 vendor/Mdmdt-LICENSE.txt）。
// 变换（白名单 + 结构化重映射，机械可复核）：
//   1. 丢掉全部 @media（都是 Typora 响应式/打印：#write 宽度、目录、打印分页）与编辑器外壳规则。
//   2. `#write` 作用域 → `.mdio-doc`；裸内容标签 → `.mdio-doc <tag>`；Typora 内容类 → 我们的类：
//      `.md-alert*` → `.md-admonition*`、`.md-fences` → `.md-codeblock`。
//   3. 调色板：上游 `:root` 的 `--*` 变量统一改名 `--mdt-*`（避免与宿主/其他主题的 --bg-color 之类撞名），
//      亮色 token 挂 `[data-md-theme="mdmdt"]`，暗色挂 `body.dark` 与 `:host(.dark)` 两份。
//   4. 暗色差分：同一选择器在 dark 文件里**与 light 不同**的声明才输出（颜色为主），避免整份 68 KB 复制。
//   5. 补一条显式等宽字体：上游 `--monospace` 声明了 JetBrains Mono 但 **0 次使用**（实测），
//      代码块实际没有 mono 栈 ⇒ 这里补 `font-family: var(--mdt-mono)`（自托管 JBMono）。
// 用法：node scripts/port-theme-mdmdt.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const themes = path.join(here, "..", "src", "themes");
const outPath = path.join(themes, "mdmdt.css");

/** 去掉全部顶层 @media 块（括号配平）。 */
function stripMedia(css) {
  let out = "";
  let i = 0;
  while (i < css.length) {
    const m = /@media[^{]*\{/.exec(css.slice(i));
    if (!m) { out += css.slice(i); break; }
    out += css.slice(i, i + m.index);
    let j = i + m.index + m[0].length;   // m.end 不存在（正则 match 对象只有 index/input）
    let depth = 1;
    while (j < css.length && depth > 0) {
      if (css[j] === "{") depth += 1;
      else if (css[j] === "}") depth -= 1;
      j += 1;
    }
    i = j;
  }
  return out;
}
const parse = (file) => {
  const s = stripMedia(fs.readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, ""));
  const rules = [];
  let m;
  const re = /([^{}]+)\{([^{}]*)\}/g;
  while ((m = re.exec(s))) rules.push({ sel: m[1].trim().replace(/\s+/g, " "), body: m[2].trim().replace(/\s+/g, " ") });
  return rules;
};

const light = parse(path.join(themes, "vendor", "mdmdt-light.css"));
const dark = parse(path.join(themes, "vendor", "mdmdt-dark.css"));

// ── 调色板（:root 变量，改名 --mdt-*）──
const rootVars = (rules) => rules.filter((r) => r.sel === ":root").flatMap((r) => r.body.split(";").map((s) => s.trim()).filter(Boolean));
const lightVars = rootVars(light);
const darkVars = rootVars(dark);
const toToken = (decl) => {
  const i = decl.indexOf(":");
  if (i < 0) return null;
  const name = decl.slice(0, i).trim();
  const val = decl.slice(i + 1).trim();
  if (!name.startsWith("--")) return null;
  const renamed = name.replace(/^--/, "--mdt-");
  return `${renamed}: ${val.replace(/var\(--([\w-]+)\)/g, "var(--mdt-$1)")}`;
};

// ── 规则映射 ──
const DROP = [/::before/, /::after/, /:has\(/, /\.md-toc/, /\.md-htmlblock/, /\.md-math/, /\.md-inline-math/, /script/, /span\.md-image/, /\.md-image/, /\.task-list-item/, /#write > h[1-6]:hover/, /\.md-meta-block/, /\.reversefootnote/, /\.footnotes/, /\.md-def-/, /\.md-link/, /\.md-url/, /\.md-comment/, /\.md-grid/, /\.md-search/, /sidebar/i, /file-tree/i, /(^|[ ,])body > header/, /\.window/, /\.form-control/, /\.input-group/, /\.dropdown/, /\.ty-tooltip/, /\.ty-spell/, /\.ty-file/, /#md-grid/, /#dict-install/, /#recent-file/, /#sidebar/, /#ty-/, /\.nav-group/, /\.search-input/, /\.tb43e/, /\.label-/, /\.export-detail/, /\.fa-/, /\.footnotes/, /^html$/, /^html, html \*$/, /^\*$/];
const CLASS_MAP = [
  [/\.md-alert-caution/g, ".md-admonition--danger"],
  // important 与 warning 都映射成 --warning 会撞车：裸选择器相同 ⇒ 去重逻辑把后者（真正的 warning，color-5）整条吞掉，
  // 且 floor 的 --info 变体永远拿不到皮肤色。改为 important → --info（上游 color-4 / 紫一档），warning 保住 color-5。
  [/\.md-alert-important/g, ".md-admonition--info"],
  [/\.md-alert-note/g, ".md-admonition--note"],
  [/\.md-alert-tip/g, ".md-admonition--tip"],
  [/\.md-alert-warning/g, ".md-admonition--warning"],
  [/\.md-alert-text-[\w-]+/g, ".md-admonition-title"],
  [/\.md-alert/g, ".md-admonition"],
  [/\.md-fences/g, ".md-codeblock"],
];
const TAGS = "h[1-6]|p|ul|ol|li|blockquote|code|pre|table|thead|tbody|tr|th|td|hr|img|a|strong|em|del|mark|u|sup|sub|kbd|figure|figcaption|input";
const KEEP = new RegExp(`\\.mdio-doc|\\b(${TAGS})\\b|::selection|\\.md-codeblock|\\.md-admonition`);

function mapSelector(sel) {
  let s = sel;
  for (const [re, to] of CLASS_MAP) s = s.replace(re, to);
  if (DROP.some((re) => re.test(s))) return null;
  // Typora 作用域 → .mdio-doc
  s = s.replace(/\.(?:typora-export|unibody-window|mac-os|os-windows)\s+/g, "");
  s = s.replace(/#write\s*>\s*/g, ".mdio-doc > ");
  s = s.replace(/#write/g, ".mdio-doc");
  // 裸内容标签 → .mdio-doc 内
  s = s.replace(new RegExp(`(^|,\\s*)(?=${TAGS}\\b)`, "g"), "$1.mdio-doc ");
  // 纯类选择器只在**映射到我们类**时救回来（.md-admonition* / .md-codeblock）：
  // 上游 `blockquote, .md-alert {}` 这类逗号列表里带类的那一半必须保留，否则
  // ⇒ admonition 只剩从 `#write .md-alert a` 派生的零散规则，base 的背景/圆角/边框全缺。
  // 注意不能无条件补 `.mdio-doc `：那会把 Typora 的纯类规则（.md-tooltip-remove 等）也带进来。
  if (!/\.mdio-doc|::selection/.test(s)) {
    if (!/^\s*\.md-(admonition|codeblock)/.test(s)) return null;
    s = ".mdio-doc " + s;
  }
  if (!KEEP.test(s)) return null;
  return s;
}
const mapRule = (r) => {
  const parts = r.sel.split(",").map((x) => mapSelector(x.trim())).filter(Boolean);
  if (!parts.length) return null;
  const body = r.body.replace(/var\(--([\w-]+)\)/g, "var(--mdt-$1)");
  return { sel: parts.map((s) => `[data-md-theme="mdmdt"] ${s}`).join(",\n"), body, bare: parts.join(", ") };
};

const lightMapped = light.map(mapRule).filter(Boolean);
const seen = new Set();
const lightOut = [];
for (const r of lightMapped) { const k = r.bare; if (seen.has(k)) continue; seen.add(k); lightOut.push(r); }

// body/html 排版 → .mdio-doc 基线
const bodyRule = light.find((r) => r.sel === "body");
const baseDecls = (bodyRule ? bodyRule.body.split(";").map((s) => s.trim()).filter((d) => /^(font-size|line-height|font-family|letter-spacing|color|text-align)/.test(d)) : []).map((d) => d.replace(/var\(--([\w-]+)\)/g, "var(--mdt-$1)"));
const base = `[data-md-theme="mdmdt"] .mdio-doc {\n  ${baseDecls.join(";\n  ")};\n  font-family: var(--mdt-sans);\n  background: transparent;\n}`;

// ── 暗色差分：同一映射选择器上与 light 不同的声明 ──
const parseDecls = (body) => {
  const map = new Map();
  for (const d of body.split(";")) {
    const i = d.indexOf(":");
    if (i < 0) continue;
    map.set(d.slice(0, i).trim(), d.slice(i + 1).trim());
  }
  return map;
};
const lightBySel = new Map();
for (const r of light) for (const k of r.sel.split(",").map((s) => s.trim())) lightBySel.set(k, r.body.replace(/\s+/g, " "));
const darkOut = [];
const darkSeen = new Set();
for (const r of dark) {
  const mapped = mapRule(r);
  if (!mapped) continue;
  const lightBody = lightBySel.get(r.sel.split(",")[0].trim());
  const a = lightBody ? parseDecls(lightBody) : new Map();
  const b = parseDecls(r.body.replace(/var\(--([\w-]+)\)/g, "var(--mdt-$1)"));
  const changed = [...b.entries()].filter(([k, v]) => a.get(k) !== v).map(([k, v]) => `${k}: ${v}`);
  if (!changed.length) continue;
  const key = mapped.bare + "|" + changed.join(";");
  if (darkSeen.has(key)) continue;
  darkSeen.add(key);
  darkOut.push({ sel: mapped.bare, body: changed.join("; ") });
}

const darkBlock = darkOut.map((r) => `body.dark [data-md-theme="mdmdt"] ${r.sel} { ${r.body} }
:host(.dark) [data-md-theme="mdmdt"] ${r.sel} { ${r.body} }`).join("\n");

const lightTokens = lightVars.map(toToken).filter(Boolean);
const darkTokens = darkVars.map(toToken).filter(Boolean);

const header = `/* mdmdt —— Typora 主题 Mdmdt 的皮肤移植（由 scripts/port-theme-mdmdt.mjs 生成，勿手改）。
 *
 * 上游：cayxc/Mdmdt 的 mdmdt-light.css / mdmdt-dark.css（Apache-2.0，© cayxc，许可证见 themes/vendor/Mdmdt-LICENSE.txt）。
 * 变换：剥离 Typora 编辑器外壳与全部 @media；#write → .mdio-doc；.md-alert* → .md-admonition*；.md-fences → .md-codeblock；
 *       调色板 --* → --mdt-*；暗色只输出与亮色不同的声明。
 * 补丁：上游 --monospace（JetBrains Mono 栈）声明了但 0 次使用 ⇒ 这里显式给代码块补 var(--mdt-mono)（自托管 JBMono）。
 */

/* 亮色 token */
[data-md-theme="mdmdt"] {
  ${lightTokens.join(";\n  ")};
  --mdt-sans: "PingFang SC", "Microsoft YaHei", "Noto Sans CJK SC", Arial, "Helvetica Neue", Helvetica, sans-serif;
  --mdt-mono: "JBMono", "JetBrains Mono", "Source Code Pro", Consolas, Menlo, ui-monospace, monospace;
}

/* 暗色 token：document 面与影子根面双写 */
body.dark [data-md-theme="mdmdt"] {
  ${darkTokens.join(";\n  ")};
}
:host(.dark) [data-md-theme="mdmdt"] {
  ${darkTokens.join(";\n  ")};
}

${base}

[data-md-theme="mdmdt"] .mdio-doc,
[data-md-theme="mdmdt"] .mdio-doc p,
[data-md-theme="mdmdt"] .mdio-doc li,
[data-md-theme="mdmdt"] .mdio-doc td,
[data-md-theme="mdmdt"] .mdio-doc blockquote {
  color: var(--mdt-text-color);
  letter-spacing: 0.6px;
}

/* 代码块补 mono（上游缺口） */
[data-md-theme="mdmdt"] .mdio-doc code,
[data-md-theme="mdmdt"] .mdio-doc pre,
[data-md-theme="mdmdt"] .mdio-doc .md-codeblock {
  font-family: var(--mdt-mono);
}

/* ── 外壳皮肤（整壳替换）：主题接管左栏文档树 / 面包屑 / 切换器的视觉。
 * 依据 Mdmdt 自己的 Typora file-library 语言：列表项之间是发丝分隔线、hover 是「次级底 + 蓝字」、
 * active 是「蓝框 + 无底色」；目录/大纲文字用正文色，计数降权。结构尺寸与交互仍归 app。
 * 注意：[data-md-theme].br-shell 必须用复合选择器 —— 属性就写在 .br-shell 自身，后代选择器不成立。 */
[data-md-theme="mdmdt"].br-shell { background: var(--mdt-bg-color); color: var(--mdt-text-color); }
[data-md-theme="mdmdt"] .br-side { background: var(--mdt-bg-color2); border-right-color: var(--mdt-border-color); }
[data-md-theme="mdmdt"] .br-eyebrow,
[data-md-theme="mdmdt"] .br-crumb-label,
[data-md-theme="mdmdt"] .br-crumb-sep,
[data-md-theme="mdmdt"] .br-theme { color: var(--mdt-text-color); opacity: 0.6; }
[data-md-theme="mdmdt"] .br-count,
[data-md-theme="mdmdt"] .br-folder-count { color: var(--mdt-text-color); opacity: 0.55; }
[data-md-theme="mdmdt"] .br-folder { color: var(--mdt-text-color); opacity: 0.78; }
[data-md-theme="mdmdt"] .br-folder-name { color: var(--mdt-text-color); }
[data-md-theme="mdmdt"] .br-caret { border-left-color: var(--mdt-text-color); opacity: 0.6; }
[data-md-theme="mdmdt"] .br-item {
  color: var(--mdt-text-color);
  border-bottom: 1px solid var(--mdt-border-color);
  border-radius: 0;
}
[data-md-theme="mdmdt"] .br-folder:hover,
[data-md-theme="mdmdt"] .br-item:hover {
  background: var(--mdt-bg-color);
  color: var(--mdt-color-1);
  border-radius: 5px;
}
[data-md-theme="mdmdt"] .br-item.is-active {
  background: transparent;
  border: 1px solid var(--mdt-color-1);
  border-radius: 5px;
  color: var(--mdt-text-color);
}
[data-md-theme="mdmdt"] .br-item.is-active::before { display: none; }
[data-md-theme="mdmdt"] .br-file-glyph { border-color: var(--mdt-text-color); opacity: 0.7; }
[data-md-theme="mdmdt"] .br-file-glyph::after { border-color: var(--mdt-text-color); background: var(--mdt-bg-color2); }
[data-md-theme="mdmdt"] .br-item:hover .br-file-glyph::after { background: var(--mdt-bg-color); }
[data-md-theme="mdmdt"] .br-item.is-active .br-file-glyph,
[data-md-theme="mdmdt"] .br-item.is-active .br-file-glyph::after { border-color: var(--mdt-color-1); }
[data-md-theme="mdmdt"] .br-item.is-active .br-file-glyph::after { background: transparent; }
[data-md-theme="mdmdt"] .br-crumb { background: var(--mdt-bg-color); border-bottom-color: var(--mdt-border-color); }
[data-md-theme="mdmdt"] .br-crumb-path { color: var(--mdt-color-1); }
[data-md-theme="mdmdt"] .br-theme select { color: var(--mdt-text-color); background: var(--mdt-bg-color2); border-color: var(--mdt-border-color); }
[data-md-theme="mdmdt"] .br-side-empty { color: var(--mdt-text-color); opacity: 0.6; }

/* 暗色下 code 的暗色规则（:host(.dark) 前缀）特异性高于围栏内层中和 ⇒ 再中和一次，
 * 否则代码块每一行又会被套上「行内代码」的蓝底芯片。 */
body.dark [data-md-theme="mdmdt"] .mdio-doc .md-codeblock code,
:host(.dark) [data-md-theme="mdmdt"] .mdio-doc .md-codeblock code { background: transparent; padding: 0; border-radius: 0; }
/* 外壳残余件：焦点环 / 滚动条 / 加载环（滚动条伪元素里 var() 不解析 ⇒ 字面色）。 */
[data-md-theme="mdmdt"] .br-item:focus-visible,
[data-md-theme="mdmdt"] .br-folder:focus-visible,
[data-md-theme="mdmdt"] .br-theme select:focus-visible { outline-color: var(--mdt-color-1); }
[data-md-theme="mdmdt"] .br-spin-ring { border-top-color: var(--mdt-color-1); }
[data-md-theme="mdmdt"] .br-tree::-webkit-scrollbar-thumb,
[data-md-theme="mdmdt"] .br-doc::-webkit-scrollbar-thumb { background-color: #d2d2d2; }
body.dark [data-md-theme="mdmdt"] .br-tree::-webkit-scrollbar-thumb,
body.dark [data-md-theme="mdmdt"] .br-doc::-webkit-scrollbar-thumb,
:host(.dark) [data-md-theme="mdmdt"] .br-tree::-webkit-scrollbar-thumb,
:host(.dark) [data-md-theme="mdmdt"] .br-doc::-webkit-scrollbar-thumb { background-color: #5a646c; }

/* 画布：上游 body 的底色（浅 rgb(250,250,252) / 深 #1b1b1f）→ 宿主窗格 .br-doc；
 * 不涂 .mdio-doc 的原因与其它皮肤一致：那是随内容高度截断的色块，会让 app 底色在正文尾部露出来。 */
[data-md-theme="mdmdt"] .br-doc { background: var(--mdt-bg-color); }
[data-md-theme="mdmdt"] .mdio-doc { background: transparent; }

/* 兜底修正：std 的节包裹层 .mdio-std 在 floor 里是 15px，会打断上游的 em 级联 ⇒ 补回 16px/1.6/字距。 */
[data-md-theme="mdmdt"] .mdio-doc .mdio-std { font-size: 16px; line-height: 1.6; letter-spacing: 0.6px; }

/* ── Typora base floor（Mdmdt 是 overlay 主题：代码块内边距、首个块的顶距、内层 <code> 的裸样式
 *    都由 Typora 内置 base CSS 提供，vendor CSS 里没有；Typora 的围栏是 pre.md-fences > CodeMirror，
 *    mdio 是 div.md-codeblock > pre > code —— 直接套 overlay 会出现「文字贴边 + 每行都套行内代码芯片」。
 *    这里补上等价的最小 base，并把误套到内层 code 上的芯片样式中和掉。 */
[data-md-theme="mdmdt"] .mdio-doc .md-codeblock {
  margin: 16px 0 0;
  padding: 16px;
  background: var(--mdt-bg-color2);
  border-radius: 8px;
  overflow: auto;
}
[data-md-theme="mdmdt"] .mdio-doc .md-codeblock > pre,
[data-md-theme="mdmdt"] .mdio-doc .md-codeblock pre {
  margin: 0;
  padding: 0;
  background: transparent !important;
  border-radius: 0;
  overflow: visible;
}
[data-md-theme="mdmdt"] .mdio-doc .md-codeblock code {
  background: transparent;
  padding: 0;
  border-radius: 0;
  color: inherit;
  font-size: inherit;
  box-decoration-break: slice;
  -webkit-box-decoration-break: slice;
}
[data-md-theme="mdmdt"] .mdio-doc > .mdio-std:first-child > :first-child { margin-top: 0 !important; }
[data-md-theme="mdmdt"] .mdio-doc img { max-width: 100%; }


/* ── mdio 扩展元素（上游无对应概念：按 Mdmdt 自己的组件语言推导，不另起风格）──────────
 * 推导依据全部取自本皮肤已移植的上游值：
 *   · 圆角 8px + 内边距 16px —— 上游 blockquote / pre / figure 的统一口径；
 *   · 次级底 --mdt-bg-color2 —— 上游 pre / CodeMirror-gutters 用；
 *   · 分隔线 --mdt-border-color、次字色 --mdt-text-grey、代码字色 --mdt-text-code；
 *   · 强调色 --mdt-color-1，及其 12% 透明度底 --mdt-color-1-0-b（上游 md-alert-note 用同一对）；
 *   · 语气色 --mdt-color-3/4/5（上游 tip/important/warning 用）；
 *   · 正文 0.6px 字距、代码 JBMono 均沿用本皮肤已有口径。
 * 约束：只接管颜色/底色/边框/圆角/字重，不动结构（grip 定位、md-image min-height、mermaid flex 居中仍归 floor）。
 * Admonition 不在此列：上游 .md-alert* 映射已经给了底色/4px 左边/圆角/五档语气色。 */
[data-md-theme="mdmdt"] .mdio-doc .md-bullet-key {
  color: var(--mdt-text-color);
  background-color: var(--mdt-bg-color2);
  border-radius: 4px;
  padding: 0 6px;
  font-weight: 600;
}
[data-md-theme="mdmdt"] .mdio-doc .md-claim-badge {
  color: var(--mdt-color-1);
  background-color: var(--mdt-color-1-0-b);
  border-radius: 6px;
  padding: 2px 8px;
  font-size: 12px;
}
[data-md-theme="mdmdt"] .mdio-doc .md-col-grip::before { background-color: var(--mdt-border-color); }
[data-md-theme="mdmdt"] .mdio-doc th:hover .md-col-grip::before { background-color: var(--mdt-color-1); }
[data-md-theme="mdmdt"] .mdio-doc .md-math { color: var(--mdt-text-color); }
[data-md-theme="mdmdt"] .mdio-doc .md-math--raw { color: var(--mdt-text-grey); font-family: var(--mdt-mono); }
[data-md-theme="mdmdt"] .mdio-doc .md-mermaid {
  background-color: var(--mdt-bg-color2);
  border: 1px solid var(--mdt-border-color);
  border-radius: 8px;
  padding: 16px;
  overflow-x: auto;
}
[data-md-theme="mdmdt"] .mdio-doc .md-html { color: inherit; }
[data-md-theme="mdmdt"] .mdio-doc .md-html-raw {
  background-color: var(--mdt-bg-color2);
  border: 1px dashed var(--mdt-border-color);
  border-radius: 8px;
  color: var(--mdt-text-grey);
}
[data-md-theme="mdmdt"] .mdio-doc .md-image { background-color: var(--mdt-bg-color2); border-radius: 8px; }
[data-md-theme="mdmdt"] .mdio-doc .md-image.is-loaded { background-color: transparent; }
[data-md-theme="mdmdt"] .mdio-doc ins {
  color: var(--mdt-color-3);
  text-decoration: underline;
  text-decoration-color: var(--mdt-color-3);
}

`;

fs.writeFileSync(outPath, header + lightOut.map((r) => `${r.sel} { ${r.body} }`).join("\n") + "\n\n/* ── 暗色差分 ── */\n" + darkBlock + "\n");
console.log("[port-theme-mdmdt] light rules =", lightOut.length, "· dark overrides =", darkOut.length, "· light tokens =", lightTokens.length, "· dark tokens =", darkTokens.length, "· ->", path.relative(process.cwd(), outPath));
