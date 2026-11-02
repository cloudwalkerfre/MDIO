// @ts-check
/**
 * sync-wt-uses —— 把「工作树 UI 模块」的三处登记一次同步齐（§25.3 原则一：一处定义）。
 *
 * 单一真源 = src/std/host/wtModules.ts 的发现（**运行时注册表** `<rc>/wt/.mds/worktrees.md` 给出要看的树，
 * 再看各树 `ui/**` 里有 `package.json` 的目录；包名不再要求 `wtui_<uid>` 前缀 —— 迁移后就是 mod 自己的名字）。
 * 生成的三个消费者：
 *   ① package.json 的 esmx.uses           （模块名清单；缺它 links 会被 esmx 静默忽略）
 *   ② package.json 的 dependencies 的 link: 依赖（模块要能被解析成包）
 *   ③ node_modules/<name> 符号链接        （pnpm 的 link: 产物；不装依赖也能立刻开发）
 * 用法：node scripts/sync-wt-uses.mjs
 */
import { existsSync, lstatSync, readFileSync, readdirSync, readlinkSync, realpathSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { discoverWtModules } from "../src/std/host/wtModules.ts";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const DEMO = path.resolve(HERE, "..");
const REPO = path.resolve(DEMO, "../..");
const pkgPath = path.join(DEMO, "package.json");
const raw = readFileSync(pkgPath, "utf8");
const found = discoverWtModules(REPO);
const names = Object.keys(found).sort();
const usableOf = (n) => (found[n].usable === null ? found[n].built : found[n].usable);   // 戳即开关（G4/I1）
const builtNames = names.filter((n) => usableOf(n));          // 可用：进 uses（挂载面）
const notBuilt = names.filter((n) => !usableOf(n));           // 不可用（未构建 / 戳 ok:false / dev 产物）：不进 uses，但**明确打印**
const uiDirOf = (name) => found[name].pkgDir;           // link: 目标 = 包目录（wt 级包 = ui/；一 mod 一包 = ui/<mod>/）（I7/B2）

// ① esmx.uses
const reUses = /("uses"\s*:\s*\[)([^\]]*)(\])/;
const mUses = raw.match(reUses);
if (!mUses) throw new Error("package.json 里找不到 esmx.uses");
const prevAll = [...mUses[2].matchAll(/"([^"]+)"/g)].map((x) => x[1]);
// B5：**只管理 wtui_\* 集合**；非 wtui_ 项（mdio_ui / mdio_mods / …）一律原样保留（曾误删 mdio_mods）。
// **B5 修正**：非 wtui_ 项照旧保留，**但**若它在 `dependencies` 里是 `link:<路径>` 而该路径
// 已不存在（第三方 mod 包被删掉），就**不再保留** —— 否则 `uses` 会永久留一个指向不存在模块的条目，
// esmx 启动直接 `[E_NOT_LINKED]` 崩掉（本 session 真实踩过：中断的演练留下 `ext_pkg_rehearsal`）。
const linkRelOf = (n) => { const m = raw.match(new RegExp("\"" + n + "\"\\s*:\\s*\"link:([^\"]+)\"")); return m ? m[1] : null; };
const nonWtui = prevAll.filter((n) => !n.startsWith("wtui_")).filter((n) => {
  const rel = linkRelOf(n);
  if (!rel) return true;                        // 不是 link 依赖（平台包 / workspace:* 等）⇒ 保留
  return existsSync(path.resolve(DEMO, rel));   // link 目标不存在 ⇒ 丢弃（陈旧条目）
});
const keptWtui = prevAll.filter((n) => n.startsWith("wtui_") && names.includes(n));
const uses = [...new Set([...nonWtui.filter((n) => n === "ui_base"), ...nonWtui.filter((n) => n !== "ui_base"), ...keptWtui, ...builtNames])];
let next = raw.replace(reUses, (mm, a, b, c) => a + "\n" + uses.map((u) => "      " + JSON.stringify(u)).join(",\n") + "\n    " + c);

