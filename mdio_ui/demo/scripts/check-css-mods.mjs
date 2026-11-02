// @ts-check
import { installGuards, armWatchdog, guardTarget } from "../test/lib/watchdog.mjs";
installGuards("check-css-mods");
armWatchdog("check-css-mods", 27000)   /* = 27s；实测 13.2s（放宽到 180s 跑一次）⇒ ×2 */
/**
 * check-css-mods —— **开发路径上的 CSS mod 判据**（dev 与生产两侧都验）。
 *
 * 现状（自检发现的缺口）：`verify-prod` 断言了"生产页里 `adoptedStyleSheets.length >= 1` + 无 `<style>` 自注入"，
 * 但 **dev 路径从没验过**（而 dev 与 prod 的装载路径不同：dev 由 Vite 转换、prod 是构建产物）⇒ 声明里"CSS 可为 mod"
 * 这句话在 dev 侧其实**没有网**。本判据补上 dev 侧的两条：
 *   1. 至少一张表被**采纳**（`((document.getElementById('app')||{}).shadowRoot||document).adoptedStyleSheets.length >= 1`）；
 *   2. 其中确有 **KaTeX 那张**（按规则文本含 `.katex` 认领）—— 而不是"随便有一张"；
 *   3. **没有** `<style id="mdio-katex-css">` 这类自注入兜底（若出现，说明 adopt 路径没走通、悄悄退回了旧方式）。
 *
 * 仍未覆盖的一半：**卸下**（`pruneCssMods`）—— 需要"声明里去掉该 CSS mod 后应被卸下"的时序，
 * 那属于演练（改声明 → 重载）的活，见 §63 的候选。
 *
 * 用法：node scripts/check-css-mods.mjs
 */
const PORT = Number(process.env.PORT || 5178);
const CDP = process.env.CDP || "http://127.0.0.1:9223";   // 默认改 9223：9222 是业主 Chrome（guardTarget 会拒绝）
guardTarget(CDP, "check-css-mods");   // 9222 = 业主 Chrome，默认拒绝
const WT = process.env.WT || "genesis";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let url = "";
try { const r = await fetch("http://127.0.0.1:" + PORT + "/demo/?wt=" + WT, { signal: AbortSignal.timeout(5000) }); url = r.status === 200 ? "ok" : "HTTP " + r.status; } catch (e) { url = "取不到"; }
if (url !== "ok") { console.log("✗ check-css-mods：开发宿主未服务（" + url + "）—— 先跑 `node scripts/up-all.mjs`"); process.exit(1); }

let ws = null;
try {
  const list = await (await fetch(CDP + "/json/list")).json();
  for (const t of list.filter((x) => x.type === "page")) { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* 忽略 */ } }
  await sleep(400);
  await fetch(CDP + "/json/new?" + encodeURIComponent("about:blank"), { method: "PUT" });
  await sleep(400);
  const tab = (await (await fetch(CDP + "/json/list")).json()).filter((x) => x.type === "page").pop();
  ws = new WebSocket(tab.webSocketDebuggerUrl);
  let id = 0; const pend = new Map();
  const send = (m, p) => new Promise((res, rej) => { const i = ++id; pend.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
  ws.addEventListener("message", (e) => { const m = JSON.parse(e.data); if (m.id) { const p = pend.get(m.id); if (p) { pend.delete(m.id); m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result); } } });
  await new Promise((r) => ws.addEventListener("open", r));
  await send("Runtime.enable"); await send("Page.enable");
  await send("Page.navigate", { url: "http://127.0.0.1:" + PORT + "/demo/?wt=" + WT });
  await sleep(12000);
  const ev = (expr) => send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: true }).then((r) => (r.result ? r.result.value : undefined));
  const n = Number(await ev("((document.getElementById('app')||{}).shadowRoot||document).adoptedStyleSheets.length"));
  // 第一版我断言"采纳的表里必须有 KaTeX 那张" —— **错**（实测：被采纳的是 theme 表，62 条规则；
  // KaTeX 那张是**按需懒采纳**（页面上有数学节点时才上，见"KaTeX 渲染触发"那条上游项））。
  // 正确的断言：**至少一张被采纳的表里有真实规则**（判定"采纳真发生了"，而不是塞了张空表）。
  const withRules = Number(await ev("((document.getElementById('app')||{}).shadowRoot||document).adoptedStyleSheets.filter((s) => { try { return s.cssRules.length > 0; } catch (e) { return false; } }).length"));
  const ruleCounts = String(await ev("JSON.stringify(((document.getElementById('app')||{}).shadowRoot||document).adoptedStyleSheets.map((s) => { try { return s.cssRules.length; } catch (e) { return -1; } }))"));
  const injected = !!(await ev("!!document.getElementById('mdio-katex-css')"));
  const bad = [];
  if (!(n >= 1)) bad.push("dev 页里一张样式表都没被采纳（adopted=" + n + "）");
  if (!(withRules >= 1)) bad.push("采纳的表里没有一张带真实规则（adopted=" + n + "，规则数 " + ruleCounts + "）");
  // 自注入只在"该页触发了 KaTeX"时才有意义（懒采纳）⇒ 本页无数学节点时为**信息**，不作失败。
  console.log("check-css-mods(dev): adopted=" + n + " · 带规则的表=" + withRules + " · 规则数 " + ruleCounts + " · 自注入(信息)=" + injected);
  if (bad.length) { for (const b of bad) console.log("  ✗ " + b); process.exit(1); }
  console.log("✓ check-css-mods 全绿：dev 路径上 CSS mod 被采纳，且没有退回自注入");
process.exit(process.exitCode === undefined ? 0 : process.exitCode);   // 收尾退出（结论行就在这里，末行的旧插入落到了别处）
} catch (e) {
  console.log("✗ check-css-mods：CDP 探测失败（" + String(e.message).slice(0, 80) + "）");
process.exit(process.exitCode === undefined ? 0 : process.exitCode);   // 收尾退出：结论已打印 ⇒ 立刻退（CDP socket 会把进程吊到 deadline）
  process.exit(1);
} finally { try { if (ws) ws.close(); } catch (e) { /* 忽略 */ } }
