// @ts-check
/**
 * rehearse-coldstart —— **冷启动自愈的可复跑演练**（把一次性证据变成入口）。
 *
 * 步骤：① 把目标包的产物（`dist*` / `.staging` / 戳）**移到备份目录** ⇒ "只剩源码"；
 * ② 重启宿主 ⇒ 断言：本次 map **没有**该模块键，且宿主日志出现 `[autoBuild]`；
 * ③ 轮询等后台编译完成（`dist/client/manifest.json` 存在 + 戳 `ok:true && published:true`）；
 * ④ 再重启宿主 ⇒ 断言 map **恢复**该键、资产可取 200；
 * ⑤ 跑 `verify-all --only judge,check-layers,swap-live` ⇒ 断言退 0；
 * ⑥ 删除备份（`--keep-backup` 保留）。
 *
 * 用法：node scripts/rehearse-coldstart.mjs [--pkg <包目录>] [--key <map 键前缀>] [--keep-backup]
 */
import { execSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const DEMO = path.resolve(import.meta.dirname, "..");
import { requireOrchestrationLock } from "./lib/orchestrate.mjs";
requireOrchestrationLock(DEMO, path.basename(process.argv[1] || "?"));
const REPO = path.resolve(DEMO, "../..");
const args = process.argv.slice(2);
const argOf = (n, d) => { const i = args.indexOf(n); return i >= 0 ? String(args[i + 1]) : d; };
const PKG = path.resolve(argOf("--pkg", path.join(REPO, "exp_swap/ui/swap")));
const KEY = argOf("--key", "exp_swap_swap/swap-a.jsx");
const keepBackup = args.includes("--keep-backup");
const PORT = Number(process.env.PORT || 5178);
const URL_PAGE = "http://127.0.0.1:" + PORT + "/demo/?wt=genesis";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const rows = [];
const step = (name, ok, detail) => { rows.push({ name, ok }); console.log("  " + (ok ? "✓" : "✗") + " " + name + (detail ? "  → " + detail : "")); };
const pageMap = async () => { try { const r = await fetch(URL_PAGE, { signal: AbortSignal.timeout(4000) }); return r.status === 200 ? await r.text() : ""; } catch (e) { return ""; } };
//  **容错**：冷态下首次重启可能超时就绪窗口（实测踩过）⇒ 不能让它抛异常中断演练；
// 后续的轮询与断言才是判据（restart-host 自己的退出码只作记录）。
const restart = () => { const r = spawnSync(process.execPath, ["scripts/restart-host.mjs"], { cwd: DEMO, encoding: "utf8" }); return r.status === 0 ? "ok" : "exit=" + r.status; };

console.log("== rehearse-coldstart（pkg=" + path.relative(REPO, PKG) + " · key=" + KEY + "）==");
//  宿主日志是**追加**的 ⇒ 先清空，否则"日志里出现 [autoBuild]"会命中**上一次**运行留下的行（弱断言）。
try { fs.rmSync(path.join(DEMO, ".tmp/host.log"), { force: true }); } catch (e) { /* 无所谓 */ }
const backup = fs.mkdtempSync(path.join(os.tmpdir(), "coldstart-bak-"));
const moved = [];
for (const n of fs.readdirSync(PKG)) {
  if (n === "dist" || n.startsWith("dist.") || n === ".staging" || n === ".esmx-build.json") {
    fs.renameSync(path.join(PKG, n), path.join(backup, n));
    moved.push(n);
  }
}
step("① 产物移走（只剩源码）", moved.length > 0, "移走 " + moved.length + " 项: " + moved.join(", "));

restart();
//  收紧：冷态下**页面也必须能起来**（200），只是**缺该键** —— 否则"无该键"会**空过**（第一版就踩了：
// 页面取不到时 `indexOf(KEY) < 0` 也成立 ⇒ 断言形同虚设）。
let cold = "";
for (let i = 0; i < 30; i += 1) { cold = await pageMap(); if (cold) break; await sleep(1000); }
// **如实断言当前契约**（收紧判据时查明）：冷态下要么"服务但缺该键"，要么"根本不服务"
//（esmx 在 `uses` 模块缺 `dist` 时启动失败 ⇒ 端口无响应），两种都算"冷态成立"；
// **无论哪种，`[autoBuild]` 都必须被踢起来**（②b 断言）。若将来 esmx 能带缺件启动，这里会变成"服务但缺键"，脚本无须改。
// **新期望（预构建落地后）**：`restart-host` 会先 `ensure-built` ⇒ 冷态**一步到位**：页面 200 且键在。
// （旧行为：宿主启动失败 + `[autoBuild]` 兜底 + 再重启一次 —— 见 §41.2/§42。）
step("② 冷态重启后：页面 200 且键已在（预构建生效）", cold.length > 0 && cold.indexOf(KEY) >= 0, cold ? "页面 " + cold.length + " 字节、键在" : "页面 30s 内未起来");
const log = (() => { try { return fs.readFileSync(path.join(DEMO, ".tmp/host.log"), "utf8"); } catch (e) { return ""; } })();
// **新契约（预构建生效后）**：宿主启动时该包**已经就绪** ⇒ 日志里**不该**再出现 `[autoBuild]`，
// 也不该出现"只有源码 / 产物不可信"告警（旧行为是这两条都会出现，见 §41.2）。
const kicked = log.indexOf("[autoBuild]") >= 0;
const warned = /只有源码|产物不可信/.test(log);
step("②b 预构建生效：无 [autoBuild]、无缺件告警", !kicked && !warned, kicked ? "仍出现 [autoBuild]" : (warned ? "仍出现缺件告警" : "日志干净"));

let built = false;
for (let i = 0; i < 40; i += 1) {
  const man = path.join(PKG, "dist/client/manifest.json");
  let stamp = null;
  try { stamp = JSON.parse(fs.readFileSync(path.join(PKG, ".esmx-build.json"), "utf8")); } catch (e) { /* 还没有 */ }
  if (fs.existsSync(man) && stamp && stamp.ok === true && stamp.published === true) { built = true; break; }
  await sleep(1000);
}
step("③ 产物齐 + 戳 ok/published（预构建产物；旧路径下这一步等的是 [autoBuild]）", built);

restart();
const warm = await pageMap();
step("④ 重启后 map 恢复该键", warm.indexOf(KEY) >= 0);
const m = new RegExp("/" + KEY.replace(/[.*+?^${}()|[\]\\]/g, "\\$&") + "\\.[A-Za-z0-9_-]+\\.final\\.mjs").exec(warm);
let assetOk = false;
if (m) { try { const r = await fetch("http://127.0.0.1:" + PORT + m[0], { signal: AbortSignal.timeout(4000) }); assetOk = r.status === 200; } catch (e) { assetOk = false; } }
step("④b 资产可取 200", assetOk, m ? m[0] : "URL 未找到");

const va = spawnSync(process.execPath, ["scripts/verify-all.mjs", "--only", "judge,check-layers,swap-live"], { cwd: DEMO, encoding: "utf8" });
step("⑤ verify-all（judge/check-layers/swap-live）退 0", va.status === 0, ((va.stdout || "").split(String.fromCharCode(10)).filter((l) => l.trim()).pop() || "").trim().slice(0, 80));

if (keepBackup) console.log("  · 备份保留在 " + backup);
else { fs.rmSync(backup, { recursive: true, force: true }); console.log("  · 备份已删（系统已自愈）"); }
const bad = rows.filter((r) => !r.ok).length;
console.log(bad ? "✗ rehearse-coldstart 失败 " + bad + " 项" : "✓ rehearse-coldstart 全绿（冷启动自愈闭环）");
process.exit(bad ? 1 : 0);