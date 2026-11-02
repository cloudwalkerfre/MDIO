// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-esmx-validate");
armWatchdog("check-esmx-validate", 10000);   /* = 10s；实测 0.3s（扫描 1 跑）⇒ ×2 */
/**
 * check-esmx-validate —— 官方免费检查器进判据（llms.md：「声明对不对由它的退出码裁定，而不是靠人读」）。
 *
 * `esmx validate` 是**免构建 dry run**：挂载遍历 · 传递 uses · 版本检查 · 供给表 · 单一所有者（E_DUP_PROVIDER）。
 * 它不跑阶段 3（打包器才发的 E_NOT_USED/E_NO_EXPORT）、不做类型检查 ⇒ 它是第一道门，不是全部预言机。
 */
import { spawn } from "node:child_process";
import path from "node:path";
const DEMO = path.resolve(import.meta.dirname, "..");
const p = spawn(path.join(DEMO, "node_modules/.bin/esmx"), ["validate", "--json"], { cwd: DEMO });
let out = "";
p.stdout.on("data", (d) => { out += d; });
p.stderr.on("data", (d) => { out += d; });
p.on("close", (code) => {
  let diags = null;
  try { diags = JSON.parse(out).diagnostics; } catch (e) { /* 非 JSON ⇒ 视为失败 */ }
  if (code !== 0 || !Array.isArray(diags)) {
    console.log("✗ check-esmx-validate：退出码 " + code + " · 输出 " + out.slice(0, 240));
    process.exit(1);
  }
  if (diags.length) {
    console.log("✗ check-esmx-validate：diagnostics " + diags.length + " 条");
    for (const d of diags.slice(0, 6)) console.log("   " + JSON.stringify(d).slice(0, 170));
    process.exit(1);
  }
  console.log("✓ check-esmx-validate 全绿（diagnostics=0 · 挂载/uses/版本/供给表/单一所有者 全部通过）");
  process.exit(0);   // close 回调里显式退：否则 watchdog 计时器会把进程吊到 deadline（判 hang/exit 2）
});

// 收尾：**不要**在模块末尾 process.exit —— 本脚本只 spawn 子进程，结果在 close 回调里才到；
// 提前 exit 会让它永远不打印结论（verify-all 里恒红的根因）。
