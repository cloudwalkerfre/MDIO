// test/swap-live.mjs —— exp_swap 的**全套活体对齐测试**（新 demo 架构）。
import { installGuards, armWatchdog, guardTarget } from "./lib/watchdog.mjs";
installGuards("swap-live");
armWatchdog("swap-live", 88000);   /* = 88s；实测 43.9s（放宽到 280s 跑一次）⇒ ×2 */
//
// 它同时压到这四件新能力：① md: 装载（实现写在文档的 jsx 块里）② 声明热重载（按钮改 ui.md ⇒ 自动重装）
// ③ 文档级 vis（- include: /swap.md:: ⇒ 文件树只留这篇）④ 声明序即优先级（同一 sel 两条认领，后写胜出）。
// 纪律：声明是可写状态 ⇒ 开头读、结尾还原（并断言逐字一致）。
const GW = process.env.GW || "http://127.0.0.1:8011";
const CDP = process.env.CDP || "http://127.0.0.1:9222";
guardTarget(CDP, "swap-live");   // 9222 = 业主 Chrome，默认拒绝
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const WT = process.env.SWAP_WT || "c7Sw4p00";
const PATH = "/swap.md";
// 目标 wt 未挂载 ⇒ ⊘ 跳过 + 退 0（--strict 退 1）。
const { requireMountedWt } = await import("./wt-guard.mjs");
await requireMountedWt(GW, WT, "swap-live");

const gwExec = async (code) => {
  const r = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, active_doc: "", wt_uid: WT }) });
  const j = await r.json();
  return String(j.result == null ? "" : j.result);
};
const R = [];
const chk = (cid, name, ok, detail) => R.push({ cid, name, ok: !!ok, detail: detail === undefined ? "" : String(detail).slice(0, 220) });

// 源真相在**磁盘**（exp_swap/wt/…）：先按 exec 推进 VFS（网关服务的是 VFS 持久态，直接改磁盘不生效——实测踩过）。
const fs = await import("node:fs");
const path = await import("node:path");
const REPO = process.env.REPO || path.resolve(import.meta.dirname, "../../..");
const declFile = REPO + "/exp_swap/wt/.mds/ui.md";
const docFile = REPO + "/exp_swap/wt/swap.md";
const declA = fs.readFileSync(declFile, "utf8");
const docText = fs.readFileSync(docFile, "utf8");
await gwExec(JSON.stringify(docText) + " | /> " + PATH);
await gwExec(JSON.stringify(declA) + " | /> /.mds/ui.md");
const declSynced = String(await gwExec("$ /.mds/ui.md::"));
const list = await (await fetch(CDP + "/json/list")).json();
for (const t of list) { if (t.type === "page") { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* ignore */ } } }
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=" + WT), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let id = 0; const pend = new Map();
const send = (m, pa) => new Promise((res) => { const i = ++id; pend.set(i, res); try { ws.send(JSON.stringify({ id: i, method: m, params: pa })); } catch (e) { res({}); } });
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); } } catch (e) { /* ignore */ } });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const ev = (expr) => send("Runtime.evaluate", { expression: expr, returnByValue: true }).then((r) => (r.result ? r.result.value : "ERR"));
// （B 路线按根）：内容在 `#app` 的影子树里 —— 判**内容**的探针走 evs（影子根优先、缺的回退 document）；判 .dv-* 外壳的仍走 ev。
const ROOT_SHIM = (expr) => "(() => { const g = window.document; const sr = (g.getElementById('app')||{}).shadowRoot; const document = sr ? new Proxy(sr, { get: (t, k) => { const v = (k in t) ? t[k] : g[k]; return typeof v === 'function' ? v.bind((k in t) ? t : g) : v; } }) : g; return (" + expr + "); })()";
const evs = (expr) => send("Runtime.evaluate", { expression: ROOT_SHIM(expr), returnByValue: true }).then((r) => (r.result ? r.result.value : "ERR"));

