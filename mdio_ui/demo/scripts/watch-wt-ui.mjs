// watch-wt-ui.mjs —— G9 **第 2 档**（源码级热替换）的编排
//
// 事实依据（实测）：宿主的 import map 是**每请求**由 discoverWtModules 读各包 manifest 生成的
// ⇒ 包一重建，下一次请求就是新 hash ⇒ **不需要重启宿主**；换 hash 的模块需要重新取 ⇒ 属"整页重载"。
//
// 本脚本只做"自动触发重建"这一半：盯 mod 包源码 → 找到它所属的包 → 调 build-wt-ui.mjs。
// 另一半（页面 reload）不在这里做：HMR 不适用（模块 URL 变了），由开发者/宿主自助刷新。
import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
/** 需要盯的根：**在范围内的树**的 ui/ 与官方包目录（只看源码，不看 dist/node_modules）。
 *  范围口径：`genesis` · `exp_swap` · `exp_pages`。
 *  `exp_pages` 现在**没有** `ui/`（它的两个件住在 md 代码块里，走第 1 档 applyDecl，不经过构建）⇒
 *  这里列着但会被下面的 existsSync 跳过；将来它长出 mod 包时自动被盯上（只读核实补列）。 */
const ROOTS = [
  path.join(REPO, "worktrees/genesis/ui"),
  path.join(REPO, "exp_swap/ui"),
  path.join(REPO, "exp_pages/ui"),
  path.join(REPO, "mdio_ui/lib/mods"),
];
// 必须覆盖**构建自己的输出**：`dist` 只是精确名，`dist.gen*`（原子发布的代际目录）与 `.staging`（旁路）
// 里同样有 `.mjs`/`.css` 产物 ⇒ 不排除就会"构建 → 触发自己 → 再构建"的**风暴**（本 session 实测过风险）。
const IGNORE = /(^|[\\/])(node_modules|dist[^\\/]*|\.staging|\.tmp|\.vite-cache|\.esmx-build\.json)([\\/]|$)/;

/** 从被改文件向上找"最近的、含 package.json 的目录"——那就是它的包根。 */
function pkgRootOf(file) {
  let d = path.dirname(file);
  for (let i = 0; i < 8; i += 1) {
    if (fs.existsSync(path.join(d, "package.json"))) return d;
    const up = path.dirname(d);
    if (up === d) break;
    d = up;
  }
  return null;
}

const pending = new Map();   // pkgRoot → 定时器（防抖）
let builds = 0;
function schedule(pkgRoot, why) {
  clearTimeout(pending.get(pkgRoot));
  pending.set(pkgRoot, setTimeout(() => {
    pending.delete(pkgRoot);
    const t0 = Date.now();
    const p = spawn(process.execPath, [path.join(DEMO, "scripts/build-wt-ui.mjs"), pkgRoot], { cwd: DEMO, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    p.stdout.on("data", (b) => { out += String(b); });
    p.stderr.on("data", (b) => { out += String(b); });
    p.on("close", (code) => {
      builds += 1;
      const line = (out.split(String.fromCharCode(10)).filter((l) => l.includes("[build-wt-ui]")).pop() || "").trim();
      const name = path.basename(pkgRoot);
      console.log("[" + new Date().toTimeString().slice(0, 8) + "] " + (code === 0 ? "✓" : "✗") + " 重建 " + name + "（" + (Date.now() - t0) + "ms，触发: " + why + "）" + (line ? " · " + line.replace("[build-wt-ui] ", "") : ""));
      console.log("     ↳ 刷新页面即可看到新件（import map 每请求重生成，无需重启宿主）");
    });
  }, 120));
}

let watchers = 0;
for (const root of ROOTS) {
  if (!fs.existsSync(root)) continue;
  try {
    fs.watch(root, { recursive: true }, (ev, rel) => {
      if (!rel) return;
      const file = path.join(root, rel);
      if (IGNORE.test(file)) return;
      if (!/\.(js|jsx|ts|tsx|css|mjs)$/.test(file)) return;
      const pkgRoot = pkgRootOf(file);
      if (!pkgRoot) return;
      schedule(pkgRoot, path.relative(REPO, file));
    });
    watchers += 1;
  } catch (e) { console.warn("盯不住 " + root + "：" + String(e.message)); }
}
console.log("watch-wt-ui: 盯 " + watchers + " 个根（源码改动 → 自动重建所属包）");
for (const r of ROOTS) console.log("  " + (fs.existsSync(r) ? "·" : "⊘（不存在，跳过）") + " " + path.relative(REPO, r));