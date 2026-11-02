#!/usr/bin/env node
// gen-theme-fonts —— 把 src/themes/fonts/*.woff2 内联成 src/themes/fonts.css（base64 data URI）。
//
// 为什么内联而不是 url() 引文件：
//   · 主题 CSS 会经两条通道到达页面：document 的 <link>，以及镜像进影子根的 adoptedStyleSheets；
//   · constructable stylesheet 里的**相对 url() 按 document base 解析**（不是按 CSS 文件位置），
//     绝对路径又要求宿主知道把资产挂在哪个 URL ⇒ 内联是唯一两条通道都成立的形态。
//   · 先例：`src/katex-css.js` 就是把 KaTeX 9 个字体族 base64 内联（369 KB 单文件）。
//
// 字体来源：JetBrains Mono（OFL-1.1），latin 子集，400/700 两档，取自 Google Fonts 的 woff2。
// 用法：node scripts/gen-theme-fonts.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const themes = path.join(here, "..", "src", "themes");
const fontsDir = path.join(themes, "fonts");
const out = path.join(themes, "fonts.css");

const files = fs.readdirSync(fontsDir).filter((f) => /\.woff2$/.test(f)).sort();
if (!files.length) {
  console.error("[gen-theme-fonts] 没有找到 src/themes/fonts/*.woff2");
  process.exit(1);
}

const faces = files.map((f) => {
  const weight = (f.match(/-(\d+)\.woff2$/) || [])[1] || "400";
  const b64 = fs.readFileSync(path.join(fontsDir, f)).toString("base64");
  return [
    "@font-face {",
    '  font-family: "JBMono";',
    "  font-style: normal;",
    `  font-weight: ${weight};`,
    "  font-display: swap;",
    `  src: url("data:font/woff2;base64,${b64}") format("woff2");`,
    "}",
  ].join("\n");
});

const header = `/* fonts.css —— 自托管字体（由 scripts/gen-theme-fonts.mjs 生成，勿手改）。
 *
 * 字体：JetBrains Mono（SIL Open Font License 1.1），latin 子集，权重 ${files.map((f) => (f.match(/-(\d+)\./) || [])[1]).join("/")}。
 * 形态：base64 内联 —— 主题 CSS 同时走 <link> 与影子根 adoptedStyleSheets，
 *   相对 url() 在 constructable sheet 里会按 document base 解析而断链，内联是唯一两边都成立的形态。
 * 用法：字体 token 写 \`font-family: "JBMono", ui-monospace, ...\`；缺字体时按栈回退到系统等宽。
 */

`;
fs.writeFileSync(out, header + faces.join("\n\n") + "\n");
console.log("[gen-theme-fonts] ->", path.relative(process.cwd(), out), (header + faces.join("\n\n")).length, "B ·", files.join(", "));
