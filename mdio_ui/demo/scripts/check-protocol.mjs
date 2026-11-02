// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-protocol");
armWatchdog("check-protocol", 10000);   /* = 10s；实测 0.2s（扫描 1 跑）⇒ ×2 */
/**
 * check-protocol —— **"挂着的东西必须有条目"**的判据（把 §17 的约定从文档变成检查）。
 *
 * 对每个"可挂载的包"（工作树的 mod 包 ∪ 官方包 `lib/mods/**`）断言三件事：
 *   ① 宿主 `demo/package.json` 的 `esmx.uses` 里有它；
 *   ② `dependencies` 里有 `link:<相对 demo 的路径>` 且指向同一目录；
 *   ③ `demo/node_modules/<name>` 存在（esmx 用字面 fs.existsSync 找，不看 pnpm 解析）。
 *
 * 由来：`E_NOT_LINKED` 与"官方包漏条目"都踩过（§17.1）；`sync-wt-uses.mjs` 会**补**，本条判据负责**查**。
 * 用法：node scripts/check-protocol.mjs
 */
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const { discoverWtModules } = await import("../src/std/host/wtModules.ts");

const demoPkg = JSON.parse(fs.readFileSync(path.join(DEMO, "package.json"), "utf8"));
const uses = new Set(((demoPkg.esmx && demoPkg.esmx.uses) || []).slice());
const deps = demoPkg.dependencies || {};

const pkgs = [];
for (const [name, info] of Object.entries(discoverWtModules(REPO))) {
  const dir = info.pkgDir || info.ui || info.root;
  // **只有声明了 `esmx.exports` 的才是「可挂载的 mod」** —— 树级 `ui/package.json`（规则 9 的
  //   依赖落点式清单，如 `wtui_exp_scripts`：无 src、无 exports）不是模块，不该进 uses/node_modules 断言。
  let __j = null; try { __j = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8")); } catch (e) { /* 跳过 */ }
  if (!(__j && __j.name && __j.esmx && __j.esmx.exports)) continue;
  pkgs.push({ name, dir, kind: "wt" });
}
const libMods = path.join(REPO, "mdio_ui/lib/mods");
const pushOfficial = (dir) => {
  const pj = path.join(dir, "package.json");
  if (!fs.existsSync(pj)) return;
  try {
    const j = JSON.parse(fs.readFileSync(pj, "utf8"));
    if (j && j.name && j.esmx && j.esmx.exports) pkgs.push({ name: j.name, dir, kind: "official" });
  } catch (e) { /* 跳过 */ }
};
pushOfficial(libMods);
if (fs.existsSync(libMods)) {
  for (const n of fs.readdirSync(libMods)) {
    if (n.startsWith(".") || n === "node_modules" || n === "dist" || n.startsWith("dist.") || n === "src") continue;
    const sub = path.join(libMods, n);
    try { if (fs.statSync(sub).isDirectory()) pushOfficial(sub); } catch (e) { /* 跳过 */ }
  }
}

const bad = [];
for (const p of pkgs) {
  if (!uses.has(p.name)) bad.push([p.name, "uses 缺条目"]);
  const want = "link:" + path.relative(DEMO, p.dir);
  if (deps[p.name] !== want) bad.push([p.name, "dependencies 期望 " + want + "，实为 " + (deps[p.name] || "（缺）")]);
  if (!fs.existsSync(path.join(DEMO, "node_modules", p.name))) bad.push([p.name, "node_modules/" + p.name + " 不存在"]);
}

// ④/⑤ 反向断言（新增）：**条目指向的东西必须真的在**。
// 由来：三条残留（sweep_probe / ext_pkg_rehearsal / neotree_mod）都是「目标不存在、条目还在」——
// 上面的正向断言（已发现的包有没有条目）永远看不到它们，而悬空 link / 断链会让 esmx 启动期硬错。
// 与 sync-wt-uses 的清理规则同源：link 目标不存在 ⇒ 陈旧；node_modules 里断链 ⇒ 陈旧。
const dangling = [];
for (const [name, spec] of Object.entries(deps)) {
  const m = /^link:(.+)$/.exec(String(spec));
  if (!m) continue;
  if (!fs.existsSync(path.resolve(DEMO, m[1]))) dangling.push([name, "link 目标不存在：" + m[1]]);
}
try {
  for (const n of fs.readdirSync(path.join(DEMO, "node_modules"))) {
    const p = path.join(DEMO, "node_modules", n);
    try { if (!fs.lstatSync(p).isSymbolicLink()) continue; } catch (e) { continue; }
    if (!fs.existsSync(p)) dangling.push([n, "node_modules 里是断链"]);
  }
} catch (e) { /* node_modules 不存在 */ }
for (const [n, why] of dangling) bad.push([n, why]);

console.log("check-protocol: 检查 " + pkgs.length + " 个包（工作树 " + pkgs.filter((p) => p.kind === "wt").length + " · 官方 " + pkgs.filter((p) => p.kind === "official").length + "）· 反向：link/符号链接悬空 " + dangling.length + " 条");
if (bad.length) {
  for (const [n, why] of bad) console.log("  ✗ " + n + "：" + why);
  console.log("    ⇒ 跑 `node scripts/sync-wt-uses.mjs`（它会补齐）；若仍缺，检查包名/`esmx.exports`。");
  process.exit(1);
}
console.log("✓ check-protocol 全绿：uses / link / node_modules 三处条目齐备");
// 收尾显式退出（本脚本跑完仍有未释放句柄 ⇒ 会被自己的 deadline 判成「挂起」；红路径上面已 exit 1）
process.exit(0);
