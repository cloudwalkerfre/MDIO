// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-tree-form");
armWatchdog("check-tree-form", 10000);   /* = 10s；实测 0.1s（扫描 1 跑）⇒ ×2 */
/**
 * check-tree-form —— **在范围内的三棵树的"形态约定"判据**（把文档里的约定变成检查）。
 *
 * 规则（只查在范围内的树：`genesis` · `exp_swap` · `exp_pages`；其余 wt 未对齐新版，按裁定不管）：
 *   1. 声明里**不得**再出现 `.ui::` / `ui:` 形态的 ui 件引用 —— 迁移后 ui 件一律写**逻辑名**（`<包名>/<导出>`）；
 *      （`.md::` 的 md 件引用不受影响：那是另一条装载线。）
 *   2. 树的 `ui/` 顶层**只允许**：mod 包目录（含 package.json）、`node_modules`、`src`、`scripts`、lock 文件、`dist*`、`.staging`、`.esmx-build.json`；
 *      **不得有散落的 `.js/.jsx/.ts/.tsx/.mjs/.css` 文件**（"散件"是迁移前形态，规则 9 也会拦）。
 *   3. 树的 `ui/` 里**不得出现与 `mdio_ui/lib/mods/**` 逐字节相同的文件**（"官方件被复制一份"）。
 *
 * 用法：node scripts/check-tree-form.mjs
 */
import fs from "node:fs";
import path from "node:path";
import { createHash } from "node:crypto";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const TREES = ["worktrees/genesis", "exp_swap", "exp_pages"];
// 允许的顶层辅助目录/文件：`.tmp` 是 Node 的编译缓存（`node-compile-cache`，实测 2.1MB，非源码）。
const ALLOW_TOP = new Set(["node_modules", "src", "scripts", ".staging", ".tmp", ".esmx-build.json", ".dist-link.tmp"]);
const ALLOW_TOP_RE = /^(dist($|\.)|pnpm-lock\.yaml$|pnpm-workspace\.yaml$|package\.json$)/;
const LOOSE_RE = /\.(js|jsx|ts|tsx|mjs|css)$/;

const bad = [];
const notes = [];

// ① 声明里不得有 .ui:: / ui: 形态
for (const t of TREES) {
  const decl = path.join(REPO, t, "wt/.mds/ui.md");
  if (!fs.existsSync(decl)) { notes.push(t + "：无 .mds/ui.md（跳过规则 1）"); continue; }
  const txt = fs.readFileSync(decl, "utf8");
  txt.split(String.fromCharCode(10)).forEach((line, i) => {
    const m = /^\s*-\s*[A-Za-z0-9_]+:\s*(\.ui::|ui:)(.+)$/.exec(line);
    if (!m) return;
    // **放宽**：`.ui::<file>` 若指向树里**真实存在的顶层文件**，那是**单文件 mod**
    // （第三方"不大而全"的形态）⇒ 合法；只有"目标文件不存在"时才是旧形态残留。
    const target = path.join(REPO, t, "ui", m[2].trim());
    if (fs.existsSync(target)) return;
    bad.push([t + "/wt/.mds/ui.md:" + (i + 1), "旧形态（" + m[1] + m[2] + "）且 ui/" + m[2].trim() + " 不存在 ⇒ 应写逻辑名，或补上那个单文件"]);
  });
}

// ② + ③ 树的 ui/ 顶层形态；以及"官方件被复制"
const sha = (f) => createHash("sha256").update(fs.readFileSync(f)).digest("hex");
const libMods = path.join(REPO, "mdio_ui/lib/mods");
const official = new Map();
const __seen = new Set();   // 防符号链接成环（sync-wt-uses 补的 link 会成环 ⇒ 之前「10s 判挂起」其实是无尽递归）
const walk = (dir, fn) => {
  let __real; try { __real = fs.realpathSync(dir); } catch (e) { return; }
  if (__seen.has(__real)) return;
  __seen.add(__real);
  for (const n of fs.readdirSync(dir)) {
    if (n === "node_modules" || n === "dist" || n.startsWith("dist.") || n === ".staging" || n === ".vite-cache") continue;
    const f = path.join(dir, n);
    let st; try { st = fs.statSync(f); } catch (e) { continue; }
    if (st.isDirectory()) walk(f, fn); else fn(f);
  }
};
if (fs.existsSync(libMods)) walk(libMods, (f) => { if (/\.(js|jsx|ts|tsx|mjs|css)$/.test(f)) official.set(sha(f), path.relative(REPO, f)); });