// ② dependencies 的 link: 依赖（先删掉已失效的 wtui_*，再补齐缺失的）
const need = names.map((n) => ({ n, rel: path.relative(DEMO, uiDirOf(n)) }));
const blockRe = /("dependencies"\s*:\s*\{)([\s\S]*?)(\n  \})/;
const mDep = next.match(blockRe);
if (!mDep) throw new Error("package.json 里找不到 dependencies");
const keptDeps = mDep[2]
  .split("\n")
  .map((l) => l.trim())
  // 摘掉"指向**被发现的 wt mod 包**"的 link: 依赖——**按目标目录识别**，不按名字前缀：
  // 迁移后包名不再带 `wtui_` 前缀（如 `genesis_region`），按前缀摘会**每次重复追加**（实测非幂等）。
  .filter((l) => {
    if (!l) return false;
    const mm = /^"([^"]+)":\s*"link:(.+)"$/.exec(l.replace(/,$/, ""));
    if (!mm) return true;
    // ① 目标是**被发现的 wt mod 包** ⇒ 本次会重建这一行，先摘掉（原规则；按目标目录识别，不按名字前缀）
    if (new Set(Object.values(found).map((v) => path.resolve(v.pkgDir))).has(path.resolve(DEMO, mm[2]))) return false;
    // ② **目标不存在** ⇒ 陈旧条目，一律摘掉（新增）。原规则只摘「被发现的包」，
    //    第三方包被删掉后它的 link: 永远留着（实测残留三条：sweep_probe / ext_pkg_rehearsal / neotree_mod
    //    ⇒ 悬空 link + 悬空符号链接；而 `check-protocol` 只查「已发现的包有没有条目」，反向无人查）。
    return existsSync(path.resolve(DEMO, mm[2]));
  })
  .map((l) => l.replace(/,$/, ""));                          // 统一去尾逗号，后面重建时再加
const wtLines = need.map((x) => JSON.stringify(x.n) + ': "link:' + x.rel + '"');
const body = [...keptDeps, ...wtLines].map((l) => "    " + l).join(",\n");
next = next.replace(blockRe, (mm, a, b, c) => a + "\n" + body + c);
writeFileSync(pkgPath, next);

// ③ node_modules 符号链接
let links = 0;
for (const { n } of need) {
  const target = path.join(DEMO, "node_modules", n);
  const rel = path.relative(path.join(DEMO, "node_modules"), uiDirOf(n));
  // 目标是**符号链接**：必须用 unlink（rmSync 对"指向目录的软链"在 macOS 上抛 EISDIR，会被吞掉 ⇒
  // 旧链接永不刷新、且计数假报 0——实测踩过）。
  try { if (lstatSync(target)) unlinkSync(target); } catch (e) { /* 不存在 */ }
  try { symlinkSync(rel, target, "dir"); links += 1; } catch (e) { console.warn("  符号链接失败 " + n + ": " + e.message); }
}
// ③b 断链/陈旧链接清理（G5）：node_modules 里所有 wtui_* 条目，凡不在本次发现里的，一律删掉。
// 由来：曾留下 DANGLING 的 wtui_c8page01 → exp_pages/ui（树早没了）——刷新循环只遍历"发现的"名字，
// 所以**永远轮不到**它，必须单独扫一遍。
let swept = 0;
try {
  const nmDir = path.join(DEMO, "node_modules");
  for (const n of readdirSync(nmDir)) {
    if (names.includes(n)) continue;                      // 本次发现到的：保留
    // **按"目标目录"识别陈旧链接**（修）：迁移后 mod 包名不再带 `wtui_` 前缀
    //（如 `genesis_region`），只按前缀扫会**漏掉**它们的悬空链接。做法是：
    // "是符号链接 **且** 指向 `worktrees/**/ui/**`（即来自工作树的模块）**且** 不在本次发现里" ⇒ 删。
    // （官方包指向 `mdio_ui/lib/mods/**`，不属于该范围 ⇒ 不动。）
    const p = path.join(nmDir, n);
    try { if (!lstatSync(p).isSymbolicLink()) continue; } catch (e) { continue; }
    // **断链一律算陈旧**（新增）：existsSync 会跟符号链接 ⇒ false 即悬空。
    // 旧代码在这里漏掉它们：realpathSync 对断链**抛错**，被下面那个 catch 当成「不是链接」跳过 ⇒ 永远清不掉。
    if (!existsSync(p)) {
      try { unlinkSync(p); swept += 1; console.log("  清理断链：" + n); } catch (e) { /* 已消失 */ }
      continue;
    }
    try {
      const target = realpathSync(p);
      if (target.indexOf(path.join(REPO, "worktrees") + path.sep) !== 0) continue;
    } catch (e) { continue; }
    try { unlinkSync(p); swept += 1; console.log("  清理陈旧链接：" + n); } catch (e) { /* 非链接 / 已消失 */ }
  }
} catch (e) { /* node_modules 不存在 */ }

