// test/sop.mjs —— SOP 自测：把"人会做的三件事"变成常规判据，别等用户报。
import { installGuards, armWatchdog, guardTarget } from "./lib/watchdog.mjs";
installGuards("sop");
armWatchdog("sop", 165000);   /* = 165s；实测 82.3s（放宽到 280s 跑一次）⇒ ×2 */
//
// 三件事：① 点组件按钮 ② 在 Monaco 里编辑（**替换** = del+ins 两条 op，也就是出过问题的形态）
//        ③ 外部写（exec）后编辑器跟随。
// 每条都断言**三方一致**（Monaco 显示 / 文档真值 / 渲染/卡片）+ **零丢帧**。
//
// 为什么能可靠驱动 Monaco：MonaPanel 监听自定义事件 `mdio-sop-edit`（detail: {line, from, to, text}），
// 用 model.pushEditOperations 改文本 ⇒ 走**同一条** onDidChangeContent 发送链。合成鼠标/键盘事件在自动化里落点不稳。
// 为什么查丢帧：sendPayload 在上行环背压时会**静默丢弃整条 op**（曾导致"删了字符却还在"），
// 现在会经 devview 日志报 `丢帧`（见 .tmp/sop/TRACE-23.md）。
const GW = process.env.GW || "http://127.0.0.1:8011";
const CDP = process.env.CDP || "http://127.0.0.1:9222";
guardTarget(CDP, "sop");   // 9222 = 业主 Chrome，默认拒绝
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const WT = process.env.SOP_WT || "c8Page01";
const SHOT = process.env.SHOT_DIR || ".tmp/steps-gui";
const NL = String.fromCharCode(10);
const fs = await import("node:fs");
const GWX = async (code) => {
  const r = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, active_doc: "/stats.md", wt_uid: WT }) });
  const j = await r.json();
  return String(j.result == null ? "" : j.result);
};
const R = [];
const chk = (cid, name, ok, detail) => R.push({ cid, name, ok: !!ok, detail: detail === undefined ? "" : String(detail).slice(0, 220) });
const DOC0 = String(await GWX("$ /stats.md"));
const list = await (await fetch(CDP + "/json/list")).json();
for (const t of list) { if (t.type === "page") { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) {} } }
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=" + WT), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let seq = 0; const pend = new Map();
const send = (m, pa) => new Promise((res) => { const i = ++seq; pend.set(i, res); try { ws.send(JSON.stringify({ id: i, method: m, params: pa })); } catch (e) { res({}); } });
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); } } catch (e) {} });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
// 按根门面（B：app 内容在影子树里）—— 探针一行不改。
const ROOT_SHIM = (expr) => "(() => { const g = window.document; const sr = (g.getElementById('app')||{}).shadowRoot; const document = sr ? { querySelector: (s) => sr.querySelector(s) || g.querySelector(s), querySelectorAll: (s) => { const a = [...sr.querySelectorAll(s)]; return a.length ? a : g.querySelectorAll(s); }, getElementById: (s) => sr.getElementById(s) || g.getElementById(s), body: { innerText: (g.body.innerText || '') + ' ' + (sr.textContent || ''), textContent: (g.body.textContent || '') + ' ' + (sr.textContent || '') }, documentElement: g.documentElement, createElement: (t) => g.createElement(t), addEventListener: g.addEventListener.bind(g), fonts: g.fonts, title: g.title } : g; return eval(" + JSON.stringify(expr) + "); })()";
const RAW_EV = (expr) => send("Runtime.evaluate", { expression: expr, returnByValue: true }).then((r) => (r.result ? r.result.value : "ERR"));
const ev = (x) => RAW_EV(ROOT_SHIM(x));
const logText = () => ev("([...document.querySelectorAll('.dv-log-line')].map((l)=>l.innerText).join(String.fromCharCode(10)))");
const cardRec = async () => { const t = String(await ev("String((document.querySelector('[data-page=\"stats\"]')||{}).innerText||'')")); const m = t.match(/记录[:\s]*(\d+)/); return m ? m[1] : null; };
const docRec = async () => (String(await GWX("$ /stats.md")).match(/- 记录: (\d+)/) || [null, null])[1];
// 读 Monaco：把可见行**逐行拼接**（wordWrap 开着，值可能被折到下一行）再匹配
// Monaco 是**虚拟化**的 —— `.view-line` 只含视口内的行，目标行（`记录: N`）在下方时永远读不到 ⇒
//   读不到就**真滚轮往下推一拍**再读（最多 6 拍）；找到即返回。
const scrollMonaOnce = async () => {
  const p = JSON.parse(String(await ev("JSON.stringify((() => { const el = document.querySelector('.dv-left'); if (!el) return null; const r = el.getBoundingClientRect(); return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }; })())")) || null);
  if (!p) return false;
  await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: p.x, y: p.y, deltaX: 0, deltaY: 240 });
  await wait(350);
  return true;
};
const monaRec = async () => {
  for (let round = 0; round < 6; round += 1) {
    const t = String(await ev("[...(((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.view-line'))].map((e)=>(e.innerText||'')).join(String.fromCharCode(10))"));
    const m = t.match(/记录[:\s]*(\d+)/);
    if (m) return m[1];
    await scrollMonaOnce();
  }
  return null;
};
const clickIn = async (sel) => {
  const b = await ev("JSON.stringify((() => { const el = document.querySelector(" + JSON.stringify(sel) + "); if (!el) return null; el.scrollIntoView(); const r = el.getBoundingClientRect(); return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }; })())");
  const p = JSON.parse(String(b) || "null");
  if (!p) return false;
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: p.x, y: p.y, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: p.x, y: p.y, button: "left", clickCount: 1 });
  return true;
};
// 经自定义事件驱动 Monaco：替换第 line 行的 [from,to) 为 text（= del + ins 两条 op）
//  必须派发到 [data-mdio-mona]（监听者本身）：CustomEvent 默认不冒泡，派发到祖先元素收不到
const driveEdit = async (line, from, to, text) => ev("(() => { const h = ((document.querySelector('#app-left')||{}).shadowRoot||{querySelector:()=>null}).querySelector('[data-mdio-mona]'); if (!h) return 'no-host'; h.dispatchEvent(new CustomEvent('mdio-sop-edit', { detail: " + JSON.stringify({ line, from, to, text }) + " })); return 'ok'; })()");
// 等页面静稳：日志行数 + 卡片值连续 ~1.2s 不变（外部写的帧/桥同步跑完再动手）。
// 教训：紧跟外部写之后立刻编辑，会出现"删除落在别处、插入照落"的多字现象（S7 专门盯它）。
const settle = async (ms) => {
  let last = "", same = 0;
  for (let i = 0; i < 30; i += 1) {
    await wait(400);
    const sig = String(await logText()).length + "|" + (await cardRec());
    if (sig === last) { same += 1; if (same >= 3) return true; } else { same = 0; last = sig; }
  }
  return false;
};
const recLine = async () => { for (let i = 1; i <= 60; i += 1) { const l = String(await ev("(() => { const m = document.querySelector('[data-page=\"stats\"]'); return m ? '' : ''; })()")); break; } const d = String(await GWX("$ /stats.md")); const lines = d.split(NL); for (let i = 0; i < lines.length; i += 1) { if (lines[i].indexOf("记录:") >= 0) { const c = lines[i].indexOf("记录:") + 4; let e = c; while (e < lines[i].length && lines[i][e] >= "0" && lines[i][e] <= "9") e += 1; return { line: i + 1, from: c + 1, to: e + 1, val: lines[i].slice(c, e) }; } } return null; };

