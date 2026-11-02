// test/vis.mjs —— visibility 与 claim **同管线**的回归（真页面 + 应用自己的模块 + 真引擎 sel 编译器）。
import { installGuards, armWatchdog, guardTarget } from "./lib/watchdog.mjs";
installGuards("vis");
armWatchdog("vis", 30000);   /* 未测到用时（扫描时未跑成功）⇒ 暂给 30s，待实测重定 */
//
// 为什么这么测：`/.mds/` 是系统目录（exec 写不进去）⇒ 不改 genesis 声明；在页面里 import 应用同一批模块
// （uiMd / uiStore / DocPane / client 的 DocRoot），用真引擎（同源 /exec 的 sel compile）编译 vis，再真渲染 DocPane 断言 DOM。
// 页面级 window.__v 只在**测试脚本**里用（判官 R6a 只约束 src/）。
import path from "node:path";
const CDP = process.env.CDP || "http://127.0.0.1:9222";
guardTarget(CDP, "vis");   // 9222 = 业主 Chrome，默认拒绝
const BASE = process.env.BASE || "http://127.0.0.1:5178";
// 框架已下沉为库包（S3）：页面里的 URL 形态 = /demo/@fs<仓库绝对路径>/mdio_ui/lib/mods/ui_core/src/…
const REPO_VIS = path.resolve(import.meta.dirname, "../../..");
// 改**逻辑名**（官方模型：消费方看不到物理路径；源码 URL 会另起一套模块图，
// 那正是"两份 React/mobx"的来源 —— 探针必须落在应用同一份实例上）。
const CORE = "ui_core";

const list = await (await fetch(CDP + "/json/list")).json();
for (const t of list) { if (t.type === "page") { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* ignore */ } } }
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=genesis"), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let id = 0; const pend = new Map();
const send = (m, pa) => new Promise((res) => { const i = ++id; pend.set(i, res); try { ws.send(JSON.stringify({ id: i, method: m, params: pa })); } catch (e) { res({}); } });
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); } } catch (e) { /* ignore */ } });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
// 探针若返回**对象**，上层 `JSON.parse(String(v))` 会拿到 "[object Object]" ⇒ 解析失败（vis 的 9 条红就是这个）。
//   ⇒ 在 ev 这一层把对象序列化成字符串，上层一行不用改。
const RAW_EV = (expr, awaitP = false) => send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: awaitP }).then((r) => (r.result ? r.result.value : "ERR " + JSON.stringify(r.error || {}).slice(0, 120)));
const ev = async (expr, awaitP = false) => { const v = await RAW_EV(expr, awaitP); return (v && typeof v === "object") ? JSON.stringify(v) : v; };
const R = [];
const chk = (cid, name, ok, detail) => R.push({ cid, name, ok: !!ok, detail: detail === undefined ? "" : String(detail).slice(0, 220) });

let booted = false;
for (let i = 0; i < 45; i += 1) { await wait(900); if (await ev("!!document.querySelector('.dv-ft-file')")) { booted = true; break; } }
await wait(2500);

// V0：genesis 无 visibility 节 ⇒ 声明装载里没有 vis 表，日志不会出现 boot: vis（= 零行为变化）
const liveLog = String(await ev("([...document.querySelectorAll('.dv-log-line')].map((l) => l.innerText).join('|'))") || "");
chk("V0", "无 visibility 节 ⇒ 与基线一致（日志无 boot: vis、声明照常装载）", booted === true && liveLog.indexOf("boot: vis") < 0 && liveLog.indexOf("claims") >= 0, JSON.stringify({ booted, hasVisLog: liveLog.indexOf("boot: vis") >= 0 }));

