// CDP 判据：**J2 cursor 差分**（把 `__MDIO_CURSOR_DIFF__` 升成真判据，覆盖**切档往返**）
//
// 背景：`ui_core/src/render/NodeView.jsx:137-168` 在 dev 期逐节点比对「cursor 判定」与「现行 A2」，
// 不一致就 `console.error("[cursor] 不一致", …)` 并把计数挂到 `globalThis.__MDIO_CURSOR_DIFF__`。
// 本脚本把它变成判据：**切档往返（A→B→A）全程计数必须保持 0**。
//
// 纪律（UI_TEST_SOP §Step 2）：自建带 `#probe=cursor` 锚的页；**只关自己的**页；条件轮询，不写死 sleep。
const CDP = process.env.CDP || "http://127.0.0.1:9222";   // 与兄弟脚本一致：可用环境变量指向专用无头实例
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const ANCHOR = "#probe=cursor";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let id = 0; const pending = new Map();
async function send(method, params) { return new Promise((res, rej) => { const i = ++id; pending.set(i, { res, rej }); ws.send(JSON.stringify({ id: i, method, params })); }); }
async function evalJs(expression) {
  const r = await send("Runtime.evaluate", { expression, returnByValue: true, userGesture: true, awaitPromise: true });
  if (r.exceptionDetails) throw new Error("eval: " + JSON.stringify(r.exceptionDetails).slice(0, 200));
  return r.result.value;
}
const closeMine = async () => {
  const list = await (await fetch(CDP + "/json/list")).json();
  for (const t of list) if (t.type === "page" && String(t.url).includes(ANCHOR)) { try { await fetch(CDP + "/json/close/" + t.id); } catch (e) { /* 忽略 */ } }
};
await closeMine();   // 只清自己上一轮可能留下的
const page = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=genesis" + ANCHOR), { method: "PUT" })).json();
const ws = new WebSocket(page.webSocketDebuggerUrl);
const errs = [];
ws.addEventListener("message", (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id) { const p = pending.get(m.id); if (p) { pending.delete(m.id); m.error ? p.rej(new Error(JSON.stringify(m.error))) : p.res(m.result); } return; }
  if (m.method === "Runtime.consoleAPICalled" && m.params.type === "error") errs.push((m.params.args || []).map((a) => String(a.value ?? a.description ?? "")).join(" ").slice(0, 160));
});
await new Promise((r) => ws.addEventListener("open", r));
await send("Runtime.enable"); await send("Page.enable");

/** 条件轮询：等到表达式为真（或超时）——不写死 sleep。 */
async function waitFor(expr, ms = 15000) {
  const t0 = Date.now();
  for (;;) {
    let v = null; try { v = await evalJs(expr); } catch (e) { v = null; }
    if (v) return true;
    if (Date.now() - t0 > ms) return false;
    await sleep(200);
  }
}
const rows = [];
const step = (name, ok, detail) => { rows.push({ name, ok }); console.log("  " + (ok ? "✓" : "✗") + " " + name + (detail ? "  → " + detail : "")); };

const okShell = await waitFor("!!document.querySelector('.dv-wtbtn')", 20000);
step("devview 壳就绪", okShell);
if (!okShell) { console.log("✗ cdp-cursor：壳没起来，无法继续"); await closeMine(); process.exit(1); }

// 文件树里的两个不同档（切档往返用它俩）
// 用 data-path 作为档身份（行文本含扩展名徽章 "MDxxx.md"，老写法把它当身份 ⇒ 与 on 行的 data-path 对不上）。
const picks = await evalJs("(function(){var rows=[].slice.call((((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.dv-ft-row.dv-ft-file')));var seen={},out=[];for(var i=0;i<rows.length;i++){var t=rows[i].getAttribute('data-path');if(t&&!seen[t]){seen[t]=1;out.push(t);}if(out.length>=2)break;}return JSON.stringify(out);})()");
const docs = JSON.parse(picks || "[]");
step("文件树里找到两个不同的档", docs.length >= 2, docs.join(" / "));
if (docs.length < 2) { console.log("✗ cdp-cursor：档不足两篇，无法做切档往返"); await closeMine(); process.exit(1); }