for (let i = 0; i < 45; i += 1) { await wait(900); if (await ev("!!document.querySelector('.dv-ft-file')")) break; }
await wait(2500);
// DOM `.click()` 打不动文件树（现场实测）⇒ 用 CDP 真鼠标点该行。
{
  const __p = JSON.parse(String(await ev("(() => { const e = [...document.querySelectorAll('.dv-ft-file')].find((x) => x.getAttribute('data-path') === '/stats.md'); if (!e) return 'null'; e.scrollIntoView(); const r = e.getBoundingClientRect(); return JSON.stringify({ x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) }); })()")) || null);
  if (__p) { await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: __p.x, y: __p.y }); await send("Input.dispatchMouseEvent", { type: "mousePressed", x: __p.x, y: __p.y, button: "left", clickCount: 1 }); await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: __p.x, y: __p.y, button: "left", clickCount: 1 }); }
}
await wait(6000);

// ── ① 点按钮 ──
const n0 = await cardRec();
// 诊断：先把卡片里的按钮清单打出来 —— S1 全 0 时到底是「按钮选择器不中」还是「点了没联动」。
  { const __d = String(await ev("JSON.stringify({ card: document.querySelectorAll('[data-page=\"stats\"]').length, btns: [...document.querySelectorAll('[data-page=\"stats\"] button')].map((b)=>String(b.textContent||'').slice(0,10)), txt: String((document.querySelector('[data-page=\"stats\"]')||{}).innerText||'').slice(0,60) })")); console.log('   [diag S1] ' + __d.slice(0, 220)); }
