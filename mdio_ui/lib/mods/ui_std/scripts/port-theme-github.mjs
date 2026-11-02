#!/usr/bin/env node
// port-theme-github —— 把 vendored 的上游 GitHub Markdown CSS 转成我们的主题包。
//
// 上游：`src/themes/vendor/github-markdown.css`
//   = rxliuli/tailwindcss-github-markdown 仓库里的 `src/github-markdown.css`（MIT），
//     其本身是 sindresorhus/github-markdown-css（MIT）的拷贝。两份署名都保留在生成物头部。
// 变换（全部机械可复核）：
//   1. 把两个 `@media (prefers-color-scheme: …)` 变量块拆成**容器状态**：
//      light → `[data-md-theme="github"] .mdio-doc` 的默认 token；dark → `body.dark` 与 `:host(.dark)` 两份（影子根不吃 body.dark）。
//   2. `.markdown-body` → `.mdio-doc`（我们的皮肤 ABI 容器）。
//   3. 白名单：只留「内容元素」规则（`.mdio-doc` 自身 / 内容标签 / `> *` / `::selection`），
//      GitHub 网页专属 DOM（.octicon / .anchor / .markdown-alert / .highlight / 控件重置…）整条丢弃。
//   4. 每条规则前缀门控 `[data-md-theme="github"] `，与其他皮肤同时在场也只影响自己。
// 用法：node scripts/port-theme-github.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const themes = path.join(here, "..", "src", "themes");
const srcPath = path.join(themes, "vendor", "github-markdown.css");
const outPath = path.join(themes, "github.css");

let src = fs.readFileSync(srcPath, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");

/** 摘出顶层 @media 块（括号配平），并从正文里移除。 */
function takeMedia(css) {
  const blocks = [];
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
    blocks.push(css.slice(i + m.index, j));   // 块起点是 @media，不是本段未处理文本的起点
    i = j;
  }
  return { rest: out, blocks };
}

const { rest, blocks } = takeMedia(src);
const declsOf = (block) => {
  const m = /\{([^{}]*)\}/.exec(block);
  return m ? m[1].split(";").map((s) => s.trim()).filter(Boolean) : [];
};
const isDark = (b) => /prefers-color-scheme\s*:\s*dark/.test(b);
const dark = blocks.filter(isDark).flatMap(declsOf);
const light = blocks.filter((b) => !isDark(b)).flatMap(declsOf);
const pick = (list) => list.filter((d) => /^(--|color-scheme)/.test(d));

// 白名单：内容元素（去掉 `.mdio-doc` 前缀后看第一个 token）
const CONTENT = /^(> \*|::selection|h[1-6]\b|p\b|ul\b|ol\b|li\b|blockquote\b|pre\b|code\b|table\b|thead\b|tbody\b|tr\b|th\b|td\b|hr\b|img\b|a\b|strong\b|em\b|del\b|mark\b|ins\b|sub\b|sup\b|kbd\b|details\b|summary\b|figure\b|figcaption\b|dl\b|dt\b|dd\b|caption\b|center\b|\.md-)/;

const rules = [];
const { rest: flat } = takeMedia(rest);
const ruleRe = /([^{}]+)\{([^{}]*)\}/g;
let m;
while ((m = ruleRe.exec(flat))) {
  const selText = m[1].trim().replace(/\s+/g, " ").replace(/\.markdown-body/g, ".mdio-doc");
  const body = m[2].trim().replace(/\s+/g, " ").replace(/\.markdown-body/g, ".mdio-doc");
  const kept = selText.split(",").map((s) => s.trim()).filter((s) => {
    if (s.includes("::before") || s.includes("::after")) return /::selection/.test(s);
    if (!s.startsWith(".mdio-doc")) return false;
    const restSel = s.slice(".mdio-doc".length).trim();
    return restSel === "" || CONTENT.test(restSel);
  });
  if (!kept.length) continue;
  rules.push({ sel: kept.map((s) => `[data-md-theme="github"] ${s}`).join(",\n"), body });
}

