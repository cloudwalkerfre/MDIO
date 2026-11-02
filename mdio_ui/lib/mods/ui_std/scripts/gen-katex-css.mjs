// scripts/gen-katex-css.mjs —— 生成 katex-css.js：KaTeX 样式 + **base64 内联的 woff2 字体**。
//
// 为什么这么做（ui 模块的资源通道事实）：
//   模块产物只有 JS（实测 dist/client 下无 .css、无字体）⇒ 若在 std 里 import "katex/dist/katex.min.css"，
//   样式取不到、字体更取不到（CSS 里的 fonts/*.woff2 相对 URL 在页面里会 404）。
//   所以这里在**构建前**把 CSS 与字体内联成一个 JS 模块（导出字符串），由 std 在首次遇到公式时懒加载并注入 <style>。
//
// 产出：src/katex-css.js（**手动跑**：`node scripts/gen-katex-css.mjs`）。
// 本包 `package.json` 没有 `scripts` 字段 ⇒ **没有 prebuild 钩子**，本脚本需手动运行。
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const dist = path.join(root, "node_modules/katex/dist");
const cssPath = path.join(dist, "katex.min.css");
let css = fs.readFileSync(cssPath, "utf8");

let inlined = 0, bytes = 0, missing = 0;
// @font-face 的 src 列表改为"仅 woff2 的 base64"，其余声明（font-family/style/weight）保留
css = css.replace(/@font-face\{([^}]*)\}/g, (block, body) => {
  const srcMatch = body.match(/src:([^;}]*)/);
  const fam = (body.match(/font-family:\s*([^;}]+)/) || [])[1] || "?";
  if (!srcMatch) return block;
  const urls = [...srcMatch[1].matchAll(/url\(([^)]+)\)/g)].map((m) => m[1].replace(/["']/g, ""));
  const woff2 = urls.find((u) => u.endsWith(".woff2"));
  if (!woff2) { missing += 1; return block; }
  const file = path.join(dist, woff2.replace(/^\.\//, ""));
  const data = fs.readFileSync(file).toString("base64");
  bytes += data.length;
  inlined += 1;
  const srcNew = "src:url(data:font/woff2;base64," + data + ") format(\"woff2\")";
  return "@font-face{" + body.replace(/src:[^;}]*/, srcNew) + "}";
});

const out = "// 自动生成：勿手改（scripts/gen-katex-css.mjs）。KaTeX 样式 + base64 woff2 字体（自包含，宿主零参与）。\n"
  + "export const KATEX_CSS = " + JSON.stringify(css) + ";\n";
fs.writeFileSync(path.join(root, "katex-css.js"), out);
console.log("[gen-katex-css] 字体内联 " + inlined + " 个（缺失 " + missing + "），base64 " + Math.round(bytes / 1024) + "KB，输出 " + Math.round(out.length / 1024) + "KB");