// ④ 官方包条目：`<demo>/../lib/mods/` 下每个**真 mod 包**也要在 `node_modules` 里
//    **落下条目**——esmx 用字面 fs.existsSync 逐级向上找、不看 pnpm 解析，缺条目 = 硬错
//    `E_NOT_LINKED`（实测：`ui_std` 迁移后首次启动即踩到，当时靠手工 `ln -s` 补救）。
//    **只有“对外有面的包”才算 mod**：`esmx.exports`（导出件）或 `esmx.provides`（单例供给）。
//    `lib/mods/` 这个**容器目录**自己不落条目（A10-C7：它唯一的导出是零引用的 lab 探针，
//    探针不存在 ⇒ 容器不再是模块；否则它会以“空壳 mod”的身份永久占着挂载表）。
const official = [];
const libMods = path.resolve(DEMO, "..", "lib", "mods");
const collectOfficial = (pkgDir) => {
  try {
    const j = JSON.parse(readFileSync(path.join(pkgDir, "package.json"), "utf8"));
    if (!j || typeof j.name !== "string" || !j.name) return;
    const face = j.esmx && (j.esmx.exports || j.esmx.provides);
    if (!face) return;                                       // 容器 / 普通包：不是 mod，不落条目
    if (!official.some((o) => o.name === j.name)) official.push({ name: j.name, dir: pkgDir });
  } catch (e) { /* 无包或坏包 */ }
};
collectOfficial(libMods);                                    // 容器自身没有 esmx 段 ⇒ 自动跳过
try {
  for (const n of readdirSync(libMods)) {
    if (n.startsWith(".") || n === "node_modules" || n === "dist" || n.startsWith("dist.")) continue;   // dist.gen* = 代际目录（不是包）   // dist 也含 package.json（构建产物），必须跳过
    const d = path.join(libMods, n);
    try { if (lstatSync(d).isDirectory()) collectOfficial(d); } catch (e) { /* 跳过 */ }
  }
} catch (e) { /* lib/mods 不存在 */ }
let offLinks = 0;
for (const o of official) {
  const target = path.join(DEMO, "node_modules", o.name);
  const rel = path.relative(path.join(DEMO, "node_modules"), o.dir);
  try { if (lstatSync(target)) unlinkSync(target); } catch (e) { /* 不存在 */ }
  try { symlinkSync(rel, target, "dir"); offLinks += 1; } catch (e) { console.warn("  官方包符号链接失败 " + o.name + ": " + e.message); }
  const inUses = uses.includes(o.name);
  const relPkg = path.relative(DEMO, o.dir);                       // package.json 里的 link: 口径 = 相对 demo
  const dep = (next.match(new RegExp("\"" + o.name + "\"\\s*:\\s*\"link:([^\"]+)\"")) || [])[1];
  if (!inUses || dep !== relPkg) console.log("  ⚠ 官方包协议漂移：" + o.name + (inUses ? "" : "（uses 缺）") + (dep === relPkg ? "" : "（link: 期望 " + relPkg + (dep ? "，实为 " + dep : "，缺失") + "）"));
}