const header = `/* github —— GitHub Markdown 皮肤（由 scripts/port-theme-github.mjs 生成，勿手改）。
 *
 * 上游：rxliuli/tailwindcss-github-markdown 的 src/github-markdown.css（MIT，© rxliuli），
 *       该文件为 sindresorhus/github-markdown-css（MIT，© Sindre Sorhus）的拷贝 —— 两份署名随本文件保留。
 * 变换：@media 亮/暗变量块 → 容器状态；.markdown-body → .mdio-doc；白名单只留内容元素；全部规则带门控。
 * 已知裁剪：GitHub 网页专属 DOM（.octicon 锚点 / .markdown-alert / .highlight / 表单控件重置）已丢弃。
 */

/* 亮色 token（上游 prefers-color-scheme: light 块）
 * 注意：token 挂在**门控元素本身**（.br-shell），不是 .mdio-doc —— 因为画布要给宿主窗格 .br-doc 上色，
 * 而 .br-doc 是 .mdio-doc 的祖先，拿不到定义在 .mdio-doc 上的 var()。 */
[data-md-theme="github"] {
  ${pick(light).join(";\n  ")};
}

/* 暗色 token：document 面用 body.dark，影子根面用 :host(.dark)（body.dark 不跨界） */
body.dark [data-md-theme="github"] {
  ${pick(dark).join(";\n  ")};
}
:host(.dark) [data-md-theme="github"] {
  ${pick(dark).join(";\n  ")};
}

`;

