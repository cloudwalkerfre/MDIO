// build-babel.mjs —— 把 @babel/standalone 取成**自包含 ESM** 进 vendor/（大件不进 vite 的转换/预打包链路）。
// 为什么不用 CDN（实测）：mdLine.js 原先把 esm.sh 当运行期依赖，浏览器取它约 10 秒且经常失败
// （换树首屏被拖到 10.6s，jsx@page 的 md 件静默缺件：decl ✗ … 取件失败）。与 monaco 同款：本地 vendor +
// 壳的 /vendor/ 路由直出（同源、离线可用、无 CDN 依赖）。
//
// 用法：node scripts/build-babel.mjs          # 拉取 + 校验哈希 + 写入 vendor/babel-standalone.mjs
//      node scripts/build-babel.mjs --check  # 只校验现有文件（不联网）
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.join(HERE, "../vendor/babel-standalone.mjs");
const URL_ = "https://esm.sh/@babel/standalone@7.29.9/es2022/standalone.bundle.mjs";
const SHA = "7f25c56867443c51a47be448f43c9520b1823e3c507bd5610ca2de56909b2855";   // 换版本必须同时改 URL_ 与这里，并复跑换树/首屏实测

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

if (process.argv.includes("--check")) {
  if (!fs.existsSync(OUT)) { console.error("[build-babel] ✗ 缺 vendor/babel-standalone.mjs ⇒ 跑 node scripts/build-babel.mjs"); process.exit(1); }
  const buf = fs.readFileSync(OUT);
  const got = sha256(buf);
  if (got !== SHA) { console.error("[build-babel] ✗ 哈希不符：" + got.slice(0, 16) + " ≠ " + SHA.slice(0, 16)); process.exit(1); }
  console.log("[build-babel] ✓ 已就位 " + buf.length + " 字节 · sha256 校验通过");
  process.exit(0);
}

const r = await fetch(URL_);
if (!r.ok) { console.error("[build-babel] ✗ 拉取失败 HTTP " + r.status); process.exit(1); }
const buf = Buffer.from(await r.arrayBuffer());
const got = sha256(buf);
if (got !== SHA) { console.error("[build-babel] ✗ 内容哈希不符（上游变了？）：" + got.slice(0, 16)); process.exit(1); }
fs.mkdirSync(path.dirname(OUT), { recursive: true });
fs.writeFileSync(OUT, buf);
const ext = (buf.toString("utf8").match(/from\s*["']https?:/g) || []).length;
console.log("[build-babel] ✓ " + buf.length + " 字节 → vendor/babel-standalone.mjs · sha256 校验通过 · 外部 import 数=" + ext + "（应为 0）");