for (const t of TREES) {
  const ui = path.join(REPO, t, "ui");
  if (!fs.existsSync(ui)) { notes.push(t + "：无 ui/ 目录（本就不涉及 ui 包）"); continue; }
  for (const n of fs.readdirSync(ui)) {
    const f = path.join(ui, n);
    const st = fs.statSync(f);
    if (!st.isDirectory()) {
      if (LOOSE_RE.test(n)) {
        // **放宽**：顶层的**单文件 mod**（第三方"不大而全"的形态）是**合法**的 ——
        // 前提是它**被本树的声明引用**（`.ui::<文件名>` / `ui:<文件名>`）。未被声明的散件才算残留。
        const decl = path.join(REPO, t, "wt/.mds/ui.md");
        const declTxt = fs.existsSync(decl) ? fs.readFileSync(decl, "utf8") : "";
        const referenced = declTxt.indexOf(n) >= 0;
        if (!referenced) bad.push([t + "/ui/" + n, "散件（未被本树声明引用）⇒ ui/ 顶层只应是 mod 包目录，或被声明引用的单文件 mod"]);
      }
      continue;
    }
    if (ALLOW_TOP.has(n) || ALLOW_TOP_RE.test(n)) continue;
    if (!fs.existsSync(path.join(f, "package.json"))) bad.push([t + "/ui/" + n, "目录里没有 package.json ⇒ 既不是 mod 包也不是允许的辅助目录"]);
    // ③ 该包内是否有与官方件逐字节相同的文件
    walk(f, (file) => {
      if (!/\.(js|jsx|ts|tsx|mjs|css)$/.test(file)) return;
      const h = sha(file);
      if (official.has(h)) bad.push([path.relative(REPO, file), "与官方件逐字节相同：" + official.get(h) + " ⇒ 应改为引用而非复制"]);
    });
  }
}

// 规则 4（自检补）：**已构建的包，`dist` 必须是符号链接**（= 原子发布的「已转代」标志）。
// 由来：范围内四包均已转代，但没有判据守着它 —— 一旦有人把 dist 换成实体目录，
// 「永不半写」就只剩首转那个毫秒窗口，而这件事没人会发现。
const atomicPkgs = [];
const walkDist = (dir, depth) => {
  if (depth > 4) return;
  let entries = [];
  try { entries = fs.readdirSync(dir); } catch (e) { return; }
  for (const n of entries) {
    if (n === "node_modules" || n.startsWith(".")) continue;
    const sub = path.join(dir, n);
    let st; try { st = fs.lstatSync(sub); } catch (e) { continue; }
    if (st.isDirectory()) { if (fs.existsSync(path.join(sub, "package.json"))) atomicPkgs.push(sub); walkDist(sub, depth + 1); }
  }
};
for (const t of TREES) { const ui = path.join(REPO, t, "ui"); if (fs.existsSync(ui)) walkDist(ui, 0); }
for (const dir of atomicPkgs) {
  const d = path.join(dir, "dist");
  if (!fs.existsSync(d)) continue;
  // 修（独立验证者 r2-store-layers 揪出）：此处原为裸 `lstatSync`（未 import），
  // `catch { continue }` 会把 `ReferenceError: lstatSync is not defined` 一并吞掉 ⇒ 规则 4 从来不会报错
  // （撞 NEGATIVE_CHARTER 禁 6「一切失效可见」）。改用 `fs.lstatSync`，规则即刻生效。
  let st; try { st = fs.lstatSync(d); } catch (e) { bad.push([path.relative(REPO, dir), "dist 形态检查失败：" + String((e && e.message) || e)]); continue; }
  if (!st.isSymbolicLink()) bad.push([path.relative(REPO, dir), "dist 是实体目录（未转代）⇒ 原子发布的永不半写不成立；跑一次 build-wt-ui 即可转代"]);
}

console.log("check-tree-form: 检查 " + TREES.length + " 棵树（官方件指纹 " + official.size + " 个 · 已构建包 " + atomicPkgs.length + " 个）");
for (const n of notes) console.log("  · " + n);
if (bad.length) {
  for (const [f, why] of bad) console.log("  ✗ " + f + "：" + why);
  process.exit(1);
}
console.log("✓ check-tree-form 全绿：声明只用逻辑名 · ui/ 只放 mod 包 · 无官方件副本");
// 收尾显式退出（本脚本跑完仍有未释放句柄 ⇒ 会被自己的 deadline 判成「挂起」；红路径上面已 exit 1）
process.exit(0);
