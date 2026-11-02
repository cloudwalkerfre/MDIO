// @ts-check
/**
 * rehearse-new-tree —— **"从零建一棵新树"的接入演练**。
 *
 * 它建一棵**仓库根下的新树**（`<REPO>/neotree_rehearsal/`，含 `wt/` 与 `ui/`），然后逐件验证"接入清单"：
 *   ① 树的**包形态 mod** 能被宿主发现（`discoverWtModules`）；
 *   ② `sync-wt-uses` 为它建三处协议条目（uses / link / node_modules）；
 *   ③ 它能被构建（`build-wt-ui`，原子发布）；
 *   ④ **服务侧边界实证**：网关对它的 `roots` 是 `[]` ⇒ 其 **ui 资产不由网关供出**
 *      （因为它不在**挂载根**之内）⇒ "新树要能被浏览器装载 ui 件"需要**一行网关配置**（把它加进挂载）。
 * ⑤ 清场：删树 + 回滚协议条目 + 重启 ⇒ 判据回绿。
 *
 * 用法：node scripts/rehearse-new-tree.mjs [--keep]
 */
import { execSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
import { requireOrchestrationLock } from "./lib/orchestrate.mjs";
const REPO = path.resolve(DEMO, "../..");
const keep = process.argv.includes("--keep");
const TREE = "neotree_rehearsal";
const PKG = "neotree_mod";
const root = path.join(REPO, TREE);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const rows = [];
const step = (name, ok, detail) => { rows.push({ name, ok }); console.log("  " + (ok ? "✓" : "✗") + " " + name + (detail ? "  → " + detail : "")); };

// **C2（审查）修正**：清场挂在 exit / SIGINT / SIGTERM 上（本演练改的是**注册表**，被中断就会留下多余登记行）。
// **C3（第三份审查）**：取锁放在**注册清场之后**（exit 处理器按注册序跑 ⇒ 先清场、后放锁）。
let __cleaned = false;
let __cleanup = () => {};
requireOrchestrationLock(DEMO, path.basename(process.argv[1] || "?"));
const onExit = () => { try { __cleanup(); } catch (e) { /* 忽略 */ } };
process.once("exit", onExit);
for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => { onExit(); process.exit(130); });

//  **清场必须在"任何改动之前"就装好**（中断测试教我的）：第一版把 `__cleanup` 的赋值放在脚本后段，
// 于是"早于赋值的中断"调到的是空函数 ⇒ 残渣全留（实测：树目录 / 注册表行 / uses 条目 / 链接都还在）。
const REG = path.join(REPO, "worktrees/genesis/wt/.mds/worktrees.md");
const regBefore = fs.existsSync(REG) ? fs.readFileSync(REG, "utf8") : "";
const regLine = "- path: " + root;
__cleanup = () => {
  if (__cleaned) return; __cleaned = true;
  try {
    fs.rmSync(root, { recursive: true, force: true });
    if (fs.existsSync(REG)) fs.writeFileSync(REG, fs.readFileSync(REG, "utf8").split(String.fromCharCode(10)).filter((l) => l.trim() !== regLine).join(String.fromCharCode(10)));
    execSync("node scripts/sync-wt-uses.mjs", { cwd: DEMO, stdio: "ignore" });
  } catch (e) { /* 清场失败不能掩盖原错误 */ }
};

console.log("== rehearse-new-tree（" + TREE + "）==");
fs.mkdirSync(path.join(root, "wt/.mds"), { recursive: true });
fs.mkdirSync(path.join(root, "ui", PKG, "src"), { recursive: true });
fs.writeFileSync(path.join(root, "wt/.mds/ui.md"), [
  "# 新树演练",
  "",
  "## mods",
  "- NEW_TREE: " + PKG + "/mod.js",
  "- NEW_TREE_SINGLE: .ui::neotree_single.js",
  "",
].join(String.fromCharCode(10)));
fs.writeFileSync(path.join(root, "ui/neotree_single.js"), "console.log(\"NEOTREE_SINGLE\");\nexport const v = 1;\n");
// **零配置形态**（收口，与 rehearse-onboarding 同一口径）：
//   第三方**只写** name / esmx.uses（用宿主哪份基础件）/ esmx.exports；
//   **不写** `dependencies`（react 由宿主供给，写了反而像"自带一份"）、**不写** `devDependencies`
//   （esmx/vite 由宿主出：`build-wt-ui` 从 demo 解析 @esmx/*，实测零依赖包构建 54ms 通过）。
//   `uses` 必须留：实测去掉它 ⇒ React 被内联进产物（单例破），见 rehearse-onboarding ①d 判据。
fs.writeFileSync(path.join(root, "ui", PKG, "package.json"), JSON.stringify({
  name: PKG, version: "0.0.0", private: true, type: "module",
  esmx: { uses: ["ui_base"], exports: { "./mod.js": "./src/mod.js" } },
}, null, 2) + String.fromCharCode(10));
fs.writeFileSync(path.join(root, "ui", PKG, "src/mod.js"), "import { createElement as h } from \"react\";\nexport default function P() { return h(\"span\", null, \"x\"); }\n");
// ①b **注册**：发现走的是注册表 `wt/.mds/worktrees.md`（形如 `- path: <项目根>`）；
//     没有注册表时才会退回"扫盘发现"并告警 ⇒ **新树必须登记**，否则 sync/宿主都看不见它。
// （`REG` / `regBefore` / `regLine` 已在上方早期定义 —— 清场需要它们，见那条注释。）
if (regBefore.indexOf(regLine) < 0) fs.writeFileSync(REG, (regBefore.replace(/\s*$/, "") + String.fromCharCode(10) + regLine + String.fromCharCode(10)));
// ①c **清单第 3 步**（本轮自动化）：树级 `ui/node_modules` 的共享依赖落点。
//     `mdio_ui` 由 `ensure-tree-deps` 补（幂等）；`react`/`react-dom` 真接入应"在该树 ui/ 下 pnpm install"，
//     演练里**从 genesis 的 ui/node_modules 借**（同版本、同规格），以免动到"安装"这件需要批准的事。
const td = spawnSync(process.execPath, ["scripts/ensure-tree-deps.mjs"], { cwd: DEMO, encoding: "utf8" });
const nmUi = path.join(root, "ui/node_modules");
fs.mkdirSync(nmUi, { recursive: true });
const donor = path.join(REPO, "worktrees/genesis/ui/node_modules");
for (const d of ["react", "react-dom"]) {
  const dst = path.join(nmUi, d);
  if (!fs.existsSync(dst) && fs.existsSync(path.join(donor, d))) { try { fs.symlinkSync(path.join(donor, d), dst, "dir"); } catch (e) { /* 已存在 */ } }
}
step("① 建树 + 登记注册表 + 依赖落点（mdio_ui 链接/借 react）",
  fs.existsSync(path.join(root, "wt/.mds/ui.md")) && fs.readFileSync(REG, "utf8").indexOf(regLine) >= 0 && fs.existsSync(path.join(nmUi, "ui_base")),
  "td=" + String(td.stdout || "").split(String.fromCharCode(10)).filter((l) => l.indexOf("✓") >= 0).pop()?.trim().slice(0, 50));

