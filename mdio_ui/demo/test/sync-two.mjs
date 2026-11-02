// test/sync-two.mjs —— 只有两种同步，就两条。每条都做**三对比**（Monaco / render / 引擎真值）+ 截屏。
//   T1 外部写（点按钮） ⇒ Monaco 是否更新
//   T2 Monaco 编辑（真实按键：删一位+写一位） ⇒ render 是否更新
// 读取口：等就绪（等不到就以非 0 退出，不静默放行 —— 上一版就是静默放行导致读到 null 还当通过）。
const GW = process.env.GW || "http://127.0.0.1:8011";
const CDP = process.env.CDP || "http://127.0.0.1:9222";
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const WT = process.env.SOP_WT || "c8Page01";
const SHOT = process.env.SHOT_DIR || ".tmp/steps-gui";
const NL = String.fromCharCode(10);
const fs = await import("node:fs");
const gwx = async (code) => { const r = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, active_doc: "/stats.md", wt_uid: WT }) }); const j = await r.json(); return String(j.result == null ? "" : j.result); };
await gwx(JSON.stringify("0") + " | /> /stats.md::站点统计.面板._b:记录");
await new Promise((r) => setTimeout(r, 1500));
const list = await (await fetch(CDP + "/json/list")).json();
for (const t of list) if (t.type === "page") { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) {} }
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=" + WT), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let id = 0; const pend = new Map();
const send = (m, pa) => new Promise((res) => { const i = ++id; pend.set(i, res); try { ws.send(JSON.stringify({ id: i, method: m, params: pa || {} })); } catch (e) { res({}); } });
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); } } catch (e) {} });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
// 注：页面刚打开时求值可能是 undefined（上下文还没准备好）⇒ **不要抛**，交给上层的"等就绪"轮询。
const ev = (e) => send("Runtime.evaluate", { expression: e, returnByValue: true }).then((r) => (r && r.result ? r.result.value : undefined));
const shot = async (n) => { const s = await send("Page.captureScreenshot", { format: "png" }); try { fs.mkdirSync(SHOT, { recursive: true }); } catch (e) {} if (s && s.data) fs.writeFileSync(SHOT + "/" + n, Buffer.from(s.data, "base64")); };
const key = async (k, c) => { await send("Input.dispatchKeyEvent", { type: "rawKeyDown", windowsVirtualKeyCode: c, nativeVirtualKeyCode: c, key: k, code: k }); await send("Input.dispatchKeyEvent", { type: "keyUp", windowsVirtualKeyCode: c, nativeVirtualKeyCode: c, key: k, code: k }); await wait(220); };
const click = async (x, y) => { await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 }); await send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 }); };
// Monaco 的 innerText **带行号栏**（"1 2 3 … 40" 在正文前）⇒ 只认带 "- " 前缀的正文行，避免误匹配。
const num = (s) => { const m = String(s).match(/记录[:\s]*(\d+)/); return m ? m[1] : null; };
const numMona = (s) => { const m = String(s).match(/- 记录:\s*(\d+)/); return m ? m[1] : null; };
const mona = async () => numMona(await ev("String(((document.querySelector('#app-left')||{}).shadowRoot||{}).textContent||'')"));
const render = async () => num(await ev("String((document.querySelector('[data-page=\"stats\"]')||{}).innerText||'')"));
const engine = async () => (String(await gwx("$ /stats.md")).match(/- 记录: (\d+)/) || [null, null])[1];

