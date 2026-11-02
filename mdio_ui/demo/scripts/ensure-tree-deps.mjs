// @ts-check
/**
 * ensure-tree-deps —— **每棵树的 `ui/` 共享依赖落点**。
 *
 * 背景（实测）：mod 包构建时从**树级** `<tree>/ui/node_modules` 解析共享依赖；其中：
 *   · `mdio_ui` 是**手工符号链接**（→ `mdio_ui/lib/mods/ui_base`）—— 漏了就报
 *     `[E_NOT_LINKED] (<mod> → mdio_ui)`，构建直接失败；
 *   · `react` / `react-dom` / `katex` / `mermaid` / `shiki` / `@esmx` 等由**该树 `ui/` 下的 pnpm install** 提供。
 *
 * 本脚本只做**幂等的、无副作用的**那一半：补 `mdio_ui` 链接 + **报告**还缺哪些运行时依赖
 *（**不会**替你跑 pnpm install —— 那需要显式批准）。
 *
 * 用法：node scripts/ensure-tree-deps.mjs [--check]
 */
import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, readlinkSync, symlinkSync, unlinkSync } from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const PLATFORM = path.join(REPO, "mdio_ui/lib/mods/ui_base");
const checkOnly = process.argv.includes("--check");
const RUNTIME = ["react", "react-dom"];                       // 运行时必需（缺就报，不代装）

/** 树的候选根：注册表里的 `- path:` + `worktrees/*` + 仓库根下带 `ui/` 的目录。 */
function treeRoots() {
  const out = new Set();
  for (const p of [path.join(REPO, "worktrees/genesis/wt/.mds/worktrees.md"), path.join(REPO, ".mds/worktrees.md")]) {
    if (!existsSync(p)) continue;
    for (const m of readFileSync(p, "utf8").matchAll(/^-\s*path:\s*(.+)$/gm)) out.add(m[1].trim());
  }
  const wt = path.join(REPO, "worktrees");
  if (existsSync(wt)) for (const n of readdirSync(wt)) out.add(path.join(wt, n));
  for (const n of readdirSync(REPO)) { if (n.startsWith(".") || n === "node_modules") continue; out.add(path.join(REPO, n)); }
  return [...out].filter((r) => existsSync(path.join(r, "ui")));
}

const rows = [];
let fixed = 0;
for (const root of treeRoots()) {
  const ui = path.join(root, "ui");
  const nm = path.join(ui, "node_modules");
  const link = path.join(nm, "ui_base");
  let state = "ok";
  // 判存量必须用 **lstat**（不看目标）：`existsSync` 会跟符号链接，断链 ⇒ false ⇒ 于是去 symlink ⇒ EEXIST；
  //    而“目标过期但活着”的链接更隐蔽（platform 一搬家就发生，实测三条全中）⇒ 一律按“是链接 且 目标=期望”才算 ok。
  const isLink = (() => { try { return lstatSync(link).isSymbolicLink(); } catch (e) { return false; } })();
  const target = (() => { try { return readlinkSync(link); } catch (e) { return null; } })();
  const fresh = isLink && target === PLATFORM;
  if (fresh) { /* 已就位，不动 */ }
  else if (checkOnly) { state = isLink ? ("链接目标过期：" + target) : "缺 mdio_ui 链接"; }
  else {
    mkdirSync(nm, { recursive: true });
    if (isLink) { try { unlinkSync(link); } catch (e) { /* 已消失 */ } }
    try { symlinkSync(PLATFORM, link, "dir"); state = isLink ? "已刷新 mdio_ui 链接" : "已补 mdio_ui 链接"; fixed += 1; } catch (e) { state = "补链接失败：" + String(e.message).slice(0, 60); }
  }
  const missing = RUNTIME.filter((d) => !existsSync(path.join(nm, d)));
  rows.push({ root: path.relative(REPO, root), state, missing });
}

