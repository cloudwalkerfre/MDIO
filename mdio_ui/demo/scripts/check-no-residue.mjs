// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-no-residue");
armWatchdog("check-no-residue", 10000);   /* = 10s；实测 0.2s（扫描 1 跑）⇒ ×2 */
/**
 * check-no-residue —— **"探针/演练不留残渣"的判据**。
 *
 * 由来：本 session 里我两次把临时 fixture 残留在工作树里（`astprobe.md` 被写满公式、
 * `mdio_math_probe.md` 写过一版、`__stray_probe.js` 反向测试用），都是**手工才发现**的。
 * 把"用完即还原"变成判据后，将来任何一轮跑完都能自动查。
 *
 * 规则：
 *   1. `worktrees/genesis/wt/astprobe.md` —— 要么不存在，要么**0 字节**（它本是空文件，被我们当公式 fixture 用过）；
 *   2. `worktrees/genesis/wt/mdio_math_probe.md` —— **不得存在**（曾用过的临时文档）；
 *   3. 任何工作树的 `ui/` 顶层**不得**有 `__*probe*` / `*_probe.*` 形态的文件（反向测试留下的）；
 *   4. `mdio_ui/demo/.tmp/` 下不得有 `coldstart-bak-*`（演练备份目录；正常结束会自行删除）。
 *
 * 用法：node scripts/check-no-residue.mjs
 */
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
const REPO = path.resolve(DEMO, "../..");
const bad = [];

// 1 / 2：两个具名 fixture 的期望状态
const ast = path.join(REPO, "worktrees/genesis/wt/astprobe.md");
if (fs.existsSync(ast)) {
  const sz = fs.statSync(ast).size;
  if (sz !== 0) bad.push(["worktrees/genesis/wt/astprobe.md", "应为 0 字节（曾是空文件），实际 " + sz + " 字节"]);
}
const mathProbe = path.join(REPO, "worktrees/genesis/wt/mdio_math_probe.md");
if (fs.existsSync(mathProbe)) bad.push(["worktrees/genesis/wt/mdio_math_probe.md", "临时文档不得留存"]);

// 3：各工作树 ui/ 顶层的探针文件
const wtRoot = path.join(REPO, "worktrees");
const uiRoots = [];
if (fs.existsSync(wtRoot)) for (const n of fs.readdirSync(wtRoot)) uiRoots.push(path.join(wtRoot, n, "ui"));
for (const n of fs.readdirSync(REPO)) uiRoots.push(path.join(REPO, n, "ui"));
// C4（第三份审查）：**官方包也要覆盖** —— `mdio_ui/lib/mods` 及其子包在扫描范围内，
// 而"代际 ≤ 2"这条恰恰要守着它们（也正是待批 ① 要动的两个包）。
const libModsRoot = path.join(REPO, "mdio_ui/lib/mods");
if (fs.existsSync(libModsRoot)) {
  uiRoots.push(libModsRoot);
  for (const n of fs.readdirSync(libModsRoot)) {
    if (n.startsWith(".") || n === "node_modules" || n === "dist" || n.startsWith("dist.") || n === "src") continue;
    const sub = path.join(libModsRoot, n);
    try { if (fs.statSync(sub).isDirectory()) uiRoots.push(sub); } catch (e) { /* 跳过 */ }
  }
}
for (const ui of uiRoots) {
  if (!fs.existsSync(ui)) continue;
  for (const n of fs.readdirSync(ui)) {
    if (/^__.*probe/i.test(n) || /_probe\.[a-z]+$/.test(n)) bad.push([path.relative(REPO, path.join(ui, n)), "探针残留（反向测试/临时件）"]);
  }
}

// 4：演练备份目录（正常结束会删）
const tmp = path.join(DEMO, ".tmp");
if (fs.existsSync(tmp)) {
  for (const n of fs.readdirSync(tmp)) if (n.indexOf("coldstart-bak-") === 0) bad.push([".tmp/" + n, "演练备份目录残留（--keep-backup 时应手工清理）"]);
}
// 5（审查 C4 新增）：**注册表里的登记行必须指向存在的树** —— 演练被中断会留下多余 `- path:` 行，
//    而它比"探针文件"更隐蔽（宿主会认为那棵树存在）。
const REGS = [path.join(REPO, "worktrees/genesis/wt/.mds/worktrees.md"), path.join(REPO, ".mds/worktrees.md")];
for (const reg of REGS) {
  if (!fs.existsSync(reg)) continue;
  for (const line of fs.readFileSync(reg, "utf8").split(String.fromCharCode(10))) {
    const m = /^-\s*path:\s*(.+)$/.exec(line.trim());
    if (!m) continue;
    const p = m[1].trim();
    if (!fs.existsSync(p)) bad.push([path.relative(REPO, reg), "登记行指向不存在的树：" + p]);
  }
}

// 6（审查 C4 新增）：**代际数不得超过 2**（原子发布"保留两代"的不变量；多余代际＝发布/清理半途而废）。
for (const ui of uiRoots) {
  if (!fs.existsSync(ui)) continue;
  const walkPkg = (dir) => {
    let gens = 0;
    try {
      for (const n of fs.readdirSync(dir)) {
        if (n === "node_modules" || n === ".staging" || n.startsWith(".")) continue;
        const f = path.join(dir, n);
        try { if (fs.statSync(f).isDirectory()) walkPkg(f); } catch (e) { /* 忽略 */ }
      }
      gens = fs.readdirSync(dir).filter((n) => /^dist\.gen/.test(n)).length;
    } catch (e) { return; }
    if (gens > 2) bad.push([path.relative(REPO, dir), "代际目录 " + gens + " 个（>2，违反「保留两代」的清理不变量）"]);
  };
  walkPkg(ui);
}

// 另外看一眼系统临时目录里是否有上一轮的 lab（只提示，不判红）
const stray = fs.readdirSync(os.tmpdir()).filter((n) => n.indexOf("vp-cache-lab-") === 0);

console.log("check-no-residue: 检查 " + uiRoots.filter((p) => fs.existsSync(p)).length + " 个 ui 目录 + 2 个具名 fixture");
if (stray.length) console.log("  · 提示：/tmp 下有 " + stray.length + " 个缓存回归 lab（check-vp-cache 自建，可随时 rm）");
if (bad.length) {
  for (const [f, why] of bad) console.log("  ✗ " + f + "：" + why);
  process.exit(1);
}
console.log("✓ check-no-residue 全绿：无探针残留、fixture 已还原");
// 收尾显式退出（本脚本跑完仍有未释放句柄 ⇒ 被自己的 deadline 判成「挂起」）
process.exit(0);
