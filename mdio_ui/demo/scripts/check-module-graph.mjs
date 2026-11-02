// @ts-check
import { installGuards, armWatchdog, guardTarget } from "../test/lib/watchdog.mjs";
installGuards("check-module-graph");
armWatchdog("check-module-graph", 94000);   /* = 94s；实测 46.9s（扫描 1 跑）⇒ ×2 */
/**
 * check-module-graph —— **模块图守卫**。
 *
 * 背景：dev 若让 bundler 解析 `uses` 的裸名，官方件源码会进浏览器图 ⇒ **同一模块两份**
 *（实测：mobx/react 各两份、DocPane/NodeView 各两份）。后果不只是浪费：性能结论会被测在**另一份**上
 *（实测踩到：文档面「全量重渲染」无法归因）。所以钉成判据。
 *
 * 断言（dev 页面，禁缓存，从零加载）：
 *   1. 资源里**不得**出现 `/lib/mods/**`（官方件源码不得进浏览器图）；
 *   2. 每个共享依赖（平台 provides 的那批）最多加载 **1 份**产物；
 *   3. 未捕获异常 = 0；
 *   4. 页面必须真的挂起来（文件树有行）—— 防「不报错但空白」的假绿。
 */
const CDP = process.env.CDP || "http://127.0.0.1:9222";
guardTarget(CDP, "check-module-graph");   // 9222 = 业主 Chrome，默认拒绝
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const SHARED = ["react", "react-dom", "mobx", "mobx-react-lite", "mobx-state-tree"];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const hard = setTimeout(() => { console.log("✗ check-module-graph：硬超时"); process.exit(1); }, 60000);
let page = null;
try {
  const list = await (await fetch(CDP + "/json/list")).json();
  for (const t of list) if (t.type === "page" && String(t.url).includes("probe=modgraph")) { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) {} }
  page = await (await fetch(CDP + "/json/new?about:blank", { method: "PUT" })).json();
} catch (e) { console.log("✗ check-module-graph：CDP 不可达（先跑 up-all.mjs）"); process.exit(1); }
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0; const pend = new Map(); const errs = [];
const send = (m, pa) => { const i = ++id; ws.send(JSON.stringify({ id: i, method: m, params: pa || {} })); return new Promise((res) => pend.set(i, res)); };
ws.addEventListener("message", (e) => { try { const m = JSON.parse(e.data);
  if (m.method === "Runtime.exceptionThrown") errs.push(String((m.params.exceptionDetails && m.params.exceptionDetails.text) || "").slice(0, 120));
  if (m.id && pend.has(m.id)) { pend.get(m.id)(m); pend.delete(m.id); } } catch (err) {} });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Network.enable"); await send("Page.enable");
await send("Network.setCacheDisabled", { cacheDisabled: true });   // 必须：旧缓存的入口模块会让结论完全反过来
await send("Page.navigate", { url: BASE + "/?wt=" + (process.env.WT || "genesis") + "#probe=modgraph" });
// 轮询等待（不用固定 11s）：宿主刚重启 / 浏览器里还有别的页面时，首屏可能明显更慢；
// 固定等待会造成"页面没挂起来"的**假红**（实测：同一条检查手动绿、在 verify-all 里红）。
const probe = async (expr) => { const r = await send("Runtime.evaluate", { expression: expr, returnByValue: true }); return r.result && r.result.result && r.result.result.value; };
let waited = 0;
while (waited < 45000) {
  const n = Number(await probe("(((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.dv-ft-file')).length")) || 0;
  if (n > 0) break;
  await sleep(1000); waited += 1000;
}
await sleep(1500);   // 挂起来后再给一拍，让模块图稳定
const ev = async (e) => { const r = await send("Runtime.evaluate", { expression: e, returnByValue: true }); return r.result && r.result.result && r.result.result.value; };
const urls = JSON.parse(String(await ev("JSON.stringify(performance.getEntriesByType('resource').map(function(e){return e.name}))")) || "[]");
const files = Number(await ev("(((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.dv-ft-file')).length")) || 0;
const srcGraph = urls.filter((u) => u.indexOf("/lib/mods/") >= 0);
const counts = {};
for (const u of urls) { const m = /\/ui_base\/([A-Za-z0-9_-]+)\./.exec(u); if (m) counts[m[1]] = (counts[m[1]] || 0) + 1; }
const dup = Object.entries(counts).filter(([, n]) => n > 1);
const bad = [];
if (srcGraph.length) bad.push("官方件源码进图 " + srcGraph.length + " 个（例：" + srcGraph[0].slice(-46) + "）");
if (dup.length) bad.push("共享依赖多份：" + dup.map(([k, n]) => k + "×" + n).join(" "));
if (errs.length) bad.push("未捕获异常 " + errs.length + " 条（" + errs[0] + "）");
if (files <= 0) bad.push("页面没挂起来（文件树 0 行）");
for (const b of SHARED) if (!Object.keys(counts).some((k) => k === b)) bad.push("共享依赖未按平台产物加载：" + b);
console.log("check-module-graph: 资源 " + urls.length + " 条 · 源码图 " + srcGraph.length + " · 共享依赖 " + JSON.stringify(counts) + " · 异常 " + errs.length + " · 文件树 " + files + " 行");
if (bad.length) {
  for (const b of bad) console.log("  ✗ " + b);
  // 失败现场（诊断用）：页面文本 + 已加载的关键模块，便于区分"卡装载"与"真空白"
  try {
    console.log("  现场 body: " + JSON.stringify(String(await probe("(document.body.innerText||'').slice(0,90)"))));
    console.log("  现场 script src: " + String(await probe("JSON.stringify([].slice.call(document.querySelectorAll('script[type=module]')).map(function(s){return s.getAttribute('src')||''}))")));
  } catch (e) { /* ignore */ } console.log("  ⇒ 若源码图非 0：dev 的裸名接线回退了（见 entry.node.ts 的响应体改写）；若共享依赖多份：provides 链被绕过。"); process.exit(1); }
console.log("✓ check-module-graph 全绿：单一模块图 · 共享依赖各 1 份 · 0 异常 · 页面真挂起来");
try { await fetch(CDP + "/json/close/" + page.id); } catch (e) { /* 收尾：不留页面（必须在采集之后，否则后续求值全挂） */ }
clearTimeout(hard);
process.exit(0);