fs.writeFileSync(outPath, header + rules.map((r) => `${r.sel} { ${r.body} }`).join("\n") + `

/* 兜底修正（上游语法在 mdio 里的落地差异）：
 *   · std 会把每个节再包一层 .mdio-std（appearance floor 里 font-size:15px）⇒ em 级联按 15 缩放而不是 16；
 *     给包裹层补回 16px，让上游的 em 比例按设计生效。
 *   · 行内代码的颜色上游是 inherit，而 floor 的 .md-inline-code 有粉色 token ⇒ 这里显式接管。
 *   · **画布策略**：上游 .markdown-body 自带整页底色，那是"页面画布"的职责。直接给 .mdio-doc 上色会得到
 *     一块随内容高度截断的矩形（深色下蓝黑 #0d1117 压在暖黑 chrome 上、正文结束后忽然切回宿主底色；
 *     浅色下则是一块白板）。正确落点是把底色给**宿主文档窗格 .br-doc**（它撑满窗格、没有截断边），
 *     .mdio-doc 保持透明。这样既保住上游的明暗对比（代码块 #151b23 只在 #0d1117 上才对），又不会出现色块接缝。 */
[data-md-theme="github"] .br-doc { background-color: var(--bgColor-default); }
[data-md-theme="github"] .mdio-doc { background-color: transparent; }
[data-md-theme="github"] .mdio-doc .mdio-std { font-size: 16px; line-height: 1.5; letter-spacing: normal; }
[data-md-theme="github"] .mdio-doc code.md-inline-code { color: var(--fgColor-default); }
/* 任务清单勾选框：上游 [type=] 控件重置整批被裁掉了（对 md 正文无用），但 checkbox 的强调色有用 ⇒ 单独补一条。
 * 这是上一轮实测里唯一「完全不变」的元素组（github 皮肤下仍是 floor 灰勾选框）。 */
[data-md-theme="github"] .mdio-doc input[type="checkbox"] { accent-color: var(--fgColor-accent); }

/* ── 外壳皮肤（整壳替换）：整 app 换到 GitHub 的配色；结构尺寸与交互仍归 app。 */
[data-md-theme="github"].br-shell { background: var(--bgColor-default); color: var(--fgColor-default); }
[data-md-theme="github"] .br-side { background: var(--bgColor-muted); border-right-color: var(--borderColor-default); }
[data-md-theme="github"] .br-eyebrow,
[data-md-theme="github"] .br-count,
[data-md-theme="github"] .br-folder-count,
[data-md-theme="github"] .br-folder,
[data-md-theme="github"] .br-item,
[data-md-theme="github"] .br-side-empty,
[data-md-theme="github"] .br-crumb-label,
[data-md-theme="github"] .br-crumb-sep,
[data-md-theme="github"] .br-theme { color: var(--fgColor-muted); }
[data-md-theme="github"] .br-folder-name { color: var(--fgColor-default); }
[data-md-theme="github"] .br-caret { border-left-color: var(--fgColor-muted); }
[data-md-theme="github"] .br-folder:hover,
[data-md-theme="github"] .br-item:hover { background: var(--bgColor-neutral-muted); color: var(--fgColor-default); }
[data-md-theme="github"] .br-item.is-active { background: var(--bgColor-neutral-muted); color: var(--fgColor-default); }
[data-md-theme="github"] .br-item.is-active::before { background: var(--fgColor-accent); }
[data-md-theme="github"] .br-file-glyph { border-color: var(--fgColor-muted); }
[data-md-theme="github"] .br-file-glyph::after { border-color: var(--fgColor-muted); background: var(--bgColor-muted); }
[data-md-theme="github"] .br-item.is-active .br-file-glyph,
[data-md-theme="github"] .br-item.is-active .br-file-glyph::after { border-color: var(--fgColor-accent); }
[data-md-theme="github"] .br-item.is-active .br-file-glyph::after { background: var(--bgColor-neutral-muted); }
[data-md-theme="github"] .br-crumb { background: var(--bgColor-default); border-bottom-color: var(--borderColor-default); }
[data-md-theme="github"] .br-crumb-path { color: var(--fgColor-accent); }
[data-md-theme="github"] .br-theme select { color: var(--fgColor-default); background: var(--bgColor-muted); border-color: var(--borderColor-default); }

/* 外壳残余件：选区 / 焦点环 / 滚动条 / 加载环 —— 皮肤不接管就会露出 app 的暖色。
 * 注：::-webkit-scrollbar 伪元素里 var() 不解析（floor 注释里实测过）⇒ 滚动条用字面色。 */
[data-md-theme="github"] .br-shell ::selection { background: rgba(9, 105, 218, 0.2); }
body.dark [data-md-theme="github"] .br-shell ::selection { background: rgba(68, 147, 248, 0.28); }
:host(.dark) [data-md-theme="github"] .br-shell ::selection { background: rgba(68, 147, 248, 0.28); }
[data-md-theme="github"] .br-item:focus-visible,
[data-md-theme="github"] .br-folder:focus-visible,
[data-md-theme="github"] .br-theme select:focus-visible { outline-color: var(--fgColor-accent); }
[data-md-theme="github"] .br-spin-ring { border-top-color: var(--fgColor-accent); }
[data-md-theme="github"] .br-tree::-webkit-scrollbar-thumb,
[data-md-theme="github"] .br-doc::-webkit-scrollbar-thumb { background-color: #d1d9e0; }
body.dark [data-md-theme="github"] .br-tree::-webkit-scrollbar-thumb,
body.dark [data-md-theme="github"] .br-doc::-webkit-scrollbar-thumb,
:host(.dark) [data-md-theme="github"] .br-tree::-webkit-scrollbar-thumb,
:host(.dark) [data-md-theme="github"] .br-doc::-webkit-scrollbar-thumb { background-color: #3d444d; }

/* ── mdio 扩展元素（上游无对应概念：按 GitHub 自己的组件语言推导，不另起风格）──────────
 * 推导依据全部取自本皮肤已移植的上游值：
 *   · 圆角 6px —— 本皮肤 pre/code 的 border-radius；
 *   · 次级底 --bgColor-muted / --bgColor-neutral-muted —— 本皮肤 code/blockquote 底色同源；
 *   · 边框 1px solid --borderColor-default、分隔线 --borderColor-muted —— 本皮肤 th/td/hr 同源；
 *   · 次字色 --fgColor-muted（h6/blockquote 用），强调色 --fgColor-accent（链接用），语气色 success/attention/danger/done；
 *   · 标签/标题字重 600（th、strong、h* 用）。
 * 约束：只接管颜色/底色/边框/圆角/字重，**不动结构**（grip 定位、md-image 的 min-height、mermaid 的 flex 居中仍归 floor）。 */
[data-md-theme="github"] .mdio-doc .md-bullet-key {
  color: var(--fgColor-default);
  background-color: var(--bgColor-neutral-muted);
  border-radius: 6px;
  padding: 0 6px;
  font-weight: 600;
}
[data-md-theme="github"] .mdio-doc .md-claim-badge {
  color: var(--fgColor-muted);
  background-color: var(--bgColor-neutral-muted);
  border: 1px solid var(--borderColor-muted);
  border-radius: 2em;
  padding: 0 8px;
  font-size: 12px;
}
[data-md-theme="github"] .mdio-doc .md-col-grip::before { background-color: var(--borderColor-default); }
[data-md-theme="github"] .mdio-doc th:hover .md-col-grip::before { background-color: var(--fgColor-accent); }
[data-md-theme="github"] .mdio-doc .md-math { color: var(--fgColor-default); }
[data-md-theme="github"] .mdio-doc .md-math--raw { color: var(--fgColor-muted); font-family: var(--fontStack-monospace); }
[data-md-theme="github"] .mdio-doc .md-mermaid {
  background-color: var(--bgColor-muted);
  border: 1px solid var(--borderColor-muted);
  border-radius: 6px;
  padding: 16px;
  overflow-x: auto;
}
[data-md-theme="github"] .mdio-doc .md-html { color: inherit; }
[data-md-theme="github"] .mdio-doc .md-html-raw {
  background-color: var(--bgColor-muted);
  border: 1px dashed var(--borderColor-default);
  border-radius: 6px;
  color: var(--fgColor-muted);
}
[data-md-theme="github"] .mdio-doc .md-image { background-color: var(--bgColor-muted); border-radius: 6px; }
[data-md-theme="github"] .mdio-doc .md-image.is-loaded { background-color: transparent; }
[data-md-theme="github"] .mdio-doc ins {
  color: var(--fgColor-default);
  text-decoration: underline;
  text-decoration-color: var(--borderColor-default);
}
/* Admonition = GitHub Alert 语言：左侧 4px 色条 + 无底色 + 标题用语气色（上游 .markdown-alert 已裁，这里补齐语义） */
[data-md-theme="github"] .mdio-doc .md-admonition {
  border: 0;
  border-left: 4px solid var(--borderColor-default);
  border-radius: 0;
  padding: 8px 16px;
  margin: 0 0 16px;
  background-color: transparent;
  color: var(--fgColor-default);
}
[data-md-theme="github"] .mdio-doc .md-admonition-title {
  font-size: 1em;
  font-weight: 600;
  margin-bottom: 8px;
  color: var(--fgColor-default);
}
[data-md-theme="github"] .mdio-doc .md-admonition--note { border-left-color: var(--fgColor-accent); }
[data-md-theme="github"] .mdio-doc .md-admonition--tip { border-left-color: var(--fgColor-success); }
[data-md-theme="github"] .mdio-doc .md-admonition--warning { border-left-color: var(--fgColor-attention); }
[data-md-theme="github"] .mdio-doc .md-admonition--danger { border-left-color: var(--fgColor-danger); }
[data-md-theme="github"] .mdio-doc .md-admonition--info { border-left-color: var(--fgColor-done); }
[data-md-theme="github"] .mdio-doc .md-admonition--note .md-admonition-title { color: var(--fgColor-accent); }
[data-md-theme="github"] .mdio-doc .md-admonition--tip .md-admonition-title { color: var(--fgColor-success); }
[data-md-theme="github"] .mdio-doc .md-admonition--warning .md-admonition-title { color: var(--fgColor-attention); }
[data-md-theme="github"] .mdio-doc .md-admonition--danger .md-admonition-title { color: var(--fgColor-danger); }
[data-md-theme="github"] .mdio-doc .md-admonition--info .md-admonition-title { color: var(--fgColor-done); }
` + "\n");
console.log("[port-theme-github] rules =", rules.length, "·", "light tokens =", pick(light).length, "· dark tokens =", pick(dark).length, "· ->", path.relative(process.cwd(), outPath));