// ── 第二部分：**官方 mod 包自己的 `uses` 落点**（实测新增）──────────────────────
// 由来（实测三步）：
//   · `lib/mods/ui_std` 只有祖先 `lib/mods/node_modules/mdio_ui`（而且那还是条断链：→ ../../std/platform）
//     ⇒ 构建报 `[E_NOT_LINKED] (ui_std → mdio_ui)`；
//   · 只把**祖先**链接修对 ⇒ 仍然报错 ⇒ **esmx 不向上查找**；
//   · 补 `<pkg>/node_modules/mdio_ui → ../../mdio_ui` ⇒ 立刻可构建（2208ms）。
// 结论：`uses` 的提供者必须在**本包自己的 `node_modules/`** 里可见 ⇒ 与树的 `ui/` 同一条规则，必须自动补。
const pkgRows = [];
let pkgFixed = 0;
const libMods = path.join(REPO, "mdio_ui/lib/mods");
if (existsSync(libMods)) {
  const pkgs = [];
  const collectPkg = (dir) => {
    const pj = path.join(dir, "package.json");
    if (!existsSync(pj)) return;
    try { pkgs.push({ dir, j: JSON.parse(readFileSync(pj, "utf8")) }); } catch (e) { /* 坏包跳过 */ }
  };
  collectPkg(libMods);
  try { for (const n of readdirSync(libMods)) { if (n.startsWith(".") || n === "node_modules" || n === "src" || n.startsWith("dist")) continue; collectPkg(path.join(libMods, n)); } } catch (e) { /* 忽略 */ }
  const byName = new Map();
  for (const p of pkgs) if (p.j && typeof p.j.name === "string") byName.set(p.j.name, p.dir);
  for (const p of pkgs) {
    const uses = (p.j && p.j.esmx && p.j.esmx.uses) || [];
    for (const u of uses) {
      const pkgRel = path.relative(REPO, p.dir);
      const target = byName.get(u);
      if (!target) { pkgRows.push({ pkg: pkgRel, name: u, state: "被 uses 的包不在 lib/mods 里（需手工）" }); continue; }
      const nm = path.join(p.dir, "node_modules");
      const link = path.join(nm, u);
      const want = path.relative(nm, target);
      const isLink = (() => { try { return lstatSync(link).isSymbolicLink(); } catch (e) { return false; } })();
      const cur = (() => { try { return readlinkSync(link); } catch (e) { return null; } })();
      if (isLink && cur === want) { pkgRows.push({ pkg: pkgRel, name: u, state: "ok" }); continue; }
      if (checkOnly) { pkgRows.push({ pkg: pkgRel, name: u, state: isLink ? ("链接目标过期：" + cur) : "缺 uses 落点" }); continue; }
      mkdirSync(nm, { recursive: true });
      if (isLink) { try { unlinkSync(link); } catch (e) { /* 已消失 */ } }
      try { symlinkSync(want, link, "dir"); pkgRows.push({ pkg: pkgRel, name: u, state: isLink ? "已刷新 uses 落点" : "已补 uses 落点" }); pkgFixed += 1; }
      catch (e) { pkgRows.push({ pkg: pkgRel, name: u, state: "补 uses 落点失败：" + String(e.message).slice(0, 60) }); }
    }
  }
}

/**
 * **应用自身的 `link:` 依赖**（增补 · 一次真实事故的根因）：
 * `demo/package.json` 里 `"<name>": "link:<相对路径>"` 的条目，其 `node_modules/<name>` 软链**没有任何人负责刷新** ——
 * 改名/搬目录后它就成了断链（实测：把 `mdio_platform` 更名 `mdio_ui` + 目录改 `mdio_ui/` 之后，
 * `demo/node_modules/mdio_ui` 仍指旧路径 ⇒ protocol/externals/dev-provider 三项红 + 应用构建失败）。
 * 这里与上面「官方包 uses 落点」同法处理：**按 link: 的声明重建软链**。
 */
let appFixed = 0;
const appRows = [];
function fixAppLinks(demoDir) {
  const pj = path.join(demoDir, "package.json");
  if (!existsSync(pj)) return;
  const j = JSON.parse(readFileSync(pj, "utf8"));
  for (const [name, spec] of Object.entries(j.dependencies || {})) {
    if (typeof spec !== "string" || !spec.startsWith("link:")) continue;
    const target = path.resolve(demoDir, spec.slice(5));
    const nm = path.join(demoDir, "node_modules");
    const link = path.join(nm, name);
    const want = path.relative(nm, target);
    const isLink = (() => { try { return lstatSync(link).isSymbolicLink(); } catch (e) { return false; } })();
    const cur = (() => { try { return readlinkSync(link); } catch (e) { return null; } })();
    if (isLink && cur === want) { appRows.push({ name, state: "ok" }); continue; }
    if (checkOnly) { appRows.push({ name, state: isLink ? ("链接目标过期：" + cur) : "缺链接" }); continue; }
    mkdirSync(nm, { recursive: true });
    if (isLink) { try { unlinkSync(link); } catch (e) { /* 已消失 */ } }
    try { symlinkSync(want, link, "dir"); appRows.push({ name, state: isLink ? "已刷新" : "已补" }); appFixed += 1; }
    catch (e) { appRows.push({ name, state: "补链接失败：" + String(e.message).slice(0, 60) }); }
  }
}
fixAppLinks(import.meta.dirname ? path.resolve(import.meta.dirname, "..") : process.cwd());

