// 引擎哈希戳：把 src/wasm 快照的引擎指纹写进 ui_wasm 的 .esmx-build.json（drift 检测的观测基）。
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
const here = path.dirname(fileURLToPath(import.meta.url));
const wasm = path.resolve(here, "..", "src", "wasm", "editor_core_bg.wasm");
const stampPath = path.resolve(here, "..", ".esmx-build.json");
const h = createHash("sha256").update(readFileSync(wasm)).digest("hex").slice(0, 16);
let stamp = {};
try { stamp = JSON.parse(readFileSync(stampPath, "utf8")); } catch (e) { /* 无戳则新建 */ }
stamp.engineHash = h;
writeFileSync(stampPath, JSON.stringify(stamp, null, 2));
console.log("engineHash = " + h);