// 页面内装配：全部用短表达式，避免嵌套引号
const setup = [
  "window.__v = {};",
  "(async () => { const m = JSON.parse(document.querySelector('script[type=importmap]').textContent).imports; window.__v.R = await import(m['react']); return typeof window.__v.R.createElement; })()",
  "(async () => { const m = JSON.parse(document.querySelector('script[type=importmap]').textContent).imports; window.__v.rd = await import(m['react-dom/client']); return typeof window.__v.rd.createRoot; })()",
  "(async () => { window.__v.store = await import('" + CORE + "/uiStore'); return typeof window.__v.store.createUi; })()",
  "(async () => { window.__v.uiMd = await import('" + CORE + "/uiMd'); return typeof window.__v.uiMd.compileVisTable; })()",
  "(async () => { window.__v.client = await import('" + CORE + "/client'); return typeof window.__v.client.DocRoot; })()",
  "(async () => { window.__v.DocPane = (await import('" + CORE + "/DocPane')).default; return typeof window.__v.DocPane; })()",
  // §71 之后：**认领表只收"能画"的 mod**（renderClaims = 命中 ∧ 导出 default/Component 函数）⇒
  // 本测试喂的 claim 必须带一个真组件，否则 claimOf 全为 null（V9/V10/V11 会红，但红的是"测试没跟上契约"）。
  // 桩 = **本地组件**（Fallback 不存在）：画自己的文本 + 块级孩子交回框架 renderNode。
  "(function () { window.__v.Stub = function Stub({ node, renderNode }) { const Rc = window.__v.R;  const __ui = window.__v.uiNow; if (__ui && __ui.gates && node && !Array.isArray(node.ct) && (__ui.gates(node) || {}).visible === false) return null; const kids = node && Array.isArray(node.ct) ? node.ct : []; const t = node && typeof node.ct === \"string\" && node.ct ? node.ct : (node && typeof node.t === \"string\" ? node.t : \"\"); return Rc.createElement(\"div\", null, t, kids.map((c, i) => Rc.createElement(\"span\", { key: i }, renderNode ? renderNode(c) : null))); }; return typeof window.__v.Stub; })()",
];
let setupOk = true; const setupOut = [];
for (const expr of setup) { const v = await ev(expr, true); const s = String(v).slice(0, 40); setupOut.push(s); if (s.indexOf("THROW") >= 0 || s.indexOf("CDP-ERR") >= 0 || s.indexOf("ERR ") >= 0) setupOk = false; }

const RUNNER = [
  "window.__v.compileSel = async (sel) => {",
  "  const r = await fetch('/exec', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ code: 'sel compile ' + JSON.stringify(sel), active_doc: '', wt_uid: 'genesis' }) });",
  "  const j = await r.json(); const o = JSON.parse(String(j.result));",
  "  if (!o || !Array.isArray(o.steps)) throw new Error('no steps');",
  "  return o.steps;",
  "};",
  // 深度用**容器嵌套**造（bl 套 bl）：平铺树会假通过（实测踩过 ⇒ 活体测试 vis-live.mjs 抓出来的）
  // 不用 h1：内置件表不存在（A6），标题一律由 std 画 —— 合成测试用同一套桩。
  "window.__v.docJson = { nt: 'root', ct: [ { nt: 'bl', ct: [ { nt: 'bl', ct: [ { nt: 'b', ct: 'b1' }, { nt: 'b', ct: 'b2' } ] } ] }, { nt: 'p', ct: '段落甲' } ] };",
  "window.__v.invoke = async (opts) => {",
  "  const host = window.__v.host || (window.__v.host = (() => { const d = document.createElement('div'); d.id = 'vis-host'; d.style.cssText = 'position:fixed;left:-9999px;top:0;width:420px'; document.body.appendChild(d); return d; })());",
  "  const root = window.__v.root || (window.__v.root = window.__v.rd.createRoot(host));",
  "  const ui = window.__v.store.createUi();",
"  window.__v.uiNow = ui;",
  "  const model = window.__v.client.MD.create(window.__v.docJson);",
  "  ui.setDoc(model, (opts && opts.docPath) || '/x.md');   // 2026-10-06：路径现在是 setDoc 的第二参（setDocPath 已不存在）",
  "  const o = opts || {}; let table = []; let errors = []; let claims = []; let modPaths = null;",
  "  if (o.modPaths) modPaths = new Map(o.modPaths);",
  "  if (o.vis) { const bag = await window.__v.uiMd.compileVisTable({ vis: o.vis }, window.__v.compileSel); table = bag.table; errors = bag.errors; }",
  "  if (o.claim) { const bag2 = await window.__v.uiMd.compileMatchTable({ match: o.claim }, window.__v.compileSel, modPaths); claims = bag2.table; errors = errors.concat(bag2.errors); }",
  "  const mods = new Map(claims.map((c) => [c.name, { name: c.name, ns: { default: window.__v.Stub } }]));   // §71：认领要能画（桩 = 本地 Stub）",
  "  mods.set(\"std\", { name: \"std\", ns: { default: window.__v.Stub } });   // 新架构：未认领走声明里的默认渲染器（std）；合成树没有真 std ⇒ 用同款桩顶",
  "  ui.load({ claims, vis: table, errors, mods });",
  "  /* 2026-10-06：setDocPath 已并入 setDoc 的第二参 ⇒ 此行删除 */",
  "  window.__v.R.startTransition ? root.render(window.__v.R.createElement(window.__v.DocPane, { ui, exec: null })) : root.render(window.__v.R.createElement(window.__v.DocPane, { ui, exec: null }));",
  "  await new Promise((r) => setTimeout(r, 400));",
  "  const pNode = [...(model.ct || [])].find((n) => n.nt === 'p');",   // 容器对外透明 ⇒ 别拿 bl 断言（它本身不在任何命中集里）
  "  return JSON.stringify({ text: host.innerText || '', visN: table.length, errors: errors.length, claimOf0: (ui.gates(pNode) || {}).claim, visN: (table || []).length, activeVisN: (ui.activeVis || []).length, claimN: (ui.claims || []).length, visOfB: (() => { let f = null; const walk = (n) => { if (f) return; if (n && n.nt === 'b') { f = n; return; } for (const k of ((n && n.ct) || [])) walk(k); }; walk(model); return f ? (ui.gates(f) || {}).visible : 'no-b'; })(), visOfP: (ui.gates(pNode) || {}).visible, vis0: (table && table[0]) ? (table[0].sel || table[0].ir || 'entry') : null });",
  "};",
];
// 必须**整段一次** evaluate：逐行送会把 "async (sel) => {" 当成不完整语句 ⇒ 语法错误（探针已验证）
const runnerOut = String((await ev(RUNNER.join("\n"))) || "");
const runnerOk = runnerOut === "undefined" || runnerOut === "" || runnerOut.indexOf("CDP-ERR") < 0;

