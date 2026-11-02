#!/usr/bin/env node
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-links");
armWatchdog("check-links", 10000   /* = 10s；实测 0.1s（另一次跑飞被截断） */)
/**
 * check-links —— **比"按名字 grep"聪明一层**：查所有 `link:` 依赖与 node_modules 符号链接的**真实指向**。
 *
 * 为什么需要它（真实事故）：家族改名时按名字的替换规则**抓不到相对路径值** ——
 * `ui_devview/package.json` 里的 `"ui_core": "link:../core"` 不含 `lib/mods`，任何名字规则都漏，
 * 于是它一直指向已不存在的 `../core`，而所有判据全绿（没人解析过这个路径）。
 *
 * 三项检查（每项都报**扫描量**，防止"扫了个空目录还判绿"——同日 check-externals 就栽在这上面）：
 *   ① `link:<rel>` 目标必须**存在**，且目标的 `package.json.name` 必须**等于依赖名**（名不副实也算错）；
 *   ② 该包的 `node_modules/<依赖名>` 必须是符号链接、且**指向同一目标**（缺链 / 指向过期）；
 *   ③ 任何 `node_modules` 里的**悬空符号链接**（目标不存在）—— 改名/搬目录后的典型残留。
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const DEMO = fileURLToPath(new URL("..", import.meta.url));
const REPO = path.resolve(DEMO, "../..");
const SKIP = new Set(["node_modules", "dist", ".git", ".tmp", ".pnpm"]);

/** 找出所有「包目录」（含 package.json 的目录），跳过 node_modules / dist / 隐藏目录。 */
function findPackages(root, depth = 0, out = []) {
  if (depth > 6) return out;
  let entries = [];
  try { entries = fs.readdirSync(root, { withFileTypes: true }); } catch (e) { return out; }
  const names = new Set(entries.map((e) => e.name));
  if (names.has("package.json")) out.push(root);
  for (const e of entries) {
    if (!e.isDirectory() || SKIP.has(e.name) || e.name.startsWith(".")) continue;
    findPackages(path.join(root, e.name), depth + 1, out);
  }
  return out;
}

const readJson = (p) => { try { return JSON.parse(fs.readFileSync(p, "utf8")); } catch (e) { return null; } };
const realish = (p) => { try { return fs.realpathSync(p); } catch (e) { return null; } };

const roots = [
  path.join(REPO, "mdio_ui/lib/mods"),
  path.join(REPO, "mdio_ui/demo"),
  path.join(REPO, "mdio_ui/vscode-ext"),
];
for (const wt of ["worktrees", "exp_app", "exp_swap", "exp_pages"]) {
  const d = path.join(REPO, wt);
  if (fs.existsSync(d)) roots.push(d);
}

const bad = [];
const pkgs = [];
for (const r of roots) for (const p of findPackages(r)) if (!pkgs.includes(p)) pkgs.push(p);

let linkCount = 0, nmCount = 0, dangling = 0;
for (const dir of pkgs) {
  const j = readJson(path.join(dir, "package.json"));
  if (!j) continue;
  const rel = path.relative(REPO, dir) || ".";
  for (const field of ["dependencies", "devDependencies", "optionalDependencies"]) {
    for (const [name, spec] of Object.entries(j[field] || {})) {
      if (typeof spec !== "string" || !spec.startsWith("link:")) continue;
      linkCount += 1;
      const target = path.resolve(dir, spec.slice(5));
      if (!fs.existsSync(target)) { bad.push([rel, name, spec, "link 目标不存在：" + path.relative(REPO, target)]); continue; }
      const tj = readJson(path.join(target, "package.json"));
      if (!tj) { bad.push([rel, name, spec, "link 目标没有 package.json"]); continue; }
      if (tj.name !== name) { bad.push([rel, name, spec, "名不副实：目标包名是 " + tj.name]); continue; }
      const link = path.join(dir, "node_modules", name);
      let st = null; try { st = fs.lstatSync(link); } catch (e) { st = null; }
      if (!st) { bad.push([rel, name, spec, "node_modules 里缺链接（跑 ensure-tree-deps 补）"]); continue; }
      if (!st.isSymbolicLink()) { bad.push([rel, name, spec, "node_modules/" + name + " 不是符号链接"]); continue; }
      const a = realish(link), b = realish(target);
      if (!a || !b || a !== b) { bad.push([rel, name, spec, "链接指向过期：" + fs.readlinkSync(link)]); continue; }
    }
  }
  const nm = path.join(dir, "node_modules");
  let entries = [];
  try { entries = fs.readdirSync(nm, { withFileTypes: true }); } catch (e) { entries = []; }
  for (const e of entries) {
    const p = path.join(nm, e.name);
    let st = null; try { st = fs.lstatSync(p); } catch (err) { continue; }
    if (!st.isSymbolicLink()) {
      if (e.isDirectory() && e.name.startsWith("@")) {
        let subs = []; try { subs = fs.readdirSync(p, { withFileTypes: true }); } catch (err) { subs = []; }
        for (const s of subs) {
          const sp = path.join(p, s.name);
          let sst = null; try { sst = fs.lstatSync(sp); } catch (err) { continue; }
          if (!sst.isSymbolicLink()) continue;
          nmCount += 1;
          if (!fs.existsSync(sp)) { dangling += 1; bad.push([rel + "/node_modules", e.name + "/" + s.name, "", "悬空符号链接"]); }
        }
      }
      continue;
    }
    nmCount += 1;
    if (!fs.existsSync(p)) { dangling += 1; bad.push([rel + "/node_modules", e.name, "", "悬空符号链接 → " + fs.readlinkSync(p)]); }
  }
}

console.log("check-links: 扫 " + pkgs.length + " 个包 · link: 条目 " + linkCount + " · node_modules 符号链接 " + nmCount);
if (bad.length) {
  console.log("✗ check-links 发现 " + bad.length + " 处问题（含悬空 " + dangling + " 条）：");
  for (const [pkg, name, spec, why] of bad.slice(0, 20)) console.log("   " + pkg + " → " + name + (spec ? " (" + spec + ")" : "") + "：" + why);
  if (bad.length > 20) console.log("   …另有 " + (bad.length - 20) + " 处");
  process.exit(1);
}
if (linkCount === 0 && nmCount === 0) {
  console.log("✗ check-links：扫描量为 0 ⇒ 判据空跑（路径写错或目录结构变了），按失败处理");
  process.exit(1);
}
console.log("✓ check-links 全绿：link: 目标真实存在且名实相符 · 符号链接无悬空");
process.exit(process.exitCode === undefined ? 0 : process.exitCode);   // 收尾退出：结论已打印 ⇒ 立刻退（CDP socket 会把进程吊到 deadline）