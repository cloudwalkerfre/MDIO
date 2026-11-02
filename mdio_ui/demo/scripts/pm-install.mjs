#!/usr/bin/env node
/**
 * demo 的依赖安装包装。
 *
 * 为什么需要它：**esmx 与 pnpm 对同一批名字的要求互斥** ——
 *   · esmx 要求"从 uses 链收到"的每个共享名都有版本范围，而它只从 `dependencies ∪ peerDependencies` 读
 *     （`@esmx/core/src/declaration/reader.ts:72-73`、`resolver.ts:344-345`）；
 *   · pnpm 则拒绝 `react/jsx-runtime` 这类**子路径**名出现在依赖字段里
 *     （`ERR_PNPM_INVALID_DEPENDENCY_NAME`，`--ignore-workspace` 也一样拒）。
 * ⇒ 安装时临时把这三个键摘掉，装完**原样放回**（值从当前 manifest 读取，不写死版本）。
 *
 * 用法：node scripts/pm-install.mjs           （等价于在 demo 包跑 pnpm install）
 *      node scripts/pm-install.mjs <额外参数>  （参数透传给 pnpm install）
 *      node scripts/pm-install.mjs --dir <包目录> [额外参数]  （对任意包执行，如 `lib/mods/ui_std`）
 *
 * 为什么加 `--dir`：mod 包（如 ui_std）也带同样的子路径 peer 声明，
 * 它的依赖应装在自己包里（node_modules/.pnpm + pnpm-lock.yaml），不再跨树手工软链。
 */
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const DEFAULT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
// `--dir <包目录>`：对任意包执行（默认仍是 demo）；取值后从透传参数里摘掉。
const argv = process.argv.slice(2);
const dirIdx = argv.indexOf("--dir");
let DIR = DEFAULT_DIR;
if (dirIdx >= 0 && argv[dirIdx + 1]) {
  DIR = path.resolve(process.cwd(), argv[dirIdx + 1]);
  argv.splice(dirIdx, 2);
}
const PKG = path.join(DIR, "package.json");
const SUBPATHS = ["react/jsx-runtime", "react/jsx-dev-runtime", "react-dom/client"];

const raw = fs.readFileSync(PKG, "utf8");
const json = JSON.parse(raw);
// **mod 包默认不自动装 peer**：这类包的共享单例（react/mobx 系）由 esmx `uses` 从宿主取，
// 本地若自动装 peer 会多一份 React/MobX（双实例）。demo 这类应用包没有 peer 声明，不受影响。
if ((((json.esmx || {}).uses) || []).length && Object.keys(json.peerDependencies || {}).length
  && !argv.some((a) => a.indexOf("auto-install-peers") >= 0)) {
  argv.push("--config.auto-install-peers=false");
  console.log("[pm-install] mod 包（esmx.uses 非空 + 有 peerDependencies）⇒ 自动加 --config.auto-install-peers=false");
}
const saved = [];
for (const field of ["dependencies", "peerDependencies", "devDependencies"]) {
  for (const name of SUBPATHS) {
    if (json[field] && json[field][name] !== undefined) { saved.push([field, name, json[field][name]]); delete json[field][name]; }
  }
}
if (saved.length) {
  fs.writeFileSync(PKG, JSON.stringify(json, null, 2) + "\n");
  console.log("[pm-install] 临时摘掉 " + saved.length + " 个子路径声明（pnpm 不接受这类名字）：" + saved.map((s) => s[1]).join(", "));
}
let code = 0;
try {
  execFileSync("pnpm", ["install", "--no-frozen-lockfile", ...argv], { cwd: DIR, stdio: "inherit" });
} catch (e) {
  code = typeof e.status === "number" ? e.status : 1;
} finally {
  if (saved.length) {
    // **原文还原**（不重新序列化）：键顺序/空行/注释字段都不动 ⇒ 不会把 peer 顺序写乱、diff 里不出现噪音。
    fs.writeFileSync(PKG, raw);
    console.log("[pm-install] 子路径声明已原样放回（" + saved.length + " 条，原文还原）");
  }
}
process.exit(code);