await clickIn('[data-page="stats"] button');
let n1 = null; for (let i = 0; i < 12; i += 1) { await wait(900); n1 = await cardRec(); if (n1 && n1 !== n0) break; }
chk("S1 按钮 ⇒ 三方一致 + 零丢帧", "点按钮后 卡片/文档/节点 三者相同且 +1", n1 === (await docRec()) && Number(n1) === Number(n0) + 1 && String(await GWX("$ /stats.md::站点统计.面板._b:记录")).trim() === n1, JSON.stringify({ 前: n0, 后: n1, 文档: await docRec() }));

// ── ② Monaco 替换数字（del+ins）──
await settle();
const p2 = await recLine();
const next2 = String((Number(p2.val) || 0) + 1);
await driveEdit(p2.line, p2.from, p2.to, next2);
let seen2 = null; for (let i = 0; i < 12; i += 1) { await wait(900); seen2 = await cardRec(); if (seen2 === next2) break; }
// 把"驱动那一刻模型自己报的行"带进详情：若它与引擎行不一致 ⇒ 是同步问题；一致而结果错 ⇒ 是引擎侧
const lastSop = (async () => { const lg = String(await logText()); const m = lg.match(/SOP: 驱动编辑[^\n]*/g) || []; return (m.slice(-1)[0] || "").slice(0, 120); })();
const lg2 = String(await logText());
const ops2 = (lg2.match(/OP: (?:ins|del)[^\n]*/g) || []).slice(-3).map((s) => s.replace(/^\[[^\]]+\]\s*/, "").slice(0, 60));
chk("S2 Monaco 替换 ⇒ 三方一致", "Monaco/文档/卡片 都等于新值（替换 = 删+插两条 op 都送达）", seen2 === next2 && (await docRec()) === next2, JSON.stringify({ 期望: next2, 卡片: seen2, 文档: await docRec(), 驱动坐标: p2, 模型自述: await lastSop, 帧: ops2 }));

// ── ③ 连打 12 次 ──
await settle();
const p3 = await recLine();
let want = Number(p3.val) || 0;
// 坐标只在「值是一位数」时成立 —— 值涨到两位数后 p3.from/to 会漂（实测文档变成 "13210"）。
//   ⇒ 每轮都重新取**当轮**的记录坐标（这才是「连打 12 次」想测的东西：每次替换都落地）。
for (let i = 0; i < 12; i += 1) { want += 1; const cur = await recLine(); await driveEdit(cur.line, cur.from, cur.to, String(want)); await wait(120); }
let ok3 = null; for (let i = 0; i < 30; i += 1) { await wait(900); ok3 = await docRec(); if (ok3 === String(want)) break; }
chk("S3 连打 12 次 ⇒ 一次都不许丢", "每次替换都落地（文档 == 期望值）", String(ok3) === String(want), JSON.stringify({ 期望: want, 文档: ok3 }));
await wait(1500);
const lg1 = String(await logText());
chk("S4 零丢帧", "devview 日志里没有「⚠ 丢帧」（上行环背压丢弃）", lg1.indexOf("⚠ 丢帧") < 0, JSON.stringify((lg1.match(/⚠ 丢帧[^\n]*/g) || []).slice(0, 2)));