// ④b **容器/非活跃 mod 的陈旧条目**：`node_modules` 里凡是指向 `lib/mods/**` 却不在本次 official 名单里的
//     条目，一律删（`mdio_mods` 从 mod 退成容器后，它的链接必须跟着消失）。
//     只按“目标在 lib/mods 下 + 名字不在名单”两条判，不碰 pnpm 自己的链接。
//     **判据是"有没有在 package.json 里声明"，不是"是不是 mod"**（修）：`lib/mods/` 下
//     现在既有 mod（std/platform，靠 uses+link 声明），也有**库包**（devview，只靠 link 声明、不进 uses）。
//     按"不是 mod 就删"会误删库包的链接 ⇒ 一律以 `dependencies` 里的 `link:` 声明为准。
const declaredLinks = new Map();
try {
  const depsObj = (JSON.parse(readFileSync(pkgPath, "utf8")).dependencies) || {};
  for (const [n, v] of Object.entries(depsObj)) {
    const m = /^link:(.+)$/.exec(String(v));
    if (m) declaredLinks.set(n, path.resolve(DEMO, m[1]));
  }
} catch (e) { /* 读不出 ⇒ 视为无声明 */ }
let offSwept = 0, declaredLinked = 0;
const nmDir2 = path.join(DEMO, "node_modules");
// ④b-1 非活跃/未声明的 lib/mods 链接：删
try {
  const libModsAbs = path.resolve(libMods);
  for (const n of readdirSync(nmDir2)) {
    if (declaredLinks.has(n)) continue;                          // 已声明 ⇒ 保留（下面还会刷新）
    const p = path.join(nmDir2, n);
    try { if (!lstatSync(p).isSymbolicLink()) continue; } catch (e) { continue; }
    let real = null;
    try { real = realpathSync(p); } catch (e) { continue; }      // 断链由 ③b 负责
    if (real !== libModsAbs && real.indexOf(libModsAbs + path.sep) !== 0) continue;
    try { unlinkSync(p); offSwept += 1; console.log("  清理未声明的 lib/mods 链接：" + n); } catch (e) { /* 已消失 */ }
  }
} catch (e) { /* node_modules 不存在 */ }
// ④b-2 声明了 `link:` 但 node_modules 里没有（或指错）的 ⇒ 补/刷新（库包如 ui_devview 靠这条落下条目）
for (const [n, abs] of declaredLinks) {
  const p = path.join(nmDir2, n);
  let cur = null;
  try { cur = readlinkSync(p); } catch (e) { /* 不存在 */ }
  const want = path.relative(nmDir2, abs);
  if (cur === want) continue;
  try { if (lstatSync(p)) unlinkSync(p); } catch (e) { /* 不存在 */ }
  try { symlinkSync(want, p, "dir"); declaredLinked += 1; } catch (e) { console.warn("  声明链接失败 " + n + ": " + e.message); }
}

console.log("工作树模块：" + names.join(", "));
console.log("官方包：" + official.map((o) => o.name).join(", ") + "（条目 " + offLinks + " 个）");
console.log("  uses        = [" + uses.join(", ") + "]");
console.log("  link 依赖   = " + need.length + " 条");
console.log("  符号链接    = " + links + " 个（清理陈旧 " + swept + " 个）" + (declaredLinked ? "，按声明补 " + declaredLinked + " 个" : "") + (offSwept ? "（另清未声明的 lib/mods 链接 " + offSwept + " 个）" : ""));
if (notBuilt.length) {
  console.log("  未构建（只有源码 ⇒ 不进挂载表，先编再同步）：" + notBuilt.join(", "));
  for (const n of notBuilt) console.log("    " + n + "  →  node scripts/build-wt-ui.mjs " + found[n].root);
}
