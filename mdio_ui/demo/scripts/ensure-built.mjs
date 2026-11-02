// @ts-check
/**
 * ensure-built —— **启动前预构建**。
 *
 * 为什么需要：冷态（某包只有源码）下 `esmx dev` **启动会失败**（`uses` 里的模块缺 `dist` ⇒ 端口无响应）。
 * 之前靠"宿主启动时踢 `[autoBuild]` + 再重启一次"绕过；预构建把这一步**提前到启动之前** ⇒ 一次起来就是可服务状态。
 *
 * 判定（与 `src/std/host/wtModules.ts` 同口径，刻意保持简单并在此注明）：
 *   · 需要构建 = `dist/client/manifest.json` 不存在，**或**戳 `ok:false` / `source!=="build"` / `published===false`；
 *   · 按 `package.json` 有 `esmx.exports` 的目录视为"ui 包"，逐个串行构建（用同一个 `build-wt-ui.mjs`，原子发布）。
 *
 * 用法：node scripts/ensure-built.mjs [--dry] [--quiet]
 * 退出码：0 = 全部就绪（含"本来就没包"）；1 = 有包构建失败。
 */
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const dry = process.argv.includes("--dry");
const quiet = process.argv.includes("--quiet");
const REPO0 = path.resolve(DEMO, "../..");
const say = (...a) => { if (!quiet) console.log(...a); };

// **复用宿主的发现与判据**（Node 24 可直接 import 那份 TS ⇒ 不再重复实现）：
//   · 工作树的包：`discoverWtModules(REPO)`（与宿主挂载同一函数）；
//   · 官方包（`lib/mods/**`）：只做"哪个目录有 package.json + esmx.exports"的最小枚举，
//     状态判定仍调 `packageState()`（同一份戳口径）⇒ 口径只有一处。
const { discoverWtModules, packageState } = await import("../src/std/host/wtModules.ts");

function officialPkgs() {
  const out = [];
  const push = (dir) => {
    const pj = path.join(dir, "package.json");
    if (!fs.existsSync(pj)) return;
    // 认"有 esmx 段且**对外有面**"的包：`exports`（导出件）**或 `provides`（单例供给，如平台）**。
    // 只认 exports 会漏掉 `mdio_ui`——它没有 exports、只 provides ⇒ 平台挪窝后没人替它重建（实测）。
    try { const j = JSON.parse(fs.readFileSync(pj, "utf8")); if (j && j.esmx && (j.esmx.exports || j.esmx.provides)) out.push(dir); } catch (e) { /* 跳过 */ }
  };
  const libMods = path.join(REPO, "mdio_ui/lib/mods");
  if (!fs.existsSync(libMods)) return out;
  push(libMods);
  for (const n of fs.readdirSync(libMods)) {
    if (n.startsWith(".") || n === "node_modules" || n === "dist" || n.startsWith("dist.") || n === "src") continue;
    const sub = path.join(libMods, n);
    try { if (fs.statSync(sub).isDirectory()) push(sub); } catch (e) { /* 跳过 */ }
  }
  return out;
}

/** 需要构建的包：工作树（宿主同款发现）∪ 官方包（同款状态判定）。 */
function collect() {
  const found = [];
  for (const [name, info] of Object.entries(discoverWtModules(REPO))) {
    found.push({ dir: info.pkgDir || info.ui || info.root, why: info.usable === false ? info.reason : (info.built ? null : "no-dist"), name });
  }
  for (const dir of officialPkgs()) {
    const st = packageState(dir);
    found.push({ dir, why: st.usable ? null : st.reason, name: path.basename(dir) });
  }
  return found;
}
const pkgs = collect();
const todo = pkgs.filter((x) => x.why);   // collect() 已给出 why（复用 packageState/宿主发现）
say("ensure-built: 发现 " + pkgs.length + " 个 ui 包，其中 " + todo.length + " 个需要构建" + (todo.length ? "：" + todo.map((t) => t.name + "(" + t.why + ")").join(", ") : ""));
if (dry || todo.length === 0) process.exit(0);

let failed = 0;
for (const t of todo) {
  const t0 = Date.now();
  const r = spawnSync(process.execPath, [path.join(DEMO, "scripts/build-wt-ui.mjs"), t.dir], { cwd: DEMO, encoding: "utf8" });
  const line = String(r.stdout || "").split(String.fromCharCode(10)).filter((l) => l.indexOf("[build-wt-ui]") >= 0).pop() || "";
  if (r.status !== 0) failed += 1;
  say("  " + (r.status === 0 ? "✓" : "✗") + " " + t.name + "（" + (Date.now() - t0) + "ms，" + t.why + "）" + (line ? " · " + line.trim().replace("[build-wt-ui] ", "") : ""));
}
say(failed ? "✗ ensure-built：失败 " + failed + " 个" : "✓ ensure-built：全部就绪");
process.exit(failed ? 1 : 0);