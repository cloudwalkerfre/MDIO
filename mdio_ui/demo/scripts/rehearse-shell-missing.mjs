// @ts-check
/**
 * rehearse-shell-missing —— **J1「缺件降级」判据**（业主要求原文：挂载表里有产物已被删的项时，
 *   页面**必须 200**，且日志出现 **shell 缺件告警**）。
 *
 * 为什么自持宿主（不能蹭别人的）：告警按**进程**去重 —— `entry.node.ts` 的 `pruneDeadLinks()` 用
 *   `pruned` 集合保证同名只喊一次；蹭别人的宿主 ⇒ 第二次跑就看不到告警 ⇒ 判据变成"时绿时红"。
 *   本演练自己重启宿主 ⇒ 每次都是干净进程 ⇒ 告警必定出现。
 *
 * 步骤：① 找一件已构建的 mod 产物（`<tree>/ui/<mod>/dist/client/manifest.json`）并备份；
 *      ② 重启宿主（此刻产物齐全 ⇒ 起一个干净进程）；③ 记下宿主日志长度，再**拿走** manifest
 *      （模拟运行期被删：演练清场窗口、手工删目录）；④ `GET /?wt=genesis` ⇒ 必须 **200** 且是壳页面；
 *      ⑤ 从**记下的偏移**往后读日志 ⇒ 必须出现 `[shell] 缺件：`；⑥ 还原产物 + 重启宿主收场。
 *
 * 用法：node scripts/rehearse-shell-missing.mjs [--keep]
 */
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
import { requireOrchestrationLock } from "./lib/orchestrate.mjs";
const REPO = path.resolve(DEMO, "../..");
const keep = process.argv.includes("--keep");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const rows = [];
const step = (name, ok, detail) => { rows.push({ name, ok }); console.log("  " + (ok ? "✓" : "✗") + " " + name + (detail ? "  → " + detail : "")); };

/** 清场挂在 exit / SIGINT / SIGTERM 上，且**在任何改动之前**装好（本演练动的是真产物文件）。 */
let __cleaned = false;
let __cleanup = () => {};
requireOrchestrationLock(DEMO, path.basename(process.argv[1] || "?"));
const onExit = () => { try { __cleanup(); } catch (e) { /* 忽略 */ } };
process.once("exit", onExit);
for (const sig of ["SIGINT", "SIGTERM"]) process.once(sig, () => { onExit(); process.exit(130); });

/** 找一件已构建的 mod 产物（挑第一个带 dist/client/manifest.json 的）。 */
function findArtifact() {
  const trees = path.join(REPO, "worktrees");
  if (!fs.existsSync(trees)) return null;
  // **优先挑被网关挂载的那棵树**（genesis）：只有挂载树里的 mod 才会进宿主的挂载表，
  // 拿别的树的产物演练 ⇒ 永远不会被 prune ⇒ 告警不出现（假红）。
  const all = fs.readdirSync(trees);
  const order = all.includes("genesis") ? ["genesis"].concat(all.filter((t) => t !== "genesis")) : all;
  for (const t of order) {
    const ui = path.join(trees, t, "ui");
    if (!fs.existsSync(ui)) continue;
    for (const m of fs.readdirSync(ui)) {
      const f = path.join(ui, m, "dist", "client", "manifest.json");
      if (fs.existsSync(f)) return { mod: m, tree: t, file: f };
    }
  }
  return null;
}

const art = findArtifact();
if (!art) { console.log("✗ rehearse-shell-missing：找不到已构建的 mod 产物（先跑 build-wt-ui）"); process.exit(1); }
const bak = art.file + ".j1bak";
const LOG = path.join(DEMO, ".tmp", "host.log");
let moved = false;
const restartHost = () => spawnSync(process.execPath, ["scripts/restart-host.mjs"], { cwd: DEMO, stdio: "inherit" });

__cleanup = () => {
  if (__cleaned) return; __cleaned = true;
  try { if (moved && fs.existsSync(bak)) fs.renameSync(bak, art.file); } catch (e) { /* 忽略 */ }
  // 收场要把宿主恢复到"产物齐全"的状态，否则后续套件会看到被摘掉的模块。
  // （`--keep` 跳过这一步：留给人工看当时的日志；产物**仍然**照上面还原。）
  if (!keep) { try { restartHost(); } catch (e) { /* 忽略 */ } }
};

console.log("rehearse-shell-missing：拿 " + art.tree + "/ui/" + art.mod + " 的 client manifest 做缺件演练");
restartHost();
const logOffset = fs.existsSync(LOG) ? fs.statSync(LOG).size : 0;
fs.renameSync(art.file, bak); moved = true;
step("拿走产物 manifest（模拟运行期被删）", !fs.existsSync(art.file), art.mod);

let status = 0; let html = "";
try {
  const r = await fetch("http://127.0.0.1:5178/?wt=genesis", { signal: AbortSignal.timeout(20000) });
  status = r.status; html = await r.text();
} catch (e) { status = 0; }
step("页面必须 200（缺件不得把整页打成 500）", status === 200, "HTTP " + status);
step("返回的是**壳页面**（不是错误页）", html.indexOf('<script type="importmap">') >= 0, html.length + "B");

await sleep(600);   // 日志落盘
let tail = "";
try { const fd = fs.openSync(LOG, "r"); const size = fs.statSync(LOG).size; const len = Math.max(0, size - logOffset); const buf = Buffer.alloc(len); fs.readSync(fd, buf, 0, len, logOffset); fs.closeSync(fd); tail = buf.toString("utf8"); } catch (e) { tail = ""; }
step("宿主日志出现「shell 缺件」告警", tail.indexOf("[shell] 缺件：") >= 0, tail.indexOf("[shell] 缺件：") >= 0 ? String(tail.split("[shell] 缺件：")[1] || "").split(String.fromCharCode(10))[0].slice(0, 70) : "（本段日志无该行）");

__cleanup();
step("还原产物 + 重启宿主（判据回到产物齐全）", fs.existsSync(art.file) && !fs.existsSync(bak), art.mod);
const bad = rows.filter((r) => !r.ok);
if (bad.length) { console.log("✗ rehearse-shell-missing 失败 " + bad.length + " 项：" + bad.map((b) => b.name).join(" · ")); process.exit(1); }
console.log("✓ rehearse-shell-missing 全绿：缺件时页面 200 且日志有 shell 缺件告警（原 " + art.tree + "/ui/" + art.mod + "，已还原）");
if (keep) console.log("（--keep：产物已还原，仅**跳过收场重启**，便于人工查看当时的宿主日志）");