const openDoc = async (name) => {
  const clicked = await evalJs("(function(){var rows=[].slice.call((((document.querySelector('#app-left')||{}).shadowRoot||{querySelectorAll:()=>[]}).querySelectorAll('.dv-ft-row.dv-ft-file')));var el=rows.filter(function(r){return r.getAttribute('data-path')===" + JSON.stringify(name) + ";})[0];if(!el)return false;el.click();return true;})()");
  if (!clicked) return false;
  const painted = await waitFor("!!(((document.querySelector('#app')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.dv-doc-inner'))", 12000);
  // 再等「树里带 on 的那一行」真的变成目标档：避免取到上一档/半渲染状态（旧写法读正文首 40 字，恰好一直是壳文本 ⇒ 假绿）
  const landed = painted && await waitFor("(() => { const sr = (document.querySelector('#app-left')||{}).shadowRoot; const on = sr && sr.querySelector('.dv-ft-row.on'); return !!on && on.getAttribute('data-path') === " + JSON.stringify(name) + "; })()", 10000);
  return landed;
};
const textOf = async () => String(await evalJs("(((((document.querySelector('#app')||{}).shadowRoot||{querySelector:()=>null}).querySelector('.dv-doc-inner'))||{}).innerText||'').replace(/\\s+/g,' ').slice(-240)") || "");
// 判据口径：用「左格文件树里带 on 的 data-path」当当前档标识 —— 比文档正文片段稳定（正文在切档瞬间可能还没画完）。
const curPathOf = async () => String(await evalJs("(() => { const sr = (document.querySelector('#app-left')||{}).shadowRoot; const on = sr && sr.querySelector('.dv-ft-row.on'); return on ? (on.getAttribute('data-path') || '') : ''; })()") || "");
const diffOf = async () => Number(await evalJs("globalThis.__MDIO_CURSOR_DIFF__ || 0") || 0);

// **切档往返 A → B → A**：每一步都要求文档面出现，且 cursor 差分全程为 0
const seq = [docs[0], docs[1], docs[0]];
const seen = [];
const diffs = [];
for (let i = 0; i < seq.length; i++) {
  const okOpen = await openDoc(seq[i]);
  const path = okOpen ? await curPathOf() : "";
  const d = await diffOf();
  seen.push(path); diffs.push(d);
  step("切档 " + (i + 1) + "/3 → " + seq[i] + " 渲染并就位", okOpen && path === seq[i], "差分=" + d + " · " + (path || "(无 on 行)"));
}
const distinct = new Set(seen.filter((s) => s)).size;
step("A→B 真的换了内容（不是同一篇）", distinct >= 2, "不同档数=" + distinct + " · " + seen.join(" → "));
step("回到 A 时内容复原（往返闭合）", seen[0] === seen[2] && seen[0].length > 0, (seen[0] || "") + " ↔ " + (seen[2] || ""));
step("cursor 差分全程为 0（判据本体）", diffs.every((d) => d === 0), JSON.stringify(diffs));
const cursorErrs = errs.filter((e) => e.indexOf("[cursor] 不一致") >= 0);
step("控制台没有 [cursor] 不一致", cursorErrs.length === 0, cursorErrs.slice(0, 1).join(""));

await closeMine();
const bad = rows.filter((r) => !r.ok);
if (bad.length) { console.log("✗ cdp-cursor 失败 " + bad.length + " 项：" + bad.map((b) => b.name).join(" · ")); process.exit(1); }
console.log("✓ cdp-cursor 全绿：切档往返 A→B→A 全程 cursor 差分为 0（" + seq.join(" → ") + "）");