const cases = [
  ["base", "null"],
  ["pathHit", "{ vis: { include: [], exclude: [{ path: '/a.md', sel: '..[b]' }] }, docPath: '/a.md' }"],
  ["pathMiss", "{ vis: { include: [], exclude: [{ path: '/a.md', sel: '..[b]' }] }, docPath: '/b.md' }"],
  ["dirPrefix", "{ vis: { include: [], exclude: [{ path: '/demos/', sel: '..[p]' }] }, docPath: '/demos/x.md' }"],
  ["dirMiss", "{ vis: { include: [], exclude: [{ path: '/demos/', sel: '..[p]' }] }, docPath: '/top.md' }"],
  ["globAny", "{ vis: { include: [], exclude: [{ path: '.*', sel: '..[p]' }] }, docPath: '/whatever.md' }"],
  ["claimHit", "{ claim: [{ name: 'X', path: '/a.md', sel: '..' }], docPath: '/a.md' }"],
  ["claimMiss", "{ claim: [{ name: 'X', path: '/a.md', sel: '..' }], docPath: '/b.md' }"],
  ["modPathHit", "{ claim: [{ name: 'X', sel: '..' }], modPaths: [['X', '/a.md']], docPath: '/a.md' }"],
  ["modPathMiss", "{ claim: [{ name: 'X', sel: '..' }], modPaths: [['X', '/a.md']], docPath: '/b.md' }"],
  ["modPathOverridden", "{ claim: [{ name: 'X', path: '/b.md', sel: '..' }], modPaths: [['X', '/a.md']], docPath: '/b.md' }"],
  ["excludeB", "{ vis: { include: [], exclude: [{ path: '/x.md', sel: '..[b]' }] }, docPath: '/x.md' }"],   // 条目（容器对选择器透明 ⇒ 命中 b1/b2；由框架侧 Fallback 递归渲染 ⇒ 过同一判据）
  ["includeP", "{ vis: { include: [{ path: '/x.md', sel: '..[p]' }], exclude: [] }, docPath: '/x.md' }"],
  ["precedence", "{ vis: { include: [{ path: '/x.md', sel: '..[p]' }], exclude: [{ path: '/x.md', sel: '..[p]' }] }, docPath: '/x.md' }"],
  ["badSel", "{ vis: { include: [], exclude: [{ path: '/x.md', sel: 'TYPECHECK[' }] }, docPath: '/x.md' }"],
];
const o = {};
for (const [name, arg] of cases) {
  const raw = String((await ev("window.__v.invoke(" + arg + ").then((x) => x, (e) => 'INVOKE-THROW: ' + String(e && e.message))", true)) || "");
  if (String(raw).indexOf("THROW") >= 0) console.log("   [diag vis] " + String(raw).slice(0, 260));   // 只在异常时打印（API 漂移三项已修）
  // 诊断：runner 若在页面里抛，前面只会看到 {} ⇒ 这里把页面侧错误原样带出来。
  if (!/^\s*[{\[]/.test(String(raw)) || String(raw).indexOf("THROW") >= 0) console.log("   [diag vis] " + String(raw).slice(0, 260));
  try { o[name] = JSON.parse(raw); } catch (e) { o[name] = { parse: String(e.message).slice(0, 60), raw: raw.slice(0, 100) }; }
}
await ev("(() => { try { window.__v.host.remove(); } catch (e) {} })()");

const has = (s, t) => String((s && s.text) || "").indexOf(t) >= 0;
chk("V1", "基线（无 vis）三个层级都渲染", setupOk === true && runnerOk === true && has(o.base, "b1") && has(o.base, "b1") && has(o.base, "段落甲"), JSON.stringify({ setupOk, setupOut, base: o.base }));
chk("V2", "exclude: ..[b] ⇒ 两个条目消失，段落仍在（兜底递归也过判据）", !!o.excludeB && o.excludeB.visN === 1 && !has(o.excludeB, "b1") && !has(o.excludeB, "b2") && has(o.excludeB, "段落甲"), JSON.stringify(o.excludeB));
chk("V3", "include: ..[p] ⇒ 只留段落（容器块整块隐藏）", !!o.includeP && !has(o.includeP, "b1") && has(o.includeP, "段落甲"), JSON.stringify(o.includeP));
chk("V4", "include+exclude 同时命中 ⇒ exclude 优先（都隐藏）", !!o.precedence && !has(o.precedence, "段落甲"), JSON.stringify(o.precedence));
chk("V6", "path 生效（vis）：/a.md 上的 exclude 只在 /a.md 生效", !!o.pathHit && !has(o.pathHit, "b1") && has(o.pathMiss, "b1"), JSON.stringify({ hit: o.pathHit, miss: o.pathMiss }));
chk("V7", "path 生效（目录前缀 /demos/）：前缀命中才隐藏", !!o.dirPrefix && !has(o.dirPrefix, "段落甲") && has(o.dirMiss, "段落甲"), JSON.stringify({ dirPrefix: o.dirPrefix && o.dirPrefix.visN, dirMiss: o.dirMiss && o.dirMiss.text }));
chk("V8", "path 通配 .* ⇒ 任意文档都生效（std 的写法）", !!o.globAny && !has(o.globAny, "段落甲"), JSON.stringify(o.globAny));
chk("V9", "path 生效（claim）：/a.md 的认领不落到 /b.md（claimOf 分别 X / null）", !!o.claimHit && (o.claimHit.claimOf0 || {}).name === "X" && !!o.claimMiss && o.claimMiss.claimOf0 === null, JSON.stringify({ hit: o.claimHit && o.claimHit.claimOf0, miss: o.claimMiss && o.claimMiss.claimOf0 }));
chk("V10", "mods 的 path 下传：claim 自己没写 path 时用同名 mod 的 path（/a.md 命中、/b.md 不命中）", !!o.modPathHit && (o.modPathHit.claimOf0 || {}).name === "X" && !!o.modPathMiss && o.modPathMiss.claimOf0 === null, JSON.stringify({ hit: o.modPathHit && o.modPathHit.claimOf0, miss: o.modPathMiss && o.modPathMiss.claimOf0 }));
chk("V11", "claim 自己写的 path 覆盖 mod 的 path（/b.md 命中）", !!o.modPathOverridden && (o.modPathOverridden.claimOf0 || {}).name === "X", JSON.stringify(o.modPathOverridden && o.modPathOverridden.claimOf0));
chk("V5", "坏 sel ⇒ 进同一诊断账（errors>0）且渲染退回全可见", !!o.badSel && o.badSel.errors > 0 && o.badSel.visN === 0 && has(o.badSel, "b1"), JSON.stringify(o.badSel));

const failed = R.filter((x) => !x.ok);
for (const x of R) console.log((x.ok ? " ✓ " : " ✗ ") + x.cid + " " + x.name + (x.ok ? "" : "  → " + x.detail));
console.log(failed.length ? "✗ vis 失败 " + failed.length + " 项" : "✓ vis 全绿（" + R.length + " 项）");
try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) { /* ignore */ }
process.exit(failed.length ? 1 : 0);
