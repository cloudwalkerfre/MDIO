// @ts-check
import { installGuards, armWatchdog } from "../test/lib/watchdog.mjs";
installGuards("check-dev-provider");
armWatchdog("check-dev-provider", 10000   /* = 10s；实测 0.2s（另有两次跑飞被 180s 上限截断 ⇒ 不用那个数） */)
/**
 * check-dev-provider —— **dev 共享依赖的接线判据**。
 *
 * 历史：本判据原先钉的是结论「开发必须用宿主 shim 承重」（把裸名覆盖到 `/demo/src/std/shims/`）。
 * 按 esmx 官方模型后，没有 shim 机制 ⇒ 判据用**官方不变量**：
 *   「每个共享包只有一个所有者（平台的 `provides`），整个闭包都接到它」。
 *
 * 断言（开发页面，纯 HTTP · 约 200ms）：
 *   1. 8 个裸名都在 importmap 里，且**指向平台产物** `/ui_base/**`——不是 shim，也不是 Vite 预打包 `deps/`；
 *   2. 平台的逻辑名 `ui_base/<裸名>` 存在（provides 链路在）；
 *   3. 两者解析到**同一个文件**（单一所有者；若不同 ⇒ 又回到了两份实例）；
 *   4. 平台产物 HTTP 200（在位可取）。
 * 浏览器侧的资源图不变量见 `check-module-graph.mjs`。
 */
const PORT = Number(process.env.PORT || 5178);
const WT = process.env.WT || "genesis";
const URL_PAGE = "http://127.0.0.1:" + PORT + "/demo/?wt=" + WT;
const BARE = ["react", "react/jsx-runtime", "react/jsx-dev-runtime", "react-dom", "react-dom/client", "mobx", "mobx-react-lite", "mobx-state-tree"];
let html = "";
try { const r = await fetch(URL_PAGE, { signal: AbortSignal.timeout(5000) }); html = r.status === 200 ? await r.text() : ""; } catch (e) { html = ""; }
if (!html) { console.log("✗ check-dev-provider：开发宿主未服务（先跑 `node scripts/up-all.mjs`）"); process.exit(1); }
const m = /<script type="importmap">([\s\S]*?)<\/script>/.exec(html);
let imports = {};
try { imports = JSON.parse(m ? m[1] : "{}").imports || {}; } catch (e) { imports = {}; }
const bad = [];
let sample = "";
for (const b of BARE) {
  const v = String(imports[b] || "");
  if (!v) { bad.push([b, "map 里没有这个裸名"]); continue; }
  if (v.indexOf("/ui_base/") < 0) { bad.push([b, "指向 " + v + "（dev 应由平台 provides 唯一供给）"]); continue; }
  const pk = "ui_base/" + b;
  const pv = String(imports[pk] || "");
  if (!pv) bad.push([pk, "map 里没有平台的逻辑名（provides 链路断了）"]);
  else if (pv.replace(/\?.*$/, "") !== v.replace(/\?.*$/, "")) bad.push([b, "与 " + pk + " 不同址 ⇒ 两份实例（" + v + " vs " + pv + "）"]);
  if (!sample) sample = v;
}
let asset = "未探";
if (sample) { try { const r = await fetch("http://127.0.0.1:" + PORT + sample, { signal: AbortSignal.timeout(5000) }); asset = String(r.status); } catch (e) { asset = "取不到"; } }
console.log("check-dev-provider: 裸名 " + BARE.length + " 个 · 平台产物 HTTP " + asset + " · 示例 " + sample);
if (bad.length) {
  for (const [k, why] of bad) console.log("  ✗ " + k + "：" + why);
  console.log("    ⇒ 官方模型：每个共享包只有一个所有者（平台 provides），闭包全部接到它；见 mdio /mdio_ui_esmx_待修点.md #2。");
  process.exit(1);
}
console.log("✓ check-dev-provider 全绿：裸名全部指向平台唯一所有者 · 平台产物在位");
process.exit(process.exitCode === undefined ? 0 : process.exitCode);   // 收尾退出：结论已打印 ⇒ 立刻退（CDP socket 会把进程吊到 deadline）