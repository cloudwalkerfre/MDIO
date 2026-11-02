// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-externals");
armWatchdog("check-externals", 10000   /* = 10s；实测 1.7s（另一次跑飞被截断） */)
/**
 * check-externals —— **mod 产物不得内联共享依赖**的判据（把 §21.1 的结论变成检查）。
 *
 * 为什么重要：共享单例（react / react-dom / mobx 系）只应由**提供者**（平台）带一份；
 * 任何 mod 产物里出现"内联的 React/MobX 实现"都意味着运行时可能拿到**第二份实例**（§18.4 的 TypeError 就是这类）。
 *
 * 判据：对每个 mod 包 `dist/client` 下的所有 `.final.mjs`：
 *   ① 不得出现内联标记（`react.production` / `__SECRET_INTERNALS` / `cjs/react-dom`）；
 *   ② 若**源码**里 import 了某个共享包，产物里应能看到对应的**裸名**引入（`from"react"` / `import"react"` 等）。
 * 用法：node scripts/check-externals.mjs
 */
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const { discoverWtModules } = await import("../src/std/host/wtModules.ts");

// **强信号**（命中任一 ⇒ 判红）：既保留 React 18 时代的字符串，也补上**现代 React 的结构性信号**。
// 由来（审查 B1）：旧清单三条字符串对现代内联**命中为 0** —— 实测平台自建产物 `lib/mods/ui_base/dist/client/react.<hash>.final.mjs`
//（9,574 B，确实内联了 React）三条全不匹配，但它含 `Symbol.for(`react.transitional.element`)` 与内部 `useState` 定义。
const INLINE = [
  /react\.production/, /__SECRET_INTERNALS/, /cjs\/react-dom/,      // 旧（React 18 时代）
  /Symbol\.for\(\s*[`"']react\./,                                  // 元素类型符号（react.element / react.transitional.element）
  /REACT_ELEMENT_TYPE/,                                            // 内部常量名
];
// **弱信号对**（两条同时命中才判红）：React 内部实现被内联时会出现这两个函数定义；
// 单独一条容易误伤"自己写了个同名小工具"的 mod。
const WEAK = [/function\s+useState\s*\(/, /function\s+createElement\s*\(/];
const SHARED = ["react", "react-dom", "mobx", "mobx-react-lite", "mobx-state-tree"];

const pkgs = [];
for (const [name, info] of Object.entries(discoverWtModules(REPO))) pkgs.push({ name, dir: info.pkgDir || info.ui || info.root });
const libMods = path.join(REPO, "mdio_ui/lib/mods");
const pushOfficial = (dir) => {
  const pj = path.join(dir, "package.json");
  if (!fs.existsSync(pj)) return;
  try { const j = JSON.parse(fs.readFileSync(pj, "utf8")); if (j && j.name && j.esmx && j.esmx.exports) pkgs.push({ name: j.name, dir }); } catch (e) { /* 跳过 */ }
};
pushOfficial(libMods);
if (fs.existsSync(libMods)) {
  for (const n of fs.readdirSync(libMods)) {
    if (n.startsWith(".") || n === "node_modules" || n === "dist" || n.startsWith("dist.") || n === "src") continue;
    const sub = path.join(libMods, n);
    try { if (fs.statSync(sub).isDirectory()) pushOfficial(sub); } catch (e) { /* 跳过 */ }
  }
}

/**
 * 共享依赖的**提供者**（如 ui_base 打包并提供 react）：它自己就是那份唯一实例 ⇒ 产物里出现内联标记是**设计**，不是违规。
 * 提供者不算「mod 产物」：否则该判据会在 W_NO_RANGE 修好后恒红（ui_base 的 react.<hash>.final.mjs 命中强信号）。
 * 判据来源：包的 esmx.provides 里列了 SHARED 中的依赖 ⇒ 它就是该依赖的所有者。
 */
const PROVIDERS = new Set();
for (const p of pkgs) {
  try {
    const j = JSON.parse(fs.readFileSync(path.join(p.dir, "package.json"), "utf8"));
    const prov = (j && j.esmx && j.esmx.provides) || [];
    const owns = Array.isArray(prov) && prov.some((n) => SHARED.includes(String(n).split("/")[0]));
    if (owns) PROVIDERS.add(p.name);
  } catch (e) { /* 读不到 package.json ⇒ 不豁免 */ }
}

const bad = [];
let checked = 0;
for (const p of pkgs) {
  if (PROVIDERS.has(p.name)) continue;   // 提供者：它带来的就是唯一那份实例（≠ 内联违规）
  const client = path.join(p.dir, "dist/client");
  if (!fs.existsSync(client)) continue;
  const walk = (dir) => fs.readdirSync(dir).flatMap((n) => {
    const f = path.join(dir, n);
    let st; try { st = fs.statSync(f); } catch (e) { return []; }
    return st.isDirectory() ? walk(f) : (n.endsWith(".final.mjs") ? [f] : []);
  });
  for (const f of walk(client)) {
    const txt = fs.readFileSync(f, "utf8");
    checked += 1;
    for (const re of INLINE) if (re.test(txt)) bad.push([p.name, path.basename(f), "内联强信号 " + re]);
    if (WEAK.every((re) => re.test(txt))) bad.push([p.name, path.basename(f), "内联弱信号对（useState + createElement 定义同时出现）"]);
  }
  // ② 源码 import 了共享包 ⇒ 产物里应有裸名引入（只对"直接 import"的包做，避免误报）
  const srcDir = path.join(p.dir, "src");
  if (!fs.existsSync(srcDir)) continue;
  const srcText = walk2(srcDir);
  for (const pkg of SHARED) {
    if (!new RegExp("from\\s+[\"']" + pkg.replace(/[/]/g, "\\/") + "[\"']").test(srcText)) continue;
    const distText = walk(client).map((f) => fs.readFileSync(f, "utf8")).join("");
    const bare = new RegExp("(from|import)\\s*[\"']" + pkg.replace(/[/]/g, "\\/") + "[\"']");
    if (!bare.test(distText)) bad.push([p.name, "src 用了 " + pkg + "，但产物里没有裸名引入（可能被内联或改名）"]);
  }
}
function walk2(dir) {
  return fs.readdirSync(dir).flatMap((n) => {
    const f = path.join(dir, n);
    let st; try { st = fs.statSync(f); } catch (e) { return []; }
    return st.isDirectory() ? walk2(f) : (/\.[jt]sx?$/.test(n) ? [fs.readFileSync(f, "utf8")] : []);
  }).join(String.fromCharCode(10));
}

/**
 * ③ **警告面**（2026-10-01 落地 · SURVEY 第四轮结论的判据化）：
 * 引擎对"从 `uses` 链收到、却没在自己的依赖里给出范围"的共享包发 **`W_NO_RANGE`** ——
 * 这正是那份研究点名的**静默失效**前身：范围门失效 ⇒ 版本不匹配也不会红，只会静默拿到别的版本。
 * 它只在构建输出里一闪而过（构建是别的判据在跑），**没有任何判据在看** ⇒ 删依赖时留下 38 条无人发现。
 * 判据：`esmx build` 的输出里不得出现任何 `W_NO_RANGE`。
 */
const { spawnSync } = await import("node:child_process");
// 注意：`node_modules/.bin/esmx` 是 **sh shim**（不是 JS）⇒ 直接执行它，别用 node 跑（实测 SyntaxError）。
const bres = spawnSync(path.join(DEMO, "node_modules", ".bin", "esmx"), ["build"], { cwd: DEMO, encoding: "utf8" });
const bout = String(bres.stdout || "") + String(bres.stderr || "");
if (bres.status !== 0) bad.push(["demo", "esmx build", "构建失败（退出码 " + bres.status + "）"]);
for (const l of bout.split(String.fromCharCode(10)).filter((x) => x.indexOf("W_NO_RANGE") >= 0).slice(0, 8)) {
  bad.push(["demo", "esmx build", "W_NO_RANGE（范围未声明 ⇒ 版本门静默失效）：" + l.trim().slice(0, 150)]);
}

console.log("check-externals: 检查 " + pkgs.length + " 个包、" + checked + " 个产物文件");
if (bad.length) {
  for (const [n, f, why] of bad) console.log("  ✗ " + n + " · " + f + "：" + why);
  console.log("    ⇒ 见 §18.4/§21.1：共享依赖应由提供者只带一份；不要把它内联进 mod 产物。");
  process.exit(1);
}
console.log("✓ check-externals 全绿：mod 产物无内联共享依赖，且裸名引入对齐源码用法");
process.exit(process.exitCode === undefined ? 0 : process.exitCode);   // 收尾退出：结论已打印 ⇒ 立刻退（CDP socket 会把进程吊到 deadline）
