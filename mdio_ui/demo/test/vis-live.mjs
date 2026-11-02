// test/vis-live.mjs —— vis 的**活体端到端**：真实声明（/.mds/ui.md 真加一节 ## visibility）→ 真应用重启装载 → 真文档面渲染差异。
import { installGuards, armWatchdog, guardTarget } from "./lib/watchdog.mjs";
installGuards("vis-live");
armWatchdog("vis-live", 78000);   /* = 78s；实测 38.6s（放宽到 280s 跑一次）⇒ ×2 */
//
// 与 test/vis.mjs 的分工：vis.mjs 用手造树 + 手造 visDecl 测**机制**（不碰声明）；本文件走**声明→装载→渲染**全链，
// 覆盖 vis.mjs 到不了的那段：entry.client 的 compileVisTable 调用 + boot 日志 + 真实文档面。
// 纪律：先读出原声明并保存 ⇒ 结束时**必须还原**（脚本开头也先还原一次，防上次中断留下脏声明）。
const GW = process.env.GW || "http://127.0.0.1:8011";
const CDP = process.env.CDP || "http://127.0.0.1:9222";
guardTarget(CDP, "vis-live");   // 9222 = 业主 Chrome，默认拒绝
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const DOC = "/sop_probe.md";
const SEC_KEEP = "甲节";
const SEC_HIDE = "乙节";
// sel 用 ::..名 = RecursiveDrill（HS 是**层级树**：小节嵌在 h1 之下，单层 Drill 打不中 —— 实测踩过）
const VIS_BLOCK = "\n## visibility\n- exclude: ::.." + SEC_HIDE + "\n";

const gwExec = async (code, wt = "genesis") => {
  const r = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, active_doc: "", wt_uid: wt }) });
  const j = await r.json();
  return String(j.result == null ? "" : j.result);
};
const declWrite = async (text) => String(await gwExec(JSON.stringify(text) + " | /> /.mds/ui.md"));
const declRead = async () => String(await gwExec("$ /.mds/ui.md"));

const R = [];
const chk = (cid, name, ok, detail) => R.push({ cid, name, ok: !!ok, detail: detail === undefined ? "" : String(detail).slice(0, 220) });

// 0) 开跑先还原（防上次中断留脏）：把可能存在的 ## visibility 段切掉
const dirty = await declRead();
const baseDecl = dirty.split("## visibility")[0].replace(/\s+$/, "");
if (dirty !== baseDecl) await declWrite(baseDecl);
const declAfterClean = await declRead();
chk("L0", "声明已回到无 visibility 的基线（自愈）", declAfterClean.indexOf("visibility") < 0 && declAfterClean.indexOf("## match") >= 0, JSON.stringify(declAfterClean.slice(-60)));

// 1) 种子文档：两个小节 + 各自正文
await gwExec(JSON.stringify("# VIS E2E\n\n## " + SEC_KEEP + "\n\n正文甲\n\n## " + SEC_HIDE + "\n\n正文乙\n") + " | /> " + DOC);
await gwExec(JSON.stringify("# 别的文档\n\n## " + SEC_HIDE + "\n\n正文乙\n") + " | /> /sop_probe_b.md");   // 只为提供另一条路径做对照

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
// 按根门面（B：app 内容在影子树里）—— 探针一行不改。
const ROOT_SHIM = (expr) => "(() => { const g = window.document; const sr = (g.getElementById('app')||{}).shadowRoot; const document = sr ? { querySelector: (s) => sr.querySelector(s) || g.querySelector(s), querySelectorAll: (s) => { const a = [...sr.querySelectorAll(s)]; return a.length ? a : g.querySelectorAll(s); }, getElementById: (s) => sr.getElementById(s) || g.getElementById(s), body: { innerText: (g.body.innerText || '') + ' ' + (sr.textContent || ''), textContent: (g.body.textContent || '') + ' ' + (sr.textContent || '') }, documentElement: g.documentElement, createElement: (t) => g.createElement(t), addEventListener: g.addEventListener.bind(g), fonts: g.fonts, title: g.title } : g; return eval(" + JSON.stringify(expr) + "); })()";
const RAW_EV = (expr, awaitP = false) => send("Runtime.evaluate", { expression: expr, returnByValue: true, awaitPromise: awaitP }).then((r) => (r.result ? r.result.value : "ERR"));
const ev = (x) => RAW_EV(ROOT_SHIM(x));

const openDoc = async () => {
  for (let i = 0; i < 45; i += 1) { await wait(900); if (await ev("!!document.querySelector('.dv-ft-file')")) break; }
  await wait(2000);
  await ev("([...document.querySelectorAll('.dv-ft-file')].find((e) => e.getAttribute('data-path') === '" + DOC + "'))?.click()");
  await wait(4000);
  return {
    text: String(await ev("(() => { const R = (document.getElementById('app')||{}).shadowRoot || document; const a = document.querySelector('.dv-app .mdio-doc'); const b = R.querySelectorAll('.mdio-std'); const t1 = a ? (a.innerText || a.textContent || '') : ''; const t2 = [...b].map((e) => e.textContent || '').join(' '); return (t1.length > t2.length ? t1 : t2); })()") || ""),
    log: String(await ev("([...document.querySelectorAll('.dv-log-line')].map((l) => l.innerText).join('|'))") || ""),
  };
};