// ── ④b 交错回归：紧跟一次外部写之后立刻编辑（不许出现多字/丢删）──
{
  await settle();
  const g = String(await GWX("$ /stats.md::站点统计.面板._b:记录"));
  await GWX(JSON.stringify(String(Number(g) + 1)) + " | /> /stats.md::站点统计.面板._b:记录");
  const p4 = await recLine();                       // 不等静稳，立刻读行并驱动
  const nv4 = String(Number(p4.val) + 1);
  await driveEdit(p4.line, p4.from, p4.to, nv4);
  let ok4 = null;
  for (let i = 0; i < 20; i += 1) { await wait(900); ok4 = await docRec(); if (ok4 === nv4) break; }
  chk("S7 交错回归（外部写后立刻编辑）", "替换后文档恰为新值（不多字、不丢删）", String(ok4) === nv4, JSON.stringify({ 外部写后: g, 期望: nv4, 文档: ok4 }));
}

// ── ⑤ 外部写 ⇒ 编辑器跟随（桥）──
const guard = String(await GWX("$ /stats.md::站点统计.面板._b:记录"));
await GWX(JSON.stringify(String(Number(guard) + 1)) + " | /> /stats.md::站点统计.面板._b:记录");
let followed = false; for (let i = 0; i < 14; i += 1) { await wait(900); if ((await monaRec()) === String(Number(guard) + 1)) { followed = true; break; } }
// 诊断：S5 失败时把「Monaco 里到底是什么」打出来（外层探针不可靠：同一页不同次加载 mona 行数会变）。
  { const __m = String(await ev("[...(((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.view-line'))].map((e)=>String(e.innerText||'').trim()).filter(Boolean).slice(0,4).join(' | ')")); console.log('   [diag S5] mona=' + String(__m).slice(0, 160)); }
chk("S5 外部写 ⇒ Monaco 跟随", "exec 写节点后编辑器文本跟上", followed, JSON.stringify({ 目标: Number(guard) + 1, monaco: await monaRec() }));

// ── 还原 ──
const c = await cardRec();
await GWX(JSON.stringify("0") + " | /> /stats.md::站点统计.面板._b:记录");
await wait(2500);
const DOC1 = String(await GWX("$ /stats.md"));
// 比较时容忍首尾空白差异（$ 读回会去掉末尾换行；基线取磁盘源更稳）
const norm = (s) => String(s).replace(/\s+$/, "").replace(/^\s+/, "");
chk("S6 还原 + 文档无损", "整篇内容回到开局（除记录值外未被改动）", norm(DOC1) === norm(DOC0) && (await docRec()) === "0", JSON.stringify({ 长度: DOC0.length + "→" + DOC1.length, 卡片末值: c }));
const shot = await send("Page.captureScreenshot", { format: "png" });
try { fs.mkdirSync(SHOT, { recursive: true }); } catch (e) {}
if (shot && shot.data) fs.writeFileSync(SHOT + "/39-sop.png", Buffer.from(shot.data, "base64"));
try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) {}

for (const r of R) console.log((r.ok ? " ✓ " : " ✗ ") + r.cid + (r.detail ? "  → " + r.detail : ""));
const bad = R.filter((r) => !r.ok).length;
console.log(bad ? ("✗ SOP 失败 " + bad + " 项") : ("✓ SOP 全绿（" + R.length + " 项）"));
process.exitCode = bad ? 1 : 0;

// 收尾退出：老的测试脚本跑完不收 CDP socket ⇒ 不显式退出就会被自己的 deadline 判成卡死。
// 退出码沿用脚本自己设的 process.exitCode（没设 = 0）；失败路径若已 process.exit(1) 则不受影响。
process.exit(process.exitCode === undefined ? 0 : process.exitCode);