let files = 0;
for (let i = 0; i < 45; i += 1) { await wait(900); files = Number(await ev("((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file').length")) || 0; if (files) break; }
await wait(2500);
const paths = String(await ev("JSON.stringify([...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].map((e) => e.getAttribute('data-path')))"));
const bootLog = String(await ev("([...document.querySelectorAll('.dv-log-line')].map((l) => l.innerText).join('|'))") || "");
chk("S1", "fileview 完整呈现本 wt 的所有 md（docScope 不影响它）", files >= 2 && paths.indexOf(PATH) >= 0 && paths.indexOf("/README.md") >= 0 && bootLog.indexOf("只作用于产品面") >= 0, JSON.stringify({ files, paths: String(paths).slice(0, 90), scopeLogged: bootLog.indexOf("只作用于产品面") >= 0 }));

await ev("([...((document.querySelector('#app-left')||{}).shadowRoot||document).querySelectorAll('.dv-ft-file')].find((e) => e.getAttribute('data-path') === '" + PATH + "'))?.click()");   // 真鼠标版尝试失败（页面侧把 ev 变成 undefined ⇒ JSON.parse 抛）⇒ 先回退，待重写
// 一次 evaluate 取全部判据（DOM 在页面里读）
const SNAP_EXPR = "JSON.stringify((() => { const t = String(((((document.getElementById('app')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.mdio-doc'))||{}).innerText || ''); return { a: !!document.querySelector('[data-swap=\"a\"]'), b: !!document.querySelector('[data-swap=\"b\"]'), orderA: !!document.querySelector('[data-order=\"a\"]'), orderB: !!document.querySelector('[data-order=\"b\"]'), note: t.indexOf('块级被排除') >= 0, implLeak: t.indexOf('SwapA') >= 0, log: String(([...document.querySelectorAll('.dv-log-line')].map((l) => l.innerText).join('|')) || '') }; })())";
const snap = async () => { try { return JSON.parse(String(await evs(SNAP_EXPR)) || "{}"); } catch (e) { return { parse: String(e.message).slice(0, 60) }; } };
// （快照挪到 app 面就绪之后：见下面的 wait 之后）
// 诊断：S2 失败时把「文件树行数 / data-swap 计数 / app 文本」打出来（外部探针在这棵树上连树都不渲染）。
// [diag S2-map] 已结论：import map 含 exp_swap_swap（保留结论在 note，不再每轮打印）
// boot 日志显示点击只把 /swap.md 开在 **mona/fileview**（app 面板为空 `app:""`）⇒ 这里显式走到 **?app=** 那一面再断言。
// 先不跳 ?app=（上次跳了没变化）——这里只等文档面就绪，见下面。
for (let i = 0; i < 25; i += 1) { await wait(900); const __s = Number(await ev("document.querySelectorAll('.mdio-std').length")) || 0; if (__s > 0) break; }   // 照 check-doc-render 的序列等**文档面**就绪（不点 ?app=、只等）
const openState = await snap();   // 就绪后取快照（上一步重排时被误删）





// 调试期的「开发者模式点击」与「README 对照」两段已撤 —— 它们在快照之后**切走文档**，S6/S7/S9 因此看不到面板（本轮真因）。
// 调试期的「开发者模式点击」与「README 对照」两段已撤 —— 它们会在快照之后**切走文档**，导致 S6/S7/S9 看不到面板（本轮真因）。
chk("S2", "面板节由 ui 模块件（.ui::swap-a.jsx）渲染（A 版在场）", openState.a === true && openState.b === false, JSON.stringify({ a: openState.a, b: openState.b }));
chk("S3", "节点级 vis：整节「实现」不画（源码不外泄）", openState.implLeak === false, JSON.stringify({ implLeak: openState.implLeak }));
chk("S4", "块级过滤由组件完成：jsx@note 文本不出现在页面", openState.note === false, JSON.stringify({ note: openState.note }));
{ const shot = await send("Page.captureScreenshot", { format: "png" }); const fs2 = await import("node:fs"); const dir = process.env.SHOT_DIR || ".tmp/steps-gui"; try { fs2.mkdirSync(dir, { recursive: true }); } catch (e) { /* ignore */ } if (shot && shot.data) fs2.writeFileSync(dir + "/22-swap-a.png", Buffer.from(shot.data, "base64")); }
chk("S5", "声明序即优先级：同一 sel 两条认领 ⇒ 后声明的胜出", openState.orderB === true && openState.orderA === false, JSON.stringify({ orderA: openState.orderA, orderB: openState.orderB }));