// 2) 基线（无 vis 节）：两节都渲染
const baseline = await openDoc();
// 诊断：L1 失败时把「文档面到底长什么样」打出来（外部探针在这棵树上不可靠）。
  { const __d = String(await ev("JSON.stringify({ dvApp: document.querySelectorAll('.dv-app').length, mdioDoc: document.querySelectorAll('.mdio-doc').length, mdioStd: document.querySelectorAll('.mdio-std').length, sr: !!((document.getElementById('app')||{}).shadowRoot), srLen: String(((document.getElementById('app')||{}).shadowRoot||{}).textContent||'').length, bodyLen: (document.body && document.body.innerText || '').length })")); console.log('   [diag L1] ' + __d.slice(0, 220)); }
chk("L1", "基线：无 vis 节 ⇒ 两节都渲染", baseline.text.indexOf(SEC_KEEP) >= 0 && baseline.text.indexOf(SEC_HIDE) >= 0 && baseline.log.indexOf("boot: vis") < 0, JSON.stringify({ keep: baseline.text.indexOf(SEC_KEEP) >= 0, hide: baseline.text.indexOf(SEC_HIDE) >= 0, visLog: baseline.log.indexOf("boot: vis") >= 0 }));

// 3) 真加一节 ## visibility（走 exec 写声明，用户同款路径），重载页面
const wrote = await declWrite(declAfterClean + VIS_BLOCK);
await send("Page.navigate", { url: BASE + "/?wt=genesis" });
await wait(3500);
const withVis = await openDoc();
const visLogged = withVis.log.match(/boot: vis \d+ 条/);
chk("L2", "带 ## visibility 装载 ⇒ 日志出现 boot: vis 1 条（entry.client 的装载路径真跑到）", !!visLogged, JSON.stringify({ visLogged: visLogged && visLogged[0], wrote: String(wrote).slice(0, 40) }));
{ const shot = await send("Page.captureScreenshot", { format: "png" }); const fs = await import("node:fs"); const dir = process.env.SHOT_DIR || ".tmp/steps-gui"; try { fs.mkdirSync(dir, { recursive: true }); } catch (e) { /* ignore */ } if (shot && shot.data) fs.writeFileSync(dir + "/21-vis-live.png", Buffer.from(shot.data, "base64")); }
chk("L3", "真文档面：被 exclude 的节整块消失（子树一并隐藏），另一节与正文仍在", withVis.text.indexOf(SEC_HIDE) < 0 && withVis.text.indexOf(SEC_KEEP) >= 0 && withVis.text.indexOf("正文甲") >= 0, JSON.stringify({ text: withVis.text.replace(/\n+/g, "|").slice(0, 90) }));

// 3b) path 限定到**别的文档** ⇒ 本档不生效（"path 生效"的活体判据）
await declWrite(declAfterClean + "\n## visibility\n- exclude: /sop_probe_b.md::.." + SEC_HIDE + "\n");
await send("Page.navigate", { url: BASE + "/?wt=genesis" });
await wait(3500);
const otherDoc = await openDoc();
const gateLine = (otherDoc.log.match(/vis: 本档生效 [0-9]+\/[0-9]+ 条/) || [null])[0];
chk("L6", "path 限定到别的文档 ⇒ 本档不生效（日志 0/1 条，乙节照常渲染）", otherDoc.text.indexOf(SEC_HIDE) >= 0 && gateLine === "vis: 本档生效 0/1 条", JSON.stringify({ text: otherDoc.text.replace(/\n+/g, "|").slice(0, 60), gateLine }));

// 4) 还原声明（必须），重载验证复原
const restored = await declWrite(baseDecl);
await send("Page.navigate", { url: BASE + "/?wt=genesis" });
await wait(3500);
const back = await openDoc();
chk("L4", "还原声明后复原：两节都回来、boot: vis 消失", back.text.indexOf(SEC_HIDE) >= 0 && back.text.indexOf(SEC_KEEP) >= 0 && back.log.indexOf("boot: vis") < 0, JSON.stringify({ text: back.text.replace(/\n+/g, "|").slice(0, 90), visLog: back.log.indexOf("boot: vis") >= 0 }));
const finalDecl = await declRead();
chk("L5", "声明最终与原始一致（无 visibility 残留）", finalDecl.indexOf("visibility") < 0, JSON.stringify(finalDecl.slice(-50)));

const failed = R.filter((x) => !x.ok);
for (const x of R) console.log((x.ok ? " ✓ " : " ✗ ") + x.cid + " " + x.name + (x.ok ? "" : "  → " + x.detail));
console.log(failed.length ? "✗ vis-live 失败 " + failed.length + " 项" : "✓ vis-live 全绿（" + R.length + " 项）");
try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) { /* ignore */ }
process.exit(failed.length ? 1 : 0);
