// appview-live.mjs —— **appview 容器件（ui_appview）的元素契约**活体判据（真页面 · 自建无头 9223）。
// 验的是「元素自己那几件事」：影子根与挂载点 · 挂载点归属 · 重复连接不双挂 · 属性变更事件 · 卸载事件与清理。
// 跑法：node mdio_ui/demo/test/appview-live.mjs（可覆盖 CDP / BASE / WT / DOC）
import { installGuards, makeEv, armWatchdog, guardTarget, withCallTimeout } from "./lib/watchdog.mjs";
const CDP = guardTarget(process.env.CDP || "http://127.0.0.1:9223", "appview-live");
installGuards("appview-live");
armWatchdog("appview-live", 10000);   /* = 10s；实测 max=0.8s（本机 2 跑）⇒ ×2=1.6s，取下限 10s */
const BASE = process.env.BASE || "http://127.0.0.1:5178";
const WT = process.env.WT || "czUR8zxu";
const DOC = process.env.DOC || "/scripts/剧本.md";
let pass = 0, total = 0;
const chk = (n, ok, d) => { total += 1; if (ok) pass += 1; console.log((ok ? "  ok   " : "  FAIL ") + n + (d ? "  — " + String(d).slice(0, 240) : "")); };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pageErrs = [];

const tab = await (await fetch(CDP + "/json/new?" + encodeURIComponent(BASE + "/?wt=" + WT + "&app=" + encodeURIComponent(DOC)), { method: "PUT" })).json();
const ws = new WebSocket(tab.webSocketDebuggerUrl);
let seq = 0; const pend = new Map(); const pageUncaught = [];
ws.addEventListener("message", (e) => {
  try { const m = JSON.parse(e.data);
    if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result || {}); pend.delete(m.id); return; }
    if (m.method === "Runtime.exceptionThrown") pageUncaught.push(String((m.params.exceptionDetails.exception || {}).description || "").slice(0, 160));
  } catch (e2) { /* ignore */ }
});
await new Promise((r) => ws.addEventListener("open", r));
const rawSend = (method, params) => new Promise((r) => { const i = ++seq; pend.set(i, r); ws.send(JSON.stringify({ id: i, method, params })); });
const send = withCallTimeout("appview-live", rawSend);
await send("Runtime.enable", {});
const ev = makeEv(send, "appview-live", pageErrs);
for (let i = 0; i < 60; i += 1) { if ((await ev("(() => { const el = document.getElementById('app'); if (!el) return false; const root = el.shadowRoot || el; return !!root.querySelector('.sc-root'); })()")) === true) break; await sleep(600); }
/** 当前是不是影子模式（两种模式都要能跑：判据按模式断言，而不是假定）。 */
const SHADOW = (await ev("!!(document.getElementById('app')||{}).shadowRoot")) === true;

/** 在页面里装事件计数（只装一次） */
await ev("(() => { if (window.__avc) return true; window.__avc = { ready: 0, unmount: 0, change: 0 }; document.addEventListener('mdio:app-ready', () => { window.__avc.ready += 1; }); document.addEventListener('mdio:app-unmount', () => { window.__avc.unmount += 1; }); document.addEventListener('mdio:app-change', () => { window.__avc.change += 1; }); return true; })()");

