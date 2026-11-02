// 生成 appearance.css —— 把"CSS 字符串模块"(src/std/appearance.css.js) 落成真 .css，
// 供 CSS 联邦使用（真 import ⇒ 适配器记进 chunks[].css ⇒ rc.css() 出 <link>）。
// 与仓库既有的 scripts/gen-katex-css.mjs 同一惯例：生成物不入手工编辑路径。
import fs from "node:fs";
import path from "node:path";
import { STD_CSS } from "../src/std/appearance.css.js";

const out = path.resolve(import.meta.dirname, "../src/std/appearance.css");
fs.writeFileSync(out, STD_CSS.endsWith("\n") ? STD_CSS : STD_CSS + "\n");
console.log("[gen-appearance-css]", out, fs.statSync(out).size, "bytes");
