// test/ime.mjs —— 中文 IME（组合输入）回归：**组合期间引擎丢弃 LocalInsert/Delete，只有 ImeCommit 生效**，
// 所以 commit 的坐标必须是"组合开始处"（1-based）；用 compositionend 的当前光标且多减 1 会错位 ⇒
// 实测：在 "## S1B" 行末输入「阿斯顿」，字被插到上一行（空行 2），渲染成 AP2 / 空 / 阿斯顿 / 空 / S1B。
// 用例：A) 行末追加；B) 组合替换选中文本（引擎 rope 里旧文本仍在 ⇒ 需补一条删除）。
const CDP = process.env.CDP || "http://127.0.0.1:9222";
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const GW = process.env.GW || "http://127.0.0.1:8011";
const DOC = "/sop_probe.md";
const SEED = "# AP2\n\n## S1B\n";

const gwExec = async (code) => {
  const r = await fetch(GW + "/exec", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, active_doc: "", wt_uid: "genesis" }) });
  const j = await r.json();
  return String(j.result == null ? "" : j.result);
};

const list = await (await fetch(CDP + "/json/list")).json();
for (const t of list) { if (t.type === "page") { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) {} } }
const pg = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=genesis"), { method: "PUT" })).json();
const ws = new WebSocket(pg.webSocketDebuggerUrl);
let id = 0; const pend = new Map();
const send = (m, pa) => new Promise((res) => { const i = ++id; pend.set(i, res); try { ws.send(JSON.stringify({ id: i, method: m, params: pa })); } catch (e) { res({}); } });
ws.addEventListener("message", (ev) => { try { const m = JSON.parse(ev.data); if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); } } catch (e) { /* ignore */ } });
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const ev = (e) => send("Runtime.evaluate", { expression: e, returnByValue: true }).then((r) => (r.result && r.result.value));
const R = [];
const chk = (cid, name, ok, detail) => R.push({ cid, name, ok: !!ok, detail: detail === undefined ? "" : String(detail) });

for (let i = 0; i < 45; i++) { await wait(900); if (await ev("!!document.querySelector('.dv-ft-file')")) break; }
await wait(2500);

const openScratch = async () => {
  await ev("([...document.querySelectorAll('.dv-ft-file')].find((e) => e.getAttribute('data-path') === '" + DOC + "'))?.click()");
  await wait(4000);
};

const caretLine = async (needle) => {
  const idx = Number(await ev("(() => { const ls = [...(((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.view-line'))]; return ls.findIndex((x) => x.innerText.indexOf('" + needle + "') >= 0); })()"));
  if (!(idx >= 0)) return null;
  const box = JSON.parse(String(await ev("(() => { const l = [...(((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.view-line'))][" + idx + "]; if (!l) return 'null'; const r = l.getBoundingClientRect(); return JSON.stringify({ x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }); })()")) || "null");
  return box;
};

const compose = async (text) => {
  await send("Input.imeSetComposition", { text, selectionStart: text.length, selectionEnd: text.length });
  await wait(250);
  await send("Input.imeSetComposition", { text, selectionStart: text.length, selectionEnd: text.length });
  await wait(250);
  await send("Input.insertText", { text });
  await wait(2500);
};

const state = async () => ({
  doc: (await gwExec("$ " + DOC)).trim(),
  mona: String(await ev("(((document.querySelector('#app-left')||{}).shadowRoot||{}).textContent)")).replace(/^[0-9\n]+/, "").trim(),
  render: String(await ev("(document.querySelector('.dv-app .mdio-doc') || {}).innerText")).replace(/\n+/g, "|").trim(),
});

// ── A：在 "## S1B" 行末追加 ──
await gwExec(JSON.stringify(SEED) + " | /> " + DOC);
await openScratch();
const boxA = await caretLine("S1B");
if (boxA) {
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: boxA.x, y: boxA.y, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: boxA.x, y: boxA.y, button: "left", clickCount: 1 });
  await wait(250);
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: "End", code: "End", windowsVirtualKeyCode: 35 });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: "End", code: "End", windowsVirtualKeyCode: 35 });
  await wait(200);
  await compose("阿斯顿");
}
const A = await state();
chk("I1", "A 行末组合：引擎文档 = 组合开始处插入", A.doc === "# AP2\n\n## S1B阿斯顿", JSON.stringify(A.doc));
chk("I2", "A 渲染面同步（标题带字，不再掉到上一行）", A.render.indexOf("S1B阿斯顿") >= 0 && A.render.indexOf("阿斯顿|") < 0, A.render);

// ── B：组合替换选中文本（S1B → 阿斯顿）──
await gwExec(JSON.stringify(SEED) + " | /> " + DOC);
await wait(1200);
const boxB = await caretLine("S1B");
if (boxB) {
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: boxB.x, y: boxB.y, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: boxB.x, y: boxB.y, button: "left", clickCount: 1 });
  await wait(250);
  // 行末 → 向左选 3 个字符（"S1B"）
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: "End", code: "End", windowsVirtualKeyCode: 35 });
  await send("Input.dispatchKeyEvent", { type: "keyUp", key: "End", code: "End", windowsVirtualKeyCode: 35 });
  await wait(150);
  for (let k = 0; k < 3; k += 1) {
    await send("Input.dispatchKeyEvent", { type: "keyDown", key: "ArrowLeft", code: "ArrowLeft", windowsVirtualKeyCode: 37, modifiers: 8 });   // 8 = Shift
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: "ArrowLeft", code: "ArrowLeft", windowsVirtualKeyCode: 37, modifiers: 8 });
  }
  await wait(250);
  const selText = String(await ev("(window.getSelection && window.getSelection().toString()) || \"\""));
  console.log("SEL " + JSON.stringify(selText));
  await compose("阿斯顿");
}
const B = await state();
chk("I3", "B 组合替换选区：引擎文档只剩组合文本", B.doc.indexOf("阿斯顿") >= 0 && B.doc.indexOf("S1B") < 0, JSON.stringify(B.doc));

const failed = R.filter((r) => !r.ok);
for (const r of R) console.log((r.ok ? " ✓ " : " ✗ ") + r.cid + " " + r.name + (r.ok ? "" : "  → " + r.detail));
console.log(failed.length ? "✗ ime 失败 " + failed.length + " 项" : "✓ ime 全绿（" + R.length + " 项）");
try { await fetch(CDP + "/json/close/" + pg.id); } catch (e) { /* ignore */ }
process.exit(failed.length ? 1 : 0);