// 点按钮：改声明 ⇒ 声明热重载 ⇒ B 版
// 按钮在 **影子根** 里 ⇒ 用 evs（先按根问；`?.click()` 对真按钮有效）
await evs("document.querySelector('[data-swap=\"a\"] button')?.click()");
let swapped = null;
for (let i = 0; i < 12; i += 1) { await wait(900); swapped = await snap(); if (swapped.b) break; }
chk("S6", "热重载：按钮改 ui.md 后无需刷新即切到 B 版", !!swapped && swapped.b === true && swapped.a === false, JSON.stringify({ a: swapped && swapped.a, b: swapped && swapped.b }));
const declB = String(await gwExec("$ /.mds/ui.md::"));
chk("S7", "写回真的落到声明里（ui:swap-a.jsx → ui:swap-b.jsx）", declB.indexOf("swap-b.jsx") >= 0 && declB.indexOf("swap-a.jsx") < 0, JSON.stringify(declB.match(/ui:swap-[ab]\.jsx/g)));

// AppView 侧 docScope：?app= 指向被排除的文档 ⇒ 退到可见档（产品面才受 vis 约束）
await send("Page.navigate", { url: BASE + "/?wt=" + WT + "&app=" + encodeURIComponent("/README.md") });
await wait(4000);
for (let i = 0; i < 30; i += 1) { await wait(900); if (await ev("!!document.querySelector('.dv-ft-file')")) break; }
await wait(3000);
const appState = await snap();
// **判据用可观察量**（修）：原判据要求日志里出现「不在 docScope 内 ⇒ 退到」，
// 但 **AppView 是产品面、三栏与日志面板都不在 DOM**（实测 `.dv-log-line` 数 = 0）⇒ 该断言在 AppView 下
// 永远不可能成立。直接验结果：`ui.docPath` 落到可见档、被排除档 `docVisible === false`。
const DOCPATH_EXPR = "(() => { var el = document.querySelector('.mdio-doc'); if (!el) return 'no-doc'; var k = Object.keys(el).find(function(x){return x.indexOf('__reactFiber$')===0;}); if (!k) return 'no-fiber'; var f = el[k]; for (var i = 0; i < 160 && f; i += 1) { var p = f.memoizedProps; if (p && p.ui && typeof p.ui.docVisible === 'function') { return JSON.stringify({ docPath: p.ui.mdPath, readmeVisible: p.ui.docVisible('/README.md'), curVisible: p.ui.docVisible(p.ui.mdPath) }); } f = f.return; } return 'no-ui'; })()";
let dp = {};
try { dp = JSON.parse(String(await evs(DOCPATH_EXPR)) || "{}"); } catch (e) { dp = { err: String(e && e.message || e).slice(0, 60) }; }   // 内容在影子树里 ⇒ 读 docPath 也必须按根
chk("S9", "AppView 侧 docScope 生效：?app= 指向被排除档 ⇒ 退到可见档（可观察量：docPath 落在可见档、被排除档不可见）", (appState.a === true || appState.b === true) && dp.docPath === PATH && dp.curVisible === true && dp.readmeVisible === false, JSON.stringify({ a: appState.a, b: appState.b, docPath: dp.docPath, readmeVisible: dp.readmeVisible, curVisible: dp.curVisible }));

// 还原：写回 A 版声明并断言逐字一致
await gwExec(JSON.stringify(declA) + " | /> /.mds/ui.md");
await wait(2500);
const back = String(await gwExec("$ /.mds/ui.md::"));
chk("S8", "还原声明后逐字一致", back.trim() === declA.trim(), JSON.stringify({ same: back.trim() === declA.trim() }));

const failed = R.filter((x) => !x.ok);
for (const x of R) console.log((x.ok ? " ✓ " : " ✗ ") + x.cid + " " + x.name + (x.ok ? "" : "  → " + x.detail));
console.log(failed.length ? "✗ swap-live 失败 " + failed.length + " 项" : "✓ swap-live 全绿（" + R.length + " 项）");
try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) { /* ignore */ }
process.exit(failed.length ? 1 : 0);