// ① 元素被升级 + 影子根 + 挂载点
{
  const st = await ev("(() => { const el = document.getElementById('app'); if (!el) return { missing: true }; return { tag: el.tagName, upgraded: typeof el.mountPoint !== 'undefined', hasShadow: !!el.shadowRoot, mountInShadow: !!(el.shadowRoot && el.shadowRoot.getElementById('mount')), isSelf: el.mountPoint === el, scope: el.getAttribute('data-esmap-scope'), contain: el.style.contain || getComputedStyle(el).contain }; })()");
  const s = st && typeof st === "object" ? st : {};
  chk("① `#app` 是 ui_appview 的元素（已升级）", s.tag === "MDIO-APPVIEW" && s.upgraded === true, JSON.stringify(s));
  chk("① 影子模式：影子根 + 影子里的 `#mount` 都在（light 模式：无影子根）", SHADOW ? (s.hasShadow === true && s.mountInShadow === true) : (s.hasShadow === false && s.mountInShadow === false), JSON.stringify({ SHADOW: SHADOW, s: s }));
  chk("① 挂载点：影子模式 = 影子里的 `#mount`；light 模式 = 元素自身", SHADOW ? s.isSelf === false : s.isSelf === true, JSON.stringify({ SHADOW: SHADOW, s: s }));
  chk("① 元素自带作用域属性与布局容器", s.scope === "mdio-app" && String(s.contain).indexOf("layout") >= 0, JSON.stringify(s));
}
// ② app 内容只在影子树里
{
  const st = await ev("(() => { const el = document.getElementById('app'); const sr = el.shadowRoot; return { inShadow: !!(sr && sr.querySelector('.sc-root')), inLight: !!el.querySelector('.sc-root'), inDoc: !!document.querySelector('.sc-root'), sheets: sr ? sr.adoptedStyleSheets.length : document.adoptedStyleSheets.length }; })()");
  const s = st && typeof st === "object" ? st : {};
  chk("② app 内容画在库里（影子内 / light）", SHADOW ? s.inShadow === true : s.inLight === true, JSON.stringify({ SHADOW: SHADOW, s: s }));
  chk("② 硬边界（影子模式：文档层看不到 app 内容；light 模式：本就在文档层）", SHADOW ? s.inDoc === false : s.inDoc === true, JSON.stringify({ SHADOW: SHADOW, s: s }));
  chk("② 样式落点在位（影子根 / document 上的采纳 sheet ≥ 2）", Number(s.sheets) >= 2, JSON.stringify(s));
}
// ③a 干净元素的生命周期（不跟 React 抢 `#app`）：append ⇒ ready；remove ⇒ unmount 且影子根保留复用
{
  const st = await ev("(() => { const rec = { ready: 0, unmount: 0 }; const el = document.createElement('mdio-appview'); el.addEventListener('mdio:app-ready', () => { rec.ready += 1; }); el.addEventListener('mdio:app-unmount', () => { rec.unmount += 1; }); document.body.appendChild(el); const sr0 = el.shadowRoot; const mp0 = sr0 && sr0.getElementById('mount'); const afterAppend = { ready: rec.ready, hasShadow: !!sr0, hasMount: !!mp0 }; el.remove(); const afterRemove = { unmount: rec.unmount, keptShadow: !!el.shadowRoot }; document.body.appendChild(el); const sr1 = el.shadowRoot; const afterReconnect = { ready: rec.ready, sameRoot: sr0 === sr1, sameMount: mp0 === (sr1 && sr1.getElementById('mount')), mounts: sr1 ? sr1.querySelectorAll('#mount').length : -1 }; el.remove(); return { afterAppend: afterAppend, afterRemove: afterRemove, afterReconnect: afterReconnect }; })()");
  const s = st && typeof st === "object" ? st : {};
  chk("③a 新元素挂上 ⇒ 派发 ready（影子模式同时建影子根+挂载点）", s.afterAppend && s.afterAppend.ready >= 1 && (SHADOW ? (s.afterAppend.hasShadow === true && s.afterAppend.hasMount === true) : (s.afterAppend.hasShadow === false)), JSON.stringify({ SHADOW: SHADOW, a: s.afterAppend }));
  chk("③a 移除 ⇒ 派发 unmount（影子模式：影子根不被 null 掉）", s.afterRemove && s.afterRemove.unmount >= 1 && (SHADOW ? s.afterRemove.keptShadow === true : true), JSON.stringify({ SHADOW: SHADOW, r: s.afterRemove }));
  chk("③a 重连 ⇒ 复用同一影子根/挂载点（light 模式：不要造出影子根），且只有一个 `#mount`", SHADOW ? (s.afterReconnect && s.afterReconnect.sameRoot === true && s.afterReconnect.sameMount === true && s.afterReconnect.mounts === 1) : (s.afterReconnect && s.afterReconnect.mounts === -1), JSON.stringify({ SHADOW: SHADOW, c: s.afterReconnect }));
}
// ③b `#app` 的就地重连（React 在管它）：复用 + 事件计数只增不重复挂载
{
  const st = await ev("(() => { const el = document.getElementById('app'); const parent = el.parentNode; const sr0 = el.shadowRoot; const mp0 = sr0 ? sr0.getElementById('mount') : null; const r0 = window.__avc.ready; const u0 = window.__avc.unmount; el.remove(); const u1 = window.__avc.unmount; const kept = !!el.shadowRoot; parent.appendChild(el); const sr1 = el.shadowRoot; return { unmountDelta: u1 - u0, kept: kept, sameRoot: sr0 === sr1, sameMount: mp0 === (sr1 && sr1.getElementById('mount')), mounts: sr1 ? sr1.querySelectorAll('#mount').length : -1, readyDelta: window.__avc.ready - r0 }; })()");
  const s = st && typeof st === "object" ? st : {};
  chk("③b `#app` 断开/重连：不重复建挂载点（影子模式：影子根复用）", SHADOW ? (s.sameRoot === true && s.sameMount === true && s.mounts === 1 && s.kept === true) : true, JSON.stringify({ SHADOW: SHADOW, s: s }));
  chk("③b 重连后 ready 再次派发（壳可重新驱动）", Number(s.readyDelta) >= 1, JSON.stringify(s));
}

// ④ 属性变更 ⇒ `mdio:app-change`（换篇/换 app 的钩子）
{
  const st = await ev("(() => { const el = document.getElementById('app'); const before = window.__avc.change; el.setAttribute('app-doc', '/scripts/剧本M0备份.md'); el.setAttribute('app-doc', '/scripts/剧本.md'); return { before: before, after: window.__avc.change }; })()");
  const s = st && typeof st === "object" ? st : {};
  chk("④ 属性变更派发 `mdio:app-change`（带值）", Number(s.after) - Number(s.before) >= 2, JSON.stringify(s));
}
// ⑤ app 内容在往返之后仍在（没被双挂 / 没被清空）
{
  const st = await ev("(() => { const el = document.getElementById('app'); const sr = el.shadowRoot; return { sc: !!(sr ? sr.querySelector('.sc-root') : el.querySelector('.sc-root')), sheets: sr ? sr.adoptedStyleSheets.length : document.adoptedStyleSheets.length }; })()");
  const s = st && typeof st === "object" ? st : {};
  chk("⑤ 往返后 app 内容与样式都还在", s.sc === true && Number(s.sheets) >= 2, JSON.stringify(s));
}
chk("⑥ 无页面异常（Runtime.evaluate 的 exceptionDetails）", pageErrs.length === 0, pageErrs.slice(0, 2).join(" | "));
chk("⑦ 无未捕获异常（Runtime.exceptionThrown）", pageUncaught.length === 0, pageUncaught.slice(0, 2).join(" | "));
console.log((pass === total ? "✓ appview 元素契约判据全绿（" : "✗ 失败 ") + pass + "/" + total + (pass === total ? "）" : ""));
await fetch(CDP + "/json/close/" + tab.id).catch(() => {});
process.exit(pass === total ? 0 : 1);