// ── 等就绪：Monaco 与 render **都必须**能读出数字，否则直接失败（不静默放行）──
for (let i = 0; i < 50; i += 1) { await wait(900); if ((await ev("!!document.querySelector('.dv-ft-file')")) === true) break; }
await wait(2500);
// 点开 /stats.md：**反复点**直到面板确实切过去（footer 会显示当前文档路径）—— 只点一次会偶发落空
let opened = false;
for (let i = 0; i < 8; i += 1) {
  await ev("([...document.querySelectorAll('.dv-ft-file')].find((e) => e.getAttribute('data-path') === '/stats.md'))?.click()");
  await wait(3000);
  const bar = String(await ev("String((document.querySelector('.dv-footer-bar')||{}).innerText||'')"));
  if (bar.indexOf("/stats.md") >= 0) { opened = true; break; }
}
let m0 = null, r0 = null;
for (let i = 0; i < 45; i += 1) { await wait(1000); m0 = await mona(); r0 = await render(); if (m0 !== null && r0 !== null) break; }
if (m0 === null || r0 === null) {
  // 失败时把**现场**打出来（免得又是"未就绪"三个字）
  const diag = await ev("JSON.stringify({ open: " + JSON.stringify(opened) + ", footer: String((document.querySelector('.dv-footer-bar')||{}).innerText||'').slice(0,80), monaLen: String(((document.querySelector('#app-left')||{}).shadowRoot||{}).textContent||'').length, card: !!document.querySelector('[data-page=\"stats\"]'), bodyLen: String((document.body||{}).innerText||'').length })");
  console.log("✗ 未就绪：Monaco=" + m0 + " render=" + r0 + " · 现场=" + String(diag).slice(0, 260));
  process.exit(2);
}
console.log("起手：Monaco=" + m0 + " render=" + r0 + " 引擎=" + (await engine()));
let bad = 0;

// ── T1：外部写（按钮）⇒ Monaco 是否更新 ──
const bt = JSON.parse(String(await ev("JSON.stringify((() => { const el = document.querySelector('[data-page=\"stats\"] button'); if (!el) return null; const r = el.getBoundingClientRect(); return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }; })())")));
if (bt) await click(bt.x, bt.y);
let m1 = null, r1 = null, e1 = null;
for (let i = 0; i < 20; i += 1) { await wait(900); m1 = await mona(); r1 = await render(); e1 = await engine(); if (r1 !== r0 && m1 === e1) break; }
await shot("60-T1-button.png");
console.log("【T1 按钮】Monaco=" + m1 + " · render=" + r1 + " · 引擎=" + e1);
console.log("   T1 判定：" + ((m1 === e1) ? "✓ Monaco 跟上了" : "✗ **Monaco 没跟上**（render=" + r1 + " 引擎=" + e1 + " Monaco=" + m1 + "）"));
if (m1 !== e1) bad += 1;

// ── T2：Monaco 编辑（真实按键）⇒ render 是否更新 ──
const before = await render();
const target = String(Number(before) + 1);
const box = JSON.parse(String(await ev("JSON.stringify((() => { const ls = [...(((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.view-line'))]; const el = ls.find((e) => (e.innerText||'').indexOf('记录') >= 0); if (!el) return null; const r = el.getBoundingClientRect(); return { x: Math.round(r.x + 60), y: Math.round(r.y + r.height / 2) }; })())")));
if (box) { await click(box.x, box.y); await wait(450); await key("End", 35); if (String(before).length > 0) await key("Backspace", 8); await send("Input.insertText", { text: target }); }
let m2 = null, r2 = null, e2 = null;
for (let i = 0; i < 20; i += 1) { await wait(900); m2 = await mona(); r2 = await render(); e2 = await engine(); if (r2 === target && e2 === target) break; }
await shot("61-T2-monaco.png");
console.log("【T2 编辑】Monaco=" + m2 + " · render=" + r2 + " · 引擎=" + e2 + "（目标 " + target + "）");
console.log("   T2 判定：" + ((r2 === target && e2 === target) ? "✓ render 跟上了" : "✗ **render 没跟上**（目标 " + target + "，render=" + r2 + " 引擎=" + e2 + "）"));
if (!(r2 === target && e2 === target)) bad += 1;

try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) {}
console.log(bad ? ("✗ 两种同步里有 " + bad + " 条不通") : "✓ 两种同步都通");
process.exitCode = bad ? 1 : 0;