/**
 * **共享单例落点**（实测事故的修法）：像 `ui_core`/`ui_devview` 这样「被 `link:` 进宿主图、**自己没有
 * node_modules**」的包，其源码里的裸名（react/mobx/mobx-react-lite…）必须有人解析。靠 `lib/mods/node_modules`
 * 那个「容器」——容器按业主口径删除后：dev 卡在「正在装载…」（`Failed to resolve import "mobx-react-lite"`），
 * 且预打包会从别处内联第二份 react ⇒ dispatcher 只装在其中一份 ⇒ `Cannot read properties of null (reading useRef)`。
 * 修法：**在每个这样的包自己的 node_modules 里**建到宿主那一份的链接（自己需要的东西在自己家）。
 * 注意：`resolve.alias` 试过**无效**（esmx 随后覆盖 resolve），必须在文件系统层做。
 */
const SHARED_SINGLETONS = ["react", "react-dom", "mobx", "mobx-react-lite", "mobx-state-tree"];
let sharedFixed = 0;
const sharedRows = [];
{
  const hostModules = path.join(REPO, "mdio_ui/demo/node_modules");
  const modsRoot = path.join(REPO, "mdio_ui/lib/mods");
  let names = [];
  try { names = fs.readdirSync(modsRoot); } catch (e) { names = []; }
  for (const name of names) {
    const dir = path.join(modsRoot, name);
    const pj = path.join(dir, "package.json");
    if (!fs.existsSync(pj)) continue;
    let j = null;
    try { j = JSON.parse(fs.readFileSync(pj, "utf8")); } catch (e) { continue; }
    if (!(((j.esmx || {}).uses) || []).length) continue;
    const nm = path.join(dir, "node_modules");
    let have = [];
    try { have = fs.readdirSync(nm); } catch (e) { have = []; }
    if (have.length > 3) continue;
    for (const p of SHARED_SINGLETONS) {
      const target = path.join(hostModules, p);
      if (!fs.existsSync(target)) continue;
      const link = path.join(nm, p);
      const want = path.relative(nm, target);
      const isLink = (() => { try { return fs.lstatSync(link).isSymbolicLink(); } catch (e) { return false; } })();
      const cur2 = (() => { try { return fs.readlinkSync(link); } catch (e) { return null; } })();
      if (isLink && cur2 === want) continue;
      if (checkOnly) { sharedRows.push({ pkg: name, name: p, state: isLink ? ("指向过期：" + cur2) : "缺链接" }); continue; }
      fs.mkdirSync(nm, { recursive: true });
      if (isLink) { try { fs.unlinkSync(link); } catch (e) { /* 已消失 */ } }
      try { fs.symlinkSync(want, link, "dir"); sharedRows.push({ pkg: name, name: p, state: "已补" }); sharedFixed += 1; }
      catch (e) { sharedRows.push({ pkg: name, name: p, state: "补失败：" + String(e.message).slice(0, 40) }); }
    }
  }
}
if (sharedRows.length) {
  console.log("  共享单例落点（自己没有 node_modules 的官方包）：" + (checkOnly ? "（--check 只读）" : ""));
  for (const r of sharedRows) console.log("    " + (r.state === "已补" ? "✓" : "·") + " " + r.pkg + " → " + r.name + "：" + r.state);
}

console.log("ensure-tree-deps: 检查 " + rows.length + " 棵树的 ui/ 依赖落点" + (checkOnly ? "（--check 只读）" : ""));
for (const r of rows) {
  const m = r.missing.length ? " · 缺运行时依赖：" + r.missing.join(", ") + "（需在该树 ui/ 下 pnpm install）" : "";
  console.log("  " + (r.state === "ok" || r.state.startsWith("已补") ? "✓" : "·") + " " + r.root + "：" + r.state + m);
}
if (pkgRows.length) {
  console.log("  官方包的 uses 落点（" + pkgRows.length + " 条）：" + (checkOnly ? "（--check 只读）" : ""));
  for (const r of pkgRows) console.log("    " + (r.state === "ok" || r.state.startsWith("已补") || r.state.startsWith("已刷新") ? "✓" : "·") + " " + r.pkg + " → " + r.name + "：" + r.state);
}
if (appRows.length) {
  console.log("  应用自身的 link: 依赖（" + appRows.length + " 条）：" + (checkOnly ? "（--check 只读）" : ""));
  for (const r of appRows) console.log("    " + (r.state === "ok" || r.state.startsWith("已补") || r.state.startsWith("已刷新") ? "✓" : "·") + " demo → " + r.name + "：" + r.state);
}
if (checkOnly) {
  const bad = rows.filter((r) => r.state !== "ok" || r.missing.length > 0).length + pkgRows.filter((r) => r.state !== "ok").length + appRows.filter((r) => r.state !== "ok").length;
  process.exit(bad ? 1 : 0);
}
console.log(fixed || pkgFixed || appFixed ? "✓ ensure-tree-deps：补/刷新 " + (fixed + pkgFixed + appFixed) + " 个链接（树 " + fixed + " · 官方包 " + pkgFixed + " · 应用 " + appFixed + "）" : "✓ ensure-tree-deps：无需改动");