// ② 发现 + 协议条目
//  `node -e` 的 `process.argv[1]` 是**第一个用户参数** ⇒ 只能传一个（根目录）；第一版多传了一个 ⇒ 探到了错的根。
const disc = spawnSync(process.execPath, ["-e", "import('./src/std/host/wtModules.ts').then(m=>{const d=m.discoverWtModules(process.argv[1]);console.log(Object.keys(d).join(','))})", REPO], { cwd: DEMO, encoding: "utf8" });
const keys = String(disc.stdout || "").trim();
step("② 宿主发现该树的包（discoverWtModules）", keys.indexOf(PKG) >= 0, "发现：" + keys.slice(0, 110));
// ③ 先构建（**顺序要紧**：`sync` 只为"已构建"的包建 `uses` 条目 —— 未构建的包按设计不进 uses）。
const b = spawnSync(process.execPath, ["scripts/build-wt-ui.mjs", path.join(root, "ui", PKG)], { cwd: DEMO, encoding: "utf8" });
step("③ 构建该包（原子发布）", b.status === 0, b.status === 0
  ? (String(b.stdout || "").split(String.fromCharCode(10)).filter((l) => l.indexOf("[build-wt-ui]") >= 0).pop() || "").trim().slice(0, 80)
  : String(b.stderr || b.stdout || "").split(String.fromCharCode(10)).filter((l) => l.trim()).slice(-2).join(" · ").slice(0, 120));

// ④ sync ⇒ 三处条目
const sy = spawnSync(process.execPath, ["scripts/sync-wt-uses.mjs"], { cwd: DEMO, encoding: "utf8" });
const pj = JSON.parse(fs.readFileSync(path.join(DEMO, "package.json"), "utf8"));
const inUses = (pj.esmx.uses || []).includes(PKG);
const linked = fs.existsSync(path.join(DEMO, "node_modules", PKG));
step("④ sync 建三处条目（uses/link/node_modules）", inUses && linked, "uses=" + inUses + " · node_modules=" + linked);

// ⑤ 服务侧边界实证
let chain = "";
try { const r = await fetch("http://127.0.0.1:8011/control/wt/chain?uid=" + TREE, { signal: AbortSignal.timeout(5000) }); chain = await r.text(); } catch (e) { /* 网关不在也算 */ }
const rootsEmpty = /"roots"\s*:\s*\[\s*\]/.test(chain);
step("⑤ 边界实证：网关对新树的 roots 为空（ui 资产不由网关供出）", rootsEmpty, chain.slice(0, 90) || "取不到");
const asset = (() => { try { return execSync("curl -s -o /dev/null -w '%{http_code}' --max-time 6 http://127.0.0.1:5178/ui-assets/" + TREE + "/neotree_single.js", { encoding: "utf8" }); } catch (e) { return "ERR"; } })();
step("⑤b 资产路由该树 → 期望 404/非 200（未挂载）", asset !== "200", "HTTP " + asset);

// 清场：幂等函数已在**脚本早期**定义（见那条注释）；这里只负责"调用 + 断言"。
if (!keep) {
  __cleanup();
  const after = JSON.parse(fs.readFileSync(path.join(DEMO, "package.json"), "utf8"));
  step("⑥ 清场：树已删 + 协议条目已回滚", !fs.existsSync(root) && !(after.esmx.uses || []).includes(PKG));
}
const bad = rows.filter((r) => !r.ok).length;
console.log(bad ? "✗ rehearse-new-tree 失败 " + bad + " 项" : "✓ rehearse-new-tree 全绿：新树接入清单已逐条验证（含服务侧边界）");
process.exit(bad ? 1